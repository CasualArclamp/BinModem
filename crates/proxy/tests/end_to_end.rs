//! A page fetched through the whole thing.
//!
//! A real socket at each end and everything in between ours: a browser
//! connects to the client's listener and speaks SOCKS 5 at it, the request
//! crosses our TCP over a link that behaves like a modem, the server reads it,
//! opens a real connection to a real web server on the loopback, and the page
//! comes back the same way.
//!
//! The only thing missing from the picture is the modem itself, and the tests
//! in `crates/modem` put one under a link like this one.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use proxy::{Client, Server};

const SERVER: [u8; 4] = [10, 0, 0, 1];
const CLIENT: [u8; 4] = [10, 0, 0, 2];

/// A step of the link, and the delay across it.
///
/// Sixty milliseconds each way is a modem call without a VoIP trunk in it.
const STEP_MS: u32 = 10;
const DELAY_MS: u32 = 60;

/// The two ends and the link between them.
struct Link {
    client: Client,
    server: Server,
    clock: u32,
    /// Segments on their way: when they arrive, whether for the server, and
    /// the octets.
    flying: Vec<(u32, bool, Vec<u8>)>,
    lose_one_in: u32,
    crossed: u32,
}

impl Link {
    fn new(lose_one_in: u32) -> Self {
        let client = Client::new("127.0.0.1:0", CLIENT, SERVER, 11).expect("could not listen");
        Self {
            client,
            server: Server::new(SERVER, 22),
            clock: 0,
            flying: Vec::new(),
            lose_one_in,
            crossed: 0,
        }
    }

    fn step(&mut self) {
        self.clock += STEP_MS;
        for out in self.client.take_outgoing() {
            self.put(true, out.payload);
        }
        for out in self.server.take_outgoing() {
            self.put(false, out.payload);
        }

        let now = self.clock;
        let (arrived, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.flying)
            .into_iter()
            .partition(|(at, _, _)| *at <= now);
        self.flying = waiting;
        for (_, to_server, bytes) in arrived {
            if to_server {
                self.server.deliver(CLIENT, SERVER, &bytes);
            } else {
                self.client.deliver(SERVER, CLIENT, &bytes);
            }
        }

        self.client.tick(STEP_MS);
        self.server.tick(STEP_MS);
    }

    fn put(&mut self, to_server: bool, bytes: Vec<u8>) {
        self.crossed += 1;
        if self.lose_one_in > 0 && self.crossed.is_multiple_of(self.lose_one_in) {
            return;
        }
        self.flying.push((self.clock + DELAY_MS, to_server, bytes));
    }

    /// Run until `done`, or panic saying what the link was doing.
    fn run_until(&mut self, seconds: u32, done: impl Fn() -> bool) {
        for _ in 0..(seconds * 1000 / STEP_MS) {
            self.step();
            if done() {
                return;
            }
            // The two ends are threads away from the sockets they own, and a
            // busy loop here would starve them of the machine.
            thread::sleep(Duration::from_millis(1));
        }
        for line in self.client.take_log() {
            println!("  client: {line}");
        }
        for line in self.server.take_log() {
            println!("  server: {line}");
        }
        panic!("nothing happened in {seconds} s");
    }
}

/// A web server on the loopback: one request, one answer, and it says what it
/// was asked for so the test can tell the request crossed intact.
fn a_web_server(body: Vec<u8>) -> (String, thread::JoinHandle<Option<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().ok()?;
        socket
            .set_read_timeout(Some(Duration::from_secs(30)))
            .ok()?;
        // Read until the end of the request headers, which is all a GET is.
        let mut request = Vec::new();
        let mut buffer = [0u8; 512];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            match socket.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => request.extend_from_slice(&buffer[..n]),
                Err(_) => break,
            }
        }
        let mut answer = format!(
            "HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        answer.extend_from_slice(&body);
        socket.write_all(&answer).ok()?;
        // Closing is how HTTP/1.0 says the body has ended.
        drop(socket);
        Some(String::from_utf8_lossy(&request).into_owned())
    });
    (address, handle)
}

/// The browser's side: connect to the proxy, speak SOCKS 5, ask for a page.
fn a_browser(proxy: String, target: String) -> thread::JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut socket = TcpStream::connect(&proxy).map_err(|e| format!("{proxy}: {e}"))?;
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| e.to_string())?;

        // RFC 1928 3: version, one method, no authentication.
        socket.write_all(&[5, 1, 0]).map_err(|e| e.to_string())?;
        let mut greeting = [0u8; 2];
        socket.read_exact(&mut greeting).map_err(|e| e.to_string())?;
        if greeting != [5, 0] {
            return Err(format!("the proxy offered {greeting:?}"));
        }

        // 4: CONNECT to a name, which is what a browser sends.
        let (name, port) = target.rsplit_once(':').ok_or("no port")?;
        let port: u16 = port.parse().map_err(|_| "bad port")?;
        let mut request = vec![5, 1, 0, 3, name.len() as u8];
        request.extend_from_slice(name.as_bytes());
        request.extend_from_slice(&port.to_be_bytes());
        socket.write_all(&request).map_err(|e| e.to_string())?;

        let mut reply = [0u8; 10];
        socket.read_exact(&mut reply).map_err(|e| e.to_string())?;
        if reply[1] != 0 {
            return Err(format!("the proxy refused with {}", reply[1]));
        }

        socket
            .write_all(b"GET /page HTTP/1.0\r\nHost: example\r\n\r\n")
            .map_err(|e| e.to_string())?;
        let mut page = Vec::new();
        socket.read_to_end(&mut page).map_err(|e| e.to_string())?;
        Ok(page)
    })
}

