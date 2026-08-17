use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    actions, div, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, StyledExt as _,
    button::Button,
    button::ButtonVariants as _,
    h_flex,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::DropdownMenu as _,
    v_flex,
};
use jira_application::{
    ApplicationError, CancellationToken, DefaultPollingPolicy, IssueLocator, JiraCommentWritePort,
    JiraReadPort, SyncMode,
};

actions!(
    jira_dashboard,
    [
        StatusAll,
        StatusToDo,
        StatusInProgress,
        StatusDone,
        StatusUncategorized
    ]
);
use jira_domain::{AccountId, Issue, IssueId, IssueKey, User};

use crate::{
    config::{LiveSession, StartupError, ensure_authenticated_user},
    live_workspace::{CachedWorkspace, LiveWorkspace, RefreshResult},
    presentation::{
        IssueDetailViewModel, IssueStatusFilter, IssueViewModel, UpdateViewModel,
        issue_views_for_filter,
    },
    responsive::{LayoutMode, layout_for_width},
    rich_text_view::{RichTextPalette, render_rich_text},
    sample_data::{sample_issues, sample_updates, sample_users},
    semantic_icons::{PriorityTone, issue_type_icon, priority_semantics},
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
        | jira_application::ErrorKind::Internal
        | jira_application::ErrorKind::UnknownOutcome => "Refresh failed · local application error",
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
        | jira_application::ErrorKind::Internal
        | jira_application::ErrorKind::UnknownOutcome => {
            "Issue details unavailable · Jira returned an error"
        }
    }
}

