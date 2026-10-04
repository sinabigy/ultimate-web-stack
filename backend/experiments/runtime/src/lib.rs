//! Shared minimal HTTP/1.1 responder so every runtime variant does identical work: count
//! complete requests (pipelining-safe) in a read buffer and answer each with a fixed response.

pub const RESPONSE: &[u8] =
    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 13\r\n\r\nHello, World!";

/// Number of complete requests at the start of `buf` (GET without body) and bytes consumed.
pub fn complete_requests(buf: &[u8]) -> (usize, usize) {
    let (mut n, mut consumed) = (0, 0);
    while let Some(pos) = find(&buf[consumed..], b"\r\n\r\n") {
        consumed += pos + 4;
        n += 1;
    }
    (n, consumed)
}

fn find(h: &[u8], needle: &[u8]) -> Option<usize> {
    h.windows(needle.len()).position(|w| w == needle)
}

pub fn threads() -> usize {
    std::env::var("THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(1)
}

pub fn addr() -> std::net::SocketAddr {
    std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:9000".into()).parse().unwrap_or_else(|e| panic!("ADDR: {e}"))
}

/// A listening socket with SO_REUSEPORT so one listener per thread shares the port
/// (thread-per-core accept balancing by the kernel).
pub fn reuseport_listener(addr: std::net::SocketAddr) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Socket, Type};
    let s = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
    s.set_reuse_address(true)?;
    s.set_reuse_port(true)?;
    s.set_nonblocking(true)?;
    s.set_tcp_nodelay(true)?;
    s.bind(&addr.into())?;
    s.listen(4096)?;
    Ok(s.into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_pipelined_requests_and_keeps_partial_tail() {
        let b = b"GET / HTTP/1.1\r\nHost: x\r\n\r\nGET / HTTP/1.1\r\n\r\nGET /par";
        let (n, used) = super::complete_requests(b);
        assert_eq!(n, 2);
        assert_eq!(&b[used..], b"GET /par");
    }
}
