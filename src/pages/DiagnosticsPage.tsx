import type { DiagnosticItem } from "../types";

export function DiagnosticsPage({ items }: { items: DiagnosticItem[] }) {
  return (
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
  );
}
