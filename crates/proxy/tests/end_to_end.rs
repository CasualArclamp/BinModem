//! A page fetched through the whole thing.
//!
//! A real socket at each end and everything in between ours: a browser
//! connects to the client's listener and speaks HTTP at it -- a request, or a
//! CONNECT for a tunnel -- the request crosses our TCP over a link that
//! behaves like a modem, the far BinModem reads it, opens a real connection
//! to a real web server on the loopback, and the page comes back the same way.
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
/// Two hundred and thirty milliseconds each way is what a real call between
/// two of these measures: 460 ms round trip, which is four times what a modem
/// alone costs and is the number every timer above this has to live with.
const STEP_MS: u32 = 10;
const DELAY_MS: u32 = 230;

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
            server: {
                let mut server = Server::new(SERVER, 22);
                // The web server here is on the loopback, which a caller is
                // otherwise kept away from.
                server.reach_anywhere();
                server
            },
            clock: 0,
            flying: Vec::new(),
            lose_one_in,
            crossed: 0,
        }
    }

    /// The same, with the answering end kept off its own machine and network,
    /// as it is on a real call.
    fn guarded() -> Self {
        let mut link = Self::new(0);
        link.server = Server::new(SERVER, 22);
        link
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

/// Ask the proxy for a tunnel (RFC 9112 3.2.3) and read its answer's head.
fn tunnel(socket: &mut TcpStream, target: &str) -> Result<String, String> {
    let request = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n");
    socket.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    // One octet at a time, so nothing after the head is taken with it.
    let mut head = Vec::new();
    let mut octet = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut octet).map_err(|e| format!("{e} after {:?}", String::from_utf8_lossy(&head)))?;
        head.push(octet[0]);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

/// The browser's side of an https page: a tunnel, and a request through it.
fn a_browser(proxy: String, target: String) -> thread::JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut socket = TcpStream::connect(&proxy).map_err(|e| format!("{proxy}: {e}"))?;
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| e.to_string())?;
        let head = tunnel(&mut socket, &target)?;
        if !head.starts_with("HTTP/1.1 200") {
            return Err(format!("the proxy said {head:?}"));
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

/// A destination that is not there is refused in HTTP rather than left
/// hanging, which is what makes a browser show the right page.
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
    let browser = thread::spawn(move || -> String {
        let mut socket = TcpStream::connect(&proxy).expect("connect");
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .expect("timeout");
        tunnel(&mut socket, &closed).expect("no answer")
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
    // RFC 9110 15.6.3: a gateway that could not reach the server it was sent
    // to says 502.
    assert!(reply.starts_with("HTTP/1.1 502"), "it was not told: {reply:?}");
}

/// A browser that asks and then takes its time reading the answer.
fn a_slow_browser(proxy: String, target: String, wait: Duration) -> thread::JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut socket = TcpStream::connect(&proxy).map_err(|e| format!("{proxy}: {e}"))?;
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| e.to_string())?;
        tunnel(&mut socket, &target)?;
        socket
            .write_all(b"GET /page HTTP/1.0\r\nHost: example\r\n\r\n")
            .map_err(|e| e.to_string())?;
        // The page arrives while nobody is reading it, so it waits in the
        // relay rather than going into the socket.
        thread::sleep(wait);
        let mut page = Vec::new();
        socket.read_to_end(&mut page).map_err(|e| e.to_string())?;
        Ok(page)
    })
}

