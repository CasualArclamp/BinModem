//! A modem sitting on the line waiting to be dialled: a dial-in server.
//!
//! The companion to `--live`, and in the same program as it. That window has
//! a modem in it and a terminal wired to it, and nothing to ring. This is the
//! thing that answers.
//!
//! Both point at the same virtual cable, and that is not a compromise: what
//! comes back from a cable is what was written to it, a little later, summed
//! with whatever else is writing. Which is a two-wire pair, exactly, with two
//! modems across it. Each hears the other and its own reflection, which is the
//! situation every one of these modulations was designed for.
//!
//! Once connected it is what a caller to a provider met: a banner, `login:`,
//! `Password:`, and a prompt where `ppp` starts PPP -- or PPP straight away,
//! for a dialler that sends frames from the start, with the same account
//! asked for over CHAP or PAP. Echoes that arrive over the link are answered.
//! Everything the caller is shown is printed here too, so what came back on
//! the calling screen can be checked against what was sent.

use std::process::ExitCode;
use std::time::Duration;

use login::server::{self, Server};
use login::Account;
use modem::{Modem, State};
use ppp::link::{Authentication, Link};

use crate::network::{CLIENT_ADDRESS, SERVER_ADDRESS};

const FS: f64 = 16_000.0;

/// How loud to write to the line.
///
/// Two modems share the cable and it sums them, so each has to leave room for
/// the other. Half is generous: pulse shaping puts the peak of a single modem
/// well above its own average, and a clipped handshake is a failed one.
const LEVEL: f32 = 0.45;

/// What is running above the modem on the call.
enum Above {
    Nothing,
    Login(Box<Server>),
    Ppp { link: Box<Link>, announced: bool },
}

