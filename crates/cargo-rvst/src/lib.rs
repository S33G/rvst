use clap::{Parser, Subcommand, ValueEnum};
use rvst_ops::{Cancellation, Generator, NewRequest, OpError, Result};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{self, IsTerminal},
    path::PathBuf,
    sync::mpsc,
};
pub mod studio;
mod telemetry;

#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Output {
    Human,
    Json,
    Jsonl,
}
#[derive(Debug, Parser)]
#[command(
    name = "cargo rvst",
    version,
    about = "Build independent Rust VST3/CLAP plugins with RVST",
    long_about = "RVST plugin workbench. Every command except studio is non-interactive. JSON/JSONL are versioned machine interfaces. Exit codes: 0 success, 1 internal/I/O, 2 usage/input, 3 unavailable dependency, 4 conflict, 130 cancelled."
)]
pub struct Cli {
    #[arg(
        long = "format",
        visible_alias = "output",
        global = true,
        value_enum,
        default_value = "human"
    )]
    pub output: Output,
    #[arg(
        long,
        global = true,
        help = "Show a plan without generating, building, installing, or launching"
    )]
    pub dry_run: bool,
    #[arg(
        long,
        global = true,
        help = "Never enter an interactive console or prompt"
    )]
    pub non_interactive: bool,
    #[arg(long, global = true, help = "Suppress live human log output")]
    pub quiet: bool,
    #[arg(
        long,
        global = true,
        default_value = ".",
        help = "Working directory for project commands"
    )]
    pub project: PathBuf,
    #[command(flatten)]
    pub cargo: CargoOptions,
    #[command(subcommand)]
    pub command: Option<Commands>,
}
#[derive(Debug, Default, clap::Args)]
pub struct CargoOptions {
    #[arg(long, global = true)]
    pub manifest_path: Option<PathBuf>,
    #[arg(short = 'p', long = "package", global = true)]
    pub package: Option<String>,
    #[arg(long, global = true)]
    pub target: Option<String>,
    #[arg(long, global = true)]
    pub profile: Option<String>,
    #[arg(long, global = true, value_delimiter = ',')]
    pub features: Vec<String>,
}
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Build the RVST Debug feature tour, optionally install and run it.
    Showcase {
        #[arg(long, default_value = "./rvst-debug")]
        path: PathBuf,
        #[arg(long)]
        sdk_path: Option<PathBuf>,
        #[arg(long, requires = "sdk_path")]
        vendor_sdk: bool,
        #[arg(long)]
        install: bool,
        #[arg(long)]
        run: bool,
        #[arg(long)]
        open: bool,
    },
    /// Generate an independent plugin workspace and Git repository.
    New {
        path: PathBuf,
        #[arg(short, long, default_value = "gain")]
        template: String,
        #[arg(long = "set", value_name = "NAME=VALUE")]
        inputs: Vec<String>,
        #[arg(
            long,
            help = "Explicit development SDK override; recorded in provenance"
        )]
        sdk_path: Option<PathBuf>,
        #[arg(
            long,
            requires = "sdk_path",
            help = "Copy SDK into project for a portable independent build"
        )]
        vendor_sdk: bool,
        #[arg(long)]
        no_git: bool,
        #[arg(long)]
        vendor: Option<String>,
        #[arg(long)]
        id: Option<String>,
    },
    /// List, inspect, register, or validate declarative templates.
    Templates {
        #[command(subcommand)]
        command: TemplateCommand,
    },
    /// Check host tool availability and project shape.
    Doctor,
    /// Run the standalone desktop editor.
    Dev {
        #[arg(long)]
        web: bool,
        #[arg(long)]
        open: bool,
    },
    /// Run the plugin workspace test suite.
    Test,
    /// Test all workspace targets (DAW qualification remains separate).
    Validate,
    /// Print plugin diagnostics using its typed xtask entry point.
    Debug,
    /// Read live plugin state and metrics; optionally open its browser inspector.
    Inspect {
        #[arg(long)]
        open: bool,
        #[arg(long)]
        instance: Option<String>,
    },
    /// Reproduce a recorded DSP session through the plugin's typed xtask.
    Replay {
        recording: PathBuf,
        #[arg(long)]
        step: bool,
    },
    /// Build native distributable bundles.
    Package {
        #[arg(long="plugin-format",visible_alias="kind",default_value="all",value_parser=["all","vst3","clap"])]
        format: String,
        #[arg(long)]
        release: bool,
    },
    /// Print the machine interface contract and template input schema.
    Schema,
    /// Open the interactive keyboard-driven studio console.
    Studio,
}
#[derive(Debug, Subcommand)]
pub enum TemplateCommand {
    List,
    Inspect {
        template: String,
    },
    Add {
        name: String,
        #[arg(required_unless_present = "git")]
        source: Option<String>,
        #[arg(long, conflicts_with = "source", requires = "rev")]
        git: Option<String>,
        #[arg(long, help = "Required full 40-character commit SHA for Git sources")]
        rev: Option<String>,
    },
    Check {
        #[arg(required_unless_present = "path")]
        template: Option<String>,
        #[arg(long, conflicts_with = "template")]
        path: Option<PathBuf>,
    },
}
#[derive(Debug, Serialize)]
pub struct Envelope<T: Serialize> {
    pub schema_version: u32,
    pub event: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<OpError>,
}
pub fn emit<T: Serialize>(output: Output, event: &str, value: &T) {
    match output {
        Output::Human => println!("{}", serde_json::to_string_pretty(value).unwrap()),
        Output::Json | Output::Jsonl => println!(
            "{}",
            serde_json::to_string(&Envelope {
                schema_version: 1,
                event: event.into(),
                ok: true,
                data: Some(value),
                error: None
            })
            .unwrap()
        ),
    }
}
pub fn emit_error(output: Output, error: OpError) -> i32 {
    let code = error.exit_code;
    match output {
        Output::Human => {
            eprintln!("{error}");
            if error.code == "doctor_failed"
                && let Some(checks) = error
                    .details
                    .as_ref()
                    .and_then(|details| details["checks"].as_array())
            {
                for check in checks.iter().filter(|check| check["ok"] == false) {
                    let name = check["name"].as_str().unwrap_or("Unknown check");
                    let status = if check["required"] == true {
                        "FAIL (required)"
                    } else {
                        "WARN (optional)"
                    };
                    eprintln!("  {name}: {status}");
                    if let Some(detail) = check["detail"].as_str().filter(|s| !s.is_empty()) {
                        eprintln!("    {detail}");
                    }
                    if let Some(setup) = check["setup"].as_str() {
                        eprintln!("    Setup: {setup}");
                    }
                }
            }
        }
        _ => println!(
            "{}",
            serde_json::to_string(&Envelope::<serde_json::Value> {
                schema_version: 1,
                event: "error".into(),
                ok: false,
                data: None,
                error: Some(error)
            })
            .unwrap()
        ),
    }
    code
}
pub fn normalize_args(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<_> = args.into_iter().collect();
    if args.get(1).is_some_and(|a| a == "rvst") {
        args.remove(1);
    }
    args
}
pub fn run_cli(command_name: &str) {
    use clap::{CommandFactory, FromArgMatches};

    let args = normalize_args(std::env::args_os());
    let output = if args
        .windows(2)
        .any(|a| (a[0] == "--output" || a[0] == "--format") && a[1] == "jsonl")
        || args
            .iter()
            .any(|a| a == "--output=jsonl" || a == "--format=jsonl")
    {
        Output::Jsonl
    } else if args
        .windows(2)
        .any(|a| (a[0] == "--output" || a[0] == "--format") && a[1] == "json")
        || args
            .iter()
            .any(|a| a == "--output=json" || a == "--format=json")
    {
        Output::Json
    } else {
        Output::Human
    };
    let cli = match Cli::command()
        .bin_name(command_name.to_owned())
        .try_get_matches_from(args)
        .and_then(|matches| Cli::from_arg_matches(&matches))
    {
        Ok(cli) => cli,
        Err(error) => {
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                error.print().ok();
                return;
            }
            std::process::exit(emit_error(
                output,
                rvst_ops::OpError::new("usage", error.to_string(), 2),
            ));
        }
    };
    let output = cli.output;
    if let Err(error) = run(cli) {
        std::process::exit(emit_error(output, error));
    }
}
pub fn input_pairs(pairs: Vec<String>) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(OpError::new(
                "invalid_input",
                "--set requires NAME=VALUE",
                2,
            ));
        };
        if result.insert(key.into(), value.into()).is_some() {
            return Err(OpError::new(
                "duplicate_input",
                format!("Input {key} supplied twice"),
                2,
            ));
        }
    }
    Ok(result)
}
pub fn run(mut cli: Cli) -> Result<()> {
    if let Some(path) = &cli.cargo.manifest_path {
        let absolute = std::fs::canonicalize(path)?;
        cli.project = absolute
            .parent()
            .ok_or_else(|| OpError::new("invalid_manifest", "Manifest has no parent", 2))?
            .to_path_buf();
        cli.cargo.manifest_path = Some(absolute);
    }
    let Some(command) = cli.command else {
        if cli.non_interactive || cli.output != Output::Human || !io::stdin().is_terminal() {
            return Err(OpError::new(
                "command_required",
                "Specify a command; use --help for commands",
                2,
            ));
        }
        return studio::run(&cli.project);
    };
    match command {
        Commands::Showcase {
            path,
            sdk_path,
            vendor_sdk,
            install,
            run,
            open,
        } => {
            let request = rvst_ops::ShowcaseRequest {
                path: path.clone(),
                sdk_path,
                vendor_sdk,
                install,
            };
            let plan = rvst_ops::showcase_plan(&request)?;
            if cli.dry_run {
                emit(
                    cli.output,
                    "showcase_plan",
                    &serde_json::json!({"plan":plan,"run":run,"open":open}),
                );
                return Ok(());
            }
            let cancel = Cancellation::default();
            let ctrl = cancel.clone();
            ctrlc::set_handler(move || ctrl.cancel())
                .map_err(|e| OpError::new("signal_handler", e.to_string(), 1))?;
            let (tx, logger) = log_forwarder(cli.output, cli.quiet);
            let result = rvst_ops::showcase(request, &cancel, Some(tx));
            let _ = logger.join();
            let report = result?;
            if !run && !open {
                emit(cli.output, "showcase_built", &report);
                return Ok(());
            }
            let project: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path.join("rvst-project.json"))?)
                    .map_err(|e| OpError::new("invalid_project", e.to_string(), 2))?;
            let project_name = project["inputs"]["crate_name"]
                .as_str()
                .ok_or_else(|| OpError::new("invalid_project", "Missing project identity", 2))?;
            let plugin_id = project["inputs"]["plugin_id"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("org.rvst.{project_name}"));
            let live = rvst_devtools::discover()?
                .into_iter()
                .filter(|s| s.plugin_id == plugin_id)
                .collect::<Vec<_>>();
            if !live.is_empty() {
                if open {
                    open_url(&live[0].url)?;
                }
                emit(cli.output, "showcase_running", &live);
                return Ok(());
            }
            if !run {
                return Err(OpError::new(
                    "inspector_unavailable",
                    "Showcase is built; use --run --open to start its standalone inspector",
                    3,
                ));
            }
            let command = rvst_ops::task_command("dev", false, "all", None)?;
            let runner_path = path.clone();
            let runner_cancel = cancel.clone();
            let (tx, logger) = log_forwarder(cli.output, cli.quiet);
            let runner = std::thread::spawn(move || {
                rvst_ops::run_task(&runner_path, &command, &runner_cancel, Some(tx))
            });
            let mut open_result = Ok(());
            if open {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                loop {
                    if cancel.cancelled() || runner.is_finished() {
                        break;
                    }
                    match rvst_devtools::discover() {
                        Ok(sessions) => {
                            if let Some(session) =
                                sessions.iter().find(|s| s.plugin_id == plugin_id)
                            {
                                open_result = open_url(&session.url);
                                break;
                            }
                        }
                        Err(error) => {
                            open_result = Err(error.into());
                            break;
                        }
                    }
                    if std::time::Instant::now() > deadline {
                        open_result = Err(OpError::new(
                            "inspector_unavailable",
                            "Standalone started no inspector within 60 seconds",
                            3,
                        ));
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            if open_result.is_err() {
                cancel.cancel();
            }
            let report = runner
                .join()
                .map_err(|_| OpError::new("runner_failed", "Standalone worker panicked", 1))??;
            let _ = logger.join();
            open_result?;
            if report.exit_code != 0 {
                return Err(OpError::new(
                    if report.exit_code == 130 {
                        "cancelled"
                    } else {
                        "task_failed"
                    },
                    "Showcase runner stopped",
                    if report.exit_code == 130 { 130 } else { 1 },
                )
                .with_details(serde_json::to_value(report).unwrap()));
            }
            emit(cli.output, "showcase_finished", &report);
        }
        Commands::New {
            path,
            template,
            inputs,
            sdk_path,
            vendor_sdk,
            no_git,
            vendor,
            id,
        } => {
            let source = rvst_ops::resolve_template(&template)?;
            let mut inputs = input_pairs(inputs)?;
            for (name, value) in [("vendor", vendor), ("plugin_id", id)] {
                if let Some(value) = value
                    && inputs.insert(name.into(), value).is_some()
                {
                    return Err(OpError::new(
                        "duplicate_input",
                        format!("{name} supplied twice"),
                        2,
                    ));
                }
            }
            if cli.dry_run {
                let manifest = rvst_ops::inspect_template(&template)?;
                let name = path.file_name().and_then(|v| v.to_str()).ok_or_else(|| {
                    OpError::new(
                        "invalid_destination",
                        "Destination needs a UTF-8 directory name",
                        2,
                    )
                })?;
                let values = manifest.resolve_inputs(inputs, name)?;
                if path.exists() {
                    return Err(OpError::new(
                        "destination_exists",
                        "Destination already exists",
                        4,
                    ));
                }
                emit(
                    cli.output,
                    "new_plan",
                    &serde_json::json!({"path":path,"template":manifest.manifest.id,"inputs":values,"sdk_path":sdk_path,"vendor_sdk":vendor_sdk,"git":!no_git}),
                );
                return Ok(());
            }
            let report = Generator::new().generate(NewRequest {
                template: source,
                destination: path,
                inputs,
                sdk_path,
                vendor_sdk,
                git: !no_git,
            })?;
            emit(cli.output, "project_created", &report);
        }
        Commands::Templates { command } => match command {
            TemplateCommand::List => emit(cli.output, "templates", &rvst_ops::list_templates()?),
            TemplateCommand::Inspect { template } => {
                let t = rvst_ops::inspect_template(&template)?;
                emit(
                    cli.output,
                    "template",
                    &serde_json::json!({"manifest":t.manifest,"provenance":t.provenance}),
                );
            }
            TemplateCommand::Add {
                name,
                source,
                git,
                rev,
            } => {
                let source = git.or(source).ok_or_else(|| {
                    OpError::new("missing_source", "Supply SOURCE or --git URL", 2)
                })?;
                if cli.dry_run {
                    if let Some(rev) = &rev {
                        if rev.len() != 40 || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return Err(OpError::new(
                                "unpinned_git",
                                "Git templates require a full 40-character SHA",
                                2,
                            ));
                        }
                    } else {
                        rvst_ops::Template::load(std::path::Path::new(&source))?.check()?;
                    }
                    emit(
                        cli.output,
                        "template_registration_plan",
                        &serde_json::json!({"name":name,"source":source,"revision":rev}),
                    );
                } else {
                    emit(
                        cli.output,
                        "template_registered",
                        &rvst_ops::register_template(&name, &source, rev.as_deref())?,
                    );
                }
            }
            TemplateCommand::Check { template, path } => {
                let template = template
                    .or_else(|| path.map(|p| p.to_string_lossy().into_owned()))
                    .ok_or_else(|| {
                        OpError::new("missing_template", "Supply TEMPLATE or --path", 2)
                    })?;
                let t = rvst_ops::inspect_template(&template)?;
                t.check()?;
                emit(
                    cli.output,
                    "template_valid",
                    &serde_json::json!({"id":t.manifest.id,"valid":true}),
                );
            }
        },
        Commands::Doctor => {
            let result = rvst_ops::doctor(&cli.project);
            if result["checks"]
                .as_array()
                .is_some_and(|a| a.iter().any(|c| c["required"] == true && c["ok"] == false))
            {
                return Err(OpError::new(
                    "doctor_failed",
                    "One or more required tools are unavailable",
                    3,
                )
                .with_details(result));
            }
            emit(cli.output, "doctor", &result);
        }
        Commands::Schema => emit(cli.output, "schema", &schema()),
        Commands::Studio => {
            if cli.non_interactive || cli.output != Output::Human {
                return Err(OpError::new(
                    "interactive_required",
                    "studio requires human output and interactive input",
                    2,
                ));
            }
            studio::run(&cli.project)?;
        }
        Commands::Inspect { open, instance } => {
            let mut sessions = rvst_devtools::discover()?;
            if let Some(id) = instance {
                sessions.retain(|session| session.id == id);
                if sessions.is_empty() {
                    return Err(OpError::new(
                        "instance_not_found",
                        "No inspector with that instance ID",
                        2,
                    ));
                }
            }
            if open && !cli.dry_run {
                if sessions.is_empty() {
                    return Err(OpError::new(
                        "inspector_unavailable",
                        "No running plugin inspector found; load a development plugin first",
                        3,
                    ));
                }
                if sessions.len() != 1 {
                    return Err(OpError::new(
                        "instance_required",
                        "Multiple inspectors running; select --instance ID",
                        2,
                    ));
                }
                open_url(&sessions[0].url)?;
            }
            let snapshots = sessions
                .iter()
                .map(|session| rvst_devtools::snapshot(session, 0))
                .collect::<std::io::Result<Vec<_>>>()?;
            emit(cli.output, "inspectors", &snapshots);
        }
        command => {
            if !cli.project.join("Cargo.toml").is_file() {
                return Err(OpError::new(
                    "project_not_found",
                    "Working directory has no Cargo.toml; use --project PATH",
                    2,
                ));
            }
            let web_open = matches!(command, Commands::Dev { open: true, .. });
            let replay_step = matches!(command, Commands::Replay { step: true, .. });
            let (task, release, format, input) = match command {
                Commands::Dev { web: false, .. } => ("dev", false, "all", None),
                Commands::Dev { web: true, .. } => ("web", false, "all", None),
                Commands::Test => ("test", false, "all", None),
                Commands::Validate => ("validate", false, "all", None),
                Commands::Debug => ("debug", false, "all", None),
                Commands::Replay { recording, .. } => ("replay", false, "all", Some(recording)),
                Commands::Package {
                    ref format,
                    release,
                } => ("package", release, format.as_str(), None),
                _ => unreachable!(),
            };
            let mut command = rvst_ops::task_command(task, release, format, input.as_deref())?;
            if replay_step {
                command.push("--step".into());
            }
            if task == "web" && web_open {
                command.push("--open".into());
            }
            apply_cargo_options(
                &mut command,
                &cli.cargo,
                matches!(task, "package" | "web" | "debug" | "replay"),
            );
            if cli.dry_run {
                emit(
                    cli.output,
                    "task_plan",
                    &serde_json::json!({"directory":cli.project,"task":task,"command":command}),
                );
                return Ok(());
            }
            let cancel = Cancellation::default();
            let ctrl = cancel.clone();
            ctrlc::set_handler(move || ctrl.cancel())
                .map_err(|e| OpError::new("signal_handler", e.to_string(), 1))?;
            if cli.output == Output::Jsonl {
                emit(
                    cli.output,
                    "task_started",
                    &serde_json::json!({"task":task,"command":command}),
                );
            }
            let (tx, logger) = log_forwarder(cli.output, cli.quiet);
            let result = rvst_ops::run_task(&cli.project, &command, &cancel, Some(tx));
            let _ = logger.join();
            let report = result?;
            let exit = report.exit_code;
            if exit != 0 {
                return Err(OpError::new(
                    if exit == 130 {
                        "cancelled"
                    } else {
                        "task_failed"
                    },
                    format!("{task} exited with status {exit}"),
                    if exit == 130 { 130 } else { 1 },
                )
                .with_details(serde_json::to_value(report).unwrap()));
            }
            emit(cli.output, "task_finished", &report);
        }
    }
    Ok(())
}
fn log_forwarder(
    output: Output,
    quiet: bool,
) -> (
    mpsc::Sender<rvst_ops::LogEvent>,
    std::thread::JoinHandle<()>,
) {
    let (tx, rx) = mpsc::channel::<rvst_ops::LogEvent>();
    let logger = std::thread::spawn(move || {
        for event in rx {
            match output {
                Output::Jsonl => emit(output, "log", &event),
                Output::Human if !quiet => eprintln!("{}", event.line),
                _ => {}
            }
        }
    });
    (tx, logger)
}
fn open_url(url: &str) -> Result<()> {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let status = cmd
        .arg(url)
        .status()
        .map_err(|e| OpError::new("browser_unavailable", e.to_string(), 3))?;
    if !status.success() {
        return Err(OpError::new(
            "browser_unavailable",
            "Unable to open inspector URL",
            3,
        ));
    }
    Ok(())
}
pub fn schema() -> serde_json::Value {
    serde_json::json!({"json_schema":serde_json::from_str::<serde_json::Value>(include_str!("../schemas/result-v1.json")).expect("valid result schema"),"recording_schema":serde_json::from_str::<serde_json::Value>(include_str!("../schemas/recording-v1.json")).expect("valid recording schema"),"schema_version":1,"commands":["showcase","new","templates list","templates inspect","templates add","templates check","doctor","dev","test","validate","debug","inspect","replay","package","schema","studio"],"output_modes":["human","json","jsonl"],"envelope":{"schema_version":"integer","event":"string","ok":"boolean","data":"object (success)","error":{"code":"string","message":"string","exit_code":"integer"}},"exit_codes":{"0":"success","1":"operation, test, or I/O failure","2":"usage or invalid input","3":"unavailable dependency","4":"conflict","130":"cancelled"},"template":{"schema_version":1,"required_files":["rvst-template.toml","README.md","files/"],"input_types":["string","identifier","bool","integer","number","choice"],"hooks":false,"symlinks":false},"validation_scope":"Cargo tests across targets; format validators and DAW acceptance must be run separately"})
}

