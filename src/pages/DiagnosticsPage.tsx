import { FileAudio, FileDown, TestTube2 } from "lucide-react";
import { useEffect, useState } from "react";
import {
  getAccelerationStatus,
  getNativeAudioDiagnostics,
  runAccelerationSmokeTest,
  runDirectMlProbe,
  saveTextFile,
  selectAudioFiles,
  transcribeFile,
} from "../lib/api";
import type {
  AccelerationMode,
  AccelerationSmokeTestResult,
  AccelerationStatus,
  DiagnosticItem,
  DirectMlProbeResult,
  NativeAudioDiagnostics,
  TranscribeFileResult,
  UserSettings,
} from "../types";

interface DiagnosticsPageProps {
  items: DiagnosticItem[];
  modelReady: boolean;
  settings: UserSettings;
  onSettingsChange?: (settings: UserSettings) => void;
}

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

interface AccelerationBenchmarkRun {
  mode: AccelerationMode;
  elapsedMs: number;
  usedMode: AccelerationMode | "none";
  fallbackUsed: boolean;
  durationSeconds: number | null;
  rtf: number | null;
  text: string;
  error?: string;
}

interface AccelerationBenchmarkResult {
  audioPath: string;
  cpu: AccelerationBenchmarkRun;
  directml: AccelerationBenchmarkRun;
  speedup: number | null;
  textSimilarity: number | null;
  textComparable: boolean;
  verdict: "ready" | "experimental";
  recommendation: string;
}

function durationSecondsFromResult(result: TranscribeFileResult) {
  const ends = result.segments?.map((segment) => segment.end).filter((value) => Number.isFinite(value) && value > 0) ?? [];
  return ends.length > 0 ? Math.max(...ends) : null;
}

function normalizeBenchmarkText(text: string) {
  return text.toLocaleLowerCase().replace(/\s+/g, "").trim().slice(0, 2000);
}

function editDistance(a: string, b: string) {
  if (a === b) return 0;
  if (!a) return b.length;
  if (!b) return a.length;

  let previous = Array.from({ length: b.length + 1 }, (_, index) => index);
  for (let i = 1; i <= a.length; i += 1) {
    const current = [i];
    for (let j = 1; j <= b.length; j += 1) {
      current[j] = Math.min(
        previous[j] + 1,
        current[j - 1] + 1,
        previous[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1),
      );
    }
    previous = current;
  }
  return previous[b.length];
}

function textSimilarity(a: string, b: string) {
  const left = normalizeBenchmarkText(a);
  const right = normalizeBenchmarkText(b);
  const maxLength = Math.max(left.length, right.length);
  if (maxLength === 0) return 1;
  return Math.max(0, 1 - editDistance(left, right) / maxLength);
}

async function benchmarkTranscription(audioPath: string, settings: UserSettings, mode: AccelerationMode): Promise<AccelerationBenchmarkRun> {
  const startedAt = performance.now();
  try {
    const result = await transcribeFile(audioPath, { ...settings, accelerationMode: mode }, { saveOutput: false, performanceMode: "stable" });
    const elapsedMs = Math.round(performance.now() - startedAt);
    const durationSeconds = durationSecondsFromResult(result);
    return {
      mode,
      elapsedMs,
      usedMode: result.usedAccelerationMode ?? mode,
      fallbackUsed: result.accelerationFallbackUsed ?? result.usedAccelerationMode !== mode,
      durationSeconds,
      rtf: durationSeconds ? elapsedMs / 1000 / durationSeconds : null,
      text: result.text ?? "",
    };
  } catch (error) {
    const elapsedMs = Math.round(performance.now() - startedAt);
    return {
      mode,
      elapsedMs,
      usedMode: "none",
      fallbackUsed: true,
      durationSeconds: null,
      rtf: null,
      text: "",
      error: error instanceof Error ? error.message : "Transcription benchmark failed.",
    };
  }
}

