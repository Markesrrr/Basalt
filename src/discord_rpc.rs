use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

/// Public Discord Rich Presence application client ID for Basalt Launcher.
pub const BASALT_DISCORD_CLIENT_ID: &str = "1306984192837468190";
pub const BASALT_GITHUB_URL: &str = "https://github.com/Markesrrr/Basalt";
pub const BASALT_RELEASES_URL: &str = "https://github.com/Markesrrr/Basalt/releases";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordRpcStatus {
    pub enabled: bool,
    pub connected: bool,
    pub pipe_name: String,
    pub details: String,
    pub state_label: String,
    pub started_epoch_secs: u64,
    pub github_url: String,
}

impl Default for DiscordRpcStatus {
    fn default() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            enabled: true,
            connected: false,
            pipe_name: "discord-ipc-0".to_string(),
            details: "Minecraft 1.21.4 (Vanilla)".to_string(),
            state_label: "Basalt Launcher · Ready".to_string(),
            started_epoch_secs: now,
            github_url: BASALT_GITHUB_URL.to_string(),
        }
    }
}

/// Encodes a Discord IPC frame: `[opcode: u32 LE][length: u32 LE][json payload]`.
pub fn encode_ipc_frame(opcode: u32, json_payload: &str) -> Vec<u8> {
    let bytes = json_payload.as_bytes();
    let len = bytes.len() as u32;
    let mut frame = Vec::with_capacity(8 + bytes.len());
    frame.extend_from_slice(&opcode.to_le_bytes());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(bytes);
    frame
}

/// Infers loader and Java runtime labels from `details` + `mc_version` when explicit labels are omitted.
pub fn infer_loader_and_java_labels(mc_version: &str, details: &str) -> (&'static str, &'static str) {
    let lower = details.to_ascii_lowercase();
    let loader = if lower.contains("fabric") {
        "Fabric"
    } else if lower.contains("forge") {
        "Forge"
    } else {
        "Vanilla"
    };
    let java = if loader == "Forge" && mc_version.trim() == "1.16.5" {
        "Java 8/17"
    } else if loader == "Forge"
        && matches!(
            mc_version.trim(),
            "1.17.1" | "1.18.2" | "1.19.2" | "1.19.4" | "1.20" | "1.20.1" | "1.20.2" | "1.20.4"
        )
    {
        "Java 17/21"
    } else {
        "Java 21"
    };
    (loader, java)
}

/// Builds the `SET_ACTIVITY` JSON payload for Discord IPC Opcode 1 with explicit loader/Java metadata.
pub fn build_activity_payload_with_meta(
    pid: u32,
    details: &str,
    state_label: &str,
    started_epoch_secs: u64,
    mc_version: &str,
    loader_label: Option<&str>,
    java_label: Option<&str>,
) -> String {
    let (inferred_loader, inferred_java) = infer_loader_and_java_labels(mc_version, details);
    let loader = loader_label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(inferred_loader);
    let java = java_label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(inferred_java);
    let payload = serde_json::json!({
        "cmd": "SET_ACTIVITY",
        "args": {
            "pid": pid,
            "activity": {
                "details": details,
                "state": state_label,
                "timestamps": {
                    "start": started_epoch_secs
                },
                "assets": {
                    "large_image": "basalt_logo",
                    "large_text": format!("Basalt Launcher · {loader} {mc_version}"),
                    "small_image": "java21",
                    "small_text": format!("{java} · IDM Turbo · Low-Ping")
                },
                "buttons": [
                    {
                        "label": "Basalt GitHub",
                        "url": BASALT_GITHUB_URL
                    },
                    {
                        "label": "Download Release",
                        "url": BASALT_RELEASES_URL
                    }
                ]
            }
        },
        "nonce": format!("basalt-{started_epoch_secs}-{pid}")
    });
    payload.to_string()
}

/// Builds the `SET_ACTIVITY` JSON payload for Discord IPC Opcode 1.
pub fn build_activity_payload(
    pid: u32,
    details: &str,
    state_label: &str,
    started_epoch_secs: u64,
    mc_version: &str,
) -> String {
    build_activity_payload_with_meta(
        pid,
        details,
        state_label,
        started_epoch_secs,
        mc_version,
        None,
        None,
    )
}

/// Builds the `SET_ACTIVITY` clear payload when Rich Presence is disabled.
pub fn build_clear_activity_payload(pid: u32) -> String {
    serde_json::json!({
        "cmd": "SET_ACTIVITY",
        "args": {
            "pid": pid,
            "activity": null
        },
        "nonce": format!("basalt-clear-{pid}")
    })
    .to_string()
}

struct PersistentPipe {
    file: File,
    label: String,
}

fn pipe_cell() -> &'static Mutex<Option<PersistentPipe>> {
    static PIPE: OnceLock<Mutex<Option<PersistentPipe>>> = OnceLock::new();
    PIPE.get_or_init(|| Mutex::new(None))
}

