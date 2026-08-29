use super::*;
use crate::adf::{
    MAX_ADF_NODES, MAX_LINK_HREF_BYTES, UNAVAILABLE_IMAGE, UNSUPPORTED_CONTENT, adf_to_plain_text,
    attachment_id_from_inline_card_url, count_file_media_references_inner, parse_adf,
};
use jira_domain::{
    HORIZONTAL_RULE_LABEL, PanelKind, RichBlock, RichInline, RichMark, RichTable, RichTextDocument,
};

#[test]
fn maps_bulk_changelog_with_bounded_items_and_rejects_bad_timestamps() {
    let response: JiraBulkChangelogResponse = serde_json::from_value(serde_json::json!({
        "issueChangeLogs": [{
            "issueId": "10001",
            "changeHistories": [{
                "id": "history-1",
                "created": 1786876200,
                "items": [{
                    "field": "Labels",
                    "fieldId": "labels",
                    "fromString": "old\nvalue",
                    "toString": "new"
                }]
            }]
        }],
        "nextPageToken": "next-1"
    }))
    .expect("valid changelog response");
    let page = IssueMapper
        .map_changelog_page(response)
        .expect("mapped changelog");
    assert_eq!(page.next_page_token.as_deref(), Some("next-1"));
    assert_eq!(page.changelogs[0].issue_id.as_str(), "10001");
    assert_eq!(
        page.changelogs[0].histories[0].items[0].field.as_deref(),
        Some("Labels")
    );
    assert_eq!(
        page.changelogs[0].histories[0].created.unix_timestamp(),
        1786876200
    );
    assert_eq!(
        page.changelogs[0].histories[0].items[0]
            .from_string
            .as_deref(),
        Some("old value")
    );

    let malformed: JiraBulkChangelogResponse = serde_json::from_value(serde_json::json!({
        "issueChangeLogs": [{
            "issueId": "10001",
            "changeHistories": [{
                "id": "history-1",
                "created": "not-a-timestamp",
                "items": []
            }]
        }]
    }))
    .expect("response shape");
    assert!(matches!(
        IssueMapper.map_changelog_page(malformed),
        Err(MappingError::InvalidTimestamp(_))
    ));
}

#[test]
fn maps_the_same_fixture_into_the_domain_model() {
    let page: EnhancedSearchPage =
        serde_json::from_str(include_str!("../tests/fixtures/enhanced-search-page.json")).unwrap();
    let site_id = JiraSiteId::new("site-123").unwrap();
    let mapped = IssueMapper.map_domain_page(site_id, page).unwrap();
    let issue = &mapped.issues[0];

    assert_eq!(issue.key.as_str(), "ENG-42");
    assert_eq!(issue.status.name, "In Progress");
    assert_eq!(issue.due_date.unwrap().to_string(), "2026-08-30");
    assert_eq!(issue.parent.as_ref().unwrap().key.as_str(), "ENG-1");
}

#[test]
fn preserves_valid_issue_assignee_and_reporter_display_names() {
    let mut page: EnhancedSearchPage =
        serde_json::from_str(include_str!("../tests/fixtures/enhanced-search-page.json")).unwrap();
    let mut issue = page.issues.pop().expect("issue fixture");
    issue.fields.reporter = Some(JiraUser {
        account_id: "712020:reporter".to_owned(),
        display_name: "Nina Smith".to_owned(),
        active: true,
        avatar_urls: Default::default(),
    });
    let mapped = IssueMapper
        .map_domain_issue(JiraSiteId::new("site-123").unwrap(), issue)
        .unwrap();

    assert_eq!(mapped.assignee_display_name.as_deref(), Some("Asha Patel"));
    assert_eq!(mapped.reporter_display_name.as_deref(), Some("Nina Smith"));
    assert_eq!(
        mapped.reporter.as_ref().unwrap().as_str(),
        "712020:reporter"
    );
}

#[test]
fn map_user_never_exposes_account_id_as_display_name() {
    let user = JiraUser {
        account_id: "account-1".to_owned(),
        display_name: " account-1 ".to_owned(),
        active: true,
        avatar_urls: Default::default(),
    };
    let mapped = IssueMapper
        .map_user(JiraSiteId::new("site").expect("site"), user)
        .expect("user should retain stable identity");

    assert_eq!(mapped.display_name, "Unknown user");
}

#[test]
fn maps_issue_detail_adf_and_attachment_metadata_without_content_urls() {
    let issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), issue)
        .unwrap();

    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some("First line\nSecond line\nA link label")
    );
    assert!(detail.issue.rich_description.is_some());
    assert_eq!(detail.attachments.len(), 1);
    assert_eq!(detail.attachments[0].id, "10001");
    assert_eq!(detail.attachments[0].size_bytes, 2048);
}

#[test]
fn maps_inline_cards_to_existing_attachment_metadata_without_retaining_urls() {
    let issue: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-inline-card.json"
    ))
    .unwrap();
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), issue)
        .unwrap();
    let document = detail.issue.rich_description.expect("rich description");

    let RichBlock::Paragraph(first) = &document.blocks[0] else {
        panic!("expected paragraph")
    };
    assert!(matches!(
        first.as_slice(),
        [RichInline::Text { text, .. }, RichInline::AttachmentCard(card)]
            if text == "Import file: "
                && card.attachment_id == "10002"
                && card.filename == "partner-enrollment.csv"
                && card.mime_type.as_deref() == Some("text/csv")
                && card.size_bytes == Some(4096)
    ));
    assert!(matches!(
        &document.blocks[1],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::AttachmentCard(card)] if card.attachment_id == "10002")
    ));
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some(
            "Import file: [attachment: partner-enrollment.csv]\n[attachment: partner-enrollment.csv]"
        )
    );
    let serialized = serde_json::to_string(&document).unwrap();
    assert!(!serialized.contains("secure/attachment/10002"));
    assert!(!serialized.contains("rest/api/3/attachment/content/10002"));
}

