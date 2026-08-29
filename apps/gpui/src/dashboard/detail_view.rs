use super::*;
use gpui::rems;
use gpui_component::{
    Sizable as _, Size, description_list::DescriptionList, list::List, popover::Popover,
};

fn detail_metadata_value(value: String, selector: &'static str) -> AnyElement {
    div()
        .debug_selector(move || selector.to_owned())
        .min_w_0()
        .text_sm()
        .child(value)
        .into_any_element()
}

pub(super) fn normalized_lookup_query(query: &str) -> String {
    crate::presentation::normalized_issue_key(query)
        .map(|key| key.to_string())
        .unwrap_or_else(|| query.trim().to_owned())
}

impl Dashboard {
    fn rich_text_palette(&self, cx: &mut Context<Self>) -> RichTextPalette {
        RichTextPalette {
            foreground: cx.theme().foreground,
            muted: cx.theme().muted_foreground,
            border: cx.theme().border,
            code_surface: cx.theme().muted.opacity(0.18),
            link: cx.theme().link,
            info: cx.theme().link,
            warning: cx.theme().warning,
            success: cx.theme().success,
            danger: cx.theme().danger,
        }
    }

    fn active_image_states(&self) -> &RichImageRenderStates {
        if matches!(self.remote_lookup, RemoteLookupState::Loaded { .. }) {
            &self.remote_image_states
        } else {
            &self.selected_image_states
        }
    }

    fn selected_issue_detail_view(&self) -> Option<IssueViewModel> {
        selected_issue_from_sources(
            self.selected_issue.as_ref(),
            &self.domain_issues,
            self.selected_issue_core.as_ref(),
        )
        .filter(|issue| issue_has_cached_detail(issue))
        .map(|issue| IssueViewModel::from_domain(issue, &self.users))
        .or_else(|| self.selected_issue_view())
    }

