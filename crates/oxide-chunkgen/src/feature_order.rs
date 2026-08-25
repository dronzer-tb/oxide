//! The global feature ordering every decoration seed depends on.
//!
//! Vanilla seeds each feature with `setFeatureSeed(decorationSeed, globalIndexOfFeature, step)`,
//! and `globalIndexOfFeature` is a feature's position in a single ordering built across *every*
//! biome at once -- `net.minecraft.world.level.biome.FeatureSorter`. Get the ordering wrong by
//! one and every feature after it draws a different seed, so the world diverges from vanilla
//! everywhere, not just where the mistake was.
//!
//! Ported from the decompiled `FeatureSorter#buildFeaturesPerStep` and `Graph#depthFirstSearch`.
//! The cycle-reduction path vanilla runs on failure is not ported: it exists only to name which
//! biomes are involved in a cycle, and this returns the cycle as an error instead.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::Hash;

/// A feature at one decoration step, ordered the way vanilla's `TreeMap` comparator orders it.
///
/// Ordered by `(step, first_seen_index)` and nothing else -- deliberately, because that is what
/// vanilla compares. The feature itself is not part of the ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FeatureData {
    step: usize,
    first_seen: usize,
}

/// A cycle in the ordering constraints: two biomes list the same pair of features in opposite
/// orders, so no single global ordering satisfies both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureOrderCycle {
    /// The step the cycle was found at, for the error message.
    pub step: usize,
}

impl std::fmt::Display for FeatureOrderCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "feature order cycle at decoration step {}: two biomes list the same features in \
             conflicting orders",
            self.step
        )
    }
}

impl std::error::Error for FeatureOrderCycle {}

/// Builds the per-step feature ordering.
///
/// `biome_features` is one entry per biome, each a list of decoration steps, each a list of the
/// placed features that biome runs at that step -- the shape of a biome's `features` field.
///
/// `K` must identify a feature *by content*, not by registry id. Vanilla's `PlacedFeature` is a
/// record, so two registry entries with identical content are the same key and share one index;
/// the vanilla export really does contain four such pairs (`oak`/`oak_checked` and friends).
/// Keying by id instead would insert four extra indices and shift every feature after them.
///
/// Returns, per step, the features in the order whose positions are the `globalIndexOfFeature`
/// vanilla passes to `setFeatureSeed`.
pub fn build_features_per_step<K: Eq + Hash + Clone>(
    biome_features: &[Vec<Vec<K>>],
) -> Result<Vec<Vec<K>>, FeatureOrderCycle> {
    let mut first_seen: HashMap<K, usize> = HashMap::new();
    let mut features_by_index: Vec<K> = Vec::new();
    let mut edges: BTreeMap<FeatureData, BTreeSet<FeatureData>> = BTreeMap::new();
    let mut max_step = 0usize;

    for biome in biome_features {
        // One flat list per biome, in step order and then in the order the biome lists them.
        // The constraint a biome contributes is only that each feature precedes the next one in
        // *its own* list -- not that it precedes every later feature.
        let mut in_this_biome: Vec<FeatureData> = Vec::new();
        max_step = max_step.max(biome.len());

        for (step, features_at_step) in biome.iter().enumerate() {
            for feature in features_at_step {
                let index = match first_seen.get(feature) {
                    Some(&index) => index,
                    None => {
                        let index = features_by_index.len();
                        first_seen.insert(feature.clone(), index);
                        features_by_index.push(feature.clone());
                        index
                    }
                };
                in_this_biome.push(FeatureData {
                    step,
                    first_seen: index,
                });
            }
        }

        for (i, &node) in in_this_biome.iter().enumerate() {
            let successors = edges.entry(node).or_default();
            if let Some(&next) = in_this_biome.get(i + 1) {
                successors.insert(next);
            }
        }
    }

    // Post-order DFS over the vertices in (step, index) order, then reversed -- vanilla's
    // `Graph.depthFirstSearch` collects in reverse topological order and `FeatureSorter`
    // reverses at the end.
    let mut discovered: BTreeSet<FeatureData> = BTreeSet::new();
    let mut visiting: BTreeSet<FeatureData> = BTreeSet::new();
    let mut reverse_topological: Vec<FeatureData> = Vec::new();

    let vertices: Vec<FeatureData> = edges.keys().copied().collect();
    for vertex in vertices {
        if discovered.contains(&vertex) {
            continue;
        }
        if depth_first_search(
            &edges,
            &mut discovered,
            &mut visiting,
            &mut reverse_topological,
            vertex,
        ) {
            return Err(FeatureOrderCycle { step: vertex.step });
        }
    }
    reverse_topological.reverse();

    let mut per_step: Vec<Vec<K>> = vec![Vec::new(); max_step];
    for node in reverse_topological {
        per_step[node.step].push(features_by_index[node.first_seen].clone());
    }
    Ok(per_step)
}

