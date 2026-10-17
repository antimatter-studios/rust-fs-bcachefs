//! Filesystems this reader recognises and refuses with a clear error rather
//! than misreading: an encrypted one, and each member of a two-device one.
//! Both made by the reference formatter, which also printed each superblock.
//! The encrypted filesystem comes twice (#270): its master key stored
//! unsealed (`--no_passphrase`), and sealed by a known passphrase.

mod common;

use common::{fixture, read_text};
use fs_bcachefs::superblock::Superblock;
use fs_bcachefs::{Error, Filesystem};
use fs_core::FileDevice;

fn open(name: &str) -> fs_bcachefs::Result<Filesystem<FileDevice>> {
    Filesystem::open(FileDevice::open(fixture(name)).unwrap())
}

fn printer(name: &str, key: &str) -> String {
    read_text(name)
        .lines()
        .find_map(|l| {
            l.strip_prefix(key)
                .map(|v| v.trim_start_matches(':').trim().to_string())
        })
        .unwrap_or_else(|| panic!("{name}: no {key:?} line"))
}

#[test]
fn an_encrypted_filesystem_is_refused_as_encrypted() {
    assert!(
        printer("encrypted.super.txt", "Sections").contains("crypt"),
        "the fixture is not encrypted"
    );
    match open("encrypted.img") {
        Err(Error::Unsupported(m)) => assert!(
            m.contains("encrypted"),
            "refused, but not as encrypted: {m}"
        ),
        Err(e) => panic!("refused with the wrong error: {e}"),
        Ok(_) => panic!("opened an encrypted filesystem"),
    }
}

/// The message `Filesystem::open` refuses an encrypted fixture with,
/// after checking the refusal names encryption.
fn encrypted_refusal(stem: &str) -> String {
    match open(&format!("{stem}.img")) {
        Err(Error::Unsupported(m)) => {
            assert!(
                m.contains("encrypted"),
                "{stem}: refused, but not as encrypted: {m}"
            );
            m
        }
        Err(e) => panic!("{stem}: refused with the wrong error: {e}"),
        Ok(_) => panic!("{stem}: opened an encrypted filesystem"),
    }
}

/// Formatted with `--no_passphrase`: the reference printer reads its
/// superblock with no passphrase and lists the crypt field.
#[test]
fn a_master_key_stored_unsealed_is_named_in_the_refusal() {
    assert!(
        printer("encrypted.super.txt", "Sections").contains("crypt"),
        "the fixture is not encrypted"
    );
    let m = encrypted_refusal("encrypted");
    assert!(
        m.contains("unsealed"),
        "formatted with --no_passphrase, but not refused as unsealed: {m}"
    );
}

/// Formatted with a passphrase: the reference printer will not read its
/// superblock without one, which is the reference's own view that the key
/// is sealed.
#[test]
fn a_master_key_sealed_by_a_passphrase_is_named_in_the_refusal() {
    let record = read_text("encrypted-passphrase.format.txt");
    assert!(
        record.contains("-- format") && !record.contains("format: exit"),
        "the reference formatter did not format the image:\n{record}"
    );
    assert!(
        !read_text("encrypted-passphrase.passphrase").is_empty(),
        "the passphrase the formatter was given is not recorded"
    );
    let report = read_text("encrypted-passphrase.super.txt");
    assert!(
        report.contains("passphrase") && !report.contains("Sections"),
        "the reference printer read the superblock without its passphrase:\n{report}"
    );
    let m = encrypted_refusal("encrypted-passphrase");
    assert!(
        m.contains("sealed by a passphrase") && !m.contains("unsealed"),
        "formatted with a passphrase, but not refused as sealed by one: {m}"
    );
}

#[test]
fn each_member_of_a_two_device_filesystem_is_refused_as_multi_device() {
    for i in 0..2 {
        let name = format!("multi-{i}.img");
        let sb = Superblock::read(&FileDevice::open(fixture(&name)).unwrap()).unwrap();
        assert_eq!(
            sb.nr_devices.to_string(),
            printer(&format!("multi-{i}.super.txt"), "Devices"),
            "{name}: device count"
        );
        match open(&name) {
            Err(Error::Unsupported(m)) => assert!(
                m.contains("2 devices"),
                "{name}: refused, but not as multi-device: {m}"
            ),
            Err(e) => panic!("{name}: refused with the wrong error: {e}"),
            Ok(_) => panic!("{name}: opened one member of a two-device filesystem"),
        }
    }
}
