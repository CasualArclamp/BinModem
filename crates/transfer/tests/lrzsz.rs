//! The receiver against `sz` from lrzsz, where there is one to run.
//!
//! A round trip against this crate's own Sender proves the two halves agree
//! with each other, which is not the question: a board runs somebody else's
//! sender. Two things `sz` does that the Sender here never does went
//! unnoticed for that reason -- a ZSINIT at the start, and a batch -- and both
//! lost files. So the receiver is run against the real thing.
//!
//! Where `sz` is not on the PATH, which includes the Windows runner, each test
//! says so and passes: there is nothing to run it against.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use transfer::zmodem::{Received, Receiver, State};

/// Run `sz` over `files` with `flags`, and return what the receiver kept.
fn from_sz(flags: &[&str], files: &[(&str, Vec<u8>)]) -> Option<(Vec<Received>, State)> {
    let dir = scratch_dir();
    for (name, data) in files {
        std::fs::write(dir.join(name), data).expect("could not write a file to send");
    }
    let mut command = Command::new("sz");
    command
        .args(flags)
        .args(files.iter().map(|(name, _)| *name))
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else {
        eprintln!("no sz on the PATH; nothing to test against");
        return None;
    };
    let mut to_sz = child.stdin.take().expect("no stdin");
    let mut from = child.stdout.take().expect("no stdout");
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = from.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut receiver = Receiver::default();
    let mut arrived = Vec::new();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(30) {
        let out = receiver.take_out();
        if !out.is_empty() && to_sz.write_all(&out).and_then(|()| to_sz.flush()).is_err() {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(bytes) => receiver.feed(&bytes),
            Err(mpsc::RecvTimeoutError::Timeout) => receiver.tick(20),
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        arrived.extend(receiver.take_arrived());
        if matches!(receiver.state(), State::Done | State::Failed(_)) {
            break;
        }
    }
    // What is left to say -- the ZFIN's answer -- and then let it go.
    let _ = to_sz.write_all(&receiver.take_out());
    drop(to_sz);
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    Some((arrived, receiver.state()))
}

fn scratch_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "binmodem-lrzsz-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("could not make a scratch directory");
    dir
}

fn names(arrived: &[Received]) -> Vec<&str> {
    arrived.iter().map(|r| r.file.name.as_str()).collect()
}

#[test]
fn one_file_from_sz() {
    let data: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 256) as u8).collect();
    let Some((arrived, state)) = from_sz(&[], &[("ONE.BIN", data.clone())]) else { return };
    assert_eq!(state, State::Done);
    assert_eq!(names(&arrived), ["ONE.BIN"]);
    assert_eq!(arrived[0].data, data);
}

#[test]
fn a_batch_from_sz_keeps_every_file() {
    let one = b"MAIN MENU\r\n".repeat(400);
    let two: Vec<u8> = (0..50_000u32).map(|i| (i % 253) as u8).collect();
    let files = [("FIRST.TXT", one.clone()), ("SECOND.BIN", two.clone())];
    let Some((arrived, state)) = from_sz(&[], &files) else { return };
    assert_eq!(state, State::Done);
    assert_eq!(names(&arrived), ["FIRST.TXT", "SECOND.BIN"]);
    assert_eq!(arrived[0].data, one);
    assert_eq!(arrived[1].data, two);
}

/// `sz -e` escapes control characters, and says so first with a ZSINIT.
#[test]
fn sz_that_starts_with_a_zsinit_is_answered() {
    let data: Vec<u8> = (0..=255u8).cycle().take(8_000).collect();
    let Some((arrived, state)) = from_sz(&["-e"], &[("CTRL.BIN", data.clone())]) else { return };
    assert_eq!(state, State::Done);
    assert_eq!(names(&arrived), ["CTRL.BIN"]);
    assert_eq!(arrived[0].data, data);
}

/// `sz -o` sends 16-bit check sequences although the receiver offered 32.
#[test]
fn sz_with_sixteen_bit_checks() {
    let data = b"16 bit\r\n".repeat(2_000);
    let Some((arrived, state)) = from_sz(&["-o"], &[("SHORT.TXT", data.clone())]) else { return };
    assert_eq!(state, State::Done);
    assert_eq!(arrived[0].data, data);
}
