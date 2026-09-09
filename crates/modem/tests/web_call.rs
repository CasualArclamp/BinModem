//! A web page over a modem call, through every layer there is.
//!
//! The tallest test in the workspace, and the one the whole thing exists for.
//! Two modems on a simulated line agree a modulation with V.8, carry bits with
//! the data pump, make them reliable with V.42 and compress them with V.42bis.
//! PPP turns the octets back into frames, IPCP gives the two ends addresses,
//! our own TCP runs over our own IP on top of that, SOCKS 5 asks the answering
//! end to open a connection, and the answering end opens a real socket to a
//! real web server -- which is the only part of the path that belongs to the
//! operating system.
//!
//! The browser is real too: a socket connecting to the dialling end's
//! listener, speaking SOCKS at it the way any browser would.
//!
//! It is slow, because it is a modem. That is the point.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use modem::{Modem, State};
use ppp::link::Link;

const FS: f64 = 16_000.0;
const SERVER: [u8; 4] = [10, 0, 0, 1];
const CLIENT: [u8; 4] = [10, 0, 0, 2];

/// A web server on the loopback that answers one request and closes.
fn a_web_server(body: Vec<u8>) -> (String, thread::JoinHandle<Option<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("addr").to_string();
    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().ok()?;
        socket.set_read_timeout(Some(Duration::from_secs(60))).ok()?;
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
        drop(socket);
        Some(String::from_utf8_lossy(&request).into_owned())
    });
    (address, handle)
}

/// The browser: connect, speak RFC 1928, ask for a page, read to the end.
fn a_browser(proxy: String, target: String) -> thread::JoinHandle<Result<Vec<u8>, String>> {
    thread::spawn(move || {
        let mut socket = TcpStream::connect(&proxy).map_err(|e| format!("{proxy}: {e}"))?;
        socket
            .set_read_timeout(Some(Duration::from_secs(300)))
            .map_err(|e| e.to_string())?;
        socket.write_all(&[5, 1, 0]).map_err(|e| e.to_string())?;
        let mut greeting = [0u8; 2];
        socket.read_exact(&mut greeting).map_err(|e| e.to_string())?;
        if greeting != [5, 0] {
            return Err(format!("the proxy offered {greeting:?}"));
        }
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
            .write_all(b"GET /over-a-modem HTTP/1.0\r\nHost: example\r\n\r\n")
            .map_err(|e| e.to_string())?;
        let mut page = Vec::new();
        socket.read_to_end(&mut page).map_err(|e| e.to_string())?;
        Ok(page)
    })
}

#[test]
fn a_page_comes_back_over_a_call() {
    // Small enough to cross a 2400 bit/s line in a test and large enough to
    // need many segments and several windows.
    let body: Vec<u8> = (0..3_000u32)
        .map(|i| b"abcdefghijklmnopqrstuvwxyz0123456789"[(i % 36) as usize])
        .collect();
    let (web, web_thread) = a_web_server(body.clone());

    let mut caller = Modem::new(FS);
    let mut host = Modem::new(FS);
    for m in [&mut caller, &mut host] {
        // V.22bis, which is the modulation that has carried a real session
        // over a real trunk. The point here is the layers above, not the one
        // below.
        for b in b"AT+MS=V22B,0\r" {
            m.feed_dte(*b);
        }
        m.take_dte();
    }
    for b in b"ATA\r" {
        host.feed_dte(*b);
    }
    for b in b"ATD5551234\r" {
        caller.feed_dte(*b);
    }

    let mut server_link = Link::new(SERVER, CLIENT);
    let mut client_link = Link::new([0, 0, 0, 0], [0, 0, 0, 0]);
    let mut server = proxy::Server::new(SERVER, 0x9e37_79b9);
    let mut client =
        proxy::Client::new("127.0.0.1:0", CLIENT, SERVER, 0x85eb_ca6b).expect("could not listen");

    let finished = Arc::new(AtomicBool::new(false));
    let mut browser: Option<thread::JoinHandle<Result<Vec<u8>, String>>> = None;
    let mut waiter: Option<thread::JoinHandle<Result<Vec<u8>, String>>> = None;

    let mut started = false;
    let mut up_at = None;
    let per_ms = FS as usize / 1000;
    let (mut from_caller, mut from_host) = (0.0, 0.0);
    let mut connected_at = 0.0;

    // Five minutes of line. A modem at 2400 bit/s carrying three kilobytes
    // plus everything the layers above add is a minute of it.
    for i in 0..(300.0 * FS) as usize {
        let (a, b) = (from_caller, from_host);
        from_caller = caller.step(b);
        from_host = host.step(a);

        if caller.state() != State::Data || host.state() != State::Data {
            caller.take_dte();
            host.take_dte();
            continue;
        }
        if !started {
            started = true;
            connected_at = i as f64 / FS;
            client_link.open();
            server_link.open();
        }

        client_link.feed(&caller.take_dte());
        server_link.feed(&host.take_dte());

        if i % per_ms == 0 {
            client_link.tick(1);
            server_link.tick(1);

            // What PPP brought up goes to the proxies, and back again.
            for datagram in client_link.take_carried() {
                if datagram.protocol == ppp::ip::PROTOCOL_TCP {
                    client.deliver(datagram.from, datagram.to, &datagram.payload);
                }
            }
            for datagram in server_link.take_carried() {
                if datagram.protocol == ppp::ip::PROTOCOL_TCP {
                    server.deliver(datagram.from, datagram.to, &datagram.payload);
                }
            }
            client.tick(1);
            server.tick(1);
            for out in client.take_outgoing() {
                client_link.send_payload(ppp::ip::PROTOCOL_TCP, &out.payload);
            }
            for out in server.take_outgoing() {
                server_link.send_payload(ppp::ip::PROTOCOL_TCP, &out.payload);
            }
        }

        for byte in client_link.take_line() {
            caller.feed_dte(byte);
        }
        for byte in server_link.take_line() {
            host.feed_dte(byte);
        }

        if up_at.is_none() && client_link.up() && server_link.up() {
            up_at = Some(i as f64 / FS);
            // Only now is there an address to be reached at, so only now is
            // there anything for a browser to talk to.
            let at = client.bound().to_string();
            let started = a_browser(at, web.clone());
            let watching = finished.clone();
            browser = Some(thread::spawn(move || {
                let got = started.join().expect("the browser thread panicked");
                watching.store(true, Ordering::SeqCst);
                got
            }));
        }
        if finished.load(Ordering::SeqCst) {
            waiter = browser.take();
            break;
        }
    }

    for line in server.take_log() {
        println!("  {line}");
    }
    let up_at = up_at.expect("PPP never reached the network phase");
    let waiter = waiter.expect("the page never came back");
    let page = waiter.join().expect("the waiting thread panicked").expect("no page");

    let text = String::from_utf8_lossy(&page);
    assert!(
        text.starts_with("HTTP/1.0 200 OK"),
        "not a page: {:?}",
        &text[..60.min(text.len())]
    );
    let split = page
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no end of headers");
    assert_eq!(
        page[split + 4..],
        body[..],
        "the page did not come back the way it went"
    );

    let request = web_thread
        .join()
        .expect("the web thread panicked")
        .expect("the web server saw nothing");
    assert!(
        request.starts_with("GET /over-a-modem HTTP/1.0"),
        "asked for {request:?}"
    );

    println!(
        "  connected {connected_at:.1} s in, network phase at {up_at:.1} s, \
         {} octets of page at {} bit/s",
        body.len(),
        caller.rate().unwrap_or(0)
    );
}
