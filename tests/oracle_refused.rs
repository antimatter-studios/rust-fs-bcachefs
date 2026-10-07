//! Filesystems this reader recognises and refuses with a clear error rather
//! than misreading: an encrypted one, and each member of a two-device one.
//! Both made by the reference formatter, which also printed each superblock.

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
