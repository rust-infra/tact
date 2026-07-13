# tact Product Website Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a bilingual (EN/zh) Astro landing page for tact at `tact.0x81.hk`, with retro-terminal visuals, typewriter hero, install CTAs, and Cloudflare Pages deploy CI.

**Architecture:** Static Astro site in `website/`. Shared layout + section components; copy in `src/i18n/{en,zh}.json`; pages at `/` and `/zh`. Typewriter hero is a small client island; everything else is zero-JS Astro. Deploy via GitHub Actions → Cloudflare Pages.

**Tech Stack:** Astro 5 (static output), vanilla CSS (retro tokens from TUI), optional tiny client script for typewriter + copy buttons, JetBrains Mono + Noto Sans SC fonts.

**Spec:** `docs/superpowers/specs/2026-07-13-tact-website-design.md`

---

## File map

| Path | Responsibility |
|------|----------------|
| `website/package.json` | Astro deps + scripts |
| `website/astro.config.mjs` | Static site, `site: https://tact.0x81.hk` |
| `website/tsconfig.json` | Strict TS for islands |
| `website/src/styles/global.css` | Reset, fonts, layout primitives |
| `website/src/styles/retro.css` | CSS variables + CRT/box-drawing chrome |
| `website/src/i18n/en.json` | English strings |
| `website/src/i18n/zh.json` | Chinese strings |
| `website/src/i18n/index.ts` | `t(lang)`, `Lang` type, helpers |
| `website/src/layouts/Base.astro` | HTML shell, meta, fonts, status bar chrome |
| `website/src/components/LangSwitch.astro` | EN / 中文 links |
| `website/src/components/TerminalHero.astro` | Hero frame + mount point |
| `website/src/components/TerminalHero.ts` | Typewriter island script |
| `website/src/components/ValueProps.astro` | Four framed value cells |
| `website/src/components/FeatureGrid.astro` | Collapsible feature groups |
| `website/src/components/CompareTable.astro` | Comparison table |
| `website/src/components/InstallSteps.astro` | Install / configure / run + copy |
| `website/src/components/ThemePreview.astro` | Static 4-theme swatches |
| `website/src/components/SiteFooter.astro` | Footer links + lang |
| `website/src/pages/index.astro` | EN landing |
| `website/src/pages/zh/index.astro` | ZH landing |
| `website/public/favicon.svg` | Terminal-style favicon |
| `website/public/tact.png` | Copy of repo `tact.png` (or symlink note) |
| `.github/workflows/website.yml` | Build + Cloudflare Pages deploy |

---

### Task 1: Scaffold Astro project

**Files:**
- Create: `website/package.json`
- Create: `website/astro.config.mjs`
- Create: `website/tsconfig.json`
- Create: `website/.gitignore`
- Create: `website/src/pages/index.astro` (temporary stub)

- [ ] **Step 1: Create `website/package.json`**

```json
{
  "name": "tact-website",
  "private": true,
  "type": "module",
  "version": "0.19.0",
  "scripts": {
    "dev": "astro dev",
    "build": "astro build",
    "preview": "astro preview"
  },
  "dependencies": {
    "astro": "^5.7.0"
  }
}
```

- [ ] **Step 2: Create `website/astro.config.mjs`**

```js
import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://tact.0x81.hk",
  output: "static",
  trailingSlash: "never",
});
```

- [ ] **Step 3: Create `website/tsconfig.json`**

```json
{
  "extends": "astro/tsconfigs/strict",
  "include": [".astro/types.d.ts", "**/*"],
  "exclude": ["dist"]
}
```

- [ ] **Step 4: Create `website/.gitignore`**

```
node_modules/
dist/
.astro/
```

- [ ] **Step 5: Create stub `website/src/pages/index.astro`**

```astro
---
---
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>tact</title>
  </head>
  <body>
    <h1>tact</h1>
  </body>
</html>
```

- [ ] **Step 6: Install and verify build**

```bash
cd website && npm install && npm run build
```

Expected: `website/dist/index.html` exists; exit 0.

- [ ] **Step 7: Commit**

```bash
git add website/
git commit -m "$(cat <<'EOF'
chore(website): scaffold Astro static site

EOF
)"
```

---

### Task 2: Design tokens + global styles

**Files:**
- Create: `website/src/styles/retro.css`
- Create: `website/src/styles/global.css`
- Create: `website/public/favicon.svg`

- [ ] **Step 1: Create `website/src/styles/retro.css`**

Map from `crates/tui/src/theme.rs` `ThemeName::Retro` (+ preview themes later):

```css
:root {
  --bg: #0f0c06;
  --fg: #ffb432;
  --accent: #ffd250;
  --success: #c8ff50;
  --error: #ff3c28;
  --warning: #ff8c1e;
  --border: #64461e;
  --status-bar: #281c0c;
  --highlight: #503c14;
  --font-mono: "JetBrains Mono", ui-monospace, "SFMono-Regular", Menlo, Consolas, monospace;
  --font-zh: "Noto Sans SC", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", sans-serif;
  --max-width: 56rem;
  --scanline-opacity: 0.03;
}
```

- [ ] **Step 2: Create `website/src/styles/global.css`**

