#!/usr/bin/env python3
"""Translate VM reference reports into the shared property envelope.

Only the reference printer and lister's text is read, never image bytes or
the Rust reader's output. Missing reports fail, naming their producer.
"""

import json
import re
import sys
from pathlib import Path

prefix = Path(sys.argv[1])


def report(suffix):
    file = Path(str(prefix) + suffix)
    if not file.is_file():
        raise SystemExit(f"{file} is missing: run `chore fixtures`")
    return file.read_text()


def size(text):
    match = re.fullmatch(r"([0-9.]+)([kMG]?)", text)
    if not match:
        raise ValueError(f"unexpected reference size: {text!r}")
    return round(
        float(match[1]) * {"": 1, "k": 1024, "M": 1024**2, "G": 1024**3}[match[2]]
    )


fields = {}
device = ""
for line in report(".super.txt").splitlines():
    if line.startswith("Device 0:"):
        device = "dev."
    elif ":" in line:
        key, value = line.split(":", 1)
        fields.setdefault(device + key.strip(), value.strip())

bucket = size(fields["dev.Bucket size"])
free = sum(
    int(n)
    for n in re.findall(r" type set 0:[^\n]*? len (\d+)", report(".freespace.txt"))
)
clean = {"0": False, "1": True}[fields["Clean"]]
label = fields["Label"]
json.dump(
    {
        "fs": "bcachefs",
        "label": None if label == "(none)" else label,
        "total_bytes": (int(fields["dev.Buckets"]) - int(fields["dev.First bucket"]))
        * bucket,
        "free_bytes": free * bucket,
        "block_size": size(fields["block_size"]),
        "dirty": not clean,
        "bcachefs": {
            "uuid": fields["External UUID"],
            "version": re.search(r"\((\d+\.\d+)\)", fields["Version"])[1],
            "btree_node_size": size(fields["btree_node_size"]),
            "devices": int(fields["Devices"]),
            "encrypted": "crypt" in fields["Sections"].split(","),
            "clean": clean,
        },
    },
    sys.stdout,
)
