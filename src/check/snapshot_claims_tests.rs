//! Which subvolumes claim a snapshot, as the reference checker judges it (#521).
//! The guest holds the reference to the same: `tests/check_oracle_snapshot_claims.rs`.

use super::*;

const UNLINKED: u32 = SUBVOL_STATES[1];
const DELETED: u32 = SUBVOL_STATES[2];

fn subvolume(id: u64, snapshot: u32, state: u32) -> Bkey {
    let mut value = vec![0; 48];
    value[SUBVOL_SNAPSHOT..SUBVOL_SNAPSHOT + 4].copy_from_slice(&snapshot.to_le_bytes());
    value[SUBVOL_STATE..SUBVOL_STATE + 4].copy_from_slice(&state.to_le_bytes());
    Bkey {
        key_type: KEY_SUBVOLUME,
        pos: Bpos {
            inode: 0,
            offset: id,
            snapshot: 0,
        },
        size: 0,
        version_hi: 0,
        version_lo: 0,
        value,
    }
}

fn claims(keys: Vec<Bkey>) -> Vec<String> {
    let trees = BTreeMap::from([(SUBVOLUMES, keys)]);
    let mut report = Report::default();
    snapshot_claims(&trees, &mut report);
    report
        .problems
        .iter()
        .map(|problem| {
            assert_eq!(problem.kind, "snapshot_claimed_twice");
            problem.detail.clone()
        })
        .collect()
}

#[test]
fn a_snapshot_two_subvolumes_claim_is_named() {
    assert!(claims(vec![
        subvolume(1, 9, SUBVOL_LIVE),
        subvolume(2, 8, SUBVOL_LIVE)
    ])
    .is_empty());
    for state in [SUBVOL_LIVE, UNLINKED, 0, SUBVOL_LIVE ^ 1, UNLINKED ^ 3] {
        let found = claims(vec![
            subvolume(1, 9, SUBVOL_LIVE),
            subvolume(4096, 9, state),
        ]);
        assert_eq!(found.len(), 1, "{state:#x}: {found:?}");
        assert!(found[0].contains("snapshot 9"), "{found:?}");
        assert!(found[0].contains("1, 4096"), "{found:?}");
    }
    let found = claims(vec![
        subvolume(1, 9, SUBVOL_LIVE),
        subvolume(2, 9, SUBVOL_LIVE),
        subvolume(3, 9, UNLINKED),
    ]);
    assert_eq!(found.len(), 1, "one finding names them all: {found:?}");
    assert!(found[0].contains("1, 2, 3"), "{found:?}");
}

#[test]
fn a_deleted_subvolume_claims_no_snapshot() {
    for state in [DELETED, DELETED ^ 1, DELETED ^ 3] {
        let found = claims(vec![
            subvolume(1, 9, SUBVOL_LIVE),
            subvolume(4096, 9, state),
        ]);
        assert!(found.is_empty(), "{state:#x}: {found:?}");
    }
    let found = claims(vec![
        subvolume(1, 9, DELETED),
        subvolume(2, 9, DELETED),
        subvolume(3, 9, SUBVOL_LIVE),
    ]);
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_key_of_another_type_or_too_short_claims_no_snapshot() {
    let mut other = subvolume(2, 9, SUBVOL_LIVE);
    other.key_type = KEY_SET;
    assert!(claims(vec![subvolume(1, 9, SUBVOL_LIVE), other]).is_empty());
    let mut short = subvolume(2, 9, SUBVOL_LIVE);
    short.value.truncate(SUBVOL_SNAPSHOT + 3);
    assert!(claims(vec![subvolume(1, 9, SUBVOL_LIVE), short]).is_empty());
    let mut unset = subvolume(2, 9, SUBVOL_LIVE);
    unset.value.truncate(SUBVOL_SNAPSHOT + 4);
    assert_eq!(claims(vec![subvolume(1, 9, SUBVOL_LIVE), unset]).len(), 1);
}
