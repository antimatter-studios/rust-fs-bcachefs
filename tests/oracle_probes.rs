//! What the reference implementation answered when the fixture build asked
//! its mount for a subvolume, a snapshot, a casefolded directory and a
//! reflink (`probe.txt`, scripts/guest-build-fixtures.sh), and what this
//! reader must do with the image it left behind: refuse what it cannot read,
//! read what it can. Every outcome is asserted, so the day the mount starts
//! honouring one of them this turns red at once, with the lister's view of
//! the image beside it as the observation to build the reader from.

mod common;

use common::{fixture, read_text};
use fs_bcachefs::{Error, Filesystem};
use fs_core::FileDevice;

/// Whether the probe section starting `## <name>` ran without an `exit N`
/// line, which is how the build records a command that failed.
fn probe_ok(name: &str) -> Option<bool> {
    let text = read_text("probe.txt");
    let mut inside = false;
    let mut ok = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if inside {
                break;
            }
            if rest.starts_with(name) {
                inside = true;
                ok = Some(true);
            }
            continue;
        }
        if inside && line.starts_with("exit ") {
            ok = Some(false);
        }
    }
    ok
}

fn lines_of(file: &str, needle: &str) -> usize {
    read_text(file)
        .lines()
        .filter(|l| l.contains(needle))
        .count()
}

#[test]
fn the_probe_image_is_refused_or_read_according_to_what_the_mount_honoured() {
    let text = read_text("probe.txt");
    assert!(text.contains("## mount"), "probe.txt holds no probe run");
    if text.contains("did not mount") {
        // Nothing was tried; the formatted image must still read.
        Filesystem::open(FileDevice::open(fixture("probe.img")).unwrap()).unwrap();
        eprintln!("probe: the reference implementation did not mount the probe image");
        return;
    }
    let subvolume = probe_ok("subvolume create") == Some(true);
    let snapshot = probe_ok("subvolume snapshot") == Some(true);
    let casefold = probe_ok("casefold via set-file-option") == Some(true)
        || probe_ok("casefold via chattr") == Some(true);
    let reflink = probe_ok("reflink") == Some(true);
    eprintln!(
        "probe: subvolume {subvolume}, snapshot {snapshot}, casefold {casefold}, reflink {reflink}; \
         lister keys: subvolumes {}, snapshots {}, reflink {}",
        lines_of("probe.subvolumes.txt", "subvolume"),
        lines_of("probe.snapshots.txt", "snapshot"),
        lines_of("probe.reflink.txt", "reflink_v"),
    );

    let opened = Filesystem::open(FileDevice::open(fixture("probe.img")).unwrap());
    if subvolume || snapshot {
        // A second subvolume or a snapshot: keys at another snapshot id,
        // which this reader refuses rather than resolves (#53). The image
        // is the fixture #12 has been waiting for.
        match opened {
            Err(Error::Unsupported(m)) if m.contains("snapshot") => return,
            other => panic!(
                "the mount made a subvolume or snapshot, and the reader did not refuse the \
                 image: {:?}",
                other.map(|_| ())
            ),
        }
    }
    let fs = opened.expect("a probe image without snapshots reads");
    let file = fs.lookup("/sub/file").unwrap();
    assert_eq!(fs.read(file).unwrap(), b"probe\n");

    // Casefolding: whether or not the option took, the directory holds one
    // entry, `Name`, and it reads. If the option took and the dirent is
    // stored in the two-name form (S1 2.7), this is where it shows (#54).
    if fs.lookup("/casefold").is_ok() {
        let dir = fs.lookup("/casefold").unwrap();
        let names: Vec<String> = fs
            .readdir(dir)
            .unwrap()
            .iter()
            .map(|d| String::from_utf8_lossy(&d.name).into_owned())
            .collect();
        assert_eq!(
            names,
            ["Name"],
            "casefold honoured: {casefold}; dirents: {names:?}"
        );
        assert_eq!(
            fs.read(fs.lookup("/casefold/Name").unwrap()).unwrap(),
            b"Mixed\n"
        );
    }

    // Reflink: a clone the mount made must read as its source (#7); a clone
    // the mount refused to make does not exist.
    match fs.lookup("/sub/clone") {
        Ok(clone) => {
            assert!(
                reflink,
                "a clone exists but probe.txt says cp --reflink failed"
            );
            assert_eq!(
                fs.read(clone).unwrap(),
                b"probe\n",
                "the reflinked clone does not read as its source"
            );
        }
        Err(Error::NotFound(_)) => {
            assert!(!reflink, "cp --reflink succeeded but the clone is missing")
        }
        Err(e) => panic!("{e}"),
    }
}