struct DiscordUpdateReq {
    enabled: bool,
    mc_version: String,
    details: String,
    state_label: String,
    started_epoch_secs: u64,
    loader_label: Option<String>,
    java_label: Option<String>,
    discord_arc: Arc<Mutex<DiscordRpcStatus>>,
    seq: u64,
}

static DISCORD_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn discord_worker_sender() -> &'static mpsc::Sender<DiscordUpdateReq> {
    static TX: OnceLock<mpsc::Sender<DiscordUpdateReq>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<DiscordUpdateReq>();
        std::thread::spawn(move || {
            while let Ok(mut latest) = rx.recv() {
                // Coalesce burst updates and apply only the newest activity state.
                while let Ok(newer) = rx.try_recv() {
                    latest = newer;
                }
                let updated = publish_discord_activity_with_meta(
                    latest.enabled,
                    &latest.mc_version,
                    &latest.details,
                    &latest.state_label,
                    latest.started_epoch_secs,
                    latest.loader_label.as_deref(),
                    latest.java_label.as_deref(),
                );
                if DISCORD_SEQ.load(std::sync::atomic::Ordering::SeqCst) == latest.seq
                    && let Ok(mut guard) = latest.discord_arc.lock()
                {
                    *guard = updated;
                }
            }
        });
        tx
    })
}

/// Queues a non-blocking Discord Rich Presence update on a single persistent background worker.
pub fn queue_discord_activity_update(
    discord_arc: Arc<Mutex<DiscordRpcStatus>>,
    enabled: bool,
    mc_version: String,
    details: String,
    state_label: String,
    started_epoch_secs: u64,
) {
    queue_discord_activity_update_with_meta(
        discord_arc,
        enabled,
        mc_version,
        details,
        state_label,
        started_epoch_secs,
        None,
        None,
    );
}

/// Queues a non-blocking Discord Rich Presence update with explicit loader and Java labels.
#[allow(clippy::too_many_arguments)]
pub fn queue_discord_activity_update_with_meta(
    discord_arc: Arc<Mutex<DiscordRpcStatus>>,
    enabled: bool,
    mc_version: String,
    details: String,
    state_label: String,
    started_epoch_secs: u64,
    loader_label: Option<String>,
    java_label: Option<String>,
) {
    let seq = DISCORD_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let _ = discord_worker_sender().send(DiscordUpdateReq {
        enabled,
        mc_version,
        details,
        state_label,
        started_epoch_secs,
        loader_label,
        java_label,
        discord_arc,
        seq,
    });
}

#[cfg(windows)]
fn pipe_bytes_available(file: &File) -> Option<usize> {
    use std::os::windows::io::AsRawHandle;
    unsafe extern "system" {
        fn PeekNamedPipe(
            h_named_pipe: *mut core::ffi::c_void,
            lp_buffer: *mut core::ffi::c_void,
            n_buffer_size: u32,
            lp_bytes_read: *mut u32,
            lp_total_bytes_avail: *mut u32,
            lp_bytes_left_this_message: *mut u32,
        ) -> i32;
    }
    let mut avail: u32 = 0;
    let ok = unsafe {
        PeekNamedPipe(
            file.as_raw_handle(),
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            &mut avail,
            core::ptr::null_mut(),
        )
    };
    if ok != 0 { Some(avail as usize) } else { None }
}

#[cfg(not(windows))]
fn pipe_bytes_available(_file: &File) -> Option<usize> {
    None
}

fn read_one_ipc_frame(pipe: &mut File) -> bool {
    let mut hdr = [0u8; 8];
    if pipe.read_exact(&mut hdr).is_err() {
        return false;
    }
    let resp_len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    if resp_len > 0 && resp_len <= 65536 {
        let mut body = vec![0u8; resp_len];
        return pipe.read_exact(&mut body).is_ok();
    }
    resp_len == 0
}

fn wait_and_read_ipc_frame(pipe: &mut File, max_wait_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_wait_ms);
    loop {
        match pipe_bytes_available(pipe) {
            Some(avail) if avail >= 8 => return read_one_ipc_frame(pipe),
            Some(_) => {
                if std::time::Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            None => return false,
        }
    }
}

fn drain_ipc_replies_nonblocking(pipe: &mut File) {
    for _ in 0..8 {
        match pipe_bytes_available(pipe) {
            Some(avail) if avail >= 8 => {
                if !read_one_ipc_frame(pipe) {
                    break;
                }
            }
            _ => break,
        }
    }
}

/// Closes any open Discord IPC pipe cleanly so Rich Presence clears on launcher exit.
pub fn shutdown_discord_rpc() {
    if let Ok(mut guard) = pipe_cell().lock() {
        *guard = None;
    }
}

/// Sends a Discord Rich Presence handshake/activity update while retaining the open named-pipe handle
/// (`\\.\pipe\discord-ipc-0..9`) so Discord keeps displaying the activity cleanly.
pub fn publish_discord_activity(
    enabled: bool,
    mc_version: &str,
    details: &str,
    state_label: &str,
    started_epoch_secs: u64,
) -> DiscordRpcStatus {
    publish_discord_activity_with_meta(
        enabled,
        mc_version,
        details,
        state_label,
        started_epoch_secs,
        None,
        None,
    )
}

