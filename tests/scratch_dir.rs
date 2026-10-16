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
