//! Asking the router to let a call in: UPnP's Internet Gateway Device.
//!
//! A SIP agent behind a home router can call out, because the router
//! remembers who it sent to and lets the answer back. It cannot be called,
//! and the sound of a call it placed arrives only if the far end sends to
//! wherever the router happened to send from. An account on a registrar
//! gets round both by keeping a path open through the registrar. EasySip
//! has no registrar: two of these, each with nothing but the other's
//! address. Across the internet that needs the router to forward two UDP
//! ports to this machine, and to say what its own outside address is so
//! that the far end can be told where to send.
//!
//! UPnP IGD is the protocol most home routers have for exactly that
//! (UPnP Device Architecture 1.1, and WANIPConnection:1 for the actions):
//!
//! 1. **Find it.** An `M-SEARCH` to 239.255.255.250:1900 (SSDP); the router
//!    answers with the URL of a description of itself.
//! 2. **Read the description**, XML over HTTP, for the control URL of its
//!    WANIPConnection or WANPPPConnection service.
//! 3. **Ask**, SOAP over HTTP: `GetExternalIPAddress`, and an
//!    `AddPortMapping` for each port. `DeletePortMapping` when done.
//!
//! Nothing here is a general HTTP or XML reader. Routers send small, flat
//! documents, and what is wanted from each is one or two named values.
//!
//! It is also a way of opening holes in a firewall without asking anyone,
//! which is why it is only done when the account says to, the ports are
//! named in the window, and they are closed again when the line is.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// How long the router has to answer the search.
const SEARCH: Duration = Duration::from_millis(2500);

/// How long any one HTTP exchange with it may take.
const PATIENCE: Duration = Duration::from_secs(4);

/// What the mappings are called in the router's own list.
const DESCRIPTION: &str = "BinModem EasySip";

/// Seconds a mapping is asked for. A day: long enough for any call, and a
/// program that is killed rather than closed leaves nothing behind for
/// longer than that.
const LEASE: u32 = 86_400;

/// Ports forwarded on the router to this machine, and the router's outside
/// address. Dropping it asks the router to close them.
#[derive(Debug)]
pub struct Mapping {
    control: Url,
    service: String,
    ports: Vec<u16>,
    /// The address the internet sees this network at.
    pub external: IpAddr,
    /// This machine's address on the network the router is on.
    pub local: IpAddr,
}

impl Mapping {
    /// Find the router and have it forward each of `ports`, UDP, to the
    /// same port on this machine.
    pub fn open(ports: &[u16]) -> Result<Self, String> {
        let location = discover(SEARCH)?;
        Self::through(&location, ports)
    }

    /// The same of a router whose description is at `location`.
    pub fn through(location: &str, ports: &[u16]) -> Result<Self, String> {
        let location = Url::parse(location).ok_or_else(|| format!("UPnP: the router gave {location:?} as its description, which is not a URL"))?;
        let (description, local) = get(&location)?;
        let (service, control) = control_of(&description, &location)
            .ok_or("UPnP: the router's description has no WANIPConnection or WANPPPConnection service in it")?;
        let answer = soap(&control, &service, "GetExternalIPAddress", &[])?;
        let external: IpAddr = value_of(&answer, "NewExternalIPAddress")
            .and_then(|a| a.trim().parse().ok())
            .ok_or("UPnP: the router did not say what its outside address is")?;
        if !is_public(external) {
            return Err(format!(
                "UPnP: the router's outside address is {external}, which is not a public one: it is behind another router, or the provider's, and a port opened on it is not open to the internet"
            ));
        }
        let mut mapping = Self { control, service, ports: Vec::new(), external, local };
        for &port in ports {
            mapping.add(port)?;
        }
        Ok(mapping)
    }

    fn add(&mut self, port: u16) -> Result<(), String> {
        let (port_text, local) = (port.to_string(), self.local.to_string());
        let ask = |lease: u32| {
            let lease = lease.to_string();
            soap(
                &self.control,
                &self.service,
                "AddPortMapping",
                &[
                    ("NewRemoteHost", ""),
                    ("NewExternalPort", &port_text),
                    ("NewProtocol", "UDP"),
                    ("NewInternalPort", &port_text),
                    ("NewInternalClient", &local),
                    ("NewEnabled", "1"),
                    ("NewPortMappingDescription", DESCRIPTION),
                    ("NewLeaseDuration", &lease),
                ],
            )
        };
        // 725, OnlyPermanentLeasesSupported: some routers take a lease of
        // nought and no other.
        match ask(LEASE) {
            Ok(_) => {}
            Err(e) if e.contains("725") => {
                ask(0)?;
            }
            Err(e) => return Err(e),
        }
        self.ports.push(port);
        Ok(())
    }

