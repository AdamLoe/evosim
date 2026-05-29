// Theme palettes for the app shell. A theme is a name + a map of
// CSS-variable → value pairs. `applyTheme(id)` writes the tokens onto the
// `<html>` element via inline `style.setProperty`, overriding the `:root`
// fallbacks defined in `styles.css`.
//
// Invariant: every theme must set every CSS var that appears in the
// `:root` block of `styles.css` so a theme switch never leaves a stale value
// painted from the previous theme. The four palette tokens that drive layout
// (--rail-w / --tab-h / --topbar-h / --profiler-h) are NOT theme-owned —
// they're layout constants, not skin colors, so they stay on `:root`.

export interface Theme {
  id: string;
  /** Display label shown in the Settings → Display dropdown. */
  name: string;
  /** CSS variable → value map. Keys must include the `--` prefix. */
  tokens: Record<string, string>;
}

export const DEFAULT_THEME_ID = "charcoal";

/**
 * Required CSS-var tokens — every theme must define each of these.
 *
 * Categories:
 *   - bg-*       background shades (app, panel, panel-alt, canvas)
 *   - fg / fg-*  text foreground shades (full, muted, faint)
 *   - border*    panel separator lines
 *   - accent*    primary brand colors (accent, accent-2 secondary, dirty)
 *   - danger     destructive-action color (Reset hover, errors)
 *   - success    positive feedback (reserved — not yet wired)
 *   - warning    in-flight / stabilizing state
 *   - info       informational hints (reserved — not yet wired)
 *   - chart-*    population graph rendering
 */
export const REQUIRED_TOKENS = [
  "--bg-app",
  "--bg-panel",
  "--bg-panel-alt",
  "--bg-canvas",
  "--fg",
  "--fg-muted",
  "--fg-faint",
  "--border",
  "--border-strong",
  "--accent",
  "--accent-2",
  "--accent-dirty",
  "--danger",
  "--success",
  "--warning",
  "--info",
  "--chart-line",
  "--chart-grid",
  // v1.9.2 renderer tokens. --grass-tint is a comma-separated rgb-float
  // triple in [0, 1] (consumed as a vec3 uniform). --creature-ring and
  // --creature-halo are standard rgba() colors.
  "--grass-tint",
  "--creature-ring",
  "--creature-halo",
] as const;

// Charcoal mirrors the current :root fallback values exactly — so users on
// the default theme see no visual change when they upgrade.
const CHARCOAL: Theme = {
  id: "charcoal",
  name: "Charcoal",
  tokens: {
    "--bg-app": "#181613",
    "--bg-panel": "#221f1a",
    "--bg-panel-alt": "#2a2620",
    "--bg-canvas": "#0c0a08",
    "--fg": "#e8e3da",
    "--fg-muted": "rgba(232, 227, 218, 0.62)",
    "--fg-faint": "rgba(232, 227, 218, 0.42)",
    "--border": "rgba(232, 227, 218, 0.08)",
    "--border-strong": "rgba(232, 227, 218, 0.16)",
    "--accent": "#67b3a9",
    "--accent-2": "#b07ad4",
    "--accent-dirty": "#d49b54",
    "--danger": "#c87063",
    "--success": "#7fbf73",
    "--warning": "#e8b85d",
    "--info": "#7fa8d4",
    "--chart-line": "#67b3a9",
    "--chart-grid": "rgba(232, 227, 218, 0.10)",
    "--grass-tint": "0.55, 0.85, 0.45",
    "--creature-ring": "rgba(0, 0, 0, 0.6)",
    "--creature-halo": "rgba(0, 0, 0, 0.25)",
  },
};

const SLATE: Theme = {
  id: "slate",
  name: "Slate",
  tokens: {
    "--bg-app": "#0f1620",
    "--bg-panel": "#172333",
    "--bg-panel-alt": "#1d2c41",
    "--bg-canvas": "#070b12",
    "--fg": "#d9e3f1",
    "--fg-muted": "rgba(217, 227, 241, 0.62)",
    "--fg-faint": "rgba(217, 227, 241, 0.42)",
    "--border": "rgba(217, 227, 241, 0.08)",
    "--border-strong": "rgba(217, 227, 241, 0.18)",
    "--accent": "#5fb7d4",
    "--accent-2": "#9c8ce0",
    "--accent-dirty": "#cf9a55",
    "--danger": "#d3667a",
    "--success": "#73c79b",
    "--warning": "#e8c266",
    "--info": "#6ec4d4",
    "--chart-line": "#5fb7d4",
    "--chart-grid": "rgba(217, 227, 241, 0.10)",
    "--grass-tint": "0.42, 0.78, 0.55",
    "--creature-ring": "rgba(0, 0, 0, 0.5)",
    "--creature-halo": "rgba(0, 0, 0, 0.28)",
  },
};

