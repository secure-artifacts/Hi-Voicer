# Faster-Whisper Worker PoC

Hi-Voicer can route file transcription to an external Faster-Whisper worker when the selected model directory contains an `engine.json` with `engine` set to `faster-whisper`.

## engine.json

```json
{
  "engine": "faster-whisper",
  "modelId": "faster-whisper",
  "modelName": "Faster-Whisper Large v3 Turbo",
  "modelDir": "C:\\Users\\TOM\\AppData\\Local\\com.local.hivoicer\\models\\faster-whisper-large-v3-turbo",
  "executable": "C:\\Users\\TOM\\AppData\\Local\\com.local.hivoicer\\engines\\faster-whisper\\fw-worker.exe",
  "args": "--device cuda --compute-type int8_float16 --language zh --vad-filter true --condition-on-previous-text false",
  "requiredFiles": ["model.bin"]
}
```

`args` supports these placeholders:

- `{modelDir}`: selected model directory
- `{audioPath}`: transcoded 16 kHz mono WAV path
- `{outputJson}`: worker output JSON path

Hi-Voicer always appends:

```text
--audio <wav> --model-dir <modelDir> --output-json <json>
```

## Worker Output

The worker should write UTF-8 JSON to `--output-json`. If the file is not created, Hi-Voicer will try to parse stdout as JSON.

```json
{
  "text": "完整文本，可选",
  "segments": [
    { "start": 0.0, "end": 3.2, "text": "第一段" },
    { "start": 3.2, "end": 7.0, "text": "第二段" }
  ]
}
```

When `segments` is present, Hi-Voicer uses segment timestamps for SRT/timeline output. When only `text` is present, Hi-Voicer creates estimated timestamps from the full audio duration.

## Recommended Faster-Whisper Defaults

```text
--language zh
--vad-filter true
--condition-on-previous-text false
--beam-size 5
```

For NVIDIA GPU:

```text
--device cuda --compute-type int8_float16
```

For CPU fallback:

```text
--device cpu --compute-type int8
```
## Worker Source

The PoC worker source lives at:

- `scripts/faster-whisper-worker/fw_worker.py`
- `scripts/faster-whisper-worker/requirements.txt`
- `scripts/faster-whisper-worker/engine.example.json`

The main app integration is already engine-agnostic: any executable that follows the CLI and JSON contract above can be used as the worker.