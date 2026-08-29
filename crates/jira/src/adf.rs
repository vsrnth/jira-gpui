use jira_domain::{
    AccountId, AttachmentMetadata, IssueKey, PanelKind, RichAttachmentCard, RichBlock, RichImage,
    RichInline, RichListItem, RichMark, RichTable, RichTableCell, RichTableRow, RichTextDocument,
};
use serde_json::Value;
use std::collections::HashSet;
use url::Url;

pub(super) const MAX_ADF_TEXT: usize = 1_000_000;
pub(super) const MAX_ADF_DEPTH: usize = 64;
pub(super) const MAX_ADF_NODES: usize = 10_000;
pub(super) const MAX_LINK_HREF_BYTES: usize = 2_048;
pub(super) const MAX_LINK_TITLE_BYTES: usize = 512;
const MAX_MEDIA_INLINE_ID_BYTES: usize = 255;
const MAX_MEDIA_ALT_BYTES: usize = 512;
const MAX_MEDIA_DIMENSION: u32 = 10_000;
const MAX_TABLE_ROWS: usize = 64;
const MAX_TABLE_CELLS: usize = 32;
pub(super) const UNSUPPORTED_CONTENT: &str = "[unsupported Jira content]";
pub(super) const UNAVAILABLE_IMAGE: &str = "[Jira image unavailable]";

/// Extracts the safe plain-text projection of a bounded ADF document.
pub fn adf_to_plain_text(value: &Value) -> Option<String> {
    parse_adf(value)
        .map(|document| document.plain_text())
        .filter(|text| !text.is_empty())
}

pub(super) fn adf_comment_text(value: &Value) -> Option<String> {
    adf_to_plain_text(value).or_else(|| {
        let object = value.as_object()?;
        let content = object.get("content")?.as_array()?;
        (!content.is_empty()).then(|| UNSUPPORTED_CONTENT.to_owned())
    })
}

/// Parses a Jira ADF document into a bounded transport-neutral representation.
///
/// Invalid roots are rejected. Unsupported nodes become safe placeholders, and no raw attrs,
/// URLs, or account IDs are ever copied into visible fallback text.
pub(super) fn parse_adf(value: &Value) -> Option<RichTextDocument> {
    parse_adf_internal(value, None)
}

fn parse_adf_with_attachments(
    value: &Value,
    attachments: &[AttachmentMetadata],
) -> Option<RichTextDocument> {
    parse_adf_internal(value, Some(attachments))
}

fn parse_adf_internal(
    value: &Value,
    attachments: Option<&[AttachmentMetadata]>,
) -> Option<RichTextDocument> {
    let object = value.as_object()?;
    if object.get("type").and_then(Value::as_str) != Some("doc")
        || object.get("version").and_then(Value::as_u64) != Some(1)
    {
        return None;
    }
    let content = object.get("content")?.as_array()?;
    let mut state = AdfParserState {
        media: attachments.map(|attachments| AdfMediaContext {
            attachments,
            file_media_count: count_file_media_references(value),
        }),
        ..AdfParserState::default()
    };
    let blocks = parse_blocks(content, 1, &mut state);
    let fallback_images = if state.unresolved_file_media {
        collect_fallback_images(attachments.unwrap_or_default(), &blocks)
    } else {
        Vec::new()
    };
    Some(RichTextDocument::new(blocks, state.truncated).with_fallback_images(fallback_images))
}

pub(super) fn visible_adf(value: &Value) -> Option<(RichTextDocument, String)> {
    let document = parse_adf(value)?;
    let text = document.plain_text();
    (!text.is_empty()).then_some((document, text))
}

pub(super) fn visible_adf_with_attachments(
    value: &Value,
    attachments: &[AttachmentMetadata],
) -> Option<(RichTextDocument, String)> {
    let document = parse_adf_with_attachments(value, attachments)?;
    let text = document.plain_text();
    (!text.is_empty()).then_some((document, text))
}

#[derive(Default)]
struct AdfParserState<'a> {
    nodes: usize,
    text_bytes: usize,
    truncated: bool,
    unresolved_file_media: bool,
    media: Option<AdfMediaContext<'a>>,
}

struct AdfMediaContext<'a> {
    attachments: &'a [AttachmentMetadata],
    file_media_count: usize,
}

