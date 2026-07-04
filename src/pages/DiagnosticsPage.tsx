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
  AccelerationSmokeTestResult,
  AccelerationStatus,
  DiagnosticItem,
  DirectMlProbeResult,
  NativeAudioDiagnostics,
  UserSettings,
} from "../types";

interface DiagnosticsPageProps {
  items: DiagnosticItem[];
  modelReady: boolean;
  settings: UserSettings;
}

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function buildDiagnosticReport(
  settings: UserSettings,
  modelReady: boolean,
  items: DiagnosticItem[],
  status: AccelerationStatus | null,
  smokeResult: AccelerationSmokeTestResult | null,
  audioDiagnostics: NativeAudioDiagnostics | null,
  directMlProbeResult: DirectMlProbeResult | null,
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
      "SenseVoice model ready: " + (directMlProbeResult.modelReady ? "yes" : "no"),
      "DirectML session ready: " + (directMlProbeResult.directmlSessionReady ? "yes" : "no"),
      "DirectML session error: " + (directMlProbeResult.directmlSessionError || "(none)"),
      "ONNX Runtime: " + (directMlProbeResult.onnxRuntimeBuild || "(unknown)"),
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

  lines.push("", "[本机音频环境]");
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

export function DiagnosticsPage({ items, modelReady, settings }: DiagnosticsPageProps) {
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

  async function handleSaveReport() {
    const status = accelerationStatus ?? (await getAccelerationStatus(settings.accelerationMode));
    const audioDiagnostics = nativeAudioDiagnostics ?? (await getNativeAudioDiagnostics());
    setAccelerationStatus(status);
    setNativeAudioDiagnostics(audioDiagnostics);
    const report = buildDiagnosticReport(settings, modelReady, items, status, accelerationSmokeResult, audioDiagnostics, directMlProbeResult);
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
            <strong>CPU</strong>
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
            <div className={"diagnostic-row diagnostic-row--" + (directMlProbeResult.modelReady ? "ok" : "warning")}>
              <strong>SenseVoiceSmall</strong>
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
        <button className="secondary-button" type="button" onClick={() => void handleSaveReport()}>
          <FileDown size={17} />
          保存诊断报告
        </button>
        {accelerationTestResult && <p className="diagnostic-result">{accelerationTestResult}</p>}
      </section>
    </div>
  );
}