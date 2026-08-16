//! Secure Linux-local persistence bootstrap for the Phase 1 shell.

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use jira_storage::SqliteStore;

const APP_DIRECTORY: &str = "jira-desk";
const DATABASE_FILENAME: &str = "jira-desk.sqlite3";
const ENV_XDG_DATA_HOME: &str = "XDG_DATA_HOME";
const ENV_HOME: &str = "HOME";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalDataError;

pub(crate) fn open_store() -> Result<Arc<SqliteStore>, LocalDataError> {
    let xdg_data_home = env::var_os(ENV_XDG_DATA_HOME).map(PathBuf::from);
    let home = env::var_os(ENV_HOME).map(PathBuf::from);
    let app_directory = prepare_app_directory(xdg_data_home.as_deref(), home.as_deref())?;
    SqliteStore::open(app_directory.join(DATABASE_FILENAME))
        .map(Arc::new)
        .map_err(|_| LocalDataError)
}

pub(crate) fn resolve_app_directory(
    xdg_data_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, LocalDataError> {
    if let Some(xdg_data_home) = xdg_data_home.filter(|path| !path.as_os_str().is_empty()) {
        if !xdg_data_home.is_absolute() {
            return Err(LocalDataError);
        }
        return Ok(xdg_data_home.join(APP_DIRECTORY));
    }

    let home = home
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(LocalDataError)?;
    if !home.is_absolute() {
        return Err(LocalDataError);
    }
    Ok(home.join(".local").join("share").join(APP_DIRECTORY))
}

fn prepare_app_directory(
    xdg_data_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, LocalDataError> {
    let app_directory = resolve_app_directory(xdg_data_home, home)?;
    match fs::symlink_metadata(&app_directory) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(LocalDataError);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&app_directory).map_err(|_| LocalDataError)?;
        }
        Err(_) => return Err(LocalDataError),
    }

    let metadata = fs::symlink_metadata(&app_directory).map_err(|_| LocalDataError)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LocalDataError);
    }
    restrict_directory(&app_directory)?;
    Ok(app_directory)
}

fn restrict_directory(path: &Path) -> Result<(), LocalDataError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| LocalDataError)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("jira-desk-{label}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn xdg_data_home_takes_precedence_over_home() {
        let app = resolve_app_directory(Some(Path::new("/xdg/data")), Some(Path::new("/home")))
            .expect("path");
        assert_eq!(app, PathBuf::from("/xdg/data/jira-desk"));
    }

    #[test]
    fn home_fallback_uses_local_share() {
        let app = resolve_app_directory(None, Some(Path::new("/home/developer"))).expect("path");
        assert_eq!(app, PathBuf::from("/home/developer/.local/share/jira-desk"));
    }

    #[test]
    fn empty_or_missing_roots_are_rejected() {
        assert!(resolve_app_directory(None, None).is_err());
        assert!(resolve_app_directory(Some(Path::new("")), Some(Path::new(""))).is_err());
    }

    #[test]
    fn relative_roots_are_rejected() {
        assert!(resolve_app_directory(Some(Path::new("relative-xdg")), None).is_err());
        assert!(resolve_app_directory(None, Some(Path::new("relative-home"))).is_err());
    }

    #[test]
    fn app_directory_is_created_with_restricted_permissions() {
        let root = temporary_root("permissions");
        let app = prepare_app_directory(Some(&root), None).expect("app directory");
        assert!(app.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&app).expect("metadata").permissions().mode() & 0o777,
                0o700
            );
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn final_app_directory_symlink_is_rejected() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("symlink");
        let target = root.join("target");
        let app = root.join(APP_DIRECTORY);
        fs::create_dir_all(&target).expect("target");
        symlink(&target, &app).expect("symlink");
        assert!(prepare_app_directory(Some(&root), None).is_err());
        fs::remove_file(app).expect("remove symlink");
        fs::remove_dir_all(root).expect("cleanup");
    }
}
