import type { AccentTone, CornerStyle, ThemeMode } from "./types";

interface Rgb {
  r: number;
  g: number;
  b: number;
}

interface CalibratedAccent {
  accent: string;
  hover: string;
  text: string;
  fg: string;
  focus: string;
  soft: string;
}

const DARK_THEME_DEFAULTS: Record<Exclude<ThemeMode, "Light" | "Frost" | "Cherry">, CalibratedAccent> = {
  Dark: {
    accent: "#e07d44",
    hover: "#ec8f58",
    text: "#140a04",
    fg: "#eb915c",
    focus: "#f2aa7e",
    soft: "rgba(224, 125, 68, 0.14)",
  },
  Ember: {
    accent: "#df9f3a",
    hover: "#ebb050",
    text: "#170f02",
    fg: "#eab052",
    focus: "#f3c476",
    soft: "rgba(223, 159, 58, 0.14)",
  },
  Spruce: {
    accent: "#56c27e",
    hover: "#6dd090",
    text: "#08170e",
    fg: "#68cf8d",
    focus: "#8be0aa",
    soft: "rgba(86, 194, 126, 0.14)",
  },
  Cobalt: {
    accent: "#53b8ec",
    hover: "#6cc6f3",
    text: "#06141d",
    fg: "#67c3f0",
    focus: "#90d7f9",
    soft: "rgba(83, 184, 236, 0.14)",
  },
  Nether: {
    accent: "#ea6b66",
    hover: "#f1827d",
    text: "#1a0707",
    fg: "#f2817d",
    focus: "#f7a29f",
    soft: "rgba(234, 107, 102, 0.14)",
  },
  Sculk: {
    accent: "#35cad0",
    hover: "#52d7dc",
    text: "#051618",
    fg: "#4fd8dd",
    focus: "#7de6ea",
    soft: "rgba(53, 202, 208, 0.14)",
  },
  End: {
    accent: "#e4be52",
    hover: "#edd074",
    text: "#191405",
    fg: "#ecd075",
    focus: "#f4df9a",
    soft: "rgba(228, 190, 82, 0.14)",
  },
};

const LIGHT_THEME_DEFAULTS: Record<Extract<ThemeMode, "Light" | "Frost" | "Cherry">, CalibratedAccent> = {
  Light: {
    accent: "#b34c19",
    hover: "#9d4113",
    text: "#ffffff",
    fg: "#a34414",
    focus: "#b34c19",
    soft: "rgba(179, 76, 25, 0.11)",
  },
  Frost: {
    accent: "#1d5fb8",
    hover: "#164e9a",
    text: "#ffffff",
    fg: "#1853a3",
    focus: "#1d5fb8",
    soft: "rgba(29, 95, 184, 0.11)",
  },
  Cherry: {
    accent: "#a3295d",
    hover: "#8a1f4c",
    text: "#ffffff",
    fg: "#8f204f",
    focus: "#a3295d",
    soft: "rgba(163, 41, 93, 0.11)",
  },
};

const DARK_ACCENT_PRESETS: Record<Exclude<AccentTone, "Auto" | "Custom">, CalibratedAccent> = {
  Copper: DARK_THEME_DEFAULTS.Dark,
  Emerald: DARK_THEME_DEFAULTS.Spruce,
  Cyan: DARK_THEME_DEFAULTS.Cobalt,
  Amber: DARK_THEME_DEFAULTS.Ember,
  Crimson: DARK_THEME_DEFAULTS.Nether,
  Amethyst: {
    accent: "#6495ed",
    hover: "#7ba6f2",
    text: "#071021",
    fg: "#7ca7f5",
    focus: "#a3c2f9",
    soft: "rgba(100, 149, 237, 0.14)",
  },
};

const LIGHT_ACCENT_PRESETS: Record<Exclude<AccentTone, "Auto" | "Custom">, CalibratedAccent> = {
  Copper: LIGHT_THEME_DEFAULTS.Light,
  Emerald: {
    accent: "#167940",
    hover: "#116334",
    text: "#ffffff",
    fg: "#126836",
    focus: "#167940",
    soft: "rgba(22, 121, 64, 0.11)",
  },
  Cyan: {
    accent: "#0d669b",
    hover: "#095480",
    text: "#ffffff",
    fg: "#095786",
    focus: "#0d669b",
    soft: "rgba(13, 102, 155, 0.11)",
  },
  Amber: {
    accent: "#945a00",
    hover: "#7d4b00",
    text: "#ffffff",
    fg: "#804d00",
    focus: "#945a00",
    soft: "rgba(148, 90, 0, 0.11)",
  },
  Crimson: {
    accent: "#b42b26",
    hover: "#99221e",
    text: "#ffffff",
    fg: "#9e231f",
    focus: "#b42b26",
    soft: "rgba(180, 43, 38, 0.11)",
  },
  Amethyst: {
    accent: "#2956a8",
    hover: "#1f4487",
    text: "#ffffff",
    fg: "#234b94",
    focus: "#2956a8",
    soft: "rgba(41, 86, 168, 0.11)",
  },
};

