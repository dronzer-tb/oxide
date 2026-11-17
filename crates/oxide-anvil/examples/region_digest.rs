//! Prints one line per chunk in a region file: its position and a hash of its NBT content.
//! Compound keys are sorted before hashing, so two files match exactly when their chunks hold
//! the same data, whatever order the chunks sit in on disk or the tags were written in.
//!
//! `cargo run -p oxide-anvil --example region_digest -- <region.mca> [chunk_x chunk_z]`
//!
//! With a chunk position it prints that chunk's tags instead, one leaf per line, for `diff`.

use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = Path::new(args.get(1).expect("usage: region_digest <region.mca> [x z]"));
    let read = |pos: oxide_core::ChunkPos| -> Option<String> {
        let bytes = oxide_anvil::region::read_chunk(path, &pos).unwrap()?;
        let value: fastnbt::Value = fastnbt::from_bytes(&bytes).unwrap();
        let mut text = String::new();
        sorted(&value, 0, &mut text);
        Some(text)
    };
    if args.len() == 4 {
        let pos = oxide_core::ChunkPos::new(args[2].parse().unwrap(), args[3].parse().unwrap());
        print!("{}", read(pos).expect("no chunk there"));
        return;
    }
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let parts: Vec<i32> = name.split('.').skip(1).take(2).map(|n| n.parse().unwrap()).collect();
    for i in 0..1024 {
        let pos = oxide_core::ChunkPos::new(parts[0] * 32 + i % 32, parts[1] * 32 + i / 32);
        if let Some(text) = read(pos) {
            let mut hasher = DefaultHasher::new();
            text.hash(&mut hasher);
            println!("{} {} {:016x}", pos.x, pos.z, hasher.finish());
        }
    }
}

/// One line per leaf, compound keys sorted.
fn sorted(value: &fastnbt::Value, depth: usize, out: &mut String) {
    use fastnbt::Value;
    let pad = "  ".repeat(depth);
    match value {
        Value::Compound(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for key in keys {
                let _ = writeln!(out, "{pad}{key}:");
                sorted(&map[key], depth + 1, out);
            }
        }
        Value::List(items) => {
            for (i, item) in items.iter().enumerate() {
                let _ = writeln!(out, "{pad}[{i}]");
                sorted(item, depth + 1, out);
            }
        }
        other => {
            let _ = writeln!(out, "{pad}{other:?}");
        }
    }
}
