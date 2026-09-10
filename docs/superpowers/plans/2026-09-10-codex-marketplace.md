# Plan: Codex Local Marketplace Discovery

Date: 2026-09-10
Status: implemented

Spec: `docs/superpowers/specs/2026-09-10-codex-marketplace-design.md`

## Work Items

1. Extend `MarketplaceSource` with `LocalPath`.
2. Add non-persisted discovered marketplace records to `MarketplaceState`.
3. Discover Codex personal marketplaces and the nearest repo/team marketplace
   in `PluginStore`.
4. Resolve `.agents/plugins/marketplace.json` in `catalog_path`.
5. Parse Codex `source: "local"` plugin entries as relative sources.
6. Teach `MarketplaceService` to read local plugins and no-op local refreshes.
7. Teach `PluginInstaller` to resolve local marketplace source paths from the
   catalog root.
8. Make bare `plugin install <name>` prefer discovered Codex marketplaces and
   fall back to `claude-plugins-official`.
9. Update Ch 21 marketplace wording and add a Ch 26 issue-log entry.
10. Verify with targeted plugin tests and a manual `marketplace list`.

## Verification

- `cargo test -p tact plugin::`
- `cargo run -q -p tact-ui -- plugin marketplace list`
- Manual temp-home install from `$HOME/.agents/plugins/marketplace.json`.