/// Returns `true` when a cycle is found, matching `Graph#depthFirstSearch`.
///
/// Recursive like vanilla's, and bounded the same way: depth cannot exceed the number of
/// distinct (feature, step) pairs, which is in the hundreds for a real datapack.
fn depth_first_search(
    edges: &BTreeMap<FeatureData, BTreeSet<FeatureData>>,
    discovered: &mut BTreeSet<FeatureData>,
    visiting: &mut BTreeSet<FeatureData>,
    reverse_topological: &mut Vec<FeatureData>,
    current: FeatureData,
) -> bool {
    if discovered.contains(&current) {
        return false;
    }
    if visiting.contains(&current) {
        return true;
    }
    visiting.insert(current);

    if let Some(successors) = edges.get(&current) {
        for &next in successors {
            if depth_first_search(edges, discovered, visiting, reverse_topological, next) {
                return true;
            }
        }
    }

    visiting.remove(&current);
    discovered.insert(current);
    reverse_topological.push(current);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One biome's list is the ordering constraint, and it is preserved.
    #[test]
    fn keeps_a_single_biome_order() {
        let biomes = vec![vec![vec!["a", "b"], vec!["c"]]];
        let steps = build_features_per_step(&biomes).expect("no cycle");
        assert_eq!(steps, vec![vec!["a", "b"], vec!["c"]]);
    }

    /// Two biomes agreeing on a shared feature merge into one ordering rather than listing it
    /// twice -- the shared feature gets one index, which is the whole point of the global sort.
    #[test]
    fn merges_biomes_sharing_a_feature() {
        let biomes = vec![
            vec![vec!["shared", "only_first"]],
            vec![vec!["shared", "only_second"]],
        ];
        let steps = build_features_per_step(&biomes).expect("no cycle");
        assert_eq!(steps.len(), 1);
        let step = &steps[0];
        assert_eq!(step.len(), 3, "shared feature must appear once: {step:?}");
        let position = |name| step.iter().position(|f| *f == name).unwrap();
        assert!(position("shared") < position("only_first"));
        assert!(position("shared") < position("only_second"));
    }

    /// One biome listing the same feature twice at the same step is a cycle, not a duplicate to
    /// be tidied away: the edge from the feature to "the next one in this list" points at
    /// itself, and vanilla's DFS reports a self-edge as a cycle. Pinned because collapsing it
    /// silently would look like the more helpful behaviour and would not be vanilla's.
    #[test]
    fn a_feature_listed_twice_in_one_biome_is_a_cycle() {
        let biomes = vec![vec![vec!["same", "same", "other"]]];
        assert!(build_features_per_step(&biomes).is_err());
    }

    /// The same feature at two different steps is two vertices, not one.
    #[test]
    fn one_feature_at_two_steps_appears_in_both() {
        let biomes = vec![vec![vec!["a"], vec!["a"]]];
        let steps = build_features_per_step(&biomes).expect("no cycle");
        assert_eq!(steps, vec![vec!["a"], vec!["a"]]);
    }

    /// Biomes demanding opposite orders cannot be satisfied, and that is an error rather than an
    /// arbitrary pick -- an arbitrary pick would silently reseed every later feature.
    #[test]
    fn conflicting_biomes_are_a_cycle() {
        let biomes = vec![vec![vec!["a", "b"]], vec![vec!["b", "a"]]];
        assert!(build_features_per_step(&biomes).is_err());
    }

    /// Steps are independent: a step no biome reaches is still present and empty, because the
    /// step index is what `setFeatureSeed` multiplies by 10000.
    #[test]
    fn empty_steps_are_kept() {
        let biomes = vec![vec![vec!["a"], vec![], vec!["b"]]];
        let steps = build_features_per_step(&biomes).expect("no cycle");
        assert_eq!(steps, vec![vec!["a"], vec![], vec!["b"]]);
    }
}
