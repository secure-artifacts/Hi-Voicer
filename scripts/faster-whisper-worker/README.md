# Faster-Whisper Worker

This folder contains the minimal external worker used by Hi-Voicer's Faster-Whisper PoC route.

## Local run

```powershell
py -3.11 -m venv .venv-fw
.\.venv-fw\Scripts\python.exe -m pip install -U pip
.\.venv-fw\Scripts\python.exe -m pip install -r scripts\faster-whisper-worker\requirements.txt
.\.venv-fw\Scripts\python.exe scripts\faster-whisper-worker\fw_worker.py `
  --audio C:\path\sample.wav `
  --model-dir C:\path\faster-whisper-model `
  --output-json C:\tmp\fw-output.json `
  --device cpu `
  --compute-type int8 `
  --language zh `
  --vad-filter true `
  --condition-on-previous-text false
```

For NVIDIA GPU, use:

```text
--device cuda --compute-type int8_float16
```

## Hi-Voicer engine.json

Copy `engine.example.json` into the model directory and adjust:

- `modelDir`: the local Faster-Whisper/CTranslate2 model directory
- `executable`: the final `fw-worker.exe` path, or a Python launcher wrapper during development
- `args`: default faster-whisper options

Hi-Voicer appends `--audio`, `--model-dir`, and `--output-json` automatically.

## Packaging sketch

A standalone worker can be built with PyInstaller after installing the runtime dependencies:

```powershell
.\.venv-fw\Scripts\python.exe -m pip install pyinstaller
.\.venv-fw\Scripts\pyinstaller.exe --onefile --name fw-worker scripts\faster-whisper-worker\fw_worker.py
```

CUDA packaging is intentionally separate from the main Hi-Voicer installer. Keep CPU and CUDA worker packages separate so the main app stays small.