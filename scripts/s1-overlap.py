#!/usr/bin/env python3
"""Keep S1's prose out of this repository (#154).

S1, "bcachefs: Principles of Operation", is generated from the GPL-2.0
tools repository. docs/clean-room.md takes facts, names and numbers from it
and never its text. This is what checks that: it compares the repository's
own text with S1's, eight words at a time, and fails on any run of eight or
more words they share.

S1's text is never committed. `build` reads the PDF and writes only a hash
of each 8-word phrase in it (the first 8 bytes of its SHA-256) to
tests/data/s1-shingles.bin, which says nothing of what the phrases were.
`check` hashes the repository's phrases the same way and looks them up.

  s1-overlap.py build PDF            rewrite the hashes from S1's PDF; the
                                     PDF must be the pinned revision
  s1-overlap.py check [--root DIR] [--hashes FILE]
                                     fail on any shared run of 8+ words
  s1-overlap.py build-text FILE --hashes OUT
                                     hashes from plain text (for tests)

Words are lowercase runs of letters, digits and underscores, so formatting,
punctuation and line breaks do not hide a copy.
"""
import hashlib
import re
import subprocess
import sys
import zlib
from pathlib import Path

N = 8
# The S1 revision the committed hashes were made from: 1.39.7+efec39c,
# fetched 2026-10-12 from https://bcachefs.org/bcachefs-principles-of-operation.pdf
PDF_SHA256 = "50e95f298a3f877c39814fe71e14a7412c6a72d85f6e75cd78cf538767ad699e"
HASHES = Path("tests/data/s1-shingles.bin")
TEXT_SUFFIXES = (".rs", ".md", ".sh", ".py", ".yml", ".yaml", ".toml", ".h", ".c", ".txt")
WORD = re.compile(r"[a-z0-9_]+")

# TeX's OT1 ligature slots, and the punctuation the PDF escapes in octal.
LIGATURES = {0o033: "ff", 0o034: "fi", 0o035: "fl", 0o036: "ffi", 0o037: "ffl"}


def unescape(s: str) -> str:
    def octal(m):
        v = int(m.group(1), 8)
        return LIGATURES.get(v, chr(v))
    s = re.sub(r"\\([0-7]{3})", octal, s)
    return re.sub(r"\\(.)", r"\1", s)


def pdf_text(data: bytes) -> str:
    out = []
    for stream in re.findall(rb"stream\r?\n(.*?)\r?\nendstream", data, re.S):
        try:
            content = zlib.decompress(stream)
        except zlib.error:
            continue
        for array, single in re.findall(rb"\[(.*?)\]\s*TJ|\((.*?)\)\s*Tj", content, re.S):
            if single:
                out.append(single)
            else:
                for piece, kern in re.findall(rb"\(((?:\\.|[^\\)])*)\)|(-?\d+\.?\d*)", array):
                    if piece:
                        out.append(piece)
                    elif kern and float(kern) < -200:
                        out.append(b" ")
            out.append(b"\n")
    return unescape(b"".join(out).decode("latin-1"))


def digest(words) -> bytes:
    return hashlib.sha256(" ".join(words).encode()).digest()[:8]


def shingles(words):
    return {digest(words[i:i + N]) for i in range(len(words) - N + 1)}


def write_hashes(words, path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(sorted(shingles(words))))
    print(f"s1-overlap: {len(words)} words, {path.stat().st_size // 8} phrases hashed into {path}")


def load(path: Path):
    b = path.read_bytes()
    return {b[i:i + 8] for i in range(0, len(b), 8)}


def tracked(root: Path):
    out = subprocess.run(["git", "-C", str(root), "ls-files"], capture_output=True, text=True, check=True)
    return [root / f for f in out.stdout.splitlines() if f.endswith(TEXT_SUFFIXES)]


def runs(path: Path, known):
    """Each run of N+ words of `path` that S1 shares: (first line, last line, words)."""
    tokens = []  # (word, line)
    for lineno, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
        tokens += [(w, lineno) for w in WORD.findall(line.lower())]
    words = [w for w, _ in tokens]
    hit = [digest(words[i:i + N]) in known for i in range(len(words) - N + 1)]
    found, i = [], 0
    while i < len(hit):
        if not hit[i]:
            i += 1
            continue
        j = i
        while j + 1 < len(hit) and hit[j + 1]:
            j += 1
        last = j + N - 1
        found.append((tokens[i][1], tokens[last][1], words[i:last + 1]))
        i = j + 1
    return found


def check(root: Path, hashes: Path) -> int:
    if not hashes.is_file():
        print(f"s1-overlap: {hashes} is missing: `s1-overlap.py build PDF` makes it", file=sys.stderr)
        return 1
    known = load(hashes)
    bad = 0
    for path in tracked(root):
        if not path.is_file():
            continue
        for first, last, words in runs(path, known):
            bad += 1
            where = f"{path.relative_to(root)}:{first}" + (f"-{last}" if last != first else "")
            print(f"s1-overlap: {where}: {len(words)} words S1 has verbatim: {' '.join(words)}", file=sys.stderr)
    if bad:
        print(f"s1-overlap: {bad} run(s) of {N}+ words copied from S1; restate them as facts "
              "(docs/clean-room.md, \"Never transcribe\")", file=sys.stderr)
        return 1
    print(f"s1-overlap: no run of {N} or more words is S1's verbatim")
    return 0


def main(argv) -> int:
    args = list(argv)
    hashes = HASHES
    if "--hashes" in args:
        k = args.index("--hashes")
        hashes = Path(args[k + 1])
        del args[k:k + 2]
    root = Path(".")
    if "--root" in args:
        k = args.index("--root")
        root = Path(args[k + 1])
        del args[k:k + 2]
    if args[:1] == ["build"] and len(args) == 2:
        data = Path(args[1]).read_bytes()
        got = hashlib.sha256(data).hexdigest()
        if got != PDF_SHA256:
            print(f"s1-overlap: the PDF is {got}, not the pinned {PDF_SHA256}: S1 has a new "
                  "revision; re-read it (docs/clean-room.md) and move the pin", file=sys.stderr)
            return 1
        write_hashes(WORD.findall(pdf_text(data).lower()), hashes)
        return 0
    if args[:1] == ["build-text"] and len(args) == 2:
        write_hashes(WORD.findall(Path(args[1]).read_text().lower()), hashes)
        return 0
    if args[:1] == ["check"] and len(args) == 1:
        return check(root.resolve(), hashes if hashes.is_absolute() else root / hashes)
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