/// A page big enough to fill the socket between the proxy and the browser,
/// read only after the far end has finished sending it.
///
/// What comes off the link waits in the relay until the socket will take it.
/// The connection that brought it is over by then and the stack has forgotten
/// it, and dropping the relay at that point takes the rest of the page with
/// it -- which a browser sees as a connection that closed carrying nothing,
/// and calls an empty page.
#[test]
fn a_page_still_arrives_when_the_browser_is_slow_to_read_it() {
    let body: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
    let (web, web_thread) = a_web_server(body.clone());

    let mut link = Link::new(0);
    let proxy = link.client.bound().to_string();
    let target = web.replace("127.0.0.1", "localhost");
    let browser = a_slow_browser(proxy, target, Duration::from_secs(3));

    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watching = finished.clone();
    let waiter = thread::spawn(move || {
        let got = browser.join().expect("the browser thread panicked");
        watching.store(true, std::sync::atomic::Ordering::SeqCst);
        got
    });

    let done = finished.clone();
    link.run_until(600, || done.load(std::sync::atomic::Ordering::SeqCst));

    let page = waiter.join().expect("the waiting thread panicked").expect("no page");
    let text = String::from_utf8_lossy(&page[..60.min(page.len())]);
    assert!(text.starts_with("HTTP/1.0 200 OK"), "not a page: {text:?}");
    let split = page.windows(4).position(|w| w == b"\r\n\r\n").expect("no end of headers");
    assert_eq!(
        page[split + 4..].len(),
        body.len(),
        "the page was cut short: {} of {} octets",
        page[split + 4..].len(),
        body.len()
    );
    assert_eq!(page[split + 4..], body[..], "the page came back changed");
    let _ = web_thread.join();
}

/// A web server that keeps the connection and answers by length.
///
/// The HTTP/1.0 one above closes to say where the body ended, which ends the
/// browser's connection with it. Persistence is the whole point of the HTTP
/// proxy, so testing it needs an origin that does not hang up: HTTP/1.1 with a
/// Content-Length, RFC 9112 6.3 item 6.
fn a_patient_web_server(
    name: &'static str,
    answers: usize,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let handle = thread::spawn(move || {
        let mut seen = Vec::new();
        let Ok((mut socket, _)) = listener.accept() else {
            return seen;
        };
        let _ = socket.set_read_timeout(Some(Duration::from_secs(60)));
        let mut held: Vec<u8> = Vec::new();
        let mut buffer = [0u8; 512];
        while seen.len() < answers {
            let end = held.windows(4).position(|w| w == b"\r\n\r\n");
            let Some(end) = end else {
                match socket.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => held.extend_from_slice(&buffer[..n]),
                    Err(_) => break,
                }
                continue;
            };
            let request: Vec<u8> = held.drain(..end + 4).collect();
            seen.push(String::from_utf8_lossy(&request).into_owned());
            let body = format!("this is {name}");
            let answer = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if socket.write_all(answer.as_bytes()).is_err() {
                break;
            }
        }
        // And then wait to be let go rather than hanging up. An origin that
        // closes is saying the response ended there (RFC 9112 6.3 item 8), and
        // a proxy that did not pass that on would leave the browser waiting
        // for an end that had already happened -- so the closing has to be the
        // proxy's decision to make, not this test's.
        let mut sink = [0u8; 256];
        while matches!(socket.read(&mut sink), Ok(n) if n > 0) {}
        seen
    });
    (address, handle)
}

/// The browser's side of an http page: no handshake at all, just the request
/// with the whole target in it (RFC 9112 3.2.2).
fn an_http_browser(proxy: String, asks: Vec<String>) -> thread::JoinHandle<Result<Vec<String>, String>> {
    thread::spawn(move || {
        let mut socket = TcpStream::connect(&proxy).map_err(|e| format!("{proxy}: {e}"))?;
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| e.to_string())?;
        let mut got = Vec::new();
        let mut held: Vec<u8> = Vec::new();
        for url in asks {
            let request = format!("GET {url} HTTP/1.1\r\nHost: ignored.invalid\r\n\r\n");
            socket
                .write_all(request.as_bytes())
                .map_err(|e| e.to_string())?;
            // Read one whole response: head, then Content-Length octets.
            let mut buffer = [0u8; 512];
            loop {
                if let Some(end) = held.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&held[..end]).into_owned();
                    let length: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|v| v.trim().parse().ok())
                        })
                        .ok_or("no length")?;
                    if held.len() >= end + 4 + length {
                        let whole: Vec<u8> = held.drain(..end + 4 + length).collect();
                        got.push(String::from_utf8_lossy(&whole).into_owned());
                        break;
                    }
                }
                match socket.read(&mut buffer) {
                    Ok(0) => return Err(format!("the proxy hung up after {} answers", got.len())),
                    Ok(n) => held.extend_from_slice(&buffer[..n]),
                    Err(e) => return Err(format!("{e}")),
                }
            }
        }
        Ok(got)
    })
}

