//! An in-process Cast receiver, so the CASTv2 session can be tested with no speaker, no
//! multicast and no network beyond loopback.
//!
//! The receiver terminates a real TLS connection — `rustls`'s server half with an `rcgen`
//! self-signed certificate — on `127.0.0.1` and an ephemeral port, and speaks the protocol
//! a Chromecast speaks: 4-byte big-endian length, then a `CastMessage` protobuf.
//!
//! **The protobuf is decoded by hand.** `CastMessage` is seven fields and no more:
//!
//! | # | Field | Wire type |
//! |---|---|---|
//! | 1 | `protocol_version` | varint (0 = `CASTV2_1_0`) |
//! | 2 | `source_id` | length-delimited UTF-8 |
//! | 3 | `destination_id` | length-delimited UTF-8 |
//! | 4 | `namespace` | length-delimited UTF-8 |
//! | 5 | `payload_type` | varint (0 = `STRING`, 1 = `BINARY`) |
//! | 6 | `payload_utf8` | length-delimited UTF-8 |
//! | 7 | `payload_binary` | length-delimited bytes |
//!
//! A protobuf crate whose only user is one test would be a dependency bought for forty
//! lines of varint decoding, so these forty lines are here instead. The JSON is written and
//! read the same way, with `format!` and a couple of field-extraction helpers, so the test
//! carries no serde of its own either.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use starplayer_cast::{CastSession, MediaRequest, StreamKind};

// ── the four namespaces a Default Media Receiver session touches ─────────────────────
const CONNECTION_NAMESPACE: &str = "urn:x-cast:com.google.cast.tp.connection";
const HEARTBEAT_NAMESPACE: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
const RECEIVER_NAMESPACE: &str = "urn:x-cast:com.google.cast.receiver";
const MEDIA_NAMESPACE: &str = "urn:x-cast:com.google.cast.media";

/// The transport id this fake receiver hands out for its application.
const TRANSPORT_ID: &str = "web-1";
/// The session id this fake receiver hands out for its application.
const SESSION_ID: &str = "session-42";
/// The media session id it hands out on LOAD.
const MEDIA_SESSION_ID: i32 = 7;

/// One message the receiver saw.
#[derive(Clone, Debug)]
struct Seen {
    namespace: String,
    source: String,
    destination: String,
    payload: String,
}

impl Seen {
    fn message_type(&self) -> String { json_string(&self.payload, "type").unwrap_or_default() }

    fn request_id(&self) -> Option<u64> { json_number(&self.payload, "requestId").map(|value| value as u64) }
}

/// A Cast receiver that exists only inside this test binary.
struct FakeReceiver {
    address: SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeReceiver {
    /// Start one, listening on loopback.
    fn start() -> FakeReceiver {
        let certified = rcgen::generate_simple_self_signed(vec![String::from("localhost")]).expect("rcgen makes a self-signed certificate");
        let certificate = CertificateDer::from(certified.cert.der().to_vec());
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()));
        let config = ServerConfig::builder().with_no_client_auth().with_single_cert(vec![certificate], key).expect("rustls accepts the certificate");
        let config = Arc::new(config);

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("loopback has a free port");
        let address = listener.local_addr().expect("a bound listener has an address");
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        // Non-blocking accept, so the thread can notice the stop flag between senders.
        listener.set_nonblocking(true).expect("a loopback listener can be non-blocking");

