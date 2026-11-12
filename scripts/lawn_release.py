"""Check native Lawn release binaries and assemble immutable tagged assets."""

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
import zipfile


ROOT = Path(__file__).resolve().parents[1]
PLATFORMS = ("macos-aarch64", "macos-x86_64", "linux-aarch64", "linux-x86_64", "windows-x86_64")


def rust_constant(root: Path, source: str, name: str) -> int:
    match = re.search(rf"pub const {name}: u32 = ([0-9]+);", (root / source).read_text())
    if not match:
        raise ValueError(f"missing {name} in {source}")
    return int(match.group(1))


def release_version(root: Path, tag: str) -> str:
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version) or tag != f"lawn-v{version}":
        raise ValueError("tag must match Lawn's stable Cargo version: lawn-v<version>")
    return version


def check_architecture(binary: Path, platform: str) -> None:
    data = binary.read_bytes()
    machine = {"aarch64": 0xAA64, "x86_64": 0x8664}
    arch = platform.split("-")[-1]
    if platform.startswith("macos"):
        cpu = 0x0100000C if arch == "aarch64" else 0x01000007
        valid = data[:8] == struct.pack("<II", 0xFEEDFACF, cpu)
    elif platform.startswith("linux"):
        cpu = 183 if arch == "aarch64" else 62
        valid = data[:5] == b"\x7fELF\x02" and len(data) >= 20 and struct.unpack_from("<H", data, 18)[0] == cpu
    else:
        valid = (data[:2] == b"MZ" and len(data) >= 0x40 and
                 (offset := struct.unpack_from("<I", data, 0x3C)[0]) + 6 <= len(data) and
                 data[offset:offset + 4] == b"PE\0\0" and
                 struct.unpack_from("<H", data, offset + 4)[0] == machine[arch])
    if not valid:
        raise ValueError(f"expected a {platform} executable")


def package(binary: Path, output: Path, tag: str, root: Path = ROOT,
            platform: str = "macos-aarch64", windows_bundle: Path | None = None) -> None:
    version = release_version(root, tag)
    if platform not in PLATFORMS:
        raise ValueError(f"unsupported platform: {platform}")
    binary = binary.resolve(strict=True)
    check_architecture(binary, platform)
    actual = subprocess.check_output([str(binary), "--version"], text=True).strip()
    if actual != f"lawn {version}":
        raise ValueError(f"binary version mismatch: {actual!r}")
    if platform == "windows-x86_64":
        if windows_bundle is None:
            raise ValueError("Windows requires an app-local ConPTY bundle")
        with zipfile.ZipFile(windows_bundle) as archive:
            required = {"conpty/conpty.dll", "conpty/x64/OpenConsole.exe", "conpty/arm64/OpenConsole.exe", "conpty/herdr-conpty.json"}
            if archive.read("lawn.exe") != binary.read_bytes() or not required.issubset(archive.namelist()):
                raise ValueError("Windows bundle lacks Lawn or app-local ConPTY")
    output.mkdir(parents=True, exist_ok=False)
    asset = output / f"lawn-{platform}"
    if platform == "windows-x86_64":
        shutil.copyfile(windows_bundle, asset.with_suffix(".zip"))
    else:
        shutil.copyfile(binary, asset)
        asset.chmod(0o755)
        if platform == "macos-aarch64":
            archive = output / f"lawn-{version}-macos-aarch64.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                tar.add(asset, arcname="lawn")
                for name in ["LICENSE", "SPONSORS.md", "README.md"]:
                    tar.add(root / name, arcname=name)


def assemble(inputs: Path, output: Path, tag: str, root: Path = ROOT) -> None:
    version = release_version(root, tag)
    notes = (root / "docs/lawn/releases" / f"{version}.md").read_text().strip()
    if not notes:
        raise ValueError("release notes must not be empty")
    names = [f"lawn-{platform}{'.zip' if platform.startswith('windows') else ''}" for platform in PLATFORMS]
    names.append(f"lawn-{version}-macos-aarch64.tar.gz")
    for name in names:
        if not (inputs / name).is_file():
            raise ValueError(f"missing release asset: {name}")
    output.mkdir(parents=True, exist_ok=False)
    assets = {}
    for platform, name in zip(PLATFORMS, names):
        source = inputs / name
        destination = output / name
        shutil.copyfile(source, destination)
        assets[platform] = {
            "url": f"https://github.com/wkentaro/lawn/releases/download/{tag}/{name}",
            "sha256": hashlib.sha256(destination.read_bytes()).hexdigest(),
        }
    archive = output / names[-1]
    shutil.copyfile(inputs / archive.name, archive)
    manifest = {
        "version": version,
        "protocol": rust_constant(root, "src/protocol/wire.rs", "PROTOCOL_VERSION"),
        "endpoint_generation": rust_constant(root, "src/protocol/endpoint.rs", "ENDPOINT_PROTOCOL_GENERATION"),
        "notes": notes,
        "assets": assets,
    }
    (output / "release-notes.md").write_text(notes + "\n")
    (output / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (output / "SHA256SUMS").write_text("".join(
        f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
        for path in [*(output / name for name in names), output / "latest.json"]
    ))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--windows-bundle", type=Path)
    parser.add_argument("--platform", choices=PLATFORMS, default="macos-aarch64")
    parser.add_argument("--inputs", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    try:
        if args.inputs:
            assemble(args.inputs, args.output, args.tag)
        elif args.binary:
            package(args.binary, args.output, args.tag, platform=args.platform, windows_bundle=args.windows_bundle)
        else:
            parser.error("either --binary or --inputs is required")
    except (ValueError, OSError, subprocess.SubprocessError, zipfile.BadZipFile, KeyError) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
