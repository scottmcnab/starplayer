//! [`MediaServer`] — the HTTP server the speaker fetches audio from.
//!
//! # The bind address matters more than anything else here
//!
//! The receiver fetches the URL *from the speaker*, so the address in that URL has to be
//! one the speaker can route to. `127.0.0.1` is the classic failure: the server works, the
//! LOAD succeeds, and the speaker plays silence because it dialled its own loopback.
//!
//! Finding the right interface needs no dependency and no enumeration, only the routing
//! table the kernel already has: bind a UDP socket to `0.0.0.0:0`, `connect` it to the
//! device's address — which sends no packet, UDP `connect` only fixes the peer — and read
//! back `local_addr()`. That is the kernel's own answer to "which of my addresses would
//! reach that host", including the right answer on a machine with a VPN, several NICs or a
//! bridge.
//!
//! # Range requests
//!
//! A pre-rendered asset is served under RFC 7233 for a single byte range, because that is
//! what gives the receiver seek and a clean end:
//!
//! - every response to a known path carries `Accept-Ranges: bytes` and a `Content-Type`;
//! - no `Range` → `200` with the whole asset;
//! - `bytes=first-last`, `bytes=first-` and `bytes=-suffix` → `206` with `Content-Range`
//!   and exactly those bytes, `last` clamped to the end;
//! - a syntactically valid range entirely past the end → `416` with `Content-Range:
//!   bytes */total` and no body;
//! - a malformed `Range`, or a multi-range request, is **ignored** and answered `200` with
//!   the whole asset, which RFC 7233 §3.1 explicitly permits. Multipart ranges are not
//!   implemented: no Cast receiver asks for one and the code to produce them would never
//!   be exercised.
//!
//! `HEAD` gets the identical headers and no body, for both the `200` and the `206` case.
//!
//! A **streamed** path has no length and no addressable past, so it is answered `200`
//! chunked from wherever the stream is now, `Range` or no `Range`.

