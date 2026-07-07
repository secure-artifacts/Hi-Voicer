# Hi-Voicer 1.1.0 发布说明

## 发布重点

- SenseVoice DirectML 路线接入真实语音输入和文件转录，并缓存 DirectML 会话，首次后短语音输入延迟明显降低。
- Qwen3-ASR 0.6B 长音频转录恢复质量优先参数：20 秒分块、128 token 上限，并启用 Sherpa WebSocket daemon 加速。
- Qwen 长音频增加静音块跳过，含无人声素材的视频可以减少无效识别时间。
- 文件转录任务支持停止。运行中的任务可点击停止按钮，后端会停止继续派发新分块。
- 诊断页增加 DirectML PoC、CPU smoke test、CPU vs DirectML benchmark 和更明确的运行路径提示。
- 增加 Faster-Whisper 外部 worker PoC 文档与脚本，作为后续 NVIDIA/CUDA 长文件转录路线预留。
- 模型列表移除发展潜力较低的 Whisper Base 和 Zipformer 安装入口，保留后端兼容。

## 使用建议

- 普通短语音输入：优先使用 SenseVoiceSmall + DirectML。
- 中文长音频/视频转录：优先使用 Qwen3-ASR 0.6B，性能模式建议先用“平衡”。
- 长时间批处理：可以尝试“速度”模式，但 CPU 占用、风扇和系统卡顿会增加。
- 含无人声素材的视频：时间轴出现空段可能是正常结果，不一定是漏转。
- 原视频含外语现场声时，转录结果会保留原语种，不强制转中文。

## 安全与依赖

- 更新 npm 锁文件中的 `form-data` 到 4.0.6，修复 GitHub Dependabot 报告的 CRLF injection 告警。该依赖来自测试用的 `jsdom` 传递依赖，不属于桌面运行时核心路径。
- 移除未使用的 Tauri dialog 插件，前端文件选择统一走后端 `rfd` 命令，清理旧 `tauri-plugin-dialog -> rfd 0.16 -> glib` 传递依赖链，避免组织安全页继续报告 `glib` 锁文件告警。
## 发布前验证

- `cargo test --manifest-path src-tauri/Cargo.toml`
- `npm test`
- `npm run build`
- `git diff --check`
- `npm run tauri build`

## 已知限制

- Qwen3-ASR 0.6B 目前仍是 Sherpa CPU 路线，不是 DirectML GPU 路线。
- 停止转录不是强杀整个应用；如果底层正在识别当前小块，通常会等当前块返回后停止后续分块。
- Faster-Whisper 仍是外部引擎 PoC，不作为默认内置安装包分发。