#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use basalt::assets::{
    default_skin_png_bytes, generate_default_skin, load_skin_from_file, resolve_skin_is_slim,
};
use basalt::config::{
    AccentTone, AppConfig, AppPaths, CornerStyle, GraphicsSettings, LayoutMode, LoaderKind,
    SavedServer, SkinModel, ThemeMode, UiLanguage, normalize_hex_color, normalize_mc_version,
};
use basalt::discord_rpc::{
    BASALT_GITHUB_URL, DiscordRpcStatus, queue_discord_activity_update, shutdown_discord_rpc,
};
use basalt::hardware::HardwareProfile;
use basalt::loaders::{
    LoaderEnvStatus, ensure_fabric_version_ready, ensure_forge_version_ready, forge_profile_dir,
    inspect_loader_environment, loader_mods_dir, shared_resourcepacks_dir, shared_shaderpacks_dir,
    spawn_modded_client, sync_profile_shared_files,
};
use basalt::modrinth::{
    InstallOutcome, InstalledLibraryItem, LibraryCounts, LibraryKind, ModrinthSearchResponse,
    count_library_items, delete_library_item, import_library_file, install_modrinth_project,
    install_shader_preset_bundle, list_library_items, search_modrinth, toggle_library_item,
};
use basalt::options_txt::{
    ensure_launch_options, read_minecraft_options_into, sync_minecraft_options,
};
use basalt::protocol::{
    LocalHandshakeResult, ServerPingInfo, ping_server_1_21_4, probe_local_dev_login_1_21_4,
};
use basalt::saves::{MinecraftSaveSummary, delete_save as remove_save_dir, list_saves as scan_saves, resolve_save_dir};
use basalt::sounds::{
    MinecraftSoundsDto, default_local_asset_roots, fetch_missing_minecraft_sounds,
    load_local_minecraft_sounds,
};
use basalt::vanilla::{
    BASALT_SKIN_PACK_NAME, ClientOptimizationReport, CurlCapabilities, VanillaEnvStatus,
    VanillaLaunchConfig, compute_file_sha256_hex, discover_java_for_loader,
    ensure_vanilla_version_ready_with_threads,
    inspect_vanilla_environment_for_version, migrate_legacy_client_gradle_saves, resolve_curl_exe,
    resolve_tar_exe, resolve_vanilla_dir, run_client_and_network_optimization, spawn_vanilla_client,
    summarize_process_failure, write_basalt_skin_resource_pack_for,
};

#[derive(Debug, Clone, Serialize)]
struct RunningSessionDto {
    pid: u32,
    kind: String,
    label: String,
}

