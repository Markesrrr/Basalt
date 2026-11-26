use flate2::write::DeflateEncoder;
use flate2::{Compression, Crc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sounds::encode_base64;

/// Maximum `icon.png` size (256 KiB) accepted when generating inline world thumbnails.
pub const MAX_WORLD_ICON_BYTES: usize = 256 * 1024;

/// Safety limits for world backup ZIP extraction.
const MAX_BACKUP_ENTRIES: usize = 50_000;
const MAX_BACKUP_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_BACKUP_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;

const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const LOCAL_FILE_HEADER_SIG: u32 = 0x0403_4B50;
const CENTRAL_DIR_HEADER_SIG: u32 = 0x0201_4B50;
const EOCD_SIG: u32 = 0x0605_4B50;

static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinecraftSaveSummary {
    pub id: String,
    pub modified_epoch_secs: u64,
    pub size_bytes: u64,
    #[serde(default)]
    pub icon_data_url: Option<String>,
    #[serde(default)]
    pub backup_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldBackupSummary {
    pub filename: String,
    pub save_id: String,
    pub created_epoch_secs: u64,
    pub size_bytes: u64,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupArchiveComment {
    v: u8,
    save_id: String,
    created_epoch_secs: u64,
    file_count: usize,
}

#[derive(Debug, Clone)]
struct ZipCentralEntry {
    name: String,
    method: u16,
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
}

struct TempDirGuard {
    path: PathBuf,
    armed: bool,
}

impl TempDirGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub fn default_world_backups_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("backups").join("worlds")
}

pub fn validate_save_id(save_id: &str) -> Result<(), String> {
    if save_id.is_empty()
        || save_id == "."
        || save_id.len() > 128
        || save_id.contains('/')
        || save_id.contains('\\')
        || save_id.contains("..")
        || save_id.contains(':')
        || save_id.chars().any(|c| c.is_control())
    {
        return Err("Invalid world folder name.".to_string());
    }
    Ok(())
}

pub fn validate_backup_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty()
        || filename.len() > 200
        || filename.starts_with('.')
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
        || filename.contains(':')
        || filename.chars().any(|c| c.is_control())
        || !filename.to_ascii_lowercase().ends_with(".zip")
    {
        return Err("Invalid world backup filename.".to_string());
    }
    Ok(())
}

pub fn resolve_save_dir(saves_root: &Path, save_id: &str) -> Result<PathBuf, String> {
    validate_save_id(save_id)?;

    let candidate = saves_root.join(save_id);
    if !candidate.join("level.dat").is_file() {
        return Err("Selected Minecraft world (level.dat) was not found.".to_string());
    }

    let canon_root = fs::canonicalize(saves_root)
        .map_err(|e| format!("Failed to verify saves directory: {e}"))?;
    let canon_dir = fs::canonicalize(&candidate)
        .map_err(|e| format!("Failed to verify world directory: {e}"))?;

    if !canon_dir.starts_with(&canon_root) || canon_dir == canon_root {
        return Err("World path is outside the security boundary.".to_string());
    }

    Ok(canon_dir)
}

fn resolve_backup_file(backups_dir: &Path, backup_filename: &str) -> Result<PathBuf, String> {
    validate_backup_filename(backup_filename)?;

    let candidate = backups_dir.join(backup_filename);
    if !candidate.is_file() {
        return Err("Selected world backup archive was not found.".to_string());
    }

    let canon_root = fs::canonicalize(backups_dir)
        .map_err(|e| format!("Failed to verify backups directory: {e}"))?;
    let canon_file = fs::canonicalize(&candidate)
        .map_err(|e| format!("Failed to verify backup archive path: {e}"))?;

    if canon_file.parent() != Some(canon_root.as_path()) || !canon_file.starts_with(&canon_root) {
        return Err("Backup path is outside the security boundary.".to_string());
    }

    Ok(canon_file)
}

/// Safely reads `<world_dir>/icon.png` (if <= 256 KiB and valid PNG) as a `data:image/png;base64,...` URI.
pub fn read_world_icon_data_url(world_dir: &Path) -> Option<String> {
    let icon_path = world_dir.join("icon.png");
    let meta = fs::symlink_metadata(&icon_path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let len = meta.len();
    if len < PNG_MAGIC.len() as u64 || len > MAX_WORLD_ICON_BYTES as u64 {
        return None;
    }

    let canon_world = fs::canonicalize(world_dir).ok()?;
    let canon_icon = fs::canonicalize(&icon_path).ok()?;
    if canon_icon.parent() != Some(canon_world.as_path()) {
        return None;
    }

    let bytes = fs::read(&canon_icon).ok()?;
    if bytes.len() > MAX_WORLD_ICON_BYTES || !bytes.starts_with(PNG_MAGIC) {
        return None;
    }

    Some(format!("data:image/png;base64,{}", encode_base64(&bytes)))
}

pub fn list_saves(saves_root: &Path) -> Result<Vec<MinecraftSaveSummary>, String> {
    list_saves_internal(saves_root, None)
}

pub fn list_saves_with_backups(
    saves_root: &Path,
    backups_dir: &Path,
) -> Result<Vec<MinecraftSaveSummary>, String> {
    list_saves_internal(saves_root, Some(backups_dir))
}

fn list_saves_internal(
    saves_root: &Path,
    backups_dir: Option<&Path>,
) -> Result<Vec<MinecraftSaveSummary>, String> {
    if !saves_root.exists() {
        return Ok(Vec::new());
    }

    let backup_counts: HashMap<String, usize> = backups_dir
        .and_then(|dir| list_all_world_backups(dir).ok())
        .map(|all| {
            let mut counts = HashMap::new();
            for item in all {
                *counts.entry(item.save_id).or_insert(0) += 1;
            }
            counts
        })
        .unwrap_or_default();

    let entries = fs::read_dir(saves_root)
        .map_err(|e| format!("Failed to read Minecraft saves directory: {e}"))?;

    let mut list = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(".basalt_restore_") || validate_save_id(name).is_err() {
            continue;
        }
        let level_dat = path.join("level.dat");
        if !level_dat.is_file() {
            continue;
        }
        let modified_epoch_secs = fs::metadata(&level_dat)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        list.push(MinecraftSaveSummary {
            id: name.to_string(),
            modified_epoch_secs,
            size_bytes: dir_size(&path),
            icon_data_url: read_world_icon_data_url(&path),
            backup_count: backup_counts.get(name).copied().unwrap_or(0),
        });
    }

    list.sort_by(|a, b| {
        b.modified_epoch_secs
            .cmp(&a.modified_epoch_secs)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(list)
}

pub fn delete_save(saves_root: &Path, save_id: &str) -> Result<(), String> {
    let dir = resolve_save_dir(saves_root, save_id)?;
    fs::remove_dir_all(dir).map_err(|e| format!("Failed to delete world directory: {e}"))
}

/// Creates a compressed ZIP backup of `<saves_dir>/<save_id>` inside `backups_dir`.
pub fn create_world_backup(
    saves_dir: &Path,
    backups_dir: &Path,
    save_id: &str,
) -> Result<WorldBackupSummary, String> {
    let world_dir = resolve_save_dir(saves_dir, save_id)?;
    fs::create_dir_all(backups_dir)
        .map_err(|e| format!("Failed to create world backups directory: {e}"))?;

    let mut rel_files: Vec<(String, PathBuf)> = Vec::new();
    let mut rel_empty_dirs: Vec<String> = Vec::new();
    collect_world_entries(&world_dir, &world_dir, &mut rel_files, &mut rel_empty_dirs)?;
    rel_files.sort_by(|a, b| a.0.cmp(&b.0));
    rel_empty_dirs.sort();

    if rel_files.len() + rel_empty_dirs.len() > MAX_BACKUP_ENTRIES {
        return Err(format!(
            "World contains too many entries to archive safely (limit: {MAX_BACKUP_ENTRIES})."
        ));
    }

    let created_epoch_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let file_count = rel_files.len();
    let comment_meta = BackupArchiveComment {
        v: 1,
        save_id: save_id.to_string(),
        created_epoch_secs,
        file_count,
    };
    let comment_bytes = serde_json::to_vec(&comment_meta)
        .map_err(|e| format!("Failed to serialize backup metadata: {e}"))?;

    let (filename, final_path, tmp_path) =
        allocate_backup_paths(backups_dir, save_id, created_epoch_secs)?;

    let write_result = write_world_zip_file(
        &tmp_path,
        save_id,
        &rel_files,
        &rel_empty_dirs,
        &comment_bytes,
    );
    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, &final_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("Failed to finalize world backup archive: {err}"));
    }

    let size_bytes = fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);

    Ok(WorldBackupSummary {
        filename,
        save_id: save_id.to_string(),
        created_epoch_secs,
        size_bytes,
        file_count,
    })
}

