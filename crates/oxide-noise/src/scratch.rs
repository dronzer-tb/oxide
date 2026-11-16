//! Per-thread reusable scratch buffers for the batched evaluators (`Program::run_lanes`,
//! `Program::run_cell`, `Program::run`'s register spill).
//!
//! Those paths used to allocate several short-lived `Vec`s per call (lane coordinates, shifted
//! coordinates, registers, branch partitions, the per-batch `cache_once` memo): about 7.2k
//! allocations and 6 MB per chunk of fill. That is cheap on one thread but costs scaling on
//! eight, where the allocator is shared. A [`Scratch`] takes a buffer from this thread's free
//! list instead and hands it back when dropped.
//!
//! No value changes: [`Scratch::filled`] yields exactly the contents `vec![value; n]` would
//! (the buffer is cleared, then resized with `value`), so every reader sees the same initial
//! data as before, and the arithmetic that fills the buffers is untouched.

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};

/// Buffers kept per thread and element type; more are simply freed.
const MAX_POOLED: usize = 64;
/// Larger buffers are not kept, so one huge request cannot pin memory for the thread's life.
const MAX_POOLED_LEN: usize = 1 << 20;

/// An element type with its own per-thread free list.
pub(crate) trait Pooled: Copy + 'static {
    /// Runs `f` on this thread's free list; `None` while the thread is being torn down.
    fn with_pool<R>(f: impl FnOnce(&RefCell<Vec<Vec<Self>>>) -> R) -> Option<R>;
}

macro_rules! pooled {
    ($t:ty, $pool:ident) => {
        thread_local! {
            static $pool: RefCell<Vec<Vec<$t>>> = const { RefCell::new(Vec::new()) };
        }
        impl Pooled for $t {
            #[inline]
            fn with_pool<R>(f: impl FnOnce(&RefCell<Vec<Vec<Self>>>) -> R) -> Option<R> {
                $pool.try_with(f).ok()
            }
        }
    };
}

pooled!(f64, POOL_F64);
pooled!(usize, POOL_USIZE);
pooled!(u32, POOL_U32);
pooled!(u64, POOL_U64);
pooled!(Option<f64>, POOL_OPT_F64);

/// A `Vec<T>` borrowed from this thread's free list, returned to it on drop.
pub(crate) struct Scratch<T: Pooled>(Vec<T>);

impl<T: Pooled> Scratch<T> {
    /// `n` copies of `value`: the same contents as `vec![value; n]`.
    #[inline]
    pub(crate) fn filled(n: usize, value: T) -> Self {
        let mut v = T::with_pool(|p| p.borrow_mut().pop())
            .flatten()
            .unwrap_or_default();
        v.clear();
        v.resize(n, value);
        Scratch(v)
    }

    /// `n` elements with **unspecified** contents (whatever the pooled buffer last held, or
    /// `value` where it was shorter). Only for callers that write every element before
    /// reading it, which then see exactly what they wrote; this skips [`Self::filled`]'s
    /// full re-initialisation.
    #[inline]
    pub(crate) fn overwritten(n: usize, value: T) -> Self {
        let mut v = T::with_pool(|p| p.borrow_mut().pop())
            .flatten()
            .unwrap_or_default();
        if v.len() > n {
            v.truncate(n);
        } else {
            v.resize(n, value);
        }
        Scratch(v)
    }

    /// An empty buffer to push into: the same contents as `Vec::new()`.
    #[inline]
    pub(crate) fn empty() -> Self {
        let mut v = T::with_pool(|p| p.borrow_mut().pop())
            .flatten()
            .unwrap_or_default();
        v.clear();
        Scratch(v)
    }
}

impl<T: Pooled> Drop for Scratch<T> {
    #[inline]
    fn drop(&mut self) {
        let v = std::mem::take(&mut self.0);
        if v.capacity() == 0 || v.capacity() > MAX_POOLED_LEN {
            return;
        }
        let _ = T::with_pool(|p| {
            let mut p = p.borrow_mut();
            if p.len() < MAX_POOLED {
                p.push(v);
            }
        });
    }
}

impl<T: Pooled> Deref for Scratch<T> {
    type Target = Vec<T>;
    #[inline]
    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<'a, T: Pooled> IntoIterator for &'a Scratch<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<T: Pooled> DerefMut for Scratch<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reused buffer starts exactly as a fresh `vec![value; n]`, whatever it held before.
    #[test]
    fn reused_buffers_start_as_fresh_ones() {
        {
            let mut a = Scratch::filled(7, 0.0f64);
            for (i, v) in a.iter_mut().enumerate() {
                *v = i as f64 + 0.5;
            }
        }
        let b = Scratch::filled(5, 0.0f64);
        assert_eq!(b.len(), 5);
        assert!(b.iter().all(|v| v.to_bits() == 0.0f64.to_bits()));
        let c = Scratch::filled(9, -0.0f64);
        assert!(c.iter().all(|v| v.to_bits() == (-0.0f64).to_bits()));
        drop((b, c));
        let mut e = Scratch::<usize>::empty();
        assert!(e.is_empty());
        e.push(3);
        drop(e);
        assert!(Scratch::<usize>::empty().is_empty());
    }
}
