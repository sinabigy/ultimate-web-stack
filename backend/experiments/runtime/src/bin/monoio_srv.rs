//! Monoio thread-per-core with SO_REUSEPORT listeners. DRIVER=uring (io_uring) or
//! DRIVER=epoll (legacy driver): same runtime, different I/O interface.
use monoio::{
    io::{AsyncReadRent, AsyncWriteRentExt},
    net::{ListenerOpts, TcpListener},
};
use runtime_experiment::*;

async fn serve() {
    let opts = ListenerOpts::new().reuse_port(true).reuse_addr(true).backlog(4096);
    let listener = TcpListener::bind_with_config(addr(), &opts).unwrap_or_else(|e| panic!("bind: {e}"));
    loop {
        let Ok((mut s, _)) = listener.accept().await else { continue };
        let _ = s.set_nodelay(true);
        monoio::spawn(async move {
            let mut pending: Vec<u8> = Vec::with_capacity(8192);
            let mut buf = vec![0u8; 8192];
            loop {
                let (res, b) = s.read(buf).await;
                buf = b;
                let n = match res {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                pending.extend_from_slice(&buf[..n]);
                let (reqs, used) = complete_requests(&pending);
                pending.drain(..used);
                if reqs > 0 {
                    let mut out = Vec::with_capacity(RESPONSE.len() * reqs);
                    for _ in 0..reqs {
                        out.extend_from_slice(RESPONSE);
                    }
                    let (res, _) = s.write_all(out).await;
                    if res.is_err() {
                        return;
                    }
                }
                buf.resize(8192, 0);
            }
        });
    }
}

fn main() {
    let uring = std::env::var("DRIVER").map(|d| d != "epoll").unwrap_or(true);
    let handles: Vec<_> = (0..threads())
        .map(|_| {
            std::thread::spawn(move || {
                if uring {
                    monoio::RuntimeBuilder::<monoio::IoUringDriver>::new()
                        .with_entries(1024)
                        .build()
                        .unwrap_or_else(|e| panic!("io_uring runtime (seccomp?): {e}"))
                        .block_on(serve());
                } else {
                    monoio::RuntimeBuilder::<monoio::LegacyDriver>::new()
                        .build()
                        .unwrap_or_else(|e| panic!("legacy runtime: {e}"))
                        .block_on(serve());
                }
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
}