    /// The ports that are open, for saying so.
    pub fn ports(&self) -> &[u16] {
        &self.ports
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        for port in &self.ports {
            let _ = soap(
                &self.control,
                &self.service,
                "DeletePortMapping",
                &[("NewRemoteHost", ""), ("NewExternalPort", &port.to_string()), ("NewProtocol", "UDP")],
            );
        }
    }
}

/// Whether an address is one the internet can send to.
///
/// Not a private network's, a machine's own, a link's, or the range
/// providers use between their own router and a customer's (RFC 6598): a
/// port forwarded on a router whose outside address is one of those leads
/// nowhere.
pub fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(a) => {
            let o = a.octets();
            !(a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || (o[0] == 100 && (64..128).contains(&o[1])))
        }
        IpAddr::V6(a) => !(a.is_loopback() || a.is_unspecified() || (a.segments()[0] & 0xfe00) == 0xfc00 || (a.segments()[0] & 0xffc0) == 0xfe80),
    }
}

/// Search the local network for a gateway, and give the URL of the first
/// description offered.
fn discover(patience: Duration) -> Result<String, String> {
    // From the address this machine reaches the internet by, and not from
    // whichever of its networks the system lists first: a machine with a
    // virtual adapter or two sends a search from nowhere in particular
    // otherwise, and the router never hears it.
    let from = outward().map_or_else(|| "0.0.0.0".to_owned(), |a| a.to_string());
    let socket = UdpSocket::bind((from.as_str(), 0)).map_err(|e| format!("UPnP: no socket to search with: {e}"))?;
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));
    // And to where a home router usually is as well as to everybody, since
    // some answer a search sent to them that they ignore when it is sent to
    // the group.
    let mut to = vec!["239.255.255.250:1900".to_owned()];
    if let Some(IpAddr::V4(here)) = outward() {
        let [a, b, c, _] = here.octets();
        to.extend([1u8, 254].map(|last| format!("{a}.{b}.{c}.{last}:1900")));
    }
    // Asked for by device and by each service, since routers differ in
    // which they answer to.
    for target in [
        "urn:schemas-upnp-org:device:InternetGatewayDevice:1",
        "urn:schemas-upnp-org:service:WANIPConnection:1",
        "urn:schemas-upnp-org:service:WANPPPConnection:1",
        "urn:schemas-upnp-org:device:InternetGatewayDevice:2",
    ] {
        let search = format!(
            "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {target}\r\n\r\n"
        );
        for address in &to {
            let _ = socket.send_to(search.as_bytes(), address.as_str());
        }
    }
    let began = Instant::now();
    let mut buffer = [0u8; 2048];
    while began.elapsed() < patience {
        let Ok((n, _)) = socket.recv_from(&mut buffer) else { continue };
        let text = String::from_utf8_lossy(&buffer[..n]);
        if let Some(location) = location_of(&text) {
            return Ok(location);
        }
    }
    Err("UPnP: no router answered. It may not have UPnP, or have it turned off".to_owned())
}

/// The address of this machine that packets for the internet leave from.
/// Nothing is sent to find out: a UDP socket is only told where it would
/// send, and asked which of this machine's addresses it would use.
fn outward() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let here = socket.local_addr().ok()?.ip();
    (!here.is_unspecified()).then_some(here)
}

/// The LOCATION of an SSDP answer that is about a gateway.
fn location_of(answer: &str) -> Option<String> {
    let about = answer.to_ascii_lowercase();
    if !(about.contains("internetgatewaydevice") || about.contains("wanipconnection") || about.contains("wanpppconnection")) {
        return None;
    }
    answer.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim().eq_ignore_ascii_case("location").then(|| value.trim().to_owned())
    })
}

/// As much of a URL as a router's is: `http://host:port/path`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Url {
    host: String,
    port: u16,
    path: String,
}

