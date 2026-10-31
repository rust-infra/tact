# Tact Desktop visual verification

The desktop shell ships two verification paths that do not need Screen
Recording permission. Both render the production `DesktopRoot`, not a mock.

## 1. Off-screen render harness

`crates/desktop/src/capture.rs` opens the real shell in an off-screen GPUI
window through `gpui_platform::current_platform(true)` and
`VisualTestAppContext`. The capture goes through GPUI's Metal renderer, so the
PNG is what the compositor would show. Scripted entrances are skipped
(reduced-motion path) so every surface is captured in its settled state.

```bash
cargo run -p tact-desktop --example ui_capture --features ui-capture -- target/ui-capture
```

The walk opens every overlay and reader from the real shell code and writes
one PNG per surface:

| File | Surface |
|---|---|
| `01-shell-light`, `02-shell-dark` | Empty shell in both appearances |
| `03-command-center` | Flat Command Center over the dimmed shell |
| `04-slash-overlay` | Nested slash overlay over the composer |
| `05-settings` | Settings with startup/live mode labels |
| `06-session-manager` | Project chips, filters, rename/archive rows |
| `07`–`10` | Extensions MCP / Skills / Plugins / Hooks tabs |
| `11-background` | Background task list |
| `12`–`15` | Run inspector tabs |
| `16`–`20` | Typed readers (system prompt, stats, history, task DAG, balance) |
| `21`–`22` | Model picker and a free-form command form |
| `23-narrow-window` | 900×700 layout |
| `24-text-scale-150` | 150% text scale |
| `25-chinese` | Chinese catalog |
| `26`–`27` | Seeded transcript, light and dark |
| `28`–`31` | Populated inspector tabs |
| `32`–`33` | Populated background list and a permission gate |
| `34`–`37` | Tall transcript: bottom, top, and middle in both appearances |
| `38` | Populated task-dependency reader |
| `39` | Focus-visible tint after one Tab |

`crates/desktop_core/src/fixtures.rs` supplies the representative transcript
and inspector data behind the `ui-capture` feature. It is compiled only for
that feature and never by the product binary.

## 2. Runtime accessibility walkthrough

With the app running, macOS Accessibility exposes the live tree. This verifies
that each entry really opens its surface, not just that the code compiles:

```bash
osascript -l JavaScript scripts/ax-press.js "Settings"
osascript -l JavaScript scripts/ax-dump.js
```

The walkthrough used for the 2026-10-30 release covered: Settings → Model
picker, Command Center, Extensions (MCP/Skills/Plugins/Hooks), Run inspector
(Run/Goals/Subagents/Context), Session Manager, permission mode, and the
composer.

## 3. Packaged smoke test

The release workflow runs the packaged binary from a clean app-data directory:

```bash
HOME="$(mktemp -d)" TACT_DESKTOP_SMOKE=1 \
  "Tact Desktop.app/Contents/MacOS/tact-desktop"
```

Smoke mode initializes config, opens the window, and quits after one frame; a
panic, a failed shutdown, or a failed `tact-ui` sidecar version check fails the
job. The same environment variable is used on Linux (under `xvfb-run`) and
Windows.

## 4. Cross-platform package verification

`.github/workflows/desktop-package.yml` builds and packages all five release
targets without signing or publishing. It is the branch-level acceptance for
the release matrix; `release.yml` keeps the signed/notarized macOS path.

Run `41175364461` (commit `386f169b`, Rust 1.99) passed every target and
produced:

| Target | Asset | Verified payload |
|---|---|---|
| `aarch64-apple-darwin` | `tact-desktop-v2.0.0-aarch64-apple-darwin.zip` | `Tact Desktop.app`, Mach-O arm64, Info.plist 2.0.0 |
| `x86_64-apple-darwin` | `tact-desktop-v2.0.0-x86_64-apple-darwin.zip` | `Tact Desktop.app`, Mach-O x86_64, Info.plist 2.0.0 |
| `x86_64-unknown-linux-gnu` | `tact-desktop-v2.0.0-x86_64-unknown-linux-gnu.tar.gz` | ELF x86-64, licenses, Cargo.lock |
| `aarch64-unknown-linux-gnu` | `tact-desktop-v2.0.0-aarch64-unknown-linux-gnu.tar.gz` | ELF aarch64, licenses, Cargo.lock |
| `x86_64-pc-windows-msvc` | `tact-desktop-v2.0.0-x86_64-pc-windows-msvc.zip` | PE32+ GUI `tact-desktop.exe` + console `tact-ui.exe` |

Each job runs the packaged binary from a clean app-data directory
(`TACT_DESKTOP_SMOKE=1`); Linux uses Mesa lavapipe under `xvfb-run` because the
hosted runners have no GPU.

The signed release remains blocked on repository secrets the workflow requires:
`MACOS_CERTIFICATE`, `MACOS_CERTIFICATE_PASSWORD`, `MACOS_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_TEAM_ID`, and `APPLE_APP_PASSWORD`. Only
`CARGO_PACKAGER_SIGN_PRIVATE_KEY` is configured today.

## 5. Live entry walkthrough

Commands were typed into the Command Center (`Cmd+K`, type, Down, Enter) and
the resulting accessibility surface was recorded on 2026-10-31:

| Entry | Observed surface |
|---|---|
| `permission` | `Permission mode` dialog (Default/Plan/Auto) |
| `model-subagent` | `Subagent model` dialog with model choices |
| `view-system-prompt` | `System prompt` reader |
| `stats` | `Session statistics` reader |
| `tasks-dag` | `Task DAG` reader |
| `help` | `Keyboard Shortcuts` reader |
| `background` | `Background task` reader |
| `skill`, `plugin`, `hooks` | `Extensions` manager on its tab |
| `mcp` | `mcp get` argument form |
| `goal` | Run inspector with Run/Goals/Subagents/Context tabs |
| Settings → Model | `Model` picker |
| Manage sessions | `Session Manager` with rename/archive rows |

The Command Center itself exposes an `AXTextField` named `Search commands`, and
every transcript row, tab, gate, and reader carries an AccessKit role and label.

Session-dependent readers (`history`, `stats` payload, `balance`) and run
controls (`compact`, `cancel`, `save`, `subagent_cancel`) still show their
loading/disabled state because the walkthrough host has no configured model
provider; they are covered by the seeded off-screen captures in section 1.
