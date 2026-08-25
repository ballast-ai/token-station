#!/usr/bin/env python3
"""Read one field from the ordered official package set."""

import argparse
import json
import sys
from pathlib import Path


FIELDS = ("dir", "id", "kind", "wasm")
KINDS = ("agent", "south-component")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--kind", choices=KINDS)
    parser.add_argument("--field", choices=FIELDS, required=True)
    args = parser.parse_args()

    package_file = Path(__file__).resolve().parent.parent / "plugins/official/packages.json"
    document = json.loads(package_file.read_text(encoding="utf-8"))
    packages = document.get("packages")
    if not isinstance(packages, list):
        raise SystemExit("official package set must contain a packages array")

    for package in packages:
        if not isinstance(package, dict):
            raise SystemExit("every official package must be an object")
        missing = [field for field in FIELDS if not isinstance(package.get(field), str)]
        if missing:
            raise SystemExit(f"official package has invalid fields: {', '.join(missing)}")
        if package["kind"] not in KINDS:
            raise SystemExit(f"unsupported official package kind: {package['kind']}")
        if args.kind is None or package["kind"] == args.kind:
            # `sys.stdout.write` with an explicit "\n", not `print`: on Windows
            # Python's text mode translates "\n" to "\r\n", and the shell
            # readers that consume this use `IFS= read -r`, which strips the
            # newline and keeps the carriage return. The package name then ends
            # in "\r" and the path built from it breaks in the middle —
            # `plugins/official/agent-openai\r/Cargo.toml`, which reports as a
            # manifest that does not exist. Six readers across three build
            # scripts consume this, so the fix belongs at the source.
            sys.stdout.write(package[args.field] + "\n")


if __name__ == "__main__":
    main()
