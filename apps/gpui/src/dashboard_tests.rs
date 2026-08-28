use super::settings::{persisted_direct_team_member, team_identifier_lines};
use super::*;
use crate::app_shell::AppearancePreference;
use crate::presentation::{UpdateViewModel, normalized_issue_key};
use crate::responsive::sidebar_width_for_viewport;
use crate::sample_data::{sample_issues, sample_users};
use gpui::VisualTestContext;
use gpui_component::searchable_list::SearchableListDelegate as _;
use jira_application::{
    AddCommentRequest, AssignIssueRequest, AssignableUserSearchRequest, ErrorKind,
    IssueFetchRequest, IssuePage, IssueTransitionsRequest, JiraAttachmentReadPort,
    JiraCommentWritePort, JiraIssueActivityPort, JiraIssueDetailReadPort, JiraIssueEditPort,
    JiraIssueSearchPort, JiraReadPort, JiraSyncReadPort, JiraUserReadPort, PortFuture,
    TransitionIssueRequest, UserSearchRequest,
};
use jira_domain::JiraSiteId;
use std::sync::{Arc, Mutex};
use time::macros::datetime;

struct EmptyJira;

impl JiraUserReadPort for EmptyJira {
    fn fetch_current_user<'a>(
        &'a self,
        _site_id: &'a JiraSiteId,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, User> {
        Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
    }

    fn search_users<'a>(
        &'a self,
        _request: &'a UserSearchRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<User>> {
        Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
    }
}

impl JiraIssueSearchPort for EmptyJira {
    fn fetch_issue_page<'a>(
        &'a self,
        _request: &'a IssueFetchRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, IssuePage> {
        Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
    }

    fn fetch_issues_by_id<'a>(
        &'a self,
        _site_id: &'a JiraSiteId,
        _issue_ids: &'a [IssueId],
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<Issue>> {
        Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
    }
}

impl JiraIssueActivityPort for EmptyJira {}
impl JiraIssueDetailReadPort for EmptyJira {}
impl JiraAttachmentReadPort for EmptyJira {}
impl JiraSyncReadPort for EmptyJira {}
impl JiraReadPort for EmptyJira {}

struct EmptyCommentWriter;

impl JiraCommentWritePort for EmptyCommentWriter {
    fn create_comment<'a>(
        &'a self,
        _request: &'a AddCommentRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, jira_domain::IssueComment> {
        Box::pin(async { Err(ApplicationError::new(ErrorKind::Internal, "unsupported")) })
    }
}

#[derive(Default)]
struct EditCalls {
    searches: Vec<AssignableUserSearchRequest>,
    transition_reads: Vec<IssueTransitionsRequest>,
    assignments: Vec<AssignIssueRequest>,
    transition_writes: Vec<TransitionIssueRequest>,
}

#[derive(Clone)]
struct RecordingIssueEditor {
    calls: Arc<Mutex<EditCalls>>,
    users: Vec<User>,
    transitions: Vec<IssueTransition>,
}

impl JiraIssueEditPort for RecordingIssueEditor {
    fn search_assignable_users<'a>(
        &'a self,
        request: &'a AssignableUserSearchRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<User>> {
        let calls = self.calls.clone();
        let request = request.clone();
        let users = self.users.clone();
        Box::pin(async move {
            calls.lock().expect("calls lock").searches.push(request);
            Ok(users)
        })
    }

    fn fetch_issue_transitions<'a>(
        &'a self,
        request: &'a IssueTransitionsRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, Vec<IssueTransition>> {
        let calls = self.calls.clone();
        let request = request.clone();
        let transitions = self.transitions.clone();
        Box::pin(async move {
            calls
                .lock()
                .expect("calls lock")
                .transition_reads
                .push(request);
            Ok(transitions)
        })
    }

    fn assign_issue<'a>(
        &'a self,
        request: &'a AssignIssueRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, ()> {
        let calls = self.calls.clone();
        let request = request.clone();
        Box::pin(async move {
            calls.lock().expect("calls lock").assignments.push(request);
            std::future::pending::<Result<(), ApplicationError>>().await
        })
    }

    fn transition_issue<'a>(
        &'a self,
        request: &'a TransitionIssueRequest,
        _cancellation: &'a CancellationToken,
    ) -> PortFuture<'a, ()> {
        let calls = self.calls.clone();
        let request = request.clone();
        Box::pin(async move {
            calls
                .lock()
                .expect("calls lock")
                .transition_writes
                .push(request);
            std::future::pending::<Result<(), ApplicationError>>().await
        })
    }
}

fn update_view(event_id: &str, change: &str, occurred_at: &str) -> UpdateViewModel {
    UpdateViewModel {
        event_id: jira_domain::EventId::new(event_id).expect("event"),
        issue_id: IssueId::new("100").expect("issue"),
        issue_key: "IX-100".to_owned(),
        issue_summary: "Summary".to_owned(),
        change: change.to_owned(),
        occurred_at: occurred_at.to_owned(),
        unread: false,
    }
}

#[test]
fn update_filter_returns_only_unread_ticket_groups() {
    let mut groups = vec![
        UpdateGroupViewModel {
            issue_id: IssueId::new("100").expect("issue"),
            issue_key: "IX-100".to_owned(),
            issue_summary: "Unread".to_owned(),
            events: Vec::new(),
            latest_occurred_at: "now".to_owned(),
            unread_count: 1,
            unread: true,
        },
        UpdateGroupViewModel {
            issue_id: IssueId::new("200").expect("issue"),
            issue_key: "IX-200".to_owned(),
            issue_summary: "Read".to_owned(),
            events: Vec::new(),
            latest_occurred_at: "earlier".to_owned(),
            unread_count: 0,
            unread: false,
        },
    ];

    assert_eq!(
        filtered_update_group_indices(&groups, UpdateFilter::All),
        vec![0, 1]
    );
    assert_eq!(
        filtered_update_group_indices(&groups, UpdateFilter::Unread),
        vec![0]
    );

    groups[0].unread = false;
    assert!(filtered_update_group_indices(&groups, UpdateFilter::Unread).is_empty());
}

