//! Fills one section's 4x4x4 biome grid by sampling climate at each quart-cell's center and
//! looking up the nearest biome via `oxide-biome`.

use std::cell::RefCell;

use oxide_biome::{BiomeSearchTree, ClimateSample};
use oxide_core::{ChunkPos, ChunkSection};
use oxide_noise::{NoiseRouterEvaluator, RouterSlot};

/// `(y*16+z)*16+x` scaled to the biome container's 4x4x4 grid — same Y-outer/Z-middle/X-inner
/// convention as `fill::local_index`, just at quart resolution (64 entries, not 4096).
#[inline(always)]
fn local_biome_index(x: usize, y: usize, z: usize) -> usize {
    (y * 4 + z) * 4 + x
}

/// The climate slots other than depth, in the order [`ColumnClimate::values`] holds them.
const COLUMN_SLOTS: [RouterSlot; 5] = [
    RouterSlot::Temperature,
    RouterSlot::Vegetation,
    RouterSlot::Continents,
    RouterSlot::Erosion,
    RouterSlot::Ridges,
];

/// Climate values that do not depend on y, kept per quart column of the chunk being filled.
///
/// `fill_biomes` runs once per section, and every section samples the same 16 quart columns
/// (x and z at `quart*4 + 2`), only at different y. A slot whose program is proven not to read
/// y (see [`program_is_y_independent`]) gives bit-identical values at every y of a column, so it
/// is evaluated at the first y asked and reused for the rest of the chunk. Depth, and any slot
/// not proven, is still evaluated at every quart cell.
struct ColumnClimate {
    router: u64,
    chunk: (i32, i32),
    /// Which of [`COLUMN_SLOTS`] are proven y-independent for `router`.
    flat: [bool; 5],
    filled: [bool; 16],
    values: [[f64; 5]; 16],
    /// Per column, the biome-tree leaf found last there: a search hint for the next y.
    hints: [u32; 16],
}

thread_local! {
    static COLUMN_CLIMATE: RefCell<ColumnClimate> = const {
        RefCell::new(ColumnClimate {
            router: 0,
            chunk: (0, 0),
            flat: [false; 5],
            filled: [false; 16],
            values: [[0.0; 5]; 16],
            hints: [u32::MAX; 16],
        })
    };
    /// Proofs already made on this thread, per router id: parsing a program dump is cheap,
    /// but not something to redo per chunk.
    static FLAT_SLOTS: RefCell<Vec<(u64, [bool; 5])>> = const { RefCell::new(Vec::new()) };
}

fn flat_slots(router: &NoiseRouterEvaluator) -> [bool; 5] {
    FLAT_SLOTS.with(|known| {
        let mut known = known.borrow_mut();
        if let Some((_, flat)) = known.iter().find(|(id, _)| *id == router.id()) {
            return *flat;
        }
        let flat = COLUMN_SLOTS.map(|slot| program_is_y_independent(&router.dump_program(slot)));
        known.push((router.id(), flat));
        flat
    })
}

