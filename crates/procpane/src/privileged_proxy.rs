use anyhow::{Context, Result};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};

use crate::proxy::PROXY_PORT;

const LISTEN_ADDR: &str = "127.0.0.1:443";
const TARGET_ADDR: &str = "127.0.0.1";

/// Root-owned loopback TCP forwarder used by `trust install --pretty-urls`.
///
/// The regular procpane daemon remains unprivileged and owns the real TLS/SNI
/// proxy on :8443. This helper only owns the privileged :443 bind and forwards
/// bytes to :8443, so `lsof -i :443` shows an inspectable procpane process
/// instead of a hidden pf rdr rule.
pub fn run() -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_inner())
}

async fn run_inner() -> Result<()> {
    let listener = TcpListener::bind(LISTEN_ADDR)
        .await
        .with_context(|| format!("bind {LISTEN_ADDR}"))?;
    eprintln!("procpane pretty-url proxy listening on https://{LISTEN_ADDR}");

    loop {
        let (stream, peer) = listener.accept().await.context("accept pretty-url proxy")?;
        tokio::spawn(async move {
            if let Err(error) = forward(stream).await {
                tracing::debug!(?peer, ?error, "pretty-url proxy connection ended");
            }
        });
    }
}

async fn forward(mut inbound: TcpStream) -> Result<()> {
    let mut outbound = TcpStream::connect((TARGET_ADDR, PROXY_PORT))
        .await
        .with_context(|| format!("connect {TARGET_ADDR}:{PROXY_PORT}"))?;
    let _ = copy_bidirectional(&mut inbound, &mut outbound).await?;
    Ok(())
}
