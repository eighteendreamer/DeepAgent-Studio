"""Run with python3 -B on POSIX; no installed test dependencies."""
import importlib.util
import io
import os
from pathlib import Path
import stat
import tarfile
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("safe_archive", Path(__file__).parents[1] / "src" / "safe_archive.py")
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


@unittest.skipUnless(os.name == "posix", "POSIX helper requires dir_fd")
class SafeArchive(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="deepagent-archive-test-")
        self.root = Path(self.temp.name)
        self.dest = self.root / "out"
        self.dest.mkdir()
        self.archive = self.root / "input.zip"

    def tearDown(self):
        self.temp.cleanup()

    def zip(self, entries):
        with zipfile.ZipFile(self.archive, "w") as output:
            for name, content in entries:
                output.writestr(name, content)

    def request(self, action="preview", conflict="ask", **kwargs):
        return dict(action=action, archivePath=str(self.archive), destinationPath=str(self.dest), conflict=conflict, **kwargs)

    def test_unicode_binary_empty_and_cleanup(self):
        self.zip([("empty/", b""), ("nested/中文 ' $.bin", bytes(range(256)))])
        preview = helper.main(self.request())
        self.assertTrue(preview["ok"], preview)
        result = helper.main(self.request("extract", fingerprint=preview["preview"]["fingerprint"]))
        self.assertTrue(result["ok"], result)
        self.assertEqual((self.dest / "nested/中文 ' $.bin").read_bytes(), bytes(range(256)))
        self.assertTrue((self.dest / "empty").is_dir())
        self.assertFalse(list(self.dest.glob(".deepagent-extract-*")))

    def test_absolute_and_traversal_names(self):
        for name in ("/escape", "../escape", "a/../../escape", "a\\escape", "C:/escape", "a//b", "a/./b"):
            with self.subTest(name=name):
                self.zip([(name, b"bad")])
                self.assertFalse(helper.main(self.request())["ok"])
        self.assertEqual(list(self.dest.iterdir()), [])

    def test_duplicate_and_type_collision(self):
        for entries in ([("a", b"x"), ("a", b"y")], [("a", b"x"), ("a/b", b"y")], [("a/b", b"x"), ("a", b"y")]):
            self.zip(entries)
            self.assertFalse(helper.main(self.request())["ok"])

    def test_zip_symlink_and_special(self):
        for mode in (stat.S_IFLNK, stat.S_IFIFO, stat.S_IFCHR):
            item = zipfile.ZipInfo("link")
            item.external_attr = (mode | 0o777) << 16
            self.zip([(item, "../outside")])
            self.assertFalse(helper.main(self.request())["ok"])

    def test_tar_links_and_sparse_rejected(self):
        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.FIFOTYPE, tarfile.CHRTYPE):
            self.archive = self.root / "input.tar.gz"
            with tarfile.open(self.archive, "w:gz") as output:
                item = tarfile.TarInfo("unsafe")
                item.type, item.linkname = kind, "../outside"
                output.addfile(item)
            self.assertFalse(helper.main(self.request())["ok"])

    def test_tar_gzip_regular_file(self):
        self.archive = self.root / "input.tar.gz"
        with tarfile.open(self.archive, "w:gz") as output:
            item = tarfile.TarInfo("nested/file")
            item.size = 4
            output.addfile(item, io.BytesIO(b"data"))
        preview = helper.main(self.request())
        self.assertTrue(preview["ok"], preview)
        self.assertTrue(helper.main(self.request("extract", fingerprint=preview["preview"]["fingerprint"]))["ok"])
        self.assertEqual((self.dest / "nested/file").read_bytes(), b"data")

    def test_symlink_destination_never_written(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.dest / "nested").symlink_to(outside, target_is_directory=True)
        self.zip([("nested/file", b"bad")])
        self.assertFalse(helper.main(self.request())["ok"])
        self.assertFalse((outside / "file").exists())

    def test_archive_and_destination_changes_require_preview(self):
        self.zip([("file", b"first")])
        preview = helper.main(self.request("preview", "replace"))["preview"]
        (self.dest / "file").write_bytes(b"remote")
        result = helper.main(self.request("extract", "replace", fingerprint=preview["fingerprint"]))
        self.assertFalse(result["ok"])
        self.assertEqual((self.dest / "file").read_bytes(), b"remote")
        preview = helper.main(self.request("preview", "replace"))["preview"]
        self.zip([("file", b"changed")])
        self.assertFalse(helper.main(self.request("extract", "replace", fingerprint=preview["fingerprint"]))["ok"])

    def test_skip_and_replace(self):
        self.zip([("file", b"new")])
        (self.dest / "file").write_bytes(b"old")
        for policy, expected in (("skip", b"old"), ("replace", b"new")):
            preview = helper.main(self.request("preview", policy))["preview"]
            result = helper.main(self.request("extract", policy, fingerprint=preview["fingerprint"]))
            self.assertTrue(result["ok"], result)
            self.assertEqual((self.dest / "file").read_bytes(), expected)

    def test_quotas_include_implicit_directories(self):
        self.zip([("a/b/c/file", b"data")])
        old = helper.MAX_ENTRIES
        try:
            helper.MAX_ENTRIES = 3
            self.assertFalse(helper.main(self.request())["ok"])
        finally:
            helper.MAX_ENTRIES = old
        self.zip([("/".join(["a"] * 65), b"")])
        self.assertFalse(helper.main(self.request())["ok"])
        old = helper.MAX_BYTES
        try:
            helper.MAX_BYTES = 4
            self.zip([("file", b"exceeds")])
            self.assertFalse(helper.main(self.request())["ok"])
        finally:
            helper.MAX_BYTES = old

    def test_relative_input_and_self_replacement_rejected(self):
        with self.assertRaises(ValueError):
            helper.main(dict(self.request(), archivePath="input.zip"))
        self.archive = self.dest / "input.zip"
        self.zip([("input.zip", b"bad")])
        self.assertFalse(helper.main(self.request())["ok"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
