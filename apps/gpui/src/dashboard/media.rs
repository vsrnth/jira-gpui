//! Private media facade for dashboard consumers.
//!
//! Cataloging, authenticated loading, and local download are kept in separate
//! modules. This facade intentionally preserves the narrow surface used by the
//! dashboard and detail-payload phases.

use jira_domain::IssueId;

mod catalog;
mod download;
mod loader;
mod policy;

pub(super) use catalog::{
    collect_detail_images_with_context, loading_image_states, rich_image_contexts,
};
pub(super) use download::{
    AttachmentDownloadState, MAX_ATTACHMENT_DOWNLOAD_BYTES, attachment_download_button_label,
    attachment_download_is_current, attachment_issue_id, attachment_temp_path,
    attachment_temp_token, cleanup_attachment_temp, inline_attachment_for_download,
    portal_download_directory, sanitized_attachment_filename, write_attachment_temp,
};
pub(super) use loader::{fetch_cached_rich_image_states, fetch_rich_image_states};

pub(super) fn image_result_is_current(
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
    generation: u64,
    expected_generation: u64,
) -> bool {
    super::detail_result_is_current(
        selected_issue,
        expected_issue,
        generation,
        expected_generation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_results_reject_stale_selection_generation() {
        let first = IssueId::new("first").expect("issue");
        let second = IssueId::new("second").expect("issue");
        assert!(!image_result_is_current(Some(&first), &first, 3, 2));
        assert!(!image_result_is_current(Some(&second), &first, 2, 2));
        assert!(image_result_is_current(Some(&first), &first, 2, 2));
    }
}
