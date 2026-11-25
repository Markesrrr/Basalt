import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AccentTone,
  AppConfig,
  BootstrapDto,
  ClientOptimizationReport,
  CornerStyle,
  GraphicsSettings,
  ImportLibraryDto,
  InstallActionDto,
  InstalledLibraryItem,
  LaunchLogPayload,
  LaunchMode,
  LayoutMode,
  LibraryCounts,
  LibraryKind,
  LoaderKind,
  LocalHandshakeResult,
  MinecraftSaveSummary,
  MinecraftSoundsDto,
  ModrinthSearchHit,
  ModrinthSearchResponse,
  OptimizeResultDto,
  RunningSessionDto,
  SavedServer,
  ServerPingInfo,
  SessionEndedPayload,
  SkinModel,
  SkinModelResultDto,
  SkinResultDto,
  ThemeMode,
  TurboSettingsDto,
  UiLanguage,
  VanillaEnvStatus,
  VersionChangeDto,
} from "./types";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function makeSampleSkinRgba(): number[] {
  const out = new Array<number>(64 * 64 * 4).fill(0);
  const paintRect = (
    rx: number,
    ry: number,
    rw: number,
    rh: number,
    r: number,
    g: number,
    b: number
  ) => {
    for (let y = ry; y < ry + rh; y += 1) {
      for (let x = rx; x < rx + rw; x += 1) {
        const idx = (y * 64 + x) * 4;
        out[idx] = r;
        out[idx + 1] = g;
        out[idx + 2] = b;
        out[idx + 3] = 255;
      }
    }
  };
  paintRect(0, 0, 32, 16, 214, 165, 126);
  paintRect(8, 8, 8, 8, 214, 165, 126);
  paintRect(8, 8, 8, 2, 54, 42, 34);
  paintRect(9, 11, 2, 1, 28, 36, 48);
  paintRect(13, 11, 2, 1, 28, 36, 48);
  paintRect(11, 13, 2, 1, 174, 120, 92);
  paintRect(16, 16, 24, 16, 46, 70, 94);
  paintRect(20, 20, 8, 2, 62, 92, 120);
  paintRect(40, 16, 16, 16, 68, 102, 130);
  paintRect(44, 28, 4, 4, 206, 156, 118);
  paintRect(32, 48, 16, 16, 68, 102, 130);
  paintRect(36, 60, 4, 4, 206, 156, 118);
  paintRect(0, 16, 16, 16, 48, 54, 64);
  paintRect(16, 48, 16, 16, 42, 48, 58);
  return out;
}

// Isolated browser-preview sample data (only used outside Tauri desktop runtime)
const previewState: BootstrapDto = {
  config: {
    mc_version: "1.21.4",
    username: "Player",
    skin_file: null,
    skin_model: "Auto",
    theme: "Dark",
    ui_accent: "Auto",
    custom_accent_hex: "#e07d44",
    ui_corner: "Balanced",
    ui_layout: "Sidebar",
    ui_compact: false,
    ui_reduced_motion: false,
    ui_sounds: false,
    ui_language: "En",
    discord_rpc: true,
    download_threads: 24,
    network_boost: true,
    process_boost: true,
    mod_loader: "Fabric",
    last_save_id: "Survival (Sample)",
    saved_servers: [
      {
        name: "Local Dev Server",
        address: "127.0.0.1:25565",
      },
    ],
    graphics: {
      preset: "Ultra",
      render_distance: 16,
      simulation_distance: 12,
      fov_degrees: 75,
      vsync: false,
      max_fps: 260,
      mouse_sensitivity: 0.5,
      smooth_lighting: true,
      fancy_graphics: true,
      jvm_ram_mb: 4096,
      jvm_gc: "ZGC",
      show_fps_hud: true,
      auto_configured: true,
    },
  },
  saves: [
    {
      id: "Survival (Sample)",
      modified_epoch_secs: 1_763_120_000,
      size_bytes: 2_450_000,
    },
  ],
  hardware: {
    cpu_name: "Sample Multi-Core Processor",
    physical_cores: 6,
    logical_cores: 12,
    total_ram_mb: 16384,
    available_ram_mb: 9800,
    gpu_name: "Sample Discrete GPU",
    gpu_backend: "Dx12",
    gpu_device_type: "Discrete",
  },
  vanilla: {
    active_version: "1.21.4",
    installed_versions: ["1.21.4"],
    vanilla_dir: "C:\\basalt\\vanilla",
    runs_dir: "C:\\basalt\\run",
    saves_dir: "C:\\basalt\\run\\saves",
    client_ready: true,
    assets_ready: true,
    java_ready: true,
    java_home: "C:\\Program Files\\Eclipse Adoptium\\jdk-21",
    java_version_label: "Java 21.0.11",
    java_hint: "Compatible system Java 21+ runtime detected.",
  },
  loader_status: {
    mcVersion: "1.21.4",
    activeLoader: "Fabric",
    fabricReady: true,
    forgeReady: false,
    fabricLoaderVersion: "0.16.10",
    forgeVersion: null,
    activeModsDir: "C:\\basalt\\vanilla\\loader_mods\\fabric\\1.21.4",
    shaderpacksDir: "C:\\basalt\\run\\shaderpacks",
    resourcepacksDir: "C:\\basalt\\run\\resourcepacks",
  },
  library_counts: {
    modsEnabled: 3,
    modsTotal: 3,
    shadersEnabled: 1,
    shadersTotal: 1,
    resourcepacksEnabled: 1,
    resourcepacksTotal: 1,
  },
  discord_rpc_status: {
    enabled: true,
    connected: false,
    pipe_name: "Standby (WebRTC/IPC)",
    details: "Minecraft 1.21.4 (Vanilla)",
    state_label: "Basalt Launcher · Player",
    started_epoch_secs: 1_763_120_000,
    github_url: "https://github.com/Markesrrr/Basalt",
  },
  skin_rgba: makeSampleSkinRgba(),
  skin_is_slim: true,
  running_session: null,
  hardware_notice: null,
};

