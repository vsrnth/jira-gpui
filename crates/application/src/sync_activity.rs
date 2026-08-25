use std::collections::HashSet;

use jira_domain::{
    AccountId, Issue, IssueComment, IssueId, JiraSiteId, Timestamp, UpdateEvent, UpdateKind,
    UserSetId,
};

use crate::{
    ApplicationError, CancellationToken, ErrorKind, IssueChangelogRequest, JiraIssueActivityPort,
    RecentIssueCommentsRequest, enrich_with_changelog,
};

const MAX_CHANGELOG_ISSUES_PER_REQUEST: usize = 1_000;
pub(crate) const MAX_RECENT_ISSUE_COMMENTS: usize = 100;
const MAX_COMMENT_EXCERPT_BYTES: usize = 280;

/// Application-owned input boundary for enriching a diff with activity that is read separately
/// from issue snapshots. It carries application values only; the activity port remains transport
/// neutral.
pub(crate) struct SyncActivityRequest<'a> {
    pub(crate) existing: &'a [Issue],
    pub(crate) incoming: &'a [Issue],
    pub(crate) site_id: &'a JiraSiteId,
    pub(crate) user_set_id: &'a UserSetId,
    pub(crate) notification_assignees: Option<&'a [AccountId]>,
    pub(crate) cancellation: &'a CancellationToken,
}

pub(crate) struct SyncActivityEnricher;

impl SyncActivityEnricher {
    pub(crate) async fn enrich(
        activity: &dyn JiraIssueActivityPort,
        mut update_events: Vec<UpdateEvent>,
        request: SyncActivityRequest<'_>,
    ) -> Result<Vec<UpdateEvent>, ApplicationError> {
        let changelog_issue_ids = changed_issue_ids(request.existing, request.incoming);
        if !changelog_issue_ids.is_empty() {
            for issue_ids in changelog_issue_ids.chunks(MAX_CHANGELOG_ISSUES_PER_REQUEST) {
                request.cancellation.check()?;
                match activity
                    .fetch_issue_changelog(
                        &IssueChangelogRequest {
                            site_id: request.site_id.clone(),
                            issue_ids: issue_ids.to_vec(),
                        },
                        request.cancellation,
                    )
                    .await
                {
                    Ok(page) => {
                        update_events = enrich_with_changelog(
                            update_events,
                            request.existing,
                            request.incoming,
                            &page,
                            request.site_id,
                            request.user_set_id,
                        );
                    }
                    Err(error) if error.kind() == ErrorKind::Cancelled => {
                        return Err(error);
                    }
                    // Changelog enrichment is best effort. Keep the generic snapshot event when
                    // an optional activity read is unavailable or otherwise fails.
                    Err(_) => break,
                }
            }
        }

        let mention_events = mention_events(
            activity,
            request.existing,
            request.incoming,
            request.site_id,
            request.user_set_id,
            request.notification_assignees,
            request.cancellation,
        )
        .await?;
        let mentioned_issue_ids = mention_events
            .iter()
            .map(|event| event.issue_id.clone())
            .collect::<HashSet<_>>();
        // A generic snapshot fallback represents the same activity as a direct mention. Remove
        // only that fallback for the affected issue; specific field/changelog events remain useful.
        update_events.retain(|event| {
            !(mentioned_issue_ids.contains(&event.issue_id)
                && matches!(event.kind, UpdateKind::IssueUpdated))
        });
        update_events.extend(mention_events);
        Ok(update_events)
    }
}

