use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;

use basalt::assets::{detect_skin_is_slim, generate_default_skin, load_skin_from_file, resolve_skin_is_slim};
use basalt::config::{
    AccentTone, AppConfig, CornerStyle, GraphicsSettings, JvmGcMode, LayoutMode, SkinModel,
    ThemeMode, UiLanguage, normalize_hex_color,
};
use basalt::hardware::{GpuTier, HardwareProfile};
use basalt::options_txt::{
    ensure_launch_options, fov_degrees_to_options_value, read_minecraft_options_into,
    sync_minecraft_options, sync_minecraft_options_with_pack,
};
use basalt::protocol::{
    LocalHandshakeResult, MINECRAFT_1_21_4_PROTOCOL, PacketBuffer, local_player_uuid_dashed,
    parse_host_port, ping_server_1_21_4, probe_local_dev_login_1_21_4, read_framed_packet,
    write_framed_packet,
};
use basalt::saves::{delete_save, list_saves, resolve_save_dir};
use basalt::sounds::{encode_base64, load_local_minecraft_sounds};
use basalt::vanilla::{
    BASALT_SKIN_PACK_NAME, discover_java21_home, evaluate_jdk_candidates, is_ascii_path,
    migrate_legacy_client_gradle_saves, resolve_vanilla_dir,
};
use image::{ImageBuffer, Rgba};

#[test]
fn varint_roundtrip_boundaries() {
    let cases = [0, 1, 127, 128, 255, 25565, 2_097_151, i32::MAX, -1, i32::MIN];
    for val in cases {
        let mut buf = PacketBuffer::new();
        buf.write_varint(val);
        assert_eq!(buf.read_varint().unwrap(), val);
    }
}

#[test]
fn protocol_769_slp_and_encryption_guard() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server_handle = thread::spawn(move || {
        // Call 1: SLP
        {
            let (mut stream, _) = listener.accept().unwrap();
            let (id0, mut hs) = read_framed_packet(&mut stream, None).unwrap();
            assert_eq!(id0, 0x00);
            assert_eq!(hs.read_varint().unwrap(), MINECRAFT_1_21_4_PROTOCOL);
            let _ = read_framed_packet(&mut stream, None).unwrap();

            let mut resp = PacketBuffer::new();
            resp.write_string(
                r#"{"version":{"name":"1.21.4","protocol":769},"players":{"max":10,"online":2},"description":{"text":"Basalt Vanilla Local"}}"#,
            );
            write_framed_packet(&mut stream, 0x00, &resp.data, None).unwrap();
            let (ping_id, mut ping_body) = read_framed_packet(&mut stream, None).unwrap();
            assert_eq!(ping_id, 0x01);
            write_framed_packet(&mut stream, 0x01, &ping_body.data, None).unwrap();
            let _ = &mut ping_body;
        }
        // Call 2: EncryptionRequest refusal
        {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_framed_packet(&mut stream, None).unwrap();
            let _ = read_framed_packet(&mut stream, None).unwrap();
            write_framed_packet(&mut stream, 0x01, &[0x00, 0x01, 0xAA], None).unwrap();
        }
    });

    let info = ping_server_1_21_4(&addr.to_string()).unwrap();
    assert_eq!(info.protocol_version, 769);
    assert_eq!(info.online_players, 2);
    assert_eq!(info.description, "Basalt Vanilla Local");

    let login = probe_local_dev_login_1_21_4(&addr.to_string(), "BasaltTester").unwrap();
    assert!(matches!(login, LocalHandshakeResult::RequiresOfficialAuth));

    server_handle.join().unwrap();
}

