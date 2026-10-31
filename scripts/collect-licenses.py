#!/usr/bin/env python3
"""Collect license texts for the runtime dependency graph of one target."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
from collections import defaultdict
from pathlib import Path


LICENSE_NAMES = ("license", "copying", "notice", "unlicense")
COPYRIGHT = re.compile(r"copyright", re.IGNORECASE)


def metadata(target: str) -> dict:
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--filter-platform",
            target,
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(result.stdout)


def license_files(manifest: Path, declared: str | None) -> list[Path]:
    root = manifest.parent
    candidates: list[Path] = []
    if declared:
        value = Path(declared)
        candidates.append(value if value.is_absolute() else root / value)
    for current, dirs, files in os.walk(root):
        depth = len(Path(current).relative_to(root).parts)
        if depth >= 2:
            dirs[:] = []
        for name in files:
            lowered = name.lower()
            if any(marker in lowered for marker in LICENSE_NAMES):
                candidates.append(Path(current) / name)
    unique: dict[Path, None] = {}
    for path in candidates:
        if path.is_file() and path.stat().st_size <= 1_000_000:
            unique[path] = None
    return sorted(unique)


def copyright_notices(root: Path) -> list[str]:
    notices: list[str] = []
    seen: set[str] = set()
    files = 0
    for current, dirs, names in os.walk(root):
        depth = len(Path(current).relative_to(root).parts)
        if depth >= 3:
            dirs[:] = []
        for name in sorted(names):
            if not name.endswith((".rs", ".toml", ".md", ".txt")):
                continue
            files += 1
            if files > 160:
                return notices
            path = Path(current) / name
            try:
                lines = path.read_text(encoding="utf-8", errors="ignore").splitlines()
            except OSError:
                continue
            for line in lines[:40]:
                if COPYRIGHT.search(line):
                    notice = line.strip()
                    if notice and notice not in seen:
                        seen.add(notice)
                        notices.append(notice)
                    if len(notices) >= 8:
                        return notices
    return notices


def canonical_license_paths(expression: str) -> list[Path]:
    root = Path(__file__).resolve().parent.parent
    lowered = expression.lower()
    paths = []
    if "mit" in lowered:
        paths.append(root / "licenses/MIT.txt")
    if "apache" in lowered:
        paths.append(root / "THIRD-PARTY-APACHE-2.0.txt")
    if "mpl" in lowered:
        paths.append(root / "licenses/MPL-2.0.txt")
    if "zlib" in lowered:
        paths.append(root / "licenses/Zlib.txt")
    return paths


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", default="THIRD-PARTY-LICENSES.txt")
    args = parser.parse_args()

    data = metadata(args.target)
    workspace_ids = set(data.get("workspace_members", []))
    packages = [
        package
        for package in data["packages"]
        if package["id"] not in workspace_ids and package.get("source")
    ]
    packages.sort(key=lambda package: (package["name"].lower(), package["version"]))

    texts: dict[str, dict] = {}
    package_records: list[str] = []
    for package in packages:
        manifest = Path(package["manifest_path"])
        files = license_files(manifest, package.get("license_file"))
        package_records.append(
            "- {name} {version} | {license} | {repository}".format(
                name=package["name"],
                version=package["version"],
                license=package.get("license") or package.get("license_file") or "NOASSERTION",
                repository=package.get("repository") or package.get("homepage") or "unknown",
            )
        )
        for path in files:
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
            entry = texts.setdefault(
                digest,
                {
                    "file": path.name,
                    "text": text.rstrip(),
                    "packages": set(),
                },
            )
            entry["packages"].add(f'{package["name"]} {package["version"]}')
        if not files:
            notices = copyright_notices(manifest.parent)
            synthetic = [
                f'Package: {package["name"]} {package["version"]}',
                f'Declared license: {package.get("license") or "NOASSERTION"}',
                f'Repository: {package.get("repository") or package.get("homepage") or "unknown"}',
                "",
                "Copyright notices found in the package source:",
                *(notices or ["(no explicit copyright line was present in the package source)"]),
            ]
            text = "\n".join(synthetic)
            digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
            texts[digest] = {
                "file": f'{package["name"]}-{package["version"]}-NOTICE',
                "text": text,
                "packages": {f'{package["name"]} {package["version"]}'},
            }
            for canonical in canonical_license_paths(package.get("license") or ""):
                if not canonical.is_file():
                    continue
                canonical_text = canonical.read_text(encoding="utf-8").rstrip()
                canonical_digest = hashlib.sha256(
                    canonical_text.encode("utf-8")
                ).hexdigest()
                entry = texts.setdefault(
                    canonical_digest,
                    {
                        "file": canonical.name,
                        "text": canonical_text,
                        "packages": set(),
                    },
                )
                entry["packages"].add(f'{package["name"]} {package["version"]}')

    output = [
        "# Third-Party License Texts",
        "",
        f"Target: {args.target}",
        f"Packages: {len(packages)}",
        "",
        "This bundle is generated from the Cargo registry sources used by the",
        "release build. License texts shared by several packages appear once,",
        "followed by every package that supplied the same text.",
        "",
        "## Packages",
        "",
        *package_records,
        "",
        "## License texts",
        "",
    ]
    for digest in sorted(texts, key=lambda key: texts[key]["file"].lower()):
        entry = texts[digest]
        output.extend(
            [
                f"### {entry['file']}",
                "",
                "Packages: " + ", ".join(sorted(entry["packages"])),
                "",
                "```text",
                entry["text"],
                "```",
                "",
            ]
        )
    Path(args.output).write_text("\n".join(output), encoding="utf-8")


if __name__ == "__main__":
    main()
