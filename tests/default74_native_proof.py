"""CI-only positive and negative controls over retained native observations."""

import copy
import hashlib
import importlib.util
import os
import struct
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
import default74_native_proof as native

archive, fixtures = sys.argv[1:3]
sys.argv[1:] = []


class NativeContract(unittest.TestCase):
    def test_changed_or_absent_archive_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            wrong = Path(tmp) / "wrong.zip"
            wrong.write_bytes(Path(archive).read_bytes() + b"changed")
            with self.assertRaises(ValueError):
                native.matches(native.CASE, native.BINDING, str(wrong))
            with self.assertRaises(FileNotFoundError):
                native.matches(native.CASE, native.BINDING, str(wrong) + ".absent")

    def test_classifier_requires_bound_independent_read(self):
        path = Path(__file__).resolve().parents[1] / "scripts/differential-fuzz.py"
        spec = importlib.util.spec_from_file_location("differential", path)
        differential = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(differential)
        with tempfile.TemporaryDirectory() as tmp:
            ref = Path(tmp) / "ref"
            ref.mkdir()
            (ref / "default-74.status").write_text("0\n")
            (ref / "default-74.txt").write_text("Running userspace offline fsck\n")
            (ref / "default-74.read.json").write_text(
                __import__("json").dumps(native.BINDING)
            )
            os.environ["DIFFERENTIAL_NATIVE74_ARCHIVE"] = archive
            self.assertEqual(differential.judged(tmp, native.CASE)["verdict"], "found")
            os.environ.pop("DIFFERENTIAL_NATIVE74_ARCHIVE")
            self.assertEqual(differential.judged(tmp, native.CASE)["verdict"], "clean")
            os.environ["DIFFERENTIAL_NATIVE74_ARCHIVE"] = archive
            for field in native.CASE:
                if field == "id":
                    continue
                with self.subTest(field=field):
                    self.assertEqual(
                        differential.judged(tmp, dict(native.CASE, **{field: "other"}))[
                            "verdict"
                        ],
                        "clean",
                    )

    def test_retained_native_positive(self):
        self.assertTrue(native.matches(native.CASE, native.BINDING, archive))

    def test_every_observation_is_required(self):
        observations = native.proof(archive)
        for name in observations:
            if name.endswith(
                (".key.txt", ".status", ".path.txt", ".stat.txt", ".read.txt")
            ) and (
                "default-74" in name
                or "default-control" in name
                or "fusemount-help.status" in name
            ):
                # Only observations consumed by the grader are contract fields.
                if name.endswith((".mount.txt", ".extents.txt")):
                    continue
                changed = copy.deepcopy(observations)
                changed[name] = "unrelated failure\n"
                with self.subTest(name=name), self.assertRaises(ValueError):
                    native.grade(changed)

    def test_each_binding_and_case_field_is_exact(self):
        for fields, other in (
            (native.BINDING, native.CASE),
            (native.CASE, native.BINDING),
        ):
            for name in fields:
                changed = dict(fields, **{name: "different"})
                case, binding = (
                    (other, changed) if fields is native.BINDING else (changed, other)
                )
                with self.subTest(name=name):
                    self.assertFalse(native.matches(case, binding, archive))
        self.assertFalse(native.matches(native.CASE, native.BINDING, None))

    def test_exact_patch_changes_checksum_field_and_no_data(self):
        self.assertEqual(
            native.digest(fixtures),
            "9f112ded30a1894c5d83948fb2c525ff075afaff74acc3e82b63deb98493007e",
        )
        import io

        with zipfile.ZipFile(fixtures) as z:
            tarball = z.read("fixtures.tar.gz")
        self.assertEqual(
            hashlib.sha256(tarball).hexdigest(),
            "27b15c9dffa3240b2ec3f3f3a84221c255c210b157f5888d2285f4e358025869",
        )
        with tarfile.open(fileobj=io.BytesIO(tarball)) as tar:
            original = tar.extractfile("fixtures/default.img").read()
        self.assertEqual(hashlib.sha256(original).hexdigest(), native.CONTROL)
        with zipfile.ZipFile(archive) as z:
            patch = z.read("kept/default-74/mutation.patch")
        self.assertEqual(
            patch,
            (Path(__file__).parent / "data/default-74-38840695392.patch").read_bytes(),
        )
        self.assertEqual(hashlib.sha256(patch).hexdigest(), native.PATCH)
        mutated = bytearray(original)
        i = 0
        while i < len(patch):
            at, n = struct.unpack_from("<QI", patch, i)
            i += 12
            self.assertTrue(3014656 <= at < at + n <= 3047424)
            mutated[at : at + n] = patch[i : i + n]
            i += n
        self.assertEqual(hashlib.sha256(mutated).hexdigest(), native.MUTATED)
        self.assertEqual(
            [i for i, (a, b) in enumerate(zip(original, mutated)) if a != b],
            [3041280, 3041281, 3041282, 3041283, 3041646, 3041647],
        )
        before, after = (
            struct.unpack_from("<Q", data, 3041640)[0] for data in (original, mutated)
        )
        self.assertEqual(before, 0x0A2F1BA705000002)
        self.assertEqual(after, 0x00011BA705000002)
        self.assertEqual(after & 0xFFFFFFFF, before & 0xFFFFFFFF)
        # Low 32 bits retain crc32-entry tag, one sector, crc32c type 5, no compression.
        self.assertEqual(after & 0xFFFFFFFF, 0x05000002)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            control, image, patchfile = (
                root / n for n in ("control.img", "mutated.img", "mutation.patch")
            )
            control.write_bytes(original)
            image.write_bytes(mutated)
            patchfile.write_bytes(patch)
            os.environ["DIFFERENTIAL_NATIVE74_ARCHIVE"] = archive
            self.assertEqual(
                native.bind(native.CASE, control, image, patchfile, "1.39.7"),
                native.BINDING,
            )
            mutated[0] ^= 1
            image.write_bytes(mutated)
            self.assertIsNone(
                native.bind(native.CASE, control, image, patchfile, "1.39.7")
            )


if __name__ == "__main__":
    unittest.main()