impl AdfParserState<'_> {
    fn visit(&mut self) -> bool {
        if self.nodes >= MAX_ADF_NODES {
            self.truncated = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    fn text(&mut self, value: &str) -> String {
        let remaining = MAX_ADF_TEXT.saturating_sub(self.text_bytes);
        if remaining == 0 {
            self.truncated = true;
            return String::new();
        }
        let end = value
            .char_indices()
            .take_while(|(index, character)| index + character.len_utf8() <= remaining)
            .map(|(index, character)| index + character.len_utf8())
            .last()
            .unwrap_or(0);
        self.text_bytes += end;
        if end < value.len() {
            self.truncated = true;
        }
        value[..end].to_owned()
    }
}

fn parse_blocks(values: &[Value], depth: usize, state: &mut AdfParserState<'_>) -> Vec<RichBlock> {
    values
        .iter()
        .flat_map(|value| {
            let is_media_container = state.media.is_some()
                && matches!(
                    value.get("type").and_then(Value::as_str),
                    Some("mediaSingle" | "mediaGroup")
                );
            if !is_media_container {
                return parse_block(value, depth, state).into_iter().collect();
            }
            if depth > MAX_ADF_DEPTH || !state.visit() {
                state.truncated = true;
                return vec![RichBlock::Placeholder {
                    label: UNAVAILABLE_IMAGE.to_owned(),
                }];
            }
            match value.get("content").and_then(Value::as_array) {
                Some(content) => parse_media_container(content, depth + 1, state),
                None => {
                    state.truncated = true;
                    vec![RichBlock::Placeholder {
                        label: UNAVAILABLE_IMAGE.to_owned(),
                    }]
                }
            }
        })
        .collect()
}

fn parse_block(value: &Value, depth: usize, state: &mut AdfParserState<'_>) -> Option<RichBlock> {
    if depth > MAX_ADF_DEPTH || !state.visit() {
        state.truncated = true;
        return None;
    }
    let Some(object) = value.as_object() else {
        state.truncated = true;
        return Some(RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        });
    };
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let content = object.get("content").and_then(Value::as_array);
    let block = match kind {
        "paragraph" => match content {
            Some(content) => RichBlock::Paragraph(parse_inlines(content, depth + 1, state)),
            None => malformed_block(state),
        },
        "heading" => match content {
            Some(content) => RichBlock::Heading {
                level: object
                    .get("attrs")
                    .and_then(Value::as_object)
                    .and_then(|attrs| attrs.get("level"))
                    .and_then(Value::as_u64)
                    .and_then(|level| u8::try_from(level).ok())
                    .filter(|level| (1..=6).contains(level))
                    .unwrap_or(1),
                content: parse_inlines(content, depth + 1, state),
            },
            None => malformed_block(state),
        },
        "bulletList" => match content {
            Some(content) => RichBlock::BulletList(parse_list_items(content, depth + 1, state)),
            None => malformed_block(state),
        },
        "orderedList" => match content {
            Some(content) => RichBlock::OrderedList {
                order: object
                    .get("attrs")
                    .and_then(Value::as_object)
                    .and_then(|attrs| attrs.get("order"))
                    .and_then(Value::as_u64)
                    .and_then(|order| u32::try_from(order).ok())
                    .unwrap_or(1),
                items: parse_list_items(content, depth + 1, state),
            },
            None => malformed_block(state),
        },
        "listItem" => match content {
            Some(content) => RichBlock::BlockQuote(parse_blocks(content, depth + 1, state)),
            None => malformed_block(state),
        },
        "codeBlock" => match content {
            Some(content) => RichBlock::CodeBlock {
                language: object
                    .get("attrs")
                    .and_then(Value::as_object)
                    .and_then(|attrs| attrs.get("language"))
                    .and_then(Value::as_str)
                    .map(|language| state.text(language))
                    .filter(|language| !language.is_empty()),
                text: parse_code_text(content, depth + 1, state),
            },
            None => malformed_block(state),
        },
        "blockquote" => match content {
            Some(content) => RichBlock::BlockQuote(parse_blocks(content, depth + 1, state)),
            None => malformed_block(state),
        },
        "panel" => {
            let kind = object
                .get("attrs")
                .and_then(Value::as_object)
                .and_then(|attrs| attrs.get("panelType"))
                .and_then(Value::as_str)
                .and_then(panel_kind);
            match (kind, content) {
                (Some(kind), Some(content)) => RichBlock::Panel {
                    kind,
                    content: parse_blocks(content, depth + 1, state),
                },
                _ => malformed_block(state),
            }
        }
        "doc" => RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        },
        "media" => parse_media_node(object, state),
        "mediaSingle" | "mediaGroup" => match content {
            Some(content) if state.media.is_some() => {
                let blocks = parse_media_container(content, depth, state);
                match blocks.as_slice() {
                    [block] => block.clone(),
                    _ => RichBlock::BlockQuote(blocks),
                }
            }
            _ => RichBlock::Placeholder {
                label: UNSUPPORTED_CONTENT.to_owned(),
            },
        },
        "mediaInline" => match parse_media_inline_attachment_card(object, state) {
            RichInline::AttachmentCard(card) => {
                RichBlock::Paragraph(vec![RichInline::AttachmentCard(card)])
            }
            RichInline::Placeholder { label } => RichBlock::Placeholder { label },
            _ => RichBlock::Placeholder {
                label: UNSUPPORTED_CONTENT.to_owned(),
            },
        },
        "rule" if object.get("content").is_none() && object.get("attrs").is_none() => {
            RichBlock::horizontal_rule()
        }
        "rule" => malformed_block(state),
        "table" => parse_table_block(object, depth, state),
        "tableCell" | "tableHeader" | "tableRow" | "emoji" | "date" | "status" | "expand"
        | "nestedExpand" => RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        },
        "inlineCard" => match parse_inline_attachment_card(object, state) {
            RichInline::AttachmentCard(card) => {
                RichBlock::Paragraph(vec![RichInline::AttachmentCard(card)])
            }
            RichInline::Text { text, marks } => {
                RichBlock::Paragraph(vec![RichInline::Text { text, marks }])
            }
            RichInline::Placeholder { label } => RichBlock::Placeholder { label },
            _ => RichBlock::Placeholder {
                label: UNSUPPORTED_CONTENT.to_owned(),
            },
        },
        "blockCard" => match parse_inline_attachment_card(object, state) {
            RichInline::AttachmentCard(card) => {
                RichBlock::Paragraph(vec![RichInline::AttachmentCard(card)])
            }
            RichInline::Text { text, marks } => {
                RichBlock::Paragraph(vec![RichInline::Text { text, marks }])
            }
            RichInline::Placeholder { label } => RichBlock::Placeholder { label },
            _ => RichBlock::Placeholder {
                label: UNSUPPORTED_CONTENT.to_owned(),
            },
        },
        _ => RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        },
    };
    Some(block)
}