fn apply_cargo_options(command: &mut Vec<String>, options: &CargoOptions, xtask: bool) {
    let split = command
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(command.len());
    let mut extra = vec![];
    if let Some(manifest) = &options.manifest_path {
        extra.extend([
            "--manifest-path".into(),
            manifest.to_string_lossy().into_owned(),
        ]);
    }
    if !xtask && let Some(package) = &options.package {
        if let Some(pos) = command.iter().position(|arg| arg == "-p") {
            command[pos + 1] = package.clone();
        } else {
            extra.extend(["-p".into(), package.clone()]);
        }
    }
    if !xtask {
        for (flag, value) in [
            ("--target", &options.target),
            ("--profile", &options.profile),
        ] {
            if let Some(value) = value {
                extra.extend([flag.into(), value.clone()]);
            }
        }
        if !options.features.is_empty() {
            extra.extend(["--features".into(), options.features.join(",")]);
        }
    }
    command.splice(split..split, extra);
    if xtask {
        for (flag, value) in [
            ("--package", &options.package),
            ("--target", &options.target),
            ("--profile", &options.profile),
        ] {
            if let Some(value) = value {
                command.extend([flag.into(), value.clone()]);
            }
        }
        if !options.features.is_empty() {
            command.extend(["--features".into(), options.features.join(",")]);
        }
    }
}
