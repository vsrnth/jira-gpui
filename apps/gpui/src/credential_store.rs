//! Secure system credential storage for the Jira Cloud login.
//!
//! The keyring entry has one deliberately boring identity.  This keeps an
//! application upgrade from orphaning credentials because its path or artifact
//! version changed.  The value stored in the entry is a versioned, bounded
//! JSON document; it is never written to a local file or a plaintext cache.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread;

use futures_channel::oneshot;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const SERVICE: &str = "dev.jiradesk.JiraDesk";
const USERNAME: &str = "jira-cloud-default-v1";
const SUPPORTED_SCHEMA_VERSION: u8 = 1;
const MAX_SECRET_BYTES: usize = 16 * 1024;
const MAX_URL_BYTES: usize = 2 * 1024;
const MAX_EMAIL_BYTES: usize = 320;
const MAX_TOKEN_BYTES: usize = 4 * 1024;

/// Credentials accepted by the Jira Cloud client.
///
/// The fields stay private so callers cannot accidentally format or serialize
/// the token.  Use [`SavedCredentials::into_parts`] to consume the value when
/// handing it to the client.
#[derive(PartialEq, Eq)]
pub struct SavedCredentials {
    url: String,
    email: String,
    api_token: String,
}

impl SavedCredentials {
    /// Construct credentials after applying the same bounds used on load.
    pub fn new(
        mut url: String,
        mut email: String,
        mut api_token: String,
    ) -> Result<Self, CredentialStoreError> {
        if let Err(error) = validate_field(&url, MAX_URL_BYTES, Field::Url)
            .and_then(|()| validate_field(&email, MAX_EMAIL_BYTES, Field::Email))
            .and_then(|()| validate_field(&api_token, MAX_TOKEN_BYTES, Field::Token))
        {
            url.zeroize();
            email.zeroize();
            api_token.zeroize();
            return Err(error);
        }
        Ok(Self {
            url,
            email,
            api_token,
        })
    }

    /// Consume the credentials for one-way handoff to the Jira client.
    pub fn into_parts(mut self) -> (String, String, String) {
        (
            std::mem::take(&mut self.url),
            std::mem::take(&mut self.email),
            std::mem::take(&mut self.api_token),
        )
    }
}

impl Drop for SavedCredentials {
    fn drop(&mut self) {
        self.url.zeroize();
        self.email.zeroize();
        self.api_token.zeroize();
    }
}

impl fmt::Debug for SavedCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SavedCredentials { redacted: true }")
    }
}

/// Redacted failures returned by the vault boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialStoreError {
    /// The secure store could not be reached, initialized, or bridged back.
    Unavailable,
    /// Caller-provided fields are empty or contain disallowed control bytes.
    Invalid,
    /// The stored value is not the supported JSON schema.
    Malformed,
    /// The value or one of its fields exceeds the local bound.
    Oversized,
    /// The value has a schema version this binary does not understand.
    UnsupportedVersion,
}

impl fmt::Display for CredentialStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "secure credential storage is unavailable",
            Self::Invalid => "credentials are invalid",
            Self::Malformed => "saved credentials are malformed",
            Self::Oversized => "saved credentials are too large",
            Self::UnsupportedVersion => "saved credentials use an unsupported version",
        })
    }
}

impl std::error::Error for CredentialStoreError {}

/// Result of a successful save.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveOutcome {
    Saved,
}

/// Result of a delete.  `Absent` makes delete safe to retry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteOutcome {
    Deleted,
    Absent,
}

/// Load the saved Jira Cloud credentials, if an entry exists.
pub async fn load_saved_credentials() -> Result<Option<SavedCredentials>, CredentialStoreError> {
    run_on_os_thread(load_from_keyring).await
}

/// Save the credentials into the secure system credential store.
pub async fn save_credentials(
    credentials: SavedCredentials,
) -> Result<SaveOutcome, CredentialStoreError> {
    let payload = serialize_credentials(&credentials)?;
    run_on_os_thread(move || save_to_keyring(payload)).await
}

/// Delete saved credentials.  Deleting an absent entry succeeds.
pub async fn delete_saved_credentials() -> Result<DeleteOutcome, CredentialStoreError> {
    run_on_os_thread(delete_from_keyring).await
}

#[derive(Serialize)]
struct CredentialPayload<'a> {
    schema_version: u8,
    url: &'a str,
    email: &'a str,
    token: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCredentialPayload {
    schema_version: u8,
    url: String,
    email: String,
    token: String,
}

