//! Conflict-aware tool wave scheduling.

use std::path::PathBuf;
use tact_contracts::capability::ToolResources;

fn overlap(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b || a.starts_with(b) || b.starts_with(a)
}

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

pub fn schedule_waves(resources: &[ToolResources]) -> Vec<usize> {
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

pub fn waves_grouped(resources: &[ToolResources]) -> Vec<Vec<usize>> {
    let assignment = schedule_waves(resources);
    let wave_count = assignment.iter().copied().max().map_or(0, |m| m + 1);
    let mut groups = vec![Vec::new(); wave_count];
    for (idx, wave) in assignment.iter().enumerate() {
        groups[*wave].push(idx);
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_calls_share_a_wave() {
        let resources = vec![ToolResources::independent(), ToolResources::independent()];
        assert_eq!(waves_grouped(&resources), vec![vec![0, 1]]);
    }

    #[test]
    fn conflicting_write_is_deferred() {
        let resources = vec![
            ToolResources {
                writes: vec![PathBuf::from("a")],
                ..Default::default()
            },
            ToolResources {
                reads: vec![PathBuf::from("a")],
                ..Default::default()
            },
        ];
        assert_eq!(schedule_waves(&resources), vec![0, 1]);
    }
}
