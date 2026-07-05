import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { initialSettings } from "../data/mockState";
import { SettingsPage } from "../pages/SettingsPage";

vi.mock("../lib/api", () => ({
  installModel: vi.fn(() => Promise.resolve("C:\\Users\\TOM\\AppData\\Local\\Hi-Voicer\\models\\sherpa-paraformer-zh")),
  listenModelInstallProgress: vi.fn(() => Promise.resolve(() => {})),
  openExternalUrl: vi.fn(),
  selectDirectory: vi.fn(() => Promise.resolve(null)),
}));

function renderSettings(onSettingsChange = vi.fn()) {
  const result = render(
    <SettingsPage
      settings={initialSettings}
      onOpenRecordingsFolder={vi.fn()}
      onSettingsChange={onSettingsChange}
    />,
  );
  return { ...result, onSettingsChange };
}

describe("SettingsPage", () => {
  it("captures keyboard shortcuts by pressing keys", () => {
    const { onSettingsChange } = renderSettings();

    const shortcutButton = screen.getByRole("button", { name: "CapsLock" });
    fireEvent.click(shortcutButton);
    fireEvent.keyDown(shortcutButton, { key: "K", ctrlKey: true, shiftKey: true });

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        shortcut: "Ctrl+Shift+K",
      }),
    );
  });

  it("selects a preset model and updates the selected model", () => {
    const { onSettingsChange } = renderSettings();

    fireEvent.click(screen.getByRole("button", { name: /Sherpa FunASR-Nano/ }));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        selectedModelId: "sherpa-funasr-nano",
      }),
    );
  });

  it("selects a separate transcription model", () => {
    const { container, onSettingsChange } = renderSettings();

    const modelRoleButtons = Array.from(container.querySelectorAll(".setting-row--stacked .segmented-control button"));
    fireEvent.click(modelRoleButtons[1]);
    fireEvent.click(screen.getByRole("button", { name: /Sherpa FunASR-Nano/ }));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        transcriptionModelId: "sherpa-funasr-nano",
      }),
    );
  });

  it("selects dark theme", () => {
    const { container, onSettingsChange } = renderSettings();

    const themeButtons = Array.from(container.querySelectorAll(".setting-row:first-of-type button"));
    fireEvent.click(themeButtons[1]);

    expect(onSettingsChange).toHaveBeenCalledWith(expect.objectContaining({ theme: "dark" }));
  });

  it("switches between CPU and experimental DirectML acceleration", () => {
    const { onSettingsChange } = renderSettings();

    const cpuButton = screen.getByRole("button", { name: "CPU" });
    const directMlButton = screen.getByRole("button", { name: /DirectML/ });

    expect(screen.queryByRole("button", { name: /CUDA/i })).toBeNull();

    fireEvent.click(cpuButton);
    expect(onSettingsChange).toHaveBeenCalledWith(expect.objectContaining({ accelerationMode: "cpu" }));

    fireEvent.click(directMlButton);
    expect(onSettingsChange).toHaveBeenCalledWith(expect.objectContaining({ accelerationMode: "directml" }));
  });

  it("toggles launch at startup", () => {
    const { onSettingsChange } = renderSettings();

    fireEvent.click(screen.getByLabelText("开机启动"));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        launchAtStartup: true,
      }),
    );
  });

  it("toggles mini window visibility", () => {
    const { onSettingsChange } = renderSettings();

    fireEvent.click(screen.getByLabelText("显示悬浮按钮"));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        showMiniWindow: false,
      }),
    );
  });
});