fn parse_table_block(
    object: &serde_json::Map<String, Value>,
    depth: usize,
    state: &mut AdfParserState<'_>,
) -> RichBlock {
    if object.get("attrs").is_some_and(|attrs| !attrs.is_object()) {
        return malformed_block(state);
    }
    let Some(content) = object.get("content").and_then(Value::as_array) else {
        return malformed_block(state);
    };
    if content.is_empty() {
        return malformed_block(state);
    }
    if content.len() > MAX_TABLE_ROWS {
        state.truncated = true;
    }
    let mut rows = Vec::new();
    for value in content.iter().take(MAX_TABLE_ROWS) {
        let Some(row) = parse_table_row(value, depth + 1, state) else {
            return malformed_block(state);
        };
        rows.push(row);
    }
    RichBlock::Table(RichTable { rows })
}

fn parse_table_row(
    value: &Value,
    depth: usize,
    state: &mut AdfParserState<'_>,
) -> Option<RichTableRow> {
    if depth > MAX_ADF_DEPTH || !state.visit() {
        state.truncated = true;
        return None;
    }
    let Some(object) = value.as_object() else {
        state.truncated = true;
        return None;
    };
    if !matches!(object.get("type").and_then(Value::as_str), Some("tableRow"))
        || object.get("attrs").is_some_and(|attrs| !attrs.is_object())
    {
        state.truncated = true;
        return None;
    }
    let Some(content) = object.get("content").and_then(Value::as_array) else {
        state.truncated = true;
        return None;
    };
    if content.is_empty() {
        state.truncated = true;
        return None;
    }
    if content.len() > MAX_TABLE_CELLS {
        state.truncated = true;
    }
    let mut cells = Vec::new();
    for value in content.iter().take(MAX_TABLE_CELLS) {
        let cell = parse_table_cell(value, depth + 1, state)?;
        cells.push(cell);
    }
    Some(RichTableRow { cells })
}

