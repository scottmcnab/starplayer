//! The media server's HTTP behaviour, over a real socket.
//!
//! `MediaServer`'s `Range` handling is what gives a Cast receiver seek and a clean end, so
//! it is tested against a real TCP connection rather than against the parser alone: the
//! status line, the headers and the body bytes all have to agree, and `tiny_http`'s own
//! choice of transfer encoding is part of what is being checked — a response that silently
//! switched to chunked would lose the `Content-Length` a receiver seeks with.
//!
//! No device, no multicast and no network beyond loopback.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpStream};
use std::time::Duration;

use starplayer_cast::MediaServer;

/// A distinctive asset: 4 KiB whose every byte says where it is.
fn asset() -> Vec<u8> { (0..4_096u32).map(|index| (index % 251) as u8).collect() }

/// One raw HTTP/1.1 request, and the whole response as bytes.
fn request(address: std::net::SocketAddr, raw: &str) -> Vec<u8> {
    let mut socket = TcpStream::connect(address).expect("the media server accepts connections");
    socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    socket.write_all(raw.as_bytes()).unwrap();
    socket.flush().unwrap();
    let mut response = Vec::new();
    // The server closes the connection because every request here says `Connection: close`,
    // so "read to end" is exactly "read the whole response".
    socket.read_to_end(&mut response).expect("the response arrives");
    response
}

/// Split a raw response into its header block and its body.
fn split(response: &[u8]) -> (String, Vec<u8>) {
    let separator = response.windows(4).position(|window| window == b"\r\n\r\n").expect("a response has a header/body separator");
    (String::from_utf8_lossy(&response[..separator]).to_string(), response[separator + 4..].to_vec())
}

/// Whether the header block carries `name: value`, case-insensitively on the name.
fn has_header(headers: &str, name: &str, value: &str) -> bool {
    headers.lines().any(|line| {
        let Some((field, field_value)) = line.split_once(':') else { return false };
        field.trim().eq_ignore_ascii_case(name) && field_value.trim() == value
    })
}

fn status_line(headers: &str) -> String { headers.lines().next().unwrap_or_default().trim().to_string() }

/// A server serving [`asset`] under `/song.flac`, plus its address.
fn served() -> (MediaServer, std::net::SocketAddr, Vec<u8>) {
    let mut server = MediaServer::bind_for(IpAddr::V4(Ipv4Addr::LOCALHOST)).expect("a media server binds on loopback");
    let bytes = asset();
    server.serve_bytes("/song.flac", "audio/flac", bytes.clone());
    let address = server.address();
    (server, address, bytes)
}

