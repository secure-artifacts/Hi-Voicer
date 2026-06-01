import { FileAudio, TestTube2 } from "lucide-react";
import { useState } from "react";
import { selectAudioFiles, transcribeFile } from "../lib/api";
import type { DiagnosticItem, UserSettings } from "../types";

interface DiagnosticsPageProps {
  items: DiagnosticItem[];
  modelReady: boolean;
  settings: UserSettings;
}

function fileNameFromPath(path: string) {
  return path.split(/[\\/]/).pop() || path;
}

export function DiagnosticsPage({ items, modelReady, settings }: DiagnosticsPageProps) {
  const [isTestingModel, setIsTestingModel] = useState(false);
  const [testResult, setTestResult] = useState("");

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
    </div>
  );
}
