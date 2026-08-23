use std::io::BufReader;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::buffer::SharedBuffer;
use crate::process::{Proc, ProcState};
use crate::proxy::PROXY_PORT;
use crate::sidecar::Healthcheck;

#[derive(Debug, Clone)]
pub enum HealthcheckKind {
    Tcp(u16),
    /// HTTP probe with a fully-resolved target.
    ///
    /// * `tls` — when true, the connection runs through rustls with the
    ///   procpane CA trusted, and `sni_host` is sent as the SNI / `Host:` value.
    /// * `connect_host` / `connect_port` — TCP endpoint to dial.
    /// * `path` — request path (always starts with `/`).
    Http {
        tls: bool,
        sni_host: String,
        connect_host: String,
        connect_port: u16,
        path: String,
    },
    Log(regex::Regex),
    /// Wait for process to exit, treat `expected` exit code as success.
    Exit(i32),
    /// No healthcheck — task is healthy as soon as it's running.
    None,
}

impl HealthcheckKind {
    /// Pick which kind to run from the sidecar. `hostname` (when set) maps the
    /// HTTP healthcheck onto `http://<hostname>:<port>`. Without it, HTTP falls
    /// back to `127.0.0.1`. (TLS-aware variant comes with the reverse proxy.)
    pub fn from_sidecar(hc: &Healthcheck, hostname: Option<&str>) -> anyhow::Result<Self> {
        let count = [
            hc.tcp.is_some(),
            hc.http.is_some(),
            hc.log.is_some(),
            hc.exit.is_some(),
        ]
        .iter()
        .filter(|x| **x)
        .count();
        if count == 0 {
            return Ok(HealthcheckKind::None);
        }
        if count > 1 {
            anyhow::bail!("only one of tcp/http/log/exit may be set per healthcheck");
        }
        if let Some(port) = hc.tcp {
            return Ok(HealthcheckKind::Tcp(port));
        }
        if let Some(spec) = &hc.http {
            return Ok(resolve_http_target(spec, hostname)?);
        }
        if let Some(pat) = &hc.log {
            let re = regex::Regex::new(pat)
                .map_err(|e| anyhow::anyhow!("invalid healthcheck.log regex: {e}"))?;
            return Ok(HealthcheckKind::Log(re));
        }
        if let Some(code) = hc.exit {
            return Ok(HealthcheckKind::Exit(code));
        }
        Ok(HealthcheckKind::None)
    }
}

/// Run a healthcheck loop until the process becomes healthy, terminates, or the
/// stop signal fires. Flips proc state to Healthy when satisfied. Returns when
/// the proc reaches a terminal state OR becomes healthy.
pub async fn run_healthcheck_loop(
    proc: Arc<Proc>,
    buffer: SharedBuffer,
    kind: HealthcheckKind,
    interval: Duration,
    probe_timeout: Duration,
    start_period: Duration,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
) {
    if start_period > Duration::ZERO {
        tokio::select! {
            _ = tokio::time::sleep(start_period) => {},
            _ = stop_rx.changed() => { if *stop_rx.borrow() { return; } }
        }
    }

    // `None` and `Exit` are special: there's no real probe.
    if let HealthcheckKind::None = kind {
        // Flip to Healthy immediately if still alive.
        let mut st = proc.state.lock();
        if matches!(*st, ProcState::Starting) {
            *st = ProcState::Healthy;
        }
        return;
    }

    // For `Exit` kind, we just wait for the process to terminate; the spawn
    // thread already sets Completed/Crashed. Translate Completed → success
    // when exit matches the expected code; otherwise mark crashed.
    if let HealthcheckKind::Exit(expected) = kind {
        loop {
            if *stop_rx.borrow() {
                return;
            }
            let st = *proc.state.lock();
            if st.is_terminal() {
                let code = *proc.exit_code.lock();
                if matches!(st, ProcState::Completed) && code == Some(expected) {
                    // Already Completed and matches expected; nothing to fix.
                    return;
                }
                if code == Some(expected) {
                    *proc.state.lock() = ProcState::Completed;
                }
                return;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {}
                _ = stop_rx.changed() => { if *stop_rx.borrow() { return; } }
            }
        }
    }

    // Real-probe loop (Tcp / Http / Log).
    let mut last_log_seq: u64 = 0;
    loop {
        if *stop_rx.borrow() {
            return;
        }
        let st = *proc.state.lock();
        if st.is_terminal() {
            return;
        }
        if matches!(st, ProcState::Healthy) {
            return;
        }

        let ok = match &kind {
            HealthcheckKind::Tcp(port) => probe_tcp(*port, probe_timeout).await,
            HealthcheckKind::Http {
                tls,
                sni_host,
                connect_host,
                connect_port,
                path,
            } => {
                probe_http(
                    *tls,
                    sni_host,
                    connect_host,
                    *connect_port,
                    path,
                    probe_timeout,
                )
                .await
            }
            HealthcheckKind::Log(re) => {
                let (hit, last) = probe_log(&buffer, re, last_log_seq);
                last_log_seq = last;
                hit
            }
            _ => false,
        };

        if ok {
            let mut st = proc.state.lock();
            if matches!(*st, ProcState::Starting) {
                *st = ProcState::Healthy;
            }
            return;
        }

        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = stop_rx.changed() => { if *stop_rx.borrow() { return; } }
        }
    }
}

