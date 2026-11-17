#!/usr/bin/env python3
"""Run pure Android import and Downloads regressions with the existing Gradle Kotlin compiler."""
import os
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
cache = Path(os.environ.get("GRADLE_USER_HOME", Path.home() / ".gradle"))
libs = sorted(cache.glob("wrapper/dists/gradle-*-bin/*/gradle-*/lib"))
if not libs:
    raise SystemExit("Initialize the Android Gradle wrapper before running these tests")
lib = libs[-1]
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
