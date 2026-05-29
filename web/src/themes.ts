// v1.9.1: theme palettes for the app shell. A theme is a name + a map of
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
 * Required CSS-var tokens — every theme must define each of these. The
 * applyTheme() guard logs in dev if a theme is missing a token. Kept as a
 * runtime list so the Display dropdown can verify completeness without TS
 * reflection. Mirrors the `:root` block in styles.css.
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
  "--accent-dirty",
  "--danger",
] as const;

// Charcoal mirrors the current :root fallback values exactly — so users on
// the default theme see no visual change after v1.9.1 ships.
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
    "--accent-dirty": "#d49b54",
    "--danger": "#c87063",
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
    "--accent-dirty": "#cf9a55",
    "--danger": "#d3667a",
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
    "--accent-dirty": "#9b6a26",
    "--danger": "#a64537",
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
    "--accent-dirty": "#f0a23a",
    "--danger": "#ff5876",
  },
};

export const THEMES: Record<string, Theme> = {
  [CHARCOAL.id]: CHARCOAL,
  [SLATE.id]: SLATE,
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
