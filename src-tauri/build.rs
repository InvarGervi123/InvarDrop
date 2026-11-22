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
        println!("cargo:rerun-if-changed=android/bluetooth-permission.xml");
        let bluetooth = include_str!("android/bluetooth-permission.xml").trim();
        let bluetooth_block =
            format!("    <!-- InvarDrop managed Bluetooth permissions -->\n    {bluetooth}\n\n");
        xml = xml.replace(&bluetooth_block, "");
        let position = xml
            .find("    <application")
            .expect("Android manifest application element");
        xml.insert_str(position, &bluetooth_block);

        // The private profile includes data encrypted with a device-bound key and
        // files staged for recovery. Exclude it from Android backup and migration.
        let privacy = concat!(
            "\n        android:allowBackup=\"false\"",
            "\n        android:fullBackupContent=\"false\"",
            "\n        android:dataExtractionRules=\"@xml/invardrop_extraction_rules\""
        );
        xml = xml.replace(privacy, "");
        let application = xml
            .find("    <application")
            .expect("Android application element");
        let attributes = application + "    <application".len();
        let end = xml[attributes..]
            .find('>')
            .expect("Android application opening tag");
        assert!(
            ["allowBackup", "fullBackupContent", "dataExtractionRules"]
                .iter()
                .all(|attribute| !xml[attributes..attributes + end]
                    .contains(&format!("android:{attribute}"))),
            "Remove custom backup attributes from the generated Android manifest"
        );
        xml.insert_str(attributes, privacy);
        let rules_source = "android/invardrop_extraction_rules.xml";
        let rules_dir = std::path::Path::new("gen/android/app/src/main/res/xml");
        println!("cargo:rerun-if-changed={rules_source}");
        let rules = std::fs::read(rules_source).expect("Android extraction rules source");
        let rules_destination = rules_dir.join("invardrop_extraction_rules.xml");
        if std::fs::read(&rules_destination).ok().as_deref() != Some(&rules) {
            std::fs::create_dir_all(rules_dir).expect("Android XML resources directory");
            std::fs::write(rules_destination, rules).expect("Android extraction rules");
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
            "BluetoothPlugin.kt",
            "BluetoothPolicy.kt",
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
