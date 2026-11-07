//! The station-graph back end of the importers that build sections from named
//! stations (#576 B3): split a survey's legs into head-to-tail chains, place
//! the chains as a spanning tree of merges, tie every other shared station
//! with a loop closure, and order the configs so each merge target is created
//! before the sections that merge onto it. Compass and Walls use all of it;
//! TML, which reads its topology from the file, uses only the ordering.
//!
//! Chain `i` becomes section `i` in every caller, so a chain index is also the
//! section index the placements and closures name.
//!
//! Which shared station becomes a section's merge decides where a looped
//! survey sits in the Raw Data View (merges are rigid, closures are not;
//! docs/closures.md), so each importer keeps its own [`PlacementPolicy`] and a
//! re-import of an existing project lands where it did before.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::models::section::{ImportClosure, ImportPlacement, ImportSectionConfig};
use crate::models::station::GpsCoordinates;

/// Split `items` into head-to-tail chains: a chain continues while an item's
/// `from` station is the previous item's `to`, and a new one starts where it
/// is not. Returns each chain as indices into `items`, in order.
pub(crate) fn split_into_chains<T>(
    items: &[T],
    ends: impl Fn(&T) -> (&str, &str),
) -> Vec<Vec<usize>> {
    let mut chains: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut prev_to: Option<&str> = None;
    for (i, item) in items.iter().enumerate() {
        let (from, to) = ends(item);
        if prev_to != Some(from) && !current.is_empty() {
            chains.push(std::mem::take(&mut current));
        }
        current.push(i);
        prev_to = Some(to);
    }
    if !current.is_empty() {
        chains.push(current);
    }
    chains
}

/// A fixed station that can make its section a zero datum.
pub(crate) struct Anchor<'a> {
    pub(crate) station: &'a str,
    pub(crate) coordinates: GpsCoordinates,
}

/// How one importer connects its chains and roots each connected component.
/// The importers differ, and each keeps its own rule so that the merge each
/// section gets - and the order sections are created in - stays what earlier
/// imports of the same file produced.
pub(crate) enum PlacementPolicy<'a> {
    /// Compass. Every two chains sharing a station are neighbours. The first
    /// chain naming the project's one fixed station is its zero datum, and
    /// that component is placed first. Every other component starts at Origin
    /// from its lowest chain; each such start warns, except the very first
    /// one when the fixed station names no chain (or there is none).
    Compass { anchor: Option<Anchor<'a>> },
    /// Walls. A station's chains are neighbours of the first chain naming it
    /// (a star, not every pair). Components are taken in chain order, each
    /// rooted at its lowest chain holding a fixed station - at most one zero
    /// datum per component - or else at Origin from its lowest chain.
    Walls { anchors: &'a [Anchor<'a>] },
}

/// The spanning-tree placement of every chain.
pub(crate) struct Placements {
    /// Each chain's placement. `None` only for a chain no rule reached, which
    /// callers create at Origin.
    pub(crate) placement: Vec<Option<ImportPlacement>>,
    /// Chains in the order they were placed: every merge target precedes the
    /// chains merged onto it.
    pub(crate) creation_order: Vec<usize>,
    /// Each merged chain's tree edge: (its station, the parent chain, the
    /// parent's station). The two occurrences became ONE point.
    pub(crate) merge: Vec<Option<(u32, usize, u32)>>,
}

/// The tree under construction: the chain graph plus what is placed so far.
struct Tree<'g> {
    names: &'g [&'g [String]],
    adjacency: Vec<Vec<usize>>,
    /// Per chain, each station name's first index in it.
    first_index: Vec<HashMap<&'g str, u32>>,
    visited: Vec<bool>,
    out: Placements,
}

