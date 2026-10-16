#!/usr/bin/env python3
"""Reject system-temporary image allocations outside the owned scratch helper."""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
UNOWNED = re.compile(
    r'\btemp_dir\s*\(|\buse\s+std::env::temp_dir\b|"/(?:share|tmp|var/tmp)/'
)


def main():
    failures = []
    for tree in ("src", "tests"):
        for source in sorted((ROOT / tree).rglob("*.rs")):
            for line_number, line in enumerate(source.read_text().splitlines(), 1):
                if UNOWNED.search(line):
                    failures.append(f"{source.relative_to(ROOT)}:{line_number}")
    if failures:
        print(
            "Unowned temporary allocations; retain a common::ScratchDir or ScratchImage:",
            file=sys.stderr,
        )
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("test-image ownership policy: no unowned system-temporary allocations")
    return 0


if __name__ == "__main__":
    sys.exit(main())
