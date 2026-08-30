use std::{collections::HashSet, path::PathBuf, sync::Arc};

use chrono::{Local, SecondsFormat};
use time::UtcOffset;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Anchor, AnyElement, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px, rems,
};
use gpui_component::table::{DataTable, TableEvent, TableState};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, ResizableState, StyledExt as _, Theme,
    WindowExt as _,
    button::Button,
    button::ButtonVariants as _,
    combobox::{Combobox, ComboboxEvent, ComboboxState},
    dialog::Cancel,
    h_flex, h_resizable,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    notification::Notification,
    resizable_panel,
    scroll::ScrollableElement as _,
    searchable_list::{SearchableListItem, SearchableVec},
    spinner::Spinner,
    v_flex,
};
use jira_application::{
    ApplicationError, AttachmentDownloadRequest, CancellationToken, DEFAULT_JQL_SCOPE,
    DefaultPollingPolicy, IssueLocator, IssueTransition, JiraCommentWritePort, JiraIssueEditPort,
    JiraReadPort, MAX_JQL_SCOPE_LENGTH,
};

use jira_domain::{AccountId, Issue, IssueId, IssueKey, User};

#[cfg(any(test, feature = "ui-lab", feature = "ui-automation"))]
use crate::sample_data::{sample_issues, sample_updates, sample_users};

use crate::{
    app_shell::AppearancePreference,
    config::{LiveSession, ensure_authenticated_user},
    credential_store::{self, DeleteOutcome},
    diagnostics::{
        DesktopNotificationTestResult as DiagnosticDesktopNotificationTestResult,
        DiagnosticErrorKind, DiagnosticFlow, DiagnosticsSink, ImageSource, ImageStateReason,
    },
    live_workspace::{CachedWorkspace, LiveWorkspace, RefreshResult},
    local_data::{
        MAX_TEAM_MEMBERS, PersistedTeamMember, load_preferences, normalize_issue_jql_scope,
        normalize_team_members,
    },
    presentation::{
        CompactedUpdateRow, FeedbackSeverity, IssueDetailViewModel, IssueStatusFilter,
        IssueStatusSelection, IssueViewModel, OutcomeCopy, ReadSurface, RecoveryDirective,
        SavedLoginOutcomeKind, UPDATE_PREVIEW_LIMIT, UpdateFilter, UpdateGroupViewModel,
        comment_outcome_copy, compact_update_rows, filtered_update_group_indices,
        generic_summary_label, hidden_update_row_count, issue_views_for_filter_with_offset,
        lookup_workspace_unavailable_copy, read_error_copy, saved_login_outcome_copy,
        scope_outcome_copy, team_outcome_copy, update_group_event_ids,
        update_groups_for_events_with_offset, visible_update_row_count,
    },
    responsive::{IssuesPaneMode, LayoutMode, issues_pane_mode, layout_for_width},
    rich_text_view::{
        RichAttachmentCardAction, RichImageRenderStates, RichTextPalette, render_rich_text,
        render_rich_text_with_actions,
    },
    semantic_icons::{PriorityTone, issue_type_icon, priority_semantics},
    team_table::{TeamTicketTableDelegate, TeamTicketTableStateExt},
};

mod comment_flow;
mod detail_payload;
mod detail_view;
mod issue_edit_flow;
mod issues_view;
mod media;
mod request_epoch;
mod settings;
mod settings_view;
mod shell_view;
mod team_view;
mod updates_view;

#[cfg(test)]
use crate::presentation::issue_views_for_filter;
#[cfg(test)]
use team_view::{TEAM_DETAIL_INITIAL_WIDTH, team_pane_constraints};
use team_view::{TeamTableMode, team_table_mode_for_width};

#[cfg(test)]
use crate::presentation::FeedbackCertainty;
use comment_flow::{CommentCompletion, CommentFlow, CommentInvalidation, CommentTarget};

#[cfg(test)]
use issue_edit_flow::issue_edit_target_is_current;
use issue_edit_flow::{
    AssigneeSubmission, BusyDirective, IssueEditCompletion, IssueEditFlow, IssueEditOperation,
    IssueEditState, IssueEditSubmission, TransitionSubmission, issue_edit_error_message,
    status_control_is_editable,
};

use detail_payload::{
    DetailReadRequest, detail_image_issue_id, fetch_detail_images, prepare_detail_payload,
    read_detail,
};
use media::{
    AttachmentDownloadState, MAX_ATTACHMENT_DOWNLOAD_BYTES, attachment_download_button_label,
    attachment_download_is_current, attachment_issue_id, attachment_temp_path,
    attachment_temp_token, cleanup_attachment_temp, collect_detail_images_with_context,
    fetch_cached_rich_image_states, image_result_is_current, inline_attachment_for_download,
    portal_download_directory, sanitized_attachment_filename, write_attachment_temp,
};
use request_epoch::{RequestEpoch, RequestSource, RequestTicket};

fn safe_sync_error(error: &ApplicationError) -> OutcomeCopy {
    read_error_copy(ReadSurface::Sync, error.kind())
}

fn safe_detail_error(error: &ApplicationError) -> OutcomeCopy {
    read_error_copy(ReadSurface::Detail, error.kind())
}

fn safe_lookup_error(error: &ApplicationError) -> OutcomeCopy {
    read_error_copy(ReadSurface::Lookup, error.kind())
}

fn is_activation_key(event: &KeyDownEvent) -> bool {
    !event.is_held
        && !event.keystroke.modifiers.modified()
        && matches!(event.keystroke.key.as_str(), "enter" | "space")
}

fn should_close_status_filter_after_change(
    previous: IssueStatusSelection,
    next: IssueStatusSelection,
) -> bool {
    previous == IssueStatusSelection::All
        && next.values().len() == 1
        && next != IssueStatusSelection::All
}

