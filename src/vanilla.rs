use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::config::{
    GraphicsSettings, JvmGcMode, LoaderKind, SUPPORTED_MC_VERSIONS, normalize_mc_version,
};
use crate::protocol::{
    MINECRAFT_1_21_4_NAME, local_player_uuid_bytes, local_player_uuid_dashed, parse_host_port,
};

pub const MOJANG_VERSION_MANIFEST_V2_URL: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const ASSET_RESOURCES_BASE_URL: &str = "https://resources.download.minecraft.net";
pub const BASALT_SKIN_PACK_NAME: &str = "basalt_skin.zip";

/// Exact `net.minecraft.client.util.DefaultSkinHelper.SKINS` table order in Vanilla 1.21.4.
/// Slots `0..=8` resolve to `Model.SLIM`; slots `9..=17` resolve to `Model.WIDE`.
pub const DEFAULT_SKIN_PATHS: [&str; 18] = [
    "assets/minecraft/textures/entity/player/slim/alex.png",
    "assets/minecraft/textures/entity/player/slim/ari.png",
    "assets/minecraft/textures/entity/player/slim/efe.png",
    "assets/minecraft/textures/entity/player/slim/kai.png",
    "assets/minecraft/textures/entity/player/slim/makena.png",
    "assets/minecraft/textures/entity/player/slim/noor.png",
    "assets/minecraft/textures/entity/player/slim/steve.png",
    "assets/minecraft/textures/entity/player/slim/sunny.png",
    "assets/minecraft/textures/entity/player/slim/zuri.png",
    "assets/minecraft/textures/entity/player/wide/alex.png",
    "assets/minecraft/textures/entity/player/wide/ari.png",
    "assets/minecraft/textures/entity/player/wide/efe.png",
    "assets/minecraft/textures/entity/player/wide/kai.png",
    "assets/minecraft/textures/entity/player/wide/makena.png",
    "assets/minecraft/textures/entity/player/wide/noor.png",
    "assets/minecraft/textures/entity/player/wide/steve.png",
    "assets/minecraft/textures/entity/player/wide/sunny.png",
    "assets/minecraft/textures/entity/player/wide/zuri.png",
];

#[derive(Debug, Clone, Serialize)]
pub struct VanillaEnvStatus {
    pub active_version: String,
    pub installed_versions: Vec<String>,
    pub vanilla_dir: String,
    pub runs_dir: String,
    pub saves_dir: String,
    pub client_ready: bool,
    pub assets_ready: bool,
    pub java_ready: bool,
    pub java_home: Option<String>,
    pub java_version_label: String,
    pub java_hint: String,
}

#[derive(Debug, Clone, Default)]
pub struct JavaDiscovery {
    pub java21: Option<(PathBuf, String)>,
    pub older_detected: Option<String>,
}

fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\")
            && !stripped.starts_with("UNC\\")
        {
            return PathBuf::from(stripped);
        }
    }
    path
}

/// Returns true when every character in the path is strict 7-bit US-ASCII.
pub fn is_ascii_path(path: &Path) -> bool {
    path.to_str().is_some_and(|s| s.is_ascii())
}

fn ascii_fingerprint_12(seed: &Path) -> String {
    let normalized = seed.to_string_lossy().to_ascii_lowercase();
    compute_sha256_hex(std::io::Cursor::new(normalized.as_bytes()))
        .map(|h| h[..12].to_string())
        .unwrap_or_else(|_| "default".to_string())
}

fn ascii_root_candidates(fingerprint: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        // Prefer isolated %SystemDrive%\Basalt\<fp> over world-readable Public before shared folders.
        let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        let clean_drive = sys_drive.trim().trim_end_matches(['\\', '/']);
        if !clean_drive.is_empty() {
            out.push(PathBuf::from(format!("{clean_drive}\\Basalt\\{fingerprint}")));
        }
        for key in ["ProgramData", "ALLUSERSPROFILE", "PUBLIC"] {
            if let Some(val) = std::env::var_os(key) {
                let raw = val.to_string_lossy().trim().to_string();
                if !raw.is_empty() {
                    out.push(PathBuf::from(raw).join("Basalt").join(fingerprint));
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        out.push(std::env::temp_dir().join(format!("basalt-{fingerprint}")));
        out.push(PathBuf::from(format!("/tmp/basalt-{fingerprint}")));
    }
    out
}

fn is_writable_ascii_dir(dir: &Path) -> bool {
    if !is_ascii_path(dir) || fs::create_dir_all(dir).is_err() {
        return false;
    }
    let resolved = fs::canonicalize(dir).map(strip_verbatim_prefix);
    let Ok(canonical) = resolved else {
        return false;
    };
    if !is_ascii_path(&canonical) {
        return false;
    }
    let probe = canonical.join(format!(".basalt-probe-{}", std::process::id()));
    if fs::write(&probe, b"ok").is_err() {
        return false;
    }
    let _ = fs::remove_file(&probe);
    true
}

fn pick_writable_ascii_base_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    for candidate in candidates {
        if is_writable_ascii_dir(candidate) {
            return fs::canonicalize(candidate)
                .map(strip_verbatim_prefix)
                .ok()
                .filter(|p| is_ascii_path(p));
        }
    }
    None
}

/// Returns an ASCII-safe base directory for JVM/Vanilla libraries when `seed_dir`
/// contains non-ASCII characters (for example Turkish `ı` in `%LOCALAPPDATA%`).
pub fn resolve_ascii_safe_base_dir(seed_dir: &Path) -> Option<PathBuf> {
    if is_ascii_path(seed_dir) {
        return Some(seed_dir.to_path_buf());
    }
    let fp = ascii_fingerprint_12(seed_dir);
    let candidates = ascii_root_candidates(&fp);
    pick_writable_ascii_base_dir(&candidates)
}

pub fn resolve_ascii_safe_runtime_dir(app_root_dir: &Path) -> PathBuf {
    if is_ascii_path(app_root_dir) {
        app_root_dir.join("runtime")
    } else if let Some(base) = resolve_ascii_safe_base_dir(app_root_dir) {
        base.join("runtime")
    } else {
        app_root_dir.join("runtime")
    }
}

pub fn resolve_vanilla_dir(app_root_dir: &Path) -> PathBuf {
    if let Ok(raw) = std::env::var("BASALT_VANILLA_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if is_ascii_path(app_root_dir) {
        app_root_dir.join("vanilla")
    } else if let Some(base) = resolve_ascii_safe_base_dir(app_root_dir) {
        base.join("vanilla")
    } else {
        app_root_dir.join("vanilla")
    }
}

/// Migrates any saves created in older Fabric/Gradle `client-gradle/run/saves` directories
/// into the Vanilla `run/saves` folder without overwriting existing worlds.
pub fn migrate_legacy_client_gradle_saves(app_root_dir: &Path, target_saves_dir: &Path) {
    let mut legacy_dirs = vec![
        app_root_dir.join("client-gradle").join("run").join("saves"),
    ];
    if !is_ascii_path(app_root_dir) {
        let fp = ascii_fingerprint_12(app_root_dir);
        for base in ascii_root_candidates(&fp) {
            legacy_dirs.push(base.join("client-gradle").join("run").join("saves"));
        }
    }

    for src_root in legacy_dirs {
        let Ok(entries) = fs::read_dir(&src_root) else {
            continue;
        };
        for entry in entries.flatten() {
            let src = entry.path();
            if !src.is_dir() || !src.join("level.dat").is_file() {
                continue;
            }
            let Some(folder_name) = src.file_name() else {
                continue;
            };
            let dest = target_saves_dir.join(folder_name);
            if dest.exists() {
                continue;
            }
            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if fs::rename(&src, &dest).is_err() {
                let _ = copy_dir_recursive(&src, &dest);
            }
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if src_path.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

pub fn java_executable_name() -> &'static str {
    if cfg!(windows) { "java.exe" } else { "java" }
}

fn compiler_executable_name() -> &'static str {
    if cfg!(windows) { "javac.exe" } else { "javac" }
}

pub fn parse_java_major(home: &Path) -> Option<(u32, String)> {
    let release_path = home.join("release");
    if let Ok(content) = fs::read_to_string(release_path) {
        for line in content.lines() {
            if let Some(rest) = line.strip_prefix("JAVA_VERSION=") {
                let ver = rest.trim().trim_matches('"');
                let mut parts = ver.split(['.', '-', '_', '+']);
                if let Some(first) = parts.next()
                    && let Ok(major) = first.parse::<u32>()
                {
                    // Legacy Java 8 reports JAVA_VERSION="1.8.0_xxx"
                    if major == 1
                        && let Some(second) = parts.next()
                        && let Ok(legacy_major) = second.parse::<u32>()
                    {
                        return Some((legacy_major, ver.to_string()));
                    }
                    return Some((major, ver.to_string()));
                }
            }
        }
    }
    let dir_name = home.file_name()?.to_str()?;
    for digits in dir_name.split(|c: char| !c.is_ascii_digit()) {
        if !digits.is_empty()
            && let Ok(n) = digits.parse::<u32>()
            && (8..=35).contains(&n)
        {
            return Some((n, dir_name.to_string()));
        }
    }
    None
}

fn is_java_home_valid(home: &Path) -> bool {
    home.join("bin").join(java_executable_name()).is_file()
}

pub fn evaluate_jdk_candidates(candidates: &[PathBuf]) -> JavaDiscovery {
    let mut best_21: Option<(u32, PathBuf, String)> = None;
    let mut best_older: Option<(u32, String)> = None;

    for dir in candidates {
        if !is_java_home_valid(dir) {
            continue;
        }
        let Some((major, label)) = parse_java_major(dir) else {
            continue;
        };
        let has_javac = dir.join("bin").join(compiler_executable_name()).is_file();
        if (21..=29).contains(&major) {
            let jdk_bonus = if has_javac { 2000 } else { 0 };
            let version_score = if major == 21 { 1000 } else { 500 + (35 - major) };
            let score = jdk_bonus + version_score;
            if best_21.as_ref().is_none_or(|(b_score, _, _)| score > *b_score) {
                best_21 = Some((score, strip_verbatim_prefix(dir.clone()), label));
            }
        } else if best_older.as_ref().is_none_or(|(b_major, _)| major > *b_major) {
            best_older = Some((major, label));
        }
    }

    JavaDiscovery {
        java21: best_21.map(|(_, path, label)| (path, label)),
        older_detected: best_older.map(|(_, label)| format!("Java {label}")),
    }
}

fn collect_java_candidates(runtime_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(rt) = runtime_dir
        && let Ok(entries) = fs::read_dir(rt)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                candidates.push(path.clone());
                if let Ok(sub_entries) = fs::read_dir(&path) {
                    for sub in sub_entries.flatten() {
                        if sub.path().is_dir() {
                            candidates.push(sub.path());
                        }
                    }
                }
            }
        }
    }

    for env_key in [
        "JAVA_HOME",
        "JAVA_HOME_21_X64",
        "JAVA_HOME_22_X64",
        "JAVA_HOME_23_X64",
        "JAVA_HOME_24_X64",
        "JAVA_HOME_21_ARM64",
        "JAVA_HOME_17_X64",
        "JAVA_HOME_8_X64",
    ] {
        if let Ok(env_jh) = std::env::var(env_key) {
            let cleaned = env_jh.trim().trim_matches('"');
            if !cleaned.is_empty() {
                let p = PathBuf::from(cleaned);
                if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("bin"))
                    && let Some(parent) = p.parent()
                {
                    candidates.push(parent.to_path_buf());
                } else {
                    candidates.push(p);
                }
            }
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for entry in std::env::split_paths(&path_var) {
            if entry.join(java_executable_name()).is_file()
                && let Some(home) = entry.parent()
            {
                candidates.push(home.to_path_buf());
            }
        }
    }

    #[cfg(windows)]
    {
        let mut search_roots = Vec::new();
        for env_key in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA", "USERPROFILE"] {
            if let Ok(val) = std::env::var(env_key) {
                let p = PathBuf::from(val);
                search_roots.push(p.join("Eclipse Adoptium"));
                search_roots.push(p.join("Java"));
                search_roots.push(p.join("Microsoft"));
                search_roots.push(p.join("Zulu"));
                search_roots.push(p.join("BellSoft"));
                search_roots.push(p.join("Amazon Corretto"));
                search_roots.push(p.join("Semeru"));
                search_roots.push(p.join("Programs").join("Eclipse Adoptium"));
                search_roots.push(p.join(".jdks"));
            }
        }
        for root in search_roots {
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        candidates.push(path);
                    }
                }
            }
        }
    }

    candidates
}

pub fn probe_java21_environment(runtime_dir: Option<&Path>) -> JavaDiscovery {
    let candidates = collect_java_candidates(runtime_dir);
    evaluate_jdk_candidates(&candidates)
}

