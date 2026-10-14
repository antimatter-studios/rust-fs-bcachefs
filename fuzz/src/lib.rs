//! The explorer half of the fuzzing setup: what each `cargo-fuzz` target
//! drives, shared textually with the gate (`tests/fuzz_decoders.rs`)
//! through `fuzz/shared/helpers.rs` -- see that file for why it is
//! included rather than depended on, and for why checksums are
//! re-stamped.
#![allow(dead_code)]

include!("../shared/helpers.rs");
include!("../shared/whole_image.rs");

/// Each fixture of `WI_FIXTURES`, read once per process, and what a clean
/// pass over it found to mutate.
fn whole_image_bases() -> &'static [(std::sync::Arc<Vec<u8>>, Targets)] {
    static BASES: std::sync::OnceLock<Vec<(std::sync::Arc<Vec<u8>>, Targets)>> =
        std::sync::OnceLock::new();
    BASES.get_or_init(|| {
        let dir = std::env::var_os("BCACHEFS_FIXTURES")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.vm-share/fixtures")
            });
        WI_FIXTURES
            .iter()
            .map(|name| {
                let path = dir.join(name);
                let bytes = std::fs::read(&path).unwrap_or_else(|e| {
                    panic!(
                        "the whole_image target needs the fixtures: {}: {e}",
                        path.display()
                    )
                });
                let base = std::sync::Arc::new(bytes);
                let t = targets(&base);
                (base, t)
            })
            .collect()
    })
}

/// One whole-image case: the input's fixture and seed (`wi_decode`), laid
/// over that fixture, then the workload.
pub fn whole_image_case(data: &[u8]) {
    let Some((i, seed)) = wi_decode(data) else {
        return;
    };
    let bases = whole_image_bases();
    let (base, t) = &bases[i % bases.len()];
    let dev = Overlay::new(std::sync::Arc::clone(base), false);
    apply_case(&dev, t, seed);
    let _ = workload(&dev, true);
}
