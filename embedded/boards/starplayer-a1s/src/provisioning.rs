//! The captive portal: the boot personality that runs when the device has no stored
//! network, or when the owner asks for one at boot.
//!
//! M8 master-plan decision 5 — credentials come from a portal, never from build-time
//! environment variables — and this is it. The shape is
//! `../ampkeeper/esp32/firmware/src/provisioning.rs`'s, with everything StarPlayer does
//! not need taken out: there is no authentication, no presence gate, no recovery
//! personality, no static-IP form and no verify-by-reboot. The device is a music player
//! on someone's desk, and the threat model is "a phone in the same room".
//!
//! # What happens, in order
//!
//! 1. **Scan first, in station mode.** A scan takes the single radio off channel, and
//!    doing that after the SoftAP is up drops every phone that has joined it. So the
//!    network list is collected before the AP exists, and the portal serves a list rather
//!    than asking the owner to type an SSID.
//! 2. **Open SoftAP `StarPlayer-XXXX`**, the suffix being the last two bytes of the
//!    interface MAC so two boards on one desk are distinguishable.
//! 3. **A hand-rolled DHCP server** on 192.168.4.1/24 and **a DNS catch-all** that
//!    answers every query with that address, which is what makes a phone's own
//!    captive-portal detection open the page by itself.
//! 4. **The portal** — `GET /` with the network list, `POST /save`, then a soft reset
//!    two seconds later so the confirmation page reaches the phone before the radio goes.
//!
//! The audio keeps playing throughout. The portal is not a silent mode: the compiled-in
//! module plays from the moment the codec is up, in this personality exactly as in the
//! other, so the owner can hear that the board is alive while typing a passphrase.
//!
//! # Why the DHCP and DNS servers are written out by hand
//!
//! `edge-dhcp` and `edge-captive` exist and are by the same author as the `edge-mdns`
//! this firmware does use. They were re-checked for this task (research point 3) and not
//! adopted: both are built around `edge-nal`'s `UdpBind`/`UdpReceive` traits over a
//! *bound* socket, and the captive case needs a socket that accepts broadcast traffic
//! addressed to 255.255.255.255 from a client that has no address yet. smoltcp accepts
//! that on a port-only bind — its `accepts()` skips the address check when the local
//! endpoint address is unspecified — which `embassy_net::udp::UdpSocket::bind(port)`
//! gives directly and `edge-nal-embassy`'s `UdpBind` wraps in a `SocketAddr` that has to
//! name one. ampkeeper reached the same conclusion from the other direction and shipped
//! the hand-rolled pair; two DHCP options and one DNS answer record are a small enough
//! surface to own.
//!
//! The DNS parser is the one piece of this firmware that reads bytes a stranger sent over
//! an open network, and it is written accordingly: every offset is bounds-checked against
//! the slice, the answer record's own space is reserved before anything is written, and a
//! query that does not parse is dropped rather than answered.

use alloc::string::String;
use embassy_executor::Spawner;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{Config as NetConfig, Ipv4Cidr, Runner, StackResources, StaticConfigV4};
use embassy_net::{IpAddress, IpEndpoint, Stack};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use esp_println::println;
use esp_radio::wifi::ap::AccessPointConfig;
use esp_radio::wifi::scan::ScanConfig;
use esp_radio::wifi::{Config as RadioConfig, ControllerConfig, Interface};
use picoserve::request::{Path, Request};
use picoserve::ResponseSent;
use picoserve::response::{IntoResponse, Redirect, ResponseWriter, StatusCode};
use picoserve::routing::PathRouterService;
use static_cell::StaticCell;

use crate::store::{PASSPHRASE_MAX_BYTES, SSID_MAX_BYTES, WifiCredentials};

/// The portal's own address, and the only address its DNS server ever answers with.
const AP_ADDRESS: core::net::Ipv4Addr = core::net::Ipv4Addr::new(192, 168, 4, 1);
/// The first and last host octets the DHCP server hands out.
const POOL_FIRST: u8 = 2;
const POOL_LAST: u8 = 14;
/// How long a lease lasts. Two hours: long enough that nothing renews during a
/// provisioning session, short enough to be polite.
const LEASE_SECONDS: u32 = 7200;

const DHCP_SERVER_PORT: u16 = 67;
const DHCP_CLIENT_PORT: u16 = 68;
const DNS_PORT: u16 = 53;

