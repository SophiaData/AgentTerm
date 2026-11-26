// @vitest-environment jsdom

import { afterEach, describe, expect, it } from "vitest";
import {
  DEFAULT_THEME_ID,
  THEME_STORAGE_KEY,
  THEMES,
  applyDocumentTheme,
  getThemePreset,
  loadStoredTheme,
  parseStoredTheme,
  saveStoredTheme,
} from "./theme";

afterEach(() => {
  localStorage.clear();
  delete document.documentElement.dataset.theme;
});

describe("theme presets and persistence", () => {
  it("defines distinct presets with full terminal palettes", () => {
    const ids = new Set(THEMES.map((theme) => theme.id));
    expect(ids.size).toBe(THEMES.length);
    for (const theme of THEMES) {
      expect(theme.terminal.background).toBeTruthy();
      expect(theme.terminal.foreground).toBeTruthy();
      expect(theme.terminal.background).not.toBe(theme.terminal.foreground);
      expect(theme.terminal.cursor).toBeTruthy();
    }
    expect(getThemePreset("light").name).toBe("清爽浅白");
  });

  it("persists valid themes and cleans invalid stored values", () => {
    expect(loadStoredTheme()).toBe(DEFAULT_THEME_ID);
    expect(parseStoredTheme("unknown")).toBe(DEFAULT_THEME_ID);

    saveStoredTheme("nord");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("nord");
    expect(loadStoredTheme()).toBe("nord");

    localStorage.setItem(THEME_STORAGE_KEY, "bad-theme");
    expect(loadStoredTheme()).toBe(DEFAULT_THEME_ID);
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBeNull();
  });

  it("applies the selected theme to document root", () => {
    applyDocumentTheme("warm");
    expect(document.documentElement.dataset.theme).toBe("warm");
  });
});
