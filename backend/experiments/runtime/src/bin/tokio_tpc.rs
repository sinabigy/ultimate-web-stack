//! Tokio thread-per-core: one current-thread runtime per thread, SO_REUSEPORT listeners, epoll.
//! Isolates the architecture (no work stealing) from the I/O interface.
use runtime_experiment::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn main() {
    let handles: Vec<_> = (0..threads())
        .map(|_| {
            std::thread::spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap_or_else(|e| panic!("{e}"));
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async {
                    let listener = tokio::net::TcpListener::from_std(reuseport_listener(addr()).unwrap_or_else(|e| panic!("{e}")))
                        .unwrap_or_else(|e| panic!("{e}"));
                    loop {
                        let Ok((mut s, _)) = listener.accept().await else { continue };
                        let _ = s.set_nodelay(true);
                        tokio::task::spawn_local(async move {
                            let mut buf = vec![0u8; 8192];
                            let mut filled = 0;
                            let mut out = Vec::with_capacity(4096);
                            loop {
                                let n = match s.read(&mut buf[filled..]).await {
                                    Ok(0) | Err(_) => return,
                                    Ok(n) => n,
                                };
                                filled += n;
                                let (reqs, used) = complete_requests(&buf[..filled]);
                                buf.copy_within(used..filled, 0);
                                filled -= used;
                                out.clear();
                                for _ in 0..reqs {
                                    out.extend_from_slice(RESPONSE);
                                }
                                if !out.is_empty() && s.write_all(&out).await.is_err() {
                                    return;
                                }
                            }
                        });
                    }
                });
            })
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
}
