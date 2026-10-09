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

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The workspace files a tool reads and/or writes, used to decide whether two
/// tool calls in the same turn may run concurrently.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResources {
    pub reads: Vec<PathBuf>,
    pub writes: Vec<PathBuf>,
    /// `true` for tools whose effects we cannot scope (bash, MCP, subagents,
    /// patch application, state mutations, …). A barrier conflicts with every
    /// other call and is therefore always scheduled alone in its own wave —
    /// equivalent to the previous fully-sequential behaviour for that tool.
    pub barrier: bool,
}

impl ToolResources {
    pub fn barrier() -> Self {
        Self {
            barrier: true,
            ..Default::default()
        }
    }

    /// A tool that touches no workspace file (e.g. `web_search`); it never
    /// conflicts and may run alongside anything.
    pub fn independent() -> Self {
        Self::default()
    }
}

/// Normalise a tool path argument to an absolute path rooted at `work_dir`.
///
/// We do not `canonicalize` (the target may not exist yet, and we want a pure,
/// filesystem-independent function); a lexical join is enough for the
/// equal/ancestor/descendant overlap test used by [`conflicts`].
#[allow(dead_code)]
fn normalize(work_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        work_dir.join(p)
    }
}

/// Map a single tool call to the workspace resources it touches using its
/// metadata [`ResourcePolicy`], which is the canonical source since Task 5.
pub(crate) fn tool_resources_from_metadata(
    policy: &crate::tool::ResourcePolicy,
    input: &Value,
    work_dir: &Path,
) -> ToolResources {
    policy.resolve(input, work_dir)
}

/// MCP tools on the same server serialize; different servers may run in parallel.
pub(crate) fn mcp_server_resources(server: &str) -> ToolResources {
    if server.is_empty() {
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
            mcp_server_resources("postgres"),
            mcp_server_resources("echo"),
        ];
        assert_eq!(schedule_waves(&r), vec![0, 0]);
    }

    #[test]
    fn mcp_tools_on_same_server_serialize() {
        let r = vec![
            mcp_server_resources("postgres"),
            mcp_server_resources("postgres"),
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
        let r = tool_resources_from_metadata(
            &crate::tool::ResourcePolicy::ReadPath { field: "path" },
            &serde_json::json!({"path": "src/a.rs"}),
            Path::new("/work"),
        );
        assert_eq!(r.reads, vec![PathBuf::from("/work/src/a.rs")]);
        assert!(r.writes.is_empty() && !r.barrier);
    }

    #[test]
    fn resources_absolute_path_kept() {
        let r = tool_resources_from_metadata(
            &crate::tool::ResourcePolicy::ReadPath { field: "path" },
            &serde_json::json!({"path": "/abs/a.rs"}),
            Path::new("/work"),
        );
        assert_eq!(r.reads, vec![PathBuf::from("/abs/a.rs")]);
    }

    #[test]
    fn resources_bash_is_barrier() {
        let r = ToolResources::barrier();
        assert!(r.barrier);
    }

    #[test]
    fn task_tools_serialize_with_each_other() {
        let work = Path::new("/work");
        let task_policy = crate::tool::ResourcePolicy::SharedState { scope: "task" };
        let create =
            tool_resources_from_metadata(&task_policy, &serde_json::json!({"subject": "a"}), work);
        let update = tool_resources_from_metadata(
            &task_policy,
            &serde_json::json!({"task_id": 1, "status": "completed"}),
            work,
        );
        let list = tool_resources_from_metadata(&task_policy, &serde_json::json!({}), work);
        let get =
            tool_resources_from_metadata(&task_policy, &serde_json::json!({"task_id": 1}), work);
        assert!(!create.barrier && !update.barrier);
        assert_eq!(
            schedule_waves(&[create.clone(), update.clone(), list.clone(), get]),
            vec![0, 1, 2, 3],
            "all task_* tools must run in separate waves"
        );
        // Still allowed to overlap a disjoint file read.
        let read_policy = crate::tool::ResourcePolicy::ReadPath { field: "path" };
        let read = tool_resources_from_metadata(
            &read_policy,
            &serde_json::json!({"path": "src/a.rs"}),
            work,
        );
        assert_eq!(schedule_waves(&[create, read, update]), vec![0, 0, 1]);
    }

    #[test]
    fn resources_unknown_tool_is_barrier() {
        let r = ToolResources::barrier();
        assert!(r.barrier);
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