#[test]
fn maps_media_inline_to_the_only_issue_attachment_without_leaking_media_services_data() {
    let issue: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-media-inline.json"
    ))
    .unwrap();
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), issue)
        .unwrap();
    let document = detail.issue.rich_description.expect("rich description");

    let RichBlock::Paragraph(content) = &document.blocks[0] else {
        panic!("expected paragraph")
    };
    assert!(matches!(
        content.as_slice(),
        [RichInline::Text { text, .. }, RichInline::AttachmentCard(card)]
            if text == "Carrier file: "
                && card.attachment_id == "10004"
                && card.filename == "carrier-enrollment.csv"
                && card.mime_type.as_deref() == Some("text/csv")
                && card.size_bytes == Some(8192)
    ));
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some("Carrier file: [attachment: carrier-enrollment.csv]")
    );
    let serialized = serde_json::to_string(&document).unwrap();
    assert!(!serialized.contains("4478e39c-cf9b-41d1-ba92-68589487cd75"));
    assert!(!serialized.contains("MediaServicesSample"));
    assert!(!serialized.contains("jira.example.test"));
}

#[test]
fn leaves_media_inline_unsupported_with_multiple_issue_attachments() {
    let mut issue: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-media-inline.json"
    ))
    .unwrap();
    issue.fields.attachment.push(JiraAttachment {
        id: "10005".to_owned(),
        filename: "other.csv".to_owned(),
        size: 1024,
        mime_type: Some("text/csv".to_owned()),
    });

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), issue)
        .unwrap();
    let document = detail.issue.rich_description.expect("rich description");
    assert!(matches!(
        &document.blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::Text { .. }, RichInline::Placeholder { label }] if label == UNSUPPORTED_CONTENT)
    ));
}

#[test]
fn leaves_media_inline_unsupported_with_multiple_file_media_references() {
    let mut issue: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-media-inline.json"
    ))
    .unwrap();
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [
                {"type": "text", "text": "Carrier files: "},
                {"type": "mediaInline", "attrs": {
                    "id": "4478e39c-cf9b-41d1-ba92-68589487cd75",
                    "type": "file", "collection": "MediaServicesSample"
                }},
                {"type": "mediaInline", "attrs": {
                    "id": "another-media-services-id",
                    "type": "file", "collection": "MediaServicesSample"
                }}
            ]
        }]
    }));

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), issue)
        .unwrap();
    let document = detail.issue.rich_description.expect("rich description");
    assert!(matches!(
        &document.blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::Text { .. }, RichInline::Placeholder { label }, RichInline::Placeholder { label: second }] if label == UNSUPPORTED_CONTENT && second == UNSUPPORTED_CONTENT)
    ));
}

#[test]
fn wraps_a_top_level_media_inline_attachment_card_in_a_paragraph() {
    let mut issue: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-media-inline.json"
    ))
    .unwrap();
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaInline", "attrs": {
                "id": "4478e39c-cf9b-41d1-ba92-68589487cd75",
                "type": "file", "collection": "MediaServicesSample"
            }
        }]
    }));

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), issue)
        .unwrap();
    assert!(matches!(
        &detail.issue.rich_description.unwrap().blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::AttachmentCard(card)] if card.attachment_id == "10004")
    ));
}

#[test]
fn resolves_media_inline_by_unique_id_or_allowlisted_filename_before_one_to_one_fallback() {
    let mut by_id: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-media-inline.json"
    ))
    .unwrap();
    by_id.fields.attachment.push(JiraAttachment {
        id: "10005".to_owned(),
        filename: "other.csv".to_owned(),
        size: 1024,
        mime_type: Some("text/csv".to_owned()),
    });
    by_id.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaInline", "attrs": {
                "id": "10004", "type": "file", "collection": "MediaServicesSample"
            }
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), by_id)
        .unwrap();
    assert!(matches!(
        &detail.issue.rich_description.unwrap().blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::AttachmentCard(card)] if card.attachment_id == "10004")
    ));

    let mut by_filename: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-media-inline.json"
    ))
    .unwrap();
    by_filename.fields.attachment.push(JiraAttachment {
        id: "10005".to_owned(),
        filename: "other.csv".to_owned(),
        size: 1024,
        mime_type: Some("text/csv".to_owned()),
    });
    by_filename.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaInline", "attrs": {
                "id": "unmatched-media-services-id", "type": "file",
                "__fileName": "carrier-enrollment.csv",
                "collection": "MediaServicesSample"
            }
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), by_filename)
        .unwrap();
    assert!(matches!(
        &detail.issue.rich_description.unwrap().blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::AttachmentCard(card)] if card.attachment_id == "10004")
    ));
}

#[test]
fn keeps_media_inline_unsupported_without_issue_attachment_context() {
    let document = serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [{
                "type": "mediaInline", "attrs": {
                    "id": "4478e39c-cf9b-41d1-ba92-68589487cd75",
                    "type": "file", "collection": "MediaServicesSample"
                }
            }]
        }]
    });

    let parsed = parse_adf(&document).expect("valid ADF");
    assert_eq!(parsed.plain_text(), UNSUPPORTED_CONTENT);
    assert!(matches!(
        &parsed.blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::Placeholder { label }] if label == UNSUPPORTED_CONTENT)
    ));
}

#[test]
fn extracts_only_supported_attachment_url_forms() {
    assert_eq!(
        attachment_id_from_inline_card_url(
            "https://jira.example.test/secure/attachment/10002/partner-enrollment.csv"
        ),
        Some("10002".to_owned())
    );
    assert_eq!(
        attachment_id_from_inline_card_url(
            "https://jira.example.test/rest/api/3/attachment/content/10002"
        ),
        Some("10002".to_owned())
    );

    let oversized = format!(
        "https://jira.example.test/secure/attachment/10002/{}",
        "x".repeat(MAX_LINK_HREF_BYTES)
    );
    for url in [
        "https://jira.example.test/browse/ENG-43",
        "https://external.example/attachment/10002/file",
        "https://jira.example.test/secure/attachment/",
        "https://jira.example.test/rest/api/3/attachment/content/",
        "https://jira.example.test/rest/api/3/attachment/content/10002/file.csv",
        "/secure/attachment/10002/file.csv",
        "https://:secret@jira.example.test/secure/attachment/10002/file.csv",
        "https://user:secret@jira.example.test/secure/attachment/10002/file.csv",
        "javascript:attachment/10002",
        oversized.as_str(),
    ] {
        assert_eq!(attachment_id_from_inline_card_url(url), None, "{url}");
    }
}

