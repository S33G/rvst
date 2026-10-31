use cargo_rvst::{Cli, Output};
use clap::{CommandFactory, FromArgMatches};
fn main() {
    let args = cargo_rvst::normalize_args(std::env::args_os());
    let invoked_as_cargo_subcommand = args
        .first()
        .and_then(|arg| std::path::Path::new(arg).file_name())
        .is_some_and(|name| name == "cargo-rvst");
    let command_name = if invoked_as_cargo_subcommand {
        "cargo rvst"
    } else {
        "rvst"
    };
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
        .name(command_name)
        .try_get_matches_from(args)
        .and_then(|matches| Cli::from_arg_matches(&matches))
    {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(
                e.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) {
                e.print().ok();
                return;
            }
            std::process::exit(cargo_rvst::emit_error(
                output,
                rvst_ops::OpError::new("usage", e.to_string(), 2),
            ));
        }
    };
    let output = cli.output;
    if let Err(error) = cargo_rvst::run(cli) {
        std::process::exit(cargo_rvst::emit_error(output, error));
    }
}
