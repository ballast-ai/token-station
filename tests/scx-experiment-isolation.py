#!/usr/bin/env python3
"""Verify SCX configuration copying without touching user settings."""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("scx_prepare", ROOT / "scripts/prepare-scx-desktop.py")
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class IsolationTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.support = Path(self.scratch.name).resolve()
        self.stable = self.support / prepare.STABLE_ID
        self.experiment = self.support / prepare.EXPERIMENT_ID
        self.stable.mkdir()
        self.data = self.stable / "token-station-data"
        self.data.mkdir()
        (self.stable / "plugins").mkdir()
        (self.data / "secrets.json").write_bytes(b'{"fixture/provider_api_key":"test-only-secret"}')
        (self.data / "virtual-key").write_text("stable-test-only-key")
        (self.data / "requests.log").write_text("must-not-copy")
        self.config = {
            "version": 1,
            "server": {"listen": "127.0.0.1:8787", "auth": True},
            "data": {"dir": str(self.data), "metrics": True},
            "plugins": {"dir": str(self.stable / "plugins"), "agents": ["fixture"]},
            "upstreams": {"fixture": {"auth": {"slot": "provider_api_key", "store": True}}},
            "router": {"pools": {"tier_low": ["fixture-model"]}},
            "agent_routes": {"codex": {"profile": "work"}},
        }
        self.write_config()

    def write_config(self):
        (self.stable / "token-station.json").write_text(json.dumps(self.config))

    def test_copy_preserves_source_and_isolates_mutable_state(self):
        before = prepare.tree_digest(self.stable)
        prepare.copy_settings(self.stable, self.experiment)
        after = prepare.tree_digest(self.stable)
        self.assertEqual(before, after)
        copied = json.loads((self.experiment / "token-station.json").read_text())
        self.assertEqual(copied["server"], {"listen": "127.0.0.1:18787", "auth": True})
        self.assertEqual(copied["router"], self.config["router"])
        self.assertEqual(copied["agent_routes"], self.config["agent_routes"])
        self.assertEqual(copied["data"]["dir"], str(self.experiment / "token-station-data"))
        self.assertFalse(copied["data"]["request_body_capture"])
        self.assertEqual(copied["plugins"]["dir"], str(self.experiment / "plugins"))
        experimental_data = self.experiment / "token-station-data"
        self.assertEqual((experimental_data / "secrets.json").read_bytes(), (self.data / "secrets.json").read_bytes())
        self.assertFalse((experimental_data / "virtual-key").exists())
        self.assertFalse((experimental_data / "requests.log").exists())
        self.assertEqual((experimental_data / "secrets.json").stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.experiment.stat().st_mode & 0o777, 0o700)

    def test_file_credentials_are_copied_and_rewritten(self):
        credential = self.stable / "provider.key"
        credential.write_bytes(b"fixture-file-secret")
        self.config["upstreams"]["fixture"]["auth"] = {"slot": "key", "file": str(credential)}
        self.config["egress"] = {"auth": {"credential": {"slot": "proxy", "file": str(credential)}}}
        self.write_config()
        prepare.copy_settings(self.stable, self.experiment)
        copied = json.loads((self.experiment / "token-station.json").read_text())
        for auth in [copied["upstreams"]["fixture"]["auth"], copied["egress"]["auth"]["credential"]]:
            path = Path(auth["file"])
            self.assertTrue(path.is_relative_to(self.experiment))
            self.assertEqual(path.read_bytes(), credential.read_bytes())
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)

    def test_existing_experiment_is_never_overwritten(self):
        self.experiment.mkdir()
        path = self.experiment / "token-station.json"
        path.write_text("keep-existing")
        with self.assertRaises(ValueError):
            prepare.copy_settings(self.stable, self.experiment)
        self.assertEqual(path.read_text(), "keep-existing")

    def test_stable_destination_and_symlink_destination_are_rejected(self):
        with self.assertRaises(ValueError):
            prepare.copy_settings(self.stable, self.stable)
        self.experiment.symlink_to(self.stable, target_is_directory=True)
        with self.assertRaises(ValueError):
            prepare.copy_settings(self.stable, self.experiment)

    def test_source_symlinks_are_rejected_without_partial_config(self):
        secret = self.data / "secrets.json"
        secret.unlink()
        secret.symlink_to(self.data / "virtual-key")
        with self.assertRaises(ValueError):
            prepare.copy_settings(self.stable, self.experiment)
        self.assertFalse((self.experiment / "token-station.json").exists())

    def test_environment_credentials_require_explicit_reentry(self):
        self.config["upstreams"]["fixture"]["auth"] = {"slot": "key", "env": "SCX_TEST_KEY"}
        self.write_config()
        with self.assertRaises(ValueError):
            prepare.copy_settings(self.stable, self.experiment)
        self.assertFalse((self.experiment / "token-station.json").exists())

    def test_experimental_package_identity_is_explicit(self):
        overlay = json.loads((ROOT / "apps/desktop/src-tauri/tauri.scx.conf.json").read_text())
        self.assertEqual(overlay["identifier"], prepare.EXPERIMENT_ID)
        self.assertEqual(overlay["productName"], "Token Station SCX")
        self.assertEqual(overlay["app"]["windows"][0]["title"], "Token Station SCX — Experimental")
        self.assertFalse(overlay["bundle"]["createUpdaterArtifacts"])

    def test_installer_has_only_two_exact_targets(self):
        installer = ROOT / "scripts/install-local-desktop.sh"
        stable = subprocess.check_output(["bash", str(installer), "--print-target"], text=True)
        experiment = subprocess.check_output(["bash", str(installer), "--scx-experiment", "--print-target"], text=True)
        self.assertEqual(stable.splitlines(), ["/Applications/token-station.app", prepare.STABLE_ID])
        self.assertEqual(experiment.splitlines(), ["/Applications/Token Station SCX.app", prepare.EXPERIMENT_ID])
        for arguments in [["--target", "/Applications/token-station.app"], ["--copy-stable-settings"]]:
            result = subprocess.run(["bash", str(installer), *arguments], capture_output=True)
            self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()
