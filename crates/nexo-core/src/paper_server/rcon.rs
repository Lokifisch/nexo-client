//! Source RCON client — the protocol every Paper/vanilla server already
//! speaks (`enable-rcon` in `server.properties`). The one control channel
//! for "stop this server" and "run a console command", on both languages:
//! it decouples who holds the OS process handle from who can control the
//! server, so the launcher can stop a server the mod started and the other
//! way round, without a second hand-rolled cross-language IPC protocol.
//!
//! Hand-rolled rather than a dependency, matching the judgement call this
//! codebase already made for Server List Ping framing (`server_ping.rs`):
//! the protocol is a handful of length-prefixed packets over one TCP
//! connection. Mirrors `rcon/RconClient.java` exactly — see that file for
//! the packet layout diagram.

use crate::error::{Error, Result};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

const TYPE_RESPONSE_VALUE: i32 = 0;
/// SERVERDATA_AUTH_RESPONSE and SERVERDATA_EXECCOMMAND share this value;
/// direction (who sent it) disambiguates them.
const TYPE_AUTH_RESPONSE_OR_EXEC: i32 = 2;
const TYPE_AUTH: i32 = 3;
const MAX_PACKET_BYTES: usize = 1 << 16;

pub struct RconClient {
    stream: TcpStream,
    next_id: i32,
    timeout: Duration,
}

struct Packet {
    id: i32,
    kind: i32,
    body: String,
}

impl RconClient {
    /// Connects and authenticates. Errors if either fails within `call_timeout`.
    pub async fn connect(
        host: &str,
        port: u16,
        password: &str,
        call_timeout: Duration,
    ) -> Result<Self> {
        let stream = timeout(call_timeout, TcpStream::connect((host, port)))
            .await
            .map_err(|_| Error::invalid("RCON connection timed out"))?
            .map_err(|e| Error::invalid(format!("RCON connection failed: {e}")))?;

        let mut client = Self {
            stream,
            next_id: 1,
            timeout: call_timeout,
        };
        if !client.authenticate(password).await? {
            return Err(Error::invalid("RCON authentication rejected"));
        }
        Ok(client)
    }

    async fn authenticate(&mut self, password: &str) -> Result<bool> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(id, TYPE_AUTH, password).await?;
        // Some server implementations send an empty SERVERDATA_RESPONSE_VALUE
        // ahead of the real auth response; skip anything that isn't type 2.
        loop {
            let response = self.receive().await?;
            if response.kind == TYPE_AUTH_RESPONSE_OR_EXEC {
                return Ok(response.id == id);
            }
        }
    }

    /// Runs a console command (e.g. `"stop"`) and returns its text output.
    pub async fn command(&mut self, command: &str) -> Result<String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(id, TYPE_AUTH_RESPONSE_OR_EXEC, command).await?;
        let response = self.receive().await?;
        if response.kind != TYPE_RESPONSE_VALUE {
            return Err(Error::invalid(format!(
                "unexpected RCON response type {}",
                response.kind
            )));
        }
        Ok(response.body)
    }

    async fn send(&mut self, id: i32, kind: i32, body: &str) -> Result<()> {
        let body_bytes = body.as_bytes();
        let length = 4 + 4 + body_bytes.len() as i32 + 2;
        let mut packet = Vec::with_capacity(4 + length as usize);
        packet.extend_from_slice(&length.to_le_bytes());
        packet.extend_from_slice(&id.to_le_bytes());
        packet.extend_from_slice(&kind.to_le_bytes());
        packet.extend_from_slice(body_bytes);
        packet.push(0);
        packet.push(0);

        timeout(self.timeout, self.stream.write_all(&packet))
            .await
            .map_err(|_| Error::invalid("RCON write timed out"))?
            .map_err(|e| Error::invalid(format!("RCON write failed: {e}")))?;
        Ok(())
    }

    async fn receive(&mut self) -> Result<Packet> {
        let mut length_bytes = [0u8; 4];
        self.read_exact_timed(&mut length_bytes).await?;
        let length = i32::from_le_bytes(length_bytes);
        if !(10..=MAX_PACKET_BYTES as i32).contains(&length) {
            return Err(Error::invalid(format!(
                "RCON packet length out of range: {length}"
            )));
        }

        let mut rest = vec![0u8; length as usize];
        self.read_exact_timed(&mut rest).await?;

        let id = i32::from_le_bytes(rest[0..4].try_into().unwrap());
        let kind = i32::from_le_bytes(rest[4..8].try_into().unwrap());
        // The trailing two null bytes are excluded here by design.
        let body = String::from_utf8_lossy(&rest[8..rest.len() - 2]).into_owned();
        Ok(Packet { id, kind, body })
    }

    async fn read_exact_timed(&mut self, buf: &mut [u8]) -> Result<()> {
        timeout(self.timeout, self.stream.read_exact(buf))
            .await
            .map_err(|_| Error::invalid("RCON read timed out"))?
            .map_err(|e| Error::invalid(format!("RCON read failed: {e}")))?;
        Ok(())
    }
}
