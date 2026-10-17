# shellcheck shell=bash
# Embed a reference-made image without interpreting its filesystem bytes.
offset_embed() {
    python3 - "$1" "$2" "$3" <<'PY'
import shutil, sys
source, target, offset = sys.argv[1], sys.argv[2], int(sys.argv[3])
with open(source, 'rb') as src, open(target, 'wb') as dst:
    dst.write(b'P' * offset)
    shutil.copyfileobj(src, dst)
    dst.write(b'S' * 4096)
PY
}

# Check both sentinels and extract only the original device for a checker.
offset_extract() {
    python3 - "$1" "$2" "$3" "$4" <<'PY'
import sys
source, target, offset, size = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
with open(source, 'rb') as src, open(target, 'wb') as dst:
    assert src.read(offset) == b'P' * offset, 'the prefix changed'
    left = size
    while left:
        data = src.read(min(left, 1048576))
        assert data, 'the filesystem was truncated'
        dst.write(data)
        left -= len(data)
    assert src.read() == b'S' * 4096, 'the suffix changed'
PY
}
