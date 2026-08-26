use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use jira_application::CancellationToken;
use jira_domain::IssueId;

use super::policy;
use crate::presentation::AttachmentViewModel;

pub(crate) const MAX_ATTACHMENT_DOWNLOAD_BYTES: usize = policy::MAX_ATTACHMENT_DOWNLOAD_BYTES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AttachmentDownloadState {
    Idle,
    Saving { attachment_id: String },
}

pub(crate) fn sanitized_attachment_filename(filename: &str) -> String {
    policy::sanitized_attachment_filename(filename)
}

fn choose_portal_download_directory(
    home: Option<&Path>,
    downloads: Option<&Path>,
    current: &Path,
) -> PathBuf {
    policy::choose_portal_download_directory(home, downloads, current)
}

/// Resolve only the local environment's preferred initial directory. The
/// selected destination continues to come from the portal picker.
pub(crate) fn portal_download_directory() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let downloads = std::env::var_os("XDG_DOWNLOAD_DIR")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|path| path.join("Downloads")));
    choose_portal_download_directory(home.as_deref(), downloads.as_deref(), Path::new("."))
}

pub(crate) fn attachment_temp_path(destination: &Path, unique_token: &str) -> PathBuf {
    policy::attachment_temp_path(destination, unique_token)
}

pub(crate) fn attachment_temp_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

