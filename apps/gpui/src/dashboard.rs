use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, StyledExt as _, button::Button, button::ButtonVariants as _, h_flex, v_flex,
};

use crate::{
    presentation::{IssueViewModel, UpdateViewModel},
    sample_data::{sample_issues, sample_updates, sample_users},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Section {
    Issues,
    Updates,
}

pub struct Dashboard {
    section: Section,
    issues: Vec<IssueViewModel>,
    updates: Vec<UpdateViewModel>,
    selected_issue: usize,
    sync_message: String,
}

impl Dashboard {
    pub fn from_sample_data() -> Self {
        let domain_issues = sample_issues();
        let users = sample_users();
        let updates = sample_updates()
            .iter()
            .map(|event| {
                let issue = domain_issues
                    .iter()
                    .find(|issue| issue.id == event.issue_id);
                UpdateViewModel::from_domain(event, issue)
            })
            .collect();
        let issues = domain_issues
            .iter()
            .map(|issue| IssueViewModel::from_domain(issue, &users))
            .collect();

        Self {
            section: Section::Issues,
            issues,
            updates,
            selected_issue: 0,
            sync_message: "Preview data · Jira connection not configured".to_owned(),
        }
    }

    fn unread_count(&self) -> usize {
        self.updates.iter().filter(|update| update.unread).count()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .h_full()
            .w(px(236.))
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .child(
                h_flex()
                    .h(px(72.))
                    .px_4()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div()
                            .flex()
                            .size_9()
                            .items_center()
                            .justify_center()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().sidebar_primary)
                            .text_color(cx.theme().sidebar_primary_foreground)
                            .font_bold()
                            .child("JD"),
                    )
                    .child(
                        v_flex()
                            .gap_0p5()
                            .child(div().font_semibold().child("Jira Desk"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Read-only workspace"),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_3()
                    .gap_1()
                    .child(
                        div()
                            .px_3()
                            .pt_2()
                            .pb_1()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("WORKSPACE"),
                    )
                    .child(self.nav_item(
                        "Issues",
                        self.issues.len(),
                        self.section == Section::Issues,
                        Section::Issues,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Updates",
                        self.unread_count(),
                        self.section == Section::Updates,
                        Section::Updates,
                        cx,
                    ))
                    .child(
                        div()
                            .mt_5()
                            .px_3()
                            .pt_2()
                            .pb_1()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("SAVED USER SET"),
                    )
                    .child(
                        v_flex()
                            .mx_1()
                            .mt_1()
                            .p_3()
                            .gap_1()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().sidebar_border)
                            .child(div().text_sm().font_semibold().child("Platform team"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Amina, Devon, Marco"),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .p_4()
                    .gap_1()
                    .border_t_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child("sample.atlassian.net"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Local preview mode"),
                    ),
            )
    }

    fn nav_item(
        &self,
        label: &'static str,
        count: usize,
        selected: bool,
        section: Section,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .id(label)
            .w_full()
            .px_3()
            .py_2()
            .justify_between()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .when(selected, |this| {
                this.bg(cx.theme().sidebar_accent)
                    .text_color(cx.theme().sidebar_accent_foreground)
            })
            .when(!selected, |this| {
                this.hover(|style| style.bg(cx.theme().sidebar_accent))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.section = section;
                cx.notify();
            }))
            .child(div().text_sm().font_semibold().child(label))
            .child(
                div()
                    .min_w(px(26.))
                    .px_2()
                    .py_0p5()
                    .rounded_full()
                    .bg(cx.theme().muted)
                    .text_center()
                    .text_xs()
                    .child(count.to_string()),
            )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(px(72.))
            .px_5()
            .flex_shrink_0()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_lg().font_semibold().child(match self.section {
                        Section::Issues => "Issues for Platform team",
                        Section::Updates => "Update inbox",
                    }))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.sync_message.clone()),
                    ),
            )
            .child(
                Button::new("pull-updates")
                    .primary()
                    .label("Pull updates")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.sync_message =
                            "Sync requested · live Jira transport is the next adapter step"
                                .to_owned();
                        cx.notify();
                    })),
            )
    }

    fn render_issues(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .size_full()
            .min_w_0()
            .child(
                v_flex()
                    .h_full()
                    .w(px(494.))
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .h(px(44.))
                            .px_4()
                            .flex_shrink_0()
                            .justify_between()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} matching issues", self.issues.len()))
                            .child("Updated newest first"),
                    )
                    .child(
                        v_flex()
                            .id("issue-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .children(
                                self.issues
                                    .iter()
                                    .enumerate()
                                    .map(|(index, issue)| self.issue_row(index, issue, cx)),
                            ),
                    ),
            )
            .child(self.issue_detail(cx))
    }

    fn issue_row(
        &self,
        index: usize,
        issue: &IssueViewModel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = index == self.selected_issue;
        v_flex()
            .id(("issue-row", index))
            .w_full()
            .p_4()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .when(selected, |this| this.bg(cx.theme().list_active))
            .when(!selected, |this| {
                this.hover(|style| style.bg(cx.theme().list_hover))
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected_issue = index;
                cx.notify();
            }))
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().link)
                                    .child(issue.key.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(issue.issue_type.clone()),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(cx.theme().secondary)
                            .text_color(cx.theme().secondary_foreground)
                            .text_xs()
                            .child(issue.status.clone()),
                    ),
            )
            .child(div().text_sm().font_semibold().child(issue.summary.clone()))
            .child(
                h_flex()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{} · {}", issue.assignee, issue.priority))
                    .child(issue.updated.clone()),
            )
            .into_any_element()
    }

    fn issue_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let issue = &self.issues[self.selected_issue];
        v_flex()
            .id("issue-detail")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .p_6()
            .gap_5()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(issue.project.clone())
                            .child("/")
                            .child(issue.key.clone()),
                    )
                    .child(
                        div()
                            .text_2xl()
                            .font_semibold()
                            .child(issue.summary.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.pill(issue.issue_type.clone(), cx))
                            .child(self.pill(issue.status.clone(), cx))
                            .child(self.pill(issue.priority.clone(), cx)),
                    ),
            )
            .child(
                v_flex()
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
                            .child(issue.description.clone()),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().font_semibold().child("Details"))
                    .child(self.detail_field("Assignee", issue.assignee.clone(), cx))
                    .child(self.detail_field("Reporter", issue.reporter.clone(), cx))
                    .child(self.detail_field("Status category", issue.status_category.clone(), cx))
                    .child(self.detail_field(
                        "Parent",
                        issue.parent.clone().unwrap_or_else(|| "None".to_owned()),
                        cx,
                    ))
                    .child(self.detail_field("Created", issue.created.clone(), cx))
                    .child(self.detail_field("Updated", issue.updated.clone(), cx))
                    .child(self.detail_field("Due date", issue.due_date.clone(), cx)),
            )
            .when(!issue.labels.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_semibold().child("Labels"))
                        .child(
                            h_flex().gap_2().children(
                                issue
                                    .labels
                                    .iter()
                                    .cloned()
                                    .map(|label| self.pill(label, cx)),
                            ),
                        ),
                )
            })
    }

    fn detail_field(
        &self,
        label: &'static str,
        value: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .items_start()
            .child(
                div()
                    .w(px(132.))
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(div().min_w_0().text_sm().child(value))
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

    fn render_updates(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                h_flex()
                    .h(px(54.))
                    .px_5()
                    .flex_shrink_0()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} unread local updates", self.unread_count())),
                    )
                    .child(
                        Button::new("mark-all-read")
                            .ghost()
                            .label("Mark all read")
                            .on_click(cx.listener(|this, _, _, cx| {
                                for update in &mut this.updates {
                                    update.unread = false;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("update-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_5()
                    .gap_3()
                    .children(
                        self.updates
                            .iter()
                            .enumerate()
                            .map(|(index, update)| self.update_card(index, update, cx)),
                    ),
            )
    }

    fn update_card(
        &self,
        index: usize,
        update: &UpdateViewModel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id(("update-card", index))
            .w_full()
            .items_start()
            .gap_3()
            .p_4()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .when(update.unread, |this| this.bg(cx.theme().list_active))
            .child(
                div()
                    .mt_1()
                    .size_2()
                    .flex_shrink_0()
                    .rounded_full()
                    .when(update.unread, |this| this.bg(cx.theme().primary))
                    .when(!update.unread, |this| this.bg(cx.theme().muted)),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .gap_1()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(cx.theme().link)
                                            .child(update.issue_key.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .child(update.issue_summary.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(update.occurred_at.clone()),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(update.change.clone()),
                    ),
            )
            .into_any_element()
    }
}

impl Render for Dashboard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.section {
            Section::Issues => self.render_issues(cx).into_any_element(),
            Section::Updates => self.render_updates(cx).into_any_element(),
        };

        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.render_sidebar(cx))
            .child(
                v_flex()
                    .h_full()
                    .min_w_0()
                    .flex_1()
                    .child(self.render_header(cx))
                    .child(div().min_h_0().flex_1().child(content)),
            )
    }
}
