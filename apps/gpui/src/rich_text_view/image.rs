//! Image rendering and image-state presentation.

use super::{
    DecodeFallbackReason, DiagnosticEvent, ImageStateReason, MAX_IMAGE_HEIGHT,
    MAX_IMAGE_LABEL_BYTES, RenderBudget, RenderContext, RichImage, RichImageRenderState,
    RichImageRenderStates, render_element_ordinal,
};
use gpui::{
    AnyElement, ElementId, ImageSource as GpuiImageSource, InteractiveElement as _,
    IntoElement as _, ObjectFit, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    StyledImage as _, div, img, px,
};
use gpui_component::{h_flex, spinner::Spinner, v_flex};

pub(super) fn render_image(
    image: &RichImage,
    context: &RenderContext<'_>,
    budget: &mut RenderBudget,
) -> AnyElement {
    let image_ordinal = budget.image_ordinal;
    budget.image_ordinal = budget.image_ordinal.saturating_add(1);
    let name = bounded_image_name(rich_image_name(image));
    let accessible_label = budget.text(&format!("Image: {name}"));
    let mut frame = v_flex()
        .min_w_0()
        .max_w_full()
        .gap_2()
        .rounded(px(6.))
        .border_1()
        .border_color(context.palette.border)
        // The ID is internal GPUI bookkeeping, never rendered or exposed as an
        // accessibility label. A dedicated element ordinal makes repeated
        // attachments unique within this render pass.
        .id(ElementId::named_usize(
            "rich-image",
            render_element_ordinal(context.surface_ordinal, budget.next_element_ordinal()),
        ))
        .aria_label(accessible_label);

    let diagnostic_context = context.image_states.context_for(
        &image.attachment_id,
        image_ordinal,
        context.surface_ordinal,
        context.source,
    );
    match image_render_state(context.image_states, image) {
        Some(RichImageRenderState::Ready(image)) => {
            let unavailable = format!("Image unavailable · {name}");
            let loading_color = context.palette.muted;
            let fallback_color = context.palette.muted;
            frame = frame.child(
                img(GpuiImageSource::Image(image.clone()))
                    .max_w_full()
                    .max_h(px(MAX_IMAGE_HEIGHT))
                    .object_fit(ObjectFit::Contain)
                    // ImageSource::Image is already in memory, but GPUI may
                    // still decode it on the render path. Keep that fallback
                    // visible without adding a second animated spinner beside
                    // the source-state Loading view.
                    .with_loading(move || {
                        div()
                            .text_xs()
                            .text_color(loading_color)
                            .child("Loading image…")
                            .into_any_element()
                    })
                    .with_fallback(move || {
                        if let Some(context) = diagnostic_context.as_ref() {
                            context
                                .sink
                                .record_once(DiagnosticEvent::gpui_decode_fallback(
                                    context.flow,
                                    context.load_token,
                                    context.candidate_ordinal,
                                    context.surface_ordinal,
                                    context.source,
                                    DecodeFallbackReason::DecodeFailed,
                                ));
                        }
                        div()
                            .text_xs()
                            .text_color(fallback_color)
                            .child(unavailable.clone())
                            .into_any_element()
                    }),
            );
        }
        Some(RichImageRenderState::Loading) => {
            frame = frame.child(
                h_flex()
                    .min_h(px(72.))
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(Spinner::new())
                    .child("Loading image…"),
            );
        }
        Some(RichImageRenderState::Failed) => {
            let unavailable = format!("Image unavailable · {name}");
            frame = frame.child(
                div()
                    .min_h(px(72.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(context.palette.muted)
                    .child(budget.text(&unavailable)),
            );
        }
        None => {
            if let Some(context) = diagnostic_context.as_ref() {
                context.sink.record_once(DiagnosticEvent::image_state(
                    context.flow,
                    context.load_token,
                    context.candidate_ordinal,
                    context.surface_ordinal,
                    context.source,
                    ImageStateReason::Missing,
                ));
            }
            let unavailable = format!("Image unavailable · {name}");
            frame = frame.child(
                div()
                    .min_h(px(72.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(context.palette.muted)
                    .child(budget.text(&unavailable)),
            );
        }
    }

    frame
        .child(
            div()
                .text_xs()
                .text_color(context.palette.muted)
                .child(budget.text(&name)),
        )
        .into_any_element()
}

pub(super) fn rich_image_name(image: &RichImage) -> &str {
    image
        .alt_text
        .as_deref()
        .filter(|alt| !alt.trim().is_empty())
        .unwrap_or(image.filename.as_str())
}

fn bounded_image_name(value: &str) -> String {
    let mut end = value.len().min(MAX_IMAGE_LABEL_BYTES);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

pub(super) fn image_render_state<'a>(
    image_states: &'a RichImageRenderStates,
    image: &RichImage,
) -> Option<&'a RichImageRenderState> {
    image_states.get(&image.attachment_id)
}