#[test]
fn preserves_mixed_inline_card_sequence_order() {
    let mut issue: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-inline-card.json"
    ))
    .unwrap();
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [
                {"type": "text", "text": "before"},
                {"type": "inlineCard", "attrs": {
                    "url": "https://jira.example.test/secure/attachment/10002/file.csv"
                }},
                {"type": "hardBreak"},
                {"type": "text", "text": "after"}
            ]
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    let document = detail.issue.rich_description.expect("rich description");
    let RichBlock::Paragraph(content) = &document.blocks[0] else {
        panic!("expected paragraph")
    };
    assert!(matches!(
        content.as_slice(),
        [
            RichInline::Text { text: before, .. },
            RichInline::AttachmentCard(card),
            RichInline::HardBreak,
            RichInline::Text { text: after, .. }
        ] if before == "before" && card.attachment_id == "10002" && after == "after"
    ));
}

#[test]
fn keeps_unknown_trusted_attachment_id_unsupported() {
    let mut issue: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-inline-card.json"
    ))
    .unwrap();
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [{
                "type": "inlineCard", "attrs": {
                    "url": "https://jira.example.test/secure/attachment/99999/file.csv"
                }
            }]
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some(UNSUPPORTED_CONTENT)
    );
    assert!(matches!(
        &detail.issue.rich_description.unwrap().blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::Placeholder { label }] if label == UNSUPPORTED_CONTENT)
    ));
}

#[test]
fn maps_jira_browse_cards_but_keeps_external_hosts_unsupported() {
    let mut non_attachment: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-inline-card.json"
    ))
    .unwrap();
    non_attachment.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [{
                "type": "inlineCard",
                "attrs": {"url": "https://acme.atlassian.net/browse/ENG-43"}
            }]
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), non_attachment)
        .unwrap();
    assert_eq!(detail.issue.description_text.as_deref(), Some("ENG-43"));
    assert!(matches!(
        &detail.issue.rich_description.unwrap().blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::Text { text, marks }] if text == "ENG-43" && marks.is_empty())
    ));

    let mut external: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-inline-card.json"
    ))
    .unwrap();
    external.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [{
                "type": "inlineCard",
                "attrs": {"url": "https://jira.example.test/browse/ENG-43"}
            }]
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), external)
        .unwrap();
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some(UNSUPPORTED_CONTENT)
    );

    let mut ambiguous: JiraIssue = serde_json::from_str(include_str!(
        "../tests/fixtures/issue-detail-inline-card.json"
    ))
    .unwrap();
    ambiguous.fields.attachment.push(JiraAttachment {
        id: "10002".to_owned(),
        filename: "different.csv".to_owned(),
        size: 4096,
        mime_type: Some("text/csv".to_owned()),
    });
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), ambiguous)
        .unwrap();
    let document = detail.issue.rich_description.expect("rich description");
    assert!(matches!(
        &document.blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::Text { .. }, RichInline::Placeholder { label }] if label == UNSUPPORTED_CONTENT)
    ));
    assert!(
        detail
            .issue
            .description_text
            .unwrap()
            .contains(UNSUPPORTED_CONTENT)
    );
}

#[test]
fn maps_inline_cards_without_placeholder_punctuation_and_wraps_block_cards() {
    let inline = serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "paragraph", "content": [
                {"type": "text", "text": "Before "},
                {"type": "inlineCard", "attrs": {
                    "url": "https://acme.atlassian.net/browse/ENG-43"
                }},
                {"type": "text", "text": " after"}
            ]
        }]
    });
    let parsed = parse_adf(&inline).expect("valid inline-card document");
    assert_eq!(parsed.plain_text(), "Before ENG-43 after");
    assert!(matches!(
        &parsed.blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [
                RichInline::Text { text: before, .. },
                RichInline::Text { text: key, marks },
                RichInline::Text { text: after, .. }
            ] if before == "Before " && key == "ENG-43" && marks.is_empty() && after == " after")
    ));

    let block = serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "blockCard", "attrs": {
                "url": "https://acme.atlassian.net/browse/OPS-7"
            }
        }]
    });
    let parsed = parse_adf(&block).expect("valid block-card document");
    assert_eq!(parsed.plain_text(), "OPS-7");
    assert!(matches!(
        &parsed.blocks[0],
        RichBlock::Paragraph(content)
            if matches!(content.as_slice(), [RichInline::Text { text, marks }] if text == "OPS-7" && marks.is_empty())
    ));

    let uppercase_host = serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "inlineCard", "attrs": {
                "url": "https://ACME.ATLASSIAN.NET/browse/ENG-43"
            }
        }]
    });
    let parsed = parse_adf(&uppercase_host).expect("valid uppercase-host card document");
    assert_eq!(parsed.plain_text(), "ENG-43");
}

