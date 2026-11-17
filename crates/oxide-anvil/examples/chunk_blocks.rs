//! Prints a saved chunk's status and the block names in its section palettes, with how many
//! sections each appears in: a quick look at whether a chunk got its features.
//!
//! `cargo run -p oxide-anvil --example chunk_blocks -- <region.mca> <chunk_x> <chunk_z>`

use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pos = oxide_core::ChunkPos::new(args[2].parse().unwrap(), args[3].parse().unwrap());
    let Some(chunk) = oxide_anvil::region::read_chunk_nbt(Path::new(&args[1]), &pos).unwrap()
    else {
        println!("no chunk at {pos:?}");
        return;
    };
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    for section in &chunk.sections {
        for entry in &section.block_states.palette {
            *names.entry(entry.name.clone()).or_default() += 1;
        }
    }
    println!(
        "status {} | {} block entities | {} names",
        chunk.status,
        chunk.block_entities.len(),
        names.len()
    );
    for (name, sections) in names {
        print!("{name}:{sections} ");
    }
    println!();
}
