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

    /// Sum of squared per-axis distances -- vanilla's `Node.distance`, which is also the
    /// default `DistanceMetric`.
    fn distance(&self, target: &[i64; DIMENSIONS]) -> i64 {
        let space = self.space();
        let mut total = 0i64;
        for i in 0..DIMENSIONS {
            let d = space[i].distance(target[i]);
            total += d * d;
        }
        total
    }

    /// Returns the closest leaf, seeded with `candidate` so a branch that cannot beat it is
    /// skipped whole. `None` for `candidate` means "nothing found yet".
    fn search<'a>(
        &'a self,
        target: &[i64; DIMENSIONS],
        candidate: Option<&'a Node>,
    ) -> Option<&'a Node> {
        match self {
            Node::Leaf { .. } => Some(self),
            Node::SubTree { children, .. } => {
                let mut closest = candidate;
                let mut min_distance = closest.map_or(i64::MAX, |c| c.distance(target));
                for child in children {
                    let child_distance = child.distance(target);
                    // `<=` keeps the incumbent on a tie, exactly as vanilla does.
                    if min_distance <= child_distance {
                        continue;
                    }
                    let leaf = child.search(target, closest);
                    let Some(leaf) = leaf else { continue };
                    let leaf_distance = if std::ptr::eq(child, leaf) {
                        child_distance
                    } else {
                        leaf.distance(target)
                    };
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
    root: Option<Node>,
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
        let root = if entries.is_empty() {
            None
        } else {
            Some(build(
                entries
                    .iter()
                    .map(|e| Node::Leaf {
                        space: parameter_space(&e.parameters),
                        biome: e.biome.clone(),
                    })
                    .collect(),
            ))
        };
        Self { entries, root }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn nearest(&self, sample: ClimateSample) -> Option<&BiomeId> {
        let target = [
            quantize(sample.temperature as f32),
            quantize(sample.humidity as f32),
            quantize(sample.continentalness as f32),
            quantize(sample.erosion as f32),
            quantize(sample.depth as f32),
            quantize(sample.weirdness as f32),
            0,
        ];
        match self.root.as_ref()?.search(&target, None)? {
            Node::Leaf { biome, .. } => Some(biome),
            Node::SubTree { .. } => None,
        }
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