/// Picks the most compatible installed JDK for `mc_version` + `loader`:
/// Forge `1.16.5` prefers Java 8/11/17 when available; Forge `1.17.1-1.20.4` prefers Java 17;
/// all other targets use Java 21+. Falls back cleanly to Java 21 if legacy JDKs aren't present.
pub fn discover_java_for_loader(
    runtime_dir: Option<&Path>,
    mc_version: &str,
    loader: LoaderKind,
) -> Option<(PathBuf, String)> {
    let candidates = collect_java_candidates(runtime_dir);
    if loader == LoaderKind::Forge {
        let preferred_majors: &[u32] = if mc_version == "1.16.5" {
            &[8, 11, 17]
        } else if matches!(mc_version, "1.17.1" | "1.18.2" | "1.19.2" | "1.19.4" | "1.20" | "1.20.1" | "1.20.2" | "1.20.4") {
            &[17, 21]
        } else {
            &[21]
        };
        for &target_major in preferred_majors {
            for dir in &candidates {
                if is_java_home_valid(dir)
                    && let Some((maj, label)) = parse_java_major(dir)
                    && maj == target_major
                {
                    return Some((strip_verbatim_prefix(dir.clone()), label));
                }
            }
        }
    }
    evaluate_jdk_candidates(&candidates).java21
}

pub fn discover_java21_home() -> Option<(PathBuf, String)> {
    let default_runtime = crate::config::AppPaths::resolve().runtime_dir;
    probe_java21_environment(Some(&default_runtime)).java21
}

pub fn discover_java21_home_with_runtime(runtime_dir: &Path) -> Option<(PathBuf, String)> {
    probe_java21_environment(Some(runtime_dir)).java21
}

pub fn inspect_vanilla_environment_with_runtime(
    vanilla_dir: &Path,
    run_dir: &Path,
    runtime_dir: Option<&Path>,
) -> VanillaEnvStatus {
    inspect_vanilla_environment_for_version(vanilla_dir, run_dir, runtime_dir, MINECRAFT_1_21_4_NAME)
}

pub fn inspect_vanilla_environment_for_version(
    vanilla_dir: &Path,
    run_dir: &Path,
    runtime_dir: Option<&Path>,
    version: &str,
) -> VanillaEnvStatus {
    let active_version = normalize_mc_version(version);
    let saves_dir = run_dir.join("saves");
    let client_ready = check_vanilla_client_ready_for(vanilla_dir, &active_version);
    let assets_ready = check_vanilla_assets_ready_for(vanilla_dir, &active_version);
    let installed_versions: Vec<String> = SUPPORTED_MC_VERSIONS
        .iter()
        .copied()
        .filter(|v| check_vanilla_client_ready_for(vanilla_dir, v))
        .map(str::to_string)
        .collect();

    let discovery = probe_java21_environment(runtime_dir);
    let (java_ready, java_home, java_version_label, java_hint) = match discovery.java21 {
        Some((p, ver)) => {
            let is_portable = runtime_dir.is_some_and(|rt| p.starts_with(rt));
            let hint = if is_portable {
                "Portable Basalt Java 21 installation detected and ready.".to_string()
            } else {
                "Compatible system Java 21+ runtime detected.".to_string()
            };
            (
                true,
                Some(p.to_string_lossy().into_owned()),
                format!("Java {ver}"),
                hint,
            )
        }
        None => {
            let (label, hint) = if let Some(older) = discovery.older_detected {
                (
                    format!("Missing ({older} too old)"),
                    format!(
                        "Only {older} was found on this system; Vanilla Minecraft {active_version} requires Java 21+. Install it in one click below."
                    ),
                )
            } else {
                (
                    "Java 21 runtime not found".to_string(),
                    "No Java 21 runtime was detected on this PC. Click Install Java 21 below to set up a portable runtime without administrator rights."
                        .to_string(),
                )
            };
            (false, None, label, hint)
        }
    };

    VanillaEnvStatus {
        active_version,
        installed_versions,
        vanilla_dir: vanilla_dir.to_string_lossy().into_owned(),
        runs_dir: run_dir.to_string_lossy().into_owned(),
        saves_dir: saves_dir.to_string_lossy().into_owned(),
        client_ready,
        assets_ready,
        java_ready,
        java_home,
        java_version_label,
        java_hint,
    }
}

