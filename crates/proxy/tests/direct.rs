//! Pages from the internet, when the far end of the call is a provider.
//!
//! The far end here is a router: it has an address of its own, on which it
//! runs nothing, and it passes everything else on to wherever it is
//! addressed. Behind it is one web server, with an address that is not the
//! router's, running on a TCP stack of this crate's own so the whole of it
//! happens in one thread and one clock.
//!
//! The browser is a real socket, pointed at the client as its HTTP proxy.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use proxy::{Client, Route};
use tcp::stack::{Handle, Stack};

const CALLER: [u8; 4] = [192, 168, 9, 40];
const ROUTER: [u8; 4] = [192, 168, 9, 1];
const WEB: [u8; 4] = [203, 0, 113, 5];

const STEP_MS: u32 = 10;
/// Each way, through the call and the router both.
const DELAY_MS: u32 = 300;

/// What the web server does with what it is sent.
#[derive(Default)]
struct Served {
    /// What arrived and has not been answered, per connection.
    held: HashMap<Handle, Vec<u8>>,
    /// Every request head it answered.
    requests: Vec<String>,
}

/// A datagram on its way: when it arrives, to whom, from whom, and what.
type Flying = (u32, [u8; 4], [u8; 4], Vec<u8>);

struct Internet {
    client: Client,
    router: Stack,
    web: Stack,
    served: Served,
    clock: u32,
    flying: Vec<Flying>,
    /// The largest TCP payload the client sent the web server.
    largest_to_web: usize,
}

impl Internet {
    fn new(route: Route) -> Self {
        let client = Client::routed("127.0.0.1:0", CALLER, ROUTER, 5, route).expect("could not listen");
        let mut web = Stack::new(WEB, 77);
        web.listen(80);
        web.listen(443);
        Self {
            client,
            // Listening on nothing: a connection to it is refused.
            router: Stack::new(ROUTER, 66),
            web,
            served: Served::default(),
            clock: 0,
            flying: Vec::new(),
            largest_to_web: 0,
        }
    }

    fn step(&mut self) {
        self.clock += STEP_MS;
        for out in self.client.take_outgoing() {
            if out.to == WEB {
                let header = usize::from(out.payload[12] >> 4) * 4;
                self.largest_to_web = self.largest_to_web.max(out.payload.len() - header);
            }
            self.flying.push((self.clock + DELAY_MS, out.to, CALLER, out.payload));
        }
        for out in self.router.take_outgoing() {
            self.flying.push((self.clock + DELAY_MS, out.to, ROUTER, out.payload));
        }
        for out in self.web.take_outgoing() {
            self.flying.push((self.clock + DELAY_MS, out.to, WEB, out.payload));
        }
        let now = self.clock;
        let (arrived, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.flying).into_iter().partition(|f| f.0 <= now);
        self.flying = waiting;
        for (_, to, from, payload) in arrived {
            match to {
                CALLER => self.client.deliver(from, to, &payload),
                ROUTER => self.router.deliver(from, to, &payload),
                WEB => self.web.deliver(from, to, &payload),
                _ => {}
            }
        }
        self.client.tick(STEP_MS);
        self.router.tick(STEP_MS);
        self.web.tick(STEP_MS);
        self.serve();
    }

    /// Port 80 answers each request head with a page; port 443 sends back
    /// whatever it is sent, as a stand-in for TLS.
    fn serve(&mut self) {
        let handles: Vec<Handle> = self.web.connections().map(|(h, _)| h).collect();
        for handle in handles {
            let Some(c) = self.web.get_mut(handle) else { continue };
            let port = c.local.port;
            let data = c.take_received();
            if port == 443 {
                if !data.is_empty() {
                    let mut echoed = b"echo: ".to_vec();
                    echoed.extend_from_slice(&data);
                    c.send(&echoed);
                }
                continue;
            }
            let held = self.served.held.entry(handle).or_default();
            held.extend(data);
            while let Some(end) = held.windows(4).position(|w| w == b"\r\n\r\n") {
                let head: Vec<u8> = held.drain(..end + 4).collect();
                let head = String::from_utf8_lossy(&head).into_owned();
                let path = head.split(' ').nth(1).unwrap_or("?").to_owned();
                let body = format!("this is {path}, {}", "and more ".repeat(200));
                let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                c.send(answer.as_bytes());
                self.served.requests.push(head);
            }
        }
    }

    fn run_until(&mut self, seconds: u32, done: &AtomicBool) {
        for _ in 0..(seconds * 1000 / STEP_MS) {
            self.step();
            if done.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_micros(300));
        }
        for line in self.client.take_log() {
            println!("  client: {line}");
        }
        panic!("nothing happened in {seconds} s");
    }

    /// Run a browser thread against all of this until it is done.
    fn alongside<T: Send + 'static>(&mut self, seconds: u32, work: thread::JoinHandle<T>) -> T {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let waiter = thread::spawn(move || {
            let got = work.join().expect("the browser thread panicked");
            flag.store(true, Ordering::SeqCst);
            got
        });
        self.run_until(seconds, &done);
        for line in self.client.take_log() {
            println!("  client: {line}");
        }
        waiter.join().expect("the waiting thread panicked")
    }
}

