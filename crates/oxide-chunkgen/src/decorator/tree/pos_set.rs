//! A `HashSet<BlockPos>` that iterates in the order Java's does.
//!
//! Tree generation keeps its trunk, foliage, root and decoration positions in `HashSet`s, and
//! the order those sets iterate in is observable: decorators draw from the random stream once
//! per position, and the leaf-distance pass pops positions in set order. A `HashSet` iterates by
//! bucket (`spread(hash) & (capacity - 1)`), and within a bucket in insertion order, which a
//! resize preserves. That is all this models: a power-of-two table that starts at 16 slots and
//! doubles when the size passes three quarters.
//!
//! PARITY-CHECK: Java converts a bucket holding 8 or more entries to a red-black tree once the
//! table has 64 slots, and tree bins iterate in a slightly different order. `BlockPos` hashes
//! (`(y + z * 31) * 31 + x`) spread tree positions thinly enough that this should not occur;
//! confirm against a real 26.2 run if a tree's decorations ever differ.

use oxide_core::BlockPos;

const INITIAL_CAPACITY: usize = 16;

/// `BlockPos.hashCode`: `Vec3i.hashCode`, `(y + z * 31) * 31 + x`.
fn java_hash(pos: BlockPos) -> i32 {
    pos.y
        .wrapping_add(pos.z.wrapping_mul(31))
        .wrapping_mul(31)
        .wrapping_add(pos.x)
}

/// `HashMap.hash`: the high half of the hash is folded into the low half.
fn bucket_of(pos: BlockPos, capacity: usize) -> usize {
    let hash = java_hash(pos) as u32;
    ((hash ^ (hash >> 16)) as usize) & (capacity - 1)
}

#[derive(Debug, Default)]
pub struct PosSet {
    buckets: Vec<Vec<BlockPos>>,
    len: usize,
    /// No bucket below this index is occupied, so popping does not rescan the empty prefix.
    first_hint: usize,
}

impl PosSet {
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn contains(&self, pos: BlockPos) -> bool {
        !self.buckets.is_empty() && self.buckets[bucket_of(pos, self.buckets.len())].contains(&pos)
    }

    /// `HashSet.add`. Returns whether the position was new.
    pub fn insert(&mut self, pos: BlockPos) -> bool {
        if self.buckets.is_empty() {
            self.buckets = vec![Vec::new(); INITIAL_CAPACITY];
        }
        let index = bucket_of(pos, self.buckets.len());
        if self.buckets[index].contains(&pos) {
            return false;
        }
        self.buckets[index].push(pos);
        self.first_hint = self.first_hint.min(index);
        self.len += 1;
        if self.len > self.buckets.len() / 4 * 3 {
            self.grow();
        }
        true
    }

    /// Doubles the table. Walking the old buckets in order and appending keeps each new bucket's
    /// entries in their old relative order, which is what Java's lo/hi split produces.
    fn grow(&mut self) {
        let capacity = self.buckets.len() * 2;
        let mut grown = vec![Vec::new(); capacity];
        for pos in self.buckets.iter().flatten() {
            grown[bucket_of(*pos, capacity)].push(*pos);
        }
        self.buckets = grown;
        self.first_hint = 0;
    }

    /// The positions in iteration order.
    pub fn to_vec(&self) -> Vec<BlockPos> {
        self.buckets.iter().flatten().copied().collect()
    }

    /// `iterator().next()` followed by `iterator.remove()`.
    pub fn pop_first(&mut self) -> Option<BlockPos> {
        while self.first_hint < self.buckets.len() {
            let bucket = &mut self.buckets[self.first_hint];
            if !bucket.is_empty() {
                self.len -= 1;
                return Some(bucket.remove(0));
            }
            self.first_hint += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterates_by_bucket_then_insertion_order() {
        let mut set = PosSet::default();
        // hash = x for y = z = 0, so buckets are x & 15.
        for x in [17, 3, 1, 33, 2] {
            set.insert(BlockPos::new(x, 0, 0));
        }
        let xs: Vec<i32> = set.to_vec().iter().map(|pos| pos.x).collect();
        assert_eq!(xs, vec![17, 1, 33, 2, 3]);
    }

    #[test]
    fn growing_keeps_every_member_and_pop_empties_in_order() {
        let mut set = PosSet::default();
        for x in 0..40 {
            assert!(set.insert(BlockPos::new(x, 0, 0)));
        }
        assert!(!set.insert(BlockPos::new(5, 0, 0)));
        assert!(set.contains(BlockPos::new(39, 0, 0)));
        let expected = set.to_vec();
        let mut popped = Vec::new();
        while let Some(pos) = set.pop_first() {
            popped.push(pos);
        }
        assert_eq!(popped, expected);
        assert!(set.is_empty());
    }
}