async fn probe_tcp(port: u16, t: Duration) -> bool {
    let addr = format!("127.0.0.1:{port}");
    matches!(timeout(t, TcpStream::connect(&addr)).await, Ok(Ok(_)))
}

/// Turn a sidecar `healthcheck.http` spec into a concrete probe target.
///
/// Accepted forms:
/// * A path (`"/health"`) — requires the task to declare a `hostname`. The
///   probe goes through procpane's TLS proxy at `127.0.0.1:PROXY_PORT` with
///   SNI = the task's hostname. Matches what the README promises.
/// * A full URL (`"http://127.0.0.1:8787/health"` or
///   `"https://anything.localhost/health"`) — used as-is. Plain `http://` keeps it
///   straightforward to probe a task that pins its own port without a hostname.
pub fn resolve_http_target(spec: &str, hostname: Option<&str>) -> anyhow::Result<HealthcheckKind> {
    if let Some(rest) = spec.strip_prefix("https://") {
        let (host_port, path) = split_host_path(rest);
        let (h, p) = parse_host_port(host_port, 443)?;
        return Ok(HealthcheckKind::Http {
            tls: true,
            sni_host: h.clone(),
            connect_host: h,
            connect_port: p,
            path,
        });
    }
    if let Some(rest) = spec.strip_prefix("http://") {
        let (host_port, path) = split_host_path(rest);
        let (h, p) = parse_host_port(host_port, 80)?;
        return Ok(HealthcheckKind::Http {
            tls: false,
            sni_host: h.clone(),
            connect_host: h,
            connect_port: p,
            path,
        });
    }
    // Path form — must have a hostname so we know where to route through the proxy.
    let host = hostname.ok_or_else(|| {
        anyhow::anyhow!(
            "healthcheck.http = \"{spec}\" needs either a `hostname` on the task \
             or a full URL (e.g. \"http://127.0.0.1:8787/health\")"
        )
    })?;
    let path = if spec.starts_with('/') {
        spec.to_string()
    } else {
        format!("/{spec}")
    };
    Ok(HealthcheckKind::Http {
        tls: true,
        sni_host: host.to_string(),
        connect_host: "127.0.0.1".to_string(),
        connect_port: PROXY_PORT,
        path,
    })
}

fn split_host_path(s: &str) -> (&str, String) {
    match s.find('/') {
        Some(i) => (&s[..i], s[i..].to_string()),
        None => (s, "/".to_string()),
    }
}

fn parse_host_port(s: &str, default_port: u16) -> anyhow::Result<(String, u16)> {
    match s.rsplit_once(':') {
        Some((h, p)) => {
            let port: u16 = p
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid port in healthcheck.http: {p}"))?;
            Ok((h.to_string(), port))
        }
        None => Ok((s.to_string(), default_port)),
    }
}

async fn probe_http(
    tls: bool,
    sni_host: &str,
    connect_host: &str,
    connect_port: u16,
    path: &str,
    t: Duration,
) -> bool {
    let target = format!("{connect_host}:{connect_port}");
    let path_norm = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let req = format!(
        "GET {path_norm} HTTP/1.1\r\nHost: {sni_host}\r\nConnection: close\r\nUser-Agent: procpane-healthcheck/1\r\n\r\n"
    );

    let res = timeout(t, async {
        let tcp = TcpStream::connect(&target).await.ok()?;
        if tls {
            let connector = build_tls_connector().ok()?;
            let dns: rustls::pki_types::ServerName<'static> =
                rustls::pki_types::ServerName::try_from(sni_host.to_string()).ok()?;
            let mut stream = connector.connect(dns, tcp).await.ok()?;
            stream.write_all(req.as_bytes()).await.ok()?;
            let mut buf = [0u8; 64];
            let n = stream.read(&mut buf).await.ok()?;
            Some(buf[..n].to_vec())
        } else {
            let mut stream = tcp;
            stream.write_all(req.as_bytes()).await.ok()?;
            let mut buf = [0u8; 64];
            let n = stream.read(&mut buf).await.ok()?;
            Some(buf[..n].to_vec())
        }
    })
    .await;

    match res {
        Ok(Some(bytes)) => {
            let head = String::from_utf8_lossy(&bytes);
            head.starts_with("HTTP/1.1 2") || head.starts_with("HTTP/1.0 2")
        }
        _ => false,
    }
}

/// Build a rustls TLS connector that trusts the procpane root CA so we can
/// probe `https://<task>.<project>.localhost:8443/...` without `-k`-style hacks. The CA is
/// loaded fresh on each healthcheck attempt — cheap, and lets the user run
/// `procpane trust install` mid-session without restarting the daemon.
fn build_tls_connector() -> anyhow::Result<tokio_rustls::TlsConnector> {
    let mut roots = rustls::RootCertStore::empty();
    let pem = std::fs::read(crate::ca::ca_cert_path()?)?;
    let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
        rustls_pemfile::certs(&mut BufReader::new(&pem[..]))
            .filter_map(|c| c.ok())
            .collect();
    for c in certs {
        roots.add(c).ok();
    }
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(tokio_rustls::TlsConnector::from(Arc::new(cfg)))
}

/// Scan buffer lines >= last_seq for a regex hit. Returns (hit?, new_cursor).
fn probe_log(buffer: &SharedBuffer, re: &regex::Regex, last_seq: u64) -> (bool, u64) {
    let buf = buffer.lock();
    let (lines, next) = buf.since(last_seq);
    for l in &lines {
        if re.is_match(&l.text) {
            return (true, next);
        }
    }
    (false, next)
}
