//! Private hardened persistence for diagnostics JSONL.
//!
//! Filesystem failures are intentionally reduced to `Err(())`; the facade
//! disables itself after an unsuccessful append so diagnostics remain best
//! effort and cannot affect application behavior.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use super::schema::MAX_LINE_BYTES;

pub(super) const DIAGNOSTICS_FILENAME: &str = "diagnostics.jsonl";
pub(super) const DIAGNOSTICS_BACKUP_FILENAME: &str = "diagnostics.jsonl.1";
pub(super) const MAX_FILE_BYTES: u64 = 256 * 1024;

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn prepare_directory(directory: &Path) -> Result<PathBuf, ()> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => return Err(()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(directory).map_err(|_| ())?;
        }
        Err(_) => return Err(()),
    }
    let metadata = fs::symlink_metadata(directory).map_err(|_| ())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(());
    }
    restrict_directory(directory)?;
    Ok(directory.to_path_buf())
}

#[cfg_attr(not(test), allow(dead_code))]
fn restrict_directory(path: &Path) -> Result<(), ()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| ())?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn ensure_regular_or_missing(path: &Path) -> Result<bool, ()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(());
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(()),
    }
}

pub(super) fn append_line(active_path: &Path, backup_path: &Path, line: &[u8]) -> Result<(), ()> {
    if line.len() > MAX_LINE_BYTES {
        return Err(());
    }
    let active_exists = ensure_regular_or_missing(active_path)?;
    let backup_exists = ensure_regular_or_missing(backup_path)?;
    if active_exists {
        restrict_file(active_path)?;
    }
    if backup_exists {
        restrict_file(backup_path)?;
        let size = fs::metadata(backup_path).map_err(|_| ())?.len();
        if size > MAX_FILE_BYTES {
            truncate_regular(backup_path)?;
        }
    }

    let active_size = if active_exists {
        let size = fs::metadata(active_path).map_err(|_| ())?.len();
        if size > MAX_FILE_BYTES {
            truncate_regular(active_path)?;
            0
        } else {
            size
        }
    } else {
        0
    };
    let line_size = u64::try_from(line.len()).map_err(|_| ())?.saturating_add(1);
    if active_size.saturating_add(line_size) > MAX_FILE_BYTES && active_exists {
        // The backup was validated as a regular file above. Remove it before
        // rename so rotation remains portable and never leaves two oversized
        // files behind.
        if ensure_regular_or_missing(backup_path)? {
            fs::remove_file(backup_path).map_err(|_| ())?;
        }
        fs::rename(active_path, backup_path).map_err(|_| ())?;
        restrict_file(backup_path)?;
    }

    let _ = ensure_regular_or_missing(active_path)?;
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(active_path).map_err(|_| ())?;
    file.write_all(line).map_err(|_| ())?;
    file.write_all(b"\n").map_err(|_| ())?;
    file.flush().map_err(|_| ())?;
    restrict_file(active_path)
}

fn truncate_regular(path: &Path) -> Result<(), ()> {
    if !ensure_regular_or_missing(path)? {
        return Err(());
    }
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|_| ())?;
    file.set_len(0).map_err(|_| ())
}

