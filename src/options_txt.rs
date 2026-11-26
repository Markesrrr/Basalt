use std::fs;
use std::path::Path;

use crate::config::GraphicsSettings;

/// Converts Minecraft's FOV slider scale (`70 deg` = `0.0`, `110 deg` = `1.0`, `30 deg` = `-1.0`).
pub fn fov_degrees_to_options_value(degrees: f32) -> f32 {
    let safe = if degrees.is_finite() { degrees } else { 75.0 };
    ((safe.clamp(30.0, 110.0) - 70.0) / 40.0).clamp(-1.0, 1.0)
}

/// Converts Minecraft's `options.txt` FOV slider value (`-1.0..=1.0`) back to degrees (`30..=110`).
pub fn options_value_to_fov_degrees(raw: f32) -> f32 {
    if !raw.is_finite() {
        return 75.0;
    }
    (raw.clamp(-1.0, 1.0) * 40.0 + 70.0)
        .round()
        .clamp(30.0, 110.0)
}

/// Imports user changes from an existing `run/options.txt` into `gfx` so Basalt
/// stays synchronized with settings modified inside Minecraft. Returns `true` if any field changed.
pub fn read_minecraft_options_into(run_dir: &Path, gfx: &mut GraphicsSettings) -> bool {
    let options_path = run_dir.join("options.txt");
    let Ok(raw) = fs::read(&options_path) else {
        return false;
    };
    let text = String::from_utf8_lossy(&raw);
    let mut changed = false;

    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        let v = val.trim();
        match key.trim() {
            "renderDistance" => {
                if let Ok(parsed) = v.parse::<u32>() {
                    let clamped = parsed.clamp(2, 32);
                    if gfx.render_distance != clamped {
                        gfx.render_distance = clamped;
                        changed = true;
                    }
                }
            }
            "simulationDistance" => {
                if let Ok(parsed) = v.parse::<u32>() {
                    let clamped = parsed.clamp(5, 32);
                    if gfx.simulation_distance != clamped {
                        gfx.simulation_distance = clamped;
                        changed = true;
                    }
                }
            }
            "fov" => {
                if let Ok(parsed) = v.parse::<f32>()
                    && parsed.is_finite()
                {
                    let deg = options_value_to_fov_degrees(parsed);
                    if (gfx.fov_degrees - deg).abs() > 0.5 {
                        gfx.fov_degrees = deg;
                        changed = true;
                    }
                }
            }
            "enableVsync" => {
                if let Ok(parsed) = v.parse::<bool>()
                    && gfx.vsync != parsed
                {
                    gfx.vsync = parsed;
                    changed = true;
                }
            }
            "maxFps" => {
                if let Ok(parsed) = v.parse::<u32>() {
                    let clamped = parsed.clamp(10, 260);
                    if gfx.max_fps != clamped {
                        gfx.max_fps = clamped;
                        changed = true;
                    }
                }
            }
            "mouseSensitivity" => {
                if let Ok(parsed) = v.parse::<f32>()
                    && parsed.is_finite()
                {
                    let clamped = parsed.clamp(0.0, 1.0);
                    if (gfx.mouse_sensitivity - clamped).abs() > 1e-3 {
                        gfx.mouse_sensitivity = clamped;
                        changed = true;
                    }
                }
            }
            "ao" => {
                let parsed = match v {
                    "true" | "1" | "2" => Some(true),
                    "false" | "0" => Some(false),
                    _ => None,
                };
                if let Some(val) = parsed
                    && gfx.smooth_lighting != val
                {
                    gfx.smooth_lighting = val;
                    changed = true;
                }
            }
            "graphicsMode" => {
                let fancy = v != "0";
                if gfx.fancy_graphics != fancy {
                    gfx.fancy_graphics = fancy;
                    changed = true;
                }
            }
            _ => {}
        }
    }

    changed
}

