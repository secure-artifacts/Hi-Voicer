import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { initialSettings } from "../data/mockState";
import { loadSettings, saveSettings, transcribeFile, validateModelDir } from "../lib/api";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.reject(new Error("not in tauri"))),
}));

describe("settings fallback storage", () => {
  const storage = new Map<string, string>();
  const mockedInvoke = vi.mocked(invoke);

  beforeEach(() => {
    mockedInvoke.mockReset();
    mockedInvoke.mockRejectedValue(new Error("not in tauri"));
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

  it("merges new defaults into older saved settings", async () => {
    window.localStorage.setItem("hi-voicer-settings", JSON.stringify({ shortcut: "Mouse4" }));

    await expect(loadSettings(initialSettings)).resolves.toMatchObject({
      shortcut: "Mouse4",
      recordingMode: "hold",
      accelerationMode: "cpu",
      theme: "light",
      showMiniWindow: true,
    });
  });

  it("saves and loads settings from localStorage outside Tauri", async () => {
    const next = { ...initialSettings, shortcut: "Mouse4", recordingMode: "toggle" as const, theme: "dark" as const };

    await saveSettings(next);

    await expect(loadSettings(initialSettings)).resolves.toMatchObject(next);
  });

  it("reports an empty model directory as not ready", async () => {
    await expect(validateModelDir("")).resolves.toEqual({
      valid: false,
      modelName: "",
      message: "尚未配置离线模型。",
    });
  });

  it("keeps model validation permissive in browser preview mode", async () => {
    await expect(validateModelDir("C:\\models\\demo")).resolves.toEqual({
      valid: true,
      modelName: "本地模型",
      message: "浏览器预览模式无法校验本地模型目录。",
    });
  });

  it("sends the selected export format to file transcription", async () => {
    mockedInvoke.mockResolvedValue({ text: "ok", outputPath: "demo.txt", outputPaths: ["demo.txt", "demo.srt"], outputFiles: [] });

    await transcribeFile("C:\\audio\\demo.wav", { ...initialSettings, modelDir: "C:\\models\\demo" }, { outputFormat: "srt" });

    expect(mockedInvoke).toHaveBeenCalledWith("transcribe_file", {
      request: expect.objectContaining({
        audioPath: "C:\\audio\\demo.wav",
        outputFormat: "srt",
        accelerationMode: "cpu",
        saveOutput: true,
      }),
    });
  });
});
