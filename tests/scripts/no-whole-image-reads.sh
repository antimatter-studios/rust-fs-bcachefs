#!/usr/bin/env bash
# Whole-image comparisons and hashes must use the bounded helpers in tests/common/mod.rs.
# Intentional reads used to build damaged images stay on an explicit allowlist.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
python3 - "$REPO" <<'PY'
from collections import Counter, defaultdict
from pathlib import Path
import re
import sys

repo = Path(sys.argv[1])
# These are input buffers used to make damaged test images, or tiny scratch
# files whose contents are deliberately asserted. Counts make new copies of
# an existing read fail too; each entry names why it still needs whole bytes.
allowed = {
    "tests/check_oracle_key_types.rs": {
        'fixture("default.img")': (1, "fixture bytes are patched to make invalid keys"),
        "source": (5, "fixture bytes are patched to make invalid keys"),
    },
    "tests/check_oracle_reconcile_dirent.rs": {
        'common::fixture("default.img")': (1, "fixture bytes are patched to make a damaged image"),
    },
    "tests/common/alloc_v4.rs": {
        "&source": (1, "fixture bytes are patched to make a damaged image"),
    },
    "tests/common/bucket_counters.rs": {
        "&source": (1, "fixture bytes are patched to make a damaged image"),
    },
    "tests/common/key_field_bytes.rs": {
        "&source": (1, "fixture bytes are patched to make a damaged image"),
    },
    "tests/common/replicas_accounting.rs": {
        'fixture(&format!("{set}.img"))': (1, "fixture bytes are patched to make damaged accounting keys"),
    },
    "tests/fuzz_decoders.rs": {
        "&path": (1, "each bounded corpus input is passed to the decoder as complete bytes"),
    },
    "tests/oracle_conformance.rs": {
        'fixture("kernel-reuse.img")': (1, "the shared Driver needs owned damaged-image buffers; its input is capped at 64 MiB"),
    },
    "tests/oracle_check.rs": {
        'fixture(&format!("{set}.img"))': (1, "fixture bytes are patched to make damaged checker inputs"),
    },
    "tests/oracle_differential_default74.rs": {
        'common::fixture("aged-unclean.img")': (1, "fixture bytes are patched to make damaged images"),
        'common::fixture(&format!("{set}.img"))': (2, "fixture bytes are patched to make damaged images"),
        "&path": (1, "the bounded fuzz case is patched before it is judged"),
        'common::fixture("aged.img")': (1, "fixture bytes are patched to make a damaged image"),
    },
    "tests/oracle_differential_snapshot_tree_subvolume.rs": {
        "common::fixture(name)": (1, "fixture bytes are patched to make snapshot-tree test images"),
        'common::fixture("kernel.img")': (1, "the fixed kernel fixture is patched for the regression"),
    },
    "tests/oracle_encrypted_journal.rs": {
        "common::fixture(IMG)": (1, "fixture bytes are patched to make an encrypted journal image"),
        "&path": (1, "the copied image bytes are patched before oracle judgment"),
    },
    "tests/oracle_fuzz_whole_image.rs": {
        "common::fixture(name)": (1, "fixture bytes seed bounded whole-image fuzz mutations"),
        "&path": (1, "the mutated image is read back for its checker verdict"),
    },
    "tests/oracle_journal_roots.rs": {
        "&img": (1, "fixture bytes are patched to make damaged journal roots"),
    },
    "tests/scratch_dir.rs": {
        'first.join("first.img")': (1, "the owned test file contains five bytes"),
        "&image": (1, "the owned test file contains a short literal"),
    },
}

image_terms = re.compile(r"\b(?:fixture|img|image|disk|source|formatted|unedited|path)\b|\.img\b")

def reads(source):
    found = []
    for match in re.finditer(r"(?:std::)?fs::read\s*\(", source):
        start = match.end()
        depth = 1
        end = start
        while end < len(source) and depth:
            if source[end] == "(":
                depth += 1
            elif source[end] == ")":
                depth -= 1
            end += 1
        if depth:
            raise ValueError("unclosed fs::read call")
        expression = re.sub(r"\s+", " ", source[start : end - 1]).strip()
        if image_terms.search(expression):
            found.append(expression)
    return found

failures = []
observed = defaultdict(Counter)
for path in sorted((repo / "tests").rglob("*.rs")):
    rel = path.relative_to(repo).as_posix()
    for expression in reads(path.read_text()):
        observed[rel][expression] += 1
        limit, reason = allowed.get(rel, {}).get(expression, (0, "no reason recorded"))
        if observed[rel][expression] > limit:
            failures.append(f"{rel}: fs::read({expression}) — {reason}")

if failures:
    print("FAIL  whole-image reads need a bounded helper or a reviewed reason:", file=sys.stderr)
    print("\n".join(failures), file=sys.stderr)
    raise SystemExit(1)

# Prove the detector catches a newly added image read rather than passing vacuously.
probe = reads('let before = std::fs::read(&image).unwrap();')
if probe != ["&image"]:
    raise SystemExit("FAIL  the guard did not detect its whole-image probe")
print("no-whole-image-reads: all checks passed")
PY
