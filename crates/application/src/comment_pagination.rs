use std::collections::HashSet;

use jira_domain::IssueComment;

use crate::{ApplicationError, IssueCommentsPage, PageCursor};

#[derive(Debug)]
pub(crate) struct CommentPagination {
    page_size: usize,
    max_pages: usize,
    max_comments: usize,
    start_at: usize,
    page_cursor: Option<PageCursor>,
    seen_cursors: HashSet<String>,
    comments: Vec<IssueComment>,
    pages_fetched: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommentPageDecision {
    Continue,
    Complete,
}

impl CommentPagination {
    pub(crate) fn new(page_size: usize, max_pages: usize, max_comments: usize) -> Self {
        Self {
            page_size,
            max_pages,
            max_comments,
            start_at: 0,
            page_cursor: None,
            seen_cursors: HashSet::new(),
            comments: Vec::new(),
            pages_fetched: 0,
        }
    }

    pub(crate) fn start_at(&self) -> usize {
        self.start_at
    }

    pub(crate) fn page_cursor(&self) -> Option<PageCursor> {
        self.page_cursor.clone()
    }

    pub(crate) fn accept_page(
        &mut self,
        page: IssueCommentsPage,
    ) -> Result<CommentPageDecision, ApplicationError> {
        let IssueCommentsPage {
            comments: page_comments,
            start_at: page_start_at,
            next_start_at,
            next_cursor,
            total,
        } = page;
        let page_comment_count = page_comments.len();

        if page_start_at != self.start_at {
            return Err(upstream("Jira returned an invalid comments startAt"));
        }
        if page_comment_count > self.page_size {
            return Err(upstream("Jira returned more comments than requested"));
        }
        if let Some(total) = total {
            if total > self.max_comments {
                return Err(upstream(
                    "Jira comments exceeded the configured safety limit",
                ));
            }
            if self.comments.len().saturating_add(page_comment_count) > total {
                return Err(upstream(
                    "Jira returned more comments than its reported total",
                ));
            }
        }
        self.comments.extend(page_comments);
        if self.comments.len() > self.max_comments {
            return Err(upstream(
                "Jira comments exceeded the configured safety limit",
            ));
        }
        self.pages_fetched += 1;
        if let Some(total) = total
            && self.comments.len() == total
        {
            return Ok(CommentPageDecision::Complete);
        }

        if next_cursor.is_some() && next_start_at.is_some() {
            return Err(upstream("Jira returned ambiguous comments pagination"));
        }
        if let Some(next_cursor) = next_cursor {
            let value = next_cursor.0;
            if self
                .page_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.0 == value)
                || !self.seen_cursors.insert(value.clone())
            {
                return Err(upstream("Jira returned a comments cursor cycle"));
            }
            self.page_cursor = Some(PageCursor(value));
            return self.continue_or_limit();
        }
        self.page_cursor = None;

        if let Some(next_start_at) = next_start_at {
            if next_start_at <= self.start_at {
                return Err(upstream("Jira returned invalid comments startAt progress"));
            }
            if total.is_some_and(|total| next_start_at > total) {
                return Err(upstream("Jira returned comments startAt beyond total"));
            }
            self.start_at = next_start_at;
            return self.continue_or_limit();
        }

        if total.is_some() {
            return Err(upstream(
                "Jira comments pagination stopped before its total",
            ));
        }
        if page_comment_count == self.page_size {
            let next_start_at = self
                .start_at
                .checked_add(page_comment_count)
                .ok_or_else(|| upstream("Jira returned an invalid comments startAt"))?;
            if next_start_at <= self.start_at {
                return Err(upstream("Jira returned invalid comments startAt progress"));
            }
            self.start_at = next_start_at;
            return self.continue_or_limit();
        }
        Ok(CommentPageDecision::Complete)
    }

    pub(crate) fn finish(self) -> Vec<IssueComment> {
        self.comments
    }

    fn continue_or_limit(&self) -> Result<CommentPageDecision, ApplicationError> {
        if self.pages_fetched >= self.max_pages {
            return Err(upstream(
                "Jira comment pagination exceeded the safety limit",
            ));
        }
        Ok(CommentPageDecision::Continue)
    }
}

fn upstream(message: &'static str) -> ApplicationError {
    ApplicationError::new(crate::ErrorKind::Upstream, message)
}

#[cfg(test)]
mod tests {
    use jira_domain::IssueComment;
    use time::macros::datetime;

    use super::*;

    fn pagination(page_size: usize) -> CommentPagination {
        CommentPagination::new(page_size, 10, 100)
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

    fn page(
        comments: Vec<IssueComment>,
        start_at: usize,
        next_start_at: Option<usize>,
        next_cursor: Option<&str>,
        total: Option<usize>,
    ) -> IssueCommentsPage {
        IssueCommentsPage {
            comments,
            start_at,
            next_start_at,
            next_cursor: next_cursor.map(|cursor| PageCursor(cursor.into())),
            total,
        }
    }

    fn assert_error(result: Result<CommentPageDecision, ApplicationError>, message: &str) {
        let error = result.expect_err("pagination error");
        assert_eq!(error.kind(), crate::ErrorKind::Upstream);
        assert_eq!(error.message(), message);
    }

    #[test]
    fn rejects_both_continuation_mechanisms() {
        let mut state = pagination(10);

        let result = state.accept_page(page(Vec::new(), 0, Some(1), Some("next"), None));

        assert_error(result, "Jira returned ambiguous comments pagination");
    }

    #[test]
    fn rejects_reported_total_below_accumulated_comments() {
        let mut state = pagination(10);
        assert_eq!(
            state
                .accept_page(page(vec![comment("1")], 0, Some(1), None, Some(2)))
                .expect("first page"),
            CommentPageDecision::Continue
        );

        let result = state.accept_page(page(Vec::new(), 1, None, None, Some(0)));
        assert_error(
            result,
            "Jira returned more comments than its reported total",
        );
    }

    #[test]
    fn rejects_next_start_at_beyond_reported_total() {
        let mut state = pagination(10);
        let result = state.accept_page(page(vec![comment("1")], 0, Some(6), None, Some(5)));

        assert_error(result, "Jira returned comments startAt beyond total");
    }

    #[test]
    fn rejects_stopping_before_reported_total() {
        let mut state = pagination(10);
        let result = state.accept_page(page(vec![comment("1")], 0, None, None, Some(2)));

        assert_error(result, "Jira comments pagination stopped before its total");
    }

    #[test]
    fn full_page_without_metadata_uses_implicit_offset_continuation() {
        let mut state = pagination(1);
        assert_eq!(
            state
                .accept_page(page(vec![comment("1")], 0, None, None, None))
                .expect("full page"),
            CommentPageDecision::Continue
        );
        assert_eq!(state.start_at(), 1);
        assert_eq!(state.page_cursor(), None);

        assert_eq!(
            state
                .accept_page(page(Vec::new(), 1, None, None, None))
                .expect("final page"),
            CommentPageDecision::Complete
        );
        assert_eq!(state.finish().len(), 1);
    }
}