/// Run a browser thread against the link until it is done.
fn alongside<T: Send + 'static>(
    link: &mut Link,
    seconds: u32,
    work: thread::JoinHandle<T>,
) -> T {
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watching = finished.clone();
    let waiter = thread::spawn(move || {
        let got = work.join().expect("the browser thread panicked");
        watching.store(true, std::sync::atomic::Ordering::SeqCst);
        got
    });
    let done = finished.clone();
    link.run_until(seconds, || done.load(std::sync::atomic::Ordering::SeqCst));
    for line in link.server.take_log() {
        println!("  server: {line}");
    }
    waiter.join().expect("the waiting thread panicked")
}

/// A page through the proxy with no handshake in front of it.
#[test]
fn a_page_comes_back_through_the_http_proxy() {
    let (web, web_thread) = a_patient_web_server("the page", 1);
    let mut link = Link::new(0);
    let proxy = link.client.bound().to_string();
    // By name, so the far end is the one that resolves it -- which is what a
    // proxy is for, as against a route.
    let target = web.replace("127.0.0.1", "localhost");
    let browser = an_http_browser(proxy, vec![format!("http://{target}/page")]);

    let got = alongside(&mut link, 120, browser).expect("no page");
    assert_eq!(got.len(), 1);
    assert!(got[0].starts_with("HTTP/1.1 200 OK"), "not a page: {}", got[0]);
    assert!(got[0].ends_with("this is the page"), "{}", got[0]);

    drop(link);
    let seen = web_thread.join().expect("the web thread panicked");
    assert_eq!(seen.len(), 1, "the web server saw {seen:?}");
    // 3.2.2 in, 3.2.1 out: the origin server is sent a path, and a Host taken
    // from the request-target rather than the one the browser wrote.
    assert!(seen[0].starts_with("GET /page HTTP/1.1\r\n"), "asked for {:?}", seen[0]);
    assert!(
        seen[0].to_ascii_lowercase().contains(&format!("host: {target}\r\n").to_ascii_lowercase()),
        "the wrong Host reached the origin: {:?}",
        seen[0]
    );
    assert!(!seen[0].contains("ignored.invalid"), "{:?}", seen[0]);
}

/// Two pages from the same host down one connection, opening one socket.
///
/// This is the saving: the second page costs nothing before the request
/// itself, where a new connection would cost a handshake over the link.
#[test]
fn a_second_page_from_the_same_host_opens_no_second_socket() {
    let (web, web_thread) = a_patient_web_server("the page", 2);
    let mut link = Link::new(0);
    let proxy = link.client.bound().to_string();
    let target = web.replace("127.0.0.1", "localhost");
    let browser = an_http_browser(
        proxy,
        vec![
            format!("http://{target}/one"),
            format!("http://{target}/two"),
        ],
    );

    let got = alongside(&mut link, 120, browser).expect("no pages");
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got.iter().all(|g| g.starts_with("HTTP/1.1 200 OK")), "{got:?}");

    drop(link);
    let seen = web_thread.join().expect("the web thread panicked");
    // Both on the one socket: the server only ever accepted once, so two
    // requests arriving is proof the connection was kept.
    assert_eq!(seen.len(), 2, "the origin saw {seen:?}");
    assert!(seen[0].starts_with("GET /one HTTP/1.1"), "{:?}", seen[0]);
    assert!(seen[1].starts_with("GET /two HTTP/1.1"), "{:?}", seen[1]);
}

