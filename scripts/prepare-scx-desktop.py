#!/usr/bin/env python3
"""Copy local settings into a new SCX App root without sharing writable state."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile

STABLE_ID = "com.tokenstation.desktop"
EXPERIMENT_ID = "com.tokenstation.desktop.scx"


def sha256_file(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def require_regular(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError("A source file is missing or uses a symbolic link.")
    for parent in path.parents:
        if parent.is_symlink():
            raise ValueError("A source path contains a symbolic link.")


def tree_digest(root):
    """Return content hashes without exposing file contents."""
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            result[str(path.relative_to(root))] = {"symlink": os.readlink(path)}
        elif path.is_file():
            result[str(path.relative_to(root))] = sha256_file(path)
    return result


def copy_settings(source, destination):
    source, destination = Path(source), Path(destination)
    if source.name != STABLE_ID or destination.name != EXPERIMENT_ID:
        raise ValueError("Use the exact stable and SCX application identifiers.")
    if source.parent.resolve() != destination.parent.resolve() or source.is_symlink() or destination.is_symlink():
        raise ValueError("Use separate application roots under the same real parent.")
    if destination.exists():
        raise ValueError("The SCX root already exists. Keep its settings or move it before copying.")
    config_path = source / "token-station.json"
    require_regular(config_path)
    raw = config_path.read_bytes()
    config = json.loads(raw)
    old_data = Path(config["data"]["dir"])
    old_plugins = Path(config["plugins"]["dir"])
    if not old_data.is_absolute() or not old_plugins.is_absolute():
        raise ValueError("Use absolute source data and plugin paths before copying.")
    credentials = [upstream.get("auth") for upstream in config.get("upstreams", {}).values()]
    credentials.append(config.get("egress", {}).get("auth", {}).get("credential"))
    for auth in filter(None, credentials):
        if auth.get("env"):
            raise ValueError("Environment credentials cannot be copied. Use local credential storage before copying.")
        if auth.get("file"):
            if not Path(auth["file"]).is_absolute():
                raise ValueError("Use absolute credential file paths before copying.")
            require_regular(Path(auth["file"]))
    store = old_data / "secrets.json"
    needs_store = any(auth and (auth.get("store") or auth.get("keyring")) for auth in credentials)
    if needs_store or store.exists() or store.is_symlink():
        require_regular(store)
    if old_plugins.exists():
        if old_plugins.is_symlink() or not old_plugins.is_dir():
            raise ValueError("The plugin source must be a real directory.")
        for path in old_plugins.rglob("*"):
            if path.is_symlink():
                raise ValueError("The plugin source contains a symbolic link.")
            if path.is_file():
                require_regular(path)
    stage = Path(tempfile.mkdtemp(prefix=".token-station-scx-settings.", dir=destination.parent))
    inputs = {config_path: hashlib.sha256(raw).hexdigest()}
    copied_credentials = 0

    def copy_private(source_file, target):
        require_regular(source_file)
        target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        shutil.copyfile(source_file, target)
        target.chmod(0o600)
        inputs[source_file] = sha256_file(target)

    try:
        data = stage / "token-station-data"
        data.mkdir(mode=0o700)
        (stage / "plugins").mkdir(mode=0o700)
        (stage / "agent-integration").mkdir(mode=0o700)
        if store.exists():
            copy_private(store, data / "secrets.json")
            copied_credentials += 1
        for index, auth in enumerate(filter(None, credentials)):
            if auth.get("file"):
                relative = Path("token-station-data/credentials") / f"credential-{index}.key"
                copy_private(Path(auth["file"]), stage / relative)
                auth["file"] = str(destination / relative)
                copied_credentials += 1
        if old_plugins.exists():
            for path in old_plugins.rglob("*"):
                relative = path.relative_to(old_plugins)
                if path.is_dir():
                    (stage / "plugins" / relative).mkdir(parents=True, exist_ok=True, mode=0o700)
                elif path.is_file():
                    copy_private(path, stage / "plugins" / relative)
        receipts = old_data / "plugin-receipts.json"
        if receipts.exists() or receipts.is_symlink():
            copy_private(receipts, data / "plugin-receipts.json")
        config["server"] = {"listen": "127.0.0.1:18787", "auth": True}
        config["data"]["dir"] = str(destination / "token-station-data")
        config["data"]["request_body_capture"] = False
        config["plugins"]["dir"] = str(destination / "plugins")
        # The new App starts Off even if future stable settings contain this field.
        if "semantic_routing" in config:
            config.pop("semantic_routing")
        target_config = stage / "token-station.json"
        target_config.write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n")
        target_config.chmod(0o600)
        for path, expected in inputs.items():
            require_regular(path)
            if sha256_file(path) != expected:
                raise ValueError("Source settings changed during copying. Run the copy again.")
        if destination.exists() or destination.is_symlink():
            raise ValueError("The SCX root appeared during copying. No settings were replaced.")
        stage.rename(destination)
        return {"copied": True, "providers": len(config.get("upstreams", {})), "credential_files": copied_credentials}
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--copy-stable-settings", action="store_true", required=True)
    args = parser.parse_args()
    if args.copy_stable_settings:
        support = Path.home() / "Library/Application Support"
        try:
            result = copy_settings(support / STABLE_ID, support / EXPERIMENT_ID)
        except (OSError, ValueError, KeyError) as error:
            # Do not print parser diagnostics that can contain credential values.
            if isinstance(error, json.JSONDecodeError):
                raise SystemExit("The stable configuration is not valid JSON.") from None
            if isinstance(error, (OSError, KeyError)):
                raise SystemExit("The settings copy failed. Check source files and private directory access.") from None
            raise SystemExit(str(error)) from None
        print(json.dumps(result))


if __name__ == "__main__":
    main()