#[derive(Debug, Clone, Serialize)]
struct BootstrapDto {
    config: AppConfig,
    saves: Vec<MinecraftSaveSummary>,
    hardware: HardwareProfile,
    vanilla: VanillaEnvStatus,
    loader_status: LoaderEnvStatus,
    library_counts: LibraryCounts,
    discord_rpc_status: DiscordRpcStatus,
    skin_rgba: Vec<u8>,
    skin_is_slim: bool,
    running_session: Option<RunningSessionDto>,
    hardware_notice: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SkinResultDto {
    rgba: Vec<u8>,
    path: Option<String>,
    skin_model: SkinModel,
    skin_is_slim: bool,
    note: String,
}

#[derive(Debug, Clone, Serialize)]
struct SkinModelResultDto {
    skin_model: SkinModel,
    skin_is_slim: bool,
}

#[derive(Debug, Clone, Serialize)]
struct OptimizeResultDto {
    graphics: GraphicsSettings,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct LaunchLogPayload {
    line: String,
    is_err: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SessionEndedPayload {
    kind: String,
    exit_code: Option<i32>,
    error: Option<String>,
    saves: Vec<MinecraftSaveSummary>,
    vanilla: VanillaEnvStatus,
    loader_status: LoaderEnvStatus,
    library_counts: LibraryCounts,
}

#[derive(Debug, Clone, Serialize)]
struct VersionChangeDto {
    config: AppConfig,
    vanilla: VanillaEnvStatus,
    loader_status: LoaderEnvStatus,
    library_counts: LibraryCounts,
    discord_rpc_status: DiscordRpcStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallActionDto {
    config: AppConfig,
    outcome: InstallOutcome,
    loader_status: LoaderEnvStatus,
    library_counts: LibraryCounts,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportLibraryDto {
    file_name: String,
    library_counts: LibraryCounts,
}

#[derive(Debug, Deserialize)]
struct TurboSettingsInput {
    discord_rpc: bool,
    download_threads: u8,
    network_boost: bool,
    process_boost: bool,
}

#[derive(Debug, Clone, Serialize)]
struct TurboSettingsDto {
    config: AppConfig,
    discord_rpc_status: DiscordRpcStatus,
}

#[derive(Debug, Deserialize)]
struct ProfileInput {
    username: String,
    theme: ThemeMode,
    #[serde(default)]
    ui_accent: AccentTone,
    #[serde(default)]
    custom_accent_hex: Option<String>,
    #[serde(default)]
    ui_corner: CornerStyle,
    #[serde(default)]
    ui_layout: LayoutMode,
    ui_compact: bool,
    ui_reduced_motion: bool,
    #[serde(default)]
    ui_sounds: bool,
    #[serde(default)]
    ui_language: Option<UiLanguage>,
    #[serde(default)]
    mc_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LaunchInput {
    mode: String,
    save_id: Option<String>,
    server_address: Option<String>,
    #[serde(default)]
    mc_version: Option<String>,
}

struct AppState {
    paths: AppPaths,
    vanilla_dir: PathBuf,
    run_dir: PathBuf,
    config: Mutex<AppConfig>,
    hardware: Mutex<HardwareProfile>,
    hardware_notice: Mutex<Option<String>>,
    running: Arc<Mutex<Option<RunningSessionDto>>>,
    discord: Arc<Mutex<DiscordRpcStatus>>,
    started_epoch_secs: u64,
    sounds_fetching: Arc<AtomicBool>,
}

fn spawn_discord_sync(
    discord_arc: Arc<Mutex<DiscordRpcStatus>>,
    enabled: bool,
    mc_version: String,
    details: String,
    state_label: String,
    started_epoch_secs: u64,
) {
    if let Ok(mut guard) = discord_arc.lock() {
        guard.enabled = enabled;
        guard.details = details.clone();
        guard.state_label = state_label.clone();
        guard.started_epoch_secs = started_epoch_secs;
    }
    queue_discord_activity_update(
        discord_arc,
        enabled,
        mc_version,
        details,
        state_label,
        started_epoch_secs,
    );
}

impl AppState {
    fn initialize() -> Result<Self, String> {
        let paths = AppPaths::resolve();
        paths.ensure_dirs()?;

        let vanilla_dir = resolve_vanilla_dir(&paths.root_dir);
        fs::create_dir_all(&vanilla_dir)
            .map_err(|e| format!("Failed to create Vanilla cache directory: {e}"))?;
        let run_dir = paths.root_dir.join("run");
        let saves_dir = run_dir.join("saves");
        fs::create_dir_all(&saves_dir)
            .map_err(|e| format!("Failed to create Minecraft saves directory: {e}"))?;
        migrate_legacy_client_gradle_saves(&paths.root_dir, &saves_dir);

        let first_run = !paths.config_file.exists();
        let mut config = AppConfig::load(&paths.config_file);
        let hardware = HardwareProfile::detect_headless();
        let mut hardware_notice = None;

        if first_run {
            let (recommended, reason) = hardware.recommend_settings(&config.graphics);
            config.graphics = recommended;
            read_minecraft_options_into(&run_dir, &mut config.graphics);
            if let Err(err) = config.save(&paths.config_file) {
                hardware_notice = Some(format!(
                    "{reason} (Warning: failed to save initial calibration to disk: {err})"
                ));
            } else {
                hardware_notice = Some(reason);
            }
        } else if read_minecraft_options_into(&run_dir, &mut config.graphics) {
            let _ = config.save(&paths.config_file);
        }

        if let Some(ref skin_path) = config.skin_file
            && !skin_path.exists()
        {
            config.skin_file = None;
            let _ = config.save(&paths.config_file);
        }

        let started_epoch_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let initial_details = format!("Minecraft {} (Vanilla)", config.mc_version);
        let initial_state = format!("Basalt Launcher · {}", config.username);
        let discord = Arc::new(Mutex::new(DiscordRpcStatus {
            enabled: config.discord_rpc,
            connected: false,
            pipe_name: "Standby (WebRTC/IPC)".to_string(),
            details: initial_details.clone(),
            state_label: initial_state.clone(),
            started_epoch_secs,
            github_url: BASALT_GITHUB_URL.to_string(),
        }));
        spawn_discord_sync(
            Arc::clone(&discord),
            config.discord_rpc,
            config.mc_version.clone(),
            initial_details,
            initial_state,
            started_epoch_secs,
        );

        Ok(Self {
            paths,
            vanilla_dir,
            run_dir,
            config: Mutex::new(config),
            hardware: Mutex::new(hardware),
            hardware_notice: Mutex::new(hardware_notice),
            running: Arc::new(Mutex::new(None)),
            discord,
            started_epoch_secs,
            sounds_fetching: Arc::new(AtomicBool::new(false)),
        })
    }

    fn saves_dir(&self) -> PathBuf {
        self.run_dir.join("saves")
    }

    fn run_dir(&self) -> PathBuf {
        self.run_dir.clone()
    }
}

fn validate_username(raw: &str) -> Result<String, String> {
    let clean = raw.trim();
    if clean.is_empty() || clean.len() > 16 {
        return Err("Username must be between 1 and 16 characters.".to_string());
    }
    if !clean
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(
            "Username may only contain ASCII letters, digits, '_' or '-'.".to_string(),
        );
    }
    Ok(clean.to_string())
}

fn validate_graphics(mut gfx: GraphicsSettings) -> Result<GraphicsSettings, String> {
    if !gfx.fov_degrees.is_finite() || !gfx.mouse_sensitivity.is_finite() {
        return Err("Invalid numeric value in graphics settings.".to_string());
    }
    gfx.render_distance = gfx.render_distance.clamp(2, 32);
    gfx.simulation_distance = gfx.simulation_distance.clamp(5, 32);
    gfx.fov_degrees = gfx.fov_degrees.clamp(30.0, 110.0);
    gfx.max_fps = gfx.max_fps.clamp(10, 260);
    gfx.mouse_sensitivity = gfx.mouse_sensitivity.clamp(0.05, 1.0);
    gfx.jvm_ram_mb = gfx.jvm_ram_mb.clamp(1024, 16384);
    Ok(gfx)
}

fn validate_server_address(raw: &str) -> Result<String, String> {
    let clean = raw.trim();
    if clean.is_empty() || clean.len() > 128 {
        return Err("Server address cannot be empty or longer than 128 characters.".to_string());
    }
    if clean.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Server address cannot contain spaces or control characters.".to_string());
    }
    Ok(clean.to_string())
}

fn load_active_skin_image(skin_file: Option<&Path>) -> image::RgbaImage {
    if let Some(path) = skin_file
        && let Ok(img) = load_skin_from_file(path)
    {
        return img;
    }
    generate_default_skin()
}

fn load_active_skin_png_bytes(skin_file: Option<&Path>) -> Vec<u8> {
    if let Some(path) = skin_file
        && let Ok(img) = load_skin_from_file(path)
    {
        let mut out = Vec::new();
        if img
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .is_ok()
        {
            return out;
        }
    }
    default_skin_png_bytes().to_vec()
}

struct SessionDirs<'a> {
    vanilla: &'a Path,
    run: &'a Path,
    saves: &'a Path,
    runtime: &'a Path,
    version: &'a str,
    loader: LoaderKind,
}

fn loader_display_name(loader: LoaderKind) -> &'static str {
    match loader {
        LoaderKind::Vanilla => "Vanilla",
        LoaderKind::Fabric => "Fabric",
        LoaderKind::Forge => "Forge",
    }
}

fn compute_loader_status_and_counts(
    vanilla_dir: &Path,
    run_dir: &Path,
    mc_version: &str,
    loader: LoaderKind,
) -> (LoaderEnvStatus, LibraryCounts) {
    let status = inspect_loader_environment(vanilla_dir, run_dir, mc_version, loader);
    let mods_dir = loader_mods_dir(vanilla_dir, run_dir, loader, mc_version);
    let counts = count_library_items(
        &mods_dir,
        &shared_shaderpacks_dir(run_dir),
        &shared_resourcepacks_dir(run_dir),
    );
    (status, counts)
}

fn resolve_library_dir_for(
    vanilla_dir: &Path,
    run_dir: &Path,
    kind: LibraryKind,
    loader: LoaderKind,
    mc_version: &str,
) -> PathBuf {
    match kind {
        LibraryKind::Mod => loader_mods_dir(vanilla_dir, run_dir, loader, mc_version),
        LibraryKind::Shader => shared_shaderpacks_dir(run_dir),
        LibraryKind::ResourcePack => shared_resourcepacks_dir(run_dir),
    }
}

fn emit_session_ended(
    app: &tauri::AppHandle,
    kind: String,
    exit_code: Option<i32>,
    error: Option<String>,
    dirs: &SessionDirs<'_>,
) {
    let saves = scan_saves(dirs.saves).unwrap_or_default();
    let vanilla = inspect_vanilla_environment_for_version(
        dirs.vanilla,
        dirs.run,
        Some(dirs.runtime),
        dirs.version,
    );
    let (loader_status, library_counts) =
        compute_loader_status_and_counts(dirs.vanilla, dirs.run, dirs.version, dirs.loader);
    let _ = app.emit(
        "basalt://session-ended",
        SessionEndedPayload {
            kind,
            exit_code,
            error,
            saves,
            vanilla,
            loader_status,
            library_counts,
        },
    );
}

fn spawn_log_reader<R: std::io::Read + Send + 'static>(
    stream: Option<R>,
    is_err: bool,
    app: tauri::AppHandle,
    tail: Arc<Mutex<VecDeque<String>>>,
    file_writer: Arc<Mutex<Option<BufWriter<File>>>>,
) -> Option<thread::JoinHandle<()>> {
    stream.map(|s| {
        thread::spawn(move || {
            let reader = BufReader::new(s);
            for line in reader.lines().map_while(Result::ok) {
                let trimmed = line.trim_end().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(mut guard) = file_writer.lock()
                    && let Some(ref mut w) = *guard
                {
                    let prefix = if is_err { "[ERR] " } else { "" };
                    let _ = writeln!(w, "{prefix}{trimmed}");
                    let _ = w.flush();
                }
                if let Ok(mut buf) = tail.lock() {
                    if buf.len() >= 240 {
                        buf.pop_front();
                    }
                    buf.push_back(trimmed.clone());
                }
                let _ = app.emit(
                    "basalt://launch-log",
                    LaunchLogPayload {
                        line: trimmed,
                        is_err,
                    },
                );
            }
        })
    })
}

fn monitor_vanilla_child_blocking(
    app: &tauri::AppHandle,
    mut child: Child,
    log_file: &Path,
) -> (Option<i32>, Option<String>) {
    let tail = Arc::new(Mutex::new(VecDeque::<String>::with_capacity(240)));
    if let Some(parent) = log_file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let log_created = File::create(log_file).ok().map(BufWriter::new);
    let log_available = log_created.is_some();
    let file_writer = Arc::new(Mutex::new(log_created));

    let out_handle = spawn_log_reader(
        child.stdout.take(),
        false,
        app.clone(),
        Arc::clone(&tail),
        Arc::clone(&file_writer),
    );
    let err_handle = spawn_log_reader(
        child.stderr.take(),
        true,
        app.clone(),
        Arc::clone(&tail),
        Arc::clone(&file_writer),
    );

    let status_res = child.wait();
    if let Some(h) = out_handle {
        let _ = h.join();
    }
    if let Some(h) = err_handle {
        let _ = h.join();
    }
    if let Ok(mut guard) = file_writer.lock()
        && let Some(mut w) = guard.take()
    {
        let _ = w.flush();
    }

    match status_res {
        Ok(status) => {
            let code = status.code();
            if status.success() {
                (code, None)
            } else {
                let captured: Vec<String> = tail
                    .lock()
                    .ok()
                    .map(|buf| buf.iter().cloned().collect())
                    .unwrap_or_default();
                let msg = summarize_process_failure(
                    &captured,
                    code,
                    log_available.then_some(log_file),
                );
                (code, Some(msg))
            }
        }
        Err(e) => (None, Some(format!("Process monitor error: {e}"))),
    }
}

#[tauri::command]
fn get_bootstrap_state(state: tauri::State<AppState>) -> Result<BootstrapDto, String> {
    let config = {
        let mut guard = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        if read_minecraft_options_into(&state.run_dir, &mut guard.graphics) {
            let _ = guard.save(&state.paths.config_file);
        }
        guard.clone()
    };
    let hardware = state
        .hardware
        .lock()
        .map_err(|_| "Failed to unlock hardware profile".to_string())?
        .clone();
    let hardware_notice = state
        .hardware_notice
        .lock()
        .map_err(|_| "Failed to unlock notice state".to_string())?
        .take();
    let running_session = state
        .running
        .lock()
        .map_err(|_| "Failed to unlock session state".to_string())?
        .clone();

    let saves = scan_saves(&state.saves_dir())?;
    let vanilla = inspect_vanilla_environment_for_version(
        &state.vanilla_dir,
        &state.run_dir,
        Some(&state.paths.runtime_dir),
        &config.mc_version,
    );
    let (loader_status, library_counts) = compute_loader_status_and_counts(
        &state.vanilla_dir,
        &state.run_dir,
        &config.mc_version,
        config.mod_loader,
    );
    let discord_rpc_status = state
        .discord
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default();
    let skin_img = load_active_skin_image(config.skin_file.as_deref());
    let skin_is_slim = resolve_skin_is_slim(config.skin_model, &skin_img);

    Ok(BootstrapDto {
        config,
        saves,
        hardware,
        vanilla,
        loader_status,
        library_counts,
        discord_rpc_status,
        skin_rgba: skin_img.into_raw(),
        skin_is_slim,
        running_session,
        hardware_notice,
    })
}

#[tauri::command]
fn list_saves(state: tauri::State<AppState>) -> Result<Vec<MinecraftSaveSummary>, String> {
    scan_saves(&state.saves_dir())
}

#[tauri::command]
fn select_save(state: tauri::State<AppState>, save_id: Option<String>) -> Result<(), String> {
    if let Some(ref id) = save_id {
        let _ = resolve_save_dir(&state.saves_dir(), id)?;
    }
    let mut config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?;
    config.last_save_id = save_id;
    config.save(&state.paths.config_file)?;
    Ok(())
}

#[tauri::command]
fn delete_save(
    state: tauri::State<AppState>,
    save_id: String,
) -> Result<Vec<MinecraftSaveSummary>, String> {
    {
        let running = state
            .running
            .lock()
            .map_err(|_| "Failed to unlock session state".to_string())?;
        if running.is_some() {
            return Err(
                "Cannot delete a world while Minecraft or setup is running; wait or close the game first.".to_string(),
            );
        }
    }

    remove_save_dir(&state.saves_dir(), &save_id)?;

    let saves = scan_saves(&state.saves_dir())?;
    let mut config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?;
    if config.last_save_id.as_deref() == Some(save_id.as_str()) {
        config.last_save_id = saves.first().map(|s| s.id.clone());
        config.save(&state.paths.config_file)?;
    }

    Ok(saves)
}

#[tauri::command]
fn save_profile_settings(
    state: tauri::State<AppState>,
    input: ProfileInput,
) -> Result<AppConfig, String> {
    let username = validate_username(&input.username)?;
    let clean_hex = match input.custom_accent_hex.as_deref() {
        Some(raw) if !raw.trim().is_empty() => {
            let normalized = normalize_hex_color(raw)
                .ok_or_else(|| "Custom accent color must be a valid #RRGGBB hex code.".to_string())?;
            Some(normalized)
        }
        _ => None,
    };

    let mut config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?;
    config.username = username;
    config.theme = input.theme;
    config.ui_accent = input.ui_accent;
    if let Some(hex) = clean_hex {
        config.custom_accent_hex = Some(hex);
    }
    config.ui_corner = input.ui_corner;
    config.ui_layout = input.ui_layout;
    config.ui_compact = input.ui_compact;
    config.ui_reduced_motion = input.ui_reduced_motion;
    config.config_version = 1;
    config.ui_sounds = input.ui_sounds;
    if let Some(lang) = input.ui_language {
        config.ui_language = lang;
    }
    if let Some(ref ver) = input.mc_version {
        config.mc_version = normalize_mc_version(ver);
    }
    config.save(&state.paths.config_file)?;
    Ok(config.clone())
}

fn build_version_change_dto(state: &AppState, config_snapshot: AppConfig) -> VersionChangeDto {
    let clean_ver = config_snapshot.mc_version.clone();
    let loader = config_snapshot.mod_loader;
    let vanilla = inspect_vanilla_environment_for_version(
        &state.vanilla_dir,
        &state.run_dir,
        Some(&state.paths.runtime_dir),
        &clean_ver,
    );
    let (loader_status, library_counts) = compute_loader_status_and_counts(
        &state.vanilla_dir,
        &state.run_dir,
        &clean_ver,
        loader,
    );
    let details = format!("Minecraft {clean_ver} ({})", loader_display_name(loader));
    let state_label = format!("Basalt Launcher · {}", config_snapshot.username);
    spawn_discord_sync(
        Arc::clone(&state.discord),
        config_snapshot.discord_rpc,
        clean_ver,
        details,
        state_label,
        state.started_epoch_secs,
    );
    let discord_rpc_status = state
        .discord
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default();
    VersionChangeDto {
        config: config_snapshot,
        vanilla,
        loader_status,
        library_counts,
        discord_rpc_status,
    }
}

#[tauri::command]
fn set_mc_version(
    state: tauri::State<AppState>,
    version: String,
) -> Result<VersionChangeDto, String> {
    let clean_ver = normalize_mc_version(&version);
    let config_snapshot = {
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        config.mc_version = clean_ver;
        config.save(&state.paths.config_file)?;
        config.clone()
    };
    Ok(build_version_change_dto(&state, config_snapshot))
}

#[tauri::command]
fn set_mod_loader(
    state: tauri::State<AppState>,
    loader: LoaderKind,
) -> Result<VersionChangeDto, String> {
    let config_snapshot = {
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        config.mod_loader = loader;
        config.save(&state.paths.config_file)?;
        config.clone()
    };
    Ok(build_version_change_dto(&state, config_snapshot))
}

#[tauri::command]
async fn search_modrinth_catalog(
    state: tauri::State<'_, AppState>,
    query: String,
    kind: LibraryKind,
    loader: LoaderKind,
    sort: String,
    limit: u32,
    category: Option<String>,
) -> Result<ModrinthSearchResponse, String> {
    let mc_version = state
        .config
        .lock()
        .map(|c| c.mc_version.clone())
        .unwrap_or_else(|_| "1.21.4".to_string());
    let target_dir = resolve_library_dir_for(&state.vanilla_dir, &state.run_dir, kind, loader, &mc_version);

    tauri::async_runtime::spawn_blocking(move || {
        search_modrinth(
            &query,
            kind,
            loader,
            &mc_version,
            &sort,
            limit,
            category.as_deref(),
            &target_dir,
        )
    })
    .await
    .map_err(|e| format!("Modrinth catalog search task failed: {e}"))?
}

#[tauri::command]
async fn install_modrinth_item(
    state: tauri::State<'_, AppState>,
    project_id: String,
    kind: LibraryKind,
    loader: LoaderKind,
) -> Result<InstallActionDto, String> {
    let config_snapshot = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?
        .clone();
    let mc_version = config_snapshot.mc_version.clone();
    let dl_threads = config_snapshot.download_threads;
    let target_dir = resolve_library_dir_for(&state.vanilla_dir, &state.run_dir, kind, loader, &mc_version);
    let temp_dir = state.vanilla_dir.join("cache").join("tmp");

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        install_modrinth_project(
            &project_id,
            kind,
            loader,
            &mc_version,
            &target_dir,
            &temp_dir,
            dl_threads,
        )
    })
    .await
    .map_err(|e| format!("Modrinth install task failed: {e}"))??;

    let (loader_status, library_counts) = compute_loader_status_and_counts(
        &state.vanilla_dir,
        &state.run_dir,
        &config_snapshot.mc_version,
        loader,
    );
    Ok(InstallActionDto {
        config: config_snapshot,
        outcome,
        loader_status,
        library_counts,
    })
}

