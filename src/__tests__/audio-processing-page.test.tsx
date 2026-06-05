import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listAudioFilesInDirectory, prepareAudioPreview, processAudioFile, selectAudioFiles, selectDirectory } from "../lib/api";
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
  listAudioFilesInDirectory: vi.fn(() => Promise.resolve(["D:\\folder\\call.wav", "D:\\folder\\talk.mp3"])),
  prepareAudioPreview: vi.fn((audioPath: string) => Promise.resolve(`C:\\cache\\preview-${audioPath.split("\\").pop()}`)),
  processAudioFile: vi.fn(() => Promise.resolve({ outputPath: "C:\\exports\\voice-basic.wav", message: "done" })),
  selectAudioFiles: vi.fn(() => Promise.resolve(["C:\\recordings\\voice.wav", "C:\\recordings\\meeting.wav"])),
  selectDirectory: vi.fn(() => Promise.resolve("D:\\processed")),
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

    fireEvent.click(screen.getByRole("button", { name: "清空历史" }));
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

    fireEvent.click(screen.getByRole("button", { name: "清空历史" }));
    expect(window.localStorage.getItem("hi-voicer-audio-processing-history")).toBeNull();
    expect(screen.queryByText("C:\\recordings\\voice.wav")).toBeNull();
  });

  it("shows file picker errors", async () => {
    vi.mocked(selectAudioFiles).mockRejectedValueOnce(new Error("picker unavailable"));
    render(<AudioProcessingPage />);

    fireEvent.click(screen.getByRole("button", { name: "选择文件" }));

    expect(await screen.findByText("picker unavailable")).toBeTruthy();
  });
});
