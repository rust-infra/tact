#!/usr/bin/env bash
# Build Tact Book as Windows CHM (Compiled HTML Help).
#
# Step 1 (any OS): convert Markdown → HTML + .hhp / .hhc project files.
# Step 2 (Windows): compile with HTML Help Workshop (hhc.exe).
#
# Usage:
#   ./book/scripts/build-chm.sh           # HTML + project files only
#   ./book/scripts/build-chm.sh --open    # build + open index.html (macOS/Linux)
#   ./book/scripts/build-chm.sh --compile # also run hhc.exe when available
#
# Windows compile (after step 1):
#   powershell -File book/scripts/build-chm.ps1

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BOOK="${ROOT}/book"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=book/scripts/lib/common.sh
source "${SCRIPT_DIR}/lib/common.sh"

OUT="${BOOK}/output/chm"
HTML="${OUT}/html"
CSS="${SCRIPT_DIR}/chm/chm.css"
CHM_NAME="tact-book.chm"
TITLE="Tact Agent Development Tutorials"
COMPILE=false
OPEN=false

for arg in "$@"; do
  case "$arg" in
    --compile) COMPILE=true ;;
    --open) OPEN=true ;;
    -h|--help)
      sed -n '2,14p' "$0"
      exit 0
      ;;
    *) log_error "Unknown argument: $arg"; exit 1 ;;
  esac
done

require_cmd pandoc

mkdir -p "${HTML}"

log_info "Converting Markdown → HTML in ${HTML}"

convert_md() {
  local src="$1"
  local out_name="$2"
  pandoc "${src}" \
    --from=gfm \
    --to=html5 \
    --standalone \
    --embed-resources \
    --css="${CSS}" \
    --metadata title="${TITLE}" \
    --lua-filter="${SCRIPT_DIR}/chm/fix-links.lua" \
    -o "${HTML}/${out_name}"
  log_ok "${out_name}"
}

convert_md "${BOOK}/index.md" "index.html"

if [[ -f "${BOOK}/mindmap.png" ]]; then
  cp "${BOOK}/mindmap.png" "${HTML}/mindmap.png"
fi
convert_md "${BOOK}/mindmap.md" "mindmap-overview.html"

for ch in "${BOOK}"/[0-9][0-9]_chapter_*.md; do
  [[ -f "$ch" ]] || continue
  base="$(basename "${ch}" .md)"
  convert_md "$ch" "${base}.html"
done

cp "${BOOK}/mindmap.html" "${HTML}/mindmap.html"
log_ok "mindmap.html (embed + standalone)"