#[test]
fn rejects_noncanonical_or_non_url_jira_cards() {
    let rejected_urls = [
        "http://acme.atlassian.net/browse/ENG-43",
        "https://atlassian.net/browse/ENG-43",
        "https://acme.atlassian.net.evil.test/browse/ENG-43",
        "https://user:secret@acme.atlassian.net/browse/ENG-43",
        "https://acme.atlassian.net:443/browse/ENG-43",
        "https://acme.atlassian.net/browse/ENG-43?view=full",
        "https://acme.atlassian.net/browse/ENG-43#details",
        "https://acme.atlassian.net/browse/ENG-43/comments",
        "https://acme.atlassian.net/browse/eng-43",
    ];
    for url in rejected_urls {
        let document = serde_json::json!({
            "type": "doc", "version": 1, "content": [{
                "type": "paragraph", "content": [{
                    "type": "inlineCard", "attrs": {"url": url}
                }]
            }]
        });
        let parsed = parse_adf(&document).expect("valid ADF");
        assert_eq!(parsed.plain_text(), UNSUPPORTED_CONTENT, "{url}");
    }

    for attrs in [
        serde_json::json!({"data": {"url": "https://acme.atlassian.net/browse/ENG-43"}}),
        serde_json::json!({"datasource": {"url": "https://acme.atlassian.net/browse/ENG-43"}}),
    ] {
        let document = serde_json::json!({
            "type": "doc", "version": 1, "content": [{
                "type": "inlineCard", "attrs": attrs
            }]
        });
        let parsed = parse_adf(&document).expect("valid ADF");
        assert_eq!(parsed.plain_text(), UNSUPPORTED_CONTENT);
    }

    let card_document = |kind: &str, attrs: serde_json::Value| {
        let card = serde_json::json!({"type": kind, "attrs": attrs});
        if kind == "inlineCard" {
            serde_json::json!({
                "type": "doc", "version": 1, "content": [{
                    "type": "paragraph", "content": [card]
                }]
            })
        } else {
            serde_json::json!({
                "type": "doc", "version": 1, "content": [card]
            })
        }
    };
    for kind in ["inlineCard", "blockCard"] {
        for extra in ["data", "datasource"] {
            let mut attrs = serde_json::Map::new();
            attrs.insert(
                "url".to_owned(),
                serde_json::json!("https://acme.atlassian.net/browse/ENG-43"),
            );
            attrs.insert(
                extra.to_owned(),
                serde_json::json!({"url": "https://acme.atlassian.net/browse/ENG-43"}),
            );
            let document = card_document(kind, serde_json::Value::Object(attrs));
            let parsed = parse_adf(&document).expect("valid mixed-shape ADF");
            assert_eq!(parsed.plain_text(), UNSUPPORTED_CONTENT, "{kind} + {extra}");
        }
    }
}

#[test]
fn maps_media_only_description_by_unique_attachment_filename() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaSingle", "content": [{
                "type": "media", "attrs": {
                    "type": "file", "alt": "design.png", "width": 640, "height": 480
                }
            }]
        }]
    }));

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    let RichBlock::Image(image) = &detail.issue.rich_description.unwrap().blocks[0] else {
        panic!("expected an image block")
    };
    assert_eq!(image.attachment_id, "10001");
    assert_eq!(image.filename, "design.png");
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.alt_text.as_deref(), Some("design.png"));
    assert_eq!(image.width, Some(640));
    assert_eq!(image.height, Some(480));
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some("[image: design.png]")
    );
}

#[test]
fn normalizes_allowlisted_attachment_mime_before_projecting_image() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.attachment[0].mime_type = Some("  IMAGE/PNG  ".to_owned());
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaSingle", "content": [{
                "type": "media", "attrs": {"type": "file", "id": "10001"}
            }]
        }]
    }));

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    let RichBlock::Image(image) = &detail.issue.rich_description.unwrap().blocks[0] else {
        panic!("expected an image block")
    };
    assert_eq!(image.mime_type, "image/png");
}

#[test]
fn maps_one_file_media_to_the_only_allowed_image_attachment_without_alt_or_id() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaGroup", "content": [{
                "type": "media", "attrs": {"type": "file"}
            }]
        }]
    }));

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some("[image: design.png]")
    );
    assert!(matches!(
        detail.issue.rich_description.unwrap().blocks[0],
        RichBlock::Image(_)
    ));
}

#[test]
fn keeps_ambiguous_or_unsupported_media_visible_without_guessing() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.attachment.push(JiraAttachment {
        id: "10002".to_owned(),
        filename: "design.png".to_owned(),
        size: 1024,
        mime_type: Some("image/jpeg".to_owned()),
    });
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaSingle", "content": [{
                "type": "media", "attrs": {"type": "file", "alt": "design.png"}
            }]
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some("[Jira image unavailable]")
    );

    let mut svg: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    svg.fields.attachment[0].mime_type = Some("image/svg+xml".to_owned());
    svg.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaSingle", "content": [{
                "type": "media", "attrs": {"type": "file", "id": "10001"}
            }]
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), svg)
        .unwrap();
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some("[Jira image unavailable]")
    );
}

#[test]
fn leaves_multiple_unidentified_image_attachments_unavailable() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.attachment.push(JiraAttachment {
        id: "10002".to_owned(),
        filename: "other.jpg".to_owned(),
        size: 1024,
        mime_type: Some("image/jpeg".to_owned()),
    });
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaSingle", "content": [{
                "type": "media", "attrs": {"type": "file"}
            }]
        }]
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    assert_eq!(
        detail.issue.description_text.as_deref(),
        Some("[Jira image unavailable]")
    );
}

#[test]
fn exposes_bounded_candidates_for_real_media_services_uuid_without_guessing() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.attachment.push(JiraAttachment {
        id: "10002".to_owned(),
        filename: "other.jpg".to_owned(),
        size: 1024,
        mime_type: Some("image/jpeg".to_owned()),
    });
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaSingle", "attrs": {"layout": "center"}, "content": [{
                "type": "media", "attrs": {
                    "type": "file",
                    "id": "4478e39c-cf9b-41d1-ba92-68589487cd75",
                    "collection": "MediaServicesSample"
                }
            }]
        }]
    }));

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    let document = detail.issue.rich_description.unwrap();

    assert!(matches!(
        document.blocks.first(),
        Some(RichBlock::Placeholder { label }) if label == UNAVAILABLE_IMAGE
    ));
    assert_eq!(
        document
            .fallback_images
            .iter()
            .map(|image| image.attachment_id.as_str())
            .collect::<Vec<_>>(),
        vec!["10001", "10002"]
    );
}