use std::collections::HashMap;
use std::io::{self, Cursor, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use tiny_http::{Header, Request, Response, Server, StatusCode};

use crate::CastError;

/// What the server holds under one path.
enum Asset {
    /// A complete, `Range`-addressable asset.
    Bytes { content_type: String, bytes: Arc<Vec<u8>> },
    /// A live stream, consumable exactly once: the first request takes the channel.
    Stream { content_type: String, chunks: Option<Receiver<Vec<u8>>> },
}

/// The set of paths this server answers, shared with its thread.
type Assets = Arc<Mutex<HashMap<String, Asset>>>;

/// A small HTTP server on the LAN interface that reaches one Cast device.
pub struct MediaServer {
    server: Arc<Server>,
    assets: Assets,
    address: SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl MediaServer {
    /// Bind a server on a free port of whichever local interface reaches
    /// `device_address`, and start its thread.
    ///
    /// Falls back to `0.0.0.0` only when the routing table has no answer at all — a
    /// machine with no route to the device, which is what WSL2's NAT looks like from
    /// inside. The URL then names whatever address the listener reports, and the caller is
    /// expected to say out loud which address went into it, because an unroutable one is
    /// the single most likely reason a speaker stays silent.
    pub fn bind_for(device_address: IpAddr) -> Result<MediaServer, CastError> {
        let bind_address = local_address_reaching(device_address).unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        let server = Server::http((bind_address, 0u16)).map_err(|error| CastError::Io(format!("could not bind a media server on {bind_address}: {error}")))?;
        let address = server
            .server_addr()
            .to_ip()
            .ok_or_else(|| CastError::Io(String::from("the media server bound something that is not an IP socket")))?;
        let server = Arc::new(server);
        let assets: Assets = Arc::new(Mutex::new(HashMap::new()));

        let thread_server = Arc::clone(&server);
        let thread_assets = Arc::clone(&assets);
        let thread = std::thread::spawn(move || {
            for request in thread_server.incoming_requests() {
                serve_one(&thread_assets, request);
            }
        });

        Ok(MediaServer { server, assets, address, thread: Some(thread) })
    }

    /// The address the server is listening on. Whether the *speaker* can reach it is the
    /// question `bind_for`'s fallback cannot answer.
    pub fn address(&self) -> SocketAddr { self.address }

    /// Serve `bytes` under `path` — `Range`-addressable, with a known length.
    pub fn serve_bytes(&mut self, path: &str, content_type: &str, bytes: Vec<u8>) {
        let asset = Asset::Bytes { content_type: content_type.to_string(), bytes: Arc::new(bytes) };
        self.insert(path, asset);
    }

    /// Serve a live stream under `path`: one chunk per `Vec<u8>`, ending when the channel
    /// closes.
    ///
    /// Exactly one request can consume it — a stream has no second copy — so a second `GET`
    /// on the same path gets a `404`.
    pub fn serve_stream(&mut self, path: &str, content_type: &str, chunks: Receiver<Vec<u8>>) {
        let asset = Asset::Stream { content_type: content_type.to_string(), chunks: Some(chunks) };
        self.insert(path, asset);
    }

    fn insert(&mut self, path: &str, asset: Asset) {
        let key = normalise(path);
        if let Ok(mut assets) = self.assets.lock() {
            assets.insert(key, asset);
        }
    }

    /// The absolute URL of `path` on this server, for a `contentId`.
    pub fn url(&self, path: &str) -> String { format!("http://{}{}", self.address, normalise(path)) }

    /// Stop accepting connections and wait for the server thread to finish.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for MediaServer {
    fn drop(&mut self) { self.stop(); }
}

/// A leading `/`, and nothing else changed: the path is ours, not user input.
fn normalise(path: &str) -> String {
    if path.starts_with('/') { path.to_string() } else { format!("/{path}") }
}

/// The local address the routing table would use to reach `device_address`.
///
/// UDP `connect` sends nothing; it only fixes the peer so that `local_addr` has an answer.
/// `None` means there is no route at all, which is a real answer and not a failure to
/// report — see [`MediaServer::bind_for`].
pub fn local_address_reaching(device_address: IpAddr) -> Option<IpAddr> {
    let bind: SocketAddr = match device_address {
        IpAddr::V4(_) => SocketAddr::from(([0, 0, 0, 0], 0)),
        IpAddr::V6(_) => SocketAddr::from(([0u16, 0, 0, 0, 0, 0, 0, 0], 0)),
    };
    let socket = UdpSocket::bind(bind).ok()?;
    // Port 9 is `discard`; nothing is sent to it, the number only has to be legal.
    socket.connect(SocketAddr::new(device_address, 9)).ok()?;
    let local = socket.local_addr().ok()?.ip();
    // An unspecified answer is the kernel saying it has not picked an interface, which is
    // no more use in a URL than loopback would be.
    if local.is_unspecified() { None } else { Some(local) }
}

/// Answer one request.
fn serve_one(assets: &Assets, request: Request) {
    let path = request.url().split('?').next().unwrap_or("").to_string();
    let is_head = *request.method() == tiny_http::Method::Head;
    let range_header = header_value(&request, "Range");

    let mut guard = match assets.lock() {
        Ok(guard) => guard,
        Err(_) => {
            let _ = request.respond(text_response(500, "media server state is poisoned"));
            return;
        }
    };

    match guard.get_mut(&path) {
        None => {
            drop(guard);
            let _ = request.respond(text_response(404, "not found"));
        }
        Some(Asset::Bytes { content_type, bytes }) => {
            let content_type = content_type.clone();
            let bytes = Arc::clone(bytes);
            drop(guard);
            respond_with_bytes(request, &content_type, &bytes, range_header.as_deref(), is_head);
        }
        Some(Asset::Stream { content_type, chunks }) => {
            let content_type = content_type.clone();
            let taken = chunks.take();
            drop(guard);
            match taken {
                // A live stream has no addressable past, so `Range` is ignored entirely
                // and the response starts from wherever the stream is now.
                Some(chunks) => {
                    let headers = vec![header("Content-Type", &content_type), header("Accept-Ranges", "none"), header("Cache-Control", "no-store")];
                    let response = Response::new(StatusCode(200), headers, ChunkReader::new(chunks), None, None);
                    let _ = request.respond(response);
                }
                None => {
                    let _ = request.respond(text_response(404, "this live stream has already been claimed"));
                }
            }
        }
    }
}

/// The `200`/`206`/`416` decision for a complete asset, and the response that follows.
fn respond_with_bytes(request: Request, content_type: &str, bytes: &[u8], range_header: Option<&str>, is_head: bool) {
    let total = bytes.len();
    let mut headers = vec![header("Content-Type", content_type), header("Accept-Ranges", "bytes")];

    let (status, body): (u16, &[u8]) = match parse_range(range_header, total) {
        RangeOutcome::Whole => (200, bytes),
        RangeOutcome::Partial { first, last } => {
            headers.push(header("Content-Range", &format!("bytes {first}-{last}/{total}")));
            (206, &bytes[first..=last])
        }
        RangeOutcome::Unsatisfiable => {
            headers.push(header("Content-Range", &format!("bytes */{total}")));
            (416, &[])
        }
    };

    // A `HEAD` must report the `Content-Length` the matching `GET` would send, so the body
    // is described here and suppressed by tiny_http rather than left out.
    let length = body.len();
    let reader: Box<dyn Read + Send> = if is_head { Box::new(io::empty()) } else { Box::new(Cursor::new(body.to_vec())) };
    // Without this, tiny_http switches to chunked transfer for anything over 32 KiB and
    // the response loses its `Content-Length` — which is the one header a media receiver
    // needs in order to seek.
    let response = Response::new(StatusCode(status), headers, reader, Some(length), None).with_chunked_threshold(usize::MAX);
    let _ = request.respond(response);
}

/// What a `Range` header asks for, once it has been checked against the asset's length.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RangeOutcome {
    /// No range, or one this server declines to honour: send everything with `200`.
    Whole,
    /// Inclusive byte offsets, both within the asset.
    Partial { first: usize, last: usize },
    /// A well-formed range that names nothing inside the asset: `416`.
    Unsatisfiable,
}

/// Parse a single-range `Range: bytes=…` header against an asset of `total` bytes.
///
/// Anything this does not understand — a unit other than `bytes`, a comma-separated list,
/// a reversed range, non-numeric bounds — is [`RangeOutcome::Whole`], which RFC 7233 §3.1
/// allows: "a server MAY ignore the Range header field".
pub fn parse_range(header: Option<&str>, total: usize) -> RangeOutcome {
    let Some(header) = header else { return RangeOutcome::Whole };
    let Some(spec) = header.trim().strip_prefix("bytes=") else { return RangeOutcome::Whole };
    // A multi-range request is answered with the whole asset rather than multipart.
    if spec.contains(',') {
        return RangeOutcome::Whole;
    }
    let Some((start, end)) = spec.split_once('-') else { return RangeOutcome::Whole };
    let (start, end) = (start.trim(), end.trim());

    if start.is_empty() {
        // `bytes=-N`: the last N bytes.
        let Ok(suffix) = end.parse::<usize>() else { return RangeOutcome::Whole };
        if suffix == 0 {
            return RangeOutcome::Unsatisfiable;
        }
        if total == 0 {
            return RangeOutcome::Unsatisfiable;
        }
        let first = total.saturating_sub(suffix);
        return RangeOutcome::Partial { first, last: total - 1 };
    }

    let Ok(first) = start.parse::<usize>() else { return RangeOutcome::Whole };
    if first >= total {
        return RangeOutcome::Unsatisfiable;
    }
    if end.is_empty() {
        // `bytes=N-`: from N to the end.
        return RangeOutcome::Partial { first, last: total - 1 };
    }
    let Ok(last) = end.parse::<usize>() else { return RangeOutcome::Whole };
    if last < first {
        return RangeOutcome::Whole;
    }
    // A `last` past the end clamps rather than failing: RFC 7233 §2.1.
    RangeOutcome::Partial { first, last: last.min(total - 1) }
}

/// The value of one request header, case-insensitively.
///
/// `HeaderField::equiv` only accepts a `&'static str`, which a helper taking a borrowed
/// name cannot promise, so the comparison is spelled out here instead.
fn header_value(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str().to_string())
}

