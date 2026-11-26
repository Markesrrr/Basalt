use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::sounds::encode_base64;

/// Maximum PNG screenshot size (16 MiB) returned via inline base64 data URL.
pub const MAX_SCREENSHOT_DATA_URL_BYTES: usize = 16 * 1024 * 1024;
const PNG_MAGIC: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenshotItem {
    pub filename: String,
    pub size_bytes: u64,
    pub modified_epoch_secs: u64,
    pub width: u32,
    pub height: u32,
}

pub fn screenshots_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("screenshots")
}

/// Validates that `filename` is a strict single-segment `.png` filename with no path traversal.
pub fn validate_screenshot_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty()
        || filename.len() > 255
        || filename.starts_with('.')
        || filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
        || filename.contains(':')
        || filename.chars().any(|c| c.is_control())
        || !filename.to_ascii_lowercase().ends_with(".png")
    {
        return Err("Invalid screenshot filename.".to_string());
    }
    Ok(())
}

/// Resolves `<run_dir>/screenshots/<filename>` and verifies it stays strictly inside `screenshots/`.
pub fn resolve_screenshot_path(run_dir: &Path, filename: &str) -> Result<PathBuf, String> {
    validate_screenshot_filename(filename)?;

    let root = screenshots_dir(run_dir);
    let candidate = root.join(filename);
    if !candidate.is_file() {
        return Err("Requested screenshot file was not found.".to_string());
    }

    let canon_root = fs::canonicalize(&root)
        .map_err(|e| format!("Failed to verify screenshots directory: {e}"))?;
    let canon_file = fs::canonicalize(&candidate)
        .map_err(|e| format!("Failed to verify screenshot path: {e}"))?;

    if canon_file.parent() != Some(canon_root.as_path()) || !canon_file.starts_with(&canon_root) {
        return Err("Screenshot path is outside the security boundary.".to_string());
    }

    Ok(canon_file)
}

/// Scans `run_dir/screenshots` for valid `.png` files and returns them sorted newest-first.
pub fn list_screenshots(run_dir: &Path) -> Result<Vec<ScreenshotItem>, String> {
    let root = screenshots_dir(run_dir);
    if !root.exists() {
        return Ok(Vec::new());
    }

    let canon_root = fs::canonicalize(&root)
        .map_err(|e| format!("Failed to verify screenshots directory: {e}"))?;
    let entries = fs::read_dir(&canon_root)
        .map_err(|e| format!("Failed to read screenshots directory: {e}"))?;

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(link_meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !link_meta.file_type().is_file() {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if validate_screenshot_filename(filename).is_err() {
            continue;
        }
        let Ok(canon_file) = fs::canonicalize(&path) else {
            continue;
        };
        if canon_file.parent() != Some(canon_root.as_path()) {
            continue;
        }

        let Some((width, height)) = read_png_dimensions(&canon_file) else {
            continue;
        };

        let modified_epoch_secs = link_meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        items.push(ScreenshotItem {
            filename: filename.to_string(),
            size_bytes: link_meta.len(),
            modified_epoch_secs,
            width,
            height,
        });
    }

    items.sort_by(|a, b| {
        b.modified_epoch_secs
            .cmp(&a.modified_epoch_secs)
            .then_with(|| b.filename.cmp(&a.filename))
    });
    Ok(items)
}

/// Reads a single screenshot as a `data:image/png;base64,...` URI with strict path and size guards.
pub fn read_screenshot_data_url(run_dir: &Path, filename: &str) -> Result<String, String> {
    let path = resolve_screenshot_path(run_dir, filename)?;
    let meta = fs::metadata(&path)
        .map_err(|e| format!("Failed to inspect screenshot metadata: {e}"))?;
    if meta.len() < 24 {
        return Err("Screenshot file is truncated or invalid.".to_string());
    }
    if meta.len() > MAX_SCREENSHOT_DATA_URL_BYTES as u64 {
        return Err(format!(
            "Screenshot exceeds maximum preview size limit ({} MiB).",
            MAX_SCREENSHOT_DATA_URL_BYTES / (1024 * 1024)
        ));
    }

    let bytes =
        fs::read(&path).map_err(|e| format!("Failed to read screenshot file: {e}"))?;
    if !bytes.starts_with(PNG_MAGIC) {
        return Err("Screenshot file is not a valid PNG image.".to_string());
    }

    Ok(format!("data:image/png;base64,{}", encode_base64(&bytes)))
}

/// Deletes a single screenshot from `run_dir/screenshots` with strict boundary checks.
pub fn delete_screenshot(run_dir: &Path, filename: &str) -> Result<(), String> {
    let path = resolve_screenshot_path(run_dir, filename)?;
    fs::remove_file(path).map_err(|e| format!("Failed to delete screenshot: {e}"))
}

fn read_png_dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 24];
    file.read_exact(&mut header).ok()?;

    if &header[0..8] != PNG_MAGIC {
        return None;
    }
    // Standard PNG must start with IHDR chunk of length 13
    if header[8..12] != [0, 0, 0, 13] || &header[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_screenshot_filenames_strictly() {
        assert!(validate_screenshot_filename("2026-11-26_22.30.00.png").is_ok());
        assert!(validate_screenshot_filename("SHOT.PNG").is_ok());
        assert!(validate_screenshot_filename("../secret.png").is_err());
        assert!(validate_screenshot_filename("sub/shot.png").is_err());
        assert!(validate_screenshot_filename("sub\\shot.png").is_err());
        assert!(validate_screenshot_filename("C:shot.png").is_err());
        assert!(validate_screenshot_filename(".hidden.png").is_err());
        assert!(validate_screenshot_filename("notes.txt").is_err());
        assert!(validate_screenshot_filename("").is_err());
    }
}
