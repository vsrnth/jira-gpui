//! Shared rendering policy and safety budget.

use super::{MAX_RENDER_DEPTH, MAX_RENDER_NODES, MAX_RENDER_TEXT_BYTES, RENDER_SURFACE_STRIDE};

#[derive(Default)]
pub(super) struct RenderBudget {
    pub(super) nodes: usize,
    pub(super) image_ordinal: usize,
    element_ordinal: usize,
    text_bytes: usize,
    pub(super) omitted: bool,
}

impl RenderBudget {
    pub(super) fn enter(&mut self, depth: usize) -> bool {
        if depth > MAX_RENDER_DEPTH || self.nodes >= MAX_RENDER_NODES {
            self.omitted = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    pub(super) fn next_element_ordinal(&mut self) -> usize {
        let ordinal = self.element_ordinal.min(MAX_RENDER_NODES);
        self.element_ordinal = self.element_ordinal.saturating_add(1);
        ordinal
    }

    pub(super) fn text(&mut self, value: &str) -> String {
        self.text_with_wrap(value, true)
    }

    pub(super) fn text_nowrap(&mut self, value: &str) -> String {
        self.text_with_wrap(value, false)
    }

    fn text_with_wrap(&mut self, value: &str, soft_wrap: bool) -> String {
        let remaining = MAX_RENDER_TEXT_BYTES.saturating_sub(self.text_bytes);
        let mut end = value.len().min(remaining);
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        let result = value[..end].to_owned();
        self.text_bytes += result.len();
        if end < value.len() {
            self.omitted = true;
        }
        if soft_wrap {
            insert_soft_wraps(&result)
        } else {
            result
        }
    }
}

pub(super) fn insert_soft_wraps(value: &str) -> String {
    const SOFT_WRAP_AFTER: usize = 64;
    let mut wrapped = String::with_capacity(value.len());
    let mut run = 0;
    for character in value.chars() {
        if character.is_whitespace() {
            run = 0;
        } else if run >= SOFT_WRAP_AFTER {
            wrapped.push('\u{200b}');
            run = 0;
        }
        wrapped.push(character);
        run += 1;
    }
    wrapped
}

/// Give each rendered surface an isolated ordinal namespace without putting
/// untrusted Jira identifiers into GPUI element IDs.
pub(super) fn render_element_ordinal(surface_ordinal: usize, node_ordinal: usize) -> usize {
    surface_ordinal
        .saturating_mul(RENDER_SURFACE_STRIDE)
        .saturating_add(node_ordinal.min(MAX_RENDER_NODES))
}