fn safe_lookup_error(error: &ApplicationError) -> &'static str {
    match error.kind() {
        jira_application::ErrorKind::Authentication => {
            "Jira lookup failed · authentication was rejected"
        }
        jira_application::ErrorKind::Authorization => {
            "Jira lookup failed · authorization was denied"
        }
        jira_application::ErrorKind::NotFound => "Jira lookup · issue was not found",
        jira_application::ErrorKind::RateLimited => "Jira lookup paused · rate limit reached",
        jira_application::ErrorKind::Offline => "Jira lookup failed · Jira is unreachable",
        jira_application::ErrorKind::Cancelled => "Jira lookup cancelled",
        jira_application::ErrorKind::InvalidInput
        | jira_application::ErrorKind::Upstream
        | jira_application::ErrorKind::Storage
        | jira_application::ErrorKind::Notification
        | jira_application::ErrorKind::Internal
        | jira_application::ErrorKind::UnknownOutcome => {
            "Jira lookup failed · request was not completed"
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
        "Refresh complete · {} issues · {} new local updates · {} local updates loaded · desktop notifications: {} delivered, {} unavailable · {mode}",
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum DetailState {
    Empty,
    Loading { issue_id: IssueId },
    RemoteLoading { query: String },
    Loaded(IssueDetailViewModel),
    Error { issue_id: IssueId, message: String },
    RemoteError { query: String, message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CommentPostState {
    Idle,
    Confirming {
        issue_id: IssueId,
        issue_key: String,
        body: String,
        chars: usize,
        bytes: usize,
    },
    Posting {
        issue_id: IssueId,
    },
    Error {
        issue_id: IssueId,
        message: String,
        unknown_outcome: bool,
    },
}

fn comment_error_message(error: &ApplicationError) -> (&'static str, bool) {
    match error.kind() {
        jira_application::ErrorKind::Authentication => (
            "Comment not posted · Jira authentication was rejected",
            false,
        ),
        jira_application::ErrorKind::Authorization => {
            ("Comment not posted · Jira denied comment permission", false)
        }
        jira_application::ErrorKind::NotFound => {
            ("Comment not posted · the Jira issue was not found", false)
        }
        jira_application::ErrorKind::RateLimited => (
            "Comment not posted · Jira rate limit reached; try later",
            false,
        ),
        jira_application::ErrorKind::InvalidInput => {
            ("Comment not posted · the comment text is invalid", false)
        }
        jira_application::ErrorKind::UnknownOutcome => (
            "Jira may have accepted this comment. Refresh comments before retrying.",
            true,
        ),
        _ => ("Comment not posted · Jira returned an error", false),
    }
}

fn confirmed_comment_snapshot(
    state: &CommentPostState,
    selected_issue: Option<&IssueId>,
) -> Option<(IssueId, String)> {
    let CommentPostState::Confirming { issue_id, body, .. } = state else {
        return None;
    };
    (selected_issue == Some(issue_id)).then(|| (issue_id.clone(), body.clone()))
}

fn comment_target_is_current(
    remote_issue_id: Option<&IssueId>,
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
) -> bool {
    remote_issue_id.or(selected_issue) == Some(expected_issue)
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
enum RemoteLookupState {
    Idle,
    Loading {
        query: String,
    },
    Loaded {
        query: String,
        issue: Issue,
        detail: IssueDetailViewModel,
    },
    Error {
        query: String,
        message: String,
    },
}

fn detail_result_is_current(
    selected_issue: Option<&IssueId>,
    expected_issue: &IssueId,
    generation: u64,
    expected_generation: u64,
) -> bool {
    generation == expected_generation && selected_issue == Some(expected_issue)
}

fn remote_lookup_result_is_current(
    current_query: &str,
    expected_query: &str,
    generation: u64,
    expected_generation: u64,
) -> bool {
    generation == expected_generation
        && current_query
            .trim()
            .eq_ignore_ascii_case(expected_query.trim())
}

fn local_issue_id_for_key(issues: &[Issue], key: &IssueKey) -> Option<IssueId> {
    issues
        .iter()
        .find(|issue| issue.key.as_str().eq_ignore_ascii_case(key.as_str()))
        .map(|issue| issue.id.clone())
}

pub struct Dashboard {
    section: Section,
    domain_issues: Vec<Issue>,
    issues: Vec<IssueViewModel>,
    updates: Vec<UpdateViewModel>,
    selected_issue: Option<IssueId>,
    mobile_detail_open: bool,
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
    authenticated_account: Option<AccountId>,
    status_filter: IssueStatusFilter,
    search_query: String,
    search_input: Option<Entity<InputState>>,
    search_subscriptions: Vec<Subscription>,
    detail_state: DetailState,
    detail_generation: u64,
    detail_cancellation: Option<CancellationToken>,
    detail_task: Option<gpui::Task<()>>,
    remote_lookup: RemoteLookupState,
    remote_lookup_generation: u64,
    remote_lookup_cancellation: Option<CancellationToken>,
    remote_lookup_task: Option<gpui::Task<()>>,
    comment_input: Option<Entity<TextareaState>>,
    comment_subscriptions: Vec<Subscription>,
    comment_state: CommentPostState,
    comment_generation: u64,
    comment_cancellation: Option<CancellationToken>,
    comment_task: Option<gpui::Task<()>>,
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
                UpdateViewModel::from_domain(event, issue, &users)
            })
            .collect();
        let issues = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::All, "");
        let selected_issue = issues.first().map(|issue| issue.id.clone());

        Self {
            section: Section::Issues,
            domain_issues,
            issues,
            updates,
            selected_issue,
            mobile_detail_open: false,
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
            authenticated_account: None,
            status_filter: IssueStatusFilter::All,
            search_query: String::new(),
            search_input: None,
            search_subscriptions: Vec::new(),
            detail_state: DetailState::Empty,
            detail_generation: 0,
            detail_cancellation: None,
            detail_task: None,
            remote_lookup: RemoteLookupState::Idle,
            remote_lookup_generation: 0,
            remote_lookup_cancellation: None,
            remote_lookup_task: None,
            comment_input: None,
            comment_subscriptions: Vec::new(),
            comment_state: CommentPostState::Idle,
            comment_generation: 0,
            comment_cancellation: None,
            comment_task: None,
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
            mobile_detail_open: false,
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
            mode_label:
                "Live Jira sync · explicit comments only · best-effort desktop notifications"
                    .to_owned(),
            operation_in_progress: true,
            polling_task: None,
            automatic_polling_paused: false,
            authenticated_account: initial_authenticated_account,
            status_filter: IssueStatusFilter::All,
            search_query: String::new(),
            search_input: None,
            search_subscriptions: Vec::new(),
            detail_state: DetailState::Empty,
            detail_generation: 0,
            detail_cancellation: None,
            detail_task: None,
            remote_lookup: RemoteLookupState::Idle,
            remote_lookup_generation: 0,
            remote_lookup_cancellation: None,
            remote_lookup_task: None,
            comment_input: None,
            comment_subscriptions: Vec::new(),
            comment_state: CommentPostState::Idle,
            comment_generation: 0,
            comment_cancellation: None,
            comment_task: None,
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
                    let jira_read: Arc<dyn JiraReadPort> = jira.clone();
                    let jira_write: Arc<dyn JiraCommentWritePort> = jira.clone();
                    match LiveWorkspace::initialize_with_comment_writer(
                        site_id,
                        Some(authenticated_account),
                        jira_read,
                        jira_write,
                        cache,
                    )
                    .await
                    {
                        Ok(workspace) => {
                            let workspace = Arc::new(workspace);
                            workspace
                                .load_cached_for_authenticated_account()
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
        self.issues = issue_views_for_filter(
            &self.domain_issues,
            &self.users,
            self.status_filter,
            &self.search_query,
        );
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

    fn set_search_query(&mut self, query: String, cx: &mut Context<Self>) {
        let query = query.trim().to_owned();
        if self.search_query == query {
            return;
        }
        self.clear_remote_lookup();
        self.invalidate_comment_selection();
        self.search_query = query;
        self.rebuild_issue_views(false, cx);
        cx.notify();
    }

    fn clear_remote_lookup(&mut self) {
        if let Some(cancellation) = self.remote_lookup_cancellation.take() {
            cancellation.cancel();
        }
        self.remote_lookup_task.take();
        self.remote_lookup_generation = self.remote_lookup_generation.wrapping_add(1);
        self.remote_lookup = RemoteLookupState::Idle;
    }

    fn search_jira(&mut self, cx: &mut Context<Self>) {
        let query = self.search_query.trim().to_owned();
        let Some(key) = crate::presentation::normalized_issue_key(&query) else {
            self.clear_remote_lookup();
            self.sync_message = "Jira lookup · enter a valid issue key such as IX-123".to_owned();
            cx.notify();
            return;
        };

        if let Some(issue_id) = local_issue_id_for_key(&self.domain_issues, &key) {
            self.clear_remote_lookup();
            self.select_issue(issue_id, cx, true);
            return;
        }

        self.invalidate_comment_selection();
        let Some(workspace) = self.workspace.clone() else {
            self.remote_lookup = RemoteLookupState::Error {
                query,
                message: "Jira lookup unavailable · live workspace is not ready".to_owned(),
            };
            cx.notify();
            return;
        };

        if let Some(cancellation) = self.remote_lookup_cancellation.take() {
            cancellation.cancel();
        }
        self.remote_lookup_task.take();
        self.remote_lookup_generation = self.remote_lookup_generation.wrapping_add(1);
        let generation = self.remote_lookup_generation;
        let cancellation = CancellationToken::new();
        self.remote_lookup_cancellation = Some(cancellation.clone());
        self.remote_lookup = RemoteLookupState::Loading {
            query: query.clone(),
        };
        let expected_query = query.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = workspace.lookup_issue(key, &cancellation).await;
            let _ = this.update(cx, |this, cx| {
                if !remote_lookup_result_is_current(
                    &this.search_query,
                    &expected_query,
                    this.remote_lookup_generation,
                    generation,
                ) {
                    return;
                }
                this.remote_lookup_cancellation = None;
                this.remote_lookup_task = None;
                this.remote_lookup = match result {
                    Ok(detail) => {
                        let issue = detail.core.issue.clone();
                        let detail = IssueDetailViewModel::from_domain(&detail, &this.users);
                        RemoteLookupState::Loaded {
                            query: expected_query.clone(),
                            issue,
                            detail,
                        }
                    }
                    Err(error) => RemoteLookupState::Error {
                        query: expected_query.clone(),
                        message: safe_lookup_error(&error).to_owned(),
                    },
                };
                cx.notify();
            });
        });
        self.remote_lookup_task = Some(task);
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

    fn invalidate_comment_selection(&mut self) {
        self.comment_generation = self.comment_generation.wrapping_add(1);
        self.comment_input = None;
        self.comment_subscriptions.clear();
        if !matches!(&self.comment_state, CommentPostState::Posting { .. }) {
            if let Some(cancellation) = self.comment_cancellation.take() {
                cancellation.cancel();
            }
            self.comment_task.take();
            self.comment_state = CommentPostState::Idle;
        } else {
            // A dispatched POST may have succeeded even if its UI selection is
            // gone; its completion is ignored by the generation guard.
            self.comment_state = CommentPostState::Idle;
        }
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
        self.invalidate_comment_selection();
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
                .fetch_issue_detail(IssueLocator::Id(issue_id.clone()), &cancellation)
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
                    Ok(detail) => {
                        DetailState::Loaded(IssueDetailViewModel::from_domain(&detail, &this.users))
                    }
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

    fn begin_comment_confirmation(&mut self, cx: &mut Context<Self>) {
        if matches!(
            &self.comment_state,
            CommentPostState::Error {
                unknown_outcome: true,
                ..
            }
        ) {
            self.sync_message =
                "Refresh comments before retrying a comment with an unknown outcome".to_owned();
            cx.notify();
            return;
        }
        let Some(input) = self.comment_input.as_ref() else {
            return;
        };
        let Some(issue) = self.comment_target_issue() else {
            return;
        };
        let body = input.read(cx).value().to_string().trim().to_owned();
        if body.trim().is_empty() {
            self.comment_state = CommentPostState::Error {
                issue_id: issue.id.clone(),
                message: "Comment not posted · enter a non-empty comment".to_owned(),
                unknown_outcome: false,
            };
        } else if body.len() > jira_application::MAX_COMMENT_BYTES {
            self.comment_state = CommentPostState::Error {
                issue_id: issue.id.clone(),
                message: "Comment not posted · comment exceeds the byte limit".to_owned(),
                unknown_outcome: false,
            };
        } else if body.chars().count() > jira_application::MAX_COMMENT_CHARS {
            self.comment_state = CommentPostState::Error {
                issue_id: issue.id.clone(),
                message: "Comment not posted · comment exceeds the character limit".to_owned(),
                unknown_outcome: false,
            };
        } else {
            let chars = body.chars().count();
            let bytes = body.len();
            self.comment_state = CommentPostState::Confirming {
                issue_id: issue.id.clone(),
                issue_key: issue.key.as_str().to_owned(),
                body,
                chars,
                bytes,
            };
        }
        cx.notify();
    }

    fn cancel_comment_confirmation(&mut self, cx: &mut Context<Self>) {
        self.comment_state = CommentPostState::Idle;
        cx.notify();
    }

    fn post_comment(&mut self, cx: &mut Context<Self>) {
        let Some(target_issue_id) = self.comment_target_issue().map(|issue| issue.id.clone())
        else {
            return;
        };
        let Some((issue_id, body)) =
            confirmed_comment_snapshot(&self.comment_state, Some(&target_issue_id))
        else {
            return;
        };
        let Some(workspace) = self.workspace.clone() else {
            self.comment_state = CommentPostState::Error {
                issue_id,
                message: "Comment not posted · live Jira workspace is not ready".to_owned(),
                unknown_outcome: false,
            };
            cx.notify();
            return;
        };
        let generation = self.comment_generation.wrapping_add(1);
        self.comment_generation = generation;
        let cancellation = CancellationToken::new();
        self.comment_cancellation = Some(cancellation.clone());
        self.comment_state = CommentPostState::Posting {
            issue_id: issue_id.clone(),
        };
        let task = cx.spawn(async move |this, cx| {
            let result = workspace
                .create_comment(IssueLocator::Id(issue_id.clone()), body, &cancellation)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.comment_generation != generation
                    || !comment_target_is_current(
                        match &this.remote_lookup {
                            RemoteLookupState::Loaded { issue, .. } => Some(&issue.id),
                            RemoteLookupState::Idle
                            | RemoteLookupState::Loading { .. }
                            | RemoteLookupState::Error { .. } => None,
                        },
                        this.selected_issue.as_ref(),
                        &issue_id,
                    )
                {
                    return;
                }
                this.comment_cancellation = None;
                this.comment_task = None;
                match result {
                    Ok(_) => {
                        this.comment_input = None;
                        this.comment_subscriptions.clear();
                        this.comment_state = CommentPostState::Idle;
                        if matches!(
                            &this.remote_lookup,
                            RemoteLookupState::Loaded { issue, .. } if issue.id == issue_id
                        ) {
                            this.search_jira(cx);
                        } else {
                            this.reload_selected_detail(cx);
                        }
                    }
                    Err(error) => {
                        let (message, unknown_outcome) = comment_error_message(&error);
                        this.comment_state = CommentPostState::Error {
                            issue_id: issue_id.clone(),
                            message: message.to_owned(),
                            unknown_outcome,
                        };
                    }
                }
                cx.notify();
            });
        });
        self.comment_task = Some(task);
        cx.notify();
    }

    fn refresh_comments(&mut self, cx: &mut Context<Self>) {
        if !matches!(&self.comment_state, CommentPostState::Posting { .. }) {
            if matches!(
                &self.comment_state,
                CommentPostState::Error {
                    unknown_outcome: true,
                    ..
                }
            ) {
                self.comment_state = CommentPostState::Idle;
            }
            if matches!(&self.remote_lookup, RemoteLookupState::Loaded { .. }) {
                self.search_jira(cx);
            } else {
                self.reload_selected_detail(cx);
            }
        }
    }

    fn apply_cached(&mut self, cached: CachedWorkspace, cx: &mut Context<Self>) {
        let CachedWorkspace { issues, events } = cached;
        let updates = events
            .iter()
            .map(|event| {
                let issue = issues.iter().find(|issue| issue.id == event.issue_id);
                UpdateViewModel::from_domain(event, issue, &self.users)
            })
            .collect();
        self.apply_live_issues(issues, true, cx);
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
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = workspace.mark_all_read().await;
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

    fn issue_key_with_icon(
        &self,
        key: impl Into<String>,
        issue_type: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .min_w_0()
            .gap_1()
            .child(Icon::new(issue_type_icon(issue_type)).text_color(cx.theme().link))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().link)
                    .child(key.into()),
            )
            .into_any_element()
    }

    fn priority_badge(&self, label: String, cx: &mut Context<Self>) -> AnyElement {
        let (icon, tone) = priority_semantics(&label);
        let color = self.priority_color(tone, cx);
        h_flex()
            .min_w_0()
            .gap_1()
            .child(Icon::new(icon).text_color(color))
            .child(div().min_w_0().truncate().child(label))
            .into_any_element()
    }

    fn priority_color(&self, tone: PriorityTone, cx: &mut Context<Self>) -> gpui::Hsla {
        match tone {
            PriorityTone::Critical => cx.theme().danger,
            PriorityTone::Elevated => cx.theme().warning,
            PriorityTone::Neutral | PriorityTone::Unknown => cx.theme().muted_foreground,
            PriorityTone::Low | PriorityTone::Minimal => cx.theme().link,
        }
    }

    fn render_sidebar(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let rail = layout.is_rail();
        v_flex()
            .h_full()
            .w(px(layout.sidebar_width()))
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
                    .when(rail, |this| this.justify_center().px_2().gap_0())
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
                    .child(v_flex().min_w_0().gap_0p5().when(!rail, |this| {
                        this.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_semibold()
                                .child("Jira Desk"),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Read-only sync · explicit comments"),
                        )
                    })),
            )
            .child(
                v_flex()
                    .flex_1()
                    .p_3()
                    .gap_1()
                    .when(!rail, |this| {
                        this.child(
                            div()
                                .px_3()
                                .pt_2()
                                .pb_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("WORKSPACE"),
                        )
                    })
                    .child(self.nav_item(
                        "Issues",
                        self.issues.len(),
                        self.section == Section::Issues,
                        Section::Issues,
                        rail,
                        cx,
                    ))
                    .child(self.nav_item(
                        "Local updates",
                        self.unread_count(),
                        self.section == Section::Updates,
                        Section::Updates,
                        rail,
                        cx,
                    ))
                    .when(!rail, |this| {
                        this.child(
                            div()
                                .mt_5()
                                .px_3()
                                .pt_2()
                                .pb_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("Jira Project VIEW"),
                        )
                    })
                    .when(!rail, |this| {
                        this.child(
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
                        )
                    }),
            )
            .when(!rail, |this| {
                this.child(
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
            })
    }

    fn nav_item(
        &self,
        label: &'static str,
        count: usize,
        selected: bool,
        section: Section,
        rail: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let icon = match section {
            Section::Issues => IconName::LayoutDashboard,
            Section::Updates => IconName::Inbox,
        };
        let visual = if rail {
            Icon::new(icon).into_any_element()
        } else {
            div()
                .text_sm()
                .font_semibold()
                .child(label)
                .into_any_element()
        };
        h_flex()
            .id(label)
            .w_full()
            .px_3()
            .py_2()
            .justify_between()
            .when(rail, |this| this.justify_center().px_1())
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .aria_label(label)
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
            .child(visual)
            .when(!rail, |this| {
                this.child(
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
            })
    }

    fn render_header(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        v_flex()
            .h(px(if mobile { 84. } else { 72. }))
            .px(px(if mobile { 12. } else { 20. }))
            .py(px(if mobile { 10. } else { 12. }))
            .flex_shrink_0()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .min_w_0()
                    .justify_between()
                    .child(div().min_w_0().truncate().text_lg().font_semibold().child(
                        match self.section {
                            Section::Issues => "Jira Project issues",
                            Section::Updates => "Local updates",
                        },
                    ))
                    .child(
                        Button::new("refresh")
                            .compact()
                            .primary()
                            .label(if self.operation_in_progress {
                                "Refreshing…"
                            } else {
                                "Refresh"
                            })
                            .on_click(cx.listener(|this, _, _, cx| this.begin_refresh(cx))),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.sync_message.clone()),
            )
    }

    fn render_issues(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        let issue_list = v_flex()
            .h_full()
            .when(mobile, |this| this.w_full())
            .when(!mobile, |this| this.w(px(layout.issue_list_width())))
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(if mobile {
                v_flex()
                    .h(px(58.))
                    .px_3()
                    .justify_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().min_w_0().truncate().child(format!(
                        "{} matching Jira Project issues · My issues",
                        self.issues.len(),
                    )))
                    .into_any_element()
            } else {
                h_flex()
                    .h(px(44.))
                    .px_4()
                    .flex_shrink_0()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().min_w_0().truncate().child(format!(
                        "{} matching Jira Project issues · My issues",
                        self.issues.len(),
                    )))
                    .child(div().flex_shrink_0().child("Updated newest first"))
                    .into_any_element()
            })
            .when_some(self.search_input.clone(), |this, input| {
                if mobile {
                    this.child(
                        v_flex()
                            .gap_1()
                            .px_2()
                            .py_2()
                            .min_w_0()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(Input::new(&input).cleanable(true).min_w_0().w_full())
                            .child(
                                Button::new("search-jira")
                                    .compact()
                                    .w_full()
                                    .label("Search Jira")
                                    .on_click(cx.listener(|this, _, _, cx| this.search_jira(cx))),
                            ),
                    )
                } else {
                    this.child(
                        h_flex()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .min_w_0()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(Input::new(&input).cleanable(true).min_w_0().flex_1())
                            .child(
                                Button::new("search-jira")
                                    .compact()
                                    .label("Search Jira")
                                    .on_click(cx.listener(|this, _, _, cx| this.search_jira(cx))),
                            ),
                    )
                }
            })
            .child(
                h_flex()
                    .h(px(44.))
                    .px_3()
                    .gap_1()
                    .flex_shrink_0()
                    .min_w_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(self.status_filter_dropdown()),
            )
            .child(
                v_flex()
                    .id("issue-list")
                    .min_h_0()
                    .flex_1()
                    .when(mobile, |this| this.w_full())
                    .overflow_y_scroll()
                    .when_some(self.remote_lookup_view(), |this, issue| {
                        this.child(self.issue_row_with_label(
                            &issue,
                            "Jira lookup result",
                            layout,
                            cx,
                        ))
                    })
                    .children(
                        self.issues
                            .iter()
                            .map(|issue| self.issue_row(issue, layout, cx)),
                    ),
            )
            .into_any_element();

        h_flex()
            .size_full()
            .min_w_0()
            .on_action(cx.listener(|this, _: &StatusAll, _, cx| {
                this.set_status_filter(IssueStatusFilter::All, cx)
            }))
            .on_action(cx.listener(|this, _: &StatusToDo, _, cx| {
                this.set_status_filter(IssueStatusFilter::ToDo, cx)
            }))
            .on_action(cx.listener(|this, _: &StatusInProgress, _, cx| {
                this.set_status_filter(IssueStatusFilter::InProgress, cx)
            }))
            .on_action(cx.listener(|this, _: &StatusDone, _, cx| {
                this.set_status_filter(IssueStatusFilter::Done, cx)
            }))
            .on_action(cx.listener(|this, _: &StatusUncategorized, _, cx| {
                this.set_status_filter(IssueStatusFilter::Uncategorized, cx)
            }))
            .when(mobile && self.mobile_detail_open, |this| {
                this.child(
                    v_flex()
                        .size_full()
                        .min_w_0()
                        .child(
                            h_flex()
                                .h(px(44.))
                                .px_3()
                                .flex_shrink_0()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child(
                                    Button::new("mobile-detail-back")
                                        .compact()
                                        .ghost()
                                        .label("Back to issues")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.mobile_detail_open = false;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .child(self.issue_detail(layout, cx)),
                )
            })
            .when(!mobile || !self.mobile_detail_open, |this| {
                this.child(issue_list)
            })
    }

    fn status_filter_dropdown(&self) -> impl IntoElement {
        let selected = self.status_filter;
        Button::new("status-filter-menu")
            .label(selected.label())
            .dropdown_menu(move |menu, _, _| {
                menu.menu_with_check(
                    IssueStatusFilter::All.label(),
                    selected == IssueStatusFilter::All,
                    Box::new(StatusAll),
                )
                .menu_with_check(
                    IssueStatusFilter::ToDo.label(),
                    selected == IssueStatusFilter::ToDo,
                    Box::new(StatusToDo),
                )
                .menu_with_check(
                    IssueStatusFilter::InProgress.label(),
                    selected == IssueStatusFilter::InProgress,
                    Box::new(StatusInProgress),
                )
                .menu_with_check(
                    IssueStatusFilter::Done.label(),
                    selected == IssueStatusFilter::Done,
                    Box::new(StatusDone),
                )
                .menu_with_check(
                    IssueStatusFilter::Uncategorized.label(),
                    selected == IssueStatusFilter::Uncategorized,
                    Box::new(StatusUncategorized),
                )
            })
    }

    fn remote_lookup_view(&self) -> Option<IssueViewModel> {
        match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => {
                Some(IssueViewModel::from_domain(issue, &self.users))
            }
            RemoteLookupState::Idle
            | RemoteLookupState::Loading { .. }
            | RemoteLookupState::Error { .. } => None,
        }
    }

    fn selected_issue_view(&self) -> Option<IssueViewModel> {
        let selected = self.selected_issue.as_ref()?;
        self.issues
            .iter()
            .find(|issue| &issue.id == selected)
            .cloned()
            .or_else(|| {
                self.domain_issues
                    .iter()
                    .find(|issue| &issue.id == selected)
                    .map(|issue| IssueViewModel::from_domain(issue, &self.users))
            })
    }

    fn comment_target_issue(&self) -> Option<&Issue> {
        match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => Some(issue),
            RemoteLookupState::Idle
            | RemoteLookupState::Loading { .. }
            | RemoteLookupState::Error { .. } => self
                .selected_issue
                .as_ref()
                .and_then(|id| self.domain_issues.iter().find(|issue| &issue.id == id)),
        }
    }

    fn issue_row(
        &self,
        issue: &IssueViewModel,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.issue_row_with_label(issue, "", layout, cx)
    }

    fn issue_row_with_label(
        &self,
        issue: &IssueViewModel,
        label: &str,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_issue.as_ref() == Some(&issue.id)
            || matches!(
                &self.remote_lookup,
                RemoteLookupState::Loaded { issue: remote, .. } if remote.id == issue.id
            );
        let issue_id = issue.id.clone();
        let is_remote_result = !label.is_empty();
        let mobile = layout.is_mobile();
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
                if !is_remote_result {
                    this.clear_remote_lookup();
                    this.select_issue(issue_id.clone(), cx, false);
                }
                this.mobile_detail_open = mobile;
                cx.notify();
            }))
            .child(
                h_flex()
                    .min_w_0()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(self.issue_key_with_icon(
                                issue.key.clone(),
                                &issue.issue_type,
                                cx,
                            ))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(issue.issue_type.clone()),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_shrink_0()
                            .truncate()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(cx.theme().secondary)
                            .text_color(cx.theme().secondary_foreground)
                            .text_xs()
                            .child(issue.status.clone()),
                    ),
            )
            .when(!label.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().link)
                        .child(label.to_owned()),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .line_clamp(2)
                    .text_sm()
                    .font_semibold()
                    .child(issue.summary.clone()),
            )
            .child(
                h_flex()
                    .min_w_0()
                    .justify_between()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(format!("{} ·", issue.assignee)),
                            )
                            .child(self.priority_badge(issue.priority.clone(), cx)),
                    )
                    .child(div().flex_shrink_0().child(issue.updated.clone())),
            )
            .into_any_element()
    }

    fn issue_detail(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let issue = match &self.remote_lookup {
            RemoteLookupState::Loaded { .. } => self.remote_lookup_view(),
            RemoteLookupState::Loading { .. } | RemoteLookupState::Error { .. } => None,
            RemoteLookupState::Idle => self.selected_issue_view(),
        };
        let lookup_query = match &self.remote_lookup {
            RemoteLookupState::Loading { query } | RemoteLookupState::Error { query, .. } => {
                Some(query.as_str())
            }
            RemoteLookupState::Idle | RemoteLookupState::Loaded { .. } => None,
        };
        let project = issue
            .as_ref()
            .map(|issue| issue.project.clone())
            .unwrap_or_else(|| "Jira".to_owned());
        let key = issue
            .as_ref()
            .map(|issue| issue.key.clone())
            .or_else(|| lookup_query.map(str::to_owned))
            .unwrap_or_else(|| "—".to_owned());
        let summary = issue
            .as_ref()
            .map(|issue| issue.summary.clone())
            .unwrap_or_else(|| {
                if lookup_query.is_some() {
                    "Jira lookup".to_owned()
                } else {
                    "No issues loaded".to_owned()
                }
            });
        let issue_type = issue
            .as_ref()
            .map(|issue| issue.issue_type.clone())
            .unwrap_or_else(|| "—".to_owned());
        let status = issue
            .as_ref()
            .map(|issue| issue.status.clone())
            .unwrap_or_else(|| "Ready to refresh".to_owned());
        let priority = issue
            .as_ref()
            .map(|issue| issue.priority.clone())
            .unwrap_or_else(|| "—".to_owned());
        let detail_state = match &self.remote_lookup {
            RemoteLookupState::Loaded { detail, .. } => DetailState::Loaded(detail.clone()),
            RemoteLookupState::Loading { query } => DetailState::RemoteLoading {
                query: query.clone(),
            },
            RemoteLookupState::Error { query, message } => DetailState::RemoteError {
                query: query.clone(),
                message: message.clone(),
            },
            RemoteLookupState::Idle => self.detail_state.clone(),
        };
        let description = match &detail_state {
            DetailState::Loaded(detail) => detail.description.clone(),
            _ => issue.as_ref().map_or_else(
                || "Select an issue to load its details.".to_owned(),
                |issue| issue.description.clone(),
            ),
        };
        let rich_description = match &detail_state {
            DetailState::Loaded(detail) => detail.rich_description.clone(),
            _ => issue
                .as_ref()
                .and_then(|issue| issue.rich_description.clone()),
        };
        let description_content = rich_description
            .as_ref()
            .map(|document| render_rich_text(document, self.rich_text_palette(cx)))
            .unwrap_or_else(|| div().text_sm().child(description).into_any_element());
        let assignee = issue
            .as_ref()
            .map(|issue| issue.assignee.clone())
            .unwrap_or_else(|| "—".to_owned());
        let reporter = issue
            .as_ref()
            .map(|issue| issue.reporter.clone())
            .unwrap_or_else(|| "—".to_owned());
        let status_category = issue
            .as_ref()
            .map(|issue| issue.status_category.clone())
            .unwrap_or_else(|| "—".to_owned());
        let parent = issue
            .as_ref()
            .and_then(|issue| issue.parent.clone())
            .unwrap_or_else(|| "None".to_owned());
        let created = issue
            .as_ref()
            .map(|issue| issue.created.clone())
            .unwrap_or_else(|| "—".to_owned());
        let updated = issue
            .as_ref()
            .map(|issue| issue.updated.clone())
            .unwrap_or_else(|| "—".to_owned());
        let due_date = issue
            .as_ref()
            .map(|issue| issue.due_date.clone())
            .unwrap_or_else(|| "—".to_owned());
        let labels = issue
            .as_ref()
            .map(|issue| issue.labels.clone())
            .unwrap_or_default();
        v_flex()
            .id("issue-detail")
            .h_full()
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .p(px(layout.detail_padding()))
            .gap(px(if layout.is_mobile() { 16. } else { 20. }))
            .child(
                v_flex()
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
                            .child(self.pill(status, cx))
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
                            .child(description_content),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().font_semibold().child("Details"))
                    .child(self.detail_field("Assignee", assignee, layout, cx))
                    .child(self.detail_field("Reporter", reporter, layout, cx))
                    .child(self.detail_field("Status category", status_category, layout, cx))
                    .child(self.detail_field("Parent", parent, layout, cx))
                    .child(self.detail_field("Created", created, layout, cx))
                    .child(self.detail_field("Updated", updated, layout, cx))
                    .child(self.detail_field("Due date", due_date, layout, cx)),
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
            DetailState::RemoteError { message, .. } => v_flex()
                .gap_2()
                .child(div().text_sm().font_semibold().child("Jira lookup"))
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(message.clone()),
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
                        .gap_3()
                        .children(detail.comments.iter().map(|comment| {
                            let body = comment
                                .rich_body
                                .as_ref()
                                .map(|document| render_rich_text(document, palette))
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
                                .child(div().min_w_0().child(body))
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
                                    .ghost()
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

    fn render_comment_composer(&self, layout: LayoutMode, cx: &mut Context<Self>) -> AnyElement {
        let Some(input) = self.comment_input.as_ref() else {
            return div().into_any_element();
        };
        let state = self.comment_state.clone();
        let body = match &state {
            CommentPostState::Confirming { body, .. } => body.clone(),
            _ => input.read(cx).value().to_string(),
        };
        let posting = matches!(&state, CommentPostState::Posting { .. });
        let editing_confirmed = matches!(&state, CommentPostState::Confirming { .. });
        let mut composer = v_flex()
            .min_w_0()
            .gap_2()
            .child(div().text_sm().font_semibold().child("Add comment"))
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
        composer = match state {
            CommentPostState::Confirming {
                issue_key,
                body: _,
                chars,
                bytes,
                ..
            } => composer
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
                                .on_click(cx.listener(|this, _, _, cx| this.post_comment(cx))),
                        )
                        .child(Button::new("cancel-comment").label("Cancel").on_click(
                            cx.listener(|this, _, _, cx| this.cancel_comment_confirmation(cx)),
                        )),
                ),
            CommentPostState::Posting { .. } => composer.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Posting comment…"),
            ),
            CommentPostState::Error {
                message,
                unknown_outcome,
                ..
            } => composer
                .child(div().text_sm().text_color(cx.theme().danger).child(message))
                .child(
                    h_flex()
                        .when(layout.is_mobile(), |this| this.flex_col())
                        .gap_2()
                        .child(
                            Button::new("post-comment")
                                .primary()
                                .label("Post comment")
                                .disabled(posting)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.begin_comment_confirmation(cx)
                                })),
                        )
                        .when(unknown_outcome, |this| {
                            this.child(
                                Button::new("refresh-comments-after-unknown")
                                    .label("Refresh comments")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.refresh_comments(cx)),
                                    ),
                            )
                        }),
                ),
            CommentPostState::Idle => composer.child(
                Button::new("post-comment")
                    .primary()
                    .label("Post comment")
                    .disabled(posting)
                    .on_click(cx.listener(|this, _, _, cx| this.begin_comment_confirmation(cx))),
            ),
        };
        composer.into_any_element()
    }

    fn detail_field(
        &self,
        label: &'static str,
        value: String,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if layout.is_mobile() {
            v_flex()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(div().min_w_0().text_sm().child(value))
                .into_any_element()
        } else {
            h_flex()
                .min_w_0()
                .items_start()
                .child(
                    div()
                        .w(px(if layout.is_rail() { 108. } else { 132. }))
                        .flex_shrink_0()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                )
                .child(div().min_w_0().text_sm().child(value))
                .into_any_element()
        }
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

    fn ensure_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_input.is_some() {
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search issue key or summary"));
        self.search_subscriptions
            .push(cx.subscribe_in(&input, window, {
                let input = input.clone();
                move |this, _, event: &InputEvent, _window, cx| match event {
                    InputEvent::Change => {
                        this.set_search_query(input.read(cx).value().to_string(), cx);
                    }
                    InputEvent::PressEnter { .. } => this.search_jira(cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                }
            }));
        self.search_input = Some(input);
    }

    fn ensure_comment_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.comment_input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(4)
                .placeholder("Write a Jira comment")
        });
        self.comment_subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                    if matches!(&this.comment_state, CommentPostState::Error { .. }) {
                        this.comment_state = CommentPostState::Idle;
                    }
                }
            },
        ));
        self.comment_input = Some(input);
    }

    fn render_updates(&self, layout: LayoutMode, cx: &mut Context<Self>) -> impl IntoElement {
        let mobile = layout.is_mobile();
        v_flex()
            .size_full()
            .min_w_0()
            .child(
                v_flex()
                    .h(px(if mobile { 80. } else { 54. }))
                    .px(px(if mobile { 12. } else { 20. }))
                    .py(px(if mobile { 8. } else { 0. }))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .min_w_0()
                            .justify_between()
                            .child(div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} unread local updates · Changes detected by Jira Desk, not Jira notifications",
                                self.unread_count()
                            )))
                            .child(
                                Button::new("mark-all-read")
                                    .compact()
                                    .ghost()
                                    .label("Mark all read")
                                    .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("update-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .min_h_0()
                    .p(px(layout.list_padding()))
                    .gap_3()
                    .children(
                        self.updates
                            .iter()
                            .enumerate()
                            .map(|(index, update)| self.update_card(index, update, layout, cx)),
                    ),
            )
    }

    fn update_card(
        &self,
        index: usize,
        update: &UpdateViewModel,
        layout: LayoutMode,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let issue_type = self
            .domain_issues
            .iter()
            .find(|issue| issue.key.as_str().eq_ignore_ascii_case(&update.issue_key))
            .map(|issue| issue.issue_type.name.as_str())
            .unwrap_or("Unknown");
        h_flex()
            .id(("update-card", index))
            .w_full()
            .items_start()
            .min_w_0()
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
                            .min_w_0()
                            .justify_between()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .when(layout.is_mobile(), |this| this.flex_col())
                                    .gap_2()
                                    .child(self.issue_key_with_icon(
                                        update.issue_key.clone(),
                                        issue_type,
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .line_clamp(2)
                                            .text_sm()
                                            .font_semibold()
                                            .child(update.issue_summary.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
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

    fn render_mobile_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(48.))
            .flex_shrink_0()
            .gap_2()
            .px_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Button::new("mobile-issues")
                    .compact()
                    .when(self.section == Section::Issues, |this| this.primary())
                    .label(format!("Issues · {}", self.issues.len()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Issues;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("mobile-updates")
                    .compact()
                    .when(self.section == Section::Updates, |this| this.primary())
                    .label(format!("Updates · {}", self.unread_count()))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.section = Section::Updates;
                        this.mobile_detail_open = false;
                        cx.notify();
                    })),
            )
    }
}

