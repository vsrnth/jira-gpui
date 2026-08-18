//! Best-effort per-user integration for directly launched AppImages.
//!
//! Registration is gated on the AppImage runtime environment. The mounted
//! AppDir remains the source of truth for the desktop entry and icon, while
//! host-side writes are bounded and atomic.

use std::{
    env, fs,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const APPLICATIONS_DIRECTORY: &str = "applications";
const ICON_DIRECTORY: &str = "icons/hicolor/256x256/apps";
const DESKTOP_FILENAME: &str = "dev.jiradesk.JiraDesk.desktop";
const ICON_FILENAME: &str = "dev.jiradesk.JiraDesk.png";
const DESKTOP_SOURCE: &str = "usr/share/applications/dev.jiradesk.JiraDesk.desktop";
const ICON_SOURCE: &str = "usr/share/icons/hicolor/256x256/apps/dev.jiradesk.JiraDesk.png";
const MAX_DESKTOP_BYTES: usize = 64 * 1024;
const MAX_ICON_BYTES: usize = 1024 * 1024;
const ENV_APPIMAGE: &str = "APPIMAGE";
const ENV_APPDIR: &str = "APPDIR";
const ENV_XDG_DATA_HOME: &str = "XDG_DATA_HOME";
const ENV_HOME: &str = "HOME";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegistrationError;

pub(crate) fn register_from_environment() -> Result<bool, RegistrationError> {
    let appimage = env::var_os(ENV_APPIMAGE).map(PathBuf::from);
    let appdir = env::var_os(ENV_APPDIR).map(PathBuf::from);
    let xdg_data_home = env::var_os(ENV_XDG_DATA_HOME).map(PathBuf::from);
    let home = env::var_os(ENV_HOME).map(PathBuf::from);
    let Ok(Some((data_home, appdir, appimage))) = resolve_environment(
        appimage.as_deref(),
        appdir.as_deref(),
        xdg_data_home.as_deref(),
        home.as_deref(),
    ) else {
        return Ok(false);
    };

    register(&data_home, &appdir, &appimage)?;
    Ok(true)
}

fn resolve_environment(
    appimage: Option<&Path>,
    appdir: Option<&Path>,
    xdg_data_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<Option<(PathBuf, PathBuf, PathBuf)>, RegistrationError> {
    let (Some(appimage), Some(appdir)) = (appimage, appdir) else {
        return Ok(None);
    };
    if !appimage.is_absolute() || !appdir.is_absolute() {
        return Ok(None);
    }
    let Some(data_home) = resolve_data_home(xdg_data_home, home) else {
        return Ok(None);
    };
    let appimage = fs::canonicalize(appimage).map_err(|_| RegistrationError)?;
    let appdir = fs::canonicalize(appdir).map_err(|_| RegistrationError)?;
    if !appimage.is_file() || !appdir.is_dir() {
        return Ok(None);
    }
    if appimage.to_str().is_none()
        || appimage
            .to_str()
            .is_some_and(|path| path.chars().any(char::is_control))
    {
        return Ok(None);
    }
    Ok(Some((data_home, appdir, appimage)))
}

fn resolve_data_home(xdg_data_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = xdg_data_home
        .filter(|path| !path.as_os_str().is_empty())
        .filter(|path| path.is_absolute())
    {
        return Some(path.to_path_buf());
    }
    let home = home.filter(|path| !path.as_os_str().is_empty())?;
    home.is_absolute()
        .then(|| home.join(".local").join("share"))
}

fn register(data_home: &Path, appdir: &Path, appimage: &Path) -> Result<(), RegistrationError> {
    let desktop_source = source_inside_appdir(appdir, DESKTOP_SOURCE)?;
    let icon_source = source_inside_appdir(appdir, ICON_SOURCE)?;
    let desktop_template = read_bounded(&desktop_source, MAX_DESKTOP_BYTES)?;
    let icon = read_bounded(&icon_source, MAX_ICON_BYTES)?;
    let desktop = rewrite_desktop_entry(&desktop_template, appimage)?;
    validate_png(&icon)?;

    let applications = data_home.join(APPLICATIONS_DIRECTORY);
    let icons = data_home.join(ICON_DIRECTORY);
    ensure_directory(&applications)?;
    ensure_directory(&icons)?;

    // Install the icon first so a newly installed desktop entry never points
    // at a missing icon. rename replaces a target symlink itself.
    atomic_write(&icons.join(ICON_FILENAME), &icon)?;
    atomic_write(&applications.join(DESKTOP_FILENAME), &desktop)?;
    Ok(())
}

fn source_inside_appdir(appdir: &Path, relative: &str) -> Result<PathBuf, RegistrationError> {
    let source = fs::canonicalize(appdir.join(relative)).map_err(|_| RegistrationError)?;
    if !source.starts_with(appdir) || !source.is_file() {
        return Err(RegistrationError);
    }
    Ok(source)
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, RegistrationError> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|_| RegistrationError)?
        .take((max_bytes + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| RegistrationError)?;
    if bytes.len() > max_bytes {
        return Err(RegistrationError);
    }
    Ok(bytes)
}

fn rewrite_desktop_entry(template: &[u8], appimage: &Path) -> Result<Vec<u8>, RegistrationError> {
    if template.len() > MAX_DESKTOP_BYTES {
        return Err(RegistrationError);
    }
    let template = std::str::from_utf8(template).map_err(|_| RegistrationError)?;
    let appimage = appimage.to_str().ok_or(RegistrationError)?;
    if appimage.is_empty() || appimage.chars().any(char::is_control) {
        return Err(RegistrationError);
    }
    let escaped = escape_exec_argument(appimage);
    let replacement = format!(r#"Exec="{escaped}""#);

    let mut output = String::with_capacity(template.len() + replacement.len());
    let mut replacements = 0;
    let mut names = 0;
    let mut icons = 0;
    let mut exec_lines = 0;
    let mut name_lines = 0;
    let mut icon_lines = 0;
    for line in template.split_inclusive('\n') {
        let (body, ending) = line
            .strip_suffix('\n')
            .map_or((line, ""), |line| (line, "\n"));
        if body == "Exec=jira-gpui" {
            output.push_str(&replacement);
            output.push_str(ending);
            replacements += 1;
        } else {
            output.push_str(line);
        }
        exec_lines += usize::from(body.starts_with("Exec="));
        name_lines += usize::from(body.starts_with("Name="));
        icon_lines += usize::from(body.starts_with("Icon="));
        names += usize::from(body == "Name=Jira Desk");
        icons += usize::from(body == "Icon=dev.jiradesk.JiraDesk");
    }
    if replacements != 1
        || exec_lines != 1
        || name_lines != 1
        || icon_lines != 1
        || names != 1
        || icons != 1
        || output.len() > MAX_DESKTOP_BYTES
    {
        return Err(RegistrationError);
    }
    Ok(output.into_bytes())
}

fn escape_exec_argument(path: &str) -> String {
    let backslash = 92_u8 as char;
    let mut escaped = String::with_capacity(path.len());
    for character in path.chars() {
        match character {
            '"' => {
                escaped.push(backslash);
                escaped.push('"');
            }
            '$' => {
                escaped.push(backslash);
                escaped.push('$');
            }
            '%' => escaped.push_str("%%"),
            character if character == backslash => {
                escaped.push(backslash);
                escaped.push(backslash);
            }
            character if character == 96_u8 as char => {
                escaped.push(backslash);
                escaped.push(character);
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn validate_png(icon: &[u8]) -> Result<(), RegistrationError> {
    if icon.len() > MAX_ICON_BYTES || icon.len() < 24 {
        return Err(RegistrationError);
    }
    if &icon[..8] != b"\x89PNG\r\n\x1a\n" || &icon[12..16] != b"IHDR" {
        return Err(RegistrationError);
    }
    let width = u32::from_be_bytes(icon[16..20].try_into().map_err(|_| RegistrationError)?);
    let height = u32::from_be_bytes(icon[20..24].try_into().map_err(|_| RegistrationError)?);
    (width == 256 && height == 256)
        .then_some(())
        .ok_or(RegistrationError)
}

fn ensure_directory(path: &Path) -> Result<(), RegistrationError> {
    let created = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(RegistrationError);
            }
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| RegistrationError)?;
            true
        }
        Err(_) => return Err(RegistrationError),
    };
    let metadata = fs::symlink_metadata(path).map_err(|_| RegistrationError)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(RegistrationError);
    }
    if created {
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .map_err(|_| RegistrationError)?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), RegistrationError> {
    let max_bytes = if path.extension().and_then(|extension| extension.to_str()) == Some("png") {
        MAX_ICON_BYTES
    } else {
        MAX_DESKTOP_BYTES
    };
    if bytes.len() > max_bytes {
        return Err(RegistrationError);
    }
    let parent = path.parent().ok_or(RegistrationError)?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(RegistrationError)?;
    let pid = std::process::id();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RegistrationError)?
        .as_nanos();

    for attempt in 0..8u8 {
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{filename}.{pid}-{timestamp}-{counter}-{attempt}.tmp"
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o644);
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(RegistrationError),
        };
        let result = (|| {
            file.write_all(bytes).map_err(|_| RegistrationError)?;
            file.sync_all().map_err(|_| RegistrationError)?;
            drop(file);
            fs::rename(&temporary, path).map_err(|_| RegistrationError)?;
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| RegistrationError)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result;
    }
    Err(RegistrationError)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, time::SystemTime};

    const DESKTOP: &[u8] = b"[Desktop Entry]\nType=Application\nName=Jira Desk\nComment=Focused Jira workspace\nExec=jira-gpui\nIcon=dev.jiradesk.JiraDesk\nTerminal=false\n";

    fn temporary_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "jira-desk-desktop-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn png_256() -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        png.extend_from_slice(&256_u32.to_be_bytes());
        png.extend_from_slice(&256_u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        png
    }

    fn fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let appdir = root.join("appdir");
        fs::create_dir_all(appdir.join("usr/share/applications")).expect("desktop dir");
        fs::create_dir_all(appdir.join("usr/share/icons/hicolor/256x256/apps")).expect("icon dir");
        let appimage = root.join("Jira Desk.AppImage");
        fs::write(&appimage, b"appimage").expect("appimage");
        fs::write(appdir.join(DESKTOP_SOURCE), DESKTOP).expect("desktop");
        fs::write(appdir.join(ICON_SOURCE), png_256()).expect("icon");
        (appdir, appimage, root.join("data"))
    }

    #[test]
    fn skips_without_a_real_appimage_environment_or_with_relative_values() {
        let root = temporary_root("skip");
        assert_eq!(
            resolve_environment(
                None,
                None,
                Some(Path::new("/xdg")),
                Some(Path::new("/home"))
            )
            .expect("resolve"),
            None
        );
        assert_eq!(
            resolve_environment(
                Some(Path::new("appimage")),
                Some(Path::new("/appdir")),
                Some(Path::new("/xdg")),
                Some(Path::new("/home"))
            )
            .expect("resolve"),
            None
        );
        assert!(!root.exists());
    }

    #[test]
    fn resolves_data_home_and_home_fallback_only_when_absolute() {
        assert_eq!(
            resolve_data_home(Some(Path::new("/xdg")), Some(Path::new("/home"))),
            Some(PathBuf::from("/xdg"))
        );
        assert_eq!(
            resolve_data_home(None, Some(Path::new("/home"))),
            Some(PathBuf::from("/home/.local/share"))
        );
        assert_eq!(
            resolve_data_home(Some(Path::new("relative")), Some(Path::new("/home"))),
            Some(PathBuf::from("/home/.local/share"))
        );
        assert_eq!(resolve_data_home(None, Some(Path::new("relative"))), None);
    }

    #[test]
    fn rewrites_exact_exec_line_and_escapes_special_characters() {
        let appimage = format!(
            "/tmp/Jira Desk \"release\\100%$file{}tick.AppImage",
            96_u8 as char
        );
        let result = rewrite_desktop_entry(DESKTOP, Path::new(&appimage)).expect("desktop");
        assert!(
            std::str::from_utf8(&result)
                .expect("utf8")
                .contains("Name=Jira Desk")
        );
        assert!(
            std::str::from_utf8(&result)
                .expect("utf8")
                .contains("Icon=dev.jiradesk.JiraDesk")
        );
        assert!(
            std::str::from_utf8(&result)
                .expect("utf8")
                .contains(&format!(
                    r#"Exec="/tmp/Jira Desk \"release\\100%%\$file\{}tick.AppImage""#,
                    96_u8 as char
                ))
        );
        assert!(rewrite_desktop_entry(DESKTOP, Path::new("/tmp/new\nline")).is_err());
    }

    #[test]
    fn rejects_ambiguous_unlocalized_desktop_fields() {
        let template = [
            DESKTOP,
            b"Name=Attacker\n",
            b"Icon=attacker\n",
            b"Exec=attacker\n",
        ]
        .concat();
        assert!(rewrite_desktop_entry(&template, Path::new("/tmp/jira.AppImage")).is_err());
    }

    #[test]
    fn invalid_source_or_template_writes_nothing() {
        let root = temporary_root("invalid");
        let (appdir, appimage, data) = fixture(&root);
        fs::write(appdir.join(DESKTOP_SOURCE), b"Name=invalid\n").expect("invalid desktop");
        assert!(register(&data, &appdir, &appimage).is_err());
        assert!(!data.exists());
        fs::write(appdir.join(DESKTOP_SOURCE), DESKTOP).expect("desktop");
        fs::write(appdir.join(ICON_SOURCE), b"not png").expect("invalid icon");
        assert!(register(&data, &appdir, &appimage).is_err());
        assert!(!data.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn installs_exact_targets_and_updates_repeatedly() {
        let root = temporary_root("install");
        let (appdir, appimage, data) = fixture(&root);
        register(&data, &appdir, &appimage).expect("first install");
        let desktop_path = data.join(APPLICATIONS_DIRECTORY).join(DESKTOP_FILENAME);
        let icon_path = data.join(ICON_DIRECTORY).join(ICON_FILENAME);
        assert_eq!(fs::read(&icon_path).expect("icon"), png_256());
        let first_desktop = fs::read_to_string(&desktop_path).expect("desktop");
        assert!(first_desktop.contains("Name=Jira Desk"));
        assert!(first_desktop.contains("Icon=dev.jiradesk.JiraDesk"));
        assert!(first_desktop.contains(r#"Exec="/tmp/"#));
        register(&data, &appdir, &appimage).expect("repeat update");
        assert_eq!(
            fs::read_to_string(&desktop_path).expect("desktop"),
            first_desktop
        );
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn replaces_target_symlink_without_following_it() {
        let root = temporary_root("symlink");
        let (appdir, appimage, data) = fixture(&root);
        register(&data, &appdir, &appimage).expect("first install");
        let desktop_path = data.join(APPLICATIONS_DIRECTORY).join(DESKTOP_FILENAME);
        let old = root.join("old-desktop");
        fs::write(&old, b"must not be changed").expect("old");
        fs::remove_file(&desktop_path).expect("remove");
        std::os::unix::fs::symlink(&old, &desktop_path).expect("symlink");
        register(&data, &appdir, &appimage).expect("replace symlink");
        assert!(
            !fs::symlink_metadata(&desktop_path)
                .expect("target")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&old).expect("old target"), b"must not be changed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn embedded_fixture_png_is_bounded_and_256_pixels() {
        let png = png_256();
        validate_png(&png).expect("png");
        assert!(png.len() < MAX_ICON_BYTES);
    }
}
