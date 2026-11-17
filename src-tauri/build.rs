fn main() {
    // NDK r27 defaults to 4 KiB ELF pages. Apply only to the Android cdylib;
    // desktop link settings and dependency versions remain unchanged.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-arg-cdylib=-Wl,-z,max-page-size=16384");
        println!("cargo:rustc-link-arg-cdylib=-Wl,-z,common-page-size=16384");
        println!("cargo:rerun-if-changed=android/network-permission.xml");
        let manifest = std::path::Path::new("gen/android/app/src/main/AndroidManifest.xml");
        let gradle = std::path::Path::new("gen/android/app/build.gradle.kts");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rerun-if-changed={}", gradle.display());
        let target_sdk = std::fs::read_to_string(gradle)
            .expect("Android Gradle config must exist")
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("targetSdk = ")
                    .and_then(|v| v.parse::<u32>().ok())
            })
            .expect("Android targetSdk must be a literal API level for permission configuration");
        let original =
            std::fs::read_to_string(manifest).expect("Android project must be initialized");
        let permission = include_str!("android/network-permission.xml").trim();
        let managed = format!(
            "    <!-- InvarDrop managed local network permission -->\n    {permission}\n\n"
        );
        let mut xml = original.replace(&managed, "");
        if target_sdk >= 37 && !xml.contains(permission) {
            let position = xml
                .find("    <application")
                .expect("Android manifest application element");
            xml.insert_str(position, &managed);
        }
        if xml != original {
            std::fs::write(manifest, xml).expect("Android permissions must be configured");
        }
        // Keep Android sources tracked outside Tauri's ignored generated project.
        let target =
            std::path::Path::new("gen/android/app/src/main/java/io/github/invargervi/invardrop");
        std::fs::create_dir_all(target).expect("Android project must be initialized");
        for file in [
            "FilePickerPlugin.kt",
            "DocumentImport.kt",
            "DownloadsPublisher.kt",
            "VerifiedDownload.kt",
            "NetworkPermissionPolicy.kt",
        ] {
            let source = std::path::Path::new("android").join(file);
            println!("cargo:rerun-if-changed={}", source.display());
            let content = std::fs::read(&source).expect("Android source must exist");
            let destination = target.join(file);
            if std::fs::read(&destination).ok().as_deref() != Some(&content) {
                std::fs::write(destination, content).expect("Android source must be copied");
            }
        }
    }
    tauri_build::build()
}
