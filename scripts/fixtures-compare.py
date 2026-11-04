#!/usr/bin/env python3
"""fixtures-compare.py KEPT FRESH: is a fresh fixtures build the kept one's equal?

Two builds never match byte for byte: every image has its own UUIDs, times and
hash seeds. .github/workflows/fixtures-fresh.yml runs the whole test suite over
its fresh build, which proves the tests still hold. This says whether the
reference's record changed even where no test looks, so that a change in the
reference cannot hide behind a kept build:

- the same files, and each image the same size;
- kernel.txt byte for byte: the kernel, the module's load and each stage it
  recorded, the same in every build measured;
- every other text record the reference printed, with what differs between any
  two builds folded away (UUIDs, numbers, times, temporary paths) and its lines
  sorted, the same. A version number is not folded: it is what a reference
  change looks like.

VARIABLE is what still differs between builds of one tree, measured over the
fixtures of CI runs 41805364959, 41789838751, 41795639102, 41774859180 and
41736924207. These records depend on where the reference's allocator or journal
stopped, so they are counted and not judged. A pattern is added only from a
measured difference between builds nothing else separates.

Exit 0: the builds agree. Exit 1: names each difference, with its first line.
"""

import re
import sys
import tarfile
from collections import Counter

VARIABLE = re.compile(
    r"""^fixtures/(?:
        .*\.freespace\.txt
      | .*\.journal-(?:dirty|headers)\.txt
      | .*\.(?:judge-)?console\.txt
      | .*\.extents\.ondisk\.txt
      | (?:aged|aged-unclean)\.super\.txt
      | (?:bgcompress|encrypted-compressed|encrypted-compressed\.written|encrypted|kernel)\.extents\.txt
      | bgcompress\.fsck\.txt
      | crc128-overwrite\.txt
      | encrypted-compressed\.txt
      | encrypted\.(?:alloc|need_discard)\.txt
      | large\.(?:dirents|inodes)\.formats\.txt
      | poison\.moved\.settle\.txt
      | write-study/base-small-nodes\.logged_ops\.txt
      | write-study/inline-(?:bs4k|default)\..*
      | write-study/small-nodes-grown\..*
      | write-study/options\.txt
    )$""",
    re.X,
)
EXACT = {"fixtures/kernel.txt"}
VERSION = re.compile(r"\b(\d+\.\d+\.\d+[\w+.~-]*)")
FOLD = [
    (re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"), "UUID"),
    (re.compile(r"\b[A-Z][a-z]{2} [A-Z][a-z]{2} +\d+ \d\d:\d\d:\d\d \d{4}\b"), "TIME"),
    (re.compile(r"\b[A-Z][a-z]{2} +\d+ \d\d:\d\d\b"), "TIME"),
    (re.compile(r"fixture-build\.[0-9A-Za-z]+"), "TMP"),
    (re.compile(r"\b\d+(?:\.\d+)?[KMG]\b"), "SIZE"),
    (re.compile(r"0x[0-9a-f]+|\b[0-9a-f]*\d[0-9a-f]*\b"), "#"),
]


def fold(text):
    # The odd pieces of the split are version numbers, kept as they are.
    pieces = VERSION.split(text)
    for i in range(0, len(pieces), 2):
        for pattern, replacement in FOLD:
            pieces[i] = pattern.sub(replacement, pieces[i])
    return sorted("".join(pieces).splitlines())


def first_difference(before, after):
    """The first line, in sorted order, that only one build holds."""
    gone = Counter(before) - Counter(after)
    new = Counter(after) - Counter(before)
    lines = [(line, "-") for line in gone] + [(line, "+") for line in new]
    line, sign = min(lines)
    return f"{sign} {line}"


def is_text(name):
    return name.endswith((".txt", ".json"))


def read(tarball):
    """Each member's size, and the text of each text record."""
    sizes, texts = {}, {}
    with tarfile.open(tarball, "r:gz") as tar:
        for member in tar:
            if not member.isfile():
                continue
            sizes[member.name] = member.size
            if is_text(member.name):
                texts[member.name] = tar.extractfile(member).read().decode("latin1")
    return sizes, texts


def compare(kept, fresh):
    """Every difference, one line each, and how many records were judged."""
    kept_sizes, kept_texts = kept
    fresh_sizes, fresh_texts = fresh
    differences = []
    for name in sorted(kept_sizes.keys() - fresh_sizes.keys()):
        differences.append(f"{name}: in the kept build only")
    for name in sorted(fresh_sizes.keys() - kept_sizes.keys()):
        differences.append(f"{name}: in the fresh build only")
    judged = variable = 0
    for name in sorted(kept_sizes.keys() & fresh_sizes.keys()):
        if name in kept_texts:
            if VARIABLE.match(name):
                variable += 1
                continue
            judged += 1
            if name in EXACT:
                before, after = kept_texts[name].splitlines(), fresh_texts[name].splitlines()
            else:
                before, after = fold(kept_texts[name]), fold(fresh_texts[name])
            if before != after:
                differences.append(f"{name}: {first_difference(before, after)}")
        elif kept_sizes[name] != fresh_sizes[name]:
            differences.append(
                f"{name}: {kept_sizes[name]} bytes kept, {fresh_sizes[name]} fresh"
            )
    return differences, judged, variable


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    differences, judged, variable = compare(read(sys.argv[1]), read(sys.argv[2]))
    for line in differences:
        print(f"fixtures-compare: {line}")
    print(
        f"fixtures-compare: {judged} records judged, {variable} variable records "
        f"counted only, {len(differences)} differences"
    )
    return 1 if differences else 0


if __name__ == "__main__":
    sys.exit(main())
