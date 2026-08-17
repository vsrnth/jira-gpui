use serde::{Deserialize, Serialize};

use crate::AccountId;

/// A bounded, transport-neutral representation of the subset of Atlassian Document Format
/// that Jira Desk can display safely. It intentionally contains no raw JSON or UI types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RichTextDocument {
    pub blocks: Vec<RichBlock>,
    #[serde(default)]
    pub truncated: bool,
}

impl RichTextDocument {
    /// Keep projections bounded even when a cached/public model was deserialized
    /// without passing through the Jira ADF parser.
    pub const MAX_PLAIN_TEXT_BYTES: usize = 1_000_000;
    pub const MAX_PLAIN_TEXT_DEPTH: usize = 64;
    pub const MAX_PLAIN_TEXT_NODES: usize = 10_000;

    pub fn new(blocks: Vec<RichBlock>, truncated: bool) -> Self {
        Self { blocks, truncated }
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
        self.blocks.is_empty() && !self.truncated
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
    Placeholder {
        label: String,
    },
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
        }
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

#[cfg(test)]
mod tests {
    use super::{RichBlock, RichInline, RichTextDocument};

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
}
