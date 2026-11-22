"""Inspect and sign the ARM64 sideload APK using a locally managed release key."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import zipfile


HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
CONFIG = ROOT / "src-tauri/tauri.conf.json"


def run(*args):
    return subprocess.check_output(args, text=True)


def check_android_privacy(tools, apk):
    manifest = run(tools / "aapt2", "dump", "xmltree", "--file", "AndroidManifest.xml", apk)
    for attribute in ("allowBackup", "fullBackupContent", "usesCleartextTraffic"):
        if not re.search(rf":{attribute}\([^\n]*\)=false$", manifest, re.MULTILINE):
            raise ValueError(f"APK must set android:{attribute} to false")
    reference = re.search(r":dataExtractionRules\([^\n]*\)=@(0x[0-9a-f]+)$", manifest, re.MULTILINE)
    if reference is None:
        raise ValueError("APK manifest must reference Android extraction rules")
    resources = run(tools / "aapt2", "dump", "resources", apk)
    resource = re.search(
        rf"resource {reference[1]} xml/invardrop_extraction_rules\s*\n[^\n]*\(file\) (\S+)",
        resources,
    )
    if resource is None:
        raise ValueError("APK does not include its declared Android extraction rules")
    rules = run(tools / "aapt2", "dump", "xmltree", "--file", resource[1], apk)
    domains = {"root", "file", "database", "sharedpref", "external", "device_root",
               "device_file", "device_database", "device_sharedpref"}
    for mode in ("cloud-backup", "device-transfer"):
        section = re.search(rf"^(\s*)E: {mode}\b[^\n]*\n(.*?)(?=^\1E:|\Z)",
                            rules, re.MULTILINE | re.DOTALL)
        exclusions = set(re.findall(r'E: exclude[^\n]*\n\s*A: domain="([^"]+)"[^\n]*\n\s*A: path="\."', section[2])) if section else set()
        if exclusions != domains:
            raise ValueError(f"Android extraction rules must exclude private data from {mode}")

def check_package(tools, apk):
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    major, minor, patch = map(int, config["version"].split("-")[0].split("."))
    android = config["bundle"].get("android", {})
    version_code = android.get("versionCode", major * 1000000 + minor * 1000 + patch)
    badging = run(tools / "aapt2", "dump", "badging", apk)
    header = badging.splitlines()[0]
    for attribute, value in (
        ("name", config["identifier"]),
        ("versionCode", version_code),
        ("versionName", config["version"]),
    ):
        if not re.search(rf"(?:^| ){attribute}='{re.escape(str(value))}'(?: |$)", header):
            raise ValueError(f"Unexpected APK package {attribute}")
    if "application-debuggable" in badging.splitlines():
        raise ValueError("A debug build cannot be used as a release APK")
    target = re.search(r"^targetSdkVersion:\s*'(\d+)'", badging, re.MULTILINE)
    if target is None:
        raise ValueError("APK has no target SDK metadata")
    permissions = {"android.permission.INTERNET", "android.permission.BLUETOOTH_SCAN",
                   "android.permission.BLUETOOTH_ADVERTISE", "android.permission.BLUETOOTH_CONNECT"}
    check_android_privacy(tools, apk)
    if int(target[1]) >= 37:
        permissions.add("android.permission.ACCESS_LOCAL_NETWORK")
    declared = set(re.findall(r"^uses-permission: name='([^']+)'", badging, re.MULTILINE))
    if missing := permissions - declared:
        raise ValueError("APK is missing " + ", ".join(sorted(missing)))
    with zipfile.ZipFile(apk) as archive:
        libraries = [entry for entry in archive.infolist() if entry.filename.startswith("lib/") and not entry.is_dir()]
        if not libraries or any(
            not re.fullmatch(r"lib/arm64-v8a/[^/]+\.so", entry.filename)
            or entry.compress_type != zipfile.ZIP_STORED
            for entry in libraries
        ):
            raise ValueError("APK must contain only uncompressed ARM64 native libraries")
        if sum(entry.filename == "lib/arm64-v8a/libinvardrop_lib.so" for entry in libraries) != 1:
            raise ValueError("APK must contain the InvarDrop ARM64 library exactly once")
    subprocess.run([tools / "zipalign", "-c", "-P", "16", "4", apk], check=True)
    subprocess.run([sys.executable, HERE / "check-android-alignment.py", apk], check=True)
    print(f"PASS: {config['identifier']} {config['version']} ({version_code}), target SDK {target[1]}, ARM64 release manifest")


def check_unsigned(tools, apk):
    result = subprocess.run([tools / "apksigner", "verify", apk], capture_output=True)
    if result.returncode == 0:
        raise ValueError("Expected an unsigned APK, but the APK already has a valid signature")


def check_signature(tools, apk, expected):
    signatures = run(tools / "apksigner", "verify", "--verbose", "--print-certs", apk)
    fingerprints = re.findall(r"^Signer #\d+ certificate SHA-256 digest:\s*([0-9a-fA-F]+)\s*$",
                              signatures, re.MULTILINE)
    if [fingerprint.lower() for fingerprint in fingerprints] != [expected]:
        raise ValueError("APK signer does not match the expected certificate fingerprint")
    if not re.search(r"^Verified using v[23][^:]*: true$", signatures, re.MULTILINE):
        raise ValueError("APK must have a verified v2 or v3 signature")
    print(f"PASS: APK signature and certificate SHA-256 {expected}")


def required_env(name):
    if not (value := os.environ.get(name)):
        raise ValueError(f"Set {name} before signing")
    return value


def sign(tools, source, output, certificate):
    if output.exists() or output.is_symlink():
        raise ValueError("Choose a new output path")
    keystore = Path(required_env("INVARDROP_ANDROID_KEYSTORE")).expanduser().resolve(strict=True)
    if not keystore.is_file() or keystore.is_relative_to(ROOT):
        raise ValueError("Store the release keystore outside the repository")
    alias = required_env("INVARDROP_ANDROID_KEY_ALIAS")
    required_env("INVARDROP_ANDROID_STORE_PASSWORD")
    required_env("INVARDROP_ANDROID_KEY_PASSWORD")
    with tempfile.TemporaryDirectory(prefix=".invardrop-sign-", dir=output.parent) as scratch:
        aligned = Path(scratch) / "aligned.apk"
        signed = Path(scratch) / "signed.apk"
        subprocess.run([tools / "zipalign", "-P", "16", "4", source, aligned], check=True)
        check_package(tools, aligned)
        check_unsigned(tools, aligned)
        subprocess.run([
            tools / "apksigner", "sign", "--ks", keystore, "--ks-key-alias", alias,
            "--ks-pass", "env:INVARDROP_ANDROID_STORE_PASSWORD",
            "--key-pass", "env:INVARDROP_ANDROID_KEY_PASSWORD",
            "--v4-signing-enabled", "false", "--out", signed, aligned,
        ], check=True)
        check_package(tools, signed)
        check_signature(tools, signed, certificate)
        # Publish the checked file atomically without replacing anything.
        os.link(signed, output)


def sha256(apk):
    with apk.open("rb") as data:
        return hashlib.file_digest(data, "sha256").hexdigest()


def certificate_digest(value):
    digest = value.replace(":", "").lower()
    if not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise argparse.ArgumentTypeError("Expected the SHA-256 certificate fingerprint")
    return digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-tools", required=True, type=Path)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check-unsigned").add_argument("apk", type=Path)
    for name in ("sign", "verify"):
        command = commands.add_parser(name)
        command.add_argument("apk", type=Path)
        if name == "sign":
            command.add_argument("output", type=Path)
        command.add_argument("--certificate-sha256", required=True, type=certificate_digest)
    args = parser.parse_args()
    tools = args.build_tools.resolve()
    try:
        if args.command == "sign":
            check_unsigned(tools, args.apk)
            sign(tools, args.apk, args.output, args.certificate_sha256)
            artifact = args.output
        else:
            check_package(tools, args.apk)
            if args.command == "verify":
                check_signature(tools, args.apk, args.certificate_sha256)
            else:
                check_unsigned(tools, args.apk)
            artifact = args.apk
        print(f"{sha256(artifact)}  {artifact.name}")
    except (OSError, ValueError, subprocess.CalledProcessError, zipfile.BadZipFile) as error:
        parser.exit(1, f"Android APK check failed: {error}\n")


if __name__ == "__main__":
    main()