#[test]
fn a_page_comes_back_through_the_proxy() {
    let body: Vec<u8> = (0..4_000u32)
        .map(|i| b"abcdefghijklmnopqrstuvwxyz"[(i % 26) as usize])
        .collect();
    let (web, web_thread) = a_web_server(body.clone());

    let mut link = Link::new(0);
    let proxy = link.client.bound().to_string();
    // 127.0.0.1 with a port, asked for by name so the far end resolves it --
    // which is the whole point of a proxy rather than a route.
    let target = web.replace("127.0.0.1", "localhost");
    let browser = a_browser(proxy, target);

    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watching = finished.clone();
    let waiter = thread::spawn(move || {
        let got = browser.join().expect("the browser thread panicked");
        watching.store(true, std::sync::atomic::Ordering::SeqCst);
        got
    });

    let done = finished.clone();
    link.run_until(120, || done.load(std::sync::atomic::Ordering::SeqCst));
    for line in link.server.take_log() {
        println!("  server: {line}");
    }

    let page = waiter.join().expect("the waiting thread panicked").expect("no page");
    let text = String::from_utf8_lossy(&page);
    assert!(text.starts_with("HTTP/1.0 200 OK"), "not a page: {:?}", &text[..60.min(text.len())]);
    let split = text.find("\r\n\r\n").expect("no end of headers");
    assert_eq!(
        page[split + 4..],
        body[..],
        "the body did not come back the way it went"
    );

    let request = web_thread.join().expect("the web thread panicked");
    let request = request.expect("the web server saw nothing");
    assert!(request.starts_with("GET /page HTTP/1.0"), "asked for {request:?}");
}

/// The same, over a link that loses one segment in eleven -- which is what the
/// TCP under it is for.
#[test]
fn a_page_comes_back_over_a_line_that_loses_things() {
    let body: Vec<u8> = (0..2_000u32).map(|i| (i % 251) as u8).collect();
    let (web, web_thread) = a_web_server(body.clone());

    let mut link = Link::new(11);
    let proxy = link.client.bound().to_string();
    let browser = a_browser(proxy, web);

    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watching = finished.clone();
    let waiter = thread::spawn(move || {
        let got = browser.join().expect("the browser thread panicked");
        watching.store(true, std::sync::atomic::Ordering::SeqCst);
        got
    });

    let started = Instant::now();
    let done = finished.clone();
    link.run_until(180, || done.load(std::sync::atomic::Ordering::SeqCst));
    let page = waiter.join().expect("the waiting thread panicked").expect("no page");
    let split = page
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no end of headers");
    assert_eq!(page[split + 4..], body[..], "the body arrived changed");
    println!(
        "  {} octets through a lossy link in {:.1} s",
        body.len(),
        started.elapsed().as_secs_f64()
    );
    let _ = web_thread.join();
}

/// A destination that is not there is refused in the client's own terms rather
/// than left hanging, which is what makes a browser show the right page.
#[test]
fn a_destination_that_is_not_there_is_refused() {
    // Bound and dropped, so nothing is listening on a port that certainly
    // existed a moment ago.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").to_string()
    };

    let mut link = Link::new(0);
    let proxy = link.client.bound().to_string();
    let browser = thread::spawn(move || -> u8 {
        let mut socket = TcpStream::connect(&proxy).expect("connect");
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .expect("timeout");
        socket.write_all(&[5, 1, 0]).expect("greeting");
        let mut greeting = [0u8; 2];
        socket.read_exact(&mut greeting).expect("no greeting back");

        let (name, port) = closed.rsplit_once(':').expect("no port");
        let port: u16 = port.parse().expect("bad port");
        let mut request = vec![5, 1, 0, 3, name.len() as u8];
        request.extend_from_slice(name.as_bytes());
        request.extend_from_slice(&port.to_be_bytes());
        socket.write_all(&request).expect("request");
        let mut reply = [0u8; 10];
        socket.read_exact(&mut reply).expect("no reply");
        reply[1]
    });

    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watching = finished.clone();
    let waiter = thread::spawn(move || {
        let got = browser.join().expect("the browser thread panicked");
        watching.store(true, std::sync::atomic::Ordering::SeqCst);
        got
    });

    let done = finished.clone();
    link.run_until(60, || done.load(std::sync::atomic::Ordering::SeqCst));
    let reply = waiter.join().expect("the waiting thread panicked");
    // RFC 1928 6: X'05' is "Connection refused".
    assert_eq!(reply, 5, "it was not told the connection was refused");
}
