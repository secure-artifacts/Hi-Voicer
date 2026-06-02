import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { initialSettings, initialTasks } from "../data/mockState";
import { saveExistingFile, selectAudioFiles, selectDirectory, transcribeFile } from "../lib/api";
import { TranscriptionPage } from "../pages/TranscriptionPage";
import type { TranscriptTask } from "../types";

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: vi.fn(() => Promise.resolve(() => {})),
  }),
}));

vi.mock("../lib/api", () => ({
  listenTranscriptionProgress: vi.fn(() => Promise.resolve(() => {})),
  saveExistingFile: vi.fn(() => Promise.resolve("C:\\exports\\demo.txt")),
  selectAudioFiles: vi.fn(() => Promise.resolve(["C:\\audio\\demo.wav"])),
  selectDirectory: vi.fn(() => Promise.resolve("C:\\exports")),
  transcribeFile: vi.fn(() =>
    Promise.resolve({
      text: "recognized text",
      outputPath: "C:\\temp\\demo.txt",
      outputPaths: ["C:\\temp\\demo.txt", "C:\\temp\\demo.srt"],
      outputFiles: [
        { format: "plainText", label: "无时间码纯文字", path: "C:\\temp\\demo.txt" },
        { format: "srt", label: "SRT 字幕", path: "C:\\temp\\demo.srt" },
      ],
    }),
  ),
}));

function renderTranscriptionPage() {
  function Harness() {
    const [tasks, setTasks] = useState<TranscriptTask[]>(initialTasks);

    return <TranscriptionPage tasks={tasks} onTasksChange={setTasks} settings={initialSettings} />;
  }

  render(<Harness />);
}

describe("TranscriptionPage", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("keeps completed file transcription in the task queue without transcript history", async () => {
    renderTranscriptionPage();

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));

    await waitFor(() => {
      expect(transcribeFile).toHaveBeenCalledWith(
        "C:\\audio\\demo.wav",
        initialSettings,
        expect.objectContaining({ performanceMode: "balanced" }),
      );
    });
    expect(await screen.findByText("demo.wav")).toBeTruthy();
    expect(screen.getByText("已生成临时结果，选择需要的格式保存到音频文件夹。")).toBeTruthy();
    expect(screen.queryByText("转录历史")).toBeNull();
  });

  it("clears visible tasks", async () => {
    renderTranscriptionPage();

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("demo.wav");

    fireEvent.click(screen.getByRole("button", { name: "清空任务" }));
    expect(screen.queryByText("demo.wav")).toBeNull();
    expect(screen.getByText("还没有任务，选择音频或把文件拖到上方即可开始。")).toBeTruthy();
  });

  it("uses a custom export directory for single and batch downloads", async () => {
    renderTranscriptionPage();

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("demo.wav");

    fireEvent.click(screen.getByRole("button", { name: "选择保存目录" }));
    await screen.findByText("保存目录：C:\\exports");

    fireEvent.click(screen.getByRole("button", { name: "SRT 字幕" }));
    await waitFor(() => {
      expect(saveExistingFile).toHaveBeenCalledWith("C:\\temp\\demo.srt", "demo.srt", "C:\\exports");
    });

    fireEvent.click(screen.getByRole("button", { name: "批量下载" }));
    await waitFor(() => {
      expect(saveExistingFile).toHaveBeenCalledWith("C:\\temp\\demo.txt", "demo.txt", "C:\\exports");
      expect(saveExistingFile).toHaveBeenCalledWith("C:\\temp\\demo.srt", "demo.srt", "C:\\exports");
    });
    expect(selectDirectory).toHaveBeenCalledTimes(1);
  });

  it("requests audio files from the picker", async () => {
    renderTranscriptionPage();

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));

    await waitFor(() => {
      expect(selectAudioFiles).toHaveBeenCalledTimes(1);
    });
  });
});
