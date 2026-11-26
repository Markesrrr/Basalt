import { applyDynamicDesignTokens, normalizeHex } from "./colors";
import { initCommandPalette, type PaletteCommand } from "./command_palette";
import {
  compareLocaleStrings,
  formatLocaleBytes,
  formatLocaleEpoch,
  formatLocaleNumber,
  toLocaleLowerFold,
  toLocaleUpperSafe,
} from "./formatting";
import {
  ACCENT_LABELS,
  CORNER_LABELS,
  GPU_TIER_LABELS,
  LANG_INTL_LOCALE,
  LAYOUT_SHORT_LABELS,
  THEME_SHORT_LABELS,
  UI_LANGUAGE_ORDER,
  applyStaticI18n,
  tr,
} from "./i18n";
import * as ipc from "./ipc";
import {
  animateDialogOpen,
  animateLightboxOpen,
  animatePanelReveal,
  animateTextSwap,
  animateToastIn,
  animateViewHandoff,
  cancelElementAnimation,
  resolveEffectiveMotion,
  syncRootMotionAttribute,
  syncSlidingIndicators,
  watchSystemMotion,
  wireDetailsAccordion,
  wireRovingTabGroup,
  wireSlidingGroup,
  type EffectiveMotion,
} from "./motion";
import {
  drawSkinPreview,
  drawTopbarHead,
  type SkinViewMode,
} from "./skin_canvas";
import { Skin3DViewer } from "./skin3d";
import { initMinecraftSounds, playMcSound, setSoundsEnabled, unlockAudioContext } from "./sounds";
import { pulseStrata, renderStrata, seedHue } from "./stage_art";
import {
  type AccentTone,
  type BootstrapDto,
  type CornerStyle,
  type CrashDiagnosis,
  type GraphicsSettings,
  type InstalledLibraryItem,
  type LaunchMode,
  type LayoutMode,
  type LibraryKind,
  type LoaderKind,
  type MinecraftSaveSummary,
  type ModrinthSearchHit,
  type ModUpdateInfo,
  type SavedServer,
  type ScreenshotItem,
  type ServerPingInfo,
  type SkinModel,
  type ThemeMode,
  type UiLanguage,
  type VanillaEnvStatus,
  type WorldBackupSummary,
} from "./types";

type ViewId = "play" | "library" | "worlds" | "profile" | "system";
const VIEW_ORDER: readonly ViewId[] = ["play", "library", "worlds", "profile", "system"];

let libActiveTab: LibraryKind | "installed" = "mod";
let libTargetLoader: LoaderKind = "Fabric";
let libInstalledKind: LibraryKind = "mod";
let libCatalogHits: ModrinthSearchHit[] = [];
let libTotalHits = 0;
let libInstalledItems: InstalledLibraryItem[] = [];
let libCatalogLoading = false;
let libCatalogError: string | null = null;
let libLastCatalogKey = "";
let libCatalogSeq = 0;
let libInstalledSeq = 0;
let loaderSwitchSeq = 0;
let versionSwitchSeq = 0;
let libImportBusy = false;
const installingProjects = new Set<string>();
const installedUpdates = new Map<string, ModUpdateInfo>();
let libUpdatesChecking = false;
let libUpdatesCheckedScope = "";
let libUpdatingAll = false;
const updatingModFiles = new Set<string>();

let worldsActiveTab: "saves" | "screenshots" = "saves";
const expandedWorldBackups = new Set<string>();
const worldBackupsCache = new Map<string, WorldBackupSummary[]>();
const worldBackupsLoading = new Set<string>();
const worldBackupBusy = new Set<string>();
let pendingRestoreBackup: WorldBackupSummary | null = null;
let lastRestoreDialogTrigger: HTMLElement | null = null;

let screenshotItems: ScreenshotItem[] = [];
let screenshotsLoaded = false;
const screenshotUrlCache = new Map<string, string>();
const screenshotUrlLoading = new Set<string>();
let screenshotObserver: IntersectionObserver | null = null;
let lightboxIndex = -1;
let lightboxTriggerEl: HTMLElement | null = null;

let lastCrashDiagnosis: CrashDiagnosis | null = null;
const serverPingInFlight = new Set<string>();
let initialServersAutoPinged = false;

interface ServerCheckState {
  status: "idle" | "checking" | "online" | "error";
  ping?: ServerPingInfo;
  message?: string;
}

const VALID_USERNAME_RE = /^[A-Za-z0-9_-]{1,16}$/;
const MAX_LOG_LINES = 400;

const THEME_ORDER: readonly ThemeMode[] = [
  "Dark",
  "Ember",
  "Light",
  "Spruce",
  "Cobalt",
  "Nether",
  "Frost",
  "Sculk",
  "End",
  "Cherry",
];
const THEME_ATTRS: Record<ThemeMode, string> = {
  Dark: "dark",
  Ember: "ember",
  Light: "light",
  Spruce: "spruce",
  Cobalt: "cobalt",
  Nether: "nether",
  Frost: "frost",
  Sculk: "sculk",
  End: "end",
  Cherry: "cherry",
};

const ACCENT_ORDER: readonly AccentTone[] = [
  "Auto",
  "Copper",
  "Emerald",
  "Cyan",
  "Amber",
  "Crimson",
  "Amethyst",
  "Custom",
];
const CORNER_ORDER: readonly CornerStyle[] = ["Sharp", "Balanced", "Soft"];

const DESKTOP_LAYOUT_ORDER: readonly LayoutMode[] = [
  "Sidebar",
  "Topbar",
  "CompactDeck",
  "RightRail",
  "MinimalRail",
];
const LAYOUT_ORDER: readonly LayoutMode[] = [
  ...DESKTOP_LAYOUT_ORDER,
  "DeckMode",
];
const LAYOUT_ATTRS: Record<LayoutMode, string> = {
  Sidebar: "sidebar",
  Topbar: "topbar",
  CompactDeck: "compact-deck",
  RightRail: "right-rail",
  MinimalRail: "minimal-rail",
  DeckMode: "deck",
};

type DeckCardKind = "launch" | "world" | "server" | "nav";

interface DeckCard {
  id: string;
  kind: DeckCardKind;
  kicker: string;
  title: string;
  meta: string;
  sub: string;
  tag: string;
  seedKey: string;
  primaryLabel: string;
  secondaryLabel: string;
  secondaryDisabled?: boolean;
  launchMode?: LaunchMode;
  saveId?: string | null;
  serverAddress?: string | null;
  targetView?: ViewId;
}

let state: BootstrapDto | null = null;
let skinViewMode: SkinViewMode = "front";
let pendingDeleteSaveId: string | null = null;
let lastExpandedRailLayout: LayoutMode = "Sidebar";
let playSkin3d: Skin3DViewer | null = null;
let profileSkin3d: Skin3DViewer | null = null;
let deckSkin3d: Skin3DViewer | null = null;
let lastDialogTrigger: HTMLElement | null = null;
let toastTimer: number | null = null;
let logLines: string[] = [];
const serverChecks = new Map<string, ServerCheckState>();

let consoleModeActive = false;
let consoleAudioMuted = false;
let lastDesktopLayout: LayoutMode = "Sidebar";
let prevAppliedTheme: ThemeMode | null = null;
let prevAppliedLayout: LayoutMode | null = null;
let deckCards: DeckCard[] = [];
let selectedDeckIdx = 0;
let deckClockTimer: number | null = null;

function byId<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) {
    throw new Error(`Missing UI element: #${id}`);
  }
  return node as T;
}

function getEffectiveLanguage(): UiLanguage {
  const raw = new URLSearchParams(window.location.search).get("lang")?.toLowerCase();
  if (raw === "tr") return "Tr";
  if (raw === "de") return "De";
  if (raw === "en") return "En";
  const cfg = state?.config.ui_language;
  return cfg === "Tr" || cfg === "De" || cfg === "En" ? cfg : "En";
}

let pendingLaunch = false;
let pendingSaveProfile = false;
let pendingSaveGfx = false;
let worldsFilterQuery = "";
let worldsSortOrder: "recent" | "name" | "size" = "recent";

function getEffectiveMotion(): EffectiveMotion {
  return resolveEffectiveMotion(Boolean(state?.config.ui_reduced_motion));
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return tr(getEffectiveLanguage(), "err.unexpected");
}

function showToast(message: string, tone: "neutral" | "success" | "error" = "neutral"): void {
  const toast = byId<HTMLDivElement>("toast");
  toast.textContent = message;
  if (tone === "neutral") {
    toast.removeAttribute("data-tone");
  } else {
    toast.setAttribute("data-tone", tone);
  }
  toast.hidden = false;
  animateToastIn(toast, getEffectiveMotion(), tone);

  if (toastTimer !== null) {
    window.clearTimeout(toastTimer);
  }
  const durationMs = tone === "error" ? 5600 : 4200;
  toastTimer = window.setTimeout(() => {
    cancelElementAnimation(toast);
    toast.hidden = true;
    toastTimer = null;
  }, durationMs);
}

function setBanner(
  element: HTMLElement,
  message: string | null,
  tone: "neutral" | "success" | "error" = "neutral"
): void {
  if (!message) {
    element.hidden = true;
    element.textContent = "";
    element.removeAttribute("data-tone");
    return;
  }
  element.textContent = message;
  if (tone === "neutral") {
    element.removeAttribute("data-tone");
  } else {
    element.setAttribute("data-tone", tone);
  }
  element.hidden = false;
}

function formatBytes(bytes: number): string {
  return formatLocaleBytes(bytes, getEffectiveLanguage());
}

function formatEpoch(secs: number): string {
  const lang = getEffectiveLanguage();
  return formatLocaleEpoch(secs, lang, tr(lang, "misc.unknownDate"));
}

function validateUsername(raw: string): string | null {
  const lang = getEffectiveLanguage();
  const trimmed = raw.trim();
  if (!trimmed) {
    return tr(lang, "val.usernameEmpty");
  }
  if (!VALID_USERNAME_RE.test(trimmed)) {
    return tr(lang, "val.usernameChars");
  }
  return null;
}

function md5Bytes(input: Uint8Array): Uint8Array {
  const s = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
  ];
  const K = new Uint32Array(64);
  for (let i = 0; i < 64; i += 1) {
    K[i] = Math.floor(Math.abs(Math.sin(i + 1)) * 0x100000000) >>> 0;
  }

  const origLen = input.length;
  const paddedLen = ((origLen + 8) >> 6) * 64 + 64;
  const msg = new Uint8Array(paddedLen);
  msg.set(input);
  msg[origLen] = 0x80;
  const view = new DataView(msg.buffer);
  const bitLenLo = (origLen * 8) >>> 0;
  const bitLenHi = Math.floor((origLen * 8) / 0x100000000) >>> 0;
  view.setUint32(paddedLen - 8, bitLenLo, true);
  view.setUint32(paddedLen - 4, bitLenHi, true);

  let a0 = 0x67452301;
  let b0 = 0xefcdab89;
  let c0 = 0x98badcfe;
  let d0 = 0x10325476;

  const M = new Uint32Array(16);
  for (let offset = 0; offset < paddedLen; offset += 64) {
    for (let j = 0; j < 16; j += 1) {
      M[j] = view.getUint32(offset + j * 4, true);
    }
    let A = a0;
    let B = b0;
    let C = c0;
    let D = d0;
    for (let i = 0; i < 64; i += 1) {
      let F = 0;
      let g = 0;
      if (i < 16) {
        F = (B & C) | (~B & D);
        g = i;
      } else if (i < 32) {
        F = (D & B) | (~D & C);
        g = (5 * i + 1) % 16;
      } else if (i < 48) {
        F = B ^ C ^ D;
        g = (3 * i + 5) % 16;
      } else {
        F = C ^ (B | ~D);
        g = (7 * i) % 16;
      }
      const sum = (F + A + K[i] + M[g]) >>> 0;
      A = D;
      D = C;
      C = B;
      const rot = s[i];
      B = (B + ((sum << rot) | (sum >>> (32 - rot)))) >>> 0;
    }
    a0 = (a0 + A) >>> 0;
    b0 = (b0 + B) >>> 0;
    c0 = (c0 + C) >>> 0;
    d0 = (d0 + D) >>> 0;
  }

  const digest = new Uint8Array(16);
  const outView = new DataView(digest.buffer);
  outView.setUint32(0, a0, true);
  outView.setUint32(4, b0, true);
  outView.setUint32(8, c0, true);
  outView.setUint32(12, d0, true);
  return digest;
}

function computeOfflineUuidPreview(username: string): string {
  const input = `OfflinePlayer:${username.trim() || "Player"}`;
  const bytes = md5Bytes(new TextEncoder().encode(input));
  // RFC 4122 version 3 (MD5) & IETF variant
  bytes[6] = (bytes[6] & 0x0f) | 0x30;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
}

function formatPlaytime(secs: number, lang: UiLanguage): string {
  if (!Number.isFinite(secs) || secs <= 0) return "";
  if (secs < 60) return tr(lang, "playtime.lessThanMin");
  const totalMins = Math.floor(secs / 60);
  const h = Math.floor(totalMins / 60);
  const m = totalMins % 60;
  if (h > 0) {
    return tr(lang, "playtime.hoursMins", {
      h: formatLocaleNumber(h, lang, 0),
      m: formatLocaleNumber(m, lang, 0),
    });
  }
  return tr(lang, "playtime.minsOnly", { m: formatLocaleNumber(m, lang, 0) });
}

function shortJavaLabel(vanilla: VanillaEnvStatus, lang: UiLanguage): string {
  if (!vanilla.java_ready) return tr(lang, "play.specJdkMissing");
  const m = /Java\s+(\d+)/i.exec(vanilla.java_version_label || "");
  if (m) return `Java ${m[1]}`;
  return vanilla.java_version_label || "Java 21";
}

function validateServerAddress(raw: string): string | null {
  const lang = getEffectiveLanguage();
  const trimmed = raw.trim();
  if (!trimmed) return tr(lang, "servers.addrEmpty");
  if (/\s/.test(trimmed) || /[\u0000-\u001f]/.test(trimmed)) {
    return tr(lang, "servers.addrInvalid");
  }

  let host = trimmed;
  let portStr: string | null = null;
  if (trimmed.startsWith("[")) {
    const closeIdx = trimmed.indexOf("]");
    if (closeIdx < 2) return tr(lang, "servers.addrInvalid");
    host = trimmed.slice(1, closeIdx);
    const rest = trimmed.slice(closeIdx + 1);
    if (rest.length > 0) {
      if (!rest.startsWith(":")) return tr(lang, "servers.addrInvalid");
      portStr = rest.slice(1);
    }
    if (!/^[0-9a-fA-F:]+$/.test(host) || !host.includes(":")) {
      return tr(lang, "servers.addrInvalid");
    }
  } else {
    const firstColon = trimmed.indexOf(":");
    const lastColon = trimmed.lastIndexOf(":");
    if (firstColon !== lastColon) {
      return tr(lang, "servers.addrInvalid");
    }
    if (firstColon >= 0) {
      host = trimmed.slice(0, firstColon);
      portStr = trimmed.slice(firstColon + 1);
    }
    if (!host || host.length > 253) return tr(lang, "servers.addrInvalid");
    const isIpv4 = /^(\d{1,3}\.){3}\d{1,3}$/.test(host);
    if (isIpv4) {
      const octets = host.split(".").map(Number);
      if (octets.some((o) => !Number.isInteger(o) || o < 0 || o > 255)) {
        return tr(lang, "servers.addrInvalid");
      }
    } else if (
      !/^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*$/.test(host)
    ) {
      return tr(lang, "servers.addrInvalid");
    }
  }

  if (portStr !== null) {
    if (!/^\d{1,5}$/.test(portStr)) return tr(lang, "servers.addrInvalid");
    const portNum = Number(portStr);
    if (!Number.isInteger(portNum) || portNum < 1 || portNum > 65535) {
      return tr(lang, "servers.addrInvalid");
    }
  }
  return null;
}

function sanitizePathForDiag(p: string): string {
  return p
    .replace(/([A-Za-z]:\\Users\\)[^\\]+/gi, "$1<USER>")
    .replace(/(\/home\/)[^/]+/g, "$1<USER>")
    .replace(/(\/Users\/)[^/]+/g, "$1<USER>");
}

function buildLaunchDiagnosticsText(): string {
  if (!state) return "Basalt Launcher Diagnostics (no state loaded)";
  const { config, hardware, vanilla, loader_status, library_counts, playtime_stats } = state;
  const loaderVer =
    config.mod_loader === "Fabric"
      ? loader_status?.fabricLoaderVersion ?? "unprepared"
      : config.mod_loader === "Forge"
      ? loader_status?.forgeVersion ?? "unprepared"
      : "none";
  const lines: string[] = [
    "=== Basalt Launcher Diagnostics ===",
    `Timestamp: ${new Date().toISOString()}`,
    `Minecraft: ${config.mc_version} | Loader: ${config.mod_loader} (${loaderVer})`,
    `Java: ${vanilla.java_ready ? vanilla.java_version_label : "MISSING"}`,
    `JVM: -Xmx${config.graphics.jvm_ram_mb}M | GC: ${config.graphics.jvm_gc} | NetBoost: ${config.network_boost} | ProcBoost: ${config.process_boost}`,
    `Graphics: Preset=${config.graphics.preset}, Render=${config.graphics.render_distance}, Sim=${config.graphics.simulation_distance}, FOV=${config.graphics.fov_degrees}, VSync=${config.graphics.vsync}, MaxFPS=${config.graphics.max_fps}`,
    `Hardware: ${hardware.cpu_name} (${hardware.physical_cores}C/${hardware.logical_cores}T) | RAM ${hardware.available_ram_mb}/${hardware.total_ram_mb} MB`,
    `GPU: ${hardware.gpu_name} (${hardware.gpu_backend}, ${hardware.gpu_device_type})`,
    `Library: Mods ${library_counts?.modsEnabled ?? 0}/${library_counts?.modsTotal ?? 0} | Shaders ${library_counts?.shadersEnabled ?? 0}/${library_counts?.shadersTotal ?? 0} | Packs ${library_counts?.resourcepacksEnabled ?? 0}/${library_counts?.resourcepacksTotal ?? 0}`,
    `Saves: ${state.saves.length} | Servers: ${config.saved_servers.length} | Playtime: ${playtime_stats?.total_playtime_secs ?? 0}s (${playtime_stats?.total_sessions ?? 0} sessions)`,
  ];
  if (lastCrashDiagnosis) {
    lines.push(
      "--- Latest Crash Doctor Diagnosis ---",
      `Code: ${lastCrashDiagnosis.code} (${lastCrashDiagnosis.severity})`,
      ...(lastCrashDiagnosis.offending_mod ? [`Suspected Mod: ${lastCrashDiagnosis.offending_mod}`] : []),
      `Details: ${sanitizePathForDiag(lastCrashDiagnosis.details)}`,
      ...(lastCrashDiagnosis.log_excerpt.length > 0
        ? ["Excerpt:", ...lastCrashDiagnosis.log_excerpt.slice(0, 6).map((l) => `  ${sanitizePathForDiag(l)}`)]
        : [])
    );
  }
  return lines.join("\n");
}

async function handleCopyDiagnostics(): Promise<void> {
  const lang = getEffectiveLanguage();
  const text = buildLaunchDiagnosticsText();
  try {
    if (navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
      await navigator.clipboard.writeText(text);
    } else {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
    }
    playMcSound("click");
    showToast(tr(lang, "diag.copiedToast"), "success");
  } catch {
    playMcSound("error");
    showToast(tr(lang, "diag.copyFailedToast"), "error");
  }
}

function setSelectNumericValue(select: HTMLSelectElement, targetValue: number, unit: "MB" | "FPS" = "MB"): void {
  const lang = getEffectiveLanguage();
  const strVal = String(targetValue);
  for (const existing of Array.from(select.querySelectorAll('option[data-synthetic="true"]'))) {
    existing.remove();
  }
  let exists = false;
  for (const option of Array.from(select.options)) {
    if (option.value === strVal) {
      exists = true;
      break;
    }
  }
  if (!exists) {
    const opt = document.createElement("option");
    opt.value = strVal;
    opt.dataset.synthetic = "true";
    if (unit === "FPS") {
      opt.textContent = targetValue >= 260 ? tr(lang, "system.fpsUnlimited") : `${targetValue} FPS`;
    } else {
      opt.textContent = `${targetValue} MB (${tr(lang, "system.ramRecommended", { mb: targetValue }).replace(`${targetValue} MB `, "").replace(/^\(|\)$/g, "")})`;
    }
    select.appendChild(opt);
  }
  select.value = strVal;
}

function getEffectiveTheme(): ThemeMode {
  const queryTheme = new URLSearchParams(window.location.search).get("theme")?.toLowerCase();
  const matched = THEME_ORDER.find((t) => t.toLowerCase() === queryTheme);
  return matched ?? state?.config.theme ?? "Dark";
}

function getEffectiveAccent(): AccentTone {
  const raw = new URLSearchParams(window.location.search).get("accent")?.toLowerCase();
  const matched = ACCENT_ORDER.find((item) => item.toLowerCase() === raw);
  return matched ?? state?.config.ui_accent ?? "Auto";
}

function getEffectiveCustomHex(): string {
  const raw = new URLSearchParams(window.location.search).get("customHex");
  return normalizeHex(raw) ?? normalizeHex(state?.config.custom_accent_hex) ?? "#e07d44";
}

function getEffectiveCorner(): CornerStyle {
  const raw = new URLSearchParams(window.location.search).get("corner")?.toLowerCase();
  const matched = CORNER_ORDER.find((item) => item.toLowerCase() === raw);
  return matched ?? state?.config.ui_corner ?? "Balanced";
}

function getEffectiveLayout(): LayoutMode {
  const queryLayout = new URLSearchParams(window.location.search).get("layout")?.toLowerCase();
  if (queryLayout === "topbar") return "Topbar";
  if (queryLayout === "compact-deck" || queryLayout === "compact") return "CompactDeck";
  if (queryLayout === "right-rail" || queryLayout === "right") return "RightRail";
  if (queryLayout === "minimal-rail" || queryLayout === "minimal" || queryLayout === "dock") return "MinimalRail";
  if (queryLayout === "deck" || queryLayout === "deck-mode" || queryLayout === "steamdeck") return "DeckMode";
  if (queryLayout === "sidebar") return "Sidebar";
  if (consoleModeActive) return "DeckMode";
  const cfgLayout = state?.config.ui_layout ?? "Sidebar";
  return cfgLayout === "DeckMode" ? lastDesktopLayout : cfgLayout;
}

function getEffectiveSounds(): boolean {
  const raw = new URLSearchParams(window.location.search).get("sounds")?.toLowerCase();
  if (raw === "0" || raw === "false") return false;
  if (raw === "1" || raw === "true") return true;
  if (getEffectiveLayout() === "DeckMode") {
    return !consoleAudioMuted;
  }
  return state?.config.ui_sounds ?? false;
}

function updateDeckClock(): void {
  const el = document.getElementById("deck-clock");
  if (!el) return;
  const now = new Date();
  el.textContent = now.toLocaleTimeString(LANG_INTL_LOCALE[getEffectiveLanguage()], {
    hour: "2-digit",
    minute: "2-digit",
  });
}

function syncDeckClock(active: boolean): void {
  if (deckClockTimer !== null) {
    window.clearTimeout(deckClockTimer);
    deckClockTimer = null;
  }
  if (active && !document.hidden) {
    updateDeckClock();
    const now = new Date();
    const msUntilNextMinute = Math.max(1000, (60 - now.getSeconds()) * 1000 - now.getMilliseconds());
    deckClockTimer = window.setTimeout(() => {
      deckClockTimer = null;
      if (getEffectiveLayout() === "DeckMode" && !document.hidden) {
        syncDeckClock(true);
      }
    }, msUntilNextMinute);
  }
}

function currentActiveViewId(): ViewId {
  const activeBtn = document.querySelector<HTMLButtonElement>(".nav-btn.active[data-nav]");
  const nav = activeBtn?.dataset.nav as ViewId | undefined;
  return nav && VIEW_ORDER.includes(nav) ? nav : "play";
}

function applyPreferences(): void {
  const lang = getEffectiveLanguage();
  applyStaticI18n(lang);
  syncRootMotionAttribute(Boolean(state?.config.ui_reduced_motion));

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-lang-choice]"))) {
    const active = btn.dataset.langChoice === lang;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-pressed", active ? "true" : "false");
  }
  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-corner-choice]"))) {
    const key = btn.dataset.cornerChoice as CornerStyle | undefined;
    if (key) btn.textContent = CORNER_LABELS[lang][key];
  }
  for (const span of Array.from(document.querySelectorAll<HTMLElement>("[data-accent-label]"))) {
    const key = span.dataset.accentLabel as AccentTone | undefined;
    if (key) span.textContent = ACCENT_LABELS[lang][key];
  }

  const srvNameInput = document.getElementById("srv-name");
  if (srvNameInput instanceof HTMLInputElement && document.activeElement !== srvNameInput) {
    const val = srvNameInput.value.trim();
    if (val === "Yerel Sunucu" || val === "Local Server" || val === "Lokaler Server" || !val) {
      srvNameInput.value = tr(lang, "servers.defaultName");
    }
  }

  byId<HTMLElement>("topbar-crumb").textContent = tr(lang, `view.${currentActiveViewId()}`);

  if (!state) return;
  if (state.config.ui_layout !== "DeckMode") {
    lastDesktopLayout = state.config.ui_layout;
  }

  const theme = getEffectiveTheme();
  const accent = getEffectiveAccent();
  const customHex = getEffectiveCustomHex();
  const corner = getEffectiveCorner();
  const layout = getEffectiveLayout();
  if (layout === "Sidebar" || layout === "RightRail") {
    lastExpandedRailLayout = layout;
  }
  const sounds = getEffectiveSounds();

  const themeChanged = prevAppliedTheme !== null && prevAppliedTheme !== theme;
  const layoutChanged = prevAppliedLayout !== null && prevAppliedLayout !== layout;
  const instantSwitch = themeChanged || layoutChanged || document.documentElement.classList.contains("is-hydrating");
  if (instantSwitch) {
    document.documentElement.classList.add("theme-switching");
  }
  if (layoutChanged) {
    document.getElementById("nav-indicator")?.classList.remove("is-ready");
  }

  setSoundsEnabled(sounds);
  document.documentElement.setAttribute("data-theme", THEME_ATTRS[theme] ?? "dark");
  document.documentElement.setAttribute("data-layout", LAYOUT_ATTRS[layout] ?? "sidebar");
  applyDynamicDesignTokens(theme, accent, customHex, corner);
  prevAppliedTheme = theme;
  prevAppliedLayout = layout;
  if (instantSwitch) {
    void document.documentElement.offsetWidth;
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        document.documentElement.classList.remove("theme-switching");
      });
    });
  }
  document.body.classList.toggle("compact", state.config.ui_compact);
  syncRootMotionAttribute(state.config.ui_reduced_motion);
  const reducedMotion = getEffectiveMotion() === "none";
  playSkin3d?.setReducedMotion(reducedMotion);
  profileSkin3d?.setReducedMotion(reducedMotion);
  deckSkin3d?.setReducedMotion(reducedMotion);

  byId<HTMLSpanElement>("rail-theme-label").textContent = THEME_SHORT_LABELS[lang][theme];
  byId<HTMLSpanElement>("rail-layout-label").textContent = LAYOUT_SHORT_LABELS[lang][layout];
  byId<HTMLSpanElement>("rail-sound-label").textContent = tr(
    lang,
    sounds ? "rail.soundOn" : "rail.soundOff"
  );
  byId<HTMLButtonElement>("btn-rail-sound").setAttribute("aria-pressed", sounds ? "true" : "false");

  const isDeck = layout === "DeckMode";
  byId<HTMLElement>("deck-controls-bar").hidden = !isDeck;
  byId<HTMLButtonElement>("btn-exit-deck").hidden = !isDeck;
  byId<HTMLButtonElement>("btn-enter-deck").hidden = isDeck;
  const deckMeta = document.querySelector<HTMLElement>(".deck-console-meta");
  if (deckMeta) deckMeta.hidden = !isDeck;
  byId<HTMLElement>("deck-clock").hidden = !isDeck;
  byId<HTMLElement>("console-home").hidden = !isDeck;
  syncDeckClock(isDeck);
  syncDeckGamepadLoop(isDeck);
  syncNavIndicator();
  syncSlidingIndicators();
}