pub(crate) fn write_attachment_temp(
    path: &Path,
    bytes: &[u8],
    cancellation: &CancellationToken,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("Download cancelled".to_owned());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "Could not create a temporary attachment file".to_owned())?;
    let result = (|| {
        file.write_all(bytes)
            .map_err(|_| "Could not save the attachment".to_owned())?;
        file.sync_all()
            .map_err(|_| "Could not save the attachment".to_owned())?;
        if cancellation.is_cancelled() {
            return Err("Download cancelled".to_owned());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}

pub(crate) fn cleanup_attachment_temp(path: &Path) {
    let _ = std::fs::remove_file(path);
}

pub(crate) fn attachment_download_is_current(
    current_generation: u64,
    expected_generation: u64,
    cancellation: &CancellationToken,
) -> bool {
    policy::attachment_generation_is_current(current_generation, expected_generation)
        && !cancellation.is_cancelled()
}

pub(crate) fn attachment_download_button_label(active: bool) -> &'static str {
    if active { "Saving…" } else { "Download" }
}

pub(crate) fn attachment_issue_id(
    selected_issue: Option<&IssueId>,
    remote_issue: Option<&IssueId>,
) -> Option<IssueId> {
    policy::attachment_issue_id(selected_issue, remote_issue)
}

#[cfg(test)]
fn unique_attachment_for_id(
    attachments: &[AttachmentViewModel],
    attachment_id: &str,
) -> Option<AttachmentViewModel> {
    policy::unique_exact_id_index(attachments, attachment_id, |attachment| &attachment.id)
        .map(|index| attachments[index].clone())
}

pub(crate) fn inline_attachment_for_download(
    expected_issue_id: &IssueId,
    active_issue_id: &IssueId,
    attachments: &[AttachmentViewModel],
    attachment_id: &str,
) -> Option<AttachmentViewModel> {
    policy::inline_attachment_index(
        expected_issue_id,
        active_issue_id,
        attachments,
        attachment_id,
        |attachment| &attachment.id,
    )
    .map(|index| attachments[index].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(id: &str) -> AttachmentViewModel {
        AttachmentViewModel {
            id: id.to_owned(),
            filename: "report.csv".to_owned(),
            mime_type: "text/csv".to_owned(),
            size_bytes: 12,
            size: "12 B".to_owned(),
        }
    }

    #[test]
    fn inline_attachment_download_requires_one_nonempty_exact_id() {
        let attachments = vec![attachment("attachment-1"), attachment("attachment-2")];
        let current_issue = IssueId::new("100").expect("issue");
        let stale_issue = IssueId::new("200").expect("issue");
        assert_eq!(
            inline_attachment_for_download(
                &current_issue,
                &current_issue,
                &attachments,
                "attachment-1"
            )
            .expect("current issue attachment")
            .id,
            "attachment-1"
        );
        assert!(
            inline_attachment_for_download(
                &current_issue,
                &stale_issue,
                &attachments,
                "attachment-1"
            )
            .is_none()
        );
        assert_eq!(
            unique_attachment_for_id(&attachments, "attachment-1")
                .expect("exact attachment")
                .id,
            "attachment-1"
        );
        assert!(unique_attachment_for_id(&attachments, "").is_none());
        assert!(unique_attachment_for_id(&attachments, "missing").is_none());
        assert!(
            !unique_attachment_for_id(
                &[attachment("attachment-1"), attachment("attachment-1")],
                "attachment-1"
            )
            .is_some()
        );
        assert!(unique_attachment_for_id(&[attachment("")], "").is_none());
    }

    #[test]
    fn attachment_download_state_and_labels_reflect_busy_operation() {
        assert_eq!(attachment_download_button_label(false), "Download");
        assert_eq!(attachment_download_button_label(true), "Saving…");
        assert!(matches!(
            AttachmentDownloadState::Saving {
                attachment_id: "att-1".to_owned()
            },
            AttachmentDownloadState::Saving { .. }
        ));
    }

    #[test]
    fn attachment_download_commit_requires_current_generation_and_live_token() {
        let cancellation = CancellationToken::new();
        assert!(attachment_download_is_current(4, 4, &cancellation));
        assert!(!attachment_download_is_current(5, 4, &cancellation));
        cancellation.cancel();
        assert!(!attachment_download_is_current(4, 4, &cancellation));
    }

    #[test]
    fn attachment_temp_path_is_unique_and_stays_with_destination() {
        let destination = Path::new("/tmp/downloads/report.pdf");
        let temporary = attachment_temp_path(destination, "unique-token");
        assert_eq!(temporary.parent(), destination.parent());
        assert_ne!(temporary, destination);
        assert_eq!(
            temporary.file_name().and_then(|name| name.to_str()),
            Some(".report.pdf.jira-desk-unique-token.part")
        );
    }

    struct TempDirectory {
        path: std::path::PathBuf,
    }

    impl TempDirectory {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "jira-gpui-download-{label}-{}-{}",
                std::process::id(),
                attachment_temp_token()
            ));
            std::fs::create_dir_all(&root).expect("temp root");
            Self { path: root }
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn attachment_temp_write_never_overwrites_an_existing_path() {
        let directory = TempDirectory::new("no-overwrite");
        let path = directory.path.join("attachment.part");
        std::fs::write(&path, b"existing").expect("existing file");
        assert!(write_attachment_temp(&path, b"replacement", &CancellationToken::new()).is_err());
        assert_eq!(std::fs::read(&path).expect("existing bytes"), b"existing");
        cleanup_attachment_temp(&path);
    }

    #[test]
    fn cancelled_attachment_temp_write_leaves_no_file() {
        let directory = TempDirectory::new("cancelled");
        let path = directory.path.join("attachment.part");
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            write_attachment_temp(&path, b"should not be written", &cancellation),
            Err("Download cancelled".to_owned())
        );
        assert!(!path.exists());
    }

    #[test]
    fn attachment_issue_id_prefers_loaded_remote_issue() {
        let local = IssueId::new("local").expect("issue");
        let remote = IssueId::new("remote").expect("issue");
        assert_eq!(
            attachment_issue_id(Some(&local), Some(&remote)),
            Some(remote.clone())
        );
        assert_eq!(attachment_issue_id(Some(&local), None), Some(local));
        assert_eq!(attachment_issue_id(None, None), None);
    }

    #[test]
    fn filename_sanitization_strips_paths_controls_and_bounds_utf8_bytes() {
        let filename = format!("/private/..\\{}\0", "é".repeat(200));
        let sanitized = sanitized_attachment_filename(&filename);
        assert!(!sanitized.contains('/'));
        assert!(!sanitized.contains('\\'));
        assert!(!sanitized.chars().any(char::is_control));
        assert!(sanitized.len() <= 255);
        assert!(sanitized.is_char_boundary(sanitized.len()));
        assert_eq!(sanitized_attachment_filename(" .. "), "jira-attachment");
        assert_eq!(sanitized_attachment_filename(&"a".repeat(256)).len(), 255);
    }

    #[test]
    fn download_cap_is_explicit() {
        assert_eq!(MAX_ATTACHMENT_DOWNLOAD_BYTES, 64 * 1024 * 1024);
    }

    #[test]
    fn portal_directory_prefers_existing_downloads_then_home_then_current() {
        let directory = TempDirectory::new("portal");
        let downloads = directory.path.join("Downloads");
        let home = directory.path.join("home");
        std::fs::create_dir_all(&downloads).expect("downloads");
        std::fs::create_dir_all(&home).expect("home");
        assert_eq!(
            choose_portal_download_directory(Some(&home), Some(&downloads), Path::new(".")),
            downloads
        );
        assert_eq!(
            choose_portal_download_directory(Some(&home), None, Path::new(".")),
            home
        );
        assert_eq!(
            choose_portal_download_directory(None, None, Path::new(".")),
            PathBuf::from(".")
        );
    }
}
