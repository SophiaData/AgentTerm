import type { ITheme } from "@xterm/xterm";

export type ThemeId = "dark" | "light" | "nord" | "warm" | "forest";

export type ThemePreset = {
  id: ThemeId;
  name: string;
  description: string;
  swatch: string;
  terminal: ITheme;
};

export const THEME_STORAGE_KEY = "agentterm-theme";
export const DEFAULT_THEME_ID: ThemeId = "dark";

export const THEMES: readonly ThemePreset[] = [
  {
    id: "dark",
    name: "深墨默认",
    description: "经典暗夜高对比与薄荷绿强调",
    swatch: "#0d1117",
    terminal: {
      background: "#0d1117",
      foreground: "#e6edf3",
      cursor: "#8cdec2",
      cursorAccent: "#0d1117",
      selectionBackground: "#26404f",
      black: "#484f58",
      red: "#ff7b72",
      green: "#3fb950",
      yellow: "#d29922",
      blue: "#58a6ff",
      magenta: "#bc8cff",
      cyan: "#39c5cf",
      white: "#b1bac4",
      brightBlack: "#6e7681",
      brightRed: "#ffa198",
      brightGreen: "#56d364",
      brightYellow: "#e3b341",
      brightBlue: "#79c0ff",
      brightMagenta: "#d2a8ff",
      brightCyan: "#56d4dd",
      brightWhite: "#f0f6fc",
    },
  },
  {
    id: "light",
    name: "清爽浅白",
    description: "日间明亮纸白与高对比墨色文字",
    swatch: "#ffffff",
    terminal: {
      background: "#ffffff",
      foreground: "#1f2328",
      cursor: "#0969da",
      cursorAccent: "#ffffff",
      selectionBackground: "#b6e3ff",
      black: "#24292f",
      red: "#cf222e",
      green: "#116329",
      yellow: "#9a6700",
      blue: "#0969da",
      magenta: "#8250df",
      cyan: "#1b7c83",
      white: "#57606a",
      brightBlack: "#6e7781",
      brightRed: "#a40e26",
      brightGreen: "#1a7f37",
      brightYellow: "#7d4e00",
      brightBlue: "#218bff",
      brightMagenta: "#a475f9",
      brightCyan: "#3192aa",
      brightWhite: "#8c959f",
    },
  },
  {
    id: "nord",
    name: "北欧冷蓝",
    description: "柔和冰川蓝灰，长时间阅读不刺眼",
    swatch: "#2e3440",
    terminal: {
      background: "#242933",
      foreground: "#e5e9f0",
      cursor: "#88c0d0",
      cursorAccent: "#242933",
      selectionBackground: "#434c5e",
      black: "#3b4252",
      red: "#bf616a",
      green: "#a3be8c",
      yellow: "#ebcb8b",
      blue: "#81a1c1",
      magenta: "#b48ead",
      cyan: "#88c0d0",
      white: "#d8dee9",
      brightBlack: "#616e88",
      brightRed: "#d08770",
      brightGreen: "#b5cfa0",
      brightYellow: "#f0d59e",
      brightBlue: "#8fbcbb",
      brightMagenta: "#c8a3c1",
      brightCyan: "#9dd0df",
      brightWhite: "#eceff4",
    },
  },
  {
    id: "warm",
    name: "暖棕护眼",
    description: "深咖啡暖调底色与琥珀金光强调",
    swatch: "#26211e",
    terminal: {
      background: "#1c1917",
      foreground: "#ede7e0",
      cursor: "#e8b87d",
      cursorAccent: "#1c1917",
      selectionBackground: "#44382f",
      black: "#44403c",
      red: "#f87171",
      green: "#86efac",
      yellow: "#fbbf24",
      blue: "#93c5fd",
      magenta: "#d8b4fe",
      cyan: "#67e8f9",
      white: "#d6d3d1",
      brightBlack: "#78716c",
      brightRed: "#fca5a5",
      brightGreen: "#bbf7d0",
      brightYellow: "#fcd34d",
      brightBlue: "#bfdbfe",
      brightMagenta: "#e9d5ff",
      brightCyan: "#a5f3fc",
      brightWhite: "#fafaf9",
    },
  },
  {
    id: "forest",
    name: "暮夜松绿",
    description: "低饱和松墨绿底色与翡翠高亮",
    swatch: "#13221d",
    terminal: {
      background: "#0e1714",
      foreground: "#e3efe9",
      cursor: "#6ee7b7",
      cursorAccent: "#0e1714",
      selectionBackground: "#224236",
      black: "#2d4038",
      red: "#f87171",
      green: "#4ade80",
      yellow: "#facc15",
      blue: "#60a5fa",
      magenta: "#c084fc",
      cyan: "#2dd4bf",
      white: "#cbd5e1",
      brightBlack: "#64748b",
      brightRed: "#fca5a5",
      brightGreen: "#86efac",
      brightYellow: "#fde047",
      brightBlue: "#93c5fd",
      brightMagenta: "#d8b4fe",
      brightCyan: "#5eead4",
      brightWhite: "#f8fafc",
    },
  },
];

export function isThemeId(value: unknown): value is ThemeId {
  return THEMES.some((theme) => theme.id === value);
}

export function getThemePreset(id: ThemeId): ThemePreset {
  return THEMES.find((theme) => theme.id === id) ?? THEMES[0];
}

export function parseStoredTheme(raw: string | null): ThemeId {
  return isThemeId(raw) ? raw : DEFAULT_THEME_ID;
}

export function loadStoredTheme(): ThemeId {
  try {
    const raw = localStorage.getItem(THEME_STORAGE_KEY);
    if (raw === null) return DEFAULT_THEME_ID;
    if (isThemeId(raw)) return raw;
    localStorage.removeItem(THEME_STORAGE_KEY);
    return DEFAULT_THEME_ID;
  } catch {
    return DEFAULT_THEME_ID;
  }
}

export function saveStoredTheme(id: ThemeId): void {
  try {
    localStorage.setItem(THEME_STORAGE_KEY, id);
  } catch {
    // Theme persistence is optional; the in-memory theme still applies.
  }
}

export function applyDocumentTheme(id: ThemeId): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.theme = id;
}