/// Ensures launch-only requirements in `options.txt` (`file/basalt_skin.zip` in `resourcePacks`
/// and `skipMultiplayerWarning:true`) WITHOUT overwriting any existing user graphics/VSync/FPS
/// settings. Seeds initial `options.txt` only when none exists yet.
pub fn ensure_launch_options(
    run_dir: &Path,
    gfx: &GraphicsSettings,
    skin_pack_id: Option<&str>,
) -> Result<(), String> {
    let options_path = run_dir.join("options.txt");
    if !options_path.is_file() {
        return sync_minecraft_options_with_pack(run_dir, gfx, skin_pack_id);
    }

    let raw = fs::read(&options_path)
        .map_err(|e| format!("Failed to read Minecraft options.txt: {e}"))?;
    let existing = String::from_utf8_lossy(&raw).into_owned();

    let mut wrote_resource_packs = false;
    let mut has_skip_warning = false;
    let mut lines: Vec<String> = Vec::new();

    for raw_line in existing.lines() {
        let trimmed = raw_line.trim_end_matches('\r');
        if trimmed.is_empty() {
            continue;
        }
        if let Some((key, val)) = trimmed.split_once(':') {
            if let Some(pack_id) = skin_pack_id
                && key == "resourcePacks"
            {
                if !wrote_resource_packs {
                    lines.push(format!("resourcePacks:{}", merge_resource_packs(val, pack_id)));
                    wrote_resource_packs = true;
                }
                continue;
            }
            if key == "skipMultiplayerWarning" {
                has_skip_warning = true;
            }
        }
        lines.push(trimmed.to_string());
    }

    if !has_skip_warning {
        lines.push("skipMultiplayerWarning:true".to_string());
    }
    if let Some(pack_id) = skin_pack_id
        && !wrote_resource_packs
    {
        lines.push(format!(
            "resourcePacks:{}",
            merge_resource_packs("[\"vanilla\"]", pack_id)
        ));
    }

    let mut output = lines.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }

    if output.replace("\r\n", "\n") == existing.replace("\r\n", "\n") {
        return Ok(());
    }

    fs::write(&options_path, output)
        .map_err(|e| format!("Failed to write Minecraft options.txt: {e}"))
}

/// Writes Basalt's hardware-tuned graphics parameters into Minecraft `run/options.txt`
/// while preserving unrelated user lines (keybinds, sound levels, language, packs).
pub fn sync_minecraft_options(run_dir: &Path, gfx: &GraphicsSettings) -> Result<(), String> {
    sync_minecraft_options_with_pack(run_dir, gfx, None)
}

fn merge_resource_packs(raw: &str, pack_id: &str) -> String {
    let mut packs: Vec<String> = serde_json::from_str(raw.trim()).unwrap_or_default();
    packs.retain(|entry| entry != pack_id);
    if !packs.iter().any(|entry| entry == "vanilla") {
        packs.insert(0, "vanilla".to_string());
    }
    packs.push(pack_id.to_string());
    serde_json::to_string(&packs).unwrap_or_else(|_| format!("[\"vanilla\",\"{pack_id}\"]"))
}

