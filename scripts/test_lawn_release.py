import hashlib
import json
import os
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from scripts import lawn_release


class LawnReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name, content in {
            "Cargo.toml": '[package]\nname = "lawn"\nversion = "0.1.0"\n',
            "src/protocol/wire.rs": "pub const PROTOCOL_VERSION: u32 = 22;\n",
            "src/protocol/endpoint.rs": "pub const ENDPOINT_PROTOCOL_GENERATION: u32 = 1;\n",
            "docs/lawn/releases/0.1.0.md": "First Lawn release.\n",
            "LICENSE": "license\n", "SPONSORS.md": "upstream sponsors\n", "README.md": "Lawn\n",
        }.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        self.binary = self.root / "lawn"
        self.binary.write_bytes(struct.pack("<II", 0xFEEDFACF, 0x0100000C) + b"test binary")
        self.output = self.root / "artifacts"

    def package(self, tag="lawn-v0.1.0"):
        lawn_release.package(self.binary, self.output, tag, self.root)

    @patch.object(lawn_release.subprocess, "check_output", return_value="lawn 0.1.0\n")
    def test_archive_manifest_and_checksums_agree(self, version):
        self.package()
        manifest = json.loads((self.output / "latest.json").read_text())
        asset = self.output / "lawn-macos-aarch64"
        self.assertEqual(manifest["assets"], {"macos-aarch64": {
            "url": "https://github.com/wkentaro/lawn/releases/download/lawn-v0.1.0/lawn-macos-aarch64",
            "sha256": hashlib.sha256(self.binary.read_bytes()).hexdigest(),
        }})
        self.assertEqual((manifest["version"], manifest["protocol"], manifest["endpoint_generation"]),
                         ("0.1.0", 22, 1))
        if os.name != "nt":
            self.assertEqual(asset.stat().st_mode & 0o777, 0o755)
        with tarfile.open(self.output / "lawn-0.1.0-macos-aarch64.tar.gz") as archive:
            self.assertEqual(archive.getnames(), ["lawn", "LICENSE", "SPONSORS.md", "README.md"])
            self.assertEqual(archive.extractfile("lawn").read(), self.binary.read_bytes())
        for line in (self.output / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split("  ")
            self.assertEqual(digest, hashlib.sha256((self.output / name).read_bytes()).hexdigest())
        version.assert_called_once_with([str(self.binary.resolve()), "--version"], text=True)

    def test_wrong_tag_or_architecture_creates_no_artifacts(self):
        for tag in ["v0.1.0", "lawn-v0.2.0", "lawn-v0.1.0;echo nope"]:
            with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, "tag must match"):
                self.package(tag)
        self.binary.write_bytes(b"not an Apple Silicon binary")
        with self.assertRaisesRegex(ValueError, "Apple Silicon"):
            self.package()
        self.assertFalse(self.output.exists())

    @patch.object(lawn_release.subprocess, "check_output", return_value="herdr 0.9.5\n")
    def test_wrong_binary_version_creates_no_artifacts(self, _version):
        with self.assertRaisesRegex(ValueError, "binary version mismatch"):
            self.package()
        self.assertFalse(self.output.exists())

    @patch.object(lawn_release.subprocess, "check_output", return_value="lawn 0.1.0\n")
    def test_existing_output_is_never_overwritten(self, _version):
        self.output.mkdir()
        sentinel = self.output / "keep"
        sentinel.write_text("existing output")
        with self.assertRaises(FileExistsError):
            self.package()
        self.assertEqual(sentinel.read_text(), "existing output")
        self.assertEqual(list(self.output.iterdir()), [sentinel])


if __name__ == "__main__":
    unittest.main()