fn parse_table_cell(
    value: &Value,
    depth: usize,
    state: &mut AdfParserState<'_>,
) -> Option<RichTableCell> {
    if depth > MAX_ADF_DEPTH || !state.visit() {
        state.truncated = true;
        return None;
    }
    let Some(object) = value.as_object() else {
        state.truncated = true;
        return None;
    };
    let header = match object.get("type").and_then(Value::as_str) {
        Some("tableHeader") => true,
        Some("tableCell") => false,
        _ => {
            state.truncated = true;
            return None;
        }
    };
    if object.get("attrs").is_some_and(|attrs| !attrs.is_object()) {
        state.truncated = true;
        return None;
    }
    let Some(content) = object.get("content").and_then(Value::as_array) else {
        state.truncated = true;
        return None;
    };
    if content.is_empty() {
        state.truncated = true;
        return None;
    }
    let mut blocks = Vec::new();
    for child in content {
        let Some(block) = parse_block(child, depth + 1, state) else {
            state.truncated = true;
            return None;
        };
        blocks.push(block);
    }
    Some(RichTableCell {
        header,
        content: blocks,
    })
}

fn malformed_block(state: &mut AdfParserState<'_>) -> RichBlock {
    state.truncated = true;
    RichBlock::Placeholder {
        label: UNSUPPORTED_CONTENT.to_owned(),
    }
}

fn parse_media_container(
    values: &[Value],
    depth: usize,
    state: &mut AdfParserState<'_>,
) -> Vec<RichBlock> {
    if values.is_empty() {
        state.truncated = true;
        return vec![RichBlock::Placeholder {
            label: UNAVAILABLE_IMAGE.to_owned(),
        }];
    }
    values
        .iter()
        .map(|value| {
            if depth > MAX_ADF_DEPTH || !state.visit() {
                state.truncated = true;
                return RichBlock::Placeholder {
                    label: UNAVAILABLE_IMAGE.to_owned(),
                };
            }
            match value.as_object() {
                Some(object) if object.get("type").and_then(Value::as_str) == Some("media") => {
                    parse_media_node(object, state)
                }
                _ => {
                    state.truncated = true;
                    RichBlock::Placeholder {
                        label: UNAVAILABLE_IMAGE.to_owned(),
                    }
                }
            }
        })
        .collect()
}

fn parse_media_node(
    object: &serde_json::Map<String, Value>,
    state: &mut AdfParserState<'_>,
) -> RichBlock {
    let Some(media) = state.media.as_ref() else {
        return RichBlock::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        };
    };
    let Some(attrs) = object.get("attrs").and_then(Value::as_object) else {
        state.truncated = true;
        return RichBlock::Placeholder {
            label: UNAVAILABLE_IMAGE.to_owned(),
        };
    };
    if attrs.get("type").and_then(Value::as_str) != Some("file") {
        return RichBlock::Placeholder {
            label: UNAVAILABLE_IMAGE.to_owned(),
        };
    }
    match resolve_media_image(attrs, media) {
        Some(image) => RichBlock::Image(image),
        None => {
            state.unresolved_file_media = true;
            RichBlock::Placeholder {
                label: UNAVAILABLE_IMAGE.to_owned(),
            }
        }
    }
}

fn collect_fallback_images(
    attachments: &[AttachmentMetadata],
    blocks: &[RichBlock],
) -> Vec<RichImage> {
    let mut resolved_ids = HashSet::new();
    collect_resolved_image_ids(blocks, &mut resolved_ids);
    let mut seen_ids = HashSet::new();
    attachments
        .iter()
        .filter_map(|attachment| {
            if resolved_ids.contains(&attachment.id) || !seen_ids.insert(attachment.id.clone()) {
                return None;
            }
            let mime_type = normalized_image_mime(attachment.mime_type.as_deref())?;
            Some(RichImage {
                attachment_id: attachment.id.clone(),
                filename: attachment.filename.clone(),
                mime_type,
                alt_text: None,
                width: None,
                height: None,
            })
        })
        .take(RichTextDocument::MAX_FALLBACK_IMAGES)
        .collect()
}