#[derive(Debug, Deserialize)]
struct VersionManifestV2 {
    versions: Vec<VersionManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct VersionManifestEntry {
    id: String,
    url: String,
    sha1: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangArtifactInfo {
    #[serde(default)]
    pub path: Option<String>,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangVersionDownloads {
    pub client: MojangArtifactInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangAssetIndexInfo {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangLoggingClient {
    pub file: MojangLoggingFile,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangLoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangLoggingSection {
    pub client: Option<MojangLoggingClient>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangRuleOs {
    pub name: Option<String>,
    pub arch: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangRule {
    pub action: String,
    pub os: Option<MojangRuleOs>,
    #[serde(default)]
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangLibDownloads {
    pub artifact: Option<MojangArtifactInfo>,
    #[serde(default)]
    pub classifiers: HashMap<String, MojangArtifactInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MojangLibrary {
    pub name: String,
    pub downloads: MojangLibDownloads,
    #[serde(default)]
    pub rules: Vec<MojangRule>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MojangVersionManifest {
    pub id: String,
    pub main_class: String,
    pub asset_index: MojangAssetIndexInfo,
    pub downloads: MojangVersionDownloads,
    pub logging: Option<MojangLoggingSection>,
    pub libraries: Vec<MojangLibrary>,
}

#[derive(Debug, Deserialize)]
struct MojangAssetObjectEntry {
    hash: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct MojangAssetIndexFile {
    objects: HashMap<String, MojangAssetObjectEntry>,
}

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub dest: PathBuf,
    pub sha1: String,
    pub size: u64,
    pub label: String,
}

pub fn current_mojang_os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

pub fn current_mojang_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

/// Evaluates Mojang library `rules` + native classifier suffixes for a target OS/architecture.
pub fn is_library_allowed_for_target(
    lib_name: &str,
    rules: &[MojangRule],
    os_name: &str,
    arch: &str,
) -> bool {
    let mut allowed = rules.is_empty();
    for rule in rules {
        if let Some(ref feat) = rule.features
            && !feat.is_empty()
        {
            continue;
        }
        let os_matches = match rule.os {
            None => true,
            Some(ref os) => {
                let name_ok = os.name.as_deref().is_none_or(|n| n == os_name);
                let arch_ok = os.arch.as_deref().is_none_or(|a| match a {
                    "x86_64" | "amd64" | "x64" => arch == "x64",
                    "x86" => arch == "x86",
                    "arm64" | "aarch64" => arch == "arm64",
                    other => other == arch,
                });
                name_ok && arch_ok
            }
        };
        if os_matches {
            allowed = rule.action == "allow";
        }
    }
    if !allowed {
        return false;
    }

    if os_name == "windows" {
        match arch {
            "x64" => {
                if lib_name.ends_with(":natives-windows-x86")
                    || lib_name.ends_with(":natives-windows-arm64")
                {
                    return false;
                }
            }
            "arm64" => {
                if lib_name.ends_with(":natives-windows-x86")
                    || lib_name.ends_with(":natives-windows")
                {
                    return false;
                }
            }
            "x86" => {
                if lib_name.ends_with(":natives-windows-arm64")
                    || lib_name.ends_with(":natives-windows")
                {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

pub fn native_classifier_for_library<'a>(
    lib: &'a MojangLibrary,
    os_name: &str,
) -> Option<&'a MojangArtifactInfo> {
    if lib.downloads.classifiers.is_empty() {
        return None;
    }
    let keys: &[&str] = match os_name {
        "windows" => &["natives-windows", "natives-windows-64", "natives-windows-32"],
        "osx" => &["natives-macos", "natives-osx"],
        _ => &["natives-linux"],
    };
    for &k in keys {
        if let Some(info) = lib.downloads.classifiers.get(k) {
            return Some(info);
        }
    }
    None
}

pub fn version_json_path_for(vanilla_dir: &Path, version: &str) -> PathBuf {
    vanilla_dir
        .join("versions")
        .join(version)
        .join(format!("{version}.json"))
}

pub fn client_jar_path_for(vanilla_dir: &Path, version: &str) -> PathBuf {
    vanilla_dir
        .join("versions")
        .join(version)
        .join(format!("{version}.jar"))
}

pub fn load_cached_version_manifest_for(
    vanilla_dir: &Path,
    version: &str,
) -> Option<MojangVersionManifest> {
    let raw = fs::read_to_string(version_json_path_for(vanilla_dir, version)).ok()?;
    let manifest: MojangVersionManifest = serde_json::from_str(&raw).ok()?;
    if manifest.id != version
        || manifest.libraries.is_empty()
        || manifest.main_class.trim().is_empty()
    {
        return None;
    }
    Some(manifest)
}

fn check_file_size(path: &Path, expected_size: u64) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == expected_size)
}

pub fn check_file_size_and_sha1(path: &Path, expected_size: u64, expected_sha1: &str) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    if expected_size > 0 && meta.len() != expected_size {
        return false;
    }
    if meta.len() == 0 {
        return false;
    }
    if expected_sha1.trim().is_empty() {
        return true;
    }
    compute_file_sha1_hex(path)
        .is_ok_and(|actual| actual.eq_ignore_ascii_case(expected_sha1))
}

fn natives_dir_populated(natives_dir: &Path) -> bool {
    let Ok(entries) = fs::read_dir(natives_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && fs::metadata(&path).is_ok_and(|m| m.len() > 0) {
            return true;
        }
        if path.is_dir()
            && !entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("META-INF")
            && natives_dir_populated(&path)
        {
            return true;
        }
    }
    false
}

pub fn check_vanilla_client_ready_for(vanilla_dir: &Path, version: &str) -> bool {
    let Some(manifest) = load_cached_version_manifest_for(vanilla_dir, version) else {
        return false;
    };
    if !check_file_size(
        &client_jar_path_for(vanilla_dir, version),
        manifest.downloads.client.size,
    ) {
        return false;
    }
    if let Some(ref logging) = manifest.logging
        && let Some(ref client_log) = logging.client
    {
        let log_path = vanilla_dir.join("logging").join(&client_log.file.id);
        if !check_file_size(&log_path, client_log.file.size) {
            return false;
        }
    }
    let os_name = current_mojang_os_name();
    let arch = current_mojang_arch();
    let mut requires_natives = false;
    for lib in &manifest.libraries {
        if !is_library_allowed_for_target(&lib.name, &lib.rules, os_name, arch) {
            continue;
        }
        if let Some(ref art) = lib.downloads.artifact
            && let Some(ref rel) = art.path
        {
            let p = vanilla_dir
                .join("libraries")
                .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if !check_file_size(&p, art.size) {
                return false;
            }
        }
        if let Some(nat) = native_classifier_for_library(lib, os_name)
            && let Some(ref rel) = nat.path
        {
            requires_natives = true;
            let p = vanilla_dir
                .join("libraries")
                .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if !check_file_size(&p, nat.size) {
                return false;
            }
        }
    }
    if requires_natives && !natives_dir_populated(&vanilla_dir.join("natives").join(version)) {
        return false;
    }
    true
}

fn is_valid_asset_sha1(hash: &str) -> bool {
    hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn resolve_curl_exe() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")) {
            let sys = PathBuf::from(root).join("System32").join("curl.exe");
            if sys.is_file() {
                return sys;
            }
        }
    }
    PathBuf::from("curl.exe")
}

pub fn resolve_tar_exe() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(root) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")) {
            let sys = PathBuf::from(root).join("System32").join("tar.exe");
            if sys.is_file() {
                return sys;
            }
        }
    }
    PathBuf::from("tar.exe")
}

pub fn check_vanilla_assets_ready_for(vanilla_dir: &Path, version: &str) -> bool {
    let Some(manifest) = load_cached_version_manifest_for(vanilla_dir, version) else {
        return false;
    };
    let idx_path = vanilla_dir
        .join("assets")
        .join("indexes")
        .join(format!("{}.json", manifest.asset_index.id));
    if !check_file_size(&idx_path, manifest.asset_index.size) {
        return false;
    }
    let Ok(idx_raw) = fs::read_to_string(&idx_path) else {
        return false;
    };
    let Ok(idx) = serde_json::from_str::<MojangAssetIndexFile>(&idx_raw) else {
        return false;
    };
    if idx.objects.is_empty() {
        return false;
    }
    let mut seen = HashSet::new();
    for obj in idx.objects.values() {
        if !is_valid_asset_sha1(&obj.hash) || !seen.insert(obj.hash.clone()) {
            continue;
        }
        let obj_path = vanilla_dir
            .join("assets")
            .join("objects")
            .join(&obj.hash[..2])
            .join(&obj.hash);
        if !check_file_size(&obj_path, obj.size) {
            return false;
        }
    }
    true
}

#[cfg(windows)]
pub fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn hide_window(_cmd: &mut Command) {}

#[cfg(windows)]
pub fn hide_window_with_priority(cmd: &mut Command, high_priority: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;
    let flags = if high_priority {
        CREATE_NO_WINDOW | ABOVE_NORMAL_PRIORITY_CLASS
    } else {
        CREATE_NO_WINDOW
    };
    cmd.creation_flags(flags);
}

#[cfg(not(windows))]
pub fn hide_window_with_priority(_cmd: &mut Command, _high_priority: bool) {}

pub fn curl_fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    curl_fetch_bytes_with_ua(url, None)
}

pub fn curl_fetch_bytes_with_ua(url: &str, user_agent: Option<&str>) -> Result<Vec<u8>, String> {
    let mut caps = CurlCapabilities::detect();
    let curl_exe = resolve_curl_exe();
    for attempt in 0..2 {
        let mut cmd = Command::new(&curl_exe);
        cmd.args([
            "-fsSL",
            "--retry",
            "2",
            "--connect-timeout",
            "20",
            "--speed-limit",
            "1024",
            "--speed-time",
            "30",
            "--max-time",
            "90",
        ]);
        if caps.supports_ssl_revoke_best_effort {
            cmd.arg("--ssl-revoke-best-effort");
        }
        if let Some(ua) = user_agent {
            cmd.arg("-A").arg(ua);
        }
        cmd.arg(url).stdin(Stdio::null());
        hide_window(&mut cmd);
        let out = cmd
            .output()
            .map_err(|e| format!("Failed to invoke curl.exe for {url}: {e}"))?;
        if out.status.success() {
            return Ok(out.stdout);
        }
        if attempt == 0 && out.status.code() == Some(2) && caps.supports_ssl_revoke_best_effort {
            caps.supports_ssl_revoke_best_effort = false;
            continue;
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "Download failed ({url}, exit code {:?}): {stderr}",
            out.status.code()
        ));
    }
    Err(format!("Download failed for {url}"))
}

fn escape_curl_config_value(s: &str) -> String {
    if s.starts_with("\\\\") {
        // Preserve Windows UNC prefix `\\server\share`
        return s.replace('\\', "\\\\").replace('"', "\\\"");
    }
    s.replace('\\', "/").replace('"', "\\\"")
}

/// Atomically moves `part_path` into `dest`, retrying briefly if Windows Defender holds a sharing lock.
pub fn commit_verified_part(part_path: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent folder {}: {e}", parent.display()))?;
    }
    for retry in 0..8 {
        let _ = fs::remove_file(dest);
        if fs::rename(part_path, dest).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(40 + retry * 20));
    }
    let stage = dest.with_extension("basalt_tmp");
    let _ = fs::remove_file(&stage);
    fs::copy(part_path, &stage).map_err(|e| {
        let _ = fs::remove_file(&stage);
        format!("Failed to copy verified file into {}: {e}", dest.display())
    })?;
    let _ = fs::remove_file(dest);
    fs::rename(&stage, dest).map_err(|e| {
        let _ = fs::remove_file(&stage);
        format!("Failed to move verified file into {}: {e}", dest.display())
    })?;
    let _ = fs::remove_file(part_path);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurlCapabilities {
    pub supports_http2: bool,
    pub supports_parallel: bool,
    pub supports_ssl_revoke_best_effort: bool,
}

impl CurlCapabilities {
    pub fn parse_version_output(output: &str) -> Self {
        let mut supports_http2 = false;
        let mut supports_parallel = false;
        let mut supports_ssl_revoke_best_effort = false;

        for line in output.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("curl ") {
                let ver_str = rest.split_whitespace().next().unwrap_or("");
                let mut parts = ver_str.split('.').filter_map(|p| p.parse::<u32>().ok());
                if let (Some(maj), Some(min)) = (parts.next(), parts.next()) {
                    supports_parallel = (maj, min) >= (7, 66);
                    if trimmed.contains("Schannel") && (maj, min) >= (7, 70) {
                        supports_ssl_revoke_best_effort = true;
                    }
                } else if trimmed.contains("Schannel") {
                    supports_ssl_revoke_best_effort = true;
                }
            } else if let Some(rest) = trimmed.strip_prefix("Features:") {
                supports_http2 = rest
                    .split_whitespace()
                    .any(|feat| feat.eq_ignore_ascii_case("HTTP2"));
            }
        }

        Self {
            supports_http2,
            supports_parallel,
            supports_ssl_revoke_best_effort,
        }
    }

    pub fn detect() -> Self {
        static CACHE: std::sync::OnceLock<CurlCapabilities> = std::sync::OnceLock::new();
        *CACHE.get_or_init(|| {
            let mut cmd = Command::new(resolve_curl_exe());
            cmd.arg("--version").stdin(Stdio::null());
            hide_window(&mut cmd);
            if let Ok(out) = cmd.output()
                && out.status.success()
            {
                let text = String::from_utf8_lossy(&out.stdout);
                return Self::parse_version_output(&text);
            }
            Self {
                supports_http2: false,
                supports_parallel: false,
                supports_ssl_revoke_best_effort: false,
            }
        })
    }
}

static BATCH_TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

struct ScopedTempDir(PathBuf);

impl Drop for ScopedTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn build_curl_transfer_args(
    caps: CurlCapabilities,
    parallel_max: Option<&str>,
    connect_timeout_secs: &str,
    create_dirs: bool,
) -> Vec<String> {
    let mut args = vec![
        "-sS".to_string(),
        "-fL".to_string(),
        "--tcp-nodelay".to_string(),
        "--speed-limit".to_string(),
        "1024".to_string(),
        "--speed-time".to_string(),
        "35".to_string(),
    ];
    if caps.supports_ssl_revoke_best_effort {
        args.push("--ssl-revoke-best-effort".to_string());
    }
    if caps.supports_http2 {
        args.push("--http2".to_string());
    }
    if caps.supports_parallel {
        args.push("--parallel".to_string());
        if let Some(max_s) = parallel_max {
            args.push("--parallel-max".to_string());
            args.push(max_s.to_string());
        }
    }
    args.push("--retry".to_string());
    args.push("2".to_string());
    args.push("--connect-timeout".to_string());
    args.push(connect_timeout_secs.to_string());
    if create_dirs {
        args.push("--create-dirs".to_string());
    }
    args
}

/// Splits a large file (`>= 4 MB`) into `segments` concurrent HTTP `Range` requests (IDM-style multi-segment download),
/// stitches the segments in order, and verifies the resulting SHA-1 digest.
fn download_segmented_with_curl(
    task: &DownloadTask,
    temp_dir: &Path,
    segments: usize,
) -> Result<bool, String> {
    let caps = CurlCapabilities::detect();
    if !caps.supports_parallel {
        return Ok(false);
    }
    let seg_count = segments.clamp(2, 16) as u64;
    if task.size < 4 * 1024 * 1024 {
        return Ok(false);
    }
    fs::create_dir_all(temp_dir)
        .map_err(|e| format!("Failed to create segmented temp dir: {e}"))?;

    let chunk_size = task.size.div_ceil(seg_count);
    let cfg_path = temp_dir.join(format!("curl-idm-{}.cfg", std::process::id()));
    let mut cfg = String::with_capacity((seg_count as usize) * 220);
    let mut seg_paths = Vec::with_capacity(seg_count as usize);

    for i in 0..seg_count {
        let start = i * chunk_size;
        if start >= task.size {
            break;
        }
        let end = ((i + 1) * chunk_size - 1).min(task.size - 1);
        let seg_path = temp_dir.join(format!("seg-{}-{i:02}.part", std::process::id()));
        let _ = fs::remove_file(&seg_path);
        cfg.push_str(&format!(
            "url = \"{}\"\noutput = \"{}\"\nrange = \"{start}-{end}\"\n",
            escape_curl_config_value(&task.url),
            escape_curl_config_value(&seg_path.to_string_lossy())
        ));
        seg_paths.push((seg_path, end - start + 1));
    }

    fs::write(&cfg_path, cfg.as_bytes())
        .map_err(|e| format!("Failed to write segmented curl config: {e}"))?;

    let seg_max_str = seg_paths.len().to_string();
    let args = build_curl_transfer_args(caps, Some(&seg_max_str), "15", false);
    let mut cmd = Command::new(resolve_curl_exe());
    cmd.args(&args)
        .arg("-K")
        .arg(&cfg_path)
        .stdin(Stdio::null());
    hide_window(&mut cmd);
    let out = cmd.output();
    let _ = fs::remove_file(&cfg_path);

    let Ok(out) = out else {
        for (p, _) in &seg_paths {
            let _ = fs::remove_file(p);
        }
        return Ok(false);
    };
    if !out.status.success() {
        for (p, _) in &seg_paths {
            let _ = fs::remove_file(p);
        }
        return Ok(false);
    }

    // Verify each segment's exact byte length before stitching
    for (p, expected_len) in &seg_paths {
        if !check_file_size(p, *expected_len) {
            for (sp, _) in &seg_paths {
                let _ = fs::remove_file(sp);
            }
            return Ok(false);
        }
    }

    let joined_path = temp_dir.join(format!("joined-{}.part", std::process::id()));
    let _ = fs::remove_file(&joined_path);
    {
        let mut out_file = File::create(&joined_path)
            .map_err(|e| format!("Failed to create joined file: {e}"))?;
        let mut buf = vec![0u8; 131_072];
        for (p, _) in &seg_paths {
            let mut seg_file =
                File::open(p).map_err(|e| format!("Failed to open segment file: {e}"))?;
            loop {
                let n = seg_file
                    .read(&mut buf)
                    .map_err(|e| format!("Failed to read segment file: {e}"))?;
                if n == 0 {
                    break;
                }
                out_file
                    .write_all(&buf[..n])
                    .map_err(|e| format!("Failed to write joined file: {e}"))?;
            }
            let _ = fs::remove_file(p);
        }
    }

    if check_file_size_and_sha1(&joined_path, task.size, &task.sha1) {
        commit_verified_part(&joined_path, &task.dest)?;
        Ok(true)
    } else {
        let _ = fs::remove_file(&joined_path);
        Ok(false)
    }
}

pub fn download_batch_with_curl(
    tasks: &[&DownloadTask],
    temp_dir: &Path,
    parallel_streams: u8,
) -> Result<(), String> {
    if tasks.is_empty() {
        return Ok(());
    }
    let seq = BATCH_TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let work_dir = temp_dir.join(format!("b-{}-{seq}", std::process::id()));
    fs::create_dir_all(&work_dir)
        .map_err(|e| format!("Failed to create temp download dir: {e}"))?;
    let _guard = ScopedTempDir(work_dir.clone());

    let mut pending: Vec<usize> = Vec::with_capacity(tasks.len());
    for (idx, &t) in tasks.iter().enumerate() {
        if t.size >= 4 * 1024 * 1024
            && download_segmented_with_curl(t, &work_dir, 8).unwrap_or(false)
        {
            continue;
        }
        pending.push(idx);
    }
    if pending.is_empty() {
        return Ok(());
    }

    let mut last_err = String::new();
    let mut caps = CurlCapabilities::detect();
    let max_streams = parallel_streams.clamp(4, 32).to_string();
    let curl_exe = resolve_curl_exe();

    for attempt in 0..3 {
        let cfg_path = work_dir.join(format!("curl-batch-{attempt}.cfg"));
        let mut cfg = String::with_capacity(pending.len() * 192);
        for &idx in &pending {
            let t = tasks[idx];
            let part_path = work_dir.join(format!("dl-{idx:04}.part"));
            let _ = fs::remove_file(&part_path);
            cfg.push_str(&format!(
                "url = \"{}\"\noutput = \"{}\"\n",
                escape_curl_config_value(&t.url),
                escape_curl_config_value(&part_path.to_string_lossy())
            ));
        }
        fs::write(&cfg_path, cfg.as_bytes())
            .map_err(|e| format!("Failed to write curl batch config: {e}"))?;

        let parallel_max = if attempt == 0 {
            max_streams.as_str()
        } else {
            "6"
        };
        let args = build_curl_transfer_args(caps, Some(parallel_max), "20", true);
        let mut cmd = Command::new(&curl_exe);
        cmd.args(&args)
            .arg("-K")
            .arg(&cfg_path)
            .stdin(Stdio::null());
        hide_window(&mut cmd);
        let out = cmd
            .output()
            .map_err(|e| format!("Failed to invoke curl.exe parallel batch: {e}"))?;
        let _ = fs::remove_file(&cfg_path);

        if !out.status.success() {
            // If curl exits with code 2 (unrecognized/unsupported flag), drop optional HTTP/2 + parallel flags on retry
            if out.status.code() == Some(2) {
                caps = CurlCapabilities {
                    supports_http2: false,
                    supports_parallel: false,
                    supports_ssl_revoke_best_effort: false,
                };
            }
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            last_err = format!(
                "curl exit code {:?}{}",
                out.status.code(),
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            );
        }

        let mut still_pending = Vec::new();
        for &idx in &pending {
            let t = tasks[idx];
            let part_path = work_dir.join(format!("dl-{idx:04}.part"));
            if check_file_size_and_sha1(&part_path, t.size, &t.sha1) {
                commit_verified_part(&part_path, &t.dest)?;
            } else {
                let _ = fs::remove_file(&part_path);
                still_pending.push(idx);
            }
        }

        if still_pending.is_empty() {
            return Ok(());
        }
        pending = still_pending;
    }

    let first_failed = tasks[pending[0]].label.clone();
    Err(format!(
        "Failed to download/verify {} file(s) starting with {first_failed}{}",
        pending.len(),
        if last_err.is_empty() {
            String::new()
        } else {
            format!(" ({last_err})")
        }
    ))
}

fn local_seed_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        out.push(PathBuf::from(appdata).join(".minecraft"));
    }
    out
}

