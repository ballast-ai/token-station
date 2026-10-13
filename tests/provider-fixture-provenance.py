#!/usr/bin/env python3
"""Test repository-authored and vendored fixture provenance in isolated trees."""

import contextlib
import importlib.util
import io
import json
import pathlib
import sys
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "scripts/check-provider-fixtures.py"
# Loading the CLI as a module must not write cache files into the source tree.
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("fixture_provenance", SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class FixtureProvenanceTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = pathlib.Path(self.temporary.name)
        CHECK.ROOT = root
        CHECK.PROVENANCE = root / "plugins/official/fixture-provenance.json"
        CHECK.PACKAGES = root / "plugins/official/packages.json"
        self.checkout = root / "south-test-revision"
        upstream = self.checkout / "crates/south-component-conformance/fixtures"
        upstream.mkdir(parents=True)
        self.vendored = self.pack("provider-vendored", "fixtures", "south case")
        self.local = self.pack("provider-local", "fixtures-local", "repository case")
        (upstream / "case.json").write_text("south case")
        CHECK.PACKAGES.write_text(json.dumps({"packages": [
            {"dir": "provider-vendored", "kind": "south-component"},
            {"dir": "provider-local", "kind": "south-component"},
        ]}))
        self.recorded = {"south_revision": "south-test-revision", "packages": {}}
        for name, directory, source in [
            ("provider-vendored", self.vendored, None),
            ("provider-local", self.local, "repository"),
        ]:
            digest, count = CHECK.tree_digest(directory)
            entry = {"fixtures": directory.name, "files": count, "sha256": digest}
            if source:
                entry["source"] = source
            self.recorded["packages"][name] = entry
        self.save()

    def pack(self, name, fixtures, contents):
        package = CHECK.ROOT / "plugins/official" / name
        directory = package / fixtures
        directory.mkdir(parents=True)
        (package / "manifest.json").write_text(json.dumps({"conformance": {"fixtures": fixtures}}))
        (directory / "case.json").write_text(contents)
        return directory

    def save(self):
        CHECK.PROVENANCE.write_text(json.dumps(self.recorded))

    def invoke(self, function, argument):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            result = function(argument)
        return result, output.getvalue()

    def test_repository_pack_skips_only_the_south_comparison(self):
        result, output = self.invoke(CHECK.cmd_verify, self.checkout)
        self.assertEqual(result, 0, output)
        self.assertNotIn("2 packs match South", output)

    def test_repository_content_drift_still_fails(self):
        (self.local / "case.json").write_text("changed locally")
        result, output = self.invoke(CHECK.cmd_verify, self.checkout)
        self.assertEqual(result, 1)
        self.assertIn("provider-local", output)
        self.assertIn("repository", output)

    def test_recorded_file_count_is_verified_independently(self):
        self.recorded["packages"]["provider-local"]["files"] = 99
        self.save()
        result, output = self.invoke(CHECK.cmd_verify, None)
        self.assertEqual(result, 1)
        self.assertIn("99", output)

    def test_record_preserves_repository_source(self):
        result, output = self.invoke(CHECK.cmd_record, self.checkout)
        self.assertEqual(result, 0, output)
        recorded = json.loads(CHECK.PROVENANCE.read_text())
        self.assertEqual(recorded["packages"]["provider-local"]["source"], "repository")
        self.assertEqual(recorded["packages"]["provider-vendored"].get("source", "south"), "south")
        result, output = self.invoke(CHECK.cmd_verify, self.checkout)
        self.assertEqual(result, 0, output)

    def test_legacy_south_packs_still_require_byte_identical_upstream(self):
        upstream = self.checkout / "crates/south-component-conformance/fixtures/case.json"
        upstream.write_text("different upstream")
        result, output = self.invoke(CHECK.cmd_verify, self.checkout)
        self.assertEqual(result, 1)
        self.assertIn("provider-vendored: differs from South", output)

    def test_unknown_source_cannot_bypass_verification_or_be_recorded(self):
        self.recorded["packages"]["provider-local"]["source"] = "unreviewed"
        self.save()
        before = CHECK.PROVENANCE.read_bytes()
        for function in [CHECK.cmd_verify, CHECK.cmd_record]:
            result, output = self.invoke(function, self.checkout)
            self.assertEqual(result, 1, output)
            self.assertIn("source", output)
        self.assertEqual(CHECK.PROVENANCE.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