#[test]
fn excludes_exactly_resolved_images_from_unresolved_candidate_gallery() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.attachment.push(JiraAttachment {
        id: "10002".to_owned(),
        filename: "other.jpg".to_owned(),
        size: 1024,
        mime_type: Some("image/jpeg".to_owned()),
    });
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [
            {"type": "mediaSingle", "content": [{
                "type": "media", "attrs": {"type": "file", "id": "10001"}
            }]},
            {"type": "mediaSingle", "content": [{
                "type": "media", "attrs": {
                    "type": "file",
                    "id": "4478e39c-cf9b-41d1-ba92-68589487cd75",
                    "collection": "MediaServicesSample"
                }
            }]}
        ]
    }));

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    let document = detail.issue.rich_description.unwrap();

    assert_eq!(document.fallback_images.len(), 1);
    assert_eq!(document.fallback_images[0].attachment_id, "10002");
}

#[test]
fn caps_candidates_and_rejects_unsupported_image_mimes() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.attachment[0].mime_type = Some("image/svg+xml".to_owned());
    for index in 0..20 {
        issue.fields.attachment.push(JiraAttachment {
            id: format!("{index}"),
            filename: format!("image-{index}.png"),
            size: 1024,
            mime_type: Some("image/png".to_owned()),
        });
    }
    issue.fields.description = Some(serde_json::json!({
        "type": "doc", "version": 1, "content": [{
            "type": "mediaSingle", "content": [{
                "type": "media", "attrs": {
                    "type": "file",
                    "id": "4478e39c-cf9b-41d1-ba92-68589487cd75",
                    "collection": "MediaServicesSample"
                }
            }]
        }]
    }));

    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue)
        .unwrap();
    let document = detail.issue.rich_description.unwrap();

    assert_eq!(
        document.fallback_images.len(),
        RichTextDocument::MAX_FALLBACK_IMAGES
    );
    assert!(
        document
            .fallback_images
            .iter()
            .all(|image| image.mime_type != "image/svg+xml")
    );
}

#[test]
fn old_serialized_rich_text_documents_still_deserialize() {
    let document: RichTextDocument = serde_json::from_value(serde_json::json!({
        "blocks": [{"Paragraph": [{"Text": {"text": "cached", "marks": []}}]}]
    }))
    .expect("old cached rich text should remain readable");
    assert_eq!(document.plain_text(), "cached");
    assert!(document.fallback_images.is_empty());
}

#[test]
fn bounds_wide_media_count_prepass_by_visited_adf_nodes() {
    let mut content = Vec::with_capacity(MAX_ADF_NODES * 2);
    for _ in 0..(MAX_ADF_NODES * 2) {
        content.push(serde_json::json!({"type": "paragraph", "content": []}));
    }
    let document = serde_json::json!({
        "type": "doc", "version": 1, "content": content
    });
    let mut count = 0;
    let mut visited = 0;
    count_file_media_references_inner(&document, 0, &mut count, &mut visited);

    assert_eq!(count, 0);
    assert_eq!(visited, MAX_ADF_NODES);
}

#[test]
fn drops_rich_content_when_issue_or_comment_adf_has_no_visible_text() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.description = Some(serde_json::json!({
        "type": "doc",
        "version": 1,
        "content": []
    }));
    let detail = IssueMapper
        .map_domain_issue_detail(JiraSiteId::new("site-123").unwrap(), issue)
        .unwrap();
    assert!(detail.issue.description_text.is_none());
    assert!(detail.issue.rich_description.is_none());

    let mut page: JiraCommentPage =
        serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
    page.comments[0].body = Some(serde_json::json!({
        "type": "doc",
        "version": 1,
        "content": [{"type": "paragraph", "content": []}]
    }));
    let mapped = IssueMapper.map_comment_page(page).unwrap();
    assert_eq!(mapped.comments[0].body, UNSUPPORTED_CONTENT);
    assert!(mapped.comments[0].rich_body.is_none());
}

#[test]
fn maps_comment_page_and_calculates_next_start_without_exposing_visibility_json() {
    let page: JiraCommentPage =
        serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
    let mapped = IssueMapper.map_comment_page(page).unwrap();

    assert_eq!(mapped.start_at, 0);
    assert_eq!(mapped.next_start_at, Some(2));
    assert_eq!(mapped.total, Some(3));
    assert_eq!(
        mapped.comments[0]
            .author
            .as_ref()
            .unwrap()
            .account_id
            .as_str(),
        "557058:commenter"
    );
    assert_eq!(
        mapped.comments[0]
            .author
            .as_ref()
            .unwrap()
            .display_name
            .as_deref(),
        Some("Asha")
    );
    assert_eq!(mapped.comments[0].body, "Looks good");
    assert!(mapped.comments[0].rich_body.is_some());
    assert!(
        serde_json::to_string(&mapped.comments[0])
            .unwrap()
            .contains("Looks good")
    );
}

#[test]
fn maps_one_created_comment_through_the_public_adapter_mapper() {
    let page: JiraCommentPage =
        serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
    let comment = page.comments.into_iter().next().expect("fixture comment");

    let mapped = IssueMapper.map_comment(comment).expect("mapped comment");

    assert_eq!(mapped.id.as_str(), "20001");
    assert_eq!(mapped.body, "Looks good");
    assert_eq!(
        mapped
            .author
            .as_ref()
            .and_then(|author| author.display_name.as_deref()),
        Some("Asha")
    );
}

#[test]
fn malformed_adf_is_ignored_safely_and_missing_comment_body_is_rejected() {
    assert_eq!(
        adf_to_plain_text(&serde_json::json!({"type":"mention"})),
        None
    );
    let page: JiraCommentPage = serde_json::from_value(serde_json::json!({
        "startAt": 0,
        "comments": [{
            "id": "1",
            "created": "2026-08-16T10:00:00.000+0000",
            "body": {"type":"doc"}
        }]
    }))
    .unwrap();
    assert!(matches!(
        IssueMapper.map_comment_page(page),
        Err(MappingError::MissingRequiredField("comment body"))
    ));
}

#[test]
fn rejects_non_document_adf_roots_and_versions() {
    assert!(parse_adf(&serde_json::json!({"type":"paragraph","content":[]})).is_none());
    assert!(
        parse_adf(&serde_json::json!({
            "type":"doc", "version": 2, "content": []
        }))
        .is_none()
    );
}