#[test]
fn status_control_editability_requires_live_selected_issue() {
    let idle = IssueEditState::Idle;
    assert!(status_control_is_editable(true, true, false, false, &idle));
    assert!(!status_control_is_editable(
        true, false, false, false, &idle
    ));
    assert!(!status_control_is_editable(
        false, true, false, false, &idle
    ));
    assert!(!status_control_is_editable(true, true, true, false, &idle));
    assert!(!status_control_is_editable(true, true, false, true, &idle));
}

#[test]
fn status_control_allows_only_status_lookup_states() {
    let issue_id = IssueId::new("100").expect("issue");
    let loading = IssueEditState::LoadingTransitions {
        issue_id: issue_id.clone(),
    };
    let confirming = IssueEditState::ConfirmingAssignee {
        issue_id,
        issue_key: "IX-100".to_owned(),
        account_id: None,
        display_name: "Unassigned".to_owned(),
    };
    assert!(status_control_is_editable(
        true, true, false, false, &loading
    ));
    assert!(!status_control_is_editable(
        true,
        true,
        false,
        false,
        &confirming
    ));
}

#[test]
fn transition_option_label_uses_destination_status_only() {
    let transition = IssueTransition {
        id: "31".to_owned(),
        name: "Move issue".to_owned(),
        to: jira_domain::Status {
            id: "3".to_owned(),
            name: "In Progress".to_owned(),
            category: None,
        },
    };

    assert_eq!(transition_option_label(&transition), "In Progress");
}

#[test]
fn transition_list_height_is_compact_and_bounded() {
    assert_eq!(status_transition_list_height(1), px(32.));
    assert_eq!(status_transition_list_height(2), px(68.));
    assert_eq!(
        status_transition_list_height(12),
        px(STATUS_TRANSITION_LIST_MAX_HEIGHT)
    );
}

#[test]
fn live_project_label_is_inferred_from_unique_projects() {
    assert_eq!(project_label(&[]), "Jira projects");
    let mut issues = sample_issues();
    issues.truncate(1);
    assert_eq!(project_label(&issues), issues[0].project.name);
    let mut second = issues[0].clone();
    second.project.name = "Another project".to_owned();
    issues.push(second);
    assert_eq!(project_label(&issues), "2 Jira projects");
}

#[test]
fn compacts_generic_activity_rows_but_keeps_specific_changes() {
    let rows = compact_update_rows(&[
        update_view("event-1", "Issue activity changed", "latest"),
        update_view("event-2", "Status: To do → Done", "middle"),
        update_view("event-3", "Issue activity changed", "earlier"),
    ]);

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        CompactedUpdateRow::GenericSummary {
            count: 2,
            occurred_at: "latest".to_owned(),
        }
    );
    assert!(matches!(
        &rows[1],
        CompactedUpdateRow::Event(UpdateViewModel { change, .. })
            if change == "Status: To do → Done"
    ));
}

#[test]
fn generic_summary_copy_is_honest_and_counted() {
    assert_eq!(
        generic_summary_label(1),
        "Other Jira activity · exact field not available from sync"
    );
    assert_eq!(
        generic_summary_label(3),
        "Other Jira activity · 3 events · exact field not available from sync"
    );
}

#[test]
fn update_preview_limits_rows_without_discarding_audit_data() {
    assert_eq!(visible_update_row_count(5, false), 3);
    assert_eq!(hidden_update_row_count(5, false), 2);
    assert_eq!(visible_update_row_count(5, true), 5);
    assert_eq!(hidden_update_row_count(5, true), 0);
    assert_eq!(visible_update_row_count(2, false), 2);
}

#[test]
fn appearance_defaults_to_system_and_fixture_initialization_is_side_effect_free() {
    let mut dashboard = Dashboard::from_sample_data();

    assert_eq!(
        dashboard.appearance_preference,
        AppearancePreference::System
    );
    dashboard.initialize_appearance_preference(AppearancePreference::Dark);
    assert_eq!(dashboard.appearance_preference, AppearancePreference::Dark);
    assert!(matches!(
        DashboardEvent::AppearanceChanged(AppearancePreference::Light),
        DashboardEvent::AppearanceChanged(AppearancePreference::Light)
    ));
}

#[gpui::test]
fn selecting_appearance_updates_state_and_emits_event(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.open_window(gpui::size(px(640.), px(480.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let subscription = cx.update(|cx| {
        cx.subscribe(&dashboard_entity, move |_, event: &DashboardEvent, _| {
            observed.lock().expect("appearance events").push(*event);
        })
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.select_appearance_preference(AppearancePreference::Light, window, cx);
        });
    });
    visual.run_until_parked();

    assert_eq!(
        dashboard_entity.read_with(&visual, |dashboard, _| dashboard.appearance_preference),
        AppearancePreference::Light
    );
    assert_eq!(
        *events.lock().expect("appearance events"),
        vec![DashboardEvent::AppearanceChanged(
            AppearancePreference::Light
        )]
    );
    drop(subscription);
}

#[test]
fn sidebar_state_starts_expanded_and_refresh_stays_hidden_on_settings() {
    let dashboard = Dashboard::from_sample_data();

    assert!(!dashboard.sidebar_collapsed);
    assert!(refresh_visible_for_section(Section::Issues));
    assert!(refresh_visible_for_section(Section::Updates));
    assert!(refresh_visible_for_section(Section::Team));
    assert!(!refresh_visible_for_section(Section::Settings));
}

#[gpui::test]
fn sidebar_toggle_is_manual_only_on_standard_and_wide_layouts(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.open_window(gpui::size(px(1_000.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();

    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.toggle_sidebar(LayoutMode::Compact, cx);
            assert!(!dashboard.sidebar_collapsed);
            dashboard.toggle_sidebar(LayoutMode::Standard, cx);
        });
    });
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| dashboard.sidebar_collapsed));

    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.toggle_sidebar(LayoutMode::Wide, cx);
        });
    });
    assert!(!dashboard_entity.read_with(&visual, |dashboard, _| dashboard.sidebar_collapsed));
}

