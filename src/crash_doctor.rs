use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Default lookback window (10 minutes) when matching `run/crash-reports/crash-*.txt`.
pub const DEFAULT_CRASH_REPORT_MAX_AGE_SECS: u64 = 600;
const MAX_CRASH_REPORT_READ_BYTES: u64 = 512 * 1024;
const MAX_EXCERPT_LINES: usize = 12;
const MAX_EXCERPT_LINE_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashCode {
    OutOfMemory,
    JavaVersionMismatch,
    MissingModDependency,
    ModCrash,
    GpuDriverOpengl,
    SessionLock,
    CorruptAssetOrJar,
    UnknownCrash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrashSeverity {
    Critical,
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrashDiagnosis {
    pub code: CrashCode,
    pub severity: CrashSeverity,
    pub offending_mod: Option<String>,
    pub details: String,
    pub suggested_action: String,
    pub log_excerpt: Vec<String>,
    #[serde(default)]
    pub crash_report_path: Option<String>,
}

/// Finds the newest `crash-*.txt` inside `run_dir/crash-reports` modified within `max_age_secs`
/// (or any age when `max_age_secs` is `None`).
pub fn find_recent_crash_report(run_dir: &Path, max_age_secs: Option<u64>) -> Option<PathBuf> {
    let crash_dir = run_dir.join("crash-reports");
    let entries = fs::read_dir(&crash_dir).ok()?;
    let now = SystemTime::now();
    let max_age = max_age_secs.map(Duration::from_secs);

    let mut candidates: Vec<(SystemTime, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("crash-") || !name.ends_with(".txt") {
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.len() == 0 || meta.len() > MAX_CRASH_REPORT_READ_BYTES * 8 {
            continue;
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if let Some(limit) = max_age
            && let Ok(age) = now.duration_since(modified)
            && age > limit
        {
            continue;
        }
        candidates.push((modified, name.to_string(), path));
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    candidates.into_iter().next().map(|(_, _, p)| p)
}

/// Analyzes launch logs together with the most recent crash report under `run_dir/crash-reports` (if recent).
pub fn diagnose_crash(
    log_lines: &[String],
    run_dir: Option<&Path>,
    max_report_age_secs: Option<u64>,
) -> CrashDiagnosis {
    let effective_max_age = match max_report_age_secs {
        Some(0) => None,
        Some(secs) => Some(secs),
        None => Some(DEFAULT_CRASH_REPORT_MAX_AGE_SECS),
    };

    let recent_report_path = run_dir.and_then(|dir| find_recent_crash_report(dir, effective_max_age));
    let report_text = recent_report_path.as_deref().and_then(read_bounded_utf8);

    let mut diagnosis = diagnose_crash_text(log_lines, report_text.as_deref());
    diagnosis.crash_report_path = recent_report_path.map(|p| p.display().to_string());
    diagnosis
}

/// Pure analyzer over captured launch log lines and optional crash report file contents.
pub fn diagnose_crash_text(log_lines: &[String], crash_report_text: Option<&str>) -> CrashDiagnosis {
    let mut combined: Vec<String> = Vec::new();
    if let Some(report) = crash_report_text {
        for line in report.lines() {
            let trimmed = line.trim_end();
            if !trimmed.is_empty() {
                combined.push(trimmed.to_string());
            }
        }
    }
    for line in log_lines {
        let trimmed = line.trim_end();
        if !trimmed.is_empty() {
            combined.push(trimmed.to_string());
        }
    }

    if let Some(diag) = detect_out_of_memory(&combined) {
        return diag;
    }
    if let Some(diag) = detect_java_version_mismatch(&combined) {
        return diag;
    }
    if let Some(diag) = detect_session_lock(&combined) {
        return diag;
    }
    if let Some(diag) = detect_gpu_driver_opengl(&combined) {
        return diag;
    }
    if let Some(diag) = detect_missing_mod_dependency(&combined) {
        return diag;
    }
    if let Some(diag) = detect_corrupt_asset_or_jar(&combined) {
        return diag;
    }
    if let Some(diag) = detect_mod_crash(&combined) {
        return diag;
    }

    CrashDiagnosis {
        code: CrashCode::UnknownCrash,
        severity: CrashSeverity::Error,
        offending_mod: None,
        details: build_unknown_details(&combined),
        suggested_action: "Review the crash log excerpt below or open the full launch log to identify the failing component.".to_string(),
        log_excerpt: extract_generic_excerpt(&combined),
        crash_report_path: None,
    }
}

fn read_bounded_utf8(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let bytes = if meta.len() <= MAX_CRASH_REPORT_READ_BYTES {
        fs::read(path).ok()?
    } else {
        use std::io::Read;
        let mut f = fs::File::open(path).ok()?;
        let mut buf = Vec::with_capacity(MAX_CRASH_REPORT_READ_BYTES as usize);
        f.by_ref()
            .take(MAX_CRASH_REPORT_READ_BYTES)
            .read_to_end(&mut buf)
            .ok()?;
        buf
    };
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn find_first_matching_index<F>(lines: &[String], predicate: F) -> Option<usize>
where
    F: Fn(&str) -> bool,
{
    lines.iter().position(|line| predicate(line))
}

fn detect_out_of_memory(lines: &[String]) -> Option<CrashDiagnosis> {
    let idx = find_first_matching_index(lines, |line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("outofmemoryerror")
            || lower.contains("java heap space")
            || lower.contains("gc overhead limit exceeded")
            || lower.contains("native memory allocation (malloc) failed")
            || lower.contains("insufficient memory for the java runtime")
    })?;

    let hit = &lines[idx];
    let details = if hit.to_ascii_lowercase().contains("native memory")
        || hit.to_ascii_lowercase().contains("insufficient memory")
    {
        "The JVM ran out of native system memory while running Minecraft.".to_string()
    } else {
        "Minecraft exhausted its allocated JVM heap memory (OutOfMemoryError).".to_string()
    };

    Some(CrashDiagnosis {
        code: CrashCode::OutOfMemory,
        severity: CrashSeverity::Critical,
        offending_mod: None,
        details,
        suggested_action: "Increase JVM RAM allocation in Settings (for example 4096–6144 MB) or reduce Render Distance and heavy shaders.".to_string(),
        log_excerpt: extract_window(lines, idx, 2, 8),
        crash_report_path: None,
    })
}

fn detect_java_version_mismatch(lines: &[String]) -> Option<CrashDiagnosis> {
    let idx = find_first_matching_index(lines, |line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("unsupportedclassversionerror")
            || lower.contains("compiled by a more recent version of the java runtime")
            || (lower.contains("class file version") && lower.contains("only recognizes"))
            || lower.contains("unsupported java detected")
            || (lower.contains("requires version") && lower.contains("of 'java'"))
            || (lower.contains("requires java ") && lower.contains("incompatible"))
    })?;

    let required_hint = infer_required_java_version(lines);
    let details = match required_hint {
        Some(ver) => format!(
            "Java runtime version mismatch: this Minecraft version or mod requires {ver}, but an incompatible Java runtime was used."
        ),
        None => "Java runtime version mismatch (UnsupportedClassVersionError): the active JDK is incompatible with this Minecraft version or mod.".to_string(),
    };

    Some(CrashDiagnosis {
        code: CrashCode::JavaVersionMismatch,
        severity: CrashSeverity::Critical,
        offending_mod: None,
        details,
        suggested_action: "Use Java 21 for Minecraft 1.20.5+, Java 17 for 1.18–1.20.4, or Java 8 for 1.16.5 (use Auto-Install Java in Settings if missing).".to_string(),
        log_excerpt: extract_window(lines, idx, 2, 7),
        crash_report_path: None,
    })
}

fn infer_required_java_version(lines: &[String]) -> Option<&'static str> {
    for line in lines {
        if line.contains("class file version 65") || line.contains("version 21 or later of 'java'") {
            return Some("Java 21");
        }
        if line.contains("class file version 61") || line.contains("version 17 or later of 'java'") {
            return Some("Java 17");
        }
        if line.contains("class file version 60") {
            return Some("Java 16");
        }
        if line.contains("class file version 52") {
            return Some("Java 8");
        }
    }
    None
}

fn detect_session_lock(lines: &[String]) -> Option<CrashDiagnosis> {
    let idx = find_first_matching_index(lines, |line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("sessionlock$exception")
            || lower.contains("session.lock: already locked")
            || lower.contains("failed to check session lock")
            || lower.contains("already locked (possibly by other minecraft instance")
    })?;

    Some(CrashDiagnosis {
        code: CrashCode::SessionLock,
        severity: CrashSeverity::Error,
        offending_mod: None,
        details: "The Minecraft world is locked by another active session (SessionLock$Exception).".to_string(),
        suggested_action: "Close any other running Minecraft instance using this world, then try launching again.".to_string(),
        log_excerpt: extract_window(lines, idx, 2, 6),
        crash_report_path: None,
    })
}

fn detect_gpu_driver_opengl(lines: &[String]) -> Option<CrashDiagnosis> {
    let idx = find_first_matching_index(lines, |line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("glfw error 65542")
            || lower.contains("glfw error 65543")
            || lower.contains("glfw error 65544")
            || lower.contains("wgl: the driver does not appear to support opengl")
            || lower.contains("glx: failed to create context")
            || lower.contains("pixel format not accelerated")
            || lower.contains("failed to create window") && lower.contains("opengl")
            || lower.contains("atio6axx.dll")
            || lower.contains("nvoglv64.dll")
            || lower.contains("ig7icd64.dll")
            || lower.contains("ig9icd64.dll")
            || lower.contains("ig11icd64.dll")
            || lower.contains("igxelpicd64.dll")
    })?;

    Some(CrashDiagnosis {
        code: CrashCode::GpuDriverOpengl,
        severity: CrashSeverity::Critical,
        offending_mod: None,
        details: "OpenGL / GPU graphics driver initialization failed (GLFW / WGL driver error).".to_string(),
        suggested_action: "Update your NVIDIA, AMD, or Intel graphics driver and ensure Minecraft runs on your dedicated GPU rather than a basic display adapter.".to_string(),
        log_excerpt: extract_window(lines, idx, 2, 8),
        crash_report_path: None,
    })
}

fn detect_missing_mod_dependency(lines: &[String]) -> Option<CrashDiagnosis> {
    let idx = find_first_matching_index(lines, |line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("modresolutionexception")
            || lower.contains("incompatible mod set")
            || lower.contains("incompatible mods found")
            || lower.contains("missing or unsupported mandatory dependencies")
            || lower.contains("missing mandatory dependencies")
            || lower.contains("could not find required mod:")
            || (lower.contains("requires ") && lower.contains("which is missing"))
    })?;

    let (missing_dep, requesting_mod) = extract_dependency_mods(lines);
    let offending_mod = missing_dep.clone().or_else(|| requesting_mod.clone());

    let details = match (&missing_dep, &requesting_mod) {
        (Some(dep), Some(req)) if dep != req => format!(
            "Mod '{req}' requires missing or incompatible dependency '{dep}'."
        ),
        (Some(dep), _) => format!(
            "Missing or incompatible required mod dependency: '{dep}'."
        ),
        (None, Some(req)) => format!(
            "Mod '{req}' failed dependency resolution due to missing or incompatible mods."
        ),
        (None, None) => "Mod dependency resolution failed: one or more required mods or compatible versions are missing.".to_string(),
    };

    let suggested_action = match &missing_dep {
        Some(dep) => format!(
            "Install '{dep}' (matching your Minecraft version and loader) in the Library tab, or disable the mod requiring it."
        ),
        None => "Install the missing dependency mods listed in the log excerpt or remove incompatible mods in the Library tab.".to_string(),
    };

    Some(CrashDiagnosis {
        code: CrashCode::MissingModDependency,
        severity: CrashSeverity::Error,
        offending_mod,
        details,
        suggested_action,
        log_excerpt: extract_window(lines, idx, 1, 10),
        crash_report_path: None,
    })
}

