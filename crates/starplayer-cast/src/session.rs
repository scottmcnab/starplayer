//! [`CastSession`] — one CASTv2 conversation with one Cast device.
//!
//! # Why this wraps `rust_cast` instead of exposing it
//!
//! `rust_cast` speaks CASTv2 well and has a protobuf definition straight from Chromium's
//! Open Screen mirror, but it is not the only crate that could: `cast-sender` is the other
//! live implementation, and this one has changed hands before. Every `rust_cast` type stops
//! at this module's boundary — [`MediaStatus`], [`MediaRequest`] and [`CastError`] are
//! ours, made of `String`s and `f32`s — so replacing the client would be a rewrite of one
//! file and nothing above it.
//!
//! # Why the TLS stack is built here rather than by `rust_cast`
//!
//! `rust_cast::CastDevice::connect_without_host_verification` builds its own `TcpStream`
//! and keeps it private, and a `TcpStream` this crate cannot reach is a `TcpStream` with no
//! read timeout: a speaker that stops answering mid-request would hang the CLI for ever
//! rather than reporting a timeout. So the same stack is assembled here — `rustls` client,
//! `rust_cast::message_manager::MessageManager`, the four channel helpers — over a socket
//! this module holds a `try_clone` of and sets the timeout on.
//!
//! # Host verification is deliberately off
//!
//! A Cast device serves a certificate for its *own device name*, issued by a Google device
//! CA, and this crate connects to it by IP address discovered over mDNS. There is no name
//! to verify that certificate against, and pinning Google's device CA would be a second
//! trust store to maintain for no gain against an attacker who is already on the LAN.
//! This is the one place the crate trusts the local network, it is what every other Cast
//! sender does, and nothing secret travels over the link: the payloads are a media URL and
//! transport commands for audio that is being served unencrypted over HTTP on the same LAN
//! anyway.
//!
//! # Threads
//!
//! - **The control thread** — the caller's — is the only one that ever *reads* the socket,
//!   and only while waiting for the answer to a request it just made.
//! - **The heartbeat thread** sends `PING` every [`HEARTBEAT_INTERVAL`] and never reads.
//!   A receiver drops a connection that misses two of those. It exits on a flag and is
//!   joined by [`CastSession`]'s `Drop`, never leaked.
//!
//! Incoming `PING`s are answered with `PONG` by [`CastSession::pump`], which the caller's
//! control loop is expected to call about once a second.

use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use rust_cast::channels::connection::ConnectionChannel;
use rust_cast::channels::heartbeat::{HeartbeatChannel, HeartbeatResponse};
use rust_cast::channels::media::{Media, MediaChannel, Metadata, MusicTrackMediaMetadata, StreamType};
use rust_cast::channels::receiver::{CastDeviceApp, ReceiverChannel};
use rust_cast::errors::Error as RustCastError;
use rust_cast::message_manager::MessageManager;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use crate::{CastError, DEFAULT_MEDIA_RECEIVER_APP_ID};

/// The TLS stream every channel is built over.
type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// The sender identifier this crate announces itself as. `rust_cast`'s own default, and
/// what every Cast sender uses for its first virtual connection.
const SENDER_ID: &str = "sender-0";

/// The platform receiver's identifier: the device itself, before any application runs.
const RECEIVER_ID: &str = "receiver-0";

/// How long a request waits for its reply before the socket read gives up.
///
/// Generous, because a `LAUNCH` on a cold speaker really can take several seconds; short
/// enough that a device which has gone away reports a timeout rather than hanging the CLI.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// How long [`CastSession::pump`] waits for an unsolicited message before concluding there
/// is none.
///
/// Deliberately short: `pump` is called from a control loop that has other work. A
/// CASTv2 message is a single small TLS record, so the length prefix and the payload come
/// out of rustls's plaintext buffer together and a timeout between the two — the one way
/// this could desynchronise the framing — does not arise in practice.
const PUMP_TIMEOUT: Duration = Duration::from_millis(200);

/// How long to wait for the TCP connection itself.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the heartbeat thread pings. The receiver drops a connection that misses two.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// How long the heartbeat thread sleeps between checks of its stop flag, so that dropping
/// a session is prompt rather than waiting out a whole [`HEARTBEAT_INTERVAL`].
const HEARTBEAT_POLL: Duration = Duration::from_millis(100);

