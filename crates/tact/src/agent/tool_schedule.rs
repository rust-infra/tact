//! Conflict-aware scheduling for parallel tool execution.
//!
//! Within a single LLM turn the model may return several tool calls. Calls
//! that touch disjoint resources can run concurrently; calls that touch the
//! same file (where at least one writes) must stay ordered. MCP tools on the
//! same server serialize; different MCP servers may run in parallel. Genuine data
//! dependencies between tools only ever appear *across* turns — the agent loop
//! feeds each turn's tool results back into the next request — so the only
//! intra-turn ordering we must preserve is a read/write or write/write
//! conflict on the same workspace path.
//!
//! [`schedule_waves`] turns a batch of tool calls into sequential *waves*: each
//! wave runs in parallel, waves run in order. Conflicting tools (`i` before
//! `j`) always land in different waves with `wave[i] < wave[j]`, so their
//! original relative order is preserved while independent calls overlap.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The workspace files a tool reads and/or writes, used to decide whether two
/// tool calls in the same turn may run concurrently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolResources {
    pub reads: Vec<PathBuf>,
    pub writes: Vec<PathBuf>,
    /// `true` for tools whose effects we cannot scope (bash, MCP, subagents,
    /// patch application, state mutations, …). A barrier conflicts with every
    /// other call and is therefore always scheduled alone in its own wave —
    /// equivalent to the previous fully-sequential behaviour for that tool.
    pub barrier: bool,
}

impl ToolResources {
    fn barrier() -> Self {
        Self {
            barrier: true,
            ..Default::default()
        }
    }

    /// A tool that touches no workspace file (e.g. `web_search`); it never
    /// conflicts and may run alongside anything.
    fn independent() -> Self {
        Self::default()
    }
}

/// Normalise a tool path argument to an absolute path rooted at `work_dir`.
///
/// We do not `canonicalize` (the target may not exist yet, and we want a pure,
/// filesystem-independent function); a lexical join is enough for the
/// equal/ancestor/descendant overlap test used by [`conflicts`].
fn normalize(work_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        work_dir.join(p)
    }
}

/// Map a single tool call to the workspace resources it touches.
///
/// Unknown tools default to a [barrier](ToolResources::barrier) so that newly
/// added tools never parallelise unsafely without an explicit opt-in here.
pub(crate) fn tool_resources(name: &str, input: &Value, work_dir: &Path) -> ToolResources {
    let single = |key: &str| -> Vec<PathBuf> {
        input
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| normalize(work_dir, s))
            .into_iter()
            .collect()
    };
    let list = |key: &str, item_key: &str| -> Vec<PathBuf> {
        input
            .get(key)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.get(item_key).and_then(|v| v.as_str()))
                    .map(|s| normalize(work_dir, s))
                    .collect()
            })
            .unwrap_or_default()
    };

    match name {
        "read_file" => ToolResources {
            reads: single("path"),
            ..Default::default()
        },
        "batch_read" => ToolResources {
            reads: list("files", "path"),
            ..Default::default()
        },
        // A search/grep has a directory scope; default to the whole workspace
        // when no `path` is given so it correctly conflicts with any write.
        "search_code" => {
            let scope = input
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| normalize(work_dir, s))
                .unwrap_or_else(|| work_dir.to_path_buf());
            ToolResources {
                reads: vec![scope],
                ..Default::default()
            }
        }
        "write_file" => ToolResources {
            writes: single("path"),
            ..Default::default()
        },
        "batch_edit" => ToolResources {
            writes: list("edits", "file_path"),
            ..Default::default()
        },
        // Side-effect-free tools that touch no workspace file: safe to run
        // concurrently with anything.
        "web_search" | "web_fetch" | "lsp" | "sleep" => ToolResources::independent(),
        name if name.starts_with("mcp__") => mcp_tool_resources(name),
        // bash, apply_patch (multi-file diff), task/subagent, worktree_run,
        // and all state mutations have effects we cannot scope.
        _ => ToolResources::barrier(),
    }
}

/// MCP tools on the same server serialize; different servers may run in parallel.
fn mcp_tool_resources(name: &str) -> ToolResources {
    let Some(rest) = name.strip_prefix("mcp__") else {
        return ToolResources::barrier();
    };
    let Some((server, tool)) = rest.rsplit_once("__") else {
        return ToolResources::barrier();
    };
    if server.is_empty() || tool.is_empty() {
        return ToolResources::barrier();
    }
    ToolResources {
        writes: vec![PathBuf::from(format!("__mcp__{server}"))],
        ..Default::default()
    }
}

