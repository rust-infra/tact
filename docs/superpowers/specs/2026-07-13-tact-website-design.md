# tact Product Website — Design

Date: 2026-07-13  
Status: Approved for implementation planning  
Product: [rust-infra/tact](https://github.com/rust-infra/tact) — Terminal-first AI coding agent (Rust, MIT)

## Goals

1. Ship a **product landing page** that communicates tact’s positioning in one scroll:
   terminal-first, Rust binary, self-hosted, MIT, extensible (MCP / skills / hooks).
2. Drive **install conversions**: one-command install copy, clear Configure → Run path.
3. Support **中英双语** with dedicated routes (`/` EN, `/zh` 中文).
4. Host at **tact.0x81.hk** with a static deploy from this monorepo.

## Non-goals (v1)

- User accounts, auth, or any backend API
- Web Dashboard (roadmap item; separate project)
- Full docs site hosting (`book/` stays on GitHub; footer links only)
- Blog / changelog pages (use GitHub Releases)
- Interactive theme switcher that recolors the whole site
- Third-party analytics / trackers

## Decisions (locked)

| Dimension | Choice |
|-----------|--------|
| Site type | Single-page Landing Page |
| Visual direction | Terminal aesthetic, TUI `retro` palette |
| Domain | `tact.0x81.hk` |
| Locale | EN + 中文 (`/` and `/zh`) |
| Stack | Astro static site under `website/` |
| Hero | Typewriter animation cycling a headless task demo |
| Theme section | Static preview of 4 palettes (retro / nord / brutal / dark), not interactive |
| Deploy | Cloudflare Pages (preferred) or Vercel; CNAME → `tact.0x81.hk` |

## Alternatives considered

| Option | Why not for v1 |
|--------|----------------|
| Pure HTML/CSS in `website/` | Cheaper deps, but bilingual duplication and weak animation story |
| Next.js | Heavier than a landing page; conflicts with “small Rust binary” brand |
| Docs-first (Starlight / mdBook host) | Wrong primary CTA; docs remain GitHub for now |

**Chosen:** Astro — zero-JS default, clean i18n routes, easy islands for the terminal hero, static export to any CDN.

## Information architecture

```
tact.0x81.hk
├── /                 → English landing
├── /zh               → Chinese landing
├── #features         → in-page anchors
├── #compare
├── #install
└── external
    ├── GitHub → https://github.com/rust-infra/tact
    ├── Docs   → GitHub book/ / ARCHITECTURE.md
    └── Issues / Discussions
```

No separate docs subdomain in v1.

## Visual design — Retro Terminal

Reuse TUI default `retro` tokens from `crates/tui/src/theme.rs`:

| Token | Value | Use |
|-------|-------|-----|
| `--bg` | `#0f0c06` | Page background |
| `--fg` | `#ffb432` | Body text (amber) |
| `--accent` | `#ffd250` | Titles, link hover |
| `--success` | `#c8ff50` | Checkmarks, CTA success |
| `--error` | `#ff3c28` | Comparison “missing” cells |
| `--border` | `#64461e` | Box-drawing frames |
| `--status-bar` | `#281c0c` | Top/bottom chrome |

**Typography**

- UI / code / headings: JetBrains Mono
- Chinese body paragraphs: Noto Sans SC fallback; keep titles mono where readable

**Decoration**

- Box-drawing: `┌─┐│└─┘╭─╮│╰─╯`
- Subtle CRT scanline overlay (`opacity ≈ 0.03`)
- Blinking cursor on the hero prompt line
- Status-bar chrome echoing TUI: `[tact vX.Y.Z] [MIT] [Rust]`

**Brand rule:** First viewport must read as tact even without nav — brand name + terminal window dominate; headline must not overpower the product name.

## Page sections (top → bottom)

### 1. Hero — Terminal window + typewriter

- Framed terminal mockup as the dominant visual plane (full-bleed dark bg).
- Typewriter loop demo (illustrative, not live agent):
  1. `$ tact-ui headless "Fix all clippy warnings"`
  2. `✓ read_file  src/lib.rs`
  3. `✓ bash       cargo clippy -- -D warnings`
  4. `✓ edit_file  src/lib.rs (+12 -3)`
  5. Pause → reset → repeat
- CTAs: copy install command; GitHub link; language switch `[EN | 中文]`
- Supporting line: “Terminal-first AI coding agent. Built in Rust. MIT licensed.”

### 2. Value props — four framed cells

| Cell | Message |
|------|---------|
| Rust binary | ~15MB, no Electron / Node |
| Self-hosted | Code never leaves the machine |
| MIT | Truly open source |
| Extensible | MCP · Skills · Hooks |

### 3. Features

Collapsible groups (native `<details>` or Astro components) covering:

- Agent loop (streaming, compaction, recovery)
- 40+ tools (File / Shell / LSP / Web / Team / Worktree / Cron)
- Permission modes (`default` / `plan` / `auto`)
- Sub-agents & worktree isolation
- Native MCP

Copy sourced from README; keep technical terms in English even on `/zh` where conventional (MCP, worktree, headless).

### 4. Comparison table

README comparison columns: tact vs Claude Code / Cursor / Aider / Open Interpreter.  
tact column emphasized with `--success`; other columns desaturated `--fg`.

### 5. Quick Start — three steps

1. **Install** — platform tabs: Unix `curl … \| bash`, Windows PowerShell `irm … \| iex`
2. **Configure** — minimal `tact.toml` snippet (`provider`, `model`, `api_key`)
3. **Run** — `tact-ui` and `tact-ui headless "…"`

Each code block has a copy button.

### 6. Theme preview (static)

Horizontal strip of **four** palette swatches only:

- `retro` (site default)
- `nord`
- `brutal`
- `dark`

Labels + swatch chips only; **no** live site recoloring in v1.

### 7. Footer

GitHub / Issues / Contributing · MIT · “Built with 🦀 by Rg0x80” · language switch.

## Internationalization

| Concern | Approach |
|---------|----------|
| Routes | `/` = EN, `/zh` = 中文 |
| Strings | `src/i18n/en.json`, `src/i18n/zh.json` |
| Switcher | Header + footer; optional `localStorage` preference for next visit |
| Terms | Keep MCP, worktree, headless, TUI untranslated |

## Project layout

```
website/
├── package.json
├── astro.config.mjs
├── src/
│   ├── layouts/Base.astro
│   ├── pages/
│   │   ├── index.astro          # EN
│   │   └── zh/index.astro       # 中文
│   ├── components/
│   │   ├── TerminalHero.astro   # (+ island JS for typewriter if needed)
│   │   ├── ValueProps.astro
│   │   ├── FeatureGrid.astro
│   │   ├── CompareTable.astro
│   │   ├── InstallSteps.astro
│   │   ├── ThemePreview.astro
│   │   └── LangSwitch.astro
│   ├── i18n/
│   │   ├── en.json
│   │   └── zh.json
│   └── styles/
│       ├── global.css
│       └── retro.css
└── public/
    ├── tact.png                 # from repo root branding
    └── favicon.svg
```

CI: `.github/workflows/website.yml` — on push to `main` (paths: `website/**`), `astro build`, deploy to Cloudflare Pages (or Vercel).

## Deployment

```
DNS: tact.0x81.hk
  └── CNAME → Cloudflare Pages project (preferred)
      └── build: website/  →  npm ci && npm run build  →  dist/
```

Requirements:

- HTTPS
- Preview deployments for PRs touching `website/`
- No secrets in the static site (install URLs point at public GitHub raw / releases)

## Performance & SEO

- Target: Lighthouse Performance ≥ 95, Accessibility ≥ 90
- Bilingual `<title>` / meta description / Open Graph; `og:image` from terminal-style capture or branded mark
- Subset fonts (Latin + SC essentials)
- No third-party trackers

## Content sources

| Site copy | Source of truth |
|-----------|-----------------|
| Positioning, features, tools, comparison | `README.md` |
| Install commands | `README.md` + `scripts/install.sh` / `install.ps1` |
| Version badge | workspace `version` (`0.19.0` at design time; keep in sync or read at build) |
| Colors | `crates/tui/src/theme.rs` (`Retro`, `Nord`, `Brutal`, `Dark`) |

## Success criteria

1. Visitor understands “what / why / how to install” within one scroll on mobile and desktop.
2. Install command copy works on EN and `/zh`.
3. Language switch preserves section intent (same anchors).
4. Static deploy serves correctly at `https://tact.0x81.hk`.
5. First viewport passes the brand test: after removing nav, the page is still recognizably tact (product name + terminal window dominate).

## Out of scope follow-ups

- `docs.tact.0x81.hk` or Starlight migration of `book/`
- Theme Showcase interaction (page recolor)
- Live terminal embedding / WASM demo
- crates.io publish badge automation
