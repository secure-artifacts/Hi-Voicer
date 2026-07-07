import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { initialDiagnostics, initialSettings } from "../data/mockState";
import {
  getNativeAudioDiagnostics,
  runAccelerationSmokeTest,
  runDirectMlProbe,
  saveTextFile,
  selectAudioFiles,
  transcribeFile,
} from "../lib/api";
import { DiagnosticsPage } from "../pages/DiagnosticsPage";
import type { UserSettings } from "../types";

vi.mock("../lib/api", () => ({
  getAccelerationStatus: vi.fn(() =>
    Promise.resolve({
      selectedMode: "cpu",
      effectiveMode: "cpu",
      cudaAvailable: false,
      cudaDeviceSummary: null,
      cudaDetectionError: null,
      cpuRuntimeInstalled: true,
      cudaRuntimeInstalled: false,
      cudaDisabledReason: null,
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
  runAccelerationSmokeTest: vi.fn(() =>
    Promise.resolve({
      requestedMode: "cpu",
      usedMode: "cpu",
      fallbackUsed: false,
      elapsedMs: 123,
      transcriptPreview: "",
      message: "CPU smoke test completed; silent audio does not need recognized text.",
    }),
  ),
  runDirectMlProbe: vi.fn(() =>
    Promise.resolve({
      directmlCandidate: true,
      providerSessionReady: true,
      providerSessionError: null,
      splitModelReady: true,
      splitModelDir: "C:\models\sensevoice-directml",
      splitModelMissingFiles: [],
      splitModelSessionReady: true,
      splitModelSessionError: null,
      splitModelInputs: ["encoder speech: Tensor<Float32>(1, 3000, 80)", "ctc hidden: Tensor<Float32>(1, 750, 512)"],
      splitModelOutputs: ["encoder out: Tensor<Float32>(1, 750, 512)", "ctc logits: Tensor<Float32>(1, 750, 250000)"],
      modelReady: true,
      directmlSessionReady: true,
      directmlSessionError: null,
      onnxRuntimeBuild: "ORT Build Info: DirectML test build",
      modelInputs: ["speech: Tensor<Float32>(1, feat_len, 80)", "language: Tensor<Int64>(1)"],
      modelOutputs: ["logits: Tensor<Float32>(1, dyn, 250000)"],
      modelId: "sensevoice-small",
      modelName: "SenseVoiceSmall",
      modelDir: "C:\\models\\sensevoice-small",
      missingFiles: [],
      adapters: [
        {
          name: "NVIDIA GeForce RTX 3060",
          driverVersion: "31.0.15.8195",
          adapterRamMb: 4096,
          status: "OK",
        },
      ],
      elapsedMs: 45,
      message: "DirectML SenseVoice session created; inputs: 1, outputs: 1",
      nextStep: "Add the DirectML audio feature-extraction and decoder path behind an experimental toggle.",
    }),
  ),
  saveTextFile: vi.fn(() => Promise.resolve("C:\\reports\\diagnostics.txt")),
  selectAudioFiles: vi.fn(() => Promise.resolve(["C:\\audio\\sample.wav"])),
  transcribeFile: vi.fn((_audioPath: string, settings: { accelerationMode: string }) =>
    Promise.resolve({
      text: settings.accelerationMode === "directml" ? "recognized" : "recognized",
      outputPath: "",
      outputPaths: [],
      outputFiles: [],
      segments: [{ id: "1", index: 1, start: 0, end: 10, text: "recognized", sourceAudioPath: "C:\\audio\\sample.wav" }],
      timelineKind: "estimated",
      sourceAudioPath: "C:\\audio\\sample.wav",
      usedAccelerationMode: settings.accelerationMode,
      accelerationFallbackUsed: false,
    }),
  ),
}));

describe("DiagnosticsPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("runs a model smoke test without saving transcript output", async () => {
    const { container } = render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={initialSettings} />);

    fireEvent.click(container.querySelector(".primary-button") as HTMLButtonElement);

    await waitFor(() => {
      expect(transcribeFile).toHaveBeenCalledWith("C:\\audio\\sample.wav", initialSettings, { saveOutput: false });
    });
    expect(selectAudioFiles).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("recognized")).toBeTruthy();
  });

  it("shows the current acceleration runtime status", async () => {
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={initialSettings} />);

    expect(await screen.findByText("CPU selected")).toBeTruthy();
    expect(screen.getByText("cpu")).toBeTruthy();
    expect(screen.queryByText(/NVIDIA|CUDA/)).toBeNull();
  });

  it("shows native audio diagnostics and refreshes them", async () => {
    const { container } = render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={initialSettings} />);

    expect(await screen.findByText(/USB Microphone/)).toBeTruthy();
    expect(screen.getByText(/Speakers/)).toBeTruthy();
    expect(screen.getByText(/WASAPI loopback/)).toBeTruthy();
    expect(screen.getByText(/ffmpeg.exe was not found/)).toBeTruthy();
    expect(screen.getByText(/system PATH/)).toBeTruthy();

    const refreshButton = container.querySelectorAll(".diagnostic-tool .secondary-button")[0] as HTMLButtonElement;
    fireEvent.click(refreshButton);

    await waitFor(() => {
      expect(getNativeAudioDiagnostics).toHaveBeenCalledTimes(2);
    });
  });

  it("runs the DirectML PoC probe with the current settings", async () => {
    const settings = { ...initialSettings, modelDir: "C:\\models\\sensevoice-small" };
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={settings} />);

    fireEvent.click(screen.getByRole("button", { name: /DirectML PoC probe/ }));

    await waitFor(() => {
      expect(runDirectMlProbe).toHaveBeenCalledWith(settings);
    });
    expect(await screen.findAllByText(/DirectML SenseVoice session created/)).toHaveLength(2);
    expect(screen.getByText(/Minimal ONNX session created/)).toBeTruthy();
    expect(screen.getByText(/Encoder and CTC warmups completed/)).toBeTruthy();
    expect(screen.getByText(/encoder speech: Tensor<Float32>/)).toBeTruthy();
    expect(screen.getByText(/language: Tensor<Int64>/)).toBeTruthy();
    expect(screen.getByText("NVIDIA GeForce RTX 3060")).toBeTruthy();
  });

  it("shows Qwen DirectML chain probe results", async () => {
    vi.mocked(runDirectMlProbe).mockResolvedValueOnce({
      directmlCandidate: true,
      providerSessionReady: true,
      providerSessionError: null,
      splitModelReady: false,
      splitModelDir: null,
      splitModelMissingFiles: [],
      splitModelSessionReady: false,
      splitModelSessionError: null,
      splitModelInputs: [],
      splitModelOutputs: [],
      modelReady: true,
      directmlSessionReady: true,
      directmlSessionError: null,
      onnxRuntimeBuild: "ORT Build Info: Qwen DirectML test build",
      modelInputs: ["qwen conv input_features: Tensor<f32>(batch, n_frames, 128)", "qwen decoder input_ids: Tensor<i64>(batch, seq_len)"],
      modelOutputs: ["qwen decoder logits: Tensor<f32>(batch, seq_len, vocab)"],
      modelId: "qwen3-asr-0.6b",
      modelName: "Qwen3-ASR 0.6B",
      modelDir: "C:\\models\\qwen3-asr-0.6b",
      missingFiles: [],
      adapters: [
        {
          name: "NVIDIA GeForce RTX 3060",
          driverVersion: "31.0.15.8195",
          adapterRamMb: 4096,
          status: "OK",
        },
      ],
      elapsedMs: 88,
      message: "DirectML Qwen3-ASR 0.6B conv->encoder->decoder smoke completed; logits shape: 1x1x151936",
      nextStep: "DirectML Qwen3-ASR 0.6B chain loads, but keep the stable Sherpa path until feature parity, decoded text quality, and decoder speed are proven on real samples.",
    });
    const settings = { ...initialSettings, modelDir: "C:\\models\\qwen3-asr-0.6b" };
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={settings} />);

    fireEvent.click(screen.getByRole("button", { name: /DirectML PoC probe/ }));

    await waitFor(() => {
      expect(runDirectMlProbe).toHaveBeenCalledWith(settings);
    });
    expect(await screen.findByText("Qwen DirectML chain")).toBeTruthy();
    expect(screen.getByText(/Conv, encoder, and decoder single-step warmup completed/)).toBeTruthy();
    expect(screen.getByText("Qwen3-ASR 0.6B")).toBeTruthy();
    expect(screen.getByText("Qwen chain inputs")).toBeTruthy();
    expect(screen.getByText(/qwen decoder input_ids/)).toBeTruthy();
  });
  it("runs the CPU smoke test with the current settings", async () => {
    const settings = { ...initialSettings, modelDir: "C:\\models\\demo" };
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={settings} />);

    fireEvent.click(screen.getByRole("button", { name: /CPU smoke test/ }));

    await waitFor(() => {
      expect(runAccelerationSmokeTest).toHaveBeenCalledWith(settings);
    });
    expect(await screen.findByText(/CPU smoke test completed/)).toBeTruthy();
    expect(screen.getByText(/实际路径：CPU/)).toBeTruthy();
  });

  it("saves a diagnostic report with CPU runtime and native audio details", async () => {
    const settings = { ...initialSettings, modelDir: "C:\\models\\demo" };
    const { container } = render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={settings} />);

    fireEvent.click(screen.getByRole("button", { name: /CPU smoke test/ }));
    await screen.findByText(/CPU smoke test completed/);

    const buttons = container.querySelectorAll(".diagnostic-tool .secondary-button");
    fireEvent.click(buttons[buttons.length - 1] as HTMLButtonElement);

    await waitFor(() => {
      expect(saveTextFile).toHaveBeenCalledWith(
        expect.stringMatching(/^hi-voicer-diagnostics-.+\.txt$/),
        expect.stringContaining("Hi-Voicer"),
      );
    });
    expect(saveTextFile).toHaveBeenCalledWith(expect.any(String), expect.stringContaining("实际路径: cpu"));
    expect(saveTextFile).toHaveBeenCalledWith(expect.any(String), expect.stringContaining("USB Microphone"));
  });

  it("runs a CPU vs DirectML benchmark on a selected audio file", async () => {
    const settings = { ...initialSettings, modelDir: "C:\\models\\sensevoice-small" };
    const onSettingsChange = vi.fn();
    render(<DiagnosticsPage items={initialDiagnostics} modelReady={true} settings={settings} onSettingsChange={onSettingsChange} />);
    let currentBenchmarkMode: string | null = null;
    vi.mocked(transcribeFile).mockImplementation((_audioPath: string, benchmarkSettings: UserSettings) => {
      currentBenchmarkMode = benchmarkSettings.accelerationMode;
      return Promise.resolve({
        text: "recognized",
        outputPath: "",
        outputPaths: [],
        outputFiles: [],
        segments: [{ id: "1", index: 1, start: 0, end: 10, text: "recognized", sourceAudioPath: "C:\\audio\\sample.wav" }],
        timelineKind: "estimated",
        sourceAudioPath: "C:\\audio\\sample.wav",
        usedAccelerationMode: benchmarkSettings.accelerationMode,
        accelerationFallbackUsed: false,
      });
    });
    vi.spyOn(performance, "now").mockImplementation(() => {
      if (currentBenchmarkMode === "cpu") return 10000;
      if (currentBenchmarkMode === "directml") return 11000;
      return 0;
    });

    fireEvent.click(screen.getByRole("button", { name: /CPU vs DirectML benchmark/ }));

    await waitFor(() => {
      expect(transcribeFile).toHaveBeenCalledWith(
        "C:\\audio\\sample.wav",
        expect.objectContaining({ accelerationMode: "cpu" }),
        expect.objectContaining({ saveOutput: false, performanceMode: "stable" }),
      );
      expect(transcribeFile).toHaveBeenCalledWith(
        "C:\\audio\\sample.wav",
        expect.objectContaining({ accelerationMode: "directml" }),
        expect.objectContaining({ saveOutput: false, performanceMode: "stable" }),
      );
    });
    expect(await screen.findByText("Benchmark verdict")).toBeTruthy();
    expect(screen.getByText(/Decision metrics/)).toBeTruthy();
    expect(onSettingsChange).toHaveBeenCalledWith(
      expect.objectContaining({ directmlVerified: true, directmlVerifiedAt: expect.any(String) }),
    );
  });

});

