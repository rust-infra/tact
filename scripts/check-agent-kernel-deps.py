#!/usr/bin/env python3
"""Validate dependency direction for the provider-neutral agent kernel."""

from __future__ import annotations

import json
import sys
from collections import deque
from typing import Any

FORBIDDEN = {
    "tact_agent_api": {
        "tact",
        "tact-ui",
        "tact_llm",
        "tact_protocol",
        "tui",
        "sqlx",
        "rmcp",
        "git2",
        "reqwest",
        "ratatui",
        "cpal",
        "tera",
        "image",
        "async-openai",
        "async-openai-local",
    },
    "tact_agent_core": {
        "tact",
        "tact-ui",
        "tact_llm",
        "tact_protocol",
        "tui",
        "sqlx",
        "rmcp",
        "git2",
        "reqwest",
        "ratatui",
        "cpal",
        "tera",
        "image",
        "async-openai",
        "async-openai-local",
    },
    "tact_llm": {
        "tact",
        "tact_agent_core",
        "tact_protocol",
        "tact-ui",
        "tui",
    },
    "tact_agent_sdk": {
        "tact",
        "tact_llm",
        "tact_agent_sdk",
        "tact_protocol",
        "tact-ui",
        "tui",
        "agent_tui_kit",
        "sqlx",
        "rmcp",
        "git2",
        "reqwest",
        "ratatui",
        "crossterm",
        "ratatui-markdown",
        "syntect",
        "cpal",
        "tera",
        "image",
        "async-openai",
        "async-openai-local",
    },
}

ALLOWLIST = {
    "tact_agent_api": {
        "async-trait",
        "serde",
        "serde_json",
        "thiserror",
        "futures-core",
        "tokio-util",
    },
    "tact_agent_core": {
        "tact_agent_api",
        "async-trait",
        "futures-util",
        "serde",
        "serde_json",
        "thiserror",
        "tokio",
        "tokio-util",
        "tracing",
    },
    "tact_agent_sdk": {
        "tact_agent_api",
        "tact_agent_core",
        "async-trait",
        "futures-util",
        "serde_json",
        "thiserror",
        "uuid",
        "tracing",
    },
}


def package_graph(metadata: dict[str, Any]) -> tuple[dict[str, str], dict[str, list[str]]]:
    names = {
        package["id"]: package["name"]
        for package in metadata.get("packages", [])
    }
    graph = {}
    resolve = metadata.get("resolve") or {}
    for node in resolve.get("nodes", []):
        graph[node["id"]] = [
            dependency["pkg"]
            for dependency in node.get("deps", [])
            if dependency.get("pkg") in names
            and (
                not dependency.get("dep_kinds")
                or any(
                    dep_kind.get("kind") != "dev"
                    for dep_kind in dependency["dep_kinds"]
                )
            )
        ]
    return names, graph


def production_dependency_names(package: dict[str, Any]) -> set[str]:
    """Return direct dependencies that participate in non-dev builds."""
    return {
        dependency["name"]
        for dependency in package.get("dependencies", [])
        if dependency.get("kind") != "dev"
    }


def shortest_path(
    start: str, target_name: str, names: dict[str, str], graph: dict[str, list[str]]
) -> list[str] | None:
    queue = deque([(start, [start])])
    visited = {start}
    while queue:
        current, path = queue.popleft()
        for dependency in graph.get(current, []):
            next_path = [*path, dependency]
            if names[dependency] == target_name:
                return [names[item] for item in next_path]
            if dependency not in visited:
                visited.add(dependency)
                queue.append((dependency, next_path))
    return None


def validate(metadata: dict[str, Any], requested: list[str], *, emit: bool = True) -> bool:
    packages = {package["name"]: package for package in metadata.get("packages", [])}
    names, graph = package_graph(metadata)
    failed = False

    for name in requested:
        package_failed = False
        package = packages.get(name)
        if package is None:
            if emit:
                print(f"error: workspace package {name!r} was not found", file=sys.stderr)
            failed = True
            continue

        direct = production_dependency_names(package)
        forbidden = FORBIDDEN.get(name, set())
        direct_violations = sorted(direct & forbidden)
        for violation in direct_violations:
            if emit:
                print(
                    f"error: {name} has forbidden direct dependency: {violation}",
                    file=sys.stderr,
                )
            failed = True
            package_failed = True

        allowed = ALLOWLIST.get(name)
        if allowed is not None:
            unexpected = sorted(direct - allowed)
            for dependency in unexpected:
                if emit:
                    print(
                        f"error: {name} has dependency outside its allowlist: {dependency}",
                        file=sys.stderr,
                    )
            failed = failed or bool(unexpected)
            package_failed = package_failed or bool(unexpected)

        for target in sorted(forbidden - set(direct_violations)):
            path = shortest_path(package["id"], target, names, graph)
            if path is not None:
                if emit:
                    print(
                        f"error: {name} has forbidden transitive dependency: {' -> '.join(path)}",
                        file=sys.stderr,
                    )
                failed = True
                package_failed = True

        if emit and not package_failed:
            print(f"ok: {name} dependency direction")

    return not failed


