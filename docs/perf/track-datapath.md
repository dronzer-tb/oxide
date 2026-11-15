# Track: datapath

You own `crates/oxide-core`, `crates/oxide-biome` and `crates/oxide-chunkgen` outside
`decorator/`. Read `docs/perf/PLAN.md` first. Worktree `~/oxide-wt/datapath`, branch
`perf/datapath`, bench name `datapath`.

About 17% of the machine is malloc, free and memcpy, and the profile ties part of it to
`BlockState` (a name plus a string property map) being cloned and dropped per block. Surface
rules, the biome search and aquifers are another 11%.

Work, in the order the profile suggests:
1. Profile the harness on the VPS and attribute the libc time to callers before changing
   anything.
2. Make the fill and surface hot paths work in small integer block ids, resolving to
   `BlockState` only at the palette boundary. `ChunkData`'s public shape is used by the
   decorator, `oxide-anvil` and the FFI: keep it source-compatible, or add the id path
   beside it.
3. Remove per-chunk and per-column allocation: reuse scratch buffers per thread, no
   `String`/`BTreeMap` work inside loops.
4. Surface rules: evaluate conditions once per column where they do not depend on y, and
   precompile the rule tree so `eval_rule` is not re-walking it per block.
5. Biome search (`Node::distance`) and aquifer `compute_substance`: cache per quart cell and
   per aquifer cell, not per block.
