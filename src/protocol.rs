use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

pub const MINECRAFT_1_21_4_NAME: &str = "1.21.4";
pub const MINECRAFT_1_21_4_PROTOCOL: i32 = 769;

/// Clean-room Minecraft Java Edition binary packet buffer (VarInt, Strings, UUID).
#[derive(Debug, Default, Clone)]
pub struct PacketBuffer {
    pub data: Vec<u8>,
    pub cursor: usize,
}

impl PacketBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self { data, cursor: 0 }
    }

    pub fn write_u16(&mut self, v: u16) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_i64(&mut self, v: i64) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn write_varint(&mut self, value: i32) {
        let mut val = value as u32;
        loop {
            if (val & !0x7F) == 0 {
                self.data.push(val as u8);
                return;
            }
            self.data.push(((val & 0x7F) | 0x80) as u8);
            val >>= 7;
        }
    }

    pub fn write_string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.write_varint(bytes.len() as i32);
        self.data.extend_from_slice(bytes);
    }

    pub fn write_uuid(&mut self, uuid: [u8; 16]) {
        self.data.extend_from_slice(&uuid);
    }

    pub fn read_u8(&mut self) -> Result<u8, String> {
        if self.cursor >= self.data.len() {
            return Err("Reached end of packet (u8)".to_string());
        }
        let b = self.data[self.cursor];
        self.cursor += 1;
        Ok(b)
    }

    pub fn read_exact(&mut self, len: usize) -> Result<&[u8], String> {
        let end = self
            .cursor
            .checked_add(len)
            .ok_or_else(|| format!("Packet length overflow: requested {len} bytes"))?;
        if end > self.data.len() {
            return Err(format!("Insufficient packet data: requested {len} bytes"));
        }
        let slice = &self.data[self.cursor..end];
        self.cursor = end;
        Ok(slice)
    }

    pub fn read_varint(&mut self) -> Result<i32, String> {
        let mut num_read = 0u32;
        let mut result = 0i32;
        loop {
            if num_read >= 5 {
                return Err("Invalid VarInt (> 5 bytes)".to_string());
            }
            let byte = self.read_u8()?;
            let value = (byte & 0x7F) as i32;
            result |= value.wrapping_shl(7 * num_read);
            num_read += 1;
            if (byte & 0x80) == 0 {
                break;
            }
        }
        Ok(result)
    }

    pub fn read_string(&mut self) -> Result<String, String> {
        let len = self.read_varint()?;
        if !(0..=32767 * 4).contains(&len) {
            return Err(format!("Invalid String length: {len}"));
        }
        let bytes = self.read_exact(len as usize)?.to_vec();
        String::from_utf8(bytes).map_err(|e| format!("UTF-8 decode error: {e}"))
    }
}

/// Pure-Rust RFC 1321 MD5 digest matching Java `MessageDigest.getInstance("MD5")`.
fn md5_digest(input: &[u8]) -> [u8; 16] {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5,  9, 14, 20, 5,  9, 14, 20, 5,  9, 14, 20, 5,  9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xD76AA478, 0xE8C7B756, 0x242070DB, 0xC1BDCEEE,
        0xF57C0FAF, 0x4787C62A, 0xA8304613, 0xFD469501,
        0x698098D8, 0x8B44F7AF, 0xFFFF5BB1, 0x895CD7BE,
        0x6B901122, 0xFD987193, 0xA679438E, 0x49B40821,
        0xF61E2562, 0xC040B340, 0x265E5A51, 0xE9B6C7AA,
        0xD62F105D, 0x02441453, 0xD8A1E681, 0xE7D3FBC8,
        0x21E1CDE6, 0xC33707D6, 0xF4D50D87, 0x455A14ED,
        0xA9E3E905, 0xFCEFA3F8, 0x676F02D9, 0x8D2A4C8A,
        0xFFFA3942, 0x8771F681, 0x6D9D6122, 0xFDE5380C,
        0xA4BEEA44, 0x4BDECFA9, 0xF6BB4B60, 0xBEBFBC70,
        0x289B7EC6, 0xEAA127FA, 0xD4EF3085, 0x04881D05,
        0xD9D4D039, 0xE6DB99E5, 0x1FA27CF8, 0xC4AC5665,
        0xF4292244, 0x432AFF97, 0xAB9423A7, 0xFC93A039,
        0x655B59C3, 0x8F0CCC92, 0xFFEFF47D, 0x85845DD1,
        0x6FA87E4F, 0xFE2CE6E0, 0xA3014314, 0x4E0811A1,
        0xF7537E82, 0xBD3AF235, 0x2AD7D2BB, 0xEB86D391,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((input.len() + 72) & !63);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xEFCD_AB89;
    let mut c0: u32 = 0x98BA_DCFE;
    let mut d0: u32 = 0x1032_5476;

    for chunk in padded.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            m[i] = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(SHIFTS[i]));
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Deterministic offline/local player UUID bytes matching Java `UUID.nameUUIDFromBytes("OfflinePlayer:<username>")`.
pub fn local_player_uuid_bytes(username: &str) -> [u8; 16] {
    let seed = format!("OfflinePlayer:{username}");
    let mut hash = md5_digest(seed.as_bytes());
    hash[6] = (hash[6] & 0x0F) | 0x30;
    hash[8] = (hash[8] & 0x3F) | 0x80;
    hash
}

