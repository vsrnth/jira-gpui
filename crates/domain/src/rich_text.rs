use serde::{Deserialize, Serialize};

use crate::AccountId;

/// Stable semantic marker for an ADF horizontal rule.
///
/// This uses the existing placeholder wire shape so cached documents produced
/// before the rule representation was introduced remain deserializable. The
/// domain helpers keep the marker explicit to parsers, renderers, and plain
/// text projection without carrying raw ADF data.
pub const HORIZONTAL_RULE_LABEL: &str = "[horizontal rule]";

/// A bounded, transport-neutral representation of the subset of Atlassian Document Format
/// that Jira Desk can display safely. It intentionally contains no raw JSON or UI types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RichTextDocument {
    pub blocks: Vec<RichBlock>,
    #[serde(default)]
    pub truncated: bool,
    /// Attachment candidates retained when Jira's ADF media reference cannot be
    /// mapped unambiguously to a Jira attachment ID. Candidates are metadata only;
    /// callers must not infer their position in the document.
    #[serde(default)]
    pub fallback_images: Vec<RichImage>,
}

impl RichTextDocument {
    /// Keep projections bounded even when a cached/public model was deserialized
    /// without passing through the Jira ADF parser.
    pub const MAX_FALLBACK_IMAGES: usize = 16;
    pub const MAX_PLAIN_TEXT_BYTES: usize = 1_000_000;
    pub const MAX_PLAIN_TEXT_DEPTH: usize = 64;
    pub const MAX_PLAIN_TEXT_NODES: usize = 10_000;

    pub fn new(blocks: Vec<RichBlock>, truncated: bool) -> Self {
        Self {
            blocks,
            truncated,
            fallback_images: Vec::new(),
        }
    }

    pub fn with_fallback_images(mut self, fallback_images: Vec<RichImage>) -> Self {
        self.fallback_images = fallback_images;
        self
    }

    /// Produces the bounded plain-text projection used by search and compatibility callers.
    pub fn plain_text(&self) -> String {
        let mut output = PlainTextBuilder::default();
        for block in &self.blocks {
            if output.truncated {
                break;
            }
            append_block_text(block, &mut output, 0);
        }
        let has_truncation_block = matches!(
            self.blocks.last(),
            Some(RichBlock::Placeholder { label }) if label == PLAIN_TEXT_TRUNCATION
        );
        if self.truncated && !output.truncated && !has_truncation_block {
            output.push_str("[content truncated]");
        }
        output.finish()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.fallback_images.is_empty() && !self.truncated
    }

    /// Returns whether this document contains a direct mention of the account.
    ///
    /// Mention nodes can occur below list items, block quotes, panels, and other
    /// nested ADF blocks, so callers must not inspect only the top-level blocks.
    pub fn mentions_account(&self, account_id: &AccountId) -> bool {
        self.blocks
            .iter()
            .any(|block| block_mentions_account(block, account_id))
    }
}

impl RichBlock {
    pub fn horizontal_rule() -> Self {
        Self::Placeholder {
            label: HORIZONTAL_RULE_LABEL.to_owned(),
        }
    }