fn detect_corrupt_asset_or_jar(lines: &[String]) -> Option<CrashDiagnosis> {
    let idx = find_first_matching_index(lines, |line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("java.util.zip.zipexception")
            || lower.contains("zipexception:")
            || lower.contains("ziperror:")
            || lower.contains("error in opening zip file")
            || lower.contains("invalid loc header")
            || lower.contains("invalid end header")
            || lower.contains("java.lang.classnotfoundexception")
            || lower.contains("classnotfoundexception:")
    })?;

    let is_zip = lines[idx].to_ascii_lowercase().contains("zip")
        || lines[idx].to_ascii_lowercase().contains("loc header")
        || lines[idx].to_ascii_lowercase().contains("end header");
    let details = if is_zip {
        "A game JAR, asset archive, or mod file is corrupted or incomplete (ZipException).".to_string()
    } else {
        "A required Java class could not be loaded (ClassNotFoundException), indicating a missing or corrupted JAR/library.".to_string()
    };

    Some(CrashDiagnosis {
        code: CrashCode::CorruptAssetOrJar,
        severity: CrashSeverity::Error,
        offending_mod: None,
        details,
        suggested_action: "Re-verify or re-download Minecraft game files and check recently installed mod .jar files for incomplete downloads.".to_string(),
        log_excerpt: extract_window(lines, idx, 2, 8),
        crash_report_path: None,
    })
}