/// Sends a Discord Rich Presence update with explicit loader/Java labels and drains IPC replies safely.
pub fn publish_discord_activity_with_meta(
    enabled: bool,
    mc_version: &str,
    details: &str,
    state_label: &str,
    started_epoch_secs: u64,
    loader_label: Option<&str>,
    java_label: Option<&str>,
) -> DiscordRpcStatus {
    let mut status = DiscordRpcStatus {
        enabled,
        connected: false,
        pipe_name: "Standby (WebRTC/IPC)".to_string(),
        details: details.to_string(),
        state_label: state_label.to_string(),
        started_epoch_secs,
        github_url: BASALT_GITHUB_URL.to_string(),
    };

    let pid = std::process::id();
    let activity_json = if enabled {
        build_activity_payload_with_meta(
            pid,
            details,
            state_label,
            started_epoch_secs,
            mc_version,
            loader_label,
            java_label,
        )
    } else {
        build_clear_activity_payload(pid)
    };
    let activity_frame = encode_ipc_frame(1, &activity_json);

    let Ok(mut pipe_guard) = pipe_cell().lock() else {
        return status;
    };

    if !enabled {
        if let Some(ref mut existing) = *pipe_guard {
            drain_ipc_replies_nonblocking(&mut existing.file);
            let _ = existing.file.write_all(&activity_frame);
            let _ = existing.file.flush();
            let _ = wait_and_read_ipc_frame(&mut existing.file, 150);
        }
        *pipe_guard = None;
        return status;
    }

    if let Some(ref mut existing) = *pipe_guard {
        drain_ipc_replies_nonblocking(&mut existing.file);
        if existing.file.write_all(&activity_frame).is_ok() && existing.file.flush().is_ok() {
            let _ = wait_and_read_ipc_frame(&mut existing.file, 150);
            drain_ipc_replies_nonblocking(&mut existing.file);
            status.connected = true;
            status.pipe_name = existing.label.clone();
            return status;
        }
        *pipe_guard = None;
    }

    let handshake_json = serde_json::json!({
        "v": 1,
        "client_id": BASALT_DISCORD_CLIENT_ID
    })
    .to_string();
    let handshake_frame = encode_ipc_frame(0, &handshake_json);

    for idx in 0..10 {
        let pipe_label = format!("discord-ipc-{idx}");
        let pipe_path = format!(r"\\.\pipe\{pipe_label}");
        let Ok(mut pipe) = OpenOptions::new().read(true).write(true).open(&pipe_path) else {
            continue;
        };

        if pipe.write_all(&handshake_frame).is_err() || pipe.flush().is_err() {
            continue;
        }

        if !wait_and_read_ipc_frame(&mut pipe, 1200) {
            continue;
        }

        if pipe.write_all(&activity_frame).is_ok() && pipe.flush().is_ok() {
            let _ = wait_and_read_ipc_frame(&mut pipe, 200);
            drain_ipc_replies_nonblocking(&mut pipe);
            status.connected = true;
            status.pipe_name = pipe_label.clone();
            *pipe_guard = Some(PersistentPipe {
                file: pipe,
                label: pipe_label,
            });
            break;
        }
    }

    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_discord_ipc_frame_header_and_activity_json() {
        let frame = encode_ipc_frame(1, r#"{"v":1}"#);
        assert_eq!(&frame[0..4], &1u32.to_le_bytes());
        assert_eq!(&frame[4..8], &7u32.to_le_bytes());
        assert_eq!(&frame[8..], br#"{"v":1}"#);

        let payload = build_activity_payload(
            4242,
            "Minecraft 1.21.4 (Vanilla)",
            "Singleplayer · Survival",
            1700000000,
            "1.21.4",
        );
        assert!(payload.contains("SET_ACTIVITY"));
        assert!(payload.contains("https://github.com/Markesrrr/Basalt"));
        assert!(payload.contains("Singleplayer · Survival"));

        let fabric_payload = build_activity_payload(
            4242,
            "Playing Minecraft 1.21.4 (Fabric)",
            "In-Game · Player",
            1700000000,
            "1.21.4",
        );
        assert!(fabric_payload.contains("Basalt Launcher · Fabric 1.21.4"));
        assert!(!fabric_payload.contains("Vanilla 1.21.4"));

        let forge_1_16 = build_activity_payload_with_meta(
            4242,
            "Playing Minecraft 1.16.5 (Forge)",
            "In-Game · Player",
            1700000000,
            "1.16.5",
            Some("Forge"),
            Some("Java 8"),
        );
        assert!(forge_1_16.contains("Basalt Launcher · Forge 1.16.5"));
        assert!(forge_1_16.contains("Java 8 · IDM Turbo · Low-Ping"));
    }
}
