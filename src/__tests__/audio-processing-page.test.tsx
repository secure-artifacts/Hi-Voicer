import { act, createEvent, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clipAudioSegments,
  convertAudioFile,
  listAudioFilesInDirectory,
  mergeAudioFiles,
  openOutputFolder,
  prepareAudioPreview,
  prepareAudioWaveform,
  probeMediaFrameRate,
  processAudioFile,
  selectAudioFiles,
  selectDirectory,
  splitAudioFile,
} from "../lib/api";
import { AudioProcessingPage } from "../pages/AudioProcessingPage";

vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((path: string) => `asset://local/${path}`),
  isTauri: vi.fn(() => true),
}));

const onDragDropEvent = vi.fn(() => Promise.resolve(() => {}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: vi.fn(() => ({ onDragDropEvent })),
}));

vi.mock("../lib/api", () => ({
  clipAudioSegments: vi.fn(() => Promise.resolve(["C:\\exports\\clip-001.wav"])),
  convertAudioFile: vi.fn(() => Promise.resolve({ outputPath: "C:\\exports\\converted.mp3", message: "converted" })),
  listAudioFilesInDirectory: vi.fn(() => Promise.resolve(["D:\\folder\\call.wav", "D:\\folder\\talk.mp3"])),
  mergeAudioFiles: vi.fn(() => Promise.resolve("C:\\exports\\merged-audio.wav")),
  openOutputFolder: vi.fn(() => Promise.resolve("C:\\exports")),
  prepareAudioPreview: vi.fn((audioPath: string) => Promise.resolve(`C:\\cache\\preview-${audioPath.split("\\").pop()}`)),
  prepareAudioWaveform: vi.fn(() => Promise.resolve({ waveformPath: "C:\\cache\\waveform.png", durationSeconds: 120, message: "waveform" })),
  probeMediaFrameRate: vi.fn(() => Promise.resolve({ fps: 25, source: "video", message: "Detected video frame rate: 25fps." })),
  processAudioFile: vi.fn(() => Promise.resolve({ outputPath: "C:\\exports\\voice-basic.wav", message: "done" })),
  selectAudioFiles: vi.fn(() => Promise.resolve(["C:\\recordings\\voice.wav", "C:\\recordings\\meeting.wav"])),
  selectDirectory: vi.fn(() => Promise.resolve("D:\\processed")),
  splitAudioFile: vi.fn(() => Promise.resolve(["C:\\exports\\split-001.wav", "C:\\exports\\split-002.wav"])),
}));

function installLocalStorageMock() {
  const store = new Map<string, string>();
  Object.defineProperty(window, "localStorage", {
    configurable: true,
    value: {
      clear: vi.fn(() => store.clear()),
      getItem: vi.fn((key: string) => store.get(key) ?? null),
      removeItem: vi.fn((key: string) => store.delete(key)),
      setItem: vi.fn((key: string, value: string) => {
        store.set(key, value);
      }),
    },
  });
}

