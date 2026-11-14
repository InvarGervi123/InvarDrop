//! File IO stays in Rust. Remote metadata supplies a basename, never a path.
use super::error::{Error, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::{TempDir, TempPath};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const CHUNK_SIZE: usize = 64 * 1024;
pub const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024 * 1024;

/// Use the same conservative basename rules on both platforms, including Windows
/// aliases/ADS and formatting characters that could disguise the confirmation UI.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 200
        || name.ends_with(['.', ' '])
        || name.chars().any(|c| {
            c.is_control()
                || (c.is_whitespace() && c != ' ')
                || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                || matches!(c as u32, 0x061c | 0x200e..=0x200f | 0x202a..=0x202e | 0x2066..=0x2069)
        })
    {
        return Err(Error::InvalidName);
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    }) {
        return Err(Error::InvalidName);
    }
    Ok(())
}

fn validate_size(size: u64) -> Result<()> {
    if size > MAX_FILE_SIZE {
        Err(Error::Limit)
    } else {
        Ok(())
    }
}

pub struct Source {
    pub name: String,
    pub size: u64,
    file: tokio::fs::File,
    remaining: u64,
    hasher: blake3::Hasher,
    ended: bool,
}
impl Source {
    /// The path comes only from the local native picker. Precheck special files
    /// before opening: opening an obvious FIFO for reading can otherwise block.
    pub async fn open(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(Error::InvalidName)?
            .to_owned();
        validate_name(&name)?;
        let before = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|_| Error::FileIo)?;
        if !before.is_file() {
            return Err(Error::FileIo);
        }
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|_| Error::FileIo)?;
        let metadata = file.metadata().await.map_err(|_| Error::FileIo)?;
        if !metadata.is_file() {
            return Err(Error::FileIo);
        }
        let size = metadata.len();
        validate_size(size)?;
        Ok(Self {
            name,
            size,
            file,
            remaining: size,
            hasher: blake3::Hasher::new(),
            ended: false,
        })
    }
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        if self.ended {
            return Ok(None);
        }
        if self.remaining == 0 {
            let mut extra = [0; 1];
            if self
                .file
                .read(&mut extra)
                .await
                .map_err(|_| Error::FileIo)?
                != 0
            {
                return Err(Error::FileChanged);
            }
            self.ended = true;
            return Ok(None);
        }
        let mut chunk = vec![0; self.remaining.min(CHUNK_SIZE as u64) as usize];
        let read = self
            .file
            .read(&mut chunk)
            .await
            .map_err(|_| Error::FileIo)?;
        if read == 0 {
            return Err(Error::FileChanged);
        }
        chunk.truncate(read);
        self.remaining -= read as u64;
        self.hasher.update(&chunk);
        Ok(Some(chunk))
    }
    /// The transfer actor calls this only after next_chunk returns None.
    pub fn digest(&self) -> String {
        self.hasher.finalize().to_hex().to_string()
    }
}