fn collect_resolved_image_ids(blocks: &[RichBlock], resolved_ids: &mut HashSet<String>) {
    for block in blocks {
        match block {
            RichBlock::Image(image) => {
                resolved_ids.insert(image.attachment_id.clone());
            }
            RichBlock::BlockQuote(children)
            | RichBlock::Panel {
                content: children, ..
            } => collect_resolved_image_ids(children, resolved_ids),
            RichBlock::BulletList(items) | RichBlock::OrderedList { items, .. } => {
                for item in items {
                    collect_resolved_image_ids(&item.blocks, resolved_ids);
                }
            }
            RichBlock::Table(table) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        collect_resolved_image_ids(&cell.content, resolved_ids);
                    }
                }
            }
            RichBlock::Paragraph(_)
            | RichBlock::Heading { .. }
            | RichBlock::CodeBlock { .. }
            | RichBlock::Placeholder { .. } => {}
        }
    }
}

fn resolve_media_image(
    attrs: &serde_json::Map<String, Value>,
    media: &AdfMediaContext<'_>,
) -> Option<RichImage> {
    let id = attrs
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let alt = attrs
        .get("alt")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());

    let attachment = if let Some(id) = id {
        match unique_attachment(
            media
                .attachments
                .iter()
                .filter(|attachment| attachment.id == id),
        ) {
            Ok(Some(attachment)) => attachment,
            Err(()) => return None,
            Ok(None) => alt_or_fallback(alt, media)?,
        }
    } else {
        alt_or_fallback(alt, media)?
    };

    let mime_type = normalized_image_mime(attachment.mime_type.as_deref())?;
    let alt_text = alt
        .map(|value| bounded_media_text(value, MAX_MEDIA_ALT_BYTES))
        .filter(|value| !value.is_empty());
    Some(RichImage {
        attachment_id: attachment.id.clone(),
        filename: attachment.filename.clone(),
        mime_type,
        alt_text,
        width: bounded_dimension(attrs.get("width")),
        height: bounded_dimension(attrs.get("height")),
    })
}

fn alt_or_fallback<'a>(
    alt: Option<&str>,
    media: &'a AdfMediaContext<'_>,
) -> Option<&'a AttachmentMetadata> {
    if let Some(alt) = alt {
        match unique_attachment(
            media
                .attachments
                .iter()
                .filter(|attachment| attachment.filename == alt),
        ) {
            Ok(Some(attachment)) => return Some(attachment),
            Err(()) => return None,
            Ok(None) => {}
        }
    }
    let mut allowed = media
        .attachments
        .iter()
        .filter(|attachment| allowed_image_mime(attachment.mime_type.as_deref()));
    if media.file_media_count == 1 {
        let attachment = allowed.next()?;
        if allowed.next().is_none() {
            return Some(attachment);
        }
    }
    None
}

fn unique_attachment<'a, I>(mut matches: I) -> Result<Option<&'a AttachmentMetadata>, ()>
where
    I: Iterator<Item = &'a AttachmentMetadata>,
{
    let Some(attachment) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        Err(())
    } else {
        Ok(Some(attachment))
    }
}

fn allowed_image_mime(mime_type: Option<&str>) -> bool {
    normalized_image_mime(mime_type).is_some()
}

fn normalized_image_mime(mime_type: Option<&str>) -> Option<String> {
    let mime_type = mime_type?.trim();
    if mime_type.eq_ignore_ascii_case("image/png")
        || mime_type.eq_ignore_ascii_case("image/jpeg")
        || mime_type.eq_ignore_ascii_case("image/gif")
        || mime_type.eq_ignore_ascii_case("image/webp")
    {
        Some(mime_type.to_ascii_lowercase())
    } else {
        None
    }
}

fn bounded_media_text(value: &str, maximum: usize) -> String {
    let end = value
        .char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= maximum)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    value[..end].to_owned()
}

fn bounded_dimension(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| (1..=MAX_MEDIA_DIMENSION).contains(value))
}

fn count_file_media_references(value: &Value) -> usize {
    let mut count = 0;
    let mut visited = 0;
    count_file_media_references_inner(value, 0, &mut count, &mut visited);
    count
}

pub(super) fn count_file_media_references_inner(
    value: &Value,
    depth: usize,
    count: &mut usize,
    visited: &mut usize,
) {
    if depth > MAX_ADF_DEPTH || *count >= 2 || *visited >= MAX_ADF_NODES {
        return;
    }
    *visited += 1;
    if let Some(object) = value.as_object() {
        if matches!(
            object.get("type").and_then(Value::as_str),
            Some("media" | "mediaInline")
        ) && object
            .get("attrs")
            .and_then(Value::as_object)
            .and_then(|attrs| attrs.get("type"))
            .and_then(Value::as_str)
            == Some("file")
        {
            *count = count.saturating_add(1);
        }
        for child in object.values() {
            count_file_media_references_inner(child, depth + 1, count, visited);
        }
    } else if let Some(values) = value.as_array() {
        for child in values {
            count_file_media_references_inner(child, depth + 1, count, visited);
        }
    }
}

