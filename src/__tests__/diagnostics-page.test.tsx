import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { initialDiagnostics, initialSettings } from "../data/mockState";
import {
  getNativeAudioDiagnostics,
  prepareAccelerationRuntime,
  runAccelerationSmokeTest,
  saveTextFile,
  selectAudioFiles,
  transcribeFile,
} from "../lib/api";
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
  getNativeAudioDiagnostics: vi.fn(() =>
    Promise.resolve({
      microphoneAvailable: true,
      microphoneName: "USB Microphone",
      microphoneDetail: "48000 Hz / 1 channel(s) / F32",
      systemAudioAvailable: true,
      systemAudioName: "Speakers",
      systemAudioDetail:
        "48000 Hz / 2 channel(s) / F32. Output device detected; system-audio recording still depends on WASAPI loopback support and will be verified when recording starts.",
      ffmpegInstalled: false,
      ffmpegPath: null,
      ffmpegDetail:
        "ffmpeg.exe was not found. Place ffmpeg.exe under one of these folders, or add ffmpeg to PATH: C:\\Users\\tester\\AppData\\Local\\com.local.hivoicer\\engines\\ffmpeg | system PATH. Hi-Voicer will not download ffmpeg automatically.",
      message: "Native audio environment needs attention before all recording and processing modes are available.",
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
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("runs a model smoke test without saving transcript output", async () => {
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={initialSettings} />);

    fireEvent.click(screen.getByRole("button", { name: /选择音频测试/ }));

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

  it("shows native audio diagnostics and refreshes them", async () => {
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={initialSettings} />);

    expect(await screen.findByText(/USB Microphone/)).toBeTruthy();
    expect(screen.getByText(/Speakers/)).toBeTruthy();
    expect(screen.getByText(/WASAPI loopback/)).toBeTruthy();
    expect(screen.getByText(/ffmpeg.exe was not found/)).toBeTruthy();
    expect(screen.getByText(/system PATH/)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /刷新音频环境诊断/ }));

    await waitFor(() => {
      expect(getNativeAudioDiagnostics).toHaveBeenCalledTimes(2);
    });
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

  it("saves a diagnostic report with acceleration and native audio details", async () => {
    const settings = { ...initialSettings, modelDir: "C:\\models\\demo", accelerationMode: "cuda" as const };
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={settings} />);

    fireEvent.click(screen.getByRole("button", { name: /运行加速 smoke test/ }));
    await screen.findByText(/CUDA smoke ok/);

    fireEvent.click(screen.getByRole("button", { name: /保存诊断报告/ }));

    await waitFor(() => {
      expect(saveTextFile).toHaveBeenCalledWith(
        expect.stringMatching(/^hi-voicer-diagnostics-.+\.txt$/),
        expect.stringContaining("Hi-Voicer 诊断报告"),
      );
    });
    expect(saveTextFile).toHaveBeenCalledWith(expect.any(String), expect.stringContaining("实际路径: cuda"));
    expect(saveTextFile).toHaveBeenCalledWith(expect.any(String), expect.stringContaining("[本机音频环境]"));
    expect(saveTextFile).toHaveBeenCalledWith(expect.any(String), expect.stringContaining("麦克风设备: USB Microphone"));
  });
});
