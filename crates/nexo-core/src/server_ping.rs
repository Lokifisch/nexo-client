//! Minecraft's Server List Ping, the handshake the multiplayer screen uses to
//! fill in a server's icon, MOTD and player count before you join.
//!
//! Implemented directly rather than pulled in as a dependency: it is one TCP
//! connection, two packets out and one back, and the framing (a VarInt length,
//! a VarInt packet id, a length-prefixed UTF-8 payload) is the same handful of
//! primitives throughout.
//!
//! The one thing not hand-rolled is the SRV lookup — see [`resolve`]. Skipping
//! it is not a rounding error: most real servers advertise a name that has no
//! Minecraft behind it, and without SRV the majority of a player's list reports
//! as unreachable.

use crate::error::{Error, Result};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Minecraft's default, used whenever an address carries no port.
pub const DEFAULT_PORT: u16 = 25565;

/// How long the whole exchange gets. A dead address on a routed network hangs
/// until the TCP stack gives up, which is far longer than anyone will wait for
/// a row in a list to fill in.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Refuses a response big enough to be an attack rather than a MOTD. A real
/// status response is a few kilobytes; the favicon is capped at 64×64 by the
/// protocol.
const MAX_PACKET: usize = 2 * 1024 * 1024;

/// What a server answered.
#[derive(Debug, Clone)]
pub struct Status {
    /// The MOTD, flattened to plain text with the colour codes stripped.
    pub motd: String,
    pub players_online: u32,
    pub players_max: u32,
    /// The server's own version string — often a brand ("Paper 1.21") rather
    /// than a bare number, which is why it is passed through as written.
    pub version: String,
    /// The 64×64 PNG the server publishes, already base64-decoded.
    pub favicon: Option<Vec<u8>>,
    pub latency_ms: u32,
}

/// Pings one server. `address` is what is stored in `servers.dat`: a host,
/// optionally with `:port`, optionally bracketed if it is a literal IPv6.
pub async fn ping(address: &str) -> Result<Status> {
    let (host, port) = split_address(address);
    let started = Instant::now();

    // The whole budget covers resolution too. A nameserver that never answers
    // is just as much a hang as a server that never accepts.
    let status = tokio::time::timeout(TIMEOUT, async {
        let (host, port) = resolve(&host, port).await;
        exchange(&host, port).await
    })
    .await
    .map_err(|_| Error::invalid("timed out"))??;

    // Measured over the request/response rather than with the protocol's own
    // ping packet: it is the same round trip, and it saves an exchange whose
    // only purpose would be to time it.
    let latency_ms = started.elapsed().as_millis().min(u32::MAX as u128) as u32;

    Ok(Status {
        motd: flatten_motd(&status.description),
        players_online: status.players.as_ref().map(|p| p.online).unwrap_or(0),
        players_max: status.players.as_ref().map(|p| p.max).unwrap_or(0),
        version: status
            .version
            .map(|v| v.name)
            .unwrap_or_else(|| "unknown".to_string()),
        favicon: status.favicon.as_deref().and_then(decode_favicon),
        latency_ms,
    })
}

async fn exchange(host: &str, port: u16) -> Result<Response> {
    let mut stream = TcpStream::connect((host, port))
        .await
        .map_err(|err| Error::invalid(format!("{err}")))?;

    // Nagle would sit on the two tiny packets below waiting for more to send.
    let _ = stream.set_nodelay(true);

    let mut handshake = Vec::new();
    // -1 means "I am not telling you my protocol version", which every server
    // accepts for a status request. Sending a real number would make an old
    // server answer "outdated client" instead of describing itself.
    write_varint(&mut handshake, -1);
    write_string(&mut handshake, host);
    handshake.extend(port.to_be_bytes());
    // Next state: 1 = status, 2 = login. This never logs in.
    write_varint(&mut handshake, 1);

    stream.write_all(&frame(0x00, &handshake)).await?;
    stream.write_all(&frame(0x00, &[])).await?;
    stream.flush().await?;

    let payload = read_packet(&mut stream).await?;
    let mut cursor = Cursor::new(&payload);
    let id = cursor.varint()?;
    if id != 0x00 {
        return Err(Error::invalid(format!(
            "unexpected reply (packet {id:#x})"
        )));
    }

    let json = cursor.string()?;
    serde_json::from_str(&json).map_err(|err| Error::invalid(format!("malformed status: {err}")))
}

