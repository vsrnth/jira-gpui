use std::collections::{HashMap, HashSet};

use jira_domain::{Issue, Timestamp};

use crate::{ApplicationError, CancellationToken, IssuePage, PageCursor};

const MAX_PAGE_SIZE: usize = 1_000;

pub(crate) fn validate_pagination_config(
    page_size: usize,
    max_pages: usize,
    error_message: &'static str,
) -> Result<(), ApplicationError> {
    if !(1..=MAX_PAGE_SIZE).contains(&page_size) || max_pages == 0 {
        return Err(ApplicationError::new(
            crate::ErrorKind::Internal,
            error_message,
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct IssuePagination {
    max_pages: usize,
    page_cursor: Option<PageCursor>,
    requested_cursors: HashSet<String>,
    issues: Vec<Issue>,
    positions: HashMap<jira_domain::IssueId, usize>,
    server_time: Option<Timestamp>,
    pages_fetched: usize,
    raw_issue_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PageFetchStats {
    pub(crate) page: usize,
    pub(crate) issue_count: usize,
    pub(crate) total_issue_count: usize,
}

#[derive(Debug)]
pub(crate) struct IssuePaginationOutcome {
    pub(crate) issues: Vec<Issue>,
    pub(crate) pages_fetched: usize,
    pub(crate) server_time: Option<Timestamp>,
}

impl IssuePagination {
    pub(crate) fn new(
        page_size: usize,
        max_pages: usize,
        error_message: &'static str,
    ) -> Result<Self, ApplicationError> {
        validate_pagination_config(page_size, max_pages, error_message)?;
        Ok(Self {
            max_pages,
            page_cursor: None,
            requested_cursors: HashSet::new(),
            issues: Vec::new(),
            positions: HashMap::new(),
            server_time: None,
            pages_fetched: 0,
            raw_issue_count: 0,
        })
    }

    pub(crate) fn prepare_request(
        &mut self,
        cancellation: &CancellationToken,
    ) -> Result<Option<PageCursor>, ApplicationError> {
        cancellation.check()?;
        if self.pages_fetched >= self.max_pages {
            return Err(ApplicationError::new(
                crate::ErrorKind::Upstream,
                "Jira pagination exceeded the configured safety limit",
            ));
        }

        if let Some(cursor) = &self.page_cursor {
            validate_cursor(cursor)?;
            if !self.requested_cursors.insert(cursor.0.clone()) {
                return Err(ApplicationError::new(
                    crate::ErrorKind::Upstream,
                    "Jira returned a pagination cursor cycle",
                ));
            }
        }
        Ok(self.page_cursor.clone())
    }

    pub(crate) fn accept_page(
        &mut self,
        page: IssuePage,
        cancellation: &CancellationToken,
    ) -> Result<PageFetchStats, ApplicationError> {
        cancellation.check()?;
        if let Some(cursor) = &page.next_cursor {
            validate_cursor(cursor)?;
            if self.requested_cursors.contains(&cursor.0) {
                return Err(ApplicationError::new(
                    crate::ErrorKind::Upstream,
                    "Jira returned a pagination cursor cycle",
                ));
            }
        }

        let issue_count = page.issues.len();
        self.pages_fetched += 1;
        self.raw_issue_count += issue_count;
        self.server_time = max_timestamp(self.server_time, page.server_time);
        for issue in page.issues {
            let id = issue.id.clone();
            if let Some(position) = self.positions.get(&id).copied() {
                // Preserve first-seen order while retaining the last snapshot.
                self.issues[position] = issue;
            } else {
                self.positions.insert(id, self.issues.len());
                self.issues.push(issue);
            }
        }
        self.page_cursor = page.next_cursor;

        Ok(PageFetchStats {
            page: self.pages_fetched,
            issue_count,
            total_issue_count: self.raw_issue_count,
        })
    }

    pub(crate) fn has_next_page(&self) -> bool {
        self.page_cursor.is_some()
    }

    pub(crate) fn finish(self) -> IssuePaginationOutcome {
        IssuePaginationOutcome {
            issues: self.issues,
            pages_fetched: self.pages_fetched,
            server_time: self.server_time,
        }
    }
}

fn validate_cursor(cursor: &PageCursor) -> Result<(), ApplicationError> {
    if cursor.0.trim().is_empty() {
        return Err(ApplicationError::new(
            crate::ErrorKind::Upstream,
            "Jira returned an empty pagination cursor",
        ));
    }
    Ok(())
}

fn max_timestamp(current: Option<Timestamp>, candidate: Option<Timestamp>) -> Option<Timestamp> {
    match (current, candidate) {
        (Some(current), Some(candidate)) => Some(current.max(candidate)),
        (None, candidate) => candidate,
        (current, None) => current,
    }
}
