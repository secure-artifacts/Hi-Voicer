import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { initialDiagnostics, initialSettings } from "../data/mockState";
import { DiagnosticsPage } from "../pages/DiagnosticsPage";
import { selectAudioFiles, transcribeFile } from "../lib/api";

vi.mock("../lib/api", () => ({
  selectAudioFiles: vi.fn(() => Promise.resolve(["C:\\audio\\sample.wav"])),
  transcribeFile: vi.fn(() => Promise.resolve({ text: "测试识别结果", outputPath: "" })),
}));

describe("DiagnosticsPage", () => {
  it("runs a model smoke test without saving transcript output", async () => {
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={initialSettings} />);

    fireEvent.click(screen.getByRole("button", { name: "选择音频测试" }));

    await waitFor(() => {
      expect(transcribeFile).toHaveBeenCalledWith("C:\\audio\\sample.wav", initialSettings, { saveOutput: false });
    });
    expect(selectAudioFiles).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("测试识别结果")).toBeTruthy();
  });
});
