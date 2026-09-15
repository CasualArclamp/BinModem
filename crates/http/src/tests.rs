//! What a browser actually sends a proxy, and what has to come out the far
//! side of it.

use super::*;

/// Drive a session the way the far end does: feed it, open whatever it asks
/// for, and collect everything that would reach the origin server.
fn forward(session: &mut Session, from_browser: &[u8]) -> Vec<u8> {
    let mut to_origin = session.feed(from_browser);
    while session.request().is_some() {
        to_origin.extend(session.answer(Answer::Opened));
    }
    to_origin
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_browsers_request_becomes_one_an_origin_server_will_answer() {
    let mut session = Session::new();
    let asked = forward(
        &mut session,
        b"GET http://httpforever.com/index.html HTTP/1.1\r\n\
          Host: httpforever.com\r\n\
          User-Agent: Mozilla/5.0\r\n\
          Proxy-Connection: keep-alive\r\n\
          \r\n",
    );
    assert_eq!(
        session.request(),
        None,
        "the target was asked for and answered"
    );
    let out = text(&asked);
    // 9112 3.2.2 goes in, 3.2.1 comes out: the absolute-form is the proxy's
    // business and an origin server is sent the path.
    assert!(out.starts_with("GET /index.html HTTP/1.1\r\n"), "{out}");
    assert!(out.contains("User-Agent: Mozilla/5.0\r\n"), "{out}");
    // 7.6.1: this one is for the hop and goes no further.
    assert!(!out.to_ascii_lowercase().contains("proxy-connection"), "{out}");
    assert!(out.ends_with("\r\n\r\n"), "{out}");
}

#[test]
fn the_host_comes_from_the_request_target_and_not_from_the_browser() {
    // 3.2.2: "the proxy MUST ignore the received Host header field (if any)
    // and instead replace it with the host information of the request-target".
    // A browser does not normally disagree with itself, but a proxy that
    // forwarded whatever it was handed would send a request to one host asking
    // for another, which is how a cache gets poisoned.
    let mut session = Session::new();
    let asked = forward(
        &mut session,
        b"GET http://real.invalid/page HTTP/1.1\r\nHost: pretend.invalid\r\n\r\n",
    );
    let out = text(&asked);
    assert!(out.contains("Host: real.invalid:80\r\n"), "{out}");
    assert!(!out.contains("pretend.invalid"), "{out}");
}

#[test]
fn a_target_without_a_port_gets_the_one_http_uses() {
    let mut session = Session::new();
    forward(&mut session, b"GET http://example.invalid/ HTTP/1.1\r\n\r\n");
    // Asked for and answered inside `forward`, so what it opened is what the
    // session now considers open.
    assert!(session.open());
}

#[test]
fn what_it_wants_opened_is_the_host_in_the_request() {
    let mut session = Session::new();
    session.feed(b"GET http://example.invalid:8080/ HTTP/1.1\r\n\r\n");
    assert_eq!(
        session.request(),
        Some(&Target { authority: "example.invalid:8080".to_owned(), tunnel: false })
    );
}

#[test]
fn an_empty_path_is_sent_as_a_slash() {
    // 3.2.1: "if the path component is empty, the client MUST send / as the
    // path within the origin-form". The proxy writes that origin-form, so the
    // obligation lands here.
    let mut session = Session::new();
    let asked = forward(&mut session, b"GET http://example.invalid HTTP/1.1\r\n\r\n");
    assert!(text(&asked).starts_with("GET / HTTP/1.1\r\n"), "{}", text(&asked));
}

#[test]
fn a_query_with_no_path_keeps_its_question_mark() {
    let mut session = Session::new();
    let asked = forward(&mut session, b"GET http://example.invalid?q=1 HTTP/1.1\r\n\r\n");
    assert!(text(&asked).starts_with("GET ?q=1 HTTP/1.1\r\n"), "{}", text(&asked));
}

#[test]
fn a_second_request_to_the_same_host_reuses_the_socket() {
    // The whole reason this exists instead of SOCKS. A browser keeps the
    // connection to its proxy and sends request after request down it; if
    // every one of them needed a socket opened, the saving would be given
    // straight back.
    let mut session = Session::new();
    forward(&mut session, b"GET http://example.invalid/one HTTP/1.1\r\n\r\n");
    let asked = session.feed(b"GET http://example.invalid/two HTTP/1.1\r\n\r\n");
    assert_eq!(session.request(), None, "it asked for a second socket");
    assert!(text(&asked).starts_with("GET /two HTTP/1.1\r\n"), "{}", text(&asked));
}

#[test]
fn a_request_to_somewhere_else_waits_for_its_own_socket() {
    let mut session = Session::new();
    forward(&mut session, b"GET http://first.invalid/ HTTP/1.1\r\n\r\n");
    let asked = session.feed(b"GET http://second.invalid/ HTTP/1.1\r\n\r\n");
    assert!(
        asked.is_empty(),
        "a request went to the socket belonging to somewhere else: {}",
        text(&asked)
    );
    assert_eq!(
        session.request().map(|t| t.authority.as_str()),
        Some("second.invalid:80")
    );
    let asked = session.answer(Answer::Opened);
    assert!(text(&asked).starts_with("GET / HTTP/1.1\r\n"), "{}", text(&asked));
}

#[test]
fn a_body_of_known_length_goes_through_and_the_next_request_is_still_found() {
    // 9112 6.3 item 6. Getting this wrong does not lose the body -- it loses
    // the boundary, and the next request line is read as though it were more
    // body, which is request smuggling done to oneself.
    let mut session = Session::new();
    let asked = forward(
        &mut session,
        b"POST http://example.invalid/form HTTP/1.1\r\n\
          Content-Length: 9\r\n\
          \r\n\
          name=rory\
          GET http://example.invalid/after HTTP/1.1\r\n\r\n",
    );
    let out = text(&asked);
    assert!(out.contains("Content-Length: 9\r\n"), "{out}");
    assert!(out.contains("name=rory"), "{out}");
    assert!(out.contains("GET /after HTTP/1.1\r\n"), "the second request was eaten: {out}");
}

#[test]
fn a_body_that_arrives_in_pieces_is_still_one_body() {
    let mut session = Session::new();
    let mut out = forward(
        &mut session,
        b"POST http://example.invalid/form HTTP/1.1\r\nContent-Length: 6\r\n\r\nab",
    );
    out.extend(session.feed(b"cd"));
    out.extend(session.feed(b"ef"));
    out.extend(session.feed(b"GET http://example.invalid/next HTTP/1.1\r\n\r\n"));
    let out = text(&out);
    assert!(out.contains("abcdef"), "{out}");
    assert!(out.contains("GET /next HTTP/1.1\r\n"), "{out}");
}

#[test]
fn a_chunked_body_is_passed_through_and_its_end_is_found() {
    // 7.1. The content is not decoded and is not wanted; what is wanted is
    // where it stops, and only the framing says.
    let mut session = Session::new();
    let asked = forward(
        &mut session,
        b"POST http://example.invalid/upload HTTP/1.1\r\n\
          Transfer-Encoding: chunked\r\n\
          \r\n\
          4\r\nabcd\r\n3;ext=1\r\nefg\r\n0\r\n\r\n\
          GET http://example.invalid/after HTTP/1.1\r\n\r\n",
    );
    let out = text(&asked);
    assert!(out.contains("Transfer-Encoding: chunked\r\n"), "the framing was dropped: {out}");
    assert!(out.contains("4\r\nabcd\r\n"), "{out}");
    assert!(out.contains("3;ext=1\r\nefg\r\n"), "a chunk extension broke it: {out}");
    assert!(out.contains("GET /after HTTP/1.1\r\n"), "the end of the body was missed: {out}");
}

#[test]
fn both_framings_at_once_forwards_only_the_transfer_encoding() {
    // 6.3 item 3: "an intermediary that chooses to forward the message MUST
    // first remove the received Content-Length field and process the
    // Transfer-Encoding". Leaving both in is what lets two ends of a chain
    // disagree about where a message stopped.
    let mut session = Session::new();
    let asked = forward(
        &mut session,
        b"POST http://example.invalid/ HTTP/1.1\r\n\
          Content-Length: 99\r\n\
          Transfer-Encoding: chunked\r\n\
          \r\n0\r\n\r\n",
    );
    let out = text(&asked);
    assert!(out.contains("Transfer-Encoding: chunked\r\n"), "{out}");
    assert!(!out.to_ascii_lowercase().contains("content-length"), "{out}");
}

#[test]
fn the_fields_a_connection_header_names_go_no_further() {
    // 7.6.1: an intermediary removes "any header or trailer field(s) from the
    // message with the same name as the connection-option, and then remove the
    // Connection header field itself".
    let mut session = Session::new();
    let asked = forward(
        &mut session,
        b"GET http://example.invalid/ HTTP/1.1\r\n\
          Connection: keep-alive, X-Hop\r\n\
          X-Hop: for this hop only\r\n\
          X-End: for everyone\r\n\
          \r\n",
    );
    let out = text(&asked).to_ascii_lowercase();
    assert!(!out.contains("x-hop"), "{out}");
    assert!(!out.contains("connection:"), "{out}");
    assert!(out.contains("x-end"), "{out}");
}

#[test]
fn a_connect_becomes_a_tunnel_once_the_socket_is_open() {
    // 9112 3.2.3 and 9110 9.3.6. This is how https goes over the link: nothing
    // here reads a byte of it, which is the point.
    let mut session = Session::new();
    let asked = session.feed(b"CONNECT example.invalid:443 HTTP/1.1\r\nHost: example.invalid\r\n\r\n");
    assert!(asked.is_empty(), "a CONNECT head was forwarded to the origin");
    assert_eq!(
        session.request(),
        Some(&Target { authority: "example.invalid:443".to_owned(), tunnel: true })
    );
    let asked = session.answer(Answer::Opened);
    assert!(asked.is_empty());
    assert_eq!(
        text(&session.take_out()),
        "HTTP/1.1 200 Connection established\r\n\r\n"
    );
    assert_eq!(session.state(), State::Tunnelling);
    // 6.3 item 2: everything after the blank line belongs to the tunnel, and
    // is not read as anything.
    let raw = [0x16, 0x03, 0x01, 0x00, 0x00];
    assert_eq!(session.feed(&raw), raw);
}

#[test]
fn a_tunnel_carries_what_would_otherwise_look_like_a_request() {
    let mut session = Session::new();
    session.feed(b"CONNECT example.invalid:443 HTTP/1.1\r\n\r\n");
    session.answer(Answer::Opened);
    let inside = b"GET http://elsewhere.invalid/ HTTP/1.1\r\n\r\n";
    assert_eq!(session.feed(inside), inside, "the tunnel read its own contents");
    assert_eq!(session.request(), None);
}

#[test]
fn a_far_end_that_will_not_open_is_explained_to_the_browser() {
    // A browser shown nothing at all is the failure this whole proxy spent an
    // evening being: the page is blank and no layer says why. 9110 15.6.3 is
    // the gateway saying it was the gateway.
    let mut session = Session::new();
    session.feed(b"GET http://nowhere.invalid/ HTTP/1.1\r\n\r\n");
    session.answer(Answer::Refused);
    let out = text(&session.take_out());
    assert!(out.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{out}");
    assert!(out.contains("refused"), "{out}");
    assert!(out.contains("Content-Length: "), "a reply with no length: {out}");
    assert_eq!(session.state(), State::Failed);
    assert!(session.trouble().is_some());
}

#[test]
fn a_far_end_that_never_answered_says_so_differently() {
    let mut session = Session::new();
    session.feed(b"GET http://nowhere.invalid/ HTTP/1.1\r\n\r\n");
    session.answer(Answer::TimedOut);
    let out = text(&session.take_out());
    assert!(out.starts_with("HTTP/1.1 504 Gateway Timeout\r\n"), "{out}");
}

#[test]
fn a_request_that_is_not_in_absolute_form_is_refused_rather_than_guessed_at() {
    // 3.2.2 requires absolute-form to a proxy. Origin-form arriving here is a
    // browser that thinks this is a web server, and there is no host anywhere
    // in the message to guess from -- so saying so beats inventing one.
    let mut session = Session::new();
    session.feed(b"GET /index.html HTTP/1.1\r\nHost: example.invalid\r\n\r\n");
    let out = text(&session.take_out());
    assert!(out.starts_with("HTTP/1.1 400 Bad Request\r\n"), "{out}");
    assert_eq!(session.state(), State::Failed);
}

#[test]
fn a_head_split_across_arrivals_is_still_one_head() {
    // Everything here arrives a datagram at a time over a modem, so a request
    // head split down the middle is the normal case rather than the odd one.
    let mut session = Session::new();
    assert!(session.feed(b"GET http://example.inv").is_empty());
    assert!(session.feed(b"alid/page HTTP/1.1\r\nUser-Agent: x").is_empty());
    assert!(session.feed(b"\r\n\r").is_empty());
    assert!(session.feed(b"\n").is_empty());
    let asked = session.answer(Answer::Opened);
    let out = text(&asked);
    assert!(out.starts_with("GET /page HTTP/1.1\r\n"), "{out}");
    assert!(out.contains("User-Agent: x\r\n"), "{out}");
}

#[test]
fn a_head_that_never_ends_is_given_up_on() {
    let mut session = Session::new();
    let huge = vec![b'x'; MOST_HEAD + 1];
    session.feed(b"GET http://example.invalid/ HTTP/1.1\r\nX: ");
    session.feed(&huge);
    assert_eq!(session.state(), State::Failed);
    assert!(text(&session.take_out()).starts_with("HTTP/1.1 400 "));
}

#[test]
fn the_first_octet_says_which_protocol_it_is() {
    // One octet, because it may be all there is for a while. RFC 1928 3 fixes
    // the SOCKS greeting's first octet at 5; every method name in RFC 9110 9
    // is uppercase.
    assert!(speaks_http(b'G'));
    assert!(speaks_http(b'C'));
    assert!(speaks_http(b'P'));
    assert!(!speaks_http(5));
    assert!(!speaks_http(4));
    assert!(!speaks_http(0));
}

#[test]
fn a_userinfo_in_the_target_does_not_reach_the_origin_server() {
    let mut session = Session::new();
    session.feed(b"GET http://someone@example.invalid/ HTTP/1.1\r\n\r\n");
    assert_eq!(
        session.request().map(|t| t.authority.as_str()),
        Some("example.invalid:80")
    );
}

#[test]
fn a_request_with_a_bare_linefeed_is_still_read() {
    // 9112 2.2 asks a recipient to accept a bare LF as a line terminator.
    let mut session = Session::new();
    let asked = forward(&mut session, b"GET http://example.invalid/ HTTP/1.1\nHost: x\n\n");
    let out = text(&asked);
    assert!(out.starts_with("GET / HTTP/1.1\r\n"), "{out}");
    // And what goes out is written properly whatever came in.
    assert!(out.ends_with("\r\n\r\n"), "{out}");
}