impl Render for Dashboard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_search_input(window, cx);
        self.ensure_comment_input(window, cx);
        let layout = layout_for_width(f32::from(window.viewport_size().width));
        let content = match self.section {
            Section::Issues => self.render_issues(layout, cx).into_any_element(),
            Section::Updates => self.render_updates(layout, cx).into_any_element(),
        };

        let main = v_flex()
            .h_full()
            .min_w_0()
            .flex_1()
            .child(self.render_header(layout, cx))
            .child(div().min_h_0().flex_1().child(content));

        if layout.is_mobile() {
            v_flex()
                .size_full()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_mobile_nav(cx))
                .child(main)
        } else {
            h_flex()
                .size_full()
                .bg(cx.theme().background)
                .text_color(cx.theme().foreground)
                .child(self.render_sidebar(layout, cx))
                .child(main)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::normalized_issue_key;
    use crate::sample_data::{sample_issues, sample_users};

    #[test]
    fn authenticated_user_is_seeded_for_display_mapping_and_account_filtering() {
        let authenticated_user = sample_users().into_iter().next().expect("sample user");
        let display_name = authenticated_user.display_name.clone();
        let account_id = authenticated_user.account_id.clone();
        let (users, account) = authenticated_identity(Some(authenticated_user));
        let views = issue_views_for_filter(&sample_issues(), &users, IssueStatusFilter::All, "");

        assert_eq!(account, Some(account_id));
        assert_eq!(views[0].assignee, display_name);
    }

    #[test]
    fn status_filter_rebuilds_from_loaded_domain_issues_without_remote_state() {
        let domain_issues = sample_issues();
        let users = sample_users();
        let all = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::All, "");
        let done = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::Done, "");

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

    #[test]
    fn exact_local_key_hit_returns_id_without_remote_lookup() {
        let issues = sample_issues();
        let key = IssueKey::new("DESK-163").expect("key");
        let expected = issues
            .iter()
            .find(|issue| issue.key == key)
            .map(|issue| issue.id.clone());

        assert_eq!(local_issue_id_for_key(&issues, &key), expected);
    }

    #[test]
    fn invalid_key_is_rejected_before_a_remote_lookup() {
        assert!(normalized_issue_key("summary text").is_none());
        assert!(normalized_issue_key("IX-").is_none());
    }

    #[test]
    fn remote_result_can_be_present_even_when_local_status_filter_hides_it() {
        let issues = sample_issues();
        let users = sample_users();
        let remote = issues
            .iter()
            .find(|issue| issue.key.as_str() == "DESK-163")
            .expect("sample issue")
            .clone();
        let local_done = issue_views_for_filter(&issues, &users, IssueStatusFilter::ToDo, "");
        let remote_view = IssueViewModel::from_domain(&remote, &users);

        assert!(local_done.iter().all(|issue| issue.id != remote.id));
        assert_eq!(remote_view.key, "DESK-163");
    }

    #[test]
    fn stale_remote_results_are_rejected_after_query_changes() {
        assert!(!remote_lookup_result_is_current("IX-2", "IX-1", 2, 1));
        assert!(!remote_lookup_result_is_current("IX-2", "IX-1", 2, 2));
        assert!(remote_lookup_result_is_current(" ix-1 ", "IX-1", 2, 2));
    }

    #[test]
    fn clearing_search_cancels_and_removes_remote_result() {
        let mut dashboard = Dashboard::from_sample_data();
        dashboard.remote_lookup = RemoteLookupState::Error {
            query: "IX-404".to_owned(),
            message: "not found".to_owned(),
        };
        let generation = dashboard.remote_lookup_generation;

        dashboard.clear_remote_lookup();

        assert_eq!(dashboard.remote_lookup, RemoteLookupState::Idle);
        assert_eq!(dashboard.remote_lookup_generation, generation + 1);
    }

    #[test]
    fn comment_failures_have_definite_and_unknown_outcome_messages() {
        let definite = ApplicationError::new(
            jira_application::ErrorKind::Authorization,
            "server detail must not reach UI",
        );
        let (message, unknown) = comment_error_message(&definite);
        assert_eq!(
            message,
            "Comment not posted · Jira denied comment permission"
        );
        assert!(!unknown);
        assert!(!message.contains("server detail"));

        let uncertain = ApplicationError::new(
            jira_application::ErrorKind::UnknownOutcome,
            "secret response",
        );
        let (message, unknown) = comment_error_message(&uncertain);
        assert!(unknown);
        assert!(message.contains("Refresh comments"));
        assert!(!message.contains("secret response"));
    }

    #[test]
    fn comment_post_state_keeps_confirmation_issue_and_sizes() {
        let issue_id = IssueId::new("100").expect("issue");
        let state = CommentPostState::Confirming {
            issue_id: issue_id.clone(),
            issue_key: "IX-100".to_owned(),
            body: "hello".to_owned(),
            chars: 5,
            bytes: 7,
        };
        assert_eq!(
            state,
            CommentPostState::Confirming {
                issue_id,
                issue_key: "IX-100".to_owned(),
                body: "hello".to_owned(),
                chars: 5,
                bytes: 7,
            }
        );
    }

    #[test]
    fn confirmed_comment_snapshot_uses_original_body_and_rejects_other_issue() {
        let issue_a = IssueId::new("100").expect("issue");
        let issue_b = IssueId::new("200").expect("issue");
        let state = CommentPostState::Confirming {
            issue_id: issue_a.clone(),
            issue_key: "IX-100".to_owned(),
            body: "original body".to_owned(),
            chars: 13,
            bytes: 13,
        };

        let edited_editor_value = "edited after confirmation";
        let snapshot = confirmed_comment_snapshot(&state, Some(&issue_a));
        assert_eq!(
            snapshot,
            Some((issue_a.clone(), "original body".to_owned()))
        );
        assert_ne!(
            snapshot.as_ref().map(|(_, body)| body),
            Some(&edited_editor_value.to_owned())
        );
        assert_eq!(confirmed_comment_snapshot(&state, Some(&issue_b)), None);
        assert_eq!(confirmed_comment_snapshot(&state, None), None);
        assert_eq!(
            confirmed_comment_snapshot(
                &CommentPostState::Posting {
                    issue_id: issue_a.clone()
                },
                Some(&issue_a)
            ),
            None
        );
    }

    #[test]
    fn remote_lookup_identity_can_authorize_comment_independently_of_local_selection() {
        let remote_id = IssueId::new("remote-100").expect("issue");
        let local_id = IssueId::new("local-200").expect("issue");

        assert!(comment_target_is_current(
            Some(&remote_id),
            Some(&local_id),
            &remote_id
        ));
        assert!(!comment_target_is_current(
            Some(&remote_id),
            Some(&local_id),
            &local_id
        ));
        assert!(comment_target_is_current(None, Some(&local_id), &local_id));
    }
}