fn refresh_complete_message(result: &RefreshResult) -> String {
    let mut parts = vec![format!(
        "Refresh complete · {} {}",
        result.cached.issues.len(),
        pluralize(result.cached.issues.len(), "issue", "issues")
    )];
    if result.outcome.events_inserted > 0 {
        parts.push(format!(
            "{} {}",
            result.outcome.events_inserted,
            pluralize(
                result.outcome.events_inserted,
                "new local update",
                "new local updates"
            )
        ));
    }
    if result.outcome.notifications_delivered > 0 {
        parts.push(format!(
            "{} desktop notification{} accepted by desktop service",
            result.outcome.notifications_delivered,
            if result.outcome.notifications_delivered == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    if result.outcome.notification_failures > 0 {
        parts.push(format!(
            "{} desktop notification{} unavailable",
            result.outcome.notification_failures,
            if result.outcome.notification_failures == 1 {
                ""
            } else {
                "s"
            }
        ));
    }
    parts.join(" · ")
}

fn pluralize<'a>(count: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

fn refresh_notification_message(result: &RefreshResult) -> String {
    let updates = match result.outcome.events_inserted {
        0 => "no new local updates".to_owned(),
        1 => "1 new local update".to_owned(),
        count => format!("{count} new local updates"),
    };
    format!(
        "Refresh complete · {} issues · {updates}",
        result.cached.issues.len()
    )
}

fn desktop_notification_error_category(error: DiagnosticErrorKind) -> &'static str {
    match error {
        DiagnosticErrorKind::Authentication => "authentication",
        DiagnosticErrorKind::Authorization => "authorization",
        DiagnosticErrorKind::RateLimited => "rate_limited",
        DiagnosticErrorKind::Offline => "offline",
        DiagnosticErrorKind::Cancelled => "cancelled",
        DiagnosticErrorKind::InvalidInput => "invalid_input",
        DiagnosticErrorKind::NotFound => "not_found",
        DiagnosticErrorKind::Upstream => "upstream",
        DiagnosticErrorKind::Storage => "storage",
        DiagnosticErrorKind::Notification => "notification",
        DiagnosticErrorKind::Internal => "internal",
        DiagnosticErrorKind::UnknownOutcome => "unknown_outcome",
    }
}

fn authenticated_identity(user: Option<User>) -> (Vec<User>, Option<AccountId>) {
    let account = user.as_ref().map(|user| user.account_id.clone());
    (user.into_iter().collect(), account)
}

fn project_label(issues: &[Issue]) -> String {
    let mut projects = issues
        .iter()
        .map(|issue| issue.project.name.clone())
        .collect::<Vec<_>>();
    projects.sort();
    projects.dedup();
    match projects.as_slice() {
        [] => "Jira projects".to_owned(),
        [project] => project.clone(),
        projects => format!("{} Jira projects", projects.len()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Section {
    Issues,
    Updates,
    Team,
    Settings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SettingsCategory {
    Appearance,
    IssueScope,
    TeamTracker,
    DesktopNotifications,
    SavedJiraLogin,
}

fn refresh_visible_for_section(section: Section) -> bool {
    section != Section::Settings
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TeamFeedbackErrorSource {
    Refresh,
    Save,
    Connection,
    PrimaryRefreshBlocked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TeamFeedback {
    Idle,
    Loading(String),
    Info(String),
    Error {
        source: TeamFeedbackErrorSource,
        message: String,
    },
}

impl TeamFeedback {
    fn display_message(&self) -> Option<String> {
        match self {
            Self::Idle => None,
            Self::Loading(message) | Self::Info(message) => Some(message.clone()),
            Self::Error { source, message } => Some(match source {
                TeamFeedbackErrorSource::Refresh => {
                    format!("Team tracker refresh failed · {message}")
                }
                TeamFeedbackErrorSource::Save | TeamFeedbackErrorSource::Connection => {
                    message.clone()
                }
                TeamFeedbackErrorSource::PrimaryRefreshBlocked => format!(
                    "Team tracker was not refreshed because Jira refresh failed · {message}"
                ),
            }),
        }
    }

    fn is_loading(&self) -> bool {
        matches!(self, Self::Loading(_))
    }

    fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    fn error_accessible_label(&self) -> Option<String> {
        self.is_error().then(|| {
            format!(
                "Team tracker error · {}",
                self.display_message()
                    .expect("error feedback has a display message")
            )
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DashboardEvent {
    AppearanceChanged(AppearancePreference),
}

#[cfg(any(feature = "ui-lab", feature = "ui-automation"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SampleSection {
    Issues,
    Updates,
    Team,
    Settings,
}

#[cfg(any(feature = "ui-lab", feature = "ui-automation"))]
impl From<SampleSection> for Section {
    fn from(section: SampleSection) -> Self {
        match section {
            SampleSection::Issues => Self::Issues,
            SampleSection::Updates => Self::Updates,
            SampleSection::Team => Self::Team,
            SampleSection::Settings => Self::Settings,
        }
    }
}

fn team_issue_counts(issues: &[Issue]) -> (usize, usize) {
    let displayed = issues
        .iter()
        .filter(|issue| {
            issue
                .status
                .category
                .as_deref()
                .is_some_and(|category| category.trim().eq_ignore_ascii_case("in progress"))
        })
        .count();
    (issues.len(), displayed)
}

fn team_refresh_feedback(prefix: &str, issues: &[Issue]) -> String {
    let (fetched, displayed) = team_issue_counts(issues);
    format!(
        "{prefix} · fetched {fetched} · displaying {displayed} in-progress {}",
        if displayed == 1 { "ticket" } else { "tickets" }
    )
}

fn team_summary(displayed: usize, configured_members: usize) -> String {
    format!(
        "{displayed} in-progress {} displayed · {configured_members} configured team {} · cached updates remain isolated from Jira issues",
        if displayed == 1 { "ticket" } else { "tickets" },
        if configured_members == 1 {
            "member"
        } else {
            "members"
        },
    )
}

fn persisted_team_member_has_display_name(member: &PersistedTeamMember) -> bool {
    !member.display_name.trim().is_empty()
        && !member.display_name.eq_ignore_ascii_case("unknown user")
        && member.display_name != member.account_id
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DesktopNotificationTestOutcome {
    Accepted { notification_id: u32 },
    Failed(DiagnosticErrorKind),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DesktopNotificationTestReport {
    timestamp: String,
    outcome: DesktopNotificationTestOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DesktopNotificationTestState {
    Idle,
    Sending,
    Completed(DesktopNotificationTestReport),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SavedLoginDeleteOutcome {
    Deleted,
    Absent,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SavedLoginDeleteState {
    Idle,
    Deleting,
    Completed(SavedLoginDeleteOutcome),
}

fn saved_login_delete_feedback(outcome: SavedLoginDeleteOutcome) -> OutcomeCopy {
    saved_login_outcome_copy(match outcome {
        SavedLoginDeleteOutcome::Deleted => SavedLoginOutcomeKind::Deleted,
        SavedLoginDeleteOutcome::Absent => SavedLoginOutcomeKind::Absent,
        SavedLoginDeleteOutcome::Error => SavedLoginOutcomeKind::Error,
    })
}

fn can_start_saved_login_delete(state: SavedLoginDeleteState) -> bool {
    !matches!(state, SavedLoginDeleteState::Deleting)
}

struct RefreshNotification;
struct CommentNotification;
struct IssueEditNotification;
struct AttachmentNotification;

#[derive(Clone, Debug)]
struct StatusOption(IssueStatusSelection);

impl SearchableListItem for StatusOption {
    type Value = IssueStatusSelection;

    fn title(&self) -> gpui::SharedString {
        self.0.label().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StatusTransitionReadState {
    Idle,
    Loading {
        issue_id: IssueId,
        generation: u64,
    },
    Ready {
        issue_id: IssueId,
    },
    Error {
        issue_id: IssueId,
        copy: OutcomeCopy,
    },
}

struct StatusTransitionListDelegate {
    transitions: Vec<IssueTransition>,
    selected: Option<gpui_component::IndexPath>,
}

const STATUS_TRANSITION_ROW_HEIGHT_REMS: f32 = 2.5;
const STATUS_TRANSITION_LIST_MAX_HEIGHT_REMS: f32 = 12.5;

fn status_transition_list_height(count: usize) -> gpui::Rems {
    rems(
        (count.max(1) as f32 * STATUS_TRANSITION_ROW_HEIGHT_REMS)
            .min(STATUS_TRANSITION_LIST_MAX_HEIGHT_REMS),
    )
}

impl gpui_component::list::ListDelegate for StatusTransitionListDelegate {
    type Item = gpui_component::list::ListItem;

    fn items_count(&self, _: usize, _: &gpui::App) -> usize {
        self.transitions.len()
    }

    fn render_item(
        &mut self,
        ix: gpui_component::IndexPath,
        _: &mut gpui::Window,
        _: &mut Context<gpui_component::list::ListState<Self>>,
    ) -> Option<Self::Item> {
        let transition = self.transitions.get(ix.row)?.clone();
        let target = transition_option_label(&transition).to_owned();
        let action = transition.name.trim().to_owned();
        let has_distinguishing_action = !action.is_empty() && action != target;
        let selector = format!("status-transition-{}", transition.id);
        Some(
            gpui_component::list::ListItem::new(selector.clone())
                .selected(self.selected == Some(ix))
                .child(
                    div()
                        .debug_selector(move || selector.clone())
                        .min_w_0()
                        .child(
                            v_flex()
                                .min_w_0()
                                .gap_0p5()
                                .child(div().min_w_0().truncate().child(target))
                                .when(has_distinguishing_action, |this| {
                                    this.child(div().min_w_0().truncate().text_xs().child(action))
                                }),
                        ),
                ),
        )
    }

    fn set_selected_index(
        &mut self,
        ix: Option<gpui_component::IndexPath>,
        _: &mut gpui::Window,
        cx: &mut Context<gpui_component::list::ListState<Self>>,
    ) {
        self.selected = ix;
        cx.notify();
    }

    fn confirm(
        &mut self,
        _: bool,
        _: &mut gpui::Window,
        _: &mut Context<gpui_component::list::ListState<Self>>,
    ) {
    }

    fn cancel(
        &mut self,
        _: &mut gpui::Window,
        _: &mut Context<gpui_component::list::ListState<Self>>,
    ) {
    }
}

fn status_filter_trigger_label(selection: IssueStatusSelection) -> String {
    let count = selection.values().len();
    if count > 1 {
        format!("{count} statuses")
    } else {
        selection.label().to_owned()
    }
}

fn status_options() -> SearchableVec<StatusOption> {
    SearchableVec::new([
        StatusOption(IssueStatusSelection::ToDo),
        StatusOption(IssueStatusSelection::InProgress),
        StatusOption(IssueStatusSelection::Done),
        StatusOption(IssueStatusSelection::Uncategorized),
    ])
}

fn status_filter_indices(selection: IssueStatusSelection) -> Vec<gpui_component::IndexPath> {
    let selected = selection.values();
    [
        IssueStatusSelection::ToDo,
        IssueStatusSelection::InProgress,
        IssueStatusSelection::Done,
        IssueStatusSelection::Uncategorized,
    ]
    .into_iter()
    .enumerate()
    .filter(|(_, value)| selected.contains(value))
    .map(|(index, _)| gpui_component::IndexPath::new(index))
    .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DetailState {
    Empty,
    Loading {
        issue_id: IssueId,
    },
    Refreshing {
        issue_id: IssueId,
        detail: IssueDetailViewModel,
    },
    RemoteLoading {
        query: String,
    },
    Loaded(IssueDetailViewModel),
    Error {
        issue_id: IssueId,
        copy: OutcomeCopy,
    },
    RemoteError {
        query: String,
        copy: OutcomeCopy,
    },
}

fn transition_option_label(transition: &IssueTransition) -> &str {
    transition.to.name.as_str()
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
        copy: OutcomeCopy,
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

#[cfg(test)]
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

fn selected_issue_view_from_sources(
    selected_issue: Option<&IssueId>,
    visible_issues: &[IssueViewModel],
    domain_issues: &[Issue],
    selected_issue_core: Option<&Issue>,
    users: &[User],
) -> Option<IssueViewModel> {
    let selected = selected_issue?;
    visible_issues
        .iter()
        .find(|issue| &issue.id == selected)
        .cloned()
        .or_else(|| {
            domain_issues
                .iter()
                .find(|issue| &issue.id == selected)
                .map(|issue| IssueViewModel::from_domain(issue, users))
        })
        .or_else(|| {
            selected_issue_core
                .filter(|issue| &issue.id == selected)
                .map(|issue| IssueViewModel::from_domain(issue, users))
        })
}

fn selected_issue_from_sources<'a>(
    selected_issue: Option<&IssueId>,
    domain_issues: &'a [Issue],
    selected_issue_core: Option<&'a Issue>,
) -> Option<&'a Issue> {
    let selected = selected_issue?;
    domain_issues
        .iter()
        .find(|issue| &issue.id == selected)
        .or_else(|| selected_issue_core.filter(|issue| &issue.id == selected))
}

fn detail_view_from_issue(issue: &Issue) -> IssueDetailViewModel {
    IssueDetailViewModel {
        description: issue
            .description_text
            .clone()
            .unwrap_or_else(|| "No description supplied.".to_owned()),
        rich_description: issue.rich_description.clone(),
        comments: Vec::new(),
        attachments: Vec::new(),
    }
}

fn issue_has_cached_detail(issue: &Issue) -> bool {
    issue.description_text.is_some() || issue.rich_description.is_some()
}

fn selection_after_issue_view_rebuild(
    selected_issue: Option<IssueId>,
    visible_issues: &[IssueViewModel],
) -> Option<IssueId> {
    selected_issue.or_else(|| visible_issues.first().map(|issue| issue.id.clone()))
}

fn should_defer_detail_refresh(
    selected_issue: Option<&IssueId>,
    domain_issues: &[Issue],
    detail_state: &DetailState,
    refresh_detail: bool,
) -> bool {
    let Some(selected_issue) = selected_issue else {
        return false;
    };
    refresh_detail
        && !domain_issues
            .iter()
            .any(|issue| &issue.id == selected_issue)
        && matches!(
            detail_state,
            DetailState::Loading { issue_id }
            | DetailState::Refreshing { issue_id, .. }
            | DetailState::Error { issue_id, .. }
                if issue_id == selected_issue
        )
}

pub struct Dashboard {
    diagnostics: DiagnosticsSink,
    section: Section,
    settings_category: SettingsCategory,
    sidebar_collapsed: bool,
    appearance_preference: AppearancePreference,
    domain_issues: Vec<Issue>,
    issues: Vec<IssueViewModel>,
    update_groups: Vec<UpdateGroupViewModel>,
    update_filter: UpdateFilter,
    expanded_update_groups: HashSet<IssueId>,
    selected_issue: Option<IssueId>,
    selected_issue_core: Option<Issue>,
    mobile_detail_open: bool,
    sync_message: String,
    workspace: Option<Arc<LiveWorkspace>>,
    users: Vec<User>,
    workspace_name: String,
    workspace_members: String,
    team_issues: Vec<Issue>,
    team_events: Vec<jira_domain::UpdateEvent>,
    team_members: Vec<PersistedTeamMember>,
    team_table: Option<Entity<TableState<TeamTicketTableDelegate>>>,
    team_table_subscriptions: Vec<Subscription>,
    team_input: Option<Entity<TextareaState>>,
    team_text: String,
    team_feedback: TeamFeedback,
    team_task: Option<gpui::Task<()>>,
    team_age_task: Option<gpui::Task<()>>,
    team_panes_state: Option<Entity<ResizableState>>,
    team_panes_subscription: Option<Subscription>,
    /// Fixture dashboards use a fixed clock; live dashboards resolve the clock at refresh time.
    team_clock: Option<jira_domain::Timestamp>,
    /// Fixture dashboards render all timestamps in UTC; live dashboards retain local time.
    timestamp_offset: Option<UtcOffset>,
    team_automatic_polling_paused: bool,
    site_label: String,
    mode_label: String,
    operation_in_progress: bool,
    polling_task: Option<gpui::Task<()>>,
    automatic_polling_paused: bool,
    authenticated_account: Option<AccountId>,
    status_filter: IssueStatusFilter,
    status_combobox: Option<Entity<ComboboxState<SearchableVec<StatusOption>>>>,
    status_subscriptions: Vec<Subscription>,
    status_list: Option<Entity<gpui_component::list::ListState<StatusTransitionListDelegate>>>,
    status_list_subscriptions: Vec<Subscription>,
    status_transition_items: Vec<IssueTransition>,
    status_transition_items_revision: u64,
    status_transition_items_applied_revision: u64,
    status_popover_open: bool,
    status_transition_state: StatusTransitionReadState,
    status_transition_generation: u64,
    status_transition_task: Option<gpui::Task<()>>,
    status_transition_cancellation: Option<CancellationToken>,
    #[cfg(test)]
    status_transition_reads_suppressed: bool,
    search_query: String,
    search_input: Option<Entity<InputState>>,
    search_subscriptions: Vec<Subscription>,
    detail_state: DetailState,
    detail_epoch: RequestEpoch<RequestSource, IssueId>,
    detail_task: Option<gpui::Task<()>>,
    detail_cache_task: Option<gpui::Task<()>>,
    selected_image_states: RichImageRenderStates,
    remote_image_states: RichImageRenderStates,
    remote_lookup: RemoteLookupState,
    remote_lookup_epoch: RequestEpoch<RequestSource, String>,
    remote_lookup_task: Option<gpui::Task<()>>,
    comment_input: Option<Entity<TextareaState>>,
    comment_subscriptions: Vec<Subscription>,
    comment_flow: CommentFlow,
    comment_cancellation: Option<CancellationToken>,
    comment_task: Option<gpui::Task<()>>,
    issue_edit_flow: IssueEditFlow,
    issue_edit_cancellation: Option<CancellationToken>,
    issue_edit_task: Option<gpui::Task<()>>,
    assignee_input: Option<Entity<InputState>>,
    assignee_subscriptions: Vec<Subscription>,
    attachment_download_state: AttachmentDownloadState,
    attachment_download_generation: u64,
    attachment_download_cancellation: Option<CancellationToken>,
    attachment_download_task: Option<gpui::Task<()>>,
    settings_input: Option<Entity<TextareaState>>,
    settings_subscriptions: Vec<Subscription>,
    settings_scope_text: String,
    settings_warning: Option<String>,
    settings_feedback: Option<String>,
    settings_task: Option<gpui::Task<()>>,
    saved_login_delete_state: SavedLoginDeleteState,
    saved_login_delete_task: Option<gpui::Task<()>>,
    desktop_notification_test_state: DesktopNotificationTestState,
    desktop_notification_test_task: Option<gpui::Task<()>>,
}

impl EventEmitter<DashboardEvent> for Dashboard {}

impl Dashboard {
    fn ensure_team_panes_state(&mut self, cx: &mut Context<Self>) {
        if self.team_panes_state.is_some() {
            return;
        }

        let state = cx.new(|_| ResizableState::default());
        self.team_panes_subscription = Some(cx.observe(&state, |_, _, cx| cx.notify()));
        self.team_panes_state = Some(state);
    }

    #[cfg(any(test, feature = "ui-lab", feature = "ui-automation"))]
    pub(crate) fn initialize_appearance_preference(&mut self, preference: AppearancePreference) {
        self.appearance_preference = preference;
    }

    pub(crate) fn select_appearance_preference(
        &mut self,
        preference: AppearancePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.appearance_preference = preference;
        match preference.manual_theme_mode() {
            Some(mode) => Theme::change(mode, Some(window), cx),
            None => Theme::sync_system_appearance(Some(window), cx),
        }
        cx.emit(DashboardEvent::AppearanceChanged(preference));
        cx.notify();
    }

    pub(crate) fn toggle_sidebar(&mut self, layout: LayoutMode, cx: &mut Context<Self>) {
        if layout.supports_manual_sidebar_collapse() {
            self.sidebar_collapsed = !self.sidebar_collapsed;
            cx.notify();
        }
    }

    fn selected_detail_ticket_is_current(
        &self,
        ticket: &RequestTicket<RequestSource, IssueId>,
    ) -> bool {
        self.detail_epoch.is_current(ticket)
            && image_result_is_current(
                self.selected_issue.as_ref(),
                ticket.key(),
                ticket.generation(),
                ticket.generation(),
            )
    }

    #[cfg(any(test, feature = "ui-lab", feature = "ui-automation"))]
    pub(crate) fn from_sample_data() -> Self {
        Self::from_sample_data_with_diagnostics(DiagnosticsSink::disabled(), Section::Issues)
    }

    #[cfg(any(feature = "ui-lab", feature = "ui-automation"))]
    pub(crate) fn from_sample_data_for_section(section: SampleSection) -> Self {
        let mut dashboard = match section {
            SampleSection::Issues => Self::from_sample_data(),
            SampleSection::Updates | SampleSection::Team | SampleSection::Settings => {
                Self::from_sample_data_with_diagnostics(DiagnosticsSink::disabled(), section.into())
            }
        };
        dashboard.section = section.into();
        if matches!(section, SampleSection::Team) {
            dashboard.team_members = vec![
                PersistedTeamMember {
                    identifier: "amina".to_owned(),
                    account_id: "amina".to_owned(),
                    display_name: "Amina Yusuf".to_owned(),
                },
                PersistedTeamMember {
                    identifier: "devon".to_owned(),
                    account_id: "devon".to_owned(),
                    display_name: "Devon Park".to_owned(),
                },
            ];
            dashboard.team_issues =
                dashboard
                    .domain_issues
                    .iter()
                    .filter(|issue| {
                        issue.status.category.as_deref().is_some_and(|category| {
                            category.trim().eq_ignore_ascii_case("in progress")
                        }) && issue.assignee.as_ref().is_some_and(|assignee| {
                            dashboard
                                .team_members
                                .iter()
                                .any(|member| member.account_id == assignee.as_str())
                        })
                    })
                    .cloned()
                    .collect();
            dashboard.team_events = sample_updates();
            dashboard.team_text = dashboard
                .team_members
                .iter()
                .map(|member| member.identifier.as_str())
                .collect::<Vec<_>>()
                .join("\n");
        }
        dashboard
    }

    /// Builds the rich-content fixture used exclusively by the local macOS accessibility tests.
    /// It starts from the ordinary deterministic issue fixture, then preloads one valid image so
    /// the test can prove that a selected cached image paints without a loading spinner.
    #[cfg(feature = "ui-automation")]
    pub(crate) fn from_ui_automation_rich_content() -> Self {
        use jira_domain::{
            RichBlock, RichImage, RichInline, RichStatusColor, RichTable, RichTableCell,
            RichTableRow, RichTextDocument,
        };

        let mut dashboard = Self::from_sample_data();
        let image = RichImage {
            attachment_id: "fixture-image".to_owned(),
            filename: "cached-fixture.png".to_owned(),
            mime_type: "image/png".to_owned(),
            alt_text: Some("Cached fixture image".to_owned()),
            width: Some(1),
            height: Some(1),
        };
        let text = |value: &str| {
            RichBlock::Paragraph(vec![RichInline::Text {
                text: value.to_owned(),
                marks: Vec::new(),
            }])
        };
        let status = |text: &str, color: RichStatusColor| {
            RichBlock::Paragraph(vec![RichInline::Status {
                text: text.to_owned(),
                color,
            }])
        };
        let sentence = |parts: &[&str]| {
            RichBlock::Paragraph(
                parts
                    .iter()
                    .map(|part| RichInline::Text {
                        text: (*part).to_owned(),
                        marks: Vec::new(),
                    })
                    .collect(),
            )
        };
        let table = RichTable {
            rows: vec![
                RichTableRow {
                    cells: vec![
                        RichTableCell {
                            header: true,
                            content: vec![text("Fixture field")],
                        },
                        RichTableCell {
                            header: true,
                            content: vec![text("Value")],
                        },
                    ],
                },
                RichTableRow {
                    cells: vec![
                        RichTableCell {
                            header: false,
                            content: vec![status("Pass", RichStatusColor::Green)],
                        },
                        RichTableCell {
                            header: false,
                            content: vec![
                                status("Fail", RichStatusColor::Red),
                                text("Cache is preloaded"),
                            ],
                        },
                    ],
                },
            ],
        };
        let issue = dashboard
            .domain_issues
            .first_mut()
            .expect("sample issue fixture");
        issue.description_text = Some("Rich content fixture".to_owned());
        issue.rich_description = Some(RichTextDocument::new(
            vec![
                sentence(&["Epic: ", "ENG-43"]),
                sentence(&["Per the ", "ENG-43", ", after"]),
                text("OPS-7"),
                text("Rich content fixture"),
                RichBlock::horizontal_rule(),
                RichBlock::Table(table),
                RichBlock::Image(image.clone()),
            ],
            false,
        ));
        dashboard.selected_image_states.insert(
            image.attachment_id,
            crate::rich_text_view::RichImageRenderState::Ready(Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                // A valid 1×1 RGBA PNG. Keeping it inline makes this fixture hermetic.
                vec![
                    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49,
                    0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06,
                    0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44,
                    0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00,
                    0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
                    0x44, 0xae, 0x42, 0x60, 0x82,
                ],
            ))),
        );
        dashboard.site_label = "sample".to_owned();
        dashboard
    }

    #[cfg(any(test, feature = "ui-lab", feature = "ui-automation"))]
    fn from_sample_data_with_diagnostics(diagnostics: DiagnosticsSink, section: Section) -> Self {
        let domain_issues = sample_issues();
        let users = sample_users();
        let sample_events = sample_updates();
        let timestamp_offset = Some(UtcOffset::UTC);
        let update_groups = update_groups_for_events_with_offset(
            &sample_events,
            &domain_issues,
            &users,
            timestamp_offset,
        );
        let issues = issue_views_for_filter_with_offset(
            &domain_issues,
            &users,
            IssueStatusFilter::All,
            "",
            timestamp_offset,
        );
        let selected_issue = issues.first().map(|issue| issue.id.clone());

        Self {
            diagnostics: diagnostics.clone(),
            section,
            settings_category: SettingsCategory::Appearance,
            sidebar_collapsed: false,
            appearance_preference: AppearancePreference::System,
            domain_issues,
            issues,
            update_groups,
            update_filter: UpdateFilter::All,
            expanded_update_groups: HashSet::new(),
            selected_issue,
            selected_issue_core: None,
            mobile_detail_open: false,
            sync_message: "Preview data · Jira connection not configured".to_owned(),
            workspace: None,
            users,
            workspace_name: "Platform team".to_owned(),
            workspace_members: "Amina, Devon, Marco".to_owned(),
            team_issues: Vec::new(),
            team_events: Vec::new(),
            team_members: Vec::new(),
            team_table: None,
            team_table_subscriptions: Vec::new(),
            team_input: None,
            team_text: String::new(),
            team_feedback: TeamFeedback::Idle,
            team_task: None,
            team_age_task: None,
            team_panes_state: None,
            team_panes_subscription: None,
            team_clock: Some(time::macros::datetime!(2026-08-18 00:00 UTC)),
            timestamp_offset: Some(UtcOffset::UTC),
            team_automatic_polling_paused: false,
            site_label: "sample".to_owned(),
            mode_label: "Local preview mode".to_owned(),
            operation_in_progress: false,
            polling_task: None,
            automatic_polling_paused: false,
            authenticated_account: None,
            status_filter: IssueStatusFilter::All,
            status_combobox: None,
            status_subscriptions: Vec::new(),
            status_list: None,
            status_list_subscriptions: Vec::new(),
            status_transition_items: Vec::new(),
            status_transition_items_revision: 0,
            status_transition_items_applied_revision: 0,
            status_popover_open: false,
            status_transition_state: StatusTransitionReadState::Idle,
            status_transition_generation: 0,
            status_transition_task: None,
            status_transition_cancellation: None,
            #[cfg(test)]
            status_transition_reads_suppressed: false,
            search_query: String::new(),
            search_input: None,
            search_subscriptions: Vec::new(),
            detail_state: DetailState::Empty,
            detail_epoch: RequestEpoch::default(),
            detail_task: None,
            detail_cache_task: None,
            selected_image_states: RichImageRenderStates::with_context(
                diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                0,
            ),
            remote_image_states: RichImageRenderStates::with_context(
                diagnostics.clone(),
                DiagnosticFlow::RemoteLookup,
                0,
            ),
            remote_lookup: RemoteLookupState::Idle,
            remote_lookup_epoch: RequestEpoch::default(),
            remote_lookup_task: None,
            comment_input: None,
            comment_subscriptions: Vec::new(),
            comment_flow: CommentFlow::new(),
            comment_cancellation: None,
            comment_task: None,
            issue_edit_flow: IssueEditFlow::new(),
            issue_edit_cancellation: None,
            issue_edit_task: None,
            assignee_input: None,
            assignee_subscriptions: Vec::new(),
            attachment_download_state: AttachmentDownloadState::Idle,
            attachment_download_generation: 0,
            attachment_download_cancellation: None,
            attachment_download_task: None,
            settings_input: None,
            settings_subscriptions: Vec::new(),
            settings_scope_text: DEFAULT_JQL_SCOPE.to_owned(),
            settings_warning: None,
            settings_feedback: None,
            settings_task: None,
            saved_login_delete_state: SavedLoginDeleteState::Idle,
            saved_login_delete_task: None,
            desktop_notification_test_state: DesktopNotificationTestState::Idle,
            desktop_notification_test_task: None,
        }
    }

    pub(crate) fn from_live(
        session: LiveSession,
        diagnostics: DiagnosticsSink,
        cx: &mut Context<Self>,
    ) -> Self {
        let (users, initial_authenticated_account) =
            authenticated_identity(session.authenticated_user.clone());
        let dashboard = Self {
            diagnostics: diagnostics.clone(),
            section: Section::Issues,
            settings_category: SettingsCategory::Appearance,
            sidebar_collapsed: false,
            appearance_preference: AppearancePreference::System,
            domain_issues: Vec::new(),
            issues: Vec::new(),
            update_groups: Vec::new(),
            update_filter: UpdateFilter::All,
            expanded_update_groups: HashSet::new(),
            selected_issue: None,
            selected_issue_core: None,
            mobile_detail_open: false,
            sync_message: "Opening local cache…".to_owned(),
            workspace: None,
            users,
            workspace_name: "Jira projects".to_owned(),
            workspace_members: if initial_authenticated_account.is_some() {
                "Authenticated Jira account".to_owned()
            } else {
                "Environment bootstrap · Assigned or watched view unavailable".to_owned()
            },
            team_issues: Vec::new(),
            team_events: Vec::new(),
            team_members: Vec::new(),
            team_table: None,
            team_table_subscriptions: Vec::new(),
            team_input: None,
            team_text: String::new(),
            team_feedback: TeamFeedback::Idle,
            team_task: None,
            team_age_task: None,
            team_panes_state: None,
            team_panes_subscription: None,
            team_clock: None,
            timestamp_offset: None,
            team_automatic_polling_paused: false,
            site_label: session.site_label,
            mode_label:
                "Live Jira sync · confirmed comments, assignee changes, and status transitions · best-effort desktop notifications"
                    .to_owned(),
            operation_in_progress: true,
            polling_task: None,
            automatic_polling_paused: false,
            authenticated_account: initial_authenticated_account,
            status_filter: IssueStatusFilter::All,
            status_combobox: None,
            status_subscriptions: Vec::new(),
            status_list: None,
            status_list_subscriptions: Vec::new(),
            status_transition_items: Vec::new(),
            status_transition_items_revision: 0,
            status_transition_items_applied_revision: 0,
            status_popover_open: false,
            status_transition_state: StatusTransitionReadState::Idle,
            status_transition_generation: 0,
            status_transition_task: None,
            status_transition_cancellation: None,
            #[cfg(test)]
            status_transition_reads_suppressed: false,
            search_query: String::new(),
            search_input: None,
            search_subscriptions: Vec::new(),
            detail_state: DetailState::Empty,
            detail_epoch: RequestEpoch::default(),
            detail_task: None,
            detail_cache_task: None,
            selected_image_states: RichImageRenderStates::with_context(
                diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                0,
            ),
            remote_image_states: RichImageRenderStates::with_context(
                diagnostics.clone(),
                DiagnosticFlow::RemoteLookup,
                0,
            ),
            remote_lookup: RemoteLookupState::Idle,
            remote_lookup_epoch: RequestEpoch::default(),
            remote_lookup_task: None,
            comment_input: None,
            comment_subscriptions: Vec::new(),
            comment_flow: CommentFlow::new(),
            comment_cancellation: None,
            comment_task: None,
            issue_edit_flow: IssueEditFlow::new(),
            issue_edit_cancellation: None,
            issue_edit_task: None,
            assignee_input: None,
            assignee_subscriptions: Vec::new(),
            attachment_download_state: AttachmentDownloadState::Idle,
            attachment_download_generation: 0,
            attachment_download_cancellation: None,
            attachment_download_task: None,
            settings_input: None,
            settings_subscriptions: Vec::new(),
            settings_scope_text: DEFAULT_JQL_SCOPE.to_owned(),
            settings_warning: None,
            settings_feedback: None,
            settings_task: None,
            saved_login_delete_state: SavedLoginDeleteState::Idle,
            saved_login_delete_task: None,
            desktop_notification_test_state: DesktopNotificationTestState::Idle,
            desktop_notification_test_task: None,
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
                    let (saved_scope, saved_team, preference_warning) = match load_preferences() {
                        Ok(preferences) => {
                            let (scope, scope_warning) = match normalize_issue_jql_scope(
                                preferences.issue_jql_scope.clone(),
                            ) {
                                Ok(scope) => (scope, None),
                                Err(_) => (
                                    None,
                                    Some(
                                        "Saved Jira scope was invalid; using the default scope"
                                            .to_owned(),
                                    ),
                                ),
                            };
                            let (team, team_warning) = match normalize_team_members(preferences.team_members) {
                                Ok(team) => (team, None),
                                Err(_) => (Vec::new(), Some("Saved team tracker settings were invalid; using an empty team".to_owned())),
                            };
                            let warning = scope_warning.or(team_warning);
                            (scope, team, warning)
                        }
                        Err(_) => (
                            None,
                            Vec::new(),
                            Some(
                                "Saved Jira settings could not be read; using default Jira and team settings"
                                    .to_owned(),
                            ),
                        ),
                    };
                    let authenticated_account = authenticated_user.account_id.clone();
                    let jira_read: Arc<dyn JiraReadPort> = jira.clone();
                    let jira_comment_write: Arc<dyn JiraCommentWritePort> = jira.clone();
                    let jira_issue_edit: Arc<dyn JiraIssueEditPort> = jira.clone();
                    match LiveWorkspace::initialize_with_writers_and_scope(
                        site_id,
                        Some(authenticated_account),
                        jira_read,
                        jira_comment_write,
                        jira_issue_edit,
                        cache,
                        saved_scope,
                    )
                    .await
                    {
                        Ok(workspace) => {
                            let workspace = Arc::new(workspace);
                            let team_accounts = saved_team
                                .iter()
                                .filter_map(|member| AccountId::new(member.account_id.clone()).ok())
                                .collect::<Vec<_>>();
                            match workspace.configure_team_members(team_accounts).await {
                                Err(error) => Err(safe_sync_error(&error).to_owned()),
                                Ok(()) => match workspace.load_cached_for_authenticated_account().await {
                                    Err(error) => Err(safe_sync_error(&error).to_owned()),
                                    Ok(cached) => match workspace.load_cached_team().await {
                                        Err(error) => Err(safe_sync_error(&error).to_owned()),
                                        Ok(team_cached) => Ok((workspace, cached, team_cached, authenticated_user, saved_team, preference_warning)),
                                    },
                                },
                            }
                        }
                        Err(error) => Err(safe_sync_error(&error).to_owned()),
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok((workspace, cached, team_cached, authenticated_user, saved_team, preference_warning)) => {
                        let issue_count = cached.issues.len();
                        let update_count = cached.events.len();
                        this.users = vec![authenticated_user.clone()];
                        this.users.extend(saved_team.iter().filter_map(|member| {
                            if !persisted_team_member_has_display_name(member) {
                                return None;
                            }
                            AccountId::new(member.account_id.clone()).ok().map(|account_id| {
                                User::new(workspace.site_id().clone(), account_id, member.display_name.clone(), None, true)
                            })
                        }));
                        this.team_members = saved_team;
                        this.team_text = this.team_members.iter().map(|member| member.identifier.clone()).collect::<Vec<_>>().join("\n");
                        if let Some(input) = this.team_input.clone() {
                            input.update(cx, |input, cx| {
                                input.set_value(&this.team_text, window, cx)
                            });
                        }
                        this.authenticated_account = Some(authenticated_user.account_id.clone());
                        this.settings_scope_text = workspace
                            .jql_scope()
                            .unwrap_or_else(|| DEFAULT_JQL_SCOPE.to_owned());
                        if let Some(input) = this.settings_input.clone() {
                            input.update(cx, |input, cx| {
                                input.set_value(&this.settings_scope_text, window, cx)
                            });
                        }
                        this.settings_warning = preference_warning;
                        this.workspace_members = "Authenticated Jira account".to_owned();
                        this.workspace = Some(workspace);
                        this.apply_cached(cached, cx);
                        this.apply_team_cached(team_cached, cx);
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
                        if !this.team_members.is_empty() && !this.team_automatic_polling_paused {
                            this.team_feedback =
                                TeamFeedback::Loading("Refreshing team tracker…".to_owned());
                        }
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
                let team_polling_allowed =
                    match this.update(cx, |this, _| !this.team_automatic_polling_paused) {
                        Ok(allowed) => allowed,
                        Err(_) => break,
                    };
                let team_result = if result.is_ok() && team_polling_allowed {
                    Some(
                        workspace
                            .refresh_team_automatically(&CancellationToken::new())
                            .await,
                    )
                } else {
                    None
                };
                let next_delay = match this.update(cx, |this, cx| {
                    this.operation_in_progress = false;
                    match result {
                        Ok(result) => {
                            consecutive_failures = 0;
                            this.sync_message = refresh_complete_message(&result);
                            this.apply_cached(result.cached, cx);
                            if let Some(team_result) = team_result {
                                match team_result {
                                    Ok(team_result) => {
                                        this.apply_team_cached(team_result.cached, cx);
                                        this.team_feedback =
                                            TeamFeedback::Info(team_refresh_feedback(
                                                "Team tracker refreshed",
                                                &this.team_issues,
                                            ));
                                    }
                                    Err(error) => {
                                        this.team_feedback = TeamFeedback::Error {
                                            source: TeamFeedbackErrorSource::Refresh,
                                            message: safe_sync_error(&error).to_owned(),
                                        };
                                    }
                                }
                            }
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
                            if this.team_feedback.is_loading() {
                                this.team_feedback = TeamFeedback::Error {
                                    source: TeamFeedbackErrorSource::PrimaryRefreshBlocked,
                                    message: safe_sync_error(&error).to_owned(),
                                };
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

    fn apply_live_issues(
        &mut self,
        issues: Vec<Issue>,
        refresh_detail: bool,
        cx: &mut Context<Self>,
    ) {
        self.domain_issues = issues;
        self.workspace_name = project_label(&self.domain_issues);
        self.rebuild_issue_views(refresh_detail, cx);
    }

    fn rebuild_issue_views(&mut self, refresh_detail: bool, cx: &mut Context<Self>) {
        self.issues = issue_views_for_filter_with_offset(
            &self.domain_issues,
            &self.users,
            self.status_filter,
            &self.search_query,
            self.timestamp_offset,
        );
        let retained_selection =
            selection_after_issue_view_rebuild(self.selected_issue.clone(), &self.issues);
        if self.selected_issue.is_none() {
            if let Some(issue_id) = retained_selection {
                self.select_issue(issue_id, cx, true);
            }
        } else if refresh_detail
            && !should_defer_detail_refresh(
                self.selected_issue.as_ref(),
                &self.domain_issues,
                &self.detail_state,
                refresh_detail,
            )
        {
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
        self.invalidate_attachment_download();
        self.search_query = query;
        self.rebuild_issue_views(false, cx);
        cx.notify();
    }

    fn clear_remote_lookup(&mut self) {
        self.status_popover_open = false;
        self.invalidate_attachment_download();
        self.remote_image_states.clear();
        self.remote_lookup_epoch.invalidate();
        self.remote_lookup_task.take();
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

        let load_token = self.diagnostics.begin_image_load();
        self.invalidate_comment_selection();
        let expected_query = query.clone();
        let ticket = self
            .remote_lookup_epoch
            .begin(RequestSource::RemoteLookup, expected_query.clone());
        // Drop the prior task at the same point as its cancellation: before
        // installing or spawning the replacement task.
        self.remote_lookup_task.take();
        let Some(workspace) = self.workspace.clone() else {
            self.remote_lookup_epoch.finish(&ticket);
            self.remote_image_states.clear();
            self.remote_lookup = RemoteLookupState::Error {
                query,
                copy: lookup_workspace_unavailable_copy(),
            };
            cx.notify();
            return;
        };

        let cancellation = ticket.cancellation().clone();
        self.remote_image_states.set_context(
            self.diagnostics.clone(),
            DiagnosticFlow::RemoteLookup,
            load_token,
        );
        self.remote_lookup = RemoteLookupState::Loading {
            query: expected_query.clone(),
        };
        let read_request = DetailReadRequest::Remote { key };
        let users = self.users.clone();
        let diagnostics = self.diagnostics.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = read_detail(workspace.as_ref(), read_request.clone(), &cancellation).await;
            let detail = match result {
                Ok(detail) => detail,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if !this.remote_lookup_epoch.is_current(&ticket) {
                            return;
                        }
                        this.remote_lookup_epoch.finish(&ticket);
                        this.remote_lookup_task = None;
                        this.remote_image_states.clear();
                        this.remote_lookup = RemoteLookupState::Error {
                            query: expected_query.clone(),
                            copy: safe_lookup_error(&error),
                        };
                        cx.notify();
                    });
                    return;
                }
            };
            let payload = prepare_detail_payload(
                &detail,
                &users,
                &diagnostics,
                DiagnosticFlow::RemoteLookup,
                load_token,
            );
            let image_contexts = payload.image_contexts.clone();
            let issue = payload.issue.clone();
            let image_issue_id = detail_image_issue_id(&read_request, &issue.id);
            let view = payload.view.clone();
            let loading = payload.loading.clone();
            let applied = this
                .update(cx, |this, cx| {
                    if !this.remote_lookup_epoch.is_current(&ticket) {
                        return false;
                    }
                    this.remote_lookup = RemoteLookupState::Loaded {
                        query: expected_query.clone(),
                        issue,
                        detail: view,
                    };
                    this.remote_image_states = loading;
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !applied {
                for (candidate_ordinal, (surface_ordinal, source)) in
                    image_contexts.iter().copied().enumerate()
                {
                    diagnostics.image_state(
                        DiagnosticFlow::RemoteLookup,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Stale,
                    );
                }
                return;
            }
            let states = fetch_detail_images(
                workspace,
                image_issue_id,
                payload,
                cancellation,
                diagnostics.clone(),
                DiagnosticFlow::RemoteLookup,
                load_token,
            )
            .await;
            let applied = this
                .update(cx, |this, cx| {
                    if !this.remote_lookup_epoch.is_current(&ticket) {
                        return false;
                    }
                    this.remote_lookup_epoch.finish(&ticket);
                    this.remote_lookup_task = None;
                    if let Ok(states) = states {
                        this.remote_image_states = states;
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !applied {
                for (candidate_ordinal, (surface_ordinal, source)) in
                    image_contexts.iter().copied().enumerate()
                {
                    diagnostics.image_state(
                        DiagnosticFlow::RemoteLookup,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Stale,
                    );
                }
            }
        });
        self.remote_lookup_task = Some(task);
        cx.notify();
    }

    fn invalidate_detail_selection(&mut self) {
        self.invalidate_attachment_download();
        self.selected_image_states.clear();
        self.detail_epoch.invalidate();
        self.detail_task.take();
        self.detail_cache_task.take();
        self.selected_issue = None;
        self.selected_issue_core = None;
        self.detail_state = DetailState::Empty;
    }

    fn invalidate_comment_selection(&mut self) {
        self.comment_input = None;
        self.comment_subscriptions.clear();
        if matches!(
            self.comment_flow.invalidate_selection(),
            CommentInvalidation::CancelPreDispatch
        ) {
            if let Some(cancellation) = self.comment_cancellation.take() {
                cancellation.cancel();
            }
            self.comment_task.take();
        }
        // A dispatched POST is never cancelled; its completion is ignored by
        // the flow's generation guard after selection invalidation.
    }

    fn invalidate_issue_edit_selection(&mut self) {
        self.assignee_input = None;
        self.assignee_subscriptions.clear();
        if self.issue_edit_flow.invalidate_selection() {
            if let Some(cancellation) = self.issue_edit_cancellation.take() {
                cancellation.cancel();
            }
            self.issue_edit_task.take();
        }
        // A dispatched write is never cancelled or converted into a retryable
        // state; its completion is simply ignored after this generation bump.
    }

    fn clear_selection_for_team_scope(&mut self, cx: &mut Context<Self>) {
        if self.issue_edit_flow.is_submitting() {
            return;
        }
        self.clear_remote_lookup();
        self.invalidate_detail_selection();
        self.invalidate_comment_selection();
        self.invalidate_issue_edit_selection();
        cx.notify();
    }

    fn select_issue(&mut self, issue_id: IssueId, cx: &mut Context<Self>, force: bool) {
        if self.issue_edit_flow.is_submitting() && self.selected_issue.as_ref() != Some(&issue_id) {
            self.sync_message =
                "Finish the confirmed Jira change before changing issues".to_owned();
            cx.notify();
            return;
        }
        let selection_changed = self.selected_issue.as_ref() != Some(&issue_id);
        if self.selected_issue.as_ref() == Some(&issue_id)
            && !force
            && matches!(
                self.detail_state,
                DetailState::Loading { .. }
                    | DetailState::Refreshing { .. }
                    | DetailState::Loaded(_)
            )
        {
            return;
        }
        let load_token = self.diagnostics.begin_image_load();
        let ticket = self
            .detail_epoch
            .begin(RequestSource::SelectedDetail, issue_id.clone());
        self.invalidate_attachment_download();
        if selection_changed {
            self.selected_image_states.set_context(
                self.diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                load_token,
            );
        } else {
            self.selected_image_states.rebind_context(
                self.diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                load_token,
            );
        }
        self.invalidate_comment_selection();
        self.invalidate_issue_edit_selection();
        // Preserve the prior task-drop position after all selection
        // invalidations; begin above already cancelled its token.
        self.detail_task.take();
        self.detail_cache_task.take();
        if selection_changed {
            self.selected_issue_core = None;
        }
        self.selected_issue = Some(issue_id.clone());
        self.invalidate_status_transition();

        let Some(workspace) = self.workspace.clone() else {
            self.detail_epoch.finish(&ticket);
            self.detail_state = DetailState::Empty;
            cx.notify();
            return;
        };
        self.reload_status_transitions(cx);

        let cancellation = ticket.cancellation().clone();
        let cached_detail = self
            .domain_issues
            .iter()
            .find(|issue| issue.id == issue_id)
            .filter(|issue| issue_has_cached_detail(issue))
            .map(detail_view_from_issue);
        let cached_image_catalog = cached_detail
            .as_ref()
            .map(collect_detail_images_with_context)
            .unwrap_or_default();
        if cached_image_catalog.is_empty() && !selection_changed {
            // A forced refresh without a cached image catalog cannot safely
            // preserve attachment IDs from the previous payload.
            self.selected_image_states.clear();
            self.selected_image_states.rebind_context(
                self.diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                load_token,
            );
        }
        self.detail_state = if let Some(detail) = cached_detail {
            DetailState::Refreshing {
                issue_id: issue_id.clone(),
                detail,
            }
        } else {
            DetailState::Loading {
                issue_id: issue_id.clone(),
            }
        };
        let read_request = DetailReadRequest::Selected {
            issue_id: issue_id.clone(),
        };
        let users = self.users.clone();
        let diagnostics = self.diagnostics.clone();
        if !cached_image_catalog.is_empty() {
            let cache_issue_id = issue_id.clone();
            let cache_workspace = workspace.clone();
            let cache_ticket = ticket.clone();
            let cache_cancellation = cancellation.clone();
            let cache_diagnostics = diagnostics.clone();
            let cache_task = cx.spawn(async move |this, cx| {
                let cached_states = fetch_cached_rich_image_states(
                    cache_workspace.clone(),
                    cache_workspace.site_id().clone(),
                    cache_issue_id,
                    cached_image_catalog,
                    cache_cancellation,
                    cache_diagnostics,
                    DiagnosticFlow::SelectedDetail,
                    load_token,
                )
                .await;
                let _ = this.update(cx, |this, cx| {
                    if !this.selected_detail_ticket_is_current(&cache_ticket) {
                        return;
                    }
                    this.detail_cache_task = None;
                    if let Ok(mut states) = cached_states {
                        states.merge_preserving_ready(&this.selected_image_states);
                        this.selected_image_states = states;
                        cx.notify();
                    }
                });
            });
            self.detail_cache_task = Some(cache_task);
        }
        let task = cx.spawn(async move |this, cx| {
            let result = read_detail(workspace.as_ref(), read_request.clone(), &cancellation).await;
            let detail = match result {
                Ok(detail) => detail,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if !this.selected_detail_ticket_is_current(&ticket) {
                            return;
                        }
                        this.detail_epoch.finish(&ticket);
                        this.detail_task = None;
                        if !matches!(this.detail_state, DetailState::Refreshing { .. }) {
                            this.selected_image_states.clear();
                        }
                        this.detail_state = DetailState::Error {
                            issue_id: issue_id.clone(),
                            copy: safe_detail_error(&error),
                        };
                        cx.notify();
                    });
                    return;
                }
            };
            let payload = prepare_detail_payload(
                &detail,
                &users,
                &diagnostics,
                DiagnosticFlow::SelectedDetail,
                load_token,
            );
            let image_contexts = payload.image_contexts.clone();
            let issue = payload.issue.clone();
            let image_issue_id = detail_image_issue_id(&read_request, &issue.id);
            let view = payload.view.clone();
            let loading = payload.loading.clone();
            let should_cache = this
                .update(cx, |this, cx| {
                    if !this.selected_detail_ticket_is_current(&ticket) {
                        return None;
                    }
                    let issue_changed = this
                        .domain_issues
                        .iter()
                        .find(|cached| cached.id == issue_id)
                        .is_none_or(|cached| cached != &issue);
                    if issue_changed {
                        if let Some(cached) = this
                            .domain_issues
                            .iter_mut()
                            .find(|cached| cached.id == issue_id)
                        {
                            *cached = issue.clone();
                        }
                        this.issues = issue_views_for_filter_with_offset(
                            &this.domain_issues,
                            &this.users,
                            this.status_filter,
                            &this.search_query,
                            this.timestamp_offset,
                        );
                    }
                    this.selected_issue_core = Some(issue.clone());
                    this.detail_state = DetailState::Loaded(view);
                    let mut loading = loading;
                    loading.merge_preserving_ready(&this.selected_image_states);
                    this.selected_image_states = loading;
                    cx.notify();
                    Some(issue_changed)
                })
                .unwrap_or(None);
            if should_cache == Some(true) {
                let _ = workspace.cache_detail_issue(&issue).await;
            }
            if should_cache.is_none() {
                for (candidate_ordinal, (surface_ordinal, source)) in
                    image_contexts.iter().copied().enumerate()
                {
                    diagnostics.image_state(
                        DiagnosticFlow::SelectedDetail,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Stale,
                    );
                }
                return;
            }
            let states = fetch_detail_images(
                workspace,
                image_issue_id,
                payload,
                cancellation,
                diagnostics.clone(),
                DiagnosticFlow::SelectedDetail,
                load_token,
            )
            .await;
            let applied = this
                .update(cx, |this, cx| {
                    if !this.selected_detail_ticket_is_current(&ticket) {
                        return false;
                    }
                    this.detail_epoch.finish(&ticket);
                    this.detail_task = None;
                    if let Ok(mut states) = states {
                        states.merge_preserving_ready(&this.selected_image_states);
                        this.selected_image_states = states;
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !applied {
                for (candidate_ordinal, (surface_ordinal, source)) in
                    image_contexts.iter().copied().enumerate()
                {
                    diagnostics.image_state(
                        DiagnosticFlow::SelectedDetail,
                        load_token,
                        candidate_ordinal,
                        surface_ordinal,
                        source,
                        ImageStateReason::Stale,
                    );
                }
            }
        });
        self.detail_task = Some(task);
        cx.notify();
    }

    fn open_update_issue(&mut self, issue_id: IssueId, mobile: bool, cx: &mut Context<Self>) {
        self.clear_remote_lookup();
        self.select_issue(issue_id, cx, false);
        self.section = Section::Issues;
        self.mobile_detail_open = mobile;
        cx.notify();
    }

    fn reload_selected_detail(&mut self, cx: &mut Context<Self>) {
        let Some(issue_id) = self.selected_issue.clone() else {
            return;
        };
        self.select_issue(issue_id, cx, true);
    }

    fn download_attachment(
        &mut self,
        attachment: crate::presentation::AttachmentViewModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(
            self.attachment_download_state,
            AttachmentDownloadState::Idle
        ) {
            return;
        }
        if attachment.size_bytes > MAX_ATTACHMENT_DOWNLOAD_BYTES as u64 {
            window.push_notification(
                Notification::error("Attachment is larger than the 64 MiB download limit")
                    .id::<AttachmentNotification>(),
                cx,
            );
            return;
        }
        let remote_issue = match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, .. } => Some(&issue.id),
            _ => None,
        };
        let Some(issue_id) = attachment_issue_id(self.selected_issue.as_ref(), remote_issue) else {
            return;
        };
        let Some(workspace) = self.workspace.clone() else {
            window.push_notification(
                Notification::error(
                    "Attachment download unavailable · live workspace is not ready",
                )
                .id::<AttachmentNotification>(),
                cx,
            );
            return;
        };

        let filename = sanitized_attachment_filename(&attachment.filename);
        let picker = cx.prompt_for_new_path(&portal_download_directory(), Some(&filename));
        let generation = self.attachment_download_generation.wrapping_add(1);
        self.attachment_download_generation = generation;
        let cancellation = CancellationToken::new();
        self.attachment_download_cancellation = Some(cancellation.clone());
        self.attachment_download_state = AttachmentDownloadState::Saving {
            attachment_id: attachment.id.clone(),
        };
        let request = AttachmentDownloadRequest {
            site_id: workspace.site_id().clone(),
            issue_id,
            attachment_id: attachment.id.clone(),
            max_bytes: MAX_ATTACHMENT_DOWNLOAD_BYTES,
        };
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let destination = picker
                    .await
                    .map_err(|_| "File picker unavailable".to_owned())?
                    .map_err(|_| "File picker unavailable".to_owned())?
                    .ok_or_else(|| "Download cancelled".to_owned())?;
                cancellation
                    .check()
                    .map_err(|_| "Download cancelled".to_owned())?;
                let content = workspace
                    .download_attachment(request, &cancellation)
                    .await
                    .map_err(|_| "Attachment download failed".to_owned())?;
                cancellation
                    .check()
                    .map_err(|_| "Download cancelled".to_owned())?;
                if content.bytes.len() > MAX_ATTACHMENT_DOWNLOAD_BYTES {
                    return Err("Attachment exceeded the 64 MiB download limit".to_owned());
                }
                let temporary = attachment_temp_path(&destination, &attachment_temp_token());
                let write_destination = temporary.clone();
                let write_cancellation = cancellation.clone();
                let write = cx.background_executor().spawn(async move {
                    write_attachment_temp(&write_destination, &content.bytes, &write_cancellation)
                });
                if let Err(error) = write.await {
                    cleanup_attachment_temp(&temporary);
                    return Err(error);
                }
                if cancellation.is_cancelled() {
                    cleanup_attachment_temp(&temporary);
                    return Err("Download cancelled".to_owned());
                }
                Ok::<(PathBuf, PathBuf), String>((destination, temporary))
            }
            .await;
            let temporary = result.as_ref().ok().map(|(_, temporary)| temporary.clone());
            let update_result = this.update_in(cx, |this, window, cx| {
                if !attachment_download_is_current(
                    this.attachment_download_generation,
                    generation,
                    &cancellation,
                ) {
                    if let Some(temporary) = temporary.as_deref() {
                        cleanup_attachment_temp(temporary);
                    }
                    return;
                }
                this.attachment_download_cancellation = None;
                this.attachment_download_task = None;
                this.attachment_download_state = AttachmentDownloadState::Idle;
                match result {
                    Ok((destination, temporary)) => match std::fs::rename(&temporary, &destination)
                    {
                        Ok(()) => window.push_notification(
                            Notification::success(format!(
                                "Attachment saved · {}",
                                destination.display()
                            ))
                            .id::<AttachmentNotification>(),
                            cx,
                        ),
                        Err(_) => {
                            cleanup_attachment_temp(&temporary);
                            window.push_notification(
                                Notification::error("Could not save the attachment")
                                    .id::<AttachmentNotification>(),
                                cx,
                            );
                        }
                    },
                    Err(message) if message == "Download cancelled" => {}
                    Err(message) => {
                        if let Some(temporary) = temporary.as_deref() {
                            cleanup_attachment_temp(temporary);
                        }
                        window.push_notification(
                            Notification::error(message).id::<AttachmentNotification>(),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
            if update_result.is_err()
                && let Some(temporary) = temporary.as_deref()
            {
                cleanup_attachment_temp(temporary);
            }
        });
        self.attachment_download_task = Some(task);
        cx.notify();
    }

    fn download_inline_attachment(
        &mut self,
        expected_issue_id: &IssueId,
        attachment_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (active_issue_id, attachments) = match &self.remote_lookup {
            RemoteLookupState::Loaded { issue, detail, .. } => {
                (issue.id.clone(), detail.attachments.as_slice())
            }
            _ => match (&self.selected_issue, &self.detail_state) {
                (Some(issue_id), DetailState::Loaded(detail)) => {
                    (issue_id.clone(), detail.attachments.as_slice())
                }
                _ => return,
            },
        };
        let Some(attachment) = inline_attachment_for_download(
            expected_issue_id,
            &active_issue_id,
            attachments,
            attachment_id,
        ) else {
            return;
        };
        self.download_attachment(attachment, window, cx);
    }

    fn invalidate_attachment_download(&mut self) {
        if let Some(cancellation) = self.attachment_download_cancellation.take() {
            cancellation.cancel();
        }
        self.attachment_download_task.take();
        self.attachment_download_generation = self.attachment_download_generation.wrapping_add(1);
        self.attachment_download_state = AttachmentDownloadState::Idle;
    }

    fn begin_comment_confirmation(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.comment_input.as_ref() else {
            return;
        };
        let Some(issue) = self.comment_target_issue() else {
            return;
        };
        let target = CommentTarget {
            issue_id: issue.id.clone(),
            issue_key: issue.key.as_str().to_owned(),
        };
        if let Err(comment_flow::CommentValidationError::UnknownOutcomeNeedsRefresh) = self
            .comment_flow
            .begin_confirmation(target, input.read(cx).value().as_ref())
        {
            self.sync_message =
                "Refresh comments before retrying a comment with an unknown outcome".to_owned();
        }
        cx.notify();
    }

    fn cancel_comment_confirmation(&mut self, cx: &mut Context<Self>) {
        self.comment_flow.cancel_confirmation();
        cx.notify();
    }

    fn post_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.comment_target_issue().map(|issue| CommentTarget {
            issue_id: issue.id.clone(),
            issue_key: issue.key.as_str().to_owned(),
        }) else {
            return;
        };
        if !self.comment_flow.has_confirmation_for(&target.issue_id) {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            let copy =
                comment_outcome_copy(crate::presentation::CommentOutcomeKind::WorkspaceUnavailable);
            self.comment_flow
                .fail_without_dispatch(target.issue_id, copy);
            window.push_notification(
                Notification::error(copy.message()).id::<CommentNotification>(),
                cx,
            );
            cx.notify();
            return;
        };
        let Some(submission) = self.comment_flow.consume_submission(&target.issue_id) else {
            return;
        };
        let cancellation = CancellationToken::new();
        self.comment_cancellation = Some(cancellation.clone());
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = workspace
                .create_comment(
                    IssueLocator::Id(submission.issue_id.clone()),
                    submission.body.clone(),
                    &cancellation,
                )
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                let remote_issue_id = match &this.remote_lookup {
                    RemoteLookupState::Loaded { issue, .. } => Some(&issue.id),
                    RemoteLookupState::Idle
                    | RemoteLookupState::Loading { .. }
                    | RemoteLookupState::Error { .. } => None,
                };
                let completion = this.comment_flow.finish_submission(
                    &submission,
                    remote_issue_id,
                    this.selected_issue.as_ref(),
                    result.as_ref().map(|_| ()),
                );
                if matches!(completion, CommentCompletion::Ignored) {
                    return;
                }
                this.comment_cancellation = None;
                this.comment_task = None;
                match completion {
                    CommentCompletion::Succeeded => {
                        this.comment_input = None;
                        this.comment_subscriptions.clear();
                        window.push_notification(
                            Notification::success("Comment posted to Jira.")
                                .id::<CommentNotification>(),
                            cx,
                        );
                        if matches!(
                            &this.remote_lookup,
                            RemoteLookupState::Loaded { issue, .. }
                                if issue.id == submission.issue_id
                        ) {
                            this.search_jira(cx);
                        } else {
                            this.reload_selected_detail(cx);
                        }
                    }
                    CommentCompletion::Failed { copy } => {
                        window.push_notification(
                            Notification::error(copy.message()).id::<CommentNotification>(),
                            cx,
                        );
                    }
                    CommentCompletion::Ignored => unreachable!(),
                }
                cx.notify();
            });
        });
        self.comment_task = Some(task);
        cx.notify();
    }

    fn refresh_comments(&mut self, cx: &mut Context<Self>) {
        if !self.comment_flow.can_refresh() {
            return;
        }
        if matches!(&self.remote_lookup, RemoteLookupState::Loaded { .. }) {
            self.search_jira(cx);
        } else {
            self.reload_selected_detail(cx);
        }
    }

    fn ensure_assignee_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assignee_input.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter assignees"));
        self.assignee_subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |_this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        ));
        self.assignee_input = Some(input);
    }

    fn begin_assignee_chooser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(issue) = selected_issue_from_sources(
            self.selected_issue.as_ref(),
            &self.domain_issues,
            self.selected_issue_core.as_ref(),
        )
        .cloned() else {
            return;
        };
        self.ensure_assignee_input(window, cx);
        if let Some(input) = &self.assignee_input {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.start_assignee_search_for_issue(issue.id, issue.key.to_string(), String::new(), cx);
    }

    fn start_assignee_search(&mut self, query: String, cx: &mut Context<Self>) {
        let (issue_id, issue_key) = match self.issue_edit_flow.state() {
            IssueEditState::AssigneeChooser {
                issue_id,
                issue_key,
                ..
            } => (issue_id.clone(), issue_key.clone()),
            IssueEditState::LoadingAssignees { issue_id, .. } => {
                let key = self
                    .selected_issue_view()
                    .filter(|issue| issue.id == *issue_id)
                    .map(|issue| issue.key)
                    .unwrap_or_default();
                (issue_id.clone(), key)
            }
            _ => return,
        };
        self.start_assignee_search_for_issue(issue_id, issue_key, query, cx);
    }

    fn start_assignee_search_for_issue(
        &mut self,
        issue_id: IssueId,
        issue_key: String,
        query: String,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        if let Some(cancellation) = self.issue_edit_cancellation.take() {
            cancellation.cancel();
        }
        self.issue_edit_task.take();
        let generation = self
            .issue_edit_flow
            .begin_assignee_loading(issue_id.clone(), query.clone());
        let cancellation = CancellationToken::new();
        self.issue_edit_cancellation = Some(cancellation.clone());
        let task = cx.spawn(async move |this, cx| {
            let result = workspace
                .search_assignable_users(
                    IssueLocator::Id(issue_id.clone()),
                    query.clone(),
                    100,
                    &cancellation,
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                if !this.issue_edit_flow.target_is_current(
                    this.selected_issue.as_ref(),
                    &issue_id,
                    generation,
                ) {
                    return;
                }
                this.issue_edit_cancellation = None;
                this.issue_edit_task = None;
                this.issue_edit_flow.finish_assignee_loading(
                    this.selected_issue.as_ref(),
                    issue_id,
                    issue_key,
                    query,
                    generation,
                    result,
                );
                cx.notify();
            });
        });
        self.issue_edit_task = Some(task);
        cx.notify();
    }

    fn choose_assignee(
        &mut self,
        account_id: Option<AccountId>,
        display_name: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(cancellation) = self.issue_edit_cancellation.take() {
            cancellation.cancel();
        }
        self.issue_edit_task.take();
        self.issue_edit_flow
            .choose_assignee(account_id, display_name);
        cx.notify();
    }

    fn cancel_issue_edit(&mut self, cx: &mut Context<Self>) {
        if let Some(cancellation) = self.issue_edit_cancellation.take() {
            cancellation.cancel();
        }
        self.issue_edit_task.take();
        self.issue_edit_flow.cancel();
        cx.notify();
    }

    fn submit_assignee(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let IssueEditState::ConfirmingAssignee { issue_id, .. } = self.issue_edit_flow.state()
        else {
            return;
        };
        self.submit_issue_write(window, cx, issue_id.clone(), IssueEditOperation::Assignee);
    }

    fn submit_transition(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let IssueEditState::ConfirmingTransition { issue_id, .. } = self.issue_edit_flow.state()
        else {
            return;
        };
        self.submit_issue_write(window, cx, issue_id.clone(), IssueEditOperation::Transition);
    }

    fn submit_issue_write(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        issue_id: IssueId,
        operation: IssueEditOperation,
    ) {
        if self.operation_in_progress {
            self.sync_message =
                "Finish the current Jira operation before applying another change".to_owned();
            cx.notify();
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.issue_edit_flow.unavailable(issue_id, operation);
            cx.notify();
            return;
        };
        let submission = match operation {
            IssueEditOperation::Assignee => self.issue_edit_flow.consume_assignee_submission(),
            IssueEditOperation::Transition => self.issue_edit_flow.consume_transition_submission(),
        };
        let Some(submission) = submission else { return };
        let target = submission.target();
        let identity = submission.identity().clone();
        self.operation_in_progress = true;
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = match submission {
                IssueEditSubmission::Assignee(AssigneeSubmission {
                    issue_id,
                    account_id,
                    ..
                }) => {
                    workspace
                        .assign_issue(
                            IssueLocator::Id(issue_id),
                            account_id,
                            &CancellationToken::new(),
                        )
                        .await
                }
                IssueEditSubmission::Transition(TransitionSubmission {
                    issue_id,
                    transition_id,
                    ..
                }) => {
                    workspace
                        .transition_issue(
                            IssueLocator::Id(issue_id),
                            transition_id,
                            &CancellationToken::new(),
                        )
                        .await
                }
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match this.issue_edit_flow.finish_write(
                    identity,
                    this.selected_issue.as_ref(),
                    result,
                ) {
                    IssueEditCompletion::Applied => {
                        this.operation_in_progress = false;
                        window.push_notification(
                            Notification::success(format!("Jira change applied · {target}"))
                                .id::<IssueEditNotification>(),
                            cx,
                        );
                        this.sync_message = "Change applied · reconciling with Jira…".to_owned();
                        this.begin_refresh(window, cx);
                    }
                    IssueEditCompletion::Failed { copy } => {
                        this.operation_in_progress = false;
                        window.push_notification(
                            Notification::error(copy.message()).id::<IssueEditNotification>(),
                            cx,
                        );
                    }
                    IssueEditCompletion::Ignored { busy } => {
                        if matches!(busy, BusyDirective::Release) {
                            this.operation_in_progress = false;
                        }
                    }
                }
                cx.notify();
            });
        });
        task.detach();
        cx.notify();
    }

    fn apply_cached(&mut self, cached: CachedWorkspace, cx: &mut Context<Self>) {
        let CachedWorkspace { issues, events } = cached;
        let update_groups = update_groups_for_events_with_offset(
            &events,
            &issues,
            &self.users,
            self.timestamp_offset,
        );
        self.apply_live_issues(issues, true, cx);
        self.update_groups = update_groups;
        if self.selected_issue.is_some() {
            self.reload_status_transitions(cx);
        }
    }

    fn apply_team_cached(&mut self, cached: CachedWorkspace, cx: &mut Context<Self>) {
        self.team_issues = cached.issues;
        self.team_events = cached.events;
        self.refresh_team_table(cx);
    }

    fn refresh_team_table(&mut self, cx: &mut Context<Self>) {
        let Some(table) = self.team_table.clone() else {
            return;
        };
        let issues = self.team_issues.clone();
        let events = self.team_events.clone();
        let users = self.users.clone();
        let now = self
            .team_clock
            .unwrap_or_else(jira_domain::Timestamp::now_utc);
        let offset = self.timestamp_offset;
        table.update(cx, |table, cx| {
            table.replace_team_ticket_rows_with_offset(&issues, &events, &users, now, offset, cx);
        });
    }

    fn ensure_team_table(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.team_table.is_some() {
            let table_mode = team_table_mode_for_width(f32::from(window.viewport_size().width));
            let dense_columns = !matches!(table_mode, TeamTableMode::WideTable);
            if let Some(table) = self.team_table.clone() {
                table.update(cx, |table, cx| {
                    table.set_team_ticket_density(dense_columns, cx);
                });
            }
            return;
        }
        let table_mode = team_table_mode_for_width(f32::from(window.viewport_size().width));
        let dense_columns = !matches!(table_mode, TeamTableMode::WideTable);
        let delegate = TeamTicketTableDelegate::new_with_density_and_offset(
            &self.team_issues,
            &self.team_events,
            &self.users,
            self.team_clock
                .unwrap_or_else(jira_domain::Timestamp::now_utc),
            dense_columns,
            self.timestamp_offset,
        );
        let table = cx.new(|cx| TableState::new(delegate, window, cx));
        self.team_table_subscriptions.push(cx.subscribe_in(
            &table,
            window,
            |this, table, event: &TableEvent, window, cx| match event {
                TableEvent::SelectRow(_) => {
                    let Some(issue_id) = table.read(cx).selected_team_ticket_issue_id() else {
                        return;
                    };
                    table.update(cx, |table, _| {
                        table
                            .delegate_mut()
                            .set_selected_issue_id(Some(issue_id.clone()));
                    });
                    if this.issue_edit_flow.is_submitting()
                        && this.selected_issue.as_ref() != Some(&issue_id)
                    {
                        return;
                    }
                    if this.selected_issue.as_ref() != Some(&issue_id) {
                        this.select_issue(issue_id, cx, true);
                    }
                    this.section = Section::Team;
                    this.mobile_detail_open =
                        layout_for_width(f32::from(window.viewport_size().width)).is_mobile();
                    cx.notify();
                }
                TableEvent::ClearSelection => {
                    table.update(cx, |table, _| {
                        table.delegate_mut().set_selected_issue_id(None);
                    });
                    let stale_selection = this.selected_issue.as_ref().is_some_and(|selected| {
                        !this.team_issues.iter().any(|issue| {
                            &issue.id == selected
                                && issue.status.category.as_deref().is_some_and(|category| {
                                    category.trim().eq_ignore_ascii_case("in progress")
                                })
                        })
                    });
                    if this.section == Section::Team && stale_selection {
                        this.clear_selection_for_team_scope(cx);
                    }
                }
                _ => {}
            },
        ));
        self.team_table = Some(table);
        // Fixture dashboards use a fixed clock and do not need a repeating task. Live dashboards
        // retain the age refresh so their elapsed values continue to track the current time.
        if self.team_clock.is_none() {
            let table = self.team_table.clone().expect("team table created");
            let team_clock = self.team_clock;
            let timestamp_offset = self.timestamp_offset;
            self.team_age_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(60))
                        .await;
                    let result = this.update(cx, |this, cx| {
                        if this.team_table.is_none() {
                            return false;
                        }
                        let issues = this.team_issues.clone();
                        let events = this.team_events.clone();
                        let users = this.users.clone();
                        table.update(cx, |table, cx| {
                            table.replace_team_ticket_rows_with_offset(
                                &issues,
                                &events,
                                &users,
                                team_clock.unwrap_or_else(jira_domain::Timestamp::now_utc),
                                timestamp_offset,
                                cx,
                            );
                        });
                        true
                    });
                    if !matches!(result, Ok(true)) {
                        break;
                    }
                }
            }));
        }
    }

    fn begin_team_refresh(&mut self, cx: &mut Context<Self>) {
        if self.team_task.is_some() || self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.team_feedback = TeamFeedback::Error {
                source: TeamFeedbackErrorSource::Connection,
                message: "Team tracker is unavailable until Jira is connected".to_owned(),
            };
            cx.notify();
            return;
        };
        self.operation_in_progress = true;
        self.team_feedback = TeamFeedback::Loading("Refreshing team tracker…".to_owned());
        let task = cx.spawn(async move |this, cx| {
            let result = workspace.refresh_team(&CancellationToken::new()).await;
            let _ = this.update(cx, |this, cx| {
                this.team_task = None;
                this.operation_in_progress = false;
                match result {
                    Ok(result) => {
                        this.apply_team_cached(result.cached, cx);
                        this.team_feedback = TeamFeedback::Info(team_refresh_feedback(
                            "Team tracker refreshed",
                            &this.team_issues,
                        ));
                    }
                    Err(error) => {
                        this.team_feedback = TeamFeedback::Error {
                            source: TeamFeedbackErrorSource::Refresh,
                            message: safe_sync_error(&error).to_owned(),
                        }
                    }
                }
                cx.notify();
            });
        });
        self.team_task = Some(task);
        cx.notify();
    }

    fn begin_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.operation_in_progress || self.issue_edit_flow.is_submitting() {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.sync_message = "Refresh unavailable · local workspace is not ready".to_owned();
            window.push_notification(
                Notification::error("Refresh unavailable · local workspace is not ready")
                    .id::<RefreshNotification>(),
                cx,
            );
            cx.notify();
            return;
        };
        let cancellation = CancellationToken::new();
        self.operation_in_progress = true;
        self.sync_message = "Refreshing Jira…".to_owned();
        if !self.team_members.is_empty() && !self.team_automatic_polling_paused {
            self.team_feedback = TeamFeedback::Loading("Refreshing team tracker…".to_owned());
        }
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let result = workspace.refresh(&cancellation).await;
            let team_result = workspace.refresh_team(&CancellationToken::new()).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(outcome) => {
                        window.push_notification(
                            Notification::success(refresh_notification_message(&outcome))
                                .id::<RefreshNotification>(),
                            cx,
                        );
                        this.sync_message = refresh_complete_message(&outcome);
                        this.apply_cached(outcome.cached, cx);
                        match team_result {
                            Ok(team) => {
                                this.apply_team_cached(team.cached, cx);
                                this.team_feedback = TeamFeedback::Info(team_refresh_feedback(
                                    "Team tracker refreshed",
                                    &this.team_issues,
                                ));
                            }
                            Err(error) => {
                                this.team_feedback = TeamFeedback::Error {
                                    source: TeamFeedbackErrorSource::Refresh,
                                    message: safe_sync_error(&error).to_owned(),
                                }
                            }
                        }
                        this.issue_edit_flow.refresh_succeeded();
                        this.start_automatic_polling(cx);
                    }
                    Err(error) => {
                        this.issue_edit_flow.refresh_failed();
                        let message = safe_sync_error(&error);
                        window.push_notification(
                            Notification::error(message.message()).id::<RefreshNotification>(),
                            cx,
                        );
                        this.sync_message = if this.issue_edit_flow.reconciliation_pending() {
                            "Change applied · refresh still needed".to_owned()
                        } else {
                            message.to_owned()
                        };
                        match team_result {
                            Ok(team) => {
                                this.apply_team_cached(team.cached, cx);
                                this.team_feedback = TeamFeedback::Info(team_refresh_feedback(
                                    "Team tracker refreshed",
                                    &this.team_issues,
                                ));
                            }
                            Err(error) => {
                                this.team_feedback = TeamFeedback::Error {
                                    source: TeamFeedbackErrorSource::Refresh,
                                    message: safe_sync_error(&error).to_owned(),
                                }
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            for group in &mut self.update_groups {
                group.unread = false;
                group.unread_count = 0;
                for event in &mut group.events {
                    event.unread = false;
                }
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
        self.update_groups
            .iter()
            .map(|group| group.unread_count)
            .sum()
    }

    fn set_update_filter(&mut self, filter: UpdateFilter, cx: &mut Context<Self>) {
        if self.update_filter != filter {
            self.update_filter = filter;
            cx.notify();
        }
    }

    fn toggle_update_group_expanded(&mut self, issue_id: IssueId, cx: &mut Context<Self>) {
        if !self.expanded_update_groups.remove(&issue_id) {
            self.expanded_update_groups.insert(issue_id);
        }
        cx.notify();
    }

    fn mark_group_read(&mut self, issue_id: IssueId, cx: &mut Context<Self>) {
        let Some(group) = self
            .update_groups
            .iter()
            .find(|group| group.issue_id == issue_id)
            .cloned()
        else {
            return;
        };
        if !group.unread {
            return;
        }
        let event_ids = update_group_event_ids(&group);
        let Some(workspace) = self.workspace.clone() else {
            if let Some(group) = self
                .update_groups
                .iter_mut()
                .find(|group| group.issue_id == issue_id)
            {
                group.unread = false;
                group.unread_count = 0;
                for event in &mut group.events {
                    event.unread = false;
                }
            }
            cx.notify();
            return;
        };
        if self.operation_in_progress {
            return;
        }
        self.operation_in_progress = true;
        self.sync_message = "Marking ticket updates read…".to_owned();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = workspace.mark_read(&event_ids, true).await;
            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    Ok(result) => {
                        this.apply_cached(result.cached, cx);
                        this.sync_message =
                            format!("Marked {} ticket updates read", result.changed);
                    }
                    Err(error) => this.sync_message = safe_sync_error(&error).to_owned(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn ensure_status_combobox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status_combobox.is_some() {
            self.ensure_status_list(window, cx);
            return;
        }

        let selected_indices = status_filter_indices(self.status_filter);
        let state = cx.new(|cx| {
            ComboboxState::new(status_options(), selected_indices, window, cx)
                .multiple(true)
                .searchable(false)
        });
        self.status_subscriptions.push(cx.subscribe_in(
            &state,
            window,
            |this, _, event: &ComboboxEvent<SearchableVec<StatusOption>>, window, cx| {
                let ComboboxEvent::Change(values) = event else {
                    return;
                };
                let next = IssueStatusSelection::from_values(values.iter().copied());
                let close_after_change =
                    should_close_status_filter_after_change(this.status_filter, next);
                this.set_status_filter(next, cx);
                if close_after_change {
                    window.dispatch_action(Box::new(Cancel), cx);
                }
            },
        ));
        self.status_combobox = Some(state);
        self.ensure_status_list(window, cx);
    }

    fn set_status_transition_items(&mut self, transitions: Vec<IssueTransition>) {
        self.status_transition_items = transitions;
        self.status_transition_items_revision =
            self.status_transition_items_revision.wrapping_add(1);
    }

    fn ensure_status_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.status_list.is_none() {
            let state = cx.new(|cx| {
                gpui_component::list::ListState::new(
                    StatusTransitionListDelegate {
                        transitions: self.status_transition_items.clone(),
                        selected: None,
                    },
                    window,
                    cx,
                )
                .searchable(false)
            });
            self.status_list_subscriptions.push(cx.subscribe_in(
                &state,
                window,
                |this, state, event: &gpui_component::list::ListEvent, window, cx| match event {
                    gpui_component::list::ListEvent::Confirm(ix) => {
                        let transition = state.read(cx).delegate().transitions.get(ix.row).cloned();
                        if let Some(transition) = transition {
                            this.choose_transition_from_list(transition, window, cx);
                        }
                    }
                    gpui_component::list::ListEvent::Cancel => {
                        this.status_popover_open = false;
                        cx.notify();
                    }
                    gpui_component::list::ListEvent::Select(_) => {}
                },
            ));
            self.status_list = Some(state);
            self.status_transition_items_applied_revision = self.status_transition_items_revision;
        } else if self.status_transition_items_applied_revision
            != self.status_transition_items_revision
        {
            if let Some(state) = self.status_list.clone() {
                let transitions = self.status_transition_items.clone();
                state.update(cx, |state, cx| {
                    state.delegate_mut().transitions = transitions;
                    state.delegate_mut().selected = None;
                    cx.notify();
                });
            }
            self.status_transition_items_applied_revision = self.status_transition_items_revision;
        }
    }

    fn invalidate_status_transition(&mut self) {
        self.status_popover_open = false;
        if let Some(cancellation) = self.status_transition_cancellation.take() {
            cancellation.cancel();
        }
        self.status_transition_task.take();
        self.status_transition_generation = self.status_transition_generation.wrapping_add(1);
        self.status_transition_state = StatusTransitionReadState::Idle;
        self.set_status_transition_items(Vec::new());
    }

    fn reload_status_transitions(&mut self, cx: &mut Context<Self>) {
        self.invalidate_status_transition();
        #[cfg(test)]
        if self.status_transition_reads_suppressed {
            return;
        }
        let Some(issue) = self.selected_issue_view() else {
            return;
        };
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let issue_id = issue.id.clone();
        let generation = self.status_transition_generation;
        self.status_transition_state = StatusTransitionReadState::Loading {
            issue_id: issue_id.clone(),
            generation,
        };
        let cancellation = CancellationToken::new();
        self.status_transition_cancellation = Some(cancellation.clone());
        let task = cx.spawn(async move |this, cx| {
            let result = workspace
                .available_transitions(IssueLocator::Id(issue_id.clone()), &cancellation)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.selected_issue.as_ref() != Some(&issue_id)
                    || !matches!(
                        this.status_transition_state,
                        StatusTransitionReadState::Loading {
                            issue_id: ref current,
                            generation: current_generation,
                        } if current == &issue_id && current_generation == generation
                    )
                {
                    return;
                }
                this.status_transition_cancellation = None;
                this.status_transition_task = None;
                match result {
                    Ok(transitions) => {
                        this.set_status_transition_items(transitions);
                        this.status_transition_state =
                            StatusTransitionReadState::Ready { issue_id };
                    }
                    Err(error) => {
                        this.status_transition_state = StatusTransitionReadState::Error {
                            issue_id,
                            copy: issue_edit_error_message(
                                error.kind(),
                                crate::presentation::IssueEditPhase::Lookup,
                            ),
                        };
                    }
                }
                cx.notify();
            });
        });
        self.status_transition_task = Some(task);
    }

    fn choose_transition_from_list(
        &mut self,
        transition: IssueTransition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(issue) = self
            .selected_issue_view()
            .filter(|issue| self.selected_issue.as_ref() == Some(&issue.id))
        else {
            return;
        };
        if !matches!(
            self.status_transition_state,
            StatusTransitionReadState::Ready { ref issue_id } if issue_id == &issue.id
        ) {
            return;
        }
        self.issue_edit_flow.begin_transition_confirmation(
            issue.id.clone(),
            issue.key.clone(),
            transition,
        );
        if let Some(state) = self.status_list.clone() {
            state.update(cx, |state, cx| {
                state.set_selected_index(None, window, cx);
                cx.notify();
            });
        }
        self.status_popover_open = false;
        cx.notify();
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
                    this.comment_flow.clear_error_on_edit();
                }
            },
        ));
        self.comment_input = Some(input);
    }

    fn ensure_settings_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(7)
                .placeholder("project = YOUR_PROJECT")
        });
        input.update(cx, |input, cx| {
            input.set_value(&self.settings_scope_text, window, cx)
        });
        self.settings_subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.settings_scope_text = input.read(cx).value().to_string();
                    this.settings_feedback = None;
                    cx.notify();
                }
            },
        ));
        self.settings_input = Some(input);
    }

    fn ensure_team_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.team_input.is_some() {
            return;
        }
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(5)
                .placeholder("account-id-1\nuser@example.com")
        });
        input.update(cx, |input, cx| input.set_value(&self.team_text, window, cx));
        self.settings_subscriptions.push(cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.team_text = input.read(cx).value().to_string();
                    this.team_feedback = TeamFeedback::Idle;
                    cx.notify();
                }
            },
        ));
        self.team_input = Some(input);
    }

    fn reset_settings_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_scope_text = DEFAULT_JQL_SCOPE.to_owned();
        self.settings_feedback = None;
        if let Some(input) = self.settings_input.clone() {
            input.update(cx, |input, cx| {
                input.set_value(DEFAULT_JQL_SCOPE, window, cx)
            });
        }
        cx.notify();
    }

    fn begin_save_settings(&mut self, cx: &mut Context<Self>) {
        if self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.settings_feedback = Some("Connect Jira before applying a scope".to_owned());
            cx.notify();
            return;
        };
        let entered = self.settings_scope_text.clone();
        let previous_scope = workspace.jql_scope();
        let team_members = self.team_members.clone();
        self.operation_in_progress = true;
        self.settings_feedback = Some("Applying scope and refreshing Jira…".to_owned());
        cx.notify();

        let task = cx.spawn(async move |this, cx| {
            let preferences = settings::ProductionPreferences;
            let result = settings::run_scope_transaction(
                workspace.as_ref(),
                &preferences,
                entered,
                previous_scope,
                team_members,
            )
            .await;

            let _ = this.update(cx, |this, cx| {
                this.operation_in_progress = false;
                match result {
                    settings::ScopeSaveResult::Saved {
                        refreshed,
                        normalized,
                    } => {
                        this.settings_scope_text = normalized
                            .clone()
                            .unwrap_or_else(|| DEFAULT_JQL_SCOPE.to_owned());
                        this.settings_warning = None;
                        this.settings_feedback = Some("Scope saved and Jira refreshed".to_owned());
                        this.sync_message = refresh_complete_message(&refreshed);
                        this.apply_cached(refreshed.cached, cx);
                        this.start_automatic_polling(cx);
                    }
                    settings::ScopeSaveResult::Failed(failure) => {
                        let copy = scope_outcome_copy(settings::scope_failure_kind(&failure));
                        if copy.recovery() == RecoveryDirective::InvalidateWorkspace {
                            // Never leave the old cache paired with an unknown active scope.
                            this.workspace = None;
                            this.polling_task.take();
                            this.automatic_polling_paused = true;
                            this.domain_issues.clear();
                            this.issues.clear();
                            this.update_groups.clear();
                            this.selected_issue = None;
                        }
                        this.settings_feedback = Some(copy.to_owned());
                    }
                }
                cx.notify();
            });
        });
        self.settings_task = Some(task);
    }

    fn begin_save_team(&mut self, cx: &mut Context<Self>) {
        if self.team_task.is_some() || self.operation_in_progress {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            self.team_feedback = TeamFeedback::Error {
                source: TeamFeedbackErrorSource::Connection,
                message: "Connect Jira before saving the team tracker".to_owned(),
            };
            cx.notify();
            return;
        };
        let entered = self.team_text.clone();
        let previous_members = self.team_members.clone();
        let previous_text = self.team_text.clone();
        let previous_accounts = workspace.team_members();
        let issue_jql_scope = workspace.jql_scope();
        self.operation_in_progress = true;
        self.team_feedback =
            TeamFeedback::Loading("Resolving team members and refreshing Jira…".to_owned());
        let task = cx.spawn(async move |this, cx| {
            let preferences = settings::ProductionPreferences;
            let result = settings::run_team_transaction(
                workspace.as_ref(),
                &preferences,
                entered,
                previous_accounts,
                issue_jql_scope,
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                this.team_task = None;
                this.operation_in_progress = false;
                match result {
                    settings::TeamSaveResult::Saved { members, refreshed } => {
                        this.team_automatic_polling_paused = false;
                        this.team_members = members;
                        this.team_text = this
                            .team_members
                            .iter()
                            .map(|member| member.identifier.clone())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let authenticated_account = this.authenticated_account.clone();
                        this.users.retain(|user| {
                            authenticated_account.as_ref() == Some(&user.account_id)
                        });
                        this.users
                            .extend(this.team_members.iter().filter_map(|member| {
                                if !persisted_team_member_has_display_name(member) {
                                    return None;
                                }
                                AccountId::new(member.account_id.clone())
                                    .ok()
                                    .map(|account_id| {
                                        User::new(
                                            workspace.site_id().clone(),
                                            account_id,
                                            member.display_name.clone(),
                                            None,
                                            true,
                                        )
                                    })
                            }));
                        this.apply_team_cached(refreshed.cached, cx);
                        this.team_feedback = TeamFeedback::Info(team_refresh_feedback(
                            "Team tracker saved and refreshed",
                            &this.team_issues,
                        ));
                    }
                    settings::TeamSaveResult::Failed(failure) => {
                        this.team_members = previous_members;
                        this.team_text = previous_text;
                        let copy = team_outcome_copy(settings::team_failure_kind(&failure));
                        if copy.recovery() == RecoveryDirective::PauseTeam {
                            this.team_automatic_polling_paused = true;
                            this.team_members.clear();
                            this.team_issues.clear();
                            this.team_events.clear();
                            this.refresh_team_table(cx);
                        }
                        this.team_feedback = TeamFeedback::Error {
                            source: TeamFeedbackErrorSource::Save,
                            message: copy.to_owned(),
                        };
                    }
                }
                cx.notify();
            });
        });
        self.team_task = Some(task);
        cx.notify();
    }

    fn begin_forget_saved_login(&mut self, cx: &mut Context<Self>) {
        if self.saved_login_delete_task.is_some()
            || !can_start_saved_login_delete(self.saved_login_delete_state)
        {
            return;
        }
        self.saved_login_delete_state = SavedLoginDeleteState::Deleting;
        let task = cx.spawn(async move |this, cx| {
            let outcome = match credential_store::delete_saved_credentials().await {
                Ok(DeleteOutcome::Deleted) => SavedLoginDeleteOutcome::Deleted,
                Ok(DeleteOutcome::Absent) => SavedLoginDeleteOutcome::Absent,
                Err(_) => SavedLoginDeleteOutcome::Error,
            };
            let _ = this.update(cx, |this, cx| {
                this.saved_login_delete_task = None;
                this.saved_login_delete_state = SavedLoginDeleteState::Completed(outcome);
                cx.notify();
            });
        });
        self.saved_login_delete_task = Some(task);
        cx.notify();
    }

    fn begin_test_desktop_notification(&mut self, cx: &mut Context<Self>) {
        if !self.can_start_desktop_notification_test() {
            return;
        }
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        self.desktop_notification_test_state = DesktopNotificationTestState::Sending;
        self.diagnostics.desktop_notification_test_started();
        let diagnostics = self.diagnostics.clone();
        let task = cx.spawn(async move |this, cx| {
            let outcome = match workspace.test_desktop_notification().await {
                Ok(receipt) => {
                    let result = DiagnosticDesktopNotificationTestResult::Accepted {
                        notification_id: receipt.notification_id(),
                    };
                    diagnostics.desktop_notification_test_result(result);
                    DesktopNotificationTestOutcome::Accepted {
                        notification_id: receipt.notification_id(),
                    }
                }
                Err(error) => {
                    let result = DiagnosticDesktopNotificationTestResult::Failed(
                        DiagnosticErrorKind::from(error.kind()),
                    );
                    diagnostics.desktop_notification_test_result(result);
                    DesktopNotificationTestOutcome::Failed(DiagnosticErrorKind::from(error.kind()))
                }
            };
            let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Secs, true);
            let _ = this.update(cx, |this, cx| {
                this.desktop_notification_test_task = None;
                this.desktop_notification_test_state =
                    DesktopNotificationTestState::Completed(DesktopNotificationTestReport {
                        timestamp,
                        outcome,
                    });
                cx.notify();
            });
        });
        self.desktop_notification_test_task = Some(task);
        cx.notify();
    }

    fn can_start_desktop_notification_test(&self) -> bool {
        self.workspace.is_some()
            && matches!(
                self.desktop_notification_test_state,
                DesktopNotificationTestState::Idle | DesktopNotificationTestState::Completed(_)
            )
    }
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod tests;
