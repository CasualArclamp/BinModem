//! Which build this is, for the window to say.
//!
//! Two windows that will not talk to each other are, as often as not, two
//! different builds: one started before a change and left open, one after.
//! Nothing on the screen said so. The commit the program was built from, and
//! when that commit was made, are put where the masthead can print them; a
//! `+` after the commit means there were changes on top of it.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let when = git(&["log", "-1", "--format=%cd", "--date=format:%Y-%m-%d %H:%M"]).unwrap_or_default();
    println!("cargo:rustc-env=BINMODEM_BUILD={commit}{} {when}", if dirty { "+" } else { "" });
}