```css
@import "./retro.css";

*,
*::before,
*::after {
  box-sizing: border-box;
}

html {
  color-scheme: dark;
  scroll-behavior: smooth;
}

body {
  margin: 0;
  min-height: 100vh;
  background: var(--bg);
  color: var(--fg);
  font-family: var(--font-mono);
  line-height: 1.55;
  position: relative;
}

/* Subtle CRT scanlines */
body::after {
  content: "";
  pointer-events: none;
  position: fixed;
  inset: 0;
  z-index: 9999;
  background: repeating-linear-gradient(
    to bottom,
    transparent 0,
    transparent 2px,
    rgba(0, 0, 0, var(--scanline-opacity)) 2px,
    rgba(0, 0, 0, var(--scanline-opacity)) 4px
  );
}

a {
  color: var(--accent);
  text-decoration: none;
}
a:hover {
  text-decoration: underline;
}

.wrap {
  width: min(100% - 2rem, var(--max-width));
  margin-inline: auto;
}

.section {
  padding: 3rem 0;
}

.section h2 {
  color: var(--accent);
  font-size: 1.25rem;
  font-weight: 600;
  margin: 0 0 1.25rem;
}

.frame {
  border: 1px solid var(--border);
  background: color-mix(in srgb, var(--bg) 92%, black);
}

.frame-title {
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.4rem 0.75rem;
  border-bottom: 1px solid var(--border);
  background: var(--status-bar);
  color: var(--accent);
  font-size: 0.85rem;
}

.btn {
  display: inline-flex;
  align-items: center;
  gap: 0.4rem;
  padding: 0.55rem 0.9rem;
  border: 1px solid var(--border);
  background: var(--status-bar);
  color: var(--accent);
  font: inherit;
  cursor: pointer;
}
.btn:hover {
  border-color: var(--accent);
}
.btn-primary {
  border-color: var(--success);
  color: var(--success);
}

code,
pre {
  font-family: var(--font-mono);
}

.zh-body {
  font-family: var(--font-zh);
}

@keyframes blink {
  50% {
    opacity: 0;
  }
}

.cursor {
  display: inline-block;
  width: 0.55ch;
  height: 1.1em;
  background: var(--accent);
  vertical-align: text-bottom;
  animation: blink 1s step-end infinite;
}

@media (max-width: 640px) {
  .section {
    padding: 2rem 0;
  }
}
```

- [ ] **Step 3: Create `website/public/favicon.svg`**

Simple amber terminal glyph on dark:

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <rect width="32" height="32" rx="2" fill="#0f0c06"/>
  <rect x="3" y="3" width="26" height="26" fill="none" stroke="#64461e" stroke-width="1"/>
  <text x="6" y="22" font-family="monospace" font-size="14" fill="#ffb432">&gt;_</text>
</svg>
```

- [ ] **Step 4: Copy logo**

```bash
cp tact.png website/public/tact.png
```

- [ ] **Step 5: Commit**

```bash
git add website/src/styles website/public
git commit -m "$(cat <<'EOF'
style(website): add retro terminal design tokens