impl Tree<'_> {
    /// The first of `child`'s stations that `parent` also names, as
    /// (index in child, first index in parent).
    fn shared_station(&self, child: usize, parent: usize) -> Option<(u32, u32)> {
        self.names[child].iter().enumerate().find_map(|(ci, name)| {
            self.first_index[parent]
                .get(name.as_str())
                .map(|&pi| (ci as u32, pi))
        })
    }

    /// Place `chain` as the root of a component.
    fn root(&mut self, chain: usize, placement: ImportPlacement) {
        self.out.placement[chain] = Some(placement);
        self.visited[chain] = true;
        self.out.creation_order.push(chain);
    }

    /// Breadth-first from `root`: merge every unplaced neighbour onto the
    /// chain that reached it, at their first shared station.
    fn grow(&mut self, root: usize) {
        let mut queue: VecDeque<usize> = VecDeque::from([root]);
        while let Some(parent) = queue.pop_front() {
            for nb in self.adjacency[parent].clone() {
                if self.visited[nb] {
                    continue;
                }
                if let Some((child_st, parent_st)) = self.shared_station(nb, parent) {
                    self.out.placement[nb] = Some(ImportPlacement::FromImporting {
                        import_station: child_st,
                        target_section_index: parent as u32,
                        target_station_index: parent_st,
                    });
                    self.out.merge[nb] = Some((child_st, parent, parent_st));
                }
                self.visited[nb] = true;
                self.out.creation_order.push(nb);
                queue.push_back(nb);
            }
        }
    }

    /// The chains connected to `start`, ascending.
    fn component(&self, start: usize) -> Vec<usize> {
        let mut seen: HashSet<usize> = HashSet::from([start]);
        let mut stack = vec![start];
        let mut component = Vec::new();
        while let Some(c) = stack.pop() {
            component.push(c);
            for &nb in &self.adjacency[c] {
                if seen.insert(nb) {
                    stack.push(nb);
                }
            }
        }
        component.sort_unstable();
        component
    }
}

/// Place every chain (`chain_names[i]` = chain `i`'s stations in order) as a
/// spanning tree over "shares a station": each component gets one root (a
/// zero datum or Origin, by `policy`) and every other chain merges onto the
/// chain that reached it via `FromImporting`. The stations this leaves shared
/// but unmerged become loop closures ([`build_closures`]). `label_of` names a
/// chain for a warning.
pub(crate) fn place_chains(
    chain_names: &[&[String]],
    policy: &PlacementPolicy,
    warnings: &mut Vec<String>,
    label_of: impl Fn(usize) -> String,
) -> Placements {
    let n = chain_names.len();
    // Each station's chains, ascending and without repeats.
    let mut occurrences: HashMap<&str, Vec<usize>> = HashMap::new();
    for (ci, names) in chain_names.iter().enumerate() {
        for name in *names {
            let chains = occurrences.entry(name.as_str()).or_default();
            if chains.last() != Some(&ci) {
                chains.push(ci);
            }
        }
    }
    let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); n];
    for chains in occurrences.values() {
        let mut link = |a: usize, b: usize| {
            adjacency[a].push(b);
            adjacency[b].push(a);
        };
        match policy {
            // O(k^2) edges for a station k chains share.
            PlacementPolicy::Compass { .. } => {
                for (i, &a) in chains.iter().enumerate() {
                    for &b in &chains[i + 1..] {
                        link(a, b);
                    }
                }
            }
            // O(k): the same components, fewer edges.
            PlacementPolicy::Walls { .. } => {
                for &b in &chains[1..] {
                    link(chains[0], b);
                }
            }
        }
    }
    for nbrs in &mut adjacency {
        nbrs.sort_unstable();
        nbrs.dedup();
    }
    let first_index = chain_names
        .iter()
        .map(|names| {
            let mut m: HashMap<&str, u32> = HashMap::new();
            for (i, name) in names.iter().enumerate() {
                m.entry(name.as_str()).or_insert(i as u32);
            }
            m
        })
        .collect();
    let mut tree = Tree {
        names: chain_names,
        adjacency,
        first_index,
        visited: vec![false; n],
        out: Placements {
            placement: vec![None; n],
            creation_order: Vec::with_capacity(n),
            merge: vec![None; n],
        },
    };
    match policy {
        PlacementPolicy::Compass { anchor } => {
            place_anchor_first(&mut tree, anchor.as_ref(), &occurrences, warnings, label_of);
        }
        PlacementPolicy::Walls { anchors } => {
            place_per_component(&mut tree, anchors, &occurrences, warnings, label_of);
        }
    }
    tree.out
}

