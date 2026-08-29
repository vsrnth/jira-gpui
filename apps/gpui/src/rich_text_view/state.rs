use std::{collections::HashMap, sync::Arc};

use gpui::Image;

use crate::diagnostics::{DiagnosticFlow, DiagnosticsSink, ImageSource as DiagnosticImageSource};

/// The application-owned state for a Jira attachment image.
#[derive(Clone)]
pub(crate) enum RichImageRenderState {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

#[derive(Clone)]
pub(crate) struct RichImageDiagnosticContext {
    pub(crate) sink: DiagnosticsSink,
    pub(crate) flow: DiagnosticFlow,
    pub(crate) load_token: u64,
    pub(crate) candidate_ordinal: usize,
    pub(crate) surface_ordinal: usize,
    pub(crate) source: DiagnosticImageSource,
}

#[derive(Clone, Default)]
pub(crate) struct RichImageRenderStates {
    states: HashMap<String, RichImageRenderState>,
    contexts: HashMap<String, RichImageDiagnosticContext>,
    default_context: Option<(DiagnosticsSink, DiagnosticFlow, u64)>,
}

impl RichImageRenderStates {
    pub(crate) fn with_context(
        sink: DiagnosticsSink,
        flow: DiagnosticFlow,
        load_token: u64,
    ) -> Self {
        Self {
            default_context: Some((sink, flow, load_token)),
            ..Self::default()
        }
    }

    pub(crate) fn set_context(
        &mut self,
        sink: DiagnosticsSink,
        flow: DiagnosticFlow,
        load_token: u64,
    ) {
        self.states.clear();
        self.contexts.clear();
        self.default_context = Some((sink, flow, load_token));
    }

    pub(crate) fn rebind_context(
        &mut self,
        sink: DiagnosticsSink,
        flow: DiagnosticFlow,
        load_token: u64,
    ) {
        self.contexts.clear();
        self.default_context = Some((sink, flow, load_token));
    }

    /// Merge only the previous ready states into loading slots. Existing
    /// Ready/Failed results always win, and the receiver's keys define the
    /// complete current catalog, so stale attachment IDs cannot survive.
    pub(crate) fn merge_preserving_ready(&mut self, previous: &Self) {
        for (key, state) in &previous.states {
            if matches!(self.states.get(key), Some(RichImageRenderState::Loading))
                && matches!(state, RichImageRenderState::Ready(_))
            {
                self.states.insert(key.clone(), state.clone());
            }
        }
    }

    pub(crate) fn insert(&mut self, key: String, state: RichImageRenderState) {
        self.states.insert(key, state);
    }

    pub(crate) fn insert_with_context(
        &mut self,
        key: String,
        state: RichImageRenderState,
        candidate_ordinal: usize,
        surface_ordinal: usize,
        source: DiagnosticImageSource,
    ) {
        if let Some((sink, flow, load_token)) = &self.default_context {
            self.contexts.insert(
                key.clone(),
                RichImageDiagnosticContext {
                    sink: sink.clone(),
                    flow: *flow,
                    load_token: *load_token,
                    candidate_ordinal,
                    surface_ordinal,
                    source,
                },
            );
        }
        self.states.insert(key, state);
    }

    pub(crate) fn get(&self, key: &str) -> Option<&RichImageRenderState> {
        self.states.get(key)
    }

    pub(crate) fn context_for(
        &self,
        key: &str,
        fallback_ordinal: usize,
        fallback_surface_ordinal: usize,
        fallback_source: DiagnosticImageSource,
    ) -> Option<RichImageDiagnosticContext> {
        self.contexts.get(key).cloned().or_else(|| {
            self.default_context
                .as_ref()
                .map(|(sink, flow, load_token)| RichImageDiagnosticContext {
                    sink: sink.clone(),
                    flow: *flow,
                    load_token: *load_token,
                    candidate_ordinal: fallback_ordinal,
                    surface_ordinal: fallback_surface_ordinal,
                    source: fallback_source,
                })
        })
    }

    pub(crate) fn clear(&mut self) {
        self.states.clear();
        self.contexts.clear();
    }
}

impl<const N: usize> From<[(String, RichImageRenderState); N]> for RichImageRenderStates {
    fn from(entries: [(String, RichImageRenderState); N]) -> Self {
        let mut states = Self::default();
        for (key, state) in entries {
            states.insert(key, state);
        }
        states
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::ImageFormat;

    fn ready() -> RichImageRenderState {
        RichImageRenderState::Ready(Arc::new(Image::from_bytes(
            ImageFormat::Png,
            b"\x89PNG\r\n\x1a\nvalid".to_vec(),
        )))
    }

    fn states(entries: Vec<(&str, RichImageRenderState)>) -> RichImageRenderStates {
        let mut result = RichImageRenderStates::default();
        for (key, state) in entries {
            result.insert(key.to_owned(), state);
        }
        result
    }

    #[test]
    fn merge_preserves_ready_for_same_attachment() {
        let previous = states(vec![("same", ready())]);
        let mut next = states(vec![("same", RichImageRenderState::Loading)]);
        next.merge_preserving_ready(&previous);
        assert!(matches!(
            next.get("same"),
            Some(RichImageRenderState::Ready(_))
        ));
    }

    #[test]
    fn merge_drops_ready_attachment_absent_from_new_catalog() {
        let previous = states(vec![("old", ready())]);
        let mut next = states(vec![("new", RichImageRenderState::Loading)]);
        next.merge_preserving_ready(&previous);
        assert!(next.get("old").is_none());
        assert!(matches!(
            next.get("new"),
            Some(RichImageRenderState::Loading)
        ));
    }

    #[test]
    fn merge_does_not_overwrite_new_ready_or_failed_results() {
        let previous = states(vec![("ready", ready()), ("failed", ready())]);
        let mut next = states(vec![
            ("ready", ready()),
            ("failed", RichImageRenderState::Failed),
        ]);
        next.merge_preserving_ready(&previous);
        assert!(matches!(
            next.get("ready"),
            Some(RichImageRenderState::Ready(_))
        ));
        assert!(matches!(
            next.get("failed"),
            Some(RichImageRenderState::Failed)
        ));
    }
}
