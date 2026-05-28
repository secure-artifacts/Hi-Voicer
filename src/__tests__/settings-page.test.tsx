import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { initialSettings } from "../data/mockState";
import { SettingsPage } from "../pages/SettingsPage";

vi.mock("../lib/api", () => ({
  openExternalUrl: vi.fn(),
  selectDirectory: vi.fn(() => Promise.resolve(null)),
}));

describe("SettingsPage", () => {
  it("captures keyboard shortcuts by pressing keys", () => {
    const onSettingsChange = vi.fn();

    render(<SettingsPage settings={initialSettings} onSettingsChange={onSettingsChange} />);

    const shortcutButton = screen.getByRole("button", { name: "CapsLock" });
    fireEvent.click(shortcutButton);
    fireEvent.keyDown(shortcutButton, { key: "K", ctrlKey: true, shiftKey: true });

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        shortcut: "Ctrl+Shift+K",
      }),
    );
  });

  it("selects a preset model and updates the download action", () => {
    const onSettingsChange = vi.fn();

    render(<SettingsPage settings={initialSettings} onSettingsChange={onSettingsChange} />);

    fireEvent.click(screen.getByRole("button", { name: /中文高精度模型/ }));

    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({
        selectedModelId: "vosk-cn-0.22",
      }),
    );
  });
});
