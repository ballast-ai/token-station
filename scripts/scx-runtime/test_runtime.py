"""Protocol and preparation checks without downloading or loading a model."""

import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Backend:
    def __init__(self):
        self.calls = []

    def classify(self, text):
        self.calls.append(text)
        if text == "fail":
            raise RuntimeError("private exception text")
        return "high" if text == "hard" else "low"


class RuntimeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.worker = load("worker")
        cls.setup = load("setup")

    def test_valid_request_has_a_bounded_content_free_response(self):
        backend = Backend()
        result = self.worker.handle({"id": 7, "text": "hard"}, backend)
        self.assertEqual(result, {"id": 7, "status": "ok", "tier": "high"})
        self.assertEqual(backend.calls, ["hard"])

    def test_invalid_projection_does_not_call_backend(self):
        backend = Backend()
        for value in [{"id": 1, "text": " "}, {"id": 2, "text": []},
                      {"id": 3, "text": "<<LABEL>>low"}, {"id": 4, "text": "hi", "image": "x"}]:
            self.assertEqual(self.worker.handle(value, backend)["status"], "unsupported")
        self.assertEqual(backend.calls, [])

    def test_exception_and_invalid_id_never_echo_private_text(self):
        backend = Backend()
        result = self.worker.handle({"id": 1, "text": "fail"}, backend)
        self.assertEqual(result, {"id": 1, "status": "error"})
        invalid = self.worker.handle({"id": {"secret": "private"}, "text": "hi"}, backend)
        self.assertEqual(invalid, {"id": None, "status": "error"})

    def test_oversized_formatted_input_skips_before_inference(self):
        class Pipe:
            def _build_context_and_labels(self, texts, labels, same, examples, prompt):
                return ["x" * 1000], ["y" * 25]
        class Tokenizer:
            def __call__(self, text, **kwargs):
                self.kwargs = kwargs
                return {"input_ids": list(text)}
        tokenizer = Tokenizer()
        self.assertEqual(self.worker.full_token_count(Pipe(), tokenizer, "input"), 1025)
        self.assertFalse(tokenizer.kwargs.get("truncation", False))
        self.assertFalse(tokenizer.kwargs["add_special_tokens"])
        backend = object.__new__(self.worker.SCXBackend)
        backend.pipeline = type("Pipeline", (), {"pipe": Pipe()})()
        backend.tokenizer = tokenizer
        with self.assertRaises(self.worker.UnsupportedInput):
            backend.classify("input")

    def test_token_limit_reason_is_distinct_from_unsupported_content(self):
        class TooLong:
            def classify(self, text):
                raise self_worker.TokenLimit()
        self_worker = self.worker
        result = self.worker.handle({"id": 8, "text": "task"}, TooLong())
        self.assertEqual(result, {"id": 8, "status": "unsupported", "reason": "token_limit"})
        marker = self.worker.handle({"id": 9, "text": "<<LABEL>>private"}, Backend())
        self.assertEqual(marker, {"id": 9, "status": "unsupported"})
        self.assertNotIn("private", json.dumps(marker))

    def test_formatted_token_boundary_is_exact_and_never_enables_truncation(self):
        import contextlib
        class Pipe:
            def _build_context_and_labels(self, texts, labels, same, examples, prompt):
                return [texts[0]], ["y" * 24]
        class Tokenizer:
            def __call__(self, text, **kwargs):
                self.assertions.append(kwargs)
                return {"input_ids": list(text)}
        class Pipeline:
            pipe = Pipe()
            calls = 0
            def __call__(self, *args, **kwargs):
                self.calls += 1
                return [{label: 1.0 if index == 0 else 0.0 for index, label in enumerate(self_worker.LABELS)}]
        self_worker = self.worker
        backend = object.__new__(self.worker.SCXBackend)
        backend.pipeline = Pipeline()
        backend.tokenizer = Tokenizer()
        backend.tokenizer.assertions = []
        backend.torch = type("Torch", (), {"inference_mode": staticmethod(contextlib.nullcontext)})()
        backend.device = type("Device", (), {"type": "cpu"})()
        self.assertEqual(backend.classify("x" * 1000), "low")
        with self.assertRaises(self.worker.TokenLimit):
            backend.classify("x" * 1001)
        self.assertEqual(backend.pipeline.calls, 1)
        self.assertTrue(all(not call["truncation"] for call in backend.tokenizer.assertions))

    def test_stream_recovers_after_invalid_json_and_large_line(self):
        data = b'{\n' + b'x' * (self.worker.MAX_LINE_BYTES + 1) + b'\n'
        data += b'{"id":"next","text":"hi"}\n'
        output = io.StringIO()
        self.worker.serve(io.BytesIO(data), output, Backend())
        rows = [json.loads(line) for line in output.getvalue().splitlines()]
        self.assertEqual([row["status"] for row in rows], ["error", "error", "ok"])
        self.assertEqual(rows[-1]["id"], "next")

    def test_model_verification_rejects_wrong_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            (path / "config.json").write_bytes(b"bad")
            with self.assertRaises(ValueError):
                self.setup.verify_files(path, {"config.json": {"bytes": 3, "sha256": "0" * 64}})

    def test_manifest_filenames_cannot_escape_model_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                self.setup.verify_files(Path(directory), {"../outside": {"bytes": 0, "sha256": "0" * 64}})

    def test_cancelled_stage_is_recovered_but_symlink_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stage = root / ".venv-preparing"
            stage.mkdir()
            (stage / "partial").write_text("partial")
            self.setup.clear_stage(stage)
            self.assertFalse(stage.exists())
            protected = root / "protected"
            protected.mkdir()
            (protected / "keep").write_text("keep")
            stage.symlink_to(protected, target_is_directory=True)
            with self.assertRaises(ValueError):
                self.setup.clear_stage(stage)
            self.assertEqual((protected / "keep").read_text(), "keep")


if __name__ == "__main__":
    unittest.main()
