//! Station mode: join the stored network, take a DHCP lease, and answer to
//! `starplayer.local`.
//!
//! Three tasks, all on core 0 (the audio refill owns core 1, M8-I5):
//!
//! * [`connection_task`] owns the [`WifiController`] and does nothing but keep the link
//!   up — connect, wait for the disconnect event, reconnect.
//! * [`net_task`] is embassy-net's own runner.
//! * [`mdns_task`] answers multicast DNS for `starplayer.local` so the owner never has to
//!   find the device's address.
//!
//! # esp-radio 0.18 has no `init`
//!
//! Older esp-wifi releases wanted `esp_wifi::init(timer, rng, radio_clocks)` and handed
//! back a controller token. 0.18 does not: `esp_rtos::start` must have run — this
//! firmware already calls it in `main` for the embassy time driver — and then
//! `esp_radio::wifi::new` brings the radio up and returns the controller and its two
//! interfaces. What the radio needs from the scheduler arrives through `esp-rtos`'s
//! `esp-radio` feature, which the `web` feature turns on.
//!
//! The controller also **starts implicitly** with whatever initial configuration it was
//! given, so there is no `start_async` here and none is missing.

use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, DhcpConfig, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer};
use esp_println::println;
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{Config as RadioConfig, ControllerConfig, Interface, WifiController};
use static_cell::StaticCell;

use crate::store::WifiCredentials;

/// The name the device answers to: `http://starplayer.local/`, and the hostname it offers
/// the DHCP server so a router's own client list says something legible too.
pub const HOSTNAME: &str = "starplayer";

/// The port the web server listens on.
pub const HTTP_PORT: u16 = 80;

/// smoltcp socket slots. Two web workers, one DHCP, one DNS, one mDNS, plus spare.
///
/// Deliberately counted rather than rounded up: every slot is a `SocketStorage` in
/// `.bss`, and on this chip `.bss` is subtracted from the main stack (see `main.rs`'s
/// `HEAP_BYTES` comment for the same arithmetic).
const STACK_SOCKETS: usize = 1 + 1 + 1 + crate::web::WEB_TASK_POOL_SIZE + 2;

/// How long to wait between polls while the link comes up.
const LINK_POLL: Duration = Duration::from_millis(250);

/// Backoff after a failed connect or a failed mDNS bind.
const RETRY_DELAY: Duration = Duration::from_secs(5);

/// mDNS receive and send buffers.
///
/// 1472 bytes, matching `edge-nal-embassy`'s own per-socket buffer, and the number is
/// load-bearing rather than tidy: the responder receives *every* multicast mDNS packet on
/// the network, and a datagram larger than the buffer handed to `recv` fails the whole
/// responder rather than being truncated. A television or a printer announcing itself
/// with a kilobyte of TXT records is enough to trip a smaller one.
const MDNS_BUFFER_BYTES: usize = 1472;

/// The mDNS send buffer. Only this device's own answers go through it.
const MDNS_SEND_BUFFER_BYTES: usize = 512;

/// The controller and the network stack, after [`start`].
pub struct Station {
    pub stack: Stack<'static>,
}

/// Bring the radio up in station mode, start the stack, and spawn the three tasks.
///
/// Returns as soon as the tasks are spawned — the caller waits for an address with
/// [`wait_for_address`] if it wants one.
pub fn start(spawner: Spawner, wifi: esp_hal::peripherals::WIFI<'static>, credentials: WifiCredentials, seed: u64) -> Result<Station, &'static str> {
    static CREDENTIALS: StaticCell<WifiCredentials> = StaticCell::new();
    let credentials: &'static WifiCredentials = CREDENTIALS.init(credentials);

    let (controller, interfaces) = esp_radio::wifi::new(
        wifi,
        // The queue depths are ampkeeper's, found the hard way on this chip family: the
        // defaults (5 receive, 3 transmit) are exceeded by an ordinary broadcast burst —
        // ARP, mDNS, NetBIOS — and by smoltcp's own retransmits behind a 4 KiB TCP window,
        // and an overflow shows up as a wedged socket rather than as an error.
        ControllerConfig::default().with_rx_queue_size(20).with_tx_queue_size(16).with_initial_config(station_config(credentials)),
    )
    .map_err(|_| "the WiFi controller would not start")?;

    // No DHCP hostname: `DhcpConfig::hostname` is a heapless 0.9 `String` and this crate
    // is on heapless 0.8 for picoserve's sake, so setting it would mean carrying a second
    // renamed copy of the crate for one cosmetic field in a router's client list. mDNS is
    // how the device is found, and that name is set below.
    let dhcp = DhcpConfig::default();

    static RESOURCES: StaticCell<StackResources<STACK_SOCKETS>> = StaticCell::new();
    let (stack, runner) = embassy_net::new(interfaces.station, NetConfig::dhcpv4(dhcp), RESOURCES.init(StackResources::new()), seed);

    spawner.spawn(connection_task(controller, credentials).map_err(|_| "the WiFi connection task would not spawn")?);
    spawner.spawn(net_task(runner).map_err(|_| "the network task would not spawn")?);
    spawner.spawn(mdns_task(stack).map_err(|_| "the mDNS task would not spawn")?);
    Ok(Station { stack })
}

/// The station configuration for the stored credentials.
fn station_config(credentials: &WifiCredentials) -> RadioConfig {
    RadioConfig::Station(
        // `with_password` takes an owned `alloc::string::String`, which is what the
        // driver stores; the credentials themselves stay in their fixed-capacity form.
        StationConfig::default().with_ssid(credentials.ssid.as_str()).with_password(alloc::string::String::from(credentials.passphrase.as_str())),
    )
}

/// Block until the stack has a link and an IPv4 address, logging the lease once.
pub async fn wait_for_address(stack: Stack<'static>) -> embassy_net::Ipv4Cidr {
    loop {
        if stack.is_link_up()
            && let Some(config) = stack.config_v4()
        {
            println!("WIFI address {} gateway {:?} dns {:?}", config.address, config.gateway, config.dns_servers.first());
            return config.address;
        }
        Timer::after(LINK_POLL).await;
    }
}

/// Keep the link up: connect, wait for the disconnect event, connect again.
///
/// Far simpler than ampkeeper's, deliberately. Its state machine carries an RSSI poll, a
/// scan request channel and a mode-flip that works around a transmit-queue leak after a
/// long outage; none of that has been *observed* on this board, and inventing a
/// workaround for a fault nobody has seen here would be a worse deviation than leaving it
/// out. If the owner's run finds the link does not recover from a router reboot, this is
/// the function that grows.
#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>, credentials: &'static WifiCredentials) {
    loop {
        if controller.is_connected() {
            let _ = controller.wait_for_disconnect_async().await;
            println!("WIFI disconnected from {}", credentials.ssid.as_str());
            Timer::after(RETRY_DELAY).await;
        }
        match controller.connect_async().await {
            Ok(_) => println!("WIFI connected to {}", credentials.ssid.as_str()),
            Err(error) => {
                println!("WIFI connect to {} failed: {error:?}", credentials.ssid.as_str());
                Timer::after(RETRY_DELAY).await;
            }
        }
    }
}

