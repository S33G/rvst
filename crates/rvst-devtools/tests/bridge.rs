use rvst_devtools::*;
use std::{
    io::{Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};
fn request(info: &SessionInfo, token: &str, origin: &str) -> String {
    let mut socket = TcpStream::connect(info.address).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(socket,"GET /api/events?after=0 HTTP/1.1\r\nHost: {}\r\nX-RVST-Token: {}\r\n{}Connection: close\r\n\r\n",info.address,token,origin).unwrap();
    let mut result = String::new();
    socket.read_to_string(&mut result).unwrap();
    result
}
#[test]
fn native_events_reach_authenticated_browser_and_shutdown_cleans_registry() {
    let dir = std::env::temp_dir().join(format!("rvst-bridge-test-{}", std::process::id()));
    let (session, mut rt) = DebugSession::start_in("org.test.gain", "Gain", &dir).unwrap();
    let info = session.info().clone();
    let token = info.url.split('#').nth(1).unwrap();
    session.log(Level::Info, "lifecycle", "plugin activated");
    assert!(rt.emit(42, 128, 0.25));
    session.register_event(500, "Plugin note triggered");
    let mut plugin = session.plugin_log().unwrap();
    assert!(plugin.emit(500, 256, 60.));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let result = request(&info, token, "");
        if result.contains("plugin activated")
            && result.contains("Plugin note triggered")
            && result.contains("sample_clock")
        {
            assert!(result.starts_with("HTTP/1.1 200"));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(request(&info, "wrong", "").starts_with("HTTP/1.1 403"));
    assert!(request(&info, token, "Origin: https://evil.example\r\n").starts_with("HTTP/1.1 403"));
    assert_eq!(discover_in(&dir).unwrap().len(), 1);
    drop(session);
    assert!(discover_in(&dir).unwrap().is_empty());
    let _ = std::fs::remove_dir(&dir);
}

fn response_json(info: &SessionInfo, token: &str) -> serde_json::Value {
    let response = request(info, token, "");
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap()
}

#[test]
fn state_providers_refresh_on_worker_only_after_authentication() {
    use std::sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    };
    let dir = std::env::temp_dir().join(format!("rvst-state-test-{}", std::process::id()));
    let (session, _rt) = DebugSession::start_in("org.test.state", "State", &dir).unwrap();
    let value = Arc::new(AtomicU64::new(1));
    let calls = Arc::new(AtomicU64::new(0));
    let provider_value = value.clone();
    let provider_calls = calls.clone();
    let caller_thread = std::thread::current().id();
    session.set_snapshot(serde_json::json!({"static":"kept"}));
    session
        .register_state_provider("engine", move || {
            assert_ne!(std::thread::current().id(), caller_thread);
            provider_calls.fetch_add(1, Ordering::Relaxed);
            serde_json::json!({"value":provider_value.load(Ordering::Acquire)})
        })
        .unwrap();
    let info = session.info();
    let token = info.url.split_once('#').unwrap().1;
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(request(info, "incorrect", "").starts_with("HTTP/1.1 403"));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let first = response_json(info, token);
    assert_eq!(first["snapshot"]["static"], "kept");
    assert_eq!(first["snapshot"]["live"]["engine"]["value"], 1);
    value.store(2, Ordering::Release);
    session.metrics().record_callback(200, 100);
    let second = response_json(info, token);
    assert_eq!(second["snapshot"]["live"]["engine"]["value"], 2);
    assert_eq!(second["metrics"]["callbacks"], 1);
    assert_eq!(second["metrics"]["deadline_misses"], 1);
    drop(session);
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn state_provider_registration_and_payloads_are_bounded() {
    let dir = std::env::temp_dir().join(format!("rvst-state-bounds-{}", std::process::id()));
    let (session, _rt) = DebugSession::start_in("org.test.bounds", "Bounds", &dir).unwrap();
    session
        .register_state_provider("large", || serde_json::json!("x".repeat(65537)))
        .unwrap();
    session
        .register_state_provider("panic", || panic!("provider fixture"))
        .unwrap();
    assert_eq!(
        session
            .register_state_provider("large", || serde_json::Value::Null)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert!(
        session
            .register_state_provider("invalid name", || serde_json::Value::Null)
            .is_err()
    );
    for i in 2..16 {
        session
            .register_state_provider(&format!("state{i}"), || serde_json::json!(1))
            .unwrap();
    }
    assert!(
        session
            .register_state_provider("overflow", || serde_json::Value::Null)
            .is_err()
    );
    let info = session.info();
    let token = info.url.split_once('#').unwrap().1;
    let result = response_json(info, token);
    assert!(
        result["snapshot"]["live"]["large"]["error"]
            .as_str()
            .unwrap()
            .contains("64 KiB")
    );
    assert_eq!(
        result["snapshot"]["live"]["panic"]["error"],
        "state provider panicked"
    );
    assert_eq!(result["snapshot"]["live"]["state15"], 1);
    assert!(response_json(info, token)["snapshot"]["live"].is_object());
    drop(session);
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn snapshot_client_authenticates_refreshes_and_never_exposes_credentials() {
    let dir = std::env::temp_dir().join(format!("rvst-client-test-{}", std::process::id()));
    let (session, _rt) = DebugSession::start_in("org.test.client", "Client", &dir).unwrap();
    session.metrics().record_callback(500, 1000);
    session.set_snapshot(serde_json::json!({"test": true}));
    let info = session.info();
    let first = snapshot(info, 0).unwrap();
    assert_eq!(first.instance.id, info.id);
    assert_eq!(first.metrics.callbacks, 1);
    assert_eq!(first.snapshot["test"], true);
    let encoded = serde_json::to_string(&first).unwrap();
    assert!(!encoded.contains(info.url.split_once('#').unwrap().1));
    assert!(!encoded.contains("\"url\""));
    session.metrics().record_callback(2000, 1000);
    let second = snapshot(info, first.last_sequence).unwrap();
    assert_eq!(second.metrics.callbacks, 2);
    assert_eq!(second.metrics.deadline_misses, 1);
    assert!(second.events.is_empty());
    let mut invalid = info.clone();
    invalid.url = format!("http://{}/#{}", info.address, "0".repeat(64));
    assert_eq!(
        snapshot(&invalid, 0).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    invalid.url = format!("http://{}/#bad\r\nInjected: yes", info.address);
    assert_eq!(
        snapshot(&invalid, 0).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    invalid.address.set_ip("192.0.2.1".parse().unwrap());
    assert_eq!(
        snapshot(&invalid, 0).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    drop(session);
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn fragmented_headers_are_reassembled_before_authentication() {
    let directory = std::env::temp_dir().join(format!("rvst-fragment-test-{}", std::process::id()));
    let (session, _rt) =
        DebugSession::start_in("org.test.fragments", "Fragments", &directory).unwrap();
    let info = session.info();
    let token = info.url.split_once('#').unwrap().1;
    let mut stream = TcpStream::connect(info.address).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let prefix = format!(
        "GET /api/events HTTP/1.1\r\nHost: {}\r\nX-RVST-Token: {}",
        info.address,
        &token[..10]
    );
    stream.write_all(prefix.as_bytes()).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    let suffix = format!("{}\r\nConnection: close\r\n\r\n", &token[10..]);
    stream.write_all(suffix.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("org.test.fragments"));
    drop(session);
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn favicon_is_empty_without_auth_but_still_checks_host_origin_and_method() {
    let directory = std::env::temp_dir().join(format!("rvst-favicon-test-{}", std::process::id()));
    let (session, _rt) = DebugSession::start_in("org.test.favicon", "Favicon", &directory).unwrap();
    let info = session.info();
    let request = |method: &str, path: &str, host: &str, origin: &str| {
        let mut stream = TcpStream::connect(info.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{origin}Connection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let host = info.address.to_string();
    for origin in [String::new(), format!("Origin: http://{host}\r\n")] {
        let response = request("GET", "/favicon.ico", &host, &origin);
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("HTTP/1.1 204 No Content"), "{response}");
        assert!(!headers.to_ascii_lowercase().contains("content-length:"));
        assert!(body.is_empty());
    }
    for (method, path, request_host, origin, status) in [
        ("GET", "/favicon.ico", "evil.example", "", "403"),
        ("GET", "/favicon.ico", "", "", "403"),
        (
            "GET",
            "/favicon.ico",
            host.as_str(),
            "Origin: https://evil.example\r\n",
            "403",
        ),
        ("POST", "/favicon.ico", host.as_str(), "", "405"),
        ("GET", "/favicon.ico/api/events", host.as_str(), "", "403"),
        ("GET", "/api/events", host.as_str(), "", "403"),
    ] {
        let response = request(method, path, request_host, origin);
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{response}"
        );
    }
    drop(session);
    std::fs::remove_dir(directory).unwrap();
}
