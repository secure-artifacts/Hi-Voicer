import type { HotwordRule } from "../types";

export function HotwordsPage({ rules }: { rules: HotwordRule[] }) {
  return (
    <section className="panel">
      <div className="panel-heading">
        <div>
          <p className="section-label">热词和替换</p>
          <h2>让识别结果更像你的用词</h2>
        </div>
        <button className="secondary-button" type="button">
          新增规则
        </button>
      </div>
      <div className="rule-list">
        {rules.map((rule) => (
          <div className="rule-row" key={rule.id}>
            <span>{rule.source}</span>
            <strong>{rule.target}</strong>
            <em>{rule.enabled ? "启用" : "停用"}</em>
          </div>
        ))}
      </div>
    </section>
  );
}
