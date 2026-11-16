//! Generates a square of decorated chunks and prints which feature types and block rules the
//! native decorator skipped, with counts: what stands between native decoration and a world
//! with every feature in it. Also prints a digest of every decorated block (by state name, in
//! position order, so it does not depend on palette interning order across threads) and how many
//! chunk extras (block entities, ticks, post-processing marks) of each kind came out.
//!
//! `cargo run --release -p oxide-ffi --example decoration_gaps -- <datapack> <seed> <radius>`

use anyhow::{Context, Result};
use oxide_ffi::OxideGeneratorHandle;
use rayon::prelude::*;
use std::ffi::CStr;
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let datapack = args.get(1).context("datapack path")?;
    let seed: i64 = args.get(2).context("seed")?.parse()?;
    let radius: i32 = args.get(3).map(|r| r.parse()).transpose()?.unwrap_or(16);

    let generator = OxideGeneratorHandle::open(Path::new(datapack), "minecraft:overworld", seed)?;
    generator.set_decoration(true)?;
    let coords: Vec<(i32, i32)> = (-radius..=radius)
        .flat_map(|x| (-radius..=radius).map(move |z| (x, z)))
        .collect();
    let started = std::time::Instant::now();
    let generated: Vec<(Vec<u16>, String)> = coords
        .par_iter()
        .map(|&(x, z)| -> Result<(Vec<u16>, String)> {
            let mut blocks = vec![0u16; generator.block_buffer_len()];
            let mut biomes = vec![0u16; generator.biome_buffer_len()];
            generator.generate_chunk(x, z, &mut blocks, &mut biomes)?;
            Ok((blocks, generator.chunk_extras(x, z)?))
        })
        .collect::<Result<_>>()?;
    println!(
        "generated {} chunks in {:.3} s",
        coords.len(),
        started.elapsed().as_secs_f64()
    );
    let chunks: Vec<&Vec<u16>> = generated.iter().map(|(blocks, _)| blocks).collect();

    // FNV-1a over each block's state name, chunk by chunk in `coords` order.
    let names: Vec<Vec<u8>> = {
        let palette = generator.block_palette().read().expect("palette lock");
        (0..palette.len())
            .map(|i| {
                unsafe { CStr::from_ptr(palette.name_ptr(i)) }
                    .to_bytes()
                    .to_vec()
            })
            .collect()
    };
    let mut digest: u64 = 0xcbf2_9ce4_8422_2325;
    for blocks in chunks {
        for &index in blocks {
            for &byte in names[index as usize].iter().chain(b"\n") {
                digest ^= u64::from(byte);
                digest = digest.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    println!("decorated block digest {digest:016x}");

    let mut kinds: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for line in generated.iter().flat_map(|(_, extras)| extras.lines()) {
        let mut words = line.split(' ');
        let tag = words.next().unwrap_or_default();
        let detail = match tag {
            "be" => line
                .split("id:\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_default(),
            "ft" | "bt" => words.nth(3).unwrap_or_default(),
            _ => "",
        };
        *kinds.entry(format!("{tag} {detail}")).or_insert(0) += 1;
    }
    println!("extras:");
    for (kind, count) in kinds {
        println!("{count:>10}  {kind}");
    }

    let mut gaps = generator.decoration_gaps();
    gaps.sort_by(|a, b| b.1.cmp(&a.1));
    println!("{} chunks, {} gap kinds", coords.len(), gaps.len());
    for (key, count) in gaps {
        println!("{count:>10}  {key}");
    }
    Ok(())
}
