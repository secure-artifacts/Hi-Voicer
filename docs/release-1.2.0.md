# Hi-Voicer 1.2.0 发布说明

## 发布重点

- 新增 Qwen3-ASR 0.6B 高效版：Q8 GGUF 主模型配合 llama.cpp 纯 CPU 服务，用于准确率优先的文件转录。
- 高效版按 60 秒切分长音频，复用常驻本地服务，并在空闲 5 分钟后自动释放内存。
- 模型下载支持断点续传、文件大小检查和 SHA-256 校验。
- Qwen GGUF、SenseVoice、Qwen3 兼容版、FunASR-Nano 和 Paraformer 模型目录可以整体复制到其他电脑；软件自动使用模型目录的实际位置。
- 安装包内置约 33.1 MiB 的最小 Sherpa-ONNX CPU 运行时，以及 llama.cpp、FFmpeg 和 FFprobe；复制模型后不需要额外下载引擎。
- CUDA、cuDNN、TensorRT 和 Vulkan 运行文件不进入正式安装包。
- 全局快捷键被其他程序占用时不再导致应用启动崩溃，用户仍可进入设置修改快捷键。

## 模型分发

模型与安装包保持分离。可以把完整模型目录压缩后交给其他用户，接收方解压到 `%LOCALAPPDATA%\com.local.hivoicer\models`，或通过“选择已有模型目录”指定其他磁盘位置。

GGUF 高效版目录必须包含 `engine.json`、`Qwen3-ASR-0.6B-Q8_0.gguf` 和 `mmproj-Qwen3-ASR-0.6B-Q8_0.gguf`。Sherpa 模型同样需要保留完整目录结构和 `engine.json`。

## 发布验证

- 前端测试：72 项通过。
- Rust 发布配置测试：105 项通过。
- Qwen3-ASR GGUF 音频接口、SenseVoice 静态 CPU 识别和 Qwen3 兼容 WebSocket 服务完成真实运行验证。
- 资源检查：243.9 MiB，未发现 CUDA、cuDNN、TensorRT 或 Vulkan 文件。
- Windows NSIS 与 MSI 安装包本地构建成功；正式资产由 GitHub Actions 重新构建并生成 provenance attestation。

## 已知限制

- Qwen3-ASR GGUF 模型和音频投影器合计约 972 MiB，需要单独下载或复制。
- llama.cpp 音频输入仍属于上游实验能力，因此继续保留 Qwen3-ASR 兼容版作为回退方案。
- DirectML 需要在每台电脑上单独验证；正式安装包不包含 CUDA 运行时。
