#!/usr/bin/env bash
# Conformance audit: one checkable command per checklist item.
#
# Each line prints the item, the verdict, and the evidence it was decided on. The
# point is that a reader can re-run this and get the same answer — an audit that
# asserts without naming its check is a restatement of intent.
set -uo pipefail

echo "=== 1. Kernel boundary ==="
bash scripts/check-architecture.sh 2>&1 | sed 's/^/  /'

echo
echo "=== 2. Capability domains have plugin objects ==="
# Two views matter, and the source is the wrong one: a plugin can exist and never be
# installed. The runtime inventory is asserted by
# `every_domain_a_bootstrap_supplies_is_installed`; this lists the ids the source
# declares, for orientation.
ids=$(grep -rho 'PluginId::new("[a-z._]*"' crates/tact/src crates/tui/src crates/tact-ui/src --include=*.rs \
  | sed 's/PluginId::new("//;s/"//' | sort -u)
printf 'declared in source (%d):\n' "$(printf '%s\n' "$ids" | wc -l | tr -d ' ')"
printf '%s\n' "$ids" | sed 's/^/  /'
printf '  runtime inventory: see every_domain_a_bootstrap_supplies_is_installed\n'

echo
echo "=== 3. Tool calls route through the kernel executor ==="
grep -n 'self\.tool_executor' crates/tact/src/agent/tool_dispatch.rs | head -3

echo
echo "=== 4. Ports declared in the kernel ==="
grep -n 'pub trait \(AgentService\|ChatService\|SessionService\)' crates/tact_kernel/src/runtime.rs

echo
echo "=== 5. Reverse-order shutdown ==="
printf '%s\n' "$(grep -c 'fn shutdown_runs_the_reverse_of_the_install_order' crates/tact_kernel/tests/conformance.rs) conformance test(s)"

echo
echo "=== 6. Both frontends are views ==="
grep -rn 'PluginKind::View' crates/tui/src/plugin.rs crates/tact-ui/src/view_plugin.rs | sed 's/^/  /'

echo
echo "=== 7. Commands distributed; the transport is a message enum ==="
python3 - <<'PY'
import re, pathlib
s = pathlib.Path('crates/protocol/src/agent.rs').read_text()
m = re.search(r'pub enum DriverMessage \{', s)
end = s.index("\n}\n", m.start())
names = re.findall(r'^    ([A-Z]\w*)', s[m.start():end], re.M)
print(f"  {len(names)} variants: {' '.join(names)}")
PY

echo
echo "=== 8. Phases shippable (each commit is a gate run) ==="
git log --oneline --first-parent main..HEAD 2>/dev/null | wc -l | sed 's/^/  commits on branch: /'

echo
echo "=== 9. Configuration ownership ==="
python3 - <<'PY'
import pathlib
files = ['crates/tact-ui/src/main.rs','crates/tact-ui/src/interactive.rs',
         'crates/tact-ui/src/session_bootstrap.rs','crates/tact-ui/src/driver.rs',
         'crates/tact/src/plugin/config_plugin.rs','crates/tact/src/agent/mod.rs',
         'crates/tact/src/host.rs','crates/tui/src/handlers/select.rs']
total = 0
for f in files:
    p = pathlib.Path(f)
    if not p.exists(): continue
    s = p.read_text(); cut = s.find('#[cfg(test)]')
    prod = s[:cut] if cut != -1 else s
    hits = [l for l in prod.splitlines()
            if 'config::settings()' in l and not l.strip().startswith('//')]
    if hits:
        print(f"  {f}: {len(hits)}")
        total += len(hits)
print(f"  total production readers: {total}")
PY

echo
echo "=== 10. Dynamic sources filtered ==="
# Implementors, production registration sites, and the retain that applies the filter.
grep -rn 'impl ToolSource for' crates --include=*.rs | sed 's/^/  /'
grep -n 'add_source' crates/tact/src/agent/mod.rs | sed 's/^/  production: /'
grep -n 'filter.allows(&id)' crates/tact_kernel/src/catalog.rs | sed 's/^/  filter at: /'
printf '  CommandSource (design names it, no implementation): %s\n' "$(grep -r 'CommandSource' crates --include=*.rs 2>/dev/null | wc -l | tr -d ' ')"

echo
echo "=== 11. Marketplace behaviour unchanged ==="
# The only change is the D2 deviation: `MarketplaceSource::parse` moved to an
# extension trait so the extension crate keeps it. Assert that the diff is imports
# only, which is what "behaviour unchanged" means here.
base=$(git merge-base main HEAD 2>/dev/null)
diff=$(git diff "$base"..HEAD -- crates/tact/src/plugin/marketplace.rs 2>/dev/null | grep -E '^[+-][^+-]')
if printf '%s' "$diff" | grep -qvE '^[+-](\s*(MARKETPLACE_|Marketplace|PluginStore|validate_marketplace_name|use super|\}|,|MarketplaceSourceExt))'; then
  echo "  UNEXPECTED: marketplace.rs has a non-import change"
else
  echo "  marketplace.rs: import-only change (D2 extension trait); no parser or UX diff"
fi
