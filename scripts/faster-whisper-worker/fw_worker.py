#!/usr/bin/env python3
"""Faster-Whisper worker for Hi-Voicer.

This worker intentionally keeps a small CLI surface so Hi-Voicer can call it as
an external runtime without embedding Python or CUDA dependencies into the main
Tauri app.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


def str_to_bool(value: str | bool) -> bool:
    if isinstance(value, bool):
        return value
    normalized = value.strip().lower()
    if normalized in {"1", "true", "yes", "y", "on"}:
        return True
    if normalized in {"0", "false", "no", "n", "off"}:
        return False
    raise argparse.ArgumentTypeError(f"invalid boolean value: {value}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Hi-Voicer Faster-Whisper worker")
    parser.add_argument("--audio", required=True, help="Input audio path, usually 16 kHz mono WAV")
    parser.add_argument("--model-dir", required=True, help="Faster-Whisper/CTranslate2 model directory or model id")
    parser.add_argument("--output-json", required=True, help="Where to write UTF-8 JSON output")
    parser.add_argument("--device", default="cpu", help="cpu or cuda")
    parser.add_argument("--device-index", type=int, default=0, help="CUDA device index")
    parser.add_argument("--compute-type", default="int8", help="int8, float16, int8_float16, float32, etc.")
    parser.add_argument("--language", default="zh", help="Language code; use empty string for auto-detect")
    parser.add_argument("--task", default="transcribe", choices=["transcribe", "translate"])
    parser.add_argument("--beam-size", type=int, default=5)
    parser.add_argument("--best-of", type=int, default=5)
    parser.add_argument("--vad-filter", type=str_to_bool, default=True)
    parser.add_argument("--word-timestamps", type=str_to_bool, default=False)
    parser.add_argument("--condition-on-previous-text", type=str_to_bool, default=False)
    parser.add_argument("--temperature", type=float, default=0.0)
    parser.add_argument("--initial-prompt", default=None)
    parser.add_argument("--hotwords", default=None)
    parser.add_argument("--threads", type=int, default=0, help="CPU thread count; 0 lets CTranslate2 decide")
    parser.add_argument("--workers", type=int, default=1, help="CTranslate2 worker count")
    return parser


def segment_to_dict(segment: Any) -> dict[str, Any]:
    return {
        "start": float(getattr(segment, "start", 0.0) or 0.0),
        "end": float(getattr(segment, "end", 0.0) or 0.0),
        "text": str(getattr(segment, "text", "") or "").strip(),
    }


def transcribe(args: argparse.Namespace) -> dict[str, Any]:
    try:
        from faster_whisper import WhisperModel
    except Exception as error:  # pragma: no cover - depends on runtime env
        raise RuntimeError(
            "faster-whisper is not installed. Install the worker environment first."
        ) from error

    audio_path = Path(args.audio)
    if not audio_path.exists():
        raise FileNotFoundError(f"audio file does not exist: {audio_path}")

    cpu_threads = args.threads if args.threads > 0 else 0
    model = WhisperModel(
        args.model_dir,
        device=args.device,
        device_index=args.device_index,
        compute_type=args.compute_type,
        cpu_threads=cpu_threads,
        num_workers=max(1, args.workers),
    )

    language = args.language.strip() or None
    segments_iter, info = model.transcribe(
        str(audio_path),
        language=language,
        task=args.task,
        beam_size=args.beam_size,
        best_of=args.best_of,
        vad_filter=args.vad_filter,
        word_timestamps=args.word_timestamps,
        condition_on_previous_text=args.condition_on_previous_text,
        temperature=args.temperature,
        initial_prompt=args.initial_prompt,
        hotwords=args.hotwords,
    )

    segments = [segment_to_dict(segment) for segment in segments_iter]
    text = "".join(segment["text"] for segment in segments).strip()
    return {
        "text": text,
        "segments": segments,
        "language": getattr(info, "language", None),
        "languageProbability": getattr(info, "language_probability", None),
        "duration": getattr(info, "duration", None),
        "device": args.device,
        "computeType": args.compute_type,
    }


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    output_path = Path(args.output_json)

    try:
        result = transcribe(args)
        output_path.parent.mkdir(parents=True, exist_ok=True)
        output_path.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
        print(json.dumps({"ok": True, "outputJson": str(output_path)}, ensure_ascii=False))
        return 0
    except Exception as error:
        payload = {"ok": False, "error": str(error), "errorType": type(error).__name__}
        try:
            output_path.parent.mkdir(parents=True, exist_ok=True)
            output_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
        except Exception:
            pass
        print(json.dumps(payload, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())