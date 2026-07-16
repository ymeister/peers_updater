use crate::peer::Peer;
use nu_json::Map;
use std::cell::Cell;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
#[cfg(not(target_os = "windows"))]
use std::os::unix::net::UnixStream;
use std::time;

#[derive(Debug, PartialEq)]
enum SockAddr {
    Tcp(SocketAddr),
    #[cfg(not(target_os = "windows"))]
    Unix(String),
    None,
}

enum Connection {
    Tcp(TcpStream),
    #[cfg(not(target_os = "windows"))]
    Unix(UnixStream),
    None,
}

// A peer from the getpeers response.
struct ConnectedPeer {
    remote: String,
    // The host parsed from `remote`; None if the URI is not recognized
    host: Option<String>,
    up: bool,
    // None: the daemon did not report the direction (older Yggdrasil)
    inbound: Option<bool>,
}

pub enum QuickCheck {
    // Exactly the requested number of healthy managed peers is connected
    // and there are no down or duplicate managed connections to trim
    Satisfied { extras_added: usize },
    // The managed peers differ from the requested state: downloading and
    // reconciling is needed
    NeedsWork,
    // The getpeers reply could not be parsed: peers can still be added,
    // but not counted or removed
    AddOnly,
    // The admin socket could not be reached
    Error,
}

// The getpeers result
enum Fetch {
    Peers(Vec<ConnectedPeer>),
    Unparseable,
    Error,
}

// Checks whether exactly the requested number of healthy managed peers is
// connected (without downloading anything) and ensures the extra
// ("always in") peers are present.
pub fn quick_check(socket_addr_str: &str, n_peers: u8, always_in_p: Option<&String>) -> QuickCheck {
    Session::new(socket_addr_str, always_in_p).check(n_peers)
}

pub fn update_peers(
    peers: &[Peer],
    socket_addr_str: &str,
    n_peers: u8,
    always_in_p: Option<&String>,
    force_reconcile: bool,
) -> bool {
    let session = Session::new(socket_addr_str, always_in_p);

    match session.fetch_connected() {
        Fetch::Error => false,
        Fetch::Unparseable => {
            eprintln!("Proceeding without removing peers.");
            session.add_only(peers, n_peers);
            !session.io_failed.get()
        }
        Fetch::Peers(connected) => {
            // With the ignore filters (`force_reconcile`) the connected peers
            // can only be checked against them by a full reconciliation
            if !force_reconcile && session.satisfied(n_peers, &connected) {
                // Nothing to change, just ensure the extra peers are present
                session.assert_extras(&connected);
            } else {
                session.reconcile(peers, n_peers, &connected);
            }
            !session.io_failed.get()
        }
    }
}

struct Session {
    socket_addr: SockAddr,
    extras: Vec<Extra>,
    re: regex::Regex,
    // Set when a peer request could not be delivered to the daemon
    io_failed: Cell<bool>,
}

// An extra ("always in") peer from the `-e` option
struct Extra {
    raw: String,
    host: Option<String>,
}

impl Session {
    fn new(socket_addr_str: &str, always_in_p: Option<&String>) -> Self {
        let re = crate::peer::uri_regex_anchored();
        let extras = extras_vec(always_in_p)
            .into_iter()
            .map(|raw| {
                let host = extract_host_port(&raw, &re).map(|(_h, _)| _h);
                if host.is_none() {
                    eprintln!(
                        "Warning: the extra peer URI is not recognized and will be matched literally ({}).",
                        raw
                    );
                }
                Extra { raw, host }
            })
            .collect();
        Session {
            socket_addr: get_socket_addr(socket_addr_str),
            extras,
            re,
            io_failed: Cell::new(false),
        }
    }

    // Requests the connected peers and checks whether exactly the requested
    // number of healthy managed ones is connected (with nothing to trim);
    // if so, also ensures the extra peers are present. Managed peers are
    // outbound public (non local-network) peers with a recognized URI that
    // are not in the extra ("always in") list — the only peers this utility
    // adds and removes.
    fn check(&self, n_peers: u8) -> QuickCheck {
        match self.fetch_connected() {
            Fetch::Error => QuickCheck::Error,
            Fetch::Unparseable => QuickCheck::AddOnly,
            Fetch::Peers(connected) => {
                if !self.satisfied(n_peers, &connected) {
                    return QuickCheck::NeedsWork;
                }
                let extras_added = self.assert_extras(&connected);
                if self.io_failed.get() {
                    QuickCheck::Error
                } else {
                    QuickCheck::Satisfied { extras_added }
                }
            }
        }
    }