EOF
)"
```

---

### Task 3: i18n dictionaries + helper

**Files:**
- Create: `website/src/i18n/en.json`
- Create: `website/src/i18n/zh.json`
- Create: `website/src/i18n/index.ts`

- [ ] **Step 1: Create `website/src/i18n/en.json`**

Use README positioning. Keep keys stable (zh mirrors structure).

```json
{
  "meta": {
    "title": "tact — Terminal-first AI coding agent",
    "description": "Terminal-first AI coding agent. Built in Rust. MIT licensed. Self-hosted, ~15MB binary, MCP, skills, and worktree isolation."
  },
  "nav": {
    "features": "Features",
    "compare": "Compare",
    "install": "Install",
    "github": "GitHub",
    "docs": "Docs"
  },
  "hero": {
    "tagline": "Terminal-first AI coding agent. Built in Rust. MIT licensed.",
    "ctaInstall": "Copy install command",
    "ctaGithub": "GitHub",
    "copied": "Copied",
    "status": "[tact v0.19.0] [MIT] [Rust]",
    "prompt": "tact-ui headless \"Fix all clippy warnings\"",
    "lines": [
      "✓ read_file  src/lib.rs",
      "✓ bash       cargo clippy -- -D warnings",
      "✓ edit_file  src/lib.rs (+12 -3)"
    ]
  },
  "values": {
    "title": "Why tact",
    "items": [
      { "title": "Rust binary", "body": "~15MB single binary. No Electron. No Node.js." },
      { "title": "Self-hosted", "body": "Your code never leaves your machine." },
      { "title": "MIT license", "body": "Truly open source — not source-available theater." },
      { "title": "Extensible", "body": "MCP plugins, custom skills, hooks, and tool macros." }
    ]
  },
  "features": {
    "title": "Features",
    "groups": [
      {
        "title": "Agent loop",
        "body": "Multi-turn streaming loop with auto-compaction, interrupted-session recovery, and persistent memory."
      },
      {
        "title": "40+ tools",
        "body": "File system, shell, ripgrep, LSP, web search/fetch, tasks, team inbox, worktree lanes, cron, and more."
      },
      {
        "title": "Permission modes",
        "body": "default (ask every tool) · plan (plan then ask once) · auto (CI / trusted repos)."
      },
      {
        "title": "Sub-agents & worktrees",
        "body": "Spawn teammates with inboxes; isolate parallel work in git worktree lanes."
      },
      {
        "title": "Native MCP",
        "body": "Connect any MCP server; its tools appear in the agent loop at runtime."
      }
    ]
  },
  "compare": {
    "title": "Comparison",
    "headers": ["", "tact", "Claude Code", "Cursor", "Aider", "Open Interpreter"],
    "rows": [
      ["Language", "Rust", "TypeScript", "TypeScript", "Python", "Python"],
      ["Interface", "Terminal / TUI", "Terminal", "Editor", "Terminal", "Terminal"],
      ["License", "MIT", "Proprietary", "Proprietary", "Apache 2.0", "AGPL"],
      ["Self-hosted", "✓", "✓", "✓", "✓", "✓"],
      ["Worktree isolation", "✓", "✗", "✗", "✗", "✗"],
      ["MCP", "✓", "✓", "via ext", "✗", "✗"],
      ["Cron", "✓", "✗", "✗", "✗", "✗"],
      ["Binary size", "~15MB", "Hundreds MB", "Hundreds MB", "~50MB+", "~200MB+"]
    ]
  },
  "install": {
    "title": "Quick Start",
    "step1": "Install",
    "step2": "Configure",
    "step3": "Run",
    "unix": "Linux / macOS",
    "windows": "Windows (PowerShell)",
    "unixCmd": "curl -fsSL https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.sh | bash",
    "windowsCmd": "irm https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.ps1 | iex",
    "configLabel": "Create tact.toml (or ~/.tact/config.toml)",
    "configSnippet": "[llm]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\napi_key = \"sk-ant-...\"\nbase_url = \"https://api.anthropic.com\"\n\n[permission]\nmode = \"default\"",
    "runInteractive": "tact-ui",
    "runHeadless": "tact-ui headless \"Fix all clippy warnings in src/ and run cargo test\"",
    "copy": "Copy"
  },
  "themes": {
    "title": "TUI themes",
    "subtitle": "Eleven built-in themes. A few representatives:"
  },
  "footer": {
    "license": "MIT License",
    "builtBy": "Built with 🦀 by Rg0x80",
    "contributing": "Contributing",
    "issues": "Issues"
  }
}
```

- [ ] **Step 2: Create `website/src/i18n/zh.json`**

Mirror the same keys. Translate UI copy; keep MCP / worktree / headless / TUI / tool names in English where conventional. Example hero:

```json
{
  "meta": {
    "title": "tact — 终端优先的 AI 编程助手",
    "description": "终端优先的 AI 编程助手。Rust 编写，MIT 开源，完全自托管，约 15MB 单二进制。支持 MCP、Skills、Worktree 隔离。"
  },
  "nav": {
    "features": "特性",
    "compare": "对比",
    "install": "安装",
    "github": "GitHub",
    "docs": "文档"
  },
  "hero": {
    "tagline": "终端优先的 AI 编程助手。Rust 编写。MIT 开源。",
    "ctaInstall": "复制安装命令",
    "ctaGithub": "GitHub",
    "copied": "已复制",
    "status": "[tact v0.19.0] [MIT] [Rust]",
    "prompt": "tact-ui headless \"Fix all clippy warnings\"",
    "lines": [
      "✓ read_file  src/lib.rs",
      "✓ bash       cargo clippy -- -D warnings",
      "✓ edit_file  src/lib.rs (+12 -3)"
    ]
  },
  "values": {
    "title": "为什么选 tact",
    "items": [
      { "title": "Rust 单二进制", "body": "约 15MB。无 Electron，无 Node.js。" },
      { "title": "完全自托管", "body": "代码不离开你的机器。" },
      { "title": "MIT 许可", "body": "真正开源，不是「源码可见」。" },
      { "title": "可扩展", "body": "MCP 插件、自定义 Skills、Hooks 与工具宏。" }
    ]
  },
  "features": {
    "title": "特性",
    "groups": [
      {
        "title": "Agent loop",
        "body": "多轮流式对话，自动压缩上下文，中断会话可恢复，跨会话记忆持久化。"
      },
      {
        "title": "40+ tools",
        "body": "文件系统、Shell、ripgrep、LSP、网页搜索/抓取、任务、团队收件箱、Worktree、Cron 等。"
      },
      {
        "title": "Permission modes",
        "body": "default（每次询问）· plan（先规划再确认）· auto（CI / 可信仓库）。"
      },
      {
        "title": "Sub-agents & worktrees",
        "body": "通过 inbox 协调队友；用 git worktree 隔离并行任务。"
      },
      {
        "title": "Native MCP",
        "body": "连接任意 MCP server，工具在运行时进入 agent loop。"
      }
    ]
  },
  "compare": {
    "title": "对比",
    "headers": ["", "tact", "Claude Code", "Cursor", "Aider", "Open Interpreter"],
    "rows": [
      ["语言", "Rust", "TypeScript", "TypeScript", "Python", "Python"],
      ["界面", "Terminal / TUI", "Terminal", "Editor", "Terminal", "Terminal"],
      ["许可", "MIT", "Proprietary", "Proprietary", "Apache 2.0", "AGPL"],
      ["自托管", "✓", "✓", "✓", "✓", "✓"],
      ["Worktree 隔离", "✓", "✗", "✗", "✗", "✗"],
      ["MCP", "✓", "✓", "via ext", "✗", "✗"],
      ["Cron", "✓", "✗", "✗", "✗", "✗"],
      ["体积", "~15MB", "Hundreds MB", "Hundreds MB", "~50MB+", "~200MB+"]
    ]
  },
  "install": {
    "title": "快速开始",
    "step1": "安装",
    "step2": "配置",
    "step3": "运行",
    "unix": "Linux / macOS",
    "windows": "Windows (PowerShell)",
    "unixCmd": "curl -fsSL https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.sh | bash",
    "windowsCmd": "irm https://raw.githubusercontent.com/rust-infra/tact/main/scripts/install.ps1 | iex",
    "configLabel": "创建 tact.toml（或 ~/.tact/config.toml）",
    "configSnippet": "[llm]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\napi_key = \"sk-ant-...\"\nbase_url = \"https://api.anthropic.com\"\n\n[permission]\nmode = \"default\"",
    "runInteractive": "tact-ui",
    "runHeadless": "tact-ui headless \"Fix all clippy warnings in src/ and run cargo test\"",
    "copy": "复制"
  },
  "themes": {
    "title": "TUI 主题",
    "subtitle": "内置 11 套主题。以下为代表性色板："
  },
  "footer": {
    "license": "MIT 许可证",
    "builtBy": "Built with 🦀 by Rg0x80",
    "contributing": "参与贡献",
    "issues": "Issues"
  }
}
```

- [ ] **Step 3: Create `website/src/i18n/index.ts`**

```ts
import en from "./en.json";
import zh from "./zh.json";

