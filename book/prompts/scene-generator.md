# Scene Generator Prompt

Use this prompt with any LLM (Cursor, Claude, ChatGPT, etc.) to turn a tutorial chapter into a video scene plan.

---

## System

You are a technical video script writer. Convert markdown tutorials into structured scene JSON for an automated slide + narration + video pipeline. Keep narration conversational, concise, and accurate for a developer audience. Do not invent APIs, file paths, or protocol methods that are not in the source document.

---

## User (template — fill in `{{CHAPTER}}` and paste markdown)

Convert the following tutorial chapter into scene JSON.

**Chapter id:** `{{CHAPTER}}`  
**Output path:** `book/output/{{CHAPTER}}/scenes.json`

**Rules:**

1. Split content into **6–12 scenes**, each **30–90 seconds** of narration (~75–200 words per scene).
2. One main idea per scene. Use section headings in the source as scene boundaries when possible.
3. `narration` is spoken voiceover text (English). No markdown in narration.
4. `bullets` are short on-screen bullet points (max 5 per scene). Can be empty array.
5. `code` is optional: include only when the source has a code or JSON example worth showing. Use plain text, not fenced blocks.
6. `duration_hint_sec` is estimated narration length at ~140 words/minute.
7. Preserve technical terms exactly: method names (`tools/list`, `initialize`), file paths, tool name prefixes (`mcp__`).

**JSON schema:**

```json
{
  "title": "string — episode title",
  "chapter": "string — same as chapter id",
  "source": "string — path to source markdown",
  "scenes": [
    {
      "id": "01",
      "title": "string — slide title",
      "narration": "string — full voiceover script",
      "bullets": ["string"],
      "code": "string or null",
      "duration_hint_sec": 60
    }
  ]
}
```

Output **only** valid JSON. No commentary before or after.

**Source markdown:**

```
(paste book/{{CHAPTER}}.md content here)
```

---

## After the LLM responds

1. Save the JSON to `book/output/<chapter>/scenes.json`
2. Run the pipeline:

```bash
./book/scripts/generate.sh <chapter> --all
```

Or step by step:

```bash
./book/scripts/generate.sh mcp --marp      # slides
./book/scripts/generate.sh mcp --tts       # narration (needs OPENAI_API_KEY)
./book/scripts/generate.sh mcp --video     # assemble MP4
```

---

## Quality checklist (human review, ~5 min)

- [ ] Protocol method names spelled correctly
- [ ] File paths match the repo (`crates/tact/src/mcp/mod.rs`, etc.)
- [ ] No scene longer than ~200 words narration
- [ ] JSON validates: `./book/scripts/generate.sh mcp --validate`
