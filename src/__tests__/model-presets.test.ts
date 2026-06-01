import { describe, expect, it } from "vitest";
import { modelPresets } from "../data/modelPresets";

describe("model presets", () => {
  it("keeps one-click Sherpa presets installable from Hugging Face", () => {
    const installable = modelPresets.filter((model) => model.installKind === "sherpaOnnx");

    expect(installable.map((model) => model.id)).toEqual(
      expect.arrayContaining([
        "sensevoice-small",
        "qwen3-asr-0.6b",
        "sherpa-funasr-nano",
        "whisper-base",
        "sherpa-zipformer-zh",
      ]),
    );

    for (const model of installable) {
      expect(model.downloadUrl).toContain("huggingface.co/");
      expect(model.modelFiles?.length).toBeGreaterThan(0);
      expect(model.sherpaArgs).toContain("{modelDir}");

      for (const modelFile of model.modelFiles ?? []) {
        expect(modelFile.url).toContain("huggingface.co/");
        expect(modelFile.url).toContain("/resolve/main/");
        expect(modelFile.path).not.toContain("..");
        expect(modelFile.path).not.toMatch(/^[a-zA-Z]:/);
      }
    }
  });

  it("uses the current k2-fsa Zipformer package", () => {
    const zipformer = modelPresets.find((model) => model.id === "sherpa-zipformer-zh");

    expect(zipformer?.downloadUrl).toBe(
      "https://huggingface.co/k2-fsa/sherpa-onnx-streaming-zipformer-multi-zh-hans-2023-12-12",
    );
    expect(zipformer?.modelFiles?.map((file) => file.path)).toEqual(
      expect.arrayContaining([
        "encoder-epoch-20-avg-1-chunk-16-left-128.int8.onnx",
        "decoder-epoch-20-avg-1-chunk-16-left-128.int8.onnx",
        "joiner-epoch-20-avg-1-chunk-16-left-128.int8.onnx",
        "tokens.txt",
      ]),
    );
  });

  it("keeps Qwen3-ASR 1.7B visible without pretending raw weights are ready", () => {
    const candidates = modelPresets.filter((model) => model.installKind === "engineRequired");

    expect(candidates.map((model) => model.id)).toEqual(expect.arrayContaining(["qwen3-asr-1.7b"]));

    for (const model of candidates) {
      expect(model.engineNote).toMatch(/接入|推理|运行/);
      expect(model.downloadUrl).toContain("huggingface.co/");
    }
  });
});