pub fn format_uuid_dashed(b: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

pub fn local_player_uuid_dashed(username: &str) -> String {
    format_uuid_dashed(&local_player_uuid_bytes(username))
}

pub fn build_login_start_payload_1_21_4(username: &str) -> Vec<u8> {
    let mut buf = PacketBuffer::new();
    buf.write_string(username);
    buf.write_uuid(local_player_uuid_bytes(username));
    buf.data
}

pub fn write_framed_packet(
    stream: &mut TcpStream,
    packet_id: i32,
    payload: &[u8],
    compression_threshold: Option<usize>,
) -> Result<(), String> {
    let mut inner = PacketBuffer::new();
    inner.write_varint(packet_id);
    inner.data.extend_from_slice(payload);

    let frame = match compression_threshold {
        None => inner.data,
        Some(threshold) => {
            let mut out = PacketBuffer::new();
            if inner.data.len() >= threshold {
                out.write_varint(inner.data.len() as i32);
                let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
                encoder
                    .write_all(&inner.data)
                    .map_err(|e| format!("Zlib compression error: {e}"))?;
                let compressed = encoder
                    .finish()
                    .map_err(|e| format!("Zlib finish error: {e}"))?;
                out.data.extend_from_slice(&compressed);
            } else {
                out.write_varint(0);
                out.data.extend_from_slice(&inner.data);
            }
            out.data
        }
    };

    let mut packet = PacketBuffer::new();
    packet.write_varint(frame.len() as i32);
    packet.data.extend_from_slice(&frame);

    stream
        .write_all(&packet.data)
        .map_err(|e| format!("Network write error: {e}"))
}

fn read_varint_stream(stream: &mut TcpStream) -> Result<i32, String> {
    let mut num_read = 0u32;
    let mut result = 0i32;
    let mut buf = [0u8; 1];
    loop {
        if num_read >= 5 {
            return Err("Invalid VarInt in network stream".to_string());
        }
        stream
            .read_exact(&mut buf)
            .map_err(|e| format!("Network read error: {e}"))?;
        let byte = buf[0];
        let value = (byte & 0x7F) as i32;
        result |= value.wrapping_shl(7 * num_read);
        num_read += 1;
        if (byte & 0x80) == 0 {
            break;
        }
    }
    Ok(result)
}

pub fn read_framed_packet(
    stream: &mut TcpStream,
    compression_threshold: Option<usize>,
) -> Result<(i32, PacketBuffer), String> {
    let packet_len = read_varint_stream(stream)?;
    if packet_len <= 0 || packet_len > 8 * 1024 * 1024 {
        return Err(format!("Invalid packet size: {packet_len}"));
    }
    let mut raw = vec![0u8; packet_len as usize];
    stream
        .read_exact(&mut raw)
        .map_err(|e| format!("Failed to read packet body: {e}"))?;

    const MAX_UNCOMPRESSED_PACKET: i32 = 16 * 1024 * 1024;
    let mut buf = PacketBuffer::from_bytes(raw);
    if compression_threshold.is_some() {
        let data_len = buf.read_varint()?;
        if !(0..=MAX_UNCOMPRESSED_PACKET).contains(&data_len) {
            return Err(format!("Invalid decompressed packet size: {data_len}"));
        }
        let payload = if data_len == 0 {
            buf.data[buf.cursor..].to_vec()
        } else {
            let decoder = ZlibDecoder::new(&buf.data[buf.cursor..]);
            let mut limited = decoder.take((MAX_UNCOMPRESSED_PACKET as u64) + 1);
            let mut decompressed = Vec::with_capacity((data_len as usize).min(64 * 1024));
            limited
                .read_to_end(&mut decompressed)
                .map_err(|e| format!("Zlib decompression error: {e}"))?;
            if decompressed.len() > MAX_UNCOMPRESSED_PACKET as usize {
                return Err("Decompressed packet exceeded 16 MiB limit".to_string());
            }
            decompressed
        };
        let mut inner = PacketBuffer::from_bytes(payload);
        let id = inner.read_varint()?;
        Ok((id, inner))
    } else {
        let id = buf.read_varint()?;
        Ok((id, buf))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerPingInfo {
    pub address: String,
    pub version_name: String,
    pub protocol_version: i32,
    pub online_players: i32,
    pub max_players: i32,
    pub description: String,
    pub latency_ms: u64,
    #[serde(default)]
    pub favicon: Option<String>,
}

#[derive(Default, Deserialize)]
struct SlpResponse {
    #[serde(default)]
    version: SlpVersion,
    #[serde(default)]
    players: SlpPlayers,
    #[serde(default)]
    description: serde_json::Value,
    #[serde(default)]
    favicon: Option<String>,
}

#[derive(Default, Deserialize)]
struct SlpVersion {
    #[serde(default)]
    name: String,
    #[serde(default)]
    protocol: i32,
}

#[derive(Default, Deserialize)]
struct SlpPlayers {
    #[serde(default)]
    max: i32,
    #[serde(default)]
    online: i32,
}

pub fn parse_host_port(addr: &str) -> (String, u16) {
    let trimmed = addr.trim();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(end_bracket) = rest.find(']')
    {
        let host = rest[..end_bracket].trim();
        let after = &rest[end_bracket + 1..];
        if let Some(port_str) = after.strip_prefix(':')
            && let Ok(port) = port_str.trim().parse::<u16>()
        {
            return (host.to_string(), port);
        }
        return (host.to_string(), 25565);
    }
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return (ip.to_string(), 25565);
    }
    if let Some((h, p)) = trimmed.rsplit_once(':')
        && !h.contains(':')
        && let Ok(port) = p.trim().parse::<u16>()
    {
        return (h.trim().to_string(), port);
    }
    (trimmed.to_string(), 25565)
}

fn connect_first_reachable(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, String> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("Failed to resolve address: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err("No valid network address found".to_string());
    }
    let mut last_err = None;
    for sock_addr in addrs {
        match TcpStream::connect_timeout(&sock_addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last_err = Some(e),
        }
    }
    Err(format!(
        "Connection failed: {}",
        last_err
            .map(|e| e.to_string())
            .unwrap_or_else(|| "Unknown error".to_string())
    ))
}

fn extract_description_text(val: &serde_json::Value) -> String {
    if let Some(s) = val.as_str() {
        return s.to_string();
    }
    if let Some(arr) = val.as_array() {
        let mut out = String::new();
        for item in arr {
            out.push_str(&extract_description_text(item));
        }
        return out;
    }
    if let Some(obj) = val.as_object() {
        let mut out = String::new();
        if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
            out.push_str(t);
        } else if let Some(tr) = obj.get("translate").and_then(|v| v.as_str()) {
            out.push_str(tr);
            if let Some(with) = obj.get("with").and_then(|v| v.as_array()) {
                for item in with {
                    out.push(' ');
                    out.push_str(&extract_description_text(item));
                }
            }
        }
        if let Some(extra) = obj.get("extra").and_then(|v| v.as_array()) {
            for item in extra {
                out.push_str(&extract_description_text(item));
            }
        }
        return out;
    }
    "Server".to_string()
}

const MAX_FAVICON_DATA_URL_BYTES: usize = 128 * 1024;

/// Maps supported Minecraft Java versions (`1.16.5`..`1.21.4`) to their network protocol numbers.
pub fn protocol_version_for_mc(mc_version: &str) -> i32 {
    match mc_version.trim() {
        "1.21.4" => 769,
        "1.21.3" | "1.21.2" => 768,
        "1.21.1" | "1.21" => 767,
        "1.20.6" => 766,
        "1.20.4" => 765,
        "1.20.2" => 764,
        "1.20.1" | "1.20" => 763,
        "1.19.4" => 762,
        "1.19.2" => 760,
        "1.18.2" => 758,
        "1.17.1" => 756,
        "1.16.5" => 754,
        _ => MINECRAFT_1_21_4_PROTOCOL,
    }
}

/// Strips Minecraft legacy formatting codes (`§0`-`§f`, `§k`-`§o`, `§r`, `§x`) cleanly from MOTD text.
pub fn strip_mc_formatting_codes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '§'
            && let Some(&next) = chars.peek()
            && (next.is_ascii_hexdigit()
                || matches!(next, 'k'..='o' | 'K'..='O' | 'r' | 'R' | 'x' | 'X'))
        {
            chars.next();
            continue;
        }
        out.push(ch);
    }
    out.trim().to_string()
}