async function loadBrowserPreviewDefaultSkinRgba(): Promise<number[] | null> {
  if (typeof Image === "undefined" || typeof document === "undefined") {
    return null;
  }
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      const canvas = document.createElement("canvas");
      canvas.width = 64;
      canvas.height = 64;
      const ctx = canvas.getContext("2d");
      if (!ctx) {
        resolve(null);
        return;
      }
      ctx.drawImage(img, 0, 0, 64, 64);
      const data = ctx.getImageData(0, 0, 64, 64).data;
      resolve(Array.from(data));
    };
    img.onerror = () => resolve(null);
    img.src = "/default_skin.png";
  });
}

export async function getBootstrapState(): Promise<BootstrapDto> {
  if (isTauriRuntime()) {
    return invoke<BootstrapDto>("get_bootstrap_state");
  }
  const realDefault = await loadBrowserPreviewDefaultSkinRgba();
  if (realDefault) {
    previewState.skin_rgba = realDefault;
  }
  const snapshot = structuredClone(previewState);
  if (new URLSearchParams(window.location.search).get("java") === "missing") {
    snapshot.vanilla.java_ready = false;
    snapshot.vanilla.java_home = null;
    snapshot.vanilla.java_version_label = "Java 21 runtime not found";
    snapshot.vanilla.java_hint =
      "No Java 21 runtime was detected on this PC. Click Install Java 21 below to set up a portable runtime without administrator rights.";
  }
  return snapshot;
}

export async function listSaves(): Promise<MinecraftSaveSummary[]> {
  if (isTauriRuntime()) {
    return invoke<MinecraftSaveSummary[]>("list_saves");
  }
  return structuredClone(previewState.saves);
}

export async function selectSave(saveId: string | null): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("select_save", { saveId });
    return;
  }
  previewState.config.last_save_id = saveId;
}

export async function deleteSave(saveId: string): Promise<MinecraftSaveSummary[]> {
  if (isTauriRuntime()) {
    return invoke<MinecraftSaveSummary[]>("delete_save", { saveId });
  }
  previewState.saves = previewState.saves.filter((s) => s.id !== saveId);
  if (previewState.config.last_save_id === saveId) {
    previewState.config.last_save_id = previewState.saves[0]?.id ?? null;
  }
  return structuredClone(previewState.saves);
}

export async function saveProfileSettings(input: {
  username: string;
  theme: ThemeMode;
  ui_accent: AccentTone;
  custom_accent_hex: string | null;
  ui_corner: CornerStyle;
  ui_layout: LayoutMode;
  ui_compact: boolean;
  ui_reduced_motion: boolean;
  ui_sounds: boolean;
  ui_language?: UiLanguage;
  mc_version?: string;
}): Promise<AppConfig> {
  if (isTauriRuntime()) {
    return invoke<AppConfig>("save_profile_settings", { input });
  }
  const clean = input.username.trim();
  if (!clean || !/^[A-Za-z0-9_-]{1,16}$/.test(clean)) {
    throw new Error("Username must be 1-16 ASCII letters, digits, '_' or '-'.");
  }
  previewState.config.username = clean;
  previewState.config.theme = input.theme;
  previewState.config.ui_accent = input.ui_accent;
  previewState.config.custom_accent_hex = input.custom_accent_hex;
  previewState.config.ui_corner = input.ui_corner;
  previewState.config.ui_layout = input.ui_layout;
  previewState.config.ui_compact = input.ui_compact;
  previewState.config.ui_reduced_motion = input.ui_reduced_motion;
  previewState.config.ui_sounds = input.ui_sounds;
  if (input.ui_language) {
    previewState.config.ui_language = input.ui_language;
  }
  if (input.mc_version) {
    previewState.config.mc_version = input.mc_version;
    previewState.vanilla.active_version = input.mc_version;
  }
  return structuredClone(previewState.config);
}