fn find_local_cached_client_jar_for(version: &str, sha1: &str, size: u64) -> Option<PathBuf> {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let mc_jar = PathBuf::from(appdata)
            .join(".minecraft")
            .join("versions")
            .join(version)
            .join(format!("{version}.jar"));
        if check_file_size_and_sha1(&mc_jar, size, sha1) {
            return Some(mc_jar);
        }
    }
    for env_key in ["USERPROFILE", "HOME"] {
        if let Some(profile) = std::env::var_os(env_key) {
            let loom_jar = PathBuf::from(profile)
                .join(".gradle")
                .join("caches")
                .join("fabric-loom")
                .join(version)
                .join("minecraft-client.jar");
            if check_file_size_and_sha1(&loom_jar, size, sha1) {
                return Some(loom_jar);
            }
        }
    }
    None
}

fn find_local_cached_asset(hash: &str, size: u64) -> Option<PathBuf> {
    let prefix = hash.get(..2)?;
    let mut roots = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        roots.push(PathBuf::from(appdata).join(".minecraft").join("assets"));
    }
    for env_key in ["USERPROFILE", "HOME"] {
        if let Some(profile) = std::env::var_os(env_key) {
            roots.push(
                PathBuf::from(profile)
                    .join(".gradle")
                    .join("caches")
                    .join("fabric-loom")
                    .join("assets"),
            );
        }
    }
    for root in roots {
        let candidate = root.join("objects").join(prefix).join(hash);
        if check_file_size_and_sha1(&candidate, size, hash) {
            return Some(candidate);
        }
    }
    None
}

/// Ensures official Vanilla Minecraft 1.21.4 manifest, client JAR, libraries, logging XML,
/// and assets are downloaded, SHA-1 verified, and cached inside `vanilla_dir`.
pub fn ensure_vanilla_1_21_4_ready<F>(
    vanilla_dir: &Path,
    log: F,
) -> Result<MojangVersionManifest, String>
where
    F: FnMut(&str, bool),
{
    ensure_vanilla_version_ready(vanilla_dir, MINECRAFT_1_21_4_NAME, log)
}

/// Ensures official Vanilla Minecraft `version` manifest, client JAR, libraries, logging XML,
/// and assets are downloaded, SHA-1 verified, and cached inside `vanilla_dir`.
pub fn ensure_vanilla_version_ready<F>(
    vanilla_dir: &Path,
    version: &str,
    log: F,
) -> Result<MojangVersionManifest, String>
where
    F: FnMut(&str, bool),
{
    ensure_vanilla_version_ready_with_threads(vanilla_dir, version, 24, log)
}

/// Ensures official Vanilla Minecraft `version` is ready using `parallel_streams` IDM-style worker connections.
pub fn ensure_vanilla_version_ready_with_threads<F>(
    vanilla_dir: &Path,
    version: &str,
    parallel_streams: u8,
    mut log: F,
) -> Result<MojangVersionManifest, String>
where
    F: FnMut(&str, bool),
{
    let ver = normalize_mc_version(version);
    fs::create_dir_all(vanilla_dir)
        .map_err(|e| format!("Failed to create Vanilla data directory: {e}"))?;
    let natives_dir = vanilla_dir.join("natives").join(&ver);
    let _ = fs::create_dir_all(&natives_dir);

    let vjson_path = version_json_path_for(vanilla_dir, &ver);
    let manifest: MojangVersionManifest = match load_cached_version_manifest_for(vanilla_dir, &ver)
    {
        Some(m) => m,
        None => {
            log(
                &format!("Fetching official Mojang Minecraft {ver} version manifest..."),
                false,
            );
            let index_bytes = curl_fetch_bytes(MOJANG_VERSION_MANIFEST_V2_URL)?;
            let v2: VersionManifestV2 = serde_json::from_slice(&index_bytes)
                .map_err(|e| format!("Failed to parse Mojang version_manifest_v2.json: {e}"))?;
            let entry = v2
                .versions
                .into_iter()
                .find(|v| v.id == ver)
                .ok_or_else(|| {
                    format!("Minecraft {ver} was not found in Mojang version manifest.")
                })?;
            let ver_bytes = curl_fetch_bytes(&entry.url)?;
            let actual_sha = compute_sha1_hex(std::io::Cursor::new(&ver_bytes))
                .map_err(|e| format!("Failed to hash {ver}.json: {e}"))?;
            if !actual_sha.eq_ignore_ascii_case(&entry.sha1) {
                return Err(format!("SHA-1 verification failed for Mojang {ver}.json."));
            }
            if let Some(parent) = vjson_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create version directory: {e}"))?;
            }
            fs::write(&vjson_path, &ver_bytes)
                .map_err(|e| format!("Failed to write {ver}.json: {e}"))?;
            serde_json::from_slice(&ver_bytes)
                .map_err(|e| format!("Failed to parse {ver}.json: {e}"))?
        }
    };

    // Asset index JSON
    let idx_path = vanilla_dir
        .join("assets")
        .join("indexes")
        .join(format!("{}.json", manifest.asset_index.id));
    if !check_file_size_and_sha1(&idx_path, manifest.asset_index.size, &manifest.asset_index.sha1) {
        let mut seeded = false;
        for root in local_seed_roots() {
            let local_idx = root
                .join("assets")
                .join("indexes")
                .join(format!("{}.json", manifest.asset_index.id));
            if check_file_size_and_sha1(
                &local_idx,
                manifest.asset_index.size,
                &manifest.asset_index.sha1,
            ) {
                if let Some(parent) = idx_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if fs::copy(&local_idx, &idx_path).is_ok() {
                    seeded = true;
                    break;
                }
            }
        }
        if !seeded {
            log(
                &format!(
                    "Downloading Minecraft asset index ({}.json)...",
                    manifest.asset_index.id
                ),
                false,
            );
            let idx_bytes = curl_fetch_bytes(&manifest.asset_index.url)?;
            let actual_sha = compute_sha1_hex(std::io::Cursor::new(&idx_bytes))
                .map_err(|e| format!("Failed to hash asset index: {e}"))?;
            if !actual_sha.eq_ignore_ascii_case(&manifest.asset_index.sha1) {
                return Err("SHA-1 verification failed for asset index JSON.".to_string());
            }
            if let Some(parent) = idx_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create asset index dir: {e}"))?;
            }
            fs::write(&idx_path, &idx_bytes)
                .map_err(|e| format!("Failed to write asset index JSON: {e}"))?;
        }
    }

    let mut missing_tasks: Vec<DownloadTask> = Vec::new();
    let mut reused_count: usize = 0;

    // 1. Client JAR
    let jar_dest = client_jar_path_for(vanilla_dir, &ver);
    if !check_file_size(&jar_dest, manifest.downloads.client.size) {
        if let Some(cached) = find_local_cached_client_jar_for(
            &ver,
            &manifest.downloads.client.sha1,
            manifest.downloads.client.size,
        ) {
            if let Some(parent) = jar_dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::copy(&cached, &jar_dest)
                .map_err(|e| format!("Failed to copy cached {ver} client.jar: {e}"))?;
            reused_count += 1;
        } else {
            missing_tasks.push(DownloadTask {
                url: manifest.downloads.client.url.clone(),
                dest: jar_dest,
                sha1: manifest.downloads.client.sha1.clone(),
                size: manifest.downloads.client.size,
                label: format!("Vanilla {ver} client.jar"),
            });
        }
    }

    // 2. Log4j logging config XML
    if let Some(ref logging) = manifest.logging
        && let Some(ref client_log) = logging.client
    {
        let log_dest = vanilla_dir.join("logging").join(&client_log.file.id);
        if !check_file_size(&log_dest, client_log.file.size) {
            missing_tasks.push(DownloadTask {
                url: client_log.file.url.clone(),
                dest: log_dest,
                sha1: client_log.file.sha1.clone(),
                size: client_log.file.size,
                label: client_log.file.id.clone(),
            });
        }
    }

    // 3. Libraries & legacy native classifiers
    let os_name = current_mojang_os_name();
    let arch = current_mojang_arch();
    let seed_roots = local_seed_roots();
    for lib in &manifest.libraries {
        if !is_library_allowed_for_target(&lib.name, &lib.rules, os_name, arch) {
            continue;
        }
        let mut lib_artifacts: Vec<&MojangArtifactInfo> = Vec::with_capacity(2);
        if let Some(ref art) = lib.downloads.artifact {
            lib_artifacts.push(art);
        }
        if let Some(nat) = native_classifier_for_library(lib, os_name) {
            lib_artifacts.push(nat);
        }
        for art in lib_artifacts {
            let Some(ref rel) = art.path else {
                continue;
            };
            let dest = vanilla_dir
                .join("libraries")
                .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if check_file_size(&dest, art.size) {
                continue;
            }
            let mut copied = false;
            for root in &seed_roots {
                let local_lib = root
                    .join("libraries")
                    .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                if check_file_size_and_sha1(&local_lib, art.size, &art.sha1) {
                    if let Some(parent) = dest.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if fs::copy(&local_lib, &dest).is_ok() {
                        reused_count += 1;
                        copied = true;
                        break;
                    }
                }
            }
            if !copied {
                missing_tasks.push(DownloadTask {
                    url: art.url.clone(),
                    dest,
                    sha1: art.sha1.clone(),
                    size: art.size,
                    label: rel.clone(),
                });
            }
        }
    }

    // 4. Asset objects
    let idx_raw = fs::read_to_string(&idx_path)
        .map_err(|e| format!("Failed to read asset index {}: {e}", idx_path.display()))?;
    let idx: MojangAssetIndexFile = serde_json::from_str(&idx_raw)
        .map_err(|e| format!("Failed to parse asset index JSON: {e}"))?;

    let mut seen_hashes = HashSet::new();
    for obj in idx.objects.values() {
        if !is_valid_asset_sha1(&obj.hash) || !seen_hashes.insert(obj.hash.clone()) {
            continue;
        }
        let dest = vanilla_dir
            .join("assets")
            .join("objects")
            .join(&obj.hash[..2])
            .join(&obj.hash);
        if check_file_size(&dest, obj.size) {
            continue;
        }
        if let Some(local_asset) = find_local_cached_asset(&obj.hash, obj.size) {
            if let Some(parent) = dest.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if fs::copy(&local_asset, &dest).is_ok() {
                reused_count += 1;
                continue;
            }
        }
        missing_tasks.push(DownloadTask {
            url: format!("{ASSET_RESOURCES_BASE_URL}/{}/{}", &obj.hash[..2], obj.hash),
            dest,
            sha1: obj.hash.clone(),
            size: obj.size,
            label: format!("asset {}", &obj.hash[..8]),
        });
    }

    if reused_count > 0 {
        log(
            &format!("Reused {reused_count} verified file(s) from local Minecraft caches."),
            false,
        );
    }

    let stamp = vanilla_dir
        .join("assets")
        .join(format!(".verified-{}.ok", manifest.asset_index.id));

    if !missing_tasks.is_empty() {
        let _ = fs::remove_file(&stamp);
        let total = missing_tasks.len();
        let streams = parallel_streams.clamp(4, 32);
        log(
            &format!(
                "[Downloader · {streams} threads] Downloading {total} verified Vanilla {ver} file(s) from Mojang CDN..."
            ),
            false,
        );
        let temp_dir = vanilla_dir.join("tmp");
        const CHUNK_SIZE: usize = 160;
        let mut completed = 0usize;
        for chunk in missing_tasks.chunks(CHUNK_SIZE) {
            let chunk_start = std::time::Instant::now();
            let chunk_bytes: u64 = chunk.iter().map(|t| t.size).sum();
            let refs: Vec<&DownloadTask> = chunk.iter().collect();
            download_batch_with_curl(&refs, &temp_dir, streams)?;
            completed += chunk.len();
            let secs = chunk_start.elapsed().as_secs_f64().max(0.05);
            let mbps = (chunk_bytes as f64 / (1024.0 * 1024.0)) / secs;
            log(
                &format!(
                    "[Downloader · {mbps:.1} MB/s] Vanilla {ver} progress: {completed} / {total} verified."
                ),
                false,
            );
        }
        let _ = fs::remove_dir_all(&temp_dir);
    } else {
        log(
            &format!(
                "Vanilla Minecraft {ver} client, libraries, and assets are verified and ready."
            ),
            false,
        );
    }

    // Extract legacy native classifier DLLs (used in 1.16.5 - 1.18.2) into natives/<ver> if present.
    let tar_exe = resolve_tar_exe();
    for lib in &manifest.libraries {
        if !is_library_allowed_for_target(&lib.name, &lib.rules, os_name, arch) {
            continue;
        }
        if let Some(nat) = native_classifier_for_library(lib, os_name)
            && let Some(ref rel) = nat.path
        {
            let jar_path = vanilla_dir
                .join("libraries")
                .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            if jar_path.is_file() {
                let mut cmd = Command::new(&tar_exe);
                cmd.args(["-xf"])
                    .arg(&jar_path)
                    .arg("-C")
                    .arg(&natives_dir)
                    .stdin(Stdio::null());
                hide_window(&mut cmd);
                let out = cmd.output().map_err(|e| {
                    format!(
                        "Failed to invoke {} to extract native DLLs from {}: {e}",
                        tar_exe.display(),
                        jar_path.display()
                    )
                })?;
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    return Err(format!(
                        "Failed to extract native DLLs from {}: {stderr}",
                        jar_path.display()
                    ));
                }
            }
        }
    }

    let _ = fs::write(&stamp, format!("{ver}\n").as_bytes());

    Ok(manifest)
}