/// Whether a router program, given as its [`NoiseRouterEvaluator::dump_program`] listing,
/// yields the same bits at every y of a fixed (x, z). Conservative: anything not recognised
/// below -- including any change to the listing format -- answers `false`.
///
/// Accepted, at every nesting level:
/// - `Const`, and the pure arithmetic ops on registers (`Add` .. `Clamp`): functions of their
///   inputs only.
/// - `Shift` in its `Xz` / `Zx` forms, which read the offset noise at `(x*0.25, 0, z*0.25)` /
///   `(z*0.25, x*0.25, 0)`: no y at all.
/// - `ShiftedNoise` with `y_scale == 0` whose `shift_y` register is a finite constant `c` other
///   than `-0.0`. Its noise y is `y as f64 * 0.0 + c`; for a finite y the product is `+0.0` or
///   `-0.0`, and either plus such a `c` is exactly `c` (`c != 0`), or `+0.0` (`c == +0.0`).
///   Noise x and z do not involve y, and its shift registers are checked like any other op.
/// - `Cache` of kind `FlatCache`, `Cache2d` or `Once` whose argument program passes this same
///   check. The first two store one value per column, evaluated at whatever y first asked --
///   the same bits for any y when the argument does not read y; `Once` is keyed by the full
///   position and returns its argument's value there.
///
/// Rejected: everything else -- y gradients, plain `Noise`, 3D shifts, blended noise,
/// `interpolated` caches, and the ops whose nested programs the listing does not show in full
/// (splines, range choices, interval selects, top-surface searches).
fn program_is_y_independent(lines: &[String]) -> bool {
    struct Frame {
        depth: usize,
        ops: Vec<String>,
    }
    let mut stack = vec![Frame {
        depth: 0,
        ops: Vec::new(),
    }];
    let close_to = |stack: &mut Vec<Frame>, depth: usize| -> bool {
        while stack.last().is_some_and(|frame| frame.depth > depth) {
            let frame = stack.pop().expect("checked non-empty");
            if !frame_is_y_independent(&frame.ops) {
                return false;
            }
        }
        true
    };
    for line in lines {
        let lead = line.len() - line.trim_start_matches(' ').len();
        let rest = &line[lead..];
        match rest.split_once(": ") {
            None => {
                // `{pad}     {label}:` -- the program owned by the op just listed follows,
                // three levels deeper.
                let Some(label) = rest.strip_suffix(':') else {
                    return false;
                };
                if label.is_empty() || !label.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_')
                {
                    return false;
                }
                let Some(pad) = lead.checked_sub(5) else {
                    return false;
                };
                if pad % 2 != 0 {
                    return false;
                }
                let depth = pad / 2;
                if !close_to(&mut stack, depth) {
                    return false;
                }
                match stack.last() {
                    Some(frame) if frame.depth == depth && !frame.ops.is_empty() => {}
                    _ => return false,
                }
                stack.push(Frame {
                    depth: depth + 3,
                    ops: Vec::new(),
                });
            }
            Some((index, text)) => {
                // `{pad}{i:3}: {text}`.
                let Ok(i) = index.parse::<usize>() else {
                    return false;
                };
                let width = index.len().max(3);
                let Some(pad) = (lead + index.len()).checked_sub(width) else {
                    return false;
                };
                if pad % 2 != 0 {
                    return false;
                }
                let depth = pad / 2;
                if !close_to(&mut stack, depth) {
                    return false;
                }
                match stack.last_mut() {
                    Some(frame) if frame.depth == depth && frame.ops.len() == i => {
                        frame.ops.push(text.to_string());
                    }
                    _ => return false,
                }
            }
        }
    }
    if !close_to(&mut stack, 0) {
        return false;
    }
    match stack.pop() {
        Some(root) => !root.ops.is_empty() && frame_is_y_independent(&root.ops),
        None => false,
    }
}

/// One program level of [`program_is_y_independent`]; nested programs are checked separately.
fn frame_is_y_independent(ops: &[String]) -> bool {
    const PURE: [&str; 12] = [
        "Add(r",
        "Mul(r",
        "Min(r",
        "Max(r",
        "Abs(r",
        "Square(r",
        "Cube(r",
        "HalfNegative(r",
        "QuarterNegative(r",
        "Squeeze(r",
        "Invert(r",
        "Clamp(r",
    ];
    const CACHES: [&str; 3] = [
        "Cache(kind=FlatCache,",
        "Cache(kind=Cache2d,",
        "Cache(kind=Once,",
    ];
    if ops.is_empty() {
        return false;
    }
    ops.iter().enumerate().all(|(own, text)| {
        if PURE.iter().chain(CACHES.iter()).any(|p| text.starts_with(p)) {
            return true;
        }
        if text.starts_with("Const(") {
            return const_value(text).is_some();
        }
        if text.starts_with("Shift(#") {
            return text.ends_with(" Xz)") || text.ends_with(" Zx)");
        }
        if let Some(args) = text.strip_prefix("ShiftedNoise(#") {
            return shifted_noise_ignores_y(ops, own, args);
        }
        false
    })
}