function syncNavIndicator(): void {
  const nav = document.querySelector<HTMLElement>("aside.rail .nav");
  const ind = document.getElementById("nav-indicator");
  const active = nav?.querySelector<HTMLElement>(".nav-btn.active") ?? null;
  if (!nav || !ind || !active || getEffectiveLayout() === "DeckMode" || active.offsetWidth === 0) {
    nav?.classList.remove("has-indicator");
    ind?.classList.remove("is-ready");
    return;
  }
  ind.style.setProperty("--ind-x", `${active.offsetLeft}px`);
  ind.style.setProperty("--ind-y", `${active.offsetTop}px`);
  ind.style.setProperty("--ind-w", `${active.offsetWidth}px`);
  ind.style.setProperty("--ind-h", `${active.offsetHeight}px`);
  nav.classList.add("has-indicator");
  requestAnimationFrame(() => ind.classList.add("is-ready"));
}

function clearSearchParam(param: string): void {
  const url = new URL(window.location.href);
  if (url.searchParams.has(param)) {
    url.searchParams.delete(param);
    window.history.replaceState(null, "", url.pathname + url.search + url.hash);
  }
}

function setActiveView(view: ViewId, updateHash = true, withSound = false): void {
  const delDlg = document.getElementById("delete-world-dialog");
  if (delDlg instanceof HTMLDialogElement && delDlg.open) {
    delDlg.close();
  }
  let changed = false;
  const navButtons = document.querySelectorAll<HTMLButtonElement>(".nav-btn[data-nav]");
  for (const btn of Array.from(navButtons)) {
    const isMatch = btn.dataset.nav === view;
    if (isMatch && !btn.classList.contains("active")) {
      changed = true;
    }
    btn.classList.toggle("active", isMatch);
    if (isMatch) {
      btn.setAttribute("aria-current", "page");
    } else {
      btn.removeAttribute("aria-current");
    }
  }

  let incomingSection: HTMLElement | null = null;
  const sections = document.querySelectorAll<HTMLElement>("main .view[data-view]");
  for (const section of Array.from(sections)) {
    const isTarget = section.dataset.view === view;
    section.hidden = !isTarget;
    if (isTarget) {
      incomingSection = section;
    } else {
      cancelElementAnimation(section);
    }
  }

  if (changed && incomingSection) {
    animateViewHandoff(incomingSection, getEffectiveMotion());
  }

  byId<HTMLElement>("topbar-crumb").textContent = tr(getEffectiveLanguage(), `view.${view}`);

  if (updateHash && window.location.hash !== `#${view}`) {
    window.history.replaceState(null, "", `#${view}`);
  }
  if (view === "play" && getEffectiveLayout() === "DeckMode") {
    renderConsoleHome();
  }
  if (view === "library" && state) {
    renderLibraryView();
    const currentKey = buildCurrentCatalogKey();
    if (libActiveTab !== "installed" && (libLastCatalogKey !== currentKey || libCatalogError !== null)) {
      void loadLibraryCatalog(false);
    } else if (libActiveTab === "installed") {
      void loadLibraryInstalled();
    }
  }
  if (view === "worlds" && state && !screenshotsLoaded) {
    void loadScreenshots(false);
  }
  if (view === "system" && state) {
    const netPanel = document.querySelector<HTMLElement>('[data-settings-panel="network"]');
    if (netPanel && !netPanel.hidden) {
      void autoPingSavedServers(false);
    }
  }
  if (withSound && changed) {
    playMcSound("tab");
  }
  syncNavIndicator();
  syncSlidingIndicators();
}

function readInitialViewFromHash(): ViewId {
  const raw = window.location.hash.replace(/^#/, "").trim();
  if (raw === "library" || raw === "worlds" || raw === "profile" || raw === "system" || raw === "play") {
    return raw;
  }
  return "play";
}

function appendLogLine(line: string, isErr: boolean): void {
  const clean = line.replace(/\r$/, "");
  if (!clean) return;
  const prefix = isErr ? "[ERR] " : "";
  logLines.push(`${prefix}${clean}`);
  if (logLines.length > MAX_LOG_LINES) {
    logLines = logLines.slice(-MAX_LOG_LINES);
  }
  const box = byId<HTMLPreElement>("gradle-log-box");
  box.textContent = logLines.join("\n");
  box.hidden = false;
  box.scrollTop = box.scrollHeight;
}

function syncVersionUi(): void {
  if (!state) return;
  const ver = state.config.mc_version || "1.21.4";
  const loader = state.config.mod_loader || "Vanilla";

  const railSub = document.getElementById("rail-brand-sub");
  if (railSub) railSub.textContent = `${loader.toUpperCase()} · ${ver}`;

  const railBuildVer = document.getElementById("rail-build-ver");
  if (railBuildVer) railBuildVer.textContent = `Minecraft ${ver} (${loader})`;

  const railPlayerVer = document.getElementById("rail-player-ver");
  if (railPlayerVer) railPlayerVer.textContent = `${ver} · ${loader}`;

  const stageKicker = document.getElementById("stage-kicker");
  if (stageKicker) stageKicker.textContent = `${loader.toUpperCase()} · ${ver}`;

  const loaderPills = document.querySelectorAll<HTMLButtonElement>(".loader-pill[data-loader]");
  for (const btn of Array.from(loaderPills)) {
    const isActive = btn.dataset.loader === loader;
    btn.classList.toggle("active", isActive);
    btn.setAttribute("aria-pressed", isActive ? "true" : "false");
    btn.disabled = state.running_session !== null;
  }

  const playVerSelect = document.getElementById("play-version-select");
  if (playVerSelect instanceof HTMLSelectElement) {
    playVerSelect.value = ver;
    playVerSelect.disabled = state.running_session !== null;
  }

  const settingsVerSelect = document.getElementById("settings-version-select");
  if (settingsVerSelect instanceof HTMLSelectElement) {
    settingsVerSelect.value = ver;
    settingsVerSelect.disabled = state.running_session !== null;
  }
  syncSlidingIndicators();
}

function renderTopbarAndSession(): void {
  if (!state) return;
  const lang = getEffectiveLanguage();
  syncVersionUi();
  byId<HTMLSpanElement>("preview-mode-badge").hidden = ipc.isTauriRuntime();
  byId<HTMLSpanElement>("topbar-username").textContent = state.config.username;
  byId<HTMLSpanElement>("deck-username").textContent = state.config.username;
  drawTopbarHead(byId<HTMLCanvasElement>("topbar-head-canvas"), state.skin_rgba);
  drawTopbarHead(byId<HTMLCanvasElement>("deck-head-canvas"), state.skin_rgba);

  const railUsername = document.getElementById("rail-username");
  if (railUsername) railUsername.textContent = state.config.username;
  const railHead = document.getElementById("rail-head-canvas");
  if (railHead instanceof HTMLCanvasElement) {
    drawTopbarHead(railHead, state.skin_rgba);
  }
  const railHwMini = document.getElementById("rail-hw-mini");
  if (railHwMini) {
    railHwMini.textContent = state.vanilla.java_ready
      ? `${state.config.graphics.jvm_ram_mb} MB`
      : tr(lang, "play.specJdkMissing");
  }
  const rtJava = document.getElementById("rail-rt-java");
  if (rtJava) {
    rtJava.textContent = shortJavaLabel(state.vanilla, lang);
  }
  const rtCache = document.getElementById("rail-rt-cache");
  if (rtCache) {
    const ready = state.vanilla.client_ready && state.vanilla.assets_ready;
    rtCache.textContent = tr(lang, ready ? "play.specModReady" : "play.specModAuto");
  }
  renderRailTargets();

  const railStatusRow = byId<HTMLDivElement>("rail-status-row");
  const railStatusText = byId<HTMLSpanElement>("rail-status-text");
  const pill = byId<HTMLDivElement>("engine-status-pill");
  const launchBtn = byId<HTMLButtonElement>("btn-launch-client");
  const prepareBtn = byId<HTMLButtonElement>("btn-prepare-gradle");
  const openMcBtn = byId<HTMLButtonElement>("btn-open-mc-for-world");
  const installJavaBtn = byId<HTMLButtonElement>("btn-install-java");
  const recheckJavaBtn = byId<HTMLButtonElement>("btn-recheck-java");
  const sysInstallJavaBtn = byId<HTMLButtonElement>("btn-system-install-java");
  const sysRecheckJavaBtn = byId<HTMLButtonElement>("btn-system-recheck-java");
  const launchLabel = byId<HTMLSpanElement>("launch-btn-label");

  if (state.running_session) {
    const kind = state.running_session.kind;
    const isPrepare = kind === "prepare";
    const isJavaInstall = kind === "java-install";
    railStatusRow.setAttribute("data-state", "ready");
    railStatusText.textContent = tr(
      lang,
      isJavaInstall ? "rail.statusDownloadingJava" : "rail.statusSessionActive"
    );
    pill.setAttribute("data-state", "running");
    pill.textContent = tr(
      lang,
      isJavaInstall ? "pill.javaInstalling" : isPrepare ? "pill.building" : "pill.running"
    );
    launchBtn.disabled = true;
    prepareBtn.disabled = true;
    openMcBtn.disabled = true;
    installJavaBtn.disabled = true;
    recheckJavaBtn.disabled = true;
    sysInstallJavaBtn.disabled = true;
    sysRecheckJavaBtn.disabled = true;
    launchBtn.dataset.state = isPrepare || isJavaInstall ? "busy" : "playing";
    launchLabel.textContent = tr(
      lang,
      isJavaInstall ? "btn.installingJava" : isPrepare ? "btn.building" : "btn.mcRunning"
    );
  } else {
    installJavaBtn.disabled = false;
    recheckJavaBtn.disabled = false;
    sysInstallJavaBtn.disabled = false;
    sysRecheckJavaBtn.disabled = false;
    if (!state.vanilla.java_ready) {
      railStatusRow.setAttribute("data-state", "warning");
      railStatusText.textContent = tr(lang, "rail.statusJavaMissing");
      pill.setAttribute("data-state", "warning");
      pill.textContent = tr(lang, "pill.javaRequired");
      launchBtn.disabled = true;
      launchBtn.dataset.state = "blocked";
      prepareBtn.disabled = true;
      openMcBtn.disabled = true;
      launchLabel.textContent = tr(lang, "btn.installJavaFirst");
    } else {
      const cacheReady = state.vanilla.client_ready && state.vanilla.assets_ready;
      railStatusRow.setAttribute("data-state", "ready");
      railStatusText.textContent = tr(lang, "rail.statusReady");
      pill.setAttribute("data-state", "idle");
      pill.textContent = tr(lang, cacheReady ? "pill.ready" : "pill.awaitFirstBuild");
      launchBtn.disabled = false;
      launchBtn.dataset.state = "ready";
      prepareBtn.disabled = false;
      openMcBtn.disabled = false;
      launchLabel.textContent = tr(
        lang,
        state.config.last_save_id ? "btn.launchSelected" : "btn.launchMc"
      );
    }
  }
}

function renderRailTargets(): void {
  const list = document.getElementById("rail-targets-list");
  if (!list || !state) return;

  const lang = getEffectiveLanguage();
  const { config, saves, vanilla, running_session } = state;
  const activeSaveId = saves.some((s) => s.id === config.last_save_id) ? config.last_save_id : null;
  const launchDisabled = !vanilla.java_ready || running_session !== null;
  list.innerHTML = "";

  const appendTarget = (opts: {
    kind: "title" | "save" | "server";
    title: string;
    sub: string;
    active: boolean;
    onSelect: () => void;
    onLaunch: () => void;
  }): void => {
    const item = document.createElement("div");
    item.className = "rail-target-item" + (opts.active ? " is-active" : "");
    item.setAttribute("role", "listitem");
    item.dataset.kind = opts.kind;

    const mainBtn = document.createElement("button");
    mainBtn.type = "button";
    mainBtn.className = "rail-target-main";
    if (opts.active) mainBtn.setAttribute("aria-current", "true");
    mainBtn.addEventListener("click", opts.onSelect);

    const dot = document.createElement("span");
    dot.className = "rail-target-dot";
    dot.setAttribute("aria-hidden", "true");

    const copy = document.createElement("span");
    copy.className = "rail-target-copy";

    const nameSpan = document.createElement("span");
    nameSpan.className = "rail-target-name";
    nameSpan.textContent = opts.title;

    const subSpan = document.createElement("span");
    subSpan.className = "rail-target-sub tabular";
    subSpan.textContent = opts.sub;

    copy.append(nameSpan, subSpan);
    mainBtn.append(dot, copy);

    const launchBtn = document.createElement("button");
    launchBtn.type = "button";
    launchBtn.className = "rail-target-launch";
    launchBtn.disabled = launchDisabled;
    launchBtn.title = tr(lang, "worlds.btnLaunch");
    launchBtn.setAttribute("aria-label", `${tr(lang, "worlds.btnLaunch")}: ${opts.title}`);
    launchBtn.innerHTML = `<svg class="ico small" aria-hidden="true"><use href="#ico-play" /></svg>`;
    launchBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      opts.onLaunch();
    });

    item.append(mainBtn, launchBtn);
    list.appendChild(item);
  };

  appendTarget({
    kind: "title",
    title: tr(lang, "play.mainMenuTitle"),
    sub: tr(lang, "rail.titleScreenSub"),
    active: !activeSaveId,
    onSelect: () => {
      void handleSelectSave(null);
      setActiveView("play", true, true);
    },
    onLaunch: () => {
      void handleLaunchClient("title", null, null);
    },
  });

  for (const save of saves.slice(0, 6)) {
    appendTarget({
      kind: "save",
      title: save.id,
      sub: tr(lang, "rail.worldSub", { size: formatBytes(save.size_bytes) }),
      active: activeSaveId === save.id,
      onSelect: () => {
        void handleSelectSave(save.id);
        setActiveView("play", true, true);
      },
      onLaunch: () => {
        void handleLaunchClient("singleplayer", save.id, null);
      },
    });
  }

  for (const srv of config.saved_servers.slice(0, 4)) {
    const check = serverChecks.get(srv.address);
    const liveSub =
      check?.status === "online" && check.ping
        ? `${check.ping.latency_ms} ms · ${check.ping.online_players}/${check.ping.max_players}`
        : tr(lang, "rail.serverSub", { addr: srv.address });
    appendTarget({
      kind: "server",
      title: srv.name,
      sub: liveSub,
      active: false,
      onSelect: () => {
        setActiveView("system", true, true);
        document.querySelector<HTMLButtonElement>('[data-settings-tab="network"]')?.click();
      },
      onLaunch: () => {
        void handleLaunchClient("multiplayer", null, srv.address);
      },
    });
  }
}

let lastPlayTargetKey = "";

function renderPlayView(): void {
  if (!state) return;
  const strata = document.getElementById("stage-strata");
  if (strata instanceof SVGSVGElement) {
    renderStrata(strata);
  }
  const lang = getEffectiveLanguage();
  const { config, saves, hardware, vanilla } = state;
  const cacheReady = vanilla.client_ready && vanilla.assets_ready;
  syncVersionUi();

  const selectedSave = saves.find((s) => s.id === config.last_save_id) ?? null;
  const activeSaveId = selectedSave ? selectedSave.id : null;
  if (config.last_save_id !== activeSaveId) {
    config.last_save_id = activeSaveId;
  }

  const titleEl = byId<HTMLHeadingElement>("play-world-title");
  titleEl.textContent = activeSaveId ?? tr(lang, "play.mainMenuTitle");
  const nextTargetKey = `${activeSaveId ?? "__title__"}::${config.mc_version || "1.21.4"}::${config.mod_loader}`;
  if (lastPlayTargetKey !== "" && lastPlayTargetKey !== nextTargetKey) {
    animateTextSwap(titleEl, getEffectiveMotion());
  }
  lastPlayTargetKey = nextTargetKey;

  const targetSub = document.getElementById("play-target-sub");
  if (targetSub) {
    targetSub.textContent = selectedSave
      ? `${formatBytes(selectedSave.size_bytes)} · ${formatEpoch(selectedSave.modified_epoch_secs)}`
      : tr(lang, "play.titleScreenMeta", { ver: `${config.mc_version || "1.21.4"} (${config.mod_loader})` });
  }

  const loaderSummaryEl = document.getElementById("play-loader-summary");
  if (loaderSummaryEl && state.library_counts) {
    loaderSummaryEl.textContent = tr(lang, "loader.summary", {
      mods: state.library_counts.modsEnabled,
      shaders: state.library_counts.shadersEnabled,
    });
  }

  const playTimePill = document.getElementById("play-playtime-pill");
  if (playTimePill) {
    const totalSecs = state.playtime_stats?.total_playtime_secs ?? 0;
    if (totalSecs > 0) {
      playTimePill.textContent = tr(lang, "playtime.totalBadge", {
        time: formatPlaytime(totalSecs, lang),
      });
      playTimePill.hidden = false;
    } else {
      playTimePill.hidden = true;
    }
  }

  const playSkinCanvas = document.getElementById("play-skin-canvas");
  if (playSkinCanvas instanceof HTMLCanvasElement) {
    if (!playSkin3d) {
      playSkin3d = new Skin3DViewer(playSkinCanvas, {
        interactive: true,
        trackCursor: true,
        onClickFlip: () => playMcSound("tab"),
        onResetPose: () => {
          playMcSound("click");
          showToast(tr(getEffectiveLanguage(), "skin.poseResetToast"), "neutral");
        },
      });
    }
    playSkin3d.setReducedMotion(getEffectiveMotion() === "none");
    playSkin3d.setSkin(state.skin_rgba, state.skin_is_slim);
  }
  const tuneRam = document.getElementById("play-tune-ram");
  if (tuneRam) tuneRam.textContent = `${config.graphics.jvm_ram_mb} MB · ${config.graphics.jvm_gc}`;
  const tuneGfx = document.getElementById("play-tune-gfx");
  if (tuneGfx) {
    tuneGfx.textContent = tr(lang, "play.tuneGfxVal", {
      n: config.graphics.render_distance,
      preset: config.graphics.preset,
    });
  }
  const tuneNet = document.getElementById("play-tune-net");
  if (tuneNet) {
    tuneNet.textContent = tr(lang, config.network_boost ? "play.netBoostVal" : "play.netStdVal");
  }
  const playSkinName = document.getElementById("play-skin-name");
  if (playSkinName) playSkinName.textContent = config.username;
  const playSkinModel = document.getElementById("play-skin-model");
  if (playSkinModel) {
    playSkinModel.textContent = tr(
      lang,
      state.skin_is_slim ? "skin.badgeSlim" : "skin.badgeClassic"
    );
  }
  renderRailTargets();

  const select = byId<HTMLSelectElement>("play-save-select");
  select.innerHTML = "";
  const titleOpt = document.createElement("option");
  titleOpt.value = "";
  titleOpt.textContent = tr(lang, "play.mainMenuOpt");
  select.appendChild(titleOpt);

  for (const save of saves) {
    const opt = document.createElement("option");
    opt.value = save.id;
    opt.textContent = save.id;
    select.appendChild(opt);
  }
  select.value = activeSaveId ?? "";

  byId<HTMLElement>("play-spec-jdk").textContent = shortJavaLabel(vanilla, lang);
  byId<HTMLElement>("play-spec-ram").textContent = `${config.graphics.jvm_ram_mb} MB (${config.graphics.jvm_gc})`;
  byId<HTMLElement>("play-spec-chunks").textContent = tr(lang, "play.chunksValue", {
    n: config.graphics.render_distance,
  });
  byId<HTMLElement>("play-spec-mod").textContent = tr(
    lang,
    cacheReady ? "play.specModReady" : "play.specModAuto"
  );

  const javaPanel = byId<HTMLDivElement>("java-setup-panel");
  if (!vanilla.java_ready) {
    javaPanel.hidden = false;
    byId<HTMLParagraphElement>("java-setup-desc").textContent =
      lang === "En" ? vanilla.java_hint : tr(lang, "play.javaSetupDesc");
  } else {
    javaPanel.hidden = true;
  }

  const gpuShort = hardware.gpu_name.length > 34 ? `${hardware.gpu_name.slice(0, 31)}...` : hardware.gpu_name;
  byId<HTMLSpanElement>("play-hw-summary").textContent =
    `${hardware.physical_cores}C/${hardware.logical_cores}T · ${Math.round(hardware.total_ram_mb / 1024)} GB RAM · ${gpuShort} (${config.graphics.preset})`;

  const playUsername = byId<HTMLInputElement>("play-username");
  if (document.activeElement !== playUsername) {
    playUsername.value = config.username;
  }

  byId<HTMLSpanElement>("prepare-status-hint").textContent = tr(
    lang,
    cacheReady ? "play.prepareHintReady" : "play.prepareHintWait"
  );

  // Quick Jump · Worlds & Servers list on Play view
  const recentList = byId<HTMLDivElement>("play-recent-list");
  recentList.innerHTML = "";

  interface QuickJumpEntry {
    kind: "world" | "server";
    title: string;
    meta: string;
    isSelected?: boolean;
    saveId?: string;
    serverAddr?: string;
  }

  const jumpEntries: QuickJumpEntry[] = [];
  for (const save of saves.slice(0, 3)) {
    jumpEntries.push({
      kind: "world",
      title: save.id,
      meta: tr(lang, "play.recentWorldMeta", {
        size: formatBytes(save.size_bytes),
        date: formatEpoch(save.modified_epoch_secs),
      }),
      isSelected: save.id === activeSaveId,
      saveId: save.id,
    });
  }
  for (const srv of config.saved_servers.slice(0, Math.max(0, 3 - jumpEntries.length))) {
    jumpEntries.push({
      kind: "server",
      title: srv.name,
      meta: tr(lang, "play.recentServerMeta", { addr: srv.address }),
      serverAddr: srv.address,
    });
  }

  const totalTargets = saves.length + config.saved_servers.length;
  byId<HTMLSpanElement>("play-recent-count").textContent = tr(lang, "deck.itemsCount", {
    count: totalTargets,
  });

  if (jumpEntries.length === 0) {
    const emptyEl = document.createElement("p");
    emptyEl.className = "muted-copy recent-empty-note";
    emptyEl.textContent = tr(lang, "play.recentEmpty");
    recentList.appendChild(emptyEl);
  } else {
    for (const item of jumpEntries) {
      const row = document.createElement("div");
      row.className = item.isSelected ? "recent-jump-row is-active" : "recent-jump-row";
      row.setAttribute("role", "listitem");
      row.dataset.kind = item.kind;
      row.style.setProperty("--tile-hue", String(seedHue(item.saveId ?? item.serverAddr ?? item.title)));

      const art = document.createElement("div");
      art.className = "recent-jump-art";
      art.setAttribute("aria-hidden", "true");
      if (item.kind === "world" && item.saveId) {
        const matchedSave = saves.find((s) => s.id === item.saveId);
        if (matchedSave?.icon_data_url) {
          const img = document.createElement("img");
          img.src = matchedSave.icon_data_url;
          img.alt = "";
          img.style.width = "100%";
          img.style.height = "100%";
          img.style.objectFit = "cover";
          img.style.borderRadius = "inherit";
          art.appendChild(img);
        } else {
          art.innerHTML = `<svg class="ico"><use href="#ico-cube" /></svg>`;
        }
      } else if (item.kind === "server" && item.serverAddr) {
        const srvCheck = serverChecks.get(item.serverAddr);
        if (srvCheck?.status === "online" && srvCheck.ping?.favicon?.startsWith("data:image/png;base64,")) {
          const img = document.createElement("img");
          img.src = srvCheck.ping.favicon;
          img.alt = "";
          img.style.width = "100%";
          img.style.height = "100%";
          img.style.objectFit = "cover";
          img.style.borderRadius = "inherit";
          art.appendChild(img);
        } else {
          art.innerHTML = `<svg class="ico"><use href="#ico-server" /></svg>`;
        }
      } else {
        art.innerHTML = `<svg class="ico"><use href="#ico-cube" /></svg>`;
      }

      const copy = document.createElement("div");
      copy.className = "recent-jump-copy";

      const titleLine = document.createElement("div");
      titleLine.className = "recent-jump-title";
      const nameSpan = document.createElement("span");
      nameSpan.textContent = item.title;
      titleLine.appendChild(nameSpan);

      if (item.isSelected) {
        const tag = document.createElement("span");
        tag.className = "row-tag";
        tag.textContent = tr(lang, "worlds.selectedTag");
        titleLine.appendChild(tag);
      }

      const metaLine = document.createElement("div");
      metaLine.className = "recent-jump-meta tabular";
      if (item.kind === "server" && item.serverAddr) {
        const srvCheck = serverChecks.get(item.serverAddr);
        if (srvCheck?.status === "online" && srvCheck.ping) {
          metaLine.textContent = `${srvCheck.ping.latency_ms} ms · ${srvCheck.ping.online_players}/${srvCheck.ping.max_players} · ${item.serverAddr}`;
        } else if (srvCheck?.status === "error") {
          metaLine.textContent = `${tr(lang, "servers.statusOffline")} · ${item.serverAddr}`;
        } else {
          metaLine.textContent = item.meta;
        }
      } else {
        metaLine.textContent = item.meta;
      }

      copy.append(titleLine, metaLine);
      if (item.kind === "server" && item.serverAddr) {
        const srvCheck = serverChecks.get(item.serverAddr);
        if (srvCheck && srvCheck.status !== "idle") {
          copy.appendChild(createServerPingPill(srvCheck, lang));
        }
      }

      const actionBtn = document.createElement("button");
      actionBtn.type = "button";
      actionBtn.className = "btn-secondary btn-sm";
      actionBtn.disabled = state.running_session !== null || !vanilla.java_ready;
      actionBtn.textContent = tr(
        lang,
        item.kind === "world" ? "worlds.btnLaunch" : "servers.btnConnect"
      );
      actionBtn.addEventListener("click", () => {
        if (item.kind === "world" && item.saveId) {
          void handleLaunchClient("singleplayer", item.saveId, null);
        } else if (item.kind === "server" && item.serverAddr) {
          void handleLaunchClient("multiplayer", null, item.serverAddr);
        }
      });

      row.append(art, copy, actionBtn);
      recentList.appendChild(row);
    }
  }

  if (getEffectiveLayout() === "DeckMode") {
    renderConsoleHome();
  }
}