fn parse_list_items(
    values: &[Value],
    depth: usize,
    state: &mut AdfParserState<'_>,
) -> Vec<RichListItem> {
    values
        .iter()
        .filter_map(|value| {
            let Some(object) = value.as_object() else {
                state.truncated = true;
                return Some(RichListItem {
                    blocks: vec![RichBlock::Placeholder {
                        label: UNSUPPORTED_CONTENT.to_owned(),
                    }],
                });
            };
            if object.get("type").and_then(Value::as_str) != Some("listItem") {
                state.truncated = true;
                return Some(RichListItem {
                    blocks: vec![RichBlock::Placeholder {
                        label: UNSUPPORTED_CONTENT.to_owned(),
                    }],
                });
            }
            if depth > MAX_ADF_DEPTH || !state.visit() {
                state.truncated = true;
                return None;
            }
            let Some(content) = object.get("content").and_then(Value::as_array) else {
                state.truncated = true;
                return Some(RichListItem {
                    blocks: vec![RichBlock::Placeholder {
                        label: UNSUPPORTED_CONTENT.to_owned(),
                    }],
                });
            };
            Some(RichListItem {
                blocks: parse_blocks(content, depth + 1, state),
            })
        })
        .collect()
}

fn parse_code_text(values: &[Value], depth: usize, state: &mut AdfParserState<'_>) -> String {
    let mut text = String::new();
    for value in values {
        if depth > MAX_ADF_DEPTH || !state.visit() {
            state.truncated = true;
            break;
        }
        if value.get("type").and_then(Value::as_str) == Some("text") {
            if let Some(value) = value.get("text").and_then(Value::as_str) {
                text.push_str(&state.text(value));
            }
        } else {
            state.truncated = true;
        }
    }
    text
}

fn parse_inlines(
    values: &[Value],
    depth: usize,
    state: &mut AdfParserState<'_>,
) -> Vec<RichInline> {
    values
        .iter()
        .filter_map(|value| parse_inline(value, depth, state))
        .collect()
}

fn parse_inline(value: &Value, depth: usize, state: &mut AdfParserState<'_>) -> Option<RichInline> {
    if depth > MAX_ADF_DEPTH || !state.visit() {
        state.truncated = true;
        return None;
    }
    let Some(object) = value.as_object() else {
        state.truncated = true;
        return Some(RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        });
    };
    match object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => match object.get("text").and_then(Value::as_str) {
            Some(text) => Some(RichInline::Text {
                text: state.text(text),
                marks: parse_marks(object.get("marks").and_then(Value::as_array)),
            }),
            None => {
                state.truncated = true;
                Some(RichInline::Placeholder {
                    label: UNSUPPORTED_CONTENT.to_owned(),
                })
            }
        },
        "hardBreak" => Some(RichInline::HardBreak),
        "mention" => {
            let attrs = object.get("attrs").and_then(Value::as_object);
            let normalized_id = attrs
                .and_then(|attrs| attrs.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty());
            let account_id = normalized_id.and_then(|id| AccountId::new(id.to_owned()).ok());
            let label = attrs
                .and_then(|attrs| {
                    ["text", "displayText", "displayName"]
                        .into_iter()
                        .find_map(|key| attrs.get(key).and_then(Value::as_str))
                })
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .filter(|text| Some(*text) != normalized_id)
                .filter(|text| !looks_like_opaque_account_id(text))
                .map(|text| state.text(text))
                .unwrap_or_else(|| "Mentioned user".to_owned());
            Some(RichInline::Mention { account_id, label })
        }
        "emoji" | "date" | "status" => Some(RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        }),
        "mediaInline" => Some(parse_media_inline_attachment_card(object, state)),
        "inlineCard" => Some(parse_inline_attachment_card(object, state)),
        _ => Some(RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        }),
    }
}

