use super::*;
use crate::sample_data::{sample_issues, sample_users};
use jira_domain::{
    AttachmentMetadata, IssueComment, IssueCommentAuthor, IssueDetail, IssueDetailCore,
};
use time::macros::datetime;

#[test]
fn maps_domain_identity_to_display_name() {
    let issues = sample_issues();
    let users = sample_users();
    let view = IssueViewModel::from_domain(&issues[0], &users);

    assert_eq!(view.key, "DESK-184");
    assert_eq!(view.assignee, "Amina Yusuf");
    assert_eq!(view.project, "Developer Experience");
}

#[test]
fn formats_timestamp_with_explicit_offset_and_date_rollover() {
    let value = datetime!(2026-08-17 23:45 UTC);
    let offset = UtcOffset::from_hms(2, 0, 0).expect("valid offset");

    assert_eq!(
        format_timestamp_with_offset(value, offset),
        "Aug 18, 2026 · 01:45 +02:00"
    );
}

#[test]
fn renders_non_whole_hour_local_offset() {
    let value = datetime!(2026-08-17 12:00 UTC);
    let offset = UtcOffset::from_hms(-3, -30, 0).expect("valid offset");

    assert_eq!(
        format_timestamp_with_offset(value, offset),
        "Aug 17, 2026 · 08:30 -03:30"
    );
}

#[test]
fn never_renders_an_unknown_account_id_as_a_display_label() {
    let mut issues = sample_issues();
    issues[0].assignee = Some(
        jira_domain::AccountId::new("unknown-account").expect("test account ID must be valid"),
    );
    let view = IssueViewModel::from_domain(&issues[0], &[]);

    assert_eq!(view.assignee, "Unknown user");
    assert!(!view.assignee.contains("unknown-account"));
}

#[test]
fn prefers_issue_embedded_display_names_for_assignee_and_reporter() {
    let mut issue = sample_issues().into_iter().next().expect("sample issue");
    issue.assignee_display_name = Some("Asha Patel".to_owned());
    issue.reporter_display_name = Some("Nina Smith".to_owned());

    let view = IssueViewModel::from_domain(&issue, &[]);

    assert_eq!(view.assignee, "Asha Patel");
    assert_eq!(view.reporter, "Nina Smith");
    assert!(!view.reporter.contains("marco"));
}

#[test]
fn rejects_embedded_account_ids_as_display_names() {
    let mut issue = sample_issues().into_iter().next().expect("sample issue");
    let assignee = issue.assignee.clone().expect("sample assignee");
    let reporter = issue.reporter.clone().expect("sample reporter");
    issue.assignee_display_name = Some(format!("  {assignee}  "));
    issue.reporter_display_name = Some(reporter.to_string());

    let view = IssueViewModel::from_domain(&issue, &[]);

    assert_eq!(view.assignee, "Unknown user");
    assert_eq!(view.reporter, "Unknown user");
    assert!(!view.assignee.contains(assignee.as_str()));
    assert!(!view.reporter.contains(reporter.as_str()));
}

#[test]
fn status_filters_match_categories_case_insensitively_and_keep_uncategorized_separate() {
    assert!(IssueStatusFilter::All.matches("anything"));
    assert!(IssueStatusFilter::ToDo.matches("TO DO"));
    assert!(IssueStatusFilter::InProgress.matches("in progress"));
    assert!(IssueStatusFilter::Done.matches("Done"));
    assert!(IssueStatusFilter::Uncategorized.matches(""));
    assert!(!IssueStatusFilter::Uncategorized.matches("In Review"));
    assert!(IssueStatusFilter::Uncategorized.matches("  "));
}

#[test]
fn status_selection_empty_means_all() {
    let selection = IssueStatusSelection::from_values([]);

    assert_eq!(selection, IssueStatusSelection::All);
    assert!(selection.matches("To Do"));
    assert!(selection.matches("In Progress"));
    assert!(selection.matches("Done"));
    assert!(selection.matches(""));
}