const previewLibraryStore: Record<string, InstalledLibraryItem[]> = {
  "mod:Fabric": [
    {
      fileName: "fabric-api-0.115.0+1.21.4.jar",
      displayName: "fabric-api-0.115.0+1.21.4",
      kind: "mod",
      enabled: true,
      sizeBytes: 2_310_400,
      path: "C:\\basalt\\vanilla\\loader_mods\\fabric\\1.21.4\\fabric-api-0.115.0+1.21.4.jar",
      projectId: "P7dR8mSH",
    },
    {
      fileName: "sodium-fabric-0.6.5+mc1.21.4.jar",
      displayName: "sodium-fabric-0.6.5+mc1.21.4",
      kind: "mod",
      enabled: true,
      sizeBytes: 1_180_920,
      path: "C:\\basalt\\vanilla\\loader_mods\\fabric\\1.21.4\\sodium-fabric-0.6.5+mc1.21.4.jar",
      projectId: "AANobbMI",
    },
    {
      fileName: "iris-fabric-1.8.8+mc1.21.4.jar",
      displayName: "iris-fabric-1.8.8+mc1.21.4",
      kind: "mod",
      enabled: true,
      sizeBytes: 2_695_100,
      path: "C:\\basalt\\vanilla\\loader_mods\\fabric\\1.21.4\\iris-fabric-1.8.8+mc1.21.4.jar",
      projectId: "YL57xq9U",
    },
  ],
  "mod:Forge": [],
  "mod:Vanilla": [],
  shader: [
    {
      fileName: "ComplementaryUnbound_r5.4.zip",
      displayName: "ComplementaryUnbound_r5.4",
      kind: "shader",
      enabled: true,
      sizeBytes: 518_400,
      path: "C:\\basalt\\run\\shaderpacks\\ComplementaryUnbound_r5.4.zip",
      projectId: "R6NEzAwj",
    },
  ],
  resourcePack: [
    {
      fileName: "FreshAnimations_v1.9.2.zip",
      displayName: "FreshAnimations_v1.9.2",
      kind: "resourcePack",
      enabled: true,
      sizeBytes: 1_940_220,
      path: "C:\\basalt\\run\\resourcepacks\\FreshAnimations_v1.9.2.zip",
      projectId: "50dA9Sha",
    },
  ],
};