fn detect_mod_crash(lines: &[String]) -> Option<CrashDiagnosis> {
    let suspected = extract_suspected_mod(lines);
    let mod_error_idx = find_first_matching_index(lines, |line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("suspected mods:")
            || lower.contains("mixinapplyerror")
            || lower.contains("mixintransformererror")
            || lower.contains("invalidmixinexception")
            || lower.contains("modloadingexception")
            || lower.contains("has failed to load correctly")
            || lower.contains("mod file:")
            || lower.contains("from mod ")
            || lower.contains("at transformer/")
            || (lower.contains(".mixins.json") && (lower.contains("error") || lower.contains("exception") || lower.contains("failed")))
    });

    if suspected.is_none() && mod_error_idx.is_none() {
        return None;
    }

    let anchor = mod_error_idx.unwrap_or_else(|| {
        find_first_matching_index(lines, |l| {
            let lower = l.to_ascii_lowercase();
            lower.contains("exception") || lower.contains("error") || lower.contains("caused by:")
        })
        .unwrap_or(0)
    });

    let details = match &suspected {
        Some(mod_id) => format!("A mod crash was detected, with suspected mod '{mod_id}'."),
        None => "A modded Minecraft crash or Mixin injection failure was detected.".to_string(),
    };

    let suggested_action = match &suspected {
        Some(mod_id) => format!(
            "Try disabling or updating '{mod_id}' in the Library tab, then relaunch Minecraft."
        ),
        None => "Disable recently added mods one by one in the Library tab to isolate the conflicting mod.".to_string(),
    };

    Some(CrashDiagnosis {
        code: CrashCode::ModCrash,
        severity: CrashSeverity::Error,
        offending_mod: suspected,
        details,
        suggested_action,
        log_excerpt: extract_window(lines, anchor, 2, 9),
        crash_report_path: None,
    })
}