    // Whether the connected managed peers need no reconciliation: exactly
    // n unique up hosts with no down or duplicate managed connections
    // left to remove
    fn satisfied(&self, n_peers: u8, connected: &[ConnectedPeer]) -> bool {
        let mut hosts: Vec<&String> = Vec::new();
        for cp in connected {
            if !self.is_managed(cp) {
                continue;
            }
            match &cp.host {
                Some(_h) if cp.up && !hosts.contains(&_h) => hosts.push(_h),
                // A down or duplicate connection would have to be removed
                _ => return false,
            }
        }
        hosts.len() == n_peers as usize
    }

    // Brings the managed peers to the best n alive public peers: the managed
    // peers that are up and in the desired set are kept, down and duplicate
    // connections are removed, the missing desired ones are added. Multicast
    // (link-local), local-network, inbound, extra and unrecognized peers are
    // not touched.
    fn reconcile(&self, peers: &[Peer], n_peers: u8, connected: &[ConnectedPeer]) {
        let (desired, desired_hosts) = self.select_desired(peers, n_peers);

        // Removing peers when there is nothing to replace them with would
        // only degrade connectivity (e.g. when all the latency probes failed)
        if desired.is_empty() && n_peers > 0 {
            eprintln!("No alive public peers found; keeping the existing peers.");
            self.assert_extras(connected);
            return;
        }

        let (removals, adds) = self.plan(&desired, &desired_hosts, n_peers, connected);
        for cp in removals {
            self.remove_peer(&cp.remote);
        }
        for peer in adds {
            self.add_peer(&peer.uri);
        }

        //Always in
        self.assert_extras(connected);
    }

    // Decides which managed connections to remove and which desired peers
    // to add. Down and duplicate connections are always removed; a healthy
    // peer that is no longer among the desired ones is removed only when a
    // desired peer is there to take its slot — when the alive downloaded
    // peers cannot fill the quota (e.g. most latency probes failed), the
    // connected healthy peers keep the remaining slots instead
    fn plan<'a, 'b>(
        &self,
        desired: &[&'a Peer],
        desired_hosts: &[String],
        n_peers: u8,
        connected: &'b [ConnectedPeer],
    ) -> (Vec<&'b ConnectedPeer>, Vec<&'a Peer>) {
        let mut seen_hosts: Vec<&String> = Vec::new();
        let mut in_best: Vec<&String> = Vec::new();
        let mut fillers: Vec<&ConnectedPeer> = Vec::new();
        let mut removals: Vec<&ConnectedPeer> = Vec::new();

        for cp in connected {
            if !self.is_managed(cp) {
                continue;
            }
            match &cp.host {
                Some(_h) if cp.up && !seen_hosts.contains(&_h) => {
                    seen_hosts.push(_h);
                    if desired_hosts.contains(_h) {
                        in_best.push(_h);
                    } else {
                        fillers.push(cp);
                    }
                }
                _ => removals.push(cp),
            }
        }

        let filler_slots = (n_peers as usize).saturating_sub(desired_hosts.len());
        removals.extend(fillers.into_iter().skip(filler_slots));

        let adds = desired
            .iter()
            .zip(desired_hosts)
            .filter(|(_, _h)| !in_best.contains(_h))
            .map(|(_p, _)| *_p)
            .collect();

        (removals, adds)
    }

    // The desired set: the best n alive public peers, one per host,
    // not duplicating the hosts of the extra peers
    fn select_desired<'a>(&self, peers: &'a [Peer], n_peers: u8) -> (Vec<&'a Peer>, Vec<String>) {
        let mut desired: Vec<&Peer> = Vec::with_capacity(n_peers.into());
        let mut desired_hosts: Vec<String> = Vec::with_capacity(n_peers.into());
        for peer in peers {
            if desired.len() >= n_peers as usize || !peer.is_alive {
                break;
            }
            let host = peer.addr.to_lowercase();
            if desired_hosts.contains(&host)
                || self.extras.iter().any(|e| e.host.as_ref() == Some(&host))
            {
                continue;
            }
            desired_hosts.push(host);
            desired.push(peer);
        }
        (desired, desired_hosts)
    }

