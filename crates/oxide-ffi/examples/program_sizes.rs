// Reports how large the compiled density programs actually are for the live datapack,
// to check whether Program::run is spilling its registers to the heap on every call.
use std::path::Path;

fn main() {
    let datapack = std::env::args().nth(1).unwrap_or_else(|| {
        "/root/oxide-datapack-copy".to_string()
    });
    let seed: i64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4489057056054590644);

    let handle =
        oxide_ffi::OxideGeneratorHandle::open(Path::new(&datapack), "minecraft:overworld", seed)
            .expect("open failed");

    for (name, len) in handle.program_sizes() {
        println!("{len:6}  {name}");
    }
    println!("\n--- final_density op listing ---");
    for line in handle.dump_program("final_density") {
        println!("  {line}");
    }
}
