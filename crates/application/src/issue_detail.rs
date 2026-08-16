use std::{collections::HashSet, sync::Arc};

use jira_domain::IssueDetail;

use crate::{
    ApplicationError, CancellationToken, IssueCommentsPageRequest, IssueDetailRequest, JiraReadPort,
};

const MAX_PAGE_SIZE: usize = 1_000;

/// Bounds for the read-only comment aggregation performed by `IssueDetailService`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IssueDetailConfig {
    pub comment_page_size: usize,
    pub max_comment_pages: usize,
    pub max_comments: usize,
}

impl Default for IssueDetailConfig {
    fn default() -> Self {
        Self {
            comment_page_size: 100,
            max_comment_pages: 1_000,
            max_comments: 10_000,
        }
    }
}

/// Application orchestration for one issue's core data and all paginated comments.
#[derive(Clone)]
pub struct IssueDetailService {
    jira: Arc<dyn JiraReadPort>,
    config: IssueDetailConfig,
}

impl IssueDetailService {
    pub fn new(jira: Arc<dyn JiraReadPort>, config: IssueDetailConfig) -> Self {
        Self { jira, config }
    }

    pub async fn fetch(
        &self,
        request: IssueDetailRequest,
        cancellation: &CancellationToken,
    ) -> Result<IssueDetail, ApplicationError> {
        self.validate_config()?;
        cancellation.check()?;

        let core = self.jira.fetch_issue_detail(&request, cancellation).await?;
        cancellation.check()?;
        if core.issue.site_id != request.site_id || core.issue.id != request.issue_id {
            return Err(upstream("Jira returned detail for a different issue"));
        }

        let mut comments = Vec::new();
        let mut start_at = 0;
        let mut page_cursor = None;
        let mut seen_cursors = HashSet::new();
        let mut completed = false;

        for _ in 0..self.config.max_comment_pages {
            cancellation.check()?;
            let page = self
                .jira
                .fetch_issue_comments_page(
                    &IssueCommentsPageRequest {
                        site_id: request.site_id.clone(),
                        issue_id: request.issue_id.clone(),
                        start_at,
                        page_cursor: page_cursor.clone(),
                        page_size: self.config.comment_page_size,
                    },
                    cancellation,
                )
                .await?;
            cancellation.check()?;

            let crate::IssueCommentsPage {
                comments: page_comments,
                start_at: page_start_at,
                next_start_at,
                next_cursor,
                total,
            } = page;
            let page_comment_count = page_comments.len();

            if page_start_at != start_at {
                return Err(upstream("Jira returned an invalid comments startAt"));
            }
            if page_comment_count > self.config.comment_page_size {
                return Err(upstream("Jira returned more comments than requested"));
            }
            if let Some(total) = total {
                if total > self.config.max_comments {
                    return Err(upstream(
                        "Jira comments exceeded the configured safety limit",
                    ));
                }
                if comments.len().saturating_add(page_comment_count) > total {
                    return Err(upstream(
                        "Jira returned more comments than its reported total",
                    ));
                }
            }
            comments.extend(page_comments);
            if comments.len() > self.config.max_comments {
                return Err(upstream(
                    "Jira comments exceeded the configured safety limit",
                ));
            }
            if let Some(total) = total
                && comments.len() == total
            {
                completed = true;
                break;
            }

            if next_cursor.is_some() && next_start_at.is_some() {
                return Err(upstream("Jira returned ambiguous comments pagination"));
            }
            if let Some(next_cursor) = next_cursor {
                let value = next_cursor.0;
                if page_cursor.as_ref().is_some_and(|cursor| cursor.0 == value)
                    || !seen_cursors.insert(value.clone())
                {
                    return Err(upstream("Jira returned a comments cursor cycle"));
                }
                page_cursor = Some(crate::PageCursor(value));
                continue;
            }
            page_cursor = None;

            if let Some(next_start_at) = next_start_at {
                if next_start_at <= start_at {
                    return Err(upstream("Jira returned invalid comments startAt progress"));
                }
                if total.is_some_and(|total| next_start_at > total) {
                    return Err(upstream("Jira returned comments startAt beyond total"));
                }
                start_at = next_start_at;
                continue;
            }

            if total.is_some() {
                return Err(upstream(
                    "Jira comments pagination stopped before its total",
                ));
            }
            if page_comment_count == self.config.comment_page_size {
                let next_start_at = start_at
                    .checked_add(page_comment_count)
                    .ok_or_else(|| upstream("Jira returned an invalid comments startAt"))?;
                if next_start_at <= start_at {
                    return Err(upstream("Jira returned invalid comments startAt progress"));
                }
                start_at = next_start_at;
                continue;
            }
            completed = true;
            break;
        }

        if !completed {
            return Err(upstream(
                "Jira comment pagination exceeded the safety limit",
            ));
        }

        IssueDetail::new(core, comments)
            .map_err(|_| ApplicationError::invalid_input("invalid Jira issue detail payload"))
    }