const PREVIEW_CATALOG: ModrinthSearchHit[] = [
  {
    projectId: "AANobbMI",
    slug: "sodium",
    title: "Sodium",
    description: "Modern rendering engine and client-side optimization mod for Minecraft.",
    author: "jellysquid3",
    iconUrl: "https://cdn.modrinth.com/data/AANobbMI/295862f4724dc3f78df3447ad6072b2dcd3ef0c9_96.webp",
    downloads: 112_400_000,
    follows: 94_200,
    categories: ["optimization", "fabric"],
    projectType: "mod",
    installed: false,
  },
  {
    projectId: "YL57xq9U",
    slug: "iris",
    title: "Iris Shaders",
    description: "A modern shaderpack mod for Minecraft compatible with existing OptiFine shader packs.",
    author: "coderbot",
    iconUrl: "https://cdn.modrinth.com/data/YL57xq9U/18d0e7f076d3d6ed5bedd472b853909aac5da202_96.webp",
    downloads: 68_300_000,
    follows: 52_100,
    categories: ["optimization", "fabric"],
    projectType: "mod",
    installed: false,
  },
  {
    projectId: "gvQqBUqZ",
    slug: "lithium",
    title: "Lithium",
    description: "No-compromises game logic optimization mod for tick rate and memory usage.",
    author: "jellysquid3",
    iconUrl: "https://cdn.modrinth.com/data/gvQqBUqZ/bcc8686c13af0143adf4285d741256af824f70b7_96.webp",
    downloads: 74_500_000,
    follows: 49_800,
    categories: ["optimization", "fabric"],
    projectType: "mod",
    installed: false,
  },
  {
    projectId: "P7dR8mSH",
    slug: "fabric-api",
    title: "Fabric API",
    description: "Essential hooks and shared APIs required by most Fabric mods.",
    author: "modmuss50",
    iconUrl: "https://cdn.modrinth.com/data/P7dR8mSH/icon.png",
    downloads: 190_000_000,
    follows: 82_000,
    categories: ["library", "fabric"],
    projectType: "mod",
    installed: false,
  },
  {
    projectId: "sk9rgfiA",
    slug: "embeddium",
    title: "Embeddium",
    description: "High-performance rendering engine fork bringing Sodium speed cleanly to Forge.",
    author: "embeddedt",
    iconUrl: "https://cdn.modrinth.com/data/sk9rgfiA/55f9c50284f8abbbe2a485abfd6a16209201e451_96.webp",
    downloads: 34_200_000,
    follows: 21_400,
    categories: ["optimization", "forge"],
    projectType: "mod",
    installed: false,
  },
  {
    projectId: "GchcoXML",
    slug: "oculus",
    title: "Oculus",
    description: "Unofficial Iris Shaders port for Forge, enabling full shader packs with Embeddium.",
    author: "NanoLive",
    iconUrl: "https://cdn.modrinth.com/data/GchcoXML/09ef853cdfd3d67dead4faef37102535a2185d5_96.webp",
    downloads: 29_100_000,
    follows: 17_800,
    categories: ["optimization", "forge"],
    projectType: "mod",
    installed: false,
  },
  {
    projectId: "Q1vvjJYV",
    slug: "bsl-shaders",
    title: "BSL Shaders",
    description: "Vibrant lighting, volumetric clouds, customizable water and soft shadows.",
    author: "CaptTatsu",
    iconUrl: "https://cdn.modrinth.com/data/Q1vvjJYV/2a611a3cb434fb52fb81fa5dace13c5d8b67e55d_96.webp",
    downloads: 22_800_000,
    follows: 46_200,
    categories: ["iris", "optifine", "atmosphere"],
    projectType: "shader",
    installed: false,
  },
  {
    projectId: "R6NEzAwj",
    slug: "complementary-unbound",
    title: "Complementary Shaders - Unbound",
    description: "High-detail lighting and water reflections built to preserve Minecraft's identity.",
    author: "EminGT",
    iconUrl: "https://cdn.modrinth.com/data/R6NEzAwj/c85ce4049aac76360d2cd24fd9a7003de01ef312_96.webp",
    downloads: 44_900_000,
    follows: 71_000,
    categories: ["iris", "optifine", "pbr"],
    projectType: "shader",
    installed: false,
  },
  {
    projectId: "izsIPI7a",
    slug: "makeup-ultra-fast-shaders",
    title: "MakeUp - Ultra Fast",
    description: "Lightweight shaderpack delivering crisp shadows and glow on integrated GPUs.",
    author: "XavierFST",
    iconUrl: "https://cdn.modrinth.com/data/izsIPI7a/a08432baa86b8ffd58c08f4b3a001ef976ff764d_96.webp",
    downloads: 16_400_000,
    follows: 28_600,
    categories: ["iris", "optifine", "potato"],
    projectType: "shader",
    installed: false,
  },
  {
    projectId: "50dA9Sha",
    slug: "fresh-animations",
    title: "Fresh Animations",
    description: "Smooth overhaul of vanilla mob animations for Java Edition.",
    author: "FreshLX",
    iconUrl: "https://cdn.modrinth.com/data/50dA9Sha/3132c10e9e3c73fde9799720fd3da5561071708c_96.webp",
    downloads: 29_700_000,
    follows: 59_100,
    categories: ["mobs", "curated"],
    projectType: "resourcepack",
    installed: false,
  },
  {
    projectId: "yBW8D80W",
    slug: "stay-true",
    title: "Stay True",
    description: "Refined foliage, bricks and connected textures faithful to default Minecraft.",
    author: "Haite",
    iconUrl: null,
    downloads: 14_200_000,
    follows: 32_400,
    categories: ["16x", "Curated"],
    projectType: "resourcepack",
    installed: false,
  },
];

function computePreviewCounts(): LibraryCounts {
  const modsKey = `mod:${previewState.config.mod_loader}`;
  const mods = previewLibraryStore[modsKey] ?? [];
  const shaders = previewLibraryStore.shader ?? [];
  const packs = previewLibraryStore.resourcePack ?? [];
  return {
    modsEnabled: mods.filter((m) => m.enabled).length,
    modsTotal: mods.length,
    shadersEnabled: shaders.filter((s) => s.enabled).length,
    shadersTotal: shaders.length,
    resourcepacksEnabled: packs.filter((p) => p.enabled).length,
    resourcepacksTotal: packs.length,
  };
}