        let thread_seen = Arc::clone(&seen);
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            // The receiver's own state outlives any one sender's connection: that is what
            // makes "attach to the application that is already running" testable.
            let mut state = ReceiverState { launched: false, loaded: false };
            while !thread_stop.load(Ordering::SeqCst) {
                let socket = match listener.accept() {
                    Ok((socket, _peer)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => return,
                };
                socket.set_nonblocking(false).ok();
                socket.set_read_timeout(Some(Duration::from_millis(100))).ok();
                // A real Chromecast does not sit on a 40 ms delayed ACK before answering a
                // control message, and neither should this: without it every round trip in
                // the latency measurement below is Nagle's timer rather than the protocol.
                socket.set_nodelay(true).ok();
                let Ok(connection) = ServerConnection::new(Arc::clone(&config)) else { return };
                let mut stream = StreamOwned::new(connection, socket);
                serve(&mut stream, &thread_seen, &thread_stop, &mut state);
            }
        });

        FakeReceiver { address, seen, stop, thread: Some(thread) }
    }

    fn seen(&self) -> Vec<Seen> { self.seen.lock().expect("the record is not poisoned").clone() }

    /// Wait until `predicate` matches something the receiver has seen, or give up.
    fn wait_for(&self, what: &str, predicate: impl Fn(&Seen) -> bool) -> Seen {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(found) = self.seen().into_iter().find(&predicate) {
                return found;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the receiver never saw {what}; it saw {:?}", self.seen().iter().map(|seen| (seen.namespace.clone(), seen.message_type())).collect::<Vec<_>>());
    }

    fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for FakeReceiver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What the fake receiver remembers between senders.
struct ReceiverState {
    launched: bool,
    loaded: bool,
}

/// The receiver's own loop for one sender's connection: read a message, record it, answer
/// it the way a Chromecast would. Returns when the sender goes away.
fn serve(stream: &mut StreamOwned<ServerConnection, std::net::TcpStream>, seen: &Arc<Mutex<Vec<Seen>>>, stop: &Arc<AtomicBool>, state: &mut ReceiverState) {
    while !stop.load(Ordering::SeqCst) {
        let message = match read_message(stream) {
            Ok(Some(message)) => message,
            // A read timeout: nothing waiting, go round again and re-check the stop flag.
            Ok(None) => continue,
            Err(_) => return,
        };
        seen.lock().expect("the record is not poisoned").push(message.clone());

        let request_id = message.request_id().unwrap_or(0);
        match message.namespace.as_str() {
            // A virtual connection is opened and closed without an answer.
            CONNECTION_NAMESPACE => {}
            HEARTBEAT_NAMESPACE => {
                if message.message_type() == "PING" {
                    let _ = write_message(stream, HEARTBEAT_NAMESPACE, "receiver-0", &message.source, r#"{"type":"PONG"}"#);
                }
            }
            RECEIVER_NAMESPACE => match message.message_type().as_str() {
                "LAUNCH" => {
                    state.launched = true;
                    let _ = write_message(stream, RECEIVER_NAMESPACE, "receiver-0", &message.source, &receiver_status(request_id, state.launched));
                    // A real receiver pings its senders; this is where the session's
                    // PONG gets exercised.
                    let _ = write_message(stream, HEARTBEAT_NAMESPACE, "receiver-0", &message.source, r#"{"type":"PING"}"#);
                }
                "STOP" => {
                    state.launched = false;
                    let _ = write_message(stream, RECEIVER_NAMESPACE, "receiver-0", &message.source, &receiver_status(request_id, state.launched));
                }
                // GET_STATUS and SET_VOLUME both answer with the receiver's status.
                _ => {
                    let _ = write_message(stream, RECEIVER_NAMESPACE, "receiver-0", &message.source, &receiver_status(request_id, state.launched));
                }
            },
            MEDIA_NAMESPACE => {
                // A decoy carrying somebody else's request id, sent first. A sender that
                // matched replies to requests by arrival order rather than by `requestId`
                // would take this one and report the wrong state.
                let _ = write_message(stream, MEDIA_NAMESPACE, TRANSPORT_ID, &message.source, &media_status(request_id + 10_000, "BUFFERING", None, 999.0));
                let (state, idle_reason) = match message.message_type().as_str() {
                    "LOAD" => {
                        state.loaded = true;
                        ("PLAYING", None)
                    }
                    "PAUSE" => ("PAUSED", None),
                    "STOP" => {
                        state.loaded = false;
                        ("IDLE", Some("CANCELLED"))
                    }
                    _ => {
                        if state.loaded {
                            ("PLAYING", None)
                        } else {
                            ("IDLE", None)
                        }
                    }
                };
                let _ = write_message(stream, MEDIA_NAMESPACE, TRANSPORT_ID, &message.source, &media_status(request_id, state, idle_reason, 12.5));
            }
            _ => {}
        }
    }
}

// ── the JSON a real receiver sends ───────────────────────────────────────────────────

fn receiver_status(request_id: u64, launched: bool) -> String {
    let applications = if launched {
        format!(
            r#"[{{"appId":"CC1AD845","sessionId":"{SESSION_ID}","transportId":"{TRANSPORT_ID}","namespaces":[{{"name":"{MEDIA_NAMESPACE}"}}],"displayName":"Default Media Receiver","statusText":"Ready To Cast"}}]"#
        )
    } else {
        String::from("[]")
    };
    format!(
        r#"{{"requestId":{request_id},"type":"RECEIVER_STATUS","status":{{"applications":{applications},"isActiveInput":true,"isStandBy":false,"volume":{{"level":0.4,"muted":false}}}}}}"#
    )
}

fn media_status(request_id: u64, player_state: &str, idle_reason: Option<&str>, current_time: f32) -> String {
    let idle = match idle_reason {
        Some(reason) => format!(r#","idleReason":"{reason}""#),
        None => String::new(),
    };
    format!(
        r#"{{"requestId":{request_id},"type":"MEDIA_STATUS","status":[{{"mediaSessionId":{MEDIA_SESSION_ID},"playbackRate":1.0,"playerState":"{player_state}","currentTime":{current_time},"supportedMediaCommands":274447{idle}}}]}}"#
    )
}

// ── CASTv2 framing and the `CastMessage` protobuf, by hand ───────────────────────────

/// Read one length-prefixed `CastMessage`. `Ok(None)` is a read timeout with nothing
/// waiting.
fn read_message(stream: &mut StreamOwned<ServerConnection, std::net::TcpStream>) -> std::io::Result<Option<Seen>> {
    let mut length_bytes = [0u8; 4];
    match stream.read_exact(&mut length_bytes) {
        Ok(()) => {}
        Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(length_bytes) as usize;
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body)?;
    decode_cast_message(&body).map(Some).ok_or_else(|| std::io::Error::other("not a CastMessage"))
}

/// Write one length-prefixed `CastMessage` with a `STRING` payload.
fn write_message(stream: &mut StreamOwned<ServerConnection, std::net::TcpStream>, namespace: &str, source: &str, destination: &str, payload: &str) -> std::io::Result<()> {
    let body = encode_cast_message(namespace, source, destination, payload);
    stream.write_all(&(body.len() as u32).to_be_bytes())?;
    stream.write_all(&body)?;
    stream.flush()
}

fn encode_cast_message(namespace: &str, source: &str, destination: &str, payload: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    // 1: protocol_version = CASTV2_1_0
    bytes.push(0x08);
    bytes.push(0x00);
    push_length_delimited(&mut bytes, 0x12, source.as_bytes());
    push_length_delimited(&mut bytes, 0x1A, destination.as_bytes());
    push_length_delimited(&mut bytes, 0x22, namespace.as_bytes());
    // 5: payload_type = STRING
    bytes.push(0x28);
    bytes.push(0x00);
    push_length_delimited(&mut bytes, 0x32, payload.as_bytes());
    bytes
}

fn push_length_delimited(destination: &mut Vec<u8>, tag: u8, value: &[u8]) {
    destination.push(tag);
    push_varint(destination, value.len() as u64);
    destination.extend_from_slice(value);
}

fn push_varint(destination: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            destination.push(byte);
            return;
        }
        destination.push(byte | 0x80);
    }
}

fn decode_cast_message(bytes: &[u8]) -> Option<Seen> {
    let mut cursor = 0usize;
    let mut source = String::new();
    let mut destination = String::new();
    let mut namespace = String::new();
    let mut payload = String::new();

    while cursor < bytes.len() {
        let (tag, next) = read_varint(bytes, cursor)?;
        cursor = next;
        let field = tag >> 3;
        let wire_type = tag & 0x07;
        match wire_type {
            // varint: fields 1 (protocol_version) and 5 (payload_type), neither of which
            // this receiver needs to look at.
            0 => {
                let (_value, next) = read_varint(bytes, cursor)?;
                cursor = next;
            }
            2 => {
                let (length, next) = read_varint(bytes, cursor)?;
                cursor = next;
                let end = cursor.checked_add(length as usize)?;
                let slice = bytes.get(cursor..end)?;
                cursor = end;
                match field {
                    2 => source = String::from_utf8_lossy(slice).to_string(),
                    3 => destination = String::from_utf8_lossy(slice).to_string(),
                    4 => namespace = String::from_utf8_lossy(slice).to_string(),
                    6 => payload = String::from_utf8_lossy(slice).to_string(),
                    // 7 is `payload_binary`, which nothing in this session uses.
                    _ => {}
                }
            }
            _ => return None,
        }
    }
    Some(Seen { namespace, source, destination, payload })
}

fn read_varint(bytes: &[u8], mut cursor: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let byte = *bytes.get(cursor)?;
        cursor += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((value, cursor));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

// ── just enough JSON reading for the assertions ──────────────────────────────────────

/// The value of the first `"key":"…"` in `json`.
fn json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// The value of the first `"key":<number>` in `json`.
fn json_number(json: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\":");
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let end = rest.find(|character: char| !matches!(character, '0'..='9' | '.' | '-' | '+' | 'e' | 'E')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Whether `json` has a `"key":` at all.
fn json_has_key(json: &str, key: &str) -> bool { json.contains(&format!("\"{key}\":")) }

// ── the tests ────────────────────────────────────────────────────────────────────────

/// Drive one complete buffered session against a fake receiver and return what it saw.
fn buffered_session() -> (Vec<Seen>, String) {
    let receiver = FakeReceiver::start();
    let mut session = CastSession::connect(receiver.address.ip(), receiver.address.port()).expect("the session connects to the fake receiver");

    session.launch_default_media_receiver().expect("the fake receiver launches the default media receiver");
    assert!(session.launched_here(), "nothing was running, so this session started it");

    let media_url = String::from("http://192.0.2.10:54321/Beyond-Music.flac");
    session
        .load(&MediaRequest {
            content_id: media_url.clone(),
            content_type: String::from("audio/flac"),
            stream_kind: StreamKind::Buffered,
            title: String::from("beyond music"),
            duration_seconds: Some(212.5),
        })
        .expect("the fake receiver loads the media");

    session.play().expect("PLAY is answered");
    let status = session.status().expect("GET_STATUS is answered");
    assert_eq!(status.media_session_id, MEDIA_SESSION_ID);
    assert_eq!(status.player_state, "PLAYING");
    assert!((status.current_time_seconds - 12.5).abs() < 0.001, "{status:?}");

    session.pause().expect("PAUSE is answered");
    session.set_volume(0.4).expect("SET_VOLUME is answered");
    session.stop_media().expect("STOP is answered");
    // The receiver's PING is sitting in the message buffer by now; this is what answers it.
    session.pump().expect("pumping the session does not fail");
    receiver.wait_for("a PONG", |seen| seen.namespace == HEARTBEAT_NAMESPACE && seen.message_type() == "PONG");

    session.stop_app().expect("the application is stopped");
    // Dropping the session sends a CLOSE, which — unlike every request above — has no
    // reply to wait for, so the receiver has to be given the chance to read it before the
    // record is snapshotted.
    drop(session);
    receiver.wait_for("the connection CLOSE", |seen| seen.namespace == CONNECTION_NAMESPACE && seen.message_type() == "CLOSE");

    let seen = receiver.seen();
    receiver.shutdown();
    (seen, media_url)
}

#[test]
fn the_session_walks_the_castv2_handshake_in_order_on_the_right_namespaces() {
    let (seen, _url) = buffered_session();

    let sequence: Vec<(String, String, String)> = seen
        .iter()
        .filter(|message| message.namespace != HEARTBEAT_NAMESPACE)
        .map(|message| (message.namespace.clone(), message.message_type(), message.destination.clone()))
        .collect();

    let expected = [
        (CONNECTION_NAMESPACE, "CONNECT", "receiver-0"),
        (RECEIVER_NAMESPACE, "GET_STATUS", "receiver-0"),
        (RECEIVER_NAMESPACE, "LAUNCH", "receiver-0"),
        (CONNECTION_NAMESPACE, "CONNECT", TRANSPORT_ID),
        (MEDIA_NAMESPACE, "LOAD", TRANSPORT_ID),
        (MEDIA_NAMESPACE, "PLAY", TRANSPORT_ID),
        (MEDIA_NAMESPACE, "GET_STATUS", TRANSPORT_ID),
        (MEDIA_NAMESPACE, "PAUSE", TRANSPORT_ID),
        (RECEIVER_NAMESPACE, "SET_VOLUME", "receiver-0"),
        (MEDIA_NAMESPACE, "STOP", TRANSPORT_ID),
        (RECEIVER_NAMESPACE, "STOP", "receiver-0"),
        (CONNECTION_NAMESPACE, "CLOSE", "receiver-0"),
    ];
    assert_eq!(sequence.len(), expected.len(), "saw {sequence:?}");
    for (index, (namespace, message_type, destination)) in expected.iter().enumerate() {
        assert_eq!(&sequence[index].0, namespace, "message {index}");
        assert_eq!(&sequence[index].1, message_type, "message {index}");
        assert_eq!(&sequence[index].2, destination, "message {index}");
    }
}

#[test]
fn every_request_comes_from_one_sender_and_carries_an_increasing_request_id() {
    let (seen, _url) = buffered_session();

    assert!(seen.iter().all(|message| message.source == "sender-0"), "every request comes from the one virtual connection");

    let request_ids: Vec<u64> = seen.iter().filter_map(Seen::request_id).collect();
    assert!(request_ids.len() >= 7, "saw {request_ids:?}");
    for window in request_ids.windows(2) {
        assert!(window[1] > window[0], "request ids must increase: {request_ids:?}");
    }
}

#[test]
fn the_buffered_load_payload_is_the_servers_own_url_with_a_duration_and_a_music_title() {
    let (seen, url) = buffered_session();
    let load = seen.into_iter().find(|message| message.namespace == MEDIA_NAMESPACE && message.message_type() == "LOAD").expect("a LOAD was sent");

    assert_eq!(json_string(&load.payload, "sessionId").as_deref(), Some(SESSION_ID));
    assert_eq!(json_string(&load.payload, "contentId").as_deref(), Some(url.as_str()));
    assert_eq!(json_string(&load.payload, "contentType").as_deref(), Some("audio/flac"));
    assert_eq!(json_string(&load.payload, "streamType").as_deref(), Some("BUFFERED"));
    assert_eq!(json_string(&load.payload, "title").as_deref(), Some("beyond music"), "the module's own title is what the Google Home app shows");
    // `metadataType` 3 is `MusicTrackMediaMetadata`, which is what makes the speaker's own
    // controls show a track rather than a generic file.
    assert_eq!(json_number(&load.payload, "metadataType"), Some(3.0), "{}", load.payload);
    assert_eq!(json_number(&load.payload, "duration"), Some(212.5), "a buffered stream has a length and says so");
}

#[test]
fn a_live_load_payload_says_live_and_carries_no_duration() {
    let receiver = FakeReceiver::start();
    let mut session = CastSession::connect(receiver.address.ip(), receiver.address.port()).expect("the session connects");
    session.launch_default_media_receiver().expect("the receiver launches");
    session
        .load(&MediaRequest {
            content_id: String::from("http://192.0.2.10:54321/jam.wav"),
            content_type: String::from("audio/wav"),
            stream_kind: StreamKind::Live,
            title: String::from("jam"),
            duration_seconds: None,
        })
        .expect("the receiver loads a live stream");
    drop(session);

    let seen = receiver.seen();
    receiver.shutdown();
    let load = seen.into_iter().find(|message| message.namespace == MEDIA_NAMESPACE && message.message_type() == "LOAD").expect("a LOAD was sent");

    assert_eq!(json_string(&load.payload, "streamType").as_deref(), Some("LIVE"));
    assert_eq!(json_string(&load.payload, "contentType").as_deref(), Some("audio/wav"));
    assert!(!json_has_key(&load.payload, "duration"), "a live stream has no length, so the field is absent: {}", load.payload);
}

#[test]
fn a_session_that_attached_to_a_running_receiver_did_not_launch_it() {
    let receiver = FakeReceiver::start();
    {
        // The first session finds nothing running and launches the application.
        let mut first = CastSession::connect(receiver.address.ip(), receiver.address.port()).expect("the first session connects");
        first.launch_default_media_receiver().expect("the first session launches");
        assert!(first.launched_here(), "nothing was running, so this session started it");
    }
    {
        // The second finds it already running and attaches, which is what stops the exit
        // path from stopping an application somebody else's music is coming out of.
        let mut second = CastSession::connect(receiver.address.ip(), receiver.address.port()).expect("the second session connects");
        second.launch_default_media_receiver().expect("the second session attaches");
        assert!(!second.launched_here(), "the application was already running, so this session did not start it");
    }

    let seen = receiver.seen();
    receiver.shutdown();
    let launches = seen.iter().filter(|message| message.namespace == RECEIVER_NAMESPACE && message.message_type() == "LAUNCH").count();
    assert_eq!(launches, 1, "the application was launched exactly once");
}

#[test]
fn a_seek_names_the_media_session_and_the_position() {
    let receiver = FakeReceiver::start();
    let mut session = CastSession::connect(receiver.address.ip(), receiver.address.port()).expect("the session connects");
    session.launch_default_media_receiver().expect("the receiver launches");
    session
        .load(&MediaRequest {
            content_id: String::from("http://192.0.2.10:54321/song.flac"),
            content_type: String::from("audio/flac"),
            stream_kind: StreamKind::Buffered,
            title: String::from("song"),
            duration_seconds: Some(90.0),
        })
        .expect("the receiver loads");
    session.seek(30.0).expect("SEEK is answered");
    drop(session);

    let seen = receiver.seen();
    receiver.shutdown();
    let seek = seen.into_iter().find(|message| message.namespace == MEDIA_NAMESPACE && message.message_type() == "SEEK").expect("a SEEK was sent");
    assert_eq!(json_number(&seek.payload, "mediaSessionId"), Some(f64::from(MEDIA_SESSION_ID)));
    assert_eq!(json_number(&seek.payload, "currentTime"), Some(30.0));
}

#[test]
#[ignore = "a measurement, not an assertion: run with --ignored --nocapture"]
fn measure_round_trip_latency() {
    let receiver = FakeReceiver::start();
    let connect_started = Instant::now();
    let mut session = CastSession::connect(receiver.address.ip(), receiver.address.port()).expect("connect");
    let connect_elapsed = connect_started.elapsed();

    let launch_started = Instant::now();
    session.launch_default_media_receiver().expect("launch");
    let launch_elapsed = launch_started.elapsed();

    let load_started = Instant::now();
    session
        .load(&MediaRequest {
            content_id: String::from("http://192.0.2.10:1/x.flac"),
            content_type: String::from("audio/flac"),
            stream_kind: StreamKind::Buffered,
            title: String::from("x"),
            duration_seconds: Some(60.0),
        })
        .expect("load");
    let load_elapsed = load_started.elapsed();

    let mut pause_total = Duration::ZERO;
    let rounds = 50;
    for _ in 0..rounds {
        let started = Instant::now();
        session.pause().expect("pause");
        pause_total += started.elapsed();
        session.play().expect("play");
    }
    println!("connect  {:?}", connect_elapsed);
    println!("launch   {:?}", launch_elapsed);
    println!("load     {:?}", load_elapsed);
    println!("pause    {:?} (mean of {rounds})", pause_total / rounds);
    drop(session);
    receiver.shutdown();
}