/// One response header. The inputs are all this crate's own constants and formatted
/// numbers, so a rejected header would be a bug rather than bad input.
fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a header this crate composed is well-formed")
}

/// One line of `text/plain`, for the error paths.
fn text_response(status: u16, message: &str) -> Response<Cursor<Vec<u8>>> {
    Response::from_string(format!("{message}\n")).with_status_code(StatusCode(status)).with_header(header("Content-Type", "text/plain; charset=utf-8"))
}

/// Turns a channel of chunks into the `Read` tiny_http copies from.
///
/// One `recv` per exhausted chunk, and `Ok(0)` — end of response — when the sender is
/// dropped. Blocking is the point: the HTTP thread should wait for the encoder rather than
/// spin, and the encoder's bounded channel is what stops it running ahead.
struct ChunkReader {
    chunks: Receiver<Vec<u8>>,
    current: Vec<u8>,
    offset: usize,
}

impl ChunkReader {
    fn new(chunks: Receiver<Vec<u8>>) -> ChunkReader { ChunkReader { chunks, current: Vec::new(), offset: 0 } }
}

impl Read for ChunkReader {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        while self.offset >= self.current.len() {
            match self.chunks.recv() {
                Ok(chunk) => {
                    self.current = chunk;
                    self.offset = 0;
                }
                // The sender is gone: the stream is over and the chunked response ends.
                Err(_) => return Ok(0),
            }
        }
        let available = &self.current[self.offset..];
        let count = available.len().min(destination.len());
        destination[..count].copy_from_slice(&available[..count]);
        self.offset += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_range_header_asks_for_the_whole_asset() {
        assert_eq!(parse_range(None, 1_000), RangeOutcome::Whole);
    }

