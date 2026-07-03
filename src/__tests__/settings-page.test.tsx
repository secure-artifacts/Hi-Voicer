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
  render(
    <SettingsPage
      settings={initialSettings}
      onOpenRecordingsFolder={vi.fn()}
      onSettingsChange={onSettingsChange}
    />,
  );
  return onSettingsChange;
}

describe("SettingsPage", () => {
  it("captures keyboard shortcuts by pressing keys", () => {
    const onSettingsChange = renderSettings();

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
    const onSettingsChange = renderSettings();

    fireEvent.click(screen.getByRole("button", { name: /Sherpa FunASR-Nano/ }));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        selectedModelId: "sherpa-funasr-nano",
      }),
    );
  });

  it("selects a separate transcription model", () => {
    const onSettingsChange = renderSettings();

    fireEvent.click(screen.getByRole("group", { name: "模型用途" }).querySelectorAll("button")[1]);
    fireEvent.click(screen.getByRole("button", { name: /Sherpa FunASR-Nano/ }));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        transcriptionModelId: "sherpa-funasr-nano",
      }),
    );
  });

  it("selects dark theme", () => {
    const onSettingsChange = renderSettings();

    fireEvent.click(screen.getByRole("button", { name: "暗色" }));

    expect(onSettingsChange).toHaveBeenCalledWith(expect.objectContaining({ theme: "dark" }));
  });

  it("selects CUDA acceleration without preparing or downloading runtime files", () => {
    const onSettingsChange = renderSettings();

    const cudaButton = screen.getByRole("button", { name: "CUDA (experimental)" });

    expect(cudaButton).not.toHaveProperty("disabled", true);
    fireEvent.click(cudaButton);

    expect(onSettingsChange).toHaveBeenCalledWith(expect.objectContaining({ accelerationMode: "cuda" }));
    expect(cudaButton.getAttribute("title")).toMatch(/will not download CUDA files automatically/i);
  });

  it("toggles launch at startup", () => {
    const onSettingsChange = renderSettings();

    fireEvent.click(screen.getByLabelText("开机启动"));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        launchAtStartup: true,
      }),
    );
  });

  it("toggles mini window visibility", () => {
    const onSettingsChange = renderSettings();

    fireEvent.click(screen.getByLabelText("显示悬浮按钮"));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        showMiniWindow: false,
      }),
    );
  });
});
