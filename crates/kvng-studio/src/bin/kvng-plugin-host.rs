//! Plugin helper process; launched by KVNG Studio, not meant to be run by hand.

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = bridge::scan::run_helper_mode(&args).unwrap_or_else(|| bridge::server::run(&args));
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kvng-plugin-host: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
