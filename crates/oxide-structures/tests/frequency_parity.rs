//! Cross-checks the four frequency reducers against real Java output.
//!
//! Vectors from `tests/data/FreqOracle.java`, a verbatim transcription of the decompiled
//! reducers plus the two `WorldgenRandom` seed derivations they use, standalone so it runs on a
//! plain JDK. Regenerate:
//!
//! ```text
//! java crates/oxide-structures/tests/data/FreqOracle.java > crates/oxide-structures/tests/data/freq_vectors.txt
//! ```
//!
//! 2880 cases over six seeds, four salts, six chunk positions and five probabilities. The point
//! is the three near-identical methods: `default`, `legacy_type_2` and `legacy_type_3` differ
//! only in argument order, salt source and float-vs-double, and reading them as the same
//! function is the easy mistake.

use oxide_datapack::FrequencyReductionMethod;
use oxide_structures::should_generate;

#[test]
fn matches_java_for_every_vector() {
    let text = include_str!("data/freq_vectors.txt");
    let mut checked = 0usize;

    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let at = line_number + 1;

        let (method, seed, salt, x, z, probability, want) = match f[0] {
            "def" => (
                FrequencyReductionMethod::Default,
                f[1].parse::<i64>().unwrap(),
                f[2].parse::<i32>().unwrap(),
                f[3].parse::<i32>().unwrap(),
                f[4].parse::<i32>().unwrap(),
                f32::from_bits(f[5].parse::<i32>().unwrap() as u32),
                f[6].parse::<bool>().unwrap(),
            ),
            kind => {
                let method = match kind {
                    "t1" => FrequencyReductionMethod::LegacyType1,
                    "t2" => FrequencyReductionMethod::LegacyType2,
                    "t3" => FrequencyReductionMethod::LegacyType3,
                    other => panic!("unknown vector kind {other:?} on line {at}"),
                };
                (
                    method,
                    f[1].parse::<i64>().unwrap(),
                    // These three ignore the set's salt entirely; pass a value that would be
                    // visible in the output if one of them started reading it.
                    0x05EE_DBAD,
                    f[2].parse::<i32>().unwrap(),
                    f[3].parse::<i32>().unwrap(),
                    f32::from_bits(f[4].parse::<i32>().unwrap() as u32),
                    f[5].parse::<bool>().unwrap(),
                )
            }
        };

        let got = should_generate(method, seed, salt, x, z, probability, None);
        assert_eq!(
            got, want,
            "{method:?} seed {seed} salt {salt} chunk {x},{z} p {probability} on line {at}"
        );
        checked += 1;
    }

    assert!(
        checked > 2500,
        "expected the full vector set, checked {checked}"
    );
}
