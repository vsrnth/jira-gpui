use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use jira_domain::{
    Issue, IssueComment, IssueDetailCore, IssueId, IssueKey, IssueType, JiraSiteId, Priority,
    Project, Status,
};
use time::macros::datetime;

use super::*;
use crate::{
    ApplicationError, CancellationToken, ErrorKind, IssueCommentsPage, IssueCommentsPageRequest,
    IssueDetailRequest, IssueLocator, JiraIssueDetailReadPort, PortFuture, test_support::block_on,
};

#[derive(Default)]
struct FakeJira {
    core: Mutex<VecDeque<Result<IssueDetailCore, ApplicationError>>>,
    pages: Mutex<VecDeque<Result<IssueCommentsPage, ApplicationError>>>,
    page_requests: Mutex<Vec<IssueCommentsPageRequest>>,
    cancel_on_comments: bool,
}

impl JiraIssueDetailReadPort for FakeJira {
    fn fetch_issue_detail<'a>(
        &'a self,
        _request: &'a IssueDetailRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssueDetailCore> {
        let result = self
            .core
            .lock()
            .expect("core lock")
            .pop_front()
            .expect("core response");
        Box::pin(async move { result })
    }

    fn fetch_issue_comments_page<'a>(
        &'a self,
        request: &'a IssueCommentsPageRequest,
        cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssueCommentsPage> {
        self.page_requests
            .lock()
            .expect("page requests lock")
            .push(request.clone());
        let result = self
            .pages
            .lock()
            .expect("pages lock")
            .pop_front()
            .expect("comments page response");
        let cancel = self.cancel_on_comments;
        let cancellation = cancellation.clone();
        Box::pin(async move {
            if cancel {
                cancellation.cancel();
            }
            result
        })
    }
}

fn request() -> IssueDetailRequest {
    IssueDetailRequest {
        site_id: JiraSiteId::new("site").expect("site"),
        locator: IssueLocator::Id(IssueId::new("100").expect("issue")),
    }
}

#[test]
fn key_locator_uses_returned_numeric_id_for_comment_requests() {
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([Ok(IssueCommentsPage {
            comments: Vec::new(),
            start_at: 0,
            next_start_at: None,
            next_cursor: None,
            total: Some(0),
        })])),
        ..FakeJira::default()
    });
    let request = IssueDetailRequest {
        site_id: JiraSiteId::new("site").expect("site"),
        locator: IssueLocator::Key(IssueKey::new("APP-100").expect("key")),
    };

    block_on(
        IssueDetailService::new(jira.clone(), IssueDetailConfig::default())
            .fetch(request, &CancellationToken::new()),
    )
    .expect("key detail");

    let requests = jira.page_requests.lock().expect("page requests lock");
    assert_eq!(requests[0].issue_id.as_str(), "100");
}

#[test]
fn key_locator_rejects_a_different_returned_key() {
    let mut returned = core();
    returned.issue.key = IssueKey::new("APP-999").expect("key");
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(returned)])),
        ..FakeJira::default()
    });
    let request = IssueDetailRequest {
        site_id: JiraSiteId::new("site").expect("site"),
        locator: IssueLocator::Key(IssueKey::new("APP-100").expect("key")),
    };

    let error = block_on(
        IssueDetailService::new(jira, IssueDetailConfig::default())
            .fetch(request, &CancellationToken::new()),
    )
    .expect_err("key mismatch");
    assert_eq!(error.kind(), ErrorKind::Upstream);
}

#[test]
fn id_locator_rejects_a_different_returned_id() {
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        ..FakeJira::default()
    });
    let request = IssueDetailRequest {
        site_id: JiraSiteId::new("site").expect("site"),
        locator: IssueLocator::Id(IssueId::new("999").expect("issue")),
    };

    let error = block_on(
        IssueDetailService::new(jira, IssueDetailConfig::default())
            .fetch(request, &CancellationToken::new()),
    )
    .expect_err("ID mismatch");
    assert_eq!(error.kind(), ErrorKind::Upstream);
}

