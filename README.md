# Hi-Voicer

Hi-Voicer 是一个本地离线中文语音输入和文件转录桌面软件。当前版本参考 CapsWriter-Offline / ime_audio 的本地 ASR 思路，优先走 Sherpa-ONNX CPU 稳定路线：软件负责录音、转码、调用本地模型、把结果粘贴上屏或保存为文本文件。

<img width="1919" height="976" alt="屏幕截图 2026-06-01 181536" src="https://github.com/user-attachments/assets/de738e7b-8eea-48ee-afd5-83bfb2bdffbf" />

## 普通用户怎么用

1. 从 [GitHub Releases](https://github.com/secure-artifacts/Hi-Voicer/releases) 下载最新的 `Hi-Voicer_0.2.1_x64-setup.exe`，也可以下载同版本 MSI 包。
2. 打开 Hi-Voicer，进入“设置”。
3. 在“离线模型”里选择一个支持自动配置的 Sherpa 模型，点击“下载并配置”。
4. 回到首页，在任意输入框按住快捷键说话，松开后自动识别并粘贴。
5. 文件转录在“转录”页选择音频或视频文件，结果可保存为纯文本，或带时间线文本加 `.srt` 字幕。

普通用户不需要安装 Node.js、Rust、Visual Studio Build Tools 这些开发工具。首次配置模型时需要联网，软件会下载 Sherpa-ONNX 运行时、模型文件和 ffmpeg；下载完成后，录音识别和文件转录在本地运行。Windows 需要 Microsoft Edge WebView2 Runtime，绝大多数 Windows 10/11 机器已经自带；如果极少数电脑打不开软件，先安装 WebView2 Runtime。

## 验证软件来源

发布安装包由 GitHub Actions 在 tag push 后自动构建并上传，不使用本地手工打包产物。下载后可以用 GitHub CLI 验证来源：

```powershell
gh attestation verify .\Hi-Voicer_0.2.1_x64-setup.exe --repo secure-artifacts/Hi-Voicer
```

## 当前模型策略

0.2.1 正式版继续维护 Sherpa-ONNX CPU 运行时，避免普通用户同时维护太多引擎。模型文件统一从 Hugging Face 下载；Sherpa-ONNX 运行时来自官方 GitHub Release，它是本地推理程序，不是模型源。

- SenseVoiceSmall：默认推荐，中文输入和短音频延迟更合适，可一键配置。
- Qwen3-ASR 0.6B：可一键配置，体积更大，适合试 Qwen3-ASR 路线。
- Sherpa FunASR-Nano：可一键配置，中文质量优先，下载时间更长。
- OpenAI Whisper Base：可一键配置，适合多语言文件转录验证。
- Sherpa Paraformer / Zipformer：轻量备用模型，适合低配置电脑快速跑通。
- Qwen3-ASR 1.7B：先保留为候选入口；官方原始权重不能直接当本地转录模型用，需要稳定的 ONNX/GGUF 推理包后再做一键运行。

## 常驻和启动

- 正式安装包不会显示服务端窗口。
- 关闭主窗口会隐藏到系统托盘。
- 托盘左键可重新打开，托盘菜单可退出。
- 设置里可以开启开机启动，登录 Windows 后自动后台常驻。
- 语音输入会粘贴上屏，并在首页保留最近文字历史，可复制或下载。
- 需要排查时可在设置里开启“保存录音”，录音片段会保留在应用数据目录的 `recordings` 文件夹。
- 软件包含一个置顶 mini 录制按钮窗口，录制时底部会显示声波提示。

## 数据和迁移

用户设置、下载的模型、Sherpa-ONNX、ffmpeg、录音缓存都放在系统的应用数据目录下，不写到项目源码目录。换电脑时建议：

1. 先安装 Hi-Voicer。
2. 打开一次软件，让系统创建应用数据目录。
3. 重新在设置里点击“下载并配置”模型，或者把旧电脑应用数据目录里的 `models` 和 `engines` 复制过去。

如果 `models` 目录里已经有可用模型，软件启动时会自动绑定，不需要每次重新选择模型目录。

## GPU 加速

GPU 加速还在后续实验阶段，0.2.1 正式版默认发布 CPU 稳定路线。当前 SenseVoice + Sherpa-ONNX CPU 方案在中文转录速度和准确度之间更稳，后续会另开 GPU 后端验证路线。

开发源码和依赖都留在当前项目目录里，适合整体迁移：

- Node 依赖锁定在 `package-lock.json`。
- Rust 工具链锁定在 `rust-toolchain.toml`。
- Windows 环境检查脚本在 `scripts\check-env.ps1`。
- Windows 环境安装脚本在 `scripts\setup-windows.ps1`。

## 开发验证

```powershell
npm ci
npm test
npm run build
cargo check --manifest-path src-tauri\Cargo.toml
npm run tauri -- build
```

更多模型说明见 `docs\模型说明.md`，环境说明见 `docs\环境准备.md`。