    #[test]
    fn the_three_single_range_forms_are_understood() {
        assert_eq!(parse_range(Some("bytes=0-99"), 1_000), RangeOutcome::Partial { first: 0, last: 99 });
        assert_eq!(parse_range(Some("bytes=500-"), 1_000), RangeOutcome::Partial { first: 500, last: 999 });
        assert_eq!(parse_range(Some("bytes=-100"), 1_000), RangeOutcome::Partial { first: 900, last: 999 });
    }

    #[test]
    fn an_end_past_the_asset_clamps_rather_than_failing() {
        assert_eq!(parse_range(Some("bytes=900-99999"), 1_000), RangeOutcome::Partial { first: 900, last: 999 });
        assert_eq!(parse_range(Some("bytes=-99999"), 1_000), RangeOutcome::Partial { first: 0, last: 999 });
    }

    #[test]
    fn a_range_entirely_past_the_asset_is_unsatisfiable() {
        assert_eq!(parse_range(Some("bytes=1000-1100"), 1_000), RangeOutcome::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=1000-"), 1_000), RangeOutcome::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), 1_000), RangeOutcome::Unsatisfiable);
    }

    #[test]
    fn a_malformed_or_multi_range_is_ignored_rather_than_refused() {
        assert_eq!(parse_range(Some("bytes=0-1,4-5"), 1_000), RangeOutcome::Whole, "multipart is not implemented, so the whole asset is the answer");
        assert_eq!(parse_range(Some("items=0-1"), 1_000), RangeOutcome::Whole);
        assert_eq!(parse_range(Some("bytes=abc-def"), 1_000), RangeOutcome::Whole);
        assert_eq!(parse_range(Some("bytes=99-10"), 1_000), RangeOutcome::Whole);
        assert_eq!(parse_range(Some("nonsense"), 1_000), RangeOutcome::Whole);
    }

    #[test]
    fn a_url_always_has_exactly_one_slash_between_the_port_and_the_path() {
        assert_eq!(normalise("song.flac"), "/song.flac");
        assert_eq!(normalise("/song.flac"), "/song.flac");
    }

    #[test]
    fn a_chunk_reader_ends_when_its_sender_is_dropped() {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(vec![1u8, 2, 3]).unwrap();
        sender.send(vec![4u8]).unwrap();
        drop(sender);

        let mut reader = ChunkReader::new(receiver);
        let mut everything = Vec::new();
        reader.read_to_end(&mut everything).unwrap();
        assert_eq!(everything, vec![1, 2, 3, 4]);
    }

    #[test]
    fn the_route_to_loopback_is_loopback() {
        let local = local_address_reaching(IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(local, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)), "the kernel's own answer for reaching 127.0.0.1");
    }
}
