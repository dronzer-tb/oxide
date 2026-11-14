# Track: pipeline

You own `crates/oxide-pregen`, `crates/oxide-anvil` and `scripts/`. Read `docs/perf/PLAN.md`
first. Worktree `~/oxide-wt/pipeline`, branch `perf/pipeline`, bench name `pipeline`.

Terrain speed means nothing if the rest of the path is slow. On the live server the JVM side
caps generation near 240 chunks/s whatever Oxide does. This track builds the path with no
server in it and measures it.

Work:
1. `oxide-pregen` is still a stub that writes checkerboard terrain. Make it generate real
   chunks: terrain through `oxide_chunkgen::generate_chunk`, decoration through
   `BiomeDecorator` over a `DecorationRegion`, the way `oxide-harness`'s `world_map` does
   (`crates/oxide-harness/src/oracle.rs`), on all cores, written with `oxide-anvil`.
2. Give it a timing line in the same `BENCH` format as the harness, split into generate,
   decorate, serialise, compress and write, and add `scripts/bench-pregen-vps.sh` beside
   `bench-vps.sh` using the same lock. That is this track's number.
3. Then cut the non-terrain cost: region compression (zlib is ~3 ms a chunk; offer LZ4 and
   none), serialisation without intermediate allocation, batched region writes.
4. Heightmaps for decorated chunks and sky light are not computed natively yet. Without them
   a server recomputes on load. Do heightmaps; scope light and report before starting it.
5. Check a pregenerated region loads on the live server only by handing the user exact
   steps. Do not touch the server yourself.