    pub fn is_horizontal_rule(&self) -> bool {
        matches!(
            self,
            Self::Placeholder { label } if label == HORIZONTAL_RULE_LABEL
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RichBlock {
    Paragraph(Vec<RichInline>),
    Heading {
        level: u8,
        content: Vec<RichInline>,
    },
    BulletList(Vec<RichListItem>),
    OrderedList {
        order: u32,
        items: Vec<RichListItem>,
    },
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    BlockQuote(Vec<RichBlock>),
    Panel {
        kind: PanelKind,
        content: Vec<RichBlock>,
    },
    Table(RichTable),
    Image(RichImage),
    Placeholder {
        label: String,
    },
}

/// Bounded, transport-neutral metadata for an image attached to a Jira issue.
///
/// The attachment ID is intentionally the only fetch handle retained here. Content and
/// thumbnail URLs, bytes, and HTTP/UI types do not belong in the domain model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RichImage {
    pub attachment_id: String,
    pub filename: String,
    pub mime_type: String,
    #[serde(default)]
    pub alt_text: Option<String>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// Bounded metadata for an inline Jira attachment card.
///
/// The card intentionally retains no source URL. Callers may use the attachment ID to
/// request metadata/content through an explicit application port, but the rich-text domain
/// model never carries an arbitrary remote URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RichAttachmentCard {
    pub attachment_id: String,
    pub filename: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

/// Bounded semantic representation of a Jira ADF table.
///
/// The table, row, and cell types remain explicit so renderers never need to
/// interpret ADF JSON or delimiter-encoded text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RichTable {
    pub rows: Vec<RichTableRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RichTableRow {
    pub cells: Vec<RichTableCell>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RichTableCell {
    #[serde(default)]
    pub header: bool,
    pub content: Vec<RichBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RichListItem {
    pub blocks: Vec<RichBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RichInline {
    Text {
        text: String,
        marks: Vec<RichMark>,
    },
    HardBreak,
    Mention {
        #[serde(default)]
        account_id: Option<AccountId>,
        label: String,
    },
    AttachmentCard(RichAttachmentCard),
    Placeholder {
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RichMark {
    Code,
    Emphasis,
    Strong,
    Strike,
    Link { href: String, title: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelKind {
    Info,
    Note,
    Warning,
    Success,
    Error,
}

const PLAIN_TEXT_TRUNCATION: &str = "[content truncated]";

#[derive(Default)]
struct PlainTextBuilder {
    output: String,
    truncated: bool,
    nodes: usize,
}

impl PlainTextBuilder {
    fn visit(&mut self, depth: usize) -> bool {
        if depth > RichTextDocument::MAX_PLAIN_TEXT_DEPTH
            || self.nodes >= RichTextDocument::MAX_PLAIN_TEXT_NODES
        {
            self.truncate();
            return false;
        }
        self.nodes += 1;
        true
    }

    fn truncate(&mut self) {
        if self.truncated {
            return;
        }
        let content_limit =
            RichTextDocument::MAX_PLAIN_TEXT_BYTES.saturating_sub(PLAIN_TEXT_TRUNCATION.len());
        if self.output.len() > content_limit {
            let mut end = content_limit;
            while !self.output.is_char_boundary(end) {
                end -= 1;
            }
            self.output.truncate(end);
        }
        self.output.push_str(PLAIN_TEXT_TRUNCATION);
        self.truncated = true;
    }

    fn push_str(&mut self, value: &str) {
        if self.truncated || value.is_empty() {
            return;
        }
        if self.output.len().saturating_add(value.len()) <= RichTextDocument::MAX_PLAIN_TEXT_BYTES {
            self.output.push_str(value);
            return;
        }

        let content_limit =
            RichTextDocument::MAX_PLAIN_TEXT_BYTES.saturating_sub(PLAIN_TEXT_TRUNCATION.len());
        let remaining = content_limit.saturating_sub(self.output.len());
        let prefix_end = value
            .char_indices()
            .take_while(|(index, character)| index + character.len_utf8() <= remaining)
            .map(|(index, character)| index + character.len_utf8())
            .last()
            .unwrap_or(0);
        self.output.push_str(&value[..prefix_end]);
        self.truncate();
    }

    fn trim_trailing_newlines(&mut self) {
        if self.truncated {
            return;
        }
        while self.output.ends_with('\n') {
            self.output.pop();
        }
    }

    fn finish(self) -> String {
        normalize_plain_text(self.output)
    }
}

fn append_block_text(block: &RichBlock, output: &mut PlainTextBuilder, depth: usize) {
    if !output.visit(depth) {
        return;
    }
    match block {
        RichBlock::Paragraph(content) | RichBlock::Heading { content, .. } => {
            append_inline_text(content, output, depth + 1);
            output.push_str("\n");
        }
        RichBlock::BulletList(items) => {
            for item in items {
                if output.truncated {
                    break;
                }
                output.push_str("• ");
                append_list_item_text(item, output, depth + 1);
            }
        }
        RichBlock::OrderedList { order, items } => {
            for (offset, item) in items.iter().enumerate() {
                if output.truncated {
                    break;
                }
                output.push_str(&(order.saturating_add(offset as u32)).to_string());
                output.push_str(". ");
                append_list_item_text(item, output, depth + 1);
            }
        }
        RichBlock::CodeBlock { text, .. } => {
            output.push_str(text);
            output.push_str("\n");
        }
        RichBlock::BlockQuote(content) | RichBlock::Panel { content, .. } => {
            for child in content {
                if output.truncated {
                    break;
                }
                append_block_text(child, output, depth + 1);
            }
        }
        RichBlock::Table(table) => append_table_text(table, output, depth + 1),
        RichBlock::Image(image) => {
            output.push_str("[image: ");
            output.push_str(
                image
                    .alt_text
                    .as_deref()
                    .filter(|alt| !alt.is_empty())
                    .unwrap_or(&image.filename),
            );
            output.push_str("]\n");
        }
        RichBlock::Placeholder { label } if label == HORIZONTAL_RULE_LABEL => {}
        RichBlock::Placeholder { label } => {
            output.push_str(label);
            output.push_str("\n");
        }
    }
}

fn append_list_item_text(item: &RichListItem, output: &mut PlainTextBuilder, depth: usize) {
    if !output.visit(depth) {
        return;
    }
    for block in &item.blocks {
        if output.truncated {
            break;
        }
        append_block_text(block, output, depth + 1);
    }
}

fn append_inline_text(content: &[RichInline], output: &mut PlainTextBuilder, depth: usize) {
    for inline in content {
        if !output.visit(depth) {
            return;
        }
        match inline {
            RichInline::Text { text, .. } => output.push_str(text),
            RichInline::HardBreak => output.push_str("\n"),
            RichInline::Mention { label, .. } | RichInline::Placeholder { label } => {
                output.push_str(label)
            }
            RichInline::AttachmentCard(card) => {
                output.push_str("[attachment: ");
                output.push_str(&card.filename);
                output.push_str("]");
            }
        }
    }
}

fn append_table_text(table: &RichTable, output: &mut PlainTextBuilder, depth: usize) {
    if !output.visit(depth) {
        return;
    }
    for row in &table.rows {
        if !output.visit(depth + 1) {
            return;
        }
        for (index, cell) in row.cells.iter().enumerate() {
            if !output.visit(depth + 2) {
                return;
            }
            if index > 0 {
                output.trim_trailing_newlines();
                output.push_str(" | ");
            }
            for block in &cell.content {
                append_block_text(block, output, depth + 3);
                if output.truncated {
                    return;
                }
            }
        }
        output.push_str("\n");
    }
}

fn normalize_plain_text(value: String) -> String {
    let mut normalized = String::with_capacity(value.len());
    for character in value.chars() {
        if character == '\n' && normalized.ends_with('\n') {
            continue;
        }
        normalized.push(character);
    }
    normalized.trim().to_owned()
}

fn block_mentions_account(block: &RichBlock, account_id: &AccountId) -> bool {
    match block {
        RichBlock::Paragraph(content) | RichBlock::Heading { content, .. } => {
            inline_mentions_account(content, account_id)
        }
        RichBlock::BulletList(items) => items.iter().any(|item| {
            item.blocks
                .iter()
                .any(|block| block_mentions_account(block, account_id))
        }),
        RichBlock::OrderedList { items, .. } => items.iter().any(|item| {
            item.blocks
                .iter()
                .any(|block| block_mentions_account(block, account_id))
        }),
        RichBlock::BlockQuote(content) | RichBlock::Panel { content, .. } => content
            .iter()
            .any(|block| block_mentions_account(block, account_id)),
        RichBlock::Table(table) => table.rows.iter().any(|row| {
            row.cells.iter().any(|cell| {
                cell.content
                    .iter()
                    .any(|block| block_mentions_account(block, account_id))
            })
        }),
        RichBlock::CodeBlock { .. } | RichBlock::Image(_) | RichBlock::Placeholder { .. } => false,
    }
}

fn inline_mentions_account(content: &[RichInline], account_id: &AccountId) -> bool {
    content.iter().any(|inline| {
        matches!(
            inline,
            RichInline::Mention {
                account_id: Some(mentioned),
                ..
            } if mentioned == account_id
        )
    })
}

#[cfg(test)]
mod tests {
    use crate::AccountId;

    use super::{
        HORIZONTAL_RULE_LABEL, PanelKind, RichBlock, RichImage, RichInline, RichListItem,
        RichTable, RichTableCell, RichTableRow, RichTextDocument,
    };

    #[test]
    fn plain_text_is_bounded_for_deserialized_models() {
        let document = RichTextDocument::new(
            (0..200_000)
                .map(|_| {
                    RichBlock::Paragraph(vec![RichInline::Text {
                        text: "abcdefghij".to_owned(),
                        marks: Vec::new(),
                    }])
                })
                .collect(),
            false,
        );

        let first = document.plain_text();
        let second = document.plain_text();
        assert_eq!(first, second);
        assert!(first.len() <= RichTextDocument::MAX_PLAIN_TEXT_BYTES);
        assert!(first.ends_with("[content truncated]"));
    }

    #[test]
    fn explicit_truncation_is_projected_once() {
        let document = RichTextDocument::new(
            vec![RichBlock::Paragraph(vec![RichInline::Text {
                text: "body".to_owned(),
                marks: Vec::new(),
            }])],
            true,
        );
        assert_eq!(document.plain_text(), "body\n[content truncated]");
    }

    #[test]
    fn horizontal_rule_is_explicit_and_plain_text_neutral() {
        let rule = RichBlock::horizontal_rule();
        assert!(rule.is_horizontal_rule());
        assert_eq!(
            RichTextDocument::new(
                vec![
                    RichBlock::Paragraph(vec![RichInline::Text {
                        text: "before".to_owned(),
                        marks: Vec::new(),
                    }]),
                    rule,
                    RichBlock::Paragraph(vec![RichInline::Text {
                        text: "after".to_owned(),
                        marks: Vec::new(),
                    }]),
                ],
                false,
            )
            .plain_text(),
            "before\nafter"
        );
        assert_eq!(HORIZONTAL_RULE_LABEL, "[horizontal rule]");
    }

    #[test]
    fn table_projects_cell_text_without_raw_structure() {
        let table = RichTable {
            rows: vec![RichTableRow {
                cells: vec![
                    RichTableCell {
                        header: true,
                        content: vec![RichBlock::Paragraph(vec![RichInline::Text {
                            text: "Criterion".to_owned(),
                            marks: Vec::new(),
                        }])],
                    },
                    RichTableCell {
                        header: false,
                        content: vec![RichBlock::Paragraph(vec![RichInline::Text {
                            text: "Expected result".to_owned(),
                            marks: Vec::new(),
                        }])],
                    },
                ],
            }],
        };
        let document = RichTextDocument::new(vec![RichBlock::Table(table)], false);

        assert_eq!(document.plain_text(), "Criterion | Expected result");
    }

    #[test]
    fn parsed_style_truncation_marker_is_not_duplicated() {
        let document = RichTextDocument::new(
            vec![RichBlock::Placeholder {
                label: "[content truncated]".to_owned(),
            }],
            true,
        );
        assert_eq!(document.plain_text(), "[content truncated]");
    }

    #[test]
    fn plain_text_limits_recursive_depth_and_node_count() {
        let mut nested = RichBlock::Paragraph(vec![RichInline::Text {
            text: "deep".to_owned(),
            marks: Vec::new(),
        }]);
        for _ in 0..100 {
            nested = RichBlock::BlockQuote(vec![nested]);
        }
        let deep = RichTextDocument::new(vec![nested], false);
        assert!(deep.plain_text().ends_with("[content truncated]"));

        let many = RichTextDocument::new(
            (0..20_000)
                .map(|_| {
                    RichBlock::Paragraph(vec![RichInline::Text {
                        text: "node".to_owned(),
                        marks: Vec::new(),
                    }])
                })
                .collect(),
            false,
        );
        let projected = many.plain_text();
        assert!(projected.len() <= RichTextDocument::MAX_PLAIN_TEXT_BYTES);
        assert!(projected.ends_with("[content truncated]"));
    }

    #[test]
    fn image_plain_text_uses_alt_text_or_filename() {
        let document = RichTextDocument::new(
            vec![
                RichBlock::Image(RichImage {
                    attachment_id: "10001".to_owned(),
                    filename: "diagram.png".to_owned(),
                    mime_type: "image/png".to_owned(),
                    alt_text: Some("Architecture diagram".to_owned()),
                    width: Some(100),
                    height: Some(80),
                }),
                RichBlock::Image(RichImage {
                    attachment_id: "10002".to_owned(),
                    filename: "screenshot.webp".to_owned(),
                    mime_type: "image/webp".to_owned(),
                    alt_text: None,
                    width: None,
                    height: None,
                }),
            ],
            false,
        );
        assert_eq!(
            document.plain_text(),
            "[image: Architecture diagram]\n[image: screenshot.webp]"
        );
    }

    #[test]
    fn fallback_images_make_a_document_nonempty() {
        let image = RichImage {
            attachment_id: "10001".to_owned(),
            filename: "diagram.png".to_owned(),
            mime_type: "image/png".to_owned(),
            alt_text: None,
            width: None,
            height: None,
        };
        assert!(RichTextDocument::new(Vec::new(), false).is_empty());
        assert!(
            !RichTextDocument::new(Vec::new(), false)
                .with_fallback_images(vec![image])
                .is_empty()
        );
    }

    #[test]
    fn attachment_cards_project_to_bounded_plain_text() {
        let document = RichTextDocument::new(
            vec![RichBlock::Paragraph(vec![RichInline::AttachmentCard(
                super::RichAttachmentCard {
                    attachment_id: "10002".to_owned(),
                    filename: "partner-enrollment.csv".to_owned(),
                    mime_type: Some("text/csv".to_owned()),
                    size_bytes: Some(4096),
                },
            )])],
            false,
        );
        assert_eq!(
            document.plain_text(),
            "[attachment: partner-enrollment.csv]"
        );
    }

    #[test]
    fn mentions_account_traverses_nested_blocks_and_lists() {
        let account = AccountId::new("account-1").expect("account");
        let document = RichTextDocument::new(
            vec![RichBlock::Panel {
                kind: PanelKind::Info,
                content: vec![RichBlock::BulletList(vec![RichListItem {
                    blocks: vec![RichBlock::BlockQuote(vec![RichBlock::Paragraph(vec![
                        RichInline::Mention {
                            account_id: Some(account.clone()),
                            label: "@Asha".to_owned(),
                        },
                    ])])],
                }])],
            }],
            false,
        );

        assert!(document.mentions_account(&account));
    }

    #[test]
    fn mentions_account_ignores_missing_or_unrelated_ids() {
        let account = AccountId::new("account-1").expect("account");
        let other = AccountId::new("account-2").expect("account");
        let document = RichTextDocument::new(
            vec![RichBlock::OrderedList {
                order: 1,
                items: vec![RichListItem {
                    blocks: vec![RichBlock::Paragraph(vec![
                        RichInline::Mention {
                            account_id: None,
                            label: "@unknown".to_owned(),
                        },
                        RichInline::Mention {
                            account_id: Some(other),
                            label: "@Other".to_owned(),
                        },
                    ])],
                }],
            }],
            false,
        );

        assert!(!document.mentions_account(&account));
    }
}
