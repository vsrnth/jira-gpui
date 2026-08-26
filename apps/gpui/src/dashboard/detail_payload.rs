use std::sync::Arc;

use jira_application::{ApplicationError, CancellationToken, IssueLocator, PortFuture};
use jira_domain::{Issue, IssueDetail, IssueId, IssueKey, RichImage};

use super::{
    LiveWorkspace,
    media::{
        collect_detail_images_with_context, fetch_rich_image_states, loading_image_states,
        rich_image_contexts,
    },
};
use crate::{
    diagnostics::{DiagnosticFlow, DiagnosticsSink, ImageSource},
    presentation::IssueDetailViewModel,
    rich_text_view::RichImageRenderStates,
};

/// The only two read routes used by the issue-detail surfaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum DetailReadRequest {
    Selected { issue_id: IssueId },
    Remote { key: IssueKey },
}

impl DetailReadRequest {
    fn locator(&self) -> IssueLocator {
        match self {
            Self::Selected { issue_id } => IssueLocator::Id(issue_id.clone()),
            Self::Remote { key } => IssueLocator::Key(key.clone()),
        }
    }
}

/// Select the exact Jira issue identity used by the image phase. Selected
/// reads remain pinned to the originally requested ID; key lookups use the ID
/// returned by the payload.
pub(super) fn detail_image_issue_id(
    request: &DetailReadRequest,
    returned_issue_id: &IssueId,
) -> IssueId {
    match request {
        DetailReadRequest::Selected { issue_id } => issue_id.clone(),
        DetailReadRequest::Remote { .. } => returned_issue_id.clone(),
    }
}

/// Narrow read seam used by the dashboard payload preparation. It contains no
/// epoch state and cannot dispatch Jira writes.
pub(super) trait DetailReadPort {
    fn fetch_detail<'a>(
        &'a self,
        request: &'a DetailReadRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssueDetail>;
}

impl DetailReadPort for LiveWorkspace {
    fn fetch_detail<'a>(
        &'a self,
        request: &'a DetailReadRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssueDetail> {
        match request {
            DetailReadRequest::Selected { .. } => {
                Box::pin(self.fetch_issue_detail(request.locator(), cancellation))
            }
            DetailReadRequest::Remote { key } => {
                Box::pin(self.lookup_issue(key.clone(), cancellation))
            }
        }
    }
}

pub(super) async fn read_detail<P: DetailReadPort + ?Sized>(
    port: &P,
    request: DetailReadRequest,
    cancellation: &CancellationToken,
) -> Result<IssueDetail, ApplicationError> {
    port.fetch_detail(&request, cancellation).await
}

/// The mapped first phase of either a selected or remote detail read.
/// `images` remains available for the authenticated second phase while
/// `loading` is applied immediately by the caller.
pub(super) struct DetailPayload {
    pub(super) issue: Issue,
    pub(super) view: IssueDetailViewModel,
    pub(super) images: Vec<(RichImage, usize, ImageSource)>,
    pub(super) image_contexts: Vec<(usize, ImageSource)>,
    pub(super) loading: RichImageRenderStates,
}

pub(super) fn prepare_detail_payload(
    detail: &IssueDetail,
    users: &[jira_domain::User],
    diagnostics: &DiagnosticsSink,
    flow: DiagnosticFlow,
    load_token: u64,
) -> DetailPayload {
    let issue = detail.core.issue.clone();
    let view = IssueDetailViewModel::from_domain(detail, users);
    let images = collect_detail_images_with_context(&view);
    let image_contexts = rich_image_contexts(&images);
    let loading = loading_image_states(&images, diagnostics, flow, load_token);
    DetailPayload {
        issue,
        view,
        images,
        image_contexts,
        loading,
    }
}