function buildDeckCards(): DeckCard[] {
  if (!state) return [];
  const lang = getEffectiveLanguage();
  const { config, saves, hardware, vanilla } = state;
  const cacheReady = vanilla.client_ready && vanilla.assets_ready;
  const cards: DeckCard[] = [
    {
      id: "main-menu",
      kind: "launch",
      kicker: tr(lang, "deck.cardMainKicker"),
      title: tr(lang, "play.mainMenuTitle"),
      meta: tr(lang, cacheReady ? "deck.cardMainMetaReady" : "deck.cardMainMetaAuto"),
      sub: tr(lang, "deck.cardMainSub", {
        user: config.username,
        preset: config.graphics.preset,
      }),
      tag: tr(lang, "deck.cardMainTag"),
      seedKey: "basalt-console-home",
      primaryLabel: tr(lang, "deck.cardMainPrimary"),
      secondaryLabel: tr(lang, "play.manageSaves"),
      launchMode: "title",
      saveId: null,
      targetView: "worlds",
    },
  ];

  for (const save of saves) {
    const isActive = save.id === config.last_save_id;
    cards.push({
      id: `world:${save.id}`,
      kind: "world",
      kicker: tr(
        lang,
        isActive ? "deck.cardWorldKickerActive" : "deck.cardWorldKicker"
      ),
      title: save.id,
      meta: formatBytes(save.size_bytes),
      sub: tr(lang, "deck.cardWorldSub", {
        date: formatEpoch(save.modified_epoch_secs),
      }),
      tag: save.id,
      seedKey: save.id,
      primaryLabel: tr(lang, "deck.cardWorldPrimary"),
      secondaryLabel: tr(
        lang,
        isActive ? "deck.cardWorldSecActive" : "deck.cardWorldSecPick"
      ),
      secondaryDisabled: isActive,
      launchMode: "singleplayer",
      saveId: save.id,
    });
  }

  for (const srv of config.saved_servers) {
    const check = serverChecks.get(srv.address);
    const metaText =
      check?.status === "online" && check.ping
        ? `${check.ping.latency_ms} ms · ${check.ping.online_players}/${check.ping.max_players}`
        : srv.address;
    cards.push({
      id: `server:${srv.address}`,
      kind: "server",
      kicker: tr(lang, "deck.cardServerKicker"),
      title: srv.name,
      meta: metaText,
      sub: tr(lang, "deck.cardServerSub", { addr: srv.address }),
      tag: tr(lang, "deck.cardServerTag", { addr: srv.address }),
      seedKey: `srv-${srv.address}`,
      primaryLabel: tr(lang, "deck.cardServerPrimary"),
      secondaryLabel: tr(lang, "nav.system"),
      launchMode: "multiplayer",
      serverAddress: srv.address,
      targetView: "system",
    });
  }

  cards.push(
    {
      id: "nav:worlds",
      kind: "nav",
      kicker: toLocaleUpperSafe(tr(lang, "worlds.heading"), lang),
      title: tr(lang, "worlds.heading"),
      meta:
        saves.length === 1
          ? tr(lang, "worlds.countOne")
          : tr(lang, "worlds.countLabel", { count: saves.length }),
      sub: tr(lang, "deck.cardNavWorldsSub"),
      tag: tr(lang, "deck.cardNavWorldsTag"),
      seedKey: "nav-worlds",
      primaryLabel: tr(lang, "deck.cardNavWorldsPrimary"),
      secondaryLabel: tr(lang, "nav.profile"),
      targetView: "worlds",
    },
    {
      id: "nav:library",
      kind: "nav",
      kicker: tr(lang, "library.kicker"),
      title: tr(lang, "nav.library"),
      meta: tr(lang, "loader.summary", {
        mods: state.library_counts?.modsEnabled ?? 0,
        shaders: state.library_counts?.shadersEnabled ?? 0,
      }),
      sub: tr(lang, "library.subtitle"),
      tag: toLocaleUpperSafe(tr(lang, "nav.library"), lang),
      seedKey: "nav-library",
      primaryLabel: tr(lang, "nav.library"),
      secondaryLabel: tr(lang, "play.manageSaves"),
      targetView: "library",
    },
    {
      id: "nav:profile",
      kind: "nav",
      kicker: tr(lang, "deck.cardNavProfileKicker"),
      title: tr(lang, "deck.cardNavProfileTitle"),
      meta: tr(lang, "deck.cardNavProfileMeta", {
        theme: THEME_SHORT_LABELS[lang][getEffectiveTheme()],
      }),
      sub: tr(lang, "deck.cardNavProfileSub"),
      tag: toLocaleUpperSafe(tr(lang, "nav.profile"), lang),
      seedKey: "nav-profile",
      primaryLabel: tr(lang, "deck.cardNavProfilePrimary"),
      secondaryLabel: tr(lang, "play.manageSaves"),
      targetView: "profile",
    },
    {
      id: "nav:system",
      kind: "nav",
      kicker: toLocaleUpperSafe(tr(lang, "system.heading"), lang),
      title: tr(lang, "deck.cardNavSystemTitle"),
      meta: `${hardware.physical_cores}C/${hardware.logical_cores}T · ${Math.round(hardware.total_ram_mb / 1024)} GB`,
      sub: tr(lang, "deck.cardNavSystemSub"),
      tag: toLocaleUpperSafe(tr(lang, "nav.system"), lang),
      seedKey: "nav-system",
      primaryLabel: tr(lang, "deck.cardNavSystemPrimary"),
      secondaryLabel: tr(lang, "deck.cardNavProfilePrimary"),
      targetView: "system",
    }
  );

  return cards;
}

function appendHeroSpec(dl: HTMLElement, term: string, val: string): void {
  const row = document.createElement("div");
  const dt = document.createElement("dt");
  dt.textContent = term;
  const dd = document.createElement("dd");
  dd.className = "tabular";
  dd.textContent = val;
  row.append(dt, dd);
  dl.appendChild(row);
}

function renderConsoleHero(card: DeckCard): void {
  if (!state) return;
  const lang = getEffectiveLanguage();
  const { config, vanilla, hardware, running_session, skin_rgba, skin_is_slim } = state;

  byId<HTMLElement>("deck-hero-kicker").textContent = running_session
    ? `${card.kicker} · PID ${running_session.pid}`
    : card.kicker;
  byId<HTMLElement>("deck-hero-title").textContent = card.title;
  byId<HTMLElement>("deck-hero-sub").textContent = card.sub;

  const specsEl = byId<HTMLElement>("deck-hero-specs");
  specsEl.innerHTML = "";
  appendHeroSpec(specsEl, "JDK", shortJavaLabel(vanilla, lang));
  appendHeroSpec(specsEl, "JVM RAM", `${config.graphics.jvm_ram_mb} MB`);
  appendHeroSpec(
    specsEl,
    toLocaleUpperSafe(tr(lang, "system.renderDist"), lang),
    tr(lang, "play.chunksValue", { n: config.graphics.render_distance })
  );
  appendHeroSpec(
    specsEl,
    tr(lang, card.kind === "world" ? "deck.sizeLabel" : "deck.hwLabel"),
    card.kind === "world"
      ? card.meta
      : `${hardware.physical_cores}C/${hardware.logical_cores}T`
  );

  const primaryBtn = byId<HTMLButtonElement>("btn-deck-primary");
  const primaryLabel = byId<HTMLSpanElement>("deck-primary-label");
  const needsJava = !vanilla.java_ready && Boolean(card.launchMode);
  primaryLabel.textContent = needsJava ? tr(lang, "deck.installJavaBtn") : card.primaryLabel;
  primaryBtn.disabled = running_session !== null && Boolean(card.launchMode);

  const secondaryBtn = byId<HTMLButtonElement>("btn-deck-secondary");
  byId<HTMLSpanElement>("deck-secondary-label").textContent = card.secondaryLabel;
  secondaryBtn.disabled = Boolean(card.secondaryDisabled);

  byId<HTMLButtonElement>("btn-deck-prepare").disabled =
    running_session !== null || !vanilla.java_ready;

  const statusLine = byId<HTMLElement>("deck-status-line");
  if (running_session) {
    statusLine.textContent = tr(lang, "deck.statusRunning", {
      label: running_session.label,
      pid: running_session.pid,
    });
  } else if (!vanilla.java_ready) {
    statusLine.textContent = tr(lang, "deck.statusMissingJava");
  } else if (config.last_save_id) {
    statusLine.textContent = tr(lang, "deck.statusReadyWorld", {
      save: config.last_save_id,
      preset: config.graphics.preset,
    });
  } else {
    statusLine.textContent = tr(lang, "deck.statusReadyProfile", {
      user: config.username,
      preset: config.graphics.preset,
      ram: config.graphics.jvm_ram_mb,
    });
  }

  const deckCanvas = byId<HTMLCanvasElement>("deck-skin-canvas");
  if (!deckSkin3d) {
    deckSkin3d = new Skin3DViewer(deckCanvas, {
      interactive: true,
      trackCursor: true,
      onClickFlip: () => playMcSound("tab"),
      onResetPose: () => {
        playMcSound("click");
        showToast(tr(getEffectiveLanguage(), "skin.poseResetToast"), "neutral");
      },
    });
  }
  deckSkin3d.setReducedMotion(getEffectiveMotion() === "none");
  deckSkin3d.setSkin(skin_rgba, skin_is_slim);
  byId<HTMLElement>("deck-skin-caption").textContent = config.username;
  byId<HTMLElement>("deck-skin-model-badge").textContent = tr(
    lang,
    skin_is_slim ? "skin.slimShort" : "skin.classicShort"
  );
}

function selectDeckCard(
  index: number,
  opts: { scroll?: boolean; focusBtn?: boolean; withSound?: boolean } = {}
): void {
  if (deckCards.length === 0) return;
  const clamped = ((index % deckCards.length) + deckCards.length) % deckCards.length;
  const changed = clamped !== selectedDeckIdx;
  selectedDeckIdx = clamped;
  const card = deckCards[selectedDeckIdx];
  renderConsoleHero(card);

  const carousel = byId<HTMLDivElement>("deck-carousel");
  const buttons = Array.from(carousel.querySelectorAll<HTMLButtonElement>(".deck-card"));
  buttons.forEach((btn, idx) => {
    const selected = idx === selectedDeckIdx;
    btn.classList.toggle("is-selected", selected);
    btn.setAttribute("aria-selected", selected ? "true" : "false");
  });

  const activeCardBtn = buttons[selectedDeckIdx];
  if (activeCardBtn) {
    if (opts.scroll) {
      activeCardBtn.scrollIntoView({
        behavior: state?.config.ui_reduced_motion ? "auto" : "smooth",
        block: "nearest",
        inline: "center",
      });
    }
    if (opts.focusBtn) {
      activeCardBtn.focus({ preventScroll: true });
    }
  }
  if (opts.withSound && changed) {
    playMcSound("focus");
  }
}

function handleDeckPrimaryAction(card: DeckCard): void {
  if (!state) return;
  unlockAudioContext();
  if (card.launchMode) {
    if (!state.vanilla.java_ready) {
      playMcSound("click");
      void handleInstallJava21();
      return;
    }
    void handleLaunchClient(card.launchMode, card.saveId ?? null, card.serverAddress ?? null);
    return;
  }
  if (card.targetView) {
    setActiveView(card.targetView, true, true);
  }
}

function handleDeckSecondaryAction(card: DeckCard): void {
  unlockAudioContext();
  if (card.kind === "world" && card.saveId) {
    playMcSound("click");
    void handleSelectSave(card.saveId);
    return;
  }
  if (card.targetView) {
    setActiveView(card.targetView, true, true);
    return;
  }
  setActiveView("worlds", true, true);
}

function renderConsoleHome(): void {
  if (!state) return;
  const lang = getEffectiveLanguage();
  const prevId = deckCards[selectedDeckIdx]?.id ?? null;
  deckCards = buildDeckCards();
  if (prevId) {
    const foundIdx = deckCards.findIndex((c) => c.id === prevId);
    selectedDeckIdx = foundIdx >= 0 ? foundIdx : 0;
  } else {
    selectedDeckIdx = 0;
  }

  byId<HTMLElement>("deck-carousel-count").textContent = tr(lang, "deck.itemsCount", {
    count: deckCards.length,
  });
  const carousel = byId<HTMLDivElement>("deck-carousel");
  carousel.innerHTML = "";

  deckCards.forEach((card, idx) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "deck-card";
    btn.dataset.kind = card.kind;
    btn.dataset.cardIndex = String(idx);
    btn.setAttribute("role", "option");
    btn.setAttribute("aria-selected", idx === selectedDeckIdx ? "true" : "false");
    if (idx === selectedDeckIdx) {
      btn.classList.add("is-selected");
    }

    const cardIconId =
      card.kind === "world"
        ? "ico-cube"
        : card.kind === "server"
        ? "ico-globe"
        : card.targetView === "profile"
        ? "ico-user"
        : card.targetView === "system"
        ? "ico-sliders"
        : card.targetView === "worlds"
        ? "ico-layers"
        : "ico-play";

    const topRow = document.createElement("span");
    topRow.className = "deck-card-top";
    const iconBox = document.createElement("span");
    iconBox.className = "deck-card-icon";
    iconBox.innerHTML = `<svg class="ico small" aria-hidden="true"><use href="#${cardIconId}" /></svg>`;
    const kicker = document.createElement("span");
    kicker.className = "deck-card-kicker";
    kicker.textContent = card.kicker;
    topRow.append(iconBox, kicker);

    const body = document.createElement("span");
    body.className = "deck-card-body";

    const title = document.createElement("span");
    title.className = "deck-card-title";
    title.textContent = card.title;

    const meta = document.createElement("span");
    meta.className = "deck-card-meta";
    meta.textContent = card.meta;

    body.append(title, meta);
    btn.append(topRow, body);

    btn.addEventListener("click", () => {
      unlockAudioContext();
      if (selectedDeckIdx !== idx) {
        selectDeckCard(idx, { scroll: true, focusBtn: false, withSound: true });
      } else {
        handleDeckPrimaryAction(card);
      }
    });
    btn.addEventListener("dblclick", () => {
      unlockAudioContext();
      selectDeckCard(idx, { scroll: true, focusBtn: false, withSound: false });
      handleDeckPrimaryAction(card);
    });
    btn.addEventListener("focus", () => {
      if (selectedDeckIdx !== idx) {
        selectDeckCard(idx, { scroll: true, focusBtn: false, withSound: false });
      }
    });

    carousel.appendChild(btn);
  });

  if (deckCards.length > 0) {
    renderConsoleHero(deckCards[selectedDeckIdx]);
  }
}

function enterConsoleMode(): void {
  if (!state) return;
  unlockAudioContext();
  if (state.config.ui_layout !== "DeckMode") {
    lastDesktopLayout = state.config.ui_layout;
  }
  consoleModeActive = true;
  consoleAudioMuted = false;
  setSoundsEnabled(true);
  playMcSound("launch", true);
  applyPreferences();
  renderProfileView();
  setActiveView("play", true, false);
  renderConsoleHome();
  void ipc.setFullscreen(true).catch(() => undefined);
  queueMicrotask(() => {
    const heroBtn = document.getElementById("btn-deck-primary");
    if (heroBtn instanceof HTMLButtonElement && !heroBtn.disabled) {
      heroBtn.focus({ preventScroll: true });
    }
  });
}

function toggleUiSound(desired?: boolean): void {
  if (!state) return;
  unlockAudioContext();
  if (getEffectiveLayout() === "DeckMode") {
    const nextMuted = desired !== undefined ? !desired : !consoleAudioMuted;
    consoleAudioMuted = nextMuted;
    const nowEnabled = !consoleAudioMuted;
    setSoundsEnabled(nowEnabled);
    playMcSound(nowEnabled ? "toggleOn" : "toggleOff", true);
    applyPreferences();
    renderProfileView();
    return;
  }
  const nextDesktopSound = desired !== undefined ? desired : !state.config.ui_sounds;
  void saveAccessibilityPrefs({ ui_sounds: nextDesktopSound });
}

function createServerPingPill(check: ServerCheckState, lang: UiLanguage): HTMLSpanElement {
  const pill = document.createElement("span");
  pill.className = "server-ping-pill";

  const dot = document.createElement("span");
  dot.className = "server-ping-dot";
  dot.setAttribute("aria-hidden", "true");
  pill.appendChild(dot);

  if (check.status === "checking") {
    pill.dataset.status = "pinging";
    const lbl = document.createElement("span");
    lbl.textContent = tr(lang, "servers.statusPinging");
    pill.appendChild(lbl);
  } else if (check.status === "online" && check.ping) {
    pill.dataset.status = check.ping.latency_ms >= 150 ? "degraded" : "online";
    const ms = document.createElement("span");
    ms.className = "server-ping-ms";
    ms.textContent = `${formatLocaleNumber(check.ping.latency_ms, lang, 0)} ms`;
    const players = document.createElement("span");
    players.className = "server-ping-players";
    players.textContent = tr(lang, "servers.playersLabel", {
      online: formatLocaleNumber(check.ping.online_players, lang, 0),
      max: formatLocaleNumber(check.ping.max_players, lang, 0),
    });
    pill.append(ms, players);
  } else {
    pill.dataset.status = "offline";
    const lbl = document.createElement("span");
    lbl.textContent = tr(lang, "servers.statusOffline");
    pill.appendChild(lbl);
  }
  return pill;
}

function renderCrashDoctor(animate = false): void {
  const host = document.getElementById("crash-doctor-host");
  if (!host) return;
  if (!lastCrashDiagnosis) {
    host.innerHTML = "";
    host.hidden = true;
    return;
  }

  const diag = lastCrashDiagnosis;
  const lang = getEffectiveLanguage();
  host.innerHTML = "";
  host.hidden = false;

  const card = document.createElement("section");
  card.className = `crash-doctor-card crash-doctor-card--${diag.severity}`;
  card.dataset.severity = diag.severity;
  card.setAttribute("role", "region");
  card.setAttribute("aria-label", tr(lang, "crash.btnLabel"));

  const head = document.createElement("div");
  head.className = "crash-doctor-head";

  const titleGroup = document.createElement("div");
  titleGroup.className = "crash-doctor-title-group";

  const badge = document.createElement("span");
  badge.className = "crash-doctor-badge";
  badge.textContent = tr(lang, `crash.sev.${diag.severity}`);

  const title = document.createElement("strong");
  title.className = "crash-doctor-title";
  title.textContent = tr(lang, `crash.title.${diag.code}`);

  const codeSpan = document.createElement("span");
  codeSpan.className = "crash-doctor-code";
  codeSpan.textContent = diag.code;

  titleGroup.append(badge, title, codeSpan);

  const dismissBtn = document.createElement("button");
  dismissBtn.type = "button";
  dismissBtn.className = "btn-icon";
  dismissBtn.setAttribute("aria-label", tr(lang, "crash.dismissAria"));
  dismissBtn.innerHTML = `<svg class="ico small" aria-hidden="true"><use href="#ico-close" /></svg>`;
  dismissBtn.addEventListener("click", () => {
    lastCrashDiagnosis = null;
    renderCrashDoctor(false);
  });

  head.append(titleGroup, dismissBtn);

  const body = document.createElement("div");
  body.className = "crash-doctor-body";

  if (diag.offending_mod) {
    const modPill = document.createElement("span");
    modPill.className = "crash-doctor-mod-pill";
    const modLbl = document.createElement("strong");
    modLbl.textContent = `${tr(lang, "crash.modLabel")}:`;
    const modVal = document.createElement("span");
    modVal.textContent = diag.offending_mod;
    modPill.append(modLbl, modVal);
    body.appendChild(modPill);
  }

  const details = document.createElement("p");
  details.className = "crash-doctor-details";
  const localizedDesc = tr(lang, `crash.desc.${diag.code}`);
  details.textContent = diag.details ? `${localizedDesc} (${diag.details})` : localizedDesc;

  const suggestion = document.createElement("p");
  suggestion.className = "crash-doctor-suggestion";
  suggestion.textContent = tr(lang, `crash.act.${diag.code}`);

  body.append(details, suggestion);

  if (diag.log_excerpt && diag.log_excerpt.length > 0) {
    const pre = document.createElement("pre");
    pre.className = "crash-doctor-excerpt tabular";
    pre.textContent = diag.log_excerpt.join("\n");
    body.appendChild(pre);
  }

  const actions = document.createElement("div");
  actions.className = "crash-doctor-actions";

  const addActionBtn = (label: string, primary: boolean, onClick: () => void): void => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = primary ? "btn-primary btn-sm" : "btn-secondary btn-sm";
    b.textContent = label;
    b.addEventListener("click", onClick);
    actions.appendChild(b);
  };

  if (diag.code === "out_of_memory") {
    addActionBtn(tr(lang, "crash.btnOpenRam"), true, () => {
      setActiveView("system", true, true);
      document.querySelector<HTMLButtonElement>('[data-settings-tab="game"]')?.click();
      requestAnimationFrame(() => document.getElementById("gfx-jvm-ram")?.focus());
    });
  } else if (diag.code === "mod_crash" || diag.code === "missing_mod_dependency") {
    addActionBtn(tr(lang, "crash.btnOpenMods"), true, () => {
      libActiveTab = "installed";
      libInstalledKind = "mod";
      setActiveView("library", true, true);
      void loadLibraryInstalled();
    });
  } else if (diag.code === "java_version_mismatch" || diag.code === "gpu_driver_opengl") {
    addActionBtn(tr(lang, "crash.btnOpenGame"), true, () => {
      setActiveView("system", true, true);
      document.querySelector<HTMLButtonElement>('[data-settings-tab="game"]')?.click();
    });
  } else if (diag.code === "corrupt_asset_or_jar") {
    addActionBtn(tr(lang, "crash.btnVerify"), true, () => {
      void handlePrepareVanilla();
    });
  } else if (diag.code === "session_lock") {
    addActionBtn(tr(lang, "crash.btnOpenFolder"), true, () => {
      void handleOpenGameFolder();
    });
  }

  addActionBtn(tr(lang, "diag.copyBtn"), false, () => {
    void handleCopyDiagnostics();
  });

  card.append(head, body, actions);
  host.appendChild(card);

  if (animate) {
    animatePanelReveal(card, getEffectiveMotion(), "down");
  }
}

async function handleRunCrashDoctor(): Promise<void> {
  unlockAudioContext();
  playMcSound("click");
  const lang = getEffectiveLanguage();
  try {
    const diag = await ipc.diagnoseLastCrash();
    if (!diag) {
      lastCrashDiagnosis = null;
      renderCrashDoctor(false);
      showToast(tr(lang, "crash.noCrashFound"), "neutral");
      return;
    }
    lastCrashDiagnosis = diag;
    setActiveView("play", true, false);
    renderCrashDoctor(true);
    document.getElementById("crash-doctor-host")?.scrollIntoView({ behavior: getEffectiveMotion() === "none" ? "auto" : "smooth", block: "nearest" });
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  }
}

function setWorldsSubTab(tab: "saves" | "screenshots", withSound = false): void {
  const changed = worldsActiveTab !== tab;
  worldsActiveTab = tab;
  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-worlds-tab]"))) {
    const active = btn.dataset.worldsTab === tab;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-selected", active ? "true" : "false");
  }
  const savesPanel = document.getElementById("worlds-saves-panel");
  const shotsPanel = document.getElementById("worlds-screenshots-panel");
  if (savesPanel) savesPanel.hidden = tab !== "saves";
  if (shotsPanel) shotsPanel.hidden = tab !== "screenshots";
  if (changed) {
    const incoming = tab === "screenshots" ? shotsPanel : savesPanel;
    if (incoming) animateViewHandoff(incoming, getEffectiveMotion());
    if (withSound) playMcSound("tab");
  }
  if (tab === "screenshots" && !screenshotsLoaded) {
    void loadScreenshots(false);
  } else if (tab === "screenshots") {
    renderScreenshotsGallery();
  }
  syncSlidingIndicators();
}

function syncScreenshotsCountBadges(): void {
  const lang = getEffectiveLanguage();
  const count = screenshotItems.length;
  const badge = document.getElementById("screenshots-count-badge");
  if (badge) badge.textContent = String(count);
  const toolbarCount = document.getElementById("screenshots-toolbar-count");
  if (toolbarCount) {
    toolbarCount.textContent =
      count === 1
        ? tr(lang, "gallery.countOne")
        : tr(lang, "gallery.countLabel", { count: formatLocaleNumber(count, lang, 0) });
  }
}

async function ensureScreenshotDataUrl(filename: string, imgEl?: HTMLImageElement): Promise<string | null> {
  const cached = screenshotUrlCache.get(filename);
  if (cached) {
    if (imgEl && imgEl.src !== cached) imgEl.src = cached;
    return cached;
  }
  if (screenshotUrlLoading.has(filename)) return null;
  screenshotUrlLoading.add(filename);
  try {
    const dataUrl = await ipc.readScreenshotDataUrl(filename);
    screenshotUrlCache.set(filename, dataUrl);
    if (imgEl && imgEl.isConnected) {
      imgEl.src = dataUrl;
    }
    return dataUrl;
  } catch {
    return null;
  } finally {
    screenshotUrlLoading.delete(filename);
  }
}

async function loadScreenshots(showToastOnDone = false): Promise<void> {
  try {
    screenshotItems = await ipc.listScreenshots();
    screenshotsLoaded = true;
    syncScreenshotsCountBadges();
    renderScreenshotsGallery();
    if (showToastOnDone) {
      showToast(tr(getEffectiveLanguage(), "worlds.refreshed"), "success");
    }
  } catch (err) {
    showToast(errorMessage(err), "error");
  }
}

