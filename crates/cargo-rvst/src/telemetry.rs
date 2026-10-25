//! Live inspector view. Network work stays off the terminal event loop.
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Paragraph, Sparkline},
};
use rvst_devtools::{InspectorSnapshot, LogEvent, SessionInfo};
use std::{
    collections::VecDeque,
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

const HISTORY: usize = 60;
type PollResult = Result<Option<(SessionInfo, InspectorSnapshot)>, String>;
pub(crate) struct Telemetry {
    current: Option<InspectorSnapshot>,
    session: Option<SessionInfo>,
    history: VecDeque<u64>,
    status: String,
    requested: bool,
    next_poll: Instant,
    worker: Option<JoinHandle<()>>,
    sender: mpsc::Sender<PollResult>,
    receiver: mpsc::Receiver<PollResult>,
}
impl Default for Telemetry {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            current: None,
            session: None,
            history: VecDeque::with_capacity(HISTORY),
            status: "Discovering local plugin inspectors…".into(),
            requested: true,
            next_poll: Instant::now(),
            worker: None,
            sender,
            receiver,
        }
    }
}
impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
impl Telemetry {
    pub fn refresh(&mut self) {
        self.requested = true;
        self.next_poll = Instant::now();
    }
    pub fn command(&self) -> Option<String> {
        self.current.as_ref().map(|snapshot| {
            format!(
                "cargo rvst inspect --format json --instance {}",
                rvst_ops::shell_quote(&snapshot.instance.id)
            )
        })
    }
    fn accept(&mut self, snapshot: InspectorSnapshot) -> Vec<LogEvent> {
        if self
            .current
            .as_ref()
            .is_none_or(|old| old.instance.id != snapshot.instance.id)
        {
            self.history.clear();
        }
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history
            .push_back(snapshot.metrics.mean_callback_ns.max(0.) as u64);
        let events = snapshot.events.clone();
        self.current = Some(snapshot);
        events
    }
    fn should_poll(&self, active_job: bool) -> bool {
        self.worker.is_none()
            && Instant::now() >= self.next_poll
            && (self.requested || active_job || self.session.is_some())
    }
    pub fn tick(&mut self, active_job: bool) -> Vec<LogEvent> {
        let mut events = Vec::new();
        while let Ok(result) = self.receiver.try_recv() {
            match result {
                Ok(Some((session, snapshot))) => {
                    self.status = "Live · updated every second".into();
                    self.session = Some(session);
                    events.extend(self.accept(snapshot));
                }
                Ok(None) => {
                    self.status =
                        "No live inspector. Load a devtools build; press i to discover.".into();
                    self.session = None;
                    self.current = None;
                    self.history.clear();
                }
                Err(error) => {
                    self.status = format!("Inspector disconnected: {error}. Press i to discover.");
                    self.session = None;
                    self.current = None;
                    self.history.clear();
                }
            }
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
            && let Some(worker) = self.worker.take()
        {
            let _ = worker.join();
        }
        if self.should_poll(active_job) {
            let refresh = std::mem::take(&mut self.requested);
            let session = if refresh { None } else { self.session.clone() };
            let previous = self
                .current
                .as_ref()
                .map(|snapshot| (snapshot.instance.id.clone(), snapshot.last_sequence));
            let sender = self.sender.clone();
            self.next_poll = Instant::now() + Duration::from_secs(1);
            self.worker = Some(std::thread::spawn(move || {
                let result = (|| -> std::io::Result<_> {
                    let session = match session {
                        Some(session) => Some(session),
                        None => rvst_devtools::discover_bounded(Duration::from_millis(750))?
                            .into_iter()
                            .max_by_key(|s| s.started_unix_ms),
                    };
                    session
                        .map(|session| {
                            let after = previous
                                .as_ref()
                                .filter(|(id, _)| id == &session.id)
                                .map(|(_, sequence)| *sequence)
                                .unwrap_or(0);
                            rvst_devtools::snapshot(&session, after)
                                .map(|snapshot| (session, snapshot))
                        })
                        .transpose()
                })()
                .map_err(|error| error.to_string());
                let _ = sender.send(result);
            }));
        }
        events
    }
    pub fn draw(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Live plugin telemetry ")
            .borders(Borders::ALL);
        let inside = block.inner(area);
        frame.render_widget(block, area);
        let Some(snapshot) = &self.current else {
            frame.render_widget(
                Paragraph::new(self.status.as_str()).style(Style::default().fg(Color::Gray)),
                inside,
            );
            return;
        };
        let metrics = &snapshot.metrics;
        let meters = &snapshot.snapshot["live"]["meters"];
        let parameters = snapshot.snapshot["live"]["parameters"]
            .as_array()
            .map_or(0, Vec::len);
        let text = vec![
            Line::from(format!(
                "{} · PID {} · {}",
                snapshot.instance.plugin_name, snapshot.instance.pid, snapshot.instance.id
            )),
            Line::from(format!(
                "Callbacks {} · mean {:.2} µs · P95 ≤ {:.2} µs · misses {}",
                metrics.callbacks,
                metrics.mean_callback_ns / 1000.,
                metrics.p95_upper_ns as f64 / 1000.,
                metrics.deadline_misses
            )),
            Line::from(format!(
                "Peaks {:.3} / {:.3} · latency {} samples · {} parameters · dropped {}",
                meters["input_peak"].as_f64().unwrap_or(0.),
                meters["output_peak"].as_f64().unwrap_or(0.),
                meters["latency_samples"].as_u64().unwrap_or(0),
                parameters,
                snapshot.dropped_events
            )),
            Line::from("Mean callback ns · last 60 polls (cumulative mean)"),
        ];
        let rows = Layout::vertical([Constraint::Length(4), Constraint::Min(1)]).split(inside);
        frame.render_widget(Paragraph::new(text), rows[0]);
        let data: Vec<_> = self.history.iter().copied().collect();
        frame.render_widget(
            Sparkline::default()
                .data(&data)
                .style(Style::default().fg(Color::Cyan)),
            rows[1],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(id: &str, mean: u64) -> InspectorSnapshot {
        serde_json::from_value(serde_json::json!({
            "schema_version":1, "instance":{"id":id,"plugin_id":"org.test","plugin_name":"Test synth","pid":42,"address":"127.0.0.1:1234","started_unix_ms":0},
            "events":[],"snapshot":{"live":{"meters":{"latency_samples":64},"parameters":[{}]}},
            "metrics":{"callbacks":7,"mean_callback_ns":mean,"max_callback_ns":mean,"deadline_misses":2,"p50_upper_ns":mean,"p95_upper_ns":mean,"p99_upper_ns":mean,"ui_frames":0,"mean_ui_ns":0.},
            "dropped_events":0,"first_sequence":0,"last_sequence":0
        })).unwrap()
    }
    #[test]
    fn telemetry_history_is_bounded_resets_per_instance_and_stops_when_idle() {
        let mut view = Telemetry::default();
        for mean in 0..100 {
            view.accept(fixture("first", mean));
        }
        assert_eq!(view.history.len(), 60);
        assert_eq!(view.history.front(), Some(&40));
        view.accept(fixture("second", 123));
        assert_eq!(view.history, VecDeque::from([123]));
        assert!(
            view.command()
                .unwrap()
                .contains("inspect --format json --instance")
        );
        view.requested = false;
        assert!(!view.should_poll(false));
        assert!(view.should_poll(true));
        view.refresh();
        assert!(view.should_poll(false));
    }
    #[test]
    fn worker_refreshes_real_session_without_blocking_ui_or_leaking_token() {
        let directory = tempfile::tempdir().unwrap();
        let (session, _rt) =
            rvst_devtools::DebugSession::start_in("org.test.studio", "Studio", directory.path())
                .unwrap();
        session.metrics().record_callback(2500, 1000);
        let mut view = Telemetry::default();
        view.session = Some(session.info().clone());
        view.requested = false;
        assert!(view.tick(false).is_empty());
        assert!(view.worker.is_some());
        let deadline = Instant::now() + Duration::from_secs(2);
        while view.current.is_none() {
            view.tick(false);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(view.current.as_ref().unwrap().metrics.callbacks, 1);
        let token = session.info().url.split_once('#').unwrap().1;
        assert!(!view.command().unwrap().contains(token));
        session.metrics().record_callback(500, 1000);
        view.next_poll = Instant::now();
        while view.current.as_ref().unwrap().metrics.callbacks < 2 {
            view.tick(false);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(view.history.len(), 2);
        assert!(view.current.as_ref().unwrap().events.is_empty());
        drop(session);
        view.next_poll = Instant::now();
        while view.current.is_some() {
            view.tick(false);
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        view.tick(false);
        assert!(view.session.is_none());
        assert!(!view.should_poll(false));
    }
    #[test]
    fn dashboard_renders_live_metrics_and_parameter_count() {
        let mut view = Telemetry::default();
        view.accept(fixture("fixture", 2500));
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 9)).unwrap();
        terminal
            .draw(|frame| view.draw(frame, frame.area()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Test synth"));
        assert!(text.contains("Callbacks 7"));
        assert!(text.contains("mean 2.50 µs"));
        assert!(text.contains("latency 64 samples"));
        assert!(text.contains("1 parameters"));
    }
}