pub(super) async fn fetch_detail_images(
    workspace: Arc<LiveWorkspace>,
    issue_id: IssueId,
    payload: DetailPayload,
    cancellation: CancellationToken,
    diagnostics: DiagnosticsSink,
    flow: DiagnosticFlow,
    load_token: u64,
) -> Result<RichImageRenderStates, ()> {
    fetch_rich_image_states(
        workspace.clone(),
        workspace.site_id().clone(),
        issue_id,
        payload.images,
        cancellation,
        diagnostics,
        flow,
        load_token,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use super::*;
    use crate::sample_data::{sample_issues, sample_users};

    #[derive(Default)]
    struct FakeReadPort {
        requests: Mutex<Vec<DetailReadRequest>>,
        cancellations: Mutex<Vec<CancellationToken>>,
        responses: Mutex<VecDeque<Result<IssueDetail, ApplicationError>>>,
    }

    impl DetailReadPort for FakeReadPort {
        fn fetch_detail<'a>(
            &'a self,
            request: &'a DetailReadRequest,
            cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssueDetail> {
            self.requests
                .lock()
                .expect("request lock")
                .push(request.clone());
            self.cancellations
                .lock()
                .expect("cancellation lock")
                .push(cancellation.clone());
            let response = self
                .responses
                .lock()
                .expect("response lock")
                .pop_front()
                .expect("queued fake response");
            Box::pin(async move { response })
        }
    }

    fn fake_detail() -> IssueDetail {
        IssueDetail {
            core: jira_domain::IssueDetailCore {
                issue: sample_issues().into_iter().next().expect("sample issue"),
                attachments: Vec::new(),
            },
            comments: Vec::new(),
        }
    }

    #[test]
    fn selected_and_remote_requests_route_exactly_once_without_writes_or_retries() {
        let port = Arc::new(FakeReadPort {
            requests: Mutex::new(Vec::new()),
            cancellations: Mutex::new(Vec::new()),
            responses: Mutex::new(VecDeque::from([Ok(fake_detail()), Ok(fake_detail())])),
        });
        let selected_id = IssueId::new("10001").expect("issue ID");
        let remote_key = IssueKey::new("DESK-176").expect("issue key");

        let selected = futures_lite::future::block_on(read_detail(
            port.as_ref(),
            DetailReadRequest::Selected {
                issue_id: selected_id.clone(),
            },
            &CancellationToken::new(),
        ))
        .expect("selected detail");
        let remote = futures_lite::future::block_on(read_detail(
            port.as_ref(),
            DetailReadRequest::Remote {
                key: remote_key.clone(),
            },
            &CancellationToken::new(),
        ))
        .expect("remote detail");

        assert_eq!(selected.core.issue.id, remote.core.issue.id);
        assert_eq!(
            *port.requests.lock().expect("request lock"),
            vec![
                DetailReadRequest::Selected {
                    issue_id: selected_id
                },
                DetailReadRequest::Remote { key: remote_key },
            ]
        );
    }

    #[test]
    fn image_fetch_identity_uses_requested_selected_id_and_returned_remote_id_once() {
        let requested = IssueId::new("requested-100").expect("requested issue");
        let returned = IssueId::new("returned-200").expect("returned issue");
        let selected_request = DetailReadRequest::Selected {
            issue_id: requested.clone(),
        };
        let remote_request = DetailReadRequest::Remote {
            key: IssueKey::new("DESK-200").expect("issue key"),
        };
        let mut recorded_issue_ids = Vec::new();
        recorded_issue_ids.push(detail_image_issue_id(&selected_request, &returned));
        recorded_issue_ids.push(detail_image_issue_id(&remote_request, &returned));

        assert_eq!(recorded_issue_ids, vec![requested, returned]);
        assert_eq!(recorded_issue_ids.len(), 2);
    }

    #[test]
    fn queued_read_error_propagates_once_without_retry_and_keeps_token_identity() {
        let port = Arc::new(FakeReadPort {
            responses: Mutex::new(VecDeque::from([Err(ApplicationError::new(
                jira_application::ErrorKind::Offline,
                "neutral fake failure",
            ))])),
            ..FakeReadPort::default()
        });
        let cancellation = CancellationToken::new();
        let result = futures_lite::future::block_on(read_detail(
            port.as_ref(),
            DetailReadRequest::Selected {
                issue_id: IssueId::new("10001").expect("issue ID"),
            },
            &cancellation,
        ));

        assert_eq!(
            result.expect_err("queued error").kind(),
            jira_application::ErrorKind::Offline
        );
        assert_eq!(port.requests.lock().expect("request lock").len(), 1);
        let recorded = port.cancellations.lock().expect("cancellation lock");
        assert_eq!(recorded.len(), 1);
        cancellation.cancel();
        assert!(recorded[0].is_cancelled());
    }

    fn image(id: &str) -> RichImage {
        RichImage {
            attachment_id: id.to_owned(),
            filename: format!("{id}.png"),
            mime_type: "image/png".to_owned(),
            alt_text: Some(format!("alt {id}")),
            width: Some(32),
            height: Some(24),
        }
    }

    #[test]
    fn payload_mapping_preserves_nonempty_order_contexts_loading_and_token() {
        use jira_domain::{
            AttachmentMetadata, IssueComment, IssueDetailCore, RichBlock, RichTextDocument,
        };

        let mut issue = fake_detail().core.issue;
        issue.description_text = Some("safe fixture description".to_owned());
        issue.rich_description = Some(
            RichTextDocument::new(vec![RichBlock::Image(image("description-image"))], false)
                .with_fallback_images(vec![image("fallback-image")]),
        );
        let mut comment = IssueComment::new(
            "comment-1",
            None,
            "safe fixture comment",
            time::macros::datetime!(2026-01-03 00:00 UTC),
            None,
            Vec::new(),
        )
        .expect("comment");
        comment.rich_body = Some(RichTextDocument::new(
            vec![RichBlock::Image(image("comment-image"))],
            false,
        ));
        let detail = IssueDetail::new(
            IssueDetailCore::new(
                issue.clone(),
                vec![
                    AttachmentMetadata::new("attachment-1", "safe.txt", 12, Some("text/plain"))
                        .expect("attachment"),
                ],
            ),
            vec![comment],
        );
        let diagnostics = DiagnosticsSink::disabled();
        let payload = prepare_detail_payload(
            &detail,
            &sample_users(),
            &diagnostics,
            DiagnosticFlow::RemoteLookup,
            17,
        );

        assert_eq!(payload.issue, issue);
        assert_eq!(
            payload
                .images
                .iter()
                .map(|(image, _, _)| image.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["description-image", "fallback-image", "comment-image"]
        );
        assert_eq!(
            payload.image_contexts,
            vec![
                (0, ImageSource::ResolvedAdf),
                (0, ImageSource::FallbackCandidate),
                (1, ImageSource::ResolvedAdf),
            ]
        );
        for (ordinal, (id, source, surface)) in [
            ("description-image", ImageSource::ResolvedAdf, 0),
            ("fallback-image", ImageSource::FallbackCandidate, 0),
            ("comment-image", ImageSource::ResolvedAdf, 1),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(matches!(
                payload.loading.get(id),
                Some(crate::rich_text_view::RichImageRenderState::Loading)
            ));
            let context = payload
                .loading
                .context_for(id, ordinal, surface, source)
                .expect("image context");
            assert_eq!(context.flow, DiagnosticFlow::RemoteLookup);
            assert_eq!(context.load_token, 17);
            assert_eq!(context.candidate_ordinal, ordinal);
            assert_eq!(context.surface_ordinal, surface);
            assert_eq!(context.source, source);
        }
    }

    #[test]
    fn payload_mapping_preserves_issue_users_and_empty_image_order() {
        let detail = fake_detail();
        let users = sample_users();
        let diagnostics = DiagnosticsSink::disabled();
        let payload = prepare_detail_payload(
            &detail,
            &users,
            &diagnostics,
            DiagnosticFlow::SelectedDetail,
            7,
        );

        assert_eq!(payload.issue, detail.core.issue);
        assert_eq!(
            payload.view,
            IssueDetailViewModel::from_domain(&detail, &users)
        );
        assert!(payload.images.is_empty());
        assert!(payload.image_contexts.is_empty());
        assert!(payload.loading.get("missing").is_none());
    }
}
