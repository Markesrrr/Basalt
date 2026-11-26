use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

/// Discord's verified Minecraft application client ID for local Rich Presence IPC.
pub const BASALT_DISCORD_CLIENT_ID: &str = "356875570916753438";
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
    build_activity_payload_inner(
        pid,
        details,
        state_label,
        started_epoch_secs,
        mc_version,
        loader_label,
        java_label,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_activity_payload_inner(
    pid: u32,
    details: &str,
    state_label: &str,
    started_epoch_secs: u64,
    mc_version: &str,
    loader_label: Option<&str>,
    java_label: Option<&str>,
    include_buttons: bool,
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
    let _ = (loader, java);

    let mut activity = serde_json::json!({
        "details": details,
        "state": state_label,
        "timestamps": {
            "start": started_epoch_secs
        }
    });
    if include_buttons && let Some(obj) = activity.as_object_mut() {
        obj.insert(
            "buttons".to_string(),
            serde_json::json!([
                {
                    "label": "Basalt GitHub",
                    "url": BASALT_GITHUB_URL
                },
                {
                    "label": "Download Release",
                    "url": BASALT_RELEASES_URL
                }
            ]),
        );
    }

    let payload = serde_json::json!({
        "cmd": "SET_ACTIVITY",
        "args": {
            "pid": pid,
            "activity": activity
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

type DiscordStatusCallback = Arc<dyn Fn(DiscordRpcStatus) + Send + Sync>;

fn status_listener_cell() -> &'static Mutex<Option<DiscordStatusCallback>> {
    static LISTENER: OnceLock<Mutex<Option<DiscordStatusCallback>>> = OnceLock::new();
    LISTENER.get_or_init(|| Mutex::new(None))
}

/// Registers a listener invoked whenever Discord RPC connection state changes in the background worker.
pub fn set_discord_status_listener(cb: DiscordStatusCallback) {
    if let Ok(mut guard) = status_listener_cell().lock() {
        *guard = Some(cb);
    }
}

fn notify_status_listener(status: &DiscordRpcStatus) {
    let listener = status_listener_cell()
        .lock()
        .ok()
        .and_then(|g| g.clone());
    if let Some(cb) = listener {
        cb(status.clone());
    }
}

#[derive(Clone)]
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

fn apply_worker_update(req: &DiscordUpdateReq, force_notify: bool) {
    let updated = publish_discord_activity_with_meta(
        req.enabled,
        &req.mc_version,
        &req.details,
        &req.state_label,
        req.started_epoch_secs,
        req.loader_label.as_deref(),
        req.java_label.as_deref(),
    );
    if DISCORD_SEQ.load(std::sync::atomic::Ordering::SeqCst) == req.seq {
        let mut changed = force_notify;
        if let Ok(mut guard) = req.discord_arc.lock() {
            if guard.connected != updated.connected
                || guard.enabled != updated.enabled
                || guard.pipe_name != updated.pipe_name
                || guard.details != updated.details
                || guard.state_label != updated.state_label
            {
                changed = true;
            }
            *guard = updated.clone();
        }
        if changed {
            notify_status_listener(&updated);
        }
    }
}

fn check_active_pipe_health(req: &DiscordUpdateReq) {
    let pipe_lost = {
        let Ok(mut pipe_guard) = pipe_cell().lock() else {
            return;
        };
        let mut lost = false;
        if let Some(ref mut existing) = *pipe_guard {
            if !drain_ipc_replies_nonblocking(&mut existing.file) {
                lost = true;
            }
        } else {
            return;
        }
        if lost {
            *pipe_guard = None;
        }
        lost
    };
    if pipe_lost {
        // Attempt clean reconnect once, or transition status to standby and notify UI.
        apply_worker_update(req, true);
    }
}

fn discord_worker_sender() -> &'static mpsc::Sender<DiscordUpdateReq> {
    static TX: OnceLock<mpsc::Sender<DiscordUpdateReq>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<DiscordUpdateReq>();
        std::thread::spawn(move || {
            let mut last_req: Option<DiscordUpdateReq> = None;
            loop {
                let first = if last_req.is_some() {
                    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
                        Ok(req) => Some(req),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                } else {
                    match rx.recv() {
                        Ok(req) => Some(req),
                        Err(_) => break,
                    }
                };

                if let Some(mut latest) = first {
                    // Coalesce burst updates and apply only the newest activity state.
                    while let Ok(newer) = rx.try_recv() {
                        latest = newer;
                    }
                    apply_worker_update(&latest, false);
                    last_req = Some(latest);
                } else if let Some(ref req) = last_req
                    && req.enabled
                    && DISCORD_SEQ.load(std::sync::atomic::Ordering::SeqCst) == req.seq
                {
                    let is_connected = pipe_cell().lock().is_ok_and(|g| g.is_some());
                    if !is_connected {
                        apply_worker_update(req, false);
                    } else {
                        check_active_pipe_health(req);
                    }
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

fn read_ipc_frame(pipe: &mut File) -> Option<(u32, String)> {
    let mut hdr = [0u8; 8];
    if pipe.read_exact(&mut hdr).is_err() {
        return None;
    }
    let opcode = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let resp_len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    if resp_len > 65536 {
        return None;
    }
    if resp_len == 0 {
        return Some((opcode, String::new()));
    }
    let mut body = vec![0u8; resp_len];
    if pipe.read_exact(&mut body).is_err() {
        return None;
    }
    Some((opcode, String::from_utf8_lossy(&body).into_owned()))
}

fn is_ipc_error_or_close(opcode: u32, body: &str) -> bool {
    if opcode == 2 {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("evt").and_then(|e| e.as_str()).map(|s| s == "ERROR"))
        .unwrap_or(false)
}

fn wait_and_read_ipc_frame(pipe: &mut File, max_wait_ms: u64) -> Option<(u32, String)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(max_wait_ms);
    loop {
        match pipe_bytes_available(pipe) {
            Some(avail) if avail >= 8 => return read_ipc_frame(pipe),
            Some(_) => {
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            None => return None,
        }
    }
}

fn drain_ipc_replies_nonblocking(pipe: &mut File) -> bool {
    for _ in 0..8 {
        match pipe_bytes_available(pipe) {
            Some(avail) if avail >= 8 => {
                let Some((opcode, body)) = read_ipc_frame(pipe) else {
                    return false;
                };
                if opcode == 2 {
                    return false;
                }
                // Respond to Discord IPC PING (opcode 3) with PONG (opcode 4) so pipe stays alive.
                if opcode == 3 {
                    let pong = encode_ipc_frame(4, &body);
                    if pipe.write_all(&pong).is_err() || pipe.flush().is_err() {
                        return false;
                    }
                }
            }
            Some(_) => break,
            None => return false,
        }
    }
    true
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

#[allow(clippy::too_many_arguments)]
fn send_activity_on_pipe(
    pipe: &mut File,
    pid: u32,
    mc_version: &str,
    details: &str,
    state_label: &str,
    started_epoch_secs: u64,
    loader_label: Option<&str>,
    java_label: Option<&str>,
) -> bool {
    let json_with_buttons = build_activity_payload_inner(
        pid,
        details,
        state_label,
        started_epoch_secs,
        mc_version,
        loader_label,
        java_label,
        true,
    );
    let frame = encode_ipc_frame(1, &json_with_buttons);
    if pipe.write_all(&frame).is_err() || pipe.flush().is_err() {
        return false;
    }
    if let Some((op, body)) = wait_and_read_ipc_frame(pipe, 250)
        && is_ipc_error_or_close(op, &body)
    {
        if op == 2 {
            return false;
        }
        // Retry once without buttons in case client/app forbids custom URL buttons.
        let json_no_buttons = build_activity_payload_inner(
            pid,
            details,
            state_label,
            started_epoch_secs,
            mc_version,
            loader_label,
            java_label,
            false,
        );
        let fallback_frame = encode_ipc_frame(1, &json_no_buttons);
        if pipe.write_all(&fallback_frame).is_err() || pipe.flush().is_err() {
            return false;
        }
        if let Some((op2, body2)) = wait_and_read_ipc_frame(pipe, 250)
            && is_ipc_error_or_close(op2, &body2)
        {
            return false;
        }
    }
    drain_ipc_replies_nonblocking(pipe)
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
    let Ok(mut pipe_guard) = pipe_cell().lock() else {
        return status;
    };

    if !enabled {
        let clear_frame = encode_ipc_frame(1, &build_clear_activity_payload(pid));
        if let Some(ref mut existing) = *pipe_guard {
            let _ = drain_ipc_replies_nonblocking(&mut existing.file);
            let _ = existing.file.write_all(&clear_frame);
            let _ = existing.file.flush();
            let _ = wait_and_read_ipc_frame(&mut existing.file, 150);
        }
        *pipe_guard = None;
        return status;
    }

    if let Some(ref mut existing) = *pipe_guard {
        if drain_ipc_replies_nonblocking(&mut existing.file)
            && send_activity_on_pipe(
                &mut existing.file,
                pid,
                mc_version,
                details,
                state_label,
                started_epoch_secs,
                loader_label,
                java_label,
            )
        {
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

        let Some((hs_op, hs_body)) = wait_and_read_ipc_frame(&mut pipe, 1200) else {
            continue;
        };
        if is_ipc_error_or_close(hs_op, &hs_body) {
            continue;
        }

        if send_activity_on_pipe(
            &mut pipe,
            pid,
            mc_version,
            details,
            state_label,
            started_epoch_secs,
            loader_label,
            java_label,
        ) {
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
        assert!(!payload.contains("basalt_logo"));

        let no_buttons = build_activity_payload_inner(
            4242,
            "Playing Minecraft 1.21.4 (Fabric)",
            "In-Game · Player",
            1700000000,
            "1.21.4",
            Some("Fabric"),
            Some("Java 21"),
            false,
        );
        assert!(no_buttons.contains("SET_ACTIVITY"));
        assert!(!no_buttons.contains("buttons"));

        assert_eq!(
            infer_loader_and_java_labels("1.16.5", "Playing Minecraft 1.16.5 (Forge)"),
            ("Forge", "Java 8/17")
        );
        assert!(is_ipc_error_or_close(2, "{}"));
        assert!(is_ipc_error_or_close(1, r#"{"evt":"ERROR","data":{"code":4000}}"#));
        assert!(!is_ipc_error_or_close(1, r#"{"evt":"READY","cmd":"DISPATCH"}"#));
    }
}