fn const_value(text: &str) -> Option<f64> {
    text.strip_prefix("Const(")?.strip_suffix(')')?.parse().ok()
}

/// `args` is the rest of `ShiftedNoise(#{noise} xz={xz} y={y} shift=r{x},r{y},r{z})`.
fn shifted_noise_ignores_y(ops: &[String], own: usize, args: &str) -> bool {
    let fields: Vec<&str> = args.split(' ').collect();
    let [_noise, _xz, y_scale, shift] = fields.as_slice() else {
        return false;
    };
    let Some(Ok(y_scale)) = y_scale.strip_prefix("y=").map(str::parse::<f64>) else {
        return false;
    };
    if y_scale != 0.0 {
        return false;
    }
    let Some(registers) = shift
        .strip_prefix("shift=")
        .and_then(|s| s.strip_suffix(')'))
    else {
        return false;
    };
    let registers: Vec<&str> = registers.split(',').collect();
    let [_, shift_y, _] = registers.as_slice() else {
        return false;
    };
    let Some(Ok(shift_y)) = shift_y.strip_prefix('r').map(str::parse::<usize>) else {
        return false;
    };
    if shift_y >= own {
        return false;
    }
    match const_value(&ops[shift_y]) {
        Some(c) => c.is_finite() && !(c == 0.0 && c.is_sign_negative()),
        None => false,
    }
}

/// PARITY-CHECK: sampling at the cell's center (`cell*4 + 2`) matches vanilla's quart-position convention.
pub(crate) fn fill_biomes(
    section: &mut ChunkSection,
    pos: ChunkPos,
    section_min_y: i32,
    router: &NoiseRouterEvaluator,
    caches: &oxide_noise::ChunkCaches,
    tree: &BiomeSearchTree,
) {
    if tree.is_empty() {
        return;
    }

    COLUMN_CLIMATE.with(|columns| {
        let mut columns = columns.borrow_mut();
        if columns.router != router.id() || columns.chunk != (pos.x, pos.z) {
            let flat = if columns.router == router.id() {
                columns.flat
            } else {
                flat_slots(router)
            };
            *columns = ColumnClimate {
                router: router.id(),
                chunk: (pos.x, pos.z),
                flat,
                filled: [false; 16],
                values: [[0.0; 5]; 16],
                hints: [u32::MAX; 16],
            };
        }
        fill_section(section, pos, section_min_y, router, caches, tree, &mut columns);
    });
}

