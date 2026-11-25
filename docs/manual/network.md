# Network: PPP, the dial-in server and TCP

[Back to the README](../../README.md)

[Two of them on a network](../usage.md#two-of-them-on-a-network) gives the
procedures.

## A call to a provider

BinModem dials a real provider. The Network panel starts PPP and logs in if
the far end asks. The far end gives it an address. An ICMP echo then crosses
and comes back with a round-trip time.

## Web pages

1. Tick *carry web traffic*.
2. Set the HTTP proxy of the browser to `127.0.0.1:8080` for http and https.

Pages then come directly from the internet. BinModem reads the request on this
machine. The connection goes to the address of the web server itself, with the
TCP of this program, through the router of the provider.

Between two BinModems, the machine that answered offers its internet instead.
The proxy finds automatically which type of far end it has.

## The dial-in server

The end that answers can be a dial-in server, as the modem pool of a provider
was. A caller gets a banner, `login:` and `Password:`, and then a prompt where
`ppp` starts PPP. **Log in, then PPP** at the end that calls answers those
prompts automatically.

Some diallers skip the text and start PPP immediately. The server asks such a
dialler for the same account over CHAP or PAP (RFC 1994, RFC 1334). Thus a
program that is not BinModem can dial in.

## TCP

The TCP is our own, written against RFC 9293. It has:

- the eleven states
- retransmission with the estimator of RFC 6298
- Nagle
- delayed acknowledgements
- zero-window probing
- out-of-order reassembly
- the fast retransmit of RFC 5681

Almost no part of the path belongs to the operating system. The exceptions are
the socket from the browser to the proxy, and the name lookups. There is no
driver, no adapter, no route and no administrator.

## The largest test

The largest test does all of this against a simulated line: V.8, V.22bis,
V.42, V.42bis, PPP, IPCP, TCP, the proxy, and a real web server on the
loopback.

- The modems connect six seconds into the call.
- The network phase starts at seven seconds.
- A page comes back at 2400 bit/s.
- The test runs without a sound card.