/// Two paths overlap when they are equal or one contains the other, so a write
/// to `src/foo.rs` is treated as conflicting with a read of the `src/` scope.
fn overlap(a: &Path, b: &Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

/// Two tool calls conflict when a barrier is involved, or when one writes a
/// path that the other reads or writes (classic read/write & write/write
/// hazards). Two pure reads never conflict.
fn conflicts(a: &ToolResources, b: &ToolResources) -> bool {
    if a.barrier || b.barrier {
        return true;
    }
    let writes_hit = |writes: &[PathBuf], other: &ToolResources| {
        writes.iter().any(|w| {
            other
                .reads
                .iter()
                .chain(other.writes.iter())
                .any(|p| overlap(w, p))
        })
    };
    writes_hit(&a.writes, b) || writes_hit(&b.writes, a)
}

/// Assign each tool a wave index. Tools sharing a wave may run concurrently;
/// waves execute in ascending order. See the module docs for guarantees.
pub(crate) fn schedule_waves(resources: &[ToolResources]) -> Vec<usize> {
    let mut waves = vec![0usize; resources.len()];
    for i in 0..resources.len() {
        let mut wave = 0;
        for j in 0..i {
            if conflicts(&resources[i], &resources[j]) {
                wave = wave.max(waves[j] + 1);
            }
        }
        waves[i] = wave;
    }
    waves
}

/// Group tool indices by wave, preserving ascending index order within each
/// wave. `groups[w]` lists the tools that run together in wave `w`.
pub(crate) fn waves_grouped(resources: &[ToolResources]) -> Vec<Vec<usize>> {
    let assignment = schedule_waves(resources);
    let wave_count = assignment.iter().copied().max().map_or(0, |m| m + 1);
    let mut groups = vec![Vec::new(); wave_count];
    for (idx, wave) in assignment.iter().enumerate() {
        groups[*wave].push(idx);
    }
    groups
}

/// One wave of the schedule, recorded for later analysis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct WaveSummary {
    /// Tool names that ran concurrently in this wave (model order preserved).
    pub tools: Vec<String>,
    /// `true` if this wave was forced solo by a barrier tool (bash/MCP/…).
    pub barrier: bool,
}

/// A compact, serialisable description of how one turn's tool calls were
/// scheduled. Persisted alongside the turn's token usage so a session can be
/// audited later: how many tools ran, how they batched into waves, and the
/// degree of parallelism achieved.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct ToolScheduleSummary {
    /// Number of tools that were cleared to execute (excludes denied/blocked).
    pub total_tools: usize,
    /// Number of sequential waves the tools were split into.
    pub wave_count: usize,
    /// Size of the largest wave (1 means everything ran serially).
    pub max_parallelism: usize,
    /// Per-wave breakdown, in execution order.
    pub waves: Vec<WaveSummary>,
}