/// Syncs graphics options and optionally guarantees `skin_pack_id` (for example
/// `"file/basalt_skin.zip"`) is enabled in `resourcePacks`.
pub fn sync_minecraft_options_with_pack(
    run_dir: &Path,
    gfx: &GraphicsSettings,
    skin_pack_id: Option<&str>,
) -> Result<(), String> {
    fs::create_dir_all(run_dir)
        .map_err(|e| format!("Failed to create Minecraft run directory: {e}"))?;

    let options_path = run_dir.join("options.txt");
    let existing = match fs::read(&options_path) {
        Ok(raw) => String::from_utf8_lossy(&raw).into_owned(),
        Err(_) => String::new(),
    };

    let fov_val = format!("{:.4}", fov_degrees_to_options_value(gfx.fov_degrees));
    let sens = if gfx.mouse_sensitivity.is_finite() {
        gfx.mouse_sensitivity.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let sens_val = format!("{sens:.4}");
    let graphics_mode = if gfx.fancy_graphics { "1" } else { "0" };

    let managed: [(&str, String); 9] = [
        ("renderDistance", gfx.render_distance.clamp(2, 32).to_string()),
        (
            "simulationDistance",
            gfx.simulation_distance.clamp(5, 32).to_string(),
        ),
        ("fov", fov_val),
        ("enableVsync", gfx.vsync.to_string()),
        ("maxFps", gfx.max_fps.clamp(10, 260).to_string()),
        ("mouseSensitivity", sens_val),
        ("ao", gfx.smooth_lighting.to_string()),
        ("graphicsMode", graphics_mode.to_string()),
        ("skipMultiplayerWarning", "true".to_string()),
    ];

    let mut applied = [false; 9];
    let mut wrote_resource_packs = false;
    let mut lines: Vec<String> = Vec::new();

    for raw_line in existing.lines() {
        let trimmed = raw_line.trim_end_matches('\r');
        if trimmed.is_empty() {
            continue;
        }
        if let Some((key, val)) = trimmed.split_once(':') {
            if let Some(pack_id) = skin_pack_id
                && key == "resourcePacks"
            {
                if !wrote_resource_packs {
                    lines.push(format!("resourcePacks:{}", merge_resource_packs(val, pack_id)));
                    wrote_resource_packs = true;
                }
                continue;
            }
            if let Some(idx) = managed.iter().position(|(k, _)| *k == key) {
                if !applied[idx] {
                    // Preserve Fabulous graphicsMode:2 when fancy_graphics is enabled.
                    let out_val = if key == "graphicsMode"
                        && gfx.fancy_graphics
                        && val.trim() == "2"
                    {
                        "2"
                    } else {
                        managed[idx].1.as_str()
                    };
                    lines.push(format!("{}:{out_val}", managed[idx].0));
                    applied[idx] = true;
                }
                continue;
            }
        }
        lines.push(trimmed.to_string());
    }

    for (idx, (key, val)) in managed.iter().enumerate() {
        if !applied[idx] {
            lines.push(format!("{key}:{val}"));
        }
    }
    if let Some(pack_id) = skin_pack_id
        && !wrote_resource_packs
    {
        lines.push(format!(
            "resourcePacks:{}",
            merge_resource_packs("[\"vanilla\"]", pack_id)
        ));
    }

    let mut output = lines.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }

    fs::write(&options_path, output)
        .map_err(|e| format!("Failed to write Minecraft options.txt: {e}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::sync_minecraft_options;
    use crate::config::GraphicsSettings;

    #[test]
    fn writes_numeric_graphics_mode_for_1_21_4() {
        let dir = std::env::temp_dir().join("basalt_options_txt_test");
        let _ = fs::remove_dir_all(&dir);
        let mut gfx = GraphicsSettings {
            fancy_graphics: true,
            ..Default::default()
        };
        sync_minecraft_options(&dir, &gfx).unwrap();
        let written = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert!(written.contains("graphicsMode:1\n"));

        gfx.fancy_graphics = false;
        sync_minecraft_options(&dir, &gfx).unwrap();
        let written_fast = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert!(written_fast.contains("graphicsMode:0\n"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_legacy_numeric_ao_and_preserves_fabulous_graphics_mode() {
        let dir = std::env::temp_dir().join(format!(
            "basalt_options_legacy_ao_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        fs::write(dir.join("options.txt"), "ao:0\ngraphicsMode:2\n").unwrap();
        let mut gfx = GraphicsSettings {
            smooth_lighting: true,
            fancy_graphics: true,
            ..Default::default()
        };
        assert!(super::read_minecraft_options_into(&dir, &mut gfx));
        assert!(!gfx.smooth_lighting);
        assert!(gfx.fancy_graphics);

        fs::write(dir.join("options.txt"), "ao:2\ngraphicsMode:2\n").unwrap();
        assert!(super::read_minecraft_options_into(&dir, &mut gfx));
        assert!(gfx.smooth_lighting);

        // Syncing with fancy_graphics=true must preserve Fabulous (2), while false switches to Fast (0).
        sync_minecraft_options(&dir, &gfx).unwrap();
        let kept_fabulous = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert!(kept_fabulous.contains("graphicsMode:2\n"));

        gfx.fancy_graphics = false;
        sync_minecraft_options(&dir, &gfx).unwrap();
        let switched_fast = fs::read_to_string(dir.join("options.txt")).unwrap();
        assert!(switched_fast.contains("graphicsMode:0\n"));
        let _ = fs::remove_dir_all(&dir);
    }
}