# Fix any remaining .md hrefs (interactive mindmap links chapters as .md)
for f in "${HTML}"/*.html; do
  [[ -f "$f" ]] || continue
  sed 's/href="\([^"]*\)\.md"/href="\1.html"/g; s/href="\([^"]*\)\.md#/"href="\1.html#/g' "$f" > "${f}.tmp"
  mv "${f}.tmp" "$f"
done
log_ok "internal links → .html"

# ── Table of contents (.hhc) ────────────────────────────────────────────────

HHC="${OUT}/tact-book.hhc"

{
  cat <<'EOF'
<!DOCTYPE HTML PUBLIC "-//IETF//DTD HTML//EN">
<HTML>
<HEAD>
<meta http-equiv="Content-Type" content="text/html; charset=UTF-8">
</HEAD>
<BODY>
<UL>
EOF

  emit_entry() {
    printf '  <LI> <OBJECT type="text/sitemap">\n'
    printf '        <param name="Name" value="%s">\n' "$1"
    printf '        <param name="Local" value="html/%s">\n' "$2"
    printf '        </OBJECT>\n'
  }

  emit_group_open() {
    printf '  <LI> <OBJECT type="text/sitemap">\n'
    printf '        <param name="Name" value="%s">\n' "$1"
    printf '        </OBJECT>\n'
    printf '  <UL>\n'
  }

  emit_group_close() { printf '  </UL>\n'; }

  emit_entry "Home" "index.html"

  emit_group_open "Mind Map"
  emit_entry "Mind Map (interactive)" "mindmap.html"
  emit_entry "Mind Map (overview)" "mindmap-overview.html"
  emit_group_close

  chapter_entry() {
    emit_entry "Ch $1 — $2" "$3.html"
  }

  emit_group_open "Runtime order (Ch 1–11)"
  chapter_entry "01" "Store and Persistence" "01_chapter_store"
  chapter_entry "02" "Skill Registry" "02_chapter_skill"
  chapter_entry "03" "Persistent Memory" "03_chapter_memory"
  chapter_entry "04" "System Prompt" "04_chapter_prompt"
  chapter_entry "05" "Context Compaction" "05_chapter_compact"
  chapter_entry "06" "Error Recovery" "06_chapter_recovery"
  chapter_entry "07" "Tool System" "07_chapter_tool"
  chapter_entry "08" "MCP Integration" "08_chapter_mcp"
  chapter_entry "09" "Agent Lifecycle Hooks" "09_chapter_hook"
  chapter_entry "10" "Permission Model" "10_chapter_permission"
  chapter_entry "11" "Tasks and Tool Scheduling" "11_chapter_task"
  emit_group_close

  emit_group_open "Tool families (Ch 12–15)"
  chapter_entry "12" "Subagents" "12_chapter_subagent"
  chapter_entry "13" "Background Tasks" "13_chapter_background"
  chapter_entry "14" "Team Coordination" "14_chapter_team"
  chapter_entry "15" "Worktree Lanes" "15_chapter_worktree"
  emit_group_close

  emit_group_open "Off-path systems (Ch 16–17)"
  chapter_entry "16" "Cron Scheduling" "16_chapter_cron"
  chapter_entry "17" "Desktop Notifications" "17_chapter_notify"
  emit_group_close

  emit_group_open "Capstone (Ch 18)"
  chapter_entry "18" "Agent Main Loop" "18_chapter_agent_loop"
  emit_group_close

  emit_group_open "Deep topics (Ch 19–20)"
  chapter_entry "19" "Persistent Task Manager" "19_chapter_persistent_tasks"
  chapter_entry "20" "LSP Code Intelligence" "20_chapter_lsp"
  emit_group_close

  emit_group_open "Bootstrap & UI (Ch 21–25)"
  chapter_entry "21" "Configuration" "21_chapter_config"
  chapter_entry "22" "LLM Providers" "22_chapter_llm"
  chapter_entry "23" "Terminal UI" "23_chapter_tui"
  chapter_entry "24" "Testing Strategy" "24_chapter_testing"
  chapter_entry "25" "Agent–TUI Protocol" "25_chapter_protocol"
  emit_group_close

  cat <<'EOF'
</UL>
</BODY>
</HTML>
EOF
} > "${HHC}"

log_ok "tact-book.hhc"

# ── Project file (.hhp) ─────────────────────────────────────────────────────

HHP="${OUT}/tact-book.hhp"

{
  cat <<EOF
[OPTIONS]
Compatibility=1.1 or later
Compiled file=${CHM_NAME}
Contents file=tact-book.hhc
Default Topic=html/index.html
Title=${TITLE}
Language=0x409 English (United States)

[FILES]
EOF

  while IFS= read -r -d '' f; do
    # Paths in [FILES] are relative to the .hhp directory
    rel="${f#${OUT}/}"
    printf '%s\n' "${rel//\\//}"
  done < <(find "${HTML}" -type f -print0 | sort -z)

} > "${HHP}"

log_ok "tact-book.hhp"

# ── Compile (Windows HTML Help Workshop) ────────────────────────────────────

find_hhc() {
  if command -v hhc >/dev/null 2>&1; then
    command -v hhc
    return 0
  fi
  local win_paths=(
    "/c/Program Files (x86)/HTML Help Workshop/hhc.exe"
    "/c/Program Files/HTML Help Workshop/hhc.exe"
  )
  for p in "${win_paths[@]}"; do
    if [[ -x "$p" ]]; then
      printf '%s\n' "$p"
      return 0
    fi
  done
  return 1
}

if [[ "${COMPILE}" == true ]]; then
  if HHC_EXE="$(find_hhc)"; then
    log_info "Compiling ${CHM_NAME} with ${HHC_EXE}"
    (cd "${OUT}" && "${HHC_EXE}" tact-book.hhp)
    if [[ -f "${OUT}/${CHM_NAME}" ]]; then
      log_ok "${OUT}/${CHM_NAME}"
    else
      log_warn "hhc finished but ${CHM_NAME} not found (check hhc log above)"
      exit 1
    fi
  else
    log_error "hhc.exe not found. Install HTML Help Workshop on Windows, then run:"
    log_error "  powershell -File book/scripts/build-chm.ps1"
    exit 1
  fi
else
  log_info "Project ready: ${OUT}/"
  log_info "macOS/Linux: open ${HTML}/index.html in a browser (or run with --open)"
  log_info "Windows CHM:   powershell -File book/scripts/build-chm.ps1"
  log_info "Or:            ./book/scripts/build-chm.sh --compile  (when hhc.exe is in PATH)"
fi

if [[ "${OPEN}" == true ]]; then
  INDEX="${HTML}/index.html"
  if [[ ! -f "${INDEX}" ]]; then
    log_error "Missing ${INDEX}"
    exit 1
  fi
  log_info "Opening ${INDEX}"
  if command -v open >/dev/null 2>&1; then
    open "${INDEX}"
  elif command -v xdg-open >/dev/null 2>&1; then
    xdg-open "${INDEX}"
  else
    log_warn "No open/xdg-open — browse to file://${INDEX}"
  fi
fi
