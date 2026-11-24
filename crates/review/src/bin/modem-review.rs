//! Read a recording of a call and print what was on the line.
//!
//! ```text
//! modem-review [--fields] <recording.wav>...
//! ```

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut fields = false;
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--fields" => fields = true,
            "-h" | "--help" => {
                println!("modem-review [--fields] <recording.wav>...\n\n  --fields   everything read out of each signal, not just a line for it");
                return ExitCode::SUCCESS;
            }
            _ => paths.push(arg),
        }
    }
    if paths.is_empty() {
        eprintln!("modem-review: no recording named; --help says how");
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    for path in paths {
        match line::wav::read(&path) {
            Ok(wav) => {
                let name = std::path::Path::new(&path)
                    .file_name()
                    .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
                println!("{}", review::analyse(&wav, &name).text(fields));
            }
            Err(error) => {
                eprintln!("modem-review: {path}: {error}");
                failed = true;
            }
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