fn core() -> IssueDetailCore {
    IssueDetailCore::new(
        Issue::new(
            JiraSiteId::new("site").expect("site"),
            IssueId::new("100").expect("issue"),
            IssueKey::new("APP-100").expect("key"),
            Project {
                id: "1".into(),
                key: "APP".into(),
                name: "App".into(),
            },
            IssueType {
                id: "1".into(),
                name: "Task".into(),
                icon_url: None,
            },
            "Task",
            Status {
                id: "1".into(),
                name: "Open".into(),
                category: None,
            },
            Priority {
                id: None,
                name: None,
                icon_url: None,
            },
            None,
            None,
            None,
            Vec::new(),
            datetime!(2026-01-01 00:00 UTC),
            datetime!(2026-01-01 00:00 UTC),
            None,
        ),
        Vec::new(),
    )
}

fn comment(id: &str) -> IssueComment {
    IssueComment::new(
        id,
        None,
        format!("Comment {id}"),
        datetime!(2026-01-02 00:00 UTC),
        None,
        Vec::new(),
    )
    .expect("comment")
}

#[test]
fn fetches_core_and_all_comment_pages_in_start_at_order() {
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([
            Ok(IssueCommentsPage {
                comments: vec![comment("1")],
                start_at: 0,
                next_start_at: Some(1),
                next_cursor: None,
                total: Some(2),
            }),
            Ok(IssueCommentsPage {
                comments: vec![comment("2")],
                start_at: 1,
                next_start_at: None,
                next_cursor: None,
                total: Some(2),
            }),
        ])),
        ..FakeJira::default()
    });
    let service = IssueDetailService::new(jira.clone(), IssueDetailConfig::default());

    let detail =
        block_on(service.fetch(request(), &CancellationToken::new())).expect("issue detail");

    assert_eq!(detail.core.issue.id.as_str(), "100");
    assert_eq!(
        detail
            .comments
            .iter()
            .map(|comment| comment.id.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "2"]
    );
    let requests = jira.page_requests.lock().expect("page requests lock");
    assert_eq!(requests[0].start_at, 0);
    assert_eq!(requests[1].start_at, 1);
}

#[test]
fn returns_core_when_the_issue_has_no_comments() {
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([Ok(IssueCommentsPage {
            comments: Vec::new(),
            start_at: 0,
            next_start_at: None,
            next_cursor: None,
            total: Some(0),
        })])),
        ..FakeJira::default()
    });

    let detail = block_on(
        IssueDetailService::new(jira, IssueDetailConfig::default())
            .fetch(request(), &CancellationToken::new()),
    )
    .expect("detail without comments");

    assert!(detail.comments.is_empty());
}

#[test]
fn follows_cursor_pages_and_preserves_typed_request_identity() {
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([
            Ok(IssueCommentsPage {
                comments: vec![comment("1")],
                start_at: 0,
                next_start_at: None,
                next_cursor: Some(crate::PageCursor("next".into())),
                total: None,
            }),
            Ok(IssueCommentsPage {
                comments: vec![comment("2")],
                start_at: 0,
                next_start_at: None,
                next_cursor: None,
                total: None,
            }),
        ])),
        ..FakeJira::default()
    });

    let detail = block_on(
        IssueDetailService::new(jira.clone(), IssueDetailConfig::default())
            .fetch(request(), &CancellationToken::new()),
    )
    .expect("cursor detail");

    assert_eq!(detail.comments.len(), 2);
    let requests = jira.page_requests.lock().expect("page requests lock");
    assert_eq!(requests[0].site_id.as_str(), "site");
    assert_eq!(requests[0].issue_id.as_str(), "100");
    assert_eq!(requests[0].page_cursor, None);
    assert_eq!(
        requests[1].page_cursor,
        Some(crate::PageCursor("next".into()))
    );
}

