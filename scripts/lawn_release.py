"""Package a checked Apple Silicon Lawn binary; never tag or publish it."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tarfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
ASSET = "lawn-macos-aarch64"


def rust_constant(root: Path, source: str, name: str) -> int:
    match = re.search(rf"pub const {name}: u32 = ([0-9]+);", (root / source).read_text())
    if not match:
        raise ValueError(f"missing {name} in {source}")
    return int(match.group(1))


def package(binary: Path, output: Path, tag: str, root: Path = ROOT) -> None:
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version) or tag != f"lawn-v{version}":
        raise ValueError("tag must match Lawn's stable Cargo version: lawn-v<version>")
    binary = binary.resolve(strict=True)
    with binary.open("rb") as stream:
        if stream.read(8) != struct.pack("<II", 0xFEEDFACF, 0x0100000C):
            raise ValueError("expected an Apple Silicon Mach-O executable")
    actual = subprocess.check_output([str(binary), "--version"], text=True).strip()
    if actual != f"lawn {version}":
        raise ValueError(f"binary version mismatch: {actual!r}")
    notes = (root / "docs/lawn/releases" / f"{version}.md").read_text().strip()
    if not notes:
        raise ValueError("release notes must not be empty")
    protocol = rust_constant(root, "src/protocol/wire.rs", "PROTOCOL_VERSION")
    generation = rust_constant(root, "src/protocol/endpoint.rs", "ENDPOINT_PROTOCOL_GENERATION")
    output.mkdir(parents=True, exist_ok=False)
    asset = output / ASSET
    shutil.copyfile(binary, asset)
    asset.chmod(0o755)
    archive = output / f"lawn-{version}-macos-aarch64.tar.gz"
    with tarfile.open(archive, "w:gz") as tar:
        tar.add(asset, arcname="lawn")
        for name in ["LICENSE", "SPONSORS.md", "README.md"]:
            tar.add(root / name, arcname=name)
    digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
    manifest = {
        "version": version,
        "protocol": protocol,
        "endpoint_generation": generation,
        "notes": notes,
        "assets": {"macos-aarch64": {
            "url": f"https://github.com/wkentaro/lawn/releases/download/{tag}/{ASSET}",
            "sha256": digest(asset),
        }},
    }
    (output / "release-notes.md").write_text(notes + "\n")
    manifest_path = output / "latest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n")
    (output / "SHA256SUMS").write_text("".join(
        f"{digest(path)}  {path.name}\n" for path in [asset, archive, manifest_path]
    ))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    try:
        package(args.binary, args.output, args.tag)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
