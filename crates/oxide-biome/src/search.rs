//! Multi-noise biome search: nearest parameter point to a sampled [`ClimateSample`].
//!
//! Ported 2026-08-24 from decompiled 26.2 `Climate.RTree`, including the fixed-point
//! representation it searches in: every coordinate is quantized to an `i64` as
//! `(value * 10000)`, so distances are exact integers rather than floats that could drift.
//!
//! The tree is not only a speed structure -- with the real overworld preset the list is
//! ~1900 points and a linear scan costs about a second per chunk, against 0.06s for the rest
//! of generation. It is a nearest-neighbour search either way, so the biome picked is the
//! same; the tree just stops visiting branches that cannot contain a closer point.

use oxide_core::BiomeId;
use oxide_datapack::climate::ClimateParam;
use oxide_datapack::{ClimateParameters, MultiNoiseBiomeEntry, MultiNoiseSource};

use crate::climate::ClimateSample;

/// Vanilla's parameter space is 7-dimensional: the six climate axes plus the parameter
/// point's `offset`, which is compared against a target of 0.
const DIMENSIONS: usize = 7;
/// `Climate.RTree.CHILDREN_PER_NODE`.
const CHILDREN_PER_NODE: usize = 6;

/// `Climate.quantizeCoord`.
fn quantize(coord: f32) -> i64 {
    (coord * 10000.0) as i64
}

/// A quantized inclusive range -- vanilla's `Climate.Parameter`.
#[derive(Debug, Clone, Copy)]
struct Param {
    min: i64,
    max: i64,
}

impl Param {
    fn point(value: i64) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    fn from_climate(param: ClimateParam) -> Self {
        match param {
            ClimateParam::Single(v) => Self::point(quantize(v)),
            ClimateParam::Range([lo, hi]) => Self {
                min: quantize(lo),
                max: quantize(hi),
            },
        }
    }

    /// `Climate.Parameter.distance(long)`: 0 inside the range, else the gap to the nearer end.
    #[cfg(test)]
    fn distance(&self, target: i64) -> i64 {
        let above = target - self.max;
        if above > 0 {
            return above;
        }
        (self.min - target).max(0)
    }

