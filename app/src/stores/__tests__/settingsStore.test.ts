import { describe, it, expect, beforeEach } from "vitest";
import { useSettingsStore } from "../settingsStore";
import type { Theme } from "../settingsStore";

beforeEach(() => {
  useSettingsStore.setState({
    tabSize: 4,
    theme: "system",
  });
});

describe("initial state", () => {
  it("defaults to tabSize 4", () => {
    expect(useSettingsStore.getState().tabSize).toBe(4);
  });

  it("defaults to system theme", () => {
    expect(useSettingsStore.getState().theme).toBe("system");
  });
});

describe("setTabSize", () => {
  it("sets tab size to 2", () => {
    useSettingsStore.getState().setTabSize(2);
    expect(useSettingsStore.getState().tabSize).toBe(2);
  });

  it("sets tab size back to 4", () => {
    useSettingsStore.getState().setTabSize(2);
    useSettingsStore.getState().setTabSize(4);
    expect(useSettingsStore.getState().tabSize).toBe(4);
  });
});

describe("setTheme", () => {
  it("cycles through all valid themes", () => {
    const themes: Theme[] = ["light", "dark", "system"];
    for (const theme of themes) {
      useSettingsStore.getState().setTheme(theme);
      expect(useSettingsStore.getState().theme).toBe(theme);
    }
  });
});