/// A BOOTP packet's fixed header, up to and including the magic cookie.
const DHCP_HEADER_BYTES: usize = 240;
const DHCP_MAGIC: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
/// The BOOTP minimum a reply is padded to.
const DHCP_MIN_REPLY: usize = 300;

/// How many networks the portal lists.
const MAX_NETWORKS: usize = 12;

/// One worker. The portal serves one phone at a time and the worker's future is `.bss`
/// whether this personality runs or not — see `web.rs`'s `WEB_TASK_POOL_SIZE`.
const PORTAL_POOL_SIZE: usize = 1;
const PORTAL_TCP_BUFFER_BYTES: usize = 1024;
const PORTAL_HTTP_BUFFER_BYTES: usize = 1024;

/// smoltcp sockets: DHCP, DNS and the portal worker, plus spare.
const PORTAL_SOCKETS: usize = 2 + PORTAL_POOL_SIZE + 2;

/// The networks the pre-AP scan found, for the portal's list.
static NETWORKS: Mutex<CriticalSectionRawMutex, heapless::Vec<heapless::String<SSID_MAX_BYTES>, MAX_NETWORKS>> =
    Mutex::new(heapless::Vec::new());

/// The SoftAP's SSID, `StarPlayer-XXXX` from the interface MAC.
pub fn access_point_ssid() -> heapless::String<SSID_MAX_BYTES> {
    let mac = esp_hal::efuse::interface_mac_address(esp_hal::efuse::InterfaceMacAddress::AccessPoint);
    let bytes = mac.as_bytes();
    let mut ssid = heapless::String::new();
    let _ = core::fmt::Write::write_fmt(&mut ssid, format_args!("StarPlayer-{:02X}{:02X}", bytes[4], bytes[5]));
    ssid
}

/// Run the portal personality. Never returns: the only way out is the soft reset
/// `POST /save` arms.
pub async fn run(spawner: Spawner, wifi: esp_hal::peripherals::WIFI<'static>, seed: u64) -> Result<(), &'static str> {
    let (mut controller, interfaces) =
        esp_radio::wifi::new(wifi, ControllerConfig::default()).map_err(|_| "the WiFi controller would not start")?;

    // Station mode, and scan **before** the AP exists: see the module documentation.
    match controller.scan_async(&ScanConfig::default().with_max(MAX_NETWORKS)).await {
        Ok(found) => {
            let mut networks = NETWORKS.lock().await;
            for access_point in found.iter() {
                if access_point.ssid.is_empty() {
                    continue;
                }
                let name = heapless::String::try_from(access_point.ssid.as_str()).unwrap_or_default();
                if !networks.contains(&name) {
                    let _ = networks.push(name);
                }
            }
            println!("PORTAL scan found {} networks", networks.len());
        }
        Err(error) => println!("PORTAL scan failed ({error:?}) — the portal will ask for a typed SSID"),
    }

    let ssid = access_point_ssid();
    controller
        .set_config(&RadioConfig::AccessPoint(AccessPointConfig::default().with_ssid(ssid.as_str())))
        .map_err(|_| "the SoftAP would not start")?;

    let config = NetConfig::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(AP_ADDRESS, 24),
        gateway: None,
        // `Default` rather than a named `heapless::Vec`: embassy-net 0.9 is built on
        // heapless 0.9 while picoserve 0.18 — and therefore this crate's own
        // `heapless` — is on 0.8, and the two `Vec` types are not the same type.
        dns_servers: Default::default(),
    });
    static RESOURCES: StaticCell<StackResources<PORTAL_SOCKETS>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(interfaces.access_point, config, RESOURCES.init(StackResources::new()), seed);

    spawner.spawn(access_point_net_task(runner).map_err(|_| "the portal network task would not spawn")?);
    spawner.spawn(dhcp_server_task(stack).map_err(|_| "the DHCP server would not spawn")?);
    spawner.spawn(dns_catchall_task(stack).map_err(|_| "the DNS catch-all would not spawn")?);

    static CONFIG: StaticCell<picoserve::Config> = StaticCell::new();
    let portal_config: &'static picoserve::Config = CONFIG.init(picoserve::Config::new(picoserve::Timeouts {
        start_read_request: Duration::from_secs(3),
        persistent_start_read_request: Duration::from_secs(1),
        read_request: Duration::from_secs(3),
        write: Duration::from_secs(3),
    }));
    for id in 0..PORTAL_POOL_SIZE {
        spawner.spawn(portal_task(id, stack, portal_config).map_err(|_| "the portal worker would not spawn")?);
    }

    println!("PORTAL open network \"{ssid}\" — join it and browse to http://{AP_ADDRESS}/");
    Ok(())
}

