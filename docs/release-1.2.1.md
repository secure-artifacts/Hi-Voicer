# Hi-Voicer 1.2.1 发布说明

## 发布重点

- 新增 Qwen3-ASR 0.6B 高效版：Q8 GGUF 主模型配合 llama.cpp 纯 CPU 服务，用于准确率优先的文件转录。
- 模型下载支持断点续传、文件大小检查和 SHA-256 校验；模型目录可以完整复制到其他电脑。
- 安装包内置最小 Sherpa-ONNX CPU 运行时、llama.cpp、FFmpeg 和 FFprobe，不包含 CUDA、cuDNN、TensorRT 或 Vulkan 运行文件。
- 修复 GitHub Actions 对新版 FFmpeg 体积估算过低导致的发布阻断；应用功能与 1.2.0 候选版本一致。

## 发布验证

- 前端测试：72 项通过。
- Rust 发布配置测试：105 项通过。
- Qwen3-ASR GGUF、SenseVoice 和 Qwen3 兼容服务已完成真实运行验证。
- 正式 NSIS 与 MSI 安装包由 GitHub Actions 从 `v1.2.1` 标签构建，并生成 provenance attestation。

## 已知限制

- Qwen3-ASR GGUF 主模型与音频投影器合计约 972 MiB，需要独立下载或复制。
- DirectML 仍需在每台电脑上单独验证；正式安装包不包含 CUDA 运行时。
