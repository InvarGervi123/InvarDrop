#!/usr/bin/env python3
"""Run Android JVM regressions with the Kotlin compiler from the project Gradle wrapper."""
import os
from pathlib import Path
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
cache = Path(os.environ.get("GRADLE_USER_HOME", Path.home() / ".gradle"))
wrapper = root / "src-tauri/gen/android/gradle/wrapper/gradle-wrapper.properties"
if not wrapper.is_file():
    raise SystemExit("Initialize the Android project before running these tests")
distribution = re.search(r"/gradle-([0-9][0-9A-Za-z.-]*)-(bin|all)\.zip$", wrapper.read_text(), re.MULTILINE)
if distribution is None:
    raise SystemExit("Could not read the Android Gradle wrapper version")
version, archive = distribution.groups()
libs = sorted(cache.glob(f"wrapper/dists/gradle-{version}-{archive}/*/gradle-{version}/lib"))
if not libs:
    raise SystemExit("Run the Android Gradle wrapper before running these tests")
lib = libs[0]
stdlib = next(lib.glob("kotlin-stdlib-*.jar"))
with tempfile.TemporaryDirectory(prefix="invardrop-saf-tests-") as output:
    subprocess.run([
        "java", "-cp", str(lib / "*"), "org.jetbrains.kotlin.cli.jvm.K2JVMCompiler",
        "-no-stdlib", "-no-reflect", "-classpath", str(stdlib), "-d", output,
        str(root / "src-tauri/android/DocumentImport.kt"),
        str(root / "src-tauri/android/DocumentImportTest.kt"),
        str(root / "src-tauri/android/VerifiedDownload.kt"),
        str(root / "src-tauri/android/VerifiedDownloadTest.kt"),
        str(root / "src-tauri/android/NetworkPermissionPolicy.kt"),
        str(root / "src-tauri/android/NetworkPermissionPolicyTest.kt"),
        str(root / "src-tauri/android/BluetoothPolicy.kt"),
        str(root / "src-tauri/android/BluetoothPolicyTest.kt"),
    ], check=True)
    subprocess.run([
        "java", "-cp", os.pathsep.join([output, str(stdlib)]),
        "io.github.invargervi.invardrop.DocumentImportTestKt",
    ], check=True)

    subprocess.run([
        "java", "-cp", os.pathsep.join([output, str(stdlib)]),
        "io.github.invargervi.invardrop.VerifiedDownloadTestKt",
    ], check=True)

    subprocess.run([
        "java", "-cp", os.pathsep.join([output, str(stdlib)]),
        "io.github.invargervi.invardrop.NetworkPermissionPolicyTestKt",
    ], check=True)
    subprocess.run([
        "java", "-cp", os.pathsep.join([output, str(stdlib)]),
        "io.github.invargervi.invardrop.BluetoothPolicyTestKt",
    ], check=True)