/// Computes `DefaultSkinHelper` slot index `0..18` matching Java `Math.floorMod(uuid.hashCode(), 18)`.
pub fn default_skin_slot(uuid_bytes: &[u8; 16]) -> usize {
    let msb = u64::from_be_bytes(uuid_bytes[0..8].try_into().unwrap());
    let lsb = u64::from_be_bytes(uuid_bytes[8..16].try_into().unwrap());
    let hilo = msb ^ lsb;
    let hash = (((hilo >> 32) as u32) ^ (hilo as u32)) as i32;
    hash.rem_euclid(18) as usize
}

/// Aligns a player UUID so Vanilla `DefaultSkinHelper.getSkinTextures(UUID)` selects
/// Slim (`slim/alex.png`, index 0) or Classic (`wide/steve.png`, index 15) deterministically
/// in singleplayer while staying RFC 4122 version/variant valid.
pub fn align_uuid_for_skin_model(base_uuid: [u8; 16], is_slim: bool) -> [u8; 16] {
    let target_slot = if is_slim { 0 } else { 15 };
    let base_lo = u32::from_be_bytes(base_uuid[12..16].try_into().unwrap());
    for delta in 0u32..128 {
        let mut candidate = base_uuid;
        candidate[12..16].copy_from_slice(&base_lo.wrapping_add(delta).to_be_bytes());
        if default_skin_slot(&candidate) == target_slot {
            return candidate;
        }
    }
    base_uuid
}

pub fn format_uuid_dashed(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Builds a valid uncompressed (STORED method 0) PKZIP archive in pure Rust.
pub fn build_stored_zip(entries: &[(&str, &[u8])]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut central = Vec::new();

    for &(name, data) in entries {
        let name_bytes = name.as_bytes();
        let name_len = u16::try_from(name_bytes.len())
            .map_err(|_| format!("ZIP entry path too long: {name}"))?;
        let data_len = u32::try_from(data.len())
            .map_err(|_| format!("ZIP entry exceeds 4 GiB: {name}"))?;
        let offset = u32::try_from(out.len())
            .map_err(|_| "ZIP archive offset exceeds 4 GiB".to_string())?;
        let crc = crc32_ieee(data);

        // Local file header (30 bytes + name)
        out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed: 2.0
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 filename flag
        out.extend_from_slice(&0u16.to_le_bytes()); // method 0: STORED
        out.extend_from_slice(&0u16.to_le_bytes()); // mod time
        out.extend_from_slice(&0x5421u16.to_le_bytes()); // mod date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        // Central directory header (46 bytes + name)
        central.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
        central.extend_from_slice(&0u16.to_le_bytes()); // STORED
        central.extend_from_slice(&0u16.to_le_bytes()); // mod time
        central.extend_from_slice(&0x5421u16.to_le_bytes()); // mod date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&data_len.to_le_bytes());
        central.extend_from_slice(&data_len.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra len
        central.extend_from_slice(&0u16.to_le_bytes()); // comment len
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }

    let central_offset = u32::try_from(out.len())
        .map_err(|_| "ZIP central directory offset exceeds 4 GiB".to_string())?;
    let central_size = u32::try_from(central.len())
        .map_err(|_| "ZIP central directory exceeds 4 GiB".to_string())?;
    let count = u16::try_from(entries.len())
        .map_err(|_| "Too many ZIP entries".to_string())?;

    out.extend_from_slice(&central);

    // End of central directory record (22 bytes)
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());

    Ok(out)
}

fn pack_format_for_mc_version(mc_version: Option<&str>) -> (u32, bool) {
    match mc_version.unwrap_or("1.21.4") {
        v if v.starts_with("1.16") => (6, false),
        v if v.starts_with("1.17") => (7, false),
        v if v.starts_with("1.18") => (8, false),
        "1.19" | "1.19.1" | "1.19.2" => (9, false),
        "1.19.3" => (12, false),
        "1.19.4" => (13, false),
        "1.20" | "1.20.1" => (15, false),
        "1.20.2" => (18, true),
        "1.20.3" | "1.20.4" => (22, true),
        "1.20.5" | "1.20.6" => (32, true),
        "1.21" | "1.21.1" => (34, true),
        "1.21.2" | "1.21.3" => (42, true),
        _ => (46, true),
    }
}

/// Writes `run/resourcepacks/basalt_skin.zip` for Minecraft (`1.16.5` through `1.21.4`),
/// overriding both the 1.19.3+ Slim/Classic aligned singleplayer slot, the offline multiplayer
/// fallback slot, and the legacy `entity/alex.png` + `entity/steve.png` slots, returning the aligned player UUID string.
pub fn write_basalt_skin_resource_pack_for(
    run_dir: &Path,
    username: &str,
    skin_png_bytes: &[u8],
    is_slim: bool,
    mc_version: Option<&str>,
) -> Result<String, String> {
    let base_uuid = local_player_uuid_bytes(username);
    let aligned_uuid = align_uuid_for_skin_model(base_uuid, is_slim);
    let aligned_slot = default_skin_slot(&aligned_uuid);
    let offline_slot = default_skin_slot(&base_uuid);

    let (fmt, has_supported_formats) = pack_format_for_mc_version(mc_version);
    let mcmeta_string = if has_supported_formats {
        format!(
            r#"{{"pack":{{"pack_format":{fmt},"supported_formats":{{"min_inclusive":6,"max_inclusive":64}},"description":"Basalt active skin"}}}}"#
        )
    } else {
        format!(
            r#"{{"pack":{{"pack_format":{fmt},"description":"Basalt active skin"}}}}"#
        )
    };
    let primary_path = DEFAULT_SKIN_PATHS[aligned_slot];
    let offline_path = DEFAULT_SKIN_PATHS[offline_slot];

    let mut entries: Vec<(&str, &[u8])> = vec![
        ("pack.mcmeta", mcmeta_string.as_bytes()),
        (primary_path, skin_png_bytes),
    ];
    if offline_path != primary_path {
        entries.push((offline_path, skin_png_bytes));
    }
    entries.push(("assets/minecraft/textures/entity/alex.png", skin_png_bytes));
    entries.push(("assets/minecraft/textures/entity/steve.png", skin_png_bytes));

    let zip_bytes = build_stored_zip(&entries)?;
    let rp_dir = run_dir.join("resourcepacks");
    fs::create_dir_all(&rp_dir)
        .map_err(|e| format!("Failed to create resourcepacks directory: {e}"))?;
    let zip_path = rp_dir.join(BASALT_SKIN_PACK_NAME);
    fs::write(&zip_path, &zip_bytes)
        .map_err(|e| format!("Failed to write {}: {e}", zip_path.display()))?;

    Ok(format_uuid_dashed(&aligned_uuid))
}

pub fn write_basalt_skin_resource_pack(
    run_dir: &Path,
    username: &str,
    skin_png_bytes: &[u8],
    is_slim: bool,
) -> Result<String, String> {
    write_basalt_skin_resource_pack_for(run_dir, username, skin_png_bytes, is_slim, None)
}

/// Returns true if the given Minecraft version supports `--quickPlaySingleplayer` / `--quickPlayMultiplayer` (`1.20+`).
pub fn version_supports_quick_play(version: &str) -> bool {
    let mut parts = version.trim().split('.');
    let major = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(1);
    let minor = parts.next().and_then(|s| s.parse::<u32>().ok()).unwrap_or(21);
    major > 1 || (major == 1 && minor >= 20)
}

#[derive(Debug, Clone)]
pub struct VanillaLaunchConfig {
    pub username: String,
    pub player_uuid: String,
    pub graphics: GraphicsSettings,
    pub quick_play_singleplayer: Option<String>,
    pub quick_play_multiplayer: Option<String>,
    pub java_home: PathBuf,
    pub network_boost: bool,
    pub process_boost: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientOptimizationReport {
    pub cdn_ping_ms: Option<u32>,
    pub local_ping_ms: Option<u32>,
    pub freed_bytes: u64,
    pub dns_flushed: bool,
    pub applied_flags_count: usize,
    pub active_flags: Vec<String>,
}

fn measure_tcp_ping_ms(host_port: &str, timeout_ms: u64) -> Option<u32> {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::{Duration, Instant};

    let addr = host_port.to_socket_addrs().ok()?.next()?;
    let start = Instant::now();
    if TcpStream::connect_timeout(&addr, Duration::from_millis(timeout_ms)).is_ok() {
        Some(start.elapsed().as_millis().max(1) as u32)
    } else {
        None
    }
}

fn clean_temp_dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut freed = 0u64;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() {
            let sz = p.metadata().map(|m| m.len()).unwrap_or(0);
            if fs::remove_file(&p).is_ok() {
                freed += sz;
            }
        }
    }
    freed
}

/// Runs a live network latency measurement, flushes Windows DNS cache, cleans temporary `.part` caches,
/// and returns the active low-ping + JVM optimization flags.
pub fn run_client_and_network_optimization(
    vanilla_dir: &Path,
    network_boost: bool,
    process_boost: bool,
) -> ClientOptimizationReport {
    let mut freed_bytes = 0u64;
    freed_bytes += clean_temp_dir_bytes(&vanilla_dir.join("tmp"));
    let _ = fs::remove_dir_all(vanilla_dir.join("tmp"));

    let mut dns_flushed = false;
    if cfg!(windows) {
        let mut cmd = Command::new("ipconfig.exe");
        cmd.arg("/flushdns")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_window(&mut cmd);
        if let Ok(st) = cmd.status() {
            dns_flushed = st.success();
        }
    }

    let cdn_ping_ms = measure_tcp_ping_ms("piston-meta.mojang.com:443", 1200);
    let local_ping_ms = measure_tcp_ping_ms("127.0.0.1:25565", 250);

    let mut active_flags = Vec::new();
    if network_boost {
        active_flags.extend([
            "-Djava.net.preferIPv4Stack=true".to_string(),
            "-Dio.netty.allocator.type=pooled".to_string(),
            "-Dio.netty.recycler.maxCapacityPerThread=4096".to_string(),
            "-Dsun.net.inetaddr.ttl=30".to_string(),
        ]);
    }
    if process_boost {
        active_flags.extend([
            "-XX:+AlwaysPreTouch".to_string(),
            "-XX:+DisableExplicitGC".to_string(),
            "-XX:+ParallelRefProcEnabled".to_string(),
            "-XX:MaxInlineLevel=15".to_string(),
            "Win32:ABOVE_NORMAL_PRIORITY".to_string(),
        ]);
    }

    ClientOptimizationReport {
        cdn_ping_ms,
        local_ping_ms,
        freed_bytes,
        dns_flushed,
        applied_flags_count: active_flags.len(),
        active_flags,
    }
}

