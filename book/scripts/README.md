# Book Video Pipeline

Automated workflow: **tutorial markdown → scene JSON → slides → TTS → MP4**.

## Quick start

```bash
# 1. Bootstrap scenes (example) or generate with LLM (recommended)
./book/scripts/generate.sh mcp --init

# 2. Full pipeline (needs marp, ffmpeg, OPENAI_API_KEY)
export OPENAI_API_KEY=sk-...
./book/scripts/generate.sh mcp --all
```

Output: `book/output/mcp/mcp.mp4`

## Recommended AI workflow

```
book/08_chapter_mcp.md
    │
    ▼  LLM + book/prompts/scene-generator.md
book/output/mcp/scenes.json
    │
    ▼  generate.sh --marp
book/output/mcp/deck.marp.md + slides/*.png
    │
    ▼  generate.sh --tts
book/output/mcp/audio/scene-*.mp3
    │
    ▼  generate.sh --video
book/output/mcp/mcp.mp4
    │
    ▼  ~5 min human QA (terms, timing, thumbnail)
publish
```

### Step 1 — Generate `scenes.json` with an LLM

1. Open `book/prompts/scene-generator.md`
2. Replace `{{CHAPTER}}` with `mcp`
3. Paste the contents of `book/08_chapter_mcp.md`
4. Save the JSON response to `book/output/mcp/scenes.json`

Or bootstrap from the example:

```bash
./book/scripts/generate.sh mcp --init
```

### Step 2 — Run the pipeline

| Command | Action |
|---------|--------|
| `--validate` | Check JSON with `jq` |
| `--marp` | Build Marp deck + PNG slides |
| `--tts` | OpenAI TTS → `audio/scene-*.mp3` |
| `--video` | FFmpeg concat → `<chapter>.mp4` |
| `--all` | All of the above |

## Dependencies

| Tool | Install | Used for |
|------|---------|----------|
| `jq` | `dnf install jq` / `apt install jq` | JSON |
| `@marp-team/marp-cli` | `npm i -g @marp-team/marp-cli` | Slides |
| `ffmpeg` | `dnf install ffmpeg` | Video |
| `curl` | usually preinstalled | TTS API |

### TTS environment

```bash
export OPENAI_API_KEY=sk-...
export OPENAI_TTS_MODEL=tts-1      # optional
export OPENAI_TTS_VOICE=onyx       # optional: alloy, echo, fable, onyx, nova, shimmer
```

To use another TTS provider, generate `audio/scene-*.mp3` yourself and run `--video` only.

## Scene JSON format

See `book/output/mcp/scenes.example.json`:

```json
{
  "title": "Episode title",
  "chapter": "mcp",
  "source": "book/08_chapter_mcp.md",
  "scenes": [
    {
      "id": "01",
      "title": "Slide title",
      "narration": "Spoken script…",
      "bullets": ["On-screen point"],
      "code": "optional text block or null",
      "duration_hint_sec": 45
    }
  ]
}
```

## CI / n8n hook (optional)

Trigger on `book/**/*.md` changes:

```bash
# Pseudocode — wire to your automation
./book/scripts/generate.sh mcp --validate
./book/scripts/generate.sh mcp --marp
# TTS + video only on release or manual approval (cost)
```

## CHM (Windows Compiled HTML Help)

Package all 25 chapters as a single `.chm` for offline reading in Windows Help Viewer.

### Prerequisites

| Tool | Platform | Install |
|------|----------|---------|
| **pandoc** | Any | `brew install pandoc` / [pandoc.org](https://pandoc.org/installing.html) |
| **HTML Help Workshop** | Windows only | [Microsoft download](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/htmlhelp/microsoft-html-help-downloads) — provides `hhc.exe` |

> CHM **compilation** requires Windows (`hhc.exe`). On macOS/Linux you can still generate HTML + project files; copy `book/output/chm/` to a Windows machine to compile.

### Viewing on macOS

macOS has **no built-in CHM viewer**. Options:

| Approach | How | Recommended |
|----------|-----|-------------|
| **Browser (best on Mac)** | After `./book/scripts/build-chm.sh`, open `book/output/chm/html/index.html` in Safari/Chrome — full TOC via sidebar links, no Windows needed | Yes |
| **Auto-open** | `./book/scripts/build-chm.sh --open` | Yes |
| **Third-party CHM app** | e.g. iChm, CHM Reader (App Store) — legacy apps; Apple Silicon support varies | Optional |
| **Compile .chm on Windows** | Copy `book/output/chm/` to Windows, run `build-chm.ps1`, transfer `tact-book.chm` back | Only if you need `.chm` specifically |

Mermaid blocks appear as code in HTML too (same as CHM). For a single portable file on Mac, use `pandoc` to PDF instead.

### Build

```bash
# Step 1 — any OS: Markdown → HTML + .hhp / .hhc
./book/scripts/build-chm.sh

# Step 2 — Windows: compile .chm
powershell -File book/scripts/build-chm.ps1
```

One-liner on Windows (Git Bash + PowerShell):

```powershell
bash book/scripts/build-chm.sh
powershell -File book/scripts/build-chm.ps1
```

Output: `book/output/chm/tact-book.chm`

### TOC structure

Matches the mind map groups: Runtime order → Tool families → Off-path → Capstone → Deep topics → Bootstrap & UI.

### Limitations

- Mermaid diagrams render as code blocks (no JS in CHM)
- Interactive `mindmap.html` works; chapter links inside it point to `.html` siblings
- CHM is a legacy Windows format; for cross-platform docs consider PDF (`pandoc` → PDF) instead

## Limitations (v1)

- Slide-based video only — no auto IDE screencast
- TTS defaults to OpenAI; swap by filling `audio/` manually
- Human review still required for protocol accuracy

## Files

| Path | Purpose |
|------|---------|
| `prompts/scene-generator.md` | LLM prompt template |
| `templates/slide.marp.md` | Marp header / theme |
| `scripts/generate.sh` | Main pipeline |
| `output/<chapter>/scenes.json` | Scene plan (LLM output) |
| `output/<chapter>/deck.marp.md` | Generated slides |
| `output/<chapter>/slides/` | PNG per slide |
| `output/<chapter>/audio/` | MP3 per scene |
| `output/<chapter>/<chapter>.mp4` | Final video |