def self_test() -> bool:
    metadata = {
        "packages": [
            {
                "id": "root",
                "name": "tact_agent_api",
                "dependencies": [{"name": "bridge"}],
            },
            {
                "id": "bridge",
                "name": "bridge",
                "dependencies": [{"name": "tact_protocol"}],
            },
            {
                "id": "protocol",
                "name": "tact_protocol",
                "dependencies": [],
            },
        ],
        "resolve": {
            "nodes": [
                {"id": "root", "deps": [{"pkg": "bridge"}]},
                {"id": "bridge", "deps": [{"pkg": "protocol"}]},
                {"id": "protocol", "deps": []},
            ]
        },
    }
    names, graph = package_graph(metadata)
    path = shortest_path("root", "tact_protocol", names, graph)
    expected = ["tact_agent_api", "bridge", "tact_protocol"]
    if path != expected or validate(metadata, ["tact_agent_api"], emit=False):
        print(
            "error: forbidden-transitive self-test accepted the negative fixture",
            file=sys.stderr,
        )
        return False
    print("ok: forbidden-transitive self-test")

    dev_only_metadata = {
        "packages": [
            {
                "id": "root",
                "name": "tact_agent_api",
                "dependencies": [
                    {"name": "tact_protocol", "kind": "dev"},
                ],
            },
            {
                "id": "protocol",
                "name": "tact_protocol",
                "dependencies": [],
            },
        ],
        "resolve": {
            "nodes": [
                {
                    "id": "root",
                    "deps": [
                        {
                            "pkg": "protocol",
                            "dep_kinds": [{"kind": "dev", "target": None}],
                        }
                    ],
                },
                {"id": "protocol", "deps": []},
            ]
        },
    }
    if not validate(dev_only_metadata, ["tact_agent_api"], emit=False):
        print(
            "error: dev-only dependency self-test rejected a valid fixture",
            file=sys.stderr,
        )
        return False
    print("ok: dev-only dependency self-test")

    sdk_llm_metadata = {
        "packages": [
            {
                "id": "sdk",
                "name": "tact_agent_sdk",
                "dependencies": [{"name": "tact_llm"}],
            },
            {
                "id": "llm",
                "name": "tact_llm",
                "dependencies": [],
            },
        ],
        "resolve": {
            "nodes": [
                {"id": "sdk", "deps": [{"pkg": "llm"}]},
                {"id": "llm", "deps": []},
            ]
        },
    }
    if validate(sdk_llm_metadata, ["tact_agent_sdk"], emit=False):
        print(
            "error: SDK self-test accepted a direct tact_llm dependency",
            file=sys.stderr,
        )
        return False
    print("ok: SDK tact_llm self-test")

    sdk_transitive_llm_metadata = {
        "packages": [
            {
                "id": "sdk",
                "name": "tact_agent_sdk",
                "dependencies": [{"name": "tact_agent_api"}],
            },
            {
                "id": "api",
                "name": "tact_agent_api",
                "dependencies": [{"name": "bridge"}],
            },
            {
                "id": "bridge",
                "name": "bridge",
                "dependencies": [{"name": "tact_llm"}],
            },
            {
                "id": "llm",
                "name": "tact_llm",
                "dependencies": [],
            },
        ],
        "resolve": {
            "nodes": [
                {"id": "sdk", "deps": [{"pkg": "api"}]},
                {"id": "api", "deps": [{"pkg": "bridge"}]},
                {"id": "bridge", "deps": [{"pkg": "llm"}]},
                {"id": "llm", "deps": []},
            ]
        },
    }
    if validate(sdk_transitive_llm_metadata, ["tact_agent_sdk"], emit=False):
        print(
            "error: SDK self-test accepted a transitive tact_llm dependency",
            file=sys.stderr,
        )
        return False
    print("ok: SDK transitive tact_llm self-test")

    sdk_transitive_ui_metadata = {
        "packages": [
            {
                "id": "sdk",
                "name": "tact_agent_sdk",
                "dependencies": [{"name": "tact_agent_api"}],
            },
            {
                "id": "api",
                "name": "tact_agent_api",
                "dependencies": [{"name": "bridge"}],
            },
            {
                "id": "bridge",
                "name": "bridge",
                "dependencies": [{"name": "agent_tui_kit"}],
            },
            {
                "id": "ui",
                "name": "agent_tui_kit",
                "dependencies": [],
            },
        ],
        "resolve": {
            "nodes": [
                {"id": "sdk", "deps": [{"pkg": "api"}]},
                {"id": "api", "deps": [{"pkg": "bridge"}]},
                {"id": "bridge", "deps": [{"pkg": "ui"}]},
                {"id": "ui", "deps": []},
            ]
        },
    }
    if validate(sdk_transitive_ui_metadata, ["tact_agent_sdk"], emit=False):
        print(
            "error: SDK self-test accepted a transitive UI dependency",
            file=sys.stderr,
        )
        return False
    print("ok: SDK transitive UI self-test")
    return True


def main() -> int:
    if sys.argv[1:] == ["--self-test"]:
        return 0 if self_test() else 1

    requested = sys.argv[1:] or ["tact_agent_api", "tact_agent_core"]
    try:
        metadata = json.load(sys.stdin)
    except json.JSONDecodeError as error:
        print(f"error: invalid cargo metadata JSON: {error}", file=sys.stderr)
        return 1

    return 0 if validate(metadata, requested) and self_test() else 1


if __name__ == "__main__":
    raise SystemExit(main())