/// How the receiver should treat the stream: a file it can seek in, or a tap it joins.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StreamKind {
    /// A complete, `Range`-addressable asset: the receiver gets seek, duration and a clean
    /// end.
    Buffered,
    /// An endless chunked stream with no duration and no addressable past.
    Live,
}

impl StreamKind {
    fn to_rust_cast(self) -> StreamType {
        match self {
            StreamKind::Buffered => StreamType::Buffered,
            StreamKind::Live => StreamType::Live,
        }
    }
}

/// What to tell the receiver to play.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaRequest {
    /// The URL the receiver fetches. Must be reachable from the *speaker*, so never
    /// `127.0.0.1`.
    pub content_id: String,
    /// `audio/flac` or `audio/wav`.
    pub content_type: String,
    /// [`StreamKind::Buffered`] for a pre-render, [`StreamKind::Live`] for a live stream.
    pub stream_kind: StreamKind,
    /// What the Google Home app and the speaker's own controls display.
    pub title: String,
    /// Length in seconds, for a buffered stream. Absent for a live one — a live stream has
    /// no length and claiming one makes the receiver's own progress bar lie.
    pub duration_seconds: Option<f32>,
}

/// What the receiver says it is doing.
///
/// Ours, not `rust_cast`'s: the player state and the idle reason are the protocol's own
/// strings (`PLAYING`, `PAUSED`, `IDLE`, `BUFFERING`; `FINISHED`, `CANCELLED`,
/// `INTERRUPTED`, `ERROR`), which is exactly what a status line wants to print.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaStatus {
    /// `IDLE`, `PLAYING`, `BUFFERING` or `PAUSED`.
    pub player_state: String,
    /// Position in the stream, in seconds, as the receiver last reported it.
    pub current_time_seconds: f32,
    /// Why the receiver went idle, when it has.
    pub idle_reason: Option<String>,
    /// The receiver's identifier for this media session.
    pub media_session_id: i32,
}

/// The application the session is talking to, once one is running.
#[derive(Clone, Debug)]
struct Application {
    session_id: String,
    transport_id: String,
}

/// One connection to one Cast device.
pub struct CastSession {
    /// A clone of the socket underneath the TLS stream, held only to change its read
    /// timeout. Never read from or written to directly.
    socket: TcpStream,
    message_manager: Arc<MessageManager<TlsStream>>,
    connection: ConnectionChannel<'static, TlsStream>,
    heartbeat: HeartbeatChannel<'static, TlsStream>,
    receiver: ReceiverChannel<'static, TlsStream>,
    media: MediaChannel<'static, TlsStream>,
    application: Option<Application>,
    media_session_id: Option<i32>,
    launched_here: bool,
    heartbeat_running: Arc<AtomicBool>,
    heartbeat_thread: Option<JoinHandle<()>>,
}

impl CastSession {
    /// Open a CASTv2 connection to `address:port` and say hello.
    ///
    /// Starts the heartbeat thread, so the connection stays up from here until the session
    /// is dropped.
    pub fn connect(address: IpAddr, port: u16) -> Result<CastSession, CastError> {
        let socket_address = SocketAddr::new(address, port);
        let socket = TcpStream::connect_timeout(&socket_address, CONNECT_TIMEOUT)
            .map_err(|error| CastError::Connect(format!("{socket_address}: {error}")))?;
        socket.set_read_timeout(Some(REQUEST_TIMEOUT)).map_err(|error| CastError::Connect(format!("could not set a read timeout: {error}")))?;
        socket.set_write_timeout(Some(REQUEST_TIMEOUT)).map_err(|error| CastError::Connect(format!("could not set a write timeout: {error}")))?;
        // Control messages are tiny and latency matters more than packing them.
        let _ = socket.set_nodelay(true);
        let timeout_handle = socket.try_clone().map_err(|error| CastError::Connect(format!("could not duplicate the socket handle: {error}")))?;

        // See this module's doc for why the certificate is not verified.
        let config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(rust_cast::NoCertificateVerification))
            .with_no_client_auth();
        // A Cast device is addressed by IP, so this is an `ServerName::IpAddress` and no
        // SNI is sent — which is what the device expects.
        let server_name = ServerName::try_from(address.to_string()).map_err(|error| CastError::Connect(format!("{address} is not a usable server name: {error}")))?;
        let connection = ClientConnection::new(Arc::new(config), server_name).map_err(|error| CastError::Connect(format!("TLS setup failed: {error}")))?;
        let stream = StreamOwned::new(connection, socket);

