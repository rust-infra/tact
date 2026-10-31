// Press one accessibility element by name in a running app.
//
//   osascript -l JavaScript scripts/ax-press.js "Open Command Center" ["Tact Desktop"]
//
// Used by docs/design/tact-desktop/visual-verification.md to walk every
// desktop entry at runtime. It is the computer-use substitute for a click when
// the harness cannot grant Screen Recording to the calling terminal.

function run(argv) {
  const wanted = argv[0];
  if (!wanted) return 'usage: ax-press.js <accessible name> [process name]';
  const processName = argv[1] || 'tact-desktop';
  const se = Application('System Events');
  const proc = se.processes.byName(processName);

  function walk(el, depth) {
    if (depth > 14) return null;
    let name = '';
    try { name = el.name(); } catch (e) {}
    if (name === wanted) return el;
    let kids = [];
    try { kids = el.uiElements(); } catch (e) {}
    for (const kid of kids) {
      const found = walk(kid, depth + 1);
      if (found) return found;
    }
    return null;
  }

  const target = walk(proc, 0);
  if (!target) return 'not found: ' + wanted;
  target.actions.byName('AXPress').perform();
  return 'pressed: ' + wanted;
}
