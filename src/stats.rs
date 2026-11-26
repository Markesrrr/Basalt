use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static STATS_TMP_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldPlayStats {
    pub playtime_secs: u64,
    pub sessions: u32,
    pub last_played_epoch_secs: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaytimeStatsDto {
    pub total_playtime_secs: u64,
    pub total_sessions: u32,
    pub per_world: BTreeMap<String, WorldPlayStats>,
}

/// Loads persisted playtime statistics from `stats_path` (returns empty defaults if missing or corrupt).
pub fn load_playtime_stats(stats_path: &Path) -> PlaytimeStatsDto {
    fs::read_to_string(stats_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<PlaytimeStatsDto>(&raw).ok())
        .unwrap_or_default()
}

/// Records a completed play session and atomically persists updated statistics to `stats_path`.
pub fn record_playtime_session(
    stats_path: &Path,
    save_id: Option<&str>,
    duration_secs: u64,
) -> Result<PlaytimeStatsDto, String> {
    let mut stats = load_playtime_stats(stats_path);
    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    stats.total_playtime_secs = stats.total_playtime_secs.saturating_add(duration_secs);
    stats.total_sessions = stats.total_sessions.saturating_add(1);

    if let Some(world_id) = save_id.map(str::trim).filter(|s| !s.is_empty()) {
        let entry = stats.per_world.entry(world_id.to_string()).or_default();
        entry.playtime_secs = entry.playtime_secs.saturating_add(duration_secs);
        entry.sessions = entry.sessions.saturating_add(1);
        entry.last_played_epoch_secs = now_epoch;
    }

    save_playtime_stats_atomic(stats_path, &stats)?;
    Ok(stats)
}

fn save_playtime_stats_atomic(stats_path: &Path, stats: &PlaytimeStatsDto) -> Result<(), String> {
    if let Some(parent) = stats_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to prepare stats directory: {e}"))?;
    }

    let encoded = serde_json::to_string_pretty(stats)
        .map_err(|e| format!("Failed to serialize playtime stats: {e}"))?;

    let seq = STATS_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let file_name = stats_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("stats.json");
    let tmp_path = stats_path.with_file_name(format!(".{file_name}.{pid}.{seq}.tmp"));
    let bak_path = stats_path.with_file_name(format!(".{file_name}.{pid}.{seq}.bak"));

    fs::write(&tmp_path, encoded.as_bytes())
        .map_err(|e| format!("Failed to write temporary stats file: {e}"))?;

    if fs::rename(&tmp_path, stats_path).is_ok() {
        return Ok(());
    }

    // Windows rename fails when destination already exists; perform reversible backup swap.
    let had_existing = stats_path.exists();
    if had_existing {
        let _ = fs::remove_file(&bak_path);
        fs::rename(stats_path, &bak_path).map_err(|e| {
            let _ = fs::remove_file(&tmp_path);
            format!("Failed to stage existing stats file: {e}")
        })?;
    }

    if let Err(err) = fs::rename(&tmp_path, stats_path) {
        if had_existing {
            let _ = fs::rename(&bak_path, stats_path);
        }
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("Failed to commit stats file: {err}"));
    }

    if had_existing {
        let _ = fs::remove_file(&bak_path);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playtime_stats_roundtrip_and_accumulation() {
        let tmp = std::env::temp_dir().join(format!(
            "basalt_stats_unit_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp);
        let stats_path = tmp.join("stats.json");

        assert_eq!(load_playtime_stats(&stats_path), PlaytimeStatsDto::default());

        let s1 = record_playtime_session(&stats_path, Some("MyWorld"), 125).unwrap();
        assert_eq!(s1.total_playtime_secs, 125);
        assert_eq!(s1.total_sessions, 1);
        assert_eq!(s1.per_world["MyWorld"].playtime_secs, 125);
        assert_eq!(s1.per_world["MyWorld"].sessions, 1);

        let s2 = record_playtime_session(&stats_path, Some("MyWorld"), 75).unwrap();
        assert_eq!(s2.total_playtime_secs, 200);
        assert_eq!(s2.total_sessions, 2);
        assert_eq!(s2.per_world["MyWorld"].playtime_secs, 200);
        assert_eq!(s2.per_world["MyWorld"].sessions, 2);

        let s3 = record_playtime_session(&stats_path, None, 40).unwrap();
        assert_eq!(s3.total_playtime_secs, 240);
        assert_eq!(s3.total_sessions, 3);
        assert_eq!(s3.per_world.len(), 1);

        let reloaded = load_playtime_stats(&stats_path);
        assert_eq!(reloaded, s3);

        let _ = fs::remove_dir_all(&tmp);
    }
}
