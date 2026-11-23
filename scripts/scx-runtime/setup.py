"""Prepare the private SCX runtime from pinned public assets or verified local seeds."""

import argparse
import base64
import csv
import fcntl
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parent


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def verify_files(directory, files):
    for name, expected in files.items():
        if Path(name).name != name:
            raise ValueError("Invalid model asset name.")
        path = directory / name
        if path.is_symlink() or path.stat().st_size != expected["bytes"]:
            raise ValueError("Model asset size does not match.")
        if digest(path) != expected["sha256"]:
            raise ValueError("Model asset checksum does not match.")


def find_uv():
    for candidate in [shutil.which("uv"), Path.home() / ".local/bin/uv",
                      Path.home() / ".cargo/bin/uv", Path("/opt/homebrew/bin/uv")]:
        if candidate and Path(candidate).is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    raise ValueError("Install uv, then select Prepare again. See https://docs.astral.sh/uv/getting-started/installation/.")


def execute(arguments):
    result = subprocess.run(arguments, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if result.returncode:
        # Tool errors can include environment paths. Return a stable action.
        raise ValueError("Runtime dependency preparation failed. Check uv and network access, then retry.")


def normalized(name):
    return name.lower().replace("_", "-").replace(".", "-")


def clear_stage(path):
    if path.is_symlink():
        raise ValueError("A preparation stage must not be a symbolic link.")
    if path.exists():
        if not path.is_dir():
            raise ValueError("A preparation stage must be a directory.")
        shutil.rmtree(path)


def verify_seed(seed, assets):
    site = seed / "lib/python3.11/site-packages"
    distributions = {normalized(d.metadata["Name"]): d
                     for d in importlib.metadata.distributions(path=[str(site)])}
    copied = set()
    for expected in assets["seed_packages"]:
        distribution = distributions.get(expected["name"])
        if distribution is None or distribution.version != expected["version"]:
            raise ValueError("Seed package versions do not match the pinned runtime.")
        record = Path(distribution._path) / "RECORD"
        if digest(record) != expected["record_sha256"]:
            raise ValueError("Seed package manifest does not match the verified runtime.")
        for relative, checksum, size in csv.reader(record.open(newline="")):
            path = (site / relative).resolve()
            try:
                within = path.relative_to(site.resolve())
            except ValueError:
                # Console scripts are not used. The new venv supplies Python.
                continue
            if "__pycache__" in within.parts or path.suffix == ".pyc":
                continue
            if not checksum:
                if path != record.resolve():
                    continue
            else:
                algorithm, encoded = checksum.split("=", 1)
                if algorithm != "sha256" or base64.urlsafe_b64encode(bytes.fromhex(digest(path))).decode().rstrip("=") != encoded:
                    raise ValueError("Seed package content verification failed.")
                if size and path.stat().st_size != int(size):
                    raise ValueError("Seed package size verification failed.")
            copied.add(within)
    return site, copied


def prepare_python(runtime, seed, assets, uv):
    target = runtime / ".venv"
    if (target / "bin/python").exists():
        execute([str(target / "bin/python"), "-I", "-c",
                 "import importlib.metadata,json,sys; a=json.load(open(sys.argv[1])); "
                 "assert all(importlib.metadata.version(p['name'])==p['version'] for p in a['seed_packages'])",
                 str(ROOT / "assets.json")])
        return
    stage = runtime / ".venv-preparing"
    # The caller holds the preparation flock. A previous cancelled attempt can
    # leave only this known stage, which is safe to rebuild on the next try.
    clear_stage(stage)
    site = copied = None
    if seed:
        site, copied = verify_seed(seed, assets)
        python = str((seed / "bin/python").resolve())
    else:
        python = assets["python_version"]
    try:
        execute([uv, "venv", "--relocatable", "--python", python, str(stage)])
        if seed:
            destination = stage / "lib/python3.11/site-packages"
            for relative in sorted(copied):
                output = destination / relative
                output.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(site / relative, output)
        else:
            execute([uv, "pip", "install", "--python", str(stage / "bin/python"),
                     "--require-hashes", "--only-binary", ":all:",
                     "--requirements", str(ROOT / "requirements-lock.txt")])
        execute([str(stage / "bin/python"), "-I", "-c",
                 "import sys; assert sys.version_info[:3] == (3,11,15); import torch,gliclass,transformers"])
        stage.rename(target)
    except Exception:
        if stage.exists():
            shutil.rmtree(stage)
        raise


def download_file(url, target, expected):
    # Bounded ranges also work through download proxies with response-size limits.
    total = expected["bytes"]
    with target.open("wb") as output:
        offset = 0
        while offset < total:
            end = min(total - 1, offset + 8 * 1024 * 1024 - 1)
            request = urllib.request.Request(url, headers={"Range": f"bytes={offset}-{end}", "User-Agent": "Token-Station-SCX-Setup/1"})
            with urllib.request.urlopen(request, timeout=60) as response:
                if response.status == 200 and offset == 0:
                    remaining = total
                elif response.status == 206 and response.headers.get("Content-Range", "").startswith(f"bytes {offset}-"):
                    remaining = end - offset + 1
                else:
                    raise ValueError("The model server returned an invalid range.")
                while remaining:
                    block = response.read(min(1024 * 1024, remaining))
                    if not block:
                        raise ValueError("The model download ended early. Retry preparation.")
                    output.write(block)
                    offset += len(block)
                    remaining -= len(block)
    if target.stat().st_size != total or digest(target) != expected["sha256"]:
        raise ValueError("Downloaded model verification failed.")


def prepare_models(runtime, seed, assets):
    target = runtime / "models/scx"
    try:
        verify_files(target, assets["files"])
        return
    except (OSError, ValueError):
        pass
    if seed:
        verify_files(seed, assets["files"])
    target.mkdir(parents=True, exist_ok=True)
    stage = runtime / ".model-preparing"
    clear_stage(stage)
    stage.mkdir(mode=0o700)
    try:
        for name, expected in assets["files"].items():
            output = stage / name
            if seed:
                shutil.copy2(seed / name, output)
            else:
                url = f"https://huggingface.co/{assets['repo_id']}/resolve/{assets['revision']}/{name}"
                download_file(url, output, expected)
        verify_files(stage, assets["files"])
        for name in assets["files"]:
            os.replace(stage / name, target / name)
    finally:
        clear_stage(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime-dir", type=Path, required=True)
    parser.add_argument("--seed-python-env", type=Path)
    parser.add_argument("--seed-model", type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        if sys.platform != "darwin" or platform.machine() != "arm64":
            raise ValueError("Local SCX routing requires macOS on Apple Silicon.")
        runtime = args.runtime_dir.expanduser().resolve()
        if runtime == Path.home() or runtime == Path("/"):
            raise ValueError("Select a private semantic-runtime directory.")
        runtime.mkdir(parents=True, exist_ok=True, mode=0o700)
        runtime.chmod(0o700)
        assets = json.loads((ROOT / "assets.json").read_text())
        if digest(ROOT / "requirements-lock.txt") != assets["requirements_sha256"]:
            raise ValueError("The pinned dependency file does not match.")
        with (runtime / ".prepare.lock").open("w") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise ValueError("Runtime preparation is already running.") from None
            uv = find_uv()
            prepare_python(runtime, args.seed_python_env, assets, uv)
            prepare_models(runtime, args.seed_model, assets)
            for name in ["setup.py", "worker.py", "assets.json", "requirements-lock.txt"]:
                source = ROOT / name
                destination = runtime / name
                if source.resolve() != destination.resolve():
                    shutil.copy2(source, destination)
            (runtime / "prepared.json").write_text(json.dumps({
                "schema_version": 1, "model_revision": assets["revision"],
                "requirements_sha256": assets["requirements_sha256"],
                "worker_sha256": digest(runtime / "worker.py"),
            }, indent=2) + "\n")
        print(json.dumps({"status": "ready"}), flush=True)
        return 0
    except Exception as error:
        message = str(error) if isinstance(error, ValueError) else "Runtime preparation failed. Check disk space, permissions, and network access."
        print(json.dumps({"status": "error", "message": message[:240]}), flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