/// Run the answering modem. `args` is what followed `--answer`.
pub fn run(args: Vec<String>) -> ExitCode {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut carrier = "V22B".to_owned();
    let mut banner = "*** THE DEAD ZONE BBS ***\n  24 hours - SysOp: nobody".to_owned();
    let mut account = Account::new("guest", "");

    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || rest.next().cloned().unwrap_or_default();
        match arg.as_str() {
            "--in" => input = Some(value()),
            "--out" => output = Some(value()),
            "--carrier" => carrier = value().to_ascii_uppercase(),
            "--banner" => banner = value(),
            "--user" => account.name = value(),
            "--password" => account.password = value(),
            "--help" | "-h" => {
                println!(
                    "binmodem --answer --in <device> --out <device> \
                     [--carrier B103|V22B|V32|V34] [--banner <text>]\n\
                     \x20                [--user <name>] [--password <password>]\n\
                     \n\
                     Answers calls on a virtual cable with a login prompt, so that\n\
                     `binmodem --live` on the same cable has something to dial.\n\
                     Log in as the user (guest, with no password, unless told\n\
                     otherwise) and type ppp, or start PPP straight away and give\n\
                     the same account over PAP. Both devices must be named."
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let (Some(input), Some(output)) = (input, output) else {
        eprintln!("both --in and --out are required; see --help");
        return ExitCode::FAILURE;
    };

    let audio = match line::Duplex::open(Some(&input), Some(&output), FS) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not open the line: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "out: {} at {} Hz\nin:  {} at {} Hz\nanswering as {carrier}, logins as {}; ctrl-c to stop",
        audio.output_device, audio.output_rate, audio.input_device, audio.input_rate, account.name
    );

    let mut host = Modem::new(FS);
    for b in format!("AT+MS={carrier}\r").bytes() {
        host.feed_dte(b);
    }
    host.take_dte();
    // Off hook and waiting. There is no ring on a virtual cable, so this
    // answers straight away and waits for a calling modem to appear.
    for b in b"ATA\r" {
        host.feed_dte(*b);
    }

    let config = server::Config {
        banner,
        accounts: vec![account.clone()],
        ppp_message: format!(
            "PPP session from {} to {} beginning....",
            dotted(SERVER_ADDRESS),
            dotted(CLIENT_ADDRESS)
        ),
        ..server::Config::default()
    };

    let mut from_line: Vec<f32> = Vec::with_capacity(4096);
    let mut to_line: Vec<f32> = Vec::with_capacity(4096);
    let mut above = Above::Nothing;
    let mut last_phase = "";
    let mut last_ec = "";
    let mut owed_ms = 0.0f64;
    let started = std::time::Instant::now();
    let stamp = || started.elapsed().as_secs_f64();

    loop {
        from_line.clear();
        audio.receive(&mut from_line);
        if from_line.is_empty() {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        to_line.clear();
        for &s in &from_line {
            // A V.90 server's samples are codewords, and the far encoder
            // only turns them back into the same ones at the level they left.
            let level = if host.exact_levels() { 1.0 } else { LEVEL };
            to_line.push(host.step(f64::from(s)) as f32 * level);
        }
        audio.transmit(&to_line);
        owed_ms += from_line.len() as f64 / FS * 1000.0;
        let ms = owed_ms as u32;
        owed_ms -= f64::from(ms);

        let phase = host.line_phase();
        if phase != last_phase {
            last_phase = phase;
            println!("[{:>6.2}s {phase}]", stamp());
        }

        // The same trace for the layer above, so that a call which connects
        // without error control says which step it lost it at.
        let ec = host.error_control_phase();
        if ec != last_ec {
            last_ec = ec;
            if !ec.is_empty() {
                println!("[{:>6.2}s V.42 {ec}]", stamp());
            }
        }

        let heard = host.take_dte();
        match (&mut above, host.state()) {
            (Above::Nothing, State::Data) => {
                println!(
                    "[{:>6.2}s connected at {}, error control {}, compression {}]",
                    stamp(),
                    crate::live::line_rates(&host),
                    host.error_control_detail(),
                    host.compression_name().unwrap_or("off")
                );
                let mut server = Box::new(Server::new(config.clone()));
                print_notes(&mut server, stamp());
                above = Above::Login(server);
            }
            (Above::Nothing, _) => {}
            (_, State::Command) => {
                println!("\n[{:>6.2}s the call ended]", stamp());
                above = Above::Nothing;
                for b in b"ATA\r" {
                    host.feed_dte(*b);
                }
            }
            (Above::Login(server), _) => {
                server.feed(&heard);
                server.tick(ms);
                let out = server.take_output();
                print_text(&out);
                for b in out {
                    host.feed_dte(b);
                }
                print_notes(server, stamp());
                match server.take_outcome() {
                    Some(server::Outcome::Ppp { user, early }) => {
                        let mut link = Box::new(Link::with_authentication(
                            SERVER_ADDRESS,
                            CLIENT_ADDRESS,
                            Authentication {
                                // Logged in at the prompt is enough. A caller
                                // that went straight to PPP is asked here.
                                callers: user.is_none().then(|| vec![account.clone()]),
                                name: "binmodem".to_owned(),
                                seed: crate::dialin::challenge_seed(),
                                ..Authentication::default()
                            },
                        ));
                        link.open();
                        link.feed(&early);
                        above = Above::Ppp { link, announced: false };
                    }
                    Some(server::Outcome::HangUp(why)) => {
                        println!("\n[{:>6.2}s hanging up: {why}]", stamp());
                        host.hang_up();
                    }
                    None => {}
                }
            }
            (Above::Ppp { link, announced }, _) => {
                link.feed(&heard);
                link.tick(ms);
                for b in link.take_line() {
                    host.feed_dte(b);
                }
                for echo in link.take_arrived() {
                    if !echo.echo.reply {
                        println!("[{:>6.2}s ping from {}, seq {}]", stamp(), dotted(echo.from), echo.echo.sequence);
                    }
                }
                let _ = link.take_carried();
                if link.up() && !*announced {
                    *announced = true;
                    let who = link.who().map(|w| format!(", {w} over PAP or CHAP")).unwrap_or_default();
                    println!(
                        "[{:>6.2}s PPP up: {} is {}{who}]",
                        stamp(),
                        dotted(CLIENT_ADDRESS),
                        dotted(SERVER_ADDRESS)
                    );
                }
                if link.ended() {
                    println!(
                        "[{:>6.2}s PPP down: {}; hanging up]",
                        stamp(),
                        link.trouble().unwrap_or("it was put down")
                    );
                    host.hang_up();
                }
            }
        }
    }
}

fn dotted([a, b, c, d]: [u8; 4]) -> String {
    format!("{a}.{b}.{c}.{d}")
}

fn print_text(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    print!("{}", String::from_utf8_lossy(bytes));
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

fn print_notes(server: &mut Server, at: f64) {
    for note in server.take_notes() {
        println!("\n[{at:>6.2}s {note}]");
    }
}
