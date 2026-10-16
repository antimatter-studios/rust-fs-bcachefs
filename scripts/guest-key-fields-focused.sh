#!/usr/bin/env bash
# PR #226's unchanged size/snapshot cases and the inherited native refusals.
# This is focused evidence; the full zero-ignored matrix remains required.
set -euo pipefail
[ "${FLTH_GUEST:-}" = 1 ] || {
    echo "key-fields-focused needs the provisioned harness guest" >&2
    exit 1
}
cd /repo
export CARGO_BUILD_JOBS=2
FS_CORE_CALLER=/repo exec bash /share/siblings/rust-fs-core/scripts/guest-rust-run.sh \
    fs-bcachefs /share rust-fs-core -- bash -c '
        set -euo pipefail
        nice -n 10 cargo test --locked --release --features write \
            --test oracle_check_key_fields --test write_oracle --no-run --quiet
        bash /share/siblings/rust-fs-core/scripts/tier.sh --refuse-ignored \
            test:key-fields-focused key-fields-focused 160 16000 -- bash -c '\''
                set -euo pipefail
                cargo test --locked --release --features write --test oracle_check_key_fields
                for case_name in \
                    a_reference_key_refusal_keeps_its_reason_with_its_own_header \
                    a_sized_bucket_gens_key_is_refused_by_the_reference \
                    an_xattr_key_in_the_dirents_btree_is_refused_by_the_reference
                do
                    cargo test --locked --release --features write --test write_oracle \
                        "$case_name" -- --exact --test-threads=1
                done
            '\''
        python3 -c '\''
import pathlib, re
text = pathlib.Path("tmp/logs/key-fields-focused.log").read_text()
counts = re.findall(r"test result: ok\. (\d+) passed; 0 failed; 0 ignored;", text)
assert counts == ["16", "1", "1", "1"], counts
print("key-fields-focused: 19 passed, 0 failed, 0 ignored")
'\''
    '