    pub(super) fn issue_detail(&self, layout: LayoutMode, cx: &mut Context<Self>) -> AnyElement {
        let issue = match &self.remote_lookup {
            RemoteLookupState::Loaded { .. } => self.remote_lookup_view(),
            RemoteLookupState::Loading { .. } | RemoteLookupState::Error { .. } => None,
            RemoteLookupState::Idle => self.selected_issue_detail_view(),
        };
        let detail_state = match &self.remote_lookup {
            RemoteLookupState::Loaded { detail, .. } => DetailState::Loaded(detail.clone()),
            RemoteLookupState::Loading { query } => DetailState::RemoteLoading {
                query: query.clone(),
            },
            RemoteLookupState::Error { query, copy } => DetailState::RemoteError {
                query: query.clone(),
                copy: *copy,
            },
            RemoteLookupState::Idle => self.detail_state.clone(),
        };
        let Some(issue) = issue else {
            let status_surface = match &detail_state {
                DetailState::RemoteLoading { query } => v_flex()
                    .id("issue-detail-remote-loading")
                    .role(gpui::accesskit::Role::Status)
                    .aria_label(format!(
                        "Jira lookup in progress for {}",
                        normalized_lookup_query(query)
                    ))
                    .gap_2()
                    .child(div().text_base().font_semibold().child("Jira lookup"))
                    .child(
                        h_flex().gap_2().child(Spinner::new()).child(
                            div()
                                .min_w_0()
                                .whitespace_normal()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!(
                                    "Looking up {}…",
                                    normalized_lookup_query(query)
                                )),
                        ),
                    ),
                DetailState::RemoteError { query, copy } => v_flex()
                    .id("issue-detail-remote-error")
                    .role(gpui::accesskit::Role::Alert)
                    .aria_label(format!(
                        "Jira lookup failed for {}: {}",
                        normalized_lookup_query(query),
                        copy.message()
                    ))
                    .gap_2()
                    .child(div().text_base().font_semibold().child("Jira lookup failed"))
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_normal()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(copy.message()),
                    ),
                DetailState::Error { copy, .. } => v_flex()
                    .id("issue-detail-error-surface")
                    .role(gpui::accesskit::Role::Alert)
                    .aria_label(format!(
                        "Unable to load issue details: {}",
                        copy.message()
                    ))
                    .gap_2()
                    .child(div().text_base().font_semibold().child("Unable to load issue details"))
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_normal()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(copy.message()),
                    ),
                DetailState::Loading { .. } => v_flex()
                    .id("issue-detail-loading-surface")
                    .role(gpui::accesskit::Role::Status)
                    .aria_label("Loading issue details")
                    .gap_2()
                    .child(div().text_base().font_semibold().child("Loading issue details"))
                    .child(h_flex().gap_2().child(Spinner::new())),
                DetailState::Empty | DetailState::Loaded(_) | DetailState::Refreshing { .. } => v_flex()
                    .id("issue-detail-empty-surface")
                    .gap_2()
                    .child(div().text_base().font_semibold().child("Select an issue"))
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_normal()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Choose a Jira issue to view its description, fields, comments, and attachments."),
                    ),
            };
            return v_flex()
                .id("issue-detail")
                .debug_selector(|| "issue-detail".to_owned())
                .w_full()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .p(rems(layout.detail_padding() / 16.0))
                .child(
                    status_surface
                        .debug_selector(|| "issue-detail-status".to_owned())
                        .w_full()
                        .max_w_full()
                        .min_w_0(),
                )
                .into_any_element();
        };
        let project = issue.project.clone();
        let key = issue.key.clone();
        let summary = issue.summary.clone();
        let issue_type = issue.issue_type.clone();
        let status = issue.status.clone();
        let priority = issue.priority.clone();
        let description = match &detail_state {
            DetailState::Loaded(detail) | DetailState::Refreshing { detail, .. } => {
                detail.description.clone()
            }
            _ => issue.description.clone(),
        };
        let rich_description = match &detail_state {
            DetailState::Loaded(detail) | DetailState::Refreshing { detail, .. } => {
                detail.rich_description.clone()
            }
            _ => issue.rich_description.clone(),
        };
        let detail_issue_id = match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => Some(issue.id.clone()),
            _ if matches!(
                &detail_state,
                DetailState::Loaded(_) | DetailState::Refreshing { .. }
            ) =>
            {
                self.selected_issue.clone()
            }
            _ => None,
        };
        let inline_attachment_action = detail_issue_id.map(|expected_issue_id| {
            let dashboard = cx.entity().downgrade();
            RichAttachmentCardAction::new(move |attachment_id, window, app| {
                if let Some(dashboard) = dashboard.upgrade() {
                    let expected_issue_id = expected_issue_id.clone();
                    dashboard.update(app, |this, cx| {
                        this.download_inline_attachment(
                            &expected_issue_id,
                            attachment_id,
                            window,
                            cx,
                        );
                    });
                }
            })
        });
        let description_content = rich_description
            .as_ref()
            .map(|document| {
                render_rich_text_with_actions(
                    document,
                    self.rich_text_palette(cx),
                    self.active_image_states(),
                    0,
                    ImageSource::ResolvedAdf,
                    inline_attachment_action.clone(),
                )
            })
            .unwrap_or_else(|| div().text_sm().child(description).into_any_element());
        let assignee = issue.assignee.clone();
        let reporter = issue.reporter.clone();
        let status_category = issue.status_category.clone();
        let parent = issue.parent.clone().unwrap_or_else(|| "None".to_owned());
        let created = issue.created.clone();
        let updated = issue.updated.clone();
        let due_date = issue.due_date.clone();
        let labels = issue.labels.clone();
        v_flex()
            .id("issue-detail")
            .flex_1()
            .min_w_0()
            .overflow_y_scrollbar()
            .p(rems(layout.detail_padding() / 16.0))
            .gap(rems(if layout.is_mobile() { 1. } else { 1.25 }))
            .child(
                v_flex()
                    .debug_selector(|| "issue-detail-header".to_owned())
                    .min_w_0()
                    .gap_2()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(div().min_w_0().truncate().child(project))
                            .child("/")
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        Icon::new(issue_type_icon(&issue_type))
                                            .text_color(cx.theme().link),
                                    )
                                    .child(div().min_w_0().truncate().child(key)),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .line_clamp(if layout.is_mobile() { 3 } else { 4 })
                            .text_2xl()
                            .font_semibold()
                            .child(summary),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .min_w_0()
                            .gap_2()
                            .child(self.pill(issue_type, cx))
                            .child(self.status_control(Some(&issue), status, cx))
                            .child(self.priority_badge(priority, cx)),
                    )
                    .when(
                        matches!(&self.remote_lookup, RemoteLookupState::Loaded { .. }),
                        |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().link)
                                    .child("Jira lookup result"),
                            )
                        },
                    ),
            )
            .when_some(
                self.render_selected_detail_feedback(&detail_state, cx),
                |this, feedback| this.child(feedback),
            )
            .child(self.render_issue_edit_controls(Some(&issue), layout, cx))
            .child(
                v_flex()
                    .debug_selector(|| "issue-detail-description".to_owned())
                    .gap_2()
                    .child(div().text_sm().font_semibold().child("Description"))
                    .child(
                        div()
                            .p_4()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(description_content),
                    ),
            )
            .child(
                v_flex()
                    .debug_selector(|| "issue-detail-details".to_owned())
                    .gap_3()
                    .child(div().text_sm().font_semibold().child("Details"))
                    .child(
                        DescriptionList::horizontal()
                            .with_size(Size::Small)
                            .columns(1)
                            .bordered(false)
                            .label_width(rems(if layout.is_rail() { 6.75 } else { 8.25 }))
                            .item(
                                "Assignee",
                                detail_metadata_value(assignee, "issue-detail-assignee"),
                                1,
                            )
                            .item(
                                "Reporter",
                                detail_metadata_value(reporter, "issue-detail-reporter"),
                                1,
                            )
                            .item(
                                "Status category",
                                detail_metadata_value(
                                    status_category,
                                    "issue-detail-status-category",
                                ),
                                1,
                            )
                            .item(
                                "Parent",
                                detail_metadata_value(parent, "issue-detail-parent"),
                                1,
                            )
                            .item(
                                "Created",
                                detail_metadata_value(created, "issue-detail-created"),
                                1,
                            )
                            .item(
                                "Updated",
                                detail_metadata_value(updated, "issue-detail-updated"),
                                1,
                            )
                            .item(
                                "Due date",
                                detail_metadata_value(due_date, "issue-detail-due-date"),
                                1,
                            ),
                    ),
            )
            .when(!labels.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_semibold().child("Labels"))
                        .child(
                            h_flex()
                                .flex_wrap()
                                .min_w_0()
                                .gap_2()
                                .children(labels.iter().cloned().map(|label| self.pill(label, cx))),
                        ),
                )
            })
            .child(self.render_detail_state_for(&detail_state, layout, cx))
            .into_any_element()
    }

    fn render_selected_detail_feedback(
        &self,
        detail_state: &DetailState,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match detail_state {
            DetailState::Loading { .. } => Some(
                h_flex()
                    .id("issue-detail-loading")
                    .debug_selector(|| "issue-detail-loading".to_owned())
                    .role(gpui::accesskit::Role::Status)
                    .aria_label("Loading issue details")
                    .min_w_0()
                    .gap_2()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().link.opacity(0.45))
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(Spinner::new())
                    .child(div().min_w_0().child("Loading issue details…"))
                    .into_any_element(),
            ),
            DetailState::Error { copy, .. } => Some(
                v_flex()
                    .id("issue-detail-error")
                    .debug_selector(|| "issue-detail-error".to_owned())
                    .role(gpui::accesskit::Role::Alert)
                    .aria_label(format!("Unable to load issue details: {}", copy.message()))
                    .min_w_0()
                    .gap_1()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().danger.opacity(0.45))
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(div().font_semibold().child("Unable to load issue details"))
                    .child(div().min_w_0().child(copy.message()))
                    .into_any_element(),
            ),
            DetailState::Empty
            | DetailState::RemoteLoading { .. }
            | DetailState::RemoteError { .. }
            | DetailState::Loaded(_)
            | DetailState::Refreshing { .. } => None,
        }
    }

    fn render_detail_state_for(
        &self,
        detail_state: &DetailState,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match detail_state {
            DetailState::Empty => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("Comments and attachments"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(if self.selected_issue.is_some() {
                            "This cached issue is a preview. Connect to Jira to load comments and attachments."
                        } else {
                            "Select an issue to load comments and attachments."
                        }),
                )
                .into_any_element(),
            DetailState::Loading { .. } | DetailState::Error { .. } => v_flex().into_any_element(),
            DetailState::Refreshing { .. } => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child("Comments and attachments"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Refreshing comments and attachments in the background."),
                )
                .into_any_element(),
            DetailState::RemoteLoading { query } => v_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child("Jira lookup"))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Looking up {query}…")),
                )
                .into_any_element(),
            DetailState::RemoteError { copy, .. } => v_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child("Jira lookup"))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(copy.message()),
                )
                .into_any_element(),
            DetailState::Loaded(detail) => {
                let palette = self.rich_text_palette(cx);
                let comments = if detail.comments.is_empty() {
                    v_flex()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No comments.")
                        .into_any_element()
                } else {
                    v_flex()
                        .min_w_0()
                        .gap_2()
                        .child(
                            h_flex()
                                .min_w_0()
                                .flex_wrap()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Issue"),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Jira exposes these comments at issue level"),
                                ),
                        )
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap_3()
                                .border_l_1()
                                .border_color(cx.theme().border)
                                .pl_3()
                                .children(detail.comments.iter().enumerate().map(
                                    |(comment_index, comment)| {
                                        let body = comment
                                            .rich_body
                                            .as_ref()
                                            .map(|document| {
                                                render_rich_text(
                                                    document,
                                                    palette,
                                                    self.active_image_states(),
                                                    comment_index.saturating_add(1),
                                                    ImageSource::ResolvedAdf,
                                                )
                                            })
                                            .unwrap_or_else(|| {
                                                div()
                                                    .text_sm()
                                                    .child(comment.body.clone())
                                                    .into_any_element()
                                            });
                                        v_flex()
                                            .gap_1()
                                            .p_3()
                                            .rounded(cx.theme().radius)
                                            .border_1()
                                            .border_color(cx.theme().border)
                                            .child(
                                                h_flex()
                                                    .min_w_0()
                                                    .flex_wrap()
                                                    .justify_between()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .text_sm()
                                                            .font_semibold()
                                                            .child(comment.author.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex_shrink_0()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(comment.created.clone()),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("On issue"),
                                            )
                                            .child(div().min_w_0().child(body))
                                            .when_some(comment.updated.clone(), |this, updated| {
                                                this.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(format!("Updated {updated}")),
                                                )
                                            })
                                    },
                                )),
                        )
                        .into_any_element()
                };
                let attachments = if detail.attachments.is_empty() {
                    v_flex()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No attachments.")
                        .into_any_element()
                } else {
                    v_flex()
                        .gap_2()
                        .children(detail.attachments.iter().map(|attachment| {
                            let attachment_for_click = attachment.clone();
                            let downloading = matches!(
                                &self.attachment_download_state,
                                AttachmentDownloadState::Saving { attachment_id }
                                    if attachment_id == &attachment.id
                            );
                            let download_active = !matches!(
                                self.attachment_download_state,
                                AttachmentDownloadState::Idle
                            );
                            h_flex()
                                .min_w_0()
                                .flex_wrap()
                                .justify_between()
                                .text_sm()
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .child(attachment.filename.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{} · {}",
                                            attachment.mime_type, attachment.size
                                        )),
                                )
                                .child(
                                    Button::new(format!("download-attachment-{}", attachment.id))
                                        .ghost()
                                        .label(attachment_download_button_label(downloading))
                                        .loading(downloading)
                                        .disabled(download_active)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.download_attachment(
                                                attachment_for_click.clone(),
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                        }))
                        .into_any_element()
                };
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(div().text_sm().font_semibold().child("Comments"))
                            .child(
                                Button::new("refresh-comments")
                                    .secondary()
                                    .outline()
                                    .compact()
                                    .label("Refresh comments")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.refresh_comments(cx)),
                                    ),
                            ),
                    )
                    .child(comments)
                    .child(div().text_sm().font_semibold().child("Attachments"))
                    .child(attachments)
                    .child(self.render_comment_composer(layout, cx))
                    .into_any_element()
            }
        }
    }

    fn status_control(
        &self,
        issue: Option<&IssueViewModel>,
        status: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_selected_issue = issue
            .map(|issue| self.selected_issue.as_ref() == Some(&issue.id))
            .unwrap_or(false);
        let is_remote_lookup = matches!(self.remote_lookup, RemoteLookupState::Loaded { .. });
        let editable = status_control_is_editable(
            self.workspace.is_some(),
            is_selected_issue,
            is_remote_lookup,
            self.operation_in_progress,
            self.issue_edit_flow.state(),
        );
        let ready = issue.is_some_and(|issue| {
            matches!(
                self.status_transition_state,
                StatusTransitionReadState::Ready { ref issue_id } if issue_id == &issue.id
            )
        });
        let trigger_disabled = !editable || !ready || self.status_transition_items.is_empty();
        let status_feedback = match &self.status_transition_state {
            StatusTransitionReadState::Error { issue_id, copy }
                if issue.map(|issue| &issue.id) == Some(issue_id) =>
            {
                let retry = Button::new("retry-status-transitions")
                    .ghost()
                    .compact()
                    .label("Retry")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.reload_status_transitions(cx);
                    }));
                Some(
                    h_flex()
                        .id("issue-status-feedback")
                        .debug_selector(|| "issue-status-feedback".to_owned())
                        .role(gpui::accesskit::Role::Alert)
                        .min_w_0()
                        .gap_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(div().min_w_0().child(copy.message().to_owned()))
                        .child(retry)
                        .into_any_element(),
                )
            }
            _ => None,
        };
        let status_aria_label = match &self.status_transition_state {
            StatusTransitionReadState::Loading { issue_id, .. }
                if issue.map(|issue| &issue.id) == Some(issue_id) =>
            {
                "Issue status · loading available transitions".to_owned()
            }
            StatusTransitionReadState::Error { issue_id, copy }
                if issue.map(|issue| &issue.id) == Some(issue_id) =>
            {
                format!("Issue status · transitions unavailable: {}", copy.message())
            }
            StatusTransitionReadState::Ready { issue_id }
                if issue.map(|issue| &issue.id) == Some(issue_id)
                    && self.status_transition_items.is_empty() =>
            {
                "Issue status · no status changes are currently available".to_owned()
            }
            _ if editable => "Change issue status".to_owned(),
            _ => "Issue status · editing unavailable in this view".to_owned(),
        };
        let trigger = Button::new("issue-status-trigger")
            .secondary()
            .dropdown_caret(true)
            .label(status)
            .disabled(trigger_disabled)
            .tooltip(status_aria_label.clone());
        let status_control = Popover::new("issue-status-popover")
            .anchor(Anchor::TopLeft)
            .open(self.status_popover_open)
            .on_open_change(cx.listener(|this, open, _, cx| {
                this.status_popover_open = *open;
                cx.notify();
            }))
            .trigger(trigger)
            .child(
                div()
                    .id("issue-status-transition-list")
                    .debug_selector(|| "issue-status-transition-list".to_owned())
                    .w_72()
                    .h(status_transition_list_height(
                        self.status_transition_items.len(),
                    ))
                    .child(List::new(
                        self.status_list.as_ref().expect("status list initialized"),
                    )),
            );
        v_flex()
            .id("issue-status-control")
            .debug_selector(|| "issue-status-control".to_owned())
            .min_w_0()
            .aria_label(status_aria_label)
            .child(status_control)
            .when_some(status_feedback, |this, feedback| this.child(feedback))
            .into_any_element()
    }

    fn render_issue_edit_controls(
        &self,
        issue: Option<&IssueViewModel>,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(issue) = issue else {
            return div().into_any_element();
        };
        if self.workspace.is_none() || self.selected_issue.as_ref() != Some(&issue.id) {
            return div().into_any_element();
        }
        let busy = self.operation_in_progress;
        let state = self.issue_edit_flow.state().clone();
        let controls = match state {
            IssueEditState::Idle => h_flex()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("change-assignee")
                        .secondary()
                        .outline()
                        .compact()
                        .label("Change assignee")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.begin_assignee_chooser(window, cx)
                        })),
                )
                .into_any_element(),
            IssueEditState::LoadingAssignees { .. } => h_flex()
                .gap_2()
                .child(Spinner::new())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Loading assignable users…"),
                )
                .child(
                    Button::new("cancel-assignee-load")
                        .compact()
                        .label("Cancel")
                        .disabled(busy)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_issue_edit(cx))),
                )
                .into_any_element(),
            IssueEditState::AssigneeChooser { users, .. } => {
                let no_users = users.is_empty();
                v_flex()
                    .gap_2()
                    .child(div().text_sm().font_semibold().child("Choose assignee"))
                    .when_some(self.assignee_input.clone(), |this, input| {
                        this.child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Input::new(&input)
                                        .cleanable(true)
                                        .aria_label("Filter assignees")
                                        .flex_1(),
                                )
                                .child(
                                    Button::new("search-assignees")
                                        .compact()
                                        .label("Search")
                                        .disabled(busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            let query = this
                                                .assignee_input
                                                .as_ref()
                                                .map(|input| input.read(cx).value().to_string())
                                                .unwrap_or_default();
                                            this.start_assignee_search(query, cx);
                                        })),
                                ),
                        )
                    })
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .child(
                                Button::new("assignee-unassigned")
                                    .compact()
                                    .label("Unassigned")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.choose_assignee(None, "Unassigned".to_owned(), cx)
                                    })),
                            )
                            .children(users.into_iter().enumerate().map(|(index, user)| {
                                let name = user.display_name.clone();
                                let account_id = user.account_id.clone();
                                Button::new(format!("assignee-{index}"))
                                    .compact()
                                    .label(name.clone())
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose_assignee(
                                            Some(account_id.clone()),
                                            name.clone(),
                                            cx,
                                        )
                                    }))
                            })),
                    )
                    .when(no_users, |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No assignable Jira users match this search."),
                        )
                    })
                    .child(
                        Button::new("cancel-assignee")
                            .compact()
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_issue_edit(cx))),
                    )
                    .into_any_element()
            }
            IssueEditState::ConfirmingAssignee {
                issue_key,
                display_name,
                ..
            } => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(format!("Assign {issue_key} to {display_name}?")),
                )
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("confirm-assignee")
                                .primary()
                                .label("Confirm change")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_assignee(window, cx)
                                })),
                        )
                        .child(
                            Button::new("cancel-assignee-confirmation")
                                .compact()
                                .label("Cancel")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_issue_edit(cx))),
                        ),
                )
                .into_any_element(),
            IssueEditState::ConfirmingTransition {
                issue_key,
                transition_name,
                target_status,
                ..
            } => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(format!(
                            "Move {issue_key} via {transition_name} to {target_status}?"
                        )),
                )
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("confirm-transition")
                                .primary()
                                .label("Confirm change")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.submit_transition(window, cx)
                                })),
                        )
                        .child(
                            Button::new("cancel-transition-confirmation")
                                .compact()
                                .label("Cancel")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_issue_edit(cx))),
                        ),
                )
                .into_any_element(),
            IssueEditState::Submitting { target, .. } => h_flex()
                .gap_2()
                .child(Spinner::new())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Applying {target}…")),
                )
                .into_any_element(),
            IssueEditState::Error {
                copy, operation, ..
            } => v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(copy.message()),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .when(copy.recovery() == RecoveryDirective::Retry, |this| {
                            this.child(
                                Button::new("retry-issue-edit")
                                    .compact()
                                    .label("Choose again")
                                    .disabled(busy)
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| match operation {
                                            IssueEditOperation::Assignee => {
                                                this.begin_assignee_chooser(window, cx)
                                            }
                                            IssueEditOperation::Transition => {
                                                this.reload_status_transitions(cx)
                                            }
                                        },
                                    )),
                            )
                        })
                        .when(copy.recovery() == RecoveryDirective::Refresh, |this| {
                            this.child(
                                Button::new("refresh-after-issue-edit")
                                    .compact()
                                    .label("Refresh Jira")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.begin_refresh(window, cx)
                                    })),
                            )
                        }),
                )
                .into_any_element(),
        };
        v_flex()
            .gap_2()
            .child(div().text_sm().font_semibold().child("Jira issue actions"))
            .child(controls)
            .into_any_element()
    }

    fn render_comment_composer(&self, layout: LayoutMode, cx: &mut Context<Self>) -> AnyElement {
        let Some(input) = self.comment_input.as_ref() else {
            return div().into_any_element();
        };
        let input_body = input.read(cx).value().to_string();
        let body = self.comment_flow.composer_body(&input_body);
        let posting = self.comment_flow.is_posting();
        let editing_confirmed = self.comment_flow.is_confirming();
        let mut composer = v_flex()
            .min_w_0()
            .gap_2()
            .child(div().text_sm().font_semibold().child("Add comment"))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Plain text accepted · sent as safe Jira ADF"),
            )
            .child(
                Textarea::new(input)
                    .w_full()
                    .aria_label("Comment text")
                    .disabled(posting || editing_confirmed),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} characters · {} bytes",
                        body.chars().count(),
                        body.len()
                    )),
            );
        if let Some((issue_key, _, chars, bytes)) = self.comment_flow.confirmation_details() {
            composer = composer
                .child(
                    div()
                        .min_w_0()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child(format!(
                            "Post this comment to {issue_key}? {chars} characters · {bytes} bytes"
                        )),
                )
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("post-comment-now")
                                .primary()
                                .label("Post now")
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.post_comment(window, cx)
                                    }),
                                ),
                        )
                        .child(Button::new("cancel-comment").label("Cancel").on_click(
                            cx.listener(|this, _, _, cx| this.cancel_comment_confirmation(cx)),
                        )),
                );
        } else if posting {
            composer = composer.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Posting comment…"),
            );
        } else if let Some(copy) = self.comment_flow.error_details() {
            composer = composer
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(copy.message()),
                )
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("post-comment")
                                .primary()
                                .label("Post comment")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.begin_comment_confirmation(cx)
                                })),
                        )
                        .when(copy.recovery() == RecoveryDirective::Refresh, |this| {
                            this.child(
                                Button::new("refresh-comments-after-unknown")
                                    .secondary()
                                    .outline()
                                    .compact()
                                    .label("Refresh comments")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.refresh_comments(cx)),
                                    ),
                            )
                        }),
                );
        } else {
            composer = composer.child(
                Button::new("post-comment")
                    .primary()
                    .label("Post comment")
                    .on_click(cx.listener(|this, _, _, cx| this.begin_comment_confirmation(cx))),
            );
        }
        composer.into_any_element()
    }

    fn pill(&self, label: String, cx: &mut Context<Self>) -> AnyElement {
        div()
            .px_2()
            .py_1()
            .rounded_full()
            .bg(cx.theme().secondary)
            .text_color(cx.theme().secondary_foreground)
            .text_xs()
            .child(label)
            .into_any_element()
    }
}
