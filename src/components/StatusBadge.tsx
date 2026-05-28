import type { ReadinessState } from "../types";

const labels: Record<ReadinessState, string> = {
  starting: "启动中",
  "loading-model": "正在加载模型",
  ready: "可以录音",
  "model-required": "需要配置模型",
  "microphone-unavailable": "麦克风不可用",
  error: "异常",
};

export function StatusBadge({ state }: { state: ReadinessState }) {
  return <span className={`status-badge status-badge--${state}`}>{labels[state]}</span>;
}
