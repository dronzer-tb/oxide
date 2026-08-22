//! oxide-pregen library surface.
//!
//! Exists only so `src/stub.rs` is reusable both by the `oxide-pregen` binary (`src/main.rs`)
//! and by this crate's integration tests, without duplicating the generator. All CLI parsing
//! and write orchestration lives in the binary — this crate has no other public surface.

pub mod stub;
