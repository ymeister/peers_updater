use std::net::{TcpStream, ToSocketAddrs};
use std::string::String;
use std::time;

// The single source of truth for the peer URI grammar. The host class has
// no uppercase letters (the public peers lists use lowercase URIs) -
// lowercase the input before matching single URIs.
const URI_PATTERN: &str = r"(tcp|tls|quic|ws|wss)://([a-z0-9\.\-:\[\]%_]+):([0-9]+)";

fn build_uri_regex(pattern: &str) -> regex::Regex {
    match regex::Regex::new(pattern) {
        Ok(_r) => _r,
        Err(e) => {
            eprintln!("Failed to create an instance of the RegEx parser ({}).", e);
            std::process::exit(1);
        }
    }
}

// Matches peer URIs anywhere in a string (for scanning text lines)
pub fn uri_regex() -> regex::Regex {
    build_uri_regex(URI_PATTERN)
}

// Matches a peer URI only from the beginning of the string (for parsing
// single URIs, so that e.g. `sockstls://proxy:1080/peer:443` does not match
// on the embedded `tls://proxy:1080`)
pub fn uri_regex_anchored() -> regex::Regex {
    build_uri_regex(format!("^{}", URI_PATTERN).as_str())
}

//#[derive(Debug)]
pub struct Peer {
    pub uri: String,
    pub addr: String,
    pub port: String,
    //proto: String,
    pub region: String,
    pub country: String,
    pub is_alive: bool,
    pub latency: u128,
}

impl Peer {
    pub fn new(
        uri: &str,
        addr: &str,
        port: &str,
        //proto: String,
        region: String,
        country: String,
        is_alive: bool,
        latency: u128,
    ) -> Self {
        Peer {
            uri: String::from(uri),
            addr: String::from(addr),
            port: String::from(port),
            //proto,
            region,
            country,
            is_alive,
            latency,
        }
    }

    pub fn set_latency(&mut self) {
        let mut addrs_iter = match format!("{}:{}", self.addr, self.port).to_socket_addrs() {
            Ok(_a) => _a,
            _ => {
                self.is_alive = false;
                return;
            }
        };
        let addr = match addrs_iter.next() {
            Some(__sa) => __sa,
            _ => {
                self.is_alive = false;
                return;
            }
        };

        let now = time::Instant::now();

        let stream = match TcpStream::connect_timeout(&addr, time::Duration::from_secs(10)) {
            Ok(_s) => _s,
            _ => {
                return;
            }
        };
        self.is_alive = true;
        self.latency = now.elapsed().as_millis();
        drop(stream);
    }
}