/// Read one response: its head, then as much body as it says it has.
fn read_response(socket: &mut TcpStream) -> Result<String, String> {
    let mut held = Vec::new();
    let mut buffer = [0u8; 1024];
    loop {
        if let Some(end) = held.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&held[..end]).to_ascii_lowercase();
            let length: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse().ok()))
                .unwrap_or(0);
            if held.len() >= end + 4 + length {
                return Ok(String::from_utf8_lossy(&held[..end + 4 + length]).into_owned());
            }
        }
        match socket.read(&mut buffer) {
            Ok(0) => return Err(format!("closed after {:?}", String::from_utf8_lossy(&held))),
            Ok(n) => held.extend_from_slice(&buffer[..n]),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn a_browser(proxy: String, requests: Vec<String>) -> thread::JoinHandle<Result<Vec<String>, String>> {
    thread::spawn(move || {
        let mut socket = TcpStream::connect(&proxy).map_err(|e| e.to_string())?;
        socket.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
        let mut got = Vec::new();
        for request in requests {
            socket.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
            got.push(read_response(&mut socket)?);
        }
        Ok(got)
    })
}

/// The far end refuses a connection to a proxy, so pages go straight out --
/// to the web server's own address, through the router -- and come back.
#[test]
fn a_page_comes_straight_from_the_internet_when_the_far_end_is_a_router() {
    let mut net = Internet::new(Route::Auto);
    let proxy = net.client.bound().to_string();
    let browser = a_browser(
        proxy,
        vec![
            "GET http://203.0.113.5/first HTTP/1.1\r\nHost: 203.0.113.5\r\n\r\n".to_owned(),
            "GET http://203.0.113.5/second HTTP/1.1\r\nHost: 203.0.113.5\r\n\r\n".to_owned(),
        ],
    );
    let got = net.alongside(120, browser).expect("no pages");
    assert!(net.client.direct(), "the route was not settled as direct");
    assert_eq!(got.len(), 2);
    assert!(got[0].starts_with("HTTP/1.1 200 OK") && got[0].contains("this is /first"), "{}", &got[0][..60]);
    assert!(got[1].contains("this is /second"), "{}", &got[1][..60]);
    // The web server saw origin-form requests with the Host from the target,
    // both down the one connection.
    assert_eq!(net.served.requests.len(), 2, "{:?}", net.served.requests);
    assert!(net.served.requests[0].starts_with("GET /first HTTP/1.1\r\n"), "{:?}", net.served.requests[0]);
    assert!(net.served.requests[0].contains("Host: 203.0.113.5:80\r\n"), "{:?}", net.served.requests[0]);
    assert_eq!(net.served.held.len(), 1, "a second connection was opened for the same host");

    let view = net.client.view();
    assert_eq!(view.route, "straight to the internet");
    assert!(view.to_browsers > 3000 && view.from_browsers > 60, "{view:?}");
}

/// https: a CONNECT opens a tunnel to the server, and what goes through it is
/// nobody's business on the way.
#[test]
fn a_tunnel_opens_straight_to_the_server() {
    let mut net = Internet::new(Route::Direct);
    let proxy = net.client.bound().to_string();
    let browser = thread::spawn(move || -> Result<String, String> {
        let mut socket = TcpStream::connect(&proxy).map_err(|e| e.to_string())?;
        socket.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
        socket
            .write_all(b"CONNECT 203.0.113.5:443 HTTP/1.1\r\nHost: 203.0.113.5:443\r\n\r\n")
            .map_err(|e| e.to_string())?;
        let head = read_response(&mut socket)?;
        if !head.starts_with("HTTP/1.1 200") {
            return Err(head);
        }
        socket.write_all(b"\x16\x03\x01 not really TLS").map_err(|e| e.to_string())?;
        let mut back = vec![0u8; b"echo: \x16\x03\x01 not really TLS".len()];
        socket.read_exact(&mut back).map_err(|e| e.to_string())?;
        Ok(String::from_utf8_lossy(&back).into_owned())
    });
    let echoed = net.alongside(60, browser).expect("no tunnel");
    assert_eq!(echoed, "echo: \u{16}\u{3}\u{1} not really TLS");
}

/// A port nothing listens on is refused by the server, and the browser is told
/// in HTTP.
#[test]
fn a_refused_connection_is_a_502() {
    let mut net = Internet::new(Route::Direct);
    let proxy = net.client.bound().to_string();
    let browser = a_browser(proxy, vec!["GET http://203.0.113.5:81/ HTTP/1.1\r\n\r\n".to_owned()]);
    let got = net.alongside(60, browser).expect("no answer");
    assert!(got[0].starts_with("HTTP/1.1 502"), "{}", got[0]);
    assert!(got[0].contains("refused"), "{}", got[0]);
}

/// A name that is not anywhere is a 502 too, and costs nothing over the link.
#[test]
fn a_name_that_does_not_exist_is_a_502() {
    let mut net = Internet::new(Route::Direct);
    let proxy = net.client.bound().to_string();
    // RFC 6761 6.4: .invalid is guaranteed never to resolve.
    let browser = a_browser(proxy, vec!["GET http://nowhere.invalid/ HTTP/1.1\r\n\r\n".to_owned()]);
    let got = net.alongside(60, browser).expect("no answer");
    assert!(got[0].starts_with("HTTP/1.1 502"), "{}", got[0]);
    assert_eq!(net.client.view().lookup_failures, 1);
    assert!(net.client.view().carried.is_empty(), "something crossed the link for a name that is nowhere");
}

/// The far end of the link asked for small frames, and no segment is built
/// larger than they carry, whatever the web server says it can take.
#[test]
fn segments_fit_the_link_whatever_the_server_offers() {
    let mut net = Internet::new(Route::Direct);
    net.client.size_for_link(1500, 576);
    assert_eq!(net.client.view().mss, (1460, 536));
    let proxy = net.client.bound().to_string();
    // A request with a body larger than any one segment.
    let body = "x".repeat(3000);
    let request = format!("POST http://203.0.113.5/form HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    let browser = a_browser(proxy, vec![request]);
    let got = net.alongside(60, browser).expect("no answer");
    assert!(got[0].contains("this is /form"), "{}", &got[0][..60]);
    assert_eq!(net.largest_to_web, 536, "a segment did not fit the link, or none was full");
}