        let message_manager = Arc::new(MessageManager::new(stream));
        let session = CastSession {
            socket: timeout_handle,
            connection: ConnectionChannel::new(SENDER_ID, Arc::clone(&message_manager)),
            heartbeat: HeartbeatChannel::new(SENDER_ID, RECEIVER_ID, Arc::clone(&message_manager)),
            receiver: ReceiverChannel::new(SENDER_ID, RECEIVER_ID, Arc::clone(&message_manager)),
            media: MediaChannel::new(SENDER_ID, Arc::clone(&message_manager)),
            message_manager,
            application: None,
            media_session_id: None,
            launched_here: false,
            heartbeat_running: Arc::new(AtomicBool::new(true)),
            heartbeat_thread: None,
        };

        session.connection.connect(RECEIVER_ID).map_err(|error| CastError::Connect(format!("the device refused the virtual connection: {error}")))?;

        let mut session = session;
        session.start_heartbeat();
        Ok(session)
    }

    /// Launch the Default Media Receiver, or attach to it if it is already running.
    ///
    /// Attaching matters for the exit path: a session that found the receiver already
    /// running did not start it, and stopping the *application* on the way out would take
    /// away whatever the household was already listening to. [`CastSession::launched_here`]
    /// reports which happened.
    pub fn launch_default_media_receiver(&mut self) -> Result<(), CastError> {
        let status = self.receiver.get_status().map_err(|error| CastError::Launch(format!("could not read the receiver's status: {error}")))?;
        let running = status.applications.into_iter().find(|application| application.app_id == DEFAULT_MEDIA_RECEIVER_APP_ID);

        let (session_id, transport_id, launched_here) = match running {
            Some(application) => (application.session_id, application.transport_id, false),
            None => {
                let application = self
                    .receiver
                    .launch_app(&CastDeviceApp::DefaultMediaReceiver)
                    .map_err(|error| CastError::Launch(format!("{DEFAULT_MEDIA_RECEIVER_APP_ID}: {error}")))?;
                (application.session_id, application.transport_id, true)
            }
        };

        // A second virtual connection, to the application rather than to the platform.
        // Without it the receiver ignores everything on the media namespace.
        self.connection
            .connect(transport_id.clone())
            .map_err(|error| CastError::Launch(format!("the receiver application refused the virtual connection: {error}")))?;
        self.application = Some(Application { session_id, transport_id });
        self.launched_here = launched_here;
        Ok(())
    }

    /// Whether [`CastSession::launch_default_media_receiver`] started the application, as
    /// opposed to attaching to one that was already running.
    pub fn launched_here(&self) -> bool { self.launched_here }

    /// Tell the receiver to load and play `media`.
    pub fn load(&mut self, media: &MediaRequest) -> Result<(), CastError> {
        let application = self.application()?;
        let request = Media {
            content_id: media.content_id.clone(),
            stream_type: media.stream_kind.to_rust_cast(),
            content_type: media.content_type.clone(),
            metadata: Some(Metadata::MusicTrack(MusicTrackMediaMetadata { title: Some(media.title.clone()), ..MusicTrackMediaMetadata::default() })),
            duration: media.duration_seconds,
        };
        let status = self
            .media
            .load(application.transport_id.clone(), application.session_id.clone(), &request)
            .map_err(|error| CastError::Load(format!("{}: {error}", media.content_id)))?;
        self.media_session_id = status.entries.first().map(|entry| entry.media_session_id);
        if self.media_session_id.is_none() {
            return Err(CastError::Load(String::from("the receiver accepted the LOAD but reported no media session")));
        }
        Ok(())
    }

    /// Resume playback of the loaded media.
    pub fn play(&mut self) -> Result<(), CastError> {
        let (transport_id, media_session_id) = self.media_target()?;
        self.media.play(transport_id, media_session_id).map(|_| ()).map_err(|error| CastError::Protocol(format!("PLAY: {error}")))
    }

    /// Pause playback without unloading the media.
    pub fn pause(&mut self) -> Result<(), CastError> {
        let (transport_id, media_session_id) = self.media_target()?;
        self.media.pause(transport_id, media_session_id).map(|_| ()).map_err(|error| CastError::Protocol(format!("PAUSE: {error}")))
    }

    /// Seek to `seconds` from the start of the stream.
    pub fn seek(&mut self, seconds: f32) -> Result<(), CastError> {
        let (transport_id, media_session_id) = self.media_target()?;
        self.media
            .seek(transport_id, media_session_id, Some(seconds.max(0.0)), None)
            .map(|_| ())
            .map_err(|error| CastError::Protocol(format!("SEEK: {error}")))
    }

    /// Stop the media. The media session is invalidated; the application keeps running.
    pub fn stop_media(&mut self) -> Result<(), CastError> {
        let (transport_id, media_session_id) = self.media_target()?;
        let result = self.media.stop(transport_id, media_session_id).map(|_| ()).map_err(|error| CastError::Protocol(format!("STOP: {error}")));
        self.media_session_id = None;
        result
    }

    /// Stop the receiver application itself, returning the device to its backdrop.
    pub fn stop_app(&mut self) -> Result<(), CastError> {
        let Some(application) = self.application.take() else { return Ok(()) };
        self.media_session_id = None;
        self.receiver.stop_app(application.session_id).map_err(|error| CastError::Protocol(format!("STOP application: {error}")))
    }

    /// Set the **device's** volume, 0.0 to 1.0.
    ///
    /// The speaker's own volume, not the engine's master gain: it is the one the listener
    /// will reach for on the device or in the Google Home app, and two volume controls that
    /// disagree with each other is a bug report.
    pub fn set_volume(&mut self, level: f32) -> Result<(), CastError> {
        self.receiver.set_volume(level.clamp(0.0, 1.0)).map(|_| ()).map_err(|error| CastError::Protocol(format!("SET_VOLUME: {error}")))
    }

    /// Mute or unmute the device.
    pub fn set_mute(&mut self, muted: bool) -> Result<(), CastError> {
        self.receiver.set_volume(muted).map(|_| ()).map_err(|error| CastError::Protocol(format!("SET_VOLUME (mute): {error}")))
    }

    /// Ask the receiver what it is doing.
    pub fn status(&mut self) -> Result<MediaStatus, CastError> {
        let application = self.application()?;
        let status = self
            .media
            .get_status(application.transport_id.clone(), self.media_session_id)
            .map_err(|error| CastError::Protocol(format!("GET_STATUS: {error}")))?;
        let entry = status
            .entries
            .into_iter()
            .find(|entry| self.media_session_id.is_none_or(|wanted| entry.media_session_id == wanted))
            .ok_or_else(|| CastError::Protocol(String::from("the receiver reported no media session")))?;
        Ok(MediaStatus {
            player_state: entry.player_state.to_string(),
            current_time_seconds: entry.current_time.unwrap_or(0.0),
            idle_reason: entry.idle_reason.map(|reason| format!("{reason:?}").to_uppercase()),
            media_session_id: entry.media_session_id,
        })
    }

    /// Answer anything the receiver sent unprompted — in practice, its `PING`.
    ///
    /// Reads with a short timeout until nothing more is waiting, so a control loop can call
    /// it once a second without ever blocking for long. Messages that are neither a
    /// heartbeat nor addressed to a request go back into `rust_cast`'s own buffer, which is
    /// where an out-of-band `MEDIA_STATUS` broadcast belongs anyway.
    pub fn pump(&mut self) -> Result<(), CastError> {
        self.set_read_timeout(PUMP_TIMEOUT)?;
        let outcome = loop {
            match self.message_manager.receive() {
                Ok(message) => {
                    if self.heartbeat.can_handle(&message)
                        && matches!(self.heartbeat.parse(&message), Ok(HeartbeatResponse::Ping))
                        && let Err(error) = self.heartbeat.pong()
                    {
                        break Err(CastError::Protocol(format!("could not answer a PING: {error}")));
                    }
                }
                Err(error) if is_would_block(&error) => break Ok(()),
                Err(error) => break Err(CastError::Protocol(format!("reading from the device failed: {error}"))),
            }
        };
        self.set_read_timeout(REQUEST_TIMEOUT)?;
        outcome
    }

    /// The receiver application, or an error saying it has not been launched.
    fn application(&self) -> Result<&Application, CastError> {
        self.application.as_ref().ok_or_else(|| CastError::Launch(String::from("no receiver application is running; launch one first")))
    }

    /// The `(transport_id, media_session_id)` a transport command needs.
    fn media_target(&self) -> Result<(String, i32), CastError> {
        let application = self.application()?;
        let media_session_id = self.media_session_id.ok_or_else(|| CastError::Load(String::from("no media is loaded")))?;
        Ok((application.transport_id.clone(), media_session_id))
    }

    fn set_read_timeout(&self, timeout: Duration) -> Result<(), CastError> {
        self.socket.set_read_timeout(Some(timeout)).map_err(|error| CastError::Io(format!("could not set the socket read timeout: {error}")))
    }

    /// Start the thread that keeps the connection alive.
    ///
    /// It only ever *sends*. Reading is the control thread's job, and two threads blocking
    /// in `read` on one socket would mean one of them holding `rust_cast`'s stream lock
    /// while the other wanted it to write.
    fn start_heartbeat(&mut self) {
        let running = Arc::clone(&self.heartbeat_running);
        let heartbeat = HeartbeatChannel::new(SENDER_ID, RECEIVER_ID, Arc::clone(&self.message_manager));
        self.heartbeat_thread = Some(std::thread::spawn(move || {
            let mut since_last_ping = Duration::ZERO;
            while running.load(Ordering::SeqCst) {
                std::thread::sleep(HEARTBEAT_POLL);
                since_last_ping += HEARTBEAT_POLL;
                if since_last_ping < HEARTBEAT_INTERVAL {
                    continue;
                }
                since_last_ping = Duration::ZERO;
                if heartbeat.ping().is_err() {
                    // The connection is gone. The control thread will see it on its next
                    // request and report it there, with the context to say what failed.
                    break;
                }
            }
        }));
    }
}

