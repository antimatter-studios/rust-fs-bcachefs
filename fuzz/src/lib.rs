//! The explorer half of the fuzzing setup: what each `cargo-fuzz` target
//! drives, shared textually with the gate (`tests/fuzz_decoders.rs`)
//! through `fuzz/shared/helpers.rs` -- see that file for why it is
//! included rather than depended on, and for why checksums are
//! re-stamped.
#![allow(dead_code)]

include!("../shared/helpers.rs");