function renderScreenshotsGallery(): void {
  const grid = document.getElementById("screenshots-grid");
  const emptyEl = document.getElementById("screenshots-empty");
  if (!grid || !emptyEl) return;
  const lang = getEffectiveLanguage();
  syncScreenshotsCountBadges();

  if (screenshotObserver) {
    screenshotObserver.disconnect();
    screenshotObserver = null;
  }
  grid.innerHTML = "";

  if (screenshotItems.length === 0) {
    emptyEl.hidden = false;
    return;
  }
  emptyEl.hidden = true;

  if (typeof IntersectionObserver !== "undefined") {
    screenshotObserver = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const img = entry.target as HTMLImageElement;
          const fname = img.dataset.shotFile;
          if (fname) {
            screenshotObserver?.unobserve(img);
            void ensureScreenshotDataUrl(fname, img);
          }
        }
      },
      { rootMargin: "120px" }
    );
  }

  screenshotItems.forEach((shot, idx) => {
    const card = document.createElement("article");
    card.className = "gallery-card";
    card.setAttribute("role", "listitem");

    const mediaBtn = document.createElement("button");
    mediaBtn.type = "button";
    mediaBtn.className = "gallery-card-media";
    mediaBtn.setAttribute("aria-label", shot.filename);
    mediaBtn.addEventListener("click", () => {
      void openLightbox(idx, mediaBtn);
    });

    const img = document.createElement("img");
    img.alt = shot.filename;
    img.dataset.shotFile = shot.filename;
    const cachedUrl = screenshotUrlCache.get(shot.filename);
    if (cachedUrl) {
      img.src = cachedUrl;
    } else if (screenshotObserver) {
      screenshotObserver.observe(img);
    } else {
      void ensureScreenshotDataUrl(shot.filename, img);
    }

    const resBadge = document.createElement("span");
    resBadge.className = "gallery-card-res";
    resBadge.textContent = `${shot.width}×${shot.height}`;

    mediaBtn.append(img, resBadge);

    const meta = document.createElement("div");
    meta.className = "gallery-card-meta";

    const info = document.createElement("div");
    info.className = "gallery-card-info";
    const nameEl = document.createElement("span");
    nameEl.className = "gallery-card-name";
    nameEl.title = shot.filename;
    nameEl.textContent = shot.filename;
    const dateEl = document.createElement("span");
    dateEl.className = "gallery-card-date";
    dateEl.textContent = `${formatEpoch(shot.modified_epoch_secs)} · ${formatBytes(shot.size_bytes)}`;
    info.append(nameEl, dateEl);

    const actions = document.createElement("div");
    actions.className = "gallery-card-actions";
    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "btn-danger btn-sm";
    delBtn.textContent = tr(lang, "gallery.deleteBtn");
    delBtn.addEventListener("click", () => {
      void handleDeleteScreenshot(shot.filename);
    });
    actions.appendChild(delBtn);

    meta.append(info, actions);
    card.append(mediaBtn, meta);
    grid.appendChild(card);
  });
}

async function renderLightboxFrame(): Promise<void> {
  if (lightboxIndex < 0 || lightboxIndex >= screenshotItems.length) return;
  const shot = screenshotItems[lightboxIndex];
  byId<HTMLElement>("lightbox-name").textContent = shot.filename;
  byId<HTMLElement>("lightbox-meta").textContent = `${shot.width}×${shot.height} · ${formatBytes(shot.size_bytes)} · ${formatEpoch(shot.modified_epoch_secs)}`;
  byId<HTMLElement>("lightbox-counter").textContent = `${lightboxIndex + 1} / ${screenshotItems.length}`;
  byId<HTMLButtonElement>("btn-lightbox-prev").disabled = screenshotItems.length <= 1;
  byId<HTMLButtonElement>("btn-lightbox-next").disabled = screenshotItems.length <= 1;

  const imgEl = byId<HTMLImageElement>("lightbox-img");
  imgEl.alt = shot.filename;
  const cached = screenshotUrlCache.get(shot.filename);
  if (cached) {
    imgEl.src = cached;
  } else {
    imgEl.removeAttribute("src");
    const loaded = await ensureScreenshotDataUrl(shot.filename);
    if (loaded && lightboxIndex >= 0 && screenshotItems[lightboxIndex]?.filename === shot.filename) {
      imgEl.src = loaded;
    }
  }
}

async function openLightbox(index: number, trigger?: HTMLElement): Promise<void> {
  if (index < 0 || index >= screenshotItems.length) return;
  lightboxIndex = index;
  lightboxTriggerEl = trigger ?? null;
  const dlg = byId<HTMLDialogElement>("gallery-lightbox");
  if (!dlg.open) {
    dlg.showModal();
    animateLightboxOpen(dlg, getEffectiveMotion());
  }
  await renderLightboxFrame();
}

function stepLightbox(delta: -1 | 1): void {
  if (screenshotItems.length <= 1 || lightboxIndex < 0) return;
  lightboxIndex = (lightboxIndex + delta + screenshotItems.length) % screenshotItems.length;
  playMcSound("focus");
  void renderLightboxFrame();
}

async function handleDeleteScreenshot(filename: string): Promise<void> {
  try {
    await ipc.deleteScreenshot(filename);
    screenshotUrlCache.delete(filename);
    screenshotItems = screenshotItems.filter((s) => s.filename !== filename);
    playMcSound("click");
    showToast(tr(getEffectiveLanguage(), "gallery.deletedToast"), "success");
    renderScreenshotsGallery();

    const dlg = document.getElementById("gallery-lightbox");
    if (dlg instanceof HTMLDialogElement && dlg.open) {
      if (screenshotItems.length === 0) {
        lightboxIndex = -1;
        dlg.close();
      } else {
        lightboxIndex = Math.min(lightboxIndex, screenshotItems.length - 1);
        await renderLightboxFrame();
      }
    }
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  }
}

async function loadWorldBackups(saveId: string): Promise<void> {
  worldBackupsLoading.add(saveId);
  renderWorldsView();
  try {
    const items = await ipc.listWorldBackups(saveId);
    worldBackupsCache.set(saveId, items);
    const matched = state?.saves.find((s) => s.id === saveId);
    if (matched) matched.backup_count = items.length;
  } catch (err) {
    showToast(errorMessage(err), "error");
  } finally {
    worldBackupsLoading.delete(saveId);
    renderWorldsView();
  }
}

function toggleWorldBackupsPanel(saveId: string): void {
  playMcSound("tab");
  if (expandedWorldBackups.has(saveId)) {
    expandedWorldBackups.delete(saveId);
    renderWorldsView();
    return;
  }
  expandedWorldBackups.add(saveId);
  renderWorldsView();
  const panel = document.querySelector<HTMLElement>(`.world-backup-panel[data-backup-save="${CSS.escape(saveId)}"]`);
  if (panel) animatePanelReveal(panel, getEffectiveMotion(), "down");
  void loadWorldBackups(saveId);
}

async function handleCreateWorldBackup(saveId: string): Promise<void> {
  if (!state || state.running_session !== null || worldBackupBusy.has(saveId)) return;
  unlockAudioContext();
  playMcSound("click");
  worldBackupBusy.add(saveId);
  expandedWorldBackups.add(saveId);
  renderWorldsView();
  try {
    const created = await ipc.createWorldBackup(saveId);
    const existing = worldBackupsCache.get(saveId) ?? [];
    const next = [created, ...existing.filter((b) => b.filename !== created.filename)];
    worldBackupsCache.set(saveId, next);
    const save = state.saves.find((s) => s.id === saveId);
    if (save) save.backup_count = next.length;
    playMcSound("launch");
    showToast(
      tr(getEffectiveLanguage(), "worlds.backupCreatedToast", {
        id: saveId,
        size: formatBytes(created.size_bytes),
      }),
      "success"
    );
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    worldBackupBusy.delete(saveId);
    renderWorldsView();
  }
}

function openRestoreBackupDialog(backup: WorldBackupSummary, trigger: HTMLElement): void {
  pendingRestoreBackup = backup;
  lastRestoreDialogTrigger = trigger;
  byId<HTMLParagraphElement>("restore-dialog-desc").textContent = tr(
    getEffectiveLanguage(),
    "worlds.restoreDesc",
    {
      id: backup.save_id,
      file: backup.filename,
      date: formatEpoch(backup.created_epoch_secs),
    }
  );
  const dialog = byId<HTMLDialogElement>("restore-backup-dialog");
  if (!dialog.open) dialog.showModal();
  animateDialogOpen(dialog, getEffectiveMotion());
  byId<HTMLButtonElement>("restore-dialog-cancel").focus({ preventScroll: true });
}

async function confirmRestoreBackup(): Promise<void> {
  if (!state || !pendingRestoreBackup) return;
  const backup = pendingRestoreBackup;
  const confirmBtn = byId<HTMLButtonElement>("restore-dialog-confirm");
  confirmBtn.disabled = true;
  worldBackupBusy.add(backup.save_id);
  try {
    const restored = await ipc.restoreWorldBackup(backup.filename);
    const idx = state.saves.findIndex((s) => s.id === restored.id);
    if (idx >= 0) {
      state.saves[idx] = restored;
    } else {
      state.saves.unshift(restored);
    }
    byId<HTMLDialogElement>("restore-backup-dialog").close();
    playMcSound("launch");
    renderPlayView();
    renderWorldsView();
    showToast(tr(getEffectiveLanguage(), "worlds.backupRestoredToast", { id: restored.id }), "success");
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    confirmBtn.disabled = false;
    worldBackupBusy.delete(backup.save_id);
    pendingRestoreBackup = null;
    renderWorldsView();
  }
}

async function handleDeleteWorldBackup(backup: WorldBackupSummary): Promise<void> {
  if (!state || state.running_session !== null) return;
  worldBackupBusy.add(backup.filename);
  renderWorldsView();
  try {
    await ipc.deleteWorldBackup(backup.filename);
    const prev = worldBackupsCache.get(backup.save_id) ?? [];
    const next = prev.filter((b) => b.filename !== backup.filename);
    worldBackupsCache.set(backup.save_id, next);
    const save = state.saves.find((s) => s.id === backup.save_id);
    if (save) save.backup_count = next.length;
    playMcSound("click");
    showToast(tr(getEffectiveLanguage(), "worlds.backupDeletedToast"), "success");
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    worldBackupBusy.delete(backup.filename);
    renderWorldsView();
  }
}

function renderWorldsView(): void {
  if (!state) return;
  const lang = getEffectiveLanguage();
  const { saves, config, vanilla } = state;
  const runningLocked = state.running_session !== null;

  const totalPlaytimePill = document.getElementById("worlds-playtime-pill");
  if (totalPlaytimePill) {
    const totalSecs = state.playtime_stats?.total_playtime_secs ?? 0;
    if (totalSecs > 0) {
      totalPlaytimePill.textContent = tr(lang, "playtime.totalBadge", {
        time: formatPlaytime(totalSecs, lang),
      });
      totalPlaytimePill.hidden = false;
    } else {
      totalPlaytimePill.hidden = true;
    }
  }
  syncScreenshotsCountBadges();

  const filterInput = document.getElementById("worlds-filter-input") as HTMLInputElement | null;
  const sortSelect = document.getElementById("worlds-sort-select") as HTMLSelectElement | null;
  const clearFilterBtn = document.getElementById("btn-clear-world-filter") as HTMLButtonElement | null;

  const trimmedQuery = worldsFilterQuery.trim();
  const foldedQuery = trimmedQuery ? toLocaleLowerFold(trimmedQuery, lang) : "";

  if (clearFilterBtn) {
    clearFilterBtn.hidden = trimmedQuery.length === 0;
  }
  if (filterInput && document.activeElement !== filterInput && filterInput.value !== worldsFilterQuery) {
    filterInput.value = worldsFilterQuery;
  }
  if (sortSelect && sortSelect.value !== worldsSortOrder) {
    sortSelect.value = worldsSortOrder;
  }

  const filteredSaves = saves
    .filter((save) => {
      if (!foldedQuery) return true;
      return toLocaleLowerFold(save.id, lang).includes(foldedQuery);
    })
    .slice()
    .sort((a, b) => {
      if (worldsSortOrder === "name") {
        return compareLocaleStrings(a.id, b.id, lang);
      }
      if (worldsSortOrder === "size") {
        return b.size_bytes - a.size_bytes;
      }
      return b.modified_epoch_secs - a.modified_epoch_secs;
    });

  byId<HTMLSpanElement>("worlds-count-label").textContent =
    trimmedQuery.length > 0
      ? tr(lang, "worlds.filteredCountLabel", {
          shown: formatLocaleNumber(filteredSaves.length, lang, 0),
          total: formatLocaleNumber(saves.length, lang, 0),
        })
      : saves.length === 1
        ? tr(lang, "worlds.countOne")
        : tr(lang, "worlds.countLabel", {
            count: formatLocaleNumber(saves.length, lang, 0),
          });

  const banner = byId<HTMLDivElement>("worlds-state-banner");
  const list = byId<HTMLDivElement>("worlds-list");
  list.innerHTML = "";

  if (saves.length === 0) {
    setBanner(
      banner,
      tr(lang, "worlds.emptyBanner", { dir: vanilla.saves_dir }),
      "neutral"
    );
    return;
  }

  if (filteredSaves.length === 0) {
    setBanner(
      banner,
      tr(lang, "worlds.zeroFilterBanner", {
        query: trimmedQuery,
        total: formatLocaleNumber(saves.length, lang, 0),
      }),
      "neutral"
    );
    return;
  }

  setBanner(banner, null);

  for (const save of filteredSaves) {
    const isActive = config.last_save_id === save.id;
    const backupCount = worldBackupsCache.get(save.id)?.length ?? save.backup_count ?? 0;
    const isBackupsOpen = expandedWorldBackups.has(save.id);
    const worldStats = state.playtime_stats?.per_world?.[save.id];

    const row = document.createElement("div");
    row.className = isActive ? "world-row is-active" : "world-row";
    row.setAttribute("role", "listitem");
    row.style.setProperty("--tile-hue", String(seedHue(save.id)));

    const thumb = document.createElement("div");
    thumb.className = save.icon_data_url
      ? "world-card-thumb"
      : "world-card-thumb world-card-thumb--fallback";
    thumb.setAttribute("aria-hidden", "true");
    if (save.icon_data_url) {
      const img = document.createElement("img");
      img.src = save.icon_data_url;
      img.alt = "";
      thumb.appendChild(img);
    } else {
      thumb.innerHTML = `<svg class="ico"><use href="#ico-cube" /></svg>`;
    }
    if (backupCount > 0) {
      const badge = document.createElement("span");
      badge.className = "world-card-thumb-badge";
      badge.textContent = String(backupCount);
      thumb.appendChild(badge);
    }

    const main = document.createElement("div");
    main.className = "row-main";

    const title = document.createElement("div");
    title.className = "row-title";
    const nameSpan = document.createElement("span");
    nameSpan.textContent = save.id;
    title.appendChild(nameSpan);

    if (isActive) {
      const tag = document.createElement("span");
      tag.className = "row-tag";
      tag.textContent = tr(lang, "worlds.selectedTag");
      title.appendChild(tag);
    }

    if (worldStats && worldStats.playtime_secs > 0) {
      const ptTag = document.createElement("span");
      ptTag.className = "row-tag tabular";
      ptTag.textContent = tr(lang, "playtime.worldBadge", {
        time: formatPlaytime(worldStats.playtime_secs, lang),
      });
      title.appendChild(ptTag);
    }

    const sub = document.createElement("div");
    sub.className = "row-sub tabular";
    sub.textContent = tr(lang, "worlds.metaLine", {
      size: formatBytes(save.size_bytes),
      date: formatEpoch(save.modified_epoch_secs),
    });

    main.append(title, sub);

    const actions = document.createElement("div");
    actions.className = "row-actions";

    const selectBtn = document.createElement("button");
    selectBtn.type = "button";
    selectBtn.className = "btn-secondary btn-sm";
    selectBtn.textContent = tr(lang, isActive ? "worlds.btnSelected" : "worlds.btnSelect");
    selectBtn.setAttribute(
      "aria-label",
      isActive ? tr(lang, "worlds.selectedTag") : tr(lang, "worlds.ariaSelectTarget", { id: save.id })
    );
    selectBtn.disabled = isActive;
    selectBtn.addEventListener("click", () => {
      void handleSelectSave(save.id);
    });

    const backupsBtn = document.createElement("button");
    backupsBtn.type = "button";
    backupsBtn.className = "btn-secondary btn-sm";
    backupsBtn.setAttribute("aria-expanded", isBackupsOpen ? "true" : "false");
    backupsBtn.textContent =
      backupCount > 0
        ? tr(lang, "worlds.btnBackupsCount", { count: backupCount })
        : tr(lang, "worlds.btnBackups");
    backupsBtn.addEventListener("click", () => {
      toggleWorldBackupsPanel(save.id);
    });

    const playBtn = document.createElement("button");
    playBtn.type = "button";
    playBtn.className = "btn-primary btn-sm";
    playBtn.textContent = tr(lang, "worlds.btnLaunch");
    playBtn.setAttribute("aria-label", tr(lang, "worlds.ariaLaunchWorld", { id: save.id }));
    playBtn.disabled = runningLocked || !vanilla.java_ready;
    playBtn.addEventListener("click", () => {
      void handleLaunchClient("singleplayer", save.id, null);
    });

    const deleteBtn = document.createElement("button");
    deleteBtn.type = "button";
    deleteBtn.className = "btn-danger btn-sm";
    deleteBtn.textContent = tr(lang, "worlds.btnDelete");
    deleteBtn.setAttribute("aria-label", tr(lang, "worlds.ariaDeleteWorld", { id: save.id }));
    deleteBtn.disabled = runningLocked;
    deleteBtn.addEventListener("click", () => {
      openDeleteDialog(save, deleteBtn);
    });

    actions.append(selectBtn, backupsBtn, playBtn, deleteBtn);
    row.append(thumb, main, actions);

    if (isBackupsOpen) {
      const panel = document.createElement("div");
      panel.className = "world-backup-panel";
      panel.dataset.backupSave = save.id;

      const head = document.createElement("div");
      head.className = "world-backup-head";

      const bTitle = document.createElement("span");
      bTitle.className = "world-backup-title";
      bTitle.innerHTML = `<svg class="ico small" aria-hidden="true"><use href="#ico-archive" /></svg>`;
      const bTitleText = document.createElement("span");
      bTitleText.textContent = tr(lang, "worlds.backupPanelTitle", { id: save.id });
      bTitle.appendChild(bTitleText);

      const createBtn = document.createElement("button");
      createBtn.type = "button";
      createBtn.className = "btn-primary btn-sm";
      const isCreating = worldBackupBusy.has(save.id);
      createBtn.disabled = runningLocked || isCreating;
      createBtn.textContent = tr(
        lang,
        isCreating ? "worlds.backupCreatingBtn" : "worlds.backupCreateBtn"
      );
      createBtn.addEventListener("click", () => {
        void handleCreateWorldBackup(save.id);
      });

      head.append(bTitle, createBtn);
      panel.appendChild(head);

      const backups = worldBackupsCache.get(save.id);
      if (worldBackupsLoading.has(save.id) && !backups) {
        const skel = document.createElement("div");
        skel.className = "skeleton skeleton-line";
        panel.appendChild(skel);
      } else if (!backups || backups.length === 0) {
        const empty = document.createElement("div");
        empty.className = "world-backup-empty";
        empty.textContent = tr(lang, "worlds.backupEmpty");
        panel.appendChild(empty);
      } else {
        const bList = document.createElement("div");
        bList.className = "world-backup-list";
        for (const b of backups) {
          const bRow = document.createElement("div");
          bRow.className = "backup-row" + (worldBackupBusy.has(b.filename) ? " is-busy" : "");

          const bMeta = document.createElement("div");
          bMeta.className = "backup-row-meta";
          const bName = document.createElement("span");
          bName.className = "backup-row-name";
          bName.title = b.filename;
          bName.textContent = b.filename;
          const bSub = document.createElement("span");
          bSub.className = "backup-row-sub";
          bSub.textContent = tr(lang, "worlds.backupRowSub", {
            date: formatEpoch(b.created_epoch_secs),
            size: formatBytes(b.size_bytes),
            files: formatLocaleNumber(b.file_count, lang, 0),
          });
          bMeta.append(bName, bSub);

          const bActs = document.createElement("div");
          bActs.className = "backup-row-actions";

          const restoreBtn = document.createElement("button");
          restoreBtn.type = "button";
          restoreBtn.className = "btn-secondary btn-sm";
          restoreBtn.disabled = runningLocked || worldBackupBusy.has(save.id);
          restoreBtn.textContent = tr(lang, "worlds.backupRestoreBtn");
          restoreBtn.addEventListener("click", () => {
            openRestoreBackupDialog(b, restoreBtn);
          });

          const delBackupBtn = document.createElement("button");
          delBackupBtn.type = "button";
          delBackupBtn.className = "btn-danger btn-sm";
          delBackupBtn.disabled = runningLocked || worldBackupBusy.has(b.filename);
          delBackupBtn.textContent = tr(lang, "worlds.backupDeleteBtn");
          delBackupBtn.addEventListener("click", () => {
            void handleDeleteWorldBackup(b);
          });

          bActs.append(restoreBtn, delBackupBtn);
          bRow.append(bMeta, bActs);
          bList.appendChild(bRow);
        }
        panel.appendChild(bList);
      }

      row.appendChild(panel);
    }

    list.appendChild(row);
  }
}

function renderProfileView(): void {
  if (!state) return;
  const lang = getEffectiveLanguage();
  const { config, skin_rgba, skin_is_slim } = state;

  const usernameInput = byId<HTMLInputElement>("profile-username");
  if (document.activeElement !== usernameInput) {
    usernameInput.value = config.username;
  }

  const effectiveTheme = getEffectiveTheme();
  const effectiveAccent = getEffectiveAccent();
  const effectiveCustomHex = getEffectiveCustomHex();
  const effectiveCorner = getEffectiveCorner();
  const effectiveLayout = getEffectiveLayout();

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]"))) {
    btn.setAttribute("aria-pressed", btn.dataset.themeChoice === effectiveTheme ? "true" : "false");
  }
  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-layout-choice]"))) {
    btn.setAttribute("aria-pressed", btn.dataset.layoutChoice === effectiveLayout ? "true" : "false");
  }
  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-accent-choice]"))) {
    btn.setAttribute("aria-pressed", btn.dataset.accentChoice === effectiveAccent ? "true" : "false");
  }
  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-corner-choice]"))) {
    const active = btn.dataset.cornerChoice === effectiveCorner;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-pressed", active ? "true" : "false");
  }
  byId<HTMLLabelElement>("pref-custom-chip").classList.toggle("is-active", effectiveAccent === "Custom");
  const colorInput = byId<HTMLInputElement>("pref-custom-accent");
  if (document.activeElement !== colorInput) {
    colorInput.value = effectiveCustomHex;
  }
  byId<HTMLSpanElement>("pref-custom-hex-label").textContent = effectiveCustomHex;
  byId<HTMLInputElement>("pref-compact").checked = config.ui_compact;
  byId<HTMLInputElement>("pref-reduced-motion").checked = config.ui_reduced_motion;
  byId<HTMLInputElement>("pref-sounds").checked = getEffectiveSounds();
  byId<HTMLInputElement>("pref-discord-rpc").checked = config.discord_rpc;
  byId<HTMLInputElement>("pref-net-boost").checked = config.network_boost;
  byId<HTMLInputElement>("pref-proc-boost").checked = config.process_boost;
  byId<HTMLSelectElement>("pref-dl-threads").value = String(config.download_threads || 24);

  const typedUsername = usernameInput.value.trim();
  const displayUuid =
    state.player_uuid && (!typedUsername || typedUsername === config.username)
      ? state.player_uuid
      : computeOfflineUuidPreview(typedUsername || config.username);
  byId<HTMLElement>("profile-uuid-preview").textContent = displayUuid;
  byId<HTMLElement>("profile-arm-summary").textContent = tr(
    lang,
    skin_is_slim ? "skin.badgeSlim" : "skin.badgeClassic"
  );

  const rpc = state.discord_rpc_status;
  const rpcBadge = byId<HTMLSpanElement>("discord-status-badge");
  if (!config.discord_rpc) {
    rpcBadge.textContent = tr(lang, "discord.statusDisabled");
  } else if (rpc?.connected) {
    rpcBadge.textContent = tr(lang, "discord.statusConnected");
  } else {
    rpcBadge.textContent = tr(lang, "discord.statusStandby");
  }

  byId<HTMLSpanElement>("skin-resolved-badge").textContent = tr(
    lang,
    skin_is_slim ? "skin.badgeSlim" : "skin.badgeClassic"
  );
  byId<HTMLSelectElement>("skin-model-select").value = config.skin_model;
  byId<HTMLParagraphElement>("skin-file-label").textContent =
    config.skin_file ?? tr(lang, "skin.defaultFileLabel");

  const skinCanvas3d = byId<HTMLCanvasElement>("skin-preview-canvas");
  const skinAtlasCanvas = document.getElementById("skin-atlas-canvas");
  if (!profileSkin3d) {
    profileSkin3d = new Skin3DViewer(skinCanvas3d, {
      interactive: true,
      trackCursor: true,
      initialView: skinViewMode === "back" ? "back" : "front",
      onClickFlip: (next) => {
        skinViewMode = next;
        playMcSound("tab");
        for (const b of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-skin-view]"))) {
          b.classList.toggle("active", b.dataset.skinView === next);
        }
        syncSlidingIndicators();
      },
      onResetPose: () => {
        playMcSound("click");
        showToast(tr(getEffectiveLanguage(), "skin.poseResetToast"), "neutral");
      },
    });
  }
  profileSkin3d.setReducedMotion(getEffectiveMotion() === "none");
  profileSkin3d.setSkin(skin_rgba, skin_is_slim);

  if (skinViewMode === "atlas" && skinAtlasCanvas instanceof HTMLCanvasElement) {
    skinCanvas3d.hidden = true;
    skinAtlasCanvas.hidden = false;
    drawSkinPreview(skinAtlasCanvas, skin_rgba, "atlas", skin_is_slim);
  } else {
    skinCanvas3d.hidden = false;
    if (skinAtlasCanvas instanceof HTMLCanvasElement) skinAtlasCanvas.hidden = true;
    profileSkin3d.setPresetView(skinViewMode === "back" ? "back" : "front");
  }

  const viewBtns = document.querySelectorAll<HTMLButtonElement>("[data-skin-view]");
  for (const btn of Array.from(viewBtns)) {
    btn.classList.toggle("active", btn.dataset.skinView === skinViewMode);
  }
  syncSlidingIndicators();
}