/// embassy-net's runner for the SoftAP interface.
#[embassy_executor::task]
async fn access_point_net_task(mut runner: Runner<'static, Interface<'static>>) -> ! { runner.run().await }

// ---------------------------------------------------------------------------
// DHCP
// ---------------------------------------------------------------------------

/// Which host octet each client MAC was given, so a renewing client keeps its address.
struct Leases {
    entries: heapless::Vec<([u8; 6], u8), 16>,
    next: u8,
}

impl Leases {
    const fn new() -> Leases { Leases { entries: heapless::Vec::new(), next: POOL_FIRST } }

    /// The octet for `mac`, allocating one if this is a client we have not seen.
    fn address_for(&mut self, mac: [u8; 6]) -> u8 {
        if let Some((_, octet)) = self.entries.iter().find(|(known, _)| *known == mac) {
            return *octet;
        }
        let octet = self.next;
        self.next = if self.next >= POOL_LAST { POOL_FIRST } else { self.next + 1 };
        // A full table means a very busy desk; recycling the oldest entry is the right
        // answer and `heapless::Vec` has no queue, so the first is evicted by hand.
        if self.entries.is_full() {
            self.entries.remove(0);
        }
        let _ = self.entries.push((mac, octet));
        octet
    }
}

/// Answer DHCP DISCOVER with an OFFER and REQUEST with an ACK, and nothing else.
#[embassy_executor::task]
async fn dhcp_server_task(stack: Stack<'static>) -> ! {
    let mut receive_metadata = [PacketMetadata::EMPTY; 4];
    let mut receive_buffer = [0u8; 1024];
    let mut transmit_metadata = [PacketMetadata::EMPTY; 4];
    let mut transmit_buffer = [0u8; 1024];
    let mut socket = UdpSocket::new(stack, &mut receive_metadata, &mut receive_buffer, &mut transmit_metadata, &mut transmit_buffer);
    if socket.bind(DHCP_SERVER_PORT).is_err() {
        println!("PORTAL could not bind the DHCP port — resetting");
        Timer::after(Duration::from_millis(250)).await;
        esp_hal::system::software_reset();
    }

    let mut leases = Leases::new();
    let mut request = [0u8; 1024];
    let mut reply = [0u8; 512];
    loop {
        let Ok((length, _from)) = socket.recv_from(&mut request).await else { continue };
        let Some(reply_length) = build_dhcp_reply(&request[..length], &mut reply, &mut leases) else { continue };
        // The client has no address yet, so the answer is broadcast whatever the request
        // said.
        let destination = IpEndpoint::new(IpAddress::Ipv4(core::net::Ipv4Addr::BROADCAST), DHCP_CLIENT_PORT);
        let _ = socket.send_to(&reply[..reply_length], destination).await;
    }
}

/// Build the OFFER or ACK for `request`, or `None` if it is not one this server answers.
fn build_dhcp_reply(request: &[u8], reply: &mut [u8; 512], leases: &mut Leases) -> Option<usize> {
    if request.len() < DHCP_HEADER_BYTES || request[0] != 1 || request[236..240] != DHCP_MAGIC {
        return None;
    }
    let message_type = *dhcp_option(request, 53)?.first()?;
    let reply_type = match message_type {
        1 => 2, // DISCOVER → OFFER
        3 => 5, // REQUEST → ACK
        _ => return None,
    };

    let mut client_mac = [0u8; 6];
    client_mac.copy_from_slice(&request[28..34]);
    let assigned = core::net::Ipv4Addr::new(192, 168, 4, leases.address_for(client_mac));

    reply.fill(0);
    reply[0] = 2; // BOOTREPLY
    reply[1] = 1; // Ethernet
    reply[2] = 6; // MAC length
    reply[4..8].copy_from_slice(&request[4..8]); // transaction id
    reply[10..12].copy_from_slice(&request[10..12]); // flags, broadcast bit preserved
    reply[16..20].copy_from_slice(&assigned.octets());
    reply[20..24].copy_from_slice(&AP_ADDRESS.octets());
    reply[28..34].copy_from_slice(&client_mac);
    reply[236..240].copy_from_slice(&DHCP_MAGIC);

    let mut at = DHCP_HEADER_BYTES;
    let mut put = |bytes: &[u8], at: &mut usize| {
        reply[*at..*at + bytes.len()].copy_from_slice(bytes);
        *at += bytes.len();
    };
    put(&[53, 1, reply_type], &mut at);
    put(&[54, 4], &mut at);
    put(&AP_ADDRESS.octets(), &mut at);
    put(&[51, 4], &mut at);
    put(&LEASE_SECONDS.to_be_bytes(), &mut at);
    put(&[1, 4, 255, 255, 255, 0], &mut at);
    put(&[3, 4], &mut at);
    put(&AP_ADDRESS.octets(), &mut at);
    // The DNS server is this device, which is what sends every phone's captive-portal
    // probe to the portal page.
    put(&[6, 4], &mut at);
    put(&AP_ADDRESS.octets(), &mut at);
    put(&[255], &mut at);
    Some(at.max(DHCP_MIN_REPLY))
}

