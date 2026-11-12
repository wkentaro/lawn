import hashlib
import json
import os
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
import zipfile
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

    def package(self, tag="lawn-v0.1.0", platform="macos-aarch64", windows_bundle=None):
        lawn_release.package(self.binary, self.output, tag, self.root, platform, windows_bundle)

    @patch.object(lawn_release.subprocess, "check_output", return_value="lawn 0.1.0\n")
    def test_archive_manifest_and_checksums_agree(self, version):
        self.package()
        asset = self.output / "lawn-macos-aarch64"
        if os.name != "nt":
            self.assertEqual(asset.stat().st_mode & 0o777, 0o755)
        with tarfile.open(self.output / "lawn-0.1.0-macos-aarch64.tar.gz") as archive:
            self.assertEqual(archive.getnames(), ["lawn", "LICENSE", "SPONSORS.md", "README.md"])
            self.assertEqual(archive.extractfile("lawn").read(), self.binary.read_bytes())
        inputs = self.root / "inputs"
        self.output.rename(inputs)
        for platform in lawn_release.PLATFORMS[1:]:
            name = f"lawn-{platform}{'.zip' if platform.startswith('windows') else ''}"
            (inputs / name).write_bytes(platform.encode())
        lawn_release.assemble(inputs, self.output, "lawn-v0.1.0", self.root)
        manifest = json.loads((self.output / "latest.json").read_text())
        self.assertEqual((manifest["version"], manifest["protocol"], manifest["endpoint_generation"]),
                         ("0.1.0", 22, 1))
        self.assertEqual(set(manifest["assets"]), set(lawn_release.PLATFORMS))
        for platform, item in manifest["assets"].items():
            name = item["url"].split("/")[-1]
            self.assertIn("/releases/download/lawn-v0.1.0/", item["url"])
            self.assertEqual(item["sha256"], hashlib.sha256((self.output / name).read_bytes()).hexdigest())
        for line in (self.output / "SHA256SUMS").read_text().splitlines():
            digest, name = line.split("  ")
            self.assertEqual(digest, hashlib.sha256((self.output / name).read_bytes()).hexdigest())
        version.assert_called_once_with([str(self.binary.resolve()), "--version"], text=True)

    def test_missing_asset_creates_no_manifest(self):
        inputs = self.root / "inputs"
        inputs.mkdir()
        with self.assertRaisesRegex(ValueError, "missing release asset"):
            lawn_release.assemble(inputs, self.output, "lawn-v0.1.0", self.root)
        self.assertFalse(self.output.exists())

    def test_wrong_tag_or_architecture_creates_no_artifacts(self):
        for tag in ["v0.1.0", "lawn-v0.2.0", "lawn-v0.1.0;echo nope"]:
            with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, "tag must match"):
                self.package(tag)
        self.binary.write_bytes(b"not an Apple Silicon binary")
        with self.assertRaisesRegex(ValueError, "macos-aarch64"):
            self.package()
        self.assertFalse(self.output.exists())

    @patch.object(lawn_release.subprocess, "check_output", return_value="lawn 0.1.0\n")
    def test_architecture_checks_and_windows_bundle(self, _version):
        for platform, data in [
            ("macos-x86_64", struct.pack("<II", 0xFEEDFACF, 0x01000007)),
            ("linux-x86_64", b"\x7fELF\x02" + b"\0" * 13 + struct.pack("<H", 62)),
            ("linux-aarch64", b"\x7fELF\x02" + b"\0" * 13 + struct.pack("<H", 183)),
        ]:
            with self.subTest(platform=platform):
                self.binary.write_bytes(data)
                self.package(platform=platform)
                self.assertEqual((self.output / f"lawn-{platform}").read_bytes(), data)
                for path in self.output.iterdir():
                    path.unlink()
                self.output.rmdir()
                with self.assertRaisesRegex(ValueError, "expected"):
                    self.package(platform="macos-aarch64")
        pe = bytearray(0x90)
        pe[:2] = b"MZ"
        struct.pack_into("<I", pe, 0x3C, 0x80)
        pe[0x80:0x84] = b"PE\0\0"
        struct.pack_into("<H", pe, 0x84, 0x8664)
        self.binary.write_bytes(pe)
        bundle = self.root / "windows.zip"
        with zipfile.ZipFile(bundle, "w") as archive:
            for name, data in {"lawn.exe": pe, "conpty/conpty.dll": b"dll", "conpty/x64/OpenConsole.exe": b"host", "conpty/arm64/OpenConsole.exe": b"arm-host", "conpty/herdr-conpty.json": b"{}"}.items():
                archive.writestr(name, data)
        self.package(platform="windows-x86_64", windows_bundle=bundle)
        self.assertEqual((self.output / "lawn-windows-x86_64.zip").read_bytes(), bundle.read_bytes())
        for path in self.output.iterdir():
            path.unlink()
        self.output.rmdir()
        with zipfile.ZipFile(bundle, "w") as archive:
            archive.writestr("lawn.exe", pe)
        with self.assertRaisesRegex(ValueError, "app-local ConPTY"):
            self.package(platform="windows-x86_64", windows_bundle=bundle)
        self.assertFalse(self.output.exists())

    def test_windows_installer_and_updater_use_only_lawn_paths(self):
        installer = (lawn_release.ROOT / "distribution/install.ps1").read_text()
        updater = (lawn_release.ROOT / "src/update.rs").read_text()
        self.assertIn('$env:LAWN_HOME', installer)
        self.assertIn('$env:LAWN_INSTALL_DIR', installer)
        self.assertIn('"Programs\\Lawn\\bin"', installer)
        self.assertIn('"https://github.com/wkentaro/lawn/releases/latest/download/latest.json"', installer)
        self.assertIn('"lawn.exe"', installer)
        self.assertIn('"conpty/herdr-conpty.json"', installer)
        self.assertIn('$stagedVersion -cne "lawn $expectedVersion"', installer)
        self.assertNotIn('herdr.exe', installer.lower())
        self.assertNotIn('$env:HERDR_', installer)
        self.assertIn('env::var_os("LAWN_INSTALL_DIR")', updater)
        self.assertIn('windows_installed_lawn_exe_path()', updater)
        self.assertIn('.join("Lawn")\n        .join("bin")\n        .join("lawn.exe")', updater)

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