function buildBenchmarkResult(audioPath: string, cpu: AccelerationBenchmarkRun, directml: AccelerationBenchmarkRun): AccelerationBenchmarkResult {
  const speedup = directml.elapsedMs > 0 && !directml.error ? cpu.elapsedMs / directml.elapsedMs : null;
  const cpuText = cpu.text.trim();
  const directmlText = directml.text.trim();
  const textComparable = cpuText.length > 0 && directmlText.length > 0;
  const similarity = textComparable ? textSimilarity(cpuText, directmlText) : null;
  const ready =
    !cpu.error &&
    !directml.error &&
    directml.usedMode === "directml" &&
    !directml.fallbackUsed &&
    (speedup ?? 0) >= 1.25 &&
    (similarity ?? 0) >= 0.9 &&
    textComparable;

  const recommendation = ready
    ? "DirectML is a good candidate on this machine for this model and sample. Validate with at least two longer real recordings before making it the default."
    : !textComparable
      ? "Keep DirectML experimental. The CPU and DirectML outputs must both contain text before quality can be compared; rerun with a real speech sample or fix the empty baseline."
      : "Keep DirectML experimental. It either fell back, failed, was not meaningfully faster, or produced text that differs too much from CPU.";

  return {
    audioPath,
    cpu,
    directml,
    speedup,
    textSimilarity: similarity,
    textComparable,
    verdict: ready ? "ready" : "experimental",
    recommendation,
  };
}

function formatRatio(value: number | null) {
  return value === null ? "n/a" : value.toFixed(2) + "x";
}

function formatRtf(value: number | null) {
  return value === null ? "n/a" : value.toFixed(2);
}

function formatSimilarity(value: number | null) {
  return value === null ? "n/a" : Math.round(value * 100) + "%";
}

