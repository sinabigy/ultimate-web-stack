//! Tokio multi-thread (work-stealing) runtime, epoll, one shared listener.
use runtime_experiment::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn main() -> std::io::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(threads()).enable_all().build()?;
    rt.block_on(async {
        let listener = tokio::net::TcpListener::bind(addr()).await?;
        loop {
            let (mut s, _) = listener.accept().await?;
            let _ = s.set_nodelay(true);
            tokio::spawn(async move {
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
    })
}
