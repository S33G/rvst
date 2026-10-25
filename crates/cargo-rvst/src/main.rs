use cargo_rvst::{Cli, Output};
use clap::Parser;
fn main() {
    let args = cargo_rvst::normalize_args(std::env::args_os());
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
    let cli = match Cli::try_parse_from(args) {
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