/// [`PlacementPolicy::Compass`]: the anchor's component first, then the rest.
fn place_anchor_first(
    tree: &mut Tree,
    anchor: Option<&Anchor>,
    occurrences: &HashMap<&str, Vec<usize>>,
    warnings: &mut Vec<String>,
    label_of: impl Fn(usize) -> String,
) {
    let anchored = anchor.and_then(|a| {
        let chain = *occurrences.get(a.station)?.first()?;
        let station_index = tree.first_index[chain].get(a.station).copied()?;
        Some((chain, station_index, a.coordinates))
    });
    if let Some((chain, station_index, coordinates)) = anchored {
        tree.root(
            chain,
            ImportPlacement::ZeroDatum {
                coordinates,
                station_index,
            },
        );
        tree.grow(chain);
    }
    // The component every later one is "not connected to": the georeferenced one when
    // the anchor placed it, else the first one placed at origin [#576 NF-W4M-3] - with no
    // fixed station in use there is no georeferenced survey to name.
    let mut first_component = anchored.map(|_| None);
    for seed in 0..tree.names.len() {
        if tree.visited[seed] {
            continue;
        }
        match &first_component {
            Some(None) => warnings.push(format!(
                "Section '{}' starts a separate component not connected to the georeferenced \
                 survey; placed at origin",
                label_of(seed)
            )),
            Some(Some(first)) => warnings.push(format!(
                "Section '{}' starts a separate component not connected to section '{first}'; \
                 no fixed station georeferences either, so both are placed at origin",
                label_of(seed)
            )),
            None => first_component = Some(Some(label_of(seed))),
        }
        tree.root(seed, ImportPlacement::Origin);
        tree.grow(seed);
    }
}

/// [`PlacementPolicy::Walls`]: components in chain order, each rooted at its
/// lowest anchored chain.
fn place_per_component(
    tree: &mut Tree,
    anchors: &[Anchor],
    occurrences: &HashMap<&str, Vec<usize>>,
    warnings: &mut Vec<String>,
    label_of: impl Fn(usize) -> String,
) {
    // Fixed-station name -> anchor index (first declaration wins per name).
    let mut anchor_by_name: HashMap<&str, usize> = HashMap::new();
    for (i, a) in anchors.iter().enumerate() {
        anchor_by_name.entry(a.station).or_insert(i);
    }
    // A fixed station that no shot references can't be anchored to anything.
    for a in anchors {
        if !occurrences.contains_key(a.station) {
            warnings.push(format!(
                "Fixed station '{}' isn't used by any shot; no zero datum created for it.",
                a.station
            ));
        }
    }
    // For each chain, its first station that is a fixed station.
    let chain_anchor: Vec<Option<(u32, usize)>> =
        tree.names
            .iter()
            .map(|names| {
                names.iter().enumerate().find_map(|(k, name)| {
                    anchor_by_name.get(name.as_str()).map(|&ai| (k as u32, ai))
                })
            })
            .collect();
    for start in 0..tree.names.len() {
        if tree.visited[start] {
            continue;
        }
        let component = tree.component(start);
        let anchored: Vec<usize> = component
            .iter()
            .copied()
            .filter(|&ci| chain_anchor[ci].is_some())
            .collect();
        // `start` is the component's lowest chain: every lower one is placed.
        let root = anchored.first().copied().unwrap_or(start);
        if let Some((station_index, ai)) = chain_anchor[root] {
            tree.root(
                root,
                ImportPlacement::ZeroDatum {
                    coordinates: anchors[ai].coordinates,
                    station_index,
                },
            );
            if anchored.len() > 1 {
                let extras: Vec<&str> = anchored
                    .iter()
                    .skip(1)
                    .filter_map(|&ci| chain_anchor[ci].map(|(_, j)| anchors[j].station))
                    .collect();
                warnings.push(format!(
                    "Component anchored at '{}'; {} other fixed station(s) in the same connected \
                     survey are kept via merges + loop closures, not as separate datums: {}.",
                    anchors[ai].station,
                    extras.len(),
                    extras.join(", "),
                ));
            }
        } else {
            tree.root(root, ImportPlacement::Origin);
            if !anchors.is_empty() {
                warnings.push(format!(
                    "Section '{}' is a separate component with no fixed station; placed at origin.",
                    label_of(root)
                ));
            }
        }
        tree.grow(root);
    }
}

