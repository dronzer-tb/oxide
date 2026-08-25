//! Cross-checks the decoration seed derivations against real Java output.
//!
//! The vectors in `tests/data/decoration_seed_vectors.txt` were produced by
//! `tests/data/Oracle.java`, which is a verbatim transcription of the decompiled 26.1.2
//! `RandomSupport`, `Xoroshiro128PlusPlus`, `XoroshiroRandomSource` and `WorldgenRandom`,
//! standalone so it runs on a plain JDK without building Minecraft. Regenerate with:
//!
//! ```text
//! java crates/oxide-chunkgen/tests/data/Oracle.java > crates/oxide-chunkgen/tests/data/decoration_seed_vectors.txt
//! ```
//!
//! Testing this against a second Rust implementation would only prove the two agree with each
//! other. The point is that they agree with Java.

use oxide_chunkgen::decoration_seed::{
    set_decoration_seed, set_feature_seed, set_large_feature_seed, set_large_feature_with_salt,
};
use oxide_core::{RandomSource, Xoroshiro128PlusPlus};

fn source() -> Xoroshiro128PlusPlus {
    // Seeded immediately by every function under test, so the starting value is irrelevant.
    Xoroshiro128PlusPlus::new(0)
}

#[test]
fn matches_java_for_every_vector() {
    let text = include_str!("data/decoration_seed_vectors.txt");
    let mut checked = 0usize;

    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let at = format!("line {} ({line})", line_number + 1);

        match f[0] {
            "decoration" => {
                let (seed, x, z, want_seed, want_next) = (
                    f[1].parse::<i64>().unwrap(),
                    f[2].parse::<i32>().unwrap(),
                    f[3].parse::<i32>().unwrap(),
                    f[4].parse::<i64>().unwrap(),
                    f[5].parse::<i64>().unwrap(),
                );
                let mut r = source();
                let got_seed = set_decoration_seed(&mut r, seed, x, z);
                assert_eq!(got_seed, want_seed, "decoration seed at {at}");
                assert_eq!(
                    r.next_long(),
                    want_next,
                    "stream after decoration seed at {at}"
                );
            }
            "feature" => {
                let (decoration, index, step, want_next) = (
                    f[1].parse::<i64>().unwrap(),
                    f[2].parse::<i32>().unwrap(),
                    f[3].parse::<i32>().unwrap(),
                    f[4].parse::<i64>().unwrap(),
                );
                let mut r = source();
                set_feature_seed(&mut r, decoration, index, step);
                assert_eq!(
                    r.next_long(),
                    want_next,
                    "stream after feature seed at {at}"
                );
            }
            "large" => {
                let (seed, x, z, want_next) = (
                    f[1].parse::<i64>().unwrap(),
                    f[2].parse::<i32>().unwrap(),
                    f[3].parse::<i32>().unwrap(),
                    f[4].parse::<i64>().unwrap(),
                );
                let mut r = source();
                set_large_feature_seed(&mut r, seed, x, z);
                assert_eq!(
                    r.next_long(),
                    want_next,
                    "stream after large feature seed at {at}"
                );
            }
            "salt" => {
                let (seed, x, z, salt, want_next) = (
                    f[1].parse::<i64>().unwrap(),
                    f[2].parse::<i32>().unwrap(),
                    f[3].parse::<i32>().unwrap(),
                    f[4].parse::<i32>().unwrap(),
                    f[5].parse::<i64>().unwrap(),
                );
                let mut r = source();
                set_large_feature_with_salt(&mut r, seed, x, z, salt);
                assert_eq!(r.next_long(), want_next, "stream after salted seed at {at}");
            }
            other => panic!("unknown vector kind {other:?} at {at}"),
        }
        checked += 1;
    }

    assert!(
        checked > 200,
        "expected the full vector set, checked {checked}"
    );
}
