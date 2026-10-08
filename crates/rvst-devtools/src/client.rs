//! Authenticated loopback client shared by human and machine interfaces.
use crate::{LogEvent, SessionInfo};
use rvst_runtime::MetricsSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);

/// Public instance identity. The browser capability URL is deliberately omitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub pid: u32,
    pub address: SocketAddr,
    pub started_unix_ms: u64,
}
impl SessionInfo {
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.id.clone(),
            plugin_id: self.plugin_id.clone(),
            plugin_name: self.plugin_name.clone(),
            pid: self.pid,
            address: self.address,
            started_unix_ms: self.started_unix_ms,
        }
    }
}

/// A bounded, read-only observation of a live native plugin instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InspectorSnapshot {
    pub schema_version: u32,
    pub instance: SessionSummary,
    pub events: Vec<LogEvent>,
    pub snapshot: Value,
    pub metrics: MetricsSnapshot,
    pub dropped_events: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
}

/// Fetch state and metrics plus events newer than `after`. Never call on audio.
/// Only loopback addresses with a valid local capability are accepted; neither
/// returned data nor errors contain that capability. Requests have a total
/// deadline and a 4 MiB response limit, with no redirects or proxy support.
pub fn snapshot(info: &SessionInfo, after: u64) -> io::Result<InspectorSnapshot> {
    let body = request(info, &format!("/api/events?after={after}"))?;
    let value: InspectorSnapshot = serde_json::from_slice(&body)
        .map_err(|_| io::Error::other("invalid inspector snapshot"))?;
    if value.schema_version != 1
        || value.instance.id != info.id
        || value.instance.plugin_id != info.plugin_id
        || value.instance.pid != info.pid
        || value.events.len() > crate::HISTORY
    {
        return Err(io::Error::other(
            "inspector snapshot identity or schema mismatch",
        ));
    }
    Ok(value)
}

pub(crate) fn healthy(info: &SessionInfo) -> bool {
    request(info, "/api/health")
        .ok()
        .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
        .is_some_and(|value| value["id"] == info.id)
}

fn request(info: &SessionInfo, path: &str) -> io::Result<Vec<u8>> {
    let prefix = format!("http://{}/#", info.address);
    let token = info
        .url
        .strip_prefix(&prefix)
        .filter(|token| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()));
    let Some(token) = token.filter(|_| info.address.ip().is_loopback()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid local inspector capability",
        ));
    };
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut stream = TcpStream::connect_timeout(&info.address, Duration::from_millis(100))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    stream.set_nodelay(true)?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nX-RVST-Token: {token}\r\nConnection: close\r\n\r\n",
        info.address
    );
    stream.write_all(request.as_bytes())?;
    let mut response = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "inspector request timed out")
            })?;
        stream.set_read_timeout(Some(remaining))?;
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        if response.len() + n > MAX_RESPONSE_BYTES {
            return Err(io::Error::other("inspector response exceeds 4 MiB"));
        }
        response.extend_from_slice(&buffer[..n]);
    }
    let split = response
        .windows(4)
        .position(|b| b == b"\r\n\r\n")
        .ok_or_else(|| io::Error::other("invalid inspector HTTP response"))?;
    let headers = &response[..split];
    if headers.starts_with(b"HTTP/1.1 403 ") {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "inspector authentication failed",
        ));
    }
    if !headers.starts_with(b"HTTP/1.1 200 ") {
        return Err(io::Error::other("inspector request failed"));
    }
    Ok(response.split_off(split + 4))
}