#[test]
fn maps_supported_rich_text_and_safe_placeholders() {
    let document = serde_json::json!({
        "type": "doc", "version": 1, "content": [
            {"type":"heading", "attrs":{"level":2}, "content":[
                {"type":"text", "text":"Title", "marks":[{"type":"strong"}]}
            ]},
            {"type":"paragraph", "content":[
                {"type":"text", "text":"styled", "marks":[
                    {"type":"em"}, {"type":"strike"}, {"type":"code"},
                    {"type":"link", "attrs":{"href":"javascript:alert(1)","title":"bad"}}
                ]},
                {"type":"hardBreak"},
                {"type":"mention", "attrs":{"id":"712020:secret","text":"@Asha"}}
            ]},
            {"type":"bulletList", "content":[{"type":"listItem", "content":[
                {"type":"paragraph", "content":[{"type":"text","text":"one"}]}
            ]}]},
            {"type":"orderedList", "attrs":{"order":3}, "content":[{"type":"listItem", "content":[
                {"type":"paragraph", "content":[{"type":"text","text":"two"}]}
            ]}]},
            {"type":"codeBlock", "attrs":{"language":"rust"}, "content":[
                {"type":"text", "text":"let answer = 42;"}
            ]},
            {"type":"blockquote", "content":[{"type":"paragraph", "content":[
                {"type":"text","text":"quoted"}
            ]}]},
            {"type":"panel", "attrs":{"panelType":"warning"}, "content":[
                {"type":"paragraph", "content":[{"type":"text","text":"careful"}]}
            ]},
            {"type":"mediaSingle", "content":[{"type":"media", "attrs":{"id":"private"}}]},
            {"type":"emoji", "attrs":{"shortName":":wave:"}}
        ]
    });

    let parsed = parse_adf(&document).expect("valid ADF");
    assert!(!parsed.truncated);
    assert!(matches!(
        parsed.blocks[0],
        RichBlock::Heading { level: 2, .. }
    ));
    assert!(matches!(parsed.blocks[2], RichBlock::BulletList(_)));
    assert!(matches!(
        parsed.blocks[3],
        RichBlock::OrderedList { order: 3, .. }
    ));
    assert!(matches!(parsed.blocks[4], RichBlock::CodeBlock { .. }));
    assert!(matches!(parsed.blocks[5], RichBlock::BlockQuote(_)));
    assert!(matches!(
        parsed.blocks[6],
        RichBlock::Panel {
            kind: PanelKind::Warning,
            ..
        }
    ));
    assert!(matches!(parsed.blocks[7], RichBlock::Placeholder { .. }));
    let plain = parsed.plain_text();
    assert!(plain.contains("@Asha"));
    assert!(!plain.contains("712020:secret"));
    assert!(!plain.contains("javascript:"));
}

#[test]
fn maps_heading_paragraph_and_rule_without_unsupported_content() {
    let document = serde_json::json!({
        "type": "doc", "version": 1, "content": [
            {"type":"heading", "attrs":{"level":2}, "content":[
                {"type":"text", "text":"Blocked by"}
            ]},
            {"type":"paragraph", "content":[
                {"type":"text", "text":"This issue is waiting on another ticket."}
            ]},
            {"type":"rule"}
        ]
    });

    let parsed = parse_adf(&document).expect("valid ADF");
    assert!(!parsed.truncated);
    assert!(matches!(
        parsed.blocks[2],
        RichBlock::Placeholder { ref label } if label == HORIZONTAL_RULE_LABEL
    ));
    assert_eq!(
        parsed.plain_text(),
        "Blocked by\nThis issue is waiting on another ticket."
    );
    assert!(!parsed.plain_text().contains(UNSUPPORTED_CONTENT));
}

#[test]
fn maps_acceptance_and_regression_tables_without_unsupported_content() {
    let document = serde_json::json!({
        "type": "doc", "version": 1, "content": [
            {"type":"heading", "attrs":{"level":2}, "content":[
                {"type":"text", "text":"Acceptance criteria"}
            ]},
            {"type":"table", "attrs":{"layout":"default"}, "content":[
                {"type":"tableRow", "content":[
                    {"type":"tableHeader", "content":[{"type":"paragraph", "content":[
                        {"type":"text", "text":"Given"}
                    ]}]},
                    {"type":"tableHeader", "content":[{"type":"paragraph", "content":[
                        {"type":"text", "text":"Then"}
                    ]}]}
                ]},
                {"type":"tableRow", "content":[
                    {"type":"tableCell", "content":[{"type":"paragraph", "content":[
                        {"type":"text", "text":"A Jira issue is open"}
                    ]}]},
                    {"type":"tableCell", "content":[{"type":"paragraph", "content":[
                        {"type":"text", "text":"The detail view is visible"}
                    ]}]}
                ]}
            ]},
            {"type":"heading", "attrs":{"level":2}, "content":[
                {"type":"text", "text":"Regression"}
            ]},
            {"type":"table", "content":[
                {"type":"tableRow", "content":[
                    {"type":"tableCell", "content":[{"type":"paragraph", "content":[
                        {"type":"text", "text":"Refresh"}
                    ]}]},
                    {"type":"tableCell", "content":[{"type":"paragraph", "content":[
                        {"type":"text", "text":"No unsupported content"}
                    ]}]}
                ]}
            ]}
        ]
    });

    let parsed = parse_adf(&document).expect("valid ADF");
    assert!(!parsed.truncated);
    assert!(matches!(
        &parsed.blocks[1],
        RichBlock::Table(RichTable { rows }) if rows.len() == 2
    ));
    assert!(matches!(
        &parsed.blocks[3],
        RichBlock::Table(RichTable { rows }) if rows.len() == 1
    ));
    let plain = parsed.plain_text();
    assert!(plain.contains("Acceptance criteria"));
    assert!(plain.contains("The detail view is visible"));
    assert!(plain.contains("Regression"));
    assert!(plain.contains("No unsupported content"));
    assert!(!plain.contains(UNSUPPORTED_CONTENT));
}

