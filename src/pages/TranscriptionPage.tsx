import { Upload } from "lucide-react";
import type { TranscriptTask } from "../types";

export function TranscriptionPage({ tasks }: { tasks: TranscriptTask[] }) {
  return (
    <div className="page-stack">
      <section className="drop-zone">
        <Upload size={28} />
        <h2>拖入音频或视频文件</h2>
        <p>第一版将支持导出 txt、srt、json。真实转录会在 ASR 集成阶段接入。</p>
      </section>

      <section className="panel">
        <p className="section-label">任务队列</p>
        <div className="task-list">
          {tasks.map((task) => (
            <div className="task-row" key={task.id}>
              <div>
                <strong>{task.fileName}</strong>
                <p>{task.message}</p>
              </div>
              <span>{task.progress}%</span>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