async fn mention_events(
    activity: &dyn JiraIssueActivityPort,
    existing: &[Issue],
    incoming: &[Issue],
    site_id: &JiraSiteId,
    user_set_id: &UserSetId,
    notification_assignees: Option<&[AccountId]>,
    cancellation: &CancellationToken,
) -> Result<Vec<UpdateEvent>, ApplicationError> {
    let Some(notification_assignees) = notification_assignees else {
        return Ok(Vec::new());
    };
    if notification_assignees.is_empty() {
        return Ok(Vec::new());
    }

    let mut events = Vec::new();
    for (old_issue, new_issue) in changed_issue_pairs(existing, incoming) {
        cancellation.check()?;
        let comments = match activity
            .fetch_recent_issue_comments(
                &RecentIssueCommentsRequest {
                    site_id: site_id.clone(),
                    issue_id: new_issue.id.clone(),
                    limit: MAX_RECENT_ISSUE_COMMENTS,
                },
                cancellation,
            )
            .await
        {
            Ok(comments) => comments,
            Err(error) if error.kind() == ErrorKind::Cancelled => {
                return Err(error);
            }
            // A gateway that predates the optional read, or a restricted/deleted issue, must not
            // prevent the rest of the sync from committing.
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::Authorization | ErrorKind::NotFound | ErrorKind::Internal
                ) =>
            {
                continue;
            }
            // Authentication, transport, rate-limit, and upstream failures are retryable. Do not
            // advance the sync cursor after one of these.
            Err(error) => return Err(error),
        };

        let mut seen_comments = HashSet::new();
        for comment in comments {
            cancellation.check()?;
            if !seen_comments.insert(comment.id.clone()) {
                continue;
            }
            let Some(activity_at) =
                comment_activity(&comment, old_issue.updated_at, new_issue.updated_at)
            else {
                continue;
            };
            let Some(rich_body) = comment.rich_body.as_ref() else {
                continue;
            };
            if !notification_assignees
                .iter()
                .any(|account| rich_body.mentions_account(account))
            {
                continue;
            }
            let kind = UpdateKind::CommentAdded {
                comment_id: comment.id.clone(),
                author: comment
                    .author
                    .as_ref()
                    .map(|author| author.account_id.clone()),
                excerpt: comment_excerpt(&comment),
            };
            events.push(UpdateEvent::new(
                crate::event_identity::comment_event_id(
                    site_id,
                    &new_issue.id,
                    comment.id.as_str(),
                    activity_at,
                ),
                site_id.clone(),
                new_issue.id.clone(),
                new_issue.key.clone(),
                kind,
                activity_at,
                vec![user_set_id.clone()],
            ));
        }
    }
    Ok(events)
}

fn changed_issue_ids(existing: &[Issue], incoming: &[Issue]) -> Vec<IssueId> {
    let mut ids = changed_issue_pairs(existing, incoming)
        .into_iter()
        .map(|(_, issue)| issue.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

fn changed_issue_pairs<'a>(
    existing: &'a [Issue],
    incoming: &'a [Issue],
) -> Vec<(&'a Issue, &'a Issue)> {
    let old = existing
        .iter()
        .map(|issue| (&issue.id, issue))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut pairs = incoming
        .iter()
        .filter_map(|issue| {
            let previous = old.get(&issue.id).copied()?;
            (previous.lifecycle == jira_domain::IssueLifecycle::Present
                && issue.lifecycle == jira_domain::IssueLifecycle::Present
                && previous.updated_at != issue.updated_at)
                .then_some((previous, issue))
        })
        .collect::<Vec<_>>();
    pairs.sort_by(|(_, left), (_, right)| left.id.cmp(&right.id));
    pairs
}

fn comment_activity(
    comment: &IssueComment,
    old_updated_at: Timestamp,
    new_updated_at: Timestamp,
) -> Option<Timestamp> {
    let in_window =
        |timestamp: Timestamp| timestamp > old_updated_at && timestamp <= new_updated_at;
    comment
        .updated_at
        .filter(|timestamp| in_window(*timestamp))
        .or_else(|| in_window(comment.created_at).then_some(comment.created_at))
}

fn comment_excerpt(comment: &IssueComment) -> String {
    let source = comment
        .rich_body
        .as_ref()
        .map(|body| body.plain_text())
        .unwrap_or_else(|| comment.body.clone());
    let mut excerpt = String::with_capacity(source.len().min(MAX_COMMENT_EXCERPT_BYTES));
    for character in source.chars() {
        if character.is_control() {
            if (character == '\n' || character == '\r' || character == '\t')
                && !excerpt.ends_with(' ')
            {
                excerpt.push(' ');
            }
        } else {
            excerpt.push(character);
        }
        if excerpt.len() >= MAX_COMMENT_EXCERPT_BYTES {
            break;
        }
    }
    excerpt.truncate(excerpt.floor_char_boundary(MAX_COMMENT_EXCERPT_BYTES));
    excerpt.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jira_domain::IssueComment;
    use time::macros::datetime;

    #[test]
    fn comment_activity_uses_update_then_creation_within_sync_window() {
        let old = datetime!(2026-08-16 10:00 UTC);
        let new = datetime!(2026-08-16 12:00 UTC);
        let comment = IssueComment::new(
            "comment-1",
            None,
            "body",
            datetime!(2026-08-16 11:00 UTC),
            Some(datetime!(2026-08-16 11:30 UTC)),
            Vec::new(),
        )
        .expect("comment");

        assert_eq!(comment_activity(&comment, old, new), comment.updated_at);
    }

    #[test]
    fn comment_activity_rejects_comments_outside_the_sync_window() {
        let comment = IssueComment::new(
            "comment-1",
            None,
            "body",
            datetime!(2026-08-16 09:00 UTC),
            Some(datetime!(2026-08-16 09:30 UTC)),
            Vec::new(),
        )
        .expect("comment");

        assert_eq!(
            comment_activity(
                &comment,
                datetime!(2026-08-16 10:00 UTC),
                datetime!(2026-08-16 12:00 UTC)
            ),
            None
        );
    }
}