    pub async fn load(
        &self,
        request: IssueDetailRequest,
        cancellation: &CancellationToken,
    ) -> Result<IssueDetail, ApplicationError> {
        self.fetch(request, cancellation).await
    }

    fn validate_config(&self) -> Result<(), ApplicationError> {
        if !(1..=MAX_PAGE_SIZE).contains(&self.config.comment_page_size)
            || self.config.max_comment_pages == 0
            || self.config.max_comments == 0
        {
            return Err(ApplicationError::invalid_input(
                "issue detail pagination configuration is invalid",
            ));
        }
        Ok(())
    }
}

fn upstream(message: &'static str) -> ApplicationError {
    ApplicationError::new(crate::ErrorKind::Upstream, message)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        future::Future,
        sync::{Arc, Mutex},
        task::{Context, Poll, Wake, Waker},
    };

    use jira_domain::{
        Issue, IssueComment, IssueDetailCore, IssueId, IssueKey, IssueType, JiraSiteId, Priority,
        Project, Status, User,
    };
    use time::macros::datetime;

    use super::*;
    use crate::{
        ApplicationError, CancellationToken, ErrorKind, IssueCommentsPage,
        IssueCommentsPageRequest, IssueDetailRequest, IssueFetchRequest, IssuePage, JiraReadPort,
        PortFuture, UserSearchRequest,
    };

    #[derive(Default)]
    struct FakeJira {
        core: Mutex<VecDeque<Result<IssueDetailCore, ApplicationError>>>,
        pages: Mutex<VecDeque<Result<IssueCommentsPage, ApplicationError>>>,
        page_requests: Mutex<Vec<IssueCommentsPageRequest>>,
        cancel_on_comments: bool,
    }

    impl JiraReadPort for FakeJira {
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

        fn fetch_current_user<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, User> {
            Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "not used")) })
        }

        fn search_users<'a>(
            &'a self,
            _request: &'a UserSearchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<User>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn fetch_issue_page<'a>(
            &'a self,
            _request: &'a IssueFetchRequest,
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, IssuePage> {
            Box::pin(async {
                Ok(IssuePage {
                    issues: Vec::new(),
                    next_cursor: None,
                    server_time: None,
                })
            })
        }

        fn fetch_issues_by_id<'a>(
            &'a self,
            _site_id: &'a JiraSiteId,
            _issue_ids: &'a [IssueId],
            _cancellation: &'a CancellationToken,
        ) -> PortFuture<'a, Vec<Issue>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    fn request() -> IssueDetailRequest {
        IssueDetailRequest {
            site_id: JiraSiteId::new("site").expect("site"),
            issue_id: IssueId::new("100").expect("issue"),
        }
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
        .expect("core")
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

    fn block_on<F: Future>(future: F) -> F::Output {
        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }

        let waker = Waker::from(Arc::new(Noop));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
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
}