#[test]
fn stops_with_cancellation_after_a_comment_page_returns() {
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([Ok(IssueCommentsPage {
            comments: Vec::new(),
            start_at: 0,
            next_start_at: None,
            next_cursor: None,
            total: Some(0),
        })])),
        cancel_on_comments: true,
        ..FakeJira::default()
    });
    let error = block_on(
        IssueDetailService::new(jira, IssueDetailConfig::default())
            .fetch(request(), &CancellationToken::new()),
    )
    .expect_err("cancellation");

    assert_eq!(error.kind(), ErrorKind::Cancelled);
}

#[test]
fn rejects_comment_page_limit_and_reported_total_limit() {
    let page = || {
        Ok(IssueCommentsPage {
            comments: vec![comment("1")],
            start_at: 0,
            next_start_at: Some(1),
            next_cursor: None,
            total: Some(2),
        })
    };
    let page_limited = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([page()])),
        ..FakeJira::default()
    });
    let error = block_on(
        IssueDetailService::new(
            page_limited,
            IssueDetailConfig {
                max_comment_pages: 1,
                ..IssueDetailConfig::default()
            },
        )
        .fetch(request(), &CancellationToken::new()),
    )
    .expect_err("page limit");
    assert_eq!(error.kind(), ErrorKind::Upstream);

    let total_limited = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([page()])),
        ..FakeJira::default()
    });
    let error = block_on(
        IssueDetailService::new(
            total_limited,
            IssueDetailConfig {
                max_comments: 1,
                ..IssueDetailConfig::default()
            },
        )
        .fetch(request(), &CancellationToken::new()),
    )
    .expect_err("total limit");
    assert_eq!(error.kind(), ErrorKind::Upstream);
}

#[test]
fn rejects_cursor_cycles_and_invalid_start_at_progress() {
    let cycle = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([
            Ok(IssueCommentsPage {
                comments: vec![comment("1")],
                start_at: 0,
                next_start_at: None,
                next_cursor: Some(crate::PageCursor("same".into())),
                total: None,
            }),
            Ok(IssueCommentsPage {
                comments: vec![comment("2")],
                start_at: 0,
                next_start_at: None,
                next_cursor: Some(crate::PageCursor("same".into())),
                total: None,
            }),
        ])),
        ..FakeJira::default()
    });
    let error = block_on(
        IssueDetailService::new(cycle, IssueDetailConfig::default())
            .fetch(request(), &CancellationToken::new()),
    )
    .expect_err("cursor cycle");
    assert_eq!(error.kind(), ErrorKind::Upstream);

    let invalid_progress = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([Ok(IssueCommentsPage {
            comments: vec![comment("1")],
            start_at: 0,
            next_start_at: Some(0),
            next_cursor: None,
            total: None,
        })])),
        ..FakeJira::default()
    });
    let error = block_on(
        IssueDetailService::new(invalid_progress, IssueDetailConfig::default())
            .fetch(request(), &CancellationToken::new()),
    )
    .expect_err("invalid startAt progress");
    assert_eq!(error.kind(), ErrorKind::Upstream);
}

#[test]
fn propagates_core_port_errors_unchanged() {
    let expected = ApplicationError::new(ErrorKind::Authentication, "auth failed");
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Err(expected.clone())])),
        ..FakeJira::default()
    });

    let error = block_on(
        IssueDetailService::new(jira, IssueDetailConfig::default())
            .fetch(request(), &CancellationToken::new()),
    )
    .expect_err("port error");

    assert_eq!(error, expected);

    let expected = ApplicationError::new(ErrorKind::Upstream, "comments failed");
    let jira = Arc::new(FakeJira {
        core: Mutex::new(VecDeque::from([Ok(core())])),
        pages: Mutex::new(VecDeque::from([Err(expected.clone())])),
        ..FakeJira::default()
    });
    let error = block_on(
        IssueDetailService::new(jira, IssueDetailConfig::default())
            .fetch(request(), &CancellationToken::new()),
    )
    .expect_err("comments port error");

    assert_eq!(error, expected);
}