export function isLightTheme(theme: ThemeMode): boolean {
  return theme === "Light" || theme === "Frost" || theme === "Cherry";
}

export function normalizeHex(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const clean = raw.trim().replace(/^#/, "");
  if (/^[0-9a-fA-F]{6}$/.test(clean)) {
    return `#${clean.toLowerCase()}`;
  }
  return null;
}

function parseHex(hex: string): Rgb {
  const clean = hex.replace("#", "");
  return {
    r: parseInt(clean.slice(0, 2), 16),
    g: parseInt(clean.slice(2, 4), 16),
    b: parseInt(clean.slice(4, 6), 16),
  };
}

function toHex(c: Rgb): string {
  const clampByte = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
  return (
    "#" +
    [clampByte(c.r), clampByte(c.g), clampByte(c.b)]
      .map((n) => n.toString(16).padStart(2, "0"))
      .join("")
  );
}

function mixRgb(a: Rgb, b: Rgb, t: number): Rgb {
  return {
    r: a.r + (b.r - a.r) * t,
    g: a.g + (b.g - a.g) * t,
    b: a.b + (b.b - a.b) * t,
  };
}

function relativeLuminance(c: Rgb): number {
  const chan = [c.r, c.g, c.b].map((v) => {
    const s = v / 255;
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * chan[0] + 0.7152 * chan[1] + 0.0722 * chan[2];
}

function contrastRatio(a: Rgb, b: Rgb): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const hi = Math.max(la, lb);
  const lo = Math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

function computeCustomAccent(hex: string, lightSurface: boolean): CalibratedAccent {
  const base = parseHex(hex);
  const white: Rgb = { r: 255, g: 255, b: 255 };
  const darkInk: Rgb = { r: 14, g: 12, b: 22 };
  const surfaceRef: Rgb = lightSurface ? white : { r: 32, g: 28, b: 41 };

  const textRgb = contrastRatio(darkInk, base) >= contrastRatio(white, base) ? darkInk : white;
  const hoverRgb = lightSurface ? mixRgb(base, darkInk, 0.14) : mixRgb(base, white, 0.14);

  let fgRgb = base;
  const targetRef = lightSurface ? { r: 14, g: 12, b: 22 } : white;
  for (let step = 0; step <= 18 && contrastRatio(fgRgb, surfaceRef) < 4.6; step += 1) {
    fgRgb = mixRgb(base, targetRef, Math.min(0.88, step * 0.055));
  }

  const focusRgb = lightSurface ? fgRgb : mixRgb(fgRgb, white, 0.24);

  return {
    accent: toHex(base),
    hover: toHex(hoverRgb),
    text: toHex(textRgb),
    fg: toHex(fgRgb),
    focus: toHex(focusRgb),
    soft: `rgba(${base.r}, ${base.g}, ${base.b}, ${lightSurface ? "0.11" : "0.15"})`,
  };
}

export function applyDynamicDesignTokens(
  theme: ThemeMode,
  accent: AccentTone,
  customHex: string | null | undefined,
  corner: CornerStyle
): void {
  const root = document.documentElement;
  const light = isLightTheme(theme);

  const radiusMap: Record<
    CornerStyle,
    { sm: string; md: string; btn: string; pill: string }
  > = {
    Sharp: { sm: "0.22rem", md: "0.45rem", btn: "0.28rem", pill: "0.35rem" },
    Balanced: { sm: "0.55rem", md: "1rem", btn: "0.62rem", pill: "9999px" },
    Soft: { sm: "0.8rem", md: "1.35rem", btn: "0.95rem", pill: "9999px" },
  };
  const rad = radiusMap[corner] ?? radiusMap.Balanced;
  root.style.setProperty("--radius-sm", rad.sm);
  root.style.setProperty("--radius-md", rad.md);
  root.style.setProperty("--radius-btn", rad.btn);
  root.style.setProperty("--radius-pill", rad.pill);

  const props = [
    "--accent",
    "--accent-hover",
    "--accent-text",
    "--accent-fg",
    "--focus-ring",
    "--accent-soft",
  ];

  if (accent === "Auto") {
    for (const p of props) {
      root.style.removeProperty(p);
    }
    return;
  }

  let resolved: CalibratedAccent;
  if (accent === "Custom") {
    const valid = normalizeHex(customHex) ?? (light ? "#b34c19" : "#e07d44");
    resolved = computeCustomAccent(valid, light);
  } else {
    resolved = (light ? LIGHT_ACCENT_PRESETS : DARK_ACCENT_PRESETS)[accent];
  }

  root.style.setProperty("--accent", resolved.accent);
  root.style.setProperty("--accent-hover", resolved.hover);
  root.style.setProperty("--accent-text", resolved.text);
  root.style.setProperty("--accent-fg", resolved.fg);
  root.style.setProperty("--focus-ring", resolved.focus);
  root.style.setProperty("--accent-soft", resolved.soft);
}