function renderVanillaStatus(vanilla: VanillaEnvStatus): void {
  const lang = getEffectiveLanguage();
  const ready = vanilla.java_ready && vanilla.client_ready && vanilla.assets_ready;
  byId<HTMLSpanElement>("gradle-status-tag").textContent = tr(
    lang,
    !vanilla.java_ready
      ? "system.tagMissingJava"
      : ready
      ? "system.tagReady"
      : "system.tagJavaReady"
  );
  byId<HTMLElement>("gradle-java-label").textContent = vanilla.java_ready
    ? vanilla.java_version_label
    : lang === "En"
    ? vanilla.java_version_label
    : tr(lang, "play.specJdkMissing");
  byId<HTMLElement>("gradle-jar-status").textContent = tr(
    lang,
    vanilla.client_ready && vanilla.assets_ready
      ? "system.modJarBuilt"
      : "system.modJarAuto"
  );
  byId<HTMLElement>("gradle-dir-label").textContent = vanilla.vanilla_dir;
  byId<HTMLElement>("gradle-saves-dir").textContent = vanilla.saves_dir;

  const genJava = document.getElementById("general-java-status");
  if (genJava) {
    genJava.textContent = vanilla.java_ready
      ? vanilla.java_version_label
      : tr(lang, "play.specJdkMissing");
  }
  const genCache = document.getElementById("general-cache-status");
  if (genCache) {
    genCache.textContent = tr(
      lang,
      ready ? "system.modJarBuilt" : "system.modJarAuto"
    );
  }
  const genDir = document.getElementById("general-vanilla-dir");
  if (genDir) {
    genDir.textContent = vanilla.vanilla_dir;
  }

  byId<HTMLButtonElement>("btn-system-install-java").hidden = vanilla.java_ready;
  const hwDrawer = document.getElementById("hardware-runtime-drawer");
  if (hwDrawer instanceof HTMLDetailsElement && !vanilla.java_ready) {
    hwDrawer.open = true;
  }
}

function renderServersList(): void {
  if (!state) return;
  const lang = getEffectiveLanguage();
  const list = byId<HTMLDivElement>("servers-list");
  list.innerHTML = "";
  const servers = state.config.saved_servers;
  const launchBlocked = state.running_session !== null || !state.vanilla.java_ready;

  const pingAllBtn = document.getElementById("btn-ping-all-servers");
  if (pingAllBtn instanceof HTMLButtonElement) {
    pingAllBtn.disabled = servers.length === 0 || serverPingInFlight.size > 0;
  }

  if (servers.length === 0) {
    const empty = document.createElement("p");
    empty.className = "muted-copy";
    empty.textContent = tr(lang, "servers.empty");
    list.appendChild(empty);
    return;
  }

  servers.forEach((srv: SavedServer, index: number) => {
    const check = serverChecks.get(srv.address) ?? { status: "idle" as const };

    const row = document.createElement("div");
    row.className = "server-row";

    const main = document.createElement("div");
    main.className = "row-main";

    const title = document.createElement("div");
    title.className = "row-title";

    if (check.status === "online" && check.ping?.favicon?.startsWith("data:image/png;base64,")) {
      const fav = document.createElement("img");
      fav.src = check.ping.favicon;
      fav.alt = "";
      fav.width = 24;
      fav.height = 24;
      fav.style.width = "1.5rem";
      fav.style.height = "1.5rem";
      fav.style.borderRadius = "0.3rem";
      fav.style.objectFit = "cover";
      fav.style.flexShrink = "0";
      title.appendChild(fav);
    }

    const nameEl = document.createElement("span");
    nameEl.textContent = srv.name;
    title.appendChild(nameEl);

    if (check.status !== "idle") {
      title.appendChild(createServerPingPill(check, lang));
    }

    const sub = document.createElement("div");
    sub.className = "row-sub tabular";
    if (check.status === "error" && check.message) {
      sub.textContent = `${srv.address} · ${check.message}`;
    } else if (check.status === "checking") {
      sub.textContent = tr(lang, "servers.subChecking", { addr: srv.address });
    } else {
      sub.textContent = srv.address;
    }

    main.append(title, sub);

    if (check.status === "online" && check.ping) {
      const motd = document.createElement("div");
      motd.className = "server-motd";
      if (check.ping.version_name) {
        const verTag = document.createElement("span");
        verTag.className = "server-motd-version";
        verTag.textContent = check.ping.version_name;
        motd.appendChild(verTag);
      }
      const descNode = document.createTextNode(check.ping.description || srv.address);
      motd.appendChild(descNode);
      main.appendChild(motd);
    }

    const actions = document.createElement("div");
    actions.className = "row-actions";

    const pingBtn = document.createElement("button");
    pingBtn.type = "button";
    pingBtn.className = "btn-secondary btn-sm";
    pingBtn.textContent = tr(
      lang,
      check.status === "checking" ? "servers.btnChecking" : "servers.btnPing"
    );
    pingBtn.setAttribute(
      "aria-label",
      tr(lang, "servers.ariaPing", { name: srv.name, addr: srv.address })
    );
    pingBtn.disabled = check.status === "checking";
    pingBtn.addEventListener("click", () => {
      void handlePingServer(srv.address);
    });

    const probeBtn = document.createElement("button");
    probeBtn.type = "button";
    probeBtn.className = "btn-secondary btn-sm";
    probeBtn.textContent = tr(lang, "servers.btnProbe");
    probeBtn.setAttribute(
      "aria-label",
      tr(lang, "servers.ariaProbe", { name: srv.name, addr: srv.address })
    );
    probeBtn.addEventListener("click", () => {
      void handleProbeServer(srv.address);
    });

    const connectBtn = document.createElement("button");
    connectBtn.type = "button";
    connectBtn.className = "btn-primary btn-sm";
    connectBtn.textContent = tr(lang, "servers.btnConnect");
    connectBtn.setAttribute(
      "aria-label",
      tr(lang, "servers.ariaConnect", { name: srv.name, addr: srv.address })
    );
    connectBtn.disabled = launchBlocked;
    connectBtn.addEventListener("click", () => {
      void handleLaunchClient("multiplayer", null, srv.address);
    });

    const removeBtn = document.createElement("button");
    removeBtn.type = "button";
    removeBtn.className = "btn-danger btn-sm";
    removeBtn.textContent = tr(lang, "servers.btnRemove");
    removeBtn.setAttribute(
      "aria-label",
      tr(lang, "servers.ariaRemove", { name: srv.name, addr: srv.address })
    );
    removeBtn.addEventListener("click", () => {
      void handleRemoveServer(index);
    });

    actions.append(pingBtn, probeBtn, connectBtn, removeBtn);
    row.append(main, actions);
    list.appendChild(row);
  });
  renderRailTargets();
}

function renderSystemView(): void {
  if (!state) return;
  const lang = getEffectiveLanguage();
  const { hardware, config, vanilla, hardware_notice } = state;
  syncVersionUi();

  setBanner(byId<HTMLDivElement>("hardware-optimize-notice"), hardware_notice, "neutral");

  const tierLabel = GPU_TIER_LABELS[lang][hardware.gpu_device_type];
  byId<HTMLSpanElement>("hw-tier-tag").textContent = tierLabel;
  byId<HTMLElement>("hw-cpu").textContent = hardware.cpu_name;
  byId<HTMLElement>("hw-cores").textContent = tr(lang, "system.coresVal", {
    phys: formatLocaleNumber(hardware.physical_cores, lang, 0),
    log: formatLocaleNumber(hardware.logical_cores, lang, 0),
  });
  byId<HTMLElement>("hw-ram").textContent = `${formatLocaleNumber(
    hardware.available_ram_mb,
    lang,
    0
  )} MB / ${formatLocaleNumber(hardware.total_ram_mb, lang, 0)} MB`;
  byId<HTMLElement>("hw-gpu").textContent = `${hardware.gpu_name} (${hardware.gpu_backend})`;

  const gfx = config.graphics;
  byId<HTMLSpanElement>("gfx-preset-badge").textContent = tr(
    lang,
    gfx.auto_configured && gfx.preset !== "Custom" ? "system.presetAuto" : "system.presetManual",
    { preset: gfx.preset }
  );

  byId<HTMLInputElement>("gfx-render-dist").value = String(gfx.render_distance);
  byId<HTMLOutputElement>("gfx-render-dist-out").textContent = String(gfx.render_distance);

  byId<HTMLInputElement>("gfx-sim-dist").value = String(gfx.simulation_distance);
  byId<HTMLOutputElement>("gfx-sim-dist-out").textContent = String(gfx.simulation_distance);

  byId<HTMLInputElement>("gfx-fov").value = String(gfx.fov_degrees);
  byId<HTMLOutputElement>("gfx-fov-out").textContent = `${gfx.fov_degrees}°`;

  setSelectNumericValue(byId<HTMLSelectElement>("gfx-jvm-ram"), gfx.jvm_ram_mb, "MB");
  byId<HTMLSelectElement>("gfx-jvm-gc").value = gfx.jvm_gc;
  setSelectNumericValue(byId<HTMLSelectElement>("gfx-max-fps"), gfx.max_fps, "FPS");
  byId<HTMLInputElement>("gfx-vsync").checked = gfx.vsync;
  byId<HTMLInputElement>("gfx-fancy").checked = gfx.fancy_graphics;
  byId<HTMLInputElement>("gfx-ao").checked = gfx.smooth_lighting;

  renderVanillaStatus(vanilla);
  renderServersList();
}

function formatDownloadsCompact(count: number): string {
  if (count >= 1_000_000) return `${(count / 1_000_000).toFixed(1)}M`;
  if (count >= 1_000) return `${(count / 1_000).toFixed(1)}K`;
  return String(count);
}

function getEffectiveCatalogLoader(): LoaderKind {
  if (libTargetLoader === "Vanilla") return "Fabric";
  return libTargetLoader;
}

function syncLibraryControls(): void {
  if (!state) return;
  const lang = getEffectiveLanguage();
  const counts = state.library_counts;
  const totalInstalled =
    (counts?.modsTotal ?? 0) + (counts?.shadersTotal ?? 0) + (counts?.resourcepacksTotal ?? 0);

  const badgeEl = document.getElementById("lib-installed-badge");
  if (badgeEl) badgeEl.textContent = String(totalInstalled);

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>(".lib-tab[data-lib-tab]"))) {
    const active = btn.dataset.libTab === libActiveTab;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-selected", active ? "true" : "false");
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-lib-loader]"))) {
    const active = btn.dataset.libLoader === libTargetLoader;
    btn.classList.toggle("active", active);
    btn.setAttribute("aria-pressed", active ? "true" : "false");
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-installed-kind]"))) {
    const active = btn.dataset.installedKind === libInstalledKind;
    btn.classList.toggle("active", active);
  }

  const runningLocked = state.running_session !== null;
  for (const id of ["btn-kit-fabric-shaders", "btn-kit-forge-shaders", "btn-import-jar", "btn-import-zip"]) {
    const btn = document.getElementById(id);
    if (btn instanceof HTMLButtonElement) btn.disabled = runningLocked || libImportBusy;
  }

  const importZipLabel = document.getElementById("lib-import-zip-label");
  if (importZipLabel) {
    importZipLabel.textContent = tr(
      lang,
      libActiveTab === "resourcePack" ? "library.importPackZipBtn" : "library.importShaderZipBtn"
    );
  }

  const checkUpdBtn = document.getElementById("btn-lib-check-updates");
  const checkUpdLabel = document.getElementById("lib-check-updates-label");
  if (checkUpdBtn instanceof HTMLButtonElement && checkUpdLabel) {
    checkUpdBtn.disabled = runningLocked || libUpdatesChecking || libUpdatingAll;
    checkUpdLabel.textContent = tr(
      lang,
      libUpdatesChecking ? "library.checkingUpdatesBtn" : "library.checkUpdatesBtn"
    );
  }
  const updateAllBtn = document.getElementById("btn-lib-update-all");
  if (updateAllBtn instanceof HTMLButtonElement) {
    const scopePrefix = `${libInstalledKind}:${getEffectiveCatalogLoader()}:`;
    let pendingCount = 0;
    for (const key of installedUpdates.keys()) {
      if (key.startsWith(scopePrefix)) pendingCount += 1;
    }
    updateAllBtn.hidden = pendingCount === 0;
    updateAllBtn.disabled = runningLocked || libUpdatingAll || libUpdatesChecking;
    if (pendingCount > 0) {
      updateAllBtn.textContent = libUpdatingAll
        ? tr(lang, "library.updatingBtn")
        : tr(lang, "library.updateAllBtn", { count: pendingCount });
    }
  }

  const prepareBtn = document.getElementById("btn-lib-prepare-loader");
  const prepareLabel = document.getElementById("lib-prepare-label");
  if (prepareBtn instanceof HTMLButtonElement && prepareLabel) {
    const loader = getEffectiveCatalogLoader();
    const ready = loader === "Fabric" ? state.loader_status?.fabricReady : state.loader_status?.forgeReady;
    prepareBtn.disabled = state.running_session !== null;
    prepareLabel.textContent = ready
      ? tr(lang, "loader.ready", { loader })
      : tr(lang, "loader.prepareBtn", { loader });
  }

  const searchInput = document.getElementById("library-search-input");
  if (searchInput instanceof HTMLInputElement) {
    const kindLabel =
      libActiveTab === "shader"
        ? tr(lang, "library.tabShaders")
        : libActiveTab === "resourcePack"
        ? tr(lang, "library.tabPacks")
        : tr(lang, "library.tabMods");
    searchInput.placeholder = tr(lang, "library.searchPlaceholder", { kind: kindLabel });
  }
  syncSlidingIndicators();

  const isInstalledTab = libActiveTab === "installed";
  const kitGrid = document.getElementById("library-kit-grid");
  const searchBar = document.getElementById("library-search-bar");
  const catalogGrid = document.getElementById("library-catalog-grid");
  const installedPanel = document.getElementById("library-installed-panel");
  if (kitGrid) kitGrid.hidden = isInstalledTab;
  if (searchBar) searchBar.hidden = isInstalledTab;
  if (catalogGrid) catalogGrid.hidden = isInstalledTab;
  if (installedPanel) installedPanel.hidden = !isInstalledTab;
}

function renderLibraryCatalog(): void {
  const grid = document.getElementById("library-catalog-grid");
  const countEl = document.getElementById("library-results-count");
  if (!grid || !state) return;
  const lang = getEffectiveLanguage();
  grid.innerHTML = "";

  if (countEl) {
    countEl.textContent = libCatalogLoading
      ? ""
      : tr(lang, "library.totalHits", { count: formatLocaleNumber(libTotalHits, lang) });
  }

  if (libCatalogLoading) {
    const loading = document.createElement("div");
    loading.className = "library-empty-state";
    loading.textContent = tr(lang, "library.loading");
    grid.appendChild(loading);
    return;
  }

  if (libCatalogError) {
    const errBox = document.createElement("div");
    errBox.className = "library-empty-state";
    errBox.textContent = tr(lang, "library.offlineCatalog", { err: libCatalogError });
    grid.appendChild(errBox);
    return;
  }

  if (libCatalogHits.length === 0) {
    const empty = document.createElement("div");
    empty.className = "library-empty-state";
    empty.textContent = tr(lang, "library.emptyCatalog", { version: state.config.mc_version });
    grid.appendChild(empty);
    return;
  }

  const activeKind: LibraryKind = libActiveTab === "installed" ? "mod" : libActiveTab;
  const sessionRunning = state.running_session !== null;
  for (const hit of libCatalogHits) {
    const card = document.createElement("article");
    card.className = "lib-card";

    const top = document.createElement("div");
    top.className = "lib-card-head";

    const thumbWrap = document.createElement("div");
    thumbWrap.className = "lib-card-icon";
    if (hit.iconUrl) {
      const img = document.createElement("img");
      img.src = hit.iconUrl;
      img.alt = "";
      img.loading = "lazy";
      img.onerror = () => {
        img.remove();
        thumbWrap.textContent = hit.title.slice(0, 2).toUpperCase();
      };
      thumbWrap.appendChild(img);
    } else {
      thumbWrap.textContent = hit.title.slice(0, 2).toUpperCase();
    }

    const metaWrap = document.createElement("div");
    metaWrap.className = "lib-card-title-wrap";
    const title = document.createElement("h3");
    title.className = "lib-card-title";
    title.textContent = hit.title;
    const author = document.createElement("span");
    author.className = "lib-card-author";
    author.textContent = `${hit.author || "Modrinth"} · ${tr(lang, "library.downloadsLabel", {
      count: formatDownloadsCompact(hit.downloads),
    })}`;
    metaWrap.append(title, author);

    top.append(thumbWrap, metaWrap);

    const desc = document.createElement("p");
    desc.className = "lib-card-desc";
    desc.textContent = hit.description || "";

    const foot = document.createElement("div");
    foot.className = "lib-card-foot";

    const tags = document.createElement("div");
    tags.className = "lib-card-tags";
    for (const cat of hit.categories.slice(0, 2)) {
      const tag = document.createElement("span");
      tag.className = "lib-cat-pill";
      tag.textContent = cat;
      tags.appendChild(tag);
    }

    const installBtn = document.createElement("button");
    installBtn.type = "button";
    const isBusy = installingProjects.has(hit.projectId);
    installBtn.className = hit.installed ? "btn-secondary btn-sm is-installed" : "btn-primary btn-sm";
    installBtn.disabled = isBusy || hit.installed || sessionRunning;
    installBtn.textContent = isBusy
      ? tr(lang, "library.installingBtn")
      : hit.installed
      ? tr(lang, "library.installedBadge")
      : tr(lang, "library.installBtn");
    installBtn.addEventListener("click", () => {
      void handleInstallCatalogItem(hit, activeKind);
    });

    foot.append(tags, installBtn);
    card.append(top, desc, foot);
    grid.appendChild(card);
  }
}

function modUpdateKey(kind: LibraryKind, loader: LoaderKind, fileName: string): string {
  return `${kind}:${loader}:${fileName}`;
}

async function handleCheckLibraryUpdates(): Promise<void> {
  if (!state || state.running_session !== null || libUpdatesChecking) return;
  unlockAudioContext();
  playMcSound("click");
  const kind = libInstalledKind;
  const loader = getEffectiveCatalogLoader();
  const scopePrefix = `${kind}:${loader}:`;
  libUpdatesChecking = true;
  syncLibraryControls();
  try {
    const updates = await ipc.checkLibraryUpdates(kind, loader);
    for (const k of Array.from(installedUpdates.keys())) {
      if (k.startsWith(scopePrefix)) installedUpdates.delete(k);
    }
    for (const upd of updates) {
      installedUpdates.set(modUpdateKey(kind, loader, upd.filename), upd);
    }
    libUpdatesCheckedScope = `${kind}:${loader}:${state.config.mc_version}`;
    const lang = getEffectiveLanguage();
    if (updates.length > 0) {
      playMcSound("launch");
      showToast(tr(lang, "library.updatesFoundToast", { count: updates.length }), "success");
    } else {
      showToast(tr(lang, "library.updatesUpToDateToast"), "neutral");
    }
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    libUpdatesChecking = false;
    syncLibraryControls();
    renderLibraryInstalled();
  }
}

async function handleUpdateSingleMod(item: InstalledLibraryItem, upd: ModUpdateInfo): Promise<void> {
  if (!state || state.running_session !== null || updatingModFiles.has(item.fileName)) return;
  unlockAudioContext();
  playMcSound("click");
  const loader = getEffectiveCatalogLoader();
  const key = modUpdateKey(item.kind, loader, item.fileName);
  updatingModFiles.add(item.fileName);
  renderLibraryInstalled();
  try {
    const res = await ipc.installModrinthItem({
      projectId: upd.project_id,
      kind: item.kind,
      loader,
    });
    if (res.loaderStatus.mcVersion === state.config.mc_version) {
      state.loader_status = res.loaderStatus;
    }
    state.library_counts = res.libraryCounts;
    installedUpdates.delete(key);
    playMcSound("launch");
    showToast(
      tr(getEffectiveLanguage(), "library.updatedOneToast", {
        title: upd.title || item.displayName,
        ver: upd.latest_version,
      }),
      "success"
    );
    await loadLibraryInstalled();
    renderPlayView();
    syncLibraryControls();
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    updatingModFiles.delete(item.fileName);
    syncLibraryControls();
    renderLibraryInstalled();
  }
}

async function handleUpdateAllMods(): Promise<void> {
  if (!state || state.running_session !== null || libUpdatingAll) return;
  const kind = libInstalledKind;
  const loader = getEffectiveCatalogLoader();
  const scopePrefix = `${kind}:${loader}:`;
  const entries = Array.from(installedUpdates.entries()).filter(([k]) => k.startsWith(scopePrefix));
  if (entries.length === 0) return;

  unlockAudioContext();
  playMcSound("click");
  libUpdatingAll = true;
  syncLibraryControls();
  let updatedCount = 0;
  try {
    for (const [key, upd] of entries) {
      updatingModFiles.add(upd.filename);
      renderLibraryInstalled();
      try {
        const res = await ipc.installModrinthItem({
          projectId: upd.project_id,
          kind,
          loader,
        });
        if (res.loaderStatus.mcVersion === state.config.mc_version) {
          state.loader_status = res.loaderStatus;
        }
        state.library_counts = res.libraryCounts;
        installedUpdates.delete(key);
        updatedCount += 1;
      } catch (err) {
        showToast(errorMessage(err), "error");
      } finally {
        updatingModFiles.delete(upd.filename);
      }
    }
    if (updatedCount > 0) {
      playMcSound("launch");
      showToast(tr(getEffectiveLanguage(), "library.updatedAllToast", { count: updatedCount }), "success");
    }
    await loadLibraryInstalled();
    renderPlayView();
  } finally {
    libUpdatingAll = false;
    syncLibraryControls();
    renderLibraryInstalled();
  }
}

function renderLibraryInstalled(): void {
  const listEl = document.getElementById("library-installed-list");
  if (!listEl || !state) return;
  const lang = getEffectiveLanguage();
  const loader = getEffectiveCatalogLoader();
  const runningLocked = state.running_session !== null;
  const currentScope = `${libInstalledKind}:${loader}:${state.config.mc_version}`;
  const checkedThisScope = libUpdatesCheckedScope === currentScope;
  listEl.innerHTML = "";

  if (libInstalledItems.length === 0) {
    const empty = document.createElement("div");
    empty.className = "library-empty-state";
    empty.textContent = tr(lang, "library.emptyInstalled");
    listEl.appendChild(empty);
    return;
  }

  for (const item of libInstalledItems) {
    const row = document.createElement("div");
    row.className = "lib-installed-row" + (item.enabled ? "" : " is-disabled");

    const info = document.createElement("div");
    info.className = "lib-installed-info";
    const name = document.createElement("strong");
    name.textContent = item.displayName;
    const sub = document.createElement("span");
    sub.className = "tabular";
    sub.textContent = `${item.fileName} · ${formatBytes(item.sizeBytes)}`;
    info.append(name, sub);

    const actions = document.createElement("div");
    actions.className = "lib-installed-actions";

    const upd = installedUpdates.get(modUpdateKey(item.kind, loader, item.fileName));
    if (upd) {
      const updBtn = document.createElement("button");
      updBtn.type = "button";
      updBtn.className = "mod-update-pill";
      const isBusy = updatingModFiles.has(item.fileName);
      if (isBusy) updBtn.dataset.state = "updating";
      updBtn.disabled = runningLocked || isBusy || libUpdatingAll;
      updBtn.textContent = isBusy
        ? tr(lang, "library.updatingBtn")
        : tr(lang, "library.updateBtn", { ver: upd.latest_version });
      updBtn.addEventListener("click", () => {
        void handleUpdateSingleMod(item, upd);
      });
      actions.appendChild(updBtn);
    } else if (checkedThisScope && item.projectId) {
      const latestPill = document.createElement("span");
      latestPill.className = "mod-update-pill mod-update-pill--latest";
      latestPill.textContent = tr(lang, "library.upToDateBadge");
      actions.appendChild(latestPill);
    }

    const statusLabel = document.createElement("span");
    statusLabel.className = "lib-state-label";
    statusLabel.textContent = tr(lang, item.enabled ? "library.enableAction" : "library.disableAction");

    const toggle = document.createElement("input");
    toggle.type = "checkbox";
    toggle.className = "switch";
    toggle.checked = item.enabled;
    toggle.disabled = runningLocked || libUpdatingAll;
    toggle.setAttribute("aria-label", item.fileName);
    toggle.addEventListener("change", () => {
      void handleToggleInstalledItem(item);
    });

    const delBtn = document.createElement("button");
    delBtn.type = "button";
    delBtn.className = "btn-danger btn-sm";
    delBtn.disabled = runningLocked || libUpdatingAll;
    delBtn.textContent = tr(lang, "library.deleteAction");
    delBtn.addEventListener("click", () => {
      void handleDeleteInstalledItem(item);
    });

    actions.append(statusLabel, toggle, delBtn);
    row.append(info, actions);
    listEl.appendChild(row);
  }
}

function renderLibraryView(): void {
  if (!state) return;
  syncLibraryControls();
  if (libActiveTab === "installed") {
    renderLibraryInstalled();
  } else {
    renderLibraryCatalog();
  }
}

function buildCurrentCatalogKey(): string {
  if (!state) return "";
  const queryEl = document.getElementById("library-search-input");
  const sortEl = document.getElementById("library-sort-select");
  const query = queryEl instanceof HTMLInputElement ? queryEl.value.trim() : "";
  const sort = sortEl instanceof HTMLSelectElement ? sortEl.value : "downloads";
  const loader = getEffectiveCatalogLoader();
  return `${libActiveTab}:${loader}:${state.config.mc_version}:${query}:${sort}`;
}

async function loadLibraryCatalog(force = false): Promise<void> {
  if (!state || libActiveTab === "installed") return;
  const requestedTab = libActiveTab;
  const queryEl = document.getElementById("library-search-input");
  const sortEl = document.getElementById("library-sort-select");
  const query = queryEl instanceof HTMLInputElement ? queryEl.value.trim() : "";
  const sort = sortEl instanceof HTMLSelectElement ? sortEl.value : "downloads";
  const loader = getEffectiveCatalogLoader();
  const key = `${requestedTab}:${loader}:${state.config.mc_version}:${query}:${sort}`;
  if (!force && libLastCatalogKey === key && libCatalogHits.length > 0 && !libCatalogError) return;

  const seq = ++libCatalogSeq;
  libCatalogLoading = true;
  libCatalogError = null;
  renderLibraryCatalog();
  try {
    const res = await ipc.searchModrinthCatalog({
      query,
      kind: requestedTab,
      loader,
      sort,
      limit: 24,
      category: null,
    });
    if (seq !== libCatalogSeq) return;
    libCatalogHits = res.hits;
    libTotalHits = res.totalHits;
    libLastCatalogKey = key;
    libCatalogError = null;
  } catch (err) {
    if (seq !== libCatalogSeq) return;
    const msg = errorMessage(err);
    libCatalogError = msg;
    showToast(msg, "error");
    libCatalogHits = [];
    libTotalHits = 0;
  } finally {
    if (seq === libCatalogSeq) {
      libCatalogLoading = false;
      if (libActiveTab === requestedTab) {
        renderLibraryCatalog();
      }
    }
  }
}

async function loadLibraryInstalled(): Promise<void> {
  if (!state) return;
  const seq = ++libInstalledSeq;
  try {
    const items = await ipc.listLibraryItems(libInstalledKind, getEffectiveCatalogLoader());
    if (seq !== libInstalledSeq) return;
    libInstalledItems = items;
  } catch (err) {
    if (seq !== libInstalledSeq) return;
    showToast(errorMessage(err), "error");
    libInstalledItems = [];
  }
  if (seq === libInstalledSeq) {
    renderLibraryInstalled();
  }
}