#[tauri::command]
async fn install_shader_bundle(
    state: tauri::State<'_, AppState>,
    preset_id: String,
) -> Result<InstallActionDto, String> {
    let target_loader = match preset_id.as_str() {
        "fabric_shaders" => LoaderKind::Fabric,
        "forge_shaders" => LoaderKind::Forge,
        _ => return Err(format!("Unsupported shader bundle '{preset_id}'")),
    };
    if state.running.lock().is_ok_and(|guard| guard.is_some()) {
        return Err("Another launch or verification task is already running.".to_string());
    }

    let (mc_version, dl_threads) = {
        let config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        (config.mc_version.clone(), config.download_threads)
    };

    let mods_dir = loader_mods_dir(&state.vanilla_dir, &state.run_dir, target_loader, &mc_version);
    let shaderpacks_dir = shared_shaderpacks_dir(&state.run_dir);
    let temp_dir = state.vanilla_dir.join("cache").join("tmp");

    let outcome = tauri::async_runtime::spawn_blocking(move || {
        install_shader_preset_bundle(
            &preset_id,
            &mc_version,
            &mods_dir,
            &shaderpacks_dir,
            &temp_dir,
            dl_threads,
        )
    })
    .await
    .map_err(|e| format!("Shader bundle installation failed: {e}"))??;

    let config_snapshot = {
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        config.mod_loader = target_loader;
        config.save(&state.paths.config_file)?;
        config.clone()
    };

    let (loader_status, library_counts) = compute_loader_status_and_counts(
        &state.vanilla_dir,
        &state.run_dir,
        &config_snapshot.mc_version,
        target_loader,
    );
    Ok(InstallActionDto {
        config: config_snapshot,
        outcome,
        loader_status,
        library_counts,
    })
}