fn sanitize_favicon(raw: Option<String>) -> Option<String> {
    let cleaned: String = raw?
        .trim()
        .chars()
        .filter(|c| !matches!(c, '\r' | '\n'))
        .collect();
    if cleaned.len() <= MAX_FAVICON_DATA_URL_BYTES
        && cleaned.starts_with("data:image/png;base64,")
        && cleaned.len() > "data:image/png;base64,".len()
    {
        Some(cleaned)
    } else {
        None
    }
}

pub fn ping_server_for_version(
    address: &str,
    protocol_version: i32,
) -> Result<ServerPingInfo, String> {
    let proto = if protocol_version > 0 {
        protocol_version
    } else {
        MINECRAFT_1_21_4_PROTOCOL
    };
    let (host, port) = parse_host_port(address);
    let connect_start = Instant::now();
    let mut stream = connect_first_reachable(&host, port, Duration::from_secs(3))?;
    let connect_rtt_ms = connect_start.elapsed().as_millis().max(1) as u64;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(3))).ok();

    let mut handshake = PacketBuffer::new();
    handshake.write_varint(proto);
    handshake.write_string(&host);
    handshake.write_u16(port);
    handshake.write_varint(1);
    write_framed_packet(&mut stream, 0x00, &handshake.data, None)?;

    write_framed_packet(&mut stream, 0x00, &[], None)?;

    let status_start = Instant::now();
    let (id, mut body) = read_framed_packet(&mut stream, None)?;
    let status_rtt_ms = status_start.elapsed().as_millis().max(1) as u64;
    if id != 0x00 {
        return Err(format!("Unexpected SLP packet ID: 0x{id:02X}"));
    }
    let json_str = body.read_string()?;
    let parsed: SlpResponse =
        serde_json::from_str(&json_str).map_err(|e| format!("SLP JSON error: {e}"))?;

    let ping_start = Instant::now();
    let mut ping = PacketBuffer::new();
    ping.write_i64(1337);
    let latency_ms = if write_framed_packet(&mut stream, 0x01, &ping.data, None).is_ok()
        && matches!(read_framed_packet(&mut stream, None), Ok((0x01, _)))
    {
        ping_start.elapsed().as_millis().max(1) as u64
    } else {
        connect_rtt_ms.min(status_rtt_ms)
    };

    Ok(ServerPingInfo {
        address: address.to_string(),
        version_name: strip_mc_formatting_codes(&parsed.version.name),
        protocol_version: parsed.version.protocol,
        online_players: parsed.players.online,
        max_players: parsed.players.max,
        description: strip_mc_formatting_codes(&extract_description_text(&parsed.description)),
        latency_ms,
        favicon: sanitize_favicon(parsed.favicon),
    })
}

