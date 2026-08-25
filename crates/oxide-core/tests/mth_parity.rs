//! Cross-checks `Mth.sin`/`Mth.cos` against real Java output.
//!
//! Vectors produced by `tests/data/MthOracle.java`, a verbatim transcription of the decompiled
//! 26.1.2 `Mth` table and index arithmetic, standalone so it runs on a plain JDK. Regenerate:
//!
//! ```text
//! java crates/oxide-core/tests/data/MthOracle.java > crates/oxide-core/tests/data/mth_vectors.txt
//! ```
//!
//! Compared as raw bit patterns rather than decimals: two distinct floats can print identically,
//! and a table entry that is one ulp out still moves a cave wall.

use oxide_core::mth;

#[test]
fn matches_java_for_every_vector() {
    let text = include_str!("data/mth_vectors.txt");
    let mut checked = 0usize;

    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            f[0],
            "mth",
            "unexpected vector kind on line {}",
            line_number + 1
        );

        let angle = f64::from_bits(f[1].parse::<i64>().unwrap() as u64);
        let want_sin = f32::from_bits(f[2].parse::<i32>().unwrap() as u32);
        let want_cos = f32::from_bits(f[3].parse::<i32>().unwrap() as u32);

        assert_eq!(
            mth::sin(angle).to_bits(),
            want_sin.to_bits(),
            "sin({angle}) on line {}",
            line_number + 1
        );
        assert_eq!(
            mth::cos(angle).to_bits(),
            want_cos.to_bits(),
            "cos({angle}) on line {}",
            line_number + 1
        );
        checked += 1;
    }

    assert!(
        checked >= 80,
        "expected the full vector set, checked {checked}"
    );
}
