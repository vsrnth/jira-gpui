use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, StyledExt as _, button::Button,
    button::ButtonVariants as _, h_flex, v_flex,
};
use jira_application::{ApplicationError, CancellationToken, DefaultPollingPolicy, SyncMode};
use jira_domain::{AccountId, Issue, IssueId, User};

use crate::{
    config::{LiveSession, StartupError, ensure_authenticated_user},
    live_workspace::{CachedWorkspace, LiveWorkspace, RefreshResult},
    presentation::{
        IssueDetailViewModel, IssueStatusFilter, IssueViewModel, UpdateViewModel,
        issue_views_for_filter,
    },
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

fn safe_detail_error(error: &ApplicationError) -> &'static str {
    match error.kind() {
        jira_application::ErrorKind::Authentication => {
            "Issue details unavailable · Jira authentication was rejected"
        }
        jira_application::ErrorKind::Authorization => {
            "Issue details unavailable · Jira authorization was denied"
        }
        jira_application::ErrorKind::NotFound => {
            "Issue details unavailable · Jira issue was not found"
        }
        jira_application::ErrorKind::RateLimited => {
            "Issue details unavailable · Jira rate limit reached"
        }
        jira_application::ErrorKind::Offline => "Issue details unavailable · Jira is unreachable",
        jira_application::ErrorKind::Cancelled => "Issue details request cancelled",
        jira_application::ErrorKind::InvalidInput
        | jira_application::ErrorKind::Upstream
        | jira_application::ErrorKind::Storage
        | jira_application::ErrorKind::Notification
        | jira_application::ErrorKind::Internal => {
            "Issue details unavailable · Jira returned an error"
        }
    }
}

fn refresh_complete_message(result: &RefreshResult) -> String {
    let mode = match result.outcome.mode {
        SyncMode::Baseline => "baseline",
        SyncMode::Incremental => "incremental",
        SyncMode::Reconciliation => "reconciliation",
    };
    format!(
        "Refresh complete · {} issues · {} new updates · {} in inbox · desktop notifications: {} delivered, {} unavailable · {mode}",
        result.cached.issues.len(),
        result.outcome.events_inserted,
        result.cached.events.len(),
        result.outcome.notifications_delivered,
        result.outcome.notification_failures,
    )
}

