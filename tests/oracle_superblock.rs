//! Every superblock field this crate reads, compared with the reference
//! superblock printer's report of the same image.

mod common;

use common::{fixture, parse_size, selected, super_report, SETS};
use fs_bcachefs::superblock::{format_uuid, Superblock};
use fs_core::FileDevice;

const CSUM_OPTS: &[&str] = &["none", "crc32c", "crc64", "xxhash"];
const COMPRESSION_OPTS: &[&str] = &["none", "lz4", "gzip", "zstd"];

#[test]
fn every_superblock_field_matches_the_reference_printer() {
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap_or_else(|e| panic!("{set}: {e}"));
        let r = super_report(set);
        let ctx = |k: &str| format!("{set}: {k}");

        assert_eq!(
            format_uuid(&sb.user_uuid),
            r["External UUID"],
            "{}",
            ctx("External UUID")
        );
        assert_eq!(
            format_uuid(&sb.uuid),
            r["Internal UUID"],
            "{}",
            ctx("Internal UUID")
        );
        assert_eq!(
            format_uuid(&sb.magic),
            r["Magic number"],
            "{}",
            ctx("Magic")
        );
        assert_eq!(
            sb.dev_idx.to_string(),
            r["Device index"],
            "{}",
            ctx("Device index")
        );
        let label = if sb.label_str().is_empty() {
            "(none)".to_string()
        } else {
            sb.label_str()
        };
        assert_eq!(label, r["Label"], "{}", ctx("Label"));
        let v = |ver: fs_bcachefs::superblock::Version| {
            format!(
                "{} ({}.{})",
                ver.name().unwrap_or("?"),
                ver.major(),
                ver.minor()
            )
        };
        assert_eq!(v(sb.version), r["Version"], "{}", ctx("Version"));
        assert_eq!(
            v(sb.version_min),
            r["Oldest version on disk"],
            "{}",
            ctx("Oldest version")
        );
        assert_eq!(
            sb.seq.to_string(),
            r["Sequence number"],
            "{}",
            ctx("Sequence number")
        );
        assert_eq!(
            sb.nr_devices.to_string(),
            r["Devices"],
            "{}",
            ctx("Devices")
        );
        assert_eq!(
            sb.field_names().join(","),
            r["Sections"],
            "{}",
            ctx("Sections")
        );
        assert_eq!(
            sb.feature_names().join(","),
            r["Features"],
            "{}",
            ctx("Features")
        );
        assert_eq!(
            sb.compat_names().join(","),
            r["Compat features"],
            "{}",
            ctx("Compat")
        );

        assert_eq!(
            sb.block_size as u64 * 512,
            parse_size(&r["block_size"]),
            "{}",
            ctx("block_size")
        );
        assert_eq!(
            sb.btree_node_size() as u64 * 512,
            parse_size(&r["btree_node_size"]),
            "{}",
            ctx("btree_node_size")
        );
        assert_eq!(
            CSUM_OPTS[sb.metadata_checksum_opt() as usize],
            selected(&r["metadata_checksum"]),
            "{}",
            ctx("metadata_checksum")
        );
        assert_eq!(
            CSUM_OPTS[sb.data_checksum_opt() as usize],
            selected(&r["data_checksum"]),
            "{}",
            ctx("data_checksum")
        );
        assert_eq!(
            COMPRESSION_OPTS[sb.compression_opt() as usize],
            r["compression"],
            "{}",
            ctx("compression")
        );

        // "Superblock size: 7.25k/1.00M": the used size, and the layout's maximum.
        let (used, max) = r["Superblock size"].split_once('/').unwrap();
        let ours = 0x2f0 + sb.u64s as u64 * 8;
        assert_eq!(human(ours), used.trim(), "{}", ctx("Superblock size"));
        assert_eq!(
            512u64 << sb.layout.sb_max_size_bits,
            parse_size(max),
            "{}",
            ctx("Superblock max")
        );

        let members = sb.members().unwrap();
        assert_eq!(members.len(), 1, "{}", ctx("members"));
        let m = &members[0];
        assert_eq!(format_uuid(&m.uuid), r["dev.UUID"], "{}", ctx("dev UUID"));
        assert_eq!(
            m.nbuckets.to_string(),
            r["dev.Buckets"],
            "{}",
            ctx("dev Buckets")
        );
        assert_eq!(
            m.first_bucket.to_string(),
            r["dev.First bucket"],
            "{}",
            ctx("dev First bucket")
        );
        assert_eq!(
            m.bucket_size as u64 * 512,
            parse_size(&r["dev.Bucket size"]),
            "{}",
            ctx("dev Bucket size")
        );
        assert_eq!(
            m.nbuckets * m.bucket_size as u64 * 512,
            parse_size(&r["dev.Size"]),
            "{}",
            ctx("dev Size")
        );

        // The layout: the primary at sector 8, the last copy at the end.
        assert_eq!(sb.layout.sb_offsets.first(), Some(&8), "{}", ctx("layout"));
        assert!(
            !sb.btree_roots().unwrap().is_empty(),
            "{}",
            ctx("btree roots")
        );
    }
}

#[test]
fn the_standalone_layout_copy_agrees_with_the_embedded_one() {
    use fs_core::BlockRead;
    for set in SETS {
        let dev = FileDevice::open(fixture(&format!("{set}.img"))).unwrap();
        let sb = Superblock::read(&dev).unwrap();
        let mut b = vec![0u8; fs_bcachefs::superblock::Layout::BYTES];
        dev.read_at(fs_bcachefs::superblock::LAYOUT_OFFSET, &mut b)
            .unwrap();
        let layout = fs_bcachefs::superblock::Layout::parse(&b).unwrap();
        assert_eq!(layout, sb.layout, "{set}");
    }
}

/// A byte count as the reference printer writes it: three significant
/// digits and a binary unit ("7.25k", "8.08k", "1.00M"). Comparing in the
/// printer's own rounding, not by parsing it back, because 8272 and 8274
/// bytes both print as "8.08k".
fn human(n: u64) -> String {
    let mut v = n as f64;
    let mut unit = "";
    for u in ["k", "M", "G", "T"] {
        if v < 1024.0 {
            break;
        }
        v /= 1024.0;
        unit = u;
    }
    if unit.is_empty() {
        return n.to_string();
    }
    if v < 10.0 {
        format!("{v:.2}{unit}")
    } else if v < 100.0 {
        format!("{v:.1}{unit}")
    } else {
        format!("{v:.0}{unit}")
    }
}