export type Lang = "en" | "zh";
export type Dict = typeof en;

const dicts: Record<Lang, Dict> = { en, zh };

export function t(lang: Lang): Dict {
  return dicts[lang];
}

export function otherLang(lang: Lang): Lang {
  return lang === "en" ? "zh" : "en";
}

export function langHref(lang: Lang): string {
  return lang === "en" ? "/" : "/zh";
}

export function langLabel(lang: Lang): string {
  return lang === "en" ? "EN" : "中文";
}
```

- [ ] **Step 4: Smoke-check keys match**

```bash
cd website && node --input-type=module -e "
import en from './src/i18n/en.json' with { type: 'json' };
import zh from './src/i18n/zh.json' with { type: 'json' };
function keys(o,p=''){return Object.entries(o).flatMap(([k,v])=>typeof v==='object'&&v&&!Array.isArray(v)?keys(v,p+k+'.'):[p+k]);}
const a=keys(en).sort().join('\n'); const b=keys(zh).sort().join('\n');
if(a!==b){console.error('key mismatch'); process.exit(1);} console.log('ok', keys(en).length);
"
```

Expected: `ok` with equal key counts.

- [ ] **Step 5: Commit**

```bash
git add website/src/i18n
git commit -m "$(cat <<'EOF'
feat(website): add EN/zh copy dictionaries

EOF
)"
```

---

### Task 4: Base layout + LangSwitch + Footer

**Files:**
- Create: `website/src/layouts/Base.astro`
- Create: `website/src/components/LangSwitch.astro`
- Create: `website/src/components/SiteFooter.astro`

- [ ] **Step 1: Create `LangSwitch.astro`**

```astro
---
import { type Lang, otherLang, langHref, langLabel } from "../i18n";
interface Props { lang: Lang }
const { lang } = Astro.props;
const other = otherLang(lang);
---
<nav class="lang" aria-label="Language">
  <a href={langHref("en")} aria-current={lang === "en" ? "page" : undefined}>EN</a>
  <span aria-hidden="true">|</span>
  <a href={langHref("zh")} aria-current={lang === "zh" ? "page" : undefined}>中文</a>
</nav>
<style>
  .lang { display: inline-flex; gap: 0.4rem; font-size: 0.85rem; }
  .lang a[aria-current="page"] { color: var(--success); text-decoration: none; }
</style>
```

- [ ] **Step 2: Create `SiteFooter.astro`**

```astro
---
import { type Lang, t } from "../i18n";
import LangSwitch from "./LangSwitch.astro";
interface Props { lang: Lang }
const { lang } = Astro.props;
const d = t(lang);
const github = "https://github.com/rust-infra/tact";
---
<footer class="footer wrap section">
  <div class="frame">
    <div class="frame-title">tact · footer</div>
    <div class="body">
      <p>
        <a href={github}>GitHub</a> ·
        <a href={`${github}/issues`}>{d.footer.issues}</a> ·
        <a href={`${github}/blob/main/README.md#contributing`}>{d.footer.contributing}</a> ·
        <a href={`${github}/blob/main/book/index.md`}>{d.nav.docs}</a>
      </p>
      <p>{d.footer.license} · {d.footer.builtBy}</p>
      <LangSwitch lang={lang} />
    </div>
  </div>
</footer>
<style>
  .body { padding: 1rem 0.9rem; font-size: 0.85rem; }
  .body p { margin: 0 0 0.6rem; }
</style>
```

- [ ] **Step 3: Create `Base.astro`**

```astro
---
import "../styles/global.css";
import { type Lang, t } from "../i18n";
import LangSwitch from "../components/LangSwitch.astro";

interface Props {
  lang: Lang;
}
const { lang } = Astro.props;
const d = t(lang);
const canonical = lang === "en" ? "https://tact.0x81.hk/" : "https://tact.0x81.hk/zh";
const github = "https://github.com/rust-infra/tact";
---
<!doctype html>
<html lang={lang === "zh" ? "zh-Hans" : "en"}>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{d.meta.title}</title>
    <meta name="description" content={d.meta.description} />
    <link rel="canonical" href={canonical} />
    <link rel="alternate" hreflang="en" href="https://tact.0x81.hk/" />
    <link rel="alternate" hreflang="zh-Hans" href="https://tact.0x81.hk/zh" />
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <meta property="og:title" content={d.meta.title} />
    <meta property="og:description" content={d.meta.description} />
    <meta property="og:url" content={canonical} />
    <meta property="og:image" content="https://tact.0x81.hk/tact.png" />
    <meta property="og:type" content="website" />
    <link rel="preconnect" href="https://fonts.googleapis.com" />
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
    <link
      href="https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@400;600&family=Noto+Sans+SC:wght@400;600&display=swap"
      rel="stylesheet"
    />
  </head>
  <body class={lang === "zh" ? "zh-body" : undefined}>
    <header class="topbar">
      <div class="wrap topbar-inner">
        <a class="brand" href={lang === "en" ? "/" : "/zh"}>
          <img src="/tact.png" alt="" width="28" height="28" />
          <span>tact</span>
        </a>
        <nav class="nav" aria-label="Primary">
          <a href="#features">{d.nav.features}</a>
          <a href="#compare">{d.nav.compare}</a>
          <a href="#install">{d.nav.install}</a>
          <a href={github}>{d.nav.github}</a>
        </nav>
        <LangSwitch lang={lang} />
      </div>
      <div class="status wrap" aria-hidden="true">{d.hero.status}</div>
    </header>
    <main>
      <slot />
    </main>
  </body>