/// Wraps a packet body in the length + id framing the protocol expects.
fn frame(id: i32, body: &[u8]) -> Vec<u8> {
    let mut inner = Vec::with_capacity(body.len() + 1);
    write_varint(&mut inner, id);
    inner.extend_from_slice(body);

    let mut out = Vec::with_capacity(inner.len() + 5);
    write_varint(&mut out, inner.len() as i32);
    out.extend(inner);
    out
}

async fn read_packet(stream: &mut TcpStream) -> Result<Vec<u8>> {
    // The length prefix is itself a VarInt, so it has to be read a byte at a
    // time — there is no header of known size to read first.
    let mut length: i32 = 0;
    for shift in 0..5 {
        let byte = stream.read_u8().await?;
        length |= ((byte & 0x7F) as i32) << (shift * 7);
        if byte & 0x80 == 0 {
            break;
        }
        if shift == 4 {
            return Err(Error::invalid("malformed packet length"));
        }
    }

    if length <= 0 || length as usize > MAX_PACKET {
        return Err(Error::invalid("implausible packet length"));
    }

    let mut payload = vec![0; length as usize];
    stream.read_exact(&mut payload).await?;
    Ok(payload)
}

/// Splits `host:port`, tolerating a bracketed IPv6 literal.
///
/// The port is an `Option` on purpose: "no port given" and "port 25565 given"
/// are different inputs to [`resolve`], because Minecraft only consults SRV
/// when the player did not name a port.
fn split_address(address: &str) -> (String, Option<u16>) {
    let address = address.trim();

    // `[::1]:25565` — the colons inside the brackets are part of the address,
    // so the port can only be looked for after the closing bracket.
    if let Some(rest) = address.strip_prefix('[')
        && let Some((literal, tail)) = rest.split_once(']')
    {
        return (
            literal.to_string(),
            tail.strip_prefix(':').and_then(|p| p.parse().ok()),
        );
    }

    match address.rsplit_once(':') {
        // Only a trailing `:number` is a port. A bare IPv6 written without
        // brackets has colons too, and splitting it would produce nonsense.
        Some((host, port)) if !host.contains(':') => match port.parse() {
            Ok(port) => (host.to_string(), Some(port)),
            Err(_) => (address.to_string(), None),
        },
        _ => (address.to_string(), None),
    }
}

/// The resolver, built once. It caches, and it reads the system's DNS config,
/// which is work worth doing a single time rather than per ping.
fn resolver() -> Option<&'static hickory_resolver::TokioResolver> {
    static RESOLVER: std::sync::OnceLock<Option<hickory_resolver::TokioResolver>> =
        std::sync::OnceLock::new();

    RESOLVER
        .get_or_init(|| match hickory_resolver::TokioResolver::builder_tokio()
            .and_then(|builder| builder.build())
        {
            Ok(resolver) => Some(resolver),
            // A machine with no readable resolver config is unusual but not
            // fatal here: every lookup then falls through to the address as
            // written, which is what this did before SRV existed.
            Err(err) => {
                tracing::warn!("no system DNS config, SRV lookups disabled: {err}");
                None
            }
        })
        .as_ref()
}

/// Applies Minecraft's own address resolution: an SRV lookup first, then the
/// name as written.
///
/// This is what makes a server list work. `minehut.gg` has no A record at all
/// and exists only as an SRV pointing at `java-us.minehut.com`; `hypixel.net`
/// and `minekeep.gg` resolve to web hosts with 25565 closed. Connecting to the
/// name as typed reports all three as unreachable, which is exactly what they
/// are — just not what the player asked about.
///
/// An explicit port suppresses the lookup, matching the game: naming a port
/// means naming a specific endpoint, and an SRV record must not redirect it.
async fn resolve(host: &str, port: Option<u16>) -> (String, u16) {
    if let Some(port) = port {
        return (host.to_string(), port);
    }

    // A literal address has nothing to look up, and asking would be a pointless
    // round trip on every LAN server in the list.
    if host.parse::<std::net::IpAddr>().is_ok() {
        return (host.to_string(), DEFAULT_PORT);
    }

    if let Some(resolver) = resolver()
        && let Ok(lookup) = resolver.srv_lookup(format!("_minecraft._tcp.{host}.")).await
        // Priority/weight are ignored: this takes the first record rather than
        // implementing RFC 2782's weighted selection. For a status ping — one
        // connection, no session to keep — the difference is only which of a
        // server's own frontends answers.
        && let Some(srv) = lookup
            .answers()
            .iter()
            .find_map(|record| match &record.data {
                hickory_resolver::proto::rr::RData::SRV(srv) => Some(srv),
                _ => None,
            })
    {
        // A name from DNS is fully qualified, so it arrives with the trailing
        // dot. `TcpStream` does not mind, but it would show up in any error
        // message the row ends up displaying.
        let target = srv.target.to_utf8();
        return (target.trim_end_matches('.').to_string(), srv.port);
    }

    (host.to_string(), DEFAULT_PORT)
}

