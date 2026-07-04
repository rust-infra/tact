#!/usr/bin/env bash
# Doc → scene JSON → Marp slides → TTS audio → MP4
#
# Usage:
#   ./book/scripts/generate.sh <chapter> [--all|--validate|--marp|--tts|--video]
#
# Examples:
#   ./book/scripts/generate.sh mcp --all
#   ./book/scripts/generate.sh mcp --marp
#   OPENAI_API_KEY=sk-... ./book/scripts/generate.sh mcp --tts
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BOOK="$ROOT/book"
SCRIPTS="$BOOK/scripts"

# shellcheck source=lib/common.sh
source "$SCRIPTS/lib/common.sh"

usage() {
  cat <<'EOF'
Usage: generate.sh <chapter> [mode]

Modes (default: --all):
  --all       validate → marp → tts → video
  --validate  check scenes.json with jq
  --marp      scenes.json → deck.marp.md → PNG slides
  --tts       scenes.json → audio/*.mp3 (OpenAI TTS, needs OPENAI_API_KEY)
  --video     PNG + audio → output/<chapter>.mp4
  --init      copy scenes.example.json → scenes.json if missing
  --help

Prerequisites:
  jq          JSON parsing
  marp        @marp-team/marp-cli for slides (npm i -g @marp-team/marp-cli)
  ffmpeg      video assembly
  curl        OpenAI TTS (--tts)

Workflow:
  1. Generate scenes.json with LLM using book/prompts/scene-generator.md
  2. ./book/scripts/generate.sh mcp --init   # optional bootstrap
  3. ./book/scripts/generate.sh mcp --all
EOF
}

chapter="${1:-}"
mode="${2:---all}"

if [[ -z "$chapter" || "$chapter" == "--help" || "$mode" == "--help" ]]; then
  usage
  exit 0
fi

SOURCE_MD="$BOOK/chapter_${chapter}.md"
OUT_DIR="$BOOK/output/${chapter}"
SCENES="$OUT_DIR/scenes.json"
EXAMPLE="$OUT_DIR/scenes.example.json"
DECK="$OUT_DIR/deck.marp.md"
SLIDES_DIR="$OUT_DIR/slides"
AUDIO_DIR="$OUT_DIR/audio"
VIDEO_OUT="$OUT_DIR/${chapter}.mp4"

require_file() {
  if [[ ! -f "$1" ]]; then
    log_error "Missing file: $1"
    exit 1
  fi
}

cmd_validate() {
  require_cmd jq
  require_file "$SCENES"
  jq -e '.title and .chapter and (.scenes | length > 0)' "$SCENES" >/dev/null
  jq -e '.scenes[] | .id and .title and .narration' "$SCENES" >/dev/null
  local count
  count="$(jq '.scenes | length' "$SCENES")"
  log_ok "scenes.json valid ($count scenes)"
}

cmd_init() {
  mkdir -p "$OUT_DIR"
  if [[ -f "$SCENES" ]]; then
    log_warn "Already exists: $SCENES"
    return 0
  fi
  if [[ -f "$EXAMPLE" ]]; then
    cp "$EXAMPLE" "$SCENES"
    log_ok "Copied $EXAMPLE → $SCENES"
    log_warn "Replace with LLM-generated scenes from book/prompts/scene-generator.md"
  else
    log_error "No example scenes at $EXAMPLE"
    log_info "Generate scenes.json using book/prompts/scene-generator.md"
    exit 1
  fi
}

cmd_marp() {
  require_cmd jq
  require_file "$SCENES"
  require_file "$BOOK/templates/slide.marp.md"

  mkdir -p "$SLIDES_DIR"
  cp "$BOOK/templates/slide.marp.md" "$DECK"

  local title
  title="$(jq -r '.title' "$SCENES")"

  {
    echo ""
    echo "<!-- ${title} -->"
    echo ""
  } >>"$DECK"

  local n
  n="$(jq '.scenes | length' "$SCENES")"
  for i in $(seq 0 $((n - 1))); do
    local scene_title narration bullets code
    scene_title="$(jq -r ".scenes[$i].title" "$SCENES")"
    narration="$(jq -r ".scenes[$i].narration" "$SCENES")"
    bullets="$(jq -r ".scenes[$i].bullets[]? // empty" "$SCENES")"
    code="$(jq -r ".scenes[$i].code // empty" "$SCENES")"

    {
      echo ""
      echo "---"
      echo ""
      echo "# ${scene_title}"
      echo ""
      if [[ -n "$bullets" ]]; then
        while IFS= read -r line; do
          [[ -z "$line" ]] && continue
          echo "- ${line}"
        done <<<"$bullets"
        echo ""
      fi
      if [[ -n "$code" && "$code" != "null" ]]; then
        echo '```text'
        echo "$code"
        echo '```'
        echo ""
      fi
      echo "<!-- narration:"
      echo "$narration"
      echo "-->"
    } >>"$DECK"
  done

  log_ok "Wrote $DECK"

  if marp_render "$DECK" "$SLIDES_DIR"; then
    log_ok "Rendered PNG slides → $SLIDES_DIR/"
  else
    log_warn "marp CLI not found — install: npm i -g @marp-team/marp-cli"
    log_info "Deck ready at $DECK (render manually or install marp / use npx)"
  fi
}

cmd_tts() {
  require_cmd jq
  require_cmd curl
  require_file "$SCENES"

  if [[ -z "${OPENAI_API_KEY:-}" ]]; then
    log_error "OPENAI_API_KEY is not set"
    log_info "Export key or add to env; optional: OPENAI_TTS_MODEL, OPENAI_TTS_VOICE"
    exit 1
  fi

  local model voice
  model="${OPENAI_TTS_MODEL:-tts-1}"
  voice="${OPENAI_TTS_VOICE:-onyx}"

  mkdir -p "$AUDIO_DIR"
  local n
  n="$(jq '.scenes | length' "$SCENES")"

  for i in $(seq 0 $((n - 1))); do
    local id narration out
    id="$(jq -r ".scenes[$i].id" "$SCENES")"
    narration="$(jq -r ".scenes[$i].narration" "$SCENES")"
    out="$AUDIO_DIR/scene-${id}.mp3"

    log_info "TTS scene-${id}..."
    curl -sS https://api.openai.com/v1/audio/speech \
      -H "Authorization: Bearer $OPENAI_API_KEY" \
      -H "Content-Type: application/json" \
      -d "$(jq -n \
        --arg model "$model" \
        --arg voice "$voice" \
        --arg input "$narration" \
        '{model: $model, voice: $voice, input: $input, response_format: "mp3"}')" \
      --output "$out"

    if [[ ! -s "$out" ]]; then
      log_error "TTS failed for scene-${id}"
      exit 1
    fi
    log_ok "Wrote $out"
  done
}

slide_png_for_id() {
  local id="$1"
  local padded
  padded="$(printf "%02d" "$((10#$id))")"
  # Marp names: deck.001.png, deck.002.png, … (1-based index)
  local idx
  idx="$((10#$id))"
  local candidate="$SLIDES_DIR/deck.$(printf '%03d' "$idx").png"
  if [[ -f "$candidate" ]]; then
    echo "$candidate"
    return 0
  fi
  # Fallback: match by scene order in directory listing
  local files=("$SLIDES_DIR"/*.png)
  if [[ -f "${files[$((idx - 1))]:-}" ]]; then
    echo "${files[$((idx - 1))]}"
    return 0
  fi
  echo ""
}

cmd_video() {
  require_cmd jq
  require_cmd ffmpeg
  require_file "$SCENES"

  if [[ ! -d "$SLIDES_DIR" ]] || ! compgen -G "$SLIDES_DIR/*.png" >/dev/null; then
    log_error "No PNG slides in $SLIDES_DIR — run: generate.sh $chapter --marp"
    exit 1
  fi
  if [[ ! -d "$AUDIO_DIR" ]] || ! compgen -G "$AUDIO_DIR/*.mp3" >/dev/null; then
    log_error "No audio in $AUDIO_DIR — run: generate.sh $chapter --tts"
    exit 1
  fi

  local work="$OUT_DIR/_video_parts"
  rm -rf "$work"
  mkdir -p "$work"

  local n concat_list
  n="$(jq '.scenes | length' "$SCENES")"
  concat_list="$work/concat.txt"
  : >"$concat_list"

  for i in $(seq 0 $((n - 1))); do
    local id slide audio part
    id="$(jq -r ".scenes[$i].id" "$SCENES")"
    slide="$(slide_png_for_id "$id")"
    audio="$AUDIO_DIR/scene-${id}.mp3"
    part="$work/part-${id}.mp4"

    if [[ -z "$slide" || ! -f "$slide" ]]; then
      log_error "Missing slide for scene id=$id (looked in $SLIDES_DIR)"
      exit 1
    fi
    if [[ ! -f "$audio" ]]; then
      log_error "Missing audio: $audio"
      exit 1
    fi

    log_info "Rendering part scene-${id}..."
    ffmpeg -y -loglevel error \
      -loop 1 -i "$slide" -i "$audio" \
      -c:v libx264 -tune stillimage -pix_fmt yuv420p \
      -c:a aac -b:a 192k -shortest \
      "$part"

    printf "file '%s'\n" "$part" >>"$concat_list"
  done

  ffmpeg -y -loglevel error -f concat -safe 0 -i "$concat_list" -c copy "$VIDEO_OUT"
  log_ok "Wrote $VIDEO_OUT"
}

run_mode() {
  case "$1" in
    --init) cmd_init ;;
    --validate) cmd_validate ;;
    --marp) cmd_marp ;;
    --tts) cmd_tts ;;
    --video) cmd_video ;;
    --all)
      cmd_init
      cmd_validate
      cmd_marp
      cmd_tts
      cmd_video
      ;;
    *)
      log_error "Unknown mode: $1"
      usage
      exit 1
      ;;
  esac
}

if [[ ! -f "$SOURCE_MD" ]]; then
  log_error "Tutorial not found: $SOURCE_MD"
  exit 1
fi

mkdir -p "$OUT_DIR"
run_mode "$mode"
