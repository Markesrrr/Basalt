use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::config::LoaderKind;
use crate::loaders::{loader_mods_dir, shared_resourcepacks_dir, shared_shaderpacks_dir};
use crate::vanilla::{
    BASALT_SKIN_PACK_NAME, CurlCapabilities, DownloadTask, compute_file_sha1_hex,
    curl_fetch_bytes_with_ua, download_batch_with_curl, hide_window, resolve_curl_exe,
};

const BASALT_HTTP_UA: &str = "Basalt-Launcher/0.3.1 (https://github.com/Markesrrr/Basalt)";
const MODRINTH_API_BASE: &str = "https://api.modrinth.com/v2";
const MANIFEST_INDEX_FILE: &str = ".basalt-manifest.json";
const MAX_DEP_DEPTH: usize = 8;

fn manifest_index_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LibraryKind {
    Mod,
    Shader,
    ResourcePack,
}

impl LibraryKind {
    pub fn modrinth_project_type(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Shader => "shader",
            Self::ResourcePack => "resourcepack",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthSearchHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub author: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub follows: u64,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub project_type: String,
    #[serde(default)]
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModrinthSearchResponse {
    pub hits: Vec<ModrinthSearchHit>,
    pub total_hits: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthRawSearchPayload {
    #[serde(default)]
    hits: Vec<ModrinthRawSearchHit>,
    #[serde(default)]
    total_hits: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthRawSearchHit {
    project_id: String,
    slug: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    icon_url: Option<String>,
    #[serde(default)]
    downloads: u64,
    #[serde(default)]
    follows: u64,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    project_type: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthVersion {
    id: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    version_type: String,
    #[serde(default)]
    files: Vec<ModrinthVersionFile>,
    #[serde(default)]
    dependencies: Vec<ModrinthDependency>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthVersionFile {
    url: String,
    filename: String,
    #[serde(default)]
    primary: bool,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    hashes: ModrinthFileHashes,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ModrinthFileHashes {
    #[serde(default)]
    sha1: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthDependency {
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    dependency_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledLibraryItem {
    pub file_name: String,
    pub display_name: String,
    pub kind: LibraryKind,
    pub enabled: bool,
    pub size_bytes: u64,
    pub path: String,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryCounts {
    pub mods_enabled: usize,
    pub mods_total: usize,
    pub shaders_enabled: usize,
    pub shaders_total: usize,
    pub resourcepacks_enabled: usize,
    pub resourcepacks_total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallOutcome {
    pub installed_files: Vec<String>,
    pub skipped_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModUpdateInfo {
    pub filename: String,
    pub project_id: String,
    pub title: String,
    pub current_version: String,
    pub latest_version: String,
    pub latest_version_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthUpdateVersion {
    id: String,
    #[serde(default)]
    project_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    version_number: String,
    #[serde(default)]
    files: Vec<ModrinthVersionFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ModrinthProjectSummary {
    id: String,
    #[serde(default)]
    title: String,
}

/// Encodes arbitrary UTF-8 query values using RFC-3986 unreserved characters.
pub fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input.as_bytes() {
        let is_unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if is_unreserved {
            out.push(*byte as char);
        } else {
            out.push('%');
            out.push(hex_upper(byte >> 4));
            out.push(hex_upper(byte & 0x0F));
        }
    }
    out
}

fn hex_upper(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        10..=15 => (b'A' + (nibble - 10)) as char,
        _ => '0',
    }
}

/// Builds a Modrinth `/search` `facets` JSON array string.
pub fn build_search_facets(
    kind: LibraryKind,
    loader: LoaderKind,
    mc_version: &str,
    category_filter: Option<&str>,
) -> String {
    let mut facets: Vec<Vec<String>> = vec![vec![format!("project_type:{}", kind.modrinth_project_type())]];
    let trimmed_ver = mc_version.trim();
    if !trimmed_ver.is_empty() {
        facets.push(vec![format!("versions:{trimmed_ver}")]);
    }
    if kind == LibraryKind::Mod && loader != LoaderKind::Vanilla {
        facets.push(vec![format!("categories:{}", loader.as_str())]);
    }
    if let Some(cat) = category_filter.map(str::trim).filter(|c| !c.is_empty()) {
        facets.push(vec![format!("categories:{cat}")]);
    }
    serde_json::to_string(&facets).unwrap_or_else(|_| "[]".to_string())
}

/// Queries Modrinth v2 catalog and annotates hits against locally installed items.
#[allow(clippy::too_many_arguments)]
pub fn search_modrinth(
    query: &str,
    kind: LibraryKind,
    loader: LoaderKind,
    mc_version: &str,
    sort_index: &str,
    limit: u32,
    category_filter: Option<&str>,
    target_dir: &Path,
) -> Result<ModrinthSearchResponse, String> {
    let index = match sort_index {
        "relevance" | "downloads" | "follows" | "newest" | "updated" => sort_index,
        _ => "downloads",
    };
    let facets = build_search_facets(kind, loader, mc_version, category_filter);
    let clamped_limit = limit.clamp(1, 36);
    let mut url = format!(
        "{MODRINTH_API_BASE}/search?facets={}&index={index}&limit={clamped_limit}",
        percent_encode(&facets)
    );
    let trimmed_query = query.trim();
    if !trimmed_query.is_empty() {
        url.push_str("&query=");
        url.push_str(&percent_encode(trimmed_query));
    }

    let bytes = curl_fetch_bytes_with_ua(&url, Some(BASALT_HTTP_UA))?;
    let raw: ModrinthRawSearchPayload = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Failed to parse Modrinth search response: {e}"))?;

    let installed_index = load_manifest_index(target_dir);
    let installed_stems_tokens = collect_installed_stems_lower(target_dir);

    let hits = raw
        .hits
        .into_iter()
        .map(|h| {
            let slug_norm = h.slug.to_ascii_lowercase().replace('-', "");
            let by_manifest = installed_index.contains_key(&h.project_id)
                && installed_index
                    .get(&h.project_id)
                    .is_some_and(|fname| is_item_present(target_dir, fname));
            let by_filename = !slug_norm.is_empty()
                && slug_norm.len() >= 3
                && installed_stems_tokens
                    .iter()
                    .any(|stem| *stem == slug_norm || stem.starts_with(&format!("{slug_norm}0")) || stem.starts_with(&format!("{slug_norm}1")) || stem.starts_with(&format!("{slug_norm}2")));
            ModrinthSearchHit {
                project_id: h.project_id,
                slug: h.slug,
                title: h.title,
                description: h.description,
                author: h.author,
                icon_url: h.icon_url,
                downloads: h.downloads,
                follows: h.follows,
                categories: h.categories,
                project_type: h.project_type,
                installed: by_manifest || by_filename,
            }
        })
        .collect();

    Ok(ModrinthSearchResponse {
        hits,
        total_hits: raw.total_hits,
    })
}

/// Installs a Modrinth project (or slug) into `target_dir`, recursively resolving required dependencies for mods.
pub fn install_modrinth_project(
    project_id_or_slug: &str,
    kind: LibraryKind,
    loader: LoaderKind,
    mc_version: &str,
    target_dir: &Path,
    temp_dir: &Path,
    parallel_streams: u8,
) -> Result<InstallOutcome, String> {
    fs::create_dir_all(target_dir)
        .map_err(|e| format!("Failed to create target folder {}: {e}", target_dir.display()))?;

    let mut visited = HashSet::new();
    let mut tasks = Vec::new();
    let mut manifest_entries: Vec<(String, String)> = Vec::new();
    let mut outcome = InstallOutcome {
        installed_files: Vec::new(),
        skipped_count: 0,
        warnings: Vec::new(),
    };

    collect_project_downloads(
        project_id_or_slug,
        kind,
        loader,
        mc_version,
        target_dir,
        0,
        &mut visited,
        &mut tasks,
        &mut manifest_entries,
        &mut outcome,
    )?;

    if !tasks.is_empty() {
        let refs: Vec<&DownloadTask> = tasks.iter().collect();
        download_batch_with_curl(&refs, temp_dir, parallel_streams)?;
    }

    if !manifest_entries.is_empty() {
        let _guard = manifest_index_lock().lock().unwrap_or_else(|p| p.into_inner());
        let mut index = load_manifest_index(target_dir);
        remove_stale_replaced_files(target_dir, &index, &manifest_entries);
        for (pid, fname) in manifest_entries {
            index.insert(pid, fname);
        }
        save_manifest_index(target_dir, &index);
    }

    Ok(outcome)
}

/// Removes stale prior `.jar`/`.zip` files (and `.disabled` variants) when an existing Modrinth
/// project is updated to a newer filename, preventing duplicate-mod crashes in Fabric/Forge.
fn remove_stale_replaced_files(
    target_dir: &Path,
    existing_index: &HashMap<String, String>,
    new_entries: &[(String, String)],
) -> usize {
    let mut removed = 0usize;
    for (pid, new_fname) in new_entries {
        let Some(prev_fname) = existing_index.get(pid) else {
            continue;
        };
        if prev_fname.eq_ignore_ascii_case(new_fname) {
            continue;
        }
        if !is_item_present(target_dir, new_fname) {
            continue;
        }
        let Ok(safe_prev) = sanitize_single_file_name(prev_fname) else {
            continue;
        };
        for candidate in [safe_prev.clone(), format!("{safe_prev}.disabled")] {
            let path = target_dir.join(&candidate);
            if path.is_file() && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

fn curl_post_json_with_ua(
    url: &str,
    json_body: &[u8],
    user_agent: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut caps = CurlCapabilities::detect();
    let curl_exe = resolve_curl_exe();
    for attempt in 0..2 {
        let mut cmd = Command::new(&curl_exe);
        cmd.args([
            "-fsSL",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            "--retry",
            "1",
            "--connect-timeout",
            "15",
            "--max-time",
            "30",
        ]);
        if caps.supports_ssl_revoke_best_effort {
            cmd.arg("--ssl-revoke-best-effort");
        }
        if let Some(ua) = user_agent {
            cmd.arg("-A").arg(ua);
        }
        cmd.arg(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_window(&mut cmd);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to invoke curl.exe POST for {url}: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(json_body)
                .map_err(|e| format!("Failed to write POST body to curl.exe: {e}"))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| format!("Failed to wait for curl.exe POST ({url}): {e}"))?;
        if out.status.success() {
            return Ok(out.stdout);
        }
        if attempt == 0 && out.status.code() == Some(2) && caps.supports_ssl_revoke_best_effort {
            caps.supports_ssl_revoke_best_effort = false;
            continue;
        }
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "Modrinth API request failed ({url}, exit code {:?}): {stderr}",
            out.status.code()
        ));
    }
    Err(format!("Modrinth API request failed for {url}"))
}

fn update_loaders_for_kind(kind: LibraryKind, loader: LoaderKind) -> Vec<&'static str> {
    match kind {
        LibraryKind::Mod => match loader {
            LoaderKind::Fabric => vec!["fabric"],
            LoaderKind::Forge => vec!["forge"],
            LoaderKind::Vanilla => vec!["fabric", "forge"],
        },
        LibraryKind::Shader => vec!["iris", "optifine"],
        LibraryKind::ResourcePack => vec!["minecraft"],
    }
}

fn build_update_request_body(
    hashes: &[String],
    kind: LibraryKind,
    loader: LoaderKind,
    mc_version: &str,
) -> Vec<u8> {
    let payload = serde_json::json!({
        "hashes": hashes,
        "algorithm": "sha1",
        "loaders": update_loaders_for_kind(kind, loader),
        "game_versions": [mc_version.trim()],
    });
    serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec())
}

fn extract_current_version_label(display_name: &str) -> String {
    if let Some(idx) = display_name.find(|c: char| c.is_ascii_digit()) {
        let start = display_name[..idx]
            .rfind(['-', '_', '+', ' '])
            .map(|p| p + 1)
            .unwrap_or(idx);
        let tail = display_name[start..].trim();
        if !tail.is_empty() {
            return tail.to_string();
        }
    }
    display_name.to_string()
}

fn parse_update_check_response(
    installed_by_hash: &HashMap<String, InstalledLibraryItem>,
    raw_json: &[u8],
) -> Result<Vec<ModUpdateInfo>, String> {
    let updates_map: HashMap<String, ModrinthUpdateVersion> = serde_json::from_slice(raw_json)
        .map_err(|e| format!("Failed to parse Modrinth update response: {e}"))?;

    let mut updates = Vec::new();
    for (hash_key, latest) in updates_map {
        let hash_lower = hash_key.to_ascii_lowercase();
        let Some(item) = installed_by_hash.get(&hash_lower) else {
            continue;
        };
        let primary_file = latest
            .files
            .iter()
            .find(|f| f.primary)
            .or_else(|| latest.files.first());
        let Some(file) = primary_file else {
            continue;
        };
        if let Some(ref latest_sha1) = file.hashes.sha1
            && latest_sha1.eq_ignore_ascii_case(&hash_lower)
        {
            continue;
        }
        let installed_base = item
            .file_name
            .strip_suffix(".disabled")
            .unwrap_or(&item.file_name);
        if file.filename.eq_ignore_ascii_case(installed_base) {
            continue;
        }

        let latest_version = if !latest.version_number.trim().is_empty() {
            latest.version_number.trim().to_string()
        } else if !latest.name.trim().is_empty() {
            latest.name.trim().to_string()
        } else {
            latest.id.clone()
        };
        let fallback_title = if !latest.name.trim().is_empty() {
            latest.name.trim().to_string()
        } else {
            item.display_name.clone()
        };

        updates.push(ModUpdateInfo {
            filename: item.file_name.clone(),
            project_id: latest.project_id.clone(),
            title: fallback_title,
            current_version: extract_current_version_label(&item.display_name),
            latest_version,
            latest_version_id: latest.id,
        });
    }

    updates.sort_by(|a, b| {
        a.filename
            .to_ascii_lowercase()
            .cmp(&b.filename.to_ascii_lowercase())
    });
    Ok(updates)
}

fn enrich_project_titles(updates: &mut [ModUpdateInfo]) {
    let ids: Vec<String> = updates
        .iter()
        .map(|u| u.project_id.clone())
        .filter(|id| !id.is_empty())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if ids.is_empty() {
        return;
    }
    let Ok(ids_json) = serde_json::to_string(&ids) else {
        return;
    };
    let url = format!("{MODRINTH_API_BASE}/projects?ids={}", percent_encode(&ids_json));
    let Ok(raw) = curl_fetch_bytes_with_ua(&url, Some(BASALT_HTTP_UA)) else {
        return;
    };
    let Ok(projects) = serde_json::from_slice::<Vec<ModrinthProjectSummary>>(&raw) else {
        return;
    };
    let title_map: HashMap<String, String> = projects
        .into_iter()
        .filter(|p| !p.title.trim().is_empty())
        .map(|p| (p.id, p.title))
        .collect();
    for u in updates.iter_mut() {
        if let Some(title) = title_map.get(&u.project_id) {
            u.title = title.clone();
        }
    }
}

/// Checks installed library files against Modrinth's SHA-1 `/v2/version_files/update` API
/// for `mc_version` and `loader`, returning only items with a genuinely newer release available.
pub fn check_library_updates(
    run_dir: &Path,
    vanilla_dir: &Path,
    kind: LibraryKind,
    loader: LoaderKind,
    mc_version: &str,
) -> Result<Vec<ModUpdateInfo>, String> {
    let target_dir = match kind {
        LibraryKind::Mod => loader_mods_dir(vanilla_dir, run_dir, loader, mc_version),
        LibraryKind::Shader => shared_shaderpacks_dir(run_dir),
        LibraryKind::ResourcePack => shared_resourcepacks_dir(run_dir),
    };
    let items = list_library_items(&target_dir, kind);
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let mut by_hash: HashMap<String, InstalledLibraryItem> = HashMap::new();
    let mut hashes = Vec::new();
    for item in items {
        let p = Path::new(&item.path);
        if !p.is_file() {
            continue;
        }
        if let Ok(sha1) = compute_file_sha1_hex(p) {
            let lower = sha1.to_ascii_lowercase();
            if let std::collections::hash_map::Entry::Vacant(entry) = by_hash.entry(lower.clone()) {
                hashes.push(lower);
                entry.insert(item);
            }
        }
    }
    if hashes.is_empty() {
        return Ok(Vec::new());
    }

    let body = build_update_request_body(&hashes, kind, loader, mc_version);
    let url = format!("{MODRINTH_API_BASE}/version_files/update");
    let raw = curl_post_json_with_ua(&url, &body, Some(BASALT_HTTP_UA))?;
    let mut updates = parse_update_check_response(&by_hash, &raw)?;
    if !updates.is_empty() {
        enrich_project_titles(&mut updates);
    }
    Ok(updates)
}

fn fetch_specific_version(version_id: &str) -> Result<Option<ModrinthVersion>, String> {
    let encoded = percent_encode(version_id);
    let url = format!("{MODRINTH_API_BASE}/version/{encoded}");
    let bytes = curl_fetch_bytes_with_ua(&url, Some(BASALT_HTTP_UA))?;
    let ver: ModrinthVersion = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Failed to parse Modrinth version {version_id}: {e}"))?;
    Ok(Some(ver))
}

#[allow(clippy::too_many_arguments)]
fn collect_project_downloads(
    project_id_or_slug: &str,
    kind: LibraryKind,
    loader: LoaderKind,
    mc_version: &str,
    target_dir: &Path,
    depth: usize,
    visited: &mut HashSet<String>,
    tasks: &mut Vec<DownloadTask>,
    manifest_entries: &mut Vec<(String, String)>,
    outcome: &mut InstallOutcome,
) -> Result<(), String> {
    let key = project_id_or_slug.trim().to_string();
    if key.is_empty() || !visited.insert(key.clone()) {
        return Ok(());
    }
    if depth > MAX_DEP_DEPTH {
        outcome
            .warnings
            .push(format!("Dependency depth limit reached for {key}"));
        return Ok(());
    }

    let version = match fetch_best_compatible_version(&key, kind, loader, mc_version)? {
        Some(v) => v,
        None => {
            if depth == 0 {
                return Err(format!(
                    "No compatible file found on Modrinth for '{key}' ({mc_version}, {})",
                    loader.as_str()
                ));
            }
            outcome
                .warnings
                .push(format!("Skipped incompatible dependency '{key}' for {mc_version}"));
            return Ok(());
        }
    };

    if !version.project_id.is_empty() {
        visited.insert(version.project_id.clone());
    }

    let file = version
        .files
        .iter()
        .find(|f| f.primary)
        .or_else(|| version.files.first())
        .ok_or_else(|| format!("Modrinth version {} has no downloadable files", version.id))?;

    let safe_name = sanitize_single_file_name(&file.filename)?;
    let lower_name = safe_name.to_ascii_lowercase();
    let expected_ext = match kind {
        LibraryKind::Mod => ".jar",
        LibraryKind::Shader | LibraryKind::ResourcePack => ".zip",
    };
    if !lower_name.ends_with(expected_ext) {
        return Err(format!(
            "Modrinth file '{safe_name}' has unexpected extension (expected {expected_ext})"
        ));
    }
    let expected_sha1 = file.hashes.sha1.clone().unwrap_or_default();
    if expected_sha1.trim().is_empty() && file.size == 0 {
        return Err(format!(
            "Modrinth file '{safe_name}' provides no size or SHA-1 hash for safe verification"
        ));
    }
    let dest = target_dir.join(&safe_name);
    let disabled_dest = target_dir.join(format!("{safe_name}.disabled"));
    if dest.is_file() || disabled_dest.is_file() {
        outcome.skipped_count += 1;
    } else {
        tasks.push(DownloadTask {
            url: file.url.clone(),
            dest,
            sha1: expected_sha1,
            size: file.size,
            label: safe_name.clone(),
        });
        outcome.installed_files.push(safe_name.clone());
    }

    let canonical_id = if version.project_id.is_empty() {
        key
    } else {
        version.project_id.clone()
    };
    manifest_entries.push((canonical_id, safe_name));

    if kind == LibraryKind::Mod {
        for dep in &version.dependencies {
            if dep.dependency_type != "required" {
                continue;
            }
            if let Some(ref dep_pid) = dep.project_id {
                collect_project_downloads(
                    dep_pid,
                    LibraryKind::Mod,
                    loader,
                    mc_version,
                    target_dir,
                    depth + 1,
                    visited,
                    tasks,
                    manifest_entries,
                    outcome,
                )?;
            } else if let Some(ref dep_vid) = dep.version_id
                && visited.insert(format!("ver:{dep_vid}"))
                && let Ok(Some(dep_ver)) = fetch_specific_version(dep_vid)
                && !dep_ver.project_id.is_empty()
            {
                collect_project_downloads(
                    &dep_ver.project_id,
                    LibraryKind::Mod,
                    loader,
                    mc_version,
                    target_dir,
                    depth + 1,
                    visited,
                    tasks,
                    manifest_entries,
                    outcome,
                )?;
            }
        }
    }

    Ok(())
}

fn pick_preferred_version(versions: Vec<ModrinthVersion>) -> Option<ModrinthVersion> {
    if let Some(rel) = versions.iter().position(|v| v.version_type == "release") {
        let mut v = versions;
        return Some(v.swap_remove(rel));
    }
    versions.into_iter().next()
}

fn fetch_best_compatible_version(
    project_id_or_slug: &str,
    kind: LibraryKind,
    loader: LoaderKind,
    mc_version: &str,
) -> Result<Option<ModrinthVersion>, String> {
    let encoded_id = percent_encode(project_id_or_slug);
    let gv = percent_encode(&format!(r#"["{mc_version}"]"#));

    let primary_url = if kind == LibraryKind::Mod && loader != LoaderKind::Vanilla {
        let ld = percent_encode(&format!(r#"["{}"]"#, loader.as_str()));
        format!("{MODRINTH_API_BASE}/project/{encoded_id}/version?game_versions={gv}&loaders={ld}")
    } else {
        format!("{MODRINTH_API_BASE}/project/{encoded_id}/version?game_versions={gv}")
    };

    let bytes = curl_fetch_bytes_with_ua(&primary_url, Some(BASALT_HTTP_UA))?;
    let versions: Vec<ModrinthVersion> = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Failed to parse Modrinth versions for {project_id_or_slug}: {e}"))?;
    if let Some(first) = pick_preferred_version(versions) {
        return Ok(Some(first));
    }

    // Resource packs and shader packs are frequently compatible across patch versions (e.g. 1.21 -> 1.21.4).
    if kind != LibraryKind::Mod {
        let fallback_url = format!("{MODRINTH_API_BASE}/project/{encoded_id}/version");
        let fb_bytes = curl_fetch_bytes_with_ua(&fallback_url, Some(BASALT_HTTP_UA))?;
        let fb_versions: Vec<ModrinthVersion> = serde_json::from_slice(&fb_bytes).map_err(|e| {
            format!("Failed to parse Modrinth fallback versions for {project_id_or_slug}: {e}")
        })?;
        return Ok(pick_preferred_version(fb_versions));
    }

    Ok(None)
}

/// Installs curated 1-click shader & FPS optimization mod kits (`fabric_shaders` or `forge_shaders`).
pub fn install_shader_preset_bundle(
    preset_id: &str,
    mc_version: &str,
    mods_dir: &Path,
    shaderpacks_dir: &Path,
    temp_dir: &Path,
    parallel_streams: u8,
) -> Result<InstallOutcome, String> {
    let (target_loader, mod_slugs, shader_slugs): (LoaderKind, &[&str], &[&str]) = match preset_id {
        "fabric_shaders" => (
            LoaderKind::Fabric,
            &["fabric-api", "iris", "sodium", "lithium"],
            &["complementary-unbound"],
        ),
        "forge_shaders" => (
            LoaderKind::Forge,
            &["embeddium", "oculus"],
            &["complementary-unbound"],
        ),
        _ => return Err(format!("Unknown preset bundle '{preset_id}'")),
    };

    let mut combined = InstallOutcome {
        installed_files: Vec::new(),
        skipped_count: 0,
        warnings: Vec::new(),
    };
    let mut mod_success_count = 0usize;

    for slug in mod_slugs {
        match install_modrinth_project(
            slug,
            LibraryKind::Mod,
            target_loader,
            mc_version,
            mods_dir,
            temp_dir,
            parallel_streams,
        ) {
            Ok(part) => {
                mod_success_count += 1;
                merge_outcome(&mut combined, part);
            }
            Err(err) => combined.warnings.push(format!("{slug}: {err}")),
        }
    }

    if !mod_slugs.is_empty() && mod_success_count == 0 {
        let loader_name = match target_loader {
            LoaderKind::Fabric => "Fabric",
            LoaderKind::Forge => "Forge",
            LoaderKind::Vanilla => "Vanilla",
        };
        return Err(format!(
            "No compatible shader/FPS mods found on Modrinth for {loader_name} {mc_version} ({}). For shaders on 1.21.4 use Fabric, or Forge on 1.20.1 / import OptiFine manually.",
            combined.warnings.join(" | ")
        ));
    }

    for shader_slug in shader_slugs {
        match install_modrinth_project(
            shader_slug,
            LibraryKind::Shader,
            target_loader,
            mc_version,
            shaderpacks_dir,
            temp_dir,
            parallel_streams,
        ) {
            Ok(part) => merge_outcome(&mut combined, part),
            Err(err) => combined.warnings.push(format!("{shader_slug}: {err}")),
        }
    }

    if combined.installed_files.is_empty()
        && combined.skipped_count == 0
        && !combined.warnings.is_empty()
    {
        return Err(combined.warnings.join(" | "));
    }

    Ok(combined)
}

fn merge_outcome(acc: &mut InstallOutcome, part: InstallOutcome) {
    acc.installed_files.extend(part.installed_files);
    acc.skipped_count += part.skipped_count;
    acc.warnings.extend(part.warnings);
}

/// Lists installed mods, shaders, or resource packs in `dir`.
pub fn list_library_items(dir: &Path, kind: LibraryKind) -> Vec<InstalledLibraryItem> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let index = load_manifest_index(dir);
    let reverse_index: HashMap<String, String> = index
        .into_iter()
        .map(|(pid, fname)| (fname, pid))
        .collect();

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let is_disabled = name.ends_with(".disabled");
        let base_name = name.strip_suffix(".disabled").unwrap_or(&name);
        if kind == LibraryKind::ResourcePack && is_managed_internal_pack(base_name) {
            continue;
        }
        let lower = base_name.to_ascii_lowercase();

        let valid = match kind {
            LibraryKind::Mod => meta.is_file() && lower.ends_with(".jar"),
            LibraryKind::Shader | LibraryKind::ResourcePack => {
                meta.is_dir() || (meta.is_file() && lower.ends_with(".zip"))
            }
        };
        if !valid {
            continue;
        }

        let display_name = base_name
            .strip_suffix(".jar")
            .or_else(|| base_name.strip_suffix(".zip"))
            .unwrap_or(base_name)
            .to_string();

        items.push(InstalledLibraryItem {
            file_name: name.clone(),
            display_name,
            kind,
            enabled: !is_disabled,
            size_bytes: if meta.is_file() { meta.len() } else { 0 },
            path: entry.path().to_string_lossy().into_owned(),
            project_id: reverse_index.get(base_name).cloned(),
        });
    }

    items.sort_by(|a, b| {
        a.display_name
            .to_ascii_lowercase()
            .cmp(&b.display_name.to_ascii_lowercase())
    });
    items
}

pub fn count_library_items(mods_dir: &Path, shaderpacks_dir: &Path, resourcepacks_dir: &Path) -> LibraryCounts {
    let mods = list_library_items(mods_dir, LibraryKind::Mod);
    let shaders = list_library_items(shaderpacks_dir, LibraryKind::Shader);
    let packs = list_library_items(resourcepacks_dir, LibraryKind::ResourcePack);
    LibraryCounts {
        mods_enabled: mods.iter().filter(|m| m.enabled).count(),
        mods_total: mods.len(),
        shaders_enabled: shaders.iter().filter(|s| s.enabled).count(),
        shaders_total: shaders.len(),
        resourcepacks_enabled: packs.iter().filter(|p| p.enabled).count(),
        resourcepacks_total: packs.len(),
    }
}

/// Flips `<name>` <-> `<name>.disabled` inside `dir` without overwriting an existing sibling.
pub fn toggle_library_item(dir: &Path, file_name: &str) -> Result<String, String> {
    let safe = sanitize_single_file_name(file_name)?;
    let src = dir.join(&safe);
    if !src.exists() {
        return Err(format!("Item '{safe}' does not exist"));
    }
    let next_name = if let Some(stripped) = safe.strip_suffix(".disabled") {
        stripped.to_string()
    } else {
        format!("{safe}.disabled")
    };
    let dst = dir.join(&next_name);
    if dst.exists() {
        return Err(format!(
            "Cannot toggle '{safe}': target '{next_name}' already exists"
        ));
    }
    fs::rename(&src, &dst).map_err(|e| format!("Failed to rename '{safe}': {e}"))?;
    Ok(next_name)
}

/// Removes a single installed file (or directory pack) from `dir`.
pub fn delete_library_item(dir: &Path, file_name: &str) -> Result<(), String> {
    let safe = sanitize_single_file_name(file_name)?;
    let target = dir.join(&safe);
    let meta = fs::symlink_metadata(&target)
        .map_err(|e| format!("Item '{safe}' could not be accessed: {e}"))?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(&target).map_err(|e| format!("Failed to delete folder '{safe}': {e}"))?;
    } else {
        fs::remove_file(&target).map_err(|e| format!("Failed to delete file '{safe}': {e}"))?;
    }

    let base_name = safe.strip_suffix(".disabled").unwrap_or(&safe);
    if !is_item_present(dir, base_name) {
        let _guard = manifest_index_lock().lock().unwrap_or_else(|p| p.into_inner());
        let mut index = load_manifest_index(dir);
        let before = index.len();
        index.retain(|_, v| v != base_name);
        if index.len() != before {
            save_manifest_index(dir, &index);
        }
    }
    Ok(())
}

/// Imports a user-provided local `.jar` (OptiFine / mod) or `.zip` (shader / resource pack) into `target_dir`.
pub fn import_library_file(src_path: &Path, target_dir: &Path, kind: LibraryKind) -> Result<String, String> {
    if !src_path.is_file() {
        return Err("Selected path is not a file".to_string());
    }
    let raw_name = src_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "Invalid file name".to_string())?;
    let safe_name = sanitize_single_file_name(raw_name)?;
    let lower = safe_name.to_ascii_lowercase();

    let allowed = match kind {
        LibraryKind::Mod => lower.ends_with(".jar"),
        LibraryKind::Shader | LibraryKind::ResourcePack => lower.ends_with(".zip"),
    };
    if !allowed {
        let expected = if kind == LibraryKind::Mod { ".jar" } else { ".zip" };
        return Err(format!("Unsupported file type for this library tab (expected {expected})"));
    }

    fs::create_dir_all(target_dir)
        .map_err(|e| format!("Failed to prepare library folder: {e}"))?;
    let dest = target_dir.join(&safe_name);
    fs::copy(src_path, &dest).map_err(|e| format!("Failed to copy '{safe_name}': {e}"))?;
    Ok(safe_name)
}

/// Validates that `name` is a single safe Windows/Unix filename segment without traversal, reserved devices, or hidden-dotfile collisions.
pub fn sanitize_single_file_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('.')
        || trimmed.ends_with(['.', ' '])
        || trimmed.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'))
    {
        return Err("Invalid file name".to_string());
    }
    let stem_upper = trimmed
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if matches!(
        stem_upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL"
            | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6" | "COM7" | "COM8" | "COM9"
            | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6" | "LPT7" | "LPT8" | "LPT9"
    ) {
        return Err("Reserved device filename is not allowed".to_string());
    }
    let path_buf = PathBuf::from(trimmed);
    let mut components = path_buf.components().collect::<Vec<_>>();
    if components.len() != 1 {
        return Err("Invalid file name path".to_string());
    }
    match components.pop() {
        Some(Component::Normal(os)) => Ok(os.to_string_lossy().into_owned()),
        _ => Err("Unsafe path component".to_string()),
    }
}

fn load_manifest_index(dir: &Path) -> HashMap<String, String> {
    let path = dir.join(MANIFEST_INDEX_FILE);
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<HashMap<String, String>>(&raw).ok())
        .unwrap_or_default()
}

fn save_manifest_index(dir: &Path, index: &HashMap<String, String>) {
    let path = dir.join(MANIFEST_INDEX_FILE);
    if let Ok(json) = serde_json::to_string_pretty(index) {
        let _ = fs::write(path, json);
    }
}

fn is_managed_internal_pack(base_name: &str) -> bool {
    base_name.eq_ignore_ascii_case(BASALT_SKIN_PACK_NAME)
}

fn collect_installed_stems_lower(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let base = name.strip_suffix(".disabled").unwrap_or(&name);
            if is_managed_internal_pack(base) {
                return None;
            }
            let stem = base
                .strip_suffix(".jar")
                .or_else(|| base.strip_suffix(".zip"))
                .unwrap_or(base);
            Some(
                stem.to_ascii_lowercase()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .collect(),
            )
        })
        .collect()
}

fn is_item_present(dir: &Path, base_name: &str) -> bool {
    dir.join(base_name).exists() || dir.join(format!("{base_name}.disabled")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_and_facets_format_cleanly() {
        assert_eq!(percent_encode("bsl shaders!"), "bsl%20shaders%21");
        assert_eq!(percent_encode(r#"["1.21.4"]"#), "%5B%221.21.4%22%5D");

        let mod_facets = build_search_facets(LibraryKind::Mod, LoaderKind::Fabric, "1.21.4", Some("optimization"));
        assert_eq!(
            mod_facets,
            r#"[["project_type:mod"],["versions:1.21.4"],["categories:fabric"],["categories:optimization"]]"#
        );

        let shader_facets = build_search_facets(LibraryKind::Shader, LoaderKind::Forge, "1.21.4", None);
        assert_eq!(
            shader_facets,
            r#"[["project_type:shader"],["versions:1.21.4"]]"#
        );
    }

    #[test]
    fn sanitize_and_toggle_mod_item_roundtrip() {
        assert!(sanitize_single_file_name("../evil.jar").is_err());
        assert!(sanitize_single_file_name("sub/evil.jar").is_err());
        assert!(sanitize_single_file_name(".basalt-manifest.json").is_err());
        assert!(sanitize_single_file_name("CON.jar").is_err());
        assert!(sanitize_single_file_name("mod:stream.jar").is_err());
        assert!(sanitize_single_file_name("mod.jar.").is_err());
        assert_eq!(sanitize_single_file_name("sodium-0.6.5.jar").unwrap(), "sodium-0.6.5.jar");

        let temp_root = std::env::temp_dir().join(format!(
            "basalt_modrinth_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let mod_path = temp_root.join("sodium-0.6.5.jar");
        fs::write(&mod_path, b"jar-bytes").unwrap();

        let items = list_library_items(&temp_root, LibraryKind::Mod);
        assert_eq!(items.len(), 1);
        assert!(items[0].enabled);

        let toggled = toggle_library_item(&temp_root, "sodium-0.6.5.jar").unwrap();
        assert_eq!(toggled, "sodium-0.6.5.jar.disabled");
        let items_after = list_library_items(&temp_root, LibraryKind::Mod);
        assert_eq!(items_after.len(), 1);
        assert!(!items_after[0].enabled);

        // If both enabled and disabled copies exist, toggle refuses instead of overwriting.
        fs::write(&mod_path, b"fresh-bytes").unwrap();
        assert!(toggle_library_item(&temp_root, "sodium-0.6.5.jar").is_err());
        assert!(mod_path.is_file());
        assert!(temp_root.join(&toggled).is_file());

        delete_library_item(&temp_root, "sodium-0.6.5.jar").unwrap();
        delete_library_item(&temp_root, &toggled).unwrap();
        assert!(list_library_items(&temp_root, LibraryKind::Mod).is_empty());
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn excludes_basalt_skin_pack_cleans_replaced_jars_and_parses_updates() {
        let temp_root = std::env::temp_dir().join(format!(
            "basalt_modrinth_phase2_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        fs::create_dir_all(&temp_root).unwrap();

        // 1. Managed basalt_skin.zip (+ .disabled) must be excluded from ResourcePack listings
        fs::write(temp_root.join(BASALT_SKIN_PACK_NAME), b"zip").unwrap();
        fs::write(temp_root.join(format!("{BASALT_SKIN_PACK_NAME}.disabled")), b"zip").unwrap();
        fs::write(temp_root.join("FreshAnimations.zip"), b"zip").unwrap();
        let packs = list_library_items(&temp_root, LibraryKind::ResourcePack);
        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].file_name, "FreshAnimations.zip");

        // 2. Updating a tracked mod removes the stale older .jar after the new .jar is in place
        let mods_dir = temp_root.join("mods");
        fs::create_dir_all(&mods_dir).unwrap();
        fs::write(mods_dir.join("sodium-0.6.4.jar"), b"old").unwrap();
        fs::write(mods_dir.join("sodium-0.6.5.jar"), b"new").unwrap();
        let mut existing = HashMap::new();
        existing.insert("AANobbMI".to_string(), "sodium-0.6.4.jar".to_string());
        let removed = remove_stale_replaced_files(
            &mods_dir,
            &existing,
            &[("AANobbMI".to_string(), "sodium-0.6.5.jar".to_string())],
        );
        assert_eq!(removed, 1);
        assert!(!mods_dir.join("sodium-0.6.4.jar").exists());
        assert!(mods_dir.join("sodium-0.6.5.jar").is_file());

        // 3. Modrinth update check parser returns only genuinely newer releases
        let mut by_hash = HashMap::new();
        by_hash.insert(
            "aaaa".to_string(),
            InstalledLibraryItem {
                file_name: "sodium-0.6.5.jar".to_string(),
                display_name: "sodium-0.6.5".to_string(),
                kind: LibraryKind::Mod,
                enabled: true,
                size_bytes: 100,
                path: mods_dir.join("sodium-0.6.5.jar").to_string_lossy().into_owned(),
                project_id: Some("AANobbMI".to_string()),
            },
        );
        by_hash.insert(
            "bbbb".to_string(),
            InstalledLibraryItem {
                file_name: "iris-1.8.8.jar".to_string(),
                display_name: "iris-1.8.8".to_string(),
                kind: LibraryKind::Mod,
                enabled: true,
                size_bytes: 100,
                path: mods_dir.join("iris-1.8.8.jar").to_string_lossy().into_owned(),
                project_id: Some("YL57xq9U".to_string()),
            },
        );
        let api_resp = serde_json::json!({
            "aaaa": {
                "id": "ver-new-1",
                "project_id": "AANobbMI",
                "name": "Sodium 0.6.9",
                "version_number": "mc1.21.4-0.6.9",
                "files": [{
                    "url": "https://cdn.modrinth.com/sodium-0.6.9.jar",
                    "filename": "sodium-0.6.9.jar",
                    "primary": true,
                    "size": 200,
                    "hashes": { "sha1": "cccc" }
                }]
            },
            "bbbb": {
                "id": "ver-same",
                "project_id": "YL57xq9U",
                "name": "Iris 1.8.8",
                "version_number": "1.8.8",
                "files": [{
                    "url": "https://cdn.modrinth.com/iris-1.8.8.jar",
                    "filename": "iris-1.8.8.jar",
                    "primary": true,
                    "size": 100,
                    "hashes": { "sha1": "bbbb" }
                }]
            }
        });
        let updates = parse_update_check_response(&by_hash, &serde_json::to_vec(&api_resp).unwrap()).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].filename, "sodium-0.6.5.jar");
        assert_eq!(updates[0].project_id, "AANobbMI");
        assert_eq!(updates[0].current_version, "0.6.5");
        assert_eq!(updates[0].latest_version, "mc1.21.4-0.6.9");
        assert_eq!(updates[0].latest_version_id, "ver-new-1");

        let _ = fs::remove_dir_all(&temp_root);
    }
}