pub struct Sink {
    // Drop the file handle before removing its path/directory, including Windows.
    file: tokio::fs::File,
    temporary: TempPath,
    directory: TempDir,
    name: String,
    size: u64,
    received: u64,
    hasher: blake3::Hasher,
}
impl Sink {
    pub async fn create(inbox: &Path, name: &str, size: u64) -> Result<Self> {
        validate_name(name)?;
        validate_size(size)?;
        if !inbox.is_absolute() {
            return Err(Error::FileIo);
        }
        let inbox = inbox.to_owned();
        let (file, temporary, directory) = tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&inbox).map_err(|_| Error::FileIo)?;
            let metadata = fs::symlink_metadata(&inbox).map_err(|_| Error::FileIo)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(Error::FileIo);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&inbox, fs::Permissions::from_mode(0o700))
                    .map_err(|_| Error::FileIo)?;
            }
            // Each accepted file has its own private directory: identical peer
            // names cannot collide with an earlier receipt or overwrite it.
            let mut builder = tempfile::Builder::new();
            builder.prefix("received-");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                builder.permissions(fs::Permissions::from_mode(0o700));
            }
            let directory = builder.tempdir_in(inbox).map_err(|_| Error::FileIo)?;
            let (file, temporary) = tempfile::NamedTempFile::new_in(directory.path())
                .map_err(|_| Error::FileIo)?
                .into_parts();
            Ok((file, temporary, directory))
        })
        .await
        .map_err(|_| Error::FileIo)??;
        Ok(Self {
            file: tokio::fs::File::from_std(file),
            temporary,
            directory,
            name: name.into(),
            size,
            received: 0,
            hasher: blake3::Hasher::new(),
        })
    }
    pub async fn write(&mut self, chunk: &[u8]) -> Result<()> {
        if chunk.is_empty() || chunk.len() > CHUNK_SIZE {
            return Err(Error::Protocol);
        }
        let next = self
            .received
            .checked_add(chunk.len() as u64)
            .ok_or(Error::Integrity)?;
        if next > self.size {
            return Err(Error::Integrity);
        }
        self.file
            .write_all(chunk)
            .await
            .map_err(|_| Error::FileIo)?;
        // Wait for the completed chunk's OS operation; this is not per-chunk
        // fsync. It also avoids leaving a pending write after a normal drop.
        self.file.flush().await.map_err(|_| Error::FileIo)?;
        self.hasher.update(chunk);
        self.received = next;
        Ok(())
    }
    /// The caller owns this future through its local commit/publication task.
    /// Once the blocking finalization starts, canceling its await cannot undo it.
    pub async fn finish(mut self, expected: &str) -> Result<PathBuf> {
        let expected = blake3::Hash::from_hex(expected).map_err(|_| Error::Integrity)?;
        if self.received != self.size || self.hasher.finalize() != expected {
            return Err(Error::Integrity);
        }
        self.file.flush().await.map_err(|_| Error::FileIo)?;
        self.file.sync_all().await.map_err(|_| Error::FileIo)?;
        let Self {
            file,
            temporary,
            directory,
            name,
            ..
        } = self;
        // into_std waits for pending operations. No cloned/open Windows handle
        // remains when the temporary path is renamed.
        drop(file.into_std().await);
        tokio::task::spawn_blocking(move || {
            let destination = directory.path().join(name);
            if let Err(error) = temporary.persist_noclobber(&destination) {
                // A local process might have placed a file here. Preserve it,
                // deleting only our own tempfile, even on a collision/error.
                let occupied = !matches!(
                    fs::symlink_metadata(&destination),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound
                );
                drop(error);
                if occupied {
                    let _ = directory.keep();
                }
                return Err(Error::FileIo);
            }
            #[cfg(unix)]
            {
                fs::File::open(directory.path())
                    .and_then(|f| f.sync_all())
                    .map_err(|_| Error::FileIo)?;
                let parent = directory.path().parent().ok_or(Error::FileIo)?;
                fs::File::open(parent)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| Error::FileIo)?;
            }
            let _ = directory.keep();
            Ok(destination)
        })
        .await
        .map_err(|_| Error::FileIo)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn copy(source: &Path, inbox: &Path) -> PathBuf {
        let mut source = Source::open(source).await.unwrap();
        let mut sink = Sink::create(inbox, &source.name, source.size)
            .await
            .unwrap();
        while let Some(chunk) = source.next_chunk().await.unwrap() {
            assert!(!chunk.is_empty() && chunk.len() <= CHUNK_SIZE);
            sink.write(&chunk).await.unwrap();
        }
        assert!(source.next_chunk().await.unwrap().is_none());
        sink.finish(&source.digest()).await.unwrap()
    }
    fn empty(inbox: &Path) {
        assert!(fs::read_dir(inbox).unwrap().next().is_none());
    }
    #[test]
    fn rejects_paths_windows_aliases_and_disguised_names_on_every_os() {
        for name in [
            "",
            ".",
            "..",
            "../secret",
            "/tmp/file",
            "C:\\file",
            "folder\\file",
            "file:stream",
            "CON",
            "con.txt",
            "CON .txt",
            "PRN",
            "AUX.bin",
            "NUL",
            "COM1.log",
            "LPT9",
            "com¹.txt",
            "CONIN$",
            "file.",
            "file ",
            "a\0b",
            "line\nname",
            "x\u{202e}txt",
            "a\u{2066}b",
            "a?b",
            "a*b",
            "a|b",
            "a<b",
            "a\"b",
        ] {
            assert!(
                matches!(validate_name(name), Err(Error::InvalidName)),
                "Invalid basename accepted"
            );
        }
        assert!(validate_name(&"x".repeat(201)).is_err());
        for name in [
            "photo.jpg",
            "notes 2026.txt",
            ".config",
            "данные.bin",
            "תמונה.png",
            "COM10.txt",
        ] {
            validate_name(name).unwrap();
        }
        assert!(validate_size(MAX_FILE_SIZE).is_ok());
        assert!(matches!(
            validate_size(MAX_FILE_SIZE + 1),
            Err(Error::Limit)
        ));
    }
    #[tokio::test]
    async fn empty_and_multiple_chunk_files_round_trip_without_modifying_source() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("received");
        for bytes in [Vec::new(), vec![0x5a; CHUNK_SIZE * 3 + 29]] {
            let path = dir.path().join("source.bin");
            fs::write(&path, &bytes).unwrap();
            let received = copy(&path, &inbox).await;
            assert_eq!(fs::read(received).unwrap(), bytes);
            assert_eq!(fs::read(path).unwrap(), bytes);
        }
    }
    #[tokio::test]
    async fn duplicate_basenames_never_overwrite_an_earlier_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("received");
        let path = dir.path().join("same.txt");
        fs::write(&path, b"first").unwrap();
        let first = copy(&path, &inbox).await;
        fs::write(path.clone(), b"second").unwrap();
        let second = copy(&path, &inbox).await;
        assert!(first != second);
        assert_eq!(fs::read(first).unwrap(), b"first");
        assert_eq!(fs::read(second).unwrap(), b"second");
    }
    #[tokio::test]
    async fn source_size_changes_and_directories_are_rejected() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(Source::open(dir.path()).await, Err(Error::FileIo)));
        let path = dir.path().join("mutable.bin");
        fs::write(&path, b"abc").unwrap();
        let mut source = Source::open(&path).await.unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"def")
            .unwrap();
        assert_eq!(source.next_chunk().await.unwrap().unwrap(), b"abc");
        assert!(matches!(source.next_chunk().await, Err(Error::FileChanged)));
        drop(source);
        fs::write(&path, b"abc").unwrap();
        let mut source = Source::open(&path).await.unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(1)
            .unwrap();
        assert_eq!(source.next_chunk().await.unwrap().unwrap(), b"a");
        assert!(matches!(source.next_chunk().await, Err(Error::FileChanged)));
    }
    #[tokio::test]
    async fn corrupt_short_oversized_and_incomplete_receipts_have_no_final_file() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("received");
        let mut sink = Sink::create(&inbox, "test.bin", 3).await.unwrap();
        sink.write(b"abc").await.unwrap();
        assert!(matches!(
            sink.finish(blake3::hash(b"abd").to_hex().as_ref()).await,
            Err(Error::Integrity)
        ));
        empty(&inbox);
        let mut sink = Sink::create(&inbox, "test.bin", 3).await.unwrap();
        sink.write(b"a").await.unwrap();
        assert!(matches!(
            sink.finish(blake3::hash(b"a").to_hex().as_ref()).await,
            Err(Error::Integrity)
        ));
        empty(&inbox);
        let mut sink = Sink::create(&inbox, "test.bin", 3).await.unwrap();
        assert!(matches!(sink.write(b"abcd").await, Err(Error::Integrity)));
        assert!(matches!(sink.write(&[]).await, Err(Error::Protocol)));
        assert!(matches!(
            sink.write(&vec![0; CHUNK_SIZE + 1]).await,
            Err(Error::Protocol)
        ));
        drop(sink);
        empty(&inbox);
        let mut sink = Sink::create(&inbox, "test.bin", 3).await.unwrap();
        sink.write(b"a").await.unwrap();
        drop(sink);
        empty(&inbox);
        let sink = Sink::create(&inbox, "test.bin", 0).await.unwrap();
        assert!(matches!(
            sink.finish("not-a-digest").await,
            Err(Error::Integrity)
        ));
        empty(&inbox);
        assert!(matches!(
            Sink::create(&inbox, "test.bin", MAX_FILE_SIZE + 1).await,
            Err(Error::Limit)
        ));
        empty(&inbox);
    }
    #[tokio::test]
    async fn finalization_collision_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("received");
        let mut sink = Sink::create(&inbox, "same.txt", 3).await.unwrap();
        sink.write(b"new").await.unwrap();
        let destination = sink.directory.path().join("same.txt");
        let temporary = sink.temporary.to_owned();
        fs::write(&destination, b"existing").unwrap();
        assert!(matches!(
            sink.finish(blake3::hash(b"new").to_hex().as_ref()).await,
            Err(Error::FileIo)
        ));
        assert_eq!(fs::read(destination).unwrap(), b"existing");
        assert!(!temporary.exists());
    }
    #[tokio::test]
    async fn file_io_failure_is_reported_and_staging_is_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let bad_inbox = dir.path().join("not-a-directory");
        fs::write(&bad_inbox, b"existing").unwrap();
        assert!(matches!(
            Sink::create(&bad_inbox, "a.txt", 1).await,
            Err(Error::FileIo)
        ));
        assert_eq!(fs::read(bad_inbox).unwrap(), b"existing");
        let inbox = dir.path().join("received");
        let mut sink = Sink::create(&inbox, "a.txt", 1).await.unwrap();
        sink.file = tokio::fs::File::from_std(fs::File::open(&sink.temporary).unwrap());
        assert!(matches!(sink.write(b"x").await, Err(Error::FileIo)));
        drop(sink);
        empty(&inbox);
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_sources_and_inboxes_are_rejected_and_receipt_modes_are_private() {
        use std::os::unix::{fs::PermissionsExt, fs::symlink};
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        fs::write(&source, b"x").unwrap();
        let alias = dir.path().join("alias.txt");
        symlink(&source, &alias).unwrap();
        assert!(matches!(Source::open(&alias).await, Err(Error::FileIo)));
        let inbox = dir.path().join("received");
        fs::create_dir(&inbox).unwrap();
        let alias = dir.path().join("alias");
        symlink(&inbox, &alias).unwrap();
        assert!(matches!(
            Sink::create(&alias, "a.txt", 1).await,
            Err(Error::FileIo)
        ));
        let receipt = copy(&source, &inbox).await;
        for path in [&inbox, receipt.parent().unwrap()] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        assert_eq!(
            fs::metadata(receipt).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
