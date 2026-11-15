# Track: noise

You own `crates/oxide-noise`. Read `docs/perf/PLAN.md` first. Worktree `~/oxide-wt/noise`,
branch `perf/noise`, bench name `noise`.

The density-function interpreter and Perlin sampling are a quarter of the whole machine:
`compiled::Program::run` 14%, `ImprovedNoise::noise_with_scale` 11%, plus `PerlinNoise` and
`BlendedNoise`. This track decides whether the goal is reachable at all.

Work, in the order the profile suggests:
1. Profile first on the VPS (`perf record` on the harness run in `/root/oxide-perf/noise`) so
   you know what inside `Program::run` is hot: dispatch, register traffic, or the leaves.
2. Evaluate many points per call instead of one: a column or a whole cell-corner lattice
   through the program at once, so dispatch is paid per op, not per op per point.
3. Vectorise `ImprovedNoise` with AVX2 across those points. Keep each lane's arithmetic in the
   exact scalar order: no FMA, no reassociation. The VPS has AVX2, FMA, BMI2; no AVX-512.
4. Cut work that cannot matter: tighter cell bounds, skipping octaves whose amplitude cannot
   change the sign that is being tested, caching 2D-only subtrees per column.
5. Only then consider replacing the interpreter (closure tree, threaded code, or generated
   Rust at build time from the exported density functions).