/// And a browser that changes host on a connection it is already using.
///
/// A browser talking to an HTTP proxy is entitled to do this and Firefox does
/// it constantly, especially with few connections allowed. The socket
/// underneath has to change while the one to the browser stays.
#[test]
fn one_browser_connection_can_visit_two_different_hosts() {
    let (first, first_thread) = a_patient_web_server("the first", 1);
    let (second, second_thread) = a_patient_web_server("the second", 1);
    let mut link = Link::new(0);
    let proxy = link.client.bound().to_string();
    let browser = an_http_browser(
        proxy,
        vec![
            format!("http://{}/a", first.replace("127.0.0.1", "localhost")),
            // By address rather than by name, so the two targets differ in
            // more than their port and a proxy that compared them loosely
            // would be caught.
            format!("http://{second}/b"),
        ],
    );

    let got = alongside(&mut link, 120, browser).expect("no pages");
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[0].ends_with("this is the first"), "{}", got[0]);
    assert!(got[1].ends_with("this is the second"), "{}", got[1]);

    drop(link);
    let seen = first_thread.join().expect("the first web thread panicked");
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].starts_with("GET /a HTTP/1.1"), "{:?}", seen[0]);
    let seen = second_thread.join().expect("the second web thread panicked");
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(seen[0].starts_with("GET /b HTTP/1.1"), "{:?}", seen[0]);
}

/// The same page over a link that loses things, because a real one does.
#[test]
fn an_http_page_comes_back_over_a_line_that_loses_things() {
    let (web, web_thread) = a_patient_web_server("the page", 1);
    let mut link = Link::new(11);
    let proxy = link.client.bound().to_string();
    let target = web.replace("127.0.0.1", "localhost");
    let browser = an_http_browser(proxy, vec![format!("http://{target}/page")]);

    let got = alongside(&mut link, 240, browser).expect("no page");
    assert!(got[0].ends_with("this is the page"), "{}", got[0]);
    assert!(link.crossed > 0, "nothing crossed, so nothing was lost either");
    drop(link);
    let seen = web_thread.join().expect("the web thread panicked");
    assert_eq!(seen.len(), 1, "{seen:?}");
}

/// Whoever dials in reaches what the answering machine can reach, and that
/// was everything -- its own loopback, its own network -- over a tunnel to
/// any port at all. A web server on the answering machine's loopback is
/// somewhere a caller does not go, by tunnel or by plain request, and the
/// server never hears from the proxy.
#[test]
fn the_answering_machine_itself_is_off_limits() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let local = listener.local_addr().expect("addr").to_string();

    let mut link = Link::guarded();
    let proxy = link.client.bound().to_string();
    let target = local.clone();
    let browser = thread::spawn(move || -> Result<String, String> {
        let mut socket = TcpStream::connect(&proxy).map_err(|e| e.to_string())?;
        socket
            .set_read_timeout(Some(Duration::from_secs(60)))
            .map_err(|e| e.to_string())?;
        tunnel(&mut socket, &target)
    });
    let reply = alongside(&mut link, 60, browser).expect("no answer to the tunnel");
    // RFC 9110 15.5.4: understood, and refused.
    assert!(reply.starts_with("HTTP/1.1 403"), "a tunnel to this machine was not refused: {reply:?}");

    let proxy = link.client.bound().to_string();
    let by_name = local.replace("127.0.0.1", "localhost");
    let browser = an_http_browser(proxy, vec![format!("http://{by_name}/secret")]);
    let got = alongside(&mut link, 60, browser).expect("no answer to the request");
    assert!(got[0].starts_with("HTTP/1.1 403"), "a request to this machine was not refused: {}", got[0]);

    listener.set_nonblocking(true).expect("nonblocking");
    assert!(
        matches!(listener.accept(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "the proxy reached the answering machine's own server"
    );
}
