"""Exercise dpkg installation/removal on an isolated CI runner, never on the developer desktop."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess

assert os.environ.get("GITHUB_ACTIONS") == "true", "This lifecycle test is restricted to ephemeral GitHub runners"
root = Path(__file__).resolve().parents[1]
package, = (root / "src-tauri/target/release/bundle/deb").glob("*.deb")
assert not Path("/usr/bin/invardrop").exists(), "Refusing to replace an existing installation"
profile = Path.home() / ".local/share/io.github.invargervi.invardrop"
profile.mkdir(parents=True, exist_ok=False)
sentinels = {}
try:
    for name in ["security-v1/identity.enc", "security-v1/received/user-file.txt", "security-v1/resume-v1/checkpoint-sentinel"]:
        path = profile / name; path.parent.mkdir(parents=True, exist_ok=True)
        content = os.urandom(64); path.write_bytes(content)
        sentinels[path] = hashlib.sha256(content).digest()
    subprocess.run(["sudo", "dpkg", "--install", str(package)], check=True)
    assert os.access("/usr/bin/invardrop", os.X_OK)
    assert Path("/usr/share/applications/InvarDrop.desktop").is_file()
    subprocess.run(["sudo", "dpkg", "--remove", "invar-drop"], check=True)
    assert not Path("/usr/bin/invardrop").exists()
    assert all(path.exists() and hashlib.sha256(path.read_bytes()).digest() == digest for path, digest in sentinels.items()), "Uninstall changed user data"
    print("PASS: actual dpkg install/remove preserved profile, received-file and recovery sentinels")
finally:
    # Only this newly created CI test directory is removed; no real profile is reused.
    shutil.rmtree(profile)