/// Builds direct JVM and Minecraft `net.minecraft.client.main.Main` command-line arguments
/// from the parsed Mojang version manifest and Basalt's hardware settings.
/// Builds shared JVM heap, GC, performance, logging, natives, and launcher identity arguments.
pub fn build_jvm_base_args(
    vanilla_dir: &Path,
    manifest: &MojangVersionManifest,
    cfg: &VanillaLaunchConfig,
) -> Vec<String> {
    let mut args = Vec::new();
    let max_mb = cfg.graphics.jvm_ram_mb.clamp(1024, 16384);
    let init_mb = (max_mb / 2).max(512);
    args.push(format!("-Xmx{max_mb}M"));
    args.push(format!("-Xms{init_mb}M"));

    let java_major = parse_java_major(&cfg.java_home)
        .map(|(m, _)| m)
        .unwrap_or(21);
    let effective_gc = match cfg.graphics.jvm_gc {
        JvmGcMode::ZGC if java_major >= 17 => JvmGcMode::ZGC,
        _ => JvmGcMode::G1GC,
    };

    match effective_gc {
        JvmGcMode::G1GC => {
            args.extend([
                "-XX:+UseG1GC".to_string(),
                "-XX:+UnlockExperimentalVMOptions".to_string(),
                "-XX:G1NewSizePercent=20".to_string(),
                "-XX:G1ReservePercent=20".to_string(),
                "-XX:MaxGCPauseMillis=50".to_string(),
                "-XX:G1HeapRegionSize=32M".to_string(),
            ]);
        }
        JvmGcMode::ZGC => {
            args.push("-XX:+UseZGC".to_string());
            // -XX:+ZGenerational is required on JDK 21-22 for generational ZGC, deprecated on 23, removed on 24+.
            if (21..=22).contains(&java_major) {
                args.push("-XX:+ZGenerational".to_string());
            }
        }
    }

    if cfg!(windows) {
        args.push(
            "-XX:HeapDumpPath=MojangTricksIntelDriversForPerformance_javaw.exe_minecraft.exe.heapdump"
                .to_string(),
        );
    }

    if cfg.process_boost {
        args.extend([
            "-XX:+AlwaysPreTouch".to_string(),
            "-XX:+DisableExplicitGC".to_string(),
            "-XX:+ParallelRefProcEnabled".to_string(),
            "-XX:MaxInlineLevel=15".to_string(),
        ]);
    }

    if cfg.network_boost {
        args.extend([
            "-Djava.net.preferIPv4Stack=true".to_string(),
            "-Dio.netty.allocator.type=pooled".to_string(),
            "-Dio.netty.recycler.maxCapacityPerThread=4096".to_string(),
            "-Dsun.net.inetaddr.ttl=30".to_string(),
        ]);
    }

    if let Some(ref logging) = manifest.logging
        && let Some(ref client_log) = logging.client
    {
        let log_xml = vanilla_dir.join("logging").join(&client_log.file.id);
        if log_xml.is_file() {
            args.push(format!("-Dlog4j.configurationFile={}", log_xml.display()));
        }
    }

    let natives_dir = vanilla_dir.join("natives").join(&manifest.id);
    let natives_str = natives_dir.to_string_lossy().into_owned();
    args.push(format!("-Djava.library.path={natives_str}"));
    args.push(format!("-Djna.tmpdir={natives_str}"));
    args.push(format!(
        "-Dorg.lwjgl.system.SharedLibraryExtractPath={natives_str}"
    ));
    args.push(format!("-Dio.netty.native.workdir={natives_str}"));
    args.push("-Dminecraft.launcher.brand=Basalt".to_string());
    args.push(format!(
        "-Dminecraft.launcher.version={}",
        env!("CARGO_PKG_VERSION")
    ));
    args
}

/// Appends standard Minecraft client arguments (identity, directories, quick-play/server).
pub fn append_minecraft_game_args(
    args: &mut Vec<String>,
    vanilla_dir: &Path,
    run_dir: &Path,
    manifest: &MojangVersionManifest,
    cfg: &VanillaLaunchConfig,
    version_label: &str,
) {
    let uuid = if cfg.player_uuid.trim().is_empty() {
        local_player_uuid_dashed(&cfg.username)
    } else {
        cfg.player_uuid.clone()
    };

    args.extend([
        "--username".to_string(),
        cfg.username.clone(),
        "--version".to_string(),
        version_label.to_string(),
        "--gameDir".to_string(),
        run_dir.to_string_lossy().into_owned(),
        "--assetsDir".to_string(),
        vanilla_dir.join("assets").to_string_lossy().into_owned(),
        "--assetIndex".to_string(),
        manifest.asset_index.id.clone(),
        "--uuid".to_string(),
        uuid,
        "--accessToken".to_string(),
        "0".to_string(),
        "--clientId".to_string(),
        "0".to_string(),
        "--xuid".to_string(),
        "0".to_string(),
        "--userType".to_string(),
        "legacy".to_string(),
        "--versionType".to_string(),
        "release".to_string(),
    ]);

    if version_supports_quick_play(&manifest.id) {
        if let Some(ref sp) = cfg.quick_play_singleplayer {
            args.push("--quickPlaySingleplayer".to_string());
            args.push(sp.clone());
        } else if let Some(ref mp) = cfg.quick_play_multiplayer {
            args.push("--quickPlayMultiplayer".to_string());
            args.push(mp.clone());
        }
    } else if let Some(ref mp) = cfg.quick_play_multiplayer {
        let (host, port) = parse_host_port(mp);
        if !host.is_empty() {
            args.push("--server".to_string());
            args.push(host);
            args.push("--port".to_string());
            args.push(port.to_string());
        }
    }
}

pub fn build_vanilla_launch_args(
    vanilla_dir: &Path,
    run_dir: &Path,
    manifest: &MojangVersionManifest,
    cfg: &VanillaLaunchConfig,
) -> Vec<String> {
    let mut args = build_jvm_base_args(vanilla_dir, manifest, cfg);

    let os_name = current_mojang_os_name();
    let arch = current_mojang_arch();
    let mut cp_entries = Vec::new();
    for lib in &manifest.libraries {
        if !is_library_allowed_for_target(&lib.name, &lib.rules, os_name, arch) {
            continue;
        }
        if let Some(ref art) = lib.downloads.artifact
            && let Some(ref rel) = art.path
        {
            cp_entries.push(
                vanilla_dir
                    .join("libraries")
                    .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if let Some(nat) = native_classifier_for_library(lib, os_name)
            && let Some(ref rel) = nat.path
        {
            cp_entries.push(
                vanilla_dir
                    .join("libraries")
                    .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    cp_entries.push(
        client_jar_path_for(vanilla_dir, &manifest.id)
            .to_string_lossy()
            .into_owned(),
    );

    let cp_sep = if cfg!(windows) { ";" } else { ":" };
    args.push("-cp".to_string());
    args.push(cp_entries.join(cp_sep));

    args.push(manifest.main_class.clone());
    append_minecraft_game_args(&mut args, vanilla_dir, run_dir, manifest, cfg, &manifest.id);

    args
}

fn escape_java_argfile_token(token: &str) -> String {
    let needs_quote = token.is_empty()
        || token
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '\\' | '#' | '\''));
    if !needs_quote {
        return token.to_string();
    }
    let mut out = String::with_capacity(token.len() + 4);
    out.push('"');
    for ch in token.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Spawns any prepared Minecraft Java command line directly via `java.exe` (shared by Vanilla, Fabric, and Forge).
pub fn spawn_java_client(
    run_dir: &Path,
    java_home: &Path,
    args: &[String],
    process_boost: bool,
    label: &str,
) -> Result<Child, String> {
    fs::create_dir_all(run_dir)
        .map_err(|e| format!("Failed to create Minecraft game directory: {e}"))?;
    let java_bin = java_home.join("bin").join(java_executable_name());
    if !java_bin.is_file() {
        return Err(format!(
            "Java executable was not found at {}",
            java_bin.display()
        ));
    }

    let approx_cmd_chars: usize = args.iter().map(|s| s.len() + 3).sum();
    let java_major = parse_java_major(java_home).map(|(m, _)| m).unwrap_or(21);
    let mut argfile_guard: Option<PathBuf> = None;

    let mut cmd = Command::new(&java_bin);
    if cfg!(windows) && approx_cmd_chars > 24_000 && java_major >= 9 {
        let argfile_path = run_dir.join(format!("basalt-launch-{}.args", std::process::id()));
        let body: String = args
            .iter()
            .map(|a| escape_java_argfile_token(a))
            .collect::<Vec<_>>()
            .join("\n");
        if fs::write(&argfile_path, body.as_bytes()).is_ok() {
            cmd.arg(format!("@{}", argfile_path.display()));
            argfile_guard = Some(argfile_path);
        } else {
            cmd.args(args);
        }
    } else {
        cmd.args(args);
    }

    cmd.current_dir(run_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for key in [
        "_JAVA_OPTIONS",
        "JAVA_TOOL_OPTIONS",
        "JAVA_OPTS",
        "GRADLE_OPTS",
    ] {
        cmd.env_remove(key);
    }

    hide_window_with_priority(&mut cmd, process_boost);

    let res = cmd
        .spawn()
        .map_err(|e| format!("Failed to start Minecraft {label} process: {e}"));
    if res.is_err()
        && let Some(p) = argfile_guard
    {
        let _ = fs::remove_file(p);
    }
    res
}

fn remove_argfiles_in_dir(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("basalt-launch-") && name.ends_with(".args") {
            let p = entry.path();
            if p.is_file() && fs::remove_file(&p).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

/// Removes stale `basalt-launch-*.args` files from `root_dir`, `root_dir/run`, and Forge profile directories.
pub fn cleanup_stale_launch_argfiles(root_dir: &Path) -> usize {
    let mut removed = 0usize;
    removed += remove_argfiles_in_dir(root_dir);
    removed += remove_argfiles_in_dir(&root_dir.join("run"));

    let forge_profiles = resolve_vanilla_dir(root_dir).join("profiles").join("forge");
    if let Ok(entries) = fs::read_dir(&forge_profiles) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                removed += remove_argfiles_in_dir(&p);
            }
        }
    }
    removed
}

/// Spawns Vanilla Minecraft directly with `java.exe` (no Gradle/Fabric intermediary).
pub fn spawn_vanilla_client(
    vanilla_dir: &Path,
    run_dir: &Path,
    manifest: &MojangVersionManifest,
    cfg: &VanillaLaunchConfig,
) -> Result<Child, String> {
    let args = build_vanilla_launch_args(vanilla_dir, run_dir, manifest, cfg);
    spawn_java_client(
        run_dir,
        &cfg.java_home,
        &args,
        cfg.process_boost,
        &manifest.id,
    )
}

/// Pure-Rust FIPS 180-4 SHA-1 digest helper for verifying official Mojang downloads.
pub fn compute_sha1_hex<R: Read>(mut reader: R) -> std::io::Result<String> {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];

    let compress = |block: &[u8; 64], state: &mut [u32; 5]| {
        let mut w = [0u32; 80];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) =
            (state[0], state[1], state[2], state[3], state[4]);

        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDCu32),
                _ => (b ^ c ^ d, 0xCA62_C1D6u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
    };

    let mut total_bytes: u64 = 0;
    let mut pending = [0u8; 64];
    let mut pending_len = 0usize;
    let mut buf = [0u8; 65536];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total_bytes = total_bytes
            .checked_add(n as u64)
            .ok_or_else(|| std::io::Error::other("input too large for SHA-1"))?;
        let mut slice = &buf[..n];
        if pending_len > 0 {
            let need = 64 - pending_len;
            let take = need.min(slice.len());
            pending[pending_len..pending_len + take].copy_from_slice(&slice[..take]);
            pending_len += take;
            slice = &slice[take..];
            if pending_len == 64 {
                compress(&pending, &mut h);
                pending_len = 0;
            }
        }
        while slice.len() >= 64 {
            let block: &[u8; 64] = slice[..64].try_into().expect("64-byte slice");
            compress(block, &mut h);
            slice = &slice[64..];
        }
        if !slice.is_empty() {
            pending[..slice.len()].copy_from_slice(slice);
            pending_len = slice.len();
        }
    }

    pending[pending_len] = 0x80;
    pending_len += 1;
    if pending_len > 56 {
        pending[pending_len..].fill(0);
        compress(&pending, &mut h);
        pending_len = 0;
    }
    pending[pending_len..56].fill(0);
    pending[56..64].copy_from_slice(&(total_bytes * 8).to_be_bytes());
    compress(&pending, &mut h);

    let mut hex = String::with_capacity(40);
    for word in h {
        hex.push_str(&format!("{word:08x}"));
    }
    Ok(hex)
}

pub fn compute_file_sha1_hex(path: &Path) -> Result<String, String> {
    let file =
        File::open(path).map_err(|e| format!("Failed to open file for SHA-1: {e}"))?;
    compute_sha1_hex(std::io::BufReader::new(file))
        .map_err(|e| format!("Failed to compute SHA-1: {e}"))
}

/// Pure-Rust FIPS 180-4 SHA-256 digest helper for verifying Adoptium JDK downloads.
pub fn compute_sha256_hex<R: Read>(mut reader: R) -> std::io::Result<String> {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let compress = |block: &[u8; 64], s: &mut [u32; 8]| {
        let mut w = [0u32; 64];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *s;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        s[0] = s[0].wrapping_add(a);
        s[1] = s[1].wrapping_add(b);
        s[2] = s[2].wrapping_add(c);
        s[3] = s[3].wrapping_add(d);
        s[4] = s[4].wrapping_add(e);
        s[5] = s[5].wrapping_add(f);
        s[6] = s[6].wrapping_add(g);
        s[7] = s[7].wrapping_add(h);
    };

    let mut total_bytes: u64 = 0;
    let mut pending = [0u8; 64];
    let mut pending_len = 0usize;
    let mut buf = [0u8; 65536];

    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total_bytes = total_bytes
            .checked_add(n as u64)
            .ok_or_else(|| std::io::Error::other("input too large for SHA-256"))?;
        let mut slice = &buf[..n];
        if pending_len > 0 {
            let need = 64 - pending_len;
            let take = need.min(slice.len());
            pending[pending_len..pending_len + take].copy_from_slice(&slice[..take]);
            pending_len += take;
            slice = &slice[take..];
            if pending_len == 64 {
                compress(&pending, &mut state);
                pending_len = 0;
            }
        }
        while slice.len() >= 64 {
            let block: &[u8; 64] = slice[..64].try_into().expect("64-byte slice");
            compress(block, &mut state);
            slice = &slice[64..];
        }
        if !slice.is_empty() {
            pending[..slice.len()].copy_from_slice(slice);
            pending_len = slice.len();
        }
    }

    pending[pending_len] = 0x80;
    pending_len += 1;
    if pending_len > 56 {
        pending[pending_len..].fill(0);
        compress(&pending, &mut state);
        pending_len = 0;
    }
    pending[pending_len..56].fill(0);
    pending[56..64].copy_from_slice(&(total_bytes * 8).to_be_bytes());
    compress(&pending, &mut state);

    let mut hex = String::with_capacity(64);
    for word in state {
        hex.push_str(&format!("{word:08x}"));
    }
    Ok(hex)
}