async function handleLoaderChange(loader: LoaderKind): Promise<void> {
  if (!state) return;
  unlockAudioContext();
  playMcSound("click");
  const seq = ++loaderSwitchSeq;
  try {
    const res = await ipc.setModLoader(loader);
    if (seq !== loaderSwitchSeq) return;
    state.config = res.config;
    state.vanilla = res.vanilla;
    state.loader_status = res.loader_status;
    state.library_counts = res.library_counts;
    if (res.discord_rpc_status) state.discord_rpc_status = res.discord_rpc_status;
    if (loader === "Fabric" || loader === "Forge") {
      libTargetLoader = loader;
      libLastCatalogKey = "";
    }
    renderTopbarAndSession();
    renderPlayView();
    renderLibraryView();
    const strata = document.getElementById("stage-strata");
    if (strata instanceof SVGSVGElement) pulseStrata(strata);
    showToast(tr(getEffectiveLanguage(), "loader.switchedToast", { loader }), "success");
  } catch (err) {
    if (seq !== loaderSwitchSeq) return;
    playMcSound("error");
    showToast(errorMessage(err), "error");
  }
}

async function handleInstallCatalogItem(hit: ModrinthSearchHit, kind: LibraryKind): Promise<void> {
  if (!state || state.running_session !== null) return;
  unlockAudioContext();
  playMcSound("click");
  const targetLoader = getEffectiveCatalogLoader();
  installingProjects.add(hit.projectId);
  renderLibraryCatalog();
  try {
    const res = await ipc.installModrinthItem({
      projectId: hit.projectId,
      kind,
      loader: targetLoader,
    });
    if (res.loaderStatus.mcVersion === state.config.mc_version) {
      state.loader_status = res.loaderStatus;
    }
    state.library_counts = res.libraryCounts;
    hit.installed = true;
    const msg =
      res.outcome.installedFiles.length > 0
        ? tr(getEffectiveLanguage(), "library.installToast", {
            count: res.outcome.installedFiles.length,
          })
        : tr(getEffectiveLanguage(), "library.alreadyToast");
    playMcSound("launch");
    showToast(msg, "success");
    renderPlayView();
    syncLibraryControls();
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    installingProjects.delete(hit.projectId);
    renderLibraryCatalog();
  }
}

async function handleInstallShaderBundle(presetId: "fabric_shaders" | "forge_shaders"): Promise<void> {
  if (!state || state.running_session !== null) return;
  unlockAudioContext();
  playMcSound("click");
  const fabBtn = byId<HTMLButtonElement>("btn-kit-fabric-shaders");
  const forgeBtn = byId<HTMLButtonElement>("btn-kit-forge-shaders");
  fabBtn.disabled = true;
  forgeBtn.disabled = true;
  try {
    const res = await ipc.installShaderBundle(presetId);
    state.config = res.config;
    state.loader_status = res.loaderStatus;
    state.library_counts = res.libraryCounts;
    libTargetLoader = res.config.mod_loader === "Forge" ? "Forge" : "Fabric";
    libLastCatalogKey = "";
    playMcSound("launch");
    showToast(
      tr(getEffectiveLanguage(), "library.bundleToast", {
        loader: res.config.mod_loader,
        count: res.outcome.installedFiles.length,
      }),
      "success"
    );
    renderTopbarAndSession();
    renderPlayView();
    renderLibraryView();
    void loadLibraryCatalog(true);
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    syncLibraryControls();
  }
}

async function handleImportLibraryFile(kind: LibraryKind): Promise<void> {
  if (!state || libImportBusy || state.running_session !== null) return;
  unlockAudioContext();
  playMcSound("click");
  libImportBusy = true;
  syncLibraryControls();
  try {
    const res = await ipc.pickAndImportLibraryFile(kind, getEffectiveCatalogLoader());
    if (!res) return;
    state.library_counts = res.libraryCounts;
    playMcSound("launch");
    showToast(tr(getEffectiveLanguage(), "library.importToast", { file: res.fileName }), "success");
    renderPlayView();
    renderLibraryView();
    if (libActiveTab === "installed") void loadLibraryInstalled();
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    libImportBusy = false;
    syncLibraryControls();
  }
}

async function handleToggleInstalledItem(item: InstalledLibraryItem): Promise<void> {
  if (!state || state.running_session !== null) return;
  try {
    const counts = await ipc.toggleLibraryItem(
      item.kind,
      getEffectiveCatalogLoader(),
      item.fileName
    );
    state.library_counts = counts;
    playMcSound("click");
    showToast(tr(getEffectiveLanguage(), "library.toggleToast", { file: item.fileName }), "success");
    await loadLibraryInstalled();
    renderPlayView();
    syncLibraryControls();
  } catch (err) {
    showToast(errorMessage(err), "error");
    await loadLibraryInstalled();
  }
}

async function handleDeleteInstalledItem(item: InstalledLibraryItem): Promise<void> {
  if (!state || state.running_session !== null) return;
  try {
    const counts = await ipc.deleteLibraryItem(
      item.kind,
      getEffectiveCatalogLoader(),
      item.fileName
    );
    state.library_counts = counts;
    playMcSound("click");
    showToast(tr(getEffectiveLanguage(), "library.deleteToast", { file: item.fileName }), "success");
    await loadLibraryInstalled();
    renderPlayView();
    syncLibraryControls();
  } catch (err) {
    showToast(errorMessage(err), "error");
  }
}

async function handlePrepareLoader(): Promise<void> {
  if (!state) return;
  unlockAudioContext();
  playMcSound("click");
  const loader = getEffectiveCatalogLoader();
  try {
    const session = await ipc.prepareLoaderEnvironment(loader);
    if (session.pid > 0) {
      state.running_session = session;
      renderTopbarAndSession();
      renderPlayView();
      syncLibraryControls();
    } else {
      showToast(session.label, "success");
    }
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  }
}

function renderAll(): void {
  if (!state) return;
  if (state.config.mod_loader === "Fabric" || state.config.mod_loader === "Forge") {
    libTargetLoader = state.config.mod_loader;
  }
  applyPreferences();
  renderTopbarAndSession();
  renderPlayView();
  renderCrashDoctor(false);
  renderLibraryView();
  renderWorldsView();
  if (screenshotsLoaded) renderScreenshotsGallery();
  renderProfileView();
  renderSystemView();
}

async function handleSelectSave(saveId: string | null): Promise<void> {
  if (!state) return;
  try {
    await ipc.selectSave(saveId);
    state.config.last_save_id = saveId;
    renderPlayView();
    renderWorldsView();
    renderTopbarAndSession();
  } catch (err) {
    showToast(errorMessage(err), "error");
  }
}

async function handleVersionChange(version: string): Promise<void> {
  if (!state) return;
  unlockAudioContext();
  playMcSound("click");
  const seq = ++versionSwitchSeq;
  try {
    const res = await ipc.setMcVersion(version);
    if (seq !== versionSwitchSeq) return;
    state.config = res.config;
    state.vanilla = res.vanilla;
    state.loader_status = res.loader_status;
    state.library_counts = res.library_counts;
    libLastCatalogKey = "";
    libCatalogError = null;
    if (res.discord_rpc_status) {
      state.discord_rpc_status = res.discord_rpc_status;
    }
    renderTopbarAndSession();
    renderPlayView();
    renderLibraryView();
    renderProfileView();
    renderSystemView();
    const strata = document.getElementById("stage-strata");
    if (strata instanceof SVGSVGElement) pulseStrata(strata);
    showToast(
      tr(getEffectiveLanguage(), "system.versionChangedToast", {
        ver: state.config.mc_version,
      }),
      "success"
    );
  } catch (err) {
    if (seq !== versionSwitchSeq) return;
    playMcSound("error");
    showToast(errorMessage(err), "error");
    syncVersionUi();
  }
}

async function handleSaveTurboSettings(patch: {
  discord_rpc?: boolean;
  download_threads?: number;
  network_boost?: boolean;
  process_boost?: boolean;
}): Promise<void> {
  if (!state) return;
  unlockAudioContext();
  playMcSound("click");
  try {
    const res = await ipc.saveTurboSettings({
      discord_rpc: patch.discord_rpc ?? state.config.discord_rpc,
      download_threads: patch.download_threads ?? state.config.download_threads,
      network_boost: patch.network_boost ?? state.config.network_boost,
      process_boost: patch.process_boost ?? state.config.process_boost,
    });
    state.config = res.config;
    state.discord_rpc_status = res.discord_rpc_status;
    renderPlayView();
    renderProfileView();
    renderSystemView();
    showToast(tr(getEffectiveLanguage(), "turbo.savedToast"), "success");
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  }
}

async function handleOptimizeClientNow(): Promise<void> {
  if (!state) return;
  unlockAudioContext();
  playMcSound("launch");
  const btn = byId<HTMLButtonElement>("btn-optimize-now");
  const summaryEl = byId<HTMLParagraphElement>("play-opt-summary");
  const pingBadge = byId<HTMLSpanElement>("play-ping-badge");
  const lang = getEffectiveLanguage();
  btn.disabled = true;
  summaryEl.textContent = tr(lang, "turbo.optimizingBtn");
  summaryEl.hidden = false;
  try {
    const report = await ipc.optimizeClientNow();
    if (report.config) {
      state.config = report.config;
      renderProfileView();
      renderPlayView();
    }
    if (report.cdn_ping_ms !== null) {
      pingBadge.textContent = `${formatLocaleNumber(report.cdn_ping_ms, lang, 0)} ms CDN`;
    } else if (report.local_ping_ms !== null) {
      pingBadge.textContent = `${formatLocaleNumber(report.local_ping_ms, lang, 0)} ms LAN`;
    } else {
      pingBadge.textContent = tr(lang, "turbo.pingOptimized");
    }
    const cdnStr =
      report.cdn_ping_ms !== null
        ? `${formatLocaleNumber(report.cdn_ping_ms, lang, 0)} ms`
        : tr(lang, "turbo.pingNa");
    const dnsStr = tr(lang, report.dns_flushed ? "turbo.dnsFlushed" : "turbo.tcpTuned");
    const summaryText = tr(lang, "turbo.optResult", {
      cdn: cdnStr,
      dns: dnsStr,
      flags: formatLocaleNumber(report.applied_flags_count, lang, 0),
    });
    summaryEl.textContent = summaryText;
    summaryEl.hidden = false;
    showToast(summaryText, "success");
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    btn.disabled = false;
  }
}

async function handleOpenGithubPage(): Promise<void> {
  unlockAudioContext();
  playMcSound("click");
  try {
    await ipc.openGithubPage();
  } catch (err) {
    showToast(errorMessage(err), "error");
  }
}

async function handleOpenGameFolder(): Promise<void> {
  unlockAudioContext();
  playMcSound("click");
  try {
    await ipc.openGameFolder();
  } catch (err) {
    showToast(errorMessage(err), "error");
  }
}

async function commitUsernameFromPlay(): Promise<boolean> {
  if (!state) return false;
  const input = byId<HTMLInputElement>("play-username");
  const errEl = byId<HTMLSpanElement>("play-username-error");
  const validation = validateUsername(input.value);
  if (validation) {
    errEl.textContent = validation;
    errEl.hidden = false;
    input.focus();
    return false;
  }
  errEl.hidden = true;
  const cleaned = input.value.trim();
  if (cleaned === state.config.username) {
    return true;
  }
  state.config = await ipc.saveProfileSettings({
    username: cleaned,
    theme: state.config.theme,
    ui_accent: state.config.ui_accent,
    custom_accent_hex: state.config.custom_accent_hex,
    ui_corner: state.config.ui_corner,
    ui_layout: state.config.ui_layout,
    ui_compact: state.config.ui_compact,
    ui_reduced_motion: state.config.ui_reduced_motion,
    ui_sounds: state.config.ui_sounds,
    ui_language: state.config.ui_language,
  });
  state.player_uuid = computeOfflineUuidPreview(state.config.username);
  renderTopbarAndSession();
  renderProfileView();
  return true;
}

async function handlePrepareVanilla(): Promise<void> {
  if (!state) return;
  logLines = [];
  const feedback = byId<HTMLDivElement>("launch-feedback");
  setBanner(feedback, null);
  try {
    const session = await ipc.prepareVanillaClient();
    state.running_session = session;
    renderTopbarAndSession();
    renderPlayView();
    renderWorldsView();
    renderServersList();
    playMcSound("launch");
    setBanner(feedback, tr(getEffectiveLanguage(), "play.prebuildStarted"), "neutral");
  } catch (err) {
    playMcSound("error");
    setBanner(feedback, errorMessage(err), "error");
    showToast(errorMessage(err), "error");
  }
}

async function handleInstallJava21(): Promise<void> {
  if (!state) return;
  logLines = [];
  const feedback = byId<HTMLDivElement>("launch-feedback");
  setActiveView("play");
  setBanner(feedback, null);
  try {
    const session = await ipc.installJava21();
    state.running_session = session;
    renderTopbarAndSession();
    renderPlayView();
    renderWorldsView();
    renderServersList();
    setBanner(feedback, tr(getEffectiveLanguage(), "play.javaInstallStarted"), "neutral");
  } catch (err) {
    setBanner(feedback, errorMessage(err), "error");
    showToast(errorMessage(err), "error");
  }
}

async function handleRecheckJava21(): Promise<void> {
  if (!state) return;
  try {
    const status = await ipc.recheckJava21();
    state.vanilla = status;
    renderTopbarAndSession();
    renderPlayView();
    renderSystemView();
    if (status.java_ready) {
      showToast(
        tr(getEffectiveLanguage(), "play.javaDetected", { ver: status.java_version_label }),
        "success"
      );
    } else {
      showToast(status.java_hint, "error");
    }
  } catch (err) {
    showToast(errorMessage(err), "error");
  }
}

async function handleOpenJavaPage(): Promise<void> {
  try {
    await ipc.openJavaDownloadPage();
  } catch (err) {
    showToast(errorMessage(err), "error");
  }
}

async function handleLaunchClient(
  mode: LaunchMode,
  saveId: string | null,
  serverAddress: string | null
): Promise<void> {
  if (!state || pendingLaunch || state.running_session !== null) return;
  pendingLaunch = true;
  const feedback = byId<HTMLDivElement>("launch-feedback");
  setBanner(feedback, null);

  try {
    if (!(await commitUsernameFromPlay())) {
      return;
    }

    logLines = [];
    const session = await ipc.launchClient({
      mode,
      save_id: saveId,
      server_address: serverAddress,
    });
    state.running_session = session;
    setActiveView("play");
    renderTopbarAndSession();
    renderPlayView();
    renderWorldsView();
    renderServersList();
    playMcSound("launch");
    setBanner(
      feedback,
      tr(getEffectiveLanguage(), "play.launchingNote", {
        label: session.label,
        pid: session.pid,
      }),
      "neutral"
    );
  } catch (err) {
    setActiveView("play");
    playMcSound("error");
    const msg = errorMessage(err);
    setBanner(feedback, msg, "error");
    showToast(msg, "error");
  } finally {
    pendingLaunch = false;
  }
}

async function handleRefreshSaves(): Promise<void> {
  if (!state) return;
  const btn = byId<HTMLButtonElement>("btn-refresh-worlds");
  btn.disabled = true;
  try {
    state.saves = await ipc.listSaves();
    renderPlayView();
    renderWorldsView();
    showToast(tr(getEffectiveLanguage(), "worlds.refreshed"), "success");
  } catch (err) {
    setBanner(byId<HTMLDivElement>("worlds-state-banner"), errorMessage(err), "error");
  } finally {
    btn.disabled = false;
  }
}

function openDeleteDialog(save: MinecraftSaveSummary, trigger: HTMLElement): void {
  pendingDeleteSaveId = save.id;
  lastDialogTrigger = trigger;
  const savesRoot = state?.vanilla.saves_dir ?? "run/saves";
  byId<HTMLParagraphElement>("delete-dialog-desc").textContent = tr(
    getEffectiveLanguage(),
    "worlds.deleteDesc",
    {
      id: save.id,
      size: formatBytes(save.size_bytes),
      root: savesRoot,
    }
  );
  const dialog = byId<HTMLDialogElement>("delete-world-dialog");
  if (!dialog.open) {
    dialog.showModal();
  }
  animateDialogOpen(dialog, getEffectiveMotion());
  byId<HTMLButtonElement>("delete-dialog-cancel").focus({ preventScroll: true });
}

async function confirmDeleteSave(): Promise<void> {
  if (!state || !pendingDeleteSaveId) return;
  const saveId = pendingDeleteSaveId;
  const confirmBtn = byId<HTMLButtonElement>("delete-dialog-confirm");
  confirmBtn.disabled = true;
  try {
    state.saves = await ipc.deleteSave(saveId);
    if (state.config.last_save_id === saveId) {
      const fallback = state.saves[0]?.id ?? null;
      await ipc.selectSave(fallback);
      state.config.last_save_id = fallback;
    }
    byId<HTMLDialogElement>("delete-world-dialog").close();
    renderPlayView();
    renderWorldsView();
    renderTopbarAndSession();
    showToast(tr(getEffectiveLanguage(), "worlds.deletedToast", { id: saveId }), "success");
  } catch (err) {
    showToast(errorMessage(err), "error");
  } finally {
    confirmBtn.disabled = false;
    pendingDeleteSaveId = null;
  }
}

async function handleSaveProfileForm(e: SubmitEvent): Promise<void> {
  e.preventDefault();
  if (!state || pendingSaveProfile) return;
  const input = byId<HTMLInputElement>("profile-username");
  const errEl = byId<HTMLSpanElement>("profile-username-error");
  const statusEl = byId<HTMLSpanElement>("profile-save-status");
  const saveBtn = byId<HTMLButtonElement>("btn-save-profile");

  const validation = validateUsername(input.value);
  if (validation) {
    errEl.textContent = validation;
    errEl.hidden = false;
    statusEl.textContent = "";
    input.focus();
    return;
  }
  errEl.hidden = true;
  pendingSaveProfile = true;
  saveBtn.disabled = true;

  try {
    state.config = await ipc.saveProfileSettings({
      username: input.value.trim(),
      theme: state.config.theme,
      ui_accent: state.config.ui_accent,
      custom_accent_hex: state.config.custom_accent_hex,
      ui_corner: state.config.ui_corner,
      ui_layout: state.config.ui_layout,
      ui_compact: state.config.ui_compact,
      ui_reduced_motion: state.config.ui_reduced_motion,
      ui_sounds: state.config.ui_sounds,
      ui_language: state.config.ui_language,
    });
    state.player_uuid = computeOfflineUuidPreview(state.config.username);
    renderTopbarAndSession();
    renderPlayView();
    renderProfileView();
    playMcSound("click");
    const lang = getEffectiveLanguage();
    statusEl.textContent = tr(lang, "profile.savedBadge");
    showToast(tr(lang, "profile.savedToast"), "success");
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  } finally {
    pendingSaveProfile = false;
    saveBtn.disabled = false;
  }
}

async function saveAccessibilityPrefs(
  patch: {
    theme?: ThemeMode;
    ui_accent?: AccentTone;
    custom_accent_hex?: string | null;
    ui_corner?: CornerStyle;
    ui_layout?: LayoutMode;
    ui_compact?: boolean;
    ui_reduced_motion?: boolean;
    ui_sounds?: boolean;
    ui_language?: UiLanguage;
  },
  silentSound = false
): Promise<void> {
  if (!state) return;
  unlockAudioContext();
  if (!silentSound) {
    if (patch.ui_sounds !== undefined) {
      setSoundsEnabled(patch.ui_sounds);
      playMcSound(patch.ui_sounds ? "toggleOn" : "toggleOff", true);
    } else if (patch.ui_compact !== undefined) {
      playMcSound(patch.ui_compact ? "toggleOn" : "toggleOff");
    } else if (patch.ui_reduced_motion !== undefined) {
      playMcSound(patch.ui_reduced_motion ? "toggleOn" : "toggleOff");
    } else {
      playMcSound("click");
    }
  }
  const prevLang = getEffectiveLanguage();
  try {
    state.config = await ipc.saveProfileSettings({
      username: state.config.username,
      theme: patch.theme ?? state.config.theme,
      ui_accent: patch.ui_accent ?? state.config.ui_accent,
      custom_accent_hex:
        patch.custom_accent_hex !== undefined
          ? patch.custom_accent_hex
          : state.config.custom_accent_hex,
      ui_corner: patch.ui_corner ?? state.config.ui_corner,
      ui_layout: patch.ui_layout ?? state.config.ui_layout,
      ui_compact: patch.ui_compact ?? state.config.ui_compact,
      ui_reduced_motion: patch.ui_reduced_motion ?? state.config.ui_reduced_motion,
      ui_sounds: patch.ui_sounds ?? state.config.ui_sounds,
      ui_language: patch.ui_language ?? state.config.ui_language,
    });
    if (patch.ui_language !== undefined || getEffectiveLanguage() !== prevLang) {
      byId<HTMLSpanElement>("gfx-save-status").textContent = "";
      byId<HTMLSpanElement>("profile-save-status").textContent = "";
      renderAll();
      updateMaximizeIconState(lastMaximizedState);
    } else {
      applyPreferences();
      renderTopbarAndSession();
      renderPlayView();
      renderProfileView();
    }
  } catch (err) {
    playMcSound("error");
    showToast(errorMessage(err), "error");
  }
}

async function handlePickSkin(): Promise<void> {
  if (!state) return;
  const feedback = byId<HTMLDivElement>("skin-feedback");
  try {
    const res = await ipc.pickAndImportSkin();
    if (!res) {
      return;
    }
    state.skin_rgba = res.rgba;
    state.config.skin_file = res.path;
    state.config.skin_model = res.skin_model;
    state.skin_is_slim = res.skin_is_slim;
    renderTopbarAndSession();
    renderProfileView();
    setBanner(feedback, res.note, "success");
  } catch (err) {
    const msg = errorMessage(err);
    const low = msg.toLowerCase();
    if (!low.includes("iptal") && !low.includes("cancel") && !low.includes("abgebrochen")) {
      setBanner(feedback, msg, "error");
    }
  }
}

async function handleResetSkin(): Promise<void> {
  if (!state) return;
  const feedback = byId<HTMLDivElement>("skin-feedback");
  try {
    const res = await ipc.resetSkin();
    state.skin_rgba = res.rgba;
    state.config.skin_file = res.path;
    state.config.skin_model = res.skin_model;
    state.skin_is_slim = res.skin_is_slim;
    renderTopbarAndSession();
    renderProfileView();
    setBanner(feedback, res.note, "success");
  } catch (err) {
    setBanner(feedback, errorMessage(err), "error");
  }
}

async function handleSkinModelChange(model: SkinModel): Promise<void> {
  if (!state) return;
  const feedback = byId<HTMLDivElement>("skin-feedback");
  try {
    const res = await ipc.saveSkinModel(model);
    state.config.skin_model = res.skin_model;
    state.skin_is_slim = res.skin_is_slim;
    renderProfileView();
    const lang = getEffectiveLanguage();
    const label = tr(lang, res.skin_is_slim ? "skin.slimShort" : "skin.classicShort");
    setBanner(feedback, tr(lang, "skin.modelUpdated", { label }), "success");
  } catch (err) {
    setBanner(feedback, errorMessage(err), "error");
  }
}

async function handleAutoOptimize(): Promise<void> {
  if (!state) return;
  const btn = byId<HTMLButtonElement>("btn-auto-optimize");
  btn.disabled = true;
  try {
    const res = await ipc.autoOptimizeGraphics();
    state.config.graphics = res.graphics;
    state.hardware_notice = res.reason;
    byId<HTMLSpanElement>("gfx-save-status").textContent = "";
    renderPlayView();
    renderSystemView();
    showToast(tr(getEffectiveLanguage(), "system.autoOptToast"), "success");
  } catch (err) {
    showToast(errorMessage(err), "error");
  } finally {
    btn.disabled = false;
  }
}

async function handleSaveGraphics(e: SubmitEvent): Promise<void> {
  e.preventDefault();
  if (!state || pendingSaveGfx) return;
  pendingSaveGfx = true;
  const saveBtn = byId<HTMLButtonElement>("btn-save-graphics");
  saveBtn.disabled = true;
  const statusEl = byId<HTMLSpanElement>("gfx-save-status");
  const nextGfx: GraphicsSettings = {
    preset: "Custom",
    render_distance: Number(byId<HTMLInputElement>("gfx-render-dist").value) || 12,
    simulation_distance: Number(byId<HTMLInputElement>("gfx-sim-dist").value) || 10,
    fov_degrees: Number(byId<HTMLInputElement>("gfx-fov").value) || 75,
    vsync: byId<HTMLInputElement>("gfx-vsync").checked,
    max_fps: Number(byId<HTMLSelectElement>("gfx-max-fps").value) || 260,
    mouse_sensitivity: state.config.graphics.mouse_sensitivity,
    smooth_lighting: byId<HTMLInputElement>("gfx-ao").checked,
    fancy_graphics: byId<HTMLInputElement>("gfx-fancy").checked,
    jvm_ram_mb: Number(byId<HTMLSelectElement>("gfx-jvm-ram").value) || 2048,
    jvm_gc: byId<HTMLSelectElement>("gfx-jvm-gc").value === "ZGC" ? "ZGC" : "G1GC",
    show_fps_hud: state.config.graphics.show_fps_hud,
    auto_configured: true,
  };

  try {
    state.config = await ipc.saveGraphicsSettings(nextGfx);
    renderPlayView();
    renderSystemView();
    const lang = getEffectiveLanguage();
    statusEl.textContent = tr(lang, "system.gfxSavedInline");
    showToast(tr(lang, "system.gfxSavedToast"), "success");
  } catch (err) {
    showToast(errorMessage(err), "error");
  } finally {
    pendingSaveGfx = false;
    saveBtn.disabled = false;
  }
}

async function handleAddServer(e: SubmitEvent): Promise<void> {
  e.preventDefault();
  if (!state) return;
  const nameInput = byId<HTMLInputElement>("srv-name");
  const addrInput = byId<HTMLInputElement>("srv-addr");
  const errEl = byId<HTMLParagraphElement>("server-form-error");

  const addr = addrInput.value.trim();
  const validation = validateServerAddress(addr);
  if (validation) {
    errEl.textContent = validation;
    errEl.hidden = false;
    addrInput.focus();
    return;
  }
  errEl.hidden = true;

  try {
    state.config.saved_servers = await ipc.addServer(nameInput.value.trim(), addr);
    renderServersList();
    renderPlayView();
    showToast(tr(getEffectiveLanguage(), "servers.addedToast"), "success");
    void handlePingServer(addr);
  } catch (err) {
    errEl.textContent = errorMessage(err);
    errEl.hidden = false;
  }
}