#[test]
fn status_selection_matches_one_category() {
    let selection = IssueStatusSelection::from_values([IssueStatusSelection::Done]);

    assert!(selection.matches("done"));
    assert!(!selection.matches("to do"));
    assert_eq!(selection.values(), vec![IssueStatusSelection::Done]);
    assert_eq!(selection.label(), "Done");
}

#[test]
fn status_selection_ors_multiple_categories_and_normalizes_duplicates() {
    let selection = IssueStatusSelection::from_values([
        IssueStatusSelection::Done,
        IssueStatusSelection::ToDo,
        IssueStatusSelection::Done,
    ]);

    assert!(selection.matches("Done"));
    assert!(selection.matches("To Do"));
    assert!(!selection.matches("In Progress"));
    assert_eq!(
        selection.values(),
        vec![IssueStatusSelection::ToDo, IssueStatusSelection::Done]
    );
    assert_eq!(selection.label(), "Multiple statuses");
}

#[test]
fn status_selection_keeps_uncategorized_explicit() {
    let selection = IssueStatusSelection::from_values([IssueStatusSelection::Uncategorized]);

    assert!(selection.matches(""));
    assert!(selection.matches("  "));
    assert!(!selection.matches("Done"));
    assert_eq!(selection.label(), "Uncategorized");
}

#[test]
fn filters_loaded_domain_issues_without_changing_their_display_mapping() {
    let issues = sample_issues();
    let users = sample_users();

    let views = issue_views_for_filter(&issues, &users, IssueStatusFilter::Done, "");

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].key, "DESK-163");
    assert_eq!(views[0].assignee, "Devon Park");
}

#[test]
fn searches_issue_key_and_summary_locally_and_composes_with_status() {
    let issues = sample_issues();
    let users = sample_users();

    assert_eq!(
        issue_views_for_filter(&issues, &users, IssueStatusFilter::All, "DESK-184")
            .iter()
            .map(|issue| issue.key.as_str())
            .collect::<Vec<_>>(),
        vec!["DESK-184"]
    );
    assert_eq!(
        issue_views_for_filter(&issues, &users, IssueStatusFilter::All, "desk-18").len(),
        1
    );
    assert_eq!(
        issue_views_for_filter(&issues, &users, IssueStatusFilter::All, "notifications").len(),
        1
    );
    assert_eq!(
        issue_views_for_filter(&issues, &users, IssueStatusFilter::Done, "desk")
            .iter()
            .map(|issue| issue.key.as_str())
            .collect::<Vec<_>>(),
        vec!["DESK-163"]
    );
    assert_eq!(
        issue_views_for_filter(&issues, &users, IssueStatusFilter::All, "   ").len(),
        issues.len()
    );
}

#[test]
fn normalizes_only_strict_issue_keys_for_future_remote_lookup() {
    assert_eq!(
        normalized_issue_key("  ix-123 ")
            .as_ref()
            .map(IssueKey::as_str),
        Some("IX-123")
    );
    assert!(normalized_issue_key("summary text").is_none());
    assert!(normalized_issue_key("IX-").is_none());
    assert!(normalized_issue_key("   ").is_none());
}

#[test]
fn maps_issue_detail_comments_and_attachment_metadata_for_display() {
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let detail = IssueDetail::new(
        IssueDetailCore::new(
            issue,
            vec![
                AttachmentMetadata::new("attachment-1", "report.txt", 1024, Some("text/plain"))
                    .expect("attachment"),
            ],
        )
        .expect("detail core"),
        vec![
            IssueComment::new(
                "comment-1",
                Some(
                    IssueCommentAuthor::new(
                        jira_domain::AccountId::new("account-1").expect("account"),
                        None::<String>,
                    )
                    .expect("author"),
                ),
                "A comment body",
                datetime!(2026-01-03 00:00 UTC),
                None,
                Vec::new(),
            )
            .expect("comment"),
        ],
    )
    .expect("detail");

    let view = IssueDetailViewModel::from_domain(&detail, &[]);

    assert_eq!(view.comments[0].author, "Unknown author");
    assert!(!view.comments[0].author.contains("account-1"));
    assert_eq!(view.comments[0].body, "A comment body");
    assert_eq!(view.attachments[0].filename, "report.txt");
    assert_eq!(view.attachments[0].id, "attachment-1");
    assert_eq!(view.attachments[0].size_bytes, 1024);
    assert_eq!(view.attachments[0].mime_type, "text/plain");
    assert_eq!(view.attachments[0].size, "1.0 KiB");
}