fn fill_section(
    section: &mut ChunkSection,
    pos: ChunkPos,
    section_min_y: i32,
    router: &NoiseRouterEvaluator,
    caches: &oxide_noise::ChunkCaches,
    tree: &BiomeSearchTree,
    columns: &mut ColumnClimate,
) {
    // The same values `ClimateSample::sample_in_chunk` returns: each field from the same slot at
    // the same position, except that a proven y-independent slot is taken from its column.
    // Evaluation order across slots does not matter -- every slot is a pure function of
    // position, the chunk caches only memoizing it.
    let mut nearest = |qx: usize, qy: usize, qz: usize| -> Option<&oxide_core::ResourceLocation> {
        let block_x = pos.min_block_x() + (qx * 4) as i32 + 2;
        let y = section_min_y + (qy * 4) as i32 + 2;
        let block_z = pos.min_block_z() + (qz * 4) as i32 + 2;
        let column = qz * 4 + qx;
        if !columns.filled[column] {
            for (i, slot) in COLUMN_SLOTS.iter().enumerate() {
                if columns.flat[i] {
                    columns.values[column][i] =
                        router.sample_in_chunk(caches, *slot, block_x, y, block_z);
                }
            }
            columns.filled[column] = true;
        }
        let flat = columns.flat;
        let values = columns.values[column];
        let value = |i: usize| {
            if flat[i] {
                values[i]
            } else {
                router.sample_in_chunk(caches, COLUMN_SLOTS[i], block_x, y, block_z)
            }
        };
        let sample = ClimateSample {
            temperature: value(0),
            humidity: value(1),
            continentalness: value(2),
            erosion: value(3),
            depth: router.sample_in_chunk(caches, RouterSlot::Depth, block_x, y, block_z),
            weirdness: value(4),
        };
        tree.nearest_with_hint(sample, &mut columns.hints[column])
    };

    // Fast-path: check the 8 corners of the 4x4x4 quart section.
    // In >80% of underground and open sections, all 8 corners resolve to the exact same biome.
    // Compared by reference into the tree -- no id is cloned to answer "same biome?".
    let corners = [
        nearest(0, 0, 0),
        nearest(3, 0, 0),
        nearest(0, 0, 3),
        nearest(3, 0, 3),
        nearest(0, 3, 0),
        nearest(3, 3, 0),
        nearest(0, 3, 3),
        nearest(3, 3, 3),
    ];
    if let Some(biome) = corners[0] {
        // Pointer first: corners that hit the same tree leaf are the same id, so the string
        // comparison only runs for distinct leaves.
        if corners
            .iter()
            .all(|corner| corner.is_some_and(|c| std::ptr::eq(c, biome) || *c == *biome))
        {
            // Entire 64-quart section is homogenous: one palette entry, written by index.
            let index = section.biomes.index_of_or_insert(biome.clone());
            for i in 0..64 {
                section.biomes.set_index(i, index);
            }
            return;
        }
    }

    // Boundary section: sample all 64 quart cells. Neighbouring cells usually repeat a biome,
    // so the palette index of the last one written is reused while the tree keeps returning
    // the same entry.
    //
    // The eight corner cells were already looked up above, at the very same positions.
    // Climate sampling is a pure function of position (the chunk caches only memoize it), and
    // the tree search a pure function of the sample, so asking again would return the same
    // leaf; they are reused rather than sampled twice.
    let mut last: Option<(&oxide_core::ResourceLocation, u32)> = None;
    for qy in 0..4usize {
        for qz in 0..4usize {
            for qx in 0..4usize {
                let biome = if qx % 3 == 0 && qy % 3 == 0 && qz % 3 == 0 {
                    corners[(qx / 3) + 2 * (qz / 3) + 4 * (qy / 3)]
                } else {
                    nearest(qx, qy, qz)
                };
                if let Some(biome) = biome {
                    let index = match last {
                        Some((known, index)) if std::ptr::eq(known, biome) => index,
                        _ => {
                            let index = section.biomes.index_of_or_insert(biome.clone());
                            last = Some((biome, index));
                            index
                        }
                    };
                    section
                        .biomes
                        .set_index(local_biome_index(qx, qy, qz), index);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::{BlockState, ResourceLocation};
    use oxide_datapack::{load_datapack, BiomeSource};
    use std::path::Path;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn y_independence_proof_rejects_what_it_cannot_show() {
        let shifted = |y: &str, c: &str| {
            lines(&[
                "  0: Cache(kind=FlatCache, slot=1)",
                "     argument:",
                "        0: Shift(#32 Xz)",
                &format!("  1: Const({c})"),
                "  2: Shift(#32 Zx)",
                &format!("  3: ShiftedNoise(#59 xz=0.25 y={y} shift=r0,r1,r2)"),
            ])
        };
        assert!(program_is_y_independent(&shifted("0", "0")));
        assert!(program_is_y_independent(&shifted("0", "1.5")));
        assert!(!program_is_y_independent(&shifted("0", "-0")));
        assert!(!program_is_y_independent(&shifted("0.5", "0")));
        assert!(!program_is_y_independent(&lines(&["  0: YClampedGradient(-64..320 -> 1.5..-1.5)"])));
        assert!(!program_is_y_independent(&lines(&["  0: Noise(#1 xz=1 y=0)"])));
        assert!(!program_is_y_independent(&lines(&["  0: Shift(#32 Xyz)"])));
        assert!(!program_is_y_independent(&lines(&[
            "  0: Cache(kind=Interpolated, slot=0)",
            "     argument:",
            "        0: Const(1)",
        ])));
        // A y read hidden inside a cache argument is still found.
        assert!(!program_is_y_independent(&lines(&[
            "  0: Cache(kind=FlatCache, slot=0)",
            "     argument:",
            "        0: YClampedGradient(-64..320 -> 1.5..-1.5)",
            "  1: Abs(r0)",
        ])));
        assert!(!program_is_y_independent(&[]));
    }

    fn reference() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../reference"))
    }

    /// On the real overworld router the five 2D climate slots are proven, depth is not, and
    /// every section's biomes are exactly those of per-quart `ClimateSample::sample_in_chunk`
    /// plus an unhinted search.
    #[test]
    fn column_climate_fill_matches_per_quart_sampling() {
        if !reference().is_dir() {
            eprintln!("skipping: no extracted vanilla export at {}", reference().display());
            return;
        }
        let datapack = load_datapack(reference()).expect("load reference datapack");
        let dimension = datapack
            .dimensions
            .get(&ResourceLocation::minecraft("overworld"))
            .expect("overworld dimension");
        let settings_id = dimension.generator.settings.as_ref().expect("settings id");
        let settings = datapack.noise_settings.get(settings_id).expect("settings").clone();
        let router = NoiseRouterEvaluator::new(
            12345,
            &settings,
            &datapack.density_functions,
            &datapack.noise_params,
        );
        let tree = match dimension.generator.biome_source.as_ref() {
            Some(BiomeSource::MultiNoise(source)) => BiomeSearchTree::from_source(source).unwrap(),
            _ => panic!("overworld is multi-noise"),
        };

        assert_eq!(flat_slots(&router), [true; 5]);
        assert!(!program_is_y_independent(&router.dump_program(RouterSlot::Depth)));

        let air = BlockState::new(ResourceLocation::minecraft("air"));
        let plains = ResourceLocation::minecraft("plains");
        let min_y = settings.noise.min_y;
        let sections = settings.noise.height / 16;
        for (cx, cz) in [(0, 0), (7, -3), (-12, 5), (100, 100), (2000, -2000)] {
            let pos = ChunkPos::new(cx, cz);
            let caches = router.chunk_caches(cx, cz);
            let reference_caches = router.chunk_caches(cx, cz);
            for i in 0..sections {
                let section_y = min_y / 16 + i;
                let section_min_y = section_y * 16;
                let mut section = ChunkSection::new(section_y as i8, air.clone(), plains.clone());
                fill_biomes(&mut section, pos, section_min_y, &router, &caches, &tree);
                for qy in 0..4 {
                    for qz in 0..4 {
                        for qx in 0..4 {
                            let sample = ClimateSample::sample_in_chunk(
                                &router,
                                &reference_caches,
                                pos.min_block_x() + (qx * 4) as i32 + 2,
                                section_min_y + (qy * 4) as i32 + 2,
                                pos.min_block_z() + (qz * 4) as i32 + 2,
                            );
                            let expected = tree.nearest(sample).unwrap();
                            assert_eq!(
                                section.biomes.get(local_biome_index(qx, qy, qz)),
                                expected,
                                "chunk ({cx}, {cz}) section {section_y} quart ({qx}, {qy}, {qz})"
                            );
                        }
                    }
                }
            }
        }
    }
}