    // Adds the first n alive public peers and the extras without removing
    // anything (used when the connected peers are unknown)
    fn add_only(&self, peers: &[Peer], n_peers: u8) {
        let (desired, _) = self.select_desired(peers, n_peers);
        for peer in desired {
            self.add_peer(&peer.uri);
        }
        self.assert_extras(&[]);
    }

    fn fetch_connected(&self) -> Fetch {
        let mut response = String::new();
        request(
            "{\"request\": \"getpeers\"}",
            &self.socket_addr,
            &mut response,
        );
        if response.is_empty() {
            eprintln!("Can't get connected peers.");
            return Fetch::Error;
        }

        match parse_connected(&response, &self.re) {
            Some(_c) => Fetch::Peers(_c),
            None => Fetch::Unparseable,
        }
    }

    fn is_extra(&self, cp: &ConnectedPeer) -> bool {
        self.extras
            .iter()
            .any(|e| uris_match(&e.host, &e.raw, &cp.host, &cp.remote))
    }

    fn is_managed(&self, cp: &ConnectedPeer) -> bool {
        cp.inbound == Some(false)
            && matches!(&cp.host, Some(_h) if host_is_public(_h))
            && !self.is_extra(cp)
    }

    // Adds the extra ("always in") peers that are not configured yet
    // (a configured but currently down extra is left alone - re-adding it
    // cannot help). Returns the number of extras the daemon accepted.
    fn assert_extras(&self, connected: &[ConnectedPeer]) -> usize {
        let mut added = 0;
        for extra in &self.extras {
            let present = connected
                .iter()
                .any(|cp| uris_match(&extra.host, &extra.raw, &cp.host, &cp.remote));
            if !present && self.add_peer(&extra.raw) {
                added += 1;
            }
        }
        added
    }

    fn add_peer(&self, peer_uri: &str) -> bool {
        self.peer_request("addpeer", peer_uri)
    }

    fn remove_peer(&self, peer_uri: &str) -> bool {
        self.peer_request("removepeer", peer_uri)
    }

    // Returns whether the daemon accepted the request. A request that
    // could not be delivered at all also raises `io_failed`, which makes
    // the run exit with a non-zero code
    fn peer_request(&self, action: &str, peer_uri: &str) -> bool {
        let mut resp = String::new();
        request(
            format!(
                "{{\"request\": \"{}\", \"arguments\": {{\"uri\": \"{}\"}}}}",
                action,
                json_escape(peer_uri)
            )
            .as_str(),
            &self.socket_addr,
            &mut resp,
        );

        // Transport errors are already reported by request()
        if resp.is_empty() {
            self.io_failed.set(true);
            return false;
        }
        if let Ok(_parsed) = nu_json::from_str::<Map<String, nu_json::Value>>(&resp) {
            if _parsed.get("status").and_then(|v| v.as_str()) == Some("error") {
                let error = _parsed
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                eprintln!("The {} request for {} failed ({}).", action, peer_uri, error);
                return false;
            }
        }
        true
    }
}

// URIs are considered equal when their hosts match (the port is
// disregarded); URIs that are not recognized are compared literally
// (case-insensitive)
fn uris_match(
    a_host: &Option<String>,
    a_raw: &str,
    b_host: &Option<String>,
    b_raw: &str,
) -> bool {
    match (a_host, b_host) {
        (Some(_a), Some(_b)) => _a == _b,
        _ => a_raw.trim().eq_ignore_ascii_case(b_raw.trim()),
    }
}

fn extras_vec(always_in_p: Option<&String>) -> Vec<String> {
    match always_in_p {
        Some(always_in) => always_in
            .split(' ')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
        None => Vec::new(),
    }
}

