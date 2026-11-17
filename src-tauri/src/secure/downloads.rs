//! Publish only verified receipts. Network staging and recovery stay private.
use super::{
    bundle,
    error::{Error, Result},
    files,
    resume::Manifest,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::Manager;

#[derive(Clone)]
pub enum Destination {
    #[cfg(any(not(target_os = "android"), test))]
    Desktop(PathBuf),
    #[cfg(target_os = "android")]
    Android(tauri::plugin::PluginHandle<tauri::Wry>),
    #[cfg(any(not(target_os = "android"), test))]
    Unavailable,
}
impl Destination {
    pub fn for_app(app: &tauri::AppHandle) -> Self {
        #[cfg(target_os = "android")]
        {
            Self::Android(app.state::<crate::android_files::Picker>().handle.clone())
        }
        #[cfg(not(target_os = "android"))]
        {
            // Tauri resolves XDG_DOWNLOAD_DIR on Linux and FOLDERID_Downloads on Windows.
            app.path()
                .download_dir()
                .map(Self::Desktop)
                .unwrap_or(Self::Unavailable)
        }
    }
    pub async fn publish(&self, id: &str, source: &Path, manifest: &Manifest) -> Result<String> {
        manifest.validate()?;
        if !uuid::Uuid::parse_str(id).is_ok_and(|v| v.to_string() == id) {
            return Err(Error::Protocol);
        }
        let id = id.to_owned();
        let source = source.to_owned();
        let manifest = manifest.clone();
        match self {
            #[cfg(any(not(target_os = "android"), test))]
            Self::Desktop(downloads) => {
                let downloads = downloads.clone();
                tokio::task::spawn_blocking(move || desktop(&downloads, &id, &source, &manifest))
                    .await
                    .map_err(|_| Error::Downloads)?
                    .map_err(|error| {
                        if error == Error::Integrity {
                            error
                        } else {
                            Error::Downloads
                        }
                    })
                    .map(|path| path.to_string_lossy().into_owned())
            }
            #[cfg(target_os = "android")]
            Self::Android(handle) => {
                let request =
                    tokio::task::spawn_blocking(move || android_request(id, source, manifest))
                        .await
                        .map_err(|_| Error::Downloads)??;
                #[derive(serde::Deserialize)]
                struct Response {
                    location: String,
                }
                let response: Response = handle
                    .run_mobile_plugin_async("publish", request)
                    .await
                    .map_err(|_| Error::Downloads)?;
                Ok(response.location)
            }
            #[cfg(any(not(target_os = "android"), test))]
            Self::Unavailable => Err(Error::Downloads),
        }
    }
}

fn source_path(root: &Path, manifest: &Manifest, relative: &str) -> PathBuf {
    if manifest.folder {
        root.join(relative)
    } else {
        root.to_owned()
    }
}
fn read_file(path: &Path) -> Result<File> {
    if !bundle::regular_metadata(path)?.is_file() {
        return Err(Error::FileIo);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|_| Error::FileIo)?;
    if !file.metadata().map_err(|_| Error::FileIo)?.is_file() {
        return Err(Error::FileIo);
    }
    Ok(file)
}
// One bounded read loop for copy and verification; sizes and counters remain u64.
fn copy_verified(path: &Path, output: &mut impl Write, size: u64, expected: &str) -> Result<()> {
    let mut source = read_file(path)?;
    if source.metadata().map_err(|_| Error::FileIo)?.len() != size {
        return Err(Error::Integrity);
    }
    let mut buffer = [0u8; files::CHUNK_SIZE];
    let mut hash = blake3::Hasher::new();
    let mut count = 0u64;
    loop {
        let n = source.read(&mut buffer).map_err(|_| Error::FileIo)?;
        if n == 0 {
            break;
        }
        count = count.checked_add(n as u64).ok_or(Error::Limit)?;
        if count > size {
            return Err(Error::Integrity);
        }
        hash.update(&buffer[..n]);
        output.write_all(&buffer[..n]).map_err(|_| Error::FileIo)?;
    }
    if count != size || hash.finalize().to_hex().as_str() != expected {
        return Err(Error::Integrity);
    }
    Ok(())
}
fn verify_layout(root: &Path, manifest: &Manifest) -> Result<()> {
    if manifest.folder {
        if !bundle::regular_metadata(root)?.is_dir() {
            return Err(Error::Integrity);
        }
        fn walk(root: &Path, current: &Path, found: &mut Vec<String>) -> Result<()> {
            for entry in fs::read_dir(current).map_err(|_| Error::FileIo)? {
                let entry = entry.map_err(|_| Error::FileIo)?;
                if found.len() >= bundle::MAX_ENTRIES {
                    return Err(Error::Limit);
                }
                let path = entry.path();
                let relative = path.strip_prefix(root).map_err(|_| Error::Integrity)?;
                if relative.components().count() > bundle::MAX_DEPTH {
                    return Err(Error::Limit);
                }
                found.push(
                    relative
                        .to_str()
                        .ok_or(Error::InvalidName)?
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                );
                if bundle::regular_metadata(&path)?.is_dir() {
                    walk(root, &path, found)?;
                }
            }
            Ok(())
        }
        let mut found = Vec::new();
        walk(root, root, &mut found)?;
        found.sort();
        if !found.iter().map(String::as_str).eq(manifest
            .tree
            .entries
            .iter()
            .map(|e| e.path.as_str()))
        {
            return Err(Error::Integrity);
        }
    }
    Ok(())
}
#[cfg(any(not(target_os = "android"), test))]
fn verify(root: &Path, manifest: &Manifest) -> Result<()> {
    verify_layout(root, manifest)?;
    for (entry, hash) in manifest.tree.entries.iter().zip(&manifest.hashes) {
        let path = source_path(root, manifest, &entry.path);
        match entry.kind {
            bundle::Kind::Directory => {
                if !bundle::regular_metadata(&path)?.is_dir() {
                    return Err(Error::Integrity);
                }
            }
            bundle::Kind::File => copy_verified(&path, &mut std::io::sink(), entry.size, hash)?,
        }
    }
    Ok(())
}
#[cfg(any(not(target_os = "android"), test))]
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Downloads)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
#[cfg(any(not(target_os = "android"), test))]
struct PrivateStaging(PathBuf);
#[cfg(any(not(target_os = "android"), test))]
impl PrivateStaging {
    fn path(&self) -> &Path {
        &self.0
    }
    fn new(parent: &Path) -> Result<Self> {
        #[cfg(not(windows))]
        {
            let mut builder = tempfile::Builder::new();
            builder.prefix(".invardrop-save-");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                builder.permissions(fs::Permissions::from_mode(0o700));
            }
            Ok(Self(
                builder
                    .tempdir_in(parent)
                    .map_err(|_| Error::Downloads)?
                    .keep(),
            ))
        }
        #[cfg(windows)]
        {
            use std::{os::windows::ffi::OsStrExt, ptr};
            use windows_sys::Win32::{
                Foundation::LocalFree,
                Security::{
                    Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
                    SECURITY_ATTRIBUTES,
                },
                Storage::FileSystem::CreateDirectoryW,
            };
            // Owner Rights and SYSTEM only, inherited by every staged child.
            // Protected DACL prevents a shared parent from granting access.
            let sddl: Vec<u16> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut descriptor = ptr::null_mut();
            // SAFETY: valid UTF-16 input and output pointer; LocalFree owns the allocation.
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(Error::Downloads);
            }
            let path = parent.join(format!(".invardrop-save-{}", uuid::Uuid::new_v4()));
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            // SAFETY: descriptor and NUL-terminated path remain valid for the call.
            let created = unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) };
            // SAFETY: allocation came from ConvertStringSecurityDescriptorToSecurityDescriptorW.
            unsafe {
                LocalFree(descriptor);
            }
            if created == 0 {
                return Err(Error::Downloads);
            }
            Ok(Self(path))
        }
    }
}
#[cfg(any(not(target_os = "android"), test))]
impl Drop for PrivateStaging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(any(not(target_os = "android"), test))]
fn desktop(downloads: &Path, id: &str, source: &Path, manifest: &Manifest) -> Result<PathBuf> {
    if !downloads.is_absolute() {
        return Err(Error::Downloads);
    }
    fs::create_dir_all(downloads).map_err(|_| Error::Downloads)?;
    // System-configured Downloads may itself be a legitimate redirected folder.
    let downloads = downloads.canonicalize().map_err(|_| Error::Downloads)?;
    let inbox = downloads.join("InvarDrop");
    match fs::create_dir(&inbox) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(Error::Downloads),
    }
    if !bundle::regular_metadata(&inbox)?.is_dir() {
        return Err(Error::Downloads);
    }
    let receipt = inbox.join(format!("received-{id}"));
    let destination = receipt.join(&manifest.tree.name);
    if receipt.try_exists().map_err(|_| Error::Downloads)? {
        if !bundle::regular_metadata(&receipt)?.is_dir() {
            return Err(Error::Downloads);
        }
        // Idempotent recovery never truncates or overwrites an existing receipt.
        verify(&destination, manifest).map_err(|_| Error::Downloads)?;
        return Ok(destination);
    }
    // Keep a private destination-volume staging directory outside Downloads.
    // Windows uses a protected DACL even when redirected Downloads has a shared parent.
    let staging = PrivateStaging::new(downloads.parent().ok_or(Error::Downloads)?)?;
    let payload = staging.path().join("payload");
    fs::create_dir(&payload).map_err(|_| Error::Downloads)?;
    let output_root = payload.join(&manifest.tree.name);
    if manifest.folder {
        fs::create_dir(&output_root).map_err(|_| Error::Downloads)?;
    }
    for (entry, hash) in manifest.tree.entries.iter().zip(&manifest.hashes) {
        let output = source_path(&output_root, manifest, &entry.path);
        if entry.kind == bundle::Kind::Directory {
            fs::create_dir(output).map_err(|_| Error::Downloads)?;
        } else {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(output).map_err(|_| Error::Downloads)?;
            copy_verified(
                &source_path(source, manifest, &entry.path),
                &mut file,
                entry.size,
                hash,
            )?;
            file.sync_all().map_err(|_| Error::Downloads)?;
        }
    }
    // Verify disk contents, not only bytes passed to write().
    verify(&output_root, manifest)?;
    for entry in manifest
        .tree
        .entries
        .iter()
        .rev()
        .filter(|e| e.kind == bundle::Kind::Directory)
    {
        sync_dir(&source_path(&output_root, manifest, &entry.path))?;
    }
    if manifest.folder {
        sync_dir(&output_root)?;
    }
    sync_dir(&payload)?;
    if !bundle::regular_metadata(&inbox)?.is_dir() {
        return Err(Error::Downloads);
    }
    if bundle::rename_noreplace(&payload, &receipt).is_err() {
        // Another finalizer can finish the same recovered operation first.
        // Reuse matching bytes; a collision or symlink is never replaced.
        if !bundle::regular_metadata(&receipt).is_ok_and(|m| m.is_dir()) {
            return Err(Error::Downloads);
        }
        verify(&destination, manifest).map_err(|_| Error::Downloads)?;
    }
    sync_dir(&receipt)?;
    sync_dir(&inbox)?;
    sync_dir(&downloads)?;
    Ok(destination)
}

