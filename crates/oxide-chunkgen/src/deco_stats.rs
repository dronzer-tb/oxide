//! Where decoration time goes, counted only when built with the `deco-stats` feature.
//!
//! Without the feature every call here is empty and compiles away. With it, each counter is a
//! pair of relaxed atomics (calls, nanoseconds), cheap next to the work it times. Counters are
//! process-wide: run one generator per process when reading them.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

/// Calls and total nanoseconds of one measured stage.
pub struct Counter {
    pub calls: AtomicU64,
    pub nanos: AtomicU64,
}

impl Counter {
    const fn new() -> Self {
        Self {
            calls: AtomicU64::new(0),
            nanos: AtomicU64::new(0),
        }
    }

    pub fn add(&self, nanos: u64) {
        self.calls.fetch_add(1, Relaxed);
        self.nanos.fetch_add(nanos, Relaxed);
    }
}

/// Hits, computes and waits of one memo (`DecorationCache`'s terrain or writes).
pub struct MemoStats {
    /// The value was already there.
    pub hits: AtomicU64,
    /// This thread computed it.
    pub computed: Counter,
    /// Another thread was computing it; this one waited.
    pub waited: Counter,
    /// Time taken to get the shard lock.
    pub lock: Counter,
}

impl MemoStats {
    const fn new() -> Self {
        Self {
            hits: AtomicU64::new(0),
            computed: Counter::new(),
            waited: Counter::new(),
            lock: Counter::new(),
        }
    }
}

pub static TERRAIN: MemoStats = MemoStats::new();
pub static WRITES: MemoStats = MemoStats::new();
/// One finished chunk, end to end.
pub static CHUNK: Counter = Counter::new();
/// Copying the cached terrain the finished chunk starts from.
pub static CLONE: Counter = Counter::new();
/// Collecting the nine sources' writes (cache lookups, decorating misses).
pub static SOURCES: Counter = Counter::new();
/// Applying the writes that land in the chunk.
pub static APPLY: Counter = Counter::new();
/// Block entities, ticks and post-processing marks.
pub static EXTRAS: Counter = Counter::new();
pub static HEIGHTMAPS: Counter = Counter::new();
/// Building the 3x3 region a source chunk's features run against.
pub static REGION_NEW: Counter = Counter::new();
/// Running every feature of one source chunk.
pub static DECORATE: Counter = Counter::new();
/// Splitting a source chunk's writes into per-chunk buckets.
pub static BUCKETS: Counter = Counter::new();

/// Per placed-feature key: (calls, nanos), and per step the same.
static FEATURES: Mutex<Option<HashMap<u32, (u64, u64)>>> = Mutex::new(None);
static STEPS: [Counter; 11] = [const { Counter::new() }; 11];
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

const STEP_NAMES: [&str; 11] = [
    "raw_generation",
    "lakes",
    "local_modifications",
    "underground_structures",
    "surface_structures",
    "strongholds",
    "underground_ores",
    "underground_decoration",
    "fluid_springs",
    "vegetal_decoration",
    "top_layer_modification",
];

