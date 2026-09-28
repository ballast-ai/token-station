"""Offline SCX worker. Standard output is a bounded JSONL protocol."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import sys

LABELS = ["very easy", "easy", "medium", "hard", "extra hard"]
TIERS = ["low", "low", "medium", "high", "high"]
MAX_TOKENS = 1024
MAX_LINE_BYTES = 262144
MAX_TEXT_BYTES = 65536


class UnsupportedInput(Exception):
    pass


class TokenLimit(UnsupportedInput):
    pass


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def verify_model(path, assets):
    for name, expected in assets["files"].items():
        if Path(name).name != name:
            raise ValueError("Invalid asset name")
        artifact = path / name
        if artifact.is_symlink() or artifact.stat().st_size != expected["bytes"]:
            raise ValueError("Invalid model asset")
        if digest(artifact) != expected["sha256"]:
            raise ValueError("Invalid model checksum")


def full_token_count(pipe, tokenizer, text):
    contexts, labels = pipe._build_context_and_labels([text], LABELS, True, None, None)
    return sum(len(tokenizer(value, add_special_tokens=False, truncation=False)["input_ids"])
               for value in (contexts[0], labels[0]))


class SCXBackend:
    def __init__(self, model_path, assets):
        sys.dont_write_bytecode = True
        for name, value in {
            "HF_HUB_OFFLINE": "1", "TRANSFORMERS_OFFLINE": "1",
            "HF_HUB_DISABLE_TELEMETRY": "1", "HF_HUB_DISABLE_PROGRESS_BARS": "1",
            "TOKENIZERS_PARALLELISM": "false", "OMP_NUM_THREADS": "2",
            "MKL_NUM_THREADS": "2", "PYTHONDONTWRITEBYTECODE": "1",
        }.items():
            os.environ[name] = value
        # All assets are validated before any model package is imported.
        verify_model(model_path, assets)
        import torch
        from gliclass import GLiClassModel, ZeroShotClassificationPipeline
        from transformers import AutoTokenizer

        torch.set_num_threads(2)
        torch.set_num_interop_threads(1)
        self.torch = torch
        self.device = torch.device("mps" if torch.backends.mps.is_available() else "cpu")
        self.tokenizer = AutoTokenizer.from_pretrained(
            model_path, local_files_only=True, trust_remote_code=False)
        model = GLiClassModel.from_pretrained(
            model_path, local_files_only=True, trust_remote_code=False, use_safetensors=True)
        if model.config.architecture_type != "decoder-kv":
            raise ValueError("Unsupported model architecture")
        model.eval()
        self.pipeline = ZeroShotClassificationPipeline(
            model, self.tokenizer, classification_type="single-label",
            device=self.device, max_length=MAX_TOKENS, progress_bar=False)
        self.classify("Translate hello into French.")

    def classify(self, text):
        # Use the exact formatter used by the pinned pipeline. Context and label
        # segments are tokenized separately there. Count both before inference.
        if full_token_count(self.pipeline.pipe, self.tokenizer, text) > MAX_TOKENS:
            raise TokenLimit()
        with self.torch.inference_mode():
            scores = self.pipeline(text, LABELS, batch_size=1, return_hierarchical=True)[0]
        if self.device.type == "mps":
            self.torch.mps.synchronize()
        if set(scores) != set(LABELS) or any(
            not isinstance(score, (int, float)) or not math.isfinite(score)
            or score < 0 or score > 1 for score in scores.values()
        ):
            raise ValueError("Invalid classifier output")
        best = max(range(len(LABELS)), key=lambda index: scores[LABELS[index]])
        return TIERS[best]


def valid_id(value):
    return (type(value) is int and 0 <= value <= 2**64 - 1) or (
        isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", value) is not None)


def handle(value, backend):
    if not isinstance(value, dict) or not valid_id(value.get("id")):
        return {"id": None, "status": "error"}
    request_id = value["id"]
    text = value.get("text")
    if set(value) != {"id", "text"} or not isinstance(text, str) or not text.strip():
        return {"id": request_id, "status": "unsupported"}
    try:
        if len(text.encode("utf-8")) > MAX_TEXT_BYTES or any(
            marker in text for marker in ["<<LABEL>>", "<<SEP>>", "<<EXAMPLE>>"]
        ):
            return {"id": request_id, "status": "unsupported"}
        tier = backend.classify(text)
        if tier not in TIERS:
            raise ValueError("Invalid tier")
        return {"id": request_id, "status": "ok", "tier": tier}
    except TokenLimit:
        return {"id": request_id, "status": "unsupported", "reason": "token_limit"}
    except UnsupportedInput:
        return {"id": request_id, "status": "unsupported"}
    except Exception:
        # Exception strings can contain request text. Never send them over IPC.
        return {"id": request_id, "status": "error"}


def emit(output, value):
    output.write(json.dumps(value, separators=(",", ":"), allow_nan=False) + "\n")
    output.flush()


def serve(stream, output, backend):
    while True:
        line = stream.readline(MAX_LINE_BYTES + 1)
        if not line:
            return
        if len(line) > MAX_LINE_BYTES:
            while line and not line.endswith(b"\n"):
                line = stream.readline(MAX_LINE_BYTES + 1)
            emit(output, {"id": None, "status": "error"})
            continue
        try:
            value = json.loads(line)
        except (ValueError, UnicodeError):
            emit(output, {"id": None, "status": "error"})
            continue
        emit(output, handle(value, backend))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", type=Path, required=True)
    args = parser.parse_args()
    # Keep a private protocol descriptor. Native extensions and Python packages
    # cannot contaminate JSONL or leak prompt-containing errors through logs.
    output = os.fdopen(os.dup(sys.stdout.fileno()), "w", buffering=1)
    with open(os.devnull, "w") as quiet:
        os.dup2(quiet.fileno(), sys.stdout.fileno())
        os.dup2(quiet.fileno(), sys.stderr.fileno())
    try:
        assets = json.loads(Path(__file__).with_name("assets.json").read_text())
        if assets["labels"] != LABELS or assets["tiers"] != TIERS or assets["max_length"] != MAX_TOKENS:
            raise ValueError("Invalid routing policy")
        backend = SCXBackend(args.model.resolve(), assets)
    except Exception:
        emit(output, {"event": "error", "code": "initialization_failed"})
        return 1
    emit(output, {"event": "ready"})
    try:
        serve(sys.stdin.buffer, output, backend)
    except (BrokenPipeError, OSError):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