#[test]
fn skin_validation_converts_legacy_64x32() {
    let default_skin = generate_default_skin();
    assert_eq!(default_skin.dimensions(), (64, 64));
    assert!(detect_skin_is_slim(&default_skin));
    assert!(resolve_skin_is_slim(SkinModel::Auto, &default_skin));
    assert!(!resolve_skin_is_slim(SkinModel::Classic, &default_skin));
    assert!(resolve_skin_is_slim(SkinModel::Slim, &default_skin));

    let tmp_dir = std::env::temp_dir().join("basalt_skin_test_gradle");
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).unwrap();

    let legacy_path = tmp_dir.join("legacy.png");
    let legacy = ImageBuffer::from_pixel(64, 32, Rgba([120u8, 140, 160, 255]));
    legacy.save(&legacy_path).unwrap();

    let loaded = load_skin_from_file(&legacy_path).unwrap();
    assert_eq!(loaded.dimensions(), (64, 64));
    assert_eq!(*loaded.get_pixel(20, 52), Rgba([120, 140, 160, 255]));
    assert!(!detect_skin_is_slim(&loaded));

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn options_txt_sync_preserves_custom_lines() {
    let tmp_dir = std::env::temp_dir().join("basalt_options_sync_test");
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).unwrap();

    fs::write(
        tmp_dir.join("options.txt"),
        "lang:tr_tr\nkey_key.attack:key.mouse.left\nrenderDistance:6\nfov:0.0\n",
    )
    .unwrap();

    let gfx = GraphicsSettings {
        render_distance: 14,
        simulation_distance: 10,
        fov_degrees: 90.0,
        fancy_graphics: false,
        smooth_lighting: false,
        max_fps: 180,
        vsync: false,
        ..Default::default()
    };
    sync_minecraft_options(&tmp_dir, &gfx).unwrap();

    let written = fs::read_to_string(tmp_dir.join("options.txt")).unwrap();
    assert!(written.contains("lang:tr_tr\n"));
    assert!(written.contains("key_key.attack:key.mouse.left\n"));
    assert!(written.contains("renderDistance:14\n"));
    assert!(written.contains("simulationDistance:10\n"));
    assert!(written.contains("graphicsMode:0\n"));
    assert!(written.contains("ao:false\n"));
    assert!(written.contains("maxFps:180\n"));
    assert!(written.contains("enableVsync:false\n"));
    assert!((fov_degrees_to_options_value(90.0) - 0.5).abs() < 1e-4);

    // Ensure sync_minecraft_options_with_pack enables basalt_skin.zip while preserving user packs
    fs::write(
        tmp_dir.join("options.txt"),
        "lang:tr_tr\nresourcePacks:[\"vanilla\",\"programmer_art\"]\n",
    )
    .unwrap();
    let pack_entry = format!("file/{BASALT_SKIN_PACK_NAME}");
    sync_minecraft_options_with_pack(&tmp_dir, &gfx, Some(&pack_entry)).unwrap();
    let packed = fs::read_to_string(tmp_dir.join("options.txt")).unwrap();
    assert!(packed.contains("resourcePacks:[\"vanilla\",\"programmer_art\",\"file/basalt_skin.zip\"]\n"));

    // ensure_launch_options MUST preserve existing user VSync / FPS / FOV / RenderDistance edits!
    fs::write(
        tmp_dir.join("options.txt"),
        "lang:tr_tr\nenableVsync:false\nmaxFps:260\nrenderDistance:20\nsimulationDistance:14\nfov:0.5000\nao:false\ngraphicsMode:0\nresourcePacks:[\"vanilla\"]\n",
    )
    .unwrap();
    let conflicting_launcher_gfx = GraphicsSettings {
        vsync: true,
        max_fps: 60,
        render_distance: 6,
        simulation_distance: 6,
        fov_degrees: 70.0,
        smooth_lighting: true,
        fancy_graphics: true,
        ..Default::default()
    };
    ensure_launch_options(&tmp_dir, &conflicting_launcher_gfx, Some(&pack_entry)).unwrap();
    let preserved = fs::read_to_string(tmp_dir.join("options.txt")).unwrap();
    assert!(preserved.contains("enableVsync:false\n"));
    assert!(preserved.contains("maxFps:260\n"));
    assert!(preserved.contains("renderDistance:20\n"));
    assert!(preserved.contains("simulationDistance:14\n"));
    assert!(preserved.contains("fov:0.5000\n"));
    assert!(preserved.contains("ao:false\n"));
    assert!(preserved.contains("graphicsMode:0\n"));
    assert!(preserved.contains("skipMultiplayerWarning:true\n"));
    assert!(preserved.contains("resourcePacks:[\"vanilla\",\"file/basalt_skin.zip\"]\n"));

    let mut imported = conflicting_launcher_gfx.clone();
    assert!(read_minecraft_options_into(&tmp_dir, &mut imported));
    assert!(!imported.vsync);
    assert_eq!(imported.max_fps, 260);
    assert_eq!(imported.render_distance, 20);
    assert_eq!(imported.simulation_distance, 14);
    assert!((imported.fov_degrees - 90.0).abs() < 0.5);
    assert!(!imported.smooth_lighting);
    assert!(!imported.fancy_graphics);

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn saves_scanner_and_path_traversal_guard() {
    let tmp_dir = std::env::temp_dir().join("basalt_saves_guard_test");
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(tmp_dir.join("YeniDunya")).unwrap();
    fs::write(tmp_dir.join("YeniDunya").join("level.dat"), [1u8, 2, 3]).unwrap();
    fs::create_dir_all(tmp_dir.join("NotAWorld")).unwrap();

    let saves = list_saves(&tmp_dir).unwrap();
    assert_eq!(saves.len(), 1);
    assert_eq!(saves[0].id, "YeniDunya");

    assert!(resolve_save_dir(&tmp_dir, "../escape").is_err());
    assert!(resolve_save_dir(&tmp_dir, "NotAWorld").is_err());
    assert!(resolve_save_dir(&tmp_dir, "YeniDunya").is_ok());

    delete_save(&tmp_dir, "YeniDunya").unwrap();
    assert!(list_saves(&tmp_dir).unwrap().is_empty());

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn hardware_and_launch_env_builder() {
    let high_hw = HardwareProfile {
        cpu_name: "Test Ryzen".into(),
        physical_cores: 8,
        logical_cores: 16,
        total_ram_mb: 16_384,
        available_ram_mb: 11_000,
        gpu_name: "Test Discrete".into(),
        gpu_backend: "Dx12".into(),
        gpu_device_type: GpuTier::Discrete,
    };
    let (rec, _) = high_hw.recommend_settings(&GraphicsSettings::default());
    assert!(rec.auto_configured);
    assert_eq!(rec.jvm_ram_mb, 4096);
    assert_eq!(rec.jvm_gc, JvmGcMode::ZGC);

    let cfg = AppConfig::default();
    let encoded = toml::to_string_pretty(&cfg).unwrap();
    let decoded: AppConfig = toml::from_str(&encoded).unwrap();
    assert_eq!(decoded.username, "Player");
    assert_eq!(decoded.theme, ThemeMode::Dark);
    assert_eq!(decoded.ui_layout, LayoutMode::Sidebar);
    assert_eq!(decoded.ui_language, UiLanguage::En);
    assert!(!decoded.ui_sounds);

    // Legacy config.toml files without newer fields should load cleanly with defaults
    let legacy_decoded: AppConfig = toml::from_str("username = \"Legacy\"\ntheme = \"Light\"\n").unwrap();
    assert_eq!(legacy_decoded.username, "Legacy");
    assert_eq!(legacy_decoded.theme, ThemeMode::Light);
    assert_eq!(legacy_decoded.ui_layout, LayoutMode::Sidebar);
    assert_eq!(legacy_decoded.ui_language, UiLanguage::En);
    assert!(!legacy_decoded.ui_sounds);

    // Loading a v0 config normalizes desktop defaults to windowed + muted while preserving v1 preferences
    let cfg_tmp_dir = std::env::temp_dir().join(format!("basalt_cfg_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&cfg_tmp_dir);
    fs::create_dir_all(&cfg_tmp_dir).unwrap();
    let v0_path = cfg_tmp_dir.join("cfg_v0.toml");
    fs::write(
        &v0_path,
        "username = \"TestUser\"\nui_layout = \"DeckMode\"\nui_sounds = true\n",
    )
    .unwrap();
    let migrated = AppConfig::load(&v0_path);
    assert_eq!(migrated.config_version, 1);
    assert_eq!(migrated.ui_layout, LayoutMode::Sidebar);
    assert_eq!(migrated.ui_language, UiLanguage::En);
    assert!(!migrated.ui_sounds);

    let mut styled = AppConfig {
        theme: ThemeMode::Sculk,
        ui_layout: LayoutMode::RightRail,
        ui_sounds: true,
        ui_language: UiLanguage::De,
        ..Default::default()
    };
    let styled_path = cfg_tmp_dir.join("cfg_v1.toml");
    styled.save(&styled_path).unwrap();
    let styled_decoded = AppConfig::load(&styled_path);
    assert_eq!(styled_decoded.theme, ThemeMode::Sculk);
    assert_eq!(styled_decoded.ui_layout, LayoutMode::RightRail);
    assert_eq!(styled_decoded.ui_language, UiLanguage::De);
    assert!(styled_decoded.ui_sounds);
    let _ = fs::remove_dir_all(&cfg_tmp_dir);

    for (theme, layout) in [
        (ThemeMode::Ember, LayoutMode::MinimalRail),
        (ThemeMode::End, LayoutMode::DeckMode),
        (ThemeMode::Cherry, LayoutMode::Topbar),
        (ThemeMode::Frost, LayoutMode::CompactDeck),
    ] {
        styled.theme = theme;
        styled.ui_layout = layout;
        styled.ui_accent = AccentTone::Custom;
        styled.custom_accent_hex = normalize_hex_color(" #FF7A45 ");
        styled.ui_corner = CornerStyle::Soft;
        let rt: AppConfig = toml::from_str(&toml::to_string_pretty(&styled).unwrap()).unwrap();
        assert_eq!(rt.theme, theme);
        assert_eq!(rt.ui_layout, layout);
        assert_eq!(rt.ui_accent, AccentTone::Custom);
        assert_eq!(rt.custom_accent_hex.as_deref(), Some("#ff7a45"));
        assert_eq!(rt.ui_corner, CornerStyle::Soft);
    }
    assert!(normalize_hex_color("not-hex").is_none());

    let detected_hw = HardwareProfile::detect_headless();
    assert!(detected_hw.logical_cores >= 1);
    assert!(discover_java21_home().is_some());
    assert!(!local_player_uuid_dashed("BasaltPlayer").is_empty());

    let extract_root = std::env::temp_dir().join(format!("basalt_vanilla_env_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&extract_root);
    let legacy_world = extract_root.join("client-gradle/run/saves/OldWorld");
    fs::create_dir_all(&legacy_world).unwrap();
    fs::write(legacy_world.join("level.dat"), b"dummy-nbt").unwrap();
    let target_saves = extract_root.join("run/saves");
    fs::create_dir_all(&target_saves).unwrap();
    migrate_legacy_client_gradle_saves(&extract_root, &target_saves);
    assert!(target_saves.join("OldWorld/level.dat").is_file());

    let resolved_vanilla = resolve_vanilla_dir(PathBuf::from("C:\\Users\\rab\u{0131}abetl\\AppData\\Local\\Basalt").as_path());
    assert!(is_ascii_path(&resolved_vanilla));
    if let Some(parent_fp) = resolved_vanilla.parent()
        && resolved_vanilla.ends_with("vanilla")
        && parent_fp.file_name().is_some()
    {
        let _ = fs::remove_dir_all(parent_fp);
    }

    let jdk17_dir = extract_root.join("mock_jdk17");
    let jdk21_dir = extract_root.join("mock_jdk21");
    let bin_name = if cfg!(windows) { "java.exe" } else { "java" };
    fs::create_dir_all(jdk17_dir.join("bin")).unwrap();
    fs::write(jdk17_dir.join("bin").join(bin_name), b"").unwrap();
    fs::write(jdk17_dir.join("release"), "JAVA_VERSION=\"17.0.9\"\n").unwrap();

    let only_17 = evaluate_jdk_candidates(std::slice::from_ref(&jdk17_dir));
    assert!(only_17.java21.is_none());
    assert_eq!(only_17.older_detected.as_deref(), Some("Java 17.0.9"));

    fs::create_dir_all(jdk21_dir.join("bin")).unwrap();
    fs::write(jdk21_dir.join("bin").join(bin_name), b"").unwrap();
    fs::write(jdk21_dir.join("release"), "JAVA_VERSION=\"21.0.5\"\n").unwrap();
    let both = evaluate_jdk_candidates(&[jdk17_dir, jdk21_dir.clone()]);
    assert_eq!(both.java21.as_ref().unwrap().1, "21.0.5");

    let _ = fs::remove_dir_all(&extract_root);
}

#[test]
fn minecraft_sounds_local_cache_and_base64() {
    assert_eq!(encode_base64(b"OggS\x00"), "T2dnUwA=");

    let tmp = std::env::temp_dir().join(format!("basalt_sounds_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let sounds_dir = tmp.join("sounds");
    let mock_assets = tmp.join("mock_assets");

    // Seed fake OggS blob at the 1.21.4 hash object for `click` (`3455ca...`)
    let click_hash = "3455ca942556c7d5eac7dd5e458e7fb3bad564c9";
    let click_obj_dir = mock_assets.join("objects").join(&click_hash[..2]);
    fs::create_dir_all(&click_obj_dir).unwrap();
    let mut dummy_ogg = b"OggS".to_vec();
    dummy_ogg.extend_from_slice(&[0u8; 40]);
    fs::write(click_obj_dir.join(click_hash), &dummy_ogg).unwrap();

    // Seed an alternate index-mapped hash for `orb` (`toggleOn`)
    let orb_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let orb_obj_dir = mock_assets.join("objects").join(&orb_hash[..2]);
    fs::create_dir_all(&orb_obj_dir).unwrap();
    fs::write(orb_obj_dir.join(orb_hash), &dummy_ogg).unwrap();
    fs::create_dir_all(mock_assets.join("indexes")).unwrap();
    fs::write(
        mock_assets.join("indexes").join("custom.json"),
        format!("{{\"objects\":{{\"minecraft/sounds/random/orb.ogg\":{{\"hash\":\"{orb_hash}\"}}}}}}"),
    )
    .unwrap();

    let dto = load_local_minecraft_sounds(&sounds_dir, &[mock_assets]);
    assert!(dto.sounds.contains_key("click"));
    assert!(dto.sounds.contains_key("toggleOn"));
    assert!(dto.sounds["click"].starts_with("data:audio/ogg;base64,T2dnUw"));
    assert!(sounds_dir.join("click.ogg").is_file());
    assert!(sounds_dir.join("toggleOn.ogg").is_file());

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn offline_uuid_ipv6_host_port_bounds_and_float_sanitization() {
    // Official Mojang UUID.nameUUIDFromBytes("OfflinePlayer:<name>") parity
    assert_eq!(
        local_player_uuid_dashed("Notch"),
        "b50ad385-829d-3141-a216-7e7d7539ba7f"
    );
    assert_eq!(
        local_player_uuid_dashed("Player"),
        "a01e3843-e521-3998-958a-f459800e4d11"
    );

    // IPv4, IPv6 bracketed/unbracketed, and default port parsing
    assert_eq!(parse_host_port("127.0.0.1:25565"), ("127.0.0.1".to_string(), 25565));
    assert_eq!(parse_host_port(" [::1]:25570 "), ("::1".to_string(), 25570));
    assert_eq!(parse_host_port("[::1]"), ("::1".to_string(), 25565));
    assert_eq!(parse_host_port("::1"), ("::1".to_string(), 25565));
    assert_eq!(parse_host_port("2001:db8::1"), ("2001:db8::1".to_string(), 25565));
    assert_eq!(parse_host_port("mc.example.org"), ("mc.example.org".to_string(), 25565));

    // Compressed packet with negative data_len must return Err cleanly instead of panicking on cast
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let sender = thread::spawn(move || {
        use std::io::Write;
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        let mut frame = PacketBuffer::new();
        frame.write_varint(-5);
        let mut outer = PacketBuffer::new();
        outer.write_varint(frame.data.len() as i32);
        outer.data.extend_from_slice(&frame.data);
        s.write_all(&outer.data).unwrap();
    });
    let (mut server_stream, _) = listener.accept().unwrap();
    let err = read_framed_packet(&mut server_stream, Some(256)).unwrap_err();
    assert!(err.contains("Invalid decompressed packet size"));
    sender.join().unwrap();

    // Non-finite floats must never leak NaN or Infinity into Minecraft options.txt or config load
    let tmp = std::env::temp_dir().join(format!("basalt_nan_guard_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let gfx = GraphicsSettings {
        fov_degrees: f32::NAN,
        mouse_sensitivity: f32::INFINITY,
        ..Default::default()
    };
    sync_minecraft_options(&tmp, &gfx).unwrap();
    let opts = fs::read_to_string(tmp.join("options.txt")).unwrap();
    assert!(!opts.contains("NaN"));
    assert!(!opts.contains("inf"));
    assert!(opts.contains("fov:0.1250\n"));
    assert!(opts.contains("mouseSensitivity:0.5000\n"));

    let cfg_path = tmp.join("config.toml");
    fs::write(
        &cfg_path,
        "[graphics]\npreset = \"Custom\"\nrender_distance = 100\nsimulation_distance = 1\nfov_degrees = nan\nvsync = true\nmax_fps = 500\nmouse_sensitivity = inf\nsmooth_lighting = true\nfancy_graphics = true\njvm_ram_mb = 99999\njvm_gc = \"G1GC\"\nshow_fps_hud = true\nauto_configured = false\n",
    )
    .unwrap();
    let loaded_cfg = AppConfig::load(&cfg_path);
    assert!(loaded_cfg.graphics.fov_degrees.is_finite());
    assert!(loaded_cfg.graphics.mouse_sensitivity.is_finite());
    assert_eq!(loaded_cfg.graphics.render_distance, 32);
    assert_eq!(loaded_cfg.graphics.simulation_distance, 5);
    assert_eq!(loaded_cfg.graphics.max_fps, 260);
    assert_eq!(loaded_cfg.graphics.jvm_ram_mb, 16384);

    let _ = fs::remove_dir_all(&tmp);
}