/// Lists all backup archives belonging to `save_id`, sorted newest-first.
pub fn list_world_backups(
    backups_dir: &Path,
    save_id: &str,
) -> Result<Vec<WorldBackupSummary>, String> {
    validate_save_id(save_id)?;
    let mut matching: Vec<WorldBackupSummary> = list_all_world_backups(backups_dir)?
        .into_iter()
        .filter(|b| b.save_id == save_id)
        .collect();
    matching.sort_by(|a, b| {
        b.created_epoch_secs
            .cmp(&a.created_epoch_secs)
            .then_with(|| b.filename.cmp(&a.filename))
    });
    Ok(matching)
}

/// Lists all valid world backup archives in `backups_dir`, sorted newest-first.
pub fn list_all_world_backups(backups_dir: &Path) -> Result<Vec<WorldBackupSummary>, String> {
    if !backups_dir.exists() {
        return Ok(Vec::new());
    }
    let canon_root = fs::canonicalize(backups_dir)
        .map_err(|e| format!("Failed to verify backups directory: {e}"))?;
    let entries = fs::read_dir(&canon_root)
        .map_err(|e| format!("Failed to read world backups directory: {e}"))?;

    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if validate_backup_filename(filename).is_err() {
            continue;
        }
        let Ok(canon_file) = fs::canonicalize(&path) else {
            continue;
        };
        if canon_file.parent() != Some(canon_root.as_path()) {
            continue;
        }
        if let Ok(summary) = inspect_backup_archive(&canon_file, filename) {
            backups.push(summary);
        }
    }

    backups.sort_by(|a, b| {
        b.created_epoch_secs
            .cmp(&a.created_epoch_secs)
            .then_with(|| b.filename.cmp(&a.filename))
    });
    Ok(backups)
}