#[test]
fn a_request_with_no_range_gets_the_whole_asset_with_a_content_length() {
    let (server, address, bytes) = served();
    let response = request(address, "GET /song.flac HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let (headers, body) = split(&response);

    assert_eq!(status_line(&headers), "HTTP/1.1 200 OK");
    assert!(has_header(&headers, "Accept-Ranges", "bytes"), "{headers}");
    assert!(has_header(&headers, "Content-Type", "audio/flac"), "{headers}");
    assert!(has_header(&headers, "Content-Length", &bytes.len().to_string()), "{headers}");
    assert!(!headers.to_lowercase().contains("transfer-encoding"), "a receiver seeks with Content-Length, so this must not be chunked:\n{headers}");
    assert_eq!(body, bytes);
    server.shutdown();
}

#[test]
fn each_of_the_three_range_forms_gets_the_right_bytes_back() {
    let (server, address, bytes) = served();

    let cases: [(&str, usize, usize); 3] = [("bytes=0-99", 0, 99), ("bytes=4000-", 4_000, 4_095), ("bytes=-100", 3_996, 4_095)];
    for (range, first, last) in cases {
        let raw = format!("GET /song.flac HTTP/1.1\r\nHost: x\r\nRange: {range}\r\nConnection: close\r\n\r\n");
        let response = request(address, &raw);
        let (headers, body) = split(&response);

        assert_eq!(status_line(&headers), "HTTP/1.1 206 Partial Content", "{range}:\n{headers}");
        assert!(has_header(&headers, "Accept-Ranges", "bytes"), "{range}:\n{headers}");
        assert!(has_header(&headers, "Content-Range", &format!("bytes {first}-{last}/{}", bytes.len())), "{range}:\n{headers}");
        assert!(has_header(&headers, "Content-Length", &(last - first + 1).to_string()), "{range}:\n{headers}");
        assert_eq!(body, bytes[first..=last], "{range} returned the wrong slice");
    }
    server.shutdown();
}

#[test]
fn a_range_end_past_the_asset_clamps_to_its_last_byte() {
    let (server, address, bytes) = served();
    let response = request(address, "GET /song.flac HTTP/1.1\r\nHost: x\r\nRange: bytes=4090-9999\r\nConnection: close\r\n\r\n");
    let (headers, body) = split(&response);

    assert_eq!(status_line(&headers), "HTTP/1.1 206 Partial Content");
    assert!(has_header(&headers, "Content-Range", &format!("bytes 4090-4095/{}", bytes.len())), "{headers}");
    assert_eq!(body, bytes[4_090..]);
    server.shutdown();
}

#[test]
fn a_range_entirely_past_the_asset_is_refused_with_416_and_no_body() {
    let (server, address, bytes) = served();
    let response = request(address, "GET /song.flac HTTP/1.1\r\nHost: x\r\nRange: bytes=5000-6000\r\nConnection: close\r\n\r\n");
    let (headers, body) = split(&response);

    assert_eq!(status_line(&headers), "HTTP/1.1 416 Range Not Satisfiable");
    assert!(has_header(&headers, "Content-Range", &format!("bytes */{}", bytes.len())), "{headers}");
    assert!(body.is_empty(), "a 416 carries no body, got {} bytes", body.len());
    server.shutdown();
}

#[test]
fn a_malformed_or_multi_range_is_answered_with_the_whole_asset() {
    let (server, address, bytes) = served();
    for range in ["bytes=0-1,4-5", "bytes=abc", "items=0-10", "nonsense"] {
        let raw = format!("GET /song.flac HTTP/1.1\r\nHost: x\r\nRange: {range}\r\nConnection: close\r\n\r\n");
        let response = request(address, &raw);
        let (headers, body) = split(&response);
        assert_eq!(status_line(&headers), "HTTP/1.1 200 OK", "{range}:\n{headers}");
        assert_eq!(body, bytes, "{range} should have been ignored, not honoured");
    }
    server.shutdown();
}

#[test]
fn head_answers_with_the_same_headers_and_no_body_for_both_200_and_206() {
    let (server, address, bytes) = served();

    let response = request(address, "HEAD /song.flac HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let (headers, body) = split(&response);
    assert_eq!(status_line(&headers), "HTTP/1.1 200 OK");
    assert!(has_header(&headers, "Content-Length", &bytes.len().to_string()), "{headers}");
    assert!(has_header(&headers, "Accept-Ranges", "bytes"), "{headers}");
    assert!(has_header(&headers, "Content-Type", "audio/flac"), "{headers}");
    assert!(body.is_empty(), "a HEAD carries no body");

    let response = request(address, "HEAD /song.flac HTTP/1.1\r\nHost: x\r\nRange: bytes=10-19\r\nConnection: close\r\n\r\n");
    let (headers, body) = split(&response);
    assert_eq!(status_line(&headers), "HTTP/1.1 206 Partial Content");
    assert!(has_header(&headers, "Content-Range", &format!("bytes 10-19/{}", bytes.len())), "{headers}");
    assert!(has_header(&headers, "Content-Length", "10"), "{headers}");
    assert!(body.is_empty(), "a HEAD carries no body");
    server.shutdown();
}

#[test]
fn an_unknown_path_is_one_line_of_plain_text_and_a_404() {
    let (server, address, _bytes) = served();
    let response = request(address, "GET /nothing-here.flac HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let (headers, body) = split(&response);

    assert_eq!(status_line(&headers), "HTTP/1.1 404 Not Found");
    let text = String::from_utf8_lossy(&body);
    assert_eq!(text.lines().count(), 1, "{text:?}");
    server.shutdown();
}

#[test]
fn a_url_names_the_bound_address_and_never_names_nothing() {
    let (server, address, _bytes) = served();
    assert_eq!(server.url("song.flac"), format!("http://{address}/song.flac"));
    assert_eq!(server.url("/song.flac"), format!("http://{address}/song.flac"));
    server.shutdown();
}

#[test]
fn a_live_path_is_answered_chunked_and_ends_when_the_channel_closes() {
    let mut server = MediaServer::bind_for(IpAddr::V4(Ipv4Addr::LOCALHOST)).expect("a media server binds on loopback");
    let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
    server.serve_stream("/live.wav", "audio/wav", receiver);
    let address = server.address();

    let feeder = std::thread::spawn(move || {
        for index in 0u8..4 {
            if sender.send(vec![index; 16]).is_err() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    // A `Range` on a live path is ignored: there is no addressable past to seek into.
    let response = request(address, "GET /live.wav HTTP/1.1\r\nHost: x\r\nRange: bytes=0-10\r\nConnection: close\r\n\r\n");
    let (headers, body) = split(&response);
    assert_eq!(status_line(&headers), "HTTP/1.1 200 OK", "{headers}");
    assert!(has_header(&headers, "Transfer-Encoding", "chunked"), "a stream of unknown length is chunked:\n{headers}");
    assert!(!headers.to_lowercase().contains("content-length"), "{headers}");

    // Decode the chunked body: `<hex length>\r\n<data>\r\n`, ending with a zero-length
    // chunk. Done by hand rather than with an HTTP client, so the test has no dependency
    // of its own.
    let mut decoded = Vec::new();
    let mut rest = body.as_slice();
    loop {
        let line_end = rest.windows(2).position(|window| window == b"\r\n").expect("a chunk starts with its length");
        let length = usize::from_str_radix(String::from_utf8_lossy(&rest[..line_end]).trim(), 16).expect("a chunk length is hexadecimal");
        rest = &rest[line_end + 2..];
        if length == 0 {
            break;
        }
        decoded.extend_from_slice(&rest[..length]);
        rest = &rest[length + 2..];
    }
    assert_eq!(decoded.len(), 4 * 16);
    assert_eq!(&decoded[0..16], &[0u8; 16]);
    assert_eq!(&decoded[48..64], &[3u8; 16]);

    feeder.join().unwrap();
    server.shutdown();
}

#[test]
fn a_live_stream_can_only_be_claimed_once() {
    let mut server = MediaServer::bind_for(IpAddr::V4(Ipv4Addr::LOCALHOST)).expect("a media server binds on loopback");
    let (sender, receiver) = std::sync::mpsc::channel::<Vec<u8>>();
    server.serve_stream("/live.wav", "audio/wav", receiver);
    let address = server.address();
    drop(sender);

    let first = request(address, "GET /live.wav HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(status_line(&split(&first).0), "HTTP/1.1 200 OK");
    let second = request(address, "GET /live.wav HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(status_line(&split(&second).0), "HTTP/1.1 404 Not Found", "a stream has no second copy");
    server.shutdown();
}