impl Url {
    fn parse(text: &str) -> Option<Self> {
        let rest = text.trim().strip_prefix("http://")?;
        let (authority, path) = rest.find('/').map_or((rest, "/"), |i| rest.split_at(i));
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, 80),
        };
        (!host.is_empty()).then(|| Self { host: host.to_owned(), port, path: path.to_owned() })
    }

    /// A URL found in this one's document: whole, or a path on this host.
    fn join(&self, found: &str) -> Option<Self> {
        let found = found.trim();
        if found.starts_with("http://") {
            return Self::parse(found);
        }
        let path = if found.starts_with('/') { found.to_owned() } else { format!("/{found}") };
        Some(Self { path, ..self.clone() })
    }

    fn address(&self) -> Result<SocketAddr, String> {
        (self.host.as_str(), self.port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut found| found.next())
            .ok_or_else(|| format!("UPnP: {} is not an address", self.host))
    }
}

/// One HTTP exchange: the body of the answer, the status, and the address
/// this machine was speaking from.
fn exchange(url: &Url, request: &str) -> Result<(u16, String, IpAddr), String> {
    let failed = |e: std::io::Error| format!("UPnP: talking to the router at {}:{}: {e}", url.host, url.port);
    let mut stream = TcpStream::connect_timeout(&url.address()?, PATIENCE).map_err(failed)?;
    let _ = stream.set_read_timeout(Some(PATIENCE));
    let _ = stream.set_write_timeout(Some(PATIENCE));
    let local = stream.local_addr().map_err(failed)?.ip();
    stream.write_all(request.as_bytes()).map_err(failed)?;
    let mut raw = Vec::new();
    // A router that keeps the connection open after its answer is read
    // until it times out, and what arrived by then is the answer.
    let _ = stream.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").ok_or("UPnP: the router's answer was not HTTP")?;
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("UPnP: the router's answer had no status")?;
    let chunked = head.to_ascii_lowercase().contains("transfer-encoding: chunked");
    Ok((status, if chunked { unchunk(body) } else { body.to_owned() }, local))
}

/// A body sent in chunks (RFC 9112 7.1), put back together.
fn unchunk(body: &str) -> String {
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16) else { break };
        if size == 0 || size > after.len() || !after.is_char_boundary(size) {
            break;
        }
        out.push_str(&after[..size]);
        rest = after[size..].trim_start_matches("\r\n");
    }
    out
}

fn get(url: &Url) -> Result<(String, IpAddr), String> {
    let request = format!("GET {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n", url.path, url.host, url.port);
    let (status, body, local) = exchange(url, &request)?;
    if status != 200 {
        return Err(format!("UPnP: the router answered {status} when asked for its description"));
    }
    Ok((body, local))
}

/// One action of the service, and the body of what came back.
fn soap(control: &Url, service: &str, action: &str, arguments: &[(&str, &str)]) -> Result<String, String> {
    let body = envelope(service, action, arguments);
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: text/xml; charset=\"utf-8\"\r\nSOAPAction: \"{service}#{action}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        control.path,
        control.host,
        control.port,
        body.len()
    );
    let (status, answer, _) = exchange(control, &request)?;
    if status == 200 {
        return Ok(answer);
    }
    let code = value_of(&answer, "errorCode").unwrap_or_default();
    let said = value_of(&answer, "errorDescription").unwrap_or_default();
    Err(format!("UPnP: the router refused {action}: {status} {code} {said}").trim_end().to_owned())
}

