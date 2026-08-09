use std::process::ExitCode;

fn main() -> ExitCode {
    ExitCode::from(revidere_cli::run(std::env::args().skip(1)))
}