function getPreviewStoreKey(kind: LibraryKind, loader: LoaderKind): string {
  return kind === "mod" ? `mod:${loader}` : kind;
}

export async function setMcVersion(version: string): Promise<VersionChangeDto> {
  if (isTauriRuntime()) {
    return invoke<VersionChangeDto>("set_mc_version", { version });
  }
  previewState.config.mc_version = version;
  previewState.vanilla.active_version = version;
  previewState.vanilla.client_ready =
    previewState.vanilla.installed_versions.includes(version);
  previewState.loader_status.mcVersion = version;
  previewState.discord_rpc_status.details = `Minecraft ${version} (${previewState.config.mod_loader})`;
  return {
    config: structuredClone(previewState.config),
    vanilla: structuredClone(previewState.vanilla),
    loader_status: structuredClone(previewState.loader_status),
    library_counts: computePreviewCounts(),
    discord_rpc_status: structuredClone(previewState.discord_rpc_status),
  };
}

export async function setModLoader(loader: LoaderKind): Promise<VersionChangeDto> {
  if (isTauriRuntime()) {
    return invoke<VersionChangeDto>("set_mod_loader", { loader });
  }
  previewState.config.mod_loader = loader;
  previewState.loader_status.activeLoader = loader;
  previewState.discord_rpc_status.details = `Minecraft ${previewState.config.mc_version} (${loader})`;
  previewState.library_counts = computePreviewCounts();
  return {
    config: structuredClone(previewState.config),
    vanilla: structuredClone(previewState.vanilla),
    loader_status: structuredClone(previewState.loader_status),
    library_counts: structuredClone(previewState.library_counts),
    discord_rpc_status: structuredClone(previewState.discord_rpc_status),
  };
}

export async function prepareLoaderEnvironment(loader: LoaderKind): Promise<RunningSessionDto> {
  if (isTauriRuntime()) {
    return invoke<RunningSessionDto>("prepare_loader_environment", { loader });
  }
  if (loader === "Fabric") {
    previewState.loader_status.fabricReady = true;
    previewState.loader_status.fabricLoaderVersion = "0.16.10";
  } else if (loader === "Forge") {
    previewState.loader_status.forgeReady = true;
    previewState.loader_status.forgeVersion = "54.1.22";
  }
  return {
    pid: 0,
    kind: "prepare",
    label: `${loader} ${previewState.config.mc_version} Ready`,
  };
}

export async function searchModrinthCatalog(params: {
  query: string;
  kind: LibraryKind;
  loader: LoaderKind;
  sort: string;
  limit: number;
  category?: string | null;
}): Promise<ModrinthSearchResponse> {
  if (isTauriRuntime()) {
    return invoke<ModrinthSearchResponse>("search_modrinth_catalog", {
      query: params.query,
      kind: params.kind,
      loader: params.loader,
      sort: params.sort,
      limit: params.limit,
      category: params.category ?? null,
    });
  }
  const typeTarget =
    params.kind === "mod" ? "mod" : params.kind === "shader" ? "shader" : "resourcepack";
  const q = params.query.trim().toLowerCase();
  const listKey = getPreviewStoreKey(params.kind, params.loader);
  const installedIds = new Set(
    (previewLibraryStore[listKey] ?? []).map((item) => item.projectId).filter(Boolean)
  );
  const hits = PREVIEW_CATALOG.filter((item) => {
    if (item.projectType !== typeTarget) return false;
    if (params.kind === "mod" && params.loader !== "Vanilla") {
      if (!item.categories.includes(params.loader.toLowerCase())) return false;
    }
    if (q && !item.title.toLowerCase().includes(q) && !item.description.toLowerCase().includes(q)) {
      return false;
    }
    return true;
  })
    .slice(0, params.limit)
    .map((item) => ({ ...item, installed: installedIds.has(item.projectId) }));
  return { hits, totalHits: hits.length };
}

export async function installModrinthItem(params: {
  projectId: string;
  kind: LibraryKind;
  loader: LoaderKind;
}): Promise<InstallActionDto> {
  if (isTauriRuntime()) {
    return invoke<InstallActionDto>("install_modrinth_item", {
      projectId: params.projectId,
      kind: params.kind,
      loader: params.loader,
    });
  }
  const key = getPreviewStoreKey(params.kind, params.loader);
  const list = (previewLibraryStore[key] ??= []);
  const hit = PREVIEW_CATALOG.find((c) => c.projectId === params.projectId || c.slug === params.projectId);
  const ext = params.kind === "mod" ? ".jar" : ".zip";
  const fileName = `${hit?.slug ?? params.projectId}-1.21.4${ext}`;
  if (!list.some((item) => item.projectId === params.projectId || item.fileName === fileName)) {
    list.push({
      fileName,
      displayName: fileName.replace(/\.(jar|zip)$/, ""),
      kind: params.kind,
      enabled: true,
      sizeBytes: 1_048_576,
      path: fileName,
      projectId: hit?.projectId ?? params.projectId,
    });
  }
  previewState.library_counts = computePreviewCounts();
  return {
    config: structuredClone(previewState.config),
    outcome: { installedFiles: [fileName], skippedCount: 0, warnings: [] },
    loaderStatus: structuredClone(previewState.loader_status),
    libraryCounts: structuredClone(previewState.library_counts),
  };
}

