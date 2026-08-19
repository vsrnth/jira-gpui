//! Secure Linux-local persistence bootstrap for the Phase 1 shell.

use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use jira_application::{DEFAULT_JQL_SCOPE, validate_jql_scope};
use jira_domain::AccountId;
use jira_storage::SqliteStore;

const APP_DIRECTORY: &str = "jira-desk";
const DATABASE_FILENAME: &str = "jira-desk.sqlite3";
const PREFERENCES_FILENAME: &str = "preferences.json";
const MAX_PREFERENCES_BYTES: usize = 64 * 1024;
/// Keep the offline team cache small enough that startup and generated JQL remain bounded.
pub(crate) const MAX_TEAM_MEMBERS: usize = 100;
const MAX_TEAM_MEMBER_IDENTIFIER_BYTES: usize = 320;
const MAX_TEAM_MEMBER_DISPLAY_NAME_BYTES: usize = 255;
const ENV_XDG_DATA_HOME: &str = "XDG_DATA_HOME";
const ENV_XDG_STATE_HOME: &str = "XDG_STATE_HOME";
const ENV_HOME: &str = "HOME";
static PREFERENCES_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalDataError;

/// User-controlled local settings. Credentials and other connection state do
/// not belong in this file; those remain session-only and are never persisted.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct LocalPreferences {
    #[serde(default)]
    pub(crate) issue_jql_scope: Option<String>,
    /// Resolved team identities are local metadata, not credentials. Omitting an empty value
    /// keeps the on-disk representation compatible with preferences written before team support.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) team_members: Vec<PersistedTeamMember>,
}

/// A locally persisted team identity. `identifier` is what the user entered (an account ID or
/// email); `account_id` is the stable Jira identity resolved from it; `display_name` is the safe
/// label used to populate the offline identity cache without another startup lookup.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct PersistedTeamMember {
    pub(crate) identifier: String,
    pub(crate) account_id: String,
    pub(crate) display_name: String,
}

/// Normalize and validate team identities before using or persisting them.
///
/// The input order is retained and the first record for a stable account ID wins. This makes
/// duplicate handling deterministic while preserving the order chosen in the team settings UI.
/// At most [`MAX_TEAM_MEMBERS`] records may be supplied, before deduplication, so malformed or
/// duplicate-heavy input cannot be used to bypass the bound.
pub(crate) fn normalize_team_members(
    members: Vec<PersistedTeamMember>,
) -> Result<Vec<PersistedTeamMember>, LocalDataError> {
    if members.len() > MAX_TEAM_MEMBERS {
        return Err(LocalDataError);
    }

    let mut normalized = Vec::with_capacity(members.len());
    for member in members {
        let identifier =
            normalize_team_member_text(member.identifier, MAX_TEAM_MEMBER_IDENTIFIER_BYTES)?;
        let account_id = normalize_team_member_account_id(member.account_id)?;
        let display_name =
            normalize_team_member_text(member.display_name, MAX_TEAM_MEMBER_DISPLAY_NAME_BYTES)?;
        if display_name == account_id {
            return Err(LocalDataError);
        }

        if normalized
            .iter()
            .any(|existing: &PersistedTeamMember| existing.account_id == account_id)
        {
            continue;
        }
        normalized.push(PersistedTeamMember {
            identifier,
            account_id,
            display_name,
        });
    }
    Ok(normalized)
}

fn normalize_team_member_text(
    value: String,
    maximum_bytes: usize,
) -> Result<String, LocalDataError> {
    if value.chars().any(char::is_control) {
        return Err(LocalDataError);
    }
    let value = value.trim();
    if value.is_empty() || value.len() > maximum_bytes {
        return Err(LocalDataError);
    }
    Ok(value.to_owned())
}

fn normalize_team_member_account_id(value: String) -> Result<String, LocalDataError> {
    if value.chars().any(char::is_control) {
        return Err(LocalDataError);
    }
    let value = value.trim();
    let account_id = AccountId::new(value.to_owned()).map_err(|_| LocalDataError)?;
    // Account IDs are interpolated as quoted JQL literals by the Jira adapter. Keep this
    // persistence boundary aligned with that adapter instead of storing a value that could alter
    // a generated query's meaning.
    if account_id
        .as_str()
        .chars()
        .any(|character| character.is_control() || matches!(character, '"' | '\\'))
    {
        return Err(LocalDataError);
    }
    Ok(account_id.into_inner())
}

