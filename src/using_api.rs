use crate::peer::Peer;
use nu_json::Map;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
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

pub fn update_peers(
    peers: &Vec<Peer>,
    socket_addr_str: &str,
    n_peers: u8,
    always_in_p: Option<&String>,
) -> bool {
    let socket_addr = get_socket_addr(socket_addr_str);

    let mut response = String::new();

    // Get peers
    request("{\"request\": \"getpeers\"}", &socket_addr, &mut response);
    if response.is_empty() {
        eprintln!("Can't get connected peers.");
        return false;
    }

    // Removing old peers
    remove_peers(&mut response, &socket_addr);

    // Adding new peers
    let mut n_added: u8 = 0;
    let mut added_hosts: Vec<String> = Vec::with_capacity(n_peers.into());
    for peer in peers {
        if n_added >= n_peers || !peer.is_alive {
            break;
        }
        if added_hosts.contains(&peer.addr) {
            continue;
        }
        response.clear();
        request(
            format!(
                "{{\"request\": \"addpeer\", \"arguments\": {{\"uri\": \"{}\"}}}}",
                json_escape(&peer.uri)
            )
            .as_str(),
            &socket_addr,
            &mut response,
        );
        n_added += 1;
        added_hosts.push(peer.addr.to_owned());
    }

    //Always in
    if let Some(always_in) = always_in_p {
        let ai = always_in.split(' ');
        for ai_s in ai {
            response.clear();
            request(
                format!(
                    "{{\"request\": \"addpeer\", \"arguments\": {{\"uri\": \"{}\"}}}}",
                    json_escape(ai_s)
                )
                .as_str(),
                &socket_addr,
                &mut response,
            );
        }
    }

    true
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

fn remove_peer(peer_uri: &String, socket_addr: &SockAddr, resp: &mut String) {
    request(
        format!(
            "{{\"request\": \"removepeer\", \"arguments\": {{\"uri\": \"{}\"}}}}",
            peer_uri
        )
        .as_str(),
        socket_addr,
        resp,
    );
}

fn remove_peers(getpeers_resp: &mut String, socket_addr: &SockAddr) {
    //parse to obj
    //Serde deserialization is not used in order to get smaller binary files
    //(there is no need to describe the structures of the json objects used in our case).

    let connected_peers: Map<String, nu_json::Value> = match nu_json::from_str(getpeers_resp) {
        Ok(cp) => cp,
        Err(e) => {
            eprintln!("Error converting a json string to an object ({}).", e);
            return;
        }
    };

    let resp = match connected_peers.get("response") {
        Some(_a) => _a,
        _ => {
            eprintln!("Couldn't get response from the getpeers result.");
            return;
        }
    };

    let peers_val = match resp.as_object() {
        Some(pv) => match pv.get("peers") {
            Some(_a) => _a,
            _ => {
                eprintln!("Couldn't get peers from the response obj.");
                return;
            }
        },
        _ => {
            eprintln!("Couldn't get peers from the response obj (0002).");
            return;
        }
    };

    let mp_array = match peers_val.as_array() {
        Some(_mv) => _mv,
        _ => {
            eprintln!("Couldn't get peers array from the the response obj.");
            return;
        }
    };

    for peer in mp_array {
        let peer_obj = match peer.as_object() {
            Some(_po) => _po,
            _ => {
                //eprintln!("Couldn't get peer obj.");
                continue;
            }
        };

        let peer_uri = match peer_obj.get("remote") {
            Some(_pu) => _pu.to_string().replace('"', ""),
            _ => {
                //eprintln!("Couldn't get peer uri.");
                continue;
            }
        };

        getpeers_resp.clear();
        remove_peer(&peer_uri, socket_addr, getpeers_resp);
    }
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
        let re = match regex::Regex::new(r"(tcp|tls|quic)://([a-zA-Z0-9\.\-:\[\]]+):([0-9]+)") {
            Ok(_r) => _r,
            Err(e) => {
                eprintln!("Failed to create an instance of the RegEx parser ({}).", e);
                std::process::exit(1);
            }
        };
        let mut cap_iter = re.captures_iter(string_addr);
        let cap = match cap_iter.next() {
            Some(_c) => _c,
            None => {
                eprintln!("Unable to parse socket URI ({}).", string_addr);
                return SockAddr::None;
            }
        };

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
    fn test_get_socket_addr_default() {
        // The built-in default must always be parseable on the target platform.
        assert_ne!(
            get_socket_addr(crate::defaults::DEF_SOCKET_ADDR),
            SockAddr::None
        );
    }
}
