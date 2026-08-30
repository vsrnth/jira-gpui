use std::collections::HashSet;

use jira_domain::{RichBlock, RichImage, RichTextDocument};

use super::policy;
use crate::{
    diagnostics::{DiagnosticsSink, ImageSource, ImageStateReason},
    presentation::IssueDetailViewModel,
    rich_text_view::{RichImageRenderState, RichImageRenderStates},
};

pub(crate) const MAX_RICH_IMAGES: usize = policy::MAX_RICH_IMAGES;

/// Traverse resolved ADF images before fallback candidates, retaining the first
/// occurrence of each attachment identity (including one empty ID) and applying
/// one global cap.
#[cfg(test)]
fn collect_rich_images(document: &RichTextDocument) -> Vec<RichImage> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for block in &document.blocks {
        collect_rich_images_from_block(block, &mut seen, &mut images);
        if images.len() == MAX_RICH_IMAGES {
            break;
        }
    }
    for image in &document.fallback_images {
        if seen.insert(image.attachment_id.clone()) {
            images.push(image.clone());
        }
        if images.len() == MAX_RICH_IMAGES {
            break;
        }
    }
    images
}

#[cfg(test)]
fn collect_rich_images_from_block(
    block: &RichBlock,
    seen: &mut HashSet<String>,
    images: &mut Vec<RichImage>,
) {
    if images.len() == MAX_RICH_IMAGES {
        return;
    }
    match block {
        RichBlock::Image(image) => {
            if seen.insert(image.attachment_id.clone()) {
                images.push(image.clone());
            }
        }
        RichBlock::BlockQuote(children)
        | RichBlock::Panel {
            content: children, ..
        }
        | RichBlock::Expand {
            content: children, ..
        }
        | RichBlock::NestedExpand {
            content: children, ..
        } => {
            for child in children {
                collect_rich_images_from_block(child, seen, images);
                if images.len() == MAX_RICH_IMAGES {
                    break;
                }
            }
        }
        RichBlock::BulletList(items) | RichBlock::OrderedList { items, .. } => {
            for item in items {
                for child in &item.blocks {
                    collect_rich_images_from_block(child, seen, images);
                    if images.len() == MAX_RICH_IMAGES {
                        return;
                    }
                }
            }
        }
        RichBlock::TaskList(items) => {
            for item in items {
                for child in &item.content {
                    collect_rich_images_from_block(child, seen, images);
                    if images.len() == MAX_RICH_IMAGES {
                        return;
                    }
                }
            }
        }
        RichBlock::Table(table) => {
            for row in &table.rows {
                for cell in &row.cells {
                    for child in &cell.content {
                        collect_rich_images_from_block(child, seen, images);
                        if images.len() == MAX_RICH_IMAGES {
                            return;
                        }
                    }
                }
            }
        }
        RichBlock::Paragraph(_)
        | RichBlock::Heading { .. }
        | RichBlock::DecisionList(_)
        | RichBlock::CodeBlock { .. }
        | RichBlock::Placeholder { .. } => {}
    }
}

pub(crate) fn collect_detail_images_with_context(
    detail: &IssueDetailViewModel,
) -> Vec<(RichImage, usize, ImageSource)> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    if let Some(document) = detail.rich_description.as_ref() {
        for image in collect_rich_images_with_context(document, 0) {
            if seen.insert(image.0.attachment_id.clone()) {
                images.push(image);
            }
        }
    }
    for (comment_index, comment) in detail.comments.iter().enumerate() {
        if let Some(document) = comment.rich_body.as_ref() {
            for image in collect_rich_images_with_context(document, comment_index + 1) {
                if seen.insert(image.0.attachment_id.clone()) {
                    images.push(image);
                    if images.len() == MAX_RICH_IMAGES {
                        return images;
                    }
                }
            }
        }
    }
    images.truncate(MAX_RICH_IMAGES);
    images
}

pub(crate) fn rich_image_contexts(
    images: &[(RichImage, usize, ImageSource)],
) -> Vec<(usize, ImageSource)> {
    images.iter().map(|image| (image.1, image.2)).collect()
}