fn is_valid_mod_id(token: &str) -> bool {
    if token.len() < 2 || token.len() > 64 {
        return false;
    }
    if !token
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return false;
    }
    !matches!(
        token.to_ascii_lowercase().as_str(),
        "minecraft"
            | "java"
            | "fabricloader"
            | "fabric"
            | "forge"
            | "neoforge"
            | "fml"
            | "unknown"
            | "none"
            | "null"
            | "any"
            | "version"
            | "mod"
            | "mods"
            | "client"
            | "server"
    )
}

fn take_mod_id_prefix(s: &str) -> Option<String> {
    let trimmed = s.trim_start_matches(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '`');
    let end = trimmed
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(trimmed.len());
    let candidate = &trimmed[..end];
    if is_valid_mod_id(candidate) {
        Some(candidate.to_ascii_lowercase())
    } else {
        None
    }
}

fn extract_dependency_mods(lines: &[String]) -> (Option<String>, Option<String>) {
    let mut missing_dep: Option<String> = None;
    let mut requesting_mod: Option<String> = None;

    for line in lines {
        let lower = line.to_ascii_lowercase();

        // Forge: "Mod ID: 'geckolib', Requested by: 'alexsmobs'"
        if let Some(pos) = lower.find("mod id:")
            && missing_dep.is_none()
        {
            missing_dep = take_mod_id_prefix(&line[pos + "mod id:".len()..]);
        }
        if let Some(pos) = lower.find("requested by:")
            && requesting_mod.is_none()
        {
            requesting_mod = take_mod_id_prefix(&line[pos + "requested by:".len()..]);
        }

        // Forge: "Missing mandatory dependencies: curios"
        if let Some(pos) = lower.find("missing mandatory dependencies:")
            && missing_dep.is_none()
        {
            missing_dep = take_mod_id_prefix(&line[pos + "missing mandatory dependencies:".len()..]);
        }

        // Fabric: "Could not find required mod: fabric-api"
        if let Some(pos) = lower.find("could not find required mod:")
            && missing_dep.is_none()
        {
            missing_dep = take_mod_id_prefix(&line[pos + "could not find required mod:".len()..]);
        }

        // Fabric: "Mod 'Iris' (iris) 1.8.0 requires any version of sodium, which is missing!"
        if let Some(pos) = lower.find("mod '")
            && requesting_mod.is_none()
            && let Some(open_paren) = line[pos..].find('(')
            && let Some(close_paren) = line[pos + open_paren + 1..].find(')')
        {
            let inside = &line[pos + open_paren + 1..pos + open_paren + 1 + close_paren];
            if let Some(id) = take_mod_id_prefix(inside) {
                requesting_mod = Some(id);
            }
        }

        if let Some(req_pos) = lower.find("requires ")
            && missing_dep.is_none()
        {
            let after_req = &line[req_pos + "requires ".len()..];
            let after_req_lower = after_req.to_ascii_lowercase();
            if let Some(of_pos) = after_req_lower.find(" of ") {
                let tail = &after_req[of_pos + " of ".len()..];
                // Handles both "of sodium," and "of mod 'Sodium' (sodium)"
                if let Some(open_p) = tail.find('(')
                    && let Some(close_p) = tail[open_p + 1..].find(')')
                    && let Some(id) = take_mod_id_prefix(&tail[open_p + 1..open_p + 1 + close_p])
                {
                    missing_dep = Some(id);
                } else if let Some(id) = take_mod_id_prefix(tail) {
                    missing_dep = Some(id);
                }
            }
        }

        // Fabric solution hint: "- Install fabric-api, any version."
        if let Some(pos) = lower.find("- install ")
            && missing_dep.is_none()
        {
            missing_dep = take_mod_id_prefix(&line[pos + "- install ".len()..]);
        }
    }

    (missing_dep, requesting_mod)
}

