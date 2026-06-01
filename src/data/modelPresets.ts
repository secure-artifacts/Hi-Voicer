import type { ModelPreset } from "../types";

const hf = (repo: string, file: string) => `https://huggingface.co/${repo}/resolve/main/${file}`;

export const modelPresets: ModelPreset[] = [
  {
    id: "sensevoice-small",
    name: "SenseVoiceSmall 中文优先",
    family: "funasr",
    installKind: "sherpaOnnx",
    size: "约 240 MB",
    quality: "中文、粤语、英文都稳，速度快",
    memory: "CPU 可跑",
    recommendedFor: "默认推荐，语音输入和低延迟文件转录",
    license: "SenseVoice 模型许可",
    downloadUrl: "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
    engineNote: "使用 Sherpa-ONNX 的 SenseVoiceSmall ONNX 版本，安装后自动写入本地配置。",
    modelFiles: [
      {
        path: "model.int8.onnx",
        url: hf("csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17", "model.int8.onnx"),
      },
      {
        path: "tokens.txt",
        url: hf("csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17", "tokens.txt"),
      },
    ],
    sherpaArgs:
      '--tokens="{modelDir}\\tokens.txt" --sense-voice-model="{modelDir}\\model.int8.onnx" --sense-voice-use-itn=1 --num-threads=4',
  },
  {
    id: "qwen3-asr-0.6b",
    name: "Qwen3-ASR 0.6B",
    family: "qwen",
    installKind: "sherpaOnnx",
    size: "约 1 GB",
    quality: "新一代多语种 ASR，质量更高",
    memory: "建议 8 GB 以上内存",
    recommendedFor: "质量优先，能接受更大下载体积",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/pantinor/sherpa-onnx-qwen3-asr-0.6b-int8",
    engineNote: "Sherpa-ONNX 兼容的 Qwen3-ASR 0.6B INT8 包，需要 conv frontend、encoder、decoder 和 tokenizer。",
    modelFiles: [
      { path: "conv_frontend.onnx", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "conv_frontend.onnx") },
      { path: "encoder.int8.onnx", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "encoder.int8.onnx") },
      { path: "decoder.int8.onnx", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "decoder.int8.onnx") },
      { path: "tokenizer/merges.txt", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "tokenizer/merges.txt") },
      {
        path: "tokenizer/tokenizer_config.json",
        url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "tokenizer/tokenizer_config.json"),
      },
      { path: "tokenizer/vocab.json", url: hf("pantinor/sherpa-onnx-qwen3-asr-0.6b-int8", "tokenizer/vocab.json") },
    ],
    sherpaArgs:
      '--qwen3-asr-conv-frontend="{modelDir}\\conv_frontend.onnx" --qwen3-asr-encoder="{modelDir}\\encoder.int8.onnx" --qwen3-asr-decoder="{modelDir}\\decoder.int8.onnx" --qwen3-asr-tokenizer="{modelDir}\\tokenizer" --feat-dim=128 --num-threads=3 --qwen3-asr-max-new-tokens=128',
  },
  {
    id: "sherpa-funasr-nano",
    name: "Sherpa FunASR-Nano",
    family: "sherpa",
    installKind: "sherpaOnnx",
    size: "约 1 GB",
    quality: "中文质量优先",
    memory: "建议 4 GB 以上内存",
    recommendedFor: "中文输入，较高准确率，下载时间更长",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30",
    engineNote: "安装后自动配置 Sherpa-ONNX 运行时和模型文件。",
    modelFiles: [
      { path: "embedding.int8.onnx", url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "embedding.int8.onnx") },
      {
        path: "encoder_adaptor.int8.onnx",
        url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "encoder_adaptor.int8.onnx"),
      },
      { path: "llm.int8.onnx", url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "llm.int8.onnx") },
      { path: "Qwen3-0.6B/merges.txt", url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "Qwen3-0.6B/merges.txt") },
      {
        path: "Qwen3-0.6B/tokenizer.json",
        url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "Qwen3-0.6B/tokenizer.json"),
      },
      { path: "Qwen3-0.6B/vocab.json", url: hf("csukuangfj/sherpa-onnx-funasr-nano-int8-2025-12-30", "Qwen3-0.6B/vocab.json") },
    ],
    sherpaArgs:
      '--funasr-nano-encoder-adaptor="{modelDir}\\encoder_adaptor.int8.onnx" --funasr-nano-llm="{modelDir}\\llm.int8.onnx" --funasr-nano-embedding="{modelDir}\\embedding.int8.onnx" --funasr-nano-tokenizer="{modelDir}\\Qwen3-0.6B" --num-threads=3',
  },
  {
    id: "whisper-base",
    name: "OpenAI Whisper Base",
    family: "whisper",
    installKind: "sherpaOnnx",
    size: "约 170 MB",
    quality: "通用、多语言、稳",
    memory: "CPU 可跑，但实时输入延迟偏高",
    recommendedFor: "多语言文件转录，兼容性验证",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/csukuangfj/sherpa-onnx-whisper-base",
    engineNote: "使用 OpenAI Whisper Base 的 Sherpa-ONNX 导出版本。",
    modelFiles: [
      { path: "base-encoder.int8.onnx", url: hf("csukuangfj/sherpa-onnx-whisper-base", "base-encoder.int8.onnx") },
      { path: "base-decoder.int8.onnx", url: hf("csukuangfj/sherpa-onnx-whisper-base", "base-decoder.int8.onnx") },
      { path: "base-tokens.txt", url: hf("csukuangfj/sherpa-onnx-whisper-base", "base-tokens.txt") },
    ],
    sherpaArgs:
      '--whisper-encoder="{modelDir}\\base-encoder.int8.onnx" --whisper-decoder="{modelDir}\\base-decoder.int8.onnx" --tokens="{modelDir}\\base-tokens.txt" --whisper-task=transcribe --num-threads=4 --model-type=whisper',
  },
  {
    id: "sherpa-paraformer-zh",
    name: "Sherpa Paraformer 中文",
    family: "sherpa",
    installKind: "sherpaOnnx",
    size: "约 250 MB",
    quality: "首装快，中文可用",
    memory: "CPU 可跑",
    recommendedFor: "低配置电脑，快速跑通全流程",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14",
    engineNote: "轻量中文模型，适合快速验证和低配置机器。",
    modelFiles: [
      { path: "model.int8.onnx", url: hf("csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14", "model.int8.onnx") },
      { path: "tokens.txt", url: hf("csukuangfj/sherpa-onnx-paraformer-zh-2023-09-14", "tokens.txt") },
    ],
    sherpaArgs:
      '--paraformer="{modelDir}\\model.int8.onnx" --tokens="{modelDir}\\tokens.txt" --num-threads=4 --decoding-method=greedy_search --model-type=paraformer',
  },
  {
    id: "sherpa-zipformer-zh",
    name: "Sherpa Zipformer 中文",
    family: "sherpa",
    installKind: "sherpaOnnx",
    size: "约 75 MB",
    quality: "短句实时识别更稳",
    memory: "CPU 可跑",
    recommendedFor: "热键语音输入，短句实时识别",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/k2-fsa/sherpa-onnx-streaming-zipformer-multi-zh-hans-2023-12-12",
    engineNote: "使用 k2-fsa 的 Zipformer transducer 模型，包含 encoder、decoder、joiner 和 tokens。",
    modelFiles: [
      {
        path: "encoder-epoch-20-avg-1-chunk-16-left-128.int8.onnx",
        url: hf("k2-fsa/sherpa-onnx-streaming-zipformer-multi-zh-hans-2023-12-12", "encoder-epoch-20-avg-1-chunk-16-left-128.int8.onnx"),
      },
      {
        path: "decoder-epoch-20-avg-1-chunk-16-left-128.int8.onnx",
        url: hf("k2-fsa/sherpa-onnx-streaming-zipformer-multi-zh-hans-2023-12-12", "decoder-epoch-20-avg-1-chunk-16-left-128.int8.onnx"),
      },
      {
        path: "joiner-epoch-20-avg-1-chunk-16-left-128.int8.onnx",
        url: hf("k2-fsa/sherpa-onnx-streaming-zipformer-multi-zh-hans-2023-12-12", "joiner-epoch-20-avg-1-chunk-16-left-128.int8.onnx"),
      },
      { path: "tokens.txt", url: hf("k2-fsa/sherpa-onnx-streaming-zipformer-multi-zh-hans-2023-12-12", "tokens.txt") },
    ],
    sherpaArgs:
      '--encoder="{modelDir}\\encoder-epoch-20-avg-1-chunk-16-left-128.int8.onnx" --decoder="{modelDir}\\decoder-epoch-20-avg-1-chunk-16-left-128.int8.onnx" --joiner="{modelDir}\\joiner-epoch-20-avg-1-chunk-16-left-128.int8.onnx" --tokens="{modelDir}\\tokens.txt" --num-threads=4 --decoding-method=greedy_search --model-type=transducer',
  },
  {
    id: "qwen3-asr-1.7b",
    name: "Qwen3-ASR 1.7B",
    family: "qwen",
    installKind: "engineRequired",
    size: "约 3-5 GB",
    quality: "质量优先",
    memory: "建议独显或高内存机器",
    recommendedFor: "后续高精度文件转录",
    license: "Apache 2.0",
    downloadUrl: "https://huggingface.co/Qwen/Qwen3-ASR-1.7B",
    engineNote: "暂未接入一键运行。需要稳定的本地推理方案后再开放。",
  },
];

export function findModelPreset(modelId: string) {
  return modelPresets.find((model) => model.id === modelId);
}