pub fn compute_file_sha256_hex(path: &Path) -> Result<String, String> {
    let file =
        File::open(path).map_err(|e| format!("Failed to open file for SHA-256: {e}"))?;
    compute_sha256_hex(std::io::BufReader::new(file))
        .map_err(|e| format!("Failed to compute SHA-256: {e}"))
}

fn is_high_signal_error_line(line: &str) -> bool {
    let s = line.trim();
    s.starts_with("Caused by:")
        || s.starts_with("Exception in thread")
        || s.starts_with("Error:")
        || s.contains("OutOfMemoryError")
        || s.contains("UnsupportedClassVersionError")
        || s.contains("NoClassDefFoundError")
        || s.contains("UnsatisfiedLinkError")
        || s.contains("GLFW error")
        || s.contains("WGL:")
        || s.contains("Pixel format not accelerated")
        || s.contains("[main/ERROR]")
        || s.contains("[main/FATAL]")
        || s.contains("[Render thread/ERROR]")
        || s.contains("[Render thread/FATAL]")
        || s.contains("Could not reserve enough space")
}

fn push_unique_line(picks: &mut Vec<String>, raw: &str) {
    let clean = raw.trim().to_string();
    if !clean.is_empty() && !picks.contains(&clean) {
        picks.push(clean);
    }
}