fn collect_rich_images_with_context(
    document: &RichTextDocument,
    surface_ordinal: usize,
) -> Vec<(RichImage, usize, ImageSource)> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for block in &document.blocks {
        collect_rich_images_from_block_with_context(
            block,
            &mut seen,
            &mut images,
            surface_ordinal,
            ImageSource::ResolvedAdf,
        );
        if images.len() == MAX_RICH_IMAGES {
            break;
        }
    }
    for image in &document.fallback_images {
        if seen.insert(image.attachment_id.clone()) {
            images.push((
                image.clone(),
                surface_ordinal,
                ImageSource::FallbackCandidate,
            ));
        }
        if images.len() == MAX_RICH_IMAGES {
            break;
        }
    }
    images
}

fn collect_rich_images_from_block_with_context(
    block: &RichBlock,
    seen: &mut HashSet<String>,
    images: &mut Vec<(RichImage, usize, ImageSource)>,
    surface_ordinal: usize,
    source: ImageSource,
) {
    if images.len() == MAX_RICH_IMAGES {
        return;
    }
    match block {
        RichBlock::Image(image) => {
            if seen.insert(image.attachment_id.clone()) {
                images.push((image.clone(), surface_ordinal, source));
            }
        }
        RichBlock::BlockQuote(children)
        | RichBlock::Panel {
            content: children, ..
        }
        | RichBlock::Expand {
            content: children, ..
        }
        | RichBlock::NestedExpand {
            content: children, ..
        } => {
            for child in children {
                collect_rich_images_from_block_with_context(
                    child,
                    seen,
                    images,
                    surface_ordinal,
                    source,
                );
                if images.len() == MAX_RICH_IMAGES {
                    break;
                }
            }
        }
        RichBlock::BulletList(items) | RichBlock::OrderedList { items, .. } => {
            for item in items {
                for child in &item.blocks {
                    collect_rich_images_from_block_with_context(
                        child,
                        seen,
                        images,
                        surface_ordinal,
                        source,
                    );
                    if images.len() == MAX_RICH_IMAGES {
                        return;
                    }
                }
            }
        }
        RichBlock::TaskList(items) => {
            for item in items {
                for child in &item.content {
                    collect_rich_images_from_block_with_context(
                        child,
                        seen,
                        images,
                        surface_ordinal,
                        source,
                    );
                    if images.len() == MAX_RICH_IMAGES {
                        return;
                    }
                }
            }
        }
        RichBlock::Table(table) => {
            for row in &table.rows {
                for cell in &row.cells {
                    for child in &cell.content {
                        collect_rich_images_from_block_with_context(
                            child,
                            seen,
                            images,
                            surface_ordinal,
                            source,
                        );
                        if images.len() == MAX_RICH_IMAGES {
                            return;
                        }
                    }
                }
            }
        }
        RichBlock::Paragraph(_)
        | RichBlock::Heading { .. }
        | RichBlock::DecisionList(_)
        | RichBlock::CodeBlock { .. }
        | RichBlock::Placeholder { .. } => {}
    }
}