/// Safely restores a world backup ZIP into `saves_dir` using full zip-slip/CRC validation
/// and atomic/reversible staging swap so an existing world is never destroyed on error.
pub fn restore_world_backup(
    saves_dir: &Path,
    backups_dir: &Path,
    backup_filename: &str,
) -> Result<MinecraftSaveSummary, String> {
    let archive_path = resolve_backup_file(backups_dir, backup_filename)?;
    let archive_bytes = fs::read(&archive_path)
        .map_err(|e| format!("Failed to read backup archive '{backup_filename}': {e}"))?;

    let (entries, comment) = parse_zip_central_directory(&archive_bytes)?;
    let save_id = validate_backup_entries(&entries, comment.as_ref())?;

    fs::create_dir_all(saves_dir)
        .map_err(|e| format!("Failed to create saves directory: {e}"))?;
    let canon_saves = fs::canonicalize(saves_dir)
        .map_err(|e| format!("Failed to verify saves directory: {e}"))?;

    let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let stage_dir = canon_saves.join(format!(".basalt_restore_stage_{pid}_{seq}"));
    fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("Failed to create restore staging directory: {e}"))?;
    let mut stage_guard = TempDirGuard::new(stage_dir.clone());
    let canon_stage = fs::canonicalize(&stage_dir)
        .map_err(|e| format!("Failed to verify staging directory: {e}"))?;

    extract_entries_into_stage(&archive_bytes, &entries, &canon_stage)?;

    let staged_world = canon_stage.join(&save_id);
    if !staged_world.join("level.dat").is_file() {
        return Err("Backup archive does not contain a valid level.dat.".to_string());
    }

    let target_world = canon_saves.join(&save_id);
    let prev_backup = canon_saves.join(format!(".basalt_restore_prev_{pid}_{seq}"));

    if target_world.exists() {
        let canon_target = fs::canonicalize(&target_world)
            .map_err(|e| format!("Failed to verify existing world directory: {e}"))?;
        if canon_target.parent() != Some(canon_saves.as_path()) {
            return Err("Existing world path is outside the security boundary.".to_string());
        }

        fs::rename(&target_world, &prev_backup)
            .map_err(|e| format!("Failed to stage existing world before restore: {e}"))?;

        if let Err(err) = fs::rename(&staged_world, &target_world) {
            let _ = fs::rename(&prev_backup, &target_world);
            return Err(format!("Failed to swap restored world into place: {err}"));
        }

        let _ = fs::remove_dir_all(&prev_backup);
    } else {
        fs::rename(&staged_world, &target_world)
            .map_err(|e| format!("Failed to move restored world into saves directory: {e}"))?;
    }

    stage_guard.disarm();
    let _ = fs::remove_dir_all(&stage_dir);

    let level_dat = target_world.join("level.dat");
    let modified_epoch_secs = fs::metadata(&level_dat)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_count = list_world_backups(backups_dir, &save_id)
        .map(|v| v.len())
        .unwrap_or(0);

    Ok(MinecraftSaveSummary {
        id: save_id,
        modified_epoch_secs,
        size_bytes: dir_size(&target_world),
        icon_data_url: read_world_icon_data_url(&target_world),
        backup_count,
    })
}

/// Deletes a single backup archive inside `backups_dir` with strict boundary checks.
pub fn delete_world_backup(backups_dir: &Path, backup_filename: &str) -> Result<(), String> {
    let path = resolve_backup_file(backups_dir, backup_filename)?;
    fs::remove_file(path).map_err(|e| format!("Failed to delete world backup: {e}"))
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let p = entry.path();
        let Ok(meta) = fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            total += dir_size(&p);
        } else if meta.is_file() {
            total += meta.len();
        }
    }
    total
}

