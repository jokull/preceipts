//! One listener on the well-known port, splicing by SNI to the workspace that
//! owns each hostname.
//!
//! **It holds no keys and terminates no TLS.** The server name in a
//! ClientHello is sent in the clear, before any encryption is negotiated, so
//! routing needs to read a few dozen bytes and then get out of the way. Every
//! workspace daemon keeps its own certificate, its own proxy, and its own
//! HTTP transcript; the router never sees a plaintext byte and could not
//! record one if it wanted to. That is the property that makes a shared,
//! machine-wide component acceptable at all.
//!
//! What it buys: portless URLs for *every* workspace instead of whichever one
//! happened to hold the well-known port. The `:443` forwarder points here, and
//! here knows where everything is.

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::routes;

/// Largest ClientHello we will buffer while looking for the server name.
///
/// A real one is a few hundred bytes; the record layer caps a handshake
/// message at 16KB. Bounded because otherwise a peer that never finishes its
/// hello decides how much memory we spend.
const MAX_HELLO: usize = 16 * 1024;

pub fn run() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(serve())
}

async fn serve() -> Result<()> {
    let bind = format!("127.0.0.1:{}", routes::ROUTER_PORT);
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    eprintln!("preceipts router listening on {bind}");

    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(?e, "router accept failed");
                continue;
            }
        };
        tokio::spawn(async move {
            if let Err(e) = route(stream).await {
                tracing::debug!(?e, "router connection ended");
            }
        });
    }
}

async fn route(mut inbound: TcpStream) -> Result<()> {
    // Read until the server name is readable, keeping every byte: the backend
    // needs the whole ClientHello, exactly as it arrived, or the handshake it
    // is about to do will fail.
    let mut hello = Vec::new();
    let mut chunk = [0u8; 4096];
    let host = loop {
        let n = inbound.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        hello.extend_from_slice(&chunk[..n]);
        if let Some(name) = server_name(&hello) {
            break name;
        }
        if hello.len() > MAX_HELLO {
            return Ok(());
        }
    };

    let table = routes::load();
    let Some(port) = table.get(&host).copied() else {
        // No TLS here to speak over, so the only honest thing is to close.
        // A browser reports a refused connection, which is what "nothing
        // serves that name" actually looks like.
        tracing::debug!(%host, "no route");
        return Ok(());
    };

    let mut backend = TcpStream::connect(("127.0.0.1", port))
        .await
        .with_context(|| format!("connect workspace proxy 127.0.0.1:{port}"))?;
    backend.write_all(&hello).await?;
    backend.flush().await?;
    tokio::io::copy_bidirectional(&mut inbound, &mut backend).await?;
    Ok(())
}

