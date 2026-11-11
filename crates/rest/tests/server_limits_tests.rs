//! server_limits_tests.rs — The HTTP server must bound what a peer can make it allocate or read.
//! ==============================================================================================
//!
//! Layer:
//!     Test Suite / REST Server Hardening

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use pdfcraft_rest::executor::{BlockingExecutor, ExecutorConfig};
use pdfcraft_rest::routes::RestService;
use pdfcraft_rest::server::{HttpServer, ServerConfig};
use pdfcraft_rest::storage::InMemoryJobRepository;
use pdfcraft_sdk::AutomationEngineAdapter;

fn start() -> HttpServer {
    let engine: Arc<dyn pdfcraft_sdk::OperationExecutor> = Arc::new(AutomationEngineAdapter::default());
    let executor = Arc::new(BlockingExecutor::new(engine, ExecutorConfig::default()));
    let service = Arc::new(RestService::new(executor, Arc::new(InMemoryJobRepository::new()), None));
    HttpServer::start(service, ServerConfig { bind_addr: "127.0.0.1:0".parse().unwrap(), allow_public_bind: false }).unwrap()
}

fn raw(port: u16, request: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn oversized_content_length_is_rejected_before_allocation() {
    // Regression: the body buffer was sized straight from the peer's Content-Length header.
    let server = start();
    let resp = raw(server.port(), "POST /v1/merged-pdf HTTP/1.1\r\nContent-Length: 99999999999\r\nConnection: close\r\n\r\n");
    assert!(resp.starts_with("HTTP/1.1 413"), "got: {resp}");
}

#[test]
fn unbounded_header_section_is_rejected() {
    let server = start();
    let mut req = String::from("GET /v1/health HTTP/1.1\r\n");
    for i in 0..500 {
        req.push_str(&format!("X-H{i}: v\r\n"));
    }
    req.push_str("\r\n");
    let resp = raw(server.port(), &req);
    assert!(resp.starts_with("HTTP/1.1 431"), "got: {resp}");
}

#[test]
fn duplicate_content_length_is_rejected() {
    let server = start();
    let req = "POST /v1/merged-pdf HTTP/1.1\r\nContent-Length: 10\r\nContent-Length: 20\r\nConnection: close\r\n\r\n";
    let resp = raw(server.port(), req);
    assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");
}

#[test]
fn malformed_header_line_is_rejected() {
    let server = start();
    let req = "GET /v1/health HTTP/1.1\r\nNotAHeader\r\nConnection: close\r\n\r\n";
    let resp = raw(server.port(), req);
    assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");
}

#[test]
fn header_with_trailing_whitespace_before_colon_is_rejected() {
    let server = start();
    let req = "GET /v1/health HTTP/1.1\r\nContent-Type : application/json\r\nConnection: close\r\n\r\n";
    let resp = raw(server.port(), req);
    assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");
}

#[test]
fn transfer_encoding_is_rejected_with_501() {
    let server = start();
    let req = "POST /v1/merged-pdf HTTP/1.1\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    let resp = raw(server.port(), req);
    assert!(resp.starts_with("HTTP/1.1 501"), "got: {resp}");
}

#[test]
fn invalid_request_line_is_rejected() {
    let server = start();
    let req = "INVALID_REQUEST_LINE\r\nConnection: close\r\n\r\n";
    let resp = raw(server.port(), req);
    assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");
}

#[test]
fn header_with_leading_whitespace_is_rejected() {
    let server = start();
    let req = "GET /v1/health HTTP/1.1\r\n Content-Type: application/json\r\nConnection: close\r\n\r\n";
    let resp = raw(server.port(), req);
    assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");
}

#[test]
fn unsupported_http_version_is_rejected() {
    let server = start();
    let req = "GET /v1/health HTTP/1.xyz\r\nConnection: close\r\n\r\n";
    let resp = raw(server.port(), req);
    assert!(resp.starts_with("HTTP/1.1 505"), "got: {resp}");
}

#[test]
fn accepted_connection_waits_for_request_bytes() {
    let server = start();
    let mut stream = TcpStream::connect(("127.0.0.1", server.port())).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
    // On BSD systems accepted sockets inherit the listener's nonblocking mode.
    // Delay the request until the server has accepted the idle connection.
    std::thread::sleep(std::time::Duration::from_millis(100));
    stream.write_all(b"GET /v1/health HTTP/1.1\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 404"), "got: {response}");
}
