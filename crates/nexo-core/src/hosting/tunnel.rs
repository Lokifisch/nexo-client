//! Public address for a hosted server through Nexo's QUIClime relay (relay.nexomc.dev), the
//! same "quiclime" protocol Nexo Mod's `lantunnel/LanTunnelSession`
//! speaks (adapted there from e4mc, MIT; see `Mod/THIRD-PARTY-NOTICES.md`).
//! This side is written fresh against that protocol, not ported line by line.
//!
//! The protocol: a QUIC connection with ALPN `quiclime`; the host opens one
//! bidirectional control stream carrying varint-length-prefixed JSON (ask for
//! a domain, get one back); then the relay opens one bidirectional stream per
//! joining player and the bytes on it are the player's raw TCP stream, which
//! this forwards to the server's game port on 127.0.0.1. Only the game port —
//! RCON is never reachable through here.

use crate::error::{Error, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::watch;

/// Nexo's own QUIClime relay: hosts connect over QUIC on UDP 25570, players
/// join `<words>.relay.nexomc.dev` on the default TCP 25565. Not through an SRV
/// record: a joining client puts the SRV *target* in its handshake, so the
/// relay could no longer tell which tunnel was meant.
const RELAY_HOST: &str = "relay.nexomc.dev";
const RELAY_PORT: u16 = 25570;
const ALPN: &[u8] = b"quiclime";
const ASSIGN_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_CONTROL_FRAME: usize = 64 * 1024;

/// A live tunnel. Dropping it doesn't close it; call [`Tunnel::close`].
pub struct Tunnel {
    pub domain: String,
    connection: quinn::Connection,
    /// Flips to `Some(reason)` when the relay connection ends on its own.
    pub closed: watch::Receiver<Option<String>>,
}

impl Tunnel {
    pub fn close(&self) {
        self.connection.close(0u32.into(), b"server stopped");
    }

    pub fn is_open(&self) -> bool {
        self.closed.borrow().is_none()
    }
}

/// Connects to Nexo's relay, gets a public domain assigned, and starts
/// forwarding joining players to `127.0.0.1:game_port`.
pub async fn open(game_port: u16) -> Result<Tunnel> {
    let addr = tokio::net::lookup_host((RELAY_HOST, RELAY_PORT))
        .await
        .map_err(|e| Error::invalid(format!("could not resolve the relay {RELAY_HOST}: {e}")))?
        .next()
        .ok_or_else(|| Error::invalid(format!("the relay {RELAY_HOST} has no address")))?;

    let bind = if addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
    let mut endpoint = quinn::Endpoint::client(bind.parse().expect("literal address"))
        .map_err(|e| Error::invalid(format!("could not open a UDP socket: {e}")))?;
    endpoint.set_default_client_config(client_config()?);

    let connection = endpoint
        .connect(addr, RELAY_HOST)
        .map_err(|e| Error::invalid(format!("relay connection failed: {e}")))?
        .await
        .map_err(|e| Error::invalid(format!("relay connection failed: {e}")))?;

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| Error::invalid(format!("relay control stream failed: {e}")))?;
    write_frame(&mut send, r#"{"kind":"request_domain_assignment"}"#).await?;

    let domain = tokio::time::timeout(ASSIGN_TIMEOUT, async {
        loop {
            let msg = read_frame(&mut recv).await?;
            let json: serde_json::Value = serde_json::from_str(&msg)?;
            match json["kind"].as_str() {
                Some("domain_assignment_complete") => {
                    let domain = json["domain"].as_str().unwrap_or_default().to_string();
                    if domain.is_empty() {
                        return Err(Error::invalid("the relay assigned an empty domain"));
                    }
                    return Ok(domain);
                }
                other => tracing::debug!(?other, "relay control message"),
            }
        }
    })
    .await
    .map_err(|_| Error::invalid("the relay never assigned an address"))??;
    tracing::info!(%domain, relay = RELAY_HOST, "tunnel open");

    let (closed_tx, closed) = watch::channel(None);
    let conn = connection.clone();
    tokio::spawn(async move {
        // Keeps the control stream (and with it the domain) alive, and logs
        // anything the relay broadcasts.
        let _send = send;
        while let Ok(msg) = read_frame(&mut recv).await {
            tracing::info!(%msg, "relay message");
        }
    });
    tokio::spawn(async move {
        let reason = loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => {
                    tokio::spawn(forward(send, recv, game_port));
                }
                Err(err) => break err.to_string(),
            }
        };
        tracing::info!(%reason, "tunnel closed");
        let _ = closed_tx.send(Some(reason));
        // Holds the endpoint until the connection is gone; dropping it early
        // would tear down the socket under a live connection.
        endpoint.wait_idle().await;
    });

    Ok(Tunnel { domain, connection, closed })
}

async fn forward(send: quinn::SendStream, recv: quinn::RecvStream, game_port: u16) {
    let mut player = tokio::io::join(recv, send);
    match tokio::net::TcpStream::connect(("127.0.0.1", game_port)).await {
        Ok(mut server) => {
            let _ = tokio::io::copy_bidirectional(&mut player, &mut server).await;
        }
        Err(err) => tracing::warn!(%err, game_port, "tunnelled player could not reach the server"),
    }
}

fn client_config() -> Result<quinn::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|e| Error::invalid(format!("TLS setup failed: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];

    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| Error::invalid(format!("QUIC setup failed: {e}")))?;
    let mut transport = quinn::TransportConfig::default();
    // The relay drops a connection idle for 10s; a server with nobody on it
    // is exactly that.
    transport.keep_alive_interval(Some(Duration::from_secs(4)));
    transport.max_concurrent_bidi_streams(512u32.into());
    let mut config = quinn::ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

async fn write_frame(send: &mut quinn::SendStream, json: &str) -> Result<()> {
    let mut frame = Vec::with_capacity(json.len() + 5);
    put_varint(&mut frame, json.len() as u32);
    frame.extend_from_slice(json.as_bytes());
    send.write_all(&frame)
        .await
        .map_err(|e| Error::invalid(format!("relay write failed: {e}")))
}

async fn read_frame(recv: &mut quinn::RecvStream) -> Result<String> {
    let mut len: usize = 0;
    for shift in (0..35).step_by(7) {
        let byte = recv
            .read_u8()
            .await
            .map_err(|e| Error::invalid(format!("relay read failed: {e}")))?;
        len |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    if len > MAX_CONTROL_FRAME {
        return Err(Error::invalid("oversized relay control frame"));
    }
    let mut buf = vec![0; len];
    recv.read_exact(&mut buf)
        .await
        .map_err(|e| Error::invalid(format!("relay read failed: {e}")))?;
    String::from_utf8(buf).map_err(|_| Error::invalid("relay sent non-UTF-8 JSON"))
}

fn put_varint(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    #[test]
    fn varint_matches_minecraft_encoding() {
        let mut out = Vec::new();
        super::put_varint(&mut out, 300);
        assert_eq!(out, [0xac, 0x02]);
        out.clear();
        super::put_varint(&mut out, 36);
        assert_eq!(out, [36]);
    }
}
