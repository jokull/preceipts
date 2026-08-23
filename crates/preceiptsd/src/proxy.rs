//! TLS reverse proxy. One listener on :8443 for the whole daemon. Each accepted
//! connection sniffs SNI, looks up the registered backend port for that
//! hostname, and bidirectionally copies bytes.
//!
//! Cert chain is a single leaf signed by the local CA naming every declared
//! hostname exactly — no wildcard, because a wildcard SAN matches one label
//! and every workspace hostname has one more than the project domain. One
//! cert is signed at daemon startup covering every task's hostname.

use anyhow::{anyhow, Context, Result};
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::io::BufReader;
use std::net::SocketAddr;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

use crate::ca;
use crate::transcript::{self, Transcript};
use parking_lot::Mutex;
use std::time::Instant;

/// Default HTTPS port for the reverse proxy. Unprivileged.
pub const PROXY_PORT: u16 = 8443;

/// Shared, mutable registry of hostname → backend port.
/// Procs update it as they become healthy.
#[derive(Default)]
pub struct PortRegistry {
    inner: RwLock<BTreeMap<String, u16>>,
}

impl PortRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn register(&self, host: &str, port: u16) {
        self.inner.write().insert(host.to_string(), port);
    }
    pub fn lookup(&self, host: &str) -> Option<u16> {
        self.inner.read().get(host).copied()
    }
}

/// Choose the crypto backend explicitly, once per process.
///
/// rustls 0.23 auto-selects a provider only when exactly one is compiled in.
/// Both `aws-lc-rs` and `ring` end up in this binary — the second arrives
/// through `magic-wormhole` — so auto-selection gives up and *panics* the
/// first time any TLS config is built. That made the whole HTTPS path a
/// crash for anyone who had actually installed the CA, which is to say for
/// anyone using the feature.
///
/// An already-installed provider is not an error: this is called from both
/// the proxy and the healthcheck connector, and whichever runs first wins.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Build a rustls ServerConfig that presents a single leaf cert covering all
/// of `hostnames`. The cert is signed by the local CA on the fly.
pub fn build_tls_config(hostnames: &[String]) -> Result<Arc<ServerConfig>> {
    install_crypto_provider();
    if !ca::is_installed() {
        return Err(anyhow!(
            "local CA not generated; run `preceipts trust install` first"
        ));
    }
    let (cert_pem, key_pem) = ca::sign_leaf(hostnames).context("sign leaf for proxy")?;

    let certs: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut BufReader::new(cert_pem.as_bytes()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("parse leaf cert PEM: {e}"))?;
    let key: PrivateKeyDer<'static> =
        rustls_pemfile::private_key(&mut BufReader::new(key_pem.as_bytes()))
            .map_err(|e| anyhow!("parse leaf key PEM: {e}"))?
            .ok_or_else(|| anyhow!("no private key found in leaf PEM"))?;

    let mut cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| anyhow!("rustls config: {e}"))?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec(), b"h2".to_vec()];
    Ok(Arc::new(cfg))
}

/// Run the proxy listener forever (or until `stop_rx` fires).
pub async fn run_proxy(
    tls_cfg: Arc<ServerConfig>,
    registry: Arc<PortRegistry>,
    transcript: Option<Arc<Transcript>>,
    bind: SocketAddr,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    let acceptor = TlsAcceptor::from(tls_cfg);
    eprintln!("preceipts reverse proxy listening on https://{bind}");
    loop {
        tokio::select! {
            changed = stop_rx.changed() => {
                match changed {
                    Ok(()) if *stop_rx.borrow() => return Ok(()),
                    Ok(()) => continue,
                    Err(_) => return Ok(()),
                }
            }
            accept = listener.accept() => {
                let (stream, _peer) = match accept {
                    Ok(p) => p,
                    Err(e) => { tracing::warn!(?e, "proxy accept failed"); continue; }
                };
                tracing::debug!(peer = %_peer, "proxy accepted connection");
                let acceptor = acceptor.clone();
                let registry = Arc::clone(&registry);
                let transcript = transcript.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_conn(acceptor, registry, transcript, stream).await {
                        tracing::debug!(?e, "proxy conn ended");
                    }
                });
            }
        }
    }
}