fn collect_world_entries(
    world_root: &Path,
    current_dir: &Path,
    files: &mut Vec<(String, PathBuf)>,
    empty_dirs: &mut Vec<String>,
) -> Result<(), String> {
    let read = fs::read_dir(current_dir)
        .map_err(|e| format!("Failed to read world directory '{}': {e}", current_dir.display()))?;
    let mut has_children = false;

    for entry in read {
        let entry = entry.map_err(|e| format!("Failed to inspect world entry: {e}"))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("Failed to read metadata for '{}': {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            continue;
        }

        let rel = path
            .strip_prefix(world_root)
            .map_err(|_| "World file escaped root directory.".to_string())?;
        let rel_str = path_to_zip_rel(rel)?;

        if rel_str == "session.lock" {
            continue;
        }

        has_children = true;
        if meta.is_dir() {
            collect_world_entries(world_root, &path, files, empty_dirs)?;
        } else if meta.is_file() {
            files.push((rel_str, path));
        }
    }

    if !has_children && current_dir != world_root {
        let rel = current_dir
            .strip_prefix(world_root)
            .map_err(|_| "World subdirectory escaped root directory.".to_string())?;
        let rel_str = path_to_zip_rel(rel)?;
        empty_dirs.push(format!("{rel_str}/"));
    }

    Ok(())
}

fn path_to_zip_rel(rel: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for comp in rel.components() {
        match comp {
            std::path::Component::Normal(seg) => {
                let s = seg
                    .to_str()
                    .ok_or_else(|| "World contains a non-UTF-8 path name.".to_string())?;
                if s.is_empty() || s == "." || s == ".." || s.contains('/') || s.contains('\\') {
                    return Err(format!("Unsupported world path segment: {s}"));
                }
                parts.push(s);
            }
            _ => return Err("Unsupported relative path inside world directory.".to_string()),
        }
    }
    if parts.is_empty() {
        return Err("Empty relative path inside world directory.".to_string());
    }
    Ok(parts.join("/"))
}

fn sanitize_backup_slug(save_id: &str) -> String {
    let mut slug = String::with_capacity(save_id.len().min(48));
    for ch in save_id.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            slug.push(ch);
        } else if ch.is_whitespace() {
            slug.push('_');
        }
        if slug.len() >= 48 {
            break;
        }
    }
    let trimmed = slug.trim_matches(|c| c == '_' || c == '-');
    if trimmed.is_empty() {
        "world".to_string()
    } else {
        trimmed.to_string()
    }
}

fn allocate_backup_paths(
    backups_dir: &Path,
    save_id: &str,
    created_epoch_secs: u64,
) -> Result<(String, PathBuf, PathBuf), String> {
    let slug = sanitize_backup_slug(save_id);
    let pid = std::process::id();

    for attempt in 0..1000u32 {
        let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let filename = if attempt == 0 {
            format!("{slug}-{created_epoch_secs}-{seq}.zip")
        } else {
            format!("{slug}-{created_epoch_secs}-{seq}-{attempt}.zip")
        };
        validate_backup_filename(&filename)?;
        let final_path = backups_dir.join(&filename);
        let tmp_path = backups_dir.join(format!(".{filename}.{pid}.{seq}.tmp"));
        if !final_path.exists() && !tmp_path.exists() {
            return Ok((filename, final_path, tmp_path));
        }
    }
    Err("Failed to allocate unique filename for world backup.".to_string())
}

