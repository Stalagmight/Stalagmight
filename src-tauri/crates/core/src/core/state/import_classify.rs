//! Jump vs Reference for an import's virtual shots (#27), on the station
//! graph.
//!
//! A parser can only tell that a shot is virtual; which virtual kind it is
//! needs the placed survey, so the import runs this once its lines are
//! placed and its closures made (0.6.0's `classify_virtual_shots`):
//!
//! - **Auto** (the chain rule): a connected run of virtual shots that meets
//!   real survey at two or more distinct points is a Jump (it bridges two
//!   places a diver can be); one that meets it at one point or nowhere is a
//!   Reference. "Meets" sees through junctions (one station) and through
//!   every loop closure, aligned or not (a closed loop is a physical
//!   connection whatever strategy reconciles it).
//! - **AllJumps / AllReferences**: every virtual shot of the file takes the
//!   one kind.
//!
//! The graph is read project-wide (a run is usually bounded only through a
//! junction with survey that was already there), but only the shots of the
//! lines in `scope` (the ones this import made) are rewritten: a per-file
//! setting never re-kinds a shot the user set in an earlier session.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::models::survey::Project;
use crate::models::{LineId, ShotKind};

use super::command_import::VirtualShotMode;

/// Union-find over dense station indices.
struct Sets(Vec<usize>);

impl Sets {
    fn new(n: usize) -> Self {
        Self((0..n).collect())
    }

    fn find(&mut self, mut x: usize) -> usize {
        while let Some(&parent) = self.0.get(x) {
            if parent == x {
                break;
            }
            let grand = self.0.get(parent).copied().unwrap_or(parent);
            if let Some(slot) = self.0.get_mut(x) {
                *slot = grand;
            }
            x = parent;
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            if let Some(slot) = self.0.get_mut(ra.max(rb)) {
                *slot = ra.min(rb);
            }
        }
    }
}

/// Classify the virtual shots of the lines in `scope` by `mode` (module
/// docs). Returns how many shots changed kind.
pub(super) fn classify_virtual_shots(
    project: &mut Project,
    mode: VirtualShotMode,
    scope: &BTreeSet<LineId>,
) -> usize {
    if scope.is_empty() {
        return 0;
    }
    let verdicts: Vec<(usize, usize, ShotKind)> = match mode {
        VirtualShotMode::Auto => chain_rule(project),
        VirtualShotMode::AllJumps => every_virtual_shot(project, ShotKind::Jump),
        VirtualShotMode::AllReferences => every_virtual_shot(project, ShotKind::Reference),
    };
    let wanted: Vec<(usize, usize, ShotKind)> = verdicts
        .into_iter()
        .filter(|&(position, index, kind)| {
            project.lines().get(position).is_some_and(|line| {
                scope.contains(&line.id) && line.shots.get(index).is_some_and(|s| s.kind != kind)
            })
        })
        .collect();
    if wanted.is_empty() {
        return 0;
    }
    let lines = project.lines_mut();
    let mut changed = 0;
    for (position, index, kind) in wanted {
        if let Some(shot) = lines
            .get_mut(position)
            .and_then(|line| line.shots.get_mut(index))
        {
            shot.kind = kind;
            changed += 1;
        }
    }
    changed
}

/// Every virtual shot of the project with `kind`, by (line position, index).
fn every_virtual_shot(project: &Project, kind: ShotKind) -> Vec<(usize, usize, ShotKind)> {
    project
        .lines()
        .iter()
        .enumerate()
        .flat_map(|(position, line)| {
            line.shots
                .iter()
                .enumerate()
                .filter(|(_, shot)| shot.kind.is_virtual())
                .map(move |(index, _)| (position, index, kind))
        })
        .collect()
}

/// The chain rule's verdict for every virtual shot of the project.
fn chain_rule(project: &Project) -> Vec<(usize, usize, ShotKind)> {
    let topology = project.topology();
    let n = topology.stations().len();
    // Stations tied by a closure are one point.
    let mut canon = Sets::new(n);
    for closure in &project.closures {
        if let (Some(a), Some(b)) = (topology.dense(closure.a), topology.dense(closure.b)) {
            canon.union(a, b);
        }
    }
    let mut touches_real: HashSet<usize> = HashSet::new();
    let mut runs = Sets::new(n);
    // (line position, shot index, canonical from-station) per virtual shot.
    let mut virtual_shots: Vec<(usize, usize, usize)> = Vec::new();
    for (position, line) in project.lines().iter().enumerate() {
        let Some(chain) = topology.chain(line.id) else {
            continue;
        };
        for (index, shot) in line.shots.iter().enumerate() {
            let (Some(from), Some(to)) = (
                chain.get(index).and_then(|&s| topology.dense(s)),
                chain.get(index + 1).and_then(|&s| topology.dense(s)),
            ) else {
                continue;
            };
            let (from, to) = (canon.find(from), canon.find(to));
            if shot.kind.is_virtual() {
                runs.union(from, to);
                virtual_shots.push((position, index, from));
            } else {
                touches_real.insert(from);
                touches_real.insert(to);
            }
        }
    }
    if virtual_shots.is_empty() {
        return Vec::new();
    }
    // The distinct canonical stations at which each run meets real survey.
    let mut boundaries: HashMap<usize, HashSet<usize>> = HashMap::new();
    for &real in &touches_real {
        boundaries.entry(runs.find(real)).or_default().insert(real);
    }
    virtual_shots
        .into_iter()
        .map(|(position, index, from)| {
            let bounded = boundaries
                .get(&runs.find(from))
                .is_some_and(|stations| stations.len() >= 2);
            let kind = if bounded {
                ShotKind::Jump
            } else {
                ShotKind::Reference
            };
            (position, index, kind)
        })
        .collect()
}