fn envelope(service: &str, action: &str, arguments: &[(&str, &str)]) -> String {
    let mut inside = String::new();
    for (name, value) in arguments {
        inside.push_str(&format!("<{name}>{}</{name}>", escape(value)));
    }
    format!(
        "<?xml version=\"1.0\"?>\r\n<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service}\">{inside}</u:{action}></s:Body></s:Envelope>\r\n"
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The text inside the first `<name>` element, whatever prefix it has.
fn value_of(document: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(found) = document[from..].find(name) {
        let at = from + found;
        from = at + name.len();
        // An opening tag: `<name>` or `<prefix:name>`.
        let before = document[..at].rfind('<')?;
        let prefix = &document[before + 1..at];
        if !(prefix.is_empty() || (prefix.ends_with(':') && !prefix.contains(['/', ' ', '>']))) {
            continue;
        }
        let Some(rest) = document[from..].strip_prefix('>') else { continue };
        let end = rest.find('<')?;
        return Some(rest[..end].trim().to_owned());
    }
    None
}

/// The service that maps ports, and where it is controlled, out of a
/// gateway's description.
fn control_of(description: &str, location: &Url) -> Option<(String, Url)> {
    let base = value_of(description, "URLBase").and_then(|b| Url::parse(&b)).unwrap_or_else(|| location.clone());
    // IP before PPP: a router with both has the one in use first in
    // practice, and the IP one is what a cable or fibre line has.
    for wanted in ["WANIPConnection", "WANPPPConnection"] {
        for service in description.split("<service>").skip(1) {
            let service = service.split("</service>").next().unwrap_or("");
            let Some(kind) = value_of(service, "serviceType") else { continue };
            if !kind.contains(wanted) {
                continue;
            }
            if let Some(control) = value_of(service, "controlURL").and_then(|c| base.join(&c)) {
                return Some((kind, control));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    const DESCRIPTION_XML: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><device>
<deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
<serviceList><service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>
<controlURL>/ctl/L3F</controlURL></service></serviceList>
<deviceList><device><deviceList><device>
<serviceList><service>
<serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
<serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
<controlURL>/ctl/IPConn</controlURL>
<eventSubURL>/evt/IPConn</eventSubURL>
</service></serviceList></device></deviceList></device></deviceList></device></root>"#;

    #[test]
    fn the_control_url_is_the_wan_connection_s_and_not_the_first_in_the_document() {
        let location = Url::parse("http://192.168.1.1:5000/rootDesc.xml").unwrap();
        let (service, control) = control_of(DESCRIPTION_XML, &location).unwrap();
        assert_eq!(service, "urn:schemas-upnp-org:service:WANIPConnection:1");
        assert_eq!(control, Url { host: "192.168.1.1".to_owned(), port: 5000, path: "/ctl/IPConn".to_owned() });
        assert!(control_of("<root><service><serviceType>x</serviceType></service></root>", &location).is_none());
    }

    #[test]
    fn a_search_answer_gives_its_location_only_if_it_is_a_gateway_s() {
        let gateway = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\nLocation: http://192.168.1.1:5000/rootDesc.xml\r\n\r\n";
        assert_eq!(location_of(gateway).as_deref(), Some("http://192.168.1.1:5000/rootDesc.xml"));
        let television = "HTTP/1.1 200 OK\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\nLOCATION: http://192.168.1.9/tv.xml\r\n\r\n";
        assert_eq!(location_of(television), None);
    }

    #[test]
    fn values_are_read_whatever_their_prefix_and_chunks_are_joined() {
        let answer = "<s:Body><u:GetExternalIPAddressResponse><NewExternalIPAddress>203.0.113.7</NewExternalIPAddress></u:GetExternalIPAddressResponse></s:Body>";
        assert_eq!(value_of(answer, "NewExternalIPAddress").as_deref(), Some("203.0.113.7"));
        let fault = "<s:Fault><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\"><errorCode>718</errorCode><errorDescription>ConflictInMappingEntry</errorDescription></UPnPError></detail></s:Fault>";
        assert_eq!(value_of(fault, "errorCode").as_deref(), Some("718"));
        assert_eq!(unchunk("5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"), "hello world");
    }

    #[test]
    fn only_addresses_the_internet_can_reach_count_as_public() {
        for (address, public) in [
            ("203.0.113.7", true),
            ("8.8.8.8", true),
            ("192.168.1.1", false),
            ("10.0.0.1", false),
            ("172.20.1.1", false),
            ("100.64.3.2", false),
            ("127.0.0.1", false),
            ("169.254.1.1", false),
        ] {
            assert_eq!(is_public(address.parse().unwrap()), public, "{address}");
        }
    }

    /// A router of the simplest kind: a description, an outside address,
    /// and a list of what it was asked to do.
    fn router(external: &'static str, refuse_leases: bool) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked = Arc::new(Mutex::new(Vec::new()));
        let log = asked.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut raw = Vec::new();
                let mut buffer = [0u8; 4096];
                // Read the head, then as much body as it says there is.
                let request = loop {
                    let Ok(n) = stream.read(&mut buffer) else { break String::new() };
                    raw.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let length = head
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if body.len() >= length {
                            break text;
                        }
                    }
                    if n == 0 {
                        break text;
                    }
                };
                let (status, body) = if request.starts_with("GET ") {
                    ("200 OK", DESCRIPTION_XML.to_owned())
                } else if request.contains("#GetExternalIPAddress") {
                    ("200 OK", format!("<NewExternalIPAddress>{external}</NewExternalIPAddress>"))
                } else if request.contains("#AddPortMapping") {
                    let port = value_of(&request, "NewExternalPort").unwrap_or_default();
                    let lease = value_of(&request, "NewLeaseDuration").unwrap_or_default();
                    if refuse_leases && lease != "0" {
                        ("500 Internal Server Error", "<errorCode>725</errorCode><errorDescription>OnlyPermanentLeasesSupported</errorDescription>".to_owned())
                    } else {
                        log.lock().unwrap().push(format!("add {port} to {} lease {lease}", value_of(&request, "NewInternalClient").unwrap_or_default()));
                        ("200 OK", String::new())
                    }
                } else if request.contains("#DeletePortMapping") {
                    log.lock().unwrap().push(format!("delete {}", value_of(&request, "NewExternalPort").unwrap_or_default()));
                    ("200 OK", String::new())
                } else {
                    ("404 Not Found", String::new())
                };
                let _ = stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}/rootDesc.xml"), asked)
    }

    #[test]
    fn ports_are_opened_to_this_machine_and_closed_when_the_mapping_is_let_go() {
        let (location, asked) = router("203.0.113.7", false);
        let mapping = Mapping::through(&location, &[5060, 16504]).unwrap();
        assert_eq!(mapping.external, "203.0.113.7".parse::<IpAddr>().unwrap());
        assert_eq!(mapping.ports(), &[5060, 16504]);
        drop(mapping);
        assert_eq!(
            *asked.lock().unwrap(),
            ["add 5060 to 127.0.0.1 lease 86400", "add 16504 to 127.0.0.1 lease 86400", "delete 5060", "delete 16504"]
        );
    }

    /// The router this machine is behind, for real: one port opened and
    /// closed again. Run by hand, since it changes something outside.
    #[test]
    #[ignore = "asks the real router on this network to open and close a port"]
    fn the_router_on_this_network_opens_and_closes_a_port() {
        let began = Instant::now();
        match Mapping::open(&[45_062]) {
            Ok(mapping) => {
                println!(
                    "UPNP ok in {:.1} s: service {}, control {}:{}{}, local {}, outside address is public: {}",
                    began.elapsed().as_secs_f64(),
                    mapping.service,
                    mapping.control.host,
                    mapping.control.port,
                    mapping.control.path,
                    mapping.local,
                    is_public(mapping.external)
                );
                if let Ok(file) = std::env::var("UPNP_ADDRESS_FILE") {
                    let _ = std::fs::write(file, mapping.external.to_string());
                }
                let (control, service) = (mapping.control.clone(), mapping.service.clone());
                let entry = |port: &str| {
                    soap(&control, &service, "GetSpecificPortMappingEntry", &[("NewRemoteHost", ""), ("NewExternalPort", port), ("NewProtocol", "UDP")])
                };
                assert!(entry("45062").is_ok(), "the router does not list the port it said it opened");
                drop(mapping);
                assert!(entry("45062").is_err(), "the port is still open after it was closed");
                println!("UPNP the port was listed while open and gone once closed");
            }
            Err(why) => println!("UPNP failed in {:.1} s: {why}", began.elapsed().as_secs_f64()),
        }
    }

    #[test]
    fn a_router_that_only_has_permanent_mappings_is_asked_for_one() {
        let (location, asked) = router("203.0.113.7", true);
        let mapping = Mapping::through(&location, &[5060]).unwrap();
        assert_eq!(asked.lock().unwrap()[0], "add 5060 to 127.0.0.1 lease 0");
        drop(mapping);
    }

    #[test]
    fn a_router_behind_another_is_said_to_be_and_nothing_is_opened_on_it() {
        let (location, asked) = router("100.72.1.9", false);
        let refused = Mapping::through(&location, &[5060]).unwrap_err();
        assert!(refused.contains("not a public one"), "{refused}");
        assert!(asked.lock().unwrap().is_empty());
    }
}