fn write_world_zip_file(
    dest_path: &Path,
    save_id: &str,
    rel_files: &[(String, PathBuf)],
    rel_empty_dirs: &[String],
    comment_bytes: &[u8],
) -> Result<(), String> {
    if comment_bytes.len() > u16::MAX as usize {
        return Err("Backup metadata comment exceeds ZIP limit.".to_string());
    }

    let mut file = File::create(dest_path)
        .map_err(|e| format!("Failed to create temporary backup archive: {e}"))?;
    let mut central = Vec::new();
    let mut offset: u64 = 0;
    let mut entry_count: u32 = 0;

    for dir_rel in rel_empty_dirs {
        let entry_name = format!("{save_id}/{dir_rel}");
        write_zip_entry_sized(&mut file, &mut central, &mut offset, &entry_name, &[], 0, 0, 0)?;
        entry_count += 1;
    }

    for (rel_str, src_path) in rel_files {
        let raw = fs::read(src_path)
            .map_err(|e| format!("Failed to read world file '{rel_str}': {e}"))?;
        let uncompressed_size = raw.len();
        let _ = u32::try_from(uncompressed_size)
            .map_err(|_| format!("World file exceeds 4 GiB ZIP limit: {rel_str}"))?;

        let mut crc = Crc::new();
        crc.update(&raw);
        let crc_val = crc.sum();

        let (method, payload) = if raw.len() >= 64 {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder
                .write_all(&raw)
                .map_err(|e| format!("Failed to compress '{rel_str}': {e}"))?;
            let deflated = encoder
                .finish()
                .map_err(|e| format!("Failed to finalize compression for '{rel_str}': {e}"))?;
            if deflated.len() < raw.len() {
                (8u16, deflated)
            } else {
                (0u16, raw)
            }
        } else {
            (0u16, raw)
        };

        let entry_name = format!("{save_id}/{rel_str}");
        write_zip_entry_sized(
            &mut file,
            &mut central,
            &mut offset,
            &entry_name,
            &payload,
            uncompressed_size,
            method,
            crc_val,
        )?;
        entry_count += 1;
    }

    let central_offset = u32::try_from(offset)
        .map_err(|_| "World backup archive exceeds 4 GiB ZIP32 limit.".to_string())?;
    let central_size = u32::try_from(central.len())
        .map_err(|_| "World backup central directory exceeds 4 GiB limit.".to_string())?;
    let entry_count_u16 = u16::try_from(entry_count)
        .map_err(|_| "World backup contains more than 65,535 entries.".to_string())?;

    file.write_all(&central)
        .map_err(|e| format!("Failed to write ZIP central directory: {e}"))?;

    // End of Central Directory (22 bytes + comment)
    let mut eocd = Vec::with_capacity(22 + comment_bytes.len());
    eocd.extend_from_slice(&EOCD_SIG.to_le_bytes());
    eocd.extend_from_slice(&0u16.to_le_bytes()); // disk number
    eocd.extend_from_slice(&0u16.to_le_bytes()); // central dir start disk
    eocd.extend_from_slice(&entry_count_u16.to_le_bytes());
    eocd.extend_from_slice(&entry_count_u16.to_le_bytes());
    eocd.extend_from_slice(&central_size.to_le_bytes());
    eocd.extend_from_slice(&central_offset.to_le_bytes());
    eocd.extend_from_slice(&(comment_bytes.len() as u16).to_le_bytes());
    eocd.extend_from_slice(comment_bytes);

    file.write_all(&eocd)
        .map_err(|e| format!("Failed to write ZIP EOCD record: {e}"))?;
    file.flush()
        .map_err(|e| format!("Failed to flush backup archive: {e}"))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn write_zip_entry_sized(
    file: &mut File,
    central: &mut Vec<u8>,
    offset: &mut u64,
    entry_name: &str,
    payload: &[u8],
    uncompressed_size: usize,
    method: u16,
    crc_val: u32,
) -> Result<(), String> {
    let name_bytes = entry_name.as_bytes();
    let name_len = u16::try_from(name_bytes.len())
        .map_err(|_| format!("ZIP entry path too long: {entry_name}"))?;
    let comp_len = u32::try_from(payload.len())
        .map_err(|_| format!("ZIP compressed entry exceeds 4 GiB: {entry_name}"))?;
    let uncomp_len = u32::try_from(uncompressed_size)
        .map_err(|_| format!("ZIP uncompressed entry exceeds 4 GiB: {entry_name}"))?;
    let local_offset = u32::try_from(*offset)
        .map_err(|_| "World backup archive exceeds 4 GiB ZIP32 limit.".to_string())?;

    let mut local = Vec::with_capacity(30 + name_bytes.len());
    local.extend_from_slice(&LOCAL_FILE_HEADER_SIG.to_le_bytes());
    local.extend_from_slice(&20u16.to_le_bytes()); // version needed: 2.0
    local.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 filename flag
    local.extend_from_slice(&method.to_le_bytes());
    local.extend_from_slice(&0u16.to_le_bytes()); // mod time
    local.extend_from_slice(&0x5421u16.to_le_bytes()); // mod date
    local.extend_from_slice(&crc_val.to_le_bytes());
    local.extend_from_slice(&comp_len.to_le_bytes());
    local.extend_from_slice(&uncomp_len.to_le_bytes());
    local.extend_from_slice(&name_len.to_le_bytes());
    local.extend_from_slice(&0u16.to_le_bytes()); // extra len
    local.extend_from_slice(name_bytes);

    file.write_all(&local)
        .map_err(|e| format!("Failed to write ZIP header for '{entry_name}': {e}"))?;
    file.write_all(payload)
        .map_err(|e| format!("Failed to write ZIP payload for '{entry_name}': {e}"))?;

    *offset = offset
        .checked_add(local.len() as u64)
        .and_then(|v| v.checked_add(payload.len() as u64))
        .ok_or_else(|| "World backup archive offset overflow.".to_string())?;

    // Central Directory File Header (46 bytes + name)
    central.extend_from_slice(&CENTRAL_DIR_HEADER_SIG.to_le_bytes());
    central.extend_from_slice(&20u16.to_le_bytes()); // version made by
    central.extend_from_slice(&20u16.to_le_bytes()); // version needed
    central.extend_from_slice(&0x0800u16.to_le_bytes()); // UTF-8 flag
    central.extend_from_slice(&method.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes()); // mod time
    central.extend_from_slice(&0x5421u16.to_le_bytes()); // mod date
    central.extend_from_slice(&crc_val.to_le_bytes());
    central.extend_from_slice(&comp_len.to_le_bytes());
    central.extend_from_slice(&uncomp_len.to_le_bytes());
    central.extend_from_slice(&name_len.to_le_bytes());
    central.extend_from_slice(&0u16.to_le_bytes()); // extra len
    central.extend_from_slice(&0u16.to_le_bytes()); // comment len
    central.extend_from_slice(&0u16.to_le_bytes()); // disk number start
    central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
    central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
    central.extend_from_slice(&local_offset.to_le_bytes());
    central.extend_from_slice(name_bytes);

    Ok(())
}

fn inspect_backup_archive(path: &Path, filename: &str) -> Result<WorldBackupSummary, String> {
    let meta = fs::metadata(path).map_err(|e| format!("Failed to stat backup: {e}"))?;
    let size_bytes = meta.len();
    let mtime_secs = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let bytes = fs::read(path).map_err(|e| format!("Failed to read backup: {e}"))?;
    let (entries, comment) = parse_zip_central_directory(&bytes)?;
    let save_id = validate_backup_entries(&entries, comment.as_ref())?;
    let file_count = entries.iter().filter(|e| !e.name.ends_with('/')).count();
    let created_epoch_secs = comment
        .as_ref()
        .map(|c| c.created_epoch_secs)
        .filter(|&ts| ts > 0)
        .unwrap_or(mtime_secs);

    Ok(WorldBackupSummary {
        filename: filename.to_string(),
        save_id,
        created_epoch_secs,
        size_bytes,
        file_count,
    })
}

fn read_u16_le(data: &[u8], offset: usize) -> Result<u16, String> {
    let slice = data
        .get(offset..offset + 2)
        .ok_or_else(|| "Unexpected end of ZIP archive.".to_string())?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32_le(data: &[u8], offset: usize) -> Result<u32, String> {
    let slice = data
        .get(offset..offset + 4)
        .ok_or_else(|| "Unexpected end of ZIP archive.".to_string())?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn parse_zip_central_directory(
    data: &[u8],
) -> Result<(Vec<ZipCentralEntry>, Option<BackupArchiveComment>), String> {
    if data.len() < 22 {
        return Err("Backup archive is truncated or not a valid ZIP file.".to_string());
    }

    let min_eocd_pos = data.len().saturating_sub(22 + u16::MAX as usize);
    let mut eocd_pos = None;
    for pos in (min_eocd_pos..=data.len() - 22).rev() {
        if read_u32_le(data, pos)? == EOCD_SIG {
            let comment_len = read_u16_le(data, pos + 20)? as usize;
            if pos + 22 + comment_len == data.len() {
                eocd_pos = Some(pos);
                break;
            }
        }
    }

    let eocd_pos = eocd_pos.ok_or_else(|| "ZIP End of Central Directory not found.".to_string())?;
    let entry_count = read_u16_le(data, eocd_pos + 10)? as usize;
    let cd_size = read_u32_le(data, eocd_pos + 12)? as usize;
    let cd_offset = read_u32_le(data, eocd_pos + 16)? as usize;
    let comment_len = read_u16_le(data, eocd_pos + 20)? as usize;

    if entry_count > MAX_BACKUP_ENTRIES {
        return Err(format!(
            "Backup archive contains too many entries ({entry_count} > {MAX_BACKUP_ENTRIES})."
        ));
    }
    let cd_end = cd_offset
        .checked_add(cd_size)
        .ok_or_else(|| "Corrupt ZIP central directory range.".to_string())?;
    if cd_end > eocd_pos {
        return Err("ZIP central directory extends past EOCD.".to_string());
    }

    let comment = if comment_len > 0 {
        let raw_comment = &data[eocd_pos + 22..eocd_pos + 22 + comment_len];
        serde_json::from_slice::<BackupArchiveComment>(raw_comment).ok()
    } else {
        None
    };

    let mut entries = Vec::with_capacity(entry_count);
    let mut cursor = cd_offset;
    for _ in 0..entry_count {
        if cursor + 46 > cd_end {
            return Err("Truncated ZIP central directory header.".to_string());
        }
        if read_u32_le(data, cursor)? != CENTRAL_DIR_HEADER_SIG {
            return Err("Invalid ZIP central directory signature.".to_string());
        }
        let method = read_u16_le(data, cursor + 10)?;
        let crc32 = read_u32_le(data, cursor + 16)?;
        let compressed_size = read_u32_le(data, cursor + 20)?;
        let uncompressed_size = read_u32_le(data, cursor + 24)?;
        let name_len = read_u16_le(data, cursor + 28)? as usize;
        let extra_len = read_u16_le(data, cursor + 30)? as usize;
        let entry_comment_len = read_u16_le(data, cursor + 32)? as usize;
        let local_header_offset = read_u32_le(data, cursor + 42)?;

        let name_start = cursor + 46;
        let name_end = name_start + name_len;
        let next_cursor = name_end
            .checked_add(extra_len)
            .and_then(|v| v.checked_add(entry_comment_len))
            .ok_or_else(|| "Corrupt ZIP entry length.".to_string())?;
        if next_cursor > cd_end {
            return Err("ZIP entry metadata overflows central directory.".to_string());
        }

        let name = std::str::from_utf8(&data[name_start..name_end])
            .map_err(|_| "ZIP archive contains a non-UTF-8 entry name.".to_string())?
            .to_string();

        entries.push(ZipCentralEntry {
            name,
            method,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset,
        });

        cursor = next_cursor;
    }

    Ok((entries, comment))
}

fn validate_zip_entry_path(name: &str) -> Result<(String, Vec<&str>, bool), String> {
    if name.is_empty()
        || name.starts_with('/')
        || name.starts_with('\\')
        || name.contains('\\')
        || name.contains(':')
        || name.contains('\0')
    {
        return Err(format!("Blocked unsafe ZIP entry path: '{name}'"));
    }

    let is_dir = name.ends_with('/');
    let trimmed = if is_dir { &name[..name.len() - 1] } else { name };
    if trimmed.is_empty() {
        return Err("Blocked empty ZIP entry path.".to_string());
    }

    let segments: Vec<&str> = trimmed.split('/').collect();
    for seg in &segments {
        if seg.is_empty()
            || *seg == "."
            || *seg == ".."
            || seg.contains("..")
            || seg.chars().any(|c| c.is_control())
        {
            return Err(format!("Blocked path-traversal ZIP entry: '{name}'"));
        }
    }

    let top = segments[0];
    validate_save_id(top)?;
    Ok((top.to_string(), segments, is_dir))
}

fn validate_backup_entries(
    entries: &[ZipCentralEntry],
    comment: Option<&BackupArchiveComment>,
) -> Result<String, String> {
    if entries.is_empty() {
        return Err("Backup archive is empty.".to_string());
    }

    let mut root_save_id: Option<String> = None;
    let mut has_level_dat = false;
    let mut total_uncompressed: u64 = 0;

    for entry in entries {
        let (top, segments, is_dir) = validate_zip_entry_path(&entry.name)?;
        match &root_save_id {
            Some(existing) if existing != &top => {
                return Err(format!(
                    "Backup archive contains multiple root folders ('{existing}' and '{top}')."
                ));
            }
            None => root_save_id = Some(top),
            _ => {}
        }

        if entry.method != 0 && entry.method != 8 {
            return Err(format!(
                "Unsupported ZIP compression method {} in '{}'.",
                entry.method, entry.name
            ));
        }

        if u64::from(entry.uncompressed_size) > MAX_BACKUP_FILE_BYTES {
            return Err(format!("ZIP entry exceeds size limit: '{}'.", entry.name));
        }
        total_uncompressed = total_uncompressed
            .checked_add(u64::from(entry.uncompressed_size))
            .ok_or_else(|| "Backup archive uncompressed size overflow.".to_string())?;
        if total_uncompressed > MAX_BACKUP_TOTAL_BYTES {
            return Err("Backup archive exceeds maximum safe extraction size.".to_string());
        }

        if !is_dir && segments.len() == 2 && segments[1] == "level.dat" {
            has_level_dat = true;
        }
    }

    let save_id = root_save_id.ok_or_else(|| "Backup archive has no root world folder.".to_string())?;
    if let Some(meta) = comment
        && meta.save_id != save_id
    {
        return Err(format!(
            "Backup metadata mismatch: expected '{}', archive contains '{}'.",
            meta.save_id, save_id
        ));
    }
    if !has_level_dat {
        return Err("Backup archive does not contain '<world>/level.dat'.".to_string());
    }

    Ok(save_id)
}

fn extract_entries_into_stage(
    archive: &[u8],
    entries: &[ZipCentralEntry],
    canon_stage: &Path,
) -> Result<(), String> {
    let mut total_written: u64 = 0;

    for entry in entries {
        let (_top, segments, is_dir) = validate_zip_entry_path(&entry.name)?;
        let mut dest = canon_stage.to_path_buf();
        for seg in &segments {
            dest.push(seg);
        }

        if is_dir {
            fs::create_dir_all(&dest)
                .map_err(|e| format!("Failed to create directory '{}': {e}", entry.name))?;
            continue;
        }

        let local_off = entry.local_header_offset as usize;
        if local_off + 30 > archive.len() {
            return Err(format!("Truncated local header for '{}'.", entry.name));
        }
        if read_u32_le(archive, local_off)? != LOCAL_FILE_HEADER_SIG {
            return Err(format!("Invalid local header signature for '{}'.", entry.name));
        }
        let local_name_len = read_u16_le(archive, local_off + 26)? as usize;
        let local_extra_len = read_u16_le(archive, local_off + 28)? as usize;
        let data_start = local_off
            .checked_add(30)
            .and_then(|v| v.checked_add(local_name_len))
            .and_then(|v| v.checked_add(local_extra_len))
            .ok_or_else(|| format!("Corrupt local header offset for '{}'.", entry.name))?;
        let data_end = data_start
            .checked_add(entry.compressed_size as usize)
            .ok_or_else(|| format!("Corrupt compressed payload range for '{}'.", entry.name))?;
        if data_end > archive.len() {
            return Err(format!("Truncated payload for '{}'.", entry.name));
        }

        let compressed_slice = &archive[data_start..data_end];
        let decompressed: Vec<u8> = match entry.method {
            0 => {
                if compressed_slice.len() != entry.uncompressed_size as usize {
                    return Err(format!("Stored entry size mismatch for '{}'.", entry.name));
                }
                compressed_slice.to_vec()
            }
            8 => {
                let limit = (entry.uncompressed_size as u64).min(MAX_BACKUP_FILE_BYTES);
                let mut decoder = flate2::read::DeflateDecoder::new(Cursor::new(compressed_slice))
                    .take(limit + 1);
                let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
                decoder
                    .read_to_end(&mut out)
                    .map_err(|e| format!("Failed to decompress '{}': {e}", entry.name))?;
                if out.len() as u64 > limit {
                    return Err(format!(
                        "Decompressed entry '{}' exceeded declared size limit.",
                        entry.name
                    ));
                }
                out
            }
            other => {
                return Err(format!(
                    "Unsupported ZIP compression method {other} for '{}'.",
                    entry.name
                ));
            }
        };

        if decompressed.len() != entry.uncompressed_size as usize {
            return Err(format!(
                "Uncompressed size mismatch for '{}' (expected {}, got {}).",
                entry.name,
                entry.uncompressed_size,
                decompressed.len()
            ));
        }

        let mut crc = Crc::new();
        crc.update(&decompressed);
        if crc.sum() != entry.crc32 {
            return Err(format!(
                "CRC-32 checksum mismatch for '{}' (corrupt archive).",
                entry.name
            ));
        }

        total_written = total_written
            .checked_add(decompressed.len() as u64)
            .ok_or_else(|| "Total extracted size overflow.".to_string())?;
        if total_written > MAX_BACKUP_TOTAL_BYTES {
            return Err("Backup archive exceeded maximum total extraction size.".to_string());
        }

        let parent = dest
            .parent()
            .ok_or_else(|| format!("Invalid parent path for '{}'.", entry.name))?;
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create parent directory for '{}': {e}", entry.name))?;
        let canon_parent = fs::canonicalize(parent)
            .map_err(|e| format!("Failed to verify staging parent for '{}': {e}", entry.name))?;
        if !canon_parent.starts_with(canon_stage) {
            return Err(format!(
                "Blocked staging directory escape while extracting '{}'.",
                entry.name
            ));
        }

        fs::write(&dest, &decompressed)
            .map_err(|e| format!("Failed to extract '{}': {e}", entry.name))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_zip_entry_preserves_uncompressed_size_for_deflated_files() {
        let tmp = std::env::temp_dir().join(format!(
            "basalt_saves_unit_deflate_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        let saves_dir = tmp.join("saves");
        let backups_dir = tmp.join("backups");
        let world_dir = saves_dir.join("DeflateWorld");
        fs::create_dir_all(world_dir.join("region")).unwrap();
        // Highly compressible payload (>64 bytes) to force method 8 (DEFLATED)
        let repeated = vec![b'A'; 4096];
        fs::write(world_dir.join("level.dat"), &repeated).unwrap();
        fs::write(world_dir.join("region").join("r.0.0.mca"), b"small").unwrap();

        let backup = create_world_backup(&saves_dir, &backups_dir, "DeflateWorld").unwrap();
        assert_eq!(backup.save_id, "DeflateWorld");
        assert_eq!(backup.file_count, 2);

        // Overwrite world with sentinel data, then restore and verify exact bytes
        fs::write(world_dir.join("level.dat"), b"modified").unwrap();
        let restored = restore_world_backup(&saves_dir, &backups_dir, &backup.filename).unwrap();
        assert_eq!(restored.id, "DeflateWorld");
        assert_eq!(fs::read(world_dir.join("level.dat")).unwrap(), repeated);

        let _ = fs::remove_dir_all(&tmp);
    }
}