#[test]
fn maps_comment_author_to_authenticated_catalog_display_name() {
    let account = jira_domain::AccountId::new("account-1").expect("account");
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let detail = IssueDetail::new(
        IssueDetailCore::new(issue, Vec::new()).expect("detail core"),
        vec![
            IssueComment::new(
                "comment-1",
                Some(IssueCommentAuthor::new(account.clone(), Some("  ")).expect("author")),
                "A comment body",
                datetime!(2026-01-03 00:00 UTC),
                None,
                Vec::new(),
            )
            .expect("comment"),
        ],
    )
    .expect("detail");
    let user = User::new(
        detail.core.issue.site_id.clone(),
        account,
        "Asha",
        None,
        true,
    );

    let view = IssueDetailViewModel::from_domain(&detail, &[user]);

    assert_eq!(view.comments[0].author, "Asha");
    assert_ne!(view.comments[0].author, "account-1");
}

#[test]
fn maps_comment_embedded_display_name_without_a_user_catalog() {
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let detail = IssueDetail::new(
        IssueDetailCore::new(issue, Vec::new()).expect("detail core"),
        vec![
            IssueComment::new(
                "comment-1",
                Some(
                    IssueCommentAuthor::new(
                        jira_domain::AccountId::new("commenter-account").expect("account"),
                        Some("Asha Patel"),
                    )
                    .expect("author"),
                ),
                "A comment body",
                datetime!(2026-01-03 00:00 UTC),
                None,
                Vec::new(),
            )
            .expect("comment"),
        ],
    )
    .expect("detail");

    let view = IssueDetailViewModel::from_domain(&detail, &[]);

    assert_eq!(view.comments[0].author, "Asha Patel");
}

#[test]
fn maps_assignee_change_accounts_through_the_identity_directory() {
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let old = jira_domain::AccountId::new("old-account").expect("account");
    let new = jira_domain::AccountId::new("new-account").expect("account");
    let users = vec![
        User::new(issue.site_id.clone(), old.clone(), "Old Name", None, true),
        User::new(issue.site_id.clone(), new.clone(), "New Name", None, true),
    ];
    let event = UpdateEvent::new(
        jira_domain::EventId::new("event-assignee").expect("event"),
        issue.site_id.clone(),
        issue.id.clone(),
        issue.key.clone(),
        UpdateKind::AssigneeChanged {
            old: ChangeValue::Account(old),
            new: ChangeValue::Account(new),
        },
        issue.updated_at,
        Vec::new(),
    );

    let view = &update_groups_for_events(
        std::slice::from_ref(&event),
        std::slice::from_ref(&issue),
        &users,
    )[0]
    .events[0];

    assert_eq!(view.change, "Assignee: Old Name → New Name");
    assert!(!view.change.contains("account"));
}

#[test]
fn maps_comment_added_authors_without_exposing_account_ids() {
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let author = jira_domain::AccountId::new("amina").expect("account");
    let event = UpdateEvent::new(
        jira_domain::EventId::new("event-comment").expect("event"),
        issue.site_id.clone(),
        issue.id.clone(),
        issue.key.clone(),
        UpdateKind::CommentAdded {
            comment_id: "comment-1".to_owned(),
            author: Some(author),
            excerpt: "A useful update".to_owned(),
        },
        issue.updated_at,
        Vec::new(),
    );

    let view = &update_groups_for_events(
        std::slice::from_ref(&event),
        std::slice::from_ref(&issue),
        &sample_users(),
    )[0]
    .events[0];

    assert_eq!(view.change, "Amina Yusuf commented: A useful update");
    assert!(!view.change.contains("amina"));
}

