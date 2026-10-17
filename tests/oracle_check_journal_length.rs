//! Black-box verdicts retained in data/journal-length-262-reference.txt.
mod common;
#[path = "common/journal_length.rs"]
mod journal_length;

use fs_bcachefs::check::check;
use fs_core::FileDevice;

#[test]
fn oversized_stale_journal_entries_are_reported() {
    let dir = common::ScratchDir::new("journal-length");
    for (oversized, unknown_version) in [(false, false), (false, true), (true, false), (true, true)]
    {
        let img = dir.join(format!("{oversized}-{unknown_version}.img"));
        std::fs::copy(common::fixture("aged-unclean.img"), &img).unwrap();
        journal_length::plant(&img, oversized, unknown_version);
        let r = check(&FileDevice::open(&img).unwrap()).unwrap();
        assert_eq!(r.clean(), !oversized, "{:?}", r.problems);
        if oversized {
            assert!(
                r.problems.iter().any(|p| p.kind == "journal"
                    && p.detail.contains("jset_past_bucket_end")
                    && p.detail.contains("34359738416")),
                "{:?}",
                r.problems
            );
        }
    }
}