#[gpui::test]
fn sidebar_bounds_switch_between_expanded_and_collapsed_widths(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.open_window(gpui::size(px(1_100.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let expanded = visual
        .debug_bounds("dashboard-sidebar")
        .expect("expanded sidebar should be laid out");
    let expanded_main = visual
        .debug_bounds("dashboard-main")
        .expect("expanded main pane should be laid out");

    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.toggle_sidebar(LayoutMode::Standard, cx);
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let collapsed = visual
        .debug_bounds("dashboard-sidebar")
        .expect("collapsed sidebar should be laid out");
    let collapsed_main = visual
        .debug_bounds("dashboard-main")
        .expect("collapsed main pane should be laid out");

    assert_eq!(expanded.size.width, px(200.));
    assert_eq!(collapsed.size.width, px(64.));
    assert_eq!(
        expanded_main.origin.x,
        expanded.origin.x + expanded.size.width
    );
    assert!(collapsed_main.size.width > expanded_main.size.width);
    assert_eq!(
        collapsed_main.origin.x,
        collapsed.origin.x + collapsed.size.width
    );
}

#[gpui::test]
fn mobile_navigation_fits_all_destinations_at_supported_minimum(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let window = cx.open_window(gpui::size(px(320.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let navigation = visual
        .debug_bounds("mobile-navigation")
        .expect("mobile navigation should be laid out");
    for id in [
        "mobile-issues",
        "mobile-updates",
        "mobile-team",
        "mobile-settings",
    ] {
        let bounds = visual
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} should be laid out"));
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        assert!(bounds.origin.x >= navigation.origin.x);
        assert!(
            bounds.origin.x + bounds.size.width
                <= navigation.origin.x + navigation.size.width + px(1.)
        );
    }
}

#[test]
fn update_filter_and_expansion_state_start_in_safe_defaults() {
    let dashboard = Dashboard::from_sample_data();

    assert_eq!(dashboard.update_filter, UpdateFilter::All);
    assert!(dashboard.expanded_update_groups.is_empty());
}

#[test]
fn settings_editor_starts_with_the_default_scope() {
    let dashboard = Dashboard::from_sample_data();

    assert_eq!(dashboard.settings_scope_text, DEFAULT_JQL_SCOPE);
}

#[test]
fn saved_login_delete_feedback_is_safe_and_exact_for_every_outcome() {
    let outcomes = [
        (
            SavedLoginDeleteOutcome::Deleted,
            "Saved Jira login forgotten. This session remains connected.",
            FeedbackSeverity::Info,
            FeedbackCertainty::Definite,
            RecoveryDirective::None,
        ),
        (
            SavedLoginDeleteOutcome::Absent,
            "No saved Jira login was present. This session remains connected.",
            FeedbackSeverity::Info,
            FeedbackCertainty::Definite,
            RecoveryDirective::None,
        ),
        (
            SavedLoginDeleteOutcome::Error,
            "Saved Jira login could not be removed from the system keyring.",
            FeedbackSeverity::Error,
            FeedbackCertainty::Definite,
            RecoveryDirective::None,
        ),
    ];
    for (outcome, message, severity, certainty, recovery) in outcomes {
        let copy = saved_login_delete_feedback(outcome);
        assert_eq!(copy.message(), message);
        assert_eq!(copy.severity(), severity);
        assert_eq!(copy.certainty(), certainty);
        assert_eq!(copy.recovery(), recovery);
    }
}

#[test]
fn saved_login_delete_gates_repeated_clicks_only_while_deleting() {
    assert!(can_start_saved_login_delete(SavedLoginDeleteState::Idle));
    assert!(!can_start_saved_login_delete(
        SavedLoginDeleteState::Deleting
    ));
    assert!(can_start_saved_login_delete(
        SavedLoginDeleteState::Completed(SavedLoginDeleteOutcome::Deleted,)
    ));
}

#[test]
fn desktop_notification_test_is_unavailable_without_live_workspace() {
    let dashboard = Dashboard::from_sample_data();

    assert!(!dashboard.can_start_desktop_notification_test());
    assert_eq!(
        dashboard.desktop_notification_test_state,
        DesktopNotificationTestState::Idle
    );
}

#[test]
fn desktop_notification_error_copy_uses_stable_category() {
    assert_eq!(
        desktop_notification_error_category(DiagnosticErrorKind::Notification),
        "notification"
    );
    assert_eq!(
        desktop_notification_error_category(DiagnosticErrorKind::UnknownOutcome),
        "unknown_outcome"
    );
}

fn refresh_result_with_inserted_events(events_inserted: usize) -> RefreshResult {
    RefreshResult {
        cached: CachedWorkspace {
            issues: sample_issues(),
            events: Vec::new(),
        },
        outcome: jira_application::SyncOutcome {
            mode: SyncMode::Incremental,
            pages_fetched: 1,
            issues_fetched: 5,
            events_inserted,
            notifications_delivered: events_inserted,
            notification_failures: 0,
            cursor: datetime!(2026-08-18 00:00 UTC),
        },
    }
}

#[test]
fn refresh_notification_reports_new_update_count() {
    let result = refresh_result_with_inserted_events(2);

    assert_eq!(
        refresh_notification_message(&result),
        "Refresh complete · 5 issues · 2 new local updates"
    );
}

#[test]
fn refresh_notification_distinguishes_zero_new_updates() {
    let result = refresh_result_with_inserted_events(0);

    assert_eq!(
        refresh_notification_message(&result),
        "Refresh complete · 5 issues · no new local updates"
    );
}

#[test]
fn refresh_status_uses_submission_wording_for_desktop_notifications() {
    let result = refresh_result_with_inserted_events(1);

    let message = refresh_complete_message(&result);
    assert!(message.contains("accepted by desktop service"));
    assert!(!message.contains("delivered"));
}

#[test]
fn status_filter_trigger_summary_is_deterministic() {
    assert_eq!(
        status_filter_trigger_label(IssueStatusSelection::All),
        "All statuses"
    );
    assert_eq!(
        status_filter_trigger_label(IssueStatusSelection::Done),
        "Done"
    );
    assert_eq!(
        status_filter_trigger_label(IssueStatusSelection::from_values([
            IssueStatusSelection::Done,
            IssueStatusSelection::ToDo,
        ])),
        "2 statuses"
    );
}

#[test]
fn status_options_keep_combobox_values_and_labels_aligned() {
    let options = status_options();
    let values = [
        IssueStatusSelection::ToDo,
        IssueStatusSelection::InProgress,
        IssueStatusSelection::Done,
        IssueStatusSelection::Uncategorized,
    ];
    for (index, expected) in values.into_iter().enumerate() {
        let item = options
            .item(gpui_component::IndexPath::new(index))
            .expect("status option");
        assert_eq!(*item.value(), expected);
        assert_eq!(item.title(), expected.label());
    }
}

#[test]
fn status_filter_initial_indices_follow_presentation_order() {
    assert_eq!(status_filter_indices(IssueStatusSelection::All), Vec::new());
    assert_eq!(
        status_filter_indices(IssueStatusSelection::from_values([
            IssueStatusSelection::Done,
            IssueStatusSelection::ToDo,
        ])),
        vec![
            gpui_component::IndexPath::new(0),
            gpui_component::IndexPath::new(2),
        ]
    );
}

#[test]
fn status_filter_closes_only_for_first_single_selection() {
    assert!(should_close_status_filter_after_change(
        IssueStatusSelection::All,
        IssueStatusSelection::ToDo,
    ));
    assert!(!should_close_status_filter_after_change(
        IssueStatusSelection::All,
        IssueStatusSelection::from_values(
            [IssueStatusSelection::ToDo, IssueStatusSelection::Done,]
        ),
    ));
    assert!(!should_close_status_filter_after_change(
        IssueStatusSelection::ToDo,
        IssueStatusSelection::from_values(
            [IssueStatusSelection::ToDo, IssueStatusSelection::Done,]
        ),
    ));
}

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

#[gpui::test]
fn selected_without_workspace_finishes_epoch_and_drops_task(cx: &mut gpui::TestAppContext) {
    let dashboard = cx.new(|_| Dashboard::from_sample_data());
    let issue_id = sample_issues().into_iter().next().expect("issue").id;
    let before_generation = cx.read_entity(&dashboard, |dashboard, _| {
        dashboard.detail_epoch.generation()
    });

    cx.update_entity(&dashboard, |dashboard, cx| {
        dashboard.select_issue(issue_id, cx, true);
    });

    let (generation, idle, task_is_none, state) = cx.read_entity(&dashboard, |dashboard, _| {
        (
            dashboard.detail_epoch.generation(),
            dashboard.detail_epoch.is_idle(),
            dashboard.detail_task.is_none(),
            dashboard.detail_state.clone(),
        )
    });
    assert_eq!(generation, before_generation + 1);
    assert!(
        idle,
        "workspace-unavailable selection must not retain a ticket"
    );
    assert!(task_is_none);
    assert_eq!(state, DetailState::Empty);
}

#[test]
fn selected_supersession_and_invalidation_cancel_exactly_the_prior_ticket() {
    let mut dashboard = Dashboard::from_sample_data();
    let first = dashboard.detail_epoch.begin(
        RequestSource::SelectedDetail,
        IssueId::new("10001").expect("issue"),
    );
    let second = dashboard.detail_epoch.begin(
        RequestSource::SelectedDetail,
        IssueId::new("10002").expect("issue"),
    );

    assert!(first.cancellation().is_cancelled());
    assert!(dashboard.detail_epoch.is_current(&second));
    dashboard.invalidate_detail_selection();
    assert!(second.cancellation().is_cancelled());
    assert!(dashboard.detail_epoch.is_idle());
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

#[gpui::test]
fn open_update_selects_issue_and_opens_mobile_detail(cx: &mut gpui::TestAppContext) {
    let issue = sample_issues()
        .into_iter()
        .find(|issue| issue.key.as_str() == "DESK-176")
        .expect("issue");
    let dashboard = cx.new(|_| Dashboard::from_sample_data());

    cx.update_entity(&dashboard, |dashboard, cx| {
        dashboard.open_update_issue(issue.id.clone(), true, cx);
    });
    let (selected_issue, section, mobile_detail_open) =
        cx.read_entity(&dashboard, |dashboard, _| {
            (
                dashboard.selected_issue.clone(),
                dashboard.section,
                dashboard.mobile_detail_open,
            )
        });

    assert_eq!(selected_issue, Some(issue.id));
    assert_eq!(section, Section::Issues);
    assert!(mobile_detail_open);
}

#[gpui::test]
fn transition_chooser_options_remain_visible_in_constrained_popover(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);

    let workspace = futures_lite::future::block_on(LiveWorkspace::initialize(
        JiraSiteId::new("site").expect("site"),
        None,
        Arc::new(EmptyJira),
        Arc::new(jira_storage::SqliteStore::in_memory().expect("store")),
    ))
    .expect("workspace");
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.workspace = Some(Arc::new(workspace));
    let issue = dashboard
        .selected_issue
        .clone()
        .expect("sample issue selected");
    let transitions = (0..12)
        .map(|index| IssueTransition {
            id: (31 + index).to_string(),
            name: format!("Move issue {index}"),
            to: jira_domain::Status {
                id: (3 + index).to_string(),
                name: format!("Status {index}"),
                category: None,
            },
        })
        .collect();
    dashboard
        .issue_edit_flow
        .set_state_for_test(IssueEditState::TransitionChooser {
            issue_id: issue,
            issue_key: "DESK-176".to_owned(),
            transitions,
        });
    dashboard.issue_edit_flow.set_status_popover_open(true);
    let window = cx.open_window(gpui::size(px(720.), px(600.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let option_bounds = visual
        .debug_bounds("status-transition-31")
        .expect("transition option should be laid out");
    assert!(
        option_bounds.size.height > px(0.),
        "transition option collapsed: {option_bounds:?}"
    );

    visual.simulate_click(
        gpui::point(
            option_bounds.origin.x + option_bounds.size.width / 2.,
            option_bounds.origin.y + option_bounds.size.height / 2.,
        ),
        Default::default(),
    );
    assert!(
        dashboard_entity.read_with(&visual, |dashboard, _| {
            matches!(
                dashboard.issue_edit_flow.state(),
                IssueEditState::ConfirmingTransition { .. }
            )
        }),
        "first transition should be clickable at {option_bounds:?}"
    );
}

#[gpui::test]
fn dashboard_issue_edit_reads_and_dispatches_each_confirmed_operation_once(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_component::init);
    let site_id = JiraSiteId::new("site").expect("site");
    let calls = Arc::new(Mutex::new(EditCalls::default()));
    let users = vec![
        User {
            site_id: site_id.clone(),
            account_id: jira_domain::AccountId::new("acct-b").expect("account"),
            display_name: "Bob Example".to_owned(),
            avatar_url: None,
            active: true,
        },
        User {
            site_id: site_id.clone(),
            account_id: jira_domain::AccountId::new("acct-a").expect("account"),
            display_name: "Alice Example".to_owned(),
            avatar_url: None,
            active: true,
        },
    ];
    let transitions = vec![IssueTransition {
        id: "31".to_owned(),
        name: "Start work".to_owned(),
        to: jira_domain::Status {
            id: "3".to_owned(),
            name: "In Progress".to_owned(),
            category: None,
        },
    }];
    let editor = Arc::new(RecordingIssueEditor {
        calls: calls.clone(),
        users: users.clone(),
        transitions: transitions.clone(),
    });
    let workspace = Arc::new(
        futures_lite::future::block_on(LiveWorkspace::initialize_with_writers(
            site_id.clone(),
            None,
            Arc::new(EmptyJira),
            Arc::new(EmptyCommentWriter),
            editor,
            Arc::new(jira_storage::SqliteStore::in_memory().expect("store")),
        ))
        .expect("workspace"),
    );

    let mut dashboard = Dashboard::from_sample_data();
    let issue_id = dashboard.selected_issue.clone().expect("sample issue");
    let assignee_users = futures_lite::future::block_on(workspace.search_assignable_users(
        IssueLocator::Id(issue_id.clone()),
        String::new(),
        100,
        &CancellationToken::new(),
    ))
    .expect("assignable users");
    let transition_options = futures_lite::future::block_on(workspace.available_transitions(
        IssueLocator::Id(issue_id.clone()),
        &CancellationToken::new(),
    ))
    .expect("transitions");
    assert_eq!(assignee_users, users);
    assert_eq!(transition_options, transitions);
    dashboard.workspace = Some(workspace);
    dashboard
        .issue_edit_flow
        .set_state_for_test(IssueEditState::AssigneeChooser {
            issue_id: issue_id.clone(),
            issue_key: "IX-100".to_owned(),
            query: String::new(),
            users: assignee_users.clone(),
        });
    let window = cx.open_window(gpui::size(px(900.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    cx.update_entity(&dashboard_entity, |dashboard, cx| {
        dashboard.choose_assignee(
            Some(users[1].account_id.clone()),
            users[1].display_name.clone(),
            cx,
        );
    });
    visual.run_until_parked();
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| {
        matches!(
            dashboard.issue_edit_flow.state(),
            IssueEditState::ConfirmingAssignee { .. }
        )
    }));
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.submit_assignee(window, cx);
            dashboard.submit_assignee(window, cx);
        });
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    cx.run_until_parked();

    cx.update_entity(&dashboard_entity, |dashboard, cx| {
        dashboard.operation_in_progress = false;
        dashboard
            .issue_edit_flow
            .set_state_for_test(IssueEditState::TransitionChooser {
                issue_id: issue_id.clone(),
                issue_key: "IX-100".to_owned(),
                transitions: transition_options.clone(),
            });
        dashboard.choose_transition(transition_options[0].clone(), cx);
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.submit_transition(window, cx);
            dashboard.submit_transition(window, cx);
        });
    });
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.run_until_parked();
    cx.run_until_parked();

    let calls = calls.lock().expect("calls lock");
    assert_eq!(calls.searches.len(), 1);
    assert_eq!(calls.searches[0].site_id, site_id);
    assert_eq!(
        calls.searches[0].locator,
        IssueLocator::Id(issue_id.clone())
    );
    assert_eq!(calls.searches[0].query, "");
    assert_eq!(calls.searches[0].limit, 100);
    assert_eq!(calls.transition_reads.len(), 1);
    assert_eq!(
        calls.transition_reads[0].locator,
        IssueLocator::Id(issue_id.clone())
    );
    assert_eq!(calls.assignments.len(), 1);
    assert_eq!(
        calls.assignments[0].locator,
        IssueLocator::Id(issue_id.clone())
    );
    assert_eq!(
        calls.assignments[0].assignee,
        Some(users[1].account_id.clone())
    );
    assert_eq!(calls.transition_writes.len(), 1);
    assert_eq!(
        calls.transition_writes[0].locator,
        IssueLocator::Id(issue_id)
    );
    assert_eq!(calls.transition_writes[0].transition_id, "31");
}

#[gpui::test]
fn team_tracker_table_and_detail_are_bounded_on_desktop(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Team;
    dashboard.team_members = vec![PersistedTeamMember {
        identifier: "amina".to_owned(),
        account_id: "amina".to_owned(),
        display_name: "Amina Yusuf".to_owned(),
    }];
    dashboard.team_issues = sample_issues();
    dashboard.team_feedback = Some("Team tracker refreshed · fetched 5 · displaying 3".to_owned());

    let window = cx.open_window(gpui::size(px(1370.), px(900.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let table_bounds = visual
        .debug_bounds("team-table")
        .expect("team table should be laid out");
    let detail_bounds = visual
        .debug_bounds("team-detail")
        .expect("team detail should be laid out");
    assert!(
        table_bounds.size.height > px(300.),
        "team table body collapsed to header: {table_bounds:?}"
    );
    assert!(
        detail_bounds.origin.y + detail_bounds.size.height <= px(900.) + px(1.),
        "team detail escapes the desktop content region: {detail_bounds:?}"
    );
    assert!(
        table_bounds.origin.x + table_bounds.size.width <= detail_bounds.origin.x,
        "team detail overlaps the team table: table={table_bounds:?}, detail={detail_bounds:?}"
    );
    assert!(
        detail_bounds.origin.x + detail_bounds.size.width <= px(1370.) + px(1.),
        "team detail escapes the desktop width: {detail_bounds:?}"
    );

    visual.simulate_resize(gpui::size(px(390.), px(800.)));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let mobile_table_bounds = visual
        .debug_bounds("team-table")
        .expect("team table should remain visible on mobile");
    assert!(
        mobile_table_bounds.size.height > px(300.),
        "mobile team table body collapsed: {mobile_table_bounds:?}"
    );
}

#[gpui::test]
fn empty_issue_detail_status_stays_within_detail_pane(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Team;
    dashboard.selected_issue = None;
    dashboard.selected_issue_core = None;
    let window = cx.open_window(gpui::size(px(960.), px(700.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let detail_bounds = visual
        .debug_bounds("issue-detail")
        .expect("issue detail should be laid out");
    let status_bounds = visual
        .debug_bounds("issue-detail-status")
        .expect("empty detail status should be laid out");
    assert!(
        status_bounds.origin.x >= detail_bounds.origin.x,
        "empty detail status escapes left edge: detail={detail_bounds:?}, status={status_bounds:?}"
    );
    assert!(
        status_bounds.origin.x + status_bounds.size.width
            <= detail_bounds.origin.x + detail_bounds.size.width + px(1.),
        "empty detail status escapes right edge: detail={detail_bounds:?}, status={status_bounds:?}"
    );
}

#[gpui::test]
fn mobile_remote_lookup_loading_and_error_states_stay_visible_in_the_list(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(gpui_component::init);
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.remote_lookup = RemoteLookupState::Loading {
        query: " ix-404 ".to_owned(),
    };
    let window = cx.open_window(gpui::size(px(320.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let loading = visual
        .debug_bounds("remote-lookup-loading")
        .expect("mobile lookup loading state should be visible in the list");
    assert!(loading.size.width > px(0.) && loading.size.height > px(0.));

    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.remote_lookup = RemoteLookupState::Error {
                query: " ix-404 ".to_owned(),
                copy: OutcomeCopy::new(
                    "Jira lookup · issue was not found",
                    FeedbackSeverity::Error,
                    FeedbackCertainty::Definite,
                    RecoveryDirective::Retry,
                ),
            };
            cx.notify();
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let error = visual
        .debug_bounds("remote-lookup-error")
        .expect("mobile lookup error state should be visible in the list");
    assert!(error.size.width > px(0.) && error.size.height > px(0.));
    assert!(visual.debug_bounds("remote-lookup-loading").is_none());
}

#[gpui::test]
fn selected_detail_feedback_is_early_and_has_stable_state_identity(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.selected_issue = Some(issue.id.clone());
    dashboard.mobile_detail_open = true;
    dashboard.detail_state = DetailState::Loading {
        issue_id: issue.id.clone(),
    };
    let window = cx.open_window(gpui::size(px(320.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let loading = visual
        .debug_bounds("issue-detail-loading")
        .expect("selected detail loading state should be laid out");
    let description = visual
        .debug_bounds("issue-detail-description")
        .expect("selected detail description should be laid out");
    assert!(loading.origin.y < description.origin.y);

    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.detail_state = DetailState::Error {
                issue_id: issue.id.clone(),
                copy: OutcomeCopy::new(
                    "Issue details unavailable · Jira issue was not found",
                    FeedbackSeverity::Error,
                    FeedbackCertainty::Definite,
                    RecoveryDirective::Retry,
                ),
            };
            cx.notify();
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let error = visual
        .debug_bounds("issue-detail-error")
        .expect("selected detail error state should be laid out");
    assert!(error.origin.y < description.origin.y);
    assert!(visual.debug_bounds("issue-detail-loading").is_none());
}

#[gpui::test]
fn update_card_keeps_issue_key_visible_at_compact_desktop_width(cx: &mut gpui::TestAppContext) {
    cx.update(gpui_component::init);

    let issue_id = IssueId::new("100").expect("issue");
    let event_id = jira_domain::EventId::new("event-1").expect("event");
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Updates;
    dashboard.update_groups = vec![UpdateGroupViewModel {
        issue_id: issue_id.clone(),
        issue_key: "PLATFORM-12345".to_owned(),
        issue_summary: "A deliberately long summary that must give up width to the stable issue key while retaining the timestamp and controls".to_owned(),
        latest_occurred_at: "2026-08-23 14:35 IST".to_owned(),
        unread_count: 1,
        unread: true,
        events: vec![UpdateViewModel {
            event_id,
            issue_id,
            issue_key: "PLATFORM-12345".to_owned(),
            issue_summary: "A deliberately long summary".to_owned(),
            change: "Status changed to In Progress".to_owned(),
            occurred_at: "2026-08-23 14:35 IST".to_owned(),
            unread: true,
        }],
    }];

    let window = cx.open_window(gpui::size(px(1095.), px(700.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let card_bounds = visual
        .debug_bounds("update-card-0")
        .expect("update card should be laid out");
    let key_bounds = visual
        .debug_bounds("update-key")
        .expect("issue key should be laid out");
    let actions_bounds = visual
        .debug_bounds("update-actions-0")
        .expect("update actions should be laid out");
    assert!(
        key_bounds.size.width >= px(90.),
        "full issue key was allowed to shrink: key={key_bounds:?}"
    );
    assert!(
        key_bounds.origin.x >= card_bounds.origin.x,
        "issue key escapes card left edge: card={card_bounds:?}, key={key_bounds:?}"
    );
    assert!(
        key_bounds.origin.x + key_bounds.size.width
            <= card_bounds.origin.x + card_bounds.size.width + px(1.),
        "issue key escapes card right edge: card={card_bounds:?}, key={key_bounds:?}"
    );
    assert!(
        actions_bounds.size.width > px(1.) && actions_bounds.size.height > px(1.),
        "update actions collapsed: {actions_bounds:?}"
    );
    assert!(
        actions_bounds.origin.x + actions_bounds.size.width
            <= card_bounds.origin.x + card_bounds.size.width + px(1.),
        "update actions escape card right edge: card={card_bounds:?}, actions={actions_bounds:?}"
    );
}

#[test]
fn team_only_selected_issue_still_resolves_detail_identity() {
    let team_issue = sample_issues().into_iter().next().expect("team issue");
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.domain_issues.clear();
    dashboard.issues.clear();
    dashboard.team_issues = vec![team_issue.clone()];
    dashboard.selected_issue = Some(team_issue.id.clone());

    let view = dashboard
        .selected_issue_view()
        .expect("team issue should resolve in detail");
    assert_eq!(view.id, team_issue.id);
    assert_eq!(view.key, team_issue.key.to_string());
}

#[test]
fn team_detail_width_preserves_a_bounded_ticket_pane_at_breakpoints() {
    for (width, layout) in [
        (720., LayoutMode::Compact),
        (959., LayoutMode::Compact),
        (960., LayoutMode::Standard),
        (1_199., LayoutMode::Standard),
        (1_200., LayoutMode::Wide),
        (1_370., LayoutMode::Wide),
        (1_919., LayoutMode::Wide),
        (1_920., LayoutMode::Wide),
    ] {
        let mode = team_table_mode_for_width(width);
        for sidebar_collapsed in [false, true] {
            let clamped = clamped_team_detail_width(
                DETAIL_SIDEBAR_DEFAULT_WIDTH,
                width,
                layout,
                mode,
                sidebar_collapsed,
            );
            let content = width
                - sidebar_width_for_viewport(layout, sidebar_collapsed, width)
                - 2. * layout.list_padding();
            let table_min = team_table_min_width(mode, layout);
            assert!(clamped >= 0.);
            assert!(clamped + TEAM_DETAIL_RESIZE_HANDLE_WIDTH + table_min <= content + 0.01);
            assert!(clamped <= content / 2. + 0.01);
            if width == 1_200. && !sidebar_collapsed {
                assert_eq!(clamped, 356.);
            }
            if width == 1_370. {
                assert_eq!(clamped, DETAIL_SIDEBAR_DEFAULT_WIDTH);
            }
        }
    }
}

#[test]
fn team_table_mode_defers_wide_columns_until_the_table_can_fit() {
    assert_eq!(team_table_mode_for_width(1_199.), TeamTableMode::Cards);
    assert_eq!(team_table_mode_for_width(1_200.), TeamTableMode::DenseTable);
    assert_eq!(team_table_mode_for_width(1_919.), TeamTableMode::DenseTable);
    assert_eq!(team_table_mode_for_width(1_920.), TeamTableMode::WideTable);
}

#[test]
fn semantic_ticket_controls_activate_only_on_plain_enter_or_space() {
    let event = |key| KeyDownEvent {
        keystroke: gpui::Keystroke::parse(key).expect("keystroke"),
        is_held: false,
        prefer_character_input: false,
    };
    assert!(is_activation_key(&event("enter")));
    assert!(is_activation_key(&event("space")));
    assert!(!is_activation_key(&event("ctrl-enter")));
    assert!(!is_activation_key(&KeyDownEvent {
        is_held: true,
        ..event("enter")
    }));
}

#[test]
fn filtered_selected_issue_resolves_header_from_domain_cache() {
    let domain_issues = sample_issues();
    let users = sample_users();
    let selected = domain_issues
        .iter()
        .find(|issue| issue.key.as_str() == "DESK-176")
        .expect("selected issue");
    let visible = issue_views_for_filter(&domain_issues, &users, IssueStatusFilter::ToDo, "");
    assert!(!visible.iter().any(|issue| issue.id == selected.id));

    let view = selected_issue_view_from_sources(
        Some(&selected.id),
        &visible,
        &domain_issues,
        None,
        &users,
    )
    .expect("filtered selected issue header");

    assert_eq!(view.id, selected.id);
    assert_eq!(view.key, selected.key.to_string());
}

#[test]
fn absent_cache_selected_issue_uses_fetched_core_for_header_and_comments() {
    let issue = sample_issues()
        .into_iter()
        .find(|issue| issue.key.as_str() == "DESK-176")
        .expect("issue");
    let users = sample_users();
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.domain_issues.clear();
    dashboard.issues.clear();
    dashboard.selected_issue = Some(issue.id.clone());
    dashboard.selected_issue_core = Some(issue.clone());

    let view = dashboard
        .selected_issue_view()
        .expect("fetched issue core header");
    let comment_target = dashboard.comment_target_issue().expect("comment target");

    assert_eq!(view.id, issue.id);
    assert_eq!(
        view.assignee,
        IssueViewModel::from_domain(&issue, &users).assignee
    );
    assert_eq!(comment_target.id, issue.id);
}

#[test]
fn rebuild_retains_hidden_selection_and_defers_refresh_of_absent_cache_fetch() {
    let issue = sample_issues()
        .into_iter()
        .find(|issue| issue.key.as_str() == "DESK-176")
        .expect("issue");
    let visible = issue_views_for_filter(
        std::slice::from_ref(&issue),
        &sample_users(),
        IssueStatusFilter::ToDo,
        "",
    );
    assert!(visible.is_empty());
    assert_eq!(
        selection_after_issue_view_rebuild(Some(issue.id.clone()), &visible),
        Some(issue.id.clone())
    );

    let loading = DetailState::Loading {
        issue_id: issue.id.clone(),
    };
    assert!(should_defer_detail_refresh(
        Some(&issue.id),
        &[],
        &loading,
        true,
    ));
    assert!(!should_defer_detail_refresh(
        Some(&issue.id),
        std::slice::from_ref(&issue),
        &loading,
        true,
    ));
    assert!(!should_defer_detail_refresh(
        Some(&issue.id),
        &[],
        &DetailState::Empty,
        true,
    ));
    assert!(should_defer_detail_refresh(
        Some(&issue.id),
        &[],
        &DetailState::Error {
            issue_id: issue.id.clone(),
            copy: OutcomeCopy::new(
                "detail unavailable",
                FeedbackSeverity::Error,
                FeedbackCertainty::Definite,
                RecoveryDirective::Retry,
            ),
        },
        true,
    ));
    assert!(!should_defer_detail_refresh(
        Some(&issue.id),
        &[],
        &DetailState::Error {
            issue_id: IssueId::new("different").expect("issue"),
            copy: OutcomeCopy::new(
                "detail unavailable",
                FeedbackSeverity::Error,
                FeedbackCertainty::Definite,
                RecoveryDirective::Retry,
            ),
        },
        true,
    ));
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
        copy: OutcomeCopy::new(
            "not found",
            FeedbackSeverity::Error,
            FeedbackCertainty::Definite,
            RecoveryDirective::Retry,
        ),
    };
    let ticket = dashboard
        .remote_lookup_epoch
        .begin(RequestSource::RemoteLookup, "IX-404".to_owned());
    let generation = dashboard.remote_lookup_epoch.generation();

    dashboard.clear_remote_lookup();

    assert!(ticket.cancellation().is_cancelled());
    assert_eq!(dashboard.remote_lookup, RemoteLookupState::Idle);
    assert_eq!(dashboard.remote_lookup_epoch.generation(), generation + 1);
}

#[test]
fn issue_edit_target_snapshot_requires_current_issue_and_generation() {
    let issue = IssueId::new("100").expect("issue");
    assert!(issue_edit_target_is_current(Some(&issue), &issue, 4, 4));
    assert!(!issue_edit_target_is_current(Some(&issue), &issue, 5, 4));
    let other = IssueId::new("200").expect("issue");
    assert!(!issue_edit_target_is_current(Some(&other), &issue, 4, 4));
}

#[test]
fn grouped_activity_dispatch_ids_include_read_and_unread_events() {
    let issue = IssueId::new("100").expect("issue");
    let first = jira_domain::EventId::new("event-1").expect("event");
    let second = jira_domain::EventId::new("event-2").expect("event");
    let group = UpdateGroupViewModel {
        issue_id: issue,
        issue_key: "IX-100".to_owned(),
        issue_summary: "Summary".to_owned(),
        latest_occurred_at: "now".to_owned(),
        unread_count: 1,
        unread: true,
        events: vec![
            UpdateViewModel {
                event_id: first.clone(),
                issue_id: IssueId::new("100").expect("issue"),
                issue_key: "IX-100".to_owned(),
                issue_summary: "Summary".to_owned(),
                change: "first".to_owned(),
                occurred_at: "earlier".to_owned(),
                unread: false,
            },
            UpdateViewModel {
                event_id: second.clone(),
                issue_id: IssueId::new("100").expect("issue"),
                issue_key: "IX-100".to_owned(),
                issue_summary: "Summary".to_owned(),
                change: "second".to_owned(),
                occurred_at: "latest".to_owned(),
                unread: true,
            },
        ],
    };
    assert_eq!(update_group_event_ids(&group), vec![first, second]);
}

#[test]
fn team_identifier_parser_trims_bounds_and_rejects_jql_metacharacters() {
    assert_eq!(
        team_identifier_lines("  account-a  \n\nuser@example.com").expect("valid"),
        vec!["account-a", "user@example.com"]
    );
    assert!(team_identifier_lines("account\"bad").is_err());
    assert!(team_identifier_lines(&"account-a\n".repeat(MAX_TEAM_MEMBERS + 1)).is_err());
}

#[test]
fn direct_account_id_member_uses_persistable_unknown_display_name() {
    let member = persisted_direct_team_member("account-123".to_owned()).expect("valid ID");
    assert_eq!(member.display_name, "Unknown user");
    assert!(normalize_team_members(vec![member]).is_ok());
}

#[test]
fn team_section_is_distinct_from_primary_issue_section() {
    assert_ne!(Section::Issues, Section::Team);
    assert_ne!(Section::Team, Section::Settings);
}

#[test]
fn team_refresh_feedback_reports_fetched_and_displayed_counts() {
    assert_eq!(
        team_refresh_feedback("Team tracker refreshed", &sample_issues()),
        "Team tracker refreshed · fetched 5 · displaying 3 in-progress tickets"
    );
}

#[test]
fn team_summary_is_cache_honest_and_pluralizes_counts() {
    assert_eq!(
        team_summary(1, 1),
        "1 in-progress ticket displayed · 1 configured team member · cached updates remain isolated from Jira issues"
    );
    assert_eq!(
        team_summary(3, 2),
        "3 in-progress tickets displayed · 2 configured team members · cached updates remain isolated from Jira issues"
    );
}