impl std::fmt::Debug for CastSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CastSession")
            .field("peer", &self.socket.peer_addr().ok())
            .field("application", &self.application)
            .field("media_session_id", &self.media_session_id)
            .field("launched_here", &self.launched_here)
            .finish()
    }
}

impl Drop for CastSession {
    fn drop(&mut self) {
        self.heartbeat_running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.heartbeat_thread.take() {
            let _ = thread.join();
        }
        // Best-effort: a `CLOSE` lets the receiver forget this sender immediately rather
        // than waiting for two missed heartbeats.
        let _ = self.connection.disconnect(RECEIVER_ID);
    }
}

/// Whether a `rust_cast` error is really "the read timed out with nothing waiting".
///
/// Platforms disagree about which kind a socket read timeout produces — `WouldBlock` on
/// Unix, `TimedOut` on Windows — so both count.
fn is_would_block(error: &RustCastError) -> bool {
    match error {
        RustCastError::Io(io_error) => matches!(io_error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
        RustCastError::Timeout(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_kind_carries_the_protocols_own_spelling() {
        assert_eq!(StreamKind::Buffered.to_rust_cast().to_string(), "BUFFERED");
        assert_eq!(StreamKind::Live.to_rust_cast().to_string(), "LIVE");
    }

    #[test]
    fn a_socket_read_timeout_is_recognised_on_either_platforms_spelling() {
        assert!(is_would_block(&RustCastError::Io(std::io::Error::from(ErrorKind::WouldBlock))));
        assert!(is_would_block(&RustCastError::Io(std::io::Error::from(ErrorKind::TimedOut))));
        assert!(!is_would_block(&RustCastError::Io(std::io::Error::from(ErrorKind::ConnectionReset))));
        assert!(!is_would_block(&RustCastError::Internal(String::from("nope"))));
    }

    #[test]
    fn connecting_to_a_closed_port_reports_a_connect_error_rather_than_hanging() {
        // Port 1 on loopback: nothing listens there, and the refusal is immediate.
        let error = CastSession::connect(IpAddr::from([127, 0, 0, 1]), 1).unwrap_err();
        assert!(matches!(error, CastError::Connect(_)), "{error}");
    }
}