fn authenticated_identity(user: Option<User>) -> (Vec<User>, Option<AccountId>) {
    let account = user.as_ref().map(|user| user.account_id.clone());
    (user.into_iter().collect(), account)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Section {
    Issues,
    Updates,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IssueScope {
    All,
    Mine,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DetailState {
    Empty,
    Loading { issue_id: IssueId },
    Loaded(IssueDetailViewModel),
    Error { issue_id: IssueId, message: String },
}

fn detail_result_is_current(
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
    generation: u64,
    expected_generation: u64,
) -> bool {
    generation == expected_generation && selected_issue == Some(expected_issue)
}

pub struct Dashboard {
    section: Section,
    domain_issues: Vec<Issue>,
    issues: Vec<IssueViewModel>,
    updates: Vec<UpdateViewModel>,
    selected_issue: Option<IssueId>,
    sync_message: String,
    workspace: Option<Arc<LiveWorkspace>>,
    users: Vec<User>,
    workspace_name: String,
    workspace_members: String,
    site_label: String,
    mode_label: String,
    operation_in_progress: bool,
    polling_task: Option<gpui::Task<()>>,
    automatic_polling_paused: bool,
    issue_scope: IssueScope,
    authenticated_account: Option<AccountId>,
    status_filter: IssueStatusFilter,
    detail_state: DetailState,
    detail_generation: u64,
    detail_cancellation: Option<CancellationToken>,
    detail_task: Option<gpui::Task<()>>,
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
        let issues = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::All);
        let selected_issue = issues.first().map(|issue| issue.id.clone());

        Self {
            section: Section::Issues,
            domain_issues,
            issues,
            updates,
            selected_issue,
            sync_message: "Preview data · Jira connection not configured".to_owned(),
            workspace: None,
            users,
            workspace_name: "Platform team".to_owned(),
            workspace_members: "Amina, Devon, Marco".to_owned(),
            site_label: "sample.atlassian.net".to_owned(),
            mode_label: "Local preview mode".to_owned(),
            operation_in_progress: false,
            polling_task: None,
            automatic_polling_paused: false,
            issue_scope: IssueScope::All,
            authenticated_account: None,
            status_filter: IssueStatusFilter::All,
            detail_state: DetailState::Empty,
            detail_generation: 0,
            detail_cancellation: None,
            detail_task: None,
        }
    }

    pub fn from_live(session: LiveSession, cx: &mut Context<Self>) -> Self {
        let (users, initial_authenticated_account) =
            authenticated_identity(session.authenticated_user.clone());
        let dashboard = Self {
            section: Section::Issues,
            domain_issues: Vec::new(),
            issues: Vec::new(),
            updates: Vec::new(),
            selected_issue: None,
            sync_message: "Opening local cache…".to_owned(),
            workspace: None,
            users,
            workspace_name: "Jira Project".to_owned(),
            workspace_members: if initial_authenticated_account.is_some() {
                "Authenticated Jira account".to_owned()
            } else {
                "Environment bootstrap · My issues unavailable".to_owned()
            },
            site_label: session.site_label,
            mode_label: "Live read-only sync · best-effort desktop notifications".to_owned(),
            operation_in_progress: true,
            polling_task: None,
            automatic_polling_paused: false,
            issue_scope: IssueScope::All,
            authenticated_account: initial_authenticated_account,
            status_filter: IssueStatusFilter::All,
            detail_state: DetailState::Empty,
            detail_generation: 0,
            detail_cancellation: None,
            detail_task: None,
        };

        let site_id = session.site_id;
        let initial_authenticated_user = session.authenticated_user;
        let jira = session.jira;
        let cache = session.cache;
        cx.spawn(async move |this, cx| {
            let result = match ensure_authenticated_user(
                initial_authenticated_user,
                jira.as_ref(),
                &site_id,
            )
            .await
            {
                Ok(authenticated_user) => {
                    let authenticated_account = authenticated_user.account_id.clone();
                    match LiveWorkspace::initialize(
                        site_id,
                        Some(authenticated_account),
                        jira,
                        cache,
                    )
                    .await
                    {
                        Ok(workspace) => {
                            let workspace = Arc::new(workspace);
                            workspace
                                .load_cached()
                                .await
                                .map(|cached| (workspace, cached, authenticated_user))
                                .map_err(|error| safe_sync_error(&error).to_owned())
                        }
                        Err(error) => Err(safe_sync_error(&error).to_owned()),
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok((workspace, cached, authenticated_user)) => {
                        let issue_count = cached.issues.len();
                        let update_count = cached.events.len();
                        this.users = vec![authenticated_user.clone()];
                        this.authenticated_account = Some(authenticated_user.account_id.clone());
                        this.workspace_members = "Authenticated Jira account".to_owned();
                        this.workspace = Some(workspace);
                        this.apply_cached(cached, cx);
                        this.start_automatic_polling(cx);
                        this.sync_message =
                            format!("Ready · cached {issue_count} issues · {update_count} updates");
                    }
                    Err(error) => this.sync_message = format!("Startup error · {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        dashboard
    }

    fn start_automatic_polling(&mut self, cx: &mut Context<Self>) {
        if self.polling_task.is_some() && !self.automatic_polling_paused {
            return;
        }
        self.polling_task.take();
        self.automatic_polling_paused = false;
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let issue_scope = self.issue_scope;
        let authenticated_account = self.authenticated_account.clone();

        let policy = DefaultPollingPolicy;
        let task = cx.spawn(async move |this, cx| {
            let mut delay = policy.next_delay_after_success();
            let mut consecutive_failures: u32 = 0;
            loop {
                cx.background_executor().timer(delay).await;
                let should_refresh = match this.update(cx, |this, cx| {
                    if this.operation_in_progress {
                        false
                    } else {
                        this.operation_in_progress = true;
                        this.sync_message = "Automatic refresh…".to_owned();
                        cx.notify();
                        true
                    }
                }) {
                    Ok(should_refresh) => should_refresh,
                    Err(_) => break,
                };
                if !should_refresh {
                    continue;
                }

                let cancellation = CancellationToken::new();
                let result = workspace.refresh_automatically(&cancellation).await;
                let result = match result {
                    Ok(mut result)
                        if issue_scope == IssueScope::Mine
                            && authenticated_account.as_ref().is_some() =>
                    {
                        let account = authenticated_account
                            .clone()
                            .expect("authenticated account checked above");
                        match workspace.load_cached_for_assignee(account).await {
                            Ok(cached) => {
                                result.cached = cached;
                                Ok(result)
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Ok(result) => Ok(result),
                    Err(error) => Err(error),
                };
                let next_delay = match this.update(cx, |this, cx| {
                    this.operation_in_progress = false;
                    match result {
                        Ok(result) => {
                            consecutive_failures = 0;
                            this.sync_message = refresh_complete_message(&result);
                            this.apply_cached(result.cached, cx);
                            cx.notify();
                            Some(policy.next_delay_after_success())
                        }
                        Err(error) => {
                            let next = policy.next_delay_after_failure(
                                &error,
                                consecutive_failures.saturating_add(1),
                            );
                            if next.is_some() {
                                consecutive_failures = consecutive_failures.saturating_add(1);
                            }
                            this.sync_message = if next.is_none() {
                                format!("{} · automatic polling paused", safe_sync_error(&error))
                            } else {
                                safe_sync_error(&error).to_owned()
                            };
                            if next.is_none() {
                                this.automatic_polling_paused = true;
                            }
                            cx.notify();
                            next
                        }
                    }
                }) {
                    Ok(next) => next,
                    Err(_) => break,
                };
                let Some(next_delay) = next_delay else {
                    break;
                };
                delay = next_delay;
            }
        });
        self.polling_task = Some(task);
    }

    pub fn from_configuration_error(error: StartupError) -> Self {
        let mut dashboard = Self::from_sample_data();
        dashboard.domain_issues.clear();
        dashboard.issues.clear();
        dashboard.updates.clear();
        dashboard.selected_issue = None;
        dashboard.invalidate_detail_selection();
        dashboard.users.clear();
        dashboard.workspace_name = "Jira Project".to_owned();
        dashboard.workspace_members = "Connect Jira to load this view".to_owned();
        dashboard.site_label = "Jira site unavailable".to_owned();
        dashboard.mode_label = "Startup configuration error".to_owned();
        dashboard.sync_message = format!("Configuration error · {error}");
        dashboard
    }

    fn apply_live_issues(
        &mut self,
        issues: Vec<Issue>,
        refresh_detail: bool,
        cx: &mut Context<Self>,
    ) {
        self.domain_issues = issues;
        self.rebuild_issue_views(refresh_detail, cx);
    }

    fn rebuild_issue_views(&mut self, refresh_detail: bool, cx: &mut Context<Self>) {
        self.issues = issue_views_for_filter(&self.domain_issues, &self.users, self.status_filter);
        let selected_visible = self
            .selected_issue
            .as_ref()
            .is_some_and(|selected| self.issues.iter().any(|issue| &issue.id == selected));
        if self.selected_issue.is_some() && !selected_visible {
            self.invalidate_detail_selection();
        }
        if self.selected_issue.is_none() {
            if let Some(issue_id) = self.issues.first().map(|issue| issue.id.clone()) {
                self.select_issue(issue_id, cx, true);
            }
        } else if refresh_detail {
            self.reload_selected_detail(cx);
        }
    }

    fn set_status_filter(&mut self, filter: IssueStatusFilter, cx: &mut Context<Self>) {
        if self.status_filter == filter {
            return;
        }
        self.status_filter = filter;
        self.rebuild_issue_views(false, cx);
        cx.notify();
    }

    fn invalidate_detail_selection(&mut self) {
        if let Some(cancellation) = self.detail_cancellation.take() {
            cancellation.cancel();
        }
        self.detail_task.take();
        self.detail_generation = self.detail_generation.wrapping_add(1);
        self.selected_issue = None;
        self.detail_state = DetailState::Empty;
    }

    fn select_issue(&mut self, issue_id: IssueId, cx: &mut Context<Self>, force: bool) {
        if self.selected_issue.as_ref() == Some(&issue_id)
            && !force
            && matches!(
                self.detail_state,
                DetailState::Loading { .. } | DetailState::Loaded(_)
            )
        {
            return;
        }
        if let Some(cancellation) = self.detail_cancellation.take() {
            cancellation.cancel();
        }
        self.detail_task.take();
        self.detail_generation = self.detail_generation.wrapping_add(1);
        let generation = self.detail_generation;
        self.selected_issue = Some(issue_id.clone());

        let Some(workspace) = self.workspace.clone() else {
            self.detail_state = DetailState::Empty;
            cx.notify();
            return;
        };

        let cancellation = CancellationToken::new();
        self.detail_cancellation = Some(cancellation.clone());
        self.detail_state = DetailState::Loading {
            issue_id: issue_id.clone(),
        };
        let task = cx.spawn(async move |this, cx| {
            let result = workspace
                .fetch_issue_detail(issue_id.clone(), &cancellation)
                .await;
            let _ = this.update(cx, |this, cx| {
                if !detail_result_is_current(
                    this.selected_issue.as_ref(),
                    &issue_id,
                    this.detail_generation,
                    generation,
                ) {
                    return;
                }
                this.detail_cancellation = None;
                this.detail_task = None;
                this.detail_state = match result {
                    Ok(detail) => DetailState::Loaded(IssueDetailViewModel::from_domain(&detail)),
                    Err(error) => DetailState::Error {
                        issue_id: issue_id.clone(),
                        message: safe_detail_error(&error).to_owned(),
                    },
                };
                cx.notify();
            });
        });
        self.detail_task = Some(task);
        cx.notify();
    }

    fn reload_selected_detail(&mut self, cx: &mut Context<Self>) {
        let Some(issue_id) = self.selected_issue.clone() else {
            return;
        };
        self.select_issue(issue_id, cx, true);
    }

    fn apply_cached(&mut self, cached: CachedWorkspace, cx: &mut Context<Self>) {
        let CachedWorkspace { issues, events } = cached;
        let updates = events
            .iter()
            .map(|event| {
                let issue = issues.iter().find(|issue| issue.id == event.issue_id);
                UpdateViewModel::from_domain(event, issue)
            })
            .collect();
        self.apply_live_issues(issues, true, cx);
        self.updates = updates;
    }

    fn set_issue_scope(&mut self, scope: IssueScope, cx: &mut Context<Self>) {
        if self.issue_scope == scope || self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let Some(account_id) = self.authenticated_account.clone() else {
            return;
        };
        self.issue_scope = scope;
        self.polling_task.take();
        self.operation_in_progress = true;
        self.sync_message = match scope {
            IssueScope::All => "Loading all cached Jira Project issues…".to_owned(),
            IssueScope::Mine => "Loading your cached Jira Project issues…".to_owned(),
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = match scope {
                IssueScope::All => workspace.load_cached().await,
                IssueScope::Mine => workspace.load_cached_for_assignee(account_id).await,
            };
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(cached) => {
                        this.apply_cached(cached, cx);
                        this.start_automatic_polling(cx);
                        this.sync_message = match scope {
                            IssueScope::All => {
                                "Showing all cached Jira Project issues".to_owned()
                            }
                            IssueScope::Mine => {
                                "Showing your cached Jira Project issues".to_owned()
                            }
                        };
                    }
                    Err(error) => {
                        this.sync_message = safe_sync_error(&error).to_owned();
                        this.start_automatic_polling(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
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
        let issue_scope = self.issue_scope;
        let authenticated_account = self.authenticated_account.clone();
        self.operation_in_progress = true;
        self.sync_message = "Refreshing Jira…".to_owned();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = workspace.refresh(&cancellation).await;
            let result = match result {
                Ok(mut result)
                    if issue_scope == IssueScope::Mine
                        && authenticated_account.as_ref().is_some() =>
                {
                    let account = authenticated_account
                        .clone()
                        .expect("authenticated account checked above");
                    match workspace.load_cached_for_assignee(account).await {
                        Ok(cached) => {
                            result.cached = cached;
                            Ok(result)
                        }
                        Err(error) => Err(error),
                    }
                }
                Ok(result) => Ok(result),
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(outcome) => {
                        this.sync_message = refresh_complete_message(&outcome);
                        this.apply_cached(outcome.cached, cx);
                        this.start_automatic_polling(cx);
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
        let issue_scope = self.issue_scope;
        let authenticated_account = self.authenticated_account.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = workspace.mark_all_read().await;
            let result = match result {
                Ok(mut result)
                    if issue_scope == IssueScope::Mine
                        && authenticated_account.as_ref().is_some() =>
                {
                    let account = authenticated_account
                        .clone()
                        .expect("authenticated account checked above");
                    match workspace.load_cached_for_assignee(account).await {
                        Ok(cached) => {
                            result.cached = cached;
                            Ok(result)
                        }
                        Err(error) => Err(error),
                    }
                }
                Ok(result) => Ok(result),
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(result) => {
                        this.apply_cached(result.cached, cx);
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
                            .child("VIEW"),
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
                        Section::Issues => "Jira Project issues",
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
                h_flex()
                    .gap_2()
                    .when(self.section == Section::Issues, |this| {
                        this.child(
                            Button::new("all-issues")
                                .label("All issues")
                                .when(self.issue_scope == IssueScope::All, |button| {
                                    button.primary()
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_issue_scope(IssueScope::All, cx)
                                })),
                        )
                        .child(
                            Button::new("my-issues")
                                .label("My issues")
                                .when(self.issue_scope == IssueScope::Mine, |button| {
                                    button.primary()
                                })
                                .disabled(self.authenticated_account.is_none())
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_issue_scope(IssueScope::Mine, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("refresh")
                            .primary()
                            .label(if self.operation_in_progress {
                                "Refreshing…"
                            } else {
                                "Refresh"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.begin_refresh(cx))),
                    ),
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
                            .child(format!(
                                "{} matching Jira Project issues · {}",
                                self.issues.len(),
                                match self.issue_scope {
                                    IssueScope::All => "All issues",
                                    IssueScope::Mine => "My issues",
                                }
                            ))
                            .child("Updated newest first"),
                    )
                    .child(
                        h_flex()
                            .h(px(44.))
                            .px_3()
                            .gap_1()
                            .flex_shrink_0()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(self.status_filter_button(IssueStatusFilter::All, cx))
                            .child(self.status_filter_button(IssueStatusFilter::ToDo, cx))
                            .child(self.status_filter_button(IssueStatusFilter::InProgress, cx))
                            .child(self.status_filter_button(IssueStatusFilter::Done, cx))
                            .child(self.status_filter_button(IssueStatusFilter::Uncategorized, cx)),
                    )
                    .child(
                        v_flex()
                            .id("issue-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .children(self.issues.iter().map(|issue| self.issue_row(issue, cx))),
                    ),
            )
            .child(self.issue_detail(cx))
    }

    fn status_filter_button(
        &self,
        filter: IssueStatusFilter,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = match filter {
            IssueStatusFilter::All => "status-filter-all",
            IssueStatusFilter::ToDo => "status-filter-to-do",
            IssueStatusFilter::InProgress => "status-filter-in-progress",
            IssueStatusFilter::Done => "status-filter-done",
            IssueStatusFilter::Uncategorized => "status-filter-uncategorized",
        };
        Button::new(id)
            .label(filter.label())
            .when(self.status_filter == filter, |button| button.primary())
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_status_filter(filter, cx);
            }))
    }

    fn issue_row(&self, issue: &IssueViewModel, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.selected_issue.as_ref() == Some(&issue.id);
        let issue_id = issue.id.clone();
        v_flex()
            .id(issue.id.to_string())
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
                this.select_issue(issue_id.clone(), cx, false);
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
        let issue = self
            .selected_issue
            .as_ref()
            .and_then(|selected| self.issues.iter().find(|issue| &issue.id == selected));
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
        let description = match &self.detail_state {
            DetailState::Loaded(detail) => detail.description.clone(),
            _ => issue.map_or_else(
                || "Select an issue to load its details.".to_owned(),
                |issue| issue.description.clone(),
            ),
        };
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
            .child(self.render_detail_state(cx))
    }

    fn render_detail_state(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.detail_state {
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
                            "Select the issue to load its Jira details."
                        } else {
                            "Select an issue to load comments and attachments."
                        }),
                )
                .into_any_element(),
            DetailState::Loading { .. } => v_flex()
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
                        .child("Loading issue details…"),
                )
                .into_any_element(),
            DetailState::Error { message, .. } => v_flex()
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
                        .text_color(cx.theme().danger)
                        .child(message.clone()),
                )
                .into_any_element(),
            DetailState::Loaded(detail) => {
                let comments = if detail.comments.is_empty() {
                    v_flex()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No comments.")
                        .into_any_element()
                } else {
                    v_flex()
                        .gap_3()
                        .children(detail.comments.iter().map(|comment| {
                            v_flex()
                                .gap_1()
                                .p_3()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .child(comment.author.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(comment.created.clone()),
                                        ),
                                )
                                .child(div().text_sm().child(comment.body.clone()))
                                .when_some(comment.updated.clone(), |this, updated| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("Updated {updated}")),
                                    )
                                })
                        }))
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
                            h_flex()
                                .justify_between()
                                .text_sm()
                                .child(attachment.filename.clone())
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{} · {}",
                                            attachment.mime_type, attachment.size
                                        )),
                                )
                        }))
                        .into_any_element()
                };
                v_flex()
                    .gap_3()
                    .child(div().text_sm().font_semibold().child("Comments"))
                    .child(comments)
                    .child(div().text_sm().font_semibold().child("Attachments"))
                    .child(attachments)
                    .into_any_element()
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample_data::{sample_issues, sample_users};

    #[test]
    fn authenticated_user_is_seeded_for_display_mapping_and_account_filtering() {
        let authenticated_user = sample_users().into_iter().next().expect("sample user");
        let display_name = authenticated_user.display_name.clone();
        let account_id = authenticated_user.account_id.clone();
        let (users, account) = authenticated_identity(Some(authenticated_user));
        let views = issue_views_for_filter(&sample_issues(), &users, IssueStatusFilter::All);

        assert_eq!(account, Some(account_id));
        assert_eq!(views[0].assignee, display_name);
    }

    #[test]
    fn status_filter_rebuilds_from_loaded_domain_issues_without_remote_state() {
        let domain_issues = sample_issues();
        let users = sample_users();
        let all = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::All);
        let done = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::Done);

        assert_eq!(all.len(), 5);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].key, "DESK-163");
    }

    #[test]
    fn stale_detail_results_are_rejected_after_selection_changes() {
        let first = IssueId::new("10001").expect("issue");
        let second = IssueId::new("10002").expect("issue");

        assert!(!detail_result_is_current(Some(&second), &first, 2, 1));
        assert!(!detail_result_is_current(Some(&first), &first, 2, 1));
        assert!(detail_result_is_current(Some(&second), &second, 2, 2));
    }
}