</html>
<style>
  .topbar {
    border-bottom: 1px solid var(--border);
    background: var(--status-bar);
    position: sticky;
    top: 0;
    z-index: 20;
  }
  .topbar-inner {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding: 0.65rem 0;
    flex-wrap: wrap;
  }
  .brand {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    color: var(--accent);
    font-weight: 600;
    font-size: 1.15rem;
    text-decoration: none;
  }
  .nav {
    display: flex;
    gap: 0.9rem;
    flex: 1;
    flex-wrap: wrap;
    font-size: 0.85rem;
  }
  .status {
    padding: 0.25rem 0 0.55rem;
    color: color-mix(in srgb, var(--fg) 70%, transparent);
    font-size: 0.75rem;
  }
</style>
```

- [ ] **Step 4: Commit**

```bash
git add website/src/layouts website/src/components/LangSwitch.astro website/src/components/SiteFooter.astro
git commit -m "$(cat <<'EOF'
feat(website): add base layout, language switch, footer

EOF
)"
```

---

### Task 5: TerminalHero typewriter island

**Files:**
- Create: `website/src/components/TerminalHero.astro`
- Create: `website/src/components/terminal-hero.ts`

- [ ] **Step 1: Create `terminal-hero.ts`**

Client script: type prompt char-by-char, append result lines, pause, clear, loop. Respect `prefers-reduced-motion` (show final state immediately).

```ts
export type HeroLines = {
  prompt: string;
  lines: string[];
};

export function mountTerminalHero(root: HTMLElement, data: HeroLines): () => void {
  const promptEl = root.querySelector<HTMLElement>("[data-prompt]");
  const logEl = root.querySelector<HTMLElement>("[data-log]");
  const cursorEl = root.querySelector<HTMLElement>("[data-cursor]");
  if (!promptEl || !logEl || !cursorEl) return () => {};

  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  let cancelled = false;
  let timer = 0;

  const sleep = (ms: number) =>
    new Promise<void>((resolve) => {
      timer = window.setTimeout(resolve, ms);
    });

  async function run() {
    while (!cancelled) {
      promptEl!.textContent = "";
      logEl!.innerHTML = "";
      cursorEl!.hidden = false;

      if (reduced) {
        promptEl!.textContent = data.prompt;
        for (const line of data.lines) {
          const div = document.createElement("div");
          div.textContent = line;
          div.className = "ok";
          logEl!.appendChild(div);
        }
        cursorEl!.hidden = true;
        return;
      }

      for (const ch of data.prompt) {
        if (cancelled) return;
        promptEl!.textContent += ch;
        await sleep(28);
      }
      await sleep(350);
      for (const line of data.lines) {
        if (cancelled) return;
        const div = document.createElement("div");
        div.textContent = line;
        div.className = "ok";
        logEl!.appendChild(div);
        await sleep(420);
      }
      cursorEl!.hidden = true;
      await sleep(1800);
    }
  }

  void run();
  return () => {
    cancelled = true;
    window.clearTimeout(timer);
  };
}
```

- [ ] **Step 2: Create `TerminalHero.astro`**

```astro
---
import { type Lang, t } from "../i18n";
interface Props { lang: Lang }
const { lang } = Astro.props;
const d = t(lang);
const installCmd = d.install.unixCmd;
---
<section class="hero wrap section" aria-label="tact">
  <div class="brand-block">
    <img src="/tact.png" alt="tact" width="72" height="72" />
    <h1>tact</h1>
    <p class="tagline">{d.hero.tagline}</p>
  </div>

  <div class="frame terminal" id="terminal-hero" data-prompt-text={d.hero.prompt} data-lines={JSON.stringify(d.hero.lines)}>
    <div class="frame-title">╭─ tact-ui ──────────────────────────────╮</div>
    <div class="screen" role="img" aria-label={`$ ${d.hero.prompt}`}>
      <div class="line">
        <span class="ps">$</span>
        <span data-prompt></span>
        <span class="cursor" data-cursor></span>
      </div>
      <div data-log class="log"></div>
    </div>
  </div>

  <div class="ctas">
    <button class="btn btn-primary" type="button" data-copy={installCmd} data-copied-label={d.hero.copied}>
      {d.hero.ctaInstall}
    </button>
    <a class="btn" href="https://github.com/rust-infra/tact">{d.hero.ctaGithub}</a>
  </div>
</section>

<script>
  import { mountTerminalHero } from "./terminal-hero";

  const root = document.getElementById("terminal-hero");
  if (root instanceof HTMLElement) {
    const prompt = root.dataset.promptText ?? "";
    const lines = JSON.parse(root.dataset.lines ?? "[]") as string[];
    mountTerminalHero(root, { prompt, lines });
  }

  document.querySelectorAll<HTMLButtonElement>("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const text = btn.dataset.copy ?? "";
      try {
        await navigator.clipboard.writeText(text);
        const prev = btn.textContent;
        btn.textContent = btn.dataset.copiedLabel ?? "Copied";
        setTimeout(() => {
          btn.textContent = prev;
        }, 1200);
      } catch {
        /* ignore */
      }
    });
  });
</script>

<style>
  .brand-block {
    text-align: center;
    margin-bottom: 1.5rem;
  }
  h1 {
    margin: 0.4rem 0 0.5rem;
    font-size: clamp(2.4rem, 6vw, 3.4rem);
    color: var(--accent);
    letter-spacing: 0.04em;
  }
  .tagline {
    margin: 0 auto;
    max-width: 36rem;
    color: var(--fg);
  }
  .terminal .screen {
    padding: 1rem 1rem 1.25rem;
    min-height: 9.5rem;
    font-size: 0.92rem;
  }
  .ps { color: var(--success); margin-right: 0.4rem; }
  .log { margin-top: 0.75rem; }
  .log :global(.ok) { color: var(--success); margin: 0.2rem 0; }
  .ctas {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
    justify-content: center;
    margin-top: 1.25rem;
  }