impl StoredCredentialPayload {
    fn into_credentials(mut self) -> Result<SavedCredentials, CredentialStoreError> {
        SavedCredentials::new(
            std::mem::take(&mut self.url),
            std::mem::take(&mut self.email),
            std::mem::take(&mut self.token),
        )
    }
}

impl Drop for StoredCredentialPayload {
    fn drop(&mut self) {
        self.url.zeroize();
        self.email.zeroize();
        self.token.zeroize();
    }
}

struct SecretBytes(Vec<u8>);

impl SecretBytes {
    fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

fn serialize_credentials(
    credentials: &SavedCredentials,
) -> Result<SecretBytes, CredentialStoreError> {
    let payload = CredentialPayload {
        schema_version: SUPPORTED_SCHEMA_VERSION,
        url: &credentials.url,
        email: &credentials.email,
        token: &credentials.api_token,
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| CredentialStoreError::Malformed)?;
    let secret = SecretBytes::new(bytes);
    if secret.as_slice().len() > MAX_SECRET_BYTES {
        return Err(CredentialStoreError::Oversized);
    }
    Ok(secret)
}

fn deserialize_credentials(secret: SecretBytes) -> Result<SavedCredentials, CredentialStoreError> {
    if secret.as_slice().len() > MAX_SECRET_BYTES {
        return Err(CredentialStoreError::Oversized);
    }
    let payload: StoredCredentialPayload =
        serde_json::from_slice(secret.as_slice()).map_err(|_| CredentialStoreError::Malformed)?;
    if payload.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(CredentialStoreError::UnsupportedVersion);
    }
    payload.into_credentials()
}

#[derive(Clone, Copy)]
enum Field {
    Url,
    Email,
    Token,
}

