"""Inspect package payloads without installing or deleting user data."""
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import struct
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = ROOT / "src-tauri/target/release/bundle"
FORBIDDEN = {".git", ".aws", ".ssh", ".codex", ".agents", "node_modules", "src-tauri", "identity.enc", "profile.lock", "received", "prepared", "resume-v1", ".env", ".netrc", ".npmrc", "ffmpeg", "ffmpeg.exe"}


def check_name(name):
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts or FORBIDDEN.intersection(part.casefold() for part in path.parts):
        raise RuntimeError(f"Unexpected package path: {name}")


def check_text(data, name):
    if b"\0" not in data[:4096] and re.search(rb"-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----", data):
        raise RuntimeError(f"Private key material in package: {name}")


def ar_members(path):
    with path.open("rb") as archive:
        assert archive.read(8) == b"!<arch>\n", "Invalid Debian archive"
        while header := archive.read(60):
            assert len(header) == 60 and header[58:] == b"`\n", "Invalid ar member"
            size = int(header[48:58]); assert 0 <= size <= 512 * 1024 * 1024
            content = archive.read(size); assert len(content) == size
            if size % 2: archive.read(1)
            yield header[:16].decode().strip().rstrip("/"), content


def verify_deb(package):
    members = dict(ar_members(package))
    control = next(value for key, value in members.items() if key.startswith("control.tar"))
    data = next(value for key, value in members.items() if key.startswith("data.tar"))
    with tarfile.open(fileobj=io.BytesIO(control)) as archive:
        assert not any(Path(name).name in {"preinst", "postinst", "prerm", "postrm", "conffiles"} for name in archive.getnames()), "Unexpected maintainer scripts or user configuration"
        entry = next(x for x in archive if Path(x.name).name == "control")
        metadata = archive.extractfile(entry).read().decode()
    assert "Package: invar-drop\n" in metadata
    assert "libwebkit2gtk-4.1-0" in metadata and "libgtk-3-0" in metadata
    files = {}
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for entry in archive:
            name = entry.name.removeprefix("./"); check_name(name)
            assert not entry.mode & 0o6000, "Unexpected setuid/setgid file"
            if entry.isdir(): continue
            assert entry.isfile(), f"Unexpected package entry type: {name}"
            assert name == "usr/bin/invardrop" or name == "usr/share/applications/InvarDrop.desktop" or re.fullmatch(r"usr/share/icons/hicolor/\d+x\d+/apps/invardrop\.png", name) or name in {"usr/share/doc/invar-drop/copyright", "usr/share/metainfo/io.github.invargervi.invardrop.metainfo.xml"} or (name.startswith("usr/lib/") and Path(name).name in {"LICENSE", "PACKAGING.md"}), f"Unexpected first-party payload: {name}"
            content = archive.extractfile(entry).read(); check_text(content, name)
            if name.endswith("/apps/invardrop.png"):
                side = int(name.split("/")[4].split("x")[0])
                assert content[:8] == b"\x89PNG\r\n\x1a\n" and struct.unpack(">II", content[16:24]) == (side, side), "Incorrect icon dimensions"
            files[name] = hashlib.sha256(content).hexdigest()
            if name == "usr/bin/invardrop": assert entry.mode & 0o111
            if name.endswith(".metainfo.xml"):
                with tempfile.NamedTemporaryFile(suffix=".xml") as metadata_file:
                    metadata_file.write(content); metadata_file.flush()
                    subprocess.run(["appstreamcli", "validate", "--no-net", metadata_file.name], check=True)
            if name.endswith(".desktop"):
                text = content.decode()
                assert "Exec=invardrop\n" in text and "Icon=invardrop\n" in text
                assert "Terminal=false" in text and "FileTransfer;" in text
                with tempfile.NamedTemporaryFile(suffix=".desktop") as desktop:
                    desktop.write(content); desktop.flush()
                    subprocess.run(["desktop-file-validate", desktop.name], check=True)
    assert any(name.endswith("/LICENSE") for name in files)
    assert any(name.endswith("/PACKAGING.md") for name in files)
    return {"control": metadata, "files": files}


def verify_appimage(package):
    assert os.access(package, os.X_OK), "AppImage is not executable"
    with tempfile.TemporaryDirectory(prefix="invardrop-appimage-audit-") as temporary:
        subprocess.run([str(package), "--appimage-extract"], cwd=temporary, stdout=subprocess.DEVNULL, check=True)
        root = Path(temporary) / "squashfs-root"
        assert (root / "AppRun").exists()
        files = {}
        links = {}
        for path in root.rglob("*"):
            name = path.relative_to(root).as_posix(); check_name(name)
            if path.is_symlink():
                target = os.readlink(path)
                if target.startswith(("/home/", "/root/", "/Users/")):
                    raise RuntimeError(f"User-directory symlink in package: {name}")
                if FORBIDDEN.intersection(part.casefold() for part in PurePosixPath(target).parts):
                    raise RuntimeError(f"Forbidden symlink target: {name}")
                links[name] = target
                continue
            if not path.is_file(): continue
            assert not path.stat().st_mode & 0o6000, "Unexpected setuid/setgid runtime file"
            content = path.read_bytes(); check_text(content, name)
            if path.name == "AppRun" or path.suffix == ".sh":
                assert b"WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS" not in content, "WebKit sandbox override found"
            files[name] = hashlib.sha256(content).hexdigest()
        assert "usr/bin/invardrop" in files
        assert any(name.endswith("/LICENSE") for name in files)
        assert any(name.endswith("/PACKAGING.md") for name in files)
        return {"files": files, "symlinks": links, "runtime_sandbox_override": False}


if __name__ == "__main__":
    packages = sorted(BUNDLE.glob("deb/*.deb")) + sorted(BUNDLE.glob("appimage/*.AppImage"))
    assert len(packages) == 2, "Expected exactly one Debian package and one AppImage"
    report = {}
    for package in packages:
        result = verify_deb(package) if package.suffix == ".deb" else verify_appimage(package)
        result["sha256"] = hashlib.sha256(package.read_bytes()).hexdigest()
        result["size"] = package.stat().st_size
        report[package.name] = result
    (BUNDLE / "package-audit-linux.json").write_text(json.dumps(report, indent=2) + "\n")
    (BUNDLE / "SHA256SUMS-linux.txt").write_text("".join(f"{value['sha256']}  {name}\n" for name, value in report.items()))
    print("PASS: Debian and AppImage payload, metadata, permissions, license and secret-file audits")