fn extract_host_port(uri: &str, re: &regex::Regex) -> Option<(String, String)> {
    let uri = uri.trim().to_lowercase();
    let cap = re.captures_iter(&uri).next()?;
    Some((
        cap.get(2)?.as_str().to_string(),
        cap.get(3)?.as_str().to_string(),
    ))
}

// Hosts this utility manages: addresses that could appear in the public
// peers list. Link-local, loopback, private (RFC1918) and unique-local
// addresses cannot, so peers on them were configured by other means and
// must not be removed
fn host_is_public(host: &str) -> bool {
    // A zone identifier is only meaningful for link-local addresses
    if host.contains('%') {
        return false;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(_a) = host.parse::<Ipv6Addr>() {
        return !((_a.segments()[0] & 0xffc0) == 0xfe80 // link-local
            || (_a.segments()[0] & 0xfe00) == 0xfc00 // unique-local (fc00::/7)
            || _a.is_loopback());
    }
    if let Ok(_a) = host.parse::<Ipv4Addr>() {
        return !(_a.is_link_local() || _a.is_private() || _a.is_loopback());
    }
    // A DNS name
    true
}

fn parse_connected(getpeers_resp: &str, re: &regex::Regex) -> Option<Vec<ConnectedPeer>> {
    //parse to obj
    //Serde deserialization is not used in order to get smaller binary files
    //(there is no need to describe the structures of the json objects used in our case).

    let parsed: Map<String, nu_json::Value> = match nu_json::from_str(getpeers_resp) {
        Ok(cp) => cp,
        Err(e) => {
            eprintln!("Error converting a json string to an object ({}).", e);
            return None;
        }
    };

    let resp = match parsed.get("response") {
        Some(_a) => _a,
        _ => {
            eprintln!("Couldn't get response from the getpeers result.");
            return None;
        }
    };

    let peers_val = match resp.as_object() {
        Some(pv) => match pv.get("peers") {
            Some(_a) => _a,
            _ => {
                eprintln!("Couldn't get peers from the response obj.");
                return None;
            }
        },
        _ => {
            eprintln!("Couldn't get peers from the response obj (0002).");
            return None;
        }
    };

    let mp_array = match peers_val.as_array() {
        Some(_mv) => _mv,
        _ => {
            eprintln!("Couldn't get peers array from the the response obj.");
            return None;
        }
    };

    let mut connected: Vec<ConnectedPeer> = Vec::with_capacity(mp_array.len());
    for peer in mp_array {
        let peer_obj = match peer.as_object() {
            Some(_po) => _po,
            _ => {
                //eprintln!("Couldn't get peer obj.");
                continue;
            }
        };

        let remote = match peer_obj.get("remote").and_then(|v| v.as_str()) {
            Some(_pu) => _pu.to_string(),
            _ => {
                //eprintln!("Couldn't get peer uri.");
                continue;
            }
        };

        connected.push(ConnectedPeer {
            host: extract_host_port(&remote, re).map(|(_h, _)| _h),
            remote,
            // A missing `up` (older Yggdrasil) means a connected peer
            up: peer_obj.get("up").and_then(|v| v.as_bool()).unwrap_or(true),
            // A missing `inbound` (older Yggdrasil) means the direction is
            // unknown: the peer will be neither counted nor removed
            inbound: peer_obj.get("inbound").and_then(|v| v.as_bool()),
        });
    }

    Some(connected)
}

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn socket_io<T: std::io::Write + std::io::Read>(
    conn: &mut T,
    req: &str,
    resp: &mut String,
) -> std::io::Result<()> {
    conn.write_all(req.as_bytes())?;
    conn.read_to_string(resp)?;

    Ok(())
}

fn request(req: &str, socket_addr: &SockAddr, resp: &mut String) {
    let connection = get_connection(socket_addr);

    match connection {
        Connection::Tcp(conn) => {
            let mut mut_conn = conn;
            if let Err(e) = socket_io(&mut mut_conn, req, resp) {
                eprintln!("Socket I/O error ({}).", e);
            }
        }
        #[cfg(not(target_os = "windows"))]
        Connection::Unix(conn) => {
            let mut mut_conn = conn;
            if let Err(e) = socket_io(&mut mut_conn, req, resp) {
                eprintln!("Socket I/O error ({}).", e);
            }
        }
        Connection::None => {
            eprintln!("Unable to connect to the administrator socket.");
        }
    };
}

fn get_connection(sock_addr: &SockAddr) -> Connection {
    match sock_addr {
        SockAddr::Tcp(_sa) => {
            match TcpStream::connect_timeout(_sa, time::Duration::from_secs(10)) {
                Ok(_s) => {
                    #[allow(clippy::needless_return)]
                    return Connection::Tcp(_s);
                }
                Err(e) => {
                    eprintln!("Failed to connect via TCP stream ({}).", e);
                    #[allow(clippy::needless_return)]
                    return Connection::None;
                }
            };
        }
        #[cfg(not(target_os = "windows"))]
        SockAddr::Unix(_sa) => {
            match UnixStream::connect(_sa) {
                Ok(_s) => {
                    #[allow(clippy::needless_return)]
                    return Connection::Unix(_s);
                }
                Err(e) => {
                    eprintln!("Failed to connect via unix domain socket ({}).", e);
                    #[allow(clippy::needless_return)]
                    return Connection::None;
                }
            };
        }
        SockAddr::None => {
            #[allow(clippy::needless_return)]
            return Connection::None;
        }
    };
}

fn get_socket_addr(string_addr: &str) -> SockAddr {
    let string_addr = string_addr.trim();
    if let Some(_path) = string_addr.strip_prefix("unix://") {
        //unix domain socket
        #[cfg(not(target_os = "windows"))]
        return SockAddr::Unix(_path.to_string());
        #[allow(unreachable_code)]
        {
            eprintln!("It is not possible to use a unix socket in Windows.");
            #[allow(clippy::needless_return)]
            return SockAddr::None;
        }
    } else {
        //tcp
        //Parsing the URI of the admin socket
        let re = crate::peer::uri_regex_anchored();
        let string_addr_lc = string_addr.to_lowercase();
        let mut cap_iter = re.captures_iter(&string_addr_lc);
        let cap = match cap_iter.next() {
            Some(_c) => _c,
            None => {
                eprintln!("Unable to parse socket URI ({}).", string_addr);
                return SockAddr::None;
            }
        };

        // The shared peer URI grammar also knows ws/wss, which make no
        // sense for the admin socket
        match cap.get(1).map(|_s| _s.as_str()) {
            Some("tcp") | Some("tls") | Some("quic") => {}
            _ => {
                eprintln!("Unable to parse socket URI ({}).", string_addr);
                return SockAddr::None;
            }
        }

        let host = match cap.get(2) {
            Some(_h) => _h.as_str(),
            None => {
                eprintln!(
                    "Unable to parse socket URI (failed to get host from URI ({})).",
                    string_addr
                );
                return SockAddr::None;
            }
        };
        let port = match cap.get(3) {
            Some(_p) => _p.as_str(),
            None => {
                eprintln!(
                    "Unable to parse socket URI (failed to get port from URI ({})).",
                    string_addr
                );
                return SockAddr::None;
            }
        };

        //getting a socket address
        let mut addrs_iter = match format!("{}:{}", host, port).to_socket_addrs() {
            Ok(_a) => _a,
            Err(e) => {
                eprintln!("Unable to parse socket address ({}).", e);
                return SockAddr::None;
            }
        };

        let sock_addr = match addrs_iter.next() {
            Some(_sa) => _sa,
            _ => {
                eprintln!("Unable to get socket address.");
                return SockAddr::None;
            }
        };

        SockAddr::Tcp(sock_addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session(extras: Option<&str>) -> Session {
        let extras_string = extras.map(|s| s.to_string());
        Session::new("tcp://127.0.0.1:9001", extras_string.as_ref())
    }

    fn test_cp(remote: &str, up: bool, inbound: Option<bool>) -> ConnectedPeer {
        let re = crate::peer::uri_regex_anchored();
        ConnectedPeer {
            host: extract_host_port(remote, &re).map(|(_h, _)| _h),
            remote: remote.to_string(),
            up,
            inbound,
        }
    }

    fn test_peer(uri: &str, addr: &str, port: &str, is_alive: bool) -> Peer {
        Peer::new(
            uri,
            addr,
            port,
            "Region".to_string(),
            "Country".to_string(),
            is_alive,
            10,
        )
    }

    #[test]
    fn test_get_socket_addr_unix() {
        let string_addr = "unix:///run/yggdrasil/yggdrasil.sock";
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            get_socket_addr(string_addr),
            SockAddr::Unix("/run/yggdrasil/yggdrasil.sock".to_string())
        );
        #[cfg(target_os = "windows")]
        assert_eq!(get_socket_addr(string_addr), SockAddr::None);
    }

    #[test]
    fn test_get_socket_addr_ip() {
        assert_eq!(
            get_socket_addr("tcp://127.0.0.1:9002"),
            SockAddr::Tcp("127.0.0.1:9002".to_socket_addrs().unwrap().next().unwrap())
        );
    }

    #[test]
    fn test_get_socket_addr_domain_name() {
        let string_addr = "tcp://localhost:9002";
        let res = get_socket_addr(string_addr)
            == SockAddr::Tcp("127.0.0.1:9002".to_socket_addrs().unwrap().next().unwrap())
            || get_socket_addr(string_addr)
                == SockAddr::Tcp("[::1]:9002".to_socket_addrs().unwrap().next().unwrap());
        assert_eq!(true, res);
    }

    #[test]
    fn test_get_socket_addr_domain_name_port_is_missing() {
        assert_eq!(get_socket_addr("tcp://localhost"), SockAddr::None);
    }

    #[test]
    fn test_get_socket_addr_rejects_peer_only_schemes() {
        assert_eq!(get_socket_addr("ws://localhost:9001"), SockAddr::None);
        assert_eq!(get_socket_addr("wss://localhost:9001"), SockAddr::None);
    }

    #[test]
    fn test_get_socket_addr_default() {
        // The built-in default must always be parseable on the target platform.
        assert_ne!(
            get_socket_addr(crate::defaults::DEF_SOCKET_ADDR),
            SockAddr::None
        );
    }

    #[test]
    fn test_extract_host_port() {
        let re = crate::peer::uri_regex_anchored();
        assert_eq!(
            extract_host_port("tls://Example.COM:443?key=000000", &re),
            Some(("example.com".to_string(), "443".to_string()))
        );
        assert_eq!(
            extract_host_port("tcp://[2001:db8::1]:9001", &re),
            Some(("[2001:db8::1]".to_string(), "9001".to_string()))
        );
        assert_eq!(
            extract_host_port("wss://ygg.example.org:80/ws", &re),
            Some(("ygg.example.org".to_string(), "80".to_string()))
        );
        assert_eq!(
            extract_host_port("tls://[fe80::1%eth0]:5000", &re),
            Some(("[fe80::1%eth0]".to_string(), "5000".to_string()))
        );
        assert_eq!(
            extract_host_port("tcp://my_host.example.com:443", &re),
            Some(("my_host.example.com".to_string(), "443".to_string()))
        );
        // Unknown schemes must not match on an embedded `tls://...`
        assert_eq!(
            extract_host_port("sockstls://proxy.example.com:1080/peer.example.com:443", &re),
            None
        );
        assert_eq!(
            extract_host_port("socks://127.0.0.1:1080/peer.example.com:443", &re),
            None
        );
        assert_eq!(extract_host_port("unix:///run/foo.sock", &re), None);
        assert_eq!(extract_host_port("tcp://no.port.example.com", &re), None);
    }

    #[test]
    fn test_host_is_public() {
        assert!(!host_is_public("[fe80::1234:5678%eth0]"));
        assert!(!host_is_public("[fe80::1]"));
        assert!(!host_is_public("fe80::1"));
        assert!(!host_is_public("169.254.10.20"));
        // Loopback, private and unique-local addresses cannot come from
        // the public peers list either
        assert!(!host_is_public("127.0.0.1"));
        assert!(!host_is_public("[::1]"));
        assert!(!host_is_public("192.168.1.5"));
        assert!(!host_is_public("10.0.0.1"));
        assert!(!host_is_public("172.16.0.1"));
        assert!(!host_is_public("[fd00::1]"));
        assert!(host_is_public("public.example.com"));
        assert!(host_is_public("[2001:db8::1]"));
        assert!(host_is_public("8.8.8.8"));
        // Substrings that used to cause false positives
        assert!(host_is_public("[2001:fe80::1]"));
        assert!(host_is_public("1.169.254.2"));
    }

    #[test]
    fn test_is_extra() {
        let session = test_session(Some("tls://extra.example.com:9443"));
        // The same host matches regardless of case, port and parameters
        assert!(session.is_extra(&test_cp("tls://extra.example.com:9443", true, Some(false))));
        assert!(session.is_extra(&test_cp("tls://EXTRA.example.com:9443?key=00", true, Some(false))));
        assert!(session.is_extra(&test_cp("tls://extra.example.com:9444", true, Some(false))));
        // A different host does not match
        assert!(!session.is_extra(&test_cp("tls://other.example.com:9443", true, Some(false))));

        // Unrecognized URIs are compared literally (case-insensitive)
        let session = test_session(Some("socks://127.0.0.1:1080/peer.example.com:443"));
        assert!(session.is_extra(&test_cp(
            "SOCKS://127.0.0.1:1080/peer.example.com:443",
            true,
            Some(false)
        )));
        assert!(!session.is_extra(&test_cp(
            "socks://127.0.0.1:1080/other.example.com:443",
            true,
            Some(false)
        )));
    }

    #[test]
    fn test_parse_connected() {
        let getpeers_resp = r#"{
            "request": "getpeers",
            "status": "success",
            "response": {
                "peers": [
                    {"remote": "tls://pub1.example.com:443", "up": true, "inbound": false},
                    {"remote": "tcp://pub1.example.com:80", "up": true, "inbound": false},
                    {"remote": "tls://pub2.example.com:443", "up": false, "inbound": false},
                    {"remote": "tls://[fe80::1234%eth0]:12345", "up": true, "inbound": false},
                    {"remote": "tls://198.51.100.7:54321", "up": true, "inbound": true},
                    {"remote": "tls://extra.example.com:9443", "up": true, "inbound": false},
                    {"remote": "quic://pub3.example.com:8443"},
                    {"remote": "quic://pub4.example.com:8443", "up": true, "inbound": false},
                    {"up": true, "inbound": false}
                ]
            }
        }"#;

        let re = crate::peer::uri_regex_anchored();
        let connected = parse_connected(getpeers_resp, &re).unwrap();
        // The entry without "remote" is skipped
        assert_eq!(connected.len(), 8);

        // pub1 has a duplicate connection and pub2 is down, so no n is
        // satisfied by this state
        let session = test_session(Some("tls://extra.example.com:9443"));
        assert!(!session.satisfied(2, &connected));
        assert!(!session.satisfied(3, &connected));
    }

    #[test]
    fn test_satisfied() {
        let session = test_session(Some("tls://extra.example.com:9443"));
        let connected = vec![
            test_cp("tls://a.example.com:443", true, Some(false)),
            test_cp("tls://b.example.com:443", true, Some(false)),
            // Ignored: extra (even when down), inbound, link-local, private,
            // unknown direction
            test_cp("tls://extra.example.com:9443", false, Some(false)),
            test_cp("tls://in.example.com:443", true, Some(true)),
            test_cp("tls://[fe80::1%eth0]:5000", true, Some(false)),
            test_cp("tcp://192.168.1.5:9001", true, Some(false)),
            test_cp("quic://old.example.com:8443", true, None),
        ];
        assert!(session.satisfied(2, &connected));
        assert!(!session.satisfied(1, &connected));
        assert!(!session.satisfied(3, &connected));

        // A duplicate connection to a managed host needs reconciling
        let mut with_dup = connected;
        with_dup.push(test_cp("tcp://a.example.com:80", true, Some(false)));
        assert!(!session.satisfied(2, &with_dup));

        // A down managed peer needs reconciling, also with n = 0
        let down = vec![test_cp("tls://dead.example.com:443", false, Some(false))];
        assert!(!session.satisfied(0, &down));
        assert!(session.satisfied(0, &[]));
    }

    #[test]
    fn test_plan() {
        let session = test_session(None);
        let peers = vec![
            test_peer("tls://a.example.com:443", "a.example.com", "443", true),
            test_peer("tls://b.example.com:443", "b.example.com", "443", true),
        ];
        let connected = vec![
            // Still among the best - kept
            test_cp("tls://a.example.com:443", true, Some(false)),
            // Duplicate connection - removed
            test_cp("tcp://a.example.com:80", true, Some(false)),
            // Healthy but displaced by a desired peer - removed
            test_cp("tls://old.example.com:443", true, Some(false)),
            // Down - removed
            test_cp("tls://dead.example.com:443", false, Some(false)),
            // Inbound - not touched
            test_cp("tls://in.example.com:443", true, Some(true)),
        ];

        let (desired, hosts) = session.select_desired(&peers, 2);
        let (removals, adds) = session.plan(&desired, &hosts, 2, &connected);
        let removed: Vec<&str> = removals.iter().map(|cp| cp.remote.as_str()).collect();
        assert_eq!(
            removed,
            vec![
                "tcp://a.example.com:80",
                "tls://dead.example.com:443",
                "tls://old.example.com:443",
            ]
        );
        let added: Vec<&str> = adds.iter().map(|p| p.uri.as_str()).collect();
        assert_eq!(added, vec!["tls://b.example.com:443"]);
    }

    #[test]
    fn test_plan_probe_shortfall_keeps_healthy() {
        // Only one downloaded peer alive while two healthy peers are
        // connected (n = 3): the healthy ones must not be torn down
        let session = test_session(None);
        let peers = vec![test_peer("tls://x.example.com:443", "x.example.com", "443", true)];
        let connected = vec![
            test_cp("tls://p.example.com:443", true, Some(false)),
            test_cp("tls://q.example.com:443", true, Some(false)),
        ];

        let (desired, hosts) = session.select_desired(&peers, 3);
        let (removals, adds) = session.plan(&desired, &hosts, 3, &connected);
        assert!(removals.is_empty());
        assert_eq!(adds.len(), 1);
    }

    #[test]
    fn test_plan_purge() {
        // n = 0 removes all managed connections but never the extras
        let session = test_session(Some("tls://extra.example.com:9443"));
        let connected = vec![
            test_cp("tls://p.example.com:443", true, Some(false)),
            test_cp("tls://dead.example.com:443", false, Some(false)),
            test_cp("tls://extra.example.com:9443", true, Some(false)),
        ];

        let (removals, adds) = session.plan(&[], &[], 0, &connected);
        assert_eq!(removals.len(), 2);
        assert!(adds.is_empty());
    }

    #[test]
    fn test_parse_connected_error_reply() {
        let re = crate::peer::uri_regex_anchored();
        // An error reply without the "response" key cannot be parsed
        assert!(parse_connected(r#"{"status": "error", "error": "boom"}"#, &re).is_none());
    }

    #[test]
    fn test_select_desired() {
        let session = test_session(Some("tls://extra.example.com:9443"));
        let peers = vec![
            test_peer("tls://a.example.com:443", "a.example.com", "443", true),
            // The same host again (even on another port) - deduplicated
            test_peer("quic://a.example.com:8443", "a.example.com", "8443", true),
            // The extra's host - excluded regardless of the port
            test_peer("tls://extra.example.com:443", "extra.example.com", "443", true),
            test_peer("tls://b.example.com:443", "b.example.com", "443", true),
            test_peer("tls://c.example.com:443", "c.example.com", "443", true),
            // The list is sorted, a dead peer ends the alive prefix
            test_peer("tls://dead.example.com:443", "dead.example.com", "443", false),
            test_peer("tls://d.example.com:443", "d.example.com", "443", true),
        ];

        let (desired, hosts) = session.select_desired(&peers, 3);
        assert_eq!(desired.len(), 3);
        assert_eq!(
            hosts,
            vec![
                "a.example.com".to_string(),
                "b.example.com".to_string(),
                "c.example.com".to_string(),
            ]
        );

        // n = 0: the desired set is empty (purge)
        let (desired, _) = session.select_desired(&peers, 0);
        assert!(desired.is_empty());
    }
}