export async function installShaderBundle(presetId: "fabric_shaders" | "forge_shaders"): Promise<InstallActionDto> {
  if (isTauriRuntime()) {
    return invoke<InstallActionDto>("install_shader_bundle", { presetId });
  }
  const targetLoader: LoaderKind = presetId === "fabric_shaders" ? "Fabric" : "Forge";
  previewState.config.mod_loader = targetLoader;
  previewState.loader_status.activeLoader = targetLoader;
  const modSlugs =
    presetId === "fabric_shaders"
      ? ["fabric-api", "sodium", "iris", "lithium"]
      : ["embeddium", "oculus"];
  const installedFiles: string[] = [];
  for (const slug of modSlugs) {
    const res = await installModrinthItem({ projectId: slug, kind: "mod", loader: targetLoader });
    installedFiles.push(...res.outcome.installedFiles);
  }
  const shaderRes = await installModrinthItem({
    projectId: "complementary-unbound",
    kind: "shader",
    loader: targetLoader,
  });
  installedFiles.push(...shaderRes.outcome.installedFiles);
  previewState.library_counts = computePreviewCounts();
  return {
    config: structuredClone(previewState.config),
    outcome: { installedFiles, skippedCount: 0, warnings: [] },
    loaderStatus: structuredClone(previewState.loader_status),
    libraryCounts: structuredClone(previewState.library_counts),
  };
}

export async function listLibraryItems(
  kind: LibraryKind,
  loader: LoaderKind
): Promise<InstalledLibraryItem[]> {
  if (isTauriRuntime()) {
    return invoke<InstalledLibraryItem[]>("list_library_items_cmd", { kind, loader });
  }
  return structuredClone(previewLibraryStore[getPreviewStoreKey(kind, loader)] ?? []);
}

export async function toggleLibraryItem(
  kind: LibraryKind,
  loader: LoaderKind,
  fileName: string
): Promise<LibraryCounts> {
  if (isTauriRuntime()) {
    return invoke<LibraryCounts>("toggle_library_item_cmd", { kind, loader, fileName });
  }
  const list = previewLibraryStore[getPreviewStoreKey(kind, loader)] ?? [];
  const item = list.find((it) => it.fileName === fileName);
  if (item) {
    item.enabled = !item.enabled;
    item.fileName = item.enabled
      ? item.fileName.replace(/\.disabled$/, "")
      : `${item.fileName}.disabled`;
  }
  previewState.library_counts = computePreviewCounts();
  return structuredClone(previewState.library_counts);
}

export async function deleteLibraryItem(
  kind: LibraryKind,
  loader: LoaderKind,
  fileName: string
): Promise<LibraryCounts> {
  if (isTauriRuntime()) {
    return invoke<LibraryCounts>("delete_library_item_cmd", { kind, loader, fileName });
  }
  const key = getPreviewStoreKey(kind, loader);
  previewLibraryStore[key] = (previewLibraryStore[key] ?? []).filter(
    (it) => it.fileName !== fileName
  );
  previewState.library_counts = computePreviewCounts();
  return structuredClone(previewState.library_counts);
}

export async function pickAndImportLibraryFile(
  kind: LibraryKind,
  loader: LoaderKind
): Promise<ImportLibraryDto | null> {
  if (isTauriRuntime()) {
    return invoke<ImportLibraryDto | null>("pick_and_import_library_file", { kind, loader });
  }
  throw new Error("Local file import picker is available in the desktop app (basalt.exe).");
}

export async function openLibraryFolder(kind: LibraryKind, loader: LoaderKind): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("open_library_folder", { kind, loader });
  }
}

export async function saveTurboSettings(input: {
  discord_rpc: boolean;
  download_threads: number;
  network_boost: boolean;
  process_boost: boolean;
}): Promise<TurboSettingsDto> {
  if (isTauriRuntime()) {
    return invoke<TurboSettingsDto>("save_turbo_settings", { input });
  }
  previewState.config.discord_rpc = input.discord_rpc;
  previewState.config.download_threads = input.download_threads;
  previewState.config.network_boost = input.network_boost;
  previewState.config.process_boost = input.process_boost;
  previewState.discord_rpc_status.enabled = input.discord_rpc;
  return {
    config: structuredClone(previewState.config),
    discord_rpc_status: structuredClone(previewState.discord_rpc_status),
  };
}

