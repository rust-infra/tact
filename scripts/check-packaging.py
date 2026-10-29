#!/usr/bin/env python3
"""Guard the crate manifests that third-party hosts consume."""

from __future__ import annotations

import pathlib
import sys
import tomllib


ROOT = pathlib.Path(__file__).resolve().parent.parent
PUBLIC_CRATES = {
    "tact_agent_api",
    "tact_agent_core",
    "tact_agent_sdk",
    "tact_llm",
}


def load(path: pathlib.Path) -> dict:
    with path.open("rb") as manifest:
        return tomllib.load(manifest)


def main() -> int:
    errors: list[str] = []
    workspace = load(ROOT / "Cargo.toml")
    if not workspace.get("workspace", {}).get("package", {}).get("rust-version"):
        errors.append("Cargo.toml: [workspace.package].rust-version is missing")

    manifests = sorted((ROOT / "crates").glob("*/Cargo.toml"))
    for path in manifests:
        manifest = load(path)
        package = manifest.get("package", {})
        name = package.get("name")
        if name in PUBLIC_CRATES:
            for field in ("description", "repository", "rust-version"):
                if not package.get(field):
                    errors.append(f"{path.relative_to(ROOT)}: package.{field} is missing")

        for table in ("dependencies", "dev-dependencies", "build-dependencies"):
            for dependency, spec in manifest.get(table, {}).items():
                if not isinstance(spec, dict):
                    continue
                if "path" in spec and "version" not in spec:
                    errors.append(
                        f"{path.relative_to(ROOT)}: {table}.{dependency} has no version"
                    )
                if name in PUBLIC_CRATES and "git" in spec:
                    errors.append(
                        f"{path.relative_to(ROOT)}: {table}.{dependency} is a git dependency"
                    )

    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("Packaging manifests passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