function buildDiagnosticReport(
  settings: UserSettings,
  modelReady: boolean,
  items: DiagnosticItem[],
  status: AccelerationStatus | null,
  smokeResult: AccelerationSmokeTestResult | null,
  audioDiagnostics: NativeAudioDiagnostics | null,
  directMlProbeResult: DirectMlProbeResult | null,
  accelerationBenchmarkResult: AccelerationBenchmarkResult | null,
) {
  const lines = [
    "Hi-Voicer 诊断报告",
    `生成时间: ${new Date().toISOString()}`,
    "",
    "[设置]",
    `模型目录: ${settings.modelDir || "(未配置)"}`,
    `识别路径: ${settings.accelerationMode}`,
    `模型可用: ${modelReady ? "是" : "否"}`,
    "",
    "[基础诊断]",
    ...items.map((item) => `${item.label}: ${item.status} - ${item.detail}`),
    "",
    "[识别运行时]",
  ];

  if (status) {
    lines.push(
      `选择路径: ${status.selectedMode}`,
      `实际路径: ${status.effectiveMode}`,
      `CPU runtime: ${status.cpuRuntimeInstalled ? "已安装" : "随模型准备"}`,
      `状态消息: ${status.message}`,
    );
  } else {
    lines.push("状态尚未完成检测。");
  }

  lines.push("", "[DirectML PoC]");
  if (directMlProbeResult) {
    lines.push(
      "DirectML candidate: " + (directMlProbeResult.directmlCandidate ? "yes" : "no"),
      "DirectML provider ready: " + (directMlProbeResult.providerSessionReady ? "yes" : "no"),
      "DirectML provider error: " + (directMlProbeResult.providerSessionError || "(none)"),
      "Split SenseVoice ready: " + (directMlProbeResult.splitModelReady ? "yes" : "no"),
      "Split SenseVoice dir: " + (directMlProbeResult.splitModelDir || "(none)"),
      "Split SenseVoice missing files: " + (directMlProbeResult.splitModelMissingFiles.join(", ") || "(none)"),
      "Split SenseVoice session ready: " + (directMlProbeResult.splitModelSessionReady ? "yes" : "no"),
      "Split SenseVoice session error: " + (directMlProbeResult.splitModelSessionError || "(none)"),
      "SenseVoice model ready: " + (directMlProbeResult.modelReady ? "yes" : "no"),
      "DirectML session ready: " + (directMlProbeResult.directmlSessionReady ? "yes" : "no"),
      "DirectML session error: " + (directMlProbeResult.directmlSessionError || "(none)"),
      "ONNX Runtime: " + (directMlProbeResult.onnxRuntimeBuild || "(unknown)"),
      "Model inputs: " + (directMlProbeResult.modelInputs.join(" | ") || "(unknown)"),
      "Model outputs: " + (directMlProbeResult.modelOutputs.join(" | ") || "(unknown)"),
      "Model: " + (directMlProbeResult.modelName || directMlProbeResult.modelId || "(none)"),
      "Missing files: " + (directMlProbeResult.missingFiles.join(", ") || "(none)"),
      "Adapters: " + (directMlProbeResult.adapters.map((adapter) => adapter.name).join(" | ") || "(none)"),
      "Message: " + directMlProbeResult.message,
      "Next step: " + directMlProbeResult.nextStep,
    );
  } else {
    lines.push("Not run.");
  }

  lines.push("", "[CPU smoke test]");
  if (smokeResult) {
    lines.push(
      `请求路径: ${smokeResult.requestedMode}`,
      `实际路径: ${smokeResult.usedMode}`,
      `是否回退: ${smokeResult.fallbackUsed ? "是" : "否"}`,
      `耗时: ${smokeResult.elapsedMs} ms`,
      `识别预览: ${smokeResult.transcriptPreview || "(无)"}`,
      `消息: ${smokeResult.message}`,
    );
  } else {
    lines.push("尚未运行。");
  }

  lines.push("", "[CPU vs DirectML benchmark]");
  if (accelerationBenchmarkResult) {
    lines.push(
      "Audio: " + accelerationBenchmarkResult.audioPath,
      "CPU elapsed: " + accelerationBenchmarkResult.cpu.elapsedMs + " ms",
      "CPU RTF: " + formatRtf(accelerationBenchmarkResult.cpu.rtf),
      "DirectML elapsed: " + accelerationBenchmarkResult.directml.elapsedMs + " ms",
      "DirectML used mode: " + accelerationBenchmarkResult.directml.usedMode,
      "DirectML fallback: " + (accelerationBenchmarkResult.directml.fallbackUsed ? "yes" : "no"),
      "DirectML RTF: " + formatRtf(accelerationBenchmarkResult.directml.rtf),
      "Speedup: " + formatRatio(accelerationBenchmarkResult.speedup),
      "Text comparable: " + (accelerationBenchmarkResult.textComparable ? "yes" : "no"),
      "Text similarity: " + formatSimilarity(accelerationBenchmarkResult.textSimilarity),
      "Verdict: " + accelerationBenchmarkResult.verdict,
      "Recommendation: " + accelerationBenchmarkResult.recommendation,
    );
    if (accelerationBenchmarkResult.directml.error) {
      lines.push("DirectML error: " + accelerationBenchmarkResult.directml.error);
    }
  } else {
    lines.push("Not run.");
  }

  lines.push("", "[Native audio environment]");
  if (audioDiagnostics) {
    lines.push(
      `麦克风可用: ${audioDiagnostics.microphoneAvailable ? "是" : "否"}`,
      `麦克风设备: ${audioDiagnostics.microphoneName || "(无)"}`,
      `麦克风详情: ${audioDiagnostics.microphoneDetail || "(无)"}`,
      `系统声音可用: ${audioDiagnostics.systemAudioAvailable ? "是" : "否"}`,
      `系统输出设备: ${audioDiagnostics.systemAudioName || "(无)"}`,
      `系统声音详情: ${audioDiagnostics.systemAudioDetail || "(无)"}`,
      `ffmpeg 已安装: ${audioDiagnostics.ffmpegInstalled ? "是" : "否"}`,
      `ffmpeg 路径: ${audioDiagnostics.ffmpegPath || "(无)"}`,
      `ffmpeg 详情: ${audioDiagnostics.ffmpegDetail || "(无)"}`,
      `音频环境消息: ${audioDiagnostics.message}`,
    );
  } else {
    lines.push("尚未完成本机音频环境检测。");
  }

  return `${lines.join("\n")}\n`;
}