#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
struct AndroidRequest {
    id: String,
    source: String,
    name: String,
    folder: bool,
    entries: Vec<AndroidEntry>,
}
#[cfg(target_os = "android")]
#[derive(serde::Serialize)]
struct AndroidEntry {
    path: String,
    directory: bool,
    size: u64,
    sha256: String,
}
#[cfg(target_os = "android")]
fn android_request(id: String, source: PathBuf, manifest: Manifest) -> Result<AndroidRequest> {
    use ring::digest;
    struct Sha(digest::Context);
    impl Write for Sha {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    bundle::regular_metadata(&source)?;
    let source = source.canonicalize().map_err(|_| Error::Downloads)?;
    verify_layout(&source, &manifest)?;
    let mut entries = Vec::new();
    for (entry, expected) in manifest.tree.entries.iter().zip(&manifest.hashes) {
        let sha256 = if entry.kind == bundle::Kind::File {
            let mut sha = Sha(digest::Context::new(&digest::SHA256));
            // Bind the native copy's SHA-256 to the original BLAKE3 proof in the
            // same bounded read. No Kotlin crypto dependency or protocol change.
            copy_verified(
                &source_path(&source, &manifest, &entry.path),
                &mut sha,
                entry.size,
                expected,
            )?;
            super::identity::hex(sha.0.finish().as_ref())
        } else {
            String::new()
        };
        entries.push(AndroidEntry {
            path: entry.path.clone(),
            directory: entry.kind == bundle::Kind::Directory,
            size: entry.size,
            sha256,
        });
    }
    Ok(AndroidRequest {
        id,
        source: source.to_string_lossy().into_owned(),
        name: manifest.tree.name,
        folder: manifest.folder,
        entries,
    })
}

#[cfg(test)]
#[path = "downloads_tests.rs"]
mod tests;