async fn handle_conn(
    acceptor: TlsAcceptor,
    registry: Arc<PortRegistry>,
    transcript: Option<Arc<Transcript>>,
    stream: TcpStream,
) -> Result<()> {
    let tls_stream = acceptor.accept(stream).await.context("tls handshake")?;
    let sni = tls_stream
        .get_ref()
        .1
        .server_name()
        .map(|s| s.to_string())
        .unwrap_or_default();
    if sni.is_empty() {
        return Err(anyhow!("client did not send SNI"));
    }
    let port = match registry.lookup(&sni) {
        Some(p) => p,
        None => {
            // 503-style canned response so curl/browser see something useful.
            let mut s = tls_stream;
            let body = format!("preceipts: unknown host {sni}\n");
            let resp = format!(
                "HTTP/1.1 503 Service Unavailable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes()).await;
            return Ok(());
        }
    };
    let backend = TcpStream::connect(("127.0.0.1", port))
        .await
        .with_context(|| format!("connect backend 127.0.0.1:{port}"))?;
    let (mut tls_r, mut tls_w) = tokio::io::split(tls_stream);
    let (mut bk_r, mut bk_w) = backend.into_split();

    // The two directions share the id of the exchange in flight. A keep-alive
    // connection carries many, one after another, and the response side needs
    // to know which request it is answering — which on HTTP/1.1 is simply
    // "the most recent one", because the protocol forbids overlap.
    let in_flight = Arc::new(Mutex::new(None::<(u64, Instant)>));
    let a = copy_recording(
        &mut tls_r,
        &mut bk_w,
        Direction::Request {
            host: sni.clone(),
            transcript: transcript.clone(),
            in_flight: Arc::clone(&in_flight),
        },
    );
    let b = copy_recording(
        &mut bk_r,
        &mut tls_w,
        Direction::Response {
            transcript,
            in_flight,
        },
    );
    let _ = tokio::join!(a, b);
    Ok(())
}

/// Which half of an exchange a copy loop is carrying.
enum Direction {
    Request {
        host: String,
        transcript: Option<Arc<Transcript>>,
        in_flight: Arc<Mutex<Option<(u64, Instant)>>>,
    },
    Response {
        transcript: Option<Arc<Transcript>>,
        in_flight: Arc<Mutex<Option<(u64, Instant)>>>,
    },
}

/// Copy bytes, recording message heads as they stream past.
///
/// The copy itself must stay exactly what it was before the transcript
/// existed: every byte through untouched, in the same order, flushed per
/// chunk so a streaming response is not held back by us. The
/// recording is strictly a side effect of watching, never a step the data
/// waits on — a transcript that could corrupt a response would be a far worse
/// bug than one that misses a line.
async fn copy_recording<R, W>(
    reader: &mut R,
    writer: &mut W,
    direction: Direction,
) -> std::io::Result<u64>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut buf = [0u8; 16 * 1024];
    let mut copied = 0u64;
    // Bytes of the current message we have not yet been able to parse. Reset
    // after each head, so a long-lived connection does not accumulate.
    let mut pending: Vec<u8> = Vec::new();
    let mut watching = true;
    let mut message_bytes = 0u64;

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            writer.shutdown().await?;
            return Ok(copied);
        }
        writer.write_all(&buf[..n]).await?;
        writer.flush().await?;
        copied += n as u64;
        message_bytes += n as u64;

        if !watching {
            continue;
        }
        pending.extend_from_slice(&buf[..n]);
        let Some(head) = transcript::parse_head(&pending) else {
            // Either not enough bytes yet, or not HTTP. Stop trying once the
            // buffer is large enough that a head would have appeared.
            if pending.len() > 32 * 1024 {
                watching = false;
                pending = Vec::new();
            }
            continue;
        };

        match &direction {
            Direction::Request {
                host,
                transcript,
                in_flight,
            } => {
                let mut parts = head.start_line.split(' ');
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                if let Some(transcript) = transcript {
                    let id = transcript.begin(host, &method, &path, message_bytes);
                    *in_flight.lock() = Some((id, Instant::now()));
                }
            }
            Direction::Response {
                transcript,
                in_flight,
            } => {
                let status = head
                    .start_line
                    .split(' ')
                    .nth(1)
                    .and_then(|code| code.parse::<u16>().ok());
                if let (Some(transcript), Some(status)) = (transcript, status) {
                    if let Some((id, started)) = *in_flight.lock() {
                        transcript.complete(
                            id,
                            status,
                            message_bytes,
                            head.header("content-type").map(str::to_string),
                            started.elapsed(),
                        );
                    }
                }
            }
        }

        // Ready for the next message on this connection.
        pending = Vec::new();
        message_bytes = 0;
    }
}

/// Allocate a free TCP port by binding to :0, reading the assigned port, and
/// dropping. Race-prone in principle; in practice fine for dev.
pub fn allocate_port() -> Result<u16> {
    use std::net::TcpListener as StdListener;
    let l = StdListener::bind("127.0.0.1:0").context("bind :0 for port allocation")?;
    Ok(l.local_addr()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context as TaskContext, Poll};
    use tokio::sync::mpsc;
    use tokio::time::{timeout, Duration};

    struct FlushObservingWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
        flush_tx: mpsc::UnboundedSender<()>,
    }

    impl AsyncWrite for FlushObservingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            let _ = self.flush_tx.send(());
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _cx: &mut TaskContext<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    /// A streaming response — server-sent events, a chunked build log — is
    /// only streaming if each chunk leaves as it arrives. Buffering until EOF
    /// would turn a live log into a file that appears when the process ends,
    /// and the transcript sits in this path now, so the property is worth
    /// pinning against the loop that actually runs.
    #[tokio::test]
    async fn the_proxy_flushes_each_chunk_rather_than_waiting_for_eof() {
        let (mut source_w, mut source_r) = tokio::io::duplex(64);
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (flush_tx, mut flush_rx) = mpsc::unbounded_channel();
        let mut writer = FlushObservingWriter {
            bytes: Arc::clone(&bytes),
            flush_tx,
        };

        let copy_task = tokio::spawn(async move {
            let _ = copy_recording(
                &mut source_r,
                &mut writer,
                Direction::Response {
                    transcript: None,
                    in_flight: Arc::new(parking_lot::Mutex::new(None)),
                },
            )
            .await;
        });

        source_w.write_all(b"HTTP/1.1 200 OK\r\n").await.unwrap();

        timeout(Duration::from_secs(1), flush_rx.recv())
            .await
            .expect("copy should flush after the first chunk");
        assert_eq!(bytes.lock().unwrap().as_slice(), b"HTTP/1.1 200 OK\r\n");

        copy_task.abort();
    }
}