#[tauri::command]
fn list_library_items_cmd(
    state: tauri::State<AppState>,
    kind: LibraryKind,
    loader: LoaderKind,
) -> Result<Vec<InstalledLibraryItem>, String> {
    let mc_version = state
        .config
        .lock()
        .map(|c| c.mc_version.clone())
        .unwrap_or_else(|_| "1.21.4".to_string());
    let dir = resolve_library_dir_for(&state.vanilla_dir, &state.run_dir, kind, loader, &mc_version);
    let _ = fs::create_dir_all(&dir);
    Ok(list_library_items(&dir, kind))
}

#[tauri::command]
fn toggle_library_item_cmd(
    state: tauri::State<AppState>,
    kind: LibraryKind,
    loader: LoaderKind,
    file_name: String,
) -> Result<LibraryCounts, String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?
        .clone();
    let dir = resolve_library_dir_for(
        &state.vanilla_dir,
        &state.run_dir,
        kind,
        loader,
        &config.mc_version,
    );
    toggle_library_item(&dir, &file_name)?;
    let (_, counts) = compute_loader_status_and_counts(
        &state.vanilla_dir,
        &state.run_dir,
        &config.mc_version,
        loader,
    );
    Ok(counts)
}

#[tauri::command]
fn delete_library_item_cmd(
    state: tauri::State<AppState>,
    kind: LibraryKind,
    loader: LoaderKind,
    file_name: String,
) -> Result<LibraryCounts, String> {
    let config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?
        .clone();
    let dir = resolve_library_dir_for(
        &state.vanilla_dir,
        &state.run_dir,
        kind,
        loader,
        &config.mc_version,
    );
    delete_library_item(&dir, &file_name)?;
    let (_, counts) = compute_loader_status_and_counts(
        &state.vanilla_dir,
        &state.run_dir,
        &config.mc_version,
        loader,
    );
    Ok(counts)
}

#[tauri::command]
async fn pick_and_import_library_file(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    kind: LibraryKind,
    loader: LoaderKind,
) -> Result<Option<ImportLibraryDto>, String> {
    let (title, filter_label, exts): (&str, &str, Vec<&str>) = match kind {
        LibraryKind::Mod => ("Select Mod or OptiFine JAR", "Minecraft Mod / OptiFine", vec!["jar"]),
        LibraryKind::Shader => ("Select Shader Pack ZIP", "Shader Pack ZIP", vec!["zip"]),
        LibraryKind::ResourcePack => ("Select Resource Pack ZIP", "Resource Pack ZIP", vec!["zip"]),
    };
    let title_owned = title.to_string();
    let filter_owned = filter_label.to_string();
    let exts_owned: Vec<String> = exts.into_iter().map(ToString::to_string).collect();
    let parent_win = app.get_webview_window("main");

    let picked = tauri::async_runtime::spawn_blocking(move || {
        let refs: Vec<&str> = exts_owned.iter().map(String::as_str).collect();
        let mut dialog = rfd::FileDialog::new()
            .set_title(&title_owned)
            .add_filter(&filter_owned, &refs);
        if let Some(ref win) = parent_win {
            dialog = dialog.set_parent(win);
        }
        dialog.pick_file()
    })
    .await
    .map_err(|e| format!("File dialog failed: {e}"))?;

    let Some(src_path) = picked else {
        return Ok(None);
    };

    let config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?
        .clone();
    let target_dir = resolve_library_dir_for(
        &state.vanilla_dir,
        &state.run_dir,
        kind,
        loader,
        &config.mc_version,
    );
    let file_name = tauri::async_runtime::spawn_blocking(move || {
        import_library_file(&src_path, &target_dir, kind)
    })
    .await
    .map_err(|e| format!("Import task failed: {e}"))??;

    let (_, library_counts) = compute_loader_status_and_counts(
        &state.vanilla_dir,
        &state.run_dir,
        &config.mc_version,
        loader,
    );
    Ok(Some(ImportLibraryDto {
        file_name,
        library_counts,
    }))
}