/// Build a [`ToolScheduleSummary`] from the tools cleared to run. `names[k]`
/// must correspond to `resources[k]` (both in run order).
pub(crate) fn summarize(names: &[String], resources: &[ToolResources]) -> ToolScheduleSummary {
    let grouped = waves_grouped(resources);
    let waves: Vec<WaveSummary> = grouped
        .iter()
        .map(|wave| WaveSummary {
            tools: wave.iter().map(|&i| names[i].clone()).collect(),
            barrier: wave.iter().any(|&i| resources[i].barrier),
        })
        .collect();
    ToolScheduleSummary {
        total_tools: names.len(),
        wave_count: grouped.len(),
        max_parallelism: grouped.iter().map(Vec::len).max().unwrap_or(0),
        waves,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(reads: &[&str], writes: &[&str]) -> ToolResources {
        ToolResources {
            reads: reads.iter().map(PathBuf::from).collect(),
            writes: writes.iter().map(PathBuf::from).collect(),
            barrier: false,
        }
    }

    #[test]
    fn all_reads_collapse_into_one_wave() {
        let r = vec![res(&["/a"], &[]), res(&["/b"], &[]), res(&["/c"], &[])];
        assert_eq!(schedule_waves(&r), vec![0, 0, 0]);
    }

    #[test]
    fn write_after_read_same_file_serializes() {
        // read A, read B, write A, read C, read A  →  waves 0,0,1,0,2
        let r = vec![
            res(&["/a"], &[]),
            res(&["/b"], &[]),
            res(&[], &["/a"]),
            res(&["/c"], &[]),
            res(&["/a"], &[]),
        ];
        assert_eq!(schedule_waves(&r), vec![0, 0, 1, 0, 2]);
    }

    #[test]
    fn write_write_same_file_serializes() {
        let r = vec![res(&[], &["/a"]), res(&[], &["/a"])];
        assert_eq!(schedule_waves(&r), vec![0, 1]);
    }

    #[test]
    fn disjoint_writes_run_in_parallel() {
        let r = vec![res(&[], &["/a"]), res(&[], &["/b"])];
        assert_eq!(schedule_waves(&r), vec![0, 0]);
    }

    #[test]
    fn read_then_write_same_file_serializes() {
        let r = vec![res(&["/a"], &[]), res(&[], &["/a"])];
        assert_eq!(schedule_waves(&r), vec![0, 1]);
    }

    #[test]
    fn barrier_forces_its_own_wave() {
        // read A, bash(barrier), read B  →  0,1,2
        let r = vec![
            res(&["/a"], &[]),
            ToolResources::barrier(),
            res(&["/b"], &[]),
        ];
        assert_eq!(schedule_waves(&r), vec![0, 1, 2]);
    }

    #[test]
    fn dir_scope_conflicts_with_file_underneath() {
        // search /src, then write /src/foo.rs
        let r = vec![res(&["/src"], &[]), res(&[], &["/src/foo.rs"])];
        assert_eq!(schedule_waves(&r), vec![0, 1]);
    }

    #[test]
    fn independent_tools_never_conflict() {
        let r = vec![
            ToolResources::independent(),
            ToolResources::independent(),
            res(&[], &["/a"]),
        ];
        assert_eq!(schedule_waves(&r), vec![0, 0, 0]);
    }

    #[test]
    fn mcp_tools_on_different_servers_run_in_parallel() {
        let r = vec![
            mcp_tool_resources("mcp__demo__postgres__query"),
            mcp_tool_resources("mcp__demo__echo__ping"),
        ];
        assert_eq!(schedule_waves(&r), vec![0, 0]);
    }

    #[test]
    fn mcp_tools_on_same_server_serialize() {
        let r = vec![
            mcp_tool_resources("mcp__demo__postgres__query"),
            mcp_tool_resources("mcp__demo__postgres__migrate"),
        ];
        assert_eq!(schedule_waves(&r), vec![0, 1]);
    }

    #[test]
    fn grouping_preserves_index_order() {
        let r = vec![res(&["/a"], &[]), res(&[], &["/a"]), res(&["/a"], &[])];
        assert_eq!(waves_grouped(&r), vec![vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn grouping_batches_parallel_indices() {
        let r = vec![
            res(&["/a"], &[]),
            res(&["/b"], &[]),
            res(&[], &["/a"]),
            res(&["/c"], &[]),
        ];
        assert_eq!(waves_grouped(&r), vec![vec![0, 1, 3], vec![2]]);
    }

    #[test]
    fn resources_read_file_path_is_normalized() {
        let r = tool_resources(
            "read_file",
            &serde_json::json!({"path": "src/a.rs"}),
            Path::new("/work"),
        );
        assert_eq!(r.reads, vec![PathBuf::from("/work/src/a.rs")]);
        assert!(r.writes.is_empty() && !r.barrier);
    }

    #[test]
    fn resources_absolute_path_kept() {
        let r = tool_resources(
            "read_file",
            &serde_json::json!({"path": "/abs/a.rs"}),
            Path::new("/work"),
        );
        assert_eq!(r.reads, vec![PathBuf::from("/abs/a.rs")]);
    }

    #[test]
    fn resources_bash_is_barrier() {
        let r = tool_resources(
            "bash",
            &serde_json::json!({"command": "ls"}),
            Path::new("/work"),
        );
        assert!(r.barrier);
    }

    #[test]
    fn resources_unknown_tool_is_barrier() {
        let r = tool_resources("some_new_tool", &serde_json::json!({}), Path::new("/work"));
        assert!(r.barrier);
    }

    #[test]
    fn resources_batch_edit_collects_all_writes() {
        let input = serde_json::json!({
            "edits": [
                {"file_path": "a.rs", "old_string": "x", "new_string": "y"},
                {"file_path": "b.rs", "old_string": "x", "new_string": "y"}
            ]
        });
        let r = tool_resources("batch_edit", &input, Path::new("/work"));
        assert_eq!(
            r.writes,
            vec![PathBuf::from("/work/a.rs"), PathBuf::from("/work/b.rs")]
        );
    }

    #[test]
    fn resources_batch_read_collects_all_reads() {
        let input = serde_json::json!({"files": [{"path": "a.rs"}, {"path": "b.rs"}]});
        let r = tool_resources("batch_read", &input, Path::new("/work"));
        assert_eq!(
            r.reads,
            vec![PathBuf::from("/work/a.rs"), PathBuf::from("/work/b.rs")]
        );
    }

    #[test]
    fn resources_search_defaults_to_workspace_scope() {
        let r = tool_resources(
            "search_code",
            &serde_json::json!({"query": "foo"}),
            Path::new("/work"),
        );
        assert_eq!(r.reads, vec![PathBuf::from("/work")]);
    }

    #[test]
    fn summarize_captures_waves_and_parallelism() {
        // read A, read B, write A  →  wave 0: [readA, readB], wave 1: [writeA]
        let names = vec!["read_file".into(), "read_file".into(), "write_file".into()];
        let resources = vec![res(&["/a"], &[]), res(&["/b"], &[]), res(&[], &["/a"])];
        let summary = summarize(&names, &resources);
        assert_eq!(summary.total_tools, 3);
        assert_eq!(summary.wave_count, 2);
        assert_eq!(summary.max_parallelism, 2);
        assert_eq!(summary.waves[0].tools, vec!["read_file", "read_file"]);
        assert!(!summary.waves[0].barrier);
        assert_eq!(summary.waves[1].tools, vec!["write_file"]);
    }

    #[test]
    fn summarize_marks_barrier_wave() {
        let names = vec!["bash".into()];
        let resources = vec![ToolResources::barrier()];
        let summary = summarize(&names, &resources);
        assert_eq!(summary.wave_count, 1);
        assert!(summary.waves[0].barrier);
    }
}
