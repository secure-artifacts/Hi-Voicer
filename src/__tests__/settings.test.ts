import { beforeEach, describe, expect, it, vi } from "vitest";
import { initialSettings } from "../data/mockState";
import { loadSettings, saveSettings } from "../lib/api";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.reject(new Error("not in tauri"))),
}));

describe("settings fallback storage", () => {
  const storage = new Map<string, string>();

  beforeEach(() => {
    storage.clear();
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      value: {
        clear: () => storage.clear(),
        getItem: (key: string) => storage.get(key) ?? null,
        setItem: (key: string, value: string) => storage.set(key, value),
      },
    });
  });

  it("returns defaults when no saved settings exist", async () => {
    await expect(loadSettings(initialSettings)).resolves.toEqual(initialSettings);
  });

  it("saves and loads settings from localStorage outside Tauri", async () => {
    const next = { ...initialSettings, shortcut: "Mouse4", saveRecordings: true };

    await saveSettings(next);

    await expect(loadSettings(initialSettings)).resolves.toMatchObject(next);
  });
});
