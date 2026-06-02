import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { initialDiagnostics, initialSettings } from "../data/mockState";
import { prepareAccelerationRuntime, runAccelerationSmokeTest, saveTextFile, selectAudioFiles, transcribeFile } from "../lib/api";
import { DiagnosticsPage } from "../pages/DiagnosticsPage";

vi.mock("../lib/api", () => ({
  getAccelerationStatus: vi.fn(() =>
    Promise.resolve({
      selectedMode: "cpu",
      effectiveMode: "cpu",
      cudaAvailable: false,
      cudaDeviceSummary: null,
      cudaDetectionError: "nvidia-smi not found",
      cpuRuntimeInstalled: false,
      cudaRuntimeInstalled: false,
      message: "CPU selected",
    }),
  ),
  prepareAccelerationRuntime: vi.fn(() =>
    Promise.resolve({
      selectedMode: "cuda",
      effectiveMode: "cuda",
      cudaAvailable: true,
      cudaDeviceSummary: "NVIDIA GeForce RTX 4070 / driver 552.44 / VRAM 12282 MB",
      cudaDetectionError: null,
      cpuRuntimeInstalled: true,
      cudaRuntimeInstalled: true,
      message: "CUDA ready",
    }),
  ),
  runAccelerationSmokeTest: vi.fn(() =>
    Promise.resolve({
      requestedMode: "cuda",
      usedMode: "cuda",
      fallbackUsed: false,
      elapsedMs: 123,
      transcriptPreview: "",
      message: "CUDA smoke ok",
    }),
  ),
  saveTextFile: vi.fn(() => Promise.resolve("C:\\reports\\gpu.txt")),
  selectAudioFiles: vi.fn(() => Promise.resolve(["C:\\audio\\sample.wav"])),
  transcribeFile: vi.fn(() => Promise.resolve({ text: "recognized", outputPath: "" })),
}));

describe("DiagnosticsPage", () => {
  it("runs a model smoke test without saving transcript output", async () => {
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={initialSettings} />);

    fireEvent.click(screen.getAllByRole("button")[0]);

    await waitFor(() => {
      expect(transcribeFile).toHaveBeenCalledWith("C:\\audio\\sample.wav", initialSettings, { saveOutput: false });
    });
    expect(selectAudioFiles).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("recognized")).toBeTruthy();
  });

  it("shows the current acceleration status", async () => {
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={initialSettings} />);

    expect(await screen.findByText("CPU selected")).toBeTruthy();
    expect(screen.getByText("nvidia-smi not found")).toBeTruthy();
    expect(screen.getByText("GPU 加速")).toBeTruthy();
  });

  it("prepares CUDA runtime when CUDA acceleration is selected", async () => {
    render(
      <DiagnosticsPage
        items={initialDiagnostics}
        modelReady={true}
        settings={{ ...initialSettings, accelerationMode: "cuda" }}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: /准备 CUDA 运行时/ }));

    await waitFor(() => {
      expect(prepareAccelerationRuntime).toHaveBeenCalledWith("cuda");
    });
    expect(await screen.findByText("CUDA ready")).toBeTruthy();
    expect(screen.getByText(/RTX 4070/)).toBeTruthy();
  });

  it("runs acceleration smoke test with the current settings", async () => {
    const settings = { ...initialSettings, modelDir: "C:\\models\\demo", accelerationMode: "cuda" as const };
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={settings} />);

    fireEvent.click(screen.getByRole("button", { name: /运行加速 smoke test/ }));

    await waitFor(() => {
      expect(runAccelerationSmokeTest).toHaveBeenCalledWith(settings);
    });
    expect(await screen.findByText(/CUDA smoke ok/)).toBeTruthy();
    expect(screen.getByText(/实际路径：CUDA/)).toBeTruthy();
  });

  it("saves a GPU diagnostic report with acceleration details", async () => {
    const settings = { ...initialSettings, modelDir: "C:\\models\\demo", accelerationMode: "cuda" as const };
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={settings} />);

    fireEvent.click(screen.getByRole("button", { name: /运行加速 smoke test/ }));
    await screen.findByText(/CUDA smoke ok/);

    fireEvent.click(screen.getByRole("button", { name: /保存 GPU 诊断报告/ }));

    await waitFor(() => {
      expect(saveTextFile).toHaveBeenCalledWith(
        expect.stringMatching(/^hi-voicer-gpu-diagnostics-.+\.txt$/),
        expect.stringContaining("Hi-Voicer GPU 诊断报告"),
      );
    });
    expect(saveTextFile).toHaveBeenCalledWith(expect.any(String), expect.stringContaining("实际路径: cuda"));
    expect(await screen.findByText(/GPU 诊断报告已保存/)).toBeTruthy();
  });
});