/// Normalize the persisted scope into the representation used by the session and user-set key.
/// The exact default is stored as `None`, while custom scopes retain trimmed text.
pub(crate) fn normalize_issue_jql_scope(
    scope: Option<String>,
) -> Result<Option<String>, LocalDataError> {
    let Some(scope) = scope else {
        return Ok(None);
    };
    validate_jql_scope(Some(&scope)).map_err(|_| LocalDataError)?;
    let scope = scope.trim().to_owned();
    if scope == DEFAULT_JQL_SCOPE {
        Ok(None)
    } else {
        Ok(Some(scope))
    }
}

pub(crate) fn open_store() -> Result<Arc<SqliteStore>, LocalDataError> {
    let xdg_data_home = env::var_os(ENV_XDG_DATA_HOME).map(PathBuf::from);
    let home = env::var_os(ENV_HOME).map(PathBuf::from);
    let app_directory = prepare_app_directory(xdg_data_home.as_deref(), home.as_deref())?;
    SqliteStore::open(app_directory.join(DATABASE_FILENAME))
        .map(Arc::new)
        .map_err(|_| LocalDataError)
}

/// Load local preferences from the private Jira Desk data directory.
pub(crate) fn load_preferences() -> Result<LocalPreferences, LocalDataError> {
    let xdg_data_home = env::var_os(ENV_XDG_DATA_HOME).map(PathBuf::from);
    let home = env::var_os(ENV_HOME).map(PathBuf::from);
    let app_directory = prepare_app_directory(xdg_data_home.as_deref(), home.as_deref())?;
    load_preferences_from_directory(&app_directory)
}

/// Save local preferences atomically in the private Jira Desk data directory.
pub(crate) fn save_preferences(preferences: &LocalPreferences) -> Result<(), LocalDataError> {
    let xdg_data_home = env::var_os(ENV_XDG_DATA_HOME).map(PathBuf::from);
    let home = env::var_os(ENV_HOME).map(PathBuf::from);
    let app_directory = prepare_app_directory(xdg_data_home.as_deref(), home.as_deref())?;
    save_preferences_in_directory(&app_directory, preferences)
}

fn preferences_path(directory: &Path) -> PathBuf {
    directory.join(PREFERENCES_FILENAME)
}

fn load_preferences_from_directory(directory: &Path) -> Result<LocalPreferences, LocalDataError> {
    let path = preferences_path(directory);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LocalPreferences::default());
        }
        Err(_) => return Err(LocalDataError),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(LocalDataError);
    }
    if metadata.len() > MAX_PREFERENCES_BYTES as u64 {
        return Err(LocalDataError);
    }

    let file = fs::File::open(&path).map_err(|_| LocalDataError)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_PREFERENCES_BYTES as u64) + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalDataError)?;
    if bytes.len() > MAX_PREFERENCES_BYTES {
        return Err(LocalDataError);
    }
    serde_json::from_slice(&bytes).map_err(|_| LocalDataError)
}

