#!/usr/bin/env python3
"""Verify provider fixture digests and their explicit source provenance.

`official_package_set` already checks that a declared directory exists, is
non-empty, and pairs every `.input.json` with an `.expected.json`. All of that
holds for a pack whose contents have quietly diverged from South — renaming
nothing and changing every byte passes it. Vendored conformance fixtures claim
agreement with South's cases. A modified copy must not retain that provenance
claim while silently changing the expected behavior.

Vendored packs default to source=south. Packs authored in this repository must
explicitly declare source=repository in the provenance record. Both sources
have their file count and content digest checked. Only South packs are compared
against a South checkout. Repository packs do not claim upstream provenance.

    check-provider-fixtures.py verify            compare packs against the recorded digest
    check-provider-fixtures.py verify --against-south <checkout>
                                                 also compare against a real South tree
    check-provider-fixtures.py record <checkout> rewrite the provenance file

`record` is deliberately a separate verb: updating the recorded digest must be
a decision someone makes, not something a verification run does for them.
"""

import argparse
import hashlib
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
PROVENANCE = ROOT / "plugins/official/fixture-provenance.json"
PACKAGES = ROOT / "plugins/official/packages.json"


def declared_fixture_dir(package_dir: pathlib.Path) -> str | None:
    manifest = json.loads((package_dir / "manifest.json").read_text())
    declared = (manifest.get("conformance") or {}).get("fixtures") or ""
    return declared.rstrip("/") or None


def tree_digest(directory: pathlib.Path) -> tuple[str, int]:
    """A digest over relative paths and file bytes, in sorted order.

    Paths are part of the hash: a fixture that keeps its bytes but changes its
    name is a different case, and renaming one onto another's name would
    otherwise be invisible.
    """
    digest = hashlib.sha256()
    count = 0
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        digest.update(str(path.relative_to(directory)).encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
        count += 1
    return digest.hexdigest(), count


def packs() -> list[tuple[str, pathlib.Path, str]]:
    """Only the South components.

    The agent adapters declare fixtures too, but those are authored in this
    repository. Provider components can also have repository-authored fixtures,
    but must explicitly declare that source in their provenance record.
    """
    found = []
    for package in json.loads(PACKAGES.read_text())["packages"]:
        if package.get("kind") != "south-component":
            continue
        package_dir = ROOT / "plugins/official" / package["dir"]
        declared = declared_fixture_dir(package_dir)
        if declared:
            found.append((package["dir"], package_dir / declared, declared))
    return found


def cmd_record(checkout: pathlib.Path) -> int:
    revision = checkout.name
    prior = json.loads(PROVENANCE.read_text()) if PROVENANCE.exists() else {"packages": {}}
    entries = {}
    for name, directory, declared in packs():
        previous = prior["packages"].get(name, {})
        source = previous.get("source", "south")
        if source not in ("south", "repository"):
            print(f"{name}: unsupported fixture source {source!r}", file=sys.stderr)
            return 1
        if not directory.is_dir():
            print(f"{name}: declares {declared}, which does not exist", file=sys.stderr)
            return 1
        digest, count = tree_digest(directory)
        entries[name] = {"fixtures": declared, "files": count, "sha256": digest}
        if "source" in previous:
            entries[name]["source"] = source
    PROVENANCE.write_text(
        json.dumps(
            {
                "comment": (
                    "Provider fixture provenance. Entries without source are vendored "
                    "from South. source=repository identifies locally authored fixtures. "
                    "Use scripts/check-provider-fixtures.py record <south-checkout> after "
                    "reviewing changes. Change South first for vendored fixtures."
                ),
                "south_revision": revision,
                "packages": entries,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )
    local_count = sum(entry.get("source", "south") == "repository" for entry in entries.values())
    print(f"recorded {len(entries) - local_count} South packs at {revision} and {local_count} repository packs")
    return 0


def cmd_verify(against_south: pathlib.Path | None) -> int:
    if not PROVENANCE.exists():
        print(f"missing {PROVENANCE.relative_to(ROOT)}; run `record` first", file=sys.stderr)
        return 1
    recorded = json.loads(PROVENANCE.read_text())
    failures = []
    declared_packs = packs()

    for name, directory, declared in declared_packs:
        entry = recorded["packages"].get(name)
        if entry is None:
            failures.append(f"{name}: declares {declared} but has no recorded provenance")
            continue
        source = entry.get("source", "south")
        if source not in ("south", "repository"):
            failures.append(f"{name}: unsupported fixture source {source!r}")
            continue
        if not directory.is_dir():
            failures.append(f"{name}: declares {declared}, which does not exist")
            continue
        digest, count = tree_digest(directory)
        if entry["fixtures"] != declared:
            failures.append(
                f"{name}: manifest declares {declared}, provenance records {entry['fixtures']}"
            )
        if count != entry["files"]:
            failures.append(f"{name}: fixture file count is {count}, provenance records {entry['files']}")
        if digest != entry["sha256"]:
            origin = f"South {recorded['south_revision']}" if source == "south" else "the repository record"
            failures.append(
                f"{name}: fixture content has drifted from {origin} ({count} files here, "
                f"{entry['files']} recorded)"
            )

    for name in recorded["packages"]:
        if not any(name == pack for pack, _, _ in declared_packs):
            failures.append(f"{name}: has recorded provenance but declares no fixtures")

    if against_south is not None:
        source = against_south / "crates/south-component-conformance"
        if not source.is_dir():
            failures.append(f"{source} is not a south-component-conformance checkout")
        else:
            for name, directory, declared in declared_packs:
                entry = recorded["packages"].get(name)
                if entry is None or entry.get("source", "south") != "south":
                    continue
                upstream = source / declared
                if not upstream.is_dir():
                    failures.append(f"{name}: South has no {declared} at this revision")
                    continue
                here, _ = tree_digest(directory)
                there, _ = tree_digest(upstream)
                if here != there:
                    failures.append(f"{name}: differs from South's own {declared}")

    if failures:
        print("provider fixture provenance:", file=sys.stderr)
        for failure in failures:
            print(f"  {failure}", file=sys.stderr)
        print(
            "\nChange vendored fixtures in South first. Re-pin and re-vendor before `record`. "
            "Review repository-authored fixture changes locally before `record`.",
            file=sys.stderr,
        )
        return 1

    local_count = sum(entry.get("source", "south") == "repository" for entry in recorded["packages"].values())
    print(f"provider fixtures: {len(recorded['packages']) - local_count} packs match South {recorded['south_revision']}; {local_count} repository packs match their recorded digests")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    verify = sub.add_parser("verify")
    verify.add_argument("--against-south", type=pathlib.Path, default=None)
    record = sub.add_parser("record")
    record.add_argument("checkout", type=pathlib.Path)
    args = parser.parse_args()
    if args.command == "record":
        return cmd_record(args.checkout)
    return cmd_verify(args.against_south)


if __name__ == "__main__":
    sys.exit(main())