    /// The range covering both -- `Parameter.span`.
    fn union(&self, other: &Param) -> Param {
        Param {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    fn center(&self) -> i64 {
        // Vanilla divides the *sum* by 2 in `long` arithmetic, which truncates toward zero;
        // averaging any other way would order nodes differently and build a different tree.
        (self.min + self.max) / 2
    }
}

fn parameter_space(params: &ClimateParameters) -> [Param; DIMENSIONS] {
    [
        Param::from_climate(params.temperature),
        Param::from_climate(params.humidity),
        Param::from_climate(params.continentalness),
        Param::from_climate(params.erosion),
        Param::from_climate(params.depth),
        Param::from_climate(params.weirdness),
        Param::point(quantize(params.offset)),
    ]
}

enum Node {
    Leaf {
        space: [Param; DIMENSIONS],
        biome: BiomeId,
    },
    SubTree {
        space: [Param; DIMENSIONS],
        children: Vec<Node>,
    },
}

impl Node {
    fn space(&self) -> &[Param; DIMENSIONS] {
        match self {
            Node::Leaf { space, .. } | Node::SubTree { space, .. } => space,
        }
    }
}

/// One node of the searched tree, flattened: the build above produces a [`Node`] tree, and
/// [`flatten`] lays it out breadth-first so every subtree's children are contiguous in one
/// array, in exactly the order the [`Node`] tree holds them. `count == 0` marks a leaf, whose
/// `first` is then its index into [`BiomeSearchTree::biomes`].
#[derive(Debug, Clone, Copy)]
struct FlatNode {
    min: [i64; DIMENSIONS],
    max: [i64; DIMENSIONS],
    first: u32,
    count: u32,
}

impl FlatNode {
    fn new(space: &[Param; DIMENSIONS], first: u32, count: u32) -> Self {
        Self {
            min: std::array::from_fn(|i| space[i].min),
            max: std::array::from_fn(|i| space[i].max),
            first,
            count,
        }
    }

    /// Vanilla's `Node.distance`: the per-axis [`Param::distance`] (`above` if positive, else
    /// `max(below, 0)`), squared and summed. Integer arithmetic, so the result is identical
    /// whatever order it is added in.
    // One counter over three parallel arrays, kept as written so it vectorises.
    #[allow(clippy::needless_range_loop)]
    #[inline(always)]
    fn distance(&self, target: &[i64; DIMENSIONS]) -> i64 {
        let mut total = 0i64;
        for i in 0..DIMENSIONS {
            let above = target[i] - self.max[i];
            let below = (self.min[i] - target[i]).max(0);
            let d = if above > 0 { above } else { below };
            total += d * d;
        }
        total
    }
}

/// Lays `root` out as [`FlatNode`]s, root at index 0, children of each subtree contiguous and
/// in their original order. Leaves' biomes go to `biomes` in visit order.
fn flatten(root: Node, biomes: &mut Vec<BiomeId>) -> Vec<FlatNode> {
    let mut nodes = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    let placeholder = FlatNode {
        min: [0; DIMENSIONS],
        max: [0; DIMENSIONS],
        first: 0,
        count: 0,
    };
    nodes.push(placeholder);
    queue.push_back((root, 0usize));
    while let Some((node, slot)) = queue.pop_front() {
        match node {
            Node::Leaf { space, biome } => {
                nodes[slot] = FlatNode::new(&space, biomes.len() as u32, 0);
                biomes.push(biome);
            }
            Node::SubTree { space, children } => {
                let first = nodes.len();
                nodes[slot] = FlatNode::new(&space, first as u32, children.len() as u32);
                for (offset, child) in children.into_iter().enumerate() {
                    nodes.push(placeholder);
                    queue.push_back((child, first + offset));
                }
            }
        }
    }
    nodes
}

/// Vanilla's `Node.search` with no seed, on the flattened tree. `best` is
/// `(leaf biome index, its distance)`, `(u32::MAX, i64::MAX)` for "nothing found yet".
///
/// The recursive form returns the closest leaf below a subtree, seeded with the incumbent,
/// and the caller adopts it only when strictly closer. A subtree only ever replaces the
/// incumbent with a strictly closer leaf, so that is the same as one running `best` that
/// every level updates in place -- which is what this keeps, with the distance alongside so
/// the incumbent's is never recomputed. Children are visited in stored order, pruned and
/// replaced under the same `<=`-keeps-the-incumbent rule.
///
/// Once `best` is at distance 0 nothing can replace it (every distance is `>= 0`, and only a
/// strictly smaller one replaces), so the remaining children would all be pruned; returning
/// then only skips computing their distances.
fn search(nodes: &[FlatNode], node: &FlatNode, target: &[i64; DIMENSIONS], best: &mut (u32, i64)) {
    let first = node.first as usize;
    let children = &nodes[first..first + node.count as usize];
    for child in children {
        let child_distance = child.distance(target);
        // `<=` keeps the incumbent on a tie, exactly as vanilla does.
        if best.1 <= child_distance {
            continue;
        }
        if child.count == 0 {
            *best = (child.first, child_distance);
        } else {
            search(nodes, child, target, best);
        }
        if best.1 == 0 {
            return;
        }
    }
}

fn union_space(children: &[Node]) -> [Param; DIMENSIONS] {
    let mut space = *children[0].space();
    for child in &children[1..] {
        let child_space = child.space();
        for i in 0..DIMENSIONS {
            space[i] = space[i].union(&child_space[i]);
        }
    }
    space
}

fn sub_tree(children: Vec<Node>) -> Node {
    Node::SubTree {
        space: union_space(&children),
        children,
    }
}

/// `Climate.RTree.cost`: total width of a bounding box, the thing bucketing minimises.
fn cost(space: &[Param; DIMENSIONS]) -> i64 {
    space.iter().map(|p| (p.max - p.min).abs()).sum()
}

/// `RTree.sort`: order by the given dimension's centre, then by each following dimension
/// cyclically. `absolute` compares |centre| instead, which is what the final pass uses.
fn sort_nodes(children: &mut [Node], dimension: usize, absolute: bool) {
    let key = |node: &Node, d: usize| -> i64 {
        let center = node.space()[d].center();
        if absolute {
            center.abs()
        } else {
            center
        }
    };
    children.sort_by(|a, b| {
        for offset in 0..DIMENSIONS {
            let d = (dimension + offset) % DIMENSIONS;
            match key(a, d).cmp(&key(b, d)) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// `RTree.bucketize`'s group size: `6^floor(log6(n - 0.01))`, so a full level of the tree
/// fills before the remainder forms a short bucket.
fn bucket_size(n: usize) -> usize {
    let expected = (CHILDREN_PER_NODE as f64)
        .powf(((n as f64 - 0.01).ln() / (CHILDREN_PER_NODE as f64).ln()).floor())
        as usize;
    expected.max(1)
}

/// The index ranges `bucketize` would cut `n` nodes into.
fn bucket_ranges(n: usize) -> Vec<std::ops::Range<usize>> {
    let size = bucket_size(n);
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < n {
        let end = (start + size).min(n);
        ranges.push(start..end);
        start = end;
    }
    ranges
}

/// `Climate.RTree.build`: pick the axis whose split yields the tightest bounding boxes, cut
/// along it, and recurse. The chosen axis only affects how fast the search prunes, never
/// which leaf it ends at.
fn build(mut children: Vec<Node>) -> Node {
    if children.len() == 1 {
        return children.pop().expect("checked len");
    }
    if children.len() <= CHILDREN_PER_NODE {
        children.sort_by_key(|node| node.space().iter().map(|p| p.center().abs()).sum::<i64>());
        return sub_tree(children);
    }

    let mut min_cost = i64::MAX;
    let mut min_dimension = 0usize;
    for dimension in 0..DIMENSIONS {
        sort_nodes(&mut children, dimension, false);
        let total: i64 = bucket_ranges(children.len())
            .into_iter()
            .map(|range| cost(&union_space(&children[range])))
            .sum();
        if total < min_cost {
            min_cost = total;
            min_dimension = dimension;
        }
    }

    sort_nodes(&mut children, min_dimension, false);
    let sizes: Vec<usize> = bucket_ranges(children.len())
        .into_iter()
        .map(|r| r.len())
        .collect();
    let mut rest = children;
    let mut buckets: Vec<Node> = Vec::with_capacity(sizes.len());
    for size in sizes {
        let tail = rest.split_off(size);
        buckets.push(sub_tree(std::mem::replace(&mut rest, tail)));
    }

    sort_nodes(&mut buckets, min_dimension, true);
    let children = buckets
        .into_iter()
        .map(|bucket| match bucket {
            Node::SubTree { children, .. } => build(children),
            leaf => leaf,
        })
        .collect();
    sub_tree(children)
}

pub struct BiomeSearchTree {
    entries: Vec<MultiNoiseBiomeEntry>,
    /// The tree in [`FlatNode`] layout, root first; empty when `entries` is.
    nodes: Vec<FlatNode>,
    /// Leaf biomes, indexed by a leaf's [`FlatNode::first`].
    biomes: Vec<BiomeId>,
    /// For each leaf biome index, the index of its leaf in `nodes`, so a hint can be measured.
    leaf_nodes: Vec<u32>,
}

impl BiomeSearchTree {
    /// An `Explicit` list is searched as given; a `Preset` resolves through
    /// [`oxide_datapack::preset_entries`], which ports the tables vanilla hardcodes in Java.
    /// An id vanilla does not hardcode yields `None`.
    pub fn from_source(source: &MultiNoiseSource) -> Option<Self> {
        let entries = match source {
            MultiNoiseSource::Explicit { biomes } => biomes.clone(),
            MultiNoiseSource::Preset { preset } => oxide_datapack::preset_entries(preset)?,
        };
        Some(Self::from_entries(entries))
    }

    pub fn from_entries(entries: Vec<MultiNoiseBiomeEntry>) -> Self {
        let mut biomes = Vec::new();
        let nodes = if entries.is_empty() {
            Vec::new()
        } else {
            let root = build(
                entries
                    .iter()
                    .map(|e| Node::Leaf {
                        space: parameter_space(&e.parameters),
                        biome: e.biome.clone(),
                    })
                    .collect(),
            );
            flatten(root, &mut biomes)
        };
        let mut leaf_nodes = vec![0u32; biomes.len()];
        for (index, node) in nodes.iter().enumerate() {
            if node.count == 0 {
                leaf_nodes[node.first as usize] = index as u32;
            }
        }
        Self {
            entries,
            nodes,
            biomes,
            leaf_nodes,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn nearest(&self, sample: ClimateSample) -> Option<&BiomeId> {
        let mut hint = u32::MAX;
        self.nearest_with_hint(sample, &mut hint)
    }

    /// [`Self::nearest`], with `hint` naming a leaf (as left by an earlier call; `u32::MAX`
    /// for none) that is probably close to `sample`. Returns the very same leaf as
    /// [`Self::nearest`] -- ties included -- and leaves the answer's leaf in `hint`.
    ///
    /// Why the answer cannot change: the unseeded search returns the first leaf, in visit
    /// order, at the minimum distance `m` (an earlier leaf at `m` would have been kept, a later
    /// one never replaces it, and until that first one is found the incumbent is farther than
    /// `m`, so its subtree is never pruned). Starting instead from "nothing found, but a
    /// distance bound of `d + 1`", with `d` the hint leaf's distance and `m <= d`, the same
    /// argument holds word for word: the incumbent is above `m` until the first leaf at `m` is
    /// reached, and nothing strictly below `m` exists to replace it. The bound only prunes
    /// subtrees whose every leaf is farther than `d`, which could never have been the answer.
    pub fn nearest_with_hint(&self, sample: ClimateSample, hint: &mut u32) -> Option<&BiomeId> {
        let target = [
            quantize(sample.temperature as f32),
            quantize(sample.humidity as f32),
            quantize(sample.continentalness as f32),
            quantize(sample.erosion as f32),
            quantize(sample.depth as f32),
            quantize(sample.weirdness as f32),
            0,
        ];
        let root = self.nodes.first()?;
        if root.count == 0 {
            // A one-entry tree is its own leaf, which vanilla returns without measuring.
            return self.biomes.get(root.first as usize);
        }
        let bound = match self.leaf_nodes.get(*hint as usize) {
            Some(&node) => self.nodes[node as usize]
                .distance(&target)
                .saturating_add(1),
            None => i64::MAX,
        };
        let mut best = (u32::MAX, bound);
        search(&self.nodes, root, &target, &mut best);
        *hint = best.0;
        self.biomes.get(best.0 as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::ResourceLocation;

    fn entry(name: &str, temp: f32) -> MultiNoiseBiomeEntry {
        MultiNoiseBiomeEntry {
            biome: ResourceLocation::minecraft(name),
            parameters: ClimateParameters {
                temperature: ClimateParam::Single(temp),
                humidity: ClimateParam::Single(0.0),
                continentalness: ClimateParam::Single(0.0),
                erosion: ClimateParam::Single(0.0),
                depth: ClimateParam::Single(0.0),
                weirdness: ClimateParam::Single(0.0),
                offset: 0.0,
            },
        }
    }

    fn sample_with_temp(t: f64) -> ClimateSample {
        ClimateSample {
            temperature: t,
            humidity: 0.0,
            continentalness: 0.0,
            erosion: 0.0,
            depth: 0.0,
            weirdness: 0.0,
        }
    }

    /// The distance the nearest entry sits at, by unpruned scan -- what the tree's answer
    /// has to match. Compared by *distance* rather than by biome id: several parameter points
    /// can tie at the same distance (the two cave biomes both sit at 0 over much of the
    /// space), and vanilla itself resolves those ties by tree order plus a thread-local
    /// previous result, so picking a different member of a tie is not a divergence.
    fn best_distance(entries: &[MultiNoiseBiomeEntry], target: &[i64; DIMENSIONS]) -> i64 {
        entries
            .iter()
            .map(|e| distance_to(&parameter_space(&e.parameters), target))
            .min()
            .expect("non-empty")
    }

    fn distance_to(space: &[Param; DIMENSIONS], target: &[i64; DIMENSIONS]) -> i64 {
        (0..DIMENSIONS)
            .map(|i| {
                let d = space[i].distance(target[i]);
                d * d
            })
            .sum()
    }

    fn target_of(sample: ClimateSample) -> [i64; DIMENSIONS] {
        [
            quantize(sample.temperature as f32),
            quantize(sample.humidity as f32),
            quantize(sample.continentalness as f32),
            quantize(sample.erosion as f32),
            quantize(sample.depth as f32),
            quantize(sample.weirdness as f32),
            0,
        ]
    }

    #[test]
    fn nearest_picks_closest_temperature() {
        let source = MultiNoiseSource::Explicit {
            biomes: vec![entry("cold", -1.0), entry("warm", 1.0)],
        };
        let tree = BiomeSearchTree::from_source(&source).unwrap();
        assert_eq!(tree.nearest(sample_with_temp(-0.9)).unwrap().path(), "cold");
        assert_eq!(tree.nearest(sample_with_temp(0.9)).unwrap().path(), "warm");
    }

    #[test]
    fn preset_source_resolves_to_the_hardcoded_table() {
        let source = MultiNoiseSource::Preset {
            preset: ResourceLocation::minecraft("overworld"),
        };
        let tree = BiomeSearchTree::from_source(&source).expect("overworld preset is known");
        assert!(!tree.is_empty());

        let unknown = MultiNoiseSource::Preset {
            preset: ResourceLocation::new("modid", "custom"),
        };
        assert!(BiomeSearchTree::from_source(&unknown).is_none());
    }

    #[test]
    fn point_inside_a_range_has_zero_distance() {
        let params = ClimateParameters {
            temperature: ClimateParam::Range([-1.0, 1.0]),
            humidity: ClimateParam::Single(0.0),
            continentalness: ClimateParam::Single(0.0),
            erosion: ClimateParam::Single(0.0),
            depth: ClimateParam::Single(0.0),
            weirdness: ClimateParam::Single(0.0),
            offset: 0.0,
        };
        let space = parameter_space(&params);
        assert_eq!(space[0].distance(quantize(0.5)), 0);
    }

    /// The whole point of the tree is that pruning cannot change the answer. Checked against
    /// the real overworld preset, which is where a wrong bucket split would actually show.
    #[test]
    fn tree_agrees_with_brute_force_on_the_overworld_preset() {
        let entries = oxide_datapack::preset_entries(&ResourceLocation::minecraft("overworld"))
            .expect("overworld preset is known");
        let tree = BiomeSearchTree::from_entries(entries.clone());

        // A deterministic sweep of the climate space rather than random probes, so a failure
        // reproduces exactly.
        let axis = [-1.0, -0.6, -0.21, 0.0, 0.17, 0.45, 0.9];
        let mut checked = 0;
        for (i, &t) in axis.iter().enumerate() {
            for (j, &h) in axis.iter().enumerate() {
                for (k, &c) in axis.iter().enumerate() {
                    let sample = ClimateSample {
                        temperature: t,
                        humidity: h,
                        continentalness: c,
                        erosion: axis[(i + j) % axis.len()],
                        depth: axis[(j + k) % axis.len()],
                        weirdness: axis[(i + k) % axis.len()],
                    };
                    let target = target_of(sample);
                    let picked = tree.nearest(sample).expect("non-empty tree");
                    let picked_distance = entries
                        .iter()
                        .filter(|e| &e.biome == picked)
                        .map(|e| distance_to(&parameter_space(&e.parameters), &target))
                        .min()
                        .expect("picked biome is in the list");
                    assert_eq!(
                        picked_distance,
                        best_distance(&entries, &target),
                        "climate {sample:?} picked {picked}"
                    );
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, axis.len().pow(3));
    }

    /// The recursive `Node.search` this module ran before the tree was flattened -- what
    /// defines the shipped digest, ties included.
    fn reference_search<'a>(
        node: &'a Node,
        target: &[i64; DIMENSIONS],
        candidate: Option<&'a Node>,
    ) -> Option<&'a Node> {
        match node {
            Node::Leaf { .. } => Some(node),
            Node::SubTree { children, .. } => {
                let mut closest = candidate;
                let mut min_distance = closest.map_or(i64::MAX, |c| distance_to(c.space(), target));
                for child in children {
                    let child_distance = distance_to(child.space(), target);
                    if min_distance <= child_distance {
                        continue;
                    }
                    let Some(leaf) = reference_search(child, target, closest) else {
                        continue;
                    };
                    let leaf_distance = distance_to(leaf.space(), target);
                    if min_distance <= leaf_distance {
                        continue;
                    }
                    min_distance = leaf_distance;
                    closest = Some(leaf);
                }
                closest
            }
        }
    }

    /// The flattened search must return the very leaf the recursive one does -- not just one
    /// at the same distance -- because ties between overlapping entries are resolved by visit
    /// order. Leaves are told apart by their position in a breadth-first walk, which is the
    /// order `flatten` assigns biome indices in.
    #[test]
    fn flat_search_returns_the_recursive_searchs_leaf() {
        let entries = oxide_datapack::preset_entries(&ResourceLocation::minecraft("overworld"))
            .expect("overworld preset is known");
        let tree = BiomeSearchTree::from_entries(entries.clone());
        let reference = build(
            entries
                .iter()
                .map(|e| Node::Leaf {
                    space: parameter_space(&e.parameters),
                    biome: e.biome.clone(),
                })
                .collect(),
        );
        let mut leaves: Vec<*const Node> = Vec::new();
        let mut queue = std::collections::VecDeque::from([&reference]);
        while let Some(node) = queue.pop_front() {
            match node {
                Node::Leaf { .. } => leaves.push(node),
                Node::SubTree { children, .. } => queue.extend(children.iter()),
            }
        }

        let axis = [
            -1.2, -0.8, -0.45, -0.2, -0.11, 0.0, 0.03, 0.2, 0.3, 0.55, 0.8, 1.1,
        ];
        let depth = [-0.3, 0.0, 0.1, 0.2, 0.9, 1.1];
        let mut index = 0usize;
        for &t in &axis {
            for &h in &axis {
                for &c in &axis {
                    for &d in &depth {
                        let sample = ClimateSample {
                            temperature: t,
                            humidity: h,
                            continentalness: c,
                            erosion: axis[index % axis.len()],
                            depth: d,
                            weirdness: axis[(index / 7) % axis.len()],
                        };
                        index += 1;
                        let target = target_of(sample);
                        let expected = reference_search(&reference, &target, None).unwrap();
                        let expected_index = leaves
                            .iter()
                            .position(|&leaf| std::ptr::eq(leaf, expected))
                            .unwrap();
                        let picked = tree.nearest(sample).unwrap();
                        assert!(
                            std::ptr::eq(picked, &tree.biomes[expected_index]),
                            "climate {sample:?}"
                        );
                    }
                }
            }
        }
    }

    /// A hint only bounds the search; it must never change which leaf comes back, whether it
    /// names the answer, a near miss, a far leaf, or nothing.
    #[test]
    fn hinted_search_returns_the_unhinted_leaf() {
        let entries = oxide_datapack::preset_entries(&ResourceLocation::minecraft("overworld"))
            .expect("overworld preset is known");
        let tree = BiomeSearchTree::from_entries(entries);
        let leaves = tree.biomes.len() as u32;
        let axis = [
            -1.2, -0.8, -0.45, -0.2, -0.11, 0.0, 0.03, 0.2, 0.3, 0.55, 0.8, 1.1,
        ];
        let depth = [-0.3, 0.0, 0.1, 0.2, 0.9, 1.1];
        let mut index = 0usize;
        let mut chained = u32::MAX;
        for &t in &axis {
            for &h in &axis {
                for &c in &axis {
                    for &d in &depth {
                        let sample = ClimateSample {
                            temperature: t,
                            humidity: h,
                            continentalness: c,
                            erosion: axis[index % axis.len()],
                            depth: d,
                            weirdness: axis[(index / 7) % axis.len()],
                        };
                        index += 1;
                        let expected = tree.nearest(sample).unwrap();
                        let mut answer = u32::MAX;
                        tree.nearest_with_hint(sample, &mut answer);
                        for mut hint in [
                            u32::MAX,
                            chained,
                            answer,
                            (index as u32 * 7919) % leaves,
                            0,
                            leaves - 1,
                            leaves + 5,
                        ] {
                            let picked = tree.nearest_with_hint(sample, &mut hint).unwrap();
                            assert!(std::ptr::eq(picked, expected), "climate {sample:?}");
                            assert_eq!(hint, answer);
                        }
                        tree.nearest_with_hint(sample, &mut chained);
                    }
                }
            }
        }
    }

    #[test]
    fn overworld_preset_returns_more_than_one_biome() {
        // The regression this guards: an unresolved preset left every position on plains.
        let entries =
            oxide_datapack::preset_entries(&ResourceLocation::minecraft("overworld")).unwrap();
        let tree = BiomeSearchTree::from_entries(entries);
        let mut seen = std::collections::HashSet::new();
        let axis = [-0.9, -0.4, 0.0, 0.35, 0.8];
        for &t in &axis {
            for &c in &axis {
                for &e in &axis {
                    seen.insert(
                        tree.nearest(ClimateSample {
                            temperature: t,
                            humidity: 0.0,
                            continentalness: c,
                            erosion: e,
                            depth: 0.0,
                            weirdness: 0.0,
                        })
                        .unwrap()
                        .to_string(),
                    );
                }
            }
        }
        assert!(seen.len() > 5, "only saw {seen:?}");
    }
}
