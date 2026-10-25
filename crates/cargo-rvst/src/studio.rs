//! The terminal is a view over the same operations used by the CLI.
use base64::Engine;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use rvst_ops::{Cancellation, LogEvent, OpError, Result, TemplateManifest};
use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

const ACTIONS: &[(&str, &str)] = &[
    ("★ Start with RVST Debug", "showcase"),
    ("New plugin", "new"),
    ("Run standalone", "dev"),
    ("Run web editor", "web"),
    ("Run tests", "test"),
    ("Validate all targets", "validate"),
    ("Plugin diagnostics", "debug"),
    ("Discover browser inspectors", "inspect"),
    ("Package native bundles", "package"),
    ("Check host tools", "doctor"),
];
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Browse,
    Search,
    New,
    Palette,
}
enum Update {
    Log(LogEvent),
    Finished(std::result::Result<String, OpError>),
}
struct App {
    project: PathBuf,
    templates: Vec<TemplateManifest>,
    selection: usize,
    filter: String,
    mode: Mode,
    wizard: Vec<(String, String)>,
    field: usize,
    wizard_template: String,
    showcase: bool,
    vendor_sdk: bool,
    logs: Vec<String>,
    task: String,
    command: String,
    running: bool,
    cancel: Cancellation,
    tx: mpsc::Sender<Update>,
    rx: mpsc::Receiver<Update>,
    palette: usize,
    workers: Vec<std::thread::JoinHandle<()>>,
    telemetry: crate::telemetry::Telemetry,
}
impl Drop for App {
    fn drop(&mut self) {
        self.cancel.cancel();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}
pub fn run(project: &Path) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(OpError::new(
            "terminal_required",
            "studio requires an interactive terminal",
            2,
        ));
    }
    let templates = rvst_ops::list_templates()?;
    terminal::enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let (tx, rx) = mpsc::channel();
    let mut app = App {
        project: project.to_path_buf(),
        templates,
        selection: 0,
        filter: String::new(),
        mode: Mode::Browse,
        wizard: vec![],
        field: 0,
        wizard_template: String::new(),
        showcase: false,
        vendor_sdk: false,
        logs: vec!["Welcome. Select a template; n creates a project. : opens commands.".into()],
        task: "Idle".into(),
        command: "cargo rvst templates list".into(),
        running: false,
        cancel: Cancellation::default(),
        tx,
        rx,
        palette: 0,
        workers: vec![],
        telemetry: crate::telemetry::Telemetry::default(),
    };
    loop {
        while let Ok(update) = app.rx.try_recv() {
            match update {
                Update::Log(event) => app.logs.push(format!("{} │ {}", event.stream, event.line)),
                Update::Finished(result) => {
                    app.running = false;
                    match result {
                        Ok(message) => {
                            app.task = "Complete".into();
                            app.logs.push(message);
                        }
                        Err(error) => {
                            app.task = if error.exit_code == 130 {
                                "Cancelled".into()
                            } else {
                                "Failed".into()
                            };
                            app.logs.push(error.to_string());
                        }
                    }
                }
            }
            if app.logs.len() > 2000 {
                app.logs.drain(..1000);
            }
        }
        for event in app.telemetry.tick(app.running) {
            app.logs.push(format!(
                "plugin {:?} │ {}: {}",
                event.level, event.target, event.message
            ));
        }
        if app.logs.len() > 2000 {
            app.logs.drain(..app.logs.len() - 2000);
        }
        terminal.draw(|frame| draw(frame, &app))?;
        if !event::poll(Duration::from_millis(40))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == event::KeyEventKind::Release {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if app.running {
                app.cancel.cancel();
                app.task = "Cancelling…".into();
            } else {
                break;
            }
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('y') {
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(equivalent_command(&app));
            write!(io::stdout(), "\x1b]52;c;{encoded}\x07")?;
            io::stdout().flush()?;
            app.logs
                .push("CLI command copied via terminal clipboard (OSC 52).".into());
            continue;
        }
        match app.mode {
            Mode::Browse => match key.code {
                KeyCode::Char('q') => {
                    app.cancel.cancel();
                    break;
                }
                KeyCode::Char('/') => app.mode = Mode::Search,
                KeyCode::Char(':') => {
                    app.mode = Mode::Palette;
                    app.palette = 0;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let len = filtered(&app).len();
                    if len > 0 {
                        app.selection = (app.selection + 1) % len;
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let len = filtered(&app).len();
                    if len > 0 {
                        app.selection = (app.selection + len - 1) % len;
                    }
                }
                KeyCode::Char('n') | KeyCode::Enter => start_wizard(&mut app, false),
                KeyCode::Char('s') => start_wizard(&mut app, true),
                KeyCode::Char('t') => launch(&mut app, "test"),
                KeyCode::Char('v') => launch(&mut app, "validate"),
                KeyCode::Char('d') => launch(&mut app, "dev"),
                KeyCode::Char('i') => launch(&mut app, "inspect"),
                KeyCode::Char('p') => launch(&mut app, "package"),
                KeyCode::Esc if app.running => {
                    app.cancel.cancel();
                    app.task = "Cancelling…".into();
                }
                _ => {}
            },
            Mode::Search => match key.code {
                KeyCode::Esc | KeyCode::Enter => app.mode = Mode::Browse,
                KeyCode::Backspace => {
                    app.filter.pop();
                    app.selection = 0;
                }
                KeyCode::Char(c) => {
                    app.filter.push(c);
                    app.selection = 0;
                }
                _ => {}
            },
            Mode::New => match key.code {
                KeyCode::Esc => app.mode = Mode::Browse,
                KeyCode::Tab | KeyCode::Down => app.field = (app.field + 1) % app.wizard.len(),
                KeyCode::BackTab | KeyCode::Up => {
                    app.field = (app.field + app.wizard.len() - 1) % app.wizard.len()
                }
                KeyCode::F(2) => app.vendor_sdk = !app.vendor_sdk,
                KeyCode::Backspace => {
                    app.wizard[app.field].1.pop();
                }
                KeyCode::Char(c) => app.wizard[app.field].1.push(c),
                KeyCode::Enter => {
                    launch_new(&mut app);
                    app.mode = Mode::Browse;
                }
                _ => {}
            },
            Mode::Palette => match key.code {
                KeyCode::Esc => app.mode = Mode::Browse,
                KeyCode::Down | KeyCode::Char('j') => {
                    app.palette = (app.palette + 1) % ACTIONS.len()
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    app.palette = (app.palette + ACTIONS.len() - 1) % ACTIONS.len()
                }
                KeyCode::Enter => {
                    let action = ACTIONS[app.palette].1;
                    app.mode = Mode::Browse;
                    if action == "new" || action == "showcase" {
                        start_wizard(&mut app, action == "showcase");
                    } else {
                        launch(&mut app, action);
                    }
                }
                _ => {}
            },
        }
    }
    app.cancel.cancel();
    Ok(())
}
fn filtered(app: &App) -> Vec<&TemplateManifest> {
    let filter = app.filter.to_lowercase();
    app.templates
        .iter()
        .filter(|t| {
            format!("{} {} {}", t.id, t.name, t.description)
                .to_lowercase()
                .contains(&filter)
        })
        .collect()
}
fn start_wizard(app: &mut App, showcase: bool) {
    if app.running {
        return;
    }
    let template = if showcase {
        "debug".to_string()
    } else {
        let templates = filtered(app);
        let Some(template) = templates.get(app.selection) else {
            return;
        };
        template.id.clone()
    };
    let Ok(manifest) = rvst_ops::inspect_template(&template) else {
        return;
    };
    app.wizard = vec![
        (
            "path".into(),
            if showcase { "rvst-debug" } else { "my-plugin" }.into(),
        ),
        (
            "sdk_path".into(),
            if app.project.join("crates/rvst-core/Cargo.toml").is_file() {
                std::fs::canonicalize(&app.project)
                    .map(|p| p.to_string_lossy().into())
                    .unwrap_or_default()
            } else {
                String::new()
            },
        ),
    ];
    if !showcase {
        for (name, spec) in manifest.manifest.inputs {
            if name != "crate_name" && name != "kind" {
                let value = spec
                    .default
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| v.to_string())
                    })
                    .unwrap_or_default();
                app.wizard.push((name, value));
            }
        }
    }
    app.wizard_template = template;
    app.showcase = showcase;
    app.field = 0;
    app.vendor_sdk = false;
    app.mode = Mode::New;
}
fn launch_new(app: &mut App) {
    if app.running || app.wizard.is_empty() {
        return;
    }
    let destination = app.project.join(app.wizard[0].1.trim());
    let sdk_path =
        (!app.wizard[1].1.trim().is_empty()).then(|| PathBuf::from(app.wizard[1].1.trim()));
    let template = app.wizard_template.clone();
    let showcase = app.showcase;
    let vendor_sdk = app.vendor_sdk;
    let inputs = app
        .wizard
        .iter()
        .skip(2)
        .filter(|(_, value)| !value.is_empty())
        .cloned()
        .collect();
    app.command = if showcase {
        format!(
            "cargo rvst showcase --path {}",
            rvst_ops::shell_quote(&destination.to_string_lossy())
        )
    } else {
        format!(
            "cargo rvst new {} --template {}",
            rvst_ops::shell_quote(&destination.to_string_lossy()),
            rvst_ops::shell_quote(&template)
        )
    };
    if let Some(sdk) = &sdk_path {
        app.command.push_str(&format!(
            " --sdk-path {}",
            rvst_ops::shell_quote(&sdk.to_string_lossy())
        ));
    }
    if vendor_sdk {
        app.command.push_str(" --vendor-sdk");
    }
    if !showcase {
        for (name, value) in app
            .wizard
            .iter()
            .skip(2)
            .filter(|(_, value)| !value.is_empty())
        {
            app.command.push_str(&format!(
                " --set {}",
                rvst_ops::shell_quote(&format!("{name}={value}"))
            ));
        }
    }
    app.task = if showcase {
        "Building RVST Debug".into()
    } else {
        "Creating plugin".into()
    };
    app.running = true;
    app.cancel = Cancellation::default();
    let tx = app.tx.clone();
    let cancel = app.cancel.clone();
    app.workers.push(std::thread::spawn(move || {
        let result = if showcase {
            let (log_tx, log_rx) = mpsc::channel();
            let forward = tx.clone();
            let logger = std::thread::spawn(move || {
                for event in log_rx {
                    let _ = forward.send(Update::Log(event));
                }
            });
            let result = rvst_ops::showcase(
                rvst_ops::ShowcaseRequest {
                    path: destination,
                    sdk_path,
                    vendor_sdk,
                    install: false,
                },
                &cancel,
                Some(log_tx),
            );
            let _ = logger.join();
            result.map(|r| r.stdout)
        } else {
            rvst_ops::resolve_template(&template)
                .and_then(|template| {
                    rvst_ops::Generator::new().generate(rvst_ops::NewRequest {
                        template,
                        destination,
                        inputs,
                        sdk_path,
                        vendor_sdk,
                        git: true,
                    })
                })
                .map(|report| format!("Created {}", report.path.display()))
        };
        let _ = tx.send(Update::Finished(result));
    }));
}
fn launch(app: &mut App, task: &str) {
    if task == "inspect" {
        app.telemetry.refresh();
    }
    if app.running {
        app.logs
            .push("A task is already running. Esc cancels it.".into());
        return;
    }
    app.command = format!(
        "cargo rvst --project {} {task}",
        rvst_ops::shell_quote(&app.project.to_string_lossy())
    );
    if task == "web" {
        app.command = app.command.replace(" web", " dev --web --open");
    }
    app.task = format!("Running {task}");
    app.running = true;
    app.cancel = Cancellation::default();
    let project = app.project.clone();
    let cancel = app.cancel.clone();
    let tx = app.tx.clone();
    let task = task.to_string();
    app.workers.push(std::thread::spawn(move || {
        let result = if task == "doctor" {
            Ok(serde_json::to_string_pretty(&rvst_ops::doctor(&project)).unwrap())
        } else if task == "inspect" {
            rvst_devtools::discover_bounded(Duration::from_millis(750))
                .map_err(OpError::from)
                .and_then(|sessions| {
                    sessions
                        .iter()
                        .map(|session| rvst_devtools::snapshot(session, 0))
                        .collect::<std::io::Result<Vec<_>>>()
                        .map_err(OpError::from)
                })
                .map(|snapshots| serde_json::to_string_pretty(&snapshots).unwrap())
        } else {
            rvst_ops::task_command(&task, false, "all", None).and_then(|mut command| {
                if task == "web" {
                    command.push("--open".into());
                }
                let (log_tx, log_rx) = mpsc::channel();
                let forward = tx.clone();
                let logger = std::thread::spawn(move || {
                    for event in log_rx {
                        let _ = forward.send(Update::Log(event));
                    }
                });
                let result = rvst_ops::run_task(&project, &command, &cancel, Some(log_tx));
                let _ = logger.join();
                let report = result?;
                if report.exit_code == 0 {
                    Ok(format!("{task} completed"))
                } else {
                    Err(OpError::new(
                        if report.exit_code == 130 {
                            "cancelled"
                        } else {
                            "task_failed"
                        },
                        format!("{task} returned {}", report.exit_code),
                        if report.exit_code == 130 { 130 } else { 1 },
                    ))
                }
            })
        };
        let _ = tx.send(Update::Finished(result));
    }));
}
fn equivalent_command(app: &App) -> String {
    if !app.running
        && let Some(command) = app.telemetry.command()
    {
        return command;
    }
    app.command.clone()
}
fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
        Constraint::Length(2),
    ])
    .split(area);
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " RVST STUDIO ",
            Style::default().fg(Color::Black).bg(Color::Cyan).bold(),
        ),
        Span::styled(
            " Rust plugins · one workspace, every surface",
            Style::default().fg(Color::Gray),
        ),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, rows[0]);
    let cols =
        Layout::horizontal([Constraint::Percentage(34), Constraint::Percentage(66)]).split(rows[1]);
    let left =
        Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)]).split(cols[0]);
    let templates = filtered(app);
    let items = templates
        .iter()
        .map(|t| ListItem::new(format!("{}  {}", t.id, t.name)))
        .collect::<Vec<_>>();
    let mut state = ListState::default().with_selected(Some(app.selection));
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .title(format!(" Templates / {} ", app.filter))
                    .borders(Borders::ALL),
            )
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::Cyan).bold())
            .highlight_symbol("› "),
        left[0],
        &mut state,
    );
    let details = templates
        .get(app.selection)
        .map(|t| {
            format!(
                "{}\n\n{}\n\n{} typed inputs\nNew creates a separate Git repository.",
                t.name,
                t.description,
                t.inputs.len()
            )
        })
        .unwrap_or_else(|| "No matching templates".into());
    frame.render_widget(
        Paragraph::new(details).wrap(Wrap { trim: false }).block(
            Block::default()
                .title(" Selected template ")
                .borders(Borders::ALL),
        ),
        left[1],
    );
    let right = Layout::vertical([Constraint::Length(8), Constraint::Min(3)]).split(cols[1]);
    app.telemetry.draw(frame, right[0]);
    let visible = right[1].height.saturating_sub(2) as usize;
    let start = app.logs.len().saturating_sub(visible);
    let logs = app.logs[start..]
        .iter()
        .cloned()
        .map(Line::from)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(logs)
            .block(
                Block::default()
                    .title(format!(
                        " {} {} ",
                        if app.running { "●" } else { "○" },
                        app.task
                    ))
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        right[1],
    );
    frame.render_widget(
        Paragraph::new(equivalent_command(app))
            .block(
                Block::default()
                    .title(" Equivalent CLI · Ctrl+Y copy ")
                    .borders(Borders::ALL),
            )
            .style(Style::default().fg(Color::Cyan)),
        rows[2],
    );
    let hint = match app.mode {
        Mode::Browse => {
            "s RVST Debug · ↑↓ select · / search · n new · d dev · t test · v validate · i inspect · p package · : commands · Esc cancel · q quit"
        }
        Mode::Search => "Type to filter · Enter accept · Esc close",
        Mode::New => "Tab field · F2 portable SDK · Enter create/build · Esc cancel",
        Mode::Palette => "↑↓ select command · Enter run · Esc close",
    };
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(Color::DarkGray)),
        rows[3],
    );
    if app.mode == Mode::New || app.mode == Mode::Palette {
        let width = area.width.min(70);
        let height = if app.mode == Mode::New {
            app.wizard.len() as u16 + 5
        } else {
            ACTIONS.len() as u16 + 2
        };
        let popup = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height.min(area.height),
        );
        frame.render_widget(ratatui::widgets::Clear, popup);
        if app.mode == Mode::New {
            let lines = app
                .wizard
                .iter()
                .enumerate()
                .map(|(i, (name, value))| {
                    Line::from(Span::styled(
                        format!(
                            "{} {name}: {value}{}",
                            if i == app.field { "›" } else { " " },
                            if i == app.field { "█" } else { "" }
                        ),
                        if i == app.field {
                            Style::default().fg(Color::Cyan)
                        } else {
                            Style::default()
                        },
                    ))
                })
                .chain([
                    Line::from(format!("  Vendor SDK: {}  (F2)", app.vendor_sdk)),
                    Line::from("  Empty SDK path uses published registry packages."),
                ])
                .collect::<Vec<_>>();
            frame.render_widget(
                Paragraph::new(lines)
                    .block(
                        Block::default()
                            .title(if app.showcase {
                                " Start with RVST Debug "
                            } else {
                                " Create independent plugin "
                            })
                            .borders(Borders::ALL),
                    )
                    .wrap(Wrap { trim: false }),
                popup,
            );
        } else {
            let items = ACTIONS
                .iter()
                .map(|(label, _)| ListItem::new(*label))
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(app.palette));
            frame.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .title(" Command palette ")
                            .borders(Borders::ALL),
                    )
                    .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan)),
                popup,
                &mut state,
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn template_search_matches_description() {
        let (tx, rx) = mpsc::channel();
        let app = App {
            project: ".".into(),
            templates: vec![TemplateManifest {
                schema_version: 1,
                version: "0.1.0".into(),
                sdk_version: "^0.1".into(),
                id: "midi".into(),
                name: "Transpose".into(),
                description: "MIDI note processor".into(),
                inputs: Default::default(),
            }],
            selection: 0,
            filter: "note".into(),
            mode: Mode::Browse,
            wizard: vec![],
            field: 0,
            wizard_template: String::new(),
            showcase: false,
            vendor_sdk: false,
            logs: vec![],
            task: String::new(),
            command: String::new(),
            running: false,
            cancel: Cancellation::default(),
            tx,
            rx,
            palette: 0,
            workers: vec![],
            telemetry: crate::telemetry::Telemetry::default(),
        };
        assert_eq!(filtered(&app).len(), 1);
    }
    #[test]
    fn wizard_uses_same_generator_and_renders_without_blocking() {
        let root = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut app = App {
            project: root.path().into(),
            templates: vec![rvst_ops::Template::builtin("gain").unwrap().manifest],
            selection: 0,
            filter: String::new(),
            mode: Mode::Browse,
            wizard: vec![],
            field: 0,
            wizard_template: String::new(),
            showcase: false,
            vendor_sdk: false,
            logs: vec![],
            task: "Idle".into(),
            command: String::new(),
            running: false,
            cancel: Cancellation::default(),
            tx,
            rx,
            palette: 0,
            workers: vec![],
            telemetry: crate::telemetry::Telemetry::default(),
        };
        start_wizard(&mut app, false);
        app.wizard[0].1 = "my-plugin".into();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 35)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        launch_new(&mut app);
        match app.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
            Update::Finished(result) => assert!(result.is_ok(), "{result:?}"),
            Update::Log(_) => panic!("generation unexpectedly launched an external task"),
        }
        assert!(root.path().join("my-plugin/Cargo.toml").exists());
        let provenance: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("my-plugin/rvst-project.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(provenance["template"], "builtin:gain@0.1.0");
        assert_eq!(provenance["inputs"]["crate_name"], "my-plugin");
        assert!(app.command.contains("cargo rvst new"));
    }
}