fn save_preferences_in_directory(
    directory: &Path,
    preferences: &LocalPreferences,
) -> Result<(), LocalDataError> {
    let bytes = serde_json::to_vec(preferences).map_err(|_| LocalDataError)?;
    if bytes.len() > MAX_PREFERENCES_BYTES {
        return Err(LocalDataError);
    }

    let path = preferences_path(directory);
    reject_non_regular_destination(&path)?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| LocalDataError)?
        .as_nanos();
    let process_id = std::process::id();
    let mut temporary_path = None;
    let mut temporary_file = None;
    for attempt in 0..8u8 {
        let counter = PREFERENCES_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let candidate = directory.join(format!(
            ".{PREFERENCES_FILENAME}.{process_id}-{timestamp}-{counter}-{attempt}.tmp"
        ));
        match restricted_create_options().open(&candidate) {
            Ok(file) => {
                temporary_path = Some(candidate);
                temporary_file = Some(file);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(LocalDataError),
        }
    }
    let (temporary_path, mut temporary_file) = match (temporary_path, temporary_file) {
        (Some(path), Some(file)) => (path, file),
        (path, _) => {
            if let Some(path) = path {
                let _ = fs::remove_file(path);
            }
            return Err(LocalDataError);
        }
    };
    let result = (|| {
        temporary_file
            .write_all(&bytes)
            .map_err(|_| LocalDataError)?;
        temporary_file.flush().map_err(|_| LocalDataError)?;
        temporary_file.sync_all().map_err(|_| LocalDataError)?;
        drop(temporary_file);

        // Re-check immediately before replacement. On Unix rename replaces the
        // directory entry itself, so a symlink can never be followed here.
        reject_non_regular_destination(&path)?;
        fs::rename(&temporary_path, &path).map_err(|_| LocalDataError)?;
        sync_directory(directory)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn reject_non_regular_destination(path: &Path) -> Result<(), LocalDataError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(LocalDataError)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(LocalDataError),
    }
}

fn sync_directory(directory: &Path) -> Result<(), LocalDataError> {
    fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| LocalDataError)
}

fn restricted_create_options() -> fs::OpenOptions {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
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
    prepare_restricted_directory(&app_directory)
}