/// embassy-net's own runner.
#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface<'static>>) -> ! { runner.run().await }

/// Answer multicast DNS for `starplayer.local` and advertise `_http._tcp`.
///
/// The responder is torn down and rebuilt on every address change rather than kept alive
/// across one. That is not laziness: `edge-mdns`'s `broadcast` sends its announcement
/// burst once and then waits on a signal for ever — there is no periodic re-announce
/// anywhere in the crate — so a rebuild, with its new socket, new multicast join and new
/// burst, *is* the re-announcement.
#[embassy_executor::task]
async fn mdns_task(stack: Stack<'static>) {
    use edge_mdns::HostAnswersMdnsHandler;
    use edge_mdns::buf::VecBufAccess;
    use edge_mdns::domain::base::Ttl;
    use edge_mdns::host::{Host, Service, ServiceAnswers};
    use edge_mdns::io::{self, IPV4_DEFAULT_SOCKET};
    use edge_nal::UdpSplit;
    use edge_nal_embassy::{Udp, UdpBuffers};
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embassy_sync::signal::Signal;

    // Task-local rather than static: `edge_nal_embassy`'s socket pool is built on plain
    // `Cell`s and is not `Sync`, so it cannot be a `static` at all.
    let buffers: UdpBuffers<1, MDNS_BUFFER_BYTES, MDNS_BUFFER_BYTES, 2> = UdpBuffers::new();
    let udp = Udp::new(stack, &buffers);
    let random = esp_hal::rng::Rng::new();

    loop {
        let address = wait_for_address(stack).await;

        // One call binds and joins 224.0.0.251 — the multicast join is not a separate
        // step, which is why `embassy-net/multicast` and `edge-nal-embassy/multicast` are
        // both on.
        let mut socket = match io::bind(&udp, IPV4_DEFAULT_SOCKET, Some(core::net::Ipv4Addr::UNSPECIFIED), None).await {
            Ok(socket) => socket,
            Err(_) => {
                println!("MDNS bind failed; retrying");
                Timer::after(RETRY_DELAY).await;
                continue;
            }
        };
        let (receive, send) = socket.split();

        let host = Host {
            hostname: HOSTNAME,
            ipv4: core::net::Ipv4Addr::from(address.address().octets()),
            ipv6: core::net::Ipv6Addr::UNSPECIFIED,
            ttl: Ttl::from_secs(60),
        };
        let service = Service {
            name: HOSTNAME,
            priority: 0,
            weight: 0,
            service: "_http",
            protocol: "_tcp",
            port: HTTP_PORT,
            service_subtypes: &[],
            txt_kvs: &[],
        };
        let answers = ServiceAnswers::new(&host, &service);
        let receive_buffer = VecBufAccess::<NoopRawMutex, MDNS_BUFFER_BYTES>::new();
        // The send buffer holds one answer — a hostname, an A record and an SRV/TXT pair
        // — and never a stranger's packet, so it is sized to what this responder writes
        // rather than to what it might receive.
        let send_buffer = VecBufAccess::<NoopRawMutex, MDNS_SEND_BUFFER_BYTES>::new();
        // Never signalled: the announcement burst `run` sends at start-up is the whole
        // announcement, and a rebuild is how this task re-announces.
        let broadcast: Signal<NoopRawMutex, ()> = Signal::new();

        let mdns = io::Mdns::new(
            Some(core::net::Ipv4Addr::UNSPECIFIED),
            None,
            receive,
            send,
            &receive_buffer,
            &send_buffer,
            random,
            &broadcast,
        );
        println!("MDNS answering for {HOSTNAME}.local at {}", address.address());

        let _ = embassy_futures::select::select(mdns.run(HostAnswersMdnsHandler::new(&answers)), wait_for_address_change(stack, address)).await;
        // The socket drops here, freeing its smoltcp slot before the next bind.
        Timer::after(RETRY_DELAY).await;
    }
}

/// Resolve when the stack's address stops being `address` — a lease change, or a link
/// that went down.
async fn wait_for_address_change(stack: Stack<'static>, address: embassy_net::Ipv4Cidr) {
    loop {
        Timer::after(Duration::from_secs(2)).await;
        let current = stack.config_v4().map(|config| config.address);
        if !stack.is_link_up() || current != Some(address) {
            return;
        }
    }
}
