import { Download, FileAudio, FileDown, TestTube2 } from "lucide-react";
import { useEffect, useState } from "react";
import {
  getAccelerationStatus,
  prepareAccelerationRuntime,
  runAccelerationSmokeTest,
  saveTextFile,
  selectAudioFiles,
  transcribeFile,
} from "../lib/api";
import type { AccelerationSmokeTestResult, AccelerationStatus, DiagnosticItem, UserSettings } from "../types";

interface DiagnosticsPageProps {
  items: DiagnosticItem[];
  modelReady: boolean;
  settings: UserSettings;
}

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

function cudaEnvironmentText(status: AccelerationStatus) {
  if (status.cudaDeviceSummary) {
    return status.cudaDeviceSummary;
  }

  return status.cudaDetectionError || "未检测到 NVIDIA CUDA 环境。";
}

function buildGpuDiagnosticReport(
  settings: UserSettings,
  modelReady: boolean,
  items: DiagnosticItem[],
  status: AccelerationStatus | null,
  smokeResult: AccelerationSmokeTestResult | null,
) {
  const lines = [
    "Hi-Voicer GPU 诊断报告",
    `生成时间: ${new Date().toISOString()}`,
    "",
    "[设置]",
    `模型目录: ${settings.modelDir || "(未配置)"}`,
    `加速模式: ${settings.accelerationMode}`,
    `模型可用: ${modelReady ? "是" : "否"}`,
    "",
    "[基础诊断]",
    ...items.map((item) => `${item.label}: ${item.status} - ${item.detail}`),
    "",
    "[GPU 状态]",
  ];

  if (status) {
    lines.push(
      `选择路径: ${status.selectedMode}`,
      `有效路径: ${status.effectiveMode}`,
      `NVIDIA 可用: ${status.cudaAvailable ? "是" : "否"}`,
      `NVIDIA 信息: ${status.cudaDeviceSummary || "(无)"}`,
      `NVIDIA 检测错误: ${status.cudaDetectionError || "(无)"}`,
      `CPU runtime: ${status.cpuRuntimeInstalled ? "已安装" : "未安装/按需准备"}`,
      `CUDA runtime: ${status.cudaRuntimeInstalled ? "已安装" : "未安装/按需下载"}`,
      `CUDA 本次会话停用原因: ${status.cudaDisabledReason || "(无)"}`,
      `状态消息: ${status.message}`,
    );
  } else {
    lines.push("状态尚未完成检测。");
  }

  lines.push("", "[加速 smoke test]");
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

  return `${lines.join("\n")}\n`;
}

export function DiagnosticsPage({ items, modelReady, settings }: DiagnosticsPageProps) {
  const [isTestingModel, setIsTestingModel] = useState(false);
  const [isPreparingAcceleration, setIsPreparingAcceleration] = useState(false);
  const [isTestingAcceleration, setIsTestingAcceleration] = useState(false);
  const [testResult, setTestResult] = useState("");
  const [accelerationTestResult, setAccelerationTestResult] = useState("");
  const [accelerationSmokeResult, setAccelerationSmokeResult] = useState<AccelerationSmokeTestResult | null>(null);
  const [accelerationStatus, setAccelerationStatus] = useState<AccelerationStatus | null>(null);

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

  async function handlePrepareAcceleration() {
    setIsPreparingAcceleration(true);
    try {
      const status = await prepareAccelerationRuntime(settings.accelerationMode);
      setAccelerationStatus(status);
    } finally {
      setIsPreparingAcceleration(false);
    }
  }

  async function handleAccelerationSmokeTest() {
    setIsTestingAcceleration(true);
    setAccelerationTestResult("");
    try {
      const result = await runAccelerationSmokeTest(settings);
      setAccelerationSmokeResult(result);
      const fallback = result.fallbackUsed ? "，已回退 CPU" : "";
      setAccelerationTestResult(`${result.message} 用时 ${result.elapsedMs} ms，实际路径：${result.usedMode.toUpperCase()}${fallback}`);
    } catch (error) {
      setAccelerationTestResult(error instanceof Error ? error.message : "加速 smoke test 失败。");
    } finally {
      setIsTestingAcceleration(false);
    }
  }

  async function handleSaveGpuReport() {
    const status = accelerationStatus ?? (await getAccelerationStatus(settings.accelerationMode));
    setAccelerationStatus(status);
    const report = buildGpuDiagnosticReport(settings, modelReady, items, status, accelerationSmokeResult);
    const stamp = new Date().toISOString().replace(/[:.]/g, "-");
    const path = await saveTextFile(`hi-voicer-gpu-diagnostics-${stamp}.txt`, report);
    if (path) {
      setAccelerationTestResult(`GPU 诊断报告已保存：${path}`);
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
          <p className="section-label">GPU 加速</p>
          <h2>当前加速路径</h2>
        </div>
        <div className="diagnostic-list">
          <div
            className={`diagnostic-row diagnostic-row--${
              accelerationStatus?.effectiveMode === "cuda" ? "ok" : settings.accelerationMode === "cuda" ? "warning" : "ok"
            }`}
          >
            <strong>{settings.accelerationMode === "cuda" ? "CUDA" : "CPU"}</strong>
            <p>{accelerationStatus?.message ?? "正在检测加速环境..."}</p>
          </div>
          {accelerationStatus && (
            <div className={`diagnostic-row diagnostic-row--${accelerationStatus.cudaAvailable ? "ok" : "warning"}`}>
              <strong>NVIDIA</strong>
              <p>{cudaEnvironmentText(accelerationStatus)}</p>
            </div>
          )}
          {accelerationStatus && (
            <div className="diagnostic-row diagnostic-row--ok">
              <strong>运行时</strong>
              <p>
                CPU {accelerationStatus.cpuRuntimeInstalled ? "已安装" : "按模型安装时准备"} / CUDA{" "}
                {accelerationStatus.cudaRuntimeInstalled ? "已安装" : "按需下载"}
              </p>
            </div>
          )}
        </div>
        <button
          className="secondary-button"
          type="button"
          disabled={settings.accelerationMode !== "cuda" || isPreparingAcceleration}
          onClick={() => void handlePrepareAcceleration()}
        >
          <Download size={17} />
          {isPreparingAcceleration ? "正在准备 CUDA..." : "准备 CUDA 运行时"}
        </button>
        <button
          className="secondary-button"
          type="button"
          disabled={!modelReady || isTestingAcceleration}
          onClick={() => void handleAccelerationSmokeTest()}
        >
          <TestTube2 size={17} />
          {isTestingAcceleration ? "正在测试加速路径..." : "运行加速 smoke test"}
        </button>
        <button className="secondary-button" type="button" onClick={() => void handleSaveGpuReport()}>
          <FileDown size={17} />
          保存 GPU 诊断报告
        </button>
        {accelerationTestResult && <p className="diagnostic-result">{accelerationTestResult}</p>}
      </section>
    </div>
  );
}