export async function optimizeClientNow(): Promise<ClientOptimizationReport> {
  if (isTauriRuntime()) {
    return invoke<ClientOptimizationReport>("optimize_client_now");
  }
  previewState.config.network_boost = true;
  previewState.config.process_boost = true;
  return {
    cdn_ping_ms: 28,
    local_ping_ms: null,
    freed_bytes: 0,
    dns_flushed: true,
    applied_flags_count: 9,
    active_flags: [
      "-Djava.net.preferIPv4Stack=true",
      "-Dio.netty.allocator.type=pooled",
      "-Dio.netty.recycler.maxCapacityPerThread=4096",
      "-Dsun.net.inetaddr.ttl=30",
      "-XX:+AlwaysPreTouch",
      "-XX:+DisableExplicitGC",
      "-XX:+ParallelRefProcEnabled",
      "-XX:MaxInlineLevel=15",
      "Win32:ABOVE_NORMAL_PRIORITY",
    ],
  };
}

export async function openGithubPage(): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("open_github_page");
    return;
  }
  window.open("https://github.com/Markesrrr/Basalt", "_blank", "noopener");
}

export async function openGameFolder(): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("open_game_folder");
    return;
  }
}

export async function saveGraphicsSettings(
  graphics: GraphicsSettings
): Promise<AppConfig> {
  if (isTauriRuntime()) {
    return invoke<AppConfig>("save_graphics_settings", { graphics });
  }
  previewState.config.graphics = structuredClone(graphics);
  return structuredClone(previewState.config);
}

export async function autoOptimizeGraphics(): Promise<OptimizeResultDto> {
  if (isTauriRuntime()) {
    return invoke<OptimizeResultDto>("auto_optimize_graphics");
  }
  const gfx: GraphicsSettings = {
    ...previewState.config.graphics,
    preset: "Ultra",
    render_distance: 16,
    simulation_distance: 12,
    jvm_ram_mb: 4096,
    jvm_gc: "ZGC",
    vsync: false,
    max_fps: 260,
    auto_configured: true,
  };
  previewState.config.graphics = gfx;
  return {
    graphics: gfx,
    reason: "Sample profile: 16 chunks render distance, 4096 MB JVM memory, and ZGC selected (preview).",
  };
}

export async function pickAndImportSkin(): Promise<SkinResultDto | null> {
  if (isTauriRuntime()) {
    return invoke<SkinResultDto | null>("pick_and_import_skin");
  }
  throw new Error("Local file picker only runs in the desktop application (basalt.exe).");
}

export async function resetSkin(): Promise<SkinResultDto> {
  if (isTauriRuntime()) {
    return invoke<SkinResultDto>("reset_skin");
  }
  previewState.config.skin_file = null;
  previewState.config.skin_model = "Auto";
  previewState.skin_is_slim = true;
  previewState.skin_rgba = (await loadBrowserPreviewDefaultSkinRgba()) ?? makeSampleSkinRgba();
  return {
    rgba: previewState.skin_rgba,
    path: null,
    skin_model: "Auto",
    skin_is_slim: true,
    note: "Restored default Basalt Slim skin (browser preview).",
  };
}

export async function saveSkinModel(skinModel: SkinModel): Promise<SkinModelResultDto> {
  if (isTauriRuntime()) {
    return invoke<SkinModelResultDto>("save_skin_model", { skinModel });
  }
  previewState.config.skin_model = skinModel;
  const resolvedSlim =
    skinModel === "Slim" ? true : skinModel === "Classic" ? false : true;
  previewState.skin_is_slim = resolvedSlim;
  return {
    skin_model: skinModel,
    skin_is_slim: resolvedSlim,
  };
}

export async function addServer(
  name: string,
  address: string
): Promise<SavedServer[]> {
  if (isTauriRuntime()) {
    return invoke<SavedServer[]>("add_server", { name, address });
  }
  const addr = address.trim();
  if (!addr) throw new Error("Server address cannot be empty.");
  previewState.config.saved_servers.push({
    name: name.trim() || "Local Server",
    address: addr,
  });
  return structuredClone(previewState.config.saved_servers);
}

export async function removeServer(index: number): Promise<SavedServer[]> {
  if (isTauriRuntime()) {
    return invoke<SavedServer[]>("remove_server", { index });
  }
  previewState.config.saved_servers.splice(index, 1);
  return structuredClone(previewState.config.saved_servers);
}

export async function pingServerCmd(address: string): Promise<ServerPingInfo> {
  if (isTauriRuntime()) {
    return invoke<ServerPingInfo>("ping_server_cmd", { address });
  }
  throw new Error(
    `Local TCP SLP query is unavailable in browser preview (${address}). Use the desktop app.`
  );
}

export async function probeLocalServerCmd(
  address: string,
  username: string
): Promise<LocalHandshakeResult> {
  if (isTauriRuntime()) {
    return invoke<LocalHandshakeResult>("probe_local_server_cmd", {
      address,
      username,
    });
  }
  throw new Error(
    `Local TCP handshake test is unavailable in browser preview (${address}). Use the desktop app.`
  );
}

export async function prepareVanillaClient(): Promise<RunningSessionDto> {
  if (isTauriRuntime()) {
    return invoke<RunningSessionDto>("prepare_vanilla_client");
  }
  throw new Error(
    "Vanilla 1.21.4 file preparation can only run in the desktop application (basalt.exe)."
  );
}

export async function installJava21(): Promise<RunningSessionDto> {
  if (isTauriRuntime()) {
    return invoke<RunningSessionDto>("install_java_21");
  }
  throw new Error(
    "Automatic Java 21 installer can only run in the desktop application (basalt.exe)."
  );
}

export async function recheckJava21(): Promise<VanillaEnvStatus> {
  if (isTauriRuntime()) {
    return invoke<VanillaEnvStatus>("recheck_java_21");
  }
  return structuredClone(previewState.vanilla);
}

export async function openJavaDownloadPage(): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("open_java_download_page");
    return;
  }
  window.open(
    "https://adoptium.net/temurin/releases/?version=21&os=windows&arch=x64&package=jdk",
    "_blank",
    "noopener"
  );
}

export async function setFullscreen(fullscreen: boolean): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("set_fullscreen", { fullscreen });
    return;
  }
  if (typeof document === "undefined") return;
  try {
    if (fullscreen && !document.fullscreenElement) {
      await document.documentElement.requestFullscreen?.();
    } else if (!fullscreen && document.fullscreenElement) {
      await document.exitFullscreen?.();
    }
  } catch {
    // Headless or browser gesture constraint
  }
}

export async function windowMinimize(): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("window_minimize");
  }
}

export async function windowToggleMaximize(): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("window_toggle_maximize");
  }
  if (typeof document === "undefined") return false;
  try {
    if (document.fullscreenElement) {
      await document.exitFullscreen?.();
      return false;
    }
    await document.documentElement.requestFullscreen?.();
    return Boolean(document.fullscreenElement);
  } catch {
    return false;
  }
}

export async function windowIsMaximized(): Promise<boolean> {
  if (isTauriRuntime()) {
    return invoke<boolean>("window_is_maximized");
  }
  return Boolean(typeof document !== "undefined" && document.fullscreenElement);
}

export async function windowClose(): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("window_close");
    return;
  }
  window.close();
}

export async function windowStartDrag(): Promise<void> {
  if (isTauriRuntime()) {
    await invoke<void>("window_start_drag");
  }
}

export async function launchClient(input: {
  mode: LaunchMode;
  save_id: string | null;
  server_address: string | null;
  mc_version?: string;
}): Promise<RunningSessionDto> {
  if (isTauriRuntime()) {
    return invoke<RunningSessionDto>("launch_client", { input });
  }
  throw new Error(
    "Minecraft Vanilla client can only be launched from the desktop application (basalt.exe)."
  );
}

export async function onLaunchLog(
  cb: (payload: LaunchLogPayload) => void
): Promise<() => void> {
  if (isTauriRuntime()) {
    return listen<LaunchLogPayload>("basalt://launch-log", (event) => {
      cb(event.payload);
    });
  }
  return () => {};
}

export async function loadMinecraftSounds(): Promise<MinecraftSoundsDto> {
  if (isTauriRuntime()) {
    return invoke<MinecraftSoundsDto>("load_minecraft_sounds");
  }
  return { ready_count: 0, total_count: 8, missing_keys: [], sounds: {} };
}

export async function onMinecraftSoundsReady(
  cb: () => void
): Promise<() => void> {
  if (isTauriRuntime()) {
    return listen<number>("basalt://minecraft-sounds-ready", () => {
      cb();
    });
  }
  return () => {};
}

export async function onSessionEnded(
  cb: (payload: SessionEndedPayload) => void
): Promise<() => void> {
  if (isTauriRuntime()) {
    return listen<SessionEndedPayload>("basalt://session-ended", (event) => {
      cb(event.payload);
    });
  }
  return () => {};
}
