//! High-performance off-heap packet cache for pre-compressed chunk payloads.
//!
//! Avoids re-encoding, re-palettizing, and re-compressing chunks on every player dispatch.
//! Uses sharded concurrency to eliminate lock contention under multi-threaded Folia region workloads.

use std::sync::atomic::{AtomicU64, Ordering};
use ahash::RandomState;
use parking_lot::RwLock;

const SHARDS: usize = 32;

#[derive(Debug, Clone)]
pub struct CachedPacket {
    pub data: Vec<u8>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkKey {
    pub world_id: i64,
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl ChunkKey {
    pub fn new(world_id: i64, chunk_x: i32, chunk_z: i32) -> Self {
        Self {
            world_id,
            chunk_x,
            chunk_z,
        }
    }
}

pub struct Shard {
    map: std::collections::HashMap<ChunkKey, CachedPacket, RandomState>,
}

pub struct PacsideCache {
    shards: Vec<RwLock<Shard>>,
    max_capacity_per_shard: usize,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    bytes_cached: AtomicU64,
}

impl PacsideCache {
    pub fn new(total_capacity: usize) -> Self {
        let max_capacity_per_shard = (total_capacity / SHARDS).max(64);
        let mut shards = Vec::with_capacity(SHARDS);
        for _ in 0..SHARDS {
            shards.push(RwLock::new(Shard {
                map: std::collections::HashMap::with_hasher(RandomState::new()),
            }));
        }

        Self {
            shards,
            max_capacity_per_shard,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            bytes_cached: AtomicU64::new(0),
        }
    }

    #[inline(always)]
    fn shard_index(&self, key: &ChunkKey) -> usize {
        let hash = (key.world_id as u64)
            ^ ((key.chunk_x as u64) << 32)
            ^ ((key.chunk_z as u64) & 0xFFFFFFFF);
        (hash as usize) % SHARDS
    }

    pub fn put(&self, key: ChunkKey, data: Vec<u8>) {
        let shard_idx = self.shard_index(&key);
        let mut shard = self.shards[shard_idx].write();

        let len = data.len() as u64;
        if shard.map.len() >= self.max_capacity_per_shard {
            // Evict arbitrary entry
            if let Some(evicted_key) = shard.map.keys().next().copied() {
                if let Some(old) = shard.map.remove(&evicted_key) {
                    self.bytes_cached.fetch_sub(old.data.len() as u64, Ordering::Relaxed);
                    self.evictions.fetch_add(1, Ordering::Relaxed);
                }
            }
        }

        if let Some(old) = shard.map.insert(
            key,
            CachedPacket {
                data,
                timestamp: current_timestamp(),
            },
        ) {
            self.bytes_cached.fetch_sub(old.data.len() as u64, Ordering::Relaxed);
        }

        self.bytes_cached.fetch_add(len, Ordering::Relaxed);
    }

    pub fn get(&self, key: &ChunkKey) -> Option<Vec<u8>> {
        let shard_idx = self.shard_index(key);
        let shard = self.shards[shard_idx].read();

        if let Some(cached) = shard.map.get(key) {
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(cached.data.clone())
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    pub fn invalidate(&self, key: &ChunkKey) {
        let shard_idx = self.shard_index(key);
        let mut shard = self.shards[shard_idx].write();
        if let Some(old) = shard.map.remove(key) {
            self.bytes_cached.fetch_sub(old.data.len() as u64, Ordering::Relaxed);
        }
    }

    pub fn clear(&self) {
        for shard in &self.shards {
            let mut s = shard.write();
            s.map.clear();
        }
        self.bytes_cached.store(0, Ordering::Relaxed);
    }

    pub fn stats(&self) -> CacheStats {
        let mut count = 0;
        for shard in &self.shards {
            count += shard.read().map.len();
        }

        CacheStats {
            cached_chunks: count,
            bytes_cached: self.bytes_cached.load(Ordering::Relaxed),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub cached_chunks: usize,
    pub bytes_cached: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_get_roundtrip() {
        let cache = PacsideCache::new(1024);
        let key = ChunkKey::new(1, 10, 20);
        let payload = vec![0x01, 0x02, 0x03, 0x04];

        cache.put(key, payload.clone());
        let result = cache.get(&key).expect("cached entry");
        assert_eq!(result, payload);

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.cached_chunks, 1);
    }

    #[test]
    fn test_cache_miss() {
        let cache = PacsideCache::new(1024);
        let key = ChunkKey::new(1, 99, 99);
        assert!(cache.get(&key).is_none());

        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 1);
    }
}
