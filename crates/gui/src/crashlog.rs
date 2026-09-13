//! Where a crash goes when there is nobody watching a terminal.
//!
//! A panic in the window thread closes the window, and a panic in the line
//! thread leaves the window open with a modem behind it that has quietly
//! stopped. Neither says anything. This program is tested by somebody placing
//! a real call on another machine, so "it crashed" is often the whole of the
//! report, and a panic that leaves nothing behind cannot be answered with
//! anything better than a guess.
//!
//! The shipped build is stripped, so there is no backtrace to be had. There
//! does not need to be: the panic location is a `&'static str` and a line
//! number compiled into the binary, which survives stripping and is almost
//! always the whole answer.

use std::io::Write;
use std::path::PathBuf;

/// The file crashes are appended to, beside the program rather than beside the
/// source: this is one file handed to somebody, and a report nobody can find
/// is a report that was not made.
pub fn path() -> PathBuf {
    let name = "crashes.txt";
    match std::env::current_exe() {
        Ok(exe) => exe.parent().map_or_else(|| PathBuf::from(name), |d| d.join(name)),
        Err(_) => PathBuf::from(name),
    }
}

/// Write every panic down before letting it do what it was going to do.
///
/// Appended rather than replaced. A panic in the line thread does not stop the
/// program, so there can be a second one, and the first is usually the one
/// worth reading.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let where_ = info
            .location()
            .map_or_else(|| "somewhere".to_owned(), ToString::to_string);
        // The payload is only ever a string or a formatted string in practice,
        // and anything else is worth saying so about rather than dropping.
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic with nothing to say".to_owned());
        let thread = std::thread::current();
        let who = thread.name().unwrap_or("unnamed").to_owned();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());

        let line = format!(
            "{stamp} {} thread {who}\n    {where_}\n    {what}\n",
            env!("CARGO_PKG_VERSION")
        );
        eprint!("{line}");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path())
        {
            let _ = file.write_all(line.as_bytes());
        }
        previous(info);
    }));
}