/// Resolve the private state directory used for bounded diagnostics.
///
/// The XDG state root takes precedence when it is present. An empty variable
/// is treated as unset, matching the data-directory resolver above. Both roots
/// must be absolute so a malformed process environment cannot redirect local
/// state into the process working directory.
pub(crate) fn resolve_state_directory(
    xdg_state_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, LocalDataError> {
    if let Some(xdg_state_home) = xdg_state_home.filter(|path| !path.as_os_str().is_empty()) {
        if !xdg_state_home.is_absolute() {
            return Err(LocalDataError);
        }
        return Ok(xdg_state_home.join(APP_DIRECTORY));
    }

    let home = home
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(LocalDataError)?;
    if !home.is_absolute() {
        return Err(LocalDataError);
    }
    Ok(home.join(".local").join("state").join(APP_DIRECTORY))
}

pub(crate) fn prepare_state_directory(
    xdg_state_home: Option<&Path>,
    home: Option<&Path>,
) -> Result<PathBuf, LocalDataError> {
    let state_directory = resolve_state_directory(xdg_state_home, home)?;
    prepare_restricted_directory(&state_directory)
}

pub(crate) fn prepare_diagnostics_directory_from_environment() -> Result<PathBuf, LocalDataError> {
    let xdg_state_home = env::var_os(ENV_XDG_STATE_HOME).map(PathBuf::from);
    let home = env::var_os(ENV_HOME).map(PathBuf::from);
    prepare_state_directory(xdg_state_home.as_deref(), home.as_deref())
}

fn prepare_restricted_directory(path: &Path) -> Result<PathBuf, LocalDataError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(LocalDataError);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| LocalDataError)?;
        }
        Err(_) => return Err(LocalDataError),
    }

    let metadata = fs::symlink_metadata(path).map_err(|_| LocalDataError)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LocalDataError);
    }
    restrict_directory(path)?;
    Ok(path.to_path_buf())
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

    #[test]
    fn xdg_state_home_takes_precedence_over_home() {
        let state =
            resolve_state_directory(Some(Path::new("/xdg/state")), Some(Path::new("/home")))
                .expect("path");
        assert_eq!(state, PathBuf::from("/xdg/state/jira-desk"));
    }

    #[test]
    fn home_fallback_uses_local_state() {
        let state =
            resolve_state_directory(None, Some(Path::new("/home/developer"))).expect("path");
        assert_eq!(
            state,
            PathBuf::from("/home/developer/.local/state/jira-desk")
        );
    }

    #[test]
    fn state_roots_reject_missing_empty_and_relative_values() {
        assert!(resolve_state_directory(None, None).is_err());
        assert!(resolve_state_directory(Some(Path::new("")), Some(Path::new(""))).is_err());
        assert!(resolve_state_directory(Some(Path::new("relative-xdg")), None).is_err());
        assert!(resolve_state_directory(None, Some(Path::new("relative-home"))).is_err());
    }

    #[test]
    fn final_state_directory_symlink_is_rejected() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = temporary_root("state-symlink");
            let target = root.join("target");
            let state = root.join(APP_DIRECTORY);
            fs::create_dir_all(&target).expect("target");
            symlink(&target, &state).expect("symlink");
            assert!(prepare_state_directory(Some(&root), None).is_err());
            fs::remove_file(state).expect("remove symlink");
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn missing_preferences_use_safe_defaults() {
        let root = temporary_root("preferences-default");
        let app = prepare_app_directory(Some(&root), None).expect("app directory");
        assert_eq!(
            load_preferences_from_directory(&app).expect("preferences"),
            LocalPreferences::default()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn legacy_preferences_without_team_members_load_unchanged() {
        let preferences: LocalPreferences =
            serde_json::from_str(r#"{"issue_jql_scope":"project = APP"}"#)
                .expect("legacy preferences");
        assert_eq!(
            preferences,
            LocalPreferences {
                issue_jql_scope: Some("project = APP".to_owned()),
                team_members: Vec::new(),
            }
        );
    }

    #[test]
    fn team_members_normalize_trim_and_deduplicate_by_account_id() {
        let normalized = normalize_team_members(vec![
            PersistedTeamMember {
                identifier: "  first@example.com  ".to_owned(),
                account_id: "  account-1  ".to_owned(),
                display_name: "  First Person  ".to_owned(),
            },
            PersistedTeamMember {
                identifier: "second@example.com".to_owned(),
                account_id: "account-1".to_owned(),
                display_name: "Second Person".to_owned(),
            },
            PersistedTeamMember {
                identifier: "third@example.com".to_owned(),
                account_id: "account-2".to_owned(),
                display_name: "Third Person".to_owned(),
            },
        ])
        .expect("normalized team");

        assert_eq!(
            normalized,
            vec![
                PersistedTeamMember {
                    identifier: "first@example.com".to_owned(),
                    account_id: "account-1".to_owned(),
                    display_name: "First Person".to_owned(),
                },
                PersistedTeamMember {
                    identifier: "third@example.com".to_owned(),
                    account_id: "account-2".to_owned(),
                    display_name: "Third Person".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn team_members_reject_empty_oversized_and_control_values() {
        let cases = [
            PersistedTeamMember {
                identifier: "   ".to_owned(),
                account_id: "account-1".to_owned(),
                display_name: "Person".to_owned(),
            },
            PersistedTeamMember {
                identifier: "person@example.com".to_owned(),
                account_id: "   ".to_owned(),
                display_name: "Person".to_owned(),
            },
            PersistedTeamMember {
                identifier: "person@example.com".to_owned(),
                account_id: "account-1\n".to_owned(),
                display_name: "Person".to_owned(),
            },
            PersistedTeamMember {
                identifier: "person@example.com".to_owned(),
                account_id: "account-1\"unsafe".to_owned(),
                display_name: "Person".to_owned(),
            },
            PersistedTeamMember {
                identifier: "person\u{0007}@example.com".to_owned(),
                account_id: "account-1".to_owned(),
                display_name: "Person".to_owned(),
            },
            PersistedTeamMember {
                identifier: "person@example.com".to_owned(),
                account_id: "account-1".to_owned(),
                display_name: "Person\u{0007}".to_owned(),
            },
            PersistedTeamMember {
                identifier: "person@example.com".to_owned(),
                account_id: "account-1".to_owned(),
                display_name: "account-1".to_owned(),
            },
            PersistedTeamMember {
                identifier: "x".repeat(MAX_TEAM_MEMBER_IDENTIFIER_BYTES + 1),
                account_id: "account-1".to_owned(),
                display_name: "Person".to_owned(),
            },
            PersistedTeamMember {
                identifier: "person@example.com".to_owned(),
                account_id: "account-1".to_owned(),
                display_name: "x".repeat(MAX_TEAM_MEMBER_DISPLAY_NAME_BYTES + 1),
            },
        ];

        for member in cases {
            assert!(normalize_team_members(vec![member]).is_err());
        }
    }

    #[test]
    fn team_members_enforce_conservative_size_limit() {
        let members = (0..=MAX_TEAM_MEMBERS)
            .map(|index| PersistedTeamMember {
                identifier: format!("person-{index}@example.com"),
                account_id: format!("account-{index}"),
                display_name: format!("Person {index}"),
            })
            .collect();
        assert!(normalize_team_members(members).is_err());

        let members = (0..MAX_TEAM_MEMBERS)
            .map(|index| PersistedTeamMember {
                identifier: format!("person-{index}@example.com"),
                account_id: format!("account-{index}"),
                display_name: format!("Person {index}"),
            })
            .collect();
        assert_eq!(
            normalize_team_members(members)
                .expect("limit boundary")
                .len(),
            MAX_TEAM_MEMBERS
        );
    }

    #[test]
    fn preference_scope_normalization_stores_default_as_none() {
        assert_eq!(normalize_issue_jql_scope(None).unwrap(), None);
        assert_eq!(
            normalize_issue_jql_scope(Some(format!("  {DEFAULT_JQL_SCOPE}  "))).unwrap(),
            None
        );
        assert_eq!(
            normalize_issue_jql_scope(Some(" project = APP ".to_owned())).unwrap(),
            Some("project = APP".to_owned())
        );
        assert!(
            normalize_issue_jql_scope(Some("project = APP ORDER BY updated".to_owned())).is_err()
        );
    }

    #[test]
    fn preferences_round_trip_through_json() {
        let root = temporary_root("preferences-round-trip");
        let app = prepare_app_directory(Some(&root), None).expect("app directory");
        let expected = LocalPreferences {
            issue_jql_scope: Some("project = DEMO ORDER BY updated DESC".to_owned()),
            team_members: vec![PersistedTeamMember {
                identifier: "ada@example.com".to_owned(),
                account_id: "account-ada".to_owned(),
                display_name: "Ada Lovelace".to_owned(),
            }],
        };
        save_preferences_in_directory(&app, &expected).expect("save preferences");
        assert_eq!(
            load_preferences_from_directory(&app).expect("load preferences"),
            expected
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn malformed_preferences_are_rejected() {
        let root = temporary_root("preferences-malformed");
        let app = prepare_app_directory(Some(&root), None).expect("app directory");
        fs::write(preferences_path(&app), b"{not-json").expect("write malformed preferences");
        assert!(load_preferences_from_directory(&app).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn oversized_preferences_are_rejected() {
        let root = temporary_root("preferences-oversized");
        let app = prepare_app_directory(Some(&root), None).expect("app directory");
        fs::write(
            preferences_path(&app),
            vec![b'x'; MAX_PREFERENCES_BYTES + 1],
        )
        .expect("write oversized preferences");
        assert!(load_preferences_from_directory(&app).is_err());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn preference_symlinks_are_rejected_without_replacing_target() {
        use std::os::unix::fs::symlink;

        let root = temporary_root("preferences-symlink");
        let app = prepare_app_directory(Some(&root), None).expect("app directory");
        let target = root.join("target.json");
        let expected = LocalPreferences {
            issue_jql_scope: Some("project = TARGET".to_owned()),
            team_members: Vec::new(),
        };
        let target_bytes = serde_json::to_vec(&expected).expect("serialize target");
        fs::write(&target, &target_bytes).expect("write target");
        symlink(&target, preferences_path(&app)).expect("symlink");

        assert!(load_preferences_from_directory(&app).is_err());
        assert!(save_preferences_in_directory(&app, &LocalPreferences::default()).is_err());
        assert_eq!(fs::read(&target).expect("read target"), target_bytes);
        fs::remove_file(preferences_path(&app)).expect("remove symlink");
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn saved_preferences_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = temporary_root("preferences-permissions");
        let app = prepare_app_directory(Some(&root), None).expect("app directory");
        save_preferences_in_directory(&app, &LocalPreferences::default()).expect("save");
        assert_eq!(
            fs::metadata(preferences_path(&app))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
