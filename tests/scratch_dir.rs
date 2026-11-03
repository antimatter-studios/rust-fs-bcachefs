//! The scratch directory guard removes its copies on a normal return and
//! while a panic unwinds, and removes nothing but its own path (#235).

mod common;

use common::ScratchDir;

#[test]
fn a_scratch_directory_is_removed_on_normal_return() {
    let path = {
        let dir = ScratchDir::new("guard-normal");
        std::fs::write(dir.join("image.img"), vec![0u8; 4096]).unwrap();
        assert!(dir.join("image.img").exists());
        dir.path().to_path_buf()
    };
    assert!(!path.exists(), "{} outlived its guard", path.display());
}

#[test]
fn a_scratch_directory_is_removed_while_a_panic_unwinds() {
    let path = std::sync::Mutex::new(None);
    let result = std::panic::catch_unwind(|| {
        let dir = ScratchDir::new("guard-unwind");
        std::fs::write(dir.join("image.img"), vec![0u8; 4096]).unwrap();
        *path.lock().unwrap() = Some(dir.path().to_path_buf());
        panic!("the test body failed");
    });
    assert!(result.is_err());
    let path = path.into_inner().unwrap().unwrap();
    assert!(
        !path.exists(),
        "{} outlived an unwinding guard",
        path.display()
    );
}

#[test]
fn a_scratch_directory_removes_only_its_own_path() {
    let sibling_owner = ScratchDir::new("guard-sibling");
    let sibling = sibling_owner.path();
    std::fs::write(sibling.join("keep.img"), b"keep").unwrap();
    drop(ScratchDir::new("guard-own"));
    assert!(
        sibling.join("keep.img").exists(),
        "a guard removed a path it did not own"
    );
}

#[test]
fn simultaneous_same_tag_directories_do_not_remove_each_others_images() {
    let first = ScratchDir::new("same-tag");
    std::fs::write(first.join("first.img"), b"first").unwrap();
    let second = ScratchDir::new("same-tag");
    std::fs::write(second.join("second.img"), b"second").unwrap();
    assert_ne!(first.path(), second.path());
    drop(second);
    assert_eq!(std::fs::read(first.join("first.img")).unwrap(), b"first");
}

#[test]
fn an_image_returned_by_a_helper_keeps_its_directory_alive() {
    fn image() -> common::ScratchImage {
        let image = ScratchDir::new("returned-image").into_image("image.img");
        std::fs::write(&image, b"kept until the caller finishes").unwrap();
        image
    }
    let image = image();
    let directory = image.parent().unwrap().to_path_buf();
    assert_eq!(
        std::fs::read(&image).unwrap(),
        b"kept until the caller finishes"
    );
    drop(image);
    assert!(!directory.exists());
}

#[test]
fn owned_directories_use_the_runners_root() {
    let expected = std::env::var_os("FS_BCACHEFS_TEST_SCRATCH_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")));
    let dir = ScratchDir::new("runner-root");
    assert_eq!(dir.path().parent(), Some(expected.as_path()));
}

#[test]
fn an_image_path_cannot_escape_its_owner() {
    let dir = ScratchDir::new("guard-boundary");
    assert!(std::panic::catch_unwind(|| dir.join("../outside.img")).is_err());
    assert!(std::panic::catch_unwind(|| dir.join("/outside.img")).is_err());
    assert!(dir.path().is_dir());
}

/// The copy holds the source's bytes, its length even when it ends in zeros, and
/// no allocated blocks for its zero blocks (#504).
#[test]
fn a_sparse_copy_holds_the_same_bytes_in_fewer_blocks() {
    use std::os::unix::fs::MetadataExt;

    // APFS leaves no hole for a gap of a few MiB; this is the size of a fixture.
    const LEN: usize = 64 * 1024 * 1024;
    let dir = common::ScratchDir::new("sparse-copy");
    let mut image = vec![0u8; LEN];
    // Across a read of the copy's own size, in the first block, and unaligned.
    image[0] = 1;
    image[1024 * 1024 - 1] = 2;
    image[1024 * 1024] = 3;
    image[3 * 1024 * 1024 + 17] = 4;
    let (src, dst) = (dir.join("full.img"), dir.join("sparse.img"));
    std::fs::write(&src, &image).unwrap();
    {
        let file = std::fs::File::open(&src).unwrap();
        file.sync_all().unwrap();
    }

    common::copy_sparse(&src, &dst);
    {
        let file = std::fs::File::open(&dst).unwrap();
        file.sync_all().unwrap();
    }

    assert!(std::fs::read(&dst).unwrap() == image, "the bytes differ");
    let (full, sparse) = (
        std::fs::metadata(&src).unwrap().blocks(),
        std::fs::metadata(&dst).unwrap().blocks(),
    );
    assert!(
        sparse * 8 < full,
        "the copy holds {sparse} blocks against the source's {full}"
    );

    // A copy over an existing longer file leaves nothing of it.
    std::fs::write(&dst, vec![0xffu8; LEN + 4096]).unwrap();
    common::copy_sparse(&src, &dst);
    assert!(
        std::fs::read(&dst).unwrap() == image,
        "an old file survives"
    );
}