fn parse_inline_attachment_card(
    object: &serde_json::Map<String, Value>,
    state: &mut AdfParserState<'_>,
) -> RichInline {
    let Some(attrs) = object.get("attrs").and_then(Value::as_object) else {
        return RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        };
    };
    if attrs.contains_key("data") || attrs.contains_key("datasource") {
        return RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        };
    }
    let Some(url) = attrs.get("url").and_then(Value::as_str) else {
        return RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        };
    };
    if let Some(media) = state.media.as_ref()
        && let Some(attachment_id) = attachment_id_from_inline_card_url(url)
    {
        let attachment = match unique_attachment(
            media
                .attachments
                .iter()
                .filter(|attachment| attachment.id == attachment_id),
        ) {
            Ok(Some(attachment)) => attachment,
            Ok(None) | Err(()) => {
                return RichInline::Placeholder {
                    label: UNSUPPORTED_CONTENT.to_owned(),
                };
            }
        };
        return RichInline::AttachmentCard(RichAttachmentCard {
            attachment_id: attachment.id.clone(),
            filename: attachment.filename.clone(),
            mime_type: normalized_attachment_mime(attachment.mime_type.as_deref()),
            size_bytes: Some(attachment.size_bytes),
        });
    }
    jira_browse_issue_key(url)
        .map(|text| RichInline::Text {
            text,
            marks: Vec::new(),
        })
        .unwrap_or(RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        })
}

fn parse_media_inline_attachment_card(
    object: &serde_json::Map<String, Value>,
    state: &mut AdfParserState<'_>,
) -> RichInline {
    let Some(media) = state.media.as_ref() else {
        return RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        };
    };
    let Some(attrs) = object.get("attrs").and_then(Value::as_object) else {
        return RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        };
    };
    if attrs.get("type").and_then(Value::as_str) != Some("file") {
        return RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        };
    }

    let attachment = if let Some(id) = attrs
        .get("id")
        .and_then(Value::as_str)
        .and_then(bounded_media_inline_id)
    {
        match unique_attachment(
            media
                .attachments
                .iter()
                .filter(|attachment| attachment.id == id),
        ) {
            Ok(Some(attachment)) => Some(attachment),
            Err(()) => {
                return RichInline::Placeholder {
                    label: UNSUPPORTED_CONTENT.to_owned(),
                };
            }
            Ok(None) => None,
        }
    } else {
        None
    };

    let attachment = attachment
        .or_else(|| unique_media_filename_match(attrs, media))
        .or_else(|| {
            (media.file_media_count == 1 && media.attachments.len() == 1)
                .then(|| &media.attachments[0])
        });
    let Some(attachment) = attachment else {
        return RichInline::Placeholder {
            label: UNSUPPORTED_CONTENT.to_owned(),
        };
    };
    RichInline::AttachmentCard(RichAttachmentCard {
        attachment_id: attachment.id.clone(),
        filename: attachment.filename.clone(),
        mime_type: normalized_attachment_mime(attachment.mime_type.as_deref()),
        size_bytes: Some(attachment.size_bytes),
    })
}

fn unique_media_filename_match<'a>(
    attrs: &serde_json::Map<String, Value>,
    media: &'a AdfMediaContext<'_>,
) -> Option<&'a AttachmentMetadata> {
    let mut resolved = None;
    for key in ["alt", "__fileName"] {
        let Some(filename) = attrs
            .get(key)
            .and_then(Value::as_str)
            .and_then(bounded_media_filename)
        else {
            continue;
        };
        let attachment = match unique_attachment(
            media
                .attachments
                .iter()
                .filter(|attachment| attachment.filename == filename),
        ) {
            Ok(attachment) => attachment,
            Err(()) => return None,
        };
        let Some(attachment) = attachment else {
            continue;
        };
        if resolved.is_some_and(|previous: &AttachmentMetadata| previous.id != attachment.id) {
            return None;
        }
        resolved = Some(attachment);
    }
    resolved
}