export function DiagnosticsPage({ items, modelReady, settings, onSettingsChange }: DiagnosticsPageProps) {
  const [isTestingModel, setIsTestingModel] = useState(false);
  const [isTestingAcceleration, setIsTestingAcceleration] = useState(false);
  const [testResult, setTestResult] = useState("");
  const [accelerationTestResult, setAccelerationTestResult] = useState("");
  const [accelerationSmokeResult, setAccelerationSmokeResult] = useState<AccelerationSmokeTestResult | null>(null);
  const [accelerationStatus, setAccelerationStatus] = useState<AccelerationStatus | null>(null);
  const [nativeAudioDiagnostics, setNativeAudioDiagnostics] = useState<NativeAudioDiagnostics | null>(null);
  const [directMlProbeResult, setDirectMlProbeResult] = useState<DirectMlProbeResult | null>(null);
  const [isCheckingDirectMl, setIsCheckingDirectMl] = useState(false);
  const [isCheckingNativeAudio, setIsCheckingNativeAudio] = useState(false);
  const [isBenchmarkingAcceleration, setIsBenchmarkingAcceleration] = useState(false);
  const [accelerationBenchmarkResult, setAccelerationBenchmarkResult] = useState<AccelerationBenchmarkResult | null>(null);

  useEffect(() => {
    let disposed = false;
    void getAccelerationStatus(settings.accelerationMode).then((status) => {
      if (!disposed) {
        setAccelerationStatus(status);
      }
    });

    return () => {
      disposed = true;
    };
  }, [settings.accelerationMode]);

  useEffect(() => {
    let disposed = false;
    setIsCheckingNativeAudio(true);
    void getNativeAudioDiagnostics()
      .then((diagnostics) => {
        if (!disposed) {
          setNativeAudioDiagnostics(diagnostics);
        }
      })
      .finally(() => {
        if (!disposed) {
          setIsCheckingNativeAudio(false);
        }
      });

    return () => {
      disposed = true;
    };
  }, []);

  async function handleTestModel() {
    setIsTestingModel(true);
    setTestResult("");

    try {
      const [audioPath] = await selectAudioFiles();
      if (!audioPath) {
        setTestResult("没有选择音频文件。");
        return;
      }

      setTestResult(`正在测试 ${fileNameFromPath(audioPath)}...`);
      const result = await transcribeFile(audioPath, settings, { saveOutput: false });
      setTestResult(result.text || "模型已运行，但没有识别到文字。");
    } catch (error) {
      setTestResult(error instanceof Error ? error.message : "模型测试失败。");
    } finally {
      setIsTestingModel(false);
    }
  }

  async function handleDirectMlProbe() {
    setIsCheckingDirectMl(true);
    setAccelerationTestResult("");
    try {
      const result = await runDirectMlProbe(settings);
      setDirectMlProbeResult(result);
      setAccelerationTestResult(result.message + " Elapsed " + result.elapsedMs + " ms. Next: " + result.nextStep);
    } catch (error) {
      setAccelerationTestResult(error instanceof Error ? error.message : "DirectML PoC probe failed.");
    } finally {
      setIsCheckingDirectMl(false);
    }
  }

  async function handleAccelerationSmokeTest() {
    setIsTestingAcceleration(true);
    setAccelerationTestResult("");
    try {
      const result = await runAccelerationSmokeTest(settings);
      setAccelerationSmokeResult(result);
      setAccelerationTestResult(`${result.message} 用时 ${result.elapsedMs} ms，实际路径：${result.usedMode.toUpperCase()}`);
    } catch (error) {
      setAccelerationTestResult(error instanceof Error ? error.message : "CPU smoke test 失败。");
    } finally {
      setIsTestingAcceleration(false);
    }
  }

  async function handleAccelerationBenchmark() {
    setIsBenchmarkingAcceleration(true);
    setAccelerationTestResult("");
    setAccelerationBenchmarkResult(null);

    try {
      const [audioPath] = await selectAudioFiles();
      if (!audioPath) {
        setAccelerationTestResult("No benchmark audio selected.");
        return;
      }

      setAccelerationTestResult("Running CPU baseline benchmark...");
      const cpu = await benchmarkTranscription(audioPath, settings, "cpu");
      setAccelerationTestResult("Running DirectML benchmark...");
      const directml = await benchmarkTranscription(audioPath, settings, "directml");
      const benchmark = buildBenchmarkResult(audioPath, cpu, directml);
      setAccelerationBenchmarkResult(benchmark);
      if (benchmark.verdict === "ready" && onSettingsChange) {
        onSettingsChange({
          ...settings,
          directmlVerified: true,
          directmlVerifiedAt: new Date().toISOString(),
        });
      }
      setAccelerationTestResult(
        (benchmark.verdict === "ready" ? "DirectML candidate" : "Keep experimental") +
          ": speedup " +
          formatRatio(benchmark.speedup) +
          ", similarity " +
          formatSimilarity(benchmark.textSimilarity) +
          ", text comparable " +
          (benchmark.textComparable ? "yes" : "no") +
          ", DirectML used " +
          benchmark.directml.usedMode,
      );
    } finally {
      setIsBenchmarkingAcceleration(false);
    }
  }

  async function handleSaveReport() {
    const status = accelerationStatus ?? (await getAccelerationStatus(settings.accelerationMode));
    const audioDiagnostics = nativeAudioDiagnostics ?? (await getNativeAudioDiagnostics());
    setAccelerationStatus(status);
    setNativeAudioDiagnostics(audioDiagnostics);
    const report = buildDiagnosticReport(
      settings,
      modelReady,
      items,
      status,
      accelerationSmokeResult,
      audioDiagnostics,
      directMlProbeResult,
      accelerationBenchmarkResult,
    );
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    const path = await saveTextFile(`hi-voicer-diagnostics-${stamp}.txt`, report);
    if (path) {
      setAccelerationTestResult(`诊断报告已保存：${path}`);
    }
  }

  async function handleRefreshNativeAudioDiagnostics() {
    setIsCheckingNativeAudio(true);
    try {
      setNativeAudioDiagnostics(await getNativeAudioDiagnostics());
    } finally {
      setIsCheckingNativeAudio(false);
    }
  }

  return (
    <div className="page-stack">
      <section className="panel">
        <p className="section-label">诊断</p>
        <div className="diagnostic-list">
          {items.map((item) => (
            <div className={`diagnostic-row diagnostic-row--${item.status}`} key={item.id}>
              <strong>{item.label}</strong>
              <p>{item.detail}</p>
            </div>
          ))}
        </div>
      </section>

      <section className="panel diagnostic-tool">
        <div>
          <p className="section-label">本机音频环境</p>
          <h2>录制和音频处理基础条件</h2>
        </div>
        <div className="diagnostic-list">
          <div className={`diagnostic-row diagnostic-row--${nativeAudioDiagnostics?.microphoneAvailable ? "ok" : "warning"}`}>
            <strong>麦克风</strong>
            <p>
              {nativeAudioDiagnostics?.microphoneName || "未检测到默认麦克风"}
              {nativeAudioDiagnostics?.microphoneDetail ? ` / ${nativeAudioDiagnostics.microphoneDetail}` : ""}
            </p>
          </div>
          <div className={`diagnostic-row diagnostic-row--${nativeAudioDiagnostics?.systemAudioAvailable ? "ok" : "warning"}`}>
            <strong>系统声音</strong>
            <p>
              {nativeAudioDiagnostics?.systemAudioName || "未检测到默认系统输出设备"}
              {nativeAudioDiagnostics?.systemAudioDetail ? ` / ${nativeAudioDiagnostics.systemAudioDetail}` : ""}
            </p>
          </div>
          <div className={`diagnostic-row diagnostic-row--${nativeAudioDiagnostics?.ffmpegInstalled ? "ok" : "warning"}`}>
            <strong>ffmpeg</strong>
            <p>{nativeAudioDiagnostics?.ffmpegPath || nativeAudioDiagnostics?.ffmpegDetail || "尚未检测"}</p>
          </div>
        </div>
        <button className="secondary-button" type="button" disabled={isCheckingNativeAudio} onClick={() => void handleRefreshNativeAudioDiagnostics()}>
          <TestTube2 size={17} />
          {isCheckingNativeAudio ? "正在检测音频环境..." : "刷新音频环境诊断"}
        </button>
        {nativeAudioDiagnostics?.message && <p className="diagnostic-result">{nativeAudioDiagnostics.message}</p>}
      </section>

      <section className="panel diagnostic-tool">
        <div>
          <p className="section-label">模型测试</p>
          <h2>用一个音频文件测试当前模型</h2>
        </div>
        <button
          className="primary-button"
          type="button"
          disabled={!modelReady || isTestingModel}
          onClick={() => void handleTestModel()}
        >
          {isTestingModel ? <TestTube2 size={17} /> : <FileAudio size={17} />}
          {isTestingModel ? "正在测试..." : "选择音频测试"}
        </button>
        {testResult && <p className="diagnostic-result">{testResult}</p>}
      </section>

      <section className="panel diagnostic-tool">
        <div>
          <p className="section-label">识别运行时</p>
          <h2>当前识别路径</h2>
        </div>
        <div className="diagnostic-list">
          <div className="diagnostic-row diagnostic-row--ok">
            <strong>
              {accelerationStatus
                ? accelerationStatus.selectedMode === accelerationStatus.effectiveMode
                  ? accelerationStatus.effectiveMode
                  : accelerationStatus.selectedMode + " -> " + accelerationStatus.effectiveMode
                : "Detecting"}
            </strong>
            <p>{accelerationStatus?.message ?? "正在检测识别运行时..."}</p>
          </div>
          {accelerationStatus && (
            <div className="diagnostic-row diagnostic-row--ok">
              <strong>运行时</strong>
              <p>CPU {accelerationStatus.cpuRuntimeInstalled ? "已安装" : "随模型准备"}</p>
            </div>
          )}
        </div>
        <button
          className="secondary-button"
          type="button"
          disabled={!modelReady || isCheckingDirectMl}
          onClick={() => void handleDirectMlProbe()}
        >
          <TestTube2 size={17} />
          {isCheckingDirectMl ? "Running DirectML PoC probe..." : "Run DirectML PoC probe"}
        </button>
        {directMlProbeResult && (
          <div className="diagnostic-list">
            <div className={"diagnostic-row diagnostic-row--" + (directMlProbeResult.directmlCandidate ? "ok" : "warning")}>
              <strong>DirectML candidate</strong>
              <p>{directMlProbeResult.directmlCandidate ? "GPU adapter candidate found" : "No candidate GPU adapter found"}</p>
            </div>
            <div className={"diagnostic-row diagnostic-row--" + (directMlProbeResult.providerSessionReady ? "ok" : "warning")}>
              <strong>DirectML provider</strong>
              <p>{directMlProbeResult.providerSessionReady ? "Minimal ONNX session created" : directMlProbeResult.providerSessionError || "DirectML provider probe failed"}</p>
            </div>
            <div className={"diagnostic-row diagnostic-row--" + (directMlProbeResult.splitModelSessionReady ? "ok" : "warning")}>
              <strong>Split SenseVoice DirectML</strong>
              <p>
                {directMlProbeResult.splitModelSessionReady
                  ? "Encoder and CTC warmups completed"
                  : directMlProbeResult.splitModelSessionError ||
                    "Missing: " + (directMlProbeResult.splitModelMissingFiles.join(", ") || "split model files")}
              </p>
            </div>
            <div className={"diagnostic-row diagnostic-row--" + (directMlProbeResult.modelReady ? "ok" : "warning")}>
              <strong>Sherpa SenseVoiceSmall</strong>
              <p>
                {directMlProbeResult.modelReady
                  ? "Model files are ready"
                  : "Missing: " + (directMlProbeResult.missingFiles.join(", ") || "SenseVoiceSmall model")}
              </p>
            </div>
            <div className={"diagnostic-row diagnostic-row--" + (directMlProbeResult.directmlSessionReady ? "ok" : "warning")}>
              <strong>DirectML session</strong>
              <p>{directMlProbeResult.directmlSessionReady ? directMlProbeResult.message : directMlProbeResult.directmlSessionError || directMlProbeResult.message}</p>
            </div>
            {directMlProbeResult.splitModelInputs.length > 0 && (
              <div className="diagnostic-row diagnostic-row--ok">
                <strong>Split model inputs</strong>
                <p>{directMlProbeResult.splitModelInputs.join(" | ")}</p>
              </div>
            )}
            {directMlProbeResult.splitModelOutputs.length > 0 && (
              <div className="diagnostic-row diagnostic-row--ok">
                <strong>Split model outputs</strong>
                <p>{directMlProbeResult.splitModelOutputs.join(" | ")}</p>
              </div>
            )}
            {directMlProbeResult.modelInputs.length > 0 && (
              <div className="diagnostic-row diagnostic-row--ok">
                <strong>Model inputs</strong>
                <p>{directMlProbeResult.modelInputs.join(" | ")}</p>
              </div>
            )}
            {directMlProbeResult.modelOutputs.length > 0 && (
              <div className="diagnostic-row diagnostic-row--ok">
                <strong>Model outputs</strong>
                <p>{directMlProbeResult.modelOutputs.join(" | ")}</p>
              </div>
            )}
            {directMlProbeResult.onnxRuntimeBuild && (
              <div className="diagnostic-row diagnostic-row--ok">
                <strong>ONNX Runtime</strong>
                <p>{directMlProbeResult.onnxRuntimeBuild}</p>
              </div>
            )}
            {directMlProbeResult.adapters.map((adapter) => (
              <div className="diagnostic-row diagnostic-row--ok" key={adapter.name}>
                <strong>{adapter.name}</strong>
                <p>
                  {adapter.driverVersion || "driver unknown"}
                  {adapter.adapterRamMb ? " / " + adapter.adapterRamMb + " MB" : ""}
                </p>
              </div>
            ))}
          </div>
        )}
        <button
          className="secondary-button"
          type="button"
          disabled={!modelReady || isTestingAcceleration}
          onClick={() => void handleAccelerationSmokeTest()}
        >
          <TestTube2 size={17} />
          {isTestingAcceleration ? "正在运行 CPU smoke test..." : "运行 CPU smoke test"}
        </button>

        <button
          className="secondary-button"
          type="button"
          disabled={!modelReady || isBenchmarkingAcceleration}
          onClick={() => void handleAccelerationBenchmark()}
        >
          <TestTube2 size={17} />
          {isBenchmarkingAcceleration ? "Running CPU vs DirectML benchmark..." : "Run CPU vs DirectML benchmark"}
        </button>
        {accelerationBenchmarkResult && (
          <div className="diagnostic-list">
            <div className={"diagnostic-row diagnostic-row--" + (accelerationBenchmarkResult.verdict === "ready" ? "ok" : "warning")}>
              <strong>Benchmark verdict</strong>
              <p>{accelerationBenchmarkResult.recommendation}</p>
            </div>
            <div className={"diagnostic-row diagnostic-row--" + (accelerationBenchmarkResult.cpu.error || accelerationBenchmarkResult.cpu.text.trim().length === 0 ? "warning" : "ok")}>
              <strong>CPU baseline</strong>
              <p>
                {accelerationBenchmarkResult.cpu.elapsedMs} ms / RTF {formatRtf(accelerationBenchmarkResult.cpu.rtf)} / text {accelerationBenchmarkResult.cpu.text.trim().length} chars
                {accelerationBenchmarkResult.cpu.error ? " / " + accelerationBenchmarkResult.cpu.error : ""}
              </p>
            </div>
            <div className={"diagnostic-row diagnostic-row--" + (accelerationBenchmarkResult.directml.fallbackUsed || accelerationBenchmarkResult.directml.error ? "warning" : "ok")}>
              <strong>DirectML run</strong>
              <p>
                {accelerationBenchmarkResult.directml.elapsedMs} ms / RTF {formatRtf(accelerationBenchmarkResult.directml.rtf)} / used {accelerationBenchmarkResult.directml.usedMode} / fallback {accelerationBenchmarkResult.directml.fallbackUsed ? "yes" : "no"}
                {accelerationBenchmarkResult.directml.error ? " / " + accelerationBenchmarkResult.directml.error : ""}
              </p>
            </div>
            <div className={"diagnostic-row diagnostic-row--" + (accelerationBenchmarkResult.textComparable ? "ok" : "warning")}>
              <strong>Decision metrics</strong>
              <p>
                Speedup {formatRatio(accelerationBenchmarkResult.speedup)} / text comparable {accelerationBenchmarkResult.textComparable ? "yes" : "no"} / text similarity {formatSimilarity(accelerationBenchmarkResult.textSimilarity)}
              </p>
            </div>
          </div>
        )}
        <button className="secondary-button" type="button" onClick={() => void handleSaveReport()}>
          <FileDown size={17} />
          保存诊断报告
        </button>
        {accelerationTestResult && <p className="diagnostic-result">{accelerationTestResult}</p>}
      </section>
    </div>
  );
}