#[tauri::command]
fn open_library_folder(
    state: tauri::State<AppState>,
    kind: LibraryKind,
    loader: LoaderKind,
) -> Result<(), String> {
    let mc_version = state
        .config
        .lock()
        .map(|c| c.mc_version.clone())
        .unwrap_or_else(|_| "1.21.4".to_string());
    let dir = resolve_library_dir_for(&state.vanilla_dir, &state.run_dir, kind, loader, &mc_version);
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to prepare library folder {}: {e}", dir.display()))?;
    #[cfg(windows)]
    {
        Command::new("explorer.exe")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("Failed to open library folder: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(format!("Library directory: {}", dir.display()))
    }
}

#[tauri::command]
fn save_turbo_settings(
    state: tauri::State<AppState>,
    input: TurboSettingsInput,
) -> Result<TurboSettingsDto, String> {
    let config_snapshot = {
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        config.discord_rpc = input.discord_rpc;
        config.download_threads = input.download_threads;
        config.network_boost = input.network_boost;
        config.process_boost = input.process_boost;
        config.sanitize();
        config.save(&state.paths.config_file)?;
        config.clone()
    };
    let details = format!(
        "Minecraft {} ({})",
        config_snapshot.mc_version,
        loader_display_name(config_snapshot.mod_loader)
    );
    let state_label = format!("Basalt Launcher · {}", config_snapshot.username);
    spawn_discord_sync(
        Arc::clone(&state.discord),
        config_snapshot.discord_rpc,
        config_snapshot.mc_version.clone(),
        details,
        state_label,
        state.started_epoch_secs,
    );
    let discord_rpc_status = state
        .discord
        .lock()
        .map(|d| d.clone())
        .unwrap_or_default();
    Ok(TurboSettingsDto {
        config: config_snapshot,
        discord_rpc_status,
    })
}

#[tauri::command]
async fn optimize_client_now(
    state: tauri::State<'_, AppState>,
) -> Result<ClientOptimizationReport, String> {
    let vanilla_dir = state.vanilla_dir.clone();
    let (network_boost, process_boost) = {
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        config.network_boost = true;
        config.process_boost = true;
        let _ = config.save(&state.paths.config_file);
        (config.network_boost, config.process_boost)
    };
    tauri::async_runtime::spawn_blocking(move || {
        run_client_and_network_optimization(&vanilla_dir, network_boost, process_boost)
    })
    .await
    .map_err(|e| format!("Optimization worker error: {e}"))
}

#[tauri::command]
fn open_github_page() -> Result<(), String> {
    #[cfg(windows)]
    {
        Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", BASALT_GITHUB_URL])
            .spawn()
            .map_err(|e| format!("Failed to open GitHub page: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(format!("GitHub repository: {BASALT_GITHUB_URL}"))
    }
}

#[tauri::command]
fn open_game_folder(state: tauri::State<AppState>) -> Result<(), String> {
    let run_dir = state.run_dir();
    fs::create_dir_all(&run_dir)
        .map_err(|e| format!("Failed to prepare game directory {}: {e}", run_dir.display()))?;
    #[cfg(windows)]
    {
        Command::new("explorer.exe")
            .arg(&run_dir)
            .spawn()
            .map_err(|e| format!("Failed to open game directory: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(format!("Game directory: {}", run_dir.display()))
    }
}

#[tauri::command]
fn set_fullscreen(window: tauri::Window, fullscreen: bool) -> Result<(), String> {
    window
        .set_fullscreen(fullscreen)
        .map_err(|e| format!("Failed to set fullscreen mode: {e}"))
}

#[tauri::command]
fn window_minimize(window: tauri::Window) -> Result<(), String> {
    window
        .minimize()
        .map_err(|e| format!("Failed to minimize window: {e}"))
}

#[tauri::command]
fn window_toggle_maximize(window: tauri::Window) -> Result<bool, String> {
    let is_max = window.is_maximized().map_err(|e| e.to_string())?;
    if is_max {
        window
            .unmaximize()
            .map_err(|e| format!("Failed to restore window size: {e}"))?;
        Ok(false)
    } else {
        window
            .maximize()
            .map_err(|e| format!("Failed to maximize window: {e}"))?;
        Ok(true)
    }
}

#[tauri::command]
fn window_is_maximized(window: tauri::Window) -> Result<bool, String> {
    window.is_maximized().map_err(|e| e.to_string())
}

#[tauri::command]
fn window_close(window: tauri::Window) -> Result<(), String> {
    window
        .close()
        .map_err(|e| format!("Failed to close window: {e}"))
}

#[tauri::command]
fn window_start_drag(window: tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct WinPoint {
            x: i32,
            y: i32,
        }
        unsafe extern "system" {
            fn ReleaseCapture() -> i32;
            fn GetCursorPos(lp_point: *mut WinPoint) -> i32;
            fn PostMessageW(
                hwnd: *mut core::ffi::c_void,
                msg: u32,
                wparam: usize,
                lparam: isize,
            ) -> i32;
        }
        const WM_NCLBUTTONDOWN: u32 = 0x00A1;
        const HTCAPTION: usize = 2;

        if let Ok(hwnd) = window.hwnd() {
            let hwnd_addr = hwnd.0 as usize;
            return window
                .run_on_main_thread(move || unsafe {
                    // Release capture on the GUI thread and pack real cursor screen coordinates
                    // into LPARAM (MAKELPARAM), bypassing tao's dangling &points LPARAM bug.
                    ReleaseCapture();
                    let mut pt = WinPoint { x: 0, y: 0 };
                    let lparam = if GetCursorPos(&mut pt) != 0 {
                        (((pt.x as i16 as u16) as u32) | (((pt.y as i16 as u16) as u32) << 16))
                            as i32 as isize
                    } else {
                        0
                    };
                    PostMessageW(
                        hwnd_addr as *mut core::ffi::c_void,
                        WM_NCLBUTTONDOWN,
                        HTCAPTION,
                        lparam,
                    );
                })
                .map_err(|e| format!("Failed to drag window: {e}"));
        }
    }

    window
        .start_dragging()
        .map_err(|e| format!("Failed to drag window: {e}"))
}

fn spawn_missing_sounds_fetch(
    app: Option<tauri::AppHandle>,
    sounds_dir: PathBuf,
    guard: Arc<AtomicBool>,
) {
    if guard.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || {
        let fetched = fetch_missing_minecraft_sounds(&sounds_dir);
        guard.store(false, Ordering::SeqCst);
        if fetched > 0
            && let Some(app_handle) = app
        {
            let _ = app_handle.emit("basalt://minecraft-sounds-ready", fetched);
        }
    });
}

#[tauri::command]
fn load_minecraft_sounds(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<MinecraftSoundsDto, String> {
    let sounds_dir = state.paths.root_dir.join("sounds");
    let roots = default_local_asset_roots(Some(&state.vanilla_dir));
    let dto = load_local_minecraft_sounds(&sounds_dir, &roots);
    if !dto.missing_keys.is_empty() {
        spawn_missing_sounds_fetch(
            Some(app),
            sounds_dir,
            Arc::clone(&state.sounds_fetching),
        );
    }
    Ok(dto)
}

#[tauri::command]
fn save_graphics_settings(
    state: tauri::State<AppState>,
    graphics: GraphicsSettings,
) -> Result<AppConfig, String> {
    let validated = validate_graphics(graphics)?;
    let snapshot = {
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        config.graphics = validated;
        config.save(&state.paths.config_file)?;
        config.clone()
    };
    let _ = sync_minecraft_options(&state.run_dir(), &snapshot.graphics);
    Ok(snapshot)
}

#[tauri::command]
fn auto_optimize_graphics(state: tauri::State<AppState>) -> Result<OptimizeResultDto, String> {
    let hardware = HardwareProfile::detect_headless();
    let mut hw_guard = state
        .hardware
        .lock()
        .map_err(|_| "Failed to unlock hardware profile".to_string())?;
    *hw_guard = hardware.clone();
    drop(hw_guard);

    let (recommended, reason) = {
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        let (mut recommended, reason) = hardware.recommend_settings(&config.graphics);
        let options_path = state.run_dir().join("options.txt");
        if options_path.is_file() {
            read_minecraft_options_into(&state.run_dir(), &mut recommended);
        }
        config.graphics = recommended.clone();
        config.save(&state.paths.config_file)?;
        (recommended, reason)
    };
    if !state.run_dir().join("options.txt").is_file() {
        let _ = sync_minecraft_options(&state.run_dir(), &recommended);
    }

    Ok(OptimizeResultDto {
        graphics: recommended,
        reason,
    })
}

#[tauri::command]
async fn pick_and_import_skin(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<SkinResultDto>, String> {
    let parent_win = app.get_webview_window("main");
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Select Local Skin PNG File")
            .add_filter("PNG Image", &["png"]);
        if let Some(ref win) = parent_win {
            dialog = dialog.set_parent(win);
        }
        dialog.pick_file()
    })
    .await
    .map_err(|e| format!("File dialog failed: {e}"))?;

    let Some(src_path) = picked else {
        return Ok(None);
    };

    let skins_dir = state.paths.skins_dir.clone();
    let (img, saved_path, file_label) = tauri::async_runtime::spawn_blocking(move || {
        let img = load_skin_from_file(&src_path)?;
        fs::create_dir_all(&skins_dir)
            .map_err(|e| format!("Failed to prepare skins directory: {e}"))?;
        let dest = skins_dir.join("active_skin.png");
        img.save(&dest)
            .map_err(|e| format!("Failed to save normalized skin to disk: {e}"))?;
        let file_label = src_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("skin.png")
            .to_string();
        Ok::<_, String>((img, dest, file_label))
    })
    .await
    .map_err(|e| format!("Skin loader task failed: {e}"))??;

    let mut config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?;
    let saved_str = saved_path.to_string_lossy().into_owned();
    config.skin_file = Some(saved_path);
    config.save(&state.paths.config_file)?;
    let skin_is_slim = resolve_skin_is_slim(config.skin_model, &img);
    let model_text = if skin_is_slim { "Slim (3 px)" } else { "Classic / Wide (4 px)" };
    let note = format!("Imported {file_label} (64x64 PNG, {model_text}).");

    Ok(Some(SkinResultDto {
        rgba: img.into_raw(),
        path: Some(saved_str),
        skin_model: config.skin_model,
        skin_is_slim,
        note,
    }))
}

#[tauri::command]
fn reset_skin(state: tauri::State<AppState>) -> Result<SkinResultDto, String> {
    let active_skin = state.paths.skins_dir.join("active_skin.png");
    if active_skin.exists() {
        let _ = fs::remove_file(active_skin);
    }

    let mut config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?;
    config.skin_file = None;
    config.skin_model = SkinModel::Auto;
    config.save(&state.paths.config_file)?;

    let img = generate_default_skin();
    let skin_is_slim = resolve_skin_is_slim(config.skin_model, &img);

    Ok(SkinResultDto {
        rgba: img.into_raw(),
        path: None,
        skin_model: config.skin_model,
        skin_is_slim,
        note: "Restored default Basalt Slim skin.".to_string(),
    })
}

#[tauri::command]
fn save_skin_model(
    state: tauri::State<AppState>,
    skin_model: SkinModel,
) -> Result<SkinModelResultDto, String> {
    let mut config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?;
    config.skin_model = skin_model;
    config.save(&state.paths.config_file)?;

    let img = load_active_skin_image(config.skin_file.as_deref());
    let skin_is_slim = resolve_skin_is_slim(skin_model, &img);
    Ok(SkinModelResultDto {
        skin_model,
        skin_is_slim,
    })
}

#[tauri::command]
fn add_server(
    state: tauri::State<AppState>,
    name: String,
    address: String,
) -> Result<Vec<SavedServer>, String> {
    let clean_addr = validate_server_address(&address)?;
    let clean_name = {
        let n = name.trim();
        if n.is_empty() {
            "Local Server".to_string()
        } else {
            n.chars().take(48).collect()
        }
    };

    let mut config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?;
    config.saved_servers.push(SavedServer {
        name: clean_name,
        address: clean_addr,
    });
    config.save(&state.paths.config_file)?;
    Ok(config.saved_servers.clone())
}

#[tauri::command]
fn remove_server(
    state: tauri::State<AppState>,
    index: usize,
) -> Result<Vec<SavedServer>, String> {
    let mut config = state
        .config
        .lock()
        .map_err(|_| "Failed to unlock configuration".to_string())?;
    if index >= config.saved_servers.len() {
        return Err("Server entry to delete was not found.".to_string());
    }
    config.saved_servers.remove(index);
    config.save(&state.paths.config_file)?;
    Ok(config.saved_servers.clone())
}

#[tauri::command]
async fn ping_server_cmd(address: String) -> Result<ServerPingInfo, String> {
    let clean_addr = validate_server_address(&address)?;
    tauri::async_runtime::spawn_blocking(move || ping_server_1_21_4(&clean_addr))
        .await
        .map_err(|e| format!("Ping worker thread error: {e}"))?
}

#[tauri::command]
async fn probe_local_server_cmd(
    address: String,
    username: String,
) -> Result<LocalHandshakeResult, String> {
    let clean_addr = validate_server_address(&address)?;
    let clean_user = validate_username(&username)?;
    tauri::async_runtime::spawn_blocking(move || {
        probe_local_dev_login_1_21_4(&clean_addr, &clean_user)
    })
    .await
    .map_err(|e| format!("Handshake worker thread error: {e}"))?
}

#[tauri::command]
fn recheck_java_21(state: tauri::State<AppState>) -> Result<VanillaEnvStatus, String> {
    let ver = state
        .config
        .lock()
        .map(|c| c.mc_version.clone())
        .unwrap_or_else(|_| "1.21.4".to_string());
    Ok(inspect_vanilla_environment_for_version(
        &state.vanilla_dir,
        &state.run_dir,
        Some(&state.paths.runtime_dir),
        &ver,
    ))
}

#[tauri::command]
fn open_java_download_page() -> Result<(), String> {
    const ADOPTIUM_JDK21_URL: &str =
        "https://adoptium.net/temurin/releases/?version=21&os=windows&arch=x64&package=jdk";
    #[cfg(windows)]
    {
        Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", ADOPTIUM_JDK21_URL])
            .spawn()
            .map_err(|e| format!("Failed to open browser: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err(format!("Please install Java 21 from: {ADOPTIUM_JDK21_URL}"))
    }
}

#[cfg(windows)]
fn hide_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(windows)]
fn install_java21_blocking(app: &tauri::AppHandle, runtime_dir: &Path) -> Result<(), String> {
    let log = |line: String, is_err: bool| {
        let _ = app.emit("basalt://launch-log", LaunchLogPayload { line, is_err });
    };

    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "x64"
    };
    fs::create_dir_all(runtime_dir)
        .map_err(|e| format!("Failed to create runtime directory: {e}"))?;
    log(
        format!("Querying Adoptium official API for Eclipse Temurin JDK 21 (Windows {arch})..."),
        false,
    );

    let api_url =
        format!("https://api.adoptium.net/v3/assets/latest/21/hotspot?os=windows&architecture={arch}&image_type=jdk");
    let curl_exe = resolve_curl_exe();
    let mut caps = CurlCapabilities::detect();
    let mut meta_args = vec![
        "-fsSL",
        "--connect-timeout",
        "20",
        "--speed-limit",
        "1024",
        "--speed-time",
        "30",
        "--max-time",
        "45",
    ];
    if caps.supports_ssl_revoke_best_effort {
        meta_args.push("--ssl-revoke-best-effort");
    }
    let mut meta_cmd = Command::new(&curl_exe);
    meta_cmd
        .args(&meta_args)
        .arg(&api_url)
        .stdin(Stdio::null());
    hide_console_window(&mut meta_cmd);
    let mut meta_out = meta_cmd
        .output()
        .map_err(|e| format!("Failed to query Adoptium API via curl.exe: {e}"))?;
    if !meta_out.status.success() && meta_out.status.code() == Some(2) && caps.supports_ssl_revoke_best_effort {
        caps.supports_ssl_revoke_best_effort = false;
        let mut retry_cmd = Command::new(&curl_exe);
        retry_cmd
            .args(["-fsSL", "--connect-timeout", "20", "--max-time", "45", &api_url])
            .stdin(Stdio::null());
        hide_console_window(&mut retry_cmd);
        meta_out = retry_cmd
            .output()
            .map_err(|e| format!("Failed to query Adoptium API via curl.exe: {e}"))?;
    }
    if !meta_out.status.success() {
        let stderr_tail = String::from_utf8_lossy(&meta_out.stderr).trim().to_string();
        return Err(format!(
            "Adoptium API did not respond (curl exit code: {}). {stderr_tail}",
            meta_out.status.code().unwrap_or(-1)
        ));
    }

    let releases: serde_json::Value = serde_json::from_slice(&meta_out.stdout)
        .map_err(|e| format!("Failed to parse Adoptium JSON response: {e}"))?;
    let pkg = releases
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("binary"))
        .and_then(|bin| bin.get("package"))
        .ok_or_else(|| "No JDK 21 package found in Adoptium response.".to_string())?;

    let link = pkg
        .get("link")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Adoptium package download link is missing.".to_string())?;
    let name = pkg
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("temurin-jdk-21.zip");
    let size_bytes = pkg.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
    let expected_sha = pkg.get("checksum").and_then(|v| v.as_str()).map(|s| s.to_ascii_lowercase());

    let size_mb = (size_bytes as f64) / 1_048_576.0;
    log(format!("Found package: {name} ({size_mb:.1} MB)"), false);

    let zip_path = runtime_dir.join("temurin-jdk21-tmp.zip");
    let ext_dir = runtime_dir.join("extract-tmp");
    let _ = fs::remove_file(&zip_path);
    let _ = fs::remove_dir_all(&ext_dir);

    struct TempInstallGuard<'a>(&'a Path, &'a Path);
    impl Drop for TempInstallGuard<'_> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
            let _ = fs::remove_dir_all(self.1);
        }
    }
    let _temp_guard = TempInstallGuard(&zip_path, &ext_dir);

    log("Downloading JDK 21 archive, please wait...".to_string(), false);
    let mut dl_args = vec![
        "-fL",
        "--retry",
        "2",
        "--connect-timeout",
        "20",
        "--speed-limit",
        "1024",
        "--speed-time",
        "35",
        "-s",
        "-S",
    ];
    if caps.supports_ssl_revoke_best_effort {
        dl_args.push("--ssl-revoke-best-effort");
    }
    let mut dl_cmd = Command::new(&curl_exe);
    dl_cmd
        .args(&dl_args)
        .arg("-o")
        .arg(&zip_path)
        .arg(link)
        .stdin(Stdio::null());
    hide_console_window(&mut dl_cmd);
    let dl_out = dl_cmd
        .output()
        .map_err(|e| format!("Failed to start JDK 21 download: {e}"))?;
    if !dl_out.status.success() {
        let err_tail = String::from_utf8_lossy(&dl_out.stderr).trim().to_string();
        return Err(format!(
            "Failed to download JDK 21 (code: {}). {err_tail}",
            dl_out.status.code().unwrap_or(-1)
        ));
    }

    if let Some(ref expected) = expected_sha {
        log("Verifying SHA-256 integrity checksum...".to_string(), false);
        let actual = compute_file_sha256_hex(&zip_path)?;
        if &actual != expected {
            let _ = fs::remove_file(&zip_path);
            return Err("Downloaded Java 21 archive SHA-256 checksum mismatch.".to_string());
        }
    }

    log("Extracting Java 21 archive...".to_string(), false);
    fs::create_dir_all(&ext_dir)
        .map_err(|e| format!("Failed to create temporary extraction directory: {e}"))?;
    let tar_exe = resolve_tar_exe();
    let mut tar_cmd = Command::new(&tar_exe);
    tar_cmd
        .args(["-xf"])
        .arg(&zip_path)
        .arg("-C")
        .arg(&ext_dir)
        .stdin(Stdio::null());
    hide_console_window(&mut tar_cmd);
    let tar_out = tar_cmd
        .output()
        .map_err(|e| format!("Failed to unpack archive via tar.exe: {e}"))?;
    if !tar_out.status.success() {
        let err_tail = String::from_utf8_lossy(&tar_out.stderr).trim().to_string();
        return Err(format!("Failed to extract Java 21 archive: {err_tail}"));
    }

    let jdk_src = fs::read_dir(&ext_dir)
        .map_err(|e| format!("Failed to read extracted directory: {e}"))?
        .flatten()
        .map(|entry| entry.path())
        .find(|p| p.is_dir() && p.join("bin").join("java.exe").is_file())
        .ok_or_else(|| "bin\\java.exe was not found inside the extracted archive.".to_string())?;

    let target = runtime_dir.join("jdk-21");
    let backup = runtime_dir.join("jdk-21.old");
    let _ = fs::remove_dir_all(&backup);
    let moved_existing = target.exists() && fs::rename(&target, &backup).is_ok();
    if target.exists() && !moved_existing {
        let _ = fs::remove_dir_all(&target);
    }
    let mut rename_err = None;
    for attempt in 0..10 {
        match fs::rename(&jdk_src, &target) {
            Ok(()) => {
                rename_err = None;
                break;
            }
            Err(e) => {
                rename_err = Some(e);
                if attempt < 9 {
                    thread::sleep(std::time::Duration::from_millis(200));
                }
            }
        }
    }
    if let Some(e) = rename_err {
        if moved_existing && !target.exists() {
            let _ = fs::rename(&backup, &target);
        }
        return Err(format!("Failed to move JDK 21 into runtime directory: {e}"));
    }
    if moved_existing {
        let _ = fs::remove_dir_all(&backup);
    }

    let ver_line = fs::read_to_string(target.join("release"))
        .ok()
        .and_then(|txt| txt.lines().find(|l| l.starts_with("JAVA_VERSION=")).map(str::to_string))
        .unwrap_or_else(|| "JDK 21".to_string());
    log(
        format!("Java 21 ready ({ver_line}): {}", target.display()),
        false,
    );
    Ok(())
}