#[test]
fn propagates_structured_issue_and_comment_content_with_plain_text_compatibility() {
    use jira_domain::{RichBlock, RichInline, RichTextDocument};

    let mut issue = sample_issues().into_iter().next().expect("sample issue");
    let rich_description = RichTextDocument::new(
        vec![RichBlock::Paragraph(vec![RichInline::Text {
            text: "Structured description".to_owned(),
            marks: Vec::new(),
        }])],
        false,
    );
    issue.rich_description = Some(rich_description.clone());
    issue.description_text = Some("Plain description fallback".to_owned());
    let mut comment = IssueComment::new(
        "comment-rich",
        None,
        "Plain comment fallback",
        datetime!(2026-01-03 00:00 UTC),
        None,
        Vec::new(),
    )
    .expect("comment");
    let rich_body = RichTextDocument::new(
        vec![RichBlock::Paragraph(vec![RichInline::Text {
            text: "Structured comment".to_owned(),
            marks: Vec::new(),
        }])],
        false,
    );
    comment.rich_body = Some(rich_body.clone());
    let detail = IssueDetail::new(
        IssueDetailCore::new(issue.clone(), Vec::new()).expect("detail core"),
        vec![comment],
    )
    .expect("detail");

    let issue_view = IssueViewModel::from_domain(&issue, &[]);
    let detail_view = IssueDetailViewModel::from_domain(&detail, &[]);

    assert_eq!(issue_view.description, "Plain description fallback");
    assert_eq!(issue_view.rich_description, Some(rich_description.clone()));
    assert_eq!(detail_view.description, "Plain description fallback");
    assert_eq!(detail_view.rich_description, Some(rich_description));
    assert_eq!(detail_view.comments[0].body, "Plain comment fallback");
    assert_eq!(detail_view.comments[0].rich_body, Some(rich_body));
}

#[test]
fn renders_generic_issue_activity_update_without_raw_enum_name() {
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let event = UpdateEvent::new(
        jira_domain::EventId::new("event-1").expect("event"),
        issue.site_id.clone(),
        issue.id.clone(),
        issue.key.clone(),
        UpdateKind::IssueUpdated,
        issue.updated_at,
        Vec::new(),
    );

    let view = &update_groups_for_events(
        std::slice::from_ref(&event),
        std::slice::from_ref(&issue),
        &[],
    )[0]
    .events[0];

    assert_eq!(view.issue_id, issue.id);
    assert_eq!(view.event_id.as_str(), "event-1");
    assert_eq!(view.change, "Issue activity changed");
}

#[test]
fn renders_changelog_field_change_as_exact_before_after_sentence() {
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let event = UpdateEvent::new(
        jira_domain::EventId::new("event-field").expect("event"),
        issue.site_id.clone(),
        issue.id.clone(),
        issue.key.clone(),
        UpdateKind::FieldChanged {
            field: "Labels".into(),
            old: ChangeValue::Text("old".into()),
            new: ChangeValue::Text("new".into()),
        },
        issue.updated_at,
        Vec::new(),
    );

    let view = &update_groups_for_events(
        std::slice::from_ref(&event),
        std::slice::from_ref(&issue),
        &[],
    )[0]
    .events[0];

    assert_eq!(view.change, "Labels: old → new");
}

fn test_update_event(
    event_id: &str,
    issue: &Issue,
    occurred_at: jira_domain::Timestamp,
) -> UpdateEvent {
    UpdateEvent::new(
        jira_domain::EventId::new(event_id).expect("event ID"),
        issue.site_id.clone(),
        issue.id.clone(),
        issue.key.clone(),
        UpdateKind::IssueUpdated,
        occurred_at,
        Vec::new(),
    )
}