/// A running timer that does nothing without the feature.
pub struct Stopwatch(#[cfg(feature = "deco-stats")] std::time::Instant);

impl Stopwatch {
    #[inline]
    pub fn start() -> Self {
        Self(
            #[cfg(feature = "deco-stats")]
            std::time::Instant::now(),
        )
    }

    #[inline]
    pub fn nanos(&self) -> u64 {
        #[cfg(feature = "deco-stats")]
        return self.0.elapsed().as_nanos() as u64;
        #[cfg(not(feature = "deco-stats"))]
        0
    }

    #[inline]
    pub fn stop(self, _counter: &Counter) {
        #[cfg(feature = "deco-stats")]
        _counter.add(self.nanos());
    }
}

#[inline]
pub fn hit(_memo: &MemoStats) {
    #[cfg(feature = "deco-stats")]
    _memo.hits.fetch_add(1, Relaxed);
}

/// Names a placed-feature key by the first registry id it was built from.
#[inline]
pub fn name_feature(_key: u32, _name: &str) {
    #[cfg(feature = "deco-stats")]
    {
        let mut names = NAMES.lock().expect("deco stats poisoned");
        let key = _key as usize;
        if names.len() <= key {
            names.resize(key + 1, String::new());
        }
        names[key] = _name.to_string();
    }
}

/// Adds one source chunk's per-feature times: `(step, key, nanos)`.
#[inline]
pub fn record_features(_times: &[(u8, u32, u64)]) {
    #[cfg(feature = "deco-stats")]
    {
        for &(step, _, nanos) in _times {
            STEPS[step as usize].add(nanos);
        }
        let mut features = FEATURES.lock().expect("deco stats poisoned");
        let map = features.get_or_insert_with(HashMap::new);
        for &(_, key, nanos) in _times {
            let entry = map.entry(key).or_default();
            entry.0 += 1;
            entry.1 += nanos;
        }
    }
}

/// Everything counted so far, as a plain-text report. `chunks` is how many finished chunks the
/// caller asked for, used for per-chunk figures.
pub fn report(chunks: u64) -> String {
    let chunks = chunks.max(1) as f64;
    let ms = |nanos: u64| nanos as f64 / 1e6;
    let per = |nanos: u64| ms(nanos) / chunks;
    let mut out = String::new();
    let line = |out: &mut String, name: &str, c: &Counter| {
        let (calls, nanos) = (c.calls.load(Relaxed), c.nanos.load(Relaxed));
        let _ = writeln!(
            out,
            "  {name:<28} {calls:>9} calls {:>10.0} ms  {:>7.2} ms/chunk",
            ms(nanos),
            per(nanos)
        );
    };
    let _ = writeln!(out, "decoration stats ({chunks:.0} finished chunks; thread time, not wall time)");
    for (name, c) in [
        ("finished chunk, total", &CHUNK),
        ("  clone terrain", &CLONE),
        ("  collect 9 sources", &SOURCES),
        ("  apply writes", &APPLY),
        ("  extras", &EXTRAS),
        ("  heightmaps", &HEIGHTMAPS),
        ("source: build 3x3 region", &REGION_NEW),
        ("source: run features", &DECORATE),
        ("source: split buckets", &BUCKETS),
    ] {
        line(&mut out, name, c);
    }
    for (name, memo) in [("terrain memo", &TERRAIN), ("writes memo", &WRITES)] {
        let hits = memo.hits.load(Relaxed);
        let computed = memo.computed.calls.load(Relaxed);
        let waited = memo.waited.calls.load(Relaxed);
        let total = (hits + computed + waited).max(1);
        let _ = writeln!(
            out,
            "{name}: {total} lookups, {hits} hits, {computed} computed ({:.2} per finished chunk), {waited} waited; hit rate {:.1}%",
            computed as f64 / chunks,
            100.0 * hits as f64 / total as f64
        );
        line(&mut out, "  compute", &memo.computed);
        line(&mut out, "  wait for another thread", &memo.waited);
        line(&mut out, "  shard lock", &memo.lock);
    }
    let _ = writeln!(out, "features by step:");
    for (name, c) in STEP_NAMES.iter().zip(STEPS.iter()) {
        if c.calls.load(Relaxed) > 0 {
            line(&mut out, name, c);
        }
    }
    let names = NAMES.lock().expect("deco stats poisoned");
    let features = FEATURES.lock().expect("deco stats poisoned");
    if let Some(map) = features.as_ref() {
        let total: u64 = map.values().map(|v| v.1).sum::<u64>().max(1);
        let mut rows: Vec<_> = map.iter().collect();
        rows.sort_by_key(|(_, v)| std::cmp::Reverse(v.1));
        let _ = writeln!(out, "top placed features ({} distinct):", rows.len());
        for (key, (calls, nanos)) in rows.into_iter().take(30) {
            let name = names.get(*key as usize).map_or("?", |n| n.as_str());
            let _ = writeln!(
                out,
                "  {name:<44} {calls:>9} calls {:>9.0} ms {:>5.1}%  {:>6.1} us/call",
                ms(*nanos),
                100.0 * *nanos as f64 / total as f64,
                *nanos as f64 / 1e3 / *calls as f64
            );
        }
    }
    out
}