/// The value of DHCP option `wanted`, walking the option area from offset 240.
fn dhcp_option(packet: &[u8], wanted: u8) -> Option<&[u8]> {
    let mut at = DHCP_HEADER_BYTES;
    while at < packet.len() {
        let code = packet[at];
        if code == 255 {
            return None;
        }
        if code == 0 {
            at += 1;
            continue;
        }
        let length = usize::from(*packet.get(at + 1)?);
        let value = packet.get(at + 2..at + 2 + length)?;
        if code == wanted {
            return Some(value);
        }
        at += 2 + length;
    }
    None
}

// ---------------------------------------------------------------------------
// DNS
// ---------------------------------------------------------------------------

/// Answer every A query with the portal's own address.
#[embassy_executor::task]
async fn dns_catchall_task(stack: Stack<'static>) -> ! {
    let mut receive_metadata = [PacketMetadata::EMPTY; 4];
    let mut receive_buffer = [0u8; 768];
    let mut transmit_metadata = [PacketMetadata::EMPTY; 4];
    let mut transmit_buffer = [0u8; 768];
    let mut socket = UdpSocket::new(stack, &mut receive_metadata, &mut receive_buffer, &mut transmit_metadata, &mut transmit_buffer);
    if socket.bind(DNS_PORT).is_err() {
        println!("PORTAL could not bind the DNS port — resetting");
        Timer::after(Duration::from_millis(250)).await;
        esp_hal::system::software_reset();
    }

    let mut query = [0u8; 512];
    let mut reply = [0u8; 512];
    loop {
        let Ok((length, from)) = socket.recv_from(&mut query).await else { continue };
        let Some(reply_length) = build_dns_reply(&query[..length], &mut reply) else { continue };
        let _ = socket.send_to(&reply[..reply_length], from.endpoint).await;
    }
}

/// Build a one-answer reply pointing at [`AP_ADDRESS`].
///
/// Every offset below is checked against the slice it indexes, and the sixteen bytes the
/// answer record needs are reserved before the question is copied — this is the one
/// parser in the firmware that reads bytes a stranger put on an open network.
fn build_dns_reply(query: &[u8], reply: &mut [u8; 512]) -> Option<usize> {
    const HEADER: usize = 12;
    const ANSWER_BYTES: usize = 16;

    if query.len() < HEADER {
        return None;
    }
    // One question, and a query rather than a response.
    if query[2] & 0x80 != 0 || u16::from_be_bytes([query[4], query[5]]) != 1 {
        return None;
    }

    // Walk the QNAME to find where the question ends. Compression pointers are not legal
    // in a question and are refused rather than followed.
    let mut at = HEADER;
    loop {
        let length = usize::from(*query.get(at)?);
        if length & 0xC0 != 0 {
            return None;
        }
        at += 1;
        if length == 0 {
            break;
        }
        at = at.checked_add(length)?;
        if at > query.len() {
            return None;
        }
    }
    // QTYPE and QCLASS.
    let question_end = at.checked_add(4)?;
    if question_end > query.len() {
        return None;
    }
    if question_end + ANSWER_BYTES > reply.len() {
        return None;
    }

    reply[..question_end].copy_from_slice(&query[..question_end]);
    reply[2] = 0x84; // response, authoritative
    reply[3] = 0x00; // no error, recursion not available
    reply[6..8].copy_from_slice(&1u16.to_be_bytes()); // one answer
    reply[8..12].fill(0); // no authority or additional records

    let answer = &mut reply[question_end..question_end + ANSWER_BYTES];
    answer[0..2].copy_from_slice(&0xC00Cu16.to_be_bytes()); // pointer to the question's name
    answer[2..4].copy_from_slice(&1u16.to_be_bytes()); // type A
    answer[4..6].copy_from_slice(&1u16.to_be_bytes()); // class IN
    answer[6..10].copy_from_slice(&60u32.to_be_bytes()); // TTL
    answer[10..12].copy_from_slice(&4u16.to_be_bytes()); // RDLENGTH
    answer[12..16].copy_from_slice(&AP_ADDRESS.octets());
    Some(question_end + ANSWER_BYTES)
}