#[test]
fn oversized_table_rows_and_cells_are_bounded_and_marked_truncated() {
    let rows = (0..65)
        .map(|_| {
            serde_json::json!({"type":"tableRow", "content":[
                {"type":"tableCell", "content":[{"type":"paragraph", "content":[
                    {"type":"text", "text":"bounded"}
                ]}]}
            ]})
        })
        .collect::<Vec<_>>();
    let document = serde_json::json!({
        "type":"doc", "version":1, "content":[
            {"type":"table", "content":rows}
        ]
    });
    let parsed = parse_adf(&document).expect("valid root");
    assert!(parsed.truncated);
    let RichBlock::Table(table) = &parsed.blocks[0] else {
        panic!("expected table payload")
    };
    assert_eq!(table.rows.len(), 64);

    let cells = (0..33)
        .map(|_| {
            serde_json::json!({"type":"tableCell", "content":[{"type":"paragraph", "content":[
                {"type":"text", "text":"bounded"}
            ]}]})
        })
        .collect::<Vec<_>>();
    let document = serde_json::json!({
        "type":"doc", "version":1, "content":[
            {"type":"table", "content":[{"type":"tableRow", "content":cells}]}
        ]
    });
    let parsed = parse_adf(&document).expect("valid root");
    assert!(parsed.truncated);
    let RichBlock::Table(table) = &parsed.blocks[0] else {
        panic!("expected table payload")
    };
    assert_eq!(table.rows[0].cells.len(), 32);
}

#[test]
fn malformed_table_shapes_are_explicit_and_bounded() {
    let malformed_tables = [
        serde_json::json!({"type":"table", "content": []}),
        serde_json::json!({"type":"table", "content":[null]}),
        serde_json::json!({"type":"table", "content":[{"type":"tableRow", "content":[]}]}),
        serde_json::json!({"type":"table", "content":[{"type":"tableRow", "content":[null]}]}),
        serde_json::json!({"type":"table", "content":[{"type":"tableRow", "content":[{"type":"tableCell", "content":[]}]}]}),
    ];

    for table in malformed_tables {
        let document = serde_json::json!({
            "type":"doc", "version":1, "content":[table]
        });
        let parsed = parse_adf(&document).expect("valid root");
        assert!(parsed.truncated);
        assert!(matches!(
            parsed.blocks.as_slice(),
            [RichBlock::Placeholder { label }] if label == UNSUPPORTED_CONTENT
        ));
    }
}

#[test]
fn malformed_rule_shapes_remain_bounded_placeholders() {
    for rule in [
        serde_json::json!({"type":"rule", "content": []}),
        serde_json::json!({"type":"rule", "attrs": {"style":"unsafe"}}),
        serde_json::json!({"type":"rule", "attrs": "invalid"}),
    ] {
        let document = serde_json::json!({
            "type": "doc", "version": 1, "content": [rule]
        });
        let parsed = parse_adf(&document).expect("valid root");
        assert!(parsed.truncated);
        assert!(matches!(
            parsed.blocks.as_slice(),
            [RichBlock::Placeholder { label }] if label == UNSUPPORTED_CONTENT
        ));
    }
}

#[test]
fn id_only_mentions_never_project_account_ids() {
    let document = serde_json::json!({
        "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
            {"type":"mention", "attrs":{"id":"712020:secret"}}
        ]}]
    });
    let parsed = parse_adf(&document).expect("valid ADF");
    assert_eq!(parsed.plain_text(), "Mentioned user");
    assert!(!parsed.plain_text().contains("712020:secret"));
}

#[test]
fn mention_labels_equal_to_account_ids_use_a_safe_fallback() {
    let document = serde_json::json!({
        "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
            {"type":"mention", "attrs":{"id":" 712020:secret ", "text":"712020:secret"}},
            {"type":"mention", "attrs":{"id":"account-2", "displayName":"account-2"}}
        ]}]
    });
    let parsed = parse_adf(&document).expect("valid root");
    assert_eq!(parsed.plain_text(), "Mentioned userMentioned user");
    assert!(!parsed.plain_text().contains("712020:secret"));
    assert!(!parsed.plain_text().contains("account-2"));
}

#[test]
fn idless_opaque_mention_labels_use_a_safe_fallback() {
    let document = serde_json::json!({
        "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
            {"type":"mention", "attrs":{"displayText":"712020:d98ae2"}},
            {"type":"mention", "attrs":{"displayText":"Team: Platform"}}
        ]}]
    });
    let parsed = parse_adf(&document).expect("valid root");
    assert_eq!(parsed.plain_text(), "Mentioned userTeam: Platform");
    assert!(!parsed.plain_text().contains("712020:d98ae2"));
}

#[test]
fn malformed_and_oversized_links_drop_only_the_mark() {
    let oversized_href = format!("https://example.test/{}", "x".repeat(2_048));
    let oversized_title = "t".repeat(513);
    let document = serde_json::json!({
        "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
            {"type":"text", "text":"bad-scheme", "marks":[{"type":"link", "attrs":{"href":"javascript:alert(1)"}}]},
            {"type":"text", "text":"credentials", "marks":[{"type":"link", "attrs":{"href":"https://user:pass@example.test"}}]},
            {"type":"text", "text":"bad-authority", "marks":[{"type":"link", "attrs":{"href":"https://example.test:bad"}}]},
            {"type":"text", "text":"long-href", "marks":[{"type":"link", "attrs":{"href":oversized_href}}]},
            {"type":"text", "text":"long-title", "marks":[{"type":"link", "attrs":{"href":"https://example.test", "title":oversized_title}}]}
        ]}]
    });
    let parsed = parse_adf(&document).expect("valid root");
    assert_eq!(
        parsed.plain_text(),
        "bad-schemecredentialsbad-authoritylong-hreflong-title"
    );
    let RichBlock::Paragraph(inlines) = &parsed.blocks[0] else {
        panic!("expected paragraph")
    };
    assert!(inlines.iter().all(|inline| matches!(inline, RichInline::Text { marks, .. } if marks.is_empty() || marks.iter().all(|mark| matches!(mark, RichMark::Link { title: None, .. })) )));
}