/// The SNI hostname in a TLS ClientHello, if the bytes are there yet.
///
/// Hand-rolled rather than pulled from a TLS crate: every crate that parses
/// this also wants to own the connection afterwards, and the whole point here
/// is to read a name and then never look again. `None` means "not enough
/// bytes, or not a ClientHello" — the caller treats both the same way.
pub fn server_name(bytes: &[u8]) -> Option<String> {
    // TLS record: type(1) version(2) length(2), type 22 = handshake.
    if bytes.len() < 5 || bytes[0] != 0x16 {
        return None;
    }
    let record_len = u16::from_be_bytes([bytes[3], bytes[4]]) as usize;
    let body = bytes.get(5..5 + record_len)?;

    // Handshake: type(1) length(3), type 1 = ClientHello.
    if body.first()? != &0x01 {
        return None;
    }
    let mut at = 4;
    // client_version(2) + random(32)
    at += 34;
    // legacy_session_id
    let session_len = *body.get(at)? as usize;
    at += 1 + session_len;
    // cipher_suites
    let suites_len = u16::from_be_bytes([*body.get(at)?, *body.get(at + 1)?]) as usize;
    at += 2 + suites_len;
    // compression_methods
    let compression_len = *body.get(at)? as usize;
    at += 1 + compression_len;
    // extensions
    let extensions_len = u16::from_be_bytes([*body.get(at)?, *body.get(at + 1)?]) as usize;
    at += 2;
    let end = at + extensions_len;

    while at + 4 <= end.min(body.len()) {
        let kind = u16::from_be_bytes([*body.get(at)?, *body.get(at + 1)?]);
        let len = u16::from_be_bytes([*body.get(at + 2)?, *body.get(at + 3)?]) as usize;
        let data = body.get(at + 4..at + 4 + len)?;
        at += 4 + len;
        // server_name extension
        if kind != 0x0000 {
            continue;
        }
        // server_name_list length(2), then entries of type(1) length(2) name.
        let mut inner = 2;
        while inner + 3 <= data.len() {
            let name_type = data[inner];
            let name_len = u16::from_be_bytes([data[inner + 1], data[inner + 2]]) as usize;
            let name = data.get(inner + 3..inner + 3 + name_len)?;
            if name_type == 0 {
                return String::from_utf8(name.to_vec()).ok();
            }
            inner += 3 + name_len;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal but structurally real ClientHello carrying `host`.
    fn client_hello(host: &str) -> Vec<u8> {
        let mut extension = Vec::new();
        extension.extend_from_slice(&((host.len() + 3) as u16).to_be_bytes()); // list len
        extension.push(0); // name type: host_name
        extension.extend_from_slice(&(host.len() as u16).to_be_bytes());
        extension.extend_from_slice(host.as_bytes());

        let mut extensions = Vec::new();
        extensions.extend_from_slice(&0x0000u16.to_be_bytes()); // server_name
        extensions.extend_from_slice(&(extension.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&extension);

        let mut body = Vec::new();
        body.push(0x01); // ClientHello
        body.extend_from_slice(&[0, 0, 0]); // length, filled below
        body.extend_from_slice(&[0x03, 0x03]); // client_version
        body.extend_from_slice(&[0x00; 32]); // random
        body.push(0); // session id len
        body.extend_from_slice(&2u16.to_be_bytes()); // cipher suites len
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(1); // compression methods len
        body.push(0);
        body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        body.extend_from_slice(&extensions);
        let length = (body.len() - 4) as u32;
        body[1..4].copy_from_slice(&length.to_be_bytes()[1..]);

        let mut record = vec![0x16, 0x03, 0x01];
        record.extend_from_slice(&(body.len() as u16).to_be_bytes());
        record.extend_from_slice(&body);
        record
    }

    #[test]
    fn a_client_hello_yields_its_server_name() {
        let hello = client_hello("api.fix-checkout.trip.localhost");
        assert_eq!(
            server_name(&hello).as_deref(),
            Some("api.fix-checkout.trip.localhost")
        );
    }

    /// The router reads from a socket, so it sees the hello a piece at a
    /// time. Every prefix must say "not yet" rather than guess.
    #[test]
    fn every_prefix_of_a_hello_is_patient_rather_than_wrong() {
        let hello = client_hello("web.localhost");
        for cut in 0..hello.len() {
            assert_eq!(
                server_name(&hello[..cut]),
                None,
                "a {cut}-byte prefix produced an answer"
            );
        }
        assert!(server_name(&hello).is_some(), "the whole thing works");
    }

    #[test]
    fn something_that_is_not_tls_is_not_a_hello() {
        assert_eq!(server_name(b"GET / HTTP/1.1\r\n\r\n"), None);
        assert_eq!(server_name(&[]), None);
        // A handshake record that is not a ClientHello.
        assert_eq!(
            server_name(&[0x16, 0x03, 0x01, 0x00, 0x04, 0x02, 0, 0, 0]),
            None
        );
    }

    /// Length fields come off the wire, so every one of them is an attacker's
    /// choice. None may index past the buffer.
    #[test]
    fn truncated_and_lying_length_fields_do_not_panic() {
        let hello = client_hello("api.localhost");
        for cut in 5..hello.len() {
            let mut damaged = hello[..cut].to_vec();
            // Claim the record is longer than what follows.
            damaged[3..5].copy_from_slice(&0xffffu16.to_be_bytes());
            assert_eq!(server_name(&damaged), None);
        }
        let mut lying = hello.clone();
        lying[43] = 0xff; // absurd session id length
        let _ = server_name(&lying);
    }

    /// A connection to an IP address sends no SNI at all. There is nothing to
    /// route it to, and guessing would send someone's traffic to a stranger.
    #[test]
    fn a_hello_without_a_server_name_routes_nowhere() {
        let mut body = Vec::new();
        body.push(0x01);
        body.extend_from_slice(&[0, 0, 0]);
        body.extend_from_slice(&[0x03, 0x03]);
        body.extend_from_slice(&[0x00; 32]);
        body.push(0);
        body.extend_from_slice(&2u16.to_be_bytes());
        body.extend_from_slice(&[0x13, 0x01]);
        body.push(1);
        body.push(0);
        body.extend_from_slice(&0u16.to_be_bytes()); // no extensions at all
        let length = (body.len() - 4) as u32;
        body[1..4].copy_from_slice(&length.to_be_bytes()[1..]);

        let mut record = vec![0x16, 0x03, 0x01];
        record.extend_from_slice(&(body.len() as u16).to_be_bytes());
        record.extend_from_slice(&body);

        assert_eq!(server_name(&record), None);
    }
}