// ---------------------------------------------------------------------------
// The portal itself
// ---------------------------------------------------------------------------

/// What `POST /save` carries. `heapless::String` bounds each field, so an over-long value
/// is a 400 from the extractor rather than a truncation nobody notices.
#[derive(serde::Deserialize)]
struct SaveForm {
    ssid: heapless::String<SSID_MAX_BYTES>,
    #[serde(default)]
    password: heapless::String<PASSPHRASE_MAX_BYTES>,
}

/// The portal page. Built into a heap `String` because the network list is not known
/// until run time; a page of this size is a few hundred bytes of heap for a moment.
async fn portal_page(message: Option<&str>) -> String {
    let mut page = String::with_capacity(2048);
    page.push_str(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>StarPlayer setup</title><style>\
         body{background:#14161c;color:#e6e8ef;font:15px/1.5 system-ui,sans-serif;margin:0;padding:24px 16px;}\
         h1{font-size:20px;margin:0 0 4px;}p{color:#9aa3b8;margin:0 0 20px;}\
         label{display:block;margin:14px 0 4px;color:#9aa3b8;font-size:13px;}\
         input,select,button{width:100%;box-sizing:border-box;padding:10px;border-radius:6px;border:1px solid #2b3040;\
         background:#1c1f28;color:#e6e8ef;font:inherit;}\
         button{margin-top:20px;background:#4d7cff;border-color:#4d7cff;color:#fff;font-weight:600;}\
         .note{background:#2a2030;border-left:3px solid #c88;padding:10px;border-radius:4px;margin-bottom:16px;}\
         </style></head><body><h1>StarPlayer</h1><p>Choose the network this player should join.</p>",
    );
    if let Some(message) = message {
        page.push_str("<div class=\"note\">");
        push_escaped(&mut page, message);
        page.push_str("</div>");
    }
    page.push_str("<form method=\"post\" action=\"/save\"><label for=\"ssid\">Network</label>");

    let networks = NETWORKS.lock().await;
    if networks.is_empty() {
        page.push_str("<input id=\"ssid\" name=\"ssid\" placeholder=\"network name\" required>");
    } else {
        page.push_str("<select id=\"ssid\" name=\"ssid\">");
        for name in networks.iter() {
            page.push_str("<option>");
            push_escaped(&mut page, name.as_str());
            page.push_str("</option>");
        }
        page.push_str("</select>");
    }
    drop(networks);

    page.push_str(
        "<label for=\"password\">Passphrase</label>\
         <input id=\"password\" name=\"password\" type=\"password\" autocomplete=\"off\">\
         <button type=\"submit\">Save and restart</button></form>\
         <p style=\"margin-top:24px;font-size:13px\">The player restarts after saving and answers at \
         <code>http://starplayer.local/</code>.</p></body></html>",
    );
    page
}

/// Escape the four characters that matter in HTML text and attribute content.
///
/// The network names come off the air from whoever is broadcasting them, so they are
/// untrusted input even though the portal has no session to steal.
fn push_escaped(page: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => page.push_str("&amp;"),
            '<' => page.push_str("&lt;"),
            '>' => page.push_str("&gt;"),
            '"' => page.push_str("&quot;"),
            _ => page.push(character),
        }
    }
}

/// An HTML body that must not be cached — the portal's page changes on every save.
const NO_STORE: (&str, &str) = ("Cache-Control", "no-store");

/// The portal's routes, flat for the same reason `web.rs`'s are.
struct PortalRoutes;

impl PathRouterService<()> for PortalRoutes {
    async fn call_path_router_service<R: picoserve::io::Read, W: ResponseWriter<Error = R::Error>>(
        &self, state: &(), _path_parameters: (), path: Path<'_>, mut request: Request<'_, R>, response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let method = request.parts.method();
        let path = path.encoded();
        match (method, path) {
            ("GET", "/") => {
                let response = (NO_STORE, HtmlPage(portal_page(None).await));
                response.write_to(request.body_connection.finalize().await?, response_writer).await
            }
            ("POST", "/save") => {
                let form = picoserve::from_request!(state, request, response_writer, picoserve::extract::Form<SaveForm>);
                let response = save(form.0).await;
                response.write_to(request.body_connection.finalize().await?, response_writer).await
            }
            // What the phones probe for. Answering any of them with a redirect is what
            // makes the portal open by itself rather than waiting to be typed in.
            ("GET", "/generate_204" | "/gen_204" | "/hotspot-detect.html" | "/library/test/success.html" | "/connecttest.txt" | "/ncsi.txt" | "/redirect" | "/canonical.html") => {
                Redirect::to("http://192.168.4.1/").write_to(request.body_connection.finalize().await?, response_writer).await
            }
            // Anything else is also the portal: a catch-all DNS means every hostname
            // arrives here, and a 404 would look like a broken network rather than a
            // setup page.
            _ => {
                Redirect::to("http://192.168.4.1/").write_to(request.body_connection.finalize().await?, response_writer).await
            }
        }
    }
}

/// Store the credentials and arm the reset.
async fn save(form: SaveForm) -> (StatusCode, (&'static str, &'static str), HtmlPage) {
    if form.ssid.is_empty() {
        return (StatusCode::BAD_REQUEST, NO_STORE, HtmlPage(portal_page(Some("Choose a network first.")).await));
    }
    let credentials = WifiCredentials { ssid: form.ssid, passphrase: form.password };
    match crate::web::request_save_wifi(credentials).await {
        crate::web::JobOutcome::Done => {
            let mut page = String::with_capacity(512);
            page.push_str(
                "<!doctype html><html><head><meta charset=\"utf-8\">\
                 <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
                 <title>StarPlayer</title><style>body{background:#14161c;color:#e6e8ef;\
                 font:15px/1.5 system-ui,sans-serif;margin:0;padding:24px 16px}</style></head><body>\
                 <h1>Saved</h1><p>The player is restarting and will join that network. \
                 It answers at <code>http://starplayer.local/</code>.</p></body></html>",
            );
            (StatusCode::OK, NO_STORE, HtmlPage(page))
        }
        crate::web::JobOutcome::Failed(message) => {
            (StatusCode::INTERNAL_SERVER_ERROR, NO_STORE, HtmlPage(portal_page(Some(message)).await))
        }
    }
}

/// An HTML response body built at run time.
struct HtmlPage(String);

impl picoserve::response::Content for HtmlPage {
    fn content_type(&self) -> &'static str { "text/html; charset=utf-8" }
    fn content_length(&self) -> usize { self.0.len() }
    async fn write_content<W: picoserve::io::Write>(self, mut writer: W) -> Result<(), W::Error> {
        writer.write_all(self.0.as_bytes()).await
    }
}

/// The portal's router.
struct PortalApplication;

impl picoserve::AppBuilder for PortalApplication {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> { picoserve::Router::new().nest_service("", PortalRoutes) }
}

/// One portal connection's worth of server.
#[embassy_executor::task(pool_size = PORTAL_POOL_SIZE)]
async fn portal_task(id: usize, stack: Stack<'static>, config: &'static picoserve::Config) {
    let mut receive_buffer = [0u8; PORTAL_TCP_BUFFER_BYTES];
    let mut transmit_buffer = [0u8; PORTAL_TCP_BUFFER_BYTES];
    let mut http_buffer = [0u8; PORTAL_HTTP_BUFFER_BYTES];
    let application = picoserve::AppBuilder::build_app(PortalApplication);

    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut receive_buffer, &mut transmit_buffer);
        if socket.accept(80).await.is_err() {
            continue;
        }
        socket.set_timeout(Some(Duration::from_secs(20)));
        if let Err(error) = picoserve::Server::new(&application, config, &mut http_buffer).serve(socket).await {
            println!("PORTAL worker {id}: {error:?}");
        }
    }
}
