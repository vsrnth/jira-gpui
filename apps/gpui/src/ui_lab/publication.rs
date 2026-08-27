//! Safe temporary-file publication for UI-lab artifacts.

use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use anyhow::{Context as _, Result, bail};

static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) fn publish_file(
    path: &Path,
    suffix: &str,
    write: impl FnOnce(&mut File) -> Result<()>,
) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }
    let (temporary, mut file) = create_temporary_file(path, suffix)?;
    let result = (|| {
        write(&mut file)
            .with_context(|| format!("write temporary file {}", temporary.display()))?;
        file.flush()
            .with_context(|| format!("flush temporary file {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary file {}", temporary.display()))?;
        verify_reserved_file(&temporary, &file)?;
        fs::rename(&temporary, path)
            .with_context(|| format!("atomically publish {}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn create_temporary_file(path: &Path, suffix: &str) -> Result<(PathBuf, File)> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("output path must name a file"))?
        .to_string_lossy();
    for _ in 0..100 {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{name}.jira-ui-{suffix}-{}-{id}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("create temporary file {}", temporary.display()));
            }
        }
    }
    bail!(
        "could not allocate a temporary file beside {}",
        path.display()
    )
}

fn verify_reserved_file(path: &Path, file: &File) -> Result<()> {
    let path_metadata = fs::symlink_metadata(path)
        .with_context(|| format!("verify temporary file identity {}", path.display()))?;
    if !path_metadata.file_type().is_file() || path_metadata.file_type().is_symlink() {
        bail!(
            "temporary path is not a regular file before publishing {}",
            path.display()
        );
    }
    let file_metadata = file
        .metadata()
        .with_context(|| format!("read reserved temporary file identity {}", path.display()))?;
    if reserved_file_is_same(&path_metadata, &file_metadata) {
        Ok(())
    } else {
        bail!(
            "temporary path identity changed before publishing {}",
            path.display()
        )
    }
}

#[cfg(unix)]
fn reserved_file_is_same(path: &fs::Metadata, file: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    path.dev() == file.dev() && path.ino() == file.ino()
}

#[cfg(not(unix))]
fn reserved_file_is_same(_path: &fs::Metadata, _file: &fs::Metadata) -> bool {
    // The supported UI-lab targets expose dev/inode identity. Refuse publication elsewhere rather
    // than relying on a metadata comparison that cannot detect replacement races.
    false
}

pub(crate) fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove file {}", path.display())),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::publish_file;
    use std::{
        fs,
        io::Write as _,
        os::unix::{fs::symlink, io::AsRawFd as _},
        path::PathBuf,
    };

    #[test]
    fn replacing_reserved_path_with_fd_symlink_is_rejected() {
        let root = std::env::temp_dir().join(format!(
            "jira-ui-publication-symlink-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let output = root.join("capture.png");

        let error = publish_file(&output, "test", |file| {
            file.write_all(b"reserved bytes")?;
            let temporary = fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.extension().and_then(|extension| extension.to_str()) == Some("tmp")
                })
                .expect("reserved temporary file");
            let reserved_fd_path = PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()));
            fs::remove_file(&temporary).unwrap();
            symlink(reserved_fd_path, &temporary).unwrap();
            Ok(())
        })
        .unwrap_err();

        assert!(error.to_string().contains("not a regular file"));
        assert!(!output.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
