//! Diffs the primitives jigsaw placement is built on against the real Minecraft 26.2 classes.
//!
//! Vectors from `tests/data/JigsawMathOracle.java`, which runs the game's own
//! `StructureTemplate.transform`, `Rotation.getShuffled`/`getRandom` and `Util.shuffle` — see
//! that file's header to regenerate.
//!
//! These three are where a jigsaw port goes wrong quietly. A transform with an axis swapped
//! still produces a plausible-looking building, just not the seed's building; a shuffle that
//! draws the wrong number of times produces a different village from the same seed and nothing
//! ever errors. Each shuffle vector therefore carries the generator's *next* value after the
//! shuffle, so a wrong draw count fails even when the permutation happens to match.

use oxide_core::{BlockPos, LegacyRandom, RandomSource};
use oxide_structures::rotation::{shuffle, transform, Mirror, Rotation};

fn mirror_of(ordinal: i32) -> Mirror {
    match ordinal {
        0 => Mirror::None,
        1 => Mirror::LeftRight,
        2 => Mirror::FrontBack,
        other => panic!("unknown mirror ordinal {other}"),
    }
}

fn rotation_of(ordinal: i32) -> Rotation {
    match ordinal {
        0 => Rotation::None,
        1 => Rotation::Clockwise90,
        2 => Rotation::Clockwise180,
        3 => Rotation::Counterclockwise90,
        other => panic!("unknown rotation ordinal {other}"),
    }
}

fn ordinal_of(rotation: Rotation) -> i32 {
    match rotation {
        Rotation::None => 0,
        Rotation::Clockwise90 => 1,
        Rotation::Clockwise180 => 2,
        Rotation::Counterclockwise90 => 3,
    }
}

#[test]
fn matches_java_for_every_vector() {
    let text = include_str!("data/jigsaw_math_vectors.txt");
    let (mut transforms, mut shuffles, mut rotation_shuffles, mut picks) = (0, 0, 0, 0);

    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let at = line_number + 1;
        let fields: Vec<&str> = line.split_whitespace().collect();

        match fields[0] {
            // t <mirror> <rotation> <pivotX> <pivotZ> <x> <y> <z> <resultX> <resultY> <resultZ>
            "t" => {
                let n: Vec<i32> = fields[1..].iter().map(|f| f.parse().unwrap()).collect();
                let result = transform(
                    BlockPos::new(n[4], n[5], n[6]),
                    mirror_of(n[0]),
                    rotation_of(n[1]),
                    BlockPos::new(n[2], 0, n[3]),
                );
                assert_eq!(
                    (result.x, result.y, result.z),
                    (n[7], n[8], n[9]),
                    "line {at}: transform({:?}, {:?}, {:?}, pivot ({}, {}))",
                    (n[4], n[5], n[6]),
                    mirror_of(n[0]),
                    rotation_of(n[1]),
                    n[2],
                    n[3]
                );
                transforms += 1;
            }
            // s <seed> <size> <permutation...> <next int after the shuffle>
            "s" => {
                let seed: i64 = fields[1].parse().unwrap();
                let size: usize = fields[2].parse().unwrap();
                let expected: Vec<i32> = fields[3..3 + size]
                    .iter()
                    .map(|f| f.parse().unwrap())
                    .collect();
                let expected_next: i32 = fields[3 + size].parse().unwrap();

                let mut random = LegacyRandom::new(seed);
                let mut list: Vec<i32> = (0..size as i32).collect();
                shuffle(&mut list, &mut random);
                assert_eq!(list, expected, "line {at}: shuffle(seed {seed}, size {size})");
                assert_eq!(
                    random.next_int(),
                    expected_next,
                    "line {at}: shuffle consumed the wrong number of draws"
                );
                shuffles += 1;
            }
            // r <seed> <four rotation ordinals> <next int after the shuffle>
            "r" => {
                let seed: i64 = fields[1].parse().unwrap();
                let expected: Vec<i32> = fields[2..6].iter().map(|f| f.parse().unwrap()).collect();
                let expected_next: i32 = fields[6].parse().unwrap();

                let mut random = LegacyRandom::new(seed);
                let shuffled: Vec<i32> = Rotation::get_shuffled(&mut random)
                    .into_iter()
                    .map(ordinal_of)
                    .collect();
                assert_eq!(shuffled, expected, "line {at}: Rotation::get_shuffled({seed})");
                assert_eq!(
                    random.next_int(),
                    expected_next,
                    "line {at}: get_shuffled consumed the wrong number of draws"
                );
                rotation_shuffles += 1;
            }
            // g <seed> <rotation ordinal> <next int after the draw>
            "g" => {
                let seed: i64 = fields[1].parse().unwrap();
                let expected: i32 = fields[2].parse().unwrap();
                let expected_next: i32 = fields[3].parse().unwrap();

                let mut random = LegacyRandom::new(seed);
                assert_eq!(
                    ordinal_of(Rotation::get_random(&mut random)),
                    expected,
                    "line {at}: Rotation::get_random({seed})"
                );
                assert_eq!(random.next_int(), expected_next, "line {at}: wrong draw count");
                picks += 1;
            }
            other => panic!("line {at}: unknown vector kind {other:?}"),
        }
    }

    assert_eq!(transforms, 336, "vector file is not the one this test expects");
    assert_eq!(shuffles, 40);
    assert_eq!(rotation_shuffles, 5);
    assert_eq!(picks, 5);
}