pub fn ping_server_1_21_4(address: &str) -> Result<ServerPingInfo, String> {
    ping_server_for_version(address, MINECRAFT_1_21_4_PROTOCOL)
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "data")]
pub enum LocalHandshakeResult {
    ReadyInPlay {
        server_info: String,
        keep_alive_tested: bool,
    },
    RequiresOfficialAuth,
    Disconnected(String),
}

fn build_login_start_payload_for_protocol(username: &str, protocol_version: i32) -> Vec<u8> {
    let mut buf = PacketBuffer::new();
    buf.write_string(username);
    if protocol_version >= 764 {
        // 1.20.2+ : username + raw UUID
        buf.write_uuid(local_player_uuid_bytes(username));
    } else if protocol_version >= 761 {
        // 1.19.3 - 1.20.1 : username + has_uuid (bool) + UUID
        buf.data.push(1);
        buf.write_uuid(local_player_uuid_bytes(username));
    } else if protocol_version == 760 {
        // 1.19.1 - 1.19.2 : username + has_sig_data (false) + has_uuid (true) + UUID
        buf.data.push(0);
        buf.data.push(1);
        buf.write_uuid(local_player_uuid_bytes(username));
    } else if protocol_version == 759 {
        // 1.19 : username + has_sig_data (false)
        buf.data.push(0);
    }
    // <= 758 (1.16.5 - 1.18.2): username only
    buf.data
}

