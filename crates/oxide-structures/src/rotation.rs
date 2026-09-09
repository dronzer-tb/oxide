//! Rotations, mirrors, and the position transform every jigsaw piece is placed through.
//!
//! Ported from 26.2's `net.minecraft.world.level.block.Rotation`, `Mirror`, and
//! `StructureTemplate.transform`. The transform is the load-bearing one: it decides where a
//! rotated piece's blocks and jigsaw connectors land, so an axis swapped here is a village whose
//! houses face into each other.

use oxide_core::{BlockPos, BlockState, RandomSource};

/// The six block faces, in vanilla's `Direction` declaration order — `Direction.values()` order
/// is observable through `getShuffled`-style helpers, so it is part of the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    pub const VALUES: [Direction; 6] = [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];

    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "down" => Direction::Down,
            "up" => Direction::Up,
            "north" => Direction::North,
            "south" => Direction::South,
            "west" => Direction::West,
            "east" => Direction::East,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Direction::Down => "down",
            Direction::Up => "up",
            Direction::North => "north",
            Direction::South => "south",
            Direction::West => "west",
            Direction::East => "east",
        }
    }

    pub fn step_x(self) -> i32 {
        match self {
            Direction::West => -1,
            Direction::East => 1,
            _ => 0,
        }
    }

    pub fn step_y(self) -> i32 {
        match self {
            Direction::Down => -1,
            Direction::Up => 1,
            _ => 0,
        }
    }

    pub fn step_z(self) -> i32 {
        match self {
            Direction::North => -1,
            Direction::South => 1,
            _ => 0,
        }
    }

    pub fn opposite(self) -> Self {
        match self {
            Direction::Down => Direction::Up,
            Direction::Up => Direction::Down,
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
            Direction::East => Direction::West,
        }
    }

    /// Vertical faces have no clockwise neighbour; vanilla throws, this returns them unchanged
    /// because every caller here has already checked the axis.
    pub fn clockwise(self) -> Self {
        match self {
            Direction::North => Direction::East,
            Direction::East => Direction::South,
            Direction::South => Direction::West,
            Direction::West => Direction::North,
            vertical => vertical,
        }
    }

    pub fn counterclockwise(self) -> Self {
        match self {
            Direction::North => Direction::West,
            Direction::West => Direction::South,
            Direction::South => Direction::East,
            Direction::East => Direction::North,
            vertical => vertical,
        }
    }

    pub fn is_vertical(self) -> bool {
        matches!(self, Direction::Down | Direction::Up)
    }
}

/// `Rotation`, in vanilla's declaration order — `Rotation.values()` is what `getRandom` indexes
/// and what `getShuffled` copies, so the order is observable in world output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Rotation {
    #[default]
    None,
    Clockwise90,
    Clockwise180,
    Counterclockwise90,
}

impl Rotation {
    pub const VALUES: [Rotation; 4] = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::Counterclockwise90,
    ];

    /// `Rotation.rotate(Direction)`: the Y axis is fixed, the horizontals turn.
    pub fn rotate(self, direction: Direction) -> Direction {
        if direction.is_vertical() {
            return direction;
        }
        match self {
            Rotation::Clockwise90 => direction.clockwise(),
            Rotation::Clockwise180 => direction.opposite(),
            Rotation::Counterclockwise90 => direction.counterclockwise(),
            Rotation::None => direction,
        }
    }

    /// `Rotation.getRandom` — `Util.getRandom(values(), random)`, one `nextInt(4)`.
    pub fn get_random(random: &mut impl RandomSource) -> Self {
        Self::VALUES[random.next_int_bounded(Self::VALUES.len() as i32) as usize]
    }

    /// `Rotation.getShuffled` — `Util.shuffledCopy(values(), random)`.
    pub fn get_shuffled(random: &mut impl RandomSource) -> Vec<Rotation> {
        let mut values = Self::VALUES.to_vec();
        shuffle(&mut values, random);
        values
    }
}

/// `Mirror`, in vanilla's declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mirror {
    #[default]
    None,
    /// Negates Z.
    LeftRight,
    /// Negates X.
    FrontBack,
}

/// `Util.shuffle`: a Fisher-Yates walking *down* from the end, drawing `nextInt(i)` for each
/// `i` from `len` to 2. The draw count and order are observable, so this is transcribed rather
/// than handed to a shuffle from the standard library.
pub fn shuffle<T>(list: &mut [T], random: &mut impl RandomSource) {
    let mut i = list.len();
    while i > 1 {
        let swap_to = random.next_int_bounded(i as i32) as usize;
        list.swap(i - 1, swap_to);
        i -= 1;
    }
}

/// `StructureTemplate.transform`: mirror first, then rotate about `pivot`.
///
/// Note the asymmetry in vanilla's rotation arms — the 90-degree cases mix `pivot.x` and
/// `pivot.z` rather than using each axis' own pivot component. Transcribed, not simplified: for
/// the square pivots templates actually use it makes no difference, and "fixing" it would be a
/// divergence.
pub fn transform(pos: BlockPos, mirror: Mirror, rotation: Rotation, pivot: BlockPos) -> BlockPos {
    let mut x = pos.x;
    let y = pos.y;
    let mut z = pos.z;
    let mirrored = match mirror {
        Mirror::LeftRight => {
            z = -z;
            true
        }
        Mirror::FrontBack => {
            x = -x;
            true
        }
        Mirror::None => false,
    };

    let px = pivot.x;
    let pz = pivot.z;
    match rotation {
        Rotation::Counterclockwise90 => BlockPos::new(px - pz + z, y, px + pz - x),
        Rotation::Clockwise90 => BlockPos::new(px + pz - z, y, pz - px + x),
        Rotation::Clockwise180 => BlockPos::new(px + px - x, y, pz + pz - z),
        Rotation::None => {
            if mirrored {
                BlockPos::new(x, y, z)
            } else {
                pos
            }
        }
    }
}

