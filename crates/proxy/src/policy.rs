//! Where the answering end's proxy will go on a caller's behalf.
//!
//! The proxy at the answering end opens sockets from the answering machine,
//! so whoever dialled in reaches whatever that machine can reach -- and that
//! includes the machine itself and the network it sits on, which are exactly
//! the places a firewall assumes nobody outside can get to: a database bound
//! to 127.0.0.1, a router's admin page, a printer, a cloud host's metadata
//! address. The caller came for the internet, so that is where it goes.
//!
//! RFC 9110 9.3.6 asks the same of CONNECT in particular: a tunnel carries
//! anything at all once it is open, so "proxies that support CONNECT SHOULD
//! restrict its use to a limited set of known ports". Here that is 443, the
//! one a browser uses it for. A plain request is HTTP that this end writes
//! itself, and it may go to 80, 443 or anything above the ports that belong
//! to other protocols, as a web address can name.
//!
//! The client side keeps its own caller away from the machine it runs on for
//! a different reason, in [`crate::resolve`]: there, what is sent is a
//! provider's router, which has no business being handed this machine's
//! addresses at all.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Why a destination is off limits, or None where it is not.
///
/// `tunnel` is whether it is a CONNECT, which can carry anything and so goes
/// to fewer ports.
pub fn refused(address: IpAddr, port: u16, tunnel: bool) -> Option<&'static str> {
    // An IPv4 address written as IPv6 is still that IPv4 address:
    // ::ffff:127.0.0.1 is this machine as much as 127.0.0.1 is, and
    // Ipv6Addr::is_loopback does not say so.
    let address = address.to_canonical();
    if let Some(why) = match address {
        IpAddr::V4(v4) => off_limits_v4(v4),
        IpAddr::V6(v6) => off_limits_v6(v6),
    } {
        return Some(why);
    }
    if tunnel && port != 443 {
        return Some("a tunnel goes only to port 443");
    }
    if !tunnel && port != 80 && port != 443 && port < 1024 {
        return Some("that port belongs to something other than the web");
    }
    None
}

fn off_limits_v4(ip: Ipv4Addr) -> Option<&'static str> {
    let [a, b, ..] = ip.octets();
    if ip.is_loopback() {
        Some("that is the answering machine itself")
    } else if ip.is_private()
        || ip.is_link_local()
        // RFC 6598's shared address space, which a carrier's NAT or a
        // Tailscale network hands out and `is_private` does not cover.
        || (a == 100 && (64..128).contains(&b))
    {
        Some("that is the answering machine's own network")
    } else if a == 0
        || ip.is_broadcast()
        || ip.is_multicast()
        // RFC 6890's 240/4, reserved and not somewhere to connect to.
        || a >= 240
    {
        Some("that is not an address a connection can go to")
    } else {
        None
    }
}

fn off_limits_v6(ip: Ipv6Addr) -> Option<&'static str> {
    let first = ip.segments()[0];
    if ip.is_loopback() {
        Some("that is the answering machine itself")
    } else if ip.is_unique_local() || ip.is_unicast_link_local() || first & 0xffc0 == 0xfec0 {
        // fc00::/7, fe80::/10, and the old site-local fec0::/10.
        Some("that is the answering machine's own network")
    } else if ip.is_unspecified() || ip.is_multicast() {
        Some("that is not an address a connection can go to")
    } else if let Some(v4) = embedded_v4(ip) {
        off_limits_v4(v4)
    } else {
        None
    }
}

/// The IPv4 address inside a NAT64 or 6to4 address, which reaches that
/// IPv4 address however it is written.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let o = ip.octets();
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        // RFC 6052's well-known prefix, 64:ff9b::/96.
        Some(Ipv4Addr::new(o[12], o[13], o[14], o[15]))
    } else if s[0] == 0x2002 {
        // RFC 3056's 6to4, 2002:V4ADDR::/48.
        Some(Ipv4Addr::new(o[2], o[3], o[4], o[5]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(a: [u8; 4]) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from(a))
    }

    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().expect("not an IPv6 address"))
    }

    #[test]
    fn the_answering_machine_and_its_network_are_off_limits() {
        for address in [
            v4([127, 0, 0, 1]),
            v4([127, 9, 9, 9]),
            v4([10, 0, 0, 5]),
            v4([172, 16, 0, 1]),
            v4([172, 31, 255, 254]),
            v4([192, 168, 1, 1]),
            v4([169, 254, 169, 254]),
            v4([100, 64, 0, 1]),
            v4([100, 127, 255, 255]),
            v4([0, 0, 0, 0]),
            v4([255, 255, 255, 255]),
            v4([224, 0, 0, 1]),
            v4([240, 0, 0, 1]),
            v6("::1"),
            v6("::"),
            v6("::ffff:127.0.0.1"),
            v6("::ffff:192.168.0.10"),
            v6("fe80::1"),
            v6("fd12:3456::1"),
            v6("fec0::1"),
            v6("ff02::1"),
            v6("64:ff9b::7f00:1"),
            v6("64:ff9b::a00:1"),
            v6("2002:c0a8:101::1"),
        ] {
            assert!(refused(address, 80, false).is_some(), "{address} was allowed");
            assert!(refused(address, 443, true).is_some(), "{address} was allowed a tunnel");
        }
    }

    #[test]
    fn the_internet_is_not() {
        for address in [
            v4([93, 184, 216, 34]),
            v4([8, 8, 8, 8]),
            v4([100, 63, 255, 255]),
            v4([100, 128, 0, 0]),
            v4([172, 32, 0, 1]),
            v4([192, 169, 0, 1]),
            v6("2606:2800:220:1:248:1893:25c8:1946"),
            v6("::ffff:93.184.216.34"),
            v6("64:ff9b::5db8:d822"),
        ] {
            assert_eq!(refused(address, 80, false), None, "{address}");
            assert_eq!(refused(address, 443, true), None, "{address}");
        }
    }

    /// RFC 9110 9.3.6: a tunnel is limited to known ports, and a plain request
    /// is kept off the ports that belong to other protocols.
    #[test]
    fn a_tunnel_goes_only_to_443_and_a_request_to_web_ports() {
        let internet = v4([93, 184, 216, 34]);
        for port in [22, 25, 80, 8443, 6379] {
            assert!(refused(internet, port, true).is_some(), "a tunnel to {port} was allowed");
        }
        for port in [22, 25, 110, 143, 445] {
            assert!(refused(internet, port, false).is_some(), "a request to {port} was allowed");
        }
        for port in [80, 443, 1024, 8080, 8443] {
            assert_eq!(refused(internet, port, false), None, "a request to {port}");
        }
    }
}