fn validate_field(
    value: &str,
    max_bytes: usize,
    _field: Field,
) -> Result<(), CredentialStoreError> {
    if value.is_empty() {
        return Err(CredentialStoreError::Invalid);
    }
    if value.len() > max_bytes {
        return Err(CredentialStoreError::Oversized);
    }
    if value.chars().any(char::is_control) {
        return Err(CredentialStoreError::Invalid);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendError {
    NoEntry,
    Unavailable,
    Rejected(CredentialStoreError),
}

type BackendResult<T> = Result<T, BackendError>;

fn map_keyring_error(error: keyring::Error) -> BackendError {
    match error {
        keyring::Error::NoEntry => BackendError::NoEntry,
        _ => BackendError::Unavailable,
    }
}

fn save_to_keyring(secret: SecretBytes) -> BackendResult<SaveOutcome> {
    let entry = keyring::Entry::new(SERVICE, USERNAME).map_err(map_keyring_error)?;
    entry
        .set_secret(secret.as_slice())
        .map_err(map_keyring_error)?;
    Ok(SaveOutcome::Saved)
}

fn load_from_keyring() -> BackendResult<Option<SavedCredentials>> {
    let entry = keyring::Entry::new(SERVICE, USERNAME).map_err(map_keyring_error)?;
    let secret = match entry.get_secret() {
        Ok(bytes) => SecretBytes::new(bytes),
        Err(keyring::Error::NoEntry) => return Ok(None),
        Err(_) => return Err(BackendError::Unavailable),
    };
    deserialize_credentials(secret)
        .map(Some)
        .map_err(BackendError::Rejected)
}

fn delete_from_keyring() -> BackendResult<DeleteOutcome> {
    let entry = keyring::Entry::new(SERVICE, USERNAME).map_err(map_keyring_error)?;
    map_delete_backend_result(entry.delete_credential().map_err(map_keyring_error))
}

fn map_delete_backend_result(result: BackendResult<()>) -> BackendResult<DeleteOutcome> {
    match result {
        Ok(()) => Ok(DeleteOutcome::Deleted),
        Err(BackendError::NoEntry) => Ok(DeleteOutcome::Absent),
        Err(error) => Err(error),
    }
}

fn map_backend_result<T>(result: BackendResult<T>) -> Result<T, CredentialStoreError> {
    result.map_err(|error| match error {
        BackendError::Rejected(error) => error,
        BackendError::NoEntry | BackendError::Unavailable => CredentialStoreError::Unavailable,
    })
}

async fn run_on_os_thread<T, F>(operation: F) -> Result<T, CredentialStoreError>
where
    T: Send + 'static,
    F: FnOnce() -> BackendResult<T> + Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    let spawned = thread::Builder::new()
        .name("jira-credential-vault".to_owned())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(operation))
                .map_err(|_| BackendError::Unavailable)
                .and_then(|result| result);
            let _ = sender.send(result);
        });
    if spawned.is_err() {
        return Err(CredentialStoreError::Unavailable);
    }
    match receiver.await {
        Ok(result) => map_backend_result(result),
        Err(_) => Err(CredentialStoreError::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> SavedCredentials {
        SavedCredentials::new(
            "https://example.atlassian.net".to_owned(),
            "person@example.com".to_owned(),
            "token-value".to_owned(),
        )
        .expect("valid credentials")
    }

    #[test]
    fn identity_is_stable_and_contains_no_profile_values() {
        assert_eq!(SERVICE, "dev.jiradesk.JiraDesk");
        assert_eq!(USERNAME, "jira-cloud-default-v1");
        assert!(!SERVICE.contains("http"));
        assert!(!USERNAME.contains("@"));
    }

    #[test]
    fn json_round_trip_preserves_consumed_values() {
        let original = credentials();
        let secret = serialize_credentials(&original).expect("serialize");
        let restored = deserialize_credentials(secret).expect("deserialize");
        assert_eq!(restored.into_parts(), credentials().into_parts());
    }

    #[test]
    fn exact_schema_version_is_required() {
        let secret = SecretBytes::new(
            br#"{"schema_version":2,"url":"https://example.atlassian.net","email":"a@b.test","token":"x"}"#.to_vec(),
        );
        assert_eq!(
            deserialize_credentials(secret),
            Err(CredentialStoreError::UnsupportedVersion)
        );
    }

    #[test]
    fn malformed_unknown_and_oversized_values_are_rejected() {
        assert_eq!(
            deserialize_credentials(SecretBytes::new(b"not-json".to_vec())),
            Err(CredentialStoreError::Malformed)
        );
        let unknown = SecretBytes::new(
            br#"{"schema_version":1,"url":"u","email":"e","token":"t","extra":true}"#.to_vec(),
        );
        assert_eq!(
            deserialize_credentials(unknown),
            Err(CredentialStoreError::Malformed)
        );
        assert_eq!(
            SavedCredentials::new(
                "u".to_owned(),
                "e".to_owned(),
                "x".repeat(MAX_TOKEN_BYTES + 1)
            ),
            Err(CredentialStoreError::Oversized)
        );
        assert_eq!(
            SavedCredentials::new(
                "u".repeat(MAX_URL_BYTES + 1),
                "e".to_owned(),
                "t".to_owned()
            ),
            Err(CredentialStoreError::Oversized)
        );
        assert_eq!(
            SavedCredentials::new(
                "u".to_owned(),
                "e".repeat(MAX_EMAIL_BYTES + 1),
                "t".to_owned()
            ),
            Err(CredentialStoreError::Oversized)
        );
        assert_eq!(
            SavedCredentials::new("u\n".to_owned(), "e".to_owned(), "t".to_owned()),
            Err(CredentialStoreError::Invalid)
        );
        assert_eq!(
            deserialize_credentials(SecretBytes::new(vec![b'x'; MAX_SECRET_BYTES + 1])),
            Err(CredentialStoreError::Oversized)
        );
    }

    #[test]
    fn debug_and_display_are_redacted() {
        let value = credentials();
        let debug = format!("{value:?}");
        assert!(!debug.contains("token-value"));
        assert!(!debug.contains("person@example.com"));
        let error = CredentialStoreError::Unavailable;
        assert!(!format!("{error}").contains("example"));
    }

    #[test]
    fn idempotent_backend_results_are_mapped_without_platform_text() {
        assert_eq!(
            map_backend_result::<()>(Err(BackendError::NoEntry)),
            Err(CredentialStoreError::Unavailable)
        );
        assert_eq!(
            map_backend_result::<()>(Err(BackendError::Unavailable)),
            Err(CredentialStoreError::Unavailable)
        );
        assert_eq!(
            map_delete_backend_result(Ok(())),
            Ok(DeleteOutcome::Deleted)
        );
        assert_eq!(
            map_delete_backend_result(Err(BackendError::NoEntry)),
            Ok(DeleteOutcome::Absent)
        );
        assert_eq!(
            map_backend_result(map_delete_backend_result(Err(BackendError::Unavailable))),
            Err(CredentialStoreError::Unavailable)
        );
    }
}