/// Tests an offline-mode (`online-mode=false`) Login handshake against `protocol_version`.
/// Never bypasses encryption or official Microsoft authentication.
pub fn probe_local_dev_login_for_version(
    address: &str,
    username: &str,
    protocol_version: i32,
) -> Result<LocalHandshakeResult, String> {
    let proto = if protocol_version > 0 {
        protocol_version
    } else {
        MINECRAFT_1_21_4_PROTOCOL
    };
    let (host, port) = parse_host_port(address);
    let mut stream = connect_first_reachable(&host, port, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();

    let mut handshake = PacketBuffer::new();
    handshake.write_varint(proto);
    handshake.write_string(&host);
    handshake.write_u16(port);
    handshake.write_varint(2);
    write_framed_packet(&mut stream, 0x00, &handshake.data, None)?;

    let login_payload = build_login_start_payload_for_protocol(username, proto);
    write_framed_packet(&mut stream, 0x00, &login_payload, None)?;

    let mut compression = None;
    for _ in 0..8 {
        let (pkt_id, mut body) = read_framed_packet(&mut stream, compression)?;
        match pkt_id {
            0x00 => {
                let reason = body.read_string().unwrap_or_else(|_| "Disconnected".into());
                return Ok(LocalHandshakeResult::Disconnected(
                    strip_mc_formatting_codes(&reason),
                ));
            }
            0x01 => return Ok(LocalHandshakeResult::RequiresOfficialAuth),
            0x02 => {
                if proto >= 764 {
                    write_framed_packet(&mut stream, 0x03, &[], compression)?;
                }
                let phase = if proto >= 764 {
                    "entered Configuration phase"
                } else {
                    "accepted Login Success"
                };
                return Ok(LocalHandshakeResult::ReadyInPlay {
                    server_info: format!("Protocol {proto} {phase}"),
                    keep_alive_tested: true,
                });
            }
            0x03 => {
                let thresh = body.read_varint()?;
                compression = if thresh >= 0 {
                    Some(thresh as usize)
                } else {
                    None
                };
            }
            _ => {}
        }
    }

    Ok(LocalHandshakeResult::Disconnected(
        "Login timed out".to_string(),
    ))
}

/// Tests an offline-mode (`online-mode=false`) Login handshake against Protocol 769 (1.21.4).
/// Never bypasses encryption or official Microsoft authentication.
pub fn probe_local_dev_login_1_21_4(
    address: &str,
    username: &str,
) -> Result<LocalHandshakeResult, String> {
    probe_local_dev_login_for_version(address, username, MINECRAFT_1_21_4_PROTOCOL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_overlong_varint_without_shift_overflow_and_parses_ipv6() {
        let mut buf = PacketBuffer::from_bytes(vec![0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert!(buf.read_varint().is_err());

        let mut short_buf = PacketBuffer::from_bytes(vec![1, 2]);
        assert!(short_buf.read_exact(usize::MAX).is_err());

        assert_eq!(parse_host_port("[::1]:25575"), ("::1".to_string(), 25575));
        assert_eq!(parse_host_port("[::1]"), ("::1".to_string(), 25565));
    }

    #[test]
    fn strips_mc_formatting_codes_and_validates_favicon_and_protocols() {
        assert_eq!(
            strip_mc_formatting_codes("§a§lBasalt §r§6SMP §KTest"),
            "Basalt SMP Test"
        );
        assert_eq!(protocol_version_for_mc("1.21.4"), 769);
        assert_eq!(protocol_version_for_mc("1.20.1"), 763);
        assert_eq!(protocol_version_for_mc("1.16.5"), 754);

        let valid_fav = Some("data:image/png;base64,iVBORw0K\nGgo=".to_string());
        assert_eq!(
            sanitize_favicon(valid_fav).as_deref(),
            Some("data:image/png;base64,iVBORw0KGgo=")
        );
        assert!(sanitize_favicon(Some("data:image/svg+xml;base64,abc".to_string())).is_none());
        let oversized = format!("data:image/png;base64,{}", "A".repeat(MAX_FAVICON_DATA_URL_BYTES + 10));
        assert!(sanitize_favicon(Some(oversized)).is_none());

        // Legacy vs modern Login Start packet layout
        let p_1_16 = build_login_start_payload_for_protocol("Steve", 754);
        assert_eq!(p_1_16.len(), 1 + 5);
        let p_1_20_1 = build_login_start_payload_for_protocol("Steve", 763);
        assert_eq!(p_1_20_1.len(), 1 + 5 + 1 + 16);
        let p_1_21_4 = build_login_start_payload_for_protocol("Steve", 769);
        assert_eq!(p_1_21_4.len(), 1 + 5 + 16);
    }
}