describe("AudioProcessingPage", () => {
  let playMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    installLocalStorageMock();
    window.localStorage.clear();
    playMock = vi.fn(() => Promise.resolve());
    Object.defineProperty(HTMLMediaElement.prototype, "play", {
      configurable: true,
      value: playMock,
    });
    onDragDropEvent.mockImplementation(() => Promise.resolve(() => {}));
  });

  it("keeps processing disabled until a file is selected", async () => {
    const { container } = render(<AudioProcessingPage />);
    const processButton = container.querySelector(".audio-process-button") as HTMLButtonElement;

    expect(processButton.disabled).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));

    expect(await screen.findByText("C:\\recordings\\voice.wav")).toBeTruthy();
    expect(processButton.disabled).toBe(false);
  });

  it("processes all queued files with the selected preset", async () => {
    const { container } = render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    await screen.findByText("C:\\recordings\\meeting.wav");
    fireEvent.click(container.querySelector(".audio-process-button") as HTMLButtonElement);

    await waitFor(() => expect(processAudioFile).toHaveBeenCalledTimes(2));
    expect(processAudioFile).toHaveBeenCalledWith(
      "C:\\recordings\\voice.wav",
      expect.objectContaining({ preset: "voiceBasic", normalize: true }),
      expect.objectContaining({ destinationDir: undefined }),
    );
    expect(processAudioFile).toHaveBeenCalledWith(
      "C:\\recordings\\meeting.wav",
      expect.objectContaining({ preset: "voiceBasic", normalize: true }),
      expect.objectContaining({ destinationDir: undefined }),
    );
    expect((await screen.findAllByText((content) => content.includes("C:\\exports\\voice-basic.wav"))).length).toBeGreaterThan(0);
  });

  it("uses a custom output folder for batch processing", async () => {
    const { container } = render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: "自定义输出目录" }));
    expect((await screen.findAllByText((content) => content.includes("D:\\processed"))).length).toBeGreaterThan(0);
    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    fireEvent.click(container.querySelector(".audio-process-button") as HTMLButtonElement);

    await waitFor(() => expect(processAudioFile).toHaveBeenCalled());
    expect(selectDirectory).toHaveBeenCalled();
    expect(processAudioFile).toHaveBeenCalledWith(
      "C:\\recordings\\voice.wav",
      expect.any(Object),
      expect.objectContaining({ destinationDir: "D:\\processed" }),
    );
  });

  it("adds supported files from a selected folder", async () => {
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件夹" }));

    expect(await screen.findByText("D:\\folder\\call.wav")).toBeTruthy();
    expect(await screen.findByText("D:\\folder\\talk.mp3")).toBeTruthy();
    expect(listAudioFilesInDirectory).toHaveBeenCalledWith("D:\\processed");
  });

  it("adds dragged single or multiple files to the processing queue", async () => {
    render(<AudioProcessingPage />);

    const dragDropCalls = onDragDropEvent.mock.calls as unknown as Array<[
      (event: { payload: { type: string; paths: string[] } }) => void,
    ]>;
    const handler = dragDropCalls[0]?.[0];
    expect(handler).toBeTypeOf("function");
    await act(async () => {
      handler?.({ payload: { type: "drop", paths: ["C:\\drops\\one.wav", "C:\\drops\\two.mp3"] } });
    });

    expect(await screen.findByText("C:\\drops\\one.wav")).toBeTruthy();
    expect(await screen.findByText("C:\\drops\\two.mp3")).toBeTruthy();
  });

  it("shows a preview player under every processed row and can clear history", async () => {
    const { container } = render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    fireEvent.click(container.querySelector(".audio-process-button") as HTMLButtonElement);

    await waitFor(() => expect(processAudioFile).toHaveBeenCalledTimes(2));
    const rowPlayers = document.querySelectorAll(".audio-row-preview audio");
    expect(rowPlayers).toHaveLength(2);
    expect(prepareAudioPreview).toHaveBeenCalledWith("C:\\exports\\voice-basic.wav");
    expect(rowPlayers[0].getAttribute("src")).toContain("asset://local/C:\\cache\\preview-voice-basic.wav");

    fireEvent.click((await screen.findAllByRole("button", { name: "试听" }))[0]);
    expect(playMock).toHaveBeenCalled();

    fireEvent.click((await screen.findAllByRole("button", { name: "打开目录" }))[0]);
    await waitFor(() => expect(openOutputFolder).toHaveBeenCalledWith("C:\\exports\\voice-basic.wav"));

    fireEvent.click(screen.getByRole("button", { name: "清空队列/历史" }));
    expect(screen.queryByText("C:\\recordings\\voice.wav")).toBeNull();
  });

  it("keeps processed results when the page is reopened until history is cleared", async () => {
    const { container, unmount } = render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    fireEvent.click(container.querySelector(".audio-process-button") as HTMLButtonElement);

    await waitFor(() => expect(window.localStorage.getItem("hi-voicer-audio-processing-history")).toContain("voice-basic.wav"));
    unmount();

    render(<AudioProcessingPage />);

    expect(await screen.findByText("C:\\recordings\\voice.wav")).toBeTruthy();
    await waitFor(() => expect(prepareAudioPreview).toHaveBeenCalledWith("C:\\exports\\voice-basic.wav"));
    expect(document.querySelectorAll(".audio-row-preview audio")).toHaveLength(2);

    fireEvent.click(screen.getByRole("button", { name: "清空队列/历史" }));
    expect(window.localStorage.getItem("hi-voicer-audio-processing-history")).toBeNull();
    expect(screen.queryByText("C:\\recordings\\voice.wav")).toBeNull();
  });

  it("converts selected files to the chosen output format", async () => {
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: /格式转换/ }));
    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    fireEvent.click(screen.getByRole("button", { name: /批量转换为 MP3/ }));

    await waitFor(() => expect(convertAudioFile).toHaveBeenCalledTimes(2));
    expect(convertAudioFile).toHaveBeenCalledWith(
      "C:\\recordings\\voice.wav",
      "mp3",
      expect.objectContaining({ destinationDir: undefined }),
    );
  });

  it("exports clip segments and can split by seconds", async () => {
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: /音频剪辑/ }));
    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    await waitFor(() => expect(probeMediaFrameRate).toHaveBeenCalledWith("C:\\recordings\\voice.wav"));
    fireEvent.click(screen.getByRole("button", { name: "开始剪辑" }));

    await waitFor(() => expect(clipAudioSegments).toHaveBeenCalled());
    expect(clipAudioSegments).toHaveBeenCalledWith(
      "C:\\recordings\\voice.wav",
      [expect.objectContaining({ startSeconds: 0, endSeconds: 10 })],
      "wav",
      expect.objectContaining({ mergeSegments: false }),
    );

    fireEvent.change(screen.getByLabelText("剪辑模式"), { target: { value: "split" } });
    fireEvent.click(screen.getByRole("button", { name: "开始剪辑" }));
    await waitFor(() => expect(splitAudioFile).toHaveBeenCalledWith("C:\\recordings\\voice.wav", 60, "wav", expect.any(Object)));
  });

  it("exports multi-clip segments in the adjusted order", async () => {
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: /音频剪辑/ }));
    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    fireEvent.click(screen.getByRole("button", { name: "添加片段" }));
    fireEvent.click(screen.getAllByTitle("片段上移")[1]);
    fireEvent.click(screen.getByRole("button", { name: "开始剪辑" }));

    await waitFor(() => expect(clipAudioSegments).toHaveBeenCalled());
    expect(clipAudioSegments).toHaveBeenCalledWith(
      "C:\\recordings\\voice.wav",
      [
        expect.objectContaining({ startSeconds: 10, endSeconds: 20 }),
        expect.objectContaining({ startSeconds: 0, endSeconds: 10 }),
      ],
      "wav",
      expect.objectContaining({ mergeSegments: false }),
    );
  });

  it("creates a new clip selection by ctrl-dragging the waveform", async () => {
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: /音频剪辑/ }));
    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    await waitFor(() => expect(prepareAudioWaveform).toHaveBeenCalled());

    const waveform = document.querySelector(".clip-waveform") as HTMLDivElement;
    Object.defineProperty(waveform, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ left: 0, right: 100, top: 0, bottom: 100, width: 100, height: 100, x: 0, y: 0, toJSON: () => {} }),
    });

    const pointerDown = createEvent.pointerDown(waveform, { clientX: 25 });
    Object.defineProperty(pointerDown, "ctrlKey", { value: true });
    fireEvent(waveform, pointerDown);
    fireEvent.pointerMove(window, { clientX: 50 });
    fireEvent.pointerUp(window, { clientX: 50 });

    expect(await screen.findByText("片段 2")).toBeTruthy();
  });

  it("clears clip selections when queue history is cleared", async () => {
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: /音频剪辑/ }));
    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    fireEvent.click(screen.getByRole("button", { name: "添加片段" }));
    expect(screen.getByText("片段 2")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "清空队列/历史" }));

    expect(screen.queryByText("片段 2")).toBeNull();
    expect(screen.getByText("片段 1")).toBeTruthy();
  });

  it("merges files in the displayed order", async () => {
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: /音频合并/ }));
    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));
    await screen.findByText("C:\\recordings\\voice.wav");
    fireEvent.click(screen.getByRole("button", { name: "开始合并" }));

    await waitFor(() => expect(mergeAudioFiles).toHaveBeenCalled());
    expect(mergeAudioFiles).toHaveBeenCalledWith(
      ["C:\\recordings\\voice.wav", "C:\\recordings\\meeting.wav"],
      "reencode",
      "wav",
      expect.objectContaining({ suggestedName: "merged-audio.wav" }),
    );
  });

  it("shows file picker errors", async () => {
    vi.mocked(selectAudioFiles).mockRejectedValueOnce(new Error("picker unavailable"));
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));

    expect(await screen.findByText("picker unavailable")).toBeTruthy();
  });
});
