use tact_agent_api::{ResourceAccess, ResourceScope};

fn scope_prefix(a: &ResourceScope, b: &ResourceScope) -> bool {
    a.key.namespace == b.key.namespace
        && a.key.components.len() <= b.key.components.len()
        && a.key
            .components
            .iter()
            .zip(&b.key.components)
            .all(|(left, right)| left == right)
}

fn scopes_overlap(a: &ResourceScope, b: &ResourceScope) -> bool {
    if a.key == b.key {
        return true;
    }
    (a.covers_descendants && scope_prefix(a, b)) || (b.covers_descendants && scope_prefix(b, a))
}

fn writes_hit(a: &ResourceAccess, b: &ResourceAccess) -> bool {
    a.writes.iter().any(|write| {
        b.reads
            .iter()
            .chain(b.writes.iter())
            .any(|other| scopes_overlap(write, other))
    })
}

/// Whether two resolved calls must be separated into different waves.
pub fn calls_conflict(a: &ResourceAccess, b: &ResourceAccess) -> bool {
    a.barrier || b.barrier || writes_hit(a, b) || writes_hit(b, a)
}

/// Assign each call a wave; higher waves run later.
pub fn schedule_waves(resources: &[ResourceAccess]) -> Vec<usize> {
    let mut waves = vec![0; resources.len()];
    for index in 0..resources.len() {
        for earlier in 0..index {
            if calls_conflict(&resources[index], &resources[earlier]) {
                waves[index] = waves[index].max(waves[earlier] + 1);
            }
        }
    }
    waves
}

/// Group call indexes by wave, preserving source order within a wave.
pub fn waves_grouped(resources: &[ResourceAccess]) -> Vec<Vec<usize>> {
    let waves = schedule_waves(resources);
    let count = waves.iter().copied().max().map_or(0, |wave| wave + 1);
    let mut grouped = vec![Vec::new(); count];
    for (index, wave) in waves.into_iter().enumerate() {
        grouped[wave].push(index);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_agent_api::ResourceKey;

    fn access(reads: &[&str], writes: &[&str], descendants: bool) -> ResourceAccess {
        let scope = |path: &str| ResourceScope {
            key: ResourceKey::new(
                "path",
                path.split('/')
                    .filter(|part| !part.is_empty())
                    .map(str::to_owned)
                    .collect(),
            ),
            covers_descendants: descendants,
        };
        ResourceAccess {
            reads: reads.iter().map(|path| scope(path)).collect(),
            writes: writes.iter().map(|path| scope(path)).collect(),
            barrier: false,
        }
    }

    #[test]
    fn read_only_calls_share_a_wave() {
        let resources = [access(&["src/a"], &[], true), access(&["src/b"], &[], true)];
        assert_eq!(schedule_waves(&resources), [0, 0]);
    }

    #[test]
    fn writes_conflict_with_overlapping_reads_and_writes() {
        let resources = [
            access(&["src"], &[], true),
            access(&[], &["src/a"], false),
            access(&[], &["src/a"], false),
        ];
        assert_eq!(schedule_waves(&resources), [0, 1, 2]);
    }

    #[test]
    fn ancestor_scopes_do_not_merge_sibling_prefixes() {
        let resources = [access(&["src"], &[], true), access(&[], &["src2"], false)];
        assert_eq!(schedule_waves(&resources), [0, 0]);
    }

    #[test]
    fn barriers_run_alone_and_independent_scopes_run_parallel() {
        let resources = [
            ResourceAccess::independent(),
            ResourceAccess::barrier(),
            ResourceAccess::independent(),
        ];
        assert_eq!(schedule_waves(&resources), [0, 1, 2]);
    }
}