fn write_varint(out: &mut Vec<u8>, mut value: i32) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        // Logical shift: a negative protocol version has to encode as five
        // bytes of two's complement, not sign-extend forever.
        value = ((value as u32) >> 7) as i32;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn write_string(out: &mut Vec<u8>, value: &str) {
    write_varint(out, value.len() as i32);
    out.extend_from_slice(value.as_bytes());
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn varint(&mut self) -> Result<i32> {
        let mut value: i32 = 0;
        for shift in 0..5 {
            let byte = *self
                .buf
                .get(self.pos)
                .ok_or_else(|| Error::invalid("truncated reply"))?;
            self.pos += 1;
            value |= ((byte & 0x7F) as i32) << (shift * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::invalid("malformed varint"))
    }

    fn string(&mut self) -> Result<String> {
        let len = self.varint()?;
        if len < 0 {
            return Err(Error::invalid("negative string length"));
        }
        let end = self.pos + len as usize;
        let bytes = self
            .buf
            .get(self.pos..end)
            .ok_or_else(|| Error::invalid("truncated string"))?;
        self.pos = end;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

#[derive(Deserialize)]
struct Response {
    /// Either a plain string or a chat component — see [`flatten_motd`].
    #[serde(default)]
    description: serde_json::Value,
    #[serde(default)]
    players: Option<Players>,
    #[serde(default)]
    version: Option<Version>,
    #[serde(default)]
    favicon: Option<String>,
}

#[derive(Deserialize)]
struct Players {
    #[serde(default)]
    online: u32,
    #[serde(default)]
    max: u32,
}

#[derive(Deserialize)]
struct Version {
    #[serde(default)]
    name: String,
}

/// Turns a chat component into the line a list row can show.
///
/// `description` is one of three shapes depending on the server's vintage: a
/// plain string, a component with `text` and nested `extra`, or a list of
/// components. All three appear in the wild, so all three are walked rather
/// than assuming the modern one.
fn flatten_motd(value: &serde_json::Value) -> String {
    fn walk(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::String(text) => out.push_str(text),
            serde_json::Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::String(text)) = map.get("text") {
                    out.push_str(text);
                }
                if let Some(extra) = map.get("extra") {
                    walk(extra, out);
                }
            }
            _ => {}
        }
    }

    let mut text = String::new();
    walk(value, &mut text);
    strip_formatting(&text)
}