fn restrict_file(path: &Path) -> Result<(), ()> {
    if !ensure_regular_or_missing(path)? {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|_| ())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::super::{
        DiagnosticFlow, DiagnosticsSink, ImagePreflight, ImageSignature, ImageSource, ResponseMime,
    };
    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "jira-desk-diagnostics-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn read_lines(path: &Path) -> Vec<String> {
        fs::read_to_string(path)
            .expect("diagnostics")
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn is_json_line(line: &str) -> bool {
        let bytes = line.as_bytes();
        if bytes.len() < 2 || bytes[0] != b'{' || bytes[bytes.len() - 1] != b'}' {
            return false;
        }
        let mut quoted = false;
        let mut escaped = false;
        for byte in bytes {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' && quoted {
                escaped = true;
            } else if *byte == b'"' {
                quoted = !quoted;
            }
        }
        !quoted && !escaped
    }

    #[test]
    fn creates_private_directory_and_files() {
        let root = temporary_root("permissions");
        let sink = DiagnosticsSink::for_directory(&root);
        sink.session_started();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&root).expect("root").permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(root.join(DIAGNOSTICS_FILENAME))
                    .expect("active")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_directory_and_files() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("symlink");
        let target = root.join("target");
        fs::create_dir_all(&target).expect("target");
        let linked = root.join("linked");
        symlink(&target, &linked).expect("directory symlink");
        let disabled = DiagnosticsSink::for_directory(&linked);
        disabled.session_started();
        assert!(!target.join(DIAGNOSTICS_FILENAME).exists());
        fs::remove_file(&linked).expect("remove directory symlink");
        fs::create_dir_all(&root).expect("root");

        let active_target = root.join("active-target");
        fs::write(&active_target, b"outside\n").expect("target file");
        symlink(&active_target, root.join(DIAGNOSTICS_FILENAME)).expect("active symlink");
        let active_sink = DiagnosticsSink::for_directory(&root);
        active_sink.session_started();
        assert_eq!(fs::read(&active_target).expect("outside"), b"outside\n");
        fs::remove_file(root.join(DIAGNOSTICS_FILENAME)).expect("remove active symlink");

        let backup_target = root.join("backup-target");
        fs::write(&backup_target, b"outside\n").expect("backup target");
        symlink(&backup_target, root.join(DIAGNOSTICS_BACKUP_FILENAME)).expect("backup symlink");
        let backup_sink = DiagnosticsSink::for_directory(&root);
        backup_sink.session_started();
        assert_eq!(fs::read(&backup_target).expect("outside"), b"outside\n");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn rotates_before_append_and_retains_one_bounded_backup() {
        let root = temporary_root("rotation");
        let sink = DiagnosticsSink::for_directory(&root);
        for _ in 0..5000 {
            sink.image_response(
                DiagnosticFlow::SelectedDetail,
                1,
                99,
                0,
                ImageSource::ResolvedAdf,
                ResponseMime::Png,
                ImageSignature::Png,
                12_345,
                ImagePreflight::Accepted,
            );
        }
        let active = root.join(DIAGNOSTICS_FILENAME);
        let backup = root.join(DIAGNOSTICS_BACKUP_FILENAME);
        assert!(fs::metadata(&active).expect("active").len() <= MAX_FILE_BYTES);
        assert!(fs::metadata(&backup).expect("backup").len() <= MAX_FILE_BYTES);
        assert!(fs::metadata(&backup).expect("backup").len() > 0);
        assert!(
            fs::metadata(&active).expect("active").len()
                + fs::metadata(&backup).expect("backup").len()
                <= MAX_FILE_BYTES * 2
        );
        assert!(read_lines(&active).iter().all(|line| is_json_line(line)));
        assert!(read_lines(&backup).iter().all(|line| is_json_line(line)));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn recovers_oversized_active_and_backup_without_preserving_them() {
        let root = temporary_root("oversized-recovery");
        fs::create_dir_all(&root).expect("root");
        fs::write(
            root.join(DIAGNOSTICS_FILENAME),
            vec![b'a'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("active");
        fs::write(
            root.join(DIAGNOSTICS_BACKUP_FILENAME),
            vec![b'b'; MAX_FILE_BYTES as usize + 1],
        )
        .expect("backup");

        let sink = DiagnosticsSink::for_directory(&root);
        sink.session_started();

        assert!(
            fs::metadata(root.join(DIAGNOSTICS_FILENAME))
                .expect("active")
                .len()
                <= MAX_FILE_BYTES
        );
        assert!(
            fs::metadata(root.join(DIAGNOSTICS_BACKUP_FILENAME))
                .expect("backup")
                .len()
                <= MAX_FILE_BYTES
        );
        assert!(
            read_lines(&root.join(DIAGNOSTICS_FILENAME))
                .iter()
                .all(|line| is_json_line(line))
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