fn extract_suspected_mod(lines: &[String]) -> Option<String> {
    // Pass 1: Fabric crash report "Suspected Mods: Sodium (sodium), Minecraft (minecraft)"
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("suspected mods:") {
            let tail = &line[pos + "suspected mods:".len()..];
            for item in tail.split(',') {
                if let Some(open) = item.find('(')
                    && let Some(close) = item[open + 1..].find(')')
                    && let Some(id) = take_mod_id_prefix(&item[open + 1..open + 1 + close])
                {
                    return Some(id);
                }
                if let Some(id) = take_mod_id_prefix(item) {
                    return Some(id);
                }
            }
        }
    }

    // Pass 2: Mixin / Fabric "from mod <modid>" or "<modid>.mixins.json"
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("from mod ")
            && let Some(id) = take_mod_id_prefix(&line[pos + "from mod ".len()..])
        {
            return Some(id);
        }
        if let Some(pos) = lower.find(".mixins.json") {
            let prefix = &line[..pos];
            let start = prefix
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .map(|i| i + 1)
                .unwrap_or(0);
            let candidate = &prefix[start..];
            if is_valid_mod_id(candidate) {
                return Some(candidate.to_ascii_lowercase());
            }
        }
    }

    // Pass 3: Forge / NeoForge "at TRANSFORMER/<modid>@..." or "Mod <modid> failed" or "Mod File: /.../foo-1.0.jar"
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(pos) = lower.find("at transformer/")
            && let Some(id) = take_mod_id_prefix(&line[pos + "at transformer/".len()..])
        {
            return Some(id);
        }
        if let Some(pos) = lower.find("mod file:") {
            let tail = line[pos + "mod file:".len()..].trim();
            let file_part = tail.rsplit(['/', '\\']).next().unwrap_or(tail);
            if let Some(stem) = file_part.strip_suffix(".jar") {
                let first_seg = stem.split(['-', '_']).next().unwrap_or(stem);
                if is_valid_mod_id(first_seg) {
                    return Some(first_seg.to_ascii_lowercase());
                }
            }
        }
        if let Some(pos) = lower.find("has failed to load correctly") {
            let before = line[..pos].trim();
            // e.g. "Create (create) has failed to load correctly"
            if let Some(open) = before.rfind('(')
                && let Some(close) = before[open + 1..].find(')')
                && let Some(id) = take_mod_id_prefix(&before[open + 1..open + 1 + close])
            {
                return Some(id);
            }
            if let Some(last_word) = before.split_whitespace().last()
                && let Some(id) = take_mod_id_prefix(last_word)
            {
                return Some(id);
            }
        }
    }

    None
}

