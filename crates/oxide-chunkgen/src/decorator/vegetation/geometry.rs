//! Directions, block-position arithmetic, `Util.shuffle` and Java's `HashSet<BlockPos>` order.
//!
//! Vegetation features iterate sets of positions and draw from the random stream per element, so
//! the *order* of iteration is part of the output. Vanilla's sets are `java.util.HashSet`, whose
//! order is a function of `BlockPos.hashCode`; [`PosSet`] reproduces it.

use std::collections::HashSet;

use oxide_core::{BlockPos, RandomSource};

/// `net.minecraft.core.Direction`, in the order of `Direction.values()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dir {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Dir {
    /// `Direction.values()`.
    pub const ALL: [Dir; 6] = [
        Dir::Down,
        Dir::Up,
        Dir::North,
        Dir::South,
        Dir::West,
        Dir::East,
    ];
    /// `Direction.Plane.HORIZONTAL`.
    pub const HORIZONTAL: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

    pub fn step(self) -> (i32, i32, i32) {
        match self {
            Dir::Down => (0, -1, 0),
            Dir::Up => (0, 1, 0),
            Dir::North => (0, 0, -1),
            Dir::South => (0, 0, 1),
            Dir::West => (-1, 0, 0),
            Dir::East => (1, 0, 0),
        }
    }

    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    /// The block-state property that names this face (`north`, `up`, ...).
    pub fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }

    /// The axis as an index: 0 = x, 1 = y, 2 = z.
    pub fn axis(self) -> u8 {
        match self {
            Dir::West | Dir::East => 0,
            Dir::Down | Dir::Up => 1,
            Dir::North | Dir::South => 2,
        }
    }
}

/// `pos.offset(dx, dy, dz)`.
pub(super) fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(
        pos.x.wrapping_add(dx),
        pos.y.wrapping_add(dy),
        pos.z.wrapping_add(dz),
    )
}

/// `pos.relative(dir, distance)`.
pub(super) fn relative(pos: BlockPos, dir: Dir, distance: i32) -> BlockPos {
    let (dx, dy, dz) = dir.step();
    offset(
        pos,
        dx.wrapping_mul(distance),
        dy.wrapping_mul(distance),
        dz.wrapping_mul(distance),
    )
}

/// `Vec3i.distManhattan`, including the float accumulation vanilla does it in.
pub(super) fn dist_manhattan(a: BlockPos, b: BlockPos) -> i32 {
    let d = |from: i32, to: i32| to.wrapping_sub(from).wrapping_abs() as f32;
    (d(a.x, b.x) + d(a.y, b.y) + d(a.z, b.z)) as i32
}

/// `Util.shuffle`: Fisher-Yates from the end, one `nextInt(i)` per step.
pub(super) fn shuffle<T>(values: &mut [T], random: &mut impl RandomSource) {
    for i in (2..=values.len()).rev() {
        let swap = random.next_int_bounded(i as i32) as usize;
        values.swap(i - 1, swap);
    }
}

/// A `java.util.HashSet<BlockPos>` that iterates as Java does.
///
/// Java walks buckets in index order and, inside a bucket, in insertion order. The bucket index
/// is the table-size mask of `h ^ (h >>> 16)` with `h = BlockPos.hashCode()`, and the table
/// starts at 16 slots and doubles whenever the size passes three quarters of it. A resize splits
/// each bucket while keeping relative order, so the final order is simply a stable sort of the
/// insertion order by bucket under the final table size, which is all this stores.
///
/// PARITY-CHECK: a bucket of eight or more entries is treeified by Java, which can reorder it.
/// Patch sets hold at most a few hundred positions spread over several hundred buckets, so this
/// is not expected to occur; confirm against a real run if a patch ever disagrees.
#[derive(Debug, Default)]
pub(super) struct PosSet {
    seen: HashSet<BlockPos>,
    order: Vec<BlockPos>,
}

impl PosSet {
    pub fn insert(&mut self, pos: BlockPos) -> bool {
        let added = self.seen.insert(pos);
        if added {
            self.order.push(pos);
        }
        added
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Positions in Java iteration order.
    pub fn iter_java_order(&self) -> Vec<BlockPos> {
        let mut capacity = 16usize;
        while self.order.len() > capacity / 4 * 3 {
            capacity *= 2;
        }
        let mut entries = self.order.clone();
        entries.sort_by_key(|pos| {
            let h = pos
                .y
                .wrapping_add(pos.z.wrapping_mul(31))
                .wrapping_mul(31)
                .wrapping_add(pos.x) as u32;
            ((h ^ (h >> 16)) as usize) & (capacity - 1)
        });
        entries
    }
}