/// Drops Minecraft's `§`-prefixed colour and style codes, and flattens the
/// MOTD's second line into a separator — a list row is one line high.
fn strip_formatting(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();

    while let Some(c) = chars.next() {
        match c {
            '§' => {
                // The code is the *next* character, whatever it is; skipping it
                // blind is what keeps an unknown future code from being shown.
                let _ = chars.next();
            }
            '\n' => out.push_str(" · "),
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

/// `data:image/png;base64,…` as published in a status response.
fn decode_favicon(favicon: &str) -> Option<Vec<u8>> {
    let encoded = favicon.split_once("base64,")?.1;
    crate::util::base64_decode(encoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_split_into_host_and_port() {
        assert_eq!(
            split_address("mc.example.com"),
            ("mc.example.com".to_string(), None)
        );
        assert_eq!(
            split_address("mc.example.com:25577"),
            ("mc.example.com".to_string(), Some(25577))
        );
        assert_eq!(split_address("127.0.0.1:80"), ("127.0.0.1".to_string(), Some(80)));
        assert_eq!(
            split_address(" trimmed.example "),
            ("trimmed.example".to_string(), None)
        );
    }

    #[test]
    fn ipv6_keeps_its_colons() {
        // Bracketed, with and without a port.
        assert_eq!(split_address("[::1]:25565"), ("::1".to_string(), Some(25565)));
        assert_eq!(split_address("[fe80::1]"), ("fe80::1".to_string(), None));
        // Bare: every colon belongs to the address, so none of them is a port.
        assert_eq!(split_address("fe80::1"), ("fe80::1".to_string(), None));
    }

    #[test]
    fn a_trailing_colon_that_is_not_a_port_is_left_alone() {
        assert_eq!(
            split_address("mc.example.com:notaport"),
            ("mc.example.com:notaport".to_string(), None)
        );
    }

    #[tokio::test]
    async fn an_explicit_port_and_a_literal_address_skip_the_srv_lookup() {
        // Both of these must answer without touching DNS at all — the test
        // would otherwise depend on the network to prove the opposite.
        assert_eq!(
            resolve("mc.example.com", Some(25577)).await,
            ("mc.example.com".to_string(), 25577)
        );
        assert_eq!(
            resolve("192.168.1.9", None).await,
            ("192.168.1.9".to_string(), 25565)
        );
        assert_eq!(resolve("::1", None).await, ("::1".to_string(), 25565));
    }

    #[test]
    fn varints_round_trip_including_the_negative_handshake_version() {
        for value in [0, 1, 127, 128, 255, 25565, i32::MAX, -1] {
            let mut buf = Vec::new();
            write_varint(&mut buf, value);
            assert_eq!(Cursor::new(&buf).varint().unwrap(), value, "{value}");
        }

        // -1 is the one the handshake actually sends, and it must be the full
        // five bytes rather than sign-extending forever.
        let mut buf = Vec::new();
        write_varint(&mut buf, -1);
        assert_eq!(buf, [0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[test]
    fn motd_flattens_every_shape_a_server_might_send() {
        let plain = serde_json::json!("A Minecraft Server");
        assert_eq!(flatten_motd(&plain), "A Minecraft Server");

        let component = serde_json::json!({
            "text": "Welcome ",
            "extra": [{"text": "home"}, {"text": "!"}]
        });
        assert_eq!(flatten_motd(&component), "Welcome home!");

        let list = serde_json::json!([{"text": "one "}, {"text": "two"}]);
        assert_eq!(flatten_motd(&list), "one two");

        // Missing entirely — an empty MOTD, not a panic.
        assert_eq!(flatten_motd(&serde_json::Value::Null), "");
    }

    #[test]
    fn formatting_codes_and_second_lines_are_folded_away() {
        assert_eq!(strip_formatting("§aGreen §lBold"), "Green Bold");
        assert_eq!(strip_formatting("line one\nline two"), "line one · line two");
        // An unknown code still costs exactly one character.
        assert_eq!(strip_formatting("§#custom"), "custom");
    }

    #[test]
    fn favicon_is_decoded_from_its_data_url() {
        // A 1×1 PNG is enough to prove the prefix handling and the decode.
        let png = b"\x89PNG\r\n\x1a\n";
        let url = format!("data:image/png;base64,{}", crate::util::base64_encode(png));
        assert_eq!(decode_favicon(&url).as_deref(), Some(&png[..]));

        assert_eq!(decode_favicon("not a data url"), None);
    }

    #[test]
    fn a_status_response_parses_into_the_fields_a_row_shows() {
        let json = serde_json::json!({
            "version": {"name": "Paper 1.21", "protocol": 767},
            "players": {"online": 12, "max": 100},
            "description": {"text": "§bHello"},
        })
        .to_string();

        let parsed: Response = serde_json::from_str(&json).unwrap();
        assert_eq!(flatten_motd(&parsed.description), "Hello");
        assert_eq!(parsed.players.as_ref().unwrap().online, 12);
        assert_eq!(parsed.version.as_ref().unwrap().name, "Paper 1.21");
        assert!(parsed.favicon.is_none());
    }

    #[test]
    fn a_response_missing_everything_optional_still_parses() {
        // Some proxies answer with almost nothing. That is a reachable server
        // with no detail, not a failed ping.
        let parsed: Response = serde_json::from_str("{}").unwrap();
        assert_eq!(flatten_motd(&parsed.description), "");
        assert!(parsed.players.is_none());
    }
}
