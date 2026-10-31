// Dump the macOS accessibility tree of a running app.
//
//   osascript -l JavaScript scripts/ax-dump.js ["Tact Desktop"]
//
// Used by docs/design/tact-desktop/visual-verification.md to prove that every
// desktop entry opens a real, labelled surface at runtime.

function run(argv) {
  const processName = argv[0] || 'tact-desktop';
  const se = Application('System Events');
  const proc = se.processes.byName(processName);
  const out = [];

  function walk(el, depth) {
    if (depth > 14) return;
    let role = '';
    let name = '';
    try { role = el.role(); } catch (e) {}
    try { name = el.name(); } catch (e) {}
    if (name === null) name = '';
    out.push('  '.repeat(depth) + role + ' | ' + name);
    let kids = [];
    try { kids = el.uiElements(); } catch (e) {}
    for (const kid of kids) walk(kid, depth + 1);
  }

  walk(proc, 0);
  return out.join('\n');
}