/// Turns a block state's direction-bearing properties to match `rotation`.
///
/// Vanilla dispatches this per block (`Block.rotate`), so what is here is the shared vocabulary
/// those implementations are written in: `facing`, `orientation` (front/top pairs, which is what
/// jigsaw blocks carry and what connector matching reads), `axis`, the 16-step `rotation`
/// property, and the four connection booleans. Blocks whose rotation is not expressible in those
/// terms — rails' `shape`, big dripleaf tilt, a few redstone components — keep their state and
/// so can come out facing the wrong way. That is a cosmetic divergence inside a piece, not a
/// structural one: it cannot move a connector, because a connector's facing lives in
/// `orientation`, which is handled.
pub fn rotate_block_state(state: &BlockState, rotation: Rotation) -> BlockState {
    if rotation == Rotation::None || state.properties.is_empty() {
        return state.clone();
    }
    let mut rotated = state.clone();

    if let Some(facing) = state.properties.get("facing") {
        if let Some(direction) = Direction::from_name(facing) {
            rotated
                .properties
                .insert("facing".into(), rotation.rotate(direction).name().into());
        }
    }

    if let Some(orientation) = state.properties.get("orientation") {
        if let Some((front, top)) = parse_front_and_top(orientation) {
            let turned = format_front_and_top(rotation.rotate(front), rotation.rotate(top));
            rotated.properties.insert("orientation".into(), turned);
        }
    }

    if let Some(axis) = state.properties.get("axis") {
        let swapped = matches!(
            rotation,
            Rotation::Clockwise90 | Rotation::Counterclockwise90
        );
        if swapped {
            let turned = match axis.as_str() {
                "x" => Some("z"),
                "z" => Some("x"),
                _ => None,
            };
            if let Some(turned) = turned {
                rotated.properties.insert("axis".into(), turned.into());
            }
        }
    }

    // Signs and banners: 16 steps of 22.5 degrees, so a quarter turn is four steps.
    if let Some(steps) = state.properties.get("rotation") {
        if let Ok(steps) = steps.parse::<i32>() {
            let quarter = match rotation {
                Rotation::Clockwise90 => 4,
                Rotation::Clockwise180 => 8,
                Rotation::Counterclockwise90 => 12,
                Rotation::None => 0,
            };
            rotated
                .properties
                .insert("rotation".into(), ((steps + quarter) % 16).to_string());
        }
    }

    // Fences, walls, panes: the connection flags move with the block.
    let connections = [
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];
    if connections
        .iter()
        .any(|d| state.properties.contains_key(d.name()))
    {
        for direction in connections {
            if let Some(value) = state.properties.get(direction.name()) {
                rotated
                    .properties
                    .insert(rotation.rotate(direction).name().into(), value.clone());
            }
        }
    }

    rotated
}

/// `FrontAndTop` is serialised as `<front>_<top>`, e.g. `north_up`, `up_east`.
pub fn parse_front_and_top(value: &str) -> Option<(Direction, Direction)> {
    let (front, top) = value.split_once('_')?;
    Some((Direction::from_name(front)?, Direction::from_name(top)?))
}

pub fn format_front_and_top(front: Direction, top: Direction) -> String {
    format!("{}_{}", front.name(), top.name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{LegacyRandom, ResourceLocation};

    #[test]
    fn quarter_turns_compose_into_a_full_circle() {
        let mut direction = Direction::North;
        for _ in 0..4 {
            direction = Rotation::Clockwise90.rotate(direction);
        }
        assert_eq!(direction, Direction::North);
        assert_eq!(Rotation::Clockwise90.rotate(Direction::Up), Direction::Up);
    }

    #[test]
    fn transform_turns_the_expected_way_about_the_origin() {
        let pivot = BlockPos::new(0, 0, 0);
        let east = BlockPos::new(1, 0, 0);
        // Clockwise seen from above with +x east and +z south: east goes to south.
        assert_eq!(
            transform(east, Mirror::None, Rotation::Clockwise90, pivot),
            BlockPos::new(0, 0, 1)
        );
        assert_eq!(
            transform(east, Mirror::None, Rotation::Counterclockwise90, pivot),
            BlockPos::new(0, 0, -1)
        );
        assert_eq!(
            transform(east, Mirror::None, Rotation::Clockwise180, pivot),
            BlockPos::new(-1, 0, 0)
        );
    }

    #[test]
    fn shuffle_consumes_one_draw_per_element_past_the_first() {
        let mut counting = LegacyRandom::new(42);
        let mut list = [0, 1, 2, 3, 4];
        shuffle(&mut list, &mut counting);
        let mut expected = LegacyRandom::new(42);
        for i in (2..=5).rev() {
            expected.next_int_bounded(i);
        }
        // Both generators have consumed the same number of draws, so their next values agree.
        assert_eq!(counting.next_int(), expected.next_int());
    }

    #[test]
    fn a_jigsaw_orientation_turns_front_and_top_together() {
        let mut state = BlockState::new(ResourceLocation::minecraft("jigsaw"));
        state
            .properties
            .insert("orientation".into(), "north_up".into());
        let turned = rotate_block_state(&state, Rotation::Clockwise90);
        assert_eq!(turned.properties.get("orientation").unwrap(), "east_up");
    }
}
