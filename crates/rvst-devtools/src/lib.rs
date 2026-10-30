//! Opt-in local browser inspector for a real native plugin instance.
//!
//! Construct/drop [`DebugSession`] off the audio thread. Audio callbacks use only
//! [`RtLog::emit`] and [`rvst_runtime::Metrics`]. HTTP and JSON never run there.
mod client;
pub use client::{InspectorSnapshot, SessionSummary, snapshot};

use rvst_runtime::{Metrics, RtTelemetry, TelemetryEvent, TelemetryReader, telemetry_channel};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HISTORY: usize = 512;
const MAX_STATE_PROVIDERS: usize = 16;
const MAX_PROVIDER_BYTES: usize = 64 * 1024;
type StateProvider = Arc<dyn Fn() -> Value + Send + Sync>;
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub pid: u32,
    pub url: String,
    pub address: SocketAddr,
    pub started_unix_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub sequence: u64,
    pub unix_ms: u64,
    pub level: Level,
    pub target: String,
    pub message: String,
    pub data: Value,
}
struct Shared {
    extra: Mutex<Vec<TelemetryReader>>,
    names: Mutex<BTreeMap<u32, String>>,
    stop: AtomicBool,
    dropped: AtomicU64,
    snapshot: Mutex<Value>,
    providers: Mutex<BTreeMap<String, StateProvider>>,
    metrics: Arc<Metrics>,
}
pub struct DebugSession {
    info: SessionInfo,
    shared: Arc<Shared>,
    sender: SyncSender<LogEvent>,
    worker: Option<JoinHandle<()>>,
    registry: PathBuf,
}
pub struct RtLog {
    sender: RtTelemetry,
}
impl RtLog {
    /// Bounded, allocation-free, single-producer audio-thread path.
    pub fn emit(&mut self, code: u32, sample_clock: u64, value: f64) -> bool {
        self.sender.emit(TelemetryEvent {
            code,
            sample_clock,
            value,
            detail: 0,
        })
    }
    pub fn dropped(&self) -> u64 {
        self.sender.dropped()
    }
}
impl rvst_core::DiagnosticSink for RtLog {
    fn emit(&mut self, code: u32, sample_clock: u64, value: f64) -> bool {
        RtLog::emit(self, code, sample_clock, value)
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
fn bounded(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
fn session_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RVST_SESSION_DIR") {
        return dir.into();
    }
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cache/rvst/sessions")
}
impl DebugSession {
    pub fn start(plugin_id: &str, plugin_name: &str) -> io::Result<(Self, RtLog)> {
        Self::start_in(plugin_id, plugin_name, &session_dir())
    }
    pub fn start_in(
        plugin_id: &str,
        plugin_name: &str,
        directory: &Path,
    ) -> io::Result<(Self, RtLog)> {
        fs::create_dir_all(directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let id = format!("{}-{}", std::process::id(), &token[..12]);
        let info = SessionInfo {
            id: id.clone(),
            plugin_id: bounded(plugin_id, 256),
            plugin_name: bounded(plugin_name, 256),
            pid: std::process::id(),
            url: format!("http://{address}/#{token}"),
            address,
            started_unix_ms: now_ms(),
        };
        let registry = directory.join(format!("{id}.json"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(&registry)?
            .write_all(&serde_json::to_vec(&info)?)?;
        let (shared_tx, rx) = mpsc::sync_channel(1024);
        let (rt_tx, rt_rx) = telemetry_channel(4096);
        let shared = Arc::new(Shared {
            extra: Mutex::new(Vec::new()),
            names: Mutex::new(BTreeMap::new()),
            stop: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
            snapshot: Mutex::new(json!({})),
            providers: Mutex::new(BTreeMap::new()),
            metrics: Arc::new(Metrics::default()),
        });
        let thread_shared = shared.clone();
        let thread_info = info.clone();
        let worker = thread::Builder::new()
            .name("rvst-inspector".into())
            .spawn(move || serve(listener, thread_info, token, thread_shared, rx, rt_rx));
        let worker = match worker {
            Ok(w) => w,
            Err(e) => {
                let _ = fs::remove_file(&registry);
                return Err(e);
            }
        };
        let session = Self {
            info,
            shared,
            sender: shared_tx,
            worker: Some(worker),
            registry,
        };
        session.log(Level::Info, "rvst", "Native plugin inspector connected");
        Ok((session, RtLog { sender: rt_tx }))
    }
    /// Allocate a separate bounded producer for a plugin or worker thread.
    /// Call only during setup. Limits prevent accidental unbounded registration.
    pub fn plugin_log(&self) -> io::Result<RtLog> {
        let mut readers = self
            .shared
            .extra
            .lock()
            .map_err(|_| io::Error::other("diagnostic registry poisoned"))?;
        if readers.len() >= 16 {
            return Err(io::Error::other("diagnostic producer limit reached"));
        }
        let (sender, reader) = telemetry_channel(1024);
        readers.push(reader);
        Ok(RtLog { sender })
    }
    /// Register an optional label for a numeric plugin event, off the audio thread.
    pub fn register_event(&self, code: u32, name: &str) {
        if let Ok(mut names) = self.shared.names.lock()
            && (names.len() < 1024 || names.contains_key(&code))
        {
            names.insert(code, bounded(name, 128));
        }
    }
    pub fn info(&self) -> &SessionInfo {
        &self.info
    }
    /// Formatted logging for control/background threads. Never call in process().
    pub fn log(&self, level: Level, target: &str, message: &str) {
        let event = LogEvent {
            sequence: 0,
            unix_ms: now_ms(),
            level,
            target: bounded(target, 128),
            message: bounded(message, 2048),
            data: Value::Null,
        };
        if self.sender.try_send(event).is_err() {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn metrics(&self) -> Arc<Metrics> {
        self.shared.metrics.clone()
    }
    /// Register a live, read-only snapshot source on the control thread.
    /// The inspector worker calls it after authentication, never from audio.
    /// Read atomics or copied state; do not access mutable DSP or block. At most
    /// 16 sources are retained and each serialized result is capped at 64 KiB.
    pub fn register_state_provider(
        &self,
        name: &str,
        provider: impl Fn() -> Value + Send + Sync + 'static,
    ) -> io::Result<()> {
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid state provider name",
            ));
        }
        let mut providers = self
            .shared
            .providers
            .lock()
            .map_err(|_| io::Error::other("state provider registry poisoned"))?;
        if providers.contains_key(name) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "state provider already registered",
            ));
        }
        if providers.len() >= MAX_STATE_PROVIDERS {
            return Err(io::Error::other("state provider limit reached"));
        }
        providers.insert(name.into(), Arc::new(provider));
        Ok(())
    }
    /// Static control-thread metadata. Registered snapshots appear under `live`.
    pub fn set_snapshot(&self, snapshot: Value) {
        if let Ok(mut value) = self.shared.snapshot.lock() {
            *value = snapshot;
        }
    }
}
impl Drop for DebugSession {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_file(&self.registry);
    }
}
fn serve(
    listener: TcpListener,
    info: SessionInfo,
    token: String,
    shared: Arc<Shared>,
    rx: mpsc::Receiver<LogEvent>,
    mut rt: TelemetryReader,
) {
    let mut history = VecDeque::with_capacity(HISTORY);
    let mut sequence = 0u64;
    while !shared.stop.load(Ordering::Acquire) {
        for mut event in rx.try_iter().take(1024) {
            sequence += 1;
            event.sequence = sequence;
            append(&mut history, event);
        }
        for _ in 0..4096 {
            let Some(event) = rt.pop() else {
                break;
            };
            sequence += 1;
            append(
                &mut history,
                LogEvent {
                    sequence,
                    unix_ms: now_ms(),
                    level: Level::Debug,
                    target: "audio".into(),
                    message: event_name(&shared, event.code),
                    data: json!({"code":event.code,"sample_clock":event.sample_clock,"value":event.value}),
                },
            );
        }
        let mut extra_dropped = 0;
        if let Ok(mut readers) = shared.extra.lock() {
            for reader in readers.iter_mut() {
                for _ in 0..1024 {
                    let Some(event) = reader.pop() else {
                        break;
                    };
                    sequence += 1;
                    append(
                        &mut history,
                        LogEvent {
                            sequence,
                            unix_ms: now_ms(),
                            level: Level::Debug,
                            target: "plugin".into(),
                            message: event_name(&shared, event.code),
                            data: json!({"code":event.code,"sample_clock":event.sample_clock,"value":event.value}),
                        },
                    );
                }
                extra_dropped += reader.dropped();
            }
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = respond(
                    stream,
                    &info,
                    &token,
                    &shared,
                    &history,
                    rt.dropped() + extra_dropped,
                    sequence,
                );
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(_) => break,
        }
    }
}
fn event_name(shared: &Shared, code: u32) -> String {
    shared
        .names
        .lock()
        .ok()
        .and_then(|names| names.get(&code).cloned())
        .unwrap_or_else(|| format!("RT event {code}"))
}
fn append(history: &mut VecDeque<LogEvent>, event: LogEvent) {
    if history.len() == HISTORY {
        history.pop_front();
    }
    history.push_back(event);
}
fn respond(
    mut stream: TcpStream,
    info: &SessionInfo,
    token: &str,
    shared: &Shared,
    history: &VecDeque<LogEvent>,
    rt_dropped: u64,
    sequence: u64,
) -> io::Result<()> {
    // Accepted sockets inherit nonblocking mode on some platforms (including macOS).
    // Timed reads require blocking mode; otherwise fragmented headers hit WouldBlock.
    stream.set_nonblocking(false)?;
    let header_deadline = Instant::now() + Duration::from_millis(100);
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let mut request = Vec::with_capacity(2048);
    let mut buf = [0u8; 1024];
    while !request.windows(4).any(|v| v == b"\r\n\r\n") {
        let remaining = header_deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "inspector header deadline"))?;
        stream.set_read_timeout(Some(remaining))?;
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buf[..n]);
        if request.len() > 8192 {
            return write_response(
                &mut stream,
                "413 Payload Too Large",
                "text/plain",
                "Request too large",
            );
        }
    }
    let text = String::from_utf8_lossy(&request);
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or_default();
    let mut start = first.split_whitespace();
    let method = start.next().unwrap_or_default();
    let path = start.next().unwrap_or_default();
    let mut host = "";
    let mut origin = None;
    let mut auth = "";
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            match k.to_ascii_lowercase().as_str() {
                "host" => host = v.trim(),
                "origin" => origin = Some(v.trim()),
                "x-rvst-token" => auth = v.trim(),
                _ => {}
            }
        }
    }
    let expected_host = info.address.to_string();
    let expected_origin = format!("http://{expected_host}");
    if host != expected_host || origin.is_some_and(|o| o != expected_origin) {
        return write_response(&mut stream, "403 Forbidden", "text/plain", "Invalid origin");
    }
    if method != "GET" {
        return write_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            "GET required",
        );
    }
    if path == "/" {
        return write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            include_str!("../assets/index.html"),
        );
    }
    if path == "/favicon.ico" {
        // Browsers request this without the inspector token. It exposes no session data.
        return stream.write_all(
            b"HTTP/1.1 204 No Content\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        );
    }
    if !constant_equal(auth.as_bytes(), token.as_bytes()) {
        return write_response(
            &mut stream,
            "403 Forbidden",
            "text/plain",
            "Inspector token required",
        );
    }
    let body = if path == "/api/health" {
        json!({"id":info.id})
    } else if path.starts_with("/api/events?") || path == "/api/events" {
        let after = path
            .split_once("after=")
            .and_then(|(_, n)| n.split('&').next())
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        let snapshot = state_snapshot(shared);
        json!({"schema_version":1,"instance":info.summary(),"events":history.iter().filter(|e|e.sequence>after).collect::<Vec<_>>(),"snapshot":snapshot,"metrics":shared.metrics.snapshot(),"dropped_events":shared.dropped.load(Ordering::Relaxed)+rt_dropped,"first_sequence":history.front().map(|e|e.sequence).unwrap_or(0),"last_sequence":sequence})
    } else {
        return write_response(&mut stream, "404 Not Found", "text/plain", "Not found");
    };
    write_response(&mut stream, "200 OK", "application/json", &body.to_string())
}
fn state_snapshot(shared: &Shared) -> Value {
    let mut snapshot = shared
        .snapshot
        .lock()
        .map(|v| v.clone())
        .unwrap_or(Value::Null);
    if !snapshot.is_object() {
        snapshot = json!({"value": snapshot});
    }
    // Release the registry lock before calling plugin-owned code.
    let providers = shared
        .providers
        .lock()
        .map(|sources| {
            sources
                .iter()
                .map(|(name, source)| (name.clone(), source.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let live: serde_json::Map<String, Value> = providers
        .into_iter()
        .map(|(name, source)| {
            let value = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| source())) {
                Ok(value) => {
                    let mut limit = SizeLimit(MAX_PROVIDER_BYTES);
                    if serde_json::to_writer(&mut limit, &value).is_ok() {
                        value
                    } else {
                        json!({"error": "state provider exceeded 64 KiB"})
                    }
                }
                Err(_) => json!({"error": "state provider panicked"}),
            };
            (name, value)
        })
        .collect();
    snapshot["live"] = Value::Object(live);
    snapshot
}
struct SizeLimit(usize);
impl Write for SizeLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(io::Error::other("snapshot size limit"));
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn constant_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |n, (x, y)| n | (x ^ y)) == 0
}
fn write_response(stream: &mut TcpStream, status: &str, kind: &str, body: &str) -> io::Result<()> {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'self'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes())?;
    stream.write_all(body.as_bytes())
}
pub fn discover() -> io::Result<Vec<SessionInfo>> {
    discover_in(&session_dir())
}
pub fn discover_in(directory: &Path) -> io::Result<Vec<SessionInfo>> {
    discover_with_budget(directory, None)
}
/// A bounded discovery pass for interactive views. Newer records are probed
/// first; the result may be partial when the time budget is exhausted.
/// At most one in-flight request can extend past the budget (500 ms).
pub fn discover_bounded(budget: Duration) -> io::Result<Vec<SessionInfo>> {
    discover_with_budget(&session_dir(), Some(budget))
}
fn discover_with_budget(
    directory: &Path,
    budget: Option<Duration>,
) -> io::Result<Vec<SessionInfo>> {
    let entries = match fs::read_dir(directory) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut candidates = Vec::new();
    for entry in entries.take(256).flatten() {
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > 65536 {
            continue;
        }
        let Ok(data) = fs::read(&path) else {
            continue;
        };
        let Ok(info) = serde_json::from_slice::<SessionInfo>(&data) else {
            continue;
        };
        candidates.push(info);
    }
    candidates.sort_by_key(|info| std::cmp::Reverse(info.started_unix_ms));
    let started = Instant::now();
    let mut result = Vec::new();
    for info in candidates {
        if budget.is_some_and(|budget| started.elapsed() >= budget) {
            break;
        }
        if client::healthy(&info) {
            result.push(info);
        }
    }
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}