fn extract_window(lines: &[String], anchor: usize, before: usize, after: usize) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    let start = anchor.saturating_sub(before);
    let end = (anchor + after + 1).min(lines.len());
    lines[start..end]
        .iter()
        .take(MAX_EXCERPT_LINES)
        .map(|s| truncate_line(s))
        .collect()
}

fn extract_generic_excerpt(lines: &[String]) -> Vec<String> {
    if lines.is_empty() {
        return Vec::new();
    }
    if let Some(idx) = find_first_matching_index(lines, |line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("description:")
            || lower.contains("caused by:")
            || lower.contains("exception")
            || lower.contains("fatal")
            || lower.contains("error")
    }) {
        return extract_window(lines, idx, 1, 9);
    }

    let start = lines.len().saturating_sub(MAX_EXCERPT_LINES);
    lines[start..].iter().map(|s| truncate_line(s)).collect()
}

fn build_unknown_details(lines: &[String]) -> String {
    for line in lines {
        if let Some(rest) = line.strip_prefix("Description:") {
            let desc = rest.trim();
            if !desc.is_empty() {
                return format!("Minecraft crashed: {}", truncate_line(desc));
            }
        }
    }
    for line in lines.iter().rev() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("caused by:") || lower.contains("exception") || lower.contains("fatal") {
            return format!("Minecraft exited with an unrecognized error: {}", truncate_line(line));
        }
    }
    "Minecraft exited unexpectedly without a recognized crash signature.".to_string()
}