</style>
```

- [ ] **Step 3: Build verify**

```bash
cd website && npm run build
```

Expected: exit 0. Dist contains bundled script for the island.

- [ ] **Step 4: Commit**

```bash
git add website/src/components/TerminalHero.astro website/src/components/terminal-hero.ts
git commit -m "$(cat <<'EOF'
feat(website): add typewriter terminal hero

EOF
)"
```

---

### Task 6: ValueProps + FeatureGrid + CompareTable

**Files:**
- Create: `website/src/components/ValueProps.astro`
- Create: `website/src/components/FeatureGrid.astro`
- Create: `website/src/components/CompareTable.astro`

- [ ] **Step 1: `ValueProps.astro`**

```astro
---
import { type Lang, t } from "../i18n";
interface Props { lang: Lang }
const { lang } = Astro.props;
const d = t(lang);
---
<section class="wrap section" aria-labelledby="values-title">
  <h2 id="values-title">{d.values.title}</h2>
  <div class="grid">
    {d.values.items.map((item) => (
      <article class="frame cell">
        <div class="frame-title">┌─ {item.title} ─┐</div>
        <p>{item.body}</p>
      </article>
    ))}
  </div>
</section>
<style>
  .grid {
    display: grid;
    gap: 0.9rem;
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .cell p { margin: 0; padding: 0.85rem; font-size: 0.92rem; }
  @media (max-width: 640px) {
    .grid { grid-template-columns: 1fr; }
  }
</style>
```

- [ ] **Step 2: `FeatureGrid.astro`**

```astro
---
import { type Lang, t } from "../i18n";
interface Props { lang: Lang }
const { lang } = Astro.props;
const d = t(lang);
---
<section id="features" class="wrap section" aria-labelledby="features-title">
  <h2 id="features-title">{d.features.title}</h2>
  <div class="list">
    {d.features.groups.map((g) => (
      <details class="frame">
        <summary class="frame-title">{g.title}</summary>
        <p>{g.body}</p>
      </details>
    ))}
  </div>
</section>
<style>
  .list { display: grid; gap: 0.65rem; }
  details p { margin: 0; padding: 0.85rem; font-size: 0.92rem; }
  summary { cursor: pointer; list-style: none; }
  summary::-webkit-details-marker { display: none; }
</style>
```

- [ ] **Step 3: `CompareTable.astro`**

```astro
---
import { type Lang, t } from "../i18n";
interface Props { lang: Lang }
const { lang } = Astro.props;
const d = t(lang);
---
<section id="compare" class="wrap section" aria-labelledby="compare-title">
  <h2 id="compare-title">{d.compare.title}</h2>
  <div class="frame table-wrap">
    <div class="frame-title">comparison</div>
    <div class="scroll">
      <table>
        <thead>
          <tr>
            {d.compare.headers.map((h, i) => (
              <th class={i === 1 ? "tact" : undefined}>{h}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {d.compare.rows.map((row) => (
            <tr>
              {row.map((cell, i) => (
                <td class={i === 1 ? "tact" : undefined}>{cell}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  </div>
</section>
<style>
  .scroll { overflow-x: auto; }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.82rem;
  }
  th, td {
    padding: 0.55rem 0.65rem;
    border-top: 1px solid var(--border);
    text-align: left;
    white-space: nowrap;
  }
  th.tact, td.tact {
    color: var(--success);
    font-weight: 600;
  }
  td:not(.tact):not(:first-child) {
    color: color-mix(in srgb, var(--fg) 72%, transparent);
  }
</style>
```

- [ ] **Step 4: Commit**

```bash
git add website/src/components/ValueProps.astro website/src/components/FeatureGrid.astro website/src/components/CompareTable.astro
git commit -m "$(cat <<'EOF'
feat(website): add value props, features, comparison

EOF
)"
```

---

### Task 7: InstallSteps + ThemePreview

**Files:**
- Create: `website/src/components/InstallSteps.astro`
- Create: `website/src/components/ThemePreview.astro`

- [ ] **Step 1: `InstallSteps.astro`**

Platform tabs via radio + CSS (no JS required for switching). Copy buttons reuse `[data-copy]` pattern from hero — include a small shared script block or duplicate the click handler in this component’s `<script>`.

```astro
---
import { type Lang, t } from "../i18n";
interface Props { lang: Lang }
const { lang } = Astro.props;
const d = t(lang);
---
<section id="install" class="wrap section" aria-labelledby="install-title">
  <h2 id="install-title">{d.install.title}</h2>

  <article class="frame step">
    <div class="frame-title">[1] {d.install.step1}</div>
    <div class="pad">
      <div class="tabs">
        <label><input type="radio" name={`os-${lang}`} value="unix" checked /> {d.install.unix}</label>
        <label><input type="radio" name={`os-${lang}`} value="win" /> {d.install.windows}</label>
      </div>
      <pre class="cmd unix"><code>{d.install.unixCmd}</code></pre>
      <pre class="cmd win"><code>{d.install.windowsCmd}</code></pre>
      <button class="btn" type="button" data-copy={d.install.unixCmd} data-copied-label={d.hero.copied} data-copy-unix>
        {d.install.copy}
      </button>
      <button class="btn win-btn" type="button" data-copy={d.install.windowsCmd} data-copied-label={d.hero.copied} data-copy-win hidden>
        {d.install.copy}
      </button>
    </div>
  </article>

  <article class="frame step">
    <div class="frame-title">[2] {d.install.step2}</div>
    <div class="pad">
      <p>{d.install.configLabel}</p>
      <pre><code>{d.install.configSnippet}</code></pre>
      <button class="btn" type="button" data-copy={d.install.configSnippet} data-copied-label={d.hero.copied}>{d.install.copy}</button>
    </div>
  </article>

  <article class="frame step">
    <div class="frame-title">[3] {d.install.step3}</div>
    <div class="pad">
      <pre><code>{d.install.runInteractive}</code></pre>
      <pre><code>{d.install.runHeadless}</code></pre>
    </div>
  </article>
</section>

<script>
  document.querySelectorAll<HTMLButtonElement>("[data-copy]").forEach((btn) => {
    if (btn.dataset.copyBound) return;
    btn.dataset.copyBound = "1";
    btn.addEventListener("click", async () => {
      const text = btn.dataset.copy ?? "";
      try {
        await navigator.clipboard.writeText(text);
        const prev = btn.textContent;
        btn.textContent = btn.dataset.copiedLabel ?? "Copied";
        setTimeout(() => { btn.textContent = prev; }, 1200);
      } catch { /* ignore */ }
    });
  });

  document.querySelectorAll<HTMLInputElement>('input[type="radio"][name^="os-"]').forEach((input) => {
    input.addEventListener("change", () => {
      const section = input.closest(".step");
      if (!(section instanceof HTMLElement)) return;
      const isWin = input.value === "win";
      section.querySelectorAll(".cmd.unix").forEach((el) => { (el as HTMLElement).hidden = isWin; });
      section.querySelectorAll(".cmd.win").forEach((el) => { (el as HTMLElement).hidden = !isWin; });
      section.querySelectorAll("[data-copy-unix]").forEach((el) => { (el as HTMLElement).hidden = isWin; });
      section.querySelectorAll("[data-copy-win]").forEach((el) => { (el as HTMLElement).hidden = !isWin; });
    });
  });
</script>

<style>
  .step { margin-bottom: 0.9rem; }
  .pad { padding: 0.9rem; }
  .tabs { display: flex; gap: 1rem; margin-bottom: 0.75rem; font-size: 0.85rem; }
  pre {
    margin: 0 0 0.75rem;
    padding: 0.75rem;
    border: 1px solid var(--border);
    overflow-x: auto;
    background: color-mix(in srgb, var(--bg) 80%, black);
    white-space: pre-wrap;
  }
  .cmd.win { display: none; }
  /* show win when corresponding radio checked — handled by JS toggling hidden;
     initial state: unix visible. Also CSS fallback: */
  .step:has(input[value="win"]:checked) .cmd.unix { display: none; }
  .step:has(input[value="win"]:checked) .cmd.win { display: block; }
  .step:has(input[value="unix"]:checked) .cmd.win { display: none; }
  .step:has(input[value="unix"]:checked) .cmd.unix { display: block; }
</style>
```

Note: Prefer CSS `:has()` for tab visibility; keep JS mainly for copy + syncing which copy button is visible.

- [ ] **Step 2: `ThemePreview.astro`**

Static swatches for `retro`, `nord`, `brutal`, `dark` from `theme.rs`:

| Theme | bg | fg | accent | success |
|-------|----|----|--------|---------|
| retro | `#0f0c06` | `#ffb432` | `#ffd250` | `#c8ff50` |
| nord | `#2e3440` | `#d8dee9` | `#88c0d0` | `#a3be8c` |
| brutal | `#ffffff` | `#141414` | `#ffdd57` | `#aaf096` |
| dark | `#000000` | `#ffffff` | `#00ffff` | `#00ff00` |

```astro
---
import { type Lang, t } from "../i18n";
interface Props { lang: Lang }
const { lang } = Astro.props;
const d = t(lang);
const themes = [
  { name: "retro", bg: "#0f0c06", fg: "#ffb432", accent: "#ffd250", success: "#c8ff50" },
  { name: "nord", bg: "#2e3440", fg: "#d8dee9", accent: "#88c0d0", success: "#a3be8c" },
  { name: "brutal", bg: "#ffffff", fg: "#141414", accent: "#ffdd57", success: "#aaf096" },
  { name: "dark", bg: "#000000", fg: "#ffffff", accent: "#00ffff", success: "#00ff00" },
];
---
<section class="wrap section" aria-labelledby="themes-title">
  <h2 id="themes-title">{d.themes.title}</h2>
  <p class="sub">{d.themes.subtitle}</p>
  <div class="row">
    {themes.map((th) => (
      <figure class="frame swatch" style={`--s-bg:${th.bg};--s-fg:${th.fg};--s-accent:${th.accent};--s-ok:${th.success}`}>
        <div class="frame-title">{th.name}</div>
        <div class="chips" aria-hidden="true">
          <span style="background:var(--s-bg)"></span>
          <span style="background:var(--s-fg)"></span>
          <span style="background:var(--s-accent)"></span>
          <span style="background:var(--s-ok)"></span>
        </div>
        <figcaption>bg · fg · accent · ok</figcaption>
      </figure>
    ))}
  </div>
</section>
<style>
  .sub { margin: -0.5rem 0 1rem; font-size: 0.9rem; opacity: 0.9; }
  .row {
    display: grid;
    gap: 0.75rem;
    grid-template-columns: repeat(4, minmax(0, 1fr));
  }
  .chips {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.35rem;
    padding: 0.75rem;
  }
  .chips span {
    display: block;
    height: 2rem;
    border: 1px solid var(--border);
  }
  figcaption {
    padding: 0 0.75rem 0.75rem;
    font-size: 0.7rem;
    opacity: 0.75;
  }
  @media (max-width: 800px) {
    .row { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  }
</style>
```

- [ ] **Step 3: Commit**

```bash
git add website/src/components/InstallSteps.astro website/src/components/ThemePreview.astro
git commit -m "$(cat <<'EOF'
feat(website): add install steps and theme preview

EOF
)"
```

---

### Task 8: Wire EN + ZH pages

**Files:**
- Modify: `website/src/pages/index.astro`
- Create: `website/src/pages/zh/index.astro`

- [ ] **Step 1: Replace `website/src/pages/index.astro`**

```astro
---
import Base from "../layouts/Base.astro";
import TerminalHero from "../components/TerminalHero.astro";
import ValueProps from "../components/ValueProps.astro";
import FeatureGrid from "../components/FeatureGrid.astro";
import CompareTable from "../components/CompareTable.astro";
import InstallSteps from "../components/InstallSteps.astro";
import ThemePreview from "../components/ThemePreview.astro";
import SiteFooter from "../components/SiteFooter.astro";
---
<Base lang="en">
  <TerminalHero lang="en" />
  <ValueProps lang="en" />
  <FeatureGrid lang="en" />
  <CompareTable lang="en" />
  <InstallSteps lang="en" />
  <ThemePreview lang="en" />
  <SiteFooter lang="en" />
</Base>
```

- [ ] **Step 2: Create `website/src/pages/zh/index.astro`**

Same imports with `lang="zh"` (adjust relative paths: `../../layouts/...`, `../../components/...`).

- [ ] **Step 3: Build and spot-check HTML**

```bash
cd website && npm run build
test -f dist/index.html && test -f dist/zh/index.html
rg -n "tact-ui headless|为什么选 tact|Quick Start|快速开始" dist/index.html dist/zh/index.html
```

Expected: both pages exist; EN has English strings, ZH has Chinese values title.

- [ ] **Step 4: Local preview (manual)**

```bash
cd website && npm run preview
```

Open `http://localhost:4321/` and `/zh`. Check: typewriter loops, copy install, OS tabs, mobile width.

- [ ] **Step 5: Commit**

```bash
git add website/src/pages
git commit -m "$(cat <<'EOF'
feat(website): wire EN and ZH landing pages

EOF
)"
```

---

### Task 9: CI deploy workflow + root README link

**Files:**
- Create: `.github/workflows/website.yml`
- Modify: `README.md` (add website link near top badges / Quick Start)

- [ ] **Step 1: Create `.github/workflows/website.yml`**

Use Cloudflare Pages GitHub Action. Secrets required in repo settings (document in commit message / plan; do not invent values):

- `CLOUDFLARE_API_TOKEN`
- `CLOUDFLARE_ACCOUNT_ID`

```yaml
name: website

on:
  push:
    branches: [main]
    paths:
      - "website/**"
      - ".github/workflows/website.yml"
  pull_request:
    paths:
      - "website/**"
      - ".github/workflows/website.yml"

permissions:
  contents: read
  deployments: write

jobs:
  build:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: website
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
          cache: npm
          cache-dependency-path: website/package-lock.json
      - run: npm ci
      - run: npm run build
      - uses: actions/upload-artifact@v4
        with:
          name: website-dist
          path: website/dist

  deploy:
    if: github.event_name == 'push' && github.ref == 'refs/heads/main'
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          name: website-dist
          path: dist
      - name: Publish to Cloudflare Pages
        uses: cloudflare/pages-action@v1
        with:
          apiToken: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          accountId: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
          projectName: tact
          directory: dist
          gitHubToken: ${{ secrets.GITHUB_TOKEN }}
```

If Cloudflare secrets are not ready yet: keep the `build` job green; leave `deploy` present so enabling is config-only. Document DNS: CNAME `tact` → `<project>.pages.dev` on `0x81.hk`.

- [ ] **Step 2: Add README link**

Near the top nav line in `README.md`, add:

```markdown
<a href="https://tact.0x81.hk"><strong>Website</strong></a> ·
```

- [ ] **Step 3: Ensure lockfile**

```bash
cd website && npm install --package-lock-only
# or npm install if lock missing
test -f package-lock.json
```

- [ ] **Step 4: Final verify**

```bash
cd website && npm ci && npm run build
```

Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/website.yml README.md website/package-lock.json
git commit -m "$(cat <<'EOF'
ci(website): add Cloudflare Pages workflow and README link

EOF
)"
```

---

### Task 10: Spec coverage checklist (executor)

Before declaring done, verify against `docs/superpowers/specs/2026-07-13-tact-website-design.md`:

- [ ] Landing only (no docs host, no dashboard, no auth)
- [ ] Domain / canonical / OG use `https://tact.0x81.hk`
- [ ] Routes `/` and `/zh`
- [ ] Retro palette tokens match TUI
- [ ] Hero typewriter loop + reduced-motion fallback
- [ ] Four value props, features, comparison (5 products), install 3 steps, 4 theme swatches (static)
- [ ] Copy buttons for install
- [ ] No third-party analytics
- [ ] CI builds `website/`; deploy gated on secrets

Manual DNS (human, not code): create CNAME `tact.0x81.hk` → Cloudflare Pages project hostname; attach custom domain in Pages settings.

---

## Self-review (plan author)

| Spec item | Task |
|-----------|------|
| Astro under `website/` | Task 1 |
| Retro tokens / CRT / fonts | Task 2 |
| EN + zh dictionaries | Task 3 |
| Layout / lang / SEO | Task 4 |
| Typewriter hero + install CTA | Task 5 |
| Values / features / compare | Task 6 |
| Install steps + theme preview | Task 7 |
| Pages wired | Task 8 |
| Cloudflare + README | Task 9 |
| Non-goals respected | Task 10 |

No TBD placeholders. Component prop type is consistently `Lang`. Install commands match README.