/// Extracts high-signal JVM / Minecraft error lines from captured process output.
pub fn summarize_process_failure(
    lines: &[String],
    exit_code: Option<i32>,
    log_path: Option<&Path>,
) -> String {
    let mut picks: Vec<String> = Vec::new();

    for line in lines.iter().rev() {
        if is_high_signal_error_line(line) {
            push_unique_line(&mut picks, line);
            if picks.len() >= 3 {
                break;
            }
        }
    }
    picks.reverse();

    if picks.is_empty() {
        for line in lines.iter().rev() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                push_unique_line(&mut picks, trimmed);
                if picks.len() >= 5 {
                    break;
                }
            }
        }
        picks.reverse();
    }

    let mut summary = if picks.is_empty() {
        format!(
            "Minecraft process exited with error status (code: {}).",
            exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "none".into())
        )
    } else {
        picks.join(" | ")
    };

    if let Some(path) = log_path {
        summary.push_str(&format!(" (Full log: {})", path.display()));
    }
    summary
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::io::{Cursor, Read};
    use std::path::{Path, PathBuf};

    use super::{
        CurlCapabilities, DEFAULT_SKIN_PATHS, MINECRAFT_1_21_4_NAME, MojangAssetIndexInfo,
        MojangArtifactInfo, MojangLibDownloads, MojangLibrary, MojangLoggingClient,
        MojangLoggingFile, MojangLoggingSection, MojangRule, MojangRuleOs,
        MojangVersionDownloads, MojangVersionManifest, VanillaLaunchConfig,
        align_uuid_for_skin_model, build_stored_zip, build_vanilla_launch_args,
        check_vanilla_assets_ready_for, compute_sha1_hex, compute_sha256_hex, default_skin_slot,
        evaluate_jdk_candidates, is_ascii_path, is_library_allowed_for_target, is_valid_asset_sha1,
        load_cached_version_manifest_for, pack_format_for_mc_version, parse_java_major,
        summarize_process_failure, version_supports_quick_play, write_basalt_skin_resource_pack,
    };
    use crate::config::{GraphicsSettings, JvmGcMode, normalize_mc_version};
    use crate::protocol::local_player_uuid_bytes;

    #[test]
    fn matches_fips_180_4_sha1_and_sha256_vectors() {
        assert_eq!(
            compute_sha1_hex(&b""[..]).unwrap(),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
        assert_eq!(
            compute_sha1_hex(&b"abc"[..]).unwrap(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            compute_sha256_hex(&b""[..]).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            compute_sha256_hex(&b"abc"[..]).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn aligns_uuid_for_slim_and_wide_default_skins() {
        let base = local_player_uuid_bytes("Markesr");
        let slim_uuid = align_uuid_for_skin_model(base, true);
        let wide_uuid = align_uuid_for_skin_model(base, false);

        assert_eq!(default_skin_slot(&slim_uuid), 0);
        assert_eq!(DEFAULT_SKIN_PATHS[default_skin_slot(&slim_uuid)], "assets/minecraft/textures/entity/player/slim/alex.png");
        assert_eq!(default_skin_slot(&wide_uuid), 15);
        assert_eq!(DEFAULT_SKIN_PATHS[default_skin_slot(&wide_uuid)], "assets/minecraft/textures/entity/player/wide/steve.png");

        // Known Java reference: UUID "ffffffff-ffff-3fff-8000-000000000001".hashCode() == -2147434495, floorMod 18 == 11
        let neg_bytes: [u8; 16] = [
            0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f, 0xff, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ];
        assert_eq!(default_skin_slot(&neg_bytes), 11);
    }

    #[test]
    fn evaluates_mojang_windows_x64_library_rules() {
        let win_rule = vec![MojangRule {
            action: "allow".to_string(),
            os: Some(MojangRuleOs {
                name: Some("windows".to_string()),
                arch: None,
            }),
            features: None,
        }];
        let linux_rule = vec![MojangRule {
            action: "allow".to_string(),
            os: Some(MojangRuleOs {
                name: Some("linux".to_string()),
                arch: None,
            }),
            features: None,
        }];

        assert!(is_library_allowed_for_target(
            "org.lwjgl:lwjgl:3.3.3:natives-windows",
            &win_rule,
            "windows",
            "x64"
        ));
        assert!(!is_library_allowed_for_target(
            "org.lwjgl:lwjgl:3.3.3:natives-windows-x86",
            &win_rule,
            "windows",
            "x64"
        ));
        assert!(!is_library_allowed_for_target(
            "org.lwjgl:lwjgl:3.3.3:natives-windows-arm64",
            &win_rule,
            "windows",
            "x64"
        ));
        assert!(!is_library_allowed_for_target(
            "com.mojang:jtracy:1.0.29:natives-linux",
            &linux_rule,
            "windows",
            "x64"
        ));
        assert!(is_library_allowed_for_target(
            "com.google.guava:guava:33.3.1-jre",
            &[],
            "windows",
            "x64"
        ));
    }

    #[test]
    fn builds_valid_skin_resource_pack_zip_and_launch_args() {
        let tmp = std::env::temp_dir().join(format!("basalt_vanilla_pack_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let run_dir = tmp.join("run");
        let vanilla_dir = tmp.join("vanilla");
        fs::create_dir_all(vanilla_dir.join("logging")).unwrap();
        fs::write(vanilla_dir.join("logging").join("client-1.21.2.xml"), b"<Configuration/>").unwrap();

        let zip_bytes = build_stored_zip(&[("a.txt", b"hello"), ("b/c.bin", b"\x01\x02")]).unwrap();
        assert!(zip_bytes.starts_with(b"PK\x03\x04"));
        let mut dec_check = Cursor::new(&zip_bytes);
        let mut head = [0u8; 4];
        dec_check.read_exact(&mut head).unwrap();
        assert_eq!(&head, b"PK\x03\x04");

        let uuid = write_basalt_skin_resource_pack(&run_dir, "Markesr", b"PNGDATA", true).unwrap();
        assert_eq!(uuid.len(), 36);
        assert!(run_dir.join("resourcepacks").join("basalt_skin.zip").is_file());

        let manifest = MojangVersionManifest {
            id: "1.21.4".to_string(),
            main_class: "net.minecraft.client.main.Main".to_string(),
            asset_index: MojangAssetIndexInfo {
                id: "19".to_string(),
                sha1: "00".to_string(),
                size: 10,
                url: String::new(),
            },
            downloads: MojangVersionDownloads {
                client: MojangArtifactInfo {
                    path: None,
                    sha1: "00".to_string(),
                    size: 10,
                    url: String::new(),
                },
            },
            logging: Some(MojangLoggingSection {
                client: Some(MojangLoggingClient {
                    file: MojangLoggingFile {
                        id: "client-1.21.2.xml".to_string(),
                        sha1: "00".to_string(),
                        size: 10,
                        url: String::new(),
                    },
                }),
            }),
            libraries: vec![MojangLibrary {
                name: "com.google.code.gson:gson:2.11.0".to_string(),
                downloads: MojangLibDownloads {
                    artifact: Some(MojangArtifactInfo {
                        path: Some("com/google/code/gson/gson/2.11.0/gson-2.11.0.jar".to_string()),
                        sha1: "00".to_string(),
                        size: 10,
                        url: String::new(),
                    }),
                    classifiers: HashMap::new(),
                },
                rules: vec![],
            }],
        };

        let gfx = GraphicsSettings {
            jvm_ram_mb: 4096,
            jvm_gc: JvmGcMode::ZGC,
            ..Default::default()
        };

        let args = build_vanilla_launch_args(
            &vanilla_dir,
            &run_dir,
            &manifest,
            &VanillaLaunchConfig {
                username: "Markesr".to_string(),
                player_uuid: uuid.clone(),
                graphics: gfx,
                quick_play_singleplayer: Some("MyWorld".to_string()),
                quick_play_multiplayer: None,
                java_home: tmp.join("jdk-21"),
                network_boost: true,
                process_boost: true,
            },
        );

        assert!(args.contains(&"-Xmx4096M".to_string()));
        assert!(args.contains(&"-XX:+UseZGC".to_string()));
        assert!(args.contains(&"-Djava.net.preferIPv4Stack=true".to_string()));
        assert!(args.contains(&"-XX:+AlwaysPreTouch".to_string()));
        assert!(args.contains(&"net.minecraft.client.main.Main".to_string()));
        assert!(args.contains(&"--quickPlaySingleplayer".to_string()));
        assert!(args.contains(&"MyWorld".to_string()));
        assert!(args.contains(&uuid));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn summarizes_vanilla_jvm_errors_and_picks_jdk21() {
        let lines: Vec<String> = vec![
            "[Render thread/INFO]: Starting client".into(),
            "Caused by: java.lang.UnsupportedClassVersionError: bad version".into(),
            "[main/FATAL]: Uncaught exception".into(),
        ];
        let summary = summarize_process_failure(
            &lines,
            Some(1),
            Some(PathBuf::from("C:\\Logs\\last-launch.log").as_path()),
        );
        assert!(summary.contains("UnsupportedClassVersionError"));
        assert!(summary.contains("last-launch.log"));

        assert!(is_ascii_path(Path::new("C:\\Basalt\\vanilla")));
        assert!(!is_ascii_path(Path::new("C:\\Users\\rab\u{0131}abetl\\vanilla")));

        let tmp = std::env::temp_dir().join("basalt_vanilla_jdk_probe");
        let _ = fs::remove_dir_all(&tmp);
        let j21 = tmp.join("jdk-21");
        let j26 = tmp.join("jdk-26");
        for (dir, ver) in [(&j21, "21.0.5"), (&j26, "26.0.1")] {
            fs::create_dir_all(dir.join("bin")).unwrap();
            let java_bin = if cfg!(windows) { "java.exe" } else { "java" };
            let javac_bin = if cfg!(windows) { "javac.exe" } else { "javac" };
            fs::write(dir.join("bin").join(java_bin), b"").unwrap();
            fs::write(dir.join("bin").join(javac_bin), b"").unwrap();
            fs::write(dir.join("release"), format!("JAVA_VERSION=\"{ver}\"\n")).unwrap();
        }
        let disc = evaluate_jdk_candidates(&[j26.clone(), j21.clone()]);
        assert_eq!(disc.java21.unwrap().1, "21.0.5");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn zgc_flag_adapts_to_jdk_major_and_validates_manifest_and_assets() {
        let tmp = std::env::temp_dir().join(format!("basalt_vanilla_audit_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let j21 = tmp.join("jdk-21");
        let j24 = tmp.join("jdk-24");
        fs::create_dir_all(&j21).unwrap();
        fs::create_dir_all(&j24).unwrap();
        fs::write(j21.join("release"), "JAVA_VERSION=\"21+35\"\n").unwrap();
        fs::write(j24.join("release"), "JAVA_VERSION=\"24.0.1+9\"\n").unwrap();
        assert_eq!(parse_java_major(&j21).map(|(m, _)| m), Some(21));
        assert_eq!(parse_java_major(&j24).map(|(m, _)| m), Some(24));

        let manifest = MojangVersionManifest {
            id: MINECRAFT_1_21_4_NAME.to_string(),
            main_class: "net.minecraft.client.main.Main".to_string(),
            asset_index: MojangAssetIndexInfo {
                id: "19".to_string(),
                sha1: "0000000000000000000000000000000000000000".to_string(),
                size: 80,
                url: "https://example.invalid/19.json".to_string(),
            },
            downloads: MojangVersionDownloads {
                client: MojangArtifactInfo {
                    sha1: "0".repeat(40),
                    size: 100,
                    url: "https://example.invalid/client.jar".to_string(),
                    path: None,
                },
            },
            libraries: vec![MojangLibrary {
                name: "org.example:dummy:1.0".to_string(),
                downloads: MojangLibDownloads {
                    artifact: Some(MojangArtifactInfo {
                        sha1: "0".repeat(40),
                        size: 10,
                        url: "https://example.invalid/dummy.jar".to_string(),
                        path: Some("org/example/dummy/1.0/dummy-1.0.jar".to_string()),
                    }),
                    classifiers: HashMap::new(),
                },
                rules: Vec::new(),
            }],
            logging: None,
        };

        let vanilla_dir = tmp.join("vanilla");
        let run_dir = tmp.join("run");
        fs::create_dir_all(&vanilla_dir).unwrap();
        fs::create_dir_all(&run_dir).unwrap();

        let gfx = GraphicsSettings {
            jvm_ram_mb: 4096,
            jvm_gc: JvmGcMode::ZGC,
            ..Default::default()
        };

        let args_21 = build_vanilla_launch_args(
            &vanilla_dir,
            &run_dir,
            &manifest,
            &VanillaLaunchConfig {
                username: "Player".to_string(),
                player_uuid: "a01e3843-e521-3998-958a-f459800e4d11".to_string(),
                graphics: gfx.clone(),
                quick_play_singleplayer: None,
                quick_play_multiplayer: None,
                java_home: j21,
                network_boost: true,
                process_boost: true,
            },
        );
        assert!(args_21.contains(&"-XX:+UseZGC".to_string()));
        assert!(args_21.contains(&"-XX:+ZGenerational".to_string()));

        let args_24 = build_vanilla_launch_args(
            &vanilla_dir,
            &run_dir,
            &manifest,
            &VanillaLaunchConfig {
                username: "Player".to_string(),
                player_uuid: "a01e3843-e521-3998-958a-f459800e4d11".to_string(),
                graphics: gfx,
                quick_play_singleplayer: None,
                quick_play_multiplayer: None,
                java_home: j24,
                network_boost: false,
                process_boost: false,
            },
        );
        assert!(args_24.contains(&"-XX:+UseZGC".to_string()));
        assert!(!args_24.contains(&"-XX:+ZGenerational".to_string()));

        // Reject corrupt/wrong-version cached manifest JSON
        let ver_dir = vanilla_dir.join("versions").join("1.21.4");
        fs::create_dir_all(&ver_dir).unwrap();
        fs::write(
            ver_dir.join("1.21.4.json"),
            r#"{"id":"1.20.4","mainClass":"x","assetIndex":{"id":"19","sha1":"0","size":10,"url":"u"},"downloads":{"client":{"sha1":"0","size":10,"url":"u"}},"libraries":[]}"#,
        )
        .unwrap();
        assert!(load_cached_version_manifest_for(&vanilla_dir, "1.21.4").is_none());

        // Even if .verified-19.ok stamp exists, missing asset object must make check_vanilla_assets_ready_for return false
        let idx_payload = r#"{"objects":{"minecraft/lang/en_us.json":{"hash":"abcd000000000000000000000000000000000000","size":12}}}"#;
        fs::write(
            ver_dir.join("1.21.4.json"),
            format!(
                r#"{{"id":"1.21.4","mainClass":"net.minecraft.client.main.Main","assetIndex":{{"id":"19","sha1":"0","size":{},"url":"u"}},"downloads":{{"client":{{"sha1":"0","size":10,"url":"u"}}}},"libraries":[{{"name":"org.example:dummy:1.0","downloads":{{"artifact":{{"sha1":"0","size":10,"url":"u","path":"org/example/dummy/1.0/dummy-1.0.jar"}}}}}}]}}"#,
                idx_payload.len()
            ),
        )
        .unwrap();
        assert!(load_cached_version_manifest_for(&vanilla_dir, "1.21.4").is_some());
        fs::create_dir_all(vanilla_dir.join("assets").join("indexes")).unwrap();
        fs::write(vanilla_dir.join("assets").join("indexes").join("19.json"), idx_payload).unwrap();
        fs::write(vanilla_dir.join("assets").join(".verified-19.ok"), b"1.21.4\n").unwrap();
        assert!(!check_vanilla_assets_ready_for(&vanilla_dir, "1.21.4"));

        // Verify multi-version quick play and version normalization
        assert!(version_supports_quick_play("1.21.4"));
        assert!(version_supports_quick_play("1.20.1"));
        assert!(version_supports_quick_play("1.20"));
        assert!(!version_supports_quick_play("1.19.4"));
        assert!(!version_supports_quick_play("1.16.5"));
        assert_eq!(normalize_mc_version("1.20.4"), "1.20.4");
        assert_eq!(normalize_mc_version("1.16.5"), "1.16.5");
        assert_eq!(normalize_mc_version("invalid"), "1.21.4");

        let win_schannel = "curl 8.22.0 (Windows) libcurl/8.22.0 Schannel zlib/1.3.2\nFeatures: alt-svc AsynchDNS HSTS IPv6 Largefile SSL SSPI";
        assert_eq!(
            CurlCapabilities::parse_version_output(win_schannel),
            CurlCapabilities {
                supports_http2: false,
                supports_parallel: true,
                supports_ssl_revoke_best_effort: true,
            }
        );
        let legacy_win10 = "curl 7.55.1 (Windows) libcurl/7.55.1 WinSSL\nFeatures: AsynchDNS IPv6 Largefile SSPI";
        assert_eq!(
            CurlCapabilities::parse_version_output(legacy_win10),
            CurlCapabilities {
                supports_http2: false,
                supports_parallel: false,
                supports_ssl_revoke_best_effort: false,
            }
        );
        let with_nghttp2 = "curl 8.5.0 (x86_64) libcurl/8.5.0 nghttp2/1.59.0\nFeatures: AsynchDNS HTTP2 HTTPS-proxy";
        assert_eq!(
            CurlCapabilities::parse_version_output(with_nghttp2),
            CurlCapabilities {
                supports_http2: true,
                supports_parallel: true,
                supports_ssl_revoke_best_effort: false,
            }
        );

        let pre_770_schannel = "curl 7.68.0 (Windows) libcurl/7.68.0 Schannel";
        assert!(!CurlCapabilities::parse_version_output(pre_770_schannel).supports_ssl_revoke_best_effort);

        // Java 8 parsing & asset hash guard
        let j8 = tmp.join("jdk-1.8.0_412");
        fs::create_dir_all(&j8).unwrap();
        fs::write(j8.join("release"), "JAVA_VERSION=\"1.8.0_412\"\n").unwrap();
        assert_eq!(parse_java_major(&j8).map(|(m, _)| m), Some(8));
        assert!(!is_valid_asset_sha1("abcd"));
        assert!(!is_valid_asset_sha1("\u{0131}bcdef0123456789012345678901234567890123"));
        assert!(is_valid_asset_sha1(&"a".repeat(40)));
        assert_eq!(pack_format_for_mc_version(Some("1.16.5")), (6, false));
        assert_eq!(pack_format_for_mc_version(Some("1.21.4")), (46, true));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn zgc_falls_back_to_g1gc_on_legacy_jdks_and_cleans_argfiles_and_checks_natives() {
        let tmp = std::env::temp_dir().join(format!(
            "basalt_vanilla_phase2_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        let j8 = tmp.join("jdk-8");
        let j11 = tmp.join("jdk-11");
        let j17 = tmp.join("jdk-17");
        fs::create_dir_all(&j8).unwrap();
        fs::create_dir_all(&j11).unwrap();
        fs::create_dir_all(&j17).unwrap();
        fs::write(j8.join("release"), "JAVA_VERSION=\"1.8.0_412\"\n").unwrap();
        fs::write(j11.join("release"), "JAVA_VERSION=\"11.0.22\"\n").unwrap();
        fs::write(j17.join("release"), "JAVA_VERSION=\"17.0.10\"\n").unwrap();

        let vanilla_dir = tmp.join("vanilla");
        let run_dir = tmp.join("run");
        fs::create_dir_all(&vanilla_dir).unwrap();
        fs::create_dir_all(&run_dir).unwrap();

        let manifest = MojangVersionManifest {
            id: "1.16.5".to_string(),
            main_class: "net.minecraft.client.main.Main".to_string(),
            asset_index: MojangAssetIndexInfo {
                id: "1.16".to_string(),
                sha1: "0".repeat(40),
                size: 10,
                url: String::new(),
            },
            downloads: MojangVersionDownloads {
                client: MojangArtifactInfo {
                    path: None,
                    sha1: "0".repeat(40),
                    size: 5,
                    url: String::new(),
                },
            },
            logging: None,
            libraries: vec![],
        };
        let gfx = GraphicsSettings {
            jvm_ram_mb: 3072,
            jvm_gc: JvmGcMode::ZGC,
            ..Default::default()
        };

        for jdk in [&j8, &j11] {
            let args = build_vanilla_launch_args(
                &vanilla_dir,
                &run_dir,
                &manifest,
                &VanillaLaunchConfig {
                    username: "Player".to_string(),
                    player_uuid: "a01e3843-e521-3998-958a-f459800e4d11".to_string(),
                    graphics: gfx.clone(),
                    quick_play_singleplayer: None,
                    quick_play_multiplayer: None,
                    java_home: jdk.clone(),
                    network_boost: false,
                    process_boost: false,
                },
            );
            assert!(args.contains(&"-XX:+UseG1GC".to_string()));
            assert!(!args.contains(&"-XX:+UseZGC".to_string()));
        }

        let args_17 = build_vanilla_launch_args(
            &vanilla_dir,
            &run_dir,
            &manifest,
            &VanillaLaunchConfig {
                username: "Player".to_string(),
                player_uuid: "a01e3843-e521-3998-958a-f459800e4d11".to_string(),
                graphics: gfx,
                quick_play_singleplayer: None,
                quick_play_multiplayer: None,
                java_home: j17,
                network_boost: false,
                process_boost: false,
            },
        );
        assert!(args_17.contains(&"-XX:+UseZGC".to_string()));
        assert!(!args_17.contains(&"-XX:+ZGenerational".to_string()));

        // check_vanilla_client_ready_for must require extracted native files when native classifiers exist
        let ver_dir = vanilla_dir.join("versions").join("1.16.5");
        fs::create_dir_all(&ver_dir).unwrap();
        fs::write(ver_dir.join("1.16.5.jar"), b"12345").unwrap();
        let nat_rel = "org/lwjgl/lwjgl/3.2.2/lwjgl-3.2.2-natives-windows.jar";
        let nat_jar = vanilla_dir.join("libraries").join(nat_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        fs::create_dir_all(nat_jar.parent().unwrap()).unwrap();
        fs::write(&nat_jar, b"nativejar").unwrap();
        let os_key = match super::current_mojang_os_name() {
            "windows" => "natives-windows",
            "osx" => "natives-macos",
            _ => "natives-linux",
        };
        let manifest_json = format!(
            r#"{{"id":"1.16.5","mainClass":"net.minecraft.client.main.Main","assetIndex":{{"id":"1.16","sha1":"0","size":10,"url":"u"}},"downloads":{{"client":{{"sha1":"0","size":5,"url":"u"}}}},"libraries":[{{"name":"org.lwjgl:lwjgl:3.2.2","downloads":{{"classifiers":{{"{os_key}":{{"sha1":"0","size":9,"url":"u","path":"{nat_rel}"}}}}}}}}]}}"#
        );
        fs::write(ver_dir.join("1.16.5.json"), manifest_json).unwrap();
        assert!(!super::check_vanilla_client_ready_for(&vanilla_dir, "1.16.5"));

        let natives_dir = vanilla_dir.join("natives").join("1.16.5");
        fs::create_dir_all(&natives_dir).unwrap();
        fs::write(natives_dir.join("lwjgl.dll"), b"dll").unwrap();
        assert!(super::check_vanilla_client_ready_for(&vanilla_dir, "1.16.5"));

        // cleanup_stale_launch_argfiles removes only basalt-launch-*.args
        fs::write(run_dir.join("basalt-launch-1234.args"), b"-Xmx4096M").unwrap();
        fs::write(run_dir.join("keep.txt"), b"keep").unwrap();
        let removed = super::cleanup_stale_launch_argfiles(&tmp);
        assert_eq!(removed, 1);
        assert!(!run_dir.join("basalt-launch-1234.args").exists());
        assert!(run_dir.join("keep.txt").is_file());

        let _ = fs::remove_dir_all(&tmp);
    }
}