fn truncate_line(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= MAX_EXCERPT_LINE_CHARS {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed.chars().take(MAX_EXCERPT_LINE_CHARS).collect();
        out.push_str("...");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_lines(raw: &str) -> Vec<String> {
        raw.lines().map(|l| l.to_string()).collect()
    }

    #[test]
    fn detects_all_known_minecraft_crash_patterns() {
        let oom = diagnose_crash_text(
            &to_lines("[main/ERROR]: java.lang.OutOfMemoryError: Java heap space\n\tat net.minecraft.world.Chunk.load(Chunk.java:42)"),
            None,
        );
        assert_eq!(oom.code, CrashCode::OutOfMemory);
        assert_eq!(oom.severity, CrashSeverity::Critical);
        assert!(!oom.log_excerpt.is_empty());

        let java = diagnose_crash_text(
            &to_lines("Exception in thread \"main\" java.lang.UnsupportedClassVersionError: net/minecraft/client/main/Main has been compiled by a more recent version of the Java Runtime (class file version 65.0), this version of the Java Runtime only recognizes class file versions up to 61.0"),
            None,
        );
        assert_eq!(java.code, CrashCode::JavaVersionMismatch);
        assert!(java.details.contains("Java 21"));

        let dep_fabric = diagnose_crash_text(
            &to_lines(
                "net.fabricmc.loader.impl.FormattedException: net.fabricmc.loader.impl.discovery.ModResolutionException: Mod resolution encountered an incompatible mod set!\n\t - Mod 'Iris' (iris) 1.8.0 requires any version of sodium, which is missing!\n\t - Install sodium, any version.",
            ),
            None,
        );
        assert_eq!(dep_fabric.code, CrashCode::MissingModDependency);
        assert_eq!(dep_fabric.offending_mod.as_deref(), Some("sodium"));

        let dep_forge = diagnose_crash_text(
            &to_lines(
                "Missing or unsupported mandatory dependencies:\n\tMod ID: 'geckolib', Requested by: 'alexsmobs', Expected range: '[4.0,)'",
            ),
            None,
        );
        assert_eq!(dep_forge.code, CrashCode::MissingModDependency);
        assert_eq!(dep_forge.offending_mod.as_deref(), Some("geckolib"));

        let mod_crash = diagnose_crash_text(
            &[],
            Some(
                "---- Minecraft Crash Report ----\nDescription: Initializing game\norg.spongepowered.asm.mixin.transformer.throwables.MixinTransformerError: An unexpected critical error was encountered\nSuspected Mods: Minecraft (minecraft), Lithium (lithium)\n",
            ),
        );
        assert_eq!(mod_crash.code, CrashCode::ModCrash);
        assert_eq!(mod_crash.offending_mod.as_deref(), Some("lithium"));

        let gpu = diagnose_crash_text(
            &to_lines("[Render thread/ERROR]: GLFW error 65542: WGL: The driver does not appear to support OpenGL"),
            None,
        );
        assert_eq!(gpu.code, CrashCode::GpuDriverOpengl);
        assert_eq!(gpu.severity, CrashSeverity::Critical);

        let lock = diagnose_crash_text(
            &to_lines("net.minecraft.util.SessionLock$Exception: C:\\saves\\World\\session.lock: already locked (possibly by other Minecraft instance?)"),
            None,
        );
        assert_eq!(lock.code, CrashCode::SessionLock);

        let corrupt = diagnose_crash_text(
            &to_lines("java.util.zip.ZipException: invalid LOC header (bad signature)"),
            None,
        );
        assert_eq!(corrupt.code, CrashCode::CorruptAssetOrJar);

        let cnf = diagnose_crash_text(
            &to_lines("Error: Could not find or load main class\nCaused by: java.lang.ClassNotFoundException: net.minecraft.client.main.Main"),
            None,
        );
        assert_eq!(cnf.code, CrashCode::CorruptAssetOrJar);

        let unknown = diagnose_crash_text(&to_lines("Something unexpected happened"), None);
        assert_eq!(unknown.code, CrashCode::UnknownCrash);
        assert_eq!(unknown.offending_mod, None);
    }
}