async function handleRemoveServer(index: number): Promise<void> {
  if (!state) return;
  try {
    state.config.saved_servers = await ipc.removeServer(index);
    renderServersList();
    renderPlayView();
  } catch (err) {
    showToast(errorMessage(err), "error");
  }
}

async function handlePingServer(address: string): Promise<void> {
  if (serverPingInFlight.has(address)) return;
  serverPingInFlight.add(address);
  serverChecks.set(address, { status: "checking" });
  renderServersList();
  renderPlayView();
  try {
    const ping = await ipc.pingServerCmd(address);
    serverChecks.set(address, { status: "online", ping });
  } catch (err) {
    serverChecks.set(address, { status: "error", message: errorMessage(err) });
  } finally {
    serverPingInFlight.delete(address);
  }
  renderServersList();
  renderPlayView();
}

async function autoPingSavedServers(force = false): Promise<void> {
  if (!state || state.config.saved_servers.length === 0) return;
  const targets = state.config.saved_servers
    .map((s) => s.address)
    .filter((addr) => {
      if (serverPingInFlight.has(addr)) return false;
      if (force) return true;
      const existing = serverChecks.get(addr);
      return !existing || existing.status === "idle";
    });
  await Promise.allSettled(targets.map((addr) => handlePingServer(addr)));
}

async function handleProbeServer(address: string): Promise<void> {
  if (!state) return;
  const probeBox = byId<HTMLDivElement>("probe-feedback");
  const lang = getEffectiveLanguage();
  setBanner(probeBox, tr(lang, "servers.probing", { addr: address }), "neutral");
  try {
    const res = await ipc.probeLocalServerCmd(address, state.config.username);
    if (res.kind === "ReadyInPlay") {
      setBanner(
        probeBox,
        tr(lang, "servers.probeReady", { addr: address, info: res.data.server_info }),
        "success"
      );
    } else if (res.kind === "RequiresOfficialAuth") {
      setBanner(probeBox, tr(lang, "servers.probeAuth", { addr: address }), "error");
    } else {
      setBanner(probeBox, tr(lang, "servers.probeDisc", { addr: address, reason: res.data }), "error");
    }
  } catch (err) {
    setBanner(probeBox, `${address}: ${errorMessage(err)}`, "error");
  }
}

let deckGamepadTimer: number | null = null;
let deckPrevButtons: boolean[] = [];
let deckLastRepeatDir: "up" | "down" | "left" | "right" | null = null;
let deckLastRepeatAt = 0;

function getVisibleDeckFocusables(): HTMLElement[] {
  const nodes = Array.from(
    document.querySelectorAll<HTMLElement>(
      ".app-shell button:not([disabled]), .app-shell input:not([disabled]), .app-shell select:not([disabled]), .app-shell [tabindex]:not([tabindex='-1'])"
    )
  );
  return nodes.filter((el) => {
    if (el.hidden || el.closest("[hidden]")) return false;
    return el.offsetParent !== null && el.getClientRects().length > 0;
  });
}

function getElementCenter(el: HTMLElement): { x: number; y: number } | null {
  const rect = el.getClientRects()[0];
  if (!rect) return null;
  return {
    x: rect.left + rect.width / 2,
    y: rect.top + rect.height / 2,
  };
}

function moveDeckFocus(dir: "up" | "down" | "left" | "right"): void {
  const isPlayVisible = !byId<HTMLElement>("view-play").hidden;
  const active = document.activeElement as HTMLElement | null;

  if (getEffectiveLayout() === "DeckMode" && isPlayVisible && deckCards.length > 0) {
    if (active?.classList.contains("deck-card")) {
      if (dir === "left") {
        selectDeckCard(selectedDeckIdx - 1, { scroll: true, focusBtn: true, withSound: true });
        return;
      }
      if (dir === "right") {
        selectDeckCard(selectedDeckIdx + 1, { scroll: true, focusBtn: true, withSound: true });
        return;
      }
      if (dir === "up") {
        const primary = byId<HTMLButtonElement>("btn-deck-primary");
        const secondary = byId<HTMLButtonElement>("btn-deck-secondary");
        const target = !primary.disabled ? primary : secondary;
        target.focus({ preventScroll: true });
        playMcSound("focus");
        return;
      }
    } else if (active?.closest(".console-hero-actions") && dir === "down") {
      selectDeckCard(selectedDeckIdx, { scroll: true, focusBtn: true, withSound: false });
      playMcSound("focus");
      return;
    } else if (!active || active === document.body) {
      if (dir === "left") {
        selectDeckCard(selectedDeckIdx - 1, { scroll: true, focusBtn: true, withSound: true });
        return;
      }
      if (dir === "right") {
        selectDeckCard(selectedDeckIdx + 1, { scroll: true, focusBtn: true, withSound: true });
        return;
      }
    }
  }

  const candidates = getVisibleDeckFocusables();
  if (candidates.length === 0) return;

  if (!active || !candidates.includes(active)) {
    const preferred =
      candidates.find((el) => el.id === "btn-deck-primary" && !el.hidden) ??
      candidates.find((el) => el.classList.contains("btn-launch")) ??
      candidates.find((el) => el.classList.contains("active")) ??
      candidates[0];
    preferred.focus({ preventScroll: true });
    preferred.scrollIntoView({ block: "nearest", inline: "nearest" });
    playMcSound("focus");
    return;
  }

  const curCenter = getElementCenter(active);
  if (!curCenter) return;
  const cx = curCenter.x;
  const cy = curCenter.y;

  let best: HTMLElement | null = null;
  let bestScore = Number.POSITIVE_INFINITY;

  for (const el of candidates) {
    if (el === active) continue;
    const targetCenter = getElementCenter(el);
    if (!targetCenter) continue;
    const tx = targetCenter.x;
    const ty = targetCenter.y;
    const dx = tx - cx;
    const dy = ty - cy;

    let primary = 0;
    let ortho = 0;
    if (dir === "right") {
      if (dx <= 6) continue;
      primary = dx;
      ortho = Math.abs(dy);
    } else if (dir === "left") {
      if (dx >= -6) continue;
      primary = -dx;
      ortho = Math.abs(dy);
    } else if (dir === "down") {
      if (dy <= 6) continue;
      primary = dy;
      ortho = Math.abs(dx);
    } else {
      if (dy >= -6) continue;
      primary = -dy;
      ortho = Math.abs(dx);
    }

    const score = primary + ortho * 2.4;
    if (score < bestScore) {
      bestScore = score;
      best = el;
    }
  }

  if (best) {
    best.focus({ preventScroll: true });
    best.scrollIntoView({ block: "nearest", inline: "nearest" });
    playMcSound("focus");
  }
}

function cycleActiveView(delta: -1 | 1): void {
  const activeBtn = document.querySelector<HTMLButtonElement>(".nav-btn.active[data-nav]");
  const current = (activeBtn?.dataset.nav as ViewId | undefined) ?? "play";
  const idx = VIEW_ORDER.indexOf(current);
  const nextIdx = (idx + delta + VIEW_ORDER.length) % VIEW_ORDER.length;
  setActiveView(VIEW_ORDER[nextIdx], true, true);
}

function exitConsoleMode(): void {
  if (getEffectiveLayout() !== "DeckMode") return;
  unlockAudioContext();
  playMcSound("back", true);
  consoleModeActive = false;
  consoleAudioMuted = false;

  const url = new URL(window.location.href);
  if (url.searchParams.has("layout")) {
    url.searchParams.delete("layout");
    window.history.replaceState(null, "", url.toString());
  }

  if (state && state.config.ui_layout === "DeckMode") {
    void saveAccessibilityPrefs({ ui_layout: lastDesktopLayout }, true);
  } else {
    applyPreferences();
    renderProfileView();
  }
  void ipc.setFullscreen(false).catch(() => undefined);
}

function pollGamepadFrame(): void {
  deckGamepadTimer = null;
  if (getEffectiveLayout() !== "DeckMode" || document.hidden) {
    return;
  }

  const badge = document.getElementById("deck-gamepad-status");
  const label = document.getElementById("deck-gamepad-label");
  const pads = navigator.getGamepads ? Array.from(navigator.getGamepads()) : [];
  const pad = pads.find((p): p is Gamepad => p !== null && p.connected);

  if (badge && label) {
    const lang = getEffectiveLanguage();
    badge.classList.toggle("is-connected", Boolean(pad));
    label.textContent = pad
      ? pad.id.replace(/\s*\(.*?\)/g, "").trim().slice(0, 28) || tr(lang, "deck.gamepadConnected")
      : tr(lang, "deck.gamepadWaiting");
  }

  if (!pad) {
    deckPrevButtons = [];
    deckLastRepeatDir = null;
    return;
  }

  const btns = pad.buttons.map((b) => b.pressed);
  const pressedEdge = (i: number) => Boolean(btns[i] && !deckPrevButtons[i]);

  const ax = pad.axes[0] ?? 0;
  const ay = pad.axes[1] ?? 0;
  let dir: "up" | "down" | "left" | "right" | null = null;
  if (btns[12] || ay < -0.55) dir = "up";
  else if (btns[13] || ay > 0.55) dir = "down";
  else if (btns[14] || ax < -0.55) dir = "left";
  else if (btns[15] || ax > 0.55) dir = "right";

  const now = performance.now();
  if (dir && (dir !== deckLastRepeatDir || now - deckLastRepeatAt > 190)) {
    deckLastRepeatDir = dir;
    deckLastRepeatAt = now;
    unlockAudioContext();
    moveDeckFocus(dir);
  } else if (!dir) {
    deckLastRepeatDir = null;
  }

  if (pressedEdge(0)) {
    unlockAudioContext();
    const active = document.activeElement as HTMLElement | null;
    const target =
      active && active !== document.body && active !== document.documentElement
        ? active
        : byId<HTMLElement>("btn-deck-primary");
    target.click();
  }
  if (pressedEdge(1)) {
    unlockAudioContext();
    const openDialog = document.querySelector<HTMLDialogElement>("dialog[open]");
    if (openDialog) {
      playMcSound("back");
      openDialog.close();
    } else {
      exitConsoleMode();
    }
  }
  if (pressedEdge(3)) {
    toggleUiSound();
  }
  if (pressedEdge(4)) {
    unlockAudioContext();
    cycleActiveView(-1);
  }
  if (pressedEdge(5)) {
    unlockAudioContext();
    cycleActiveView(1);
  }

  deckPrevButtons = btns;
  deckGamepadTimer = window.setTimeout(pollGamepadFrame, 24);
}

function syncDeckGamepadLoop(active: boolean): void {
  if (deckGamepadTimer !== null) {
    window.clearTimeout(deckGamepadTimer);
    deckGamepadTimer = null;
  }
  if (active && !document.hidden) {
    pollGamepadFrame();
  } else {
    deckPrevButtons = [];
    deckLastRepeatDir = null;
  }
}

function initDeckKeyboardNav(): void {
  window.addEventListener("gamepadconnected", () => {
    if (getEffectiveLayout() === "DeckMode") {
      syncDeckGamepadLoop(true);
    }
  });
  window.addEventListener("gamepaddisconnected", () => {
    if (getEffectiveLayout() === "DeckMode") {
      syncDeckGamepadLoop(true);
    }
  });

  window.addEventListener("keydown", (e) => {
    unlockAudioContext();
    if (getEffectiveLayout() !== "DeckMode") return;

    const target = e.target as HTMLElement | null;
    const isTextControl =
      target instanceof HTMLTextAreaElement ||
      target instanceof HTMLSelectElement ||
      (target instanceof HTMLInputElement && target.type !== "checkbox" && target.type !== "button");

    if (e.key === "Escape") {
      if (!document.querySelector("dialog[open]") && !(target instanceof HTMLSelectElement)) {
        e.preventDefault();
        exitConsoleMode();
      }
      return;
    }

    if (isTextControl) return;

    if (e.key === "ArrowUp") {
      e.preventDefault();
      moveDeckFocus("up");
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      moveDeckFocus("down");
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      moveDeckFocus("left");
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      moveDeckFocus("right");
    } else if (e.key === "q" || e.key === "Q") {
      e.preventDefault();
      cycleActiveView(-1);
    } else if (e.key === "e" || e.key === "E") {
      e.preventDefault();
      cycleActiveView(1);
    } else if (e.key === "m" || e.key === "M") {
      e.preventDefault();
      toggleUiSound();
    }
  });
}

let lastMaximizedState = false;

function updateMaximizeIconState(maximized: boolean): void {
  lastMaximizedState = maximized;
  const lang = getEffectiveLanguage();
  const maxUse = document.getElementById("ico-win-max-use");
  const maxBtn = document.getElementById("btn-win-maximize");
  if (maxUse) {
    maxUse.setAttribute("href", maximized ? "#ico-win-restore" : "#ico-win-max");
  }
  if (maxBtn) {
    maxBtn.setAttribute("aria-label", tr(lang, maximized ? "win.restore" : "win.maximize"));
    maxBtn.setAttribute("title", tr(lang, maximized ? "win.restoreTitle" : "win.maximizeTitle"));
  }
}

async function syncWindowMaximizeIcon(): Promise<void> {
  try {
    const maximized = await ipc.windowIsMaximized();
    updateMaximizeIconState(maximized);
  } catch {
    // Headless preview fallback
  }
}

function wireWindowControls(): void {
  const titlebar = document.getElementById("app-titlebar");
  const topbar = document.querySelector<HTMLElement>("header.topbar");
  const railBrand = document.querySelector<HTMLElement>("aside.rail .brand");
  const minBtn = document.getElementById("btn-win-minimize");
  const maxBtn = document.getElementById("btn-win-maximize");
  const closeBtn = document.getElementById("btn-win-close");

  minBtn?.addEventListener("click", () => {
    unlockAudioContext();
    playMcSound("click");
    void ipc.windowMinimize();
  });

  maxBtn?.addEventListener("click", () => {
    unlockAudioContext();
    playMcSound("click");
    void ipc.windowToggleMaximize().then(updateMaximizeIconState).catch(() => undefined);
  });

  closeBtn?.addEventListener("click", () => {
    unlockAudioContext();
    playMcSound("back");
    void ipc.windowClose();
  });

  const resolveTargetElement = (rawTarget: EventTarget | null): Element | null => {
    if (rawTarget instanceof Element) return rawTarget;
    if (rawTarget instanceof Node) return rawTarget.parentElement;
    return null;
  };

  const isInteractiveTarget = (target: Element | null): boolean =>
    Boolean(
      target?.closest(
        "button, a, input, select, textarea, label, summary, details, canvas, [role='button'], .titlebar-actions, .player-chip, .engine-pill"
      )
    );

  const startWindowDrag = (e: MouseEvent): void => {
    if (e.button !== 0 || e.detail > 1) return;
    const target = resolveTargetElement(e.target);
    if (isInteractiveTarget(target)) return;
    e.preventDefault();
    e.stopPropagation();
    void ipc.windowStartDrag().catch(() => undefined);
  };

  titlebar?.addEventListener("dblclick", (e) => {
    const target = resolveTargetElement(e.target);
    if (isInteractiveTarget(target)) return;
    e.stopPropagation();
    void ipc.windowToggleMaximize().then(updateMaximizeIconState).catch(() => undefined);
  });

  topbar?.addEventListener("dblclick", (e) => {
    const target = resolveTargetElement(e.target);
    if (isInteractiveTarget(target)) return;
    e.stopPropagation();
    void ipc.windowToggleMaximize().then(updateMaximizeIconState).catch(() => undefined);
  });

  titlebar?.addEventListener("mousedown", startWindowDrag);
  topbar?.addEventListener("mousedown", startWindowDrag);
  railBrand?.addEventListener("mousedown", startWindowDrag);

  window.addEventListener("resize", () => {
    void syncWindowMaximizeIcon();
  });
}

function buildPaletteCommands(query: string): PaletteCommand[] {
  if (!state) return [];
  const lang = getEffectiveLanguage();
  const launchBlocked = state.running_session !== null || !state.vanilla.java_ready;
  const cmds: PaletteCommand[] = [];

  const pagesGroup = tr(lang, "cmdk.groupPages");
  const pageIcons: Record<ViewId, string> = {
    play: "ico-play",
    library: "ico-cube",
    worlds: "ico-layers",
    profile: "ico-user",
    system: "ico-sliders",
  };
  for (const v of VIEW_ORDER) {
    cmds.push({
      id: `page-${v}`,
      group: pagesGroup,
      label: tr(lang, `view.${v}`),
      icon: pageIcons[v],
      run: () => setActiveView(v, true, true),
    });
  }

  const actionsGroup = tr(lang, "cmdk.groupActions");
  const launchBtnEl = byId<HTMLButtonElement>("btn-launch-client");
  const prepareBtnEl = byId<HTMLButtonElement>("btn-prepare-gradle");
  const themeBtnEl = byId<HTMLButtonElement>("btn-rail-theme");
  const soundBtnEl = byId<HTMLButtonElement>("btn-rail-sound");
  const deckBtnEl = byId<HTMLButtonElement>("btn-enter-deck");
  const installJavaBtnEl = byId<HTMLButtonElement>("btn-install-java");
  const optimizeBtnEl = byId<HTMLButtonElement>("btn-auto-optimize");

  cmds.push(
    {
      id: "act-launch",
      group: actionsGroup,
      label: byId<HTMLSpanElement>("launch-btn-label").textContent ?? tr(lang, "btn.launchMc"),
      icon: "ico-play",
      disabled: launchBtnEl.disabled,
      run: () => launchBtnEl.click(),
    },
    {
      id: "act-verify",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionVerify"),
      icon: "ico-refresh",
      disabled: prepareBtnEl.disabled,
      run: () => prepareBtnEl.click(),
    },
    {
      id: "act-gamedir",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionGameDir"),
      icon: "ico-folder",
      run: () => void handleOpenGameFolder(),
    },
    {
      id: "act-theme",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionTheme"),
      icon: "ico-theme",
      disabled: themeBtnEl.disabled,
      run: () => themeBtnEl.click(),
    },
    {
      id: "act-sound",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionSound"),
      icon: "ico-sound",
      disabled: soundBtnEl.disabled,
      run: () => soundBtnEl.click(),
    },
    {
      id: "act-console",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionConsole"),
      icon: "ico-gamepad",
      disabled: deckBtnEl.disabled,
      run: () => deckBtnEl.click(),
    }
  );

  if (!state.vanilla.java_ready) {
    cmds.push({
      id: "act-java",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionJava"),
      icon: "ico-bolt",
      disabled: installJavaBtnEl.disabled,
      run: () => installJavaBtnEl.click(),
    });
  }

  cmds.push(
    {
      id: "act-optimize",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionOptimize"),
      icon: "ico-sliders",
      disabled: optimizeBtnEl.disabled,
      run: () => optimizeBtnEl.click(),
    },
    {
      id: "act-screenshots",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionScreenshots"),
      keywords: "screenshots gallery photos f2 ekran",
      icon: "ico-camera",
      run: () => {
        setActiveView("worlds", true, true);
        setWorldsSubTab("screenshots", false);
      },
    },
    {
      id: "act-crash-doctor",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionCrashDoctor"),
      keywords: "crash doctor logs error diagnose",
      icon: "ico-shield",
      run: () => void handleRunCrashDoctor(),
    },
    {
      id: "act-diagnostics",
      group: actionsGroup,
      label: tr(lang, "cmdk.actionDiagnostics"),
      keywords: "copy diagnostics report clipboard system info",
      icon: "ico-copy",
      run: () => void handleCopyDiagnostics(),
    }
  );

  const worldsGroup = tr(lang, "cmdk.groupWorlds");
  cmds.push({
    id: "world-title",
    group: worldsGroup,
    label: tr(lang, "cmdk.titleScreen"),
    hint: tr(lang, "cmdk.hintLaunch"),
    icon: "ico-play",
    disabled: launchBlocked,
    run: () => void handleLaunchClient("title", null, null),
  });
  for (const save of state.saves) {
    cmds.push({
      id: `world-${save.id}`,
      group: worldsGroup,
      label: save.id,
      hint: tr(lang, "cmdk.hintLaunch"),
      icon: "ico-cube",
      disabled: launchBlocked,
      run: () => void handleLaunchClient("singleplayer", save.id, null),
    });
  }

  const serversGroup = tr(lang, "cmdk.groupServers");
  for (const srv of state.config.saved_servers) {
    cmds.push({
      id: `srv-${srv.address}`,
      group: serversGroup,
      label: srv.name,
      hint: srv.address,
      keywords: `${srv.address} ${tr(lang, "cmdk.hintConnect")}`,
      icon: "ico-server",
      disabled: launchBlocked,
      run: () => void handleLaunchClient("multiplayer", null, srv.address),
    });
  }

  const q = query.trim();
  if (q.length >= 2) {
    cmds.push({
      id: "modrinth-search",
      group: tr(lang, "cmdk.groupModrinth"),
      label: tr(lang, "cmdk.searchModrinth", { q }),
      keywords: q,
      icon: "ico-search",
      run: () => {
        setActiveView("library", true, true);
        if (libActiveTab === "installed") {
          document.querySelector<HTMLButtonElement>('.lib-tab[data-lib-tab="mod"]')?.click();
        }
        const searchInput = byId<HTMLInputElement>("library-search-input");
        searchInput.value = q;
        byId<HTMLButtonElement>("btn-library-search").click();
      },
    });
  }

  return cmds;
}