pub(crate) fn loading_image_states(
    images: &[(RichImage, usize, ImageSource)],
    diagnostics: &DiagnosticsSink,
    flow: crate::diagnostics::DiagnosticFlow,
    load_token: u64,
) -> RichImageRenderStates {
    let mut states = RichImageRenderStates::with_context(diagnostics.clone(), flow, load_token);
    for (candidate_ordinal, image) in images.iter().enumerate() {
        diagnostics.image_state(
            flow,
            load_token,
            candidate_ordinal,
            image.1,
            image.2,
            ImageStateReason::Loading,
        );
        states.insert_with_context(
            image.0.attachment_id.clone(),
            RichImageRenderState::Loading,
            candidate_ordinal,
            image.1,
            image.2,
        );
    }
    states
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_image(id: &str) -> RichImage {
        RichImage {
            attachment_id: id.to_owned(),
            filename: format!("{id}.png"),
            mime_type: "image/png".to_owned(),
            alt_text: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn rich_image_collection_is_recursive_deduplicated_and_capped() {
        let mut blocks = vec![RichBlock::Panel {
            kind: jira_domain::PanelKind::Info,
            content: vec![RichBlock::BlockQuote(vec![RichBlock::Image(test_image(
                "nested",
            ))])],
        }];
        blocks.extend((0..20).map(|index| RichBlock::Image(test_image(&format!("image-{index}")))));
        blocks.push(RichBlock::Image(test_image("nested")));
        let document = RichTextDocument::new(blocks, false);
        let images = collect_rich_images(&document);

        assert_eq!(images.len(), MAX_RICH_IMAGES);
        assert_eq!(images[0].attachment_id, "nested");
        assert_eq!(
            images
                .iter()
                .filter(|image| image.attachment_id == "nested")
                .count(),
            1
        );
    }

    #[test]
    fn task_and_expand_images_are_collected_in_document_order() {
        let document = RichTextDocument::new(
            vec![
                RichBlock::TaskList(vec![jira_domain::RichTaskItem {
                    state: jira_domain::RichTaskState::Todo,
                    content: vec![RichBlock::Expand {
                        title: Some("Task details".to_owned()),
                        content: vec![RichBlock::Image(test_image("task-image"))],
                    }],
                }]),
                RichBlock::NestedExpand {
                    title: Some("More".to_owned()),
                    content: vec![RichBlock::Image(test_image("nested-image"))],
                },
            ],
            false,
        );

        let images = collect_rich_images(&document);
        assert_eq!(
            images
                .iter()
                .map(|image| image.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["task-image", "nested-image"]
        );
    }

    #[test]
    fn rich_image_collection_appends_fallback_candidates_after_resolved_images() {
        let document = RichTextDocument::new(vec![RichBlock::Image(test_image("resolved"))], false)
            .with_fallback_images(vec![
                test_image("resolved"),
                test_image("candidate-a"),
                test_image("candidate-b"),
            ]);
        let images = collect_rich_images(&document);
        assert_eq!(
            images
                .iter()
                .map(|image| image.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["resolved", "candidate-a", "candidate-b"]
        );
    }

    #[test]
    fn rich_image_context_preserves_surface_and_source_markers() {
        let document = RichTextDocument::new(vec![RichBlock::Image(test_image("resolved"))], false)
            .with_fallback_images(vec![test_image("candidate")]);
        let images = collect_rich_images_with_context(&document, 3);
        assert_eq!(
            images,
            vec![
                (test_image("resolved"), 3, ImageSource::ResolvedAdf),
                (test_image("candidate"), 3, ImageSource::FallbackCandidate),
            ]
        );
    }

    #[test]
    fn detail_catalog_orders_description_then_comments_and_deduplicates_across_surfaces() {
        let description =
            RichTextDocument::new(vec![RichBlock::Image(test_image("description"))], false)
                .with_fallback_images(vec![
                    test_image("description-fallback"),
                    test_image("fallback-after-description"),
                ]);
        let comments = vec![crate::presentation::CommentViewModel {
            author: "A".to_owned(),
            body: String::new(),
            rich_body: Some(RichTextDocument::new(
                vec![
                    RichBlock::Image(test_image("description")),
                    RichBlock::Image(test_image("comment")),
                ],
                false,
            )),
            created: String::new(),
            updated: None,
        }];
        let detail = IssueDetailViewModel {
            description: String::new(),
            rich_description: Some(description),
            comments,
            attachments: Vec::new(),
        };
        let catalog = collect_detail_images_with_context(&detail);
        assert_eq!(
            catalog
                .iter()
                .map(|(image, surface, source)| (image.attachment_id.as_str(), *surface, *source))
                .collect::<Vec<_>>(),
            vec![
                ("description", 0, ImageSource::ResolvedAdf),
                ("description-fallback", 0, ImageSource::FallbackCandidate),
                (
                    "fallback-after-description",
                    0,
                    ImageSource::FallbackCandidate
                ),
                ("comment", 1, ImageSource::ResolvedAdf),
            ]
        );
    }

    #[test]
    fn detail_catalog_applies_one_global_cap_across_description_and_comments() {
        let description = RichTextDocument::new(
            (0..MAX_RICH_IMAGES)
                .map(|index| RichBlock::Image(test_image(&format!("description-{index}"))))
                .collect(),
            false,
        );
        let comment = crate::presentation::CommentViewModel {
            author: "A".to_owned(),
            body: String::new(),
            rich_body: Some(RichTextDocument::new(
                vec![RichBlock::Image(test_image("comment-after-cap"))],
                false,
            )),
            created: String::new(),
            updated: None,
        };
        let detail = IssueDetailViewModel {
            description: String::new(),
            rich_description: Some(description),
            comments: vec![comment],
            attachments: Vec::new(),
        };
        let catalog = collect_detail_images_with_context(&detail);
        assert_eq!(catalog.len(), MAX_RICH_IMAGES);
        assert!(
            catalog
                .iter()
                .all(|(image, _, _)| image.attachment_id != "comment-after-cap")
        );
    }

    #[test]
    fn fallback_image_candidates_obey_the_global_image_cap() {
        let candidates = (0..(MAX_RICH_IMAGES + 4))
            .map(|index| test_image(&format!("candidate-{index}")))
            .collect();
        let document = RichTextDocument::new(Vec::new(), false).with_fallback_images(candidates);
        let images = collect_rich_images(&document);
        assert_eq!(images.len(), MAX_RICH_IMAGES);
        assert_eq!(images[0].attachment_id, "candidate-0");
        assert_eq!(
            images.last().map(|image| image.attachment_id.as_str()),
            Some("candidate-15")
        );
    }

    #[test]
    fn empty_attachment_ids_preserve_one_empty_identity_and_deduplicate_it() {
        let document = RichTextDocument::new(
            vec![
                RichBlock::Image(test_image("")),
                RichBlock::Image(test_image("")),
            ],
            false,
        )
        .with_fallback_images(vec![test_image(""), test_image("real")]);

        let images = collect_rich_images(&document);
        assert_eq!(
            images
                .iter()
                .map(|image| image.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["", "real"]
        );
    }

    #[test]
    fn loading_image_states_record_order_context_flow_and_token() {
        use std::{fs, path::PathBuf};

        struct TempDirectory(PathBuf);
        impl TempDirectory {
            fn new() -> Self {
                let root = std::env::temp_dir().join(format!(
                    "jira-gpui-catalog-loading-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("clock")
                        .as_nanos()
                ));
                fs::create_dir_all(&root).expect("diagnostics temp directory");
                Self(root)
            }
        }
        impl Drop for TempDirectory {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        let directory = TempDirectory::new();
        let diagnostics = DiagnosticsSink::for_directory(&directory.0);
        let images = vec![
            (test_image("resolved"), 2, ImageSource::ResolvedAdf),
            (test_image("fallback"), 4, ImageSource::FallbackCandidate),
        ];
        let states = loading_image_states(
            &images,
            &diagnostics,
            crate::diagnostics::DiagnosticFlow::RemoteLookup,
            17,
        );

        assert!(matches!(
            states.get("resolved"),
            Some(RichImageRenderState::Loading)
        ));
        assert!(matches!(
            states.get("fallback"),
            Some(RichImageRenderState::Loading)
        ));
        for (candidate, (id, surface, source)) in [
            (0, ("resolved", 2, ImageSource::ResolvedAdf)),
            (1, ("fallback", 4, ImageSource::FallbackCandidate)),
        ] {
            let context = states
                .context_for(id, usize::MAX, usize::MAX, ImageSource::ResolvedAdf)
                .expect("loading context");
            assert_eq!(context.candidate_ordinal, candidate);
            assert_eq!(context.surface_ordinal, surface);
            assert_eq!(context.source, source);
            assert_eq!(
                context.flow,
                crate::diagnostics::DiagnosticFlow::RemoteLookup
            );
            assert_eq!(context.load_token, 17);
        }

        let lines =
            fs::read_to_string(directory.0.join("diagnostics.jsonl")).expect("diagnostics JSONL");
        let lines = lines.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains(r#""event":"image_state""#));
        assert!(lines[0].contains(r#""flow":"remote_lookup""#));
        assert!(lines[0].contains(r#""load_token":17"#));
        assert!(lines[0].contains(r#""candidate":0"#));
        assert!(lines[0].contains(r#""surface":2"#));
        assert!(lines[0].contains(r#""source":"resolved_adf""#));
        assert!(lines[0].contains(r#""reason":"loading""#));
        assert!(lines[1].contains(r#""candidate":1"#));
        assert!(lines[1].contains(r#""surface":4"#));
        assert!(lines[1].contains(r#""source":"fallback_candidate""#));
    }
}