#[test]
fn malformed_recognized_nodes_emit_placeholders_and_truncation() {
    let document = serde_json::json!({
        "type":"doc", "version":1, "content":[
            {"type":"paragraph"},
            {"type":"heading", "content": [{"type":"text"}]},
            {"type":"bulletList", "content": [{"type":"listItem"}]}
        ]
    });
    let parsed = parse_adf(&document).expect("valid root");
    assert!(parsed.truncated);
    assert!(parsed.plain_text().contains("[unsupported Jira content]"));
}

#[test]
fn deep_and_large_adf_is_deterministically_truncated() {
    let mut nested = serde_json::json!({
        "type":"paragraph", "content":[{"type":"text", "text":"deep"}]
    });
    for _ in 0..70 {
        nested = serde_json::json!({"type":"blockquote", "content":[nested]});
    }
    let deep = serde_json::json!({"type":"doc", "version":1, "content":[nested]});
    let parsed = parse_adf(&deep).expect("valid root");
    assert!(parsed.truncated);
    assert!(parsed.plain_text().contains("content truncated"));

    let many = (0..10_100)
        .map(|_| serde_json::json!({"type":"paragraph","content":[]}))
        .collect::<Vec<_>>();
    let parsed = parse_adf(&serde_json::json!({
        "type":"doc", "version":1, "content":many
    }))
    .expect("valid root");
    assert!(parsed.truncated);
    assert!(parsed.plain_text().contains("content truncated"));
}

#[test]
fn text_limit_is_bounded_without_splitting_utf8() {
    let text = "é".repeat(600_000);
    let document = serde_json::json!({
        "type":"doc", "version":1, "content":[{"type":"paragraph","content":[
            {"type":"text", "text":text}
        ]}]
    });
    let parsed = parse_adf(&document).expect("valid root");
    assert!(parsed.truncated);
    assert!(parsed.plain_text().contains("content truncated"));
    assert!(
        parsed
            .plain_text()
            .is_char_boundary(parsed.plain_text().len())
    );
    assert!(parsed.plain_text().len() <= 1_000_100);
}

#[test]
fn blank_missing_and_oversized_author_names_fall_back_without_dropping_account_id() {
    let mut blank: JiraCommentPage =
        serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
    blank.comments[0]
        .author
        .as_mut()
        .expect("fixture author")
        .display_name = "   ".to_owned();
    let mapped = IssueMapper.map_comment_page(blank).unwrap();
    let author = mapped.comments[0].author.as_ref().expect("author");
    assert_eq!(author.account_id.as_str(), "557058:commenter");
    assert_eq!(author.display_name, None);

    let mut oversized: JiraCommentPage =
        serde_json::from_str(include_str!("../tests/fixtures/comments-page.json")).unwrap();
    oversized.comments[0]
        .author
        .as_mut()
        .expect("fixture author")
        .display_name = "x".repeat(256);
    let mapped = IssueMapper.map_comment_page(oversized).unwrap();
    assert_eq!(
        mapped.comments[0]
            .author
            .as_ref()
            .expect("author")
            .display_name,
        None
    );

    let missing: JiraCommentPage = serde_json::from_value(serde_json::json!({
        "startAt": 0,
        "comments": [{
            "id": "1",
            "author": {"accountId": "account-without-name"},
            "created": "2026-08-16T10:00:00.000+0000",
            "body": {"type":"doc","content":[{"type":"paragraph","content":[{"type":"text","text":"body"}]}]}
        }]
    })).unwrap();
    let mapped = IssueMapper.map_comment_page(missing).unwrap();
    let author = mapped.comments[0].author.as_ref().expect("author");
    assert_eq!(author.account_id.as_str(), "account-without-name");
    assert_eq!(author.display_name, None);
}

#[test]
fn malformed_attachment_metadata_is_rejected_without_following_remote_urls() {
    let mut issue: JiraIssue =
        serde_json::from_str(include_str!("../tests/fixtures/issue-detail.json")).unwrap();
    issue.fields.attachment[0].id.clear();
    assert!(matches!(
        IssueMapper.map_domain_issue_detail(JiraSiteId::new("site").unwrap(), issue),
        Err(MappingError::InvalidDomainValue(_))
    ));
}

#[test]
fn comment_adf_preserves_mention_display_text_and_uses_a_safe_placeholder_for_media_only() {
    let mention = serde_json::json!({
        "type": "doc",
        "version": 1,
        "content": [{
            "type": "paragraph",
            "content": [{
                "type": "mention",
                "attrs": {"displayText": "@Asha"}
            }, {"type": "hardBreak"}, {
                "type": "text",
                "text": "reviewed"
            }]
        }]
    });
    assert_eq!(
        adf_comment_text(&mention).as_deref(),
        Some("@Asha\nreviewed")
    );

    let media_only = serde_json::json!({
        "type": "doc",
        "version": 1,
        "content": [{"type": "media", "attrs": {"id": "private"}}]
    });
    assert_eq!(
        adf_comment_text(&media_only).as_deref(),
        Some("[unsupported Jira content]")
    );
}

#[test]
fn adf_keeps_inline_children_contiguous_and_separates_block_children() {
    let document = serde_json::json!({
        "type": "doc",
        "version": 1,
        "content": [
            {"type": "paragraph", "content": [
                {"type": "text", "text": "Hello "},
                {"type": "text", "text": "linked", "marks": [{
                    "type": "link", "attrs": {"href": "https://example.invalid"}
                }]},
                {"type": "mention", "attrs": {"displayText": "@Asha"}},
                {"type": "text", "text": "!"}
            ]},
            {"type": "paragraph", "content": [
                {"type": "text", "text": "Next"}
            ]}
        ]
    });

    assert_eq!(
        adf_to_plain_text(&document).as_deref(),
        Some("Hello linked@Asha!\nNext")
    );
}