fn begin_running_session(
    running_arc: &Mutex<Option<RunningSessionDto>>,
    session: RunningSessionDto,
) -> Result<(), String> {
    let mut running = running_arc
        .lock()
        .map_err(|_| "Failed to unlock session state".to_string())?;
    if running.is_some() {
        return Err("Another operation (Minecraft or setup) is currently running.".to_string());
    }
    *running = Some(session);
    Ok(())
}

fn finish_running_session(running_arc: &Mutex<Option<RunningSessionDto>>) {
    if let Ok(mut guard) = running_arc.lock() {
        *guard = None;
    }
}

#[tauri::command]
fn install_java_21(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<RunningSessionDto, String> {
    #[cfg(not(windows))]
    {
        let _ = (app, state);
        return Err("Automatic portable Java 21 installer is currently only supported on Windows.".to_string());
    }

    #[cfg(windows)]
    {
        let session = RunningSessionDto {
            pid: std::process::id(),
            kind: "java-install".to_string(),
            label: "Eclipse Temurin Java 21 Setup".to_string(),
        };
        begin_running_session(&state.running, session.clone())?;

        let running_arc = Arc::clone(&state.running);
        let vanilla_dir = state.vanilla_dir.clone();
        let run_dir = state.run_dir.clone();
        let saves_dir = state.saves_dir();
        let runtime_dir = state.paths.runtime_dir.clone();
        let (active_ver, active_loader) = state
            .config
            .lock()
            .map(|c| (c.mc_version.clone(), c.mod_loader))
            .unwrap_or_else(|_| ("1.21.4".to_string(), LoaderKind::Vanilla));

        thread::spawn(move || {
            let result = install_java21_blocking(&app, &runtime_dir);
            finish_running_session(&running_arc);
            let (exit_code, error) = match result {
                Ok(()) => (Some(0), None),
                Err(msg) => (Some(1), Some(msg)),
            };
            emit_session_ended(
                &app,
                "java-install".to_string(),
                exit_code,
                error,
                &SessionDirs {
                    vanilla: &vanilla_dir,
                    run: &run_dir,
                    saves: &saves_dir,
                    runtime: &runtime_dir,
                    version: &active_ver,
                    loader: active_loader,
                },
            );
        });

        Ok(session)
    }
}

#[tauri::command]
fn prepare_vanilla_client(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
) -> Result<RunningSessionDto, String> {
    let active_loader = state
        .config
        .lock()
        .map(|c| c.mod_loader)
        .unwrap_or(LoaderKind::Vanilla);
    prepare_loader_environment(app, state, active_loader)
}

#[tauri::command]
fn prepare_loader_environment(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    loader: LoaderKind,
) -> Result<RunningSessionDto, String> {
    let (active_ver, dl_threads) = state
        .config
        .lock()
        .map(|c| (c.mc_version.clone(), c.download_threads))
        .unwrap_or_else(|_| ("1.21.4".to_string(), 24));
    let loader_name = loader_display_name(loader);
    let session = RunningSessionDto {
        pid: std::process::id(),
        kind: "prepare".to_string(),
        label: format!("{loader_name} {active_ver} Verification & Assets"),
    };
    begin_running_session(&state.running, session.clone())?;

    let java_home_opt = if loader == LoaderKind::Forge {
        Some(
            discover_java_for_loader(Some(&state.paths.runtime_dir), &active_ver, loader)
                .map(|(h, _)| h)
                .ok_or_else(|| {
                    finish_running_session(&state.running);
                    "Java 21 runtime is required to set up Forge.".to_string()
                })?,
        )
    } else {
        None
    };

    let running_arc = Arc::clone(&state.running);
    let vanilla_dir = state.vanilla_dir.clone();
    let run_dir = state.run_dir.clone();
    let saves_dir = state.saves_dir();
    let runtime_dir = state.paths.runtime_dir.clone();

    thread::spawn(move || {
        let log_emit = |line: &str, is_err: bool| {
            let _ = app.emit(
                "basalt://launch-log",
                LaunchLogPayload {
                    line: line.to_string(),
                    is_err,
                },
            );
        };
        let res = match loader {
            LoaderKind::Vanilla => ensure_vanilla_version_ready_with_threads(
                &vanilla_dir,
                &active_ver,
                dl_threads,
                log_emit,
            )
            .map(|_| ()),
            LoaderKind::Fabric => {
                ensure_fabric_version_ready(&vanilla_dir, &active_ver, dl_threads, log_emit)
                    .map(|_| ())
            }
            LoaderKind::Forge => {
                let java_home = java_home_opt.expect("checked above for Forge");
                ensure_forge_version_ready(
                    &vanilla_dir,
                    &java_home,
                    &active_ver,
                    dl_threads,
                    log_emit,
                )
                .map(|_| ())
            }
        };
        finish_running_session(&running_arc);
        let (exit_code, error) = match res {
            Ok(()) => (Some(0), None),
            Err(msg) => (Some(1), Some(msg)),
        };
        emit_session_ended(
            &app,
            "prepare".to_string(),
            exit_code,
            error,
            &SessionDirs {
                vanilla: &vanilla_dir,
                run: &run_dir,
                saves: &saves_dir,
                runtime: &runtime_dir,
                version: &active_ver,
                loader,
            },
        );
    });

    Ok(session)
}

#[tauri::command]
fn launch_client(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    input: LaunchInput,
) -> Result<RunningSessionDto, String> {
    struct SessionReservation<'a> {
        running: Option<&'a Mutex<Option<RunningSessionDto>>>,
    }
    impl Drop for SessionReservation<'_> {
        fn drop(&mut self) {
            if let Some(arc) = self.running {
                finish_running_session(arc);
            }
        }
    }

    let active_loader = state
        .config
        .lock()
        .map(|c| c.mod_loader)
        .unwrap_or(LoaderKind::Vanilla);
    let active_loader_name = loader_display_name(active_loader);
    let initial_session = RunningSessionDto {
        pid: std::process::id(),
        kind: "launch".to_string(),
        label: format!("Minecraft ({active_loader_name})"),
    };
    begin_running_session(&state.running, initial_session)?;
    let mut reservation = SessionReservation {
        running: Some(&state.running),
    };

    let (config_snapshot, label, quick_sp, quick_mp) = {
        let mut config = state
            .config
            .lock()
            .map_err(|_| "Failed to unlock configuration".to_string())?;
        if let Some(ref requested_ver) = input.mc_version {
            config.mc_version = normalize_mc_version(requested_ver);
        }
        let ver = config.mc_version.clone();
        let l_name = loader_display_name(config.mod_loader);

        match input.mode.as_str() {
            "singleplayer" => {
                let save_id = input
                    .save_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "Select a world save to quick-launch singleplayer.".to_string())?
                    .to_string();
                let _ = resolve_save_dir(&state.saves_dir(), &save_id)?;
                config.last_save_id = Some(save_id.clone());
                config.save(&state.paths.config_file)?;
                (
                    config.clone(),
                    format!("Minecraft {ver} ({save_id})"),
                    Some(save_id),
                    None,
                )
            }
            "multiplayer" => {
                let raw_addr = input
                    .server_address
                    .as_deref()
                    .ok_or_else(|| "No server address specified.".to_string())?;
                let addr = validate_server_address(raw_addr)?;
                config.save(&state.paths.config_file)?;
                (
                    config.clone(),
                    format!("Minecraft {ver} ({addr})"),
                    None,
                    Some(addr),
                )
            }
            "title" => {
                config.save(&state.paths.config_file)?;
                (
                    config.clone(),
                    format!("Minecraft {ver} ({l_name})"),
                    None,
                    None,
                )
            }
            _ => return Err("Invalid launch mode.".to_string()),
        }
    };

    let (java_home, _) = discover_java_for_loader(
        Some(&state.paths.runtime_dir),
        &config_snapshot.mc_version,
        config_snapshot.mod_loader,
    )
    .ok_or_else(|| {
        "Java 21 runtime was not found. Use 'Install Java 21' on the Play screen to install it."
            .to_string()
    })?;

    if config_snapshot.mod_loader == LoaderKind::Forge {
        let p_dir = forge_profile_dir(&state.vanilla_dir, &config_snapshot.mc_version);
        if p_dir.is_dir() {
            sync_profile_shared_files(&p_dir, &state.run_dir);
        }
    }

    let skin_img = load_active_skin_image(config_snapshot.skin_file.as_deref());
    let skin_is_slim = resolve_skin_is_slim(config_snapshot.skin_model, &skin_img);
    let skin_png_bytes = load_active_skin_png_bytes(config_snapshot.skin_file.as_deref());
    let player_uuid = write_basalt_skin_resource_pack_for(
        &state.run_dir,
        &config_snapshot.username,
        &skin_png_bytes,
        skin_is_slim,
        Some(&config_snapshot.mc_version),
    )?;

    let pack_entry = format!("file/{BASALT_SKIN_PACK_NAME}");
    ensure_launch_options(&state.run_dir, &config_snapshot.graphics, Some(&pack_entry))?;

    let session = RunningSessionDto {
        pid: std::process::id(),
        kind: "launch".to_string(),
        label,
    };
    if let Ok(mut guard) = state.running.lock() {
        *guard = Some(session.clone());
    }
    reservation.running = None;

    let active_ver = config_snapshot.mc_version.clone();
    let active_loader = config_snapshot.mod_loader;
    let loader_name = loader_display_name(active_loader);
    let dl_threads = config_snapshot.download_threads;
    let discord_enabled = config_snapshot.discord_rpc;
    let username_for_discord = config_snapshot.username.clone();
    let discord_arc = Arc::clone(&state.discord);
    let started_epoch_secs = state.started_epoch_secs;
    let active_state_label = if let Some(ref sp) = quick_sp {
        format!("Singleplayer · {sp}")
    } else if let Some(ref mp) = quick_mp {
        format!("Multiplayer · {mp}")
    } else {
        format!("In-Game · {}", config_snapshot.username)
    };
    spawn_discord_sync(
        Arc::clone(&discord_arc),
        discord_enabled,
        active_ver.clone(),
        format!("Playing Minecraft {active_ver} ({loader_name})"),
        active_state_label,
        started_epoch_secs,
    );

    let launch_cfg = VanillaLaunchConfig {
        username: config_snapshot.username,
        player_uuid,
        graphics: config_snapshot.graphics,
        quick_play_singleplayer: quick_sp,
        quick_play_multiplayer: quick_mp,
        java_home,
        network_boost: config_snapshot.network_boost,
        process_boost: config_snapshot.process_boost,
    };

    let running_arc = Arc::clone(&state.running);
    let vanilla_dir = state.vanilla_dir.clone();
    let run_dir = state.run_dir.clone();
    let saves_dir = state.saves_dir();
    let runtime_dir = state.paths.runtime_dir.clone();
    let log_file = state.paths.root_dir.join("logs").join("last-launch.log");

    thread::spawn(move || {
        let log_emit = |line: &str, is_err: bool| {
            let _ = app.emit(
                "basalt://launch-log",
                LaunchLogPayload {
                    line: line.to_string(),
                    is_err,
                },
            );
        };

        let (exit_code, error) = match active_loader {
            LoaderKind::Vanilla => match ensure_vanilla_version_ready_with_threads(
                &vanilla_dir,
                &active_ver,
                dl_threads,
                log_emit,
            ) {
                Err(err) => (Some(1), Some(err)),
                Ok(manifest) => match spawn_vanilla_client(&vanilla_dir, &run_dir, &manifest, &launch_cfg) {
                    Err(err) => (Some(1), Some(err)),
                    Ok(child) => {
                        if let Ok(mut guard) = running_arc.lock()
                            && let Some(ref mut active) = *guard
                        {
                            active.pid = child.id();
                        }
                        monitor_vanilla_child_blocking(&app, child, &log_file)
                    }
                },
            },
            LoaderKind::Fabric => {
                match ensure_fabric_version_ready(&vanilla_dir, &active_ver, dl_threads, log_emit) {
                    Err(err) => (Some(1), Some(err)),
                    Ok((vanilla_manifest, overlay)) => match spawn_modded_client(
                        &vanilla_dir,
                        &run_dir,
                        LoaderKind::Fabric,
                        &vanilla_manifest,
                        &overlay,
                        &launch_cfg,
                    ) {
                        Err(err) => (Some(1), Some(err)),
                        Ok((child, _)) => {
                            if let Ok(mut guard) = running_arc.lock()
                                && let Some(ref mut active) = *guard
                            {
                                active.pid = child.id();
                            }
                            monitor_vanilla_child_blocking(&app, child, &log_file)
                        }
                    },
                }
            }
            LoaderKind::Forge => match ensure_forge_version_ready(
                &vanilla_dir,
                &launch_cfg.java_home,
                &active_ver,
                dl_threads,
                log_emit,
            ) {
                Err(err) => (Some(1), Some(err)),
                Ok((vanilla_manifest, overlay)) => match spawn_modded_client(
                    &vanilla_dir,
                    &run_dir,
                    LoaderKind::Forge,
                    &vanilla_manifest,
                    &overlay,
                    &launch_cfg,
                ) {
                    Err(err) => (Some(1), Some(err)),
                    Ok((child, effective_game_dir)) => {
                        if let Ok(mut guard) = running_arc.lock()
                            && let Some(ref mut active) = *guard
                        {
                            active.pid = child.id();
                        }
                        let res = monitor_vanilla_child_blocking(&app, child, &log_file);
                        sync_profile_shared_files(&effective_game_dir, &run_dir);
                        res
                    }
                },
            },
        };

        finish_running_session(&running_arc);
        if let Some(app_state) = app.try_state::<AppState>()
            && let Ok(mut guard) = app_state.config.lock()
            && read_minecraft_options_into(&run_dir, &mut guard.graphics)
        {
            let _ = guard.save(&app_state.paths.config_file);
        }
        spawn_discord_sync(
            discord_arc,
            discord_enabled,
            active_ver.clone(),
            format!("Minecraft {active_ver} ({loader_name})"),
            format!("Basalt Launcher · {username_for_discord}"),
            started_epoch_secs,
        );
        emit_session_ended(
            &app,
            "launch".to_string(),
            exit_code,
            error,
            &SessionDirs {
                vanilla: &vanilla_dir,
                run: &run_dir,
                saves: &saves_dir,
                runtime: &runtime_dir,
                version: &active_ver,
                loader: active_loader,
            },
        );
    });

    Ok(session)
}

