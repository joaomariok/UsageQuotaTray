//! The real transport against a one-shot server on the loopback interface (no real
//! network host). Plain HTTP here only because the loopback server has no certificate;
//! the host client still refuses anything but HTTPS.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use uqt_core::http::{HttpError, HttpRequest, Transport, UreqTransport};

/// Serves one canned response and hands back the request head it received.
fn serve_once(response: &'static str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/api/usage", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
            head.push_str(&line);
        }
        let mut stream = reader.into_inner();
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
        let _ = stream.read(&mut [0; 1]);
        head
    });
    (url, handle)
}

fn get(url: &str) -> HttpRequest {
    HttpRequest {
        method: "GET".into(),
        url: url.into(),
        headers: vec![
            ("Authorization".into(), "Bearer test-token".into()),
            ("anthropic-beta".into(), "oauth-2025-04-20".into()),
        ],
    }
}

#[test]
fn error_statuses_come_back_as_responses_with_headers_and_body() {
    let (url, server) = serve_once(
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
    );
    let response = UreqTransport::without_proxy()
        .execute(&get(&url))
        .expect("a 429 is a response, not a transport error");
    assert_eq!(response.status, 429);
    assert_eq!(response.header("retry-after"), Some("120"));
    assert_eq!(response.body, b"{}");

    let head = server.join().unwrap().to_ascii_lowercase();
    assert!(head.starts_with("get /api/usage http/1.1"), "{head}");
    assert!(head.contains("authorization: bearer test-token"), "{head}");
    assert!(head.contains("anthropic-beta: oauth-2025-04-20"), "{head}");
    assert!(head.contains("user-agent: usagequotatray/"), "{head}");
}

#[test]
fn redirects_are_returned_not_followed_so_the_allowlist_cannot_be_bypassed() {
    let (url, server) = serve_once(
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    let response = UreqTransport::without_proxy().execute(&get(&url)).unwrap();
    assert_eq!(response.status, 302);
    server.join().unwrap();
}

#[test]
fn an_unreachable_server_is_a_transport_error() {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let result = UreqTransport::without_proxy().execute(&get(&format!("http://127.0.0.1:{port}/")));
    assert!(matches!(result, Err(HttpError::Transport(_))), "{result:?}");
}
