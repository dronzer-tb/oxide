// Standalone timing harness: measures pure native Rust chunk generation throughput
// with zero JVM, zero Bukkit, zero Folia scheduler involvement.
// Answers: is the Rust generator itself the bottleneck, or is it the JVM/Bukkit layer?

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let datapack = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "/home/container/plugins/OxideDebug/datapack".to_string());
    let seed: i64 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(4489057056054590644);
    let count: i32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(200);
    let threads: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(1);
    // Far-away origin so we generate genuinely fresh terrain, same as the live test.
    let origin: i32 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(25000);

    eprintln!("opening generator (datapack={datapack}, seed={seed}) ...");
    let t0 = Instant::now();
    let handle = oxide_ffi::OxideGeneratorHandle::open(
        Path::new(&datapack),
        "minecraft:overworld",
        seed,
    )
    .expect("open failed");
    eprintln!("open took {:.2}s", t0.elapsed().as_secs_f64());

    let height = handle.height() as usize;
    let blocks_len = 256 * height;
    let biomes_len = 64 * (height / 16);

    // ---- warmup ----
    {
        let mut b = vec![0u16; blocks_len];
        let mut bi = vec![0u16; biomes_len];
        for i in 0..4 {
            handle.generate_chunk(origin + i, origin, &mut b, &mut bi).unwrap();
        }
    }

    // ---- single/multi-thread throughput ----
    let side = (count as f64).sqrt().ceil() as i32;
    let done = AtomicUsize::new(0);
    let t = Instant::now();

    std::thread::scope(|s| {
        for tid in 0..threads {
            let handle = &handle;
            let done = &done;
            s.spawn(move || {
                let mut blocks = vec![0u16; blocks_len];
                let mut biomes = vec![0u16; biomes_len];
                let mut n = 0;
                for i in 0..count {
                    if (i as usize) % threads != tid {
                        continue;
                    }
                    let cx = origin + (i % side);
                    let cz = origin + (i / side);
                    handle.generate_chunk(cx, cz, &mut blocks, &mut biomes).unwrap();
                    n += 1;
                }
                done.fetch_add(n, Ordering::Relaxed);
            });
        }
    });

    let elapsed = t.elapsed().as_secs_f64();
    let n = done.load(Ordering::Relaxed);
    println!("--------------------------------------------------");
    println!("NATIVE RUST ONLY (no JVM, no Bukkit, no Folia)");
    println!("threads              : {threads}");
    println!("chunks generated     : {n}");
    println!("elapsed              : {elapsed:.3}s");
    println!("per chunk            : {:.3} ms", elapsed * 1000.0 / n as f64);
    println!("THROUGHPUT           : {:.1} CPS", n as f64 / elapsed);
    println!("--------------------------------------------------");

    #[cfg(feature = "cull-stats")]
    {
        use std::sync::atomic::Ordering;
        let c = oxide_chunkgen::CULL_CONSIDERED.load(Ordering::Relaxed);
        let b = oxide_chunkgen::CULL_BOUNDED.load(Ordering::Relaxed);
        let h = oxide_chunkgen::CULL_HI_NEG.load(Ordering::Relaxed);
        println!("cull: considered={c} bounded={b} ({:.1}%) hi<=0={h} ({:.1}%)",
                 100.0 * b as f64 / c.max(1) as f64,
                 100.0 * h as f64 / c.max(1) as f64);
    }

    // ---- biome_at cost: what Bukkit's BiomeProvider hammers per quart ----
    let t = Instant::now();
    let mut acc = 0u64;
    let quarts = 64 * (height / 16); // one chunk's worth of biome quarts
    for i in 0..quarts {
        let qx = (i % 4) as i32 * 4;
        let qz = ((i / 4) % 4) as i32 * 4;
        let qy = (i / 16) as i32 * 4;
        acc += handle.biome_at(origin * 16 + qx, qy, origin * 16 + qz).unwrap() as u64;
    }
    let e = t.elapsed().as_secs_f64();
    println!("biome_at x{quarts} (1 chunk of quarts): {:.3} ms total, {:.1} ns each  [checksum {acc}]",
             e * 1000.0, e * 1e9 / quarts as f64);
}