fn write_startup_log(line: &str) {
    use std::io::Write;
    let log_path = std::env::temp_dir().join("basalt-startup.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(windows)]
fn show_windows_alert(title: &str, body: &str, ask_yes_no: bool) -> bool {
    unsafe extern "system" {
        fn MessageBoxW(
            hwnd: *mut core::ffi::c_void,
            lp_text: *const u16,
            lp_caption: *const u16,
            u_type: u32,
        ) -> i32;
    }
    const MB_OK: u32 = 0x0000_0000;
    const MB_YESNO: u32 = 0x0000_0004;
    const MB_ICONERROR: u32 = 0x0000_0010;
    const IDYES: i32 = 6;

    let mut wide_text: Vec<u16> = body.encode_utf16().collect();
    wide_text.push(0);
    let mut wide_title: Vec<u16> = title.encode_utf16().collect();
    wide_title.push(0);
    let flags = if ask_yes_no { MB_YESNO | MB_ICONERROR } else { MB_OK | MB_ICONERROR };
    // SAFETY: Null-terminated UTF-16 buffers outlive synchronous MessageBoxW call.
    let res = unsafe {
        MessageBoxW(
            core::ptr::null_mut(),
            wide_text.as_ptr(),
            wide_title.as_ptr(),
            flags,
        )
    };
    res == IDYES
}

#[cfg(not(windows))]
fn show_windows_alert(_title: &str, body: &str, _ask_yes_no: bool) -> bool {
    eprintln!("{body}");
    false
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("Basalt encountered an unexpected startup error:\n{info}");
        write_startup_log(&msg);
        show_windows_alert("Basalt Startup Error", &msg, false);
    }));

    let app_state = match AppState::initialize() {
        Ok(s) => s,
        Err(err) => {
            let msg = format!("Failed to initialize Basalt workspace:\n{err}");
            write_startup_log(&msg);
            show_windows_alert("Basalt Startup Error", &msg, false);
            std::process::exit(1);
        }
    };

    let run_res = tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            get_bootstrap_state,
            list_saves,
            select_save,
            delete_save,
            save_profile_settings,
            set_mc_version,
            set_mod_loader,
            search_modrinth_catalog,
            install_modrinth_item,
            install_shader_bundle,
            list_library_items_cmd,
            toggle_library_item_cmd,
            delete_library_item_cmd,
            pick_and_import_library_file,
            open_library_folder,
            save_turbo_settings,
            optimize_client_now,
            open_github_page,
            open_game_folder,
            set_fullscreen,
            window_minimize,
            window_toggle_maximize,
            window_is_maximized,
            window_close,
            window_start_drag,
            load_minecraft_sounds,
            save_graphics_settings,
            auto_optimize_graphics,
            pick_and_import_skin,
            reset_skin,
            save_skin_model,
            add_server,
            remove_server,
            ping_server_cmd,
            probe_local_server_cmd,
            prepare_vanilla_client,
            prepare_loader_environment,
            launch_client,
            install_java_21,
            recheck_java_21,
            open_java_download_page,
        ])
        .setup(|app| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_title("Basalt — Minecraft Launcher (Unofficial)");
            }
            Ok(())
        })
        .on_window_event(|_window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                shutdown_discord_rpc();
            }
        })
        .run(tauri::generate_context!());

    if let Err(err) = run_res {
        let log_note = format!("Failed to initialize Tauri/WebView2: {err}");
        write_startup_log(&log_note);
        let msg = format!(
            "Basalt could not open its window ({err}).\n\n\
             Fresh Windows installations may be missing the Microsoft Edge WebView2 Runtime.\n\
             Would you like to open the official Microsoft WebView2 download page in your browser?"
        );
        if show_windows_alert("Basalt - WebView2 Required", &msg, true) {
            let _ = Command::new("rundll32.exe")
                .args([
                    "url.dll,FileProtocolHandler",
                    "https://go.microsoft.com/fwlink/p/?LinkId=2124703",
                ])
                .spawn();
        }
        std::process::exit(1);
    }
}