#[test]
fn groups_adjacent_events_for_one_issue_into_one_ticket_card() {
    let issues = sample_issues();
    let first = &issues[0];
    let second = &issues[1];
    let events = vec![
        test_update_event("event-a1", first, datetime!(2026-08-16 10:00 UTC)),
        test_update_event("event-a2", first, datetime!(2026-08-16 09:00 UTC)),
        test_update_event("event-b1", second, datetime!(2026-08-16 08:00 UTC)),
    ];

    let groups = update_groups_for_events(&events, &issues, &sample_users());

    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].issue_id, first.id);
    assert_eq!(groups[0].issue_key, "DESK-184");
    assert_eq!(groups[0].issue_summary, first.summary);
    assert_eq!(
        groups[0]
            .events
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-a1", "event-a2"]
    );
    assert_eq!(
        groups[0].latest_occurred_at,
        format_timestamp(datetime!(2026-08-16 10:00 UTC))
    );
}

#[test]
fn groups_non_adjacent_events_without_reordering_groups_or_events() {
    let issues = sample_issues();
    let first = &issues[0];
    let second = &issues[1];
    let events = vec![
        test_update_event("event-a1", first, datetime!(2026-08-16 10:00 UTC)),
        test_update_event("event-b1", second, datetime!(2026-08-16 09:00 UTC)),
        test_update_event("event-a2", first, datetime!(2026-08-16 08:00 UTC)),
    ];

    let groups = update_groups_for_events(&events, &issues, &[]);

    assert_eq!(
        groups
            .iter()
            .map(|group| group.issue_key.as_str())
            .collect::<Vec<_>>(),
        vec!["DESK-184", "DESK-179"]
    );
    assert_eq!(
        groups[0]
            .events
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-a1", "event-a2"]
    );
    assert_eq!(
        groups[1]
            .events
            .iter()
            .map(|event| event.event_id.as_str())
            .collect::<Vec<_>>(),
        vec!["event-b1"]
    );
}

#[test]
fn groups_expose_all_event_ids_and_aggregate_mixed_read_states() {
    let issues = sample_issues();
    let issue = &issues[0];
    let mut read = test_update_event("event-read", issue, datetime!(2026-08-16 09:00 UTC));
    read.mark_read();
    let events = vec![
        test_update_event("event-unread-1", issue, datetime!(2026-08-16 10:00 UTC)),
        read,
        test_update_event("event-unread-2", issue, datetime!(2026-08-16 08:00 UTC)),
    ];

    let groups = update_groups_for_events(&events, &issues, &[]);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].unread_count, 2);
    assert!(groups[0].unread);
    assert_eq!(
        groups[0]
            .events
            .iter()
            .map(|event| (event.event_id.as_str(), event.unread))
            .collect::<Vec<_>>(),
        vec![
            ("event-unread-1", true),
            ("event-read", false),
            ("event-unread-2", true)
        ]
    );
}

#[test]
fn grouping_missing_issue_uses_safe_event_metadata_fallbacks() {
    let issue_id = jira_domain::IssueId::new("missing-issue").expect("issue ID");
    let site_id = jira_domain::JiraSiteId::new("sample-site").expect("site ID");
    let issue_key = IssueKey::new("DESK-999").expect("issue key");
    let secret_account = jira_domain::AccountId::new("secret-account").expect("account ID");
    let event = UpdateEvent::new(
        jira_domain::EventId::new("event-missing").expect("event ID"),
        site_id,
        issue_id,
        issue_key,
        UpdateKind::AssigneeChanged {
            old: ChangeValue::Account(secret_account.clone()),
            new: ChangeValue::Account(secret_account),
        },
        datetime!(2026-08-16 10:00 UTC),
        Vec::new(),
    );

    let groups = update_groups_for_events(&[event], &[], &[]);

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].issue_key, "DESK-999");
    assert_eq!(groups[0].issue_summary, "Issue no longer in this view");
    assert_eq!(
        groups[0].events[0].change,
        "Assignee: Unknown user → Unknown user"
    );
    assert!(!groups[0].events[0].change.contains("secret-account"));
}