fn bounded_media_filename(value: &str) -> Option<&str> {
    if value.len() > MAX_MEDIA_ALT_BYTES {
        return None;
    }
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn bounded_media_inline_id(value: &str) -> Option<&str> {
    if value.len() > MAX_MEDIA_INLINE_ID_BYTES {
        return None;
    }
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

pub(super) fn attachment_id_from_inline_card_url(value: &str) -> Option<String> {
    if value.len() > MAX_LINK_HREF_BYTES {
        return None;
    }
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let parsed = Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().filter(|host| !host.is_empty()).is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
    {
        return None;
    }
    let segments = parsed.path_segments()?.collect::<Vec<_>>();
    let candidate = if segments.len() >= 3 && segments[0] == "secure" && segments[1] == "attachment"
    {
        segments[2]
    } else if segments.len() == 6
        && segments[0] == "rest"
        && segments[1] == "api"
        && segments[2] == "3"
        && segments[3] == "attachment"
        && segments[4] == "content"
    {
        segments[5]
    } else {
        return None;
    };
    (!candidate.is_empty()
        && candidate
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')))
    .then(|| candidate.to_owned())
}

fn jira_browse_issue_key(value: &str) -> Option<String> {
    if value.len() > MAX_LINK_HREF_BYTES {
        return None;
    }
    let parsed = Url::parse(value).ok()?;
    let authority_start = value.find("://")? + 3;
    let authority_end = value[authority_start..]
        .find(['/', '?', '#'])
        .map_or(value.len(), |offset| authority_start + offset);
    let authority = &value[authority_start..authority_end];
    if parsed.scheme() != "https"
        || parsed.username() != ""
        || parsed.password().is_some()
        || authority.contains(':')
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let host = parsed.host_str()?.to_ascii_lowercase();
    let subdomain = host.strip_suffix(".atlassian.net")?;
    if subdomain.is_empty() || subdomain.split('.').any(str::is_empty) {
        return None;
    }
    let mut segments = parsed.path_segments()?;
    if segments.next()? != "browse" {
        return None;
    }
    let candidate = segments.next()?;
    if segments.next().is_some() {
        return None;
    }
    IssueKey::new(candidate.to_owned())
        .ok()
        .map(|issue_key| issue_key.to_string())
}

fn normalized_attachment_mime(mime_type: Option<&str>) -> Option<String> {
    let mime_type = mime_type?.split(';').next()?.trim();
    let (kind, subtype) = mime_type.split_once('/')?;
    if kind.is_empty()
        || subtype.is_empty()
        || mime_type.matches('/').count() != 1
        || mime_type.len() > 255
        || !mime_type.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'+' | b'-' | b'_')
        })
    {
        return None;
    }
    Some(mime_type.to_ascii_lowercase())
}

fn parse_marks(values: Option<&Vec<Value>>) -> Vec<RichMark> {
    values
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let object = value.as_object()?;
            match object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "code" => Some(RichMark::Code),
                "em" => Some(RichMark::Emphasis),
                "strong" => Some(RichMark::Strong),
                "strike" => Some(RichMark::Strike),
                "link" => {
                    let attrs = object.get("attrs").and_then(Value::as_object)?;
                    let href = attrs
                        .get("href")
                        .and_then(Value::as_str)
                        .and_then(safe_uri)?;
                    let title = attrs
                        .get("title")
                        .and_then(Value::as_str)
                        .filter(|title| title.len() <= MAX_LINK_TITLE_BYTES)
                        .map(str::to_owned);
                    Some(RichMark::Link { href, title })
                }
                _ => None,
            }
        })
        .collect()
}

fn panel_kind(value: &str) -> Option<PanelKind> {
    match value {
        "info" => Some(PanelKind::Info),
        "note" => Some(PanelKind::Note),
        "warning" => Some(PanelKind::Warning),
        "success" => Some(PanelKind::Success),
        "error" => Some(PanelKind::Error),
        _ => None,
    }
}

/// Jira Cloud account IDs commonly use a six-digit tenant prefix followed by an opaque token.
/// This deliberately recognizes only that narrow shape so ordinary human labels containing a
/// colon remain visible while an id-less mention cannot leak an account identifier.
fn looks_like_opaque_account_id(value: &str) -> bool {
    let value = value.trim();
    let Some((prefix, suffix)) = value.split_once(':') else {
        return false;
    };
    prefix.len() == 6
        && prefix.bytes().all(|byte| byte.is_ascii_digit())
        && suffix.len() >= 6
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn safe_uri(value: &str) -> Option<String> {
    let value = value.trim();
    if value.len() > MAX_LINK_HREF_BYTES {
        return None;
    }
    let parsed = Url::parse(value).ok()?;
    let authority_start = value.find("://")? + 3;
    let authority_end = value[authority_start..]
        .find(['/', '?', '#'])
        .map_or(value.len(), |offset| authority_start + offset);
    if value[authority_start..authority_end].contains('@') {
        return None;
    }
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    Some(value.to_owned())
}