function wireEvents(): void {
  wireWindowControls();
  const unlockOnce = () => unlockAudioContext();
  window.addEventListener("pointerdown", unlockOnce, { passive: true });
  window.addEventListener("keydown", unlockOnce, { passive: true });

  const navButtons = document.querySelectorAll<HTMLButtonElement>(".nav-btn[data-nav]");
  for (const btn of Array.from(navButtons)) {
    btn.addEventListener("click", () => {
      const target = btn.dataset.nav as ViewId | undefined;
      if (target) setActiveView(target, true, true);
    });
  }

  window.addEventListener("hashchange", () => {
    setActiveView(readInitialViewFromHash(), false);
  });

  byId<HTMLButtonElement>("btn-rail-theme").addEventListener("click", () => {
    if (!state) return;
    const idx = THEME_ORDER.indexOf(state.config.theme);
    const next = THEME_ORDER[(idx + 1) % THEME_ORDER.length];
    void saveAccessibilityPrefs({ theme: next });
  });

  byId<HTMLButtonElement>("btn-rail-layout").addEventListener("click", () => {
    if (!state) return;
    if (getEffectiveLayout() === "DeckMode") {
      exitConsoleMode();
      return;
    }
    const idx = DESKTOP_LAYOUT_ORDER.indexOf(state.config.ui_layout);
    const next = DESKTOP_LAYOUT_ORDER[(idx + 1 + DESKTOP_LAYOUT_ORDER.length) % DESKTOP_LAYOUT_ORDER.length];
    void saveAccessibilityPrefs({ ui_layout: next });
  });

  byId<HTMLButtonElement>("btn-rail-sound").addEventListener("click", () => {
    toggleUiSound();
  });

  byId<HTMLButtonElement>("btn-enter-deck").addEventListener("click", () => {
    enterConsoleMode();
  });

  byId<HTMLButtonElement>("btn-exit-deck").addEventListener("click", () => {
    exitConsoleMode();
  });

  byId<HTMLButtonElement>("btn-deck-primary").addEventListener("click", () => {
    const card = deckCards[selectedDeckIdx];
    if (card) handleDeckPrimaryAction(card);
  });

  byId<HTMLButtonElement>("btn-deck-secondary").addEventListener("click", () => {
    const card = deckCards[selectedDeckIdx];
    if (card) handleDeckSecondaryAction(card);
  });

  byId<HTMLButtonElement>("btn-deck-prepare").addEventListener("click", () => {
    unlockAudioContext();
    void handlePrepareVanilla();
  });

  byId<HTMLButtonElement>("btn-deck-prev").addEventListener("click", () => {
    unlockAudioContext();
    selectDeckCard(selectedDeckIdx - 1, { scroll: true, focusBtn: false, withSound: true });
  });

  byId<HTMLButtonElement>("btn-deck-next").addEventListener("click", () => {
    unlockAudioContext();
    selectDeckCard(selectedDeckIdx + 1, { scroll: true, focusBtn: false, withSound: true });
  });

  const deckCarousel = byId<HTMLDivElement>("deck-carousel");
  deckCarousel.addEventListener(
    "wheel",
    (e) => {
      if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
        deckCarousel.scrollLeft += e.deltaY;
        e.preventDefault();
      }
    },
    { passive: false }
  );

  byId<HTMLButtonElement>("btn-goto-worlds").addEventListener("click", () => {
    setActiveView("worlds", true, true);
  });

  byId<HTMLButtonElement>("btn-goto-worlds-bottom").addEventListener("click", () => {
    setActiveView("worlds", true, true);
  });

  byId<HTMLButtonElement>("btn-play-goto-settings").addEventListener("click", () => {
    setActiveView("system", true, true);
  });

  byId<HTMLButtonElement>("btn-goto-settings").addEventListener("click", () => {
    setActiveView("system", true, true);
  });

  document.getElementById("btn-toggle-rail")?.addEventListener("click", () => {
    if (!state) return;
    const current = getEffectiveLayout();
    const next: LayoutMode = current === "MinimalRail" ? lastExpandedRailLayout : "MinimalRail";
    playMcSound("click");
    void saveAccessibilityPrefs({ ui_layout: next });
  });

  document.getElementById("btn-rail-manage-saves")?.addEventListener("click", () => {
    setActiveView("worlds", true, true);
  });

  document.getElementById("btn-rail-profile")?.addEventListener("click", () => {
    setActiveView("profile", true, true);
  });

  document.getElementById("btn-play-goto-profile")?.addEventListener("click", () => {
    setActiveView("profile", true, true);
  });
  document.getElementById("btn-play-open-library")?.addEventListener("click", () => {
    setActiveView("library", true, true);
  });
  document.getElementById("btn-play-crash-doctor")?.addEventListener("click", () => {
    void handleRunCrashDoctor();
  });
  document.getElementById("btn-general-copy-diag")?.addEventListener("click", () => {
    void handleCopyDiagnostics();
  });
  document.getElementById("btn-ping-all-servers")?.addEventListener("click", () => {
    unlockAudioContext();
    playMcSound("click");
    void autoPingSavedServers(true);
  });
  document.getElementById("btn-lib-check-updates")?.addEventListener("click", () => {
    void handleCheckLibraryUpdates();
  });
  document.getElementById("btn-lib-update-all")?.addEventListener("click", () => {
    void handleUpdateAllMods();
  });
  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-worlds-tab]"))) {
    btn.addEventListener("click", () => {
      const tab = btn.dataset.worldsTab === "screenshots" ? "screenshots" : "saves";
      setWorldsSubTab(tab, true);
    });
  }
  document.getElementById("btn-refresh-screenshots")?.addEventListener("click", () => {
    unlockAudioContext();
    playMcSound("click");
    void loadScreenshots(true);
  });
  document.getElementById("btn-open-screenshots-folder")?.addEventListener("click", () => {
    unlockAudioContext();
    playMcSound("click");
    void ipc.openScreenshotsFolder().catch((err) => showToast(errorMessage(err), "error"));
  });
  document.getElementById("btn-lightbox-close")?.addEventListener("click", () => {
    byId<HTMLDialogElement>("gallery-lightbox").close();
  });
  document.getElementById("btn-lightbox-prev")?.addEventListener("click", () => stepLightbox(-1));
  document.getElementById("btn-lightbox-next")?.addEventListener("click", () => stepLightbox(1));
  document.getElementById("btn-lightbox-folder")?.addEventListener("click", () => {
    void ipc.openScreenshotsFolder().catch((err) => showToast(errorMessage(err), "error"));
  });
  document.getElementById("btn-lightbox-delete")?.addEventListener("click", () => {
    if (lightboxIndex >= 0 && screenshotItems[lightboxIndex]) {
      void handleDeleteScreenshot(screenshotItems[lightboxIndex].filename);
    }
  });
  const lightboxDlg = document.getElementById("gallery-lightbox");
  if (lightboxDlg instanceof HTMLDialogElement) {
    lightboxDlg.addEventListener("click", (e) => {
      if (e.target === lightboxDlg) lightboxDlg.close();
    });
    lightboxDlg.addEventListener("keydown", (e) => {
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        stepLightbox(-1);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        stepLightbox(1);
      }
    });
    lightboxDlg.addEventListener("close", () => {
      cancelElementAnimation(lightboxDlg);
      lightboxIndex = -1;
      const trig = lightboxTriggerEl;
      lightboxTriggerEl = null;
      if (trig && trig.isConnected) trig.focus();
    });
  }

  const restoreDlg = document.getElementById("restore-backup-dialog");
  if (restoreDlg instanceof HTMLDialogElement) {
    const closeRestore = () => {
      if (restoreDlg.open) restoreDlg.close();
    };
    document.getElementById("restore-dialog-close")?.addEventListener("click", closeRestore);
    document.getElementById("restore-dialog-cancel")?.addEventListener("click", closeRestore);
    document.getElementById("restore-dialog-confirm")?.addEventListener("click", () => {
      void confirmRestoreBackup();
    });
    restoreDlg.addEventListener("click", (e) => {
      if (e.target === restoreDlg) closeRestore();
    });
    restoreDlg.addEventListener("close", () => {
      cancelElementAnimation(restoreDlg);
      const trig = lastRestoreDialogTrigger;
      lastRestoreDialogTrigger = null;
      if (trig && trig.isConnected) trig.focus();
    });
  }

  document.getElementById("srv-addr")?.addEventListener("input", () => {
    const errEl = document.getElementById("server-form-error");
    const addrInput = document.getElementById("srv-addr");
    if (errEl && !errEl.hidden && addrInput instanceof HTMLInputElement) {
      const validation = validateServerAddress(addrInput.value);
      if (!validation) {
        errEl.hidden = true;
        errEl.textContent = "";
      } else {
        errEl.textContent = validation;
      }
    }
  });

  document.getElementById("profile-username")?.addEventListener("input", () => {
    const inputEl = document.getElementById("profile-username");
    const uuidEl = document.getElementById("profile-uuid-preview");
    if (inputEl instanceof HTMLInputElement && uuidEl) {
      const typed = inputEl.value.trim();
      uuidEl.textContent =
        state?.player_uuid && (!typed || typed === state.config.username)
          ? state.player_uuid
          : computeOfflineUuidPreview(typed || state?.config.username || "Player");
    }
  });

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>(".loader-pill[data-loader]"))) {
    btn.addEventListener("click", () => {
      const loader = btn.dataset.loader as LoaderKind | undefined;
      if (loader && state?.config.mod_loader !== loader) {
        void handleLoaderChange(loader);
      }
    });
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>(".lib-tab[data-lib-tab]"))) {
    btn.addEventListener("click", () => {
      const tab = btn.dataset.libTab as LibraryKind | "installed" | undefined;
      if (!tab || tab === libActiveTab) return;
      const wasInstalled = libActiveTab === "installed";
      playMcSound("tab");
      libActiveTab = tab;
      renderLibraryView();
      const targetPanel = document.getElementById(
        tab === "installed" ? "library-installed-panel" : "library-catalog-grid"
      );
      if (targetPanel && (wasInstalled || tab === "installed")) {
        animateViewHandoff(targetPanel, getEffectiveMotion());
      }
      if (tab === "installed") {
        void loadLibraryInstalled();
      } else {
        void loadLibraryCatalog(true);
      }
    });
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-lib-loader]"))) {
    btn.addEventListener("click", () => {
      const loader = btn.dataset.libLoader as LoaderKind | undefined;
      if (!loader || loader === libTargetLoader) return;
      playMcSound("click");
      libTargetLoader = loader;
      renderLibraryView();
      if (libActiveTab === "installed") {
        void loadLibraryInstalled();
      } else {
        void loadLibraryCatalog(true);
      }
    });
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-installed-kind]"))) {
    btn.addEventListener("click", () => {
      const kind = btn.dataset.installedKind as LibraryKind | undefined;
      if (!kind || kind === libInstalledKind) return;
      playMcSound("tab");
      libInstalledKind = kind;
      syncLibraryControls();
      void loadLibraryInstalled();
    });
  }

  document.getElementById("btn-library-search")?.addEventListener("click", () => {
    playMcSound("click");
    void loadLibraryCatalog(true);
  });
  document.getElementById("library-search-input")?.addEventListener("keydown", (e) => {
    if ((e as KeyboardEvent).key === "Enter") {
      playMcSound("click");
      void loadLibraryCatalog(true);
    }
  });
  document.getElementById("library-sort-select")?.addEventListener("change", () => {
    void loadLibraryCatalog(true);
  });
  document.getElementById("btn-kit-fabric-shaders")?.addEventListener("click", () => {
    void handleInstallShaderBundle("fabric_shaders");
  });
  document.getElementById("btn-kit-forge-shaders")?.addEventListener("click", () => {
    void handleInstallShaderBundle("forge_shaders");
  });
  document.getElementById("btn-import-jar")?.addEventListener("click", () => {
    void handleImportLibraryFile("mod");
  });
  document.getElementById("btn-import-zip")?.addEventListener("click", () => {
    void handleImportLibraryFile(libActiveTab === "resourcePack" ? "resourcePack" : "shader");
  });
  document.getElementById("btn-lib-open-folder")?.addEventListener("click", () => {
    const kind: LibraryKind =
      libActiveTab === "installed" ? libInstalledKind : libActiveTab;
    void ipc.openLibraryFolder(kind, getEffectiveCatalogLoader()).catch((err) => {
      showToast(errorMessage(err), "error");
    });
  });
  document.getElementById("btn-lib-prepare-loader")?.addEventListener("click", () => {
    void handlePrepareLoader();
  });

  const openSettingsTab = (tab: string): void => {
    setActiveView("system", true, true);
    document.querySelector<HTMLButtonElement>(`[data-settings-tab="${tab}"]`)?.click();
  };

  document.getElementById("btn-rail-runtime")?.addEventListener("click", () => openSettingsTab("game"));
  document.getElementById("btn-play-tune-jvm")?.addEventListener("click", () => openSettingsTab("game"));
  document.getElementById("btn-play-tune-gfx")?.addEventListener("click", () => openSettingsTab("game"));
  document.getElementById("btn-play-tune-net")?.addEventListener("click", () => openSettingsTab("network"));
  document.getElementById("btn-play-add-server")?.addEventListener("click", () => openSettingsTab("network"));

  byId<HTMLSelectElement>("play-save-select").addEventListener("change", (e) => {
    const val = (e.currentTarget as HTMLSelectElement).value;
    void handleSelectSave(val ? val : null);
  });

  byId<HTMLSelectElement>("play-version-select").addEventListener("change", (e) => {
    const val = (e.currentTarget as HTMLSelectElement).value;
    if (val) void handleVersionChange(val);
  });

  byId<HTMLSelectElement>("settings-version-select").addEventListener("change", (e) => {
    const val = (e.currentTarget as HTMLSelectElement).value;
    if (val) void handleVersionChange(val);
  });

  byId<HTMLButtonElement>("btn-prepare-gradle").addEventListener("click", () => {
    void handlePrepareVanilla();
  });

  byId<HTMLButtonElement>("btn-optimize-now").addEventListener("click", () => {
    void handleOptimizeClientNow();
  });

  byId<HTMLSelectElement>("pref-dl-threads").addEventListener("change", (e) => {
    const val = Number((e.currentTarget as HTMLSelectElement).value) || 24;
    void handleSaveTurboSettings({ download_threads: val });
  });

  byId<HTMLInputElement>("pref-net-boost").addEventListener("change", (e) => {
    void handleSaveTurboSettings({
      network_boost: (e.currentTarget as HTMLInputElement).checked,
    });
  });

  byId<HTMLInputElement>("pref-proc-boost").addEventListener("change", (e) => {
    void handleSaveTurboSettings({
      process_boost: (e.currentTarget as HTMLInputElement).checked,
    });
  });

  byId<HTMLInputElement>("pref-discord-rpc").addEventListener("change", (e) => {
    void handleSaveTurboSettings({
      discord_rpc: (e.currentTarget as HTMLInputElement).checked,
    });
  });

  for (const id of ["btn-rail-github", "btn-settings-github", "btn-general-github", "btn-legal-github"]) {
    document.getElementById(id)?.addEventListener("click", () => {
      void handleOpenGithubPage();
    });
  }

  for (const id of [
    "btn-open-gamedir",
    "btn-rail-gamedir",
    "btn-profile-gamedir",
    "btn-settings-gamedir",
    "btn-general-gamedir",
  ]) {
    document.getElementById(id)?.addEventListener("click", () => {
      void handleOpenGameFolder();
    });
  }

  document.getElementById("btn-general-console")?.addEventListener("click", () => {
    enterConsoleMode();
  });

  const wireLiveUsernameClear = (inputId: string, errId: string): void => {
    const inputEl = document.getElementById(inputId);
    const errEl = document.getElementById(errId);
    if (!(inputEl instanceof HTMLInputElement) || !errEl) return;
    inputEl.addEventListener("input", () => {
      const validation = validateUsername(inputEl.value);
      if (!validation) {
        errEl.hidden = true;
        errEl.textContent = "";
      } else if (!errEl.hidden) {
        errEl.textContent = validation;
      }
    });
  };
  wireLiveUsernameClear("play-username", "play-username-error");
  wireLiveUsernameClear("profile-username", "profile-username-error");

  byId<HTMLButtonElement>("btn-install-java").addEventListener("click", () => {
    void handleInstallJava21();
  });

  byId<HTMLButtonElement>("btn-recheck-java").addEventListener("click", () => {
    void handleRecheckJava21();
  });

  byId<HTMLButtonElement>("btn-open-java-site").addEventListener("click", () => {
    void handleOpenJavaPage();
  });

  byId<HTMLButtonElement>("btn-system-install-java").addEventListener("click", () => {
    void handleInstallJava21();
  });

  byId<HTMLButtonElement>("btn-system-recheck-java").addEventListener("click", () => {
    void handleRecheckJava21();
  });

  byId<HTMLButtonElement>("btn-system-open-java").addEventListener("click", () => {
    void handleOpenJavaPage();
  });

  byId<HTMLButtonElement>("btn-launch-client").addEventListener("click", () => {
    if (!state) return;
    const saveId = state.config.last_save_id;
    void handleLaunchClient(saveId ? "singleplayer" : "title", saveId, null);
  });

  byId<HTMLButtonElement>("btn-refresh-worlds").addEventListener("click", () => {
    void handleRefreshSaves();
  });

  byId<HTMLButtonElement>("btn-open-mc-for-world").addEventListener("click", () => {
    void handleLaunchClient("title", null, null);
  });

  const worldsFilterInput = byId<HTMLInputElement>("worlds-filter-input");
  worldsFilterInput.addEventListener("input", () => {
    worldsFilterQuery = worldsFilterInput.value;
    renderWorldsView();
  });

  const worldsSortSelect = byId<HTMLSelectElement>("worlds-sort-select");
  worldsSortSelect.addEventListener("change", () => {
    const val = worldsSortSelect.value;
    worldsSortOrder = val === "name" || val === "size" ? val : "recent";
    renderWorldsView();
  });

  byId<HTMLButtonElement>("btn-clear-world-filter").addEventListener("click", () => {
    worldsFilterQuery = "";
    worldsFilterInput.value = "";
    renderWorldsView();
    worldsFilterInput.focus();
  });

  byId<HTMLFormElement>("profile-form").addEventListener("submit", (e) => {
    void handleSaveProfileForm(e);
  });

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]"))) {
    btn.addEventListener("click", () => {
      const choice = btn.dataset.themeChoice as ThemeMode | undefined;
      if (choice && THEME_ORDER.includes(choice)) {
        clearSearchParam("theme");
        void saveAccessibilityPrefs({ theme: choice });
      }
    });
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-layout-choice]"))) {
    btn.addEventListener("click", () => {
      const choice = btn.dataset.layoutChoice as LayoutMode | undefined;
      if (!choice || !LAYOUT_ORDER.includes(choice)) return;
      clearSearchParam("layout");
      if (choice === "DeckMode") {
        enterConsoleMode();
        return;
      }
      if (getEffectiveLayout() === "DeckMode") {
        consoleModeActive = false;
        consoleAudioMuted = false;
        playMcSound("back", true);
        void ipc.setFullscreen(false).catch(() => undefined);
      }
      void saveAccessibilityPrefs({ ui_layout: choice });
    });
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-accent-choice]"))) {
    btn.addEventListener("click", () => {
      const choice = btn.dataset.accentChoice as AccentTone | undefined;
      if (choice && ACCENT_ORDER.includes(choice)) {
        clearSearchParam("accent");
        void saveAccessibilityPrefs({ ui_accent: choice });
      }
    });
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-corner-choice]"))) {
    btn.addEventListener("click", () => {
      const choice = btn.dataset.cornerChoice as CornerStyle | undefined;
      if (choice && CORNER_ORDER.includes(choice)) {
        clearSearchParam("corner");
        void saveAccessibilityPrefs({ ui_corner: choice });
      }
    });
  }

  for (const btn of Array.from(document.querySelectorAll<HTMLButtonElement>("[data-lang-choice]"))) {
    btn.addEventListener("click", () => {
      const choice = btn.dataset.langChoice as UiLanguage | undefined;
      if (choice && UI_LANGUAGE_ORDER.includes(choice)) {
        const url = new URL(window.location.href);
        if (url.searchParams.has("lang")) {
          url.searchParams.delete("lang");
          window.history.replaceState(null, "", url.toString());
        }
        void saveAccessibilityPrefs({ ui_language: choice });
      }
    });
  }

  const customColorInput = byId<HTMLInputElement>("pref-custom-accent");
  customColorInput.addEventListener("input", () => {
    if (!state) return;
    const valid = normalizeHex(customColorInput.value);
    if (valid) {
      byId<HTMLSpanElement>("pref-custom-hex-label").textContent = valid;
      byId<HTMLLabelElement>("pref-custom-chip").classList.add("is-active");
      applyDynamicDesignTokens(getEffectiveTheme(), "Custom", valid, getEffectiveCorner());
    }
  });
  customColorInput.addEventListener("change", () => {
    const valid = normalizeHex(customColorInput.value);
    if (valid) {
      void saveAccessibilityPrefs({ ui_accent: "Custom", custom_accent_hex: valid });
    }
  });

  byId<HTMLInputElement>("pref-compact").addEventListener("change", (e) => {
    void saveAccessibilityPrefs({ ui_compact: (e.currentTarget as HTMLInputElement).checked });
  });

  byId<HTMLInputElement>("pref-reduced-motion").addEventListener("change", (e) => {
    void saveAccessibilityPrefs({ ui_reduced_motion: (e.currentTarget as HTMLInputElement).checked });
  });

  byId<HTMLInputElement>("pref-sounds").addEventListener("change", (e) => {
    toggleUiSound((e.currentTarget as HTMLInputElement).checked);
  });

  const skinModeBtns = document.querySelectorAll<HTMLButtonElement>("[data-skin-view]");
  for (const btn of Array.from(skinModeBtns)) {
    btn.addEventListener("click", () => {
      const mode = btn.dataset.skinView as SkinViewMode | undefined;
      if (mode && state && mode !== skinViewMode) {
        const prevMode = skinViewMode;
        skinViewMode = mode;
        playMcSound("click");
        renderProfileView();
        if (prevMode === "atlas" || mode === "atlas") {
          const previewEl = document.getElementById(
            mode === "atlas" ? "skin-atlas-canvas" : "skin-preview-canvas"
          );
          if (previewEl) animateViewHandoff(previewEl, getEffectiveMotion());
        }
      }
    });
  }

  byId<HTMLButtonElement>("btn-pick-skin").addEventListener("click", () => {
    void handlePickSkin();
  });

  byId<HTMLButtonElement>("btn-reset-skin").addEventListener("click", () => {
    void handleResetSkin();
  });

  byId<HTMLSelectElement>("skin-model-select").addEventListener("change", (e) => {
    const raw = (e.currentTarget as HTMLSelectElement).value;
    const model: SkinModel = raw === "Slim" || raw === "Classic" ? raw : "Auto";
    void handleSkinModelChange(model);
  });

  byId<HTMLButtonElement>("btn-auto-optimize").addEventListener("click", () => {
    void handleAutoOptimize();
  });

  byId<HTMLInputElement>("gfx-render-dist").addEventListener("input", (e) => {
    byId<HTMLOutputElement>("gfx-render-dist-out").textContent = (e.currentTarget as HTMLInputElement).value;
  });

  byId<HTMLInputElement>("gfx-sim-dist").addEventListener("input", (e) => {
    byId<HTMLOutputElement>("gfx-sim-dist-out").textContent = (e.currentTarget as HTMLInputElement).value;
  });

  byId<HTMLInputElement>("gfx-fov").addEventListener("input", (e) => {
    byId<HTMLOutputElement>("gfx-fov-out").textContent = `${(e.currentTarget as HTMLInputElement).value}°`;
  });

  byId<HTMLFormElement>("graphics-form").addEventListener("submit", (e) => {
    void handleSaveGraphics(e);
  });

  byId<HTMLFormElement>("add-server-form").addEventListener("submit", (e) => {
    void handleAddServer(e);
  });

  const dialog = byId<HTMLDialogElement>("delete-world-dialog");
  const closeDialog = () => {
    if (dialog.open) dialog.close();
  };
  byId<HTMLButtonElement>("delete-dialog-close").addEventListener("click", closeDialog);
  byId<HTMLButtonElement>("delete-dialog-cancel").addEventListener("click", closeDialog);
  byId<HTMLButtonElement>("delete-dialog-confirm").addEventListener("click", () => {
    void confirmDeleteSave();
  });
  dialog.addEventListener("click", (e) => {
    if (e.target === dialog) {
      closeDialog();
    }
  });
  dialog.addEventListener("close", () => {
    cancelElementAnimation(dialog);
    const trigger = lastDialogTrigger;
    lastDialogTrigger = null;
    if (trigger && trigger.isConnected) {
      trigger.focus();
    } else {
      byId<HTMLButtonElement>("btn-refresh-worlds").focus();
    }
  });

  // Settings sub-tabs (General / Game & JVM / Network & Servers / Appearance)
  const settingsTabBar = document.querySelector<HTMLElement>(".settings-nav-bar");
  const settingsTabs = Array.from(document.querySelectorAll<HTMLButtonElement>("[data-settings-tab]"));
  const settingsPanels = Array.from(document.querySelectorAll<HTMLElement>("[data-settings-panel]"));
  const activateSettingsTab = (tabId: string): void => {
    for (const btn of settingsTabs) {
      const active = btn.dataset.settingsTab === tabId;
      btn.classList.toggle("active", active);
      btn.setAttribute("aria-selected", active ? "true" : "false");
    }
    for (const panel of settingsPanels) {
      const active = panel.dataset.settingsPanel === tabId;
      const wasHidden = panel.hidden;
      panel.hidden = !active;
      if (active && wasHidden) {
        animateViewHandoff(panel, getEffectiveMotion());
      } else if (!active) {
        cancelElementAnimation(panel);
      }
    }
    syncSlidingIndicators();
    if (tabId === "network") {
      void autoPingSavedServers(false);
    }
  };
  for (const btn of settingsTabs) {
    btn.addEventListener("click", () => {
      const tabId = btn.dataset.settingsTab;
      if (tabId) {
        playMcSound("tab");
        activateSettingsTab(tabId);
      }
    });
  }
  if (settingsTabBar) {
    wireRovingTabGroup(settingsTabBar, "[data-settings-tab]");
  }

  // Sliding sticky-pill gliders + roving keyboard groups for every segmented option bar
  for (const bar of Array.from(
    document.querySelectorAll<HTMLElement>(
      ".loader-seg, .library-tabs, .library-installed-filters, .corner-seg, .skin-view-modes, .settings-nav-bar"
    )
  )) {
    wireSlidingGroup(bar);
    if (bar !== settingsTabBar) {
      wireRovingTabGroup(bar, "button");
    }
  }

  // Kinetic WebUI: accordions, roving tab groups, OS reduced-motion & window lifecycle
  const hwDrawerEl = document.getElementById("hardware-runtime-drawer");
  if (hwDrawerEl instanceof HTMLDetailsElement) {
    wireDetailsAccordion(hwDrawerEl, getEffectiveMotion);
  }

  const navContainer = document.querySelector<HTMLElement>("aside.rail .nav");
  if (navContainer) {
    wireRovingTabGroup(navContainer, ".nav-btn[data-nav]");
    if (typeof ResizeObserver !== "undefined") {
      new ResizeObserver(() => syncNavIndicator()).observe(navContainer);
    }
  }
  window.addEventListener("resize", () => {
    syncNavIndicator();
    syncSlidingIndicators();
  });

  const palette = initCommandPalette({
    getCommands: buildPaletteCommands,
    locale: () => LANG_INTL_LOCALE[getEffectiveLanguage()],
    isBlocked: () => getEffectiveLayout() === "DeckMode" || !state,
    motion: getEffectiveMotion,
  });
  document.getElementById("btn-open-cmdk")?.addEventListener("click", () => palette.open());
  window.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "k") {
      e.preventDefault();
      palette.open();
    }
  });
  for (const group of Array.from(document.querySelectorAll<HTMLElement>(".seg-control"))) {
    wireRovingTabGroup(group, ".seg-btn");
  }

  watchSystemMotion(() => {
    applyPreferences();
  });

  document.addEventListener("visibilitychange", () => {
    const isDeck = getEffectiveLayout() === "DeckMode" && !document.hidden;
    syncDeckClock(isDeck);
    syncDeckGamepadLoop(isDeck);
  });
}

async function init(): Promise<void> {
  wireEvents();
  initDeckKeyboardNav();
  initMinecraftSounds();
  setActiveView(readInitialViewFromHash(), false);

  try {
    state = await ipc.getBootstrapState();
    renderAll();
    if (!initialServersAutoPinged && state.config.saved_servers.length > 0) {
      initialServersAutoPinged = true;
      void autoPingSavedServers(false);
    }
    if (currentActiveViewId() === "worlds") {
      void loadScreenshots(false);
    }
    if (currentActiveViewId() === "library") {
      if (libActiveTab === "installed") {
        void loadLibraryInstalled();
      } else {
        void loadLibraryCatalog(false);
      }
    }
  } catch (err) {
    setActiveView("play", false, false);
    setBanner(
      byId<HTMLDivElement>("launch-feedback"),
      tr(getEffectiveLanguage(), "err.bootstrap", { err: errorMessage(err) }),
      "error"
    );
  } finally {
    void document.documentElement.offsetWidth;
    document.documentElement.classList.remove("is-hydrating");
    syncNavIndicator();
  }

  await ipc.onLaunchLog((payload) => {
    appendLogLine(payload.line, payload.is_err);
  });

  await ipc.onSessionEnded((payload) => {
    if (!state) return;
    state.running_session = null;
    state.saves = payload.saves;
    state.vanilla = payload.vanilla;
    if (payload.loader_status) state.loader_status = payload.loader_status;
    if (payload.library_counts) state.library_counts = payload.library_counts;
    if (payload.playtime_stats) state.playtime_stats = payload.playtime_stats;
    renderTopbarAndSession();
    renderPlayView();
    renderLibraryView();
    renderWorldsView();
    renderSystemView();
    if (payload.kind === "launch") {
      void ipc.getBootstrapState().then((fresh) => {
        if (!state) return;
        state.config = fresh.config;
        if (fresh.playtime_stats) state.playtime_stats = fresh.playtime_stats;
        if (fresh.player_uuid) state.player_uuid = fresh.player_uuid;
        renderPlayView();
        renderWorldsView();
        renderProfileView();
        renderSystemView();
      }).catch(() => undefined);
    }

    const lang = getEffectiveLanguage();
    const feedback = byId<HTMLDivElement>("launch-feedback");
    if (payload.diagnosis) {
      lastCrashDiagnosis = payload.diagnosis;
      renderCrashDoctor(true);
    } else if (payload.kind === "launch" && (payload.error || (payload.exit_code !== null && payload.exit_code !== 0))) {
      void ipc.diagnoseLastCrash().then((diag) => {
        if (diag) {
          lastCrashDiagnosis = diag;
          renderCrashDoctor(true);
        }
      }).catch(() => undefined);
    } else if (payload.kind === "launch" && payload.exit_code === 0 && !payload.error) {
      lastCrashDiagnosis = null;
      renderCrashDoctor(false);
    }
    if (payload.error) {
      playMcSound("error");
      setBanner(feedback, tr(lang, "err.sessionFailed", { err: payload.error }), "error");
      showToast(payload.error, "error");
    } else if (payload.exit_code !== null && payload.exit_code !== 0) {
      playMcSound("error");
      const msg = tr(lang, "err.gradleExit", { code: payload.exit_code });
      setBanner(feedback, msg, "error");
      showToast(msg, "error");
    } else if (payload.kind === "java-install") {
      if (payload.vanilla.java_ready) {
        playMcSound("launch");
        const msg = tr(lang, "ok.javaInstalled", { ver: payload.vanilla.java_version_label });
        setBanner(feedback, msg, "success");
        showToast(msg, "success");
      } else {
        playMcSound("error");
        const msg = tr(lang, "err.javaNotVerified");
        setBanner(feedback, msg, "error");
        showToast(msg, "error");
      }
    } else if (payload.kind === "prepare") {
      playMcSound("launch");
      const msg = tr(lang, "ok.prebuildDone");
      setBanner(feedback, msg, "success");
      showToast(msg, "success");
    } else {
      playMcSound("click");
      const msg = tr(lang, "ok.sessionClosed");
      setBanner(feedback, msg, "success");
      showToast(msg, "success");
    }
  });
}

void init();
