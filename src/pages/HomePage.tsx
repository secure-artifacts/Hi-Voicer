import type { AppStatus } from "../types";

export function HomePage({ status }: { status: AppStatus }) {
  return (
    <div className="page-grid page-grid--home">
      <section className="panel hero-panel">
        <p className="section-label">语音输入</p>
        <h2>{status.readiness === "ready" ? "可以开始说话" : "先完成模型配置"}</h2>
        <p>按住 {status.shortcut} 说话，松开后自动识别并输入到当前窗口。</p>
        <button className="primary-button" type="button">
          打开模型设置
        </button>
      </section>

      <section className="panel">
        <p className="section-label">最近结果</p>
        <blockquote>{status.lastResult}</blockquote>
      </section>

      <section className="panel info-list">
        <div>
          <span>模型</span>
          <strong>{status.modelName}</strong>
        </div>
        <div>
          <span>麦克风</span>
          <strong>{status.microphoneName}</strong>
        </div>
        <div>
          <span>输入方式</span>
          <strong>剪贴板兜底</strong>
        </div>
      </section>
    </div>
  );
}