const LIGHT: Theme = {
  id: "light",
  name: "Light",
  tokens: {
    "--bg-app": "#f4f1ec",
    "--bg-panel": "#ffffff",
    "--bg-panel-alt": "#ece7dd",
    "--bg-canvas": "#fbf9f5",
    "--fg": "#1d1c1a",
    "--fg-muted": "rgba(29, 28, 26, 0.65)",
    "--fg-faint": "rgba(29, 28, 26, 0.42)",
    "--border": "rgba(29, 28, 26, 0.10)",
    "--border-strong": "rgba(29, 28, 26, 0.22)",
    "--accent": "#3a8479",
    "--accent-2": "#7a4ea3",
    "--accent-dirty": "#9b6a26",
    "--danger": "#a64537",
    "--success": "#3f8a3a",
    "--warning": "#b07a18",
    "--info": "#3e6f9e",
    "--chart-line": "#3a8479",
    "--chart-grid": "rgba(29, 28, 26, 0.12)",
    "--grass-tint": "0.62, 0.88, 0.55",
    "--creature-ring": "rgba(0, 0, 0, 0.5)",
    "--creature-halo": "rgba(0, 0, 0, 0.10)",
  },
};

const VIVID: Theme = {
  id: "vivid",
  name: "Vivid",
  tokens: {
    "--bg-app": "#0b0a14",
    "--bg-panel": "#161226",
    "--bg-panel-alt": "#211a34",
    "--bg-canvas": "#05040c",
    "--fg": "#f1ecff",
    "--fg-muted": "rgba(241, 236, 255, 0.66)",
    "--fg-faint": "rgba(241, 236, 255, 0.44)",
    "--border": "rgba(241, 236, 255, 0.10)",
    "--border-strong": "rgba(241, 236, 255, 0.22)",
    "--accent": "#e85ad1",
    "--accent-2": "#5ed4e8",
    "--accent-dirty": "#f0a23a",
    "--danger": "#ff5876",
    "--success": "#5ee896",
    "--warning": "#ffd34a",
    "--info": "#7fb6ff",
    "--chart-line": "#e85ad1",
    "--chart-grid": "rgba(241, 236, 255, 0.12)",
    "--grass-tint": "0.30, 0.92, 0.40",
    "--creature-ring": "rgba(0, 0, 0, 0.7)",
    "--creature-halo": "rgba(0, 0, 0, 0.35)",
  },
};

// New in this patch: pure-black background, very dim panels, electric blue
// accent. Designed for OLED screens / "lights off" night use.
const MIDNIGHT: Theme = {
  id: "midnight",
  name: "Midnight",
  tokens: {
    "--bg-app": "#000000",
    "--bg-panel": "#050507",
    "--bg-panel-alt": "#0c0c10",
    "--bg-canvas": "#000000",
    "--fg": "#dfe6f2",
    "--fg-muted": "rgba(223, 230, 242, 0.58)",
    "--fg-faint": "rgba(223, 230, 242, 0.36)",
    "--border": "rgba(223, 230, 242, 0.06)",
    "--border-strong": "rgba(223, 230, 242, 0.14)",
    "--accent": "#4f9eff",
    "--accent-2": "#a06bff",
    "--accent-dirty": "#d4a64a",
    "--danger": "#ff6478",
    "--success": "#5ee0a8",
    "--warning": "#ffc857",
    "--info": "#7fb6ff",
    "--chart-line": "#4f9eff",
    "--chart-grid": "rgba(223, 230, 242, 0.08)",
    "--grass-tint": "0.25, 0.55, 0.50",
    "--creature-ring": "rgba(255, 255, 255, 0.4)",
    "--creature-halo": "rgba(0, 0, 0, 0.35)",
  },
};

export const THEMES: Record<string, Theme> = {
  [CHARCOAL.id]: CHARCOAL,
  [SLATE.id]: SLATE,
  [MIDNIGHT.id]: MIDNIGHT,
  [LIGHT.id]: LIGHT,
  [VIVID.id]: VIVID,
};

/**
 * Apply a theme by id. Unknown id falls back to the default theme so a
 * stale persisted setting can't break first paint. Writes inline custom
 * properties onto `<html>` — overrides the `:root` fallbacks in styles.css.
 */
export function applyTheme(id: string): void {
  const theme = THEMES[id] ?? THEMES[DEFAULT_THEME_ID];
  const root = document.documentElement;
  for (const token of REQUIRED_TOKENS) {
    const value = theme.tokens[token];
    if (value !== undefined) {
      root.style.setProperty(token, value);
    }
  }
}
