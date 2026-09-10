# Codex Local Marketplace Discovery

Date: 2026-09-10
Status: approved / implemented

## Problem

Tact standardized plugin manifests, hooks, MCP, and skills on the Codex layout,
but marketplace discovery still exposed only the hard-coded Claude official Git
marketplace (`claude-plugins-official`). A Codex user's marketplace at
`~/.agents/plugins/marketplace.json` was invisible to `tact-ui plugin
marketplace list`, and a bare `tact-ui plugin install <name>` still defaulted to
the Claude namespace.

Codex marketplace catalogs also use a local source shape:

```json
{
  "source": {
    "source": "local",
    "path": "./plugins/demo"
  }
}
```

Tact's catalog parser expected repository-relative strings, `git-subdir`, or
`url`, so Codex local plugins could not be resolved even if the catalog was
found.

## Goals

- Discover Codex personal marketplaces from
  `$HOME/.agents/plugins/marketplace.json`.
- Discover the nearest repo/team marketplace by walking up from `<cwd>` to
  an ancestor containing `.agents/plugins/marketplace.json`.
- List discovered marketplaces in `tact-ui plugin marketplace list` with their
  local root path.
- Install plugins from discovered local catalogs.
- Support Codex `source: "local"` catalog entries.
- Make bare `tact-ui plugin install <name>` prefer a matching discovered Codex
  marketplace, falling back to `claude-plugins-official`.
- Keep the existing Claude official marketplace as a compatibility fallback.

## Non-goals

- Removing `claude-plugins-official`.
- Parsing Codex desktop/app configuration.
- Adding a remote Codex marketplace service.
- Supporting `marketplace add <local-path>` as a public CLI workflow in this
  change.

## Design

### Marketplace source model

Add `MarketplaceSource::LocalPath(PathBuf)`. A local source stores the Codex
marketplace root, not the catalog file. The catalog path is always
`<root>/.agents/plugins/marketplace.json`.

`MarketplaceState` tracks discovered local marketplaces in a non-persisted
`discovered` map. Persisted user Git/catalog marketplaces keep their existing
file shape. Discovered records are recreated on every load, so deleting the
Codex marketplace file removes the marketplace from Tact.

### Discovery

`PluginStore::load_marketplaces()` discovers:

- personal: `$HOME/.agents/plugins/marketplace.json`, root `$HOME`
- repo/team: nearest ancestor `<root>/.agents/plugins/marketplace.json`, root `<root>`

Catalog discovery reads only the top-level `name`; malformed optional Codex
marketplaces are ignored so plugin commands do not fail because an unrelated
local catalog is broken.

### Catalog resolution

`catalog_path(root)` now checks, in order:

1. `<root>/.agents/plugins/marketplace.json` (Codex)
2. `<root>/.codex-plugin/marketplace.json` (compatibility manifest location)
3. `<root>/marketplace.json` (legacy/local catalog)

Codex local sources resolve relative plugin paths against the marketplace root.
For `~/.agents/plugins/marketplace.json`, this means `$HOME/plugins/...`; for a
repo marketplace, this means `<repo>/plugins/...`.

### Install resolution

`tact-ui plugin install <name>` passes an empty marketplace when no `@marketplace`
is provided. `execute_request` then scans discovered Codex marketplaces first;
the first catalog containing the plugin name wins. If none matches, Tact falls
back to `claude-plugins-official`.

Local marketplace update is a no-op refresh: the catalog is re-read from disk.

## Tests

- Parse a Codex `source: "local"` catalog entry.
- Discover `$HOME/.agents/plugins/marketplace.json`.
- Install a plugin from a Codex local catalog with `source: "local"`.
- Bare install picks the discovered Codex marketplace.
- Existing plugin marketplace/install tests continue to pass.