/// Loop closures for the shared stations the spanning tree did not merge:
/// every occurrence of a name is tied to its canonical one (the first, in the
/// lowest chain).
///
/// Decided per merged POINT, not per occurrence (#576 NF-W2C-1, NF-FIX3R-3):
/// the merge edges join occurrences into points - transitively, and through
/// any chain's edge, not only the chain's own. An occurrence already on the
/// canonical point needs no closure (one would tie a point to itself, zero
/// misclosure), and a point holding several occurrences needs ONE closure,
/// not one per occurrence (that counted its loop twice in the least-squares
/// solve). A station revisited later in the SAME chain (a loop inside one
/// survey, A1 -> A2 -> A3 -> A1) is never merged, so it keeps its
/// intra-section closure (#576 B3).
pub(crate) fn build_closures(
    chain_names: &[&[String]],
    placements: &Placements,
) -> Vec<ImportClosure> {
    // Chains are visited in index order, so the first occurrence seen is the
    // canonical one.
    let mut canonical: HashMap<&str, (usize, u32)> = HashMap::new();
    for (ci, names) in chain_names.iter().enumerate() {
        for (k, name) in names.iter().enumerate() {
            canonical.entry(name.as_str()).or_insert((ci, k as u32));
        }
    }
    // One union-find node per (chain, station) occurrence.
    let mut first_node: Vec<usize> = Vec::with_capacity(chain_names.len());
    let mut nodes = 0usize;
    for names in chain_names {
        first_node.push(nodes);
        nodes += names.len();
    }
    let node = |ci: usize, k: u32| first_node[ci] + k as usize;
    let mut point: Vec<usize> = (0..nodes).collect();
    fn root(point: &mut [usize], mut x: usize) -> usize {
        while point[x] != x {
            point[x] = point[point[x]];
            x = point[x];
        }
        x
    }
    for &child in &placements.creation_order {
        if let Some((child_st, parent, parent_st)) = placements.merge[child] {
            let a = root(&mut point, node(child, child_st));
            let b = root(&mut point, node(parent, parent_st));
            point[a] = b;
        }
    }
    let mut closures: Vec<ImportClosure> = Vec::new();
    let mut pinned: HashSet<(usize, usize)> = HashSet::new();
    for (ci, names) in chain_names.iter().enumerate() {
        for (k, name) in names.iter().enumerate() {
            let Some(&(canon_ci, canon_st)) = canonical.get(name.as_str()) else {
                continue;
            };
            let here = root(&mut point, node(ci, k as u32));
            let there = root(&mut point, node(canon_ci, canon_st));
            if here == there || !pinned.insert((here, there)) {
                continue;
            }
            closures.push(ImportClosure {
                from_section_index: ci as u32,
                from_station_index: k as u32,
                to_section_index: canon_ci as u32,
                to_station_index: canon_st,
            });
        }
    }
    closures
}

/// What [`topological_sort_configs`] does with configs that are already in a
/// valid creation order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConfigOrder {
    /// Rebuild the order anyway: every root first, then the sections merged
    /// onto them level by level (Compass, Walls).
    Rebuild,
    /// Leave a valid order alone; rebuild only one that creates a section
    /// before its target (TML).
    KeepValid,
}

/// Reorder `configs` so that the section any `FromImporting` placement targets
/// is created before its dependent ([`dependency_order`]; a target no config
/// has counts as no dependency).
pub(crate) fn topological_sort_configs(configs: &mut Vec<ImportSectionConfig>, order: ConfigOrder) {
    if configs.len() <= 1 {
        return;
    }
    let pos_of: HashMap<u32, usize> = configs
        .iter()
        .enumerate()
        .map(|(i, c)| (c.section_index, i))
        .collect();
    // dep[i] = Some(j): configs[i] merges onto configs[j].
    let dep: Vec<Option<usize>> = configs
        .iter()
        .map(|c| match &c.placement {
            ImportPlacement::FromImporting {
                target_section_index,
                ..
            } => pos_of.get(target_section_index).copied(),
            _ => None,
        })
        .collect();
    let valid = || dep.iter().enumerate().all(|(i, d)| d.is_none_or(|j| j < i));
    if order == ConfigOrder::KeepValid && valid() {
        return;
    }
    let mut old: Vec<Option<ImportSectionConfig>> = configs.drain(..).map(Some).collect();
    configs.extend(
        dependency_order(&dep)
            .into_iter()
            .filter_map(|i| old[i].take()),
    );
}

/// Kahn's algorithm over `dep` (`dep[i] = Some(j)`: `i` must come after `j`):
/// the roots in index order, then each placed index's dependents in index
/// order; anything left on a cycle is appended in its original order. The
/// cycle tail is marked in a bitmap (#576 B27(d)): an `order.contains(&i)`
/// per index was quadratic in the section count, which a crafted file
/// controls.
pub(crate) fn dependency_order(dep: &[Option<usize>]) -> Vec<usize> {
    let n = dep.len();
    let mut in_degree = vec![0u32; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, d) in dep.iter().enumerate() {
        if let Some(j) = d {
            in_degree[i] += 1;
            dependents[*j].push(i);
        }
    }
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &d in &dependents[i] {
            in_degree[d] -= 1;
            if in_degree[d] == 0 {
                queue.push_back(d);
            }
        }
    }
    if order.len() < n {
        let mut placed = vec![false; n];
        for &i in &order {
            placed[i] = true;
        }
        order.extend((0..n).filter(|&i| !placed[i]));
    }
    order
}
