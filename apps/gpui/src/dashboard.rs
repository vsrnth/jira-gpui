use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, StyledExt as _, button::Button, button::ButtonVariants as _, h_flex, v_flex,
};
use jira_application::{ApplicationError, CancellationToken, SyncMode};
use jira_domain::{Issue, User};

use crate::{
    config::{LiveSession, StartupError},
    live_workspace::{CachedWorkspace, LiveWorkspace},
    presentation::{IssueViewModel, UpdateViewModel},
    sample_data::{sample_issues, sample_updates, sample_users},
};

fn safe_sync_error(error: &ApplicationError) -> &'static str {
    match error.kind() {
        jira_application::ErrorKind::Authentication => {
            "Refresh failed · Jira authentication was rejected"
        }
        jira_application::ErrorKind::Authorization => {
            "Refresh failed · Jira authorization was denied"
        }
        jira_application::ErrorKind::RateLimited => "Refresh paused · Jira rate limit reached",
        jira_application::ErrorKind::Offline => "Refresh failed · Jira is unreachable",
        jira_application::ErrorKind::Cancelled => "Refresh cancelled",
        jira_application::ErrorKind::InvalidInput => "Refresh failed · invalid request",
        jira_application::ErrorKind::NotFound => "Refresh failed · Jira site was not found",
        jira_application::ErrorKind::Upstream => "Refresh failed · Jira returned an error",
        jira_application::ErrorKind::Storage
        | jira_application::ErrorKind::Notification
        | jira_application::ErrorKind::Internal => "Refresh failed · local application error",
    }
}

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
    workspace: Option<Arc<LiveWorkspace>>,
    users: Vec<User>,
    workspace_name: String,
    workspace_members: String,
    site_label: String,
    mode_label: String,
    operation_in_progress: bool,
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
            workspace: None,
            users,
            workspace_name: "Platform team".to_owned(),
            workspace_members: "Amina, Devon, Marco".to_owned(),
            site_label: "sample.atlassian.net".to_owned(),
            mode_label: "Local preview mode".to_owned(),
            operation_in_progress: false,
        }
    }

    pub fn from_live(session: LiveSession, cx: &mut Context<Self>) -> Self {
        let workspace_members = session
            .assignees
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let dashboard = Self {
            section: Section::Issues,
            issues: Vec::new(),
            updates: Vec::new(),
            selected_issue: 0,
            sync_message: "Opening local cache…".to_owned(),
            workspace: None,
            users: Vec::new(),
            workspace_name: "Configured user set".to_owned(),
            workspace_members,
            site_label: session.site_label,
            mode_label: "Live read-only sync · best-effort desktop notifications".to_owned(),
            operation_in_progress: true,
        };

        let site_id = session.site_id;
        let assignees = session.assignees;
        let jira = session.jira;
        let cache = session.cache;
        cx.spawn(async move |this, cx| {
            let result = match LiveWorkspace::initialize(site_id, assignees, jira, cache).await {
                Ok(workspace) => {
                    let workspace = Arc::new(workspace);
                    workspace
                        .load_cached()
                        .await
                        .map(|cached| (workspace, cached))
                }
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok((workspace, cached)) => {
                        let issue_count = cached.issues.len();
                        let update_count = cached.events.len();
                        this.workspace = Some(workspace);
                        this.apply_cached(cached);
                        this.sync_message =
                            format!("Ready · cached {issue_count} issues · {update_count} updates");
                    }
                    Err(error) => {
                        this.sync_message = safe_sync_error(&error).to_owned();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        dashboard
    }

    pub fn from_configuration_error(error: StartupError) -> Self {
        let mut dashboard = Self::from_sample_data();
        dashboard.issues.clear();
        dashboard.updates.clear();
        dashboard.selected_issue = 0;
        dashboard.users.clear();
        dashboard.workspace_name = "Configuration required".to_owned();
        dashboard.workspace_members = "Set all five Jira environment variables".to_owned();
        dashboard.site_label = "Jira site unavailable".to_owned();
        dashboard.mode_label = "Startup configuration error".to_owned();
        dashboard.sync_message = format!("Configuration error · {error}");
        dashboard
    }

    fn apply_live_issues(&mut self, issues: Vec<Issue>) {
        self.issues = issues
            .iter()
            .map(|issue| IssueViewModel::from_domain(issue, &self.users))
            .collect();
        self.selected_issue = self.selected_issue.min(self.issues.len().saturating_sub(1));
    }

    fn apply_cached(&mut self, cached: CachedWorkspace) {
        let CachedWorkspace { issues, events } = cached;
        let updates = events
            .iter()
            .map(|event| {
                let issue = issues.iter().find(|issue| issue.id == event.issue_id);
                UpdateViewModel::from_domain(event, issue)
            })
            .collect();
        self.apply_live_issues(issues);
        self.updates = updates;
    }

    fn begin_refresh(&mut self, cx: &mut Context<Self>) {
        if self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.sync_message = "Refresh unavailable · local workspace is not ready".to_owned();
            cx.notify();
            return;
        };
        let cancellation = CancellationToken::new();
        self.operation_in_progress = true;
        self.sync_message = "Refreshing Jira…".to_owned();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = workspace.refresh(&cancellation).await;
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(outcome) => {
                        let mode = match outcome.outcome.mode {
                            SyncMode::Baseline => "baseline",
                            SyncMode::Incremental => "incremental",
                            SyncMode::Reconciliation => "reconciliation",
                        };
                        let issue_count = outcome.cached.issues.len();
                        let new_event_count = outcome.outcome.events_inserted;
                        let event_count = outcome.cached.events.len();
                        let notifications_delivered = outcome.outcome.notifications_delivered;
                        let notification_failures = outcome.outcome.notification_failures;
                        this.apply_cached(outcome.cached);
                        this.sync_message = format!(
                            "Refresh complete · {issue_count} issues · {new_event_count} new updates · {event_count} in inbox · desktop notifications: {notifications_delivered} delivered, {notification_failures} unavailable · {mode}"
                        );
                    }
                    Err(error) => {
                        this.sync_message = safe_sync_error(&error).to_owned();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            for update in &mut self.updates {
                update.unread = false;
            }
            cx.notify();
            return;
        };
        if self.operation_in_progress {
            return;
        }
        self.operation_in_progress = true;
        self.sync_message = "Marking updates read…".to_owned();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = workspace.mark_all_read().await;
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(result) => {
                        this.apply_cached(result.cached);
                        this.sync_message = format!("Marked {} updates read", result.changed);
                    }
                    Err(error) => this.sync_message = safe_sync_error(&error).to_owned(),
                }
                cx.notify();
            });
        })
        .detach();
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
                            .child("USER SET"),
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
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(self.workspace_name.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.workspace_members.clone()),
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
                            .child(self.site_label.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.mode_label.clone()),
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
                        Section::Issues => "Issues for selected users",
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
                Button::new("refresh")
                    .primary()
                    .label(if self.operation_in_progress {
                        "Refreshing…"
                    } else {
                        "Refresh"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.begin_refresh(cx))),
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
        let issue = self.issues.get(self.selected_issue);
        let project = issue
            .map(|issue| issue.project.clone())
            .unwrap_or_else(|| "Jira".to_owned());
        let key = issue
            .map(|issue| issue.key.clone())
            .unwrap_or_else(|| "—".to_owned());
        let summary = issue
            .map(|issue| issue.summary.clone())
            .unwrap_or_else(|| "No issues loaded".to_owned());
        let issue_type = issue
            .map(|issue| issue.issue_type.clone())
            .unwrap_or_else(|| "—".to_owned());
        let status = issue
            .map(|issue| issue.status.clone())
            .unwrap_or_else(|| "Ready to refresh".to_owned());
        let priority = issue
            .map(|issue| issue.priority.clone())
            .unwrap_or_else(|| "—".to_owned());
        let description = issue.map_or_else(
            || "Refresh from Jira to populate this view.".to_owned(),
            |issue| issue.description.clone(),
        );
        let assignee = issue
            .map(|issue| issue.assignee.clone())
            .unwrap_or_else(|| "—".to_owned());
        let reporter = issue
            .map(|issue| issue.reporter.clone())
            .unwrap_or_else(|| "—".to_owned());
        let status_category = issue
            .map(|issue| issue.status_category.clone())
            .unwrap_or_else(|| "—".to_owned());
        let parent = issue
            .and_then(|issue| issue.parent.clone())
            .unwrap_or_else(|| "None".to_owned());
        let created = issue
            .map(|issue| issue.created.clone())
            .unwrap_or_else(|| "—".to_owned());
        let updated = issue
            .map(|issue| issue.updated.clone())
            .unwrap_or_else(|| "—".to_owned());
        let due_date = issue
            .map(|issue| issue.due_date.clone())
            .unwrap_or_else(|| "—".to_owned());
        let labels = issue.map(|issue| issue.labels.clone()).unwrap_or_default();
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
                            .child(project)
                            .child("/")
                            .child(key),
                    )
                    .child(div().text_2xl().font_semibold().child(summary))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(self.pill(issue_type, cx))
                            .child(self.pill(status, cx))
                            .child(self.pill(priority, cx)),
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
                            .child(description),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().font_semibold().child("Details"))
                    .child(self.detail_field("Assignee", assignee, cx))
                    .child(self.detail_field("Reporter", reporter, cx))
                    .child(self.detail_field("Status category", status_category, cx))
                    .child(self.detail_field("Parent", parent, cx))
                    .child(self.detail_field("Created", created, cx))
                    .child(self.detail_field("Updated", updated, cx))
                    .child(self.detail_field("Due date", due_date, cx)),
            )
            .when(!labels.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_semibold().child("Labels"))
                        .child(
                            h_flex()
                                .gap_2()
                                .children(labels.iter().cloned().map(|label| self.pill(label, cx))),
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
                            .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
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
