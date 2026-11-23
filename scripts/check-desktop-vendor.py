#!/usr/bin/env python3
"""Reconstruct adapted dependencies from checksum-pinned upstream archives."""

import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import tomllib
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / "apps/desktop/src-tauri/vendor"
REGISTRY = "https://static.crates.io/crates/"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def tree(directory):
    result = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Unexpected dependency symlink: {path}")
        if path.is_file():
            result[path.relative_to(directory).as_posix()] = digest(path.read_bytes())
    return result


def verify(record, cache):
    name, version = record["name"], record["version"]
    if not name.replace("-", "").replace("_", "").isalnum():
        raise ValueError(f"Invalid dependency name: {name}")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError(f"Invalid published dependency version: {version}")
    archive_name = f"{name}-{version}.crate"
    url = f"{REGISTRY}{name}/{archive_name}"
    if record["archive_url"] != url:
        raise ValueError(f"Unexpected upstream URL for {name}")
    archive = cache / archive_name if cache else None
    data = archive.read_bytes() if archive and archive.exists() else urllib.request.urlopen(url, timeout=60).read()
    if digest(data) != record["archive_sha256"]:
        raise ValueError(f"Upstream archive checksum mismatch: {name}")

    with tempfile.TemporaryDirectory(prefix="ts-vendor-check-") as scratch:
        scratch = Path(scratch)
        archive_path = scratch / archive_name
        archive_path.write_bytes(data)
        prefix = f"{name}-{version}"
        with tarfile.open(archive_path, "r:gz") as source:
            for member in source.getmembers():
                parts = Path(member.name).parts
                if not parts or parts[0] != prefix or ".." in parts or member.issym() or member.islnk():
                    raise ValueError(f"Unsafe archive member: {member.name}")
            source.extractall(scratch, filter="data")
        restored = scratch / prefix
        if tree(restored) != record["original_files"]:
            raise ValueError(f"Upstream source inventory mismatch: {name}")
        original = tomllib.loads((restored / "Cargo.toml").read_text())
        if original["package"]["name"] != name or original["package"]["version"] != version:
            raise ValueError(f"Upstream package identity mismatch: {name}")

        # Dependency packages do not use their own lockfiles. Keep only the
        # desktop lockfile, so copied upstream locks cannot mask the selected graph.
        (restored / "Cargo.lock").unlink(missing_ok=True)
        patch = VENDOR / "patches" / f"{name}.patch"
        if digest(patch.read_bytes()) != record["patch_sha256"]:
            raise ValueError(f"Adaptation patch checksum mismatch: {name}")
        subprocess.run(["git", "apply", f"--directory={prefix}", str(patch)], cwd=scratch, check=True)
        if tree(restored) != tree(VENDOR / name):
            raise ValueError(f"Adapted source differs from upstream plus patch: {name}")
        for path in restored.rglob("*"):
            if path.is_file():
                local = VENDOR / name / path.relative_to(restored)
                if bool(path.stat().st_mode & 0o111) != bool(local.stat().st_mode & 0o111):
                    raise ValueError(f"Adaptation changed executable permissions: {name}/{path.relative_to(restored)}")
        adapted = tomllib.loads((restored / "Cargo.toml").read_text())
        if adapted["package"]["name"] != name or adapted["package"]["version"] != version:
            raise ValueError(f"Adaptation changed package identity: {name}")
    return name


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive-cache", type=Path, help="Directory containing checksum-verified upstream .crate files")
    args = parser.parse_args()
    provenance = json.loads((VENDOR / "provenance.json").read_text())
    if provenance["schema_version"] != 1:
        raise ValueError("Unsupported desktop dependency provenance schema")
    records = provenance["crates"]
    names = [record["name"] for record in records]
    if len(set(names)) != len(names):
        raise ValueError("Duplicate adapted dependency")
    manifest = tomllib.loads((VENDOR.parent / "Cargo.toml").read_text())
    patches = manifest["patch"]["crates-io"]
    if set(patches) != set(names):
        raise ValueError("Desktop patches and provenance inventory differ")
    for name, spec in patches.items():
        if spec != {"path": f"vendor/{name}"}:
            raise ValueError(f"Unexpected desktop patch source: {name}")
    directories = {path.name for path in VENDOR.iterdir() if path.is_dir() and path.name != "patches"}
    if directories != set(names):
        raise ValueError("Unexpected or missing adapted dependency directory")
    patch_files = {path.name for path in (VENDOR / "patches").iterdir()}
    if patch_files != {f"{name}.patch" for name in names}:
        raise ValueError("Unexpected or missing adaptation patch")
    with ThreadPoolExecutor(max_workers=4) as pool:
        checked = list(pool.map(lambda record: verify(record, args.archive_cache), records))
    print(f"Desktop dependency provenance: PASS ({len(checked)} authentic upstream archives plus exact source patches)")


if __name__ == "__main__":
    main()
