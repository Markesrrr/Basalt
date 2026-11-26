use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::LoaderKind;
use crate::protocol::local_player_uuid_dashed;
use crate::vanilla::{
    DownloadTask, MojangLibDownloads, MojangRule, MojangVersionManifest, VanillaLaunchConfig,
    append_minecraft_game_args, build_jvm_base_args, check_file_size_and_sha1,
    check_vanilla_assets_ready_for, check_vanilla_client_ready_for, client_jar_path_for,
    curl_fetch_bytes_with_ua, current_mojang_arch, current_mojang_os_name, download_batch_with_curl,
    ensure_vanilla_version_ready_with_threads, hide_window, is_library_allowed_for_target,
    load_cached_version_manifest_for, native_classifier_for_library, spawn_java_client,
    summarize_process_failure,
};

const BASALT_HTTP_UA: &str = "Basalt-Launcher/0.3.1 (https://github.com/Markesrrr/Basalt)";
const FABRIC_META_BASE: &str = "https://meta.fabricmc.net/v2";
const FORGE_PROMOTIONS_URL: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const FORGE_MAVEN_BASE: &str = "https://maven.minecraftforge.net";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderEnvStatus {
    pub mc_version: String,
    pub active_loader: LoaderKind,
    pub fabric_ready: bool,
    pub forge_ready: bool,
    pub fabric_loader_version: Option<String>,
    pub forge_version: Option<String>,
    pub active_mods_dir: String,
    pub shaderpacks_dir: String,
    pub resourcepacks_dir: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FabricLoaderEntry {
    loader: FabricLoaderSpec,
}

#[derive(Debug, Clone, Deserialize)]
struct FabricLoaderSpec {
    version: String,
    #[serde(default)]
    stable: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ForgePromotions {
    #[serde(default)]
    promos: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct VersionArguments {
    #[serde(default)]
    pub game: Vec<Value>,
    #[serde(default)]
    pub jvm: Vec<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayLibrary {
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub downloads: Option<MojangLibDownloads>,
    #[serde(default)]
    pub rules: Vec<MojangRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayVersionManifest {
    pub id: String,
    #[serde(default)]
    pub inherits_from: Option<String>,
    #[serde(default)]
    pub jar: Option<String>,
    pub main_class: String,
    #[serde(default)]
    pub arguments: Option<VersionArguments>,
    #[serde(default)]
    pub minecraft_arguments: Option<String>,
    #[serde(default)]
    pub libraries: Vec<OverlayLibrary>,
}

/// Converts a Maven coordinate (`group:artifact:version[:classifier][@ext]`) into its repository-relative path.
pub fn maven_coord_to_rel_path(coord: &str) -> Result<String, String> {
    let trimmed = coord.trim();
    if trimmed.is_empty() {
        return Err("Empty Maven coordinate".to_string());
    }
    let (coords_part, ext) = match trimmed.rsplit_once('@') {
        Some((left, right)) if !right.is_empty() => (left, right),
        Some(_) => return Err(format!("Malformed Maven extension in '{trimmed}'")),
        None => (trimmed, "jar"),
    };
    let parts: Vec<&str> = coords_part.split(':').collect();
    if parts.len() < 3 || parts.len() > 4 || parts.iter().take(3).any(|p| p.is_empty()) {
        return Err(format!("Invalid Maven coordinate '{trimmed}'"));
    }
    let group_path = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3).copied().filter(|s| !s.is_empty());

    let filename = match classifier {
        Some(cls) => format!("{artifact}-{version}-{cls}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    Ok(format!("{group_path}/{artifact}/{version}/{filename}"))
}

/// Returns a stable deduplication key (`group:artifact[:classifier]`) for a Maven coordinate.
pub fn maven_group_artifact_key(coord: &str) -> String {
    let base = coord.split('@').next().unwrap_or(coord).trim();
    let mut parts = base.split(':');
    let group = parts.next().unwrap_or("");
    let artifact = parts.next().unwrap_or("");
    let _version = parts.next().unwrap_or("");
    match parts.next() {
        Some(cls) if !cls.is_empty() => format!("{group}:{artifact}:{cls}"),
        _ => format!("{group}:{artifact}"),
    }
}

/// Returns the isolated `mods/` directory for the selected loader and Minecraft version.
pub fn loader_mods_dir(vanilla_dir: &Path, run_dir: &Path, loader: LoaderKind, mc_version: &str) -> PathBuf {
    let safe_ver = sanitize_version_segment(mc_version);
    match loader {
        LoaderKind::Vanilla => run_dir.join("mods"),
        LoaderKind::Fabric => vanilla_dir.join("loader_mods").join("fabric").join(safe_ver),
        LoaderKind::Forge => forge_profile_dir(vanilla_dir, mc_version).join("mods"),
    }
}

/// Returns the isolated Forge profile root (`profiles/forge/<mc_version>`).
pub fn forge_profile_dir(vanilla_dir: &Path, mc_version: &str) -> PathBuf {
    vanilla_dir
        .join("profiles")
        .join("forge")
        .join(sanitize_version_segment(mc_version))
}

pub fn shared_shaderpacks_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("shaderpacks")
}

pub fn shared_resourcepacks_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("resourcepacks")
}

fn sanitize_version_segment(mc_version: &str) -> String {
    mc_version
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect()
}

/// Ensures shared content folders (`saves`, `shaderpacks`, `resourcepacks`, `screenshots`) exist
/// in `run_dir` and are linked cleanly into an isolated profile directory (Forge) via NTFS junctions.
pub fn ensure_shared_run_junctions(run_dir: &Path, profile_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(run_dir).map_err(|e| format!("Failed to create run dir: {e}"))?;
    fs::create_dir_all(profile_dir).map_err(|e| format!("Failed to create profile dir: {e}"))?;
    fs::create_dir_all(profile_dir.join("mods"))
        .map_err(|e| format!("Failed to create profile mods dir: {e}"))?;

    for folder in ["saves", "shaderpacks", "resourcepacks", "screenshots"] {
        let target = run_dir.join(folder);
        fs::create_dir_all(&target)
            .map_err(|e| format!("Failed to create shared folder {}: {e}", target.display()))?;
        let link = profile_dir.join(folder);
        create_dir_link_if_needed(&target, &link)?;
    }
    Ok(())
}

/// Copies `options.txt` and `servers.dat` between `run_dir` and `profile_dir` when missing or newer.
pub fn sync_profile_shared_files(source_dir: &Path, dest_dir: &Path) {
    for file in ["options.txt", "servers.dat"] {
        let src = source_dir.join(file);
        let dst = dest_dir.join(file);
        if !src.is_file() {
            continue;
        }
        let should_copy = match (fs::metadata(&src), fs::metadata(&dst)) {
            (Ok(s_meta), Ok(d_meta)) => s_meta
                .modified()
                .ok()
                .zip(d_meta.modified().ok())
                .is_some_and(|(sm, dm)| sm > dm),
            (Ok(_), Err(_)) => true,
            _ => false,
        };
        if should_copy {
            let _ = fs::copy(&src, &dst);
        }
    }
}

fn migrate_dir_non_destructively(src_dir: &Path, dst_dir: &Path) -> Result<(), String> {
    let entries = fs::read_dir(src_dir)
        .map_err(|e| format!("Failed to read existing folder {}: {e}", src_dir.display()))?;
    for entry in entries.flatten() {
        let src_item = entry.path();
        let name = entry.file_name();
        let dst_item = dst_dir.join(&name);
        if src_item.is_dir() {
            fs::create_dir_all(&dst_item)
                .map_err(|e| format!("Failed to create target subdir {}: {e}", dst_item.display()))?;
            migrate_dir_non_destructively(&src_item, &dst_item)?;
            let _ = fs::remove_dir(&src_item);
        } else if !dst_item.exists() {
            if fs::rename(&src_item, &dst_item).is_err() {
                fs::copy(&src_item, &dst_item).map_err(|e| {
                    format!("Failed to migrate {} to {}: {e}", src_item.display(), dst_item.display())
                })?;
                let _ = fs::remove_file(&src_item);
            }
        } else {
            // Preserve existing destination without destroying source collision
            let alt_name = format!("{}.forge-migrated", name.to_string_lossy());
            let alt_dst = dst_dir.join(alt_name);
            let _ = fs::rename(&src_item, &alt_dst).or_else(|_| fs::copy(&src_item, &alt_dst).map(|_| ()));
            let _ = fs::remove_file(&src_item);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_windows_reparse_point(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    meta.file_type().is_symlink() || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

#[cfg(windows)]
fn create_dir_link_if_needed(target: &Path, link: &Path) -> Result<(), String> {
    if let Ok(meta) = fs::symlink_metadata(link) {
        if is_windows_reparse_point(&meta) {
            if fs::read_dir(link).is_ok() {
                return Ok(());
            }
            // Dangling junction (target moved/deleted) — remove and recreate.
            let _ = fs::remove_dir(link).or_else(|_| fs::remove_file(link));
        } else if meta.is_dir() {
            migrate_dir_non_destructively(link, target)?;
            fs::remove_dir_all(link).map_err(|e| {
                format!("Failed to replace real folder {} with shared link: {e}", link.display())
            })?;
        } else {
            return Err(format!(
                "Cannot link shared folder {}: a regular file exists at that path",
                link.display()
            ));
        }
    }

    // Use PowerShell with env vars so special shell characters (`&`, `(`, `)`, `%`, spaces, Unicode) are never misinterpreted.
    let mut cmd = Command::new("powershell.exe");
    hide_window(&mut cmd);
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "New-Item -ItemType Junction -Path $env:BASALT_JLINK -Target $env:BASALT_JTARGET -Force | Out-Null",
    ])
    .env("BASALT_JLINK", link)
    .env("BASALT_JTARGET", target);

    let ok = cmd.output().is_ok_and(|out| out.status.success());
    if ok {
        return Ok(());
    }

    // Graceful non-NTFS / FAT32 / exFAT / UNC fallback: copy missing shared files non-destructively
    // without moving or emptying the primary shared `run/` folders.
    copy_dir_recursive_if_missing(target, link)?;
    Ok(())
}

fn copy_dir_recursive_if_missing(src_dir: &Path, dst_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dst_dir).map_err(|e| {
        format!("Failed to create fallback shared folder {}: {e}", dst_dir.display())
    })?;
    let Ok(entries) = fs::read_dir(src_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let src_item = entry.path();
        let Ok(meta) = fs::symlink_metadata(&src_item) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let dst_item = dst_dir.join(entry.file_name());
        if meta.is_dir() {
            copy_dir_recursive_if_missing(&src_item, &dst_item)?;
        } else if meta.is_file() && !dst_item.exists() {
            fs::copy(&src_item, &dst_item).map_err(|e| {
                format!(
                    "Failed to copy shared file {} to {}: {e}",
                    src_item.display(),
                    dst_item.display()
                )
            })?;
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn create_dir_link_if_needed(target: &Path, link: &Path) -> Result<(), String> {
    if fs::symlink_metadata(link).is_ok() {
        return Ok(());
    }
    std::os::unix::fs::symlink(target, link)
        .map_err(|e| format!("Failed to symlink shared folder {}: {e}", link.display()))
}

/// Scans cached `versions/*/` JSON manifests to inspect installed Fabric and Forge profiles for `mc_version`.
pub fn inspect_loader_environment(
    vanilla_dir: &Path,
    run_dir: &Path,
    mc_version: &str,
    active_loader: LoaderKind,
) -> LoaderEnvStatus {
    let fabric_manifest = find_cached_loader_manifest(vanilla_dir, mc_version, LoaderKind::Fabric);
    let forge_manifest = find_cached_loader_manifest(vanilla_dir, mc_version, LoaderKind::Forge);

    let fabric_loader_version = fabric_manifest.as_ref().and_then(|(id, _)| {
        // Format: fabric-loader-<loader_ver>-<mc_ver>
        id.strip_prefix("fabric-loader-")
            .and_then(|rest| rest.strip_suffix(&format!("-{mc_version}")))
            .map(ToString::to_string)
    });
    let forge_version = forge_manifest.as_ref().and_then(|(id, _)| {
        // Format: <mc_ver>-forge-<forge_ver>
        id.strip_prefix(&format!("{mc_version}-forge-"))
            .map(ToString::to_string)
    });

    let active_mods = loader_mods_dir(vanilla_dir, run_dir, active_loader, mc_version);
    let shaders = shared_shaderpacks_dir(run_dir);
    let packs = shared_resourcepacks_dir(run_dir);
    let _ = fs::create_dir_all(&active_mods);
    let _ = fs::create_dir_all(&shaders);
    let _ = fs::create_dir_all(&packs);

    LoaderEnvStatus {
        mc_version: mc_version.to_string(),
        active_loader,
        fabric_ready: fabric_manifest.is_some(),
        forge_ready: forge_manifest.is_some(),
        fabric_loader_version,
        forge_version,
        active_mods_dir: active_mods.to_string_lossy().into_owned(),
        shaderpacks_dir: shaders.to_string_lossy().into_owned(),
        resourcepacks_dir: packs.to_string_lossy().into_owned(),
    }
}

fn parse_version_segments(s: &str) -> Vec<u64> {
    s.split(['.', '-', '_', '+'])
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

fn fabric_loader_supports_mods_folder(loader_ver: &str) -> bool {
    let segs = parse_version_segments(loader_ver);
    let maj = segs.first().copied().unwrap_or(0);
    let min = segs.get(1).copied().unwrap_or(0);
    (maj, min) >= (0, 15)
}

fn overlay_libraries_present(vanilla_dir: &Path, overlay: &OverlayVersionManifest) -> bool {
    let os_name = current_mojang_os_name();
    let arch = current_mojang_arch();
    for lib in &overlay.libraries {
        if !is_library_allowed_for_target(&lib.name, &lib.rules, os_name, arch) {
            continue;
        }
        let rel = if let Some(ref downloads) = lib.downloads
            && let Some(ref art) = downloads.artifact
            && let Some(ref p) = art.path
        {
            p.clone()
        } else if let Ok(p) = maven_coord_to_rel_path(&lib.name) {
            p
        } else {
            return false;
        };
        let path = vanilla_dir
            .join("libraries")
            .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !path.is_file() || fs::metadata(&path).is_ok_and(|m| m.len() == 0) {
            return false;
        }
    }
    true
}

pub fn find_cached_loader_manifest(
    vanilla_dir: &Path,
    mc_version: &str,
    loader: LoaderKind,
) -> Option<(String, OverlayVersionManifest)> {
    let versions_dir = vanilla_dir.join("versions");
    let entries = fs::read_dir(&versions_dir).ok()?;
    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        let loader_ver_part = match loader {
            LoaderKind::Fabric => dir_name
                .strip_prefix("fabric-loader-")
                .and_then(|rest| rest.strip_suffix(&format!("-{mc_version}"))),
            LoaderKind::Forge => dir_name.strip_prefix(&format!("{mc_version}-forge-")),
            LoaderKind::Vanilla => None,
        };
        let Some(ver_str) = loader_ver_part else {
            continue;
        };
        if loader == LoaderKind::Fabric && !fabric_loader_supports_mods_folder(ver_str) {
            continue;
        }
        let json_path = entry.path().join(format!("{dir_name}.json"));
        if let Ok(raw) = fs::read_to_string(&json_path)
            && let Ok(parsed) = serde_json::from_str::<OverlayVersionManifest>(&raw)
            && parsed.inherits_from.as_deref() == Some(mc_version)
        {
            if loader == LoaderKind::Forge && !overlay_libraries_present(vanilla_dir, &parsed) {
                continue;
            }
            matches.push((parse_version_segments(ver_str), dir_name, parsed));
        }
    }
    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    matches.into_iter().next().map(|(_, id, manifest)| (id, manifest))
}

/// Ensures both the base Vanilla version and Fabric loader libraries/profile are downloaded and cached.
pub fn ensure_fabric_version_ready<F>(
    vanilla_dir: &Path,
    mc_version: &str,
    parallel_streams: u8,
    mut on_log: F,
) -> Result<(MojangVersionManifest, OverlayVersionManifest), String>
where
    F: FnMut(&str, bool),
{
    let vanilla_manifest = if let Some(cached) = load_cached_version_manifest_for(vanilla_dir, mc_version)
        && check_vanilla_client_ready_for(vanilla_dir, mc_version)
        && check_vanilla_assets_ready_for(vanilla_dir, mc_version)
    {
        cached
    } else {
        on_log(&format!("Preparing Vanilla {mc_version}..."), false);
        ensure_vanilla_version_ready_with_threads(
            vanilla_dir,
            mc_version,
            parallel_streams,
            &mut on_log,
        )?
    };

    on_log(&format!("Fetching Fabric metadata for {mc_version}..."), false);
    let profile_manifest = if let Some((_, cached)) =
        find_cached_loader_manifest(vanilla_dir, mc_version, LoaderKind::Fabric)
    {
        cached
    } else {
        let loader_list_url = format!("{FABRIC_META_BASE}/versions/loader/{mc_version}");
        let raw_list = curl_fetch_bytes_with_ua(&loader_list_url, Some(BASALT_HTTP_UA))?;
        let loaders: Vec<FabricLoaderEntry> = serde_json::from_slice(&raw_list)
            .map_err(|e| format!("Failed to parse Fabric loader list for {mc_version}: {e}"))?;
        let chosen = loaders
            .iter()
            .find(|entry| entry.loader.stable && fabric_loader_supports_mods_folder(&entry.loader.version))
            .or_else(|| loaders.iter().find(|entry| entry.loader.stable))
            .or_else(|| loaders.first())
            .ok_or_else(|| format!("No Fabric loader builds available for Minecraft {mc_version}"))?;

        let profile_url = format!(
            "{FABRIC_META_BASE}/versions/loader/{mc_version}/{}/profile/json",
            chosen.loader.version
        );
        let raw_profile = curl_fetch_bytes_with_ua(&profile_url, Some(BASALT_HTTP_UA))?;
        let parsed: OverlayVersionManifest = serde_json::from_slice(&raw_profile)
            .map_err(|e| format!("Failed to parse Fabric profile JSON: {e}"))?;

        let ver_dir = vanilla_dir.join("versions").join(&parsed.id);
        fs::create_dir_all(&ver_dir)
            .map_err(|e| format!("Failed to create Fabric version directory: {e}"))?;
        fs::write(ver_dir.join(format!("{}.json", parsed.id)), &raw_profile)
            .map_err(|e| format!("Failed to cache Fabric version JSON: {e}"))?;
        parsed
    };

    on_log("Downloading Fabric libraries...", false);
    download_overlay_libraries(vanilla_dir, &profile_manifest, parallel_streams)?;
    on_log(&format!("Fabric ready ({})", profile_manifest.id), false);
    Ok((vanilla_manifest, profile_manifest))
}

fn is_valid_jar_zip(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else {
        return false;
    };
    let Ok(meta) = f.metadata() else {
        return false;
    };
    if meta.len() < 10_000 {
        return false;
    }
    let mut magic = [0u8; 4];
    use std::io::Read;
    f.read_exact(&mut magic).is_ok() && &magic == b"PK\x03\x04"
}

/// Ensures both the base Vanilla version and recommended/latest Forge client are installed and ready.
pub fn ensure_forge_version_ready<F>(
    vanilla_dir: &Path,
    java_home: &Path,
    mc_version: &str,
    parallel_streams: u8,
    mut on_log: F,
) -> Result<(MojangVersionManifest, OverlayVersionManifest), String>
where
    F: FnMut(&str, bool),
{
    let vanilla_manifest = if let Some(cached) = load_cached_version_manifest_for(vanilla_dir, mc_version)
        && check_vanilla_client_ready_for(vanilla_dir, mc_version)
        && check_vanilla_assets_ready_for(vanilla_dir, mc_version)
    {
        cached
    } else {
        on_log(&format!("Preparing Vanilla {mc_version}..."), false);
        ensure_vanilla_version_ready_with_threads(
            vanilla_dir,
            mc_version,
            parallel_streams,
            &mut on_log,
        )?
    };

    if let Some((_, cached)) = find_cached_loader_manifest(vanilla_dir, mc_version, LoaderKind::Forge) {
        download_overlay_libraries(vanilla_dir, &cached, parallel_streams)?;
        on_log(&format!("Forge ready ({})", cached.id), false);
        return Ok((vanilla_manifest, cached));
    }

    on_log(&format!("Checking Forge build for {mc_version}..."), false);
    let promo_bytes = curl_fetch_bytes_with_ua(FORGE_PROMOTIONS_URL, Some(BASALT_HTTP_UA))?;
    let promos: ForgePromotions = serde_json::from_slice(&promo_bytes)
        .map_err(|e| format!("Failed to parse Forge promotions JSON: {e}"))?;
    let forge_ver = promos
        .promos
        .get(&format!("{mc_version}-recommended"))
        .or_else(|| promos.promos.get(&format!("{mc_version}-latest")))
        .cloned()
        .ok_or_else(|| {
            format!(
                "Minecraft Forge has no published build for {mc_version}. Select Vanilla or Fabric for {mc_version} instead."
            )
        })?;

    let launcher_profiles = vanilla_dir.join("launcher_profiles.json");
    if !launcher_profiles.is_file() {
        fs::write(&launcher_profiles, r#"{"profiles":{}}"#)
            .map_err(|e| format!("Failed to initialize launcher_profiles.json: {e}"))?;
    }

    let slug = format!("{mc_version}-{forge_ver}");
    let installer_name = format!("forge-{slug}-installer.jar");
    let installer_url =
        format!("{FORGE_MAVEN_BASE}/net/minecraftforge/forge/{slug}/{installer_name}");
    let installer_path = vanilla_dir.join("cache").join("installers").join(&installer_name);
    if !is_valid_jar_zip(&installer_path) {
        let _ = fs::remove_file(&installer_path);
        on_log(&format!("Downloading Forge {forge_ver} installer..."), false);
        let task = DownloadTask {
            url: installer_url,
            dest: installer_path.clone(),
            sha1: String::new(),
            size: 0,
            label: installer_name.clone(),
        };
        download_batch_with_curl(
            &[&task],
            &vanilla_dir.join("cache").join("tmp"),
            1,
        )?;
        if !is_valid_jar_zip(&installer_path) {
            let _ = fs::remove_file(&installer_path);
            return Err(format!(
                "Downloaded Forge installer ({installer_name}) is not a valid JAR archive (possible proxy or captive portal response)."
            ));
        }
    }

    on_log(
        &format!("Running Forge {forge_ver} setup (patching client libraries)..."),
        false,
    );
    let java_bin = java_home.join("bin").join(crate::vanilla::java_executable_name());
    if !java_bin.is_file() {
        return Err(format!(
            "Java executable was not found at {} for Forge setup",
            java_bin.display()
        ));
    }
    let mut cmd = Command::new(&java_bin);
    hide_window(&mut cmd);
    for key in [
        "_JAVA_OPTIONS",
        "JAVA_TOOL_OPTIONS",
        "JAVA_OPTS",
        "GRADLE_OPTS",
    ] {
        cmd.env_remove(key);
    }
    let out = cmd
        .current_dir(vanilla_dir)
        .arg("-jar")
        .arg(&installer_path)
        .arg("--installClient")
        .arg(vanilla_dir)
        .output()
        .map_err(|e| format!("Failed to launch Forge installer: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let lines: Vec<String> = stdout
            .lines()
            .chain(stderr.lines())
            .map(ToString::to_string)
            .collect();
        return Err(format!(
            "Forge installer failed for {slug}: {}",
            summarize_process_failure(&lines, out.status.code(), None)
        ));
    }

    let (_, forge_manifest) = find_cached_loader_manifest(vanilla_dir, mc_version, LoaderKind::Forge)
        .ok_or_else(|| {
            format!("Forge installer finished but version profile for {mc_version} was not found")
        })?;
    download_overlay_libraries(vanilla_dir, &forge_manifest, parallel_streams)?;
    on_log(&format!("Forge ready ({})", forge_manifest.id), false);
    Ok((vanilla_manifest, forge_manifest))
}

fn download_overlay_libraries(
    vanilla_dir: &Path,
    manifest: &OverlayVersionManifest,
    parallel_streams: u8,
) -> Result<(), String> {
    let os_name = current_mojang_os_name();
    let arch = current_mojang_arch();
    let mut tasks = Vec::new();
    let mut seen_dest = HashSet::new();

    for lib in &manifest.libraries {
        if !is_library_allowed_for_target(&lib.name, &lib.rules, os_name, arch) {
            continue;
        }
        let rel = if let Some(ref downloads) = lib.downloads
            && let Some(ref art) = downloads.artifact
            && let Some(ref p) = art.path
        {
            p.clone()
        } else {
            maven_coord_to_rel_path(&lib.name)?
        };
        let dest = vanilla_dir
            .join("libraries")
            .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !seen_dest.insert(dest.clone()) {
            continue;
        }

        let (url, sha1, size) = if let Some(ref downloads) = lib.downloads
            && let Some(ref art) = downloads.artifact
        {
            (art.url.clone(), art.sha1.clone(), art.size)
        } else {
            let base = lib
                .url
                .as_deref()
                .unwrap_or("https://maven.fabricmc.net/")
                .trim_end_matches('/');
            (
                format!("{base}/{rel}"),
                lib.sha1.clone().unwrap_or_default(),
                lib.size.unwrap_or(0),
            )
        };

        if url.trim().is_empty() || check_file_size_and_sha1(&dest, size, &sha1) {
            continue;
        }
        tasks.push(DownloadTask {
            url,
            dest,
            sha1,
            size,
            label: lib.name.clone(),
        });
    }

    if tasks.is_empty() {
        return Ok(());
    }
    let refs: Vec<&DownloadTask> = tasks.iter().collect();
    download_batch_with_curl(
        &refs,
        &vanilla_dir.join("cache").join("tmp"),
        parallel_streams,
    )
}

/// Launches Fabric or Forge cleanly with merged libraries, placeholders, and isolated mods folder.
pub fn spawn_modded_client(
    vanilla_dir: &Path,
    run_dir: &Path,
    loader: LoaderKind,
    vanilla_manifest: &MojangVersionManifest,
    overlay: &OverlayVersionManifest,
    cfg: &VanillaLaunchConfig,
) -> Result<(Child, PathBuf), String> {
    let effective_game_dir = match loader {
        LoaderKind::Fabric => {
            let mods_dir = loader_mods_dir(vanilla_dir, run_dir, LoaderKind::Fabric, &vanilla_manifest.id);
            fs::create_dir_all(&mods_dir)
                .map_err(|e| format!("Failed to create Fabric mods dir: {e}"))?;
            run_dir.to_path_buf()
        }
        LoaderKind::Forge => {
            let profile_dir = forge_profile_dir(vanilla_dir, &vanilla_manifest.id);
            ensure_shared_run_junctions(run_dir, &profile_dir)?;
            sync_profile_shared_files(run_dir, &profile_dir);
            profile_dir
        }
        LoaderKind::Vanilla => run_dir.to_path_buf(),
    };

    let args = build_modded_launch_args(
        vanilla_dir,
        &effective_game_dir,
        loader,
        vanilla_manifest,
        overlay,
        cfg,
    )?;
    let label = match loader {
        LoaderKind::Fabric => "Fabric",
        LoaderKind::Forge => "Forge",
        LoaderKind::Vanilla => "Vanilla",
    };
    let child = spawn_java_client(
        &effective_game_dir,
        &cfg.java_home,
        &args,
        cfg.process_boost,
        label,
    )?;
    Ok((child, effective_game_dir))
}

/// Constructs merged JVM + Classpath + Game arguments for Fabric or Forge overlay manifests.
pub fn build_modded_launch_args(
    vanilla_dir: &Path,
    game_dir: &Path,
    loader: LoaderKind,
    vanilla_manifest: &MojangVersionManifest,
    overlay: &OverlayVersionManifest,
    cfg: &VanillaLaunchConfig,
) -> Result<Vec<String>, String> {
    let mut args = build_jvm_base_args(vanilla_dir, vanilla_manifest, cfg);
    if loader == LoaderKind::Fabric {
        let mods_dir = loader_mods_dir(vanilla_dir, game_dir, LoaderKind::Fabric, &vanilla_manifest.id);
        args.push(format!("-Dfabric.modsFolder={}", mods_dir.display()));
    }

    let cp_entries = collect_merged_classpath(vanilla_dir, loader, vanilla_manifest, overlay)?;
    let cp_sep = if cfg!(windows) { ";" } else { ":" };
    let classpath = cp_entries.join(cp_sep);
    let vars = build_placeholder_map(vanilla_dir, game_dir, vanilla_manifest, overlay, cfg, &classpath);

    let os_name = current_mojang_os_name();
    let arch = current_mojang_arch();
    let mut jvm_has_cp = false;
    if let Some(ref overlay_args) = overlay.arguments {
        for raw_arg in evaluate_argument_values(&overlay_args.jvm, os_name, arch) {
            if raw_arg == "-cp" || raw_arg == "-classpath" {
                jvm_has_cp = true;
            }
            args.push(apply_placeholders(&raw_arg, &vars));
        }
    }

    if !jvm_has_cp {
        args.push("-cp".to_string());
        args.push(classpath);
    }
    args.push(overlay.main_class.clone());

    let mut game_args = Vec::new();
    append_minecraft_game_args(
        &mut game_args,
        vanilla_dir,
        game_dir,
        vanilla_manifest,
        cfg,
        &overlay.id,
    );

    // Strip any duplicate keys when overlay defines its own arguments.game tokens.
    let existing_keys: HashSet<String> = game_args
        .iter()
        .filter(|a| a.starts_with("--"))
        .cloned()
        .collect();

    if let Some(ref overlay_args) = overlay.arguments {
        let extra = evaluate_argument_values(&overlay_args.game, os_name, arch);
        append_dedup_game_args(&mut game_args, &extra, &vars, &existing_keys);
    } else if let Some(ref legacy) = overlay.minecraft_arguments {
        let tokens: Vec<String> = legacy.split_whitespace().map(ToString::to_string).collect();
        append_dedup_game_args(&mut game_args, &tokens, &vars, &existing_keys);
    }

    args.extend(game_args);
    Ok(args)
}

fn append_dedup_game_args(
    out: &mut Vec<String>,
    extra: &[String],
    vars: &HashMap<&'static str, String>,
    existing_keys: &HashSet<String>,
) {
    let mut skip_next = false;
    for token in extra {
        if skip_next {
            skip_next = false;
            continue;
        }
        if token.starts_with("--") && existing_keys.contains(token) {
            skip_next = true;
            continue;
        }
        out.push(apply_placeholders(token, vars));
    }
}

fn collect_merged_classpath(
    vanilla_dir: &Path,
    loader: LoaderKind,
    vanilla_manifest: &MojangVersionManifest,
    overlay: &OverlayVersionManifest,
) -> Result<Vec<String>, String> {
    let os_name = current_mojang_os_name();
    let arch = current_mojang_arch();
    let mut ordered_keys: Vec<String> = Vec::new();
    let mut by_key: HashMap<String, String> = HashMap::new();

    for lib in &vanilla_manifest.libraries {
        if !is_library_allowed_for_target(&lib.name, &lib.rules, os_name, arch) {
            continue;
        }
        if let Some(ref art) = lib.downloads.artifact
            && let Some(ref rel) = art.path
        {
            let full = vanilla_dir
                .join("libraries")
                .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
                .to_string_lossy()
                .into_owned();
            let key = maven_group_artifact_key(&lib.name);
            if !by_key.contains_key(&key) {
                ordered_keys.push(key.clone());
            }
            by_key.insert(key, full);
        }
        if let Some(nat) = native_classifier_for_library(lib, os_name)
            && let Some(ref rel) = nat.path
        {
            let full = vanilla_dir
                .join("libraries")
                .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
                .to_string_lossy()
                .into_owned();
            let key = format!("{}::native", maven_group_artifact_key(&lib.name));
            if !by_key.contains_key(&key) {
                ordered_keys.push(key.clone());
            }
            by_key.insert(key, full);
        }
    }

    for lib in &overlay.libraries {
        if !is_library_allowed_for_target(&lib.name, &lib.rules, os_name, arch) {
            continue;
        }
        let rel = if let Some(ref downloads) = lib.downloads
            && let Some(ref art) = downloads.artifact
            && let Some(ref p) = art.path
        {
            Some(p.clone())
        } else if let Some(ref downloads) = lib.downloads
            && !downloads.classifiers.is_empty()
        {
            let classifier_key = match os_name {
                "windows" => ["natives-windows", "natives-windows-64"]
                    .into_iter()
                    .find_map(|k| downloads.classifiers.get(k)),
                "osx" => downloads.classifiers.get("natives-macos").or_else(|| downloads.classifiers.get("natives-osx")),
                _ => downloads.classifiers.get("natives-linux"),
            };
            classifier_key.and_then(|c| c.path.clone())
        } else {
            Some(maven_coord_to_rel_path(&lib.name)?)
        };
        let Some(rel_path) = rel else {
            continue;
        };
        let full_path = vanilla_dir
            .join("libraries")
            .join(rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if !full_path.is_file() {
            return Err(format!(
                "Missing required loader library {} ({}). Re-run loader setup.",
                lib.name,
                full_path.display()
            ));
        }
        let key = maven_group_artifact_key(&lib.name);
        if !by_key.contains_key(&key) {
            ordered_keys.push(key.clone());
        }
        by_key.insert(key, full_path.to_string_lossy().into_owned());
    }

    let mut entries: Vec<String> = ordered_keys
        .into_iter()
        .filter_map(|k| by_key.remove(&k))
        .collect();

    // Modern Forge (1.17+ BootstrapLauncher) remaps and loads the Minecraft client via its own
    // module/library artifacts and excludes ${version_name}.jar via -DignoreList; adding raw Vanilla
    // <mc>.jar on -cp causes JPMS duplicate module ResolutionException crashes.
    let forge_supplies_remapped_client = loader == LoaderKind::Forge
        && overlay.arguments.is_some()
        && overlay.jar.as_deref() != Some(vanilla_manifest.id.as_str())
        && overlay
            .libraries
            .iter()
            .any(|l| l.name.starts_with("net.minecraft:client:") || l.name.starts_with("net.minecraftforge:forge:"));
    if !forge_supplies_remapped_client {
        entries.push(
            client_jar_path_for(vanilla_dir, &vanilla_manifest.id)
                .to_string_lossy()
                .into_owned(),
        );
    }
    Ok(entries)
}

fn build_placeholder_map(
    vanilla_dir: &Path,
    game_dir: &Path,
    vanilla_manifest: &MojangVersionManifest,
    overlay: &OverlayVersionManifest,
    cfg: &VanillaLaunchConfig,
    classpath: &str,
) -> HashMap<&'static str, String> {
    let uuid = if cfg.player_uuid.trim().is_empty() {
        local_player_uuid_dashed(&cfg.username)
    } else {
        cfg.player_uuid.clone()
    };
    let natives_dir = vanilla_dir.join("natives").join(&vanilla_manifest.id);
    let mut map = HashMap::new();
    map.insert("${library_directory}", vanilla_dir.join("libraries").to_string_lossy().into_owned());
    map.insert("${classpath_separator}", if cfg!(windows) { ";" } else { ":" }.to_string());
    map.insert("${version_name}", overlay.id.clone());
    map.insert("${natives_directory}", natives_dir.to_string_lossy().into_owned());
    map.insert("${launcher_name}", "Basalt".to_string());
    map.insert("${launcher_version}", env!("CARGO_PKG_VERSION").to_string());
    map.insert("${classpath}", classpath.to_string());
    map.insert("${auth_player_name}", cfg.username.clone());
    map.insert("${auth_uuid}", uuid);
    map.insert("${auth_access_token}", "0".to_string());
    map.insert("${clientid}", "0".to_string());
    map.insert("${auth_xuid}", "0".to_string());
    map.insert("${user_type}", "legacy".to_string());
    map.insert("${version_type}", "release".to_string());
    map.insert("${game_directory}", game_dir.to_string_lossy().into_owned());
    map.insert("${assets_root}", vanilla_dir.join("assets").to_string_lossy().into_owned());
    map.insert("${assets_index_name}", vanilla_manifest.asset_index.id.clone());
    map
}

pub fn apply_placeholders(input: &str, vars: &HashMap<&'static str, String>) -> String {
    let mut out = input.to_string();
    for (token, value) in vars {
        if out.contains(token) {
            out = out.replace(token, value);
        }
    }
    out
}

pub fn evaluate_argument_values(items: &[Value], os_name: &str, arch: &str) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::String(s) => out.push(s.clone()),
            Value::Object(obj) => {
                let rules: Vec<MojangRule> = obj
                    .get("rules")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                if !is_library_allowed_for_target("", &rules, os_name, arch) {
                    continue;
                }
                match obj.get("value") {
                    Some(Value::String(s)) => out.push(s.clone()),
                    Some(Value::Array(arr)) => {
                        for elem in arr {
                            if let Some(s) = elem.as_str() {
                                out.push(s.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_coordinate_resolution_handles_classifiers_and_extensions() {
        assert_eq!(
            maven_coord_to_rel_path("net.fabricmc:fabric-loader:0.16.10").unwrap(),
            "net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar"
        );
        assert_eq!(
            maven_coord_to_rel_path("org.ow2.asm:asm:9.7.1").unwrap(),
            "org/ow2/asm/asm/9.7.1/asm-9.7.1.jar"
        );
        assert_eq!(
            maven_coord_to_rel_path("net.minecraftforge:forge:1.21.4-54.1.22:shim@zip").unwrap(),
            "net/minecraftforge/forge/1.21.4-54.1.22/forge-1.21.4-54.1.22-shim.zip"
        );
        assert_eq!(
            maven_group_artifact_key("org.ow2.asm:asm:9.7.1"),
            "org.ow2.asm:asm"
        );
        assert_eq!(
            maven_group_artifact_key("net.minecraftforge:forge:1.21.4-54.1.22:universal"),
            "net.minecraftforge:forge:universal"
        );
    }

    #[test]
    fn forge_placeholder_and_rule_evaluation() {
        let mut vars = HashMap::new();
        vars.insert("${library_directory}", "C:\\Basalt\\libraries".to_string());
        vars.insert("${classpath_separator}", ";".to_string());
        vars.insert("${version_name}", "1.21.4-forge-54.1.22".to_string());

        let expanded = apply_placeholders(
            "-p ${library_directory}\\a.jar${classpath_separator}${library_directory}\\b.jar",
            &vars,
        );
        assert_eq!(
            expanded,
            "-p C:\\Basalt\\libraries\\a.jar;C:\\Basalt\\libraries\\b.jar"
        );

        let raw_args = vec![
            Value::String("--launchTarget".to_string()),
            serde_json::json!({
                "rules": [{ "action": "allow", "os": { "name": "windows" } }],
                "value": ["-Dwin=true", "-Darch=ok"]
            }),
            serde_json::json!({
                "rules": [{ "action": "allow", "os": { "name": "osx" } }],
                "value": "-XstartOnFirstThread"
            }),
        ];
        let evaluated = evaluate_argument_values(&raw_args, "windows", "x86");
        assert_eq!(evaluated, vec!["--launchTarget", "-Dwin=true", "-Darch=ok"]);
    }

    #[test]
    fn copy_dir_recursive_if_missing_preserves_source_and_existing_dest() {
        let tmp = std::env::temp_dir().join(format!(
            "basalt_loaders_copy_test_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        let src = tmp.join("run_saves");
        let dst = tmp.join("forge_saves");
        fs::create_dir_all(src.join("WorldA")).unwrap();
        fs::write(src.join("WorldA").join("level.dat"), b"world-a-data").unwrap();
        fs::create_dir_all(dst.join("WorldA")).unwrap();
        fs::write(dst.join("WorldA").join("existing.txt"), b"keep-me").unwrap();
        fs::write(src.join("WorldA").join("existing.txt"), b"do-not-overwrite").unwrap();

        copy_dir_recursive_if_missing(&src, &dst).unwrap();

        // Source must stay completely intact (never moved or deleted).
        assert_eq!(
            fs::read(src.join("WorldA").join("level.dat")).unwrap(),
            b"world-a-data"
        );
        // Missing files copied to destination, existing destination files preserved.
        assert_eq!(
            fs::read(dst.join("WorldA").join("level.dat")).unwrap(),
            b"world-a-data"
        );
        assert_eq!(
            fs::read(dst.join("WorldA").join("existing.txt")).unwrap(),
            b"keep-me"
        );
        let _ = fs::remove_dir_all(&tmp);
    }
}
