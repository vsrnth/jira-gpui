use super::issues_view::{
    compact_issue_workspace_summary, issue_count_label, issue_workspace_summary,
};
use super::settings::{persisted_direct_team_member, team_identifier_lines};
use super::shell_view::{refresh_action_label, should_render_sidebar_sync_message};
use super::updates_view::update_filter_is_selected;
use super::*;
use crate::app_shell::AppearancePreference;
use crate::presentation::{UpdateViewModel, normalized_issue_key};
use crate::sample_data::{sample_issues, sample_users};
use gpui_kit::VisualTestContext;
use gpui_kit::component::searchable_list::SearchableListDelegate as _;
use gpui_kit::component::table::{ColumnSort, TableDelegate as _};
use jira_application::{
    AddCommentRequest, AssignIssueRequest, AssignableUserSearchRequest, ErrorKind,
    IssueFetchRequest, IssuePage, IssueTransitionsRequest, JiraAttachmentReadPort,
    JiraCommentWritePort, JiraIssueActivityPort, JiraIssueDetailReadPort, JiraIssueEditPort,
    JiraIssueSearchPort, JiraReadPort, JiraSyncReadPort, JiraUserReadPort, PortFuture, SyncMode,
    TransitionIssueRequest, UserSearchRequest,
};
use jira_domain::JiraSiteId;
use std::sync::{Arc, Mutex};
use time::macros::datetime;

#[test]
fn issue_count_label_distinguishes_filtered_and_loaded_counts() {
    assert_eq!(issue_count_label(4, 9, true), "4 of 9 Jira issues");
    assert_eq!(issue_count_label(9, 9, false), "9 Jira issues");
}

#[test]
fn issue_workspace_summary_pluralizes_honest_local_counts() {
    assert_eq!(
        issue_workspace_summary(3, 2),
        "3 tickets with unread updates · 2 issues in progress"
    );
    assert_eq!(
        issue_workspace_summary(1, 1),
        "1 ticket with unread updates · 1 issue in progress"
    );
    assert_eq!(
        issue_workspace_summary(0, 0),
        "0 tickets with unread updates · 0 issues in progress"
    );
    assert_eq!(compact_issue_workspace_summary(3, 2), "3 unread · 2 active");
}

#[gpui_kit::test]
fn mobile_issue_workspace_uses_compact_counts_and_non_overlapping_actions(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(390.), px(800.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(compact_issue_workspace_summary(3, 3), "3 unread · 3 active");
    let overview = visual
        .debug_bounds("issues-workspace-summary")
        .expect("mobile issue workspace overview should be laid out");
    let counts = visual
        .debug_bounds("issues-workspace-summary-counts")
        .expect("mobile compact counts should be laid out");
    let updates = visual
        .debug_bounds("issues-review-updates")
        .expect("mobile Updates action should be laid out");
    let active = visual
        .debug_bounds("issues-show-active")
        .expect("mobile Active action should be laid out");

    assert!(overview.size.height < px(90.));
    assert!(counts.size.width > px(0.) && counts.size.height > px(0.));
    assert!(updates.size.width > px(0.) && updates.size.height > px(0.));
    assert!(active.size.width > px(0.) && active.size.height > px(0.));
    assert!(updates.origin.x + updates.size.width <= active.origin.x);
    for bounds in [counts, updates, active] {
        assert!(bounds.origin.x >= overview.origin.x);
        assert!(bounds.origin.y >= overview.origin.y);
        assert!(bounds.origin.x + bounds.size.width <= overview.origin.x + overview.size.width);
        assert!(bounds.origin.y + bounds.size.height <= overview.origin.y + overview.size.height);
    }
}

#[gpui_kit::test]
fn issue_search_help_wraps_inside_narrow_issue_list(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(350.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let help = visual
        .debug_bounds("issue-search-help")
        .expect("search guidance should be visible");
    assert!(help.size.height > px(16.));
    assert!(help.origin.x >= px(0.));
    assert!(help.origin.x + help.size.width <= px(350.));
}

#[gpui_kit::test]
fn long_issue_summary_keeps_narrow_issue_row_bounded(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(350.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);

    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            let issue = dashboard
                .domain_issues
                .iter_mut()
                .find(|issue| issue.id.as_str() == "10184")
                .expect("fixture issue should exist");
            issue.summary = "A reliable issue summary stays readable when the workspace list is narrow and the source ticket title is exceptionally long. "
                .repeat(18);
            dashboard.rebuild_issue_views(false, cx);
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let row = visual
        .debug_bounds("issue-row-10184")
        .expect("mutated fixture issue row should be rendered");
    assert!(row.size.width > px(0.) && row.size.height > px(0.));
    assert!(
        row.size.height < px(190.),
        "long summary should remain clamped in a bounded row: {row:?}"
    );
}

#[gpui_kit::test]
fn clear_issue_filters_resets_widgets_and_model_without_touching_selection(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let mut dashboard = Dashboard::from_sample_data();
    let selected = dashboard.domain_issues[0].id.clone();
    dashboard.selected_issue = Some(selected.clone());
    let window = cx.open_window(gpui_kit::size(px(900.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.ensure_search_input(window, cx);
            dashboard.ensure_status_combobox(window, cx);
            let input = dashboard.search_input.clone().unwrap();
            input.update(cx, |input, cx| input.set_value("summary", window, cx));
            dashboard.set_search_query("summary".to_owned(), cx);
            dashboard
                .status_combobox
                .as_ref()
                .unwrap()
                .update(cx, |combobox, cx| {
                    combobox.set_selected_indices(
                        [gpui_kit::component::IndexPath::new(2)],
                        window,
                        cx,
                    );
                });
            dashboard.set_status_filter(IssueStatusFilter::Done, cx);
            assert_eq!(
                dashboard.search_input.as_ref().unwrap().read(cx).value(),
                "summary"
            );
            assert!(
                !dashboard
                    .status_combobox
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selection()
                    .is_empty()
            );
            dashboard.clear_issue_filters(window, cx);
            assert_eq!(dashboard.search_query, "");
            assert_eq!(dashboard.status_filter, IssueStatusFilter::All);
            assert_eq!(dashboard.selected_issue.as_ref(), Some(&selected));
            assert_eq!(
                dashboard.search_input.as_ref().unwrap().read(cx).value(),
                ""
            );
            assert!(
                dashboard
                    .status_combobox
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .selection()
                    .is_empty()
            );
        });
    });
}

#[gpui_kit::test]
fn search_input_enter_ignores_summary_and_selects_local_issue_key(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let dashboard = Dashboard::from_sample_data();
    let issue = dashboard.domain_issues[1].clone();
    let initially_selected = dashboard.domain_issues[0].id.clone();
    let issue_id = issue.id.clone();
    let key = issue.key.to_string();
    let window = cx.open_window(gpui_kit::size(px(900.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.ensure_search_input(window, cx);
            dashboard.selected_issue = Some(initially_selected.clone());
            let input = dashboard.search_input.clone().unwrap();
            input.update(cx, |input, cx| {
                input.set_value("summary text", window, cx);
                cx.emit(InputEvent::Change);
                cx.emit(InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                });
            });
        });
    });
    visual.run_until_parked();
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, _| {
            assert!(!dashboard.sync_message.contains("valid issue key"));
            assert!(matches!(dashboard.remote_lookup, RemoteLookupState::Idle));
        });
    });
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            let input = dashboard.search_input.clone().unwrap();
            input.update(cx, |input, cx| {
                input.set_value(&key, window, cx);
                cx.emit(InputEvent::Change);
            });
        });
    });
    visual.run_until_parked();
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            assert_eq!(dashboard.search_query, key);
            dashboard.search_input.clone().unwrap().update(cx, |_, cx| {
                cx.emit(InputEvent::PressEnter {
                    secondary: false,
                    shift: false,
                });
            });
        });
    });
    visual.run_until_parked();
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, _| {
            // The Enter event is emitted only after Change has been drained, matching typing.
            assert_eq!(dashboard.selected_issue.as_ref(), Some(&issue_id));
            assert!(dashboard.mobile_detail_open);
        });
    });
}

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
fn update_filter_selection_drives_toggle_accessibility_state() {
    assert!(update_filter_is_selected(
        UpdateFilter::Unread,
        UpdateFilter::Unread
    ));
    assert!(!update_filter_is_selected(
        UpdateFilter::Unread,
        UpdateFilter::All
    ));
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
fn status_control_disables_during_confirmation() {
    let issue_id = IssueId::new("100").expect("issue");
    let confirming = IssueEditState::ConfirmingAssignee {
        issue_id,
        issue_key: "IX-100".to_owned(),
        account_id: None,
        display_name: "Unassigned".to_owned(),
    };
    assert!(!status_control_is_editable(
        true,
        true,
        false,
        false,
        &confirming
    ));
}

#[gpui_kit::test]
fn native_status_popover_list_is_bounded_and_selection_only_confirms(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let site_id = JiraSiteId::new("site").expect("site");
    let calls = Arc::new(Mutex::new(EditCalls::default()));
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
        users: Vec::new(),
        transitions: transitions.clone(),
    });
    let workspace = Arc::new(
        futures_lite::future::block_on(LiveWorkspace::initialize_with_writers(
            site_id,
            None,
            Arc::new(EmptyJira),
            Arc::new(EmptyCommentWriter),
            editor,
            Arc::new(jira_storage::SqliteStore::in_memory().expect("store")),
        ))
        .expect("workspace"),
    );

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.mobile_detail_open = true;
    let issue_id = dashboard.selected_issue.clone().expect("selected issue");
    dashboard.workspace = Some(workspace);
    dashboard.status_transition_items = transitions.clone();
    dashboard.status_transition_items_revision = 1;
    dashboard.status_transition_state = StatusTransitionReadState::Ready {
        issue_id: issue_id.clone(),
    };
    let window = cx.open_window(gpui_kit::size(px(1_200.), px(900.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let control = visual
        .debug_bounds("issue-status-control")
        .expect("status control should be laid out");
    assert!(control.size.width > px(0.) && control.size.height > px(0.));
    assert!(
        control.origin.x >= px(0.)
            && control.origin.y >= px(0.)
            && control.origin.x + control.size.width <= px(1_200.) + px(1.)
            && control.origin.y + control.size.height <= px(900.) + px(1.)
    );

    visual.simulate_click(
        gpui_kit::point(
            control.origin.x + control.size.width / 2.,
            control.origin.y + control.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let list = visual
        .debug_bounds("issue-status-transition-list")
        .expect("native transition list should open from the status trigger");
    assert!(
        list.size.width > px(0.)
            && list.size.height > px(0.)
            && list.origin.x >= px(0.)
            && list.origin.y >= px(0.)
            && list.origin.x + list.size.width <= px(1_200.) + px(1.)
            && list.origin.y + list.size.height <= px(900.) + px(1.)
    );
    let option = visual
        .debug_bounds("status-transition-31")
        .expect("native transition action should be visible");
    visual.simulate_click(
        gpui_kit::point(
            option.origin.x + option.size.width / 2.,
            option.origin.y + option.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    let status_state = dashboard_entity.read_with(&visual, |dashboard, _| {
        dashboard.status_transition_state.clone()
    });
    assert!(
        matches!(status_state, StatusTransitionReadState::Ready { .. }),
        "unexpected status state: {status_state:?}"
    );

    assert!(dashboard_entity.read_with(&visual, |dashboard, _| matches!(
        dashboard.issue_edit_flow.state(),
        IssueEditState::ConfirmingTransition { .. }
    )));
    assert!(
        calls
            .lock()
            .expect("calls lock")
            .transition_writes
            .is_empty()
    );
    assert!(!dashboard_entity.read_with(&visual, |dashboard, _| { dashboard.status_popover_open }));
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

#[gpui_kit::test]
fn selecting_appearance_updates_state_and_emits_event(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(640.), px(480.)), |_, _| {
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

#[test]
fn refresh_action_labels_are_concise_and_do_not_include_sync_detail() {
    assert_eq!(refresh_action_label(false), "Refresh Jira");
    assert_eq!(refresh_action_label(true), "Refreshing Jira…");
    assert!(!refresh_action_label(false).contains("Refresh complete"));
    assert!(!refresh_action_label(true).contains("Refresh complete"));
}

#[gpui_kit::test]
fn sidebar_toggle_is_manual_only_on_standard_and_wide_layouts(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(1_000.), px(700.)), |_, _| {
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

#[gpui_kit::test]
fn sidebar_bounds_switch_between_expanded_and_collapsed_widths(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(1_100.), px(700.)), |_, _| {
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

    assert_eq!(expanded.size.width, px(240.));
    assert_eq!(collapsed.size.width, px(48.));
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

#[gpui_kit::test]
fn sidebar_header_and_footer_rows_stay_bounded_and_toggle_is_reachable(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(1_100.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let sidebar = visual
        .debug_bounds("dashboard-sidebar")
        .expect("sidebar should be laid out");
    let navigation = visual
        .debug_bounds("sidebar-navigation")
        .expect("sidebar navigation should be laid out");
    assert!(navigation.origin.y >= sidebar.origin.y);
    assert!(navigation.origin.y + navigation.size.height <= sidebar.origin.y + sidebar.size.height);
    assert!(
        visual.debug_bounds("sidebar-branding").is_none(),
        "the removed branding block must not reserve a layout region"
    );
    let workspace_header = visual
        .debug_bounds("sidebar-workspace-header")
        .expect("expanded sidebar should expose workspace header");
    let workspace_icon = visual
        .debug_bounds("sidebar-workspace-icon")
        .expect("expanded sidebar should expose workspace icon");
    assert!(workspace_header.origin.y >= navigation.origin.y);
    assert!(
        workspace_header.origin.y + workspace_header.size.height
            <= sidebar.origin.y + sidebar.size.height
    );
    assert!(workspace_icon.size.width > px(0.) && workspace_icon.size.height > px(0.));
    let toggle = visual
        .debug_bounds("sidebar-toggle")
        .expect("expanded sidebar should expose its toggle in navigation");
    assert!(toggle.origin.y >= workspace_header.origin.y);
    assert!(toggle.origin.y + toggle.size.height <= navigation.origin.y + navigation.size.height);

    assert!(
        visual.debug_bounds("sidebar-sync-status").is_none(),
        "preview sync status should not duplicate the account footer"
    );
    let refresh = visual
        .debug_bounds("sidebar-refresh")
        .expect("expanded sidebar should expose refresh action");
    let profile_actions = visual
        .debug_bounds("sidebar-profile-actions")
        .expect("expanded sidebar should expose profile actions row");
    let profile = visual
        .debug_bounds("sidebar-profile")
        .expect("expanded sidebar should expose account footer");
    let settings = visual
        .debug_bounds("nav-settings")
        .expect("expanded sidebar should expose settings navigation");
    assert!(settings.origin.x >= sidebar.origin.x);
    assert!(settings.origin.x + settings.size.width <= sidebar.origin.x + sidebar.size.width);
    assert!(settings.origin.y >= sidebar.origin.y);
    assert!(settings.origin.y + settings.size.height <= sidebar.origin.y + sidebar.size.height);
    assert!(refresh.size.width <= px(24.));
    assert!(refresh.size.height <= px(24.));
    assert!(refresh.origin.x >= profile_actions.origin.x);
    assert!(
        refresh.origin.x + refresh.size.width
            <= profile_actions.origin.x + profile_actions.size.width
    );
    assert!(refresh.origin.y >= profile_actions.origin.y);
    assert!(
        refresh.origin.y + refresh.size.height
            <= profile_actions.origin.y + profile_actions.size.height
    );
    assert!(
        (f32::from(
            refresh.origin.y + refresh.size.height / 2.
                - (profile.origin.y + profile.size.height / 2.)
        ))
        .abs()
            <= 1.5
    );
    assert!(profile.origin.x >= sidebar.origin.x);
    assert!(profile.origin.x + profile.size.width <= sidebar.origin.x + sidebar.size.width);

    visual.simulate_click(
        gpui_kit::point(
            toggle.origin.x + toggle.size.width - px(16.),
            toggle.origin.y + toggle.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| dashboard.sidebar_collapsed));
    assert!(
        visual.debug_bounds("sidebar-toggle").is_some(),
        "collapsed standard sidebar must retain an expand toggle"
    );
    assert!(visual.debug_bounds("sidebar-workspace-header").is_some());
    assert!(visual.debug_bounds("sidebar-workspace-icon").is_some());
    assert!(visual.debug_bounds("sidebar-profile").is_some());
    assert!(visual.debug_bounds("sidebar-profile-label").is_none());
    assert!(visual.debug_bounds("sidebar-workspace-site").is_none());
    assert!(visual.debug_bounds("sidebar-workspace-mode").is_none());

    let collapsed_sidebar = visual
        .debug_bounds("dashboard-sidebar")
        .expect("collapsed sidebar should remain laid out");
    let collapsed_workspace_icon = visual
        .debug_bounds("sidebar-workspace-icon")
        .expect("collapsed workspace icon should remain visible");
    let collapsed_toggle_button = visual
        .debug_bounds("sidebar-toggle-button")
        .expect("collapsed toggle button should expose its control bounds");
    let collapsed_profile_actions = visual
        .debug_bounds("sidebar-profile-actions")
        .expect("collapsed sidebar should expose profile actions row");
    let collapsed_profile = visual
        .debug_bounds("sidebar-profile")
        .expect("collapsed sidebar should expose identity control");
    let collapsed_refresh = visual
        .debug_bounds("sidebar-refresh")
        .expect("collapsed sidebar should expose refresh action");
    assert!(collapsed_refresh.size.width <= px(24.));
    assert!(collapsed_refresh.size.height <= px(24.));
    assert!(collapsed_refresh.origin.x >= collapsed_sidebar.origin.x);
    assert!(
        collapsed_refresh.origin.x + collapsed_refresh.size.width
            <= collapsed_sidebar.origin.x + collapsed_sidebar.size.width
    );
    assert!(collapsed_refresh.origin.y >= collapsed_profile_actions.origin.y);
    assert!(
        collapsed_refresh.origin.y + collapsed_refresh.size.height
            <= collapsed_profile_actions.origin.y + collapsed_profile_actions.size.height
    );
    let rail_center = collapsed_sidebar.origin.x + collapsed_sidebar.size.width / 2.;
    assert!(
        collapsed_refresh.origin.y >= collapsed_profile.origin.y + collapsed_profile.size.height,
        "collapsed identity and refresh controls must not overlap: profile={collapsed_profile:?}, refresh={collapsed_refresh:?}"
    );
    let collapsed_profile_center = collapsed_profile.origin.x + collapsed_profile.size.width / 2.;
    let collapsed_refresh_center = collapsed_refresh.origin.x + collapsed_refresh.size.width / 2.;
    assert!(
        (f32::from(collapsed_profile_center) - f32::from(rail_center)).abs() <= 1.5,
        "collapsed identity should be centered in the rail: profile={collapsed_profile:?}, sidebar={collapsed_sidebar:?}"
    );
    assert!(
        (f32::from(collapsed_refresh_center) - f32::from(rail_center)).abs() <= 1.5,
        "collapsed refresh should be centered in the rail: refresh={collapsed_refresh:?}, sidebar={collapsed_sidebar:?}"
    );
    assert!(collapsed_profile_actions.origin.x >= collapsed_sidebar.origin.x);
    assert!(
        collapsed_profile_actions.origin.x + collapsed_profile_actions.size.width
            <= collapsed_sidebar.origin.x + collapsed_sidebar.size.width
    );
    let workspace_icon_center =
        collapsed_workspace_icon.origin.x + collapsed_workspace_icon.size.width / 2.;
    let toggle_button_center =
        collapsed_toggle_button.origin.x + collapsed_toggle_button.size.width / 2.;
    assert!(
        collapsed_toggle_button.origin.x >= collapsed_sidebar.origin.x
            && collapsed_toggle_button.origin.x + collapsed_toggle_button.size.width
                <= collapsed_sidebar.origin.x + collapsed_sidebar.size.width,
        "collapsed toggle must stay inside the 3rem rail: sidebar={collapsed_sidebar:?}, toggle={collapsed_toggle_button:?}"
    );
    assert!(
        (f32::from(workspace_icon_center) - f32::from(rail_center)).abs() <= 1.5,
        "collapsed workspace icon should be centered in the rail: sidebar={collapsed_sidebar:?}, icon={collapsed_workspace_icon:?}"
    );
    assert!(
        (f32::from(toggle_button_center) - f32::from(rail_center)).abs() <= 1.5,
        "collapsed toggle should share the workspace icon rail: sidebar={collapsed_sidebar:?}, toggle={collapsed_toggle_button:?}"
    );
    assert!(
        (f32::from(workspace_icon_center) - f32::from(toggle_button_center)).abs() <= 1.5,
        "collapsed workspace icon and toggle should share a horizontal center: icon={collapsed_workspace_icon:?}, toggle={collapsed_toggle_button:?}"
    );
}

#[gpui_kit::test]
fn account_menu_exposes_settings_categories_and_selects_settings_section(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(1_100.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let sidebar = visual
        .debug_bounds("dashboard-sidebar")
        .expect("sidebar should be laid out");
    let settings = visual
        .debug_bounds("nav-settings")
        .expect("settings should be visible in desktop navigation");
    assert!(settings.origin.x >= sidebar.origin.x);
    assert!(settings.origin.x + settings.size.width <= sidebar.origin.x + sidebar.size.width);
    assert!(settings.origin.y >= sidebar.origin.y);
    assert!(settings.origin.y + settings.size.height <= sidebar.origin.y + sidebar.size.height);
    let profile = visual
        .debug_bounds("sidebar-profile")
        .expect("profile should be a visible account-menu trigger");
    visual.simulate_click(
        gpui_kit::point(
            profile.origin.x + profile.size.width / 2.,
            profile.origin.y + profile.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();

    // PopupMenu owns keyboard focus and exposes each standard item as a semantic MenuItem. The
    // first down press enters the menu list, then three more move through the first categories.
    visual.simulate_keystrokes("down down down down enter");
    visual.run_until_parked();
    assert!(
        dashboard_entity.read_with(&visual, |dashboard, _| {
            dashboard.section == Section::Settings
                && dashboard.settings_category == SettingsCategory::TeamTracker
        }),
        "selecting Team tracker should activate the matching settings category"
    );
}

#[gpui_kit::test]
fn long_sync_status_stays_bounded_beside_desktop_content_at_short_height(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.sync_message = "Updated · 192 issues · 3 new updates".to_owned();
    let window = cx.open_window(gpui_kit::size(px(1_100.), px(160.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let sidebar = visual
        .debug_bounds("dashboard-sidebar")
        .expect("sidebar should be laid out");
    let status = visual
        .debug_bounds("sidebar-sync-status")
        .expect("desktop sync status should be laid out");
    let main = visual
        .debug_bounds("dashboard-main")
        .expect("main content should be laid out");
    assert!(status.origin.x + status.size.width <= main.origin.x);
    assert!(status.origin.y + status.size.height <= sidebar.origin.y + sidebar.size.height);
    assert!(main.origin.y >= sidebar.origin.y);
}

#[test]
fn sidebar_sync_message_visibility_only_hides_redundant_preview_copy() {
    assert!(!should_render_sidebar_sync_message(
        "Preview data · Jira connection not configured"
    ));
    for message in [
        "Opening local cache…",
        "Updated · 2 issues",
        "Startup error · unavailable",
    ] {
        assert!(should_render_sidebar_sync_message(message));
    }
}

#[gpui_kit::test]
fn long_sync_status_stays_above_mobile_content_at_short_height(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.sync_message = "Updated · 192 issues · 3 new updates".to_owned();
    let window = cx.open_window(gpui_kit::size(px(320.), px(160.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let navigation = visual
        .debug_bounds("mobile-navigation")
        .expect("mobile navigation should be laid out");
    let status = visual
        .debug_bounds("mobile-sync-status")
        .expect("mobile sync status should be laid out");
    let status_text = visual
        .debug_bounds("mobile-sync-status-text")
        .expect("mobile sync status text should be laid out");
    let main = visual
        .debug_bounds("dashboard-main")
        .expect("main content should be laid out");
    assert!(status.origin.y >= navigation.origin.y + navigation.size.height);
    assert!(status.origin.y + status.size.height <= main.origin.y);
    assert!(status_text.origin.y + status_text.size.height <= status.origin.y + status.size.height);
}

#[gpui_kit::test]
fn mobile_navigation_fits_all_destinations_at_supported_minimum(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(320.), px(700.)), |_, _| {
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
fn refresh_status_uses_bounded_update_wording_without_notification_counts() {
    let result = refresh_result_with_inserted_events(1);

    let message = refresh_complete_message(&result);
    assert_eq!(message, "Updated · 5 issues · 1 new update");
    assert!(!message.contains("desktop notification"));
}

#[test]
fn refresh_status_omits_zero_counts_and_internal_details() {
    let mut result = refresh_result_with_inserted_events(0);
    result.outcome.mode = SyncMode::Baseline;
    result.outcome.notifications_delivered = 0;
    result.outcome.notification_failures = 0;

    assert_eq!(refresh_complete_message(&result), "Updated · 5 issues");
}

#[test]
fn refresh_status_ignores_notification_failures_in_primary_copy() {
    let mut result = refresh_result_with_inserted_events(0);
    result.outcome.notification_failures = 2;

    let message = refresh_complete_message(&result);
    assert_eq!(message, "Updated · 5 issues");
    assert!(!message.contains("unavailable"));
}

#[test]
fn refresh_status_uses_singular_nouns_for_single_counts() {
    let mut result = refresh_result_with_inserted_events(1);
    result.cached.issues.truncate(1);
    result.outcome.notifications_delivered = 1;
    result.outcome.notification_failures = 1;

    assert_eq!(
        refresh_complete_message(&result),
        "Updated · 1 issue · 1 new update"
    );
}

#[test]
fn refresh_status_uses_plural_nouns_for_multiple_counts() {
    let mut result = refresh_result_with_inserted_events(2);
    result.outcome.notifications_delivered = 2;
    result.outcome.notification_failures = 2;

    assert_eq!(
        refresh_complete_message(&result),
        "Updated · 5 issues · 2 new updates"
    );
}

#[test]
fn refresh_status_uses_plural_noun_for_zero_issues() {
    let mut result = refresh_result_with_inserted_events(0);
    result.cached.issues.clear();

    assert_eq!(refresh_complete_message(&result), "Updated · 0 issues");
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
            .item(gpui_kit::component::IndexPath::new(index))
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
            gpui_kit::component::IndexPath::new(0),
            gpui_kit::component::IndexPath::new(2),
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

#[gpui_kit::test]
fn selected_without_workspace_finishes_epoch_and_drops_task(cx: &mut gpui_kit::TestAppContext) {
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

#[gpui_kit::test]
fn cached_detail_renders_without_spinner_and_survives_background_failure(
    cx: &mut gpui_kit::TestAppContext,
) {
    let workspace = futures_lite::future::block_on(LiveWorkspace::initialize(
        JiraSiteId::new("site").expect("site"),
        None,
        Arc::new(EmptyJira),
        Arc::new(jira_storage::SqliteStore::in_memory().expect("store")),
    ))
    .expect("workspace");
    let issue = sample_issues().into_iter().next().expect("issue");
    let issue_id = issue.id.clone();
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.mobile_detail_open = true;
    // This test isolates cached detail rendering; the native status lookup is
    // covered by the dedicated status-popover fixture below.
    dashboard.status_transition_reads_suppressed = true;
    dashboard.workspace = Some(Arc::new(workspace));
    let cached_description = "Persisted detail description";
    let cached_issue = dashboard
        .domain_issues
        .iter_mut()
        .find(|candidate| candidate.id == issue_id)
        .expect("selected issue");
    cached_issue.description_text = Some(cached_description.to_owned());
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(960.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.select_issue(issue_id.clone(), cx, true);
            assert!(matches!(
                dashboard.detail_state,
                DetailState::Refreshing { .. }
            ));
            assert!(!matches!(
                dashboard.detail_state,
                DetailState::Loading { .. }
            ));
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("issue-detail-loading").is_none());
    assert!(visual.debug_bounds("issue-detail-description").is_some());
    assert!(visual.debug_bounds("issue-detail-error").is_some());
    cx.read_entity(&dashboard_entity, |dashboard, _| {
        assert!(matches!(dashboard.detail_state, DetailState::Error { .. }));
        let selected = dashboard
            .domain_issues
            .iter()
            .find(|candidate| candidate.id == issue_id)
            .expect("cached selected issue");
        assert_eq!(
            selected.description_text.as_deref(),
            Some(cached_description)
        );
    });
}

#[gpui_kit::test]
fn team_only_cached_detail_renders_without_spinner_and_survives_background_failure(
    cx: &mut gpui_kit::TestAppContext,
) {
    let workspace = futures_lite::future::block_on(LiveWorkspace::initialize(
        JiraSiteId::new("site").expect("site"),
        None,
        Arc::new(EmptyJira),
        Arc::new(jira_storage::SqliteStore::in_memory().expect("store")),
    ))
    .expect("workspace");
    let mut team_issue = sample_issues().into_iter().next().expect("issue");
    let issue_id = team_issue.id.clone();
    team_issue.description_text = Some("Persisted Team Tracker detail".to_owned());
    team_issue.detail_loaded = true;

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.status_transition_reads_suppressed = true;
    dashboard.workspace = Some(Arc::new(workspace));
    dashboard.domain_issues.clear();
    dashboard.issues.clear();
    dashboard.team_issues = vec![team_issue];
    dashboard.section = Section::Team;

    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(960.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.select_issue(issue_id.clone(), cx, true);
            assert!(matches!(
                dashboard.detail_state,
                DetailState::Refreshing { .. }
            ));
            assert!(!matches!(
                dashboard.detail_state,
                DetailState::Loading { .. }
            ));
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("issue-detail-loading").is_none());
    assert!(visual.debug_bounds("issue-detail-description").is_some());
    assert!(visual.debug_bounds("issue-detail-error").is_some());
    cx.read_entity(&dashboard_entity, |dashboard, _| {
        assert!(matches!(dashboard.detail_state, DetailState::Error { .. }));
        let selected = dashboard
            .team_issues
            .iter()
            .find(|candidate| candidate.id == issue_id)
            .expect("cached Team Tracker issue");
        assert_eq!(
            selected.description_text.as_deref(),
            Some("Persisted Team Tracker detail")
        );
    });
}

#[gpui_kit::test]
fn empty_cached_detail_renders_without_spinner_and_survives_background_failure(
    cx: &mut gpui_kit::TestAppContext,
) {
    let workspace = futures_lite::future::block_on(LiveWorkspace::initialize(
        JiraSiteId::new("site").expect("site"),
        None,
        Arc::new(EmptyJira),
        Arc::new(jira_storage::SqliteStore::in_memory().expect("store")),
    ))
    .expect("workspace");
    let issue = sample_issues().into_iter().next().expect("issue");
    let issue_id = issue.id.clone();
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.mobile_detail_open = true;
    dashboard.status_transition_reads_suppressed = true;
    dashboard.workspace = Some(Arc::new(workspace));
    let cached_issue = dashboard
        .domain_issues
        .iter_mut()
        .find(|candidate| candidate.id == issue_id)
        .expect("selected issue");
    cached_issue.description_text = None;
    cached_issue.rich_description = None;
    cached_issue.detail_loaded = true;
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(960.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.select_issue(issue_id.clone(), cx, true);
            assert!(matches!(
                dashboard.detail_state,
                DetailState::Refreshing { .. }
            ));
            assert!(!matches!(
                dashboard.detail_state,
                DetailState::Loading { .. }
            ));
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("issue-detail-loading").is_none());
    assert!(visual.debug_bounds("issue-detail-description").is_some());
    assert!(visual.debug_bounds("issue-detail-error").is_some());
    cx.read_entity(&dashboard_entity, |dashboard, _| {
        assert!(matches!(dashboard.detail_state, DetailState::Error { .. }));
        let selected = dashboard
            .domain_issues
            .iter()
            .find(|candidate| candidate.id == issue_id)
            .expect("cached selected issue");
        assert!(selected.detail_loaded);
        assert_eq!(selected.description_text, None);
        assert_eq!(selected.rich_description, None);
        assert_eq!(
            detail_view_from_issue(selected).description,
            "No description supplied."
        );
    });
}

#[test]
fn detail_cache_policy_uses_persisted_description_only_for_quiet_refresh() {
    let mut issue = sample_issues().into_iter().next().expect("issue");
    issue.description_text = None;
    issue.rich_description = None;
    assert!(!issue_has_cached_detail(&issue));

    issue.detail_loaded = true;
    assert!(issue_has_cached_detail(&issue));
    assert_eq!(
        detail_view_from_issue(&issue).description,
        "No description supplied."
    );

    issue.detail_loaded = false;
    issue.description_text = Some("cached description".to_owned());
    assert!(issue_has_cached_detail(&issue));
    assert_eq!(
        detail_view_from_issue(&issue).description,
        "cached description"
    );

    issue.description_text = None;
    issue.rich_description = Some(jira_domain::RichTextDocument::new(vec![], false));
    assert!(issue_has_cached_detail(&issue));
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

    assert_eq!(
        issue_id_for_key_in_sources(&key, &issues, &[], None),
        expected
    );
}

#[test]
fn related_issue_source_precedence_is_domain_then_team_then_selected_core() {
    let domain = sample_issues();
    let team = sample_issues();
    let selected = sample_issues().into_iter().last().expect("selected issue");

    let domain_key = domain[0].key.clone();
    assert_eq!(
        issue_id_for_key_in_sources(&domain_key, &domain, &team, Some(&selected)),
        Some(domain[0].id.clone())
    );

    let team_only_key = IssueKey::new("TEAM-1").expect("team key");
    let mut team_only = team[0].clone();
    team_only.key = team_only_key.clone();
    assert_eq!(
        issue_id_for_key_in_sources(&team_only_key, &[], &[team_only.clone()], None),
        Some(team_only.id)
    );

    let selected_key = IssueKey::new("REMOTE-1").expect("selected key");
    let mut selected = selected;
    selected.key = selected_key.clone();
    assert_eq!(
        issue_id_for_key_in_sources(&selected_key, &[], &[], Some(&selected)),
        Some(selected.id.clone())
    );
    assert!(
        issue_id_for_key_in_sources(
            &IssueKey::new("MISSING-1").expect("missing key"),
            &domain,
            &team,
            Some(&selected)
        )
        .is_none()
    );
}

#[gpui_kit::test]
fn related_navigation_does_not_bypass_a_confirmed_write(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let dashboard = cx.new(|_| Dashboard::from_sample_data());
    cx.update_entity(&dashboard, |dashboard, _| {
        dashboard.search_query = "existing filter".to_owned();
        dashboard.issue_edit_flow.set_submitting_for_test(
            dashboard.domain_issues[0].id.clone(),
            "assignee Amina Yusuf".to_owned(),
        );
    });
    cx.update_entity(&dashboard, |dashboard, cx| {
        dashboard.open_related_issue_key(IssueKey::new("DESK-171").expect("parent key"), cx);
        assert_eq!(dashboard.search_query, "existing filter");
        assert_eq!(dashboard.remote_lookup, RemoteLookupState::Idle);
        assert!(dashboard.sync_message.contains("Finish the confirmed"));
    });
}

#[gpui_kit::test]
fn unknown_related_lookup_preserves_search_filter_when_workspace_is_unavailable(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let dashboard = cx.new(|_| Dashboard::from_sample_data());
    cx.update_entity(&dashboard, |dashboard, cx| {
        dashboard.search_query = "existing filter".to_owned();
        dashboard.open_related_issue_key(IssueKey::new("REMOTE-404").expect("remote key"), cx);
        assert_eq!(dashboard.search_query, "existing filter");
        assert!(matches!(
            dashboard.remote_lookup,
            RemoteLookupState::Error { ref query, .. } if query == "REMOTE-404"
        ));
    });
}

#[gpui_kit::test]
fn open_update_selects_issue_and_opens_desktop_detail(cx: &mut gpui_kit::TestAppContext) {
    let issue = sample_issues()
        .into_iter()
        .find(|issue| issue.key.as_str() == "DESK-176")
        .expect("issue");
    let dashboard = cx.new(|_| Dashboard::from_sample_data());

    cx.update_entity(&dashboard, |dashboard, cx| {
        dashboard.open_update_issue(issue.id.clone(), false, cx);
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

#[gpui_kit::test]
fn clicking_issue_row_keeps_selection_layout_bounded(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let window = cx.open_window(gpui_kit::size(px(1200.), px(900.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let issue = sample_issues()
        .into_iter()
        .find(|issue| issue.key.as_str() == "DESK-179")
        .expect("sample issue");
    let row = visual
        .debug_bounds("issue-row-10179")
        .expect("issue row should be laid out");
    assert!(row.size.width > px(0.));
    assert!(row.size.height > px(0.));

    visual.simulate_click(
        gpui_kit::point(
            row.origin.x + row.size.width / 2.,
            row.origin.y + row.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(
        dashboard_entity.read_with(&visual, |dashboard, _| dashboard.selected_issue.clone()),
        Some(issue.id.clone()),
        "clicking an issue row should still select it",
    );
    let selected_row = visual
        .debug_bounds("issue-row-10179")
        .expect("selected issue row should remain laid out");
    assert!(visual.debug_bounds("issue-detail").is_some());
    assert!(
        (f32::from(selected_row.size.height) - f32::from(row.size.height)).abs() <= 1.0,
        "opening details should not replace the compact row with a card: before={row:?}, after={selected_row:?}"
    );
    assert!(selected_row.size.width < row.size.width);
    assert!(selected_row.size.width <= row.size.width);
}

#[gpui_kit::test]
fn desktop_issue_overview_opens_and_closes_the_detail_panes(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let window = cx.open_window(gpui_kit::size(px(1200.), px(900.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("issues-toolbar").is_some());
    assert!(visual.debug_bounds("issues-table").is_some());
    assert!(visual.debug_bounds("issue-detail").is_none());

    let row = visual
        .debug_bounds("issue-row-10179")
        .expect("overview should render issue rows");
    let table = visual
        .debug_bounds("issues-table")
        .expect("overview table should be laid out");
    let row_left_inset = row.origin.x - table.origin.x;
    let row_right_inset = table.origin.x + table.size.width - row.origin.x - row.size.width;
    assert!(
        row_left_inset >= px(8.),
        "table row should have an 8px left inset: {row:?}"
    );
    assert!(
        row_right_inset >= px(8.),
        "table row should have an 8px right inset: row={row:?}, table={table:?}"
    );
    assert!(
        table.size.width - row.size.width >= px(16.),
        "table row should preserve at least 16px of combined horizontal inset: row={row:?}, table={table:?}"
    );
    assert!(
        row.size.height >= px(30.) && row.size.height <= px(45.),
        "overview row should stay compact: {row:?}"
    );

    let status_header = visual
        .debug_bounds("issues-column-status")
        .expect("status column header should be laid out");
    let status_cell = visual
        .debug_bounds("issue-cell-status-DESK-179")
        .expect("status column cell should be laid out");
    assert!(
        (f32::from(status_header.origin.x) - f32::from(status_cell.origin.x)).abs() <= 1.0,
        "status cell should align with its header: header={status_header:?}, cell={status_cell:?}"
    );

    visual.simulate_click(
        gpui_kit::point(
            row.origin.x + row.size.width / 2.,
            row.origin.y + row.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("issue-detail").is_some());
    assert!(visual.debug_bounds("issue-detail-back").is_some());
    assert!(visual.debug_bounds("issues-table").is_some());
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| dashboard.mobile_detail_open));

    let next_row = visual
        .debug_bounds("issue-row-10184")
        .expect("another issue row should remain available beside detail");
    visual.simulate_click(
        gpui_kit::point(
            next_row.origin.x + next_row.size.width / 2.,
            next_row.origin.y + next_row.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        dashboard_entity.read_with(&visual, |dashboard, _| dashboard.selected_issue.clone()),
        Some(IssueId::new("10184").expect("sample issue id")),
        "selecting another table row should update the issue detail selection"
    );

    let back = visual
        .debug_bounds("issue-detail-back")
        .expect("opened detail should offer a back action");
    visual.simulate_click(
        gpui_kit::point(
            back.origin.x + back.size.width / 2.,
            back.origin.y + back.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("issues-toolbar").is_some());
    assert!(visual.debug_bounds("issues-table").is_some());
    assert!(visual.debug_bounds("issue-detail").is_none());
    assert!(!dashboard_entity.read_with(&visual, |dashboard, _| dashboard.mobile_detail_open));
}

#[gpui_kit::test]
fn desktop_issue_table_and_detail_keep_usable_panes_at_breakpoints(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);

    for width in [720., 960., 1_200.] {
        let window = cx.open_window(gpui_kit::size(px(width), px(800.)), |_, _| {
            Dashboard::from_sample_data()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let row = visual
            .debug_bounds("issue-row-10179")
            .unwrap_or_else(|| panic!("DESK-179 row should be visible at {width}px"));
        visual.simulate_click(
            gpui_kit::point(
                row.origin.x + row.size.width / 2.,
                row.origin.y + row.size.height / 2.,
            ),
            Default::default(),
        );
        visual.run_until_parked();
        visual.update(|window, cx| window.draw(cx).clear(cx));

        let main = visual
            .debug_bounds("dashboard-main")
            .expect("dashboard main content should be laid out");
        let toolbar = visual
            .debug_bounds("issues-toolbar")
            .expect("toolbar should remain above both panes");
        let table = visual
            .debug_bounds("issues-table")
            .expect("table viewport should remain beside detail");
        let detail = visual
            .debug_bounds("issue-detail")
            .expect("selected issue detail should remain visible");
        let row = visual
            .debug_bounds("issue-row-10179")
            .expect("selected row should remain visible in the table");

        assert!(
            toolbar.origin.y < table.origin.y,
            "toolbar should remain above panes at {width}px"
        );
        assert!(
            table.origin.x >= main.origin.x
                && table.origin.x + table.size.width <= main.origin.x + main.size.width + px(1.),
            "table viewport should stay within dashboard content at {width}px: main={main:?}, table={table:?}"
        );
        assert!(
            detail.origin.x >= table.origin.x + table.size.width - px(1.),
            "table and detail should occupy separate horizontal regions at {width}px: table={table:?}, detail={detail:?}"
        );
        assert!(
            table.size.width >= px(280.) && detail.size.width >= px(280.),
            "both panes should retain a usable viewport at {width}px: table={table:?}, detail={detail:?}"
        );
        assert!(row.size.height >= px(30.) && row.size.height <= px(45.));
    }
}

#[gpui_kit::test]
fn desktop_issue_toolbar_fits_valid_key_and_clear_action_at_standard_width(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);

    let window = cx.open_window(gpui_kit::size(px(960.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.ensure_search_input(window, cx);
            let input = dashboard.search_input.clone().expect("issue search input");
            input.update(cx, |input, cx| input.set_value("DESK-179", window, cx));
            dashboard.set_search_query("DESK-179".to_owned(), cx);
            dashboard.set_status_filter(IssueStatusFilter::Done, cx);
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let toolbar = visual
        .debug_bounds("issues-toolbar")
        .expect("issues toolbar should be laid out");
    for selector in ["issue-search", "issue-search-submit", "issue-filters-clear"] {
        let control = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} should be laid out"));
        assert!(
            control.origin.x >= toolbar.origin.x
                && control.origin.y >= toolbar.origin.y
                && control.origin.x + control.size.width
                    <= toolbar.origin.x + toolbar.size.width + px(1.)
                && control.origin.y + control.size.height
                    <= toolbar.origin.y + toolbar.size.height + px(1.),
            "toolbar control should remain within the toolbar: selector={selector}, control={control:?}, toolbar={toolbar:?}"
        );
    }
}

#[gpui_kit::test]
fn desktop_issue_header_exposes_count_and_columns_at_compact_width(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);

    let window = cx.open_window(gpui_kit::size(px(720.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let summary = visual
        .debug_bounds("issue-list-summary")
        .expect("issue count should remain exposed to accessibility");
    let key = visual
        .debug_bounds("issues-column-key")
        .expect("Key column header should be laid out");
    assert!(
        summary.size.width > px(0.) && summary.size.height > px(0.),
        "issue count header should have visible geometry: {summary:?}"
    );
    assert!(
        key.origin.y >= summary.origin.y
            && key.origin.y + key.size.height <= summary.origin.y + summary.size.height + px(1.)
    );
}

#[gpui_kit::test]
fn dashboard_issue_edit_reads_and_dispatches_each_confirmed_operation_once(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
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
    let window = cx.open_window(gpui_kit::size(px(900.), px(700.)), |_, _| dashboard);
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

    cx.update_entity(&dashboard_entity, |dashboard, _cx| {
        dashboard.operation_in_progress = false;
        dashboard.issue_edit_flow.begin_transition_confirmation(
            issue_id.clone(),
            "IX-100".to_owned(),
            transition_options[0].clone(),
        );
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

#[gpui_kit::test]
fn team_tracker_table_and_detail_are_bounded_on_desktop(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Team;
    dashboard.team_members = vec![PersistedTeamMember {
        identifier: "amina".to_owned(),
        account_id: "amina".to_owned(),
        display_name: "Amina Yusuf".to_owned(),
    }];
    dashboard.team_issues = sample_issues();
    dashboard.team_feedback =
        TeamFeedback::Info("Team tracker refreshed · fetched 5 · displaying 3".to_owned());

    let window = cx.open_window(gpui_kit::size(px(1370.), px(900.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let main_bounds = visual
        .debug_bounds("dashboard-main")
        .expect("dashboard main should be laid out");
    let panes_bounds = visual
        .debug_bounds("team-panes")
        .expect("team panes should be laid out");
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
        panes_bounds.origin.x >= main_bounds.origin.x
            && panes_bounds.origin.y >= main_bounds.origin.y
            && panes_bounds.origin.x + panes_bounds.size.width
                <= main_bounds.origin.x + main_bounds.size.width + px(1.)
            && panes_bounds.origin.y + panes_bounds.size.height
                <= main_bounds.origin.y + main_bounds.size.height + px(1.),
        "team panes escape dashboard main: panes={panes_bounds:?}, main={main_bounds:?}"
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

    visual.simulate_resize(gpui_kit::size(px(390.), px(800.)));
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

#[gpui_kit::test]
fn native_settings_root_and_general_controls_are_bounded(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Settings;
    let window = cx.open_window(gpui_kit::size(px(960.), px(700.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let main_bounds = visual
        .debug_bounds("dashboard-main")
        .expect("dashboard main should be laid out");
    let settings_bounds = visual
        .debug_bounds("settings-root")
        .expect("native settings root should be laid out");
    assert!(
        settings_bounds.size.width > px(0.) && settings_bounds.size.height > px(0.),
        "native settings root collapsed: {settings_bounds:?}"
    );
    assert!(
        settings_bounds.origin.x >= main_bounds.origin.x
            && settings_bounds.origin.y >= main_bounds.origin.y
            && settings_bounds.origin.x + settings_bounds.size.width
                <= main_bounds.origin.x + main_bounds.size.width + px(1.)
            && settings_bounds.origin.y + settings_bounds.size.height
                <= main_bounds.origin.y + main_bounds.size.height + px(1.),
        "native settings root escapes dashboard main: settings={settings_bounds:?}, main={main_bounds:?}"
    );

    for control in ["appearance-system", "appearance-light", "appearance-dark"] {
        let bounds = visual
            .debug_bounds(control)
            .unwrap_or_else(|| panic!("default General page should expose {control}"));
        assert!(
            bounds.size.width > px(0.) && bounds.size.height > px(0.),
            "General control {control} collapsed: {bounds:?}"
        );
    }
}

#[gpui_kit::test]
fn mobile_settings_direct_navigation_selects_bounded_category_content(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Settings;
    let window = cx.open_window(gpui_kit::size(px(390.), px(800.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let root = visual
        .debug_bounds("settings-root")
        .expect("mobile settings root should be laid out");
    let navigation = visual
        .debug_bounds("settings-category-navigation")
        .expect("mobile settings should expose category navigation");
    let team = visual
        .debug_bounds("settings-category-team-tracker")
        .expect("Team tracker category should be directly visible");
    assert!(navigation.origin.x >= root.origin.x && navigation.origin.y >= root.origin.y);
    assert!(navigation.origin.x + navigation.size.width <= root.origin.x + root.size.width);
    assert!(team.origin.x >= navigation.origin.x && team.origin.y >= navigation.origin.y);
    assert!(team.origin.x + team.size.width <= navigation.origin.x + navigation.size.width);

    visual.simulate_click(
        gpui_kit::point(
            team.origin.x + team.size.width / 2.,
            team.origin.y + team.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(
        dashboard_entity.read_with(&visual, |dashboard, _| dashboard.settings_category),
        SettingsCategory::TeamTracker
    );
    let content = visual
        .debug_bounds("settings-content-team-tracker")
        .expect("selected Team tracker content should have stable identity");
    assert!(visual.debug_bounds("settings-content-appearance").is_none());
    assert!(content.size.width > px(0.) && content.size.height > px(0.));
    assert!(content.origin.x >= root.origin.x && content.origin.y >= root.origin.y);
    assert!(content.origin.x + content.size.width <= root.origin.x + root.size.width + px(1.));
    assert!(content.origin.y + content.size.height <= root.origin.y + root.size.height + px(1.));
    assert!(
        visual.debug_bounds("team-settings-feedback").is_none(),
        "navigation must not manufacture settings feedback"
    );
}

#[gpui_kit::test]
fn team_table_sort_keeps_selected_detail_identity_after_reordering(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Team;
    dashboard.team_members = vec![PersistedTeamMember {
        identifier: "amina".to_owned(),
        account_id: "amina".to_owned(),
        display_name: "Amina Yusuf".to_owned(),
    }];
    dashboard.team_issues = sample_issues();
    let window = cx.open_window(gpui_kit::size(px(1_370.), px(900.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let table = dashboard_entity
        .read_with(&visual, |dashboard, _| dashboard.team_table.clone())
        .expect("team table should be created by Team render");
    let selected_id = table.read_with(&visual, |table, _| {
        table
            .delegate()
            .issue_id_for_row(0)
            .expect("team table should have a first row")
    });

    visual.update(|_, cx| {
        table.update(cx, |table, cx| {
            table.set_selected_row(0, cx);
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        table.update(cx, |table, cx| {
            table
                .delegate_mut()
                .perform_sort(0, ColumnSort::Descending, window, cx);
        });
    });
    visual.run_until_parked();
    assert_eq!(
        table.read_with(&visual, |table, _| table.selected_row()),
        Some(2)
    );

    let selected_after_sort = table
        .read_with(&visual, |table, _| table.selected_team_ticket_issue_id())
        .expect("sorting should retain a selected row");
    assert_eq!(selected_after_sort, selected_id);
    assert_eq!(
        dashboard_entity.read_with(&visual, |dashboard, _| dashboard.selected_issue.clone()),
        Some(selected_id.clone())
    );

    let mut reordered_issues = sample_issues();
    reordered_issues.reverse();
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, _| {
            dashboard.team_issues = reordered_issues.clone()
        });
        table.update(cx, |table, cx| {
            table.replace_team_ticket_rows_with_offset(
                &reordered_issues,
                &[],
                &sample_users(),
                datetime!(2026-08-19 00:00 UTC),
                Some(UtcOffset::UTC),
                cx,
            );
        });
    });
    visual.run_until_parked();
    assert_eq!(
        table.read_with(&visual, |table, _| table.selected_team_ticket_issue_id()),
        Some(selected_id.clone())
    );

    let remaining_issues = sample_issues()
        .into_iter()
        .filter(|issue| issue.id != selected_id)
        .collect::<Vec<_>>();
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, _| {
            dashboard.team_issues = remaining_issues.clone()
        });
        table.update(cx, |table, cx| {
            table.replace_team_ticket_rows_with_offset(
                &remaining_issues,
                &[],
                &sample_users(),
                datetime!(2026-08-19 00:00 UTC),
                Some(UtcOffset::UTC),
                cx,
            );
        });
    });
    visual.run_until_parked();
    assert!(
        table
            .read_with(&visual, |table, _| table.selected_row())
            .is_none()
    );
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| dashboard.selected_issue.is_none()));
}

#[gpui_kit::test]
fn team_table_density_refresh_keeps_identity_after_hidden_sort_resets(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Team;
    dashboard.team_members = vec![PersistedTeamMember {
        identifier: "amina".to_owned(),
        account_id: "amina".to_owned(),
        display_name: "Amina Yusuf".to_owned(),
    }];
    dashboard.team_issues = sample_issues();
    let window = cx.open_window(gpui_kit::size(px(1_920.), px(900.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let table = dashboard_entity
        .read_with(&visual, |dashboard, _| dashboard.team_table.clone())
        .expect("team table should be created by Team render");
    let selected_id = table.read_with(&visual, |table, _| {
        table
            .delegate()
            .issue_id_for_row(0)
            .expect("team table should have a first row")
    });
    visual.update(|_, cx| {
        table.update(cx, |table, cx| table.set_selected_row(0, cx));
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        table.update(cx, |table, cx| {
            table
                .delegate_mut()
                .perform_sort(5, ColumnSort::Descending, window, cx);
        });
    });
    visual.run_until_parked();
    assert_eq!(
        table.read_with(&visual, |table, _| table.selected_team_ticket_issue_id()),
        Some(selected_id.clone())
    );
    assert_eq!(
        table.read_with(&visual, |table, _| table.selected_row()),
        Some(2)
    );

    visual.simulate_resize(gpui_kit::size(px(1_370.), px(900.)));
    visual.run_until_parked();
    assert_eq!(
        table.read_with(&visual, |table, _| table.selected_team_ticket_issue_id()),
        Some(selected_id)
    );
    assert_eq!(
        table.read_with(&visual, |table, _| table.selected_row()),
        Some(0)
    );
}

#[gpui_kit::test]
fn empty_issue_detail_status_stays_within_detail_pane(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Team;
    dashboard.selected_issue = None;
    dashboard.selected_issue_core = None;
    let window = cx.open_window(gpui_kit::size(px(960.), px(700.)), |_, _| dashboard);
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

#[gpui_kit::test]
fn native_issue_details_metadata_stays_bounded_at_desktop_width(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let mut issue = sample_issues().into_iter().next().expect("sample issue");
    issue.linked_issues.clear();
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Team;
    dashboard.team_members = vec![PersistedTeamMember {
        identifier: "amina".to_owned(),
        account_id: "amina".to_owned(),
        display_name: "Amina Yusuf".to_owned(),
    }];
    dashboard.domain_issues[0].linked_issues.clear();
    dashboard.team_issues = sample_issues();
    dashboard.team_issues[0].linked_issues.clear();
    dashboard.selected_issue = Some(issue.id.clone());
    dashboard.detail_state = DetailState::Loaded(IssueDetailViewModel {
        description: issue
            .description_text
            .clone()
            .unwrap_or_else(|| "No description supplied.".to_owned()),
        rich_description: issue.rich_description.clone(),
        comments: Vec::new(),
        attachments: Vec::new(),
    });
    let window = cx.open_window(gpui_kit::size(px(1370.), px(900.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let pane = visual
        .debug_bounds("team-detail")
        .expect("team detail pane should be laid out");
    let details = visual
        .debug_bounds("issue-detail-details")
        .expect("native details surface should be laid out");
    assert!(
        (f32::from(pane.size.width) - TEAM_DETAIL_INITIAL_WIDTH).abs() <= 1.,
        "team detail pane should retain its intended initial width: {pane:?}"
    );
    assert!(
        details.origin.x >= pane.origin.x
            && details.origin.y >= pane.origin.y
            && details.origin.x + details.size.width <= pane.origin.x + pane.size.width + px(1.)
            && details.origin.y + details.size.height <= pane.origin.y + pane.size.height + px(1.),
        "native details surface escapes the detail pane: pane={pane:?}, details={details:?}"
    );

    assert!(
        !dashboard_entity.read_with(&visual, |dashboard, _| dashboard.issue_details_open),
        "Details should begin collapsed"
    );
    assert!(visual.debug_bounds("issue-detail-type").is_none());

    let status_pill = visual
        .debug_bounds("issue-detail-status-pill")
        .expect("static status should be laid out in the header");
    let priority_surface = visual
        .debug_bounds("issue-detail-priority-surface")
        .expect("priority metadata surface should be laid out");
    let breadcrumb = visual
        .debug_bounds("issue-detail-breadcrumbs")
        .expect("key and status breadcrumb should be laid out");
    let key = visual
        .debug_bounds("issue-detail-key")
        .expect("selected issue key should be laid out");
    let summary = visual
        .debug_bounds("issue-detail-summary")
        .expect("issue summary should be laid out");
    let updated = visual
        .debug_bounds("issue-detail-updated-header")
        .expect("updated timestamp metadata should be laid out");
    let metadata_row = visual
        .debug_bounds("issue-detail-metadata-row")
        .expect("metadata row should be laid out");
    assert!(priority_surface.size.width > px(0.) && priority_surface.size.height > px(0.));
    assert!(
        priority_surface.origin.y >= metadata_row.origin.y
            && priority_surface.origin.y + priority_surface.size.height
                <= metadata_row.origin.y + metadata_row.size.height + px(1.),
        "priority surface escapes metadata row: row={metadata_row:?}, surface={priority_surface:?}"
    );
    assert!(
        status_pill.origin.x >= breadcrumb.origin.x
            && status_pill.origin.y >= breadcrumb.origin.y
            && status_pill.origin.x + status_pill.size.width
                <= breadcrumb.origin.x + breadcrumb.size.width + px(1.)
            && status_pill.origin.y + status_pill.size.height
                <= breadcrumb.origin.y + breadcrumb.size.height + px(1.),
        "static status escapes key/status breadcrumb: breadcrumb={breadcrumb:?}, status={status_pill:?}"
    );
    assert!(
        key.origin.y >= breadcrumb.origin.y
            && key.origin.y + key.size.height
                <= breadcrumb.origin.y + breadcrumb.size.height + px(1.)
    );
    assert!(
        summary.size.height > priority_surface.size.height,
        "summary should dominate compact metadata: summary={summary:?}, metadata={priority_surface:?}"
    );
    assert!(
        updated.origin.y >= metadata_row.origin.y
            && updated.origin.y + updated.size.height
                <= metadata_row.origin.y + metadata_row.size.height + px(1.),
        "updated timestamp escapes metadata row: row={metadata_row:?}, updated={updated:?}"
    );

    let trigger = visual
        .debug_bounds("issue-detail-details-trigger")
        .expect("details accordion trigger should be laid out");
    visual.simulate_click(
        gpui_kit::point(
            trigger.origin.x + trigger.size.width / 2.,
            trigger.origin.y + trigger.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(300));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| dashboard.issue_details_open));
    let expanded_details = visual
        .debug_bounds("issue-detail-details")
        .expect("expanded Details group should be laid out");

    for selector in [
        "issue-detail-type",
        "issue-detail-assignee",
        "issue-detail-reporter",
        "issue-detail-status-category",
        "issue-detail-parent",
        "issue-detail-created",
        "issue-detail-updated",
        "issue-detail-due-date",
    ] {
        let value = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("expanded metadata value should be laid out: {selector}"));
        assert!(value.size.width > px(0.) && value.size.height > px(0.));
        assert!(
            value.origin.x >= expanded_details.origin.x
                && value.origin.y >= expanded_details.origin.y
                && value.origin.x + value.size.width
                    <= expanded_details.origin.x + expanded_details.size.width + px(1.)
                && value.origin.y + value.size.height
                    <= expanded_details.origin.y + expanded_details.size.height + px(1.),
            "metadata value escapes native details surface: selector={selector}, details={expanded_details:?}, value={value:?}"
        );
    }
    let action_bar = visual
        .debug_bounds("issue-detail-action-bar")
        .expect("status and comment controls should be in the action toolbar");
    let status_trigger = visual
        .debug_bounds("issue-status-trigger")
        .expect("editable status control should be in the action toolbar");
    assert!(
        status_trigger.origin.x >= action_bar.origin.x
            && status_trigger.origin.y >= action_bar.origin.y
            && status_trigger.origin.x + status_trigger.size.width
                <= action_bar.origin.x + action_bar.size.width + px(1.)
            && status_trigger.origin.y + status_trigger.size.height
                <= action_bar.origin.y + action_bar.size.height + px(1.),
        "editable status control escapes the action toolbar: toolbar={action_bar:?}, status={status_trigger:?}"
    );

    let trigger = visual
        .debug_bounds("issue-detail-details-trigger")
        .expect("details accordion trigger should remain laid out when expanded");
    visual.simulate_click(
        gpui_kit::point(
            trigger.origin.x + trigger.size.width / 2.,
            trigger.origin.y + trigger.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    assert!(!dashboard_entity.read_with(&visual, |dashboard, _| dashboard.issue_details_open));
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let trigger = visual
        .debug_bounds("issue-detail-details-trigger")
        .expect("details accordion trigger should remain laid out when collapsed");
    visual.simulate_click(
        gpui_kit::point(
            trigger.origin.x + trigger.size.width / 2.,
            trigger.origin.y + trigger.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| dashboard.issue_details_open));
}

#[gpui_kit::test]
fn idle_comment_action_is_intrinsic_and_bounded_inside_composer(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.mobile_detail_open = true;
    dashboard.detail_state = DetailState::Loaded(detail_view_from_issue(&issue));
    let window = cx.open_window(gpui_kit::size(px(1_200.), px(1_200.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.ensure_comment_input(window, cx);
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let detail = visual
        .debug_bounds("issue-detail")
        .expect("issue detail should be laid out");
    let composer = visual
        .debug_bounds("comment-composer")
        .expect("comment composer should be laid out");
    let description = visual
        .debug_bounds("issue-detail-description")
        .expect("issue description should be laid out");
    let actions = visual
        .debug_bounds("comment-composer-actions")
        .expect("comment action row should be laid out");
    let add_comment = visual
        .debug_bounds("issue-detail-add-comment")
        .expect("Add comment action should be laid out");
    let post_comment = visual
        .debug_bounds("post-comment")
        .expect("post comment button should be laid out");

    assert!(post_comment.size.width > px(0.));
    assert!(post_comment.size.height > px(0.));
    assert!(
        post_comment.size.width < composer.size.width * 0.6,
        "idle post-comment should retain intrinsic width: button={post_comment:?}, composer={composer:?}"
    );
    assert!(
        actions.origin.x >= composer.origin.x
            && actions.origin.y >= composer.origin.y
            && actions.origin.x + actions.size.width
                <= composer.origin.x + composer.size.width + px(1.)
            && actions.origin.y + actions.size.height
                <= composer.origin.y + composer.size.height + px(1.),
        "comment action row should remain inside composer: actions={actions:?}, composer={composer:?}"
    );
    assert!(
        post_comment.origin.x >= actions.origin.x
            && post_comment.origin.y >= actions.origin.y
            && post_comment.origin.x + post_comment.size.width
                <= actions.origin.x + actions.size.width + px(1.)
            && post_comment.origin.y + post_comment.size.height
                <= actions.origin.y + actions.size.height + px(1.),
        "post comment should remain inside action row: button={post_comment:?}, actions={actions:?}"
    );
    assert!(
        composer.origin.x >= detail.origin.x
            && composer.origin.y >= detail.origin.y
            && composer.origin.x + composer.size.width
                <= detail.origin.x + detail.size.width + px(1.)
            && composer.origin.y + composer.size.height
                <= detail.origin.y + detail.size.height + px(1.),
        "comment composer should remain inside issue detail: composer={composer:?}, detail={detail:?}"
    );
    assert!(
        composer.origin.y >= description.origin.y + description.size.height,
        "comment composer should not overlap the description: description={description:?}, composer={composer:?}"
    );

    visual.simulate_click(
        gpui_kit::point(
            add_comment.origin.x + add_comment.size.width / 2.,
            add_comment.origin.y + add_comment.size.height / 2.,
        ),
        Default::default(),
    );
    visual.simulate_keystrokes("fixture");
    visual.run_until_parked();
    assert_eq!(
        dashboard_entity.read_with(&visual, |dashboard, cx| {
            dashboard
                .comment_input
                .as_ref()
                .map(|input| input.read(cx).value().to_string())
        }),
        Some("fixture".to_owned()),
        "Add comment should focus the comment composer for direct typing"
    );
}

#[gpui_kit::test]
fn mobile_comment_confirmation_actions_are_intrinsic_and_non_overlapping(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.detail_state = DetailState::Loaded(detail_view_from_issue(&issue));
    dashboard.mobile_detail_open = true;
    dashboard
        .comment_flow
        .begin_confirmation(
            CommentTarget {
                issue_id: issue.id.clone(),
                issue_key: issue.key.as_str().to_owned(),
            },
            "A fixture comment requiring confirmation",
        )
        .expect("fixture comment should enter confirmation");
    let window = cx.open_window(gpui_kit::size(px(640.), px(900.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.ensure_comment_input(window, cx);
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let composer = visual
        .debug_bounds("comment-composer")
        .expect("comment composer should be laid out");
    let actions = visual
        .debug_bounds("comment-composer-actions")
        .expect("comment action row should be laid out");
    let post_now = visual
        .debug_bounds("post-comment-now")
        .expect("Post now button should be laid out");
    let cancel = visual
        .debug_bounds("cancel-comment")
        .expect("Cancel button should be laid out");

    assert!(post_now.size.width > px(0.));
    assert!(cancel.size.width > px(0.));
    assert!(post_now.size.height > px(0.));
    assert!(cancel.size.height > px(0.));
    assert!(
        post_now.size.width < actions.size.width * 0.6,
        "Post now should retain intrinsic width on mobile: button={post_now:?}, actions={actions:?}"
    );
    assert!(
        cancel.size.width < actions.size.width * 0.6,
        "Cancel should retain intrinsic width on mobile: button={cancel:?}, actions={actions:?}"
    );
    assert!(
        actions.origin.x >= composer.origin.x
            && actions.origin.y >= composer.origin.y
            && actions.origin.x + actions.size.width
                <= composer.origin.x + composer.size.width + px(1.)
            && actions.origin.y + actions.size.height
                <= composer.origin.y + composer.size.height + px(1.),
        "mobile comment action row should remain inside composer: actions={actions:?}, composer={composer:?}"
    );
    for (name, button) in [("Post now", post_now), ("Cancel", cancel)] {
        assert!(
            button.origin.x >= actions.origin.x
                && button.origin.y >= actions.origin.y
                && button.origin.x + button.size.width
                    <= actions.origin.x + actions.size.width + px(1.)
                && button.origin.y + button.size.height
                    <= actions.origin.y + actions.size.height + px(1.),
            "{name} should remain inside mobile action row: button={button:?}, actions={actions:?}"
        );
    }
    assert!(
        (f32::from(post_now.origin.y) - f32::from(cancel.origin.y)).abs() <= 1.,
        "confirmation actions should share a row at narrow width: post={post_now:?}, cancel={cancel:?}"
    );
    assert!(
        post_now.origin.x + post_now.size.width <= cancel.origin.x,
        "horizontal confirmation actions should not overlap: post={post_now:?}, cancel={cancel:?}"
    );
    assert!(
        (f32::from(cancel.origin.x + cancel.size.width)
            - f32::from(actions.origin.x + actions.size.width))
        .abs()
            <= 1.,
        "confirmation action group should be trailing-aligned: cancel={cancel:?}, actions={actions:?}"
    );
}

#[gpui_kit::test]
fn mobile_remote_lookup_loading_and_error_states_stay_visible_in_the_list(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.remote_lookup = RemoteLookupState::Loading {
        query: " ix-404 ".to_owned(),
    };
    let window = cx.open_window(gpui_kit::size(px(320.), px(700.)), |_, _| dashboard);
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

#[gpui_kit::test]
fn selected_detail_feedback_is_early_and_has_stable_state_identity(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let issue = sample_issues().into_iter().next().expect("sample issue");
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.selected_issue = Some(issue.id.clone());
    dashboard.mobile_detail_open = true;
    dashboard.detail_state = DetailState::Loading {
        issue_id: issue.id.clone(),
    };
    let window = cx.open_window(gpui_kit::size(px(320.), px(700.)), |_, _| dashboard);
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

#[gpui_kit::test]
fn mobile_updates_preview_explains_missing_cached_issue_details(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Updates;
    let issue_id = dashboard
        .update_groups
        .iter()
        .find(|group| group.issue_key == "DESK-179")
        .expect("DESK-179 update group")
        .issue_id
        .clone();
    let issue = dashboard
        .domain_issues
        .iter_mut()
        .find(|issue| issue.id == issue_id)
        .expect("cached DESK-179 issue fixture");
    issue.description_text = None;
    issue.rich_description = None;
    issue.detail_loaded = false;
    dashboard.selected_update_issue = Some(issue_id);
    dashboard.mobile_update_detail_open = true;

    let window = cx.open_window(gpui_kit::size(px(390.), px(800.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("issue-detail").is_some());
    assert!(visual.debug_bounds("issue-detail-description").is_some());
    assert!(visual.debug_bounds("update-issue-unavailable").is_some());
    assert!(visual.debug_bounds("update-reading-activity").is_some());
    assert!(visual.debug_bounds("update-open-full-details").is_some());
}

#[gpui_kit::test]
fn mobile_updates_preview_uses_bounded_read_only_issue_overview(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Updates;
    dashboard.mobile_update_detail_open = true;
    let window = cx.open_window(gpui_kit::size(px(390.), px(800.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let pane = visual
        .debug_bounds("update-reading-pane")
        .expect("selected mobile update should show its reading pane");
    let overview = visual
        .debug_bounds("issue-detail")
        .expect("reading pane should use the shared issue overview");
    let summary = visual
        .debug_bounds("issue-detail-summary")
        .expect("issue summary should be laid out");
    let metadata = visual
        .debug_bounds("issue-detail-metadata-row")
        .expect("issue metadata should be laid out");
    let description = visual
        .debug_bounds("issue-detail-description")
        .expect("cached issue description should be laid out");
    let action_bar = visual
        .debug_bounds("issue-detail-action-bar")
        .expect("read-only issue actions should be laid out");
    let watch = visual
        .debug_bounds("issue-watch-button")
        .expect("watch action should be laid out");
    let add_comment = visual
        .debug_bounds("issue-detail-add-comment")
        .expect("Add comment action should be laid out");
    let change_status = visual
        .debug_bounds("issue-detail-change-status")
        .expect("Change status action should be laid out");
    let activity = visual
        .debug_bounds("update-reading-activity")
        .expect("local activity timeline should follow the issue overview");
    let open_full_details = visual
        .debug_bounds("update-open-full-details")
        .expect("full details navigation should remain available");

    assert!(overview.size.width > px(0.) && overview.size.height > px(0.));
    assert!(overview.origin.x >= pane.origin.x && overview.origin.y >= pane.origin.y);
    assert!(overview.origin.x + overview.size.width <= pane.origin.x + pane.size.width + px(1.));
    for (name, bounds) in [
        ("summary", summary),
        ("metadata", metadata),
        ("description", description),
        ("action bar", action_bar),
        ("watch", watch),
        ("Add comment", add_comment),
        ("Change status", change_status),
    ] {
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        assert!(
            bounds.origin.x >= overview.origin.x
                && bounds.origin.x + bounds.size.width
                    <= overview.origin.x + overview.size.width + px(1.),
            "{name} should stay inside the mobile issue overview: overview={overview:?}, bounds={bounds:?}"
        );
    }
    assert!(activity.origin.y >= overview.origin.y + overview.size.height - px(1.));
    assert!(open_full_details.origin.x >= pane.origin.x);
    assert!(
        open_full_details.origin.x + open_full_details.size.width
            <= pane.origin.x + pane.size.width + px(1.)
    );
    assert!(visual.debug_bounds("issue-status-trigger").is_none());
    assert!(visual.debug_bounds("comment-composer").is_none());
    assert!(visual.debug_bounds("update-issue-unavailable").is_none());
    assert!(visual.debug_bounds("updates-reading-back").is_some());
}

#[cfg(feature = "ui-automation")]
#[gpui_kit::test]
fn issue_watch_fixture_confirms_locally_and_cancels_without_dispatch(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);

    let window = cx.open_window(gpui_kit::size(px(1_200.), px(900.)), |_, _| {
        Dashboard::from_ui_automation_issue_watch()
    });
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let initial = dashboard_entity.read_with(&visual, |dashboard, _| {
        (
            dashboard.selected_issue.clone(),
            dashboard.watch_flow.state().clone(),
        )
    });
    let (issue_id, initial_state) = initial;
    assert!(matches!(
        initial_state,
        WatchState::Ready {
            watching: false,
            ..
        }
    ));
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| dashboard.workspace.is_none()));

    let watch = visual
        .debug_bounds("issue-watch-button")
        .expect("Watch action should be available in the local fixture");
    visual.simulate_click(
        gpui_kit::point(
            watch.origin.x + watch.size.width / 2.,
            watch.origin.y + watch.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let confirmation = visual
        .debug_bounds("issue-watch-confirmation")
        .expect("confirming Watch should open the scoped confirmation surface");
    let detail = visual
        .debug_bounds("issue-detail")
        .expect("the selected issue detail should remain visible during confirmation");
    let confirm = visual
        .debug_bounds("issue-watch-confirm")
        .expect("confirmation should offer a disabled Confirm action in the fixture");
    let cancel = visual
        .debug_bounds("issue-watch-cancel")
        .expect("confirmation should offer Cancel");
    for (name, bounds) in [
        ("confirmation", confirmation),
        ("Confirm", confirm),
        ("Cancel", cancel),
    ] {
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        assert!(
            bounds.origin.x >= px(0.)
                && bounds.origin.y >= px(0.)
                && bounds.origin.x + bounds.size.width <= px(1_200.) + px(1.)
                && bounds.origin.y + bounds.size.height <= px(900.) + px(1.),
            "{name} should stay inside the local fixture window: {bounds:?}"
        );
    }
    assert!(
        confirmation.origin.x >= detail.origin.x
            && confirmation.origin.y >= detail.origin.y
            && confirmation.origin.x + confirmation.size.width
                <= detail.origin.x + detail.size.width + px(1.)
            && confirmation.origin.y + confirmation.size.height
                <= detail.origin.y + detail.size.height + px(1.),
        "confirmation should stay inside the issue detail: detail={detail:?}, confirmation={confirmation:?}"
    );
    assert!(
        confirmation.origin.x <= confirm.origin.x
            && confirmation.origin.y <= confirm.origin.y
            && confirm.origin.x + confirm.size.width
                <= confirmation.origin.x + confirmation.size.width + px(1.)
            && confirm.origin.y + confirm.size.height
                <= confirmation.origin.y + confirmation.size.height + px(1.)
    );
    assert!(
        confirmation.origin.x <= cancel.origin.x
            && confirmation.origin.y <= cancel.origin.y
            && cancel.origin.x + cancel.size.width
                <= confirmation.origin.x + confirmation.size.width + px(1.)
            && cancel.origin.y + cancel.size.height
                <= confirmation.origin.y + confirmation.size.height + px(1.)
    );
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| matches!(
        dashboard.watch_flow.state(),
        WatchState::Confirming {
            issue_id: confirming_id,
            target_watching: true,
            ..
        } if Some(confirming_id) == issue_id.as_ref()
    )));

    // The fixture has no workspace, so Confirm is disabled and must not create a write task.
    visual.simulate_click(
        gpui_kit::point(
            confirm.origin.x + confirm.size.width / 2.,
            confirm.origin.y + confirm.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| {
        dashboard.workspace.is_none()
            && dashboard.watch_write_task.is_none()
            && matches!(
                dashboard.watch_flow.state(),
                WatchState::Confirming {
                    issue_id: confirming_id,
                    target_watching: true,
                    ..
                } if Some(confirming_id) == issue_id.as_ref()
            )
    }));

    visual.simulate_click(
        gpui_kit::point(
            cancel.origin.x + cancel.size.width / 2.,
            cancel.origin.y + cancel.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    assert!(dashboard_entity.read_with(&visual, |dashboard, _| matches!(
        dashboard.watch_flow.state(),
        WatchState::Ready {
            issue_id: ready_id,
            watching: false,
            ..
        } if Some(ready_id) == issue_id.as_ref()
    )));
    assert!(
        dashboard_entity.read_with(&visual, |dashboard, _| dashboard.watch_write_task.is_none())
    );
    assert!(visual.debug_bounds("issue-watch-feedback").is_none());
}

#[gpui_kit::test]
fn update_card_keeps_issue_key_visible_at_compact_desktop_width(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

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

    let window = cx.open_window(gpui_kit::size(px(1095.), px(700.)), |_, _| dashboard);
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
    let dot_bounds = visual
        .debug_bounds("update-unread-dot-0")
        .expect("unread indicator should be laid out");
    let metadata_bounds = visual
        .debug_bounds("update-metadata-0")
        .expect("update metadata row should be laid out");
    let actions_bounds = visual
        .debug_bounds("update-actions-0")
        .expect("update actions should be laid out");
    // GPUI debug bounds describe the pre-paint flow box, while the native macOS
    // AX frame includes the component spacing margin. Keep the product-level
    // midline assertion in the local XCUITest, and verify this renderer keeps
    // the marker and metadata as non-collapsed, bounded layout surfaces here.
    assert_eq!(dot_bounds.size.width, px(8.));
    assert_eq!(dot_bounds.size.height, px(8.));
    assert!(
        metadata_bounds.size.height > px(1.),
        "update metadata collapsed: {metadata_bounds:?}"
    );
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

#[gpui_kit::test]
fn updates_mobile_header_and_card_fit_the_supported_minimum(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let issue_id = IssueId::new("100").expect("issue");
    let event_id = jira_domain::EventId::new("event-mobile").expect("event");
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Updates;
    dashboard.update_groups = vec![UpdateGroupViewModel {
        issue_id: issue_id.clone(),
        issue_key: "PLATFORM-12345".to_owned(),
        issue_summary: "A deliberately long summary that must remain readable without escaping the narrow mobile card".to_owned(),
        latest_occurred_at: "2026-08-23 14:35:27 Asia/Kolkata".to_owned(),
        unread_count: 1,
        unread: true,
        events: vec![UpdateViewModel {
            event_id,
            issue_id,
            issue_key: "PLATFORM-12345".to_owned(),
            issue_summary: "A deliberately long summary".to_owned(),
            change: "Status changed to In Progress with a long activity description".to_owned(),
            occurred_at: "2026-08-23 14:35:27 Asia/Kolkata".to_owned(),
            unread: true,
        }],
    }];

    let window = cx.open_window(gpui_kit::size(px(320.), px(700.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    for id in [
        "updates-header",
        "updates-heading",
        "updates-filters",
        "updates-description",
        "update-list",
        "update-card-0",
        "update-actions-0",
    ] {
        let bounds = visual
            .debug_bounds(id)
            .unwrap_or_else(|| panic!("{id} should be laid out"));
        assert!(
            bounds.origin.x >= px(0.),
            "{id} escapes left edge: {bounds:?}"
        );
        assert!(
            bounds.origin.x + bounds.size.width <= px(320.) + px(1.),
            "{id} escapes right edge: {bounds:?}"
        );
    }

    let header = visual
        .debug_bounds("updates-header")
        .expect("updates header should be laid out");
    let filters = visual
        .debug_bounds("updates-filters")
        .expect("updates filters should be laid out");
    let description = visual
        .debug_bounds("updates-description")
        .expect("updates description should be laid out");
    let card = visual
        .debug_bounds("update-card-0")
        .expect("collapsed update card should be laid out");
    let actions = visual
        .debug_bounds("update-actions-0")
        .expect("collapsed update actions should be laid out");
    let expand = visual
        .debug_bounds("update-group-expand-0")
        .expect("collapsed activity group should expose its expand control");
    assert!(
        visual.debug_bounds("update-row-0-0").is_none(),
        "event rows should be collapsed until the group is expanded"
    );
    assert!(filters.origin.y > header.origin.y);
    assert!(description.origin.y > filters.origin.y);
    assert!(
        description.origin.y + description.size.height
            <= header.origin.y + header.size.height + px(1.),
        "updates description collides with or escapes its header: header={header:?}, description={description:?}"
    );

    assert!(
        expand.origin.x >= actions.origin.x
            && expand.origin.y >= actions.origin.y
            && expand.origin.x + expand.size.width
                <= actions.origin.x + actions.size.width + px(1.)
            && expand.origin.y + expand.size.height
                <= actions.origin.y + actions.size.height + px(1.),
        "expand control escapes its action row: actions={actions:?}, expand={expand:?}"
    );
    // Expand through the visible control so the event geometry below covers
    // the actual mobile interaction path after actions move beneath the summary.
    visual.simulate_click(
        gpui_kit::point(
            expand.origin.x + expand.size.width / 2.,
            expand.origin.y + expand.size.height / 2.,
        ),
        Default::default(),
    );
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let event = visual
        .debug_bounds("update-row-0-0")
        .expect("expanded activity event should be laid out");
    let expanded_card = visual
        .debug_bounds("update-card-0")
        .expect("expanded update card should be laid out");
    assert!(
        event.size.width > px(0.)
            && event.size.height > px(0.)
            && event.origin.x >= expanded_card.origin.x
            && event.origin.y >= expanded_card.origin.y
            && event.origin.x + event.size.width
                <= expanded_card.origin.x + expanded_card.size.width + px(1.)
            && event.origin.y + event.size.height
                <= expanded_card.origin.y + expanded_card.size.height + px(1.),
        "expanded mobile activity event escapes its card: card={expanded_card:?}, event={event:?}"
    );
    assert!(
        actions.origin.x >= card.origin.x,
        "group actions should remain within the collapsed card: card={card:?}, actions={actions:?}"
    );
}

#[gpui_kit::test]
fn updates_desktop_header_rows_stay_separated_and_bounded(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Updates;
    let window = cx.open_window(gpui_kit::size(px(1370.), px(700.)), |_, _| dashboard);
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    let header = visual
        .debug_bounds("updates-header")
        .expect("updates header should be laid out");
    let heading = visual
        .debug_bounds("updates-heading")
        .expect("updates heading should be laid out");
    let filters = visual
        .debug_bounds("updates-filters")
        .expect("updates filters should be laid out");
    assert!(
        visual.debug_bounds("updates-description").is_none(),
        "the ticket and unread summary is reserved for the mobile header"
    );

    let heading_center = heading.origin.y + heading.size.height / 2.;
    let filters_center = filters.origin.y + filters.size.height / 2.;
    assert!(
        (f32::from(heading_center) - f32::from(filters_center)).abs() <= 1.,
        "desktop heading and toolbar should align vertically: heading={heading:?}, filters={filters:?}"
    );
    for selector in [
        "inbox-search",
        "inbox-status-filter",
        "inbox-refresh",
        "mark-all-read",
    ] {
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("desktop toolbar control should be laid out: {selector}"));
        assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
        assert!(
            bounds.origin.x >= filters.origin.x
                && bounds.origin.x + bounds.size.width
                    <= filters.origin.x + filters.size.width + px(1.)
                && bounds.origin.y >= filters.origin.y
                && bounds.origin.y + bounds.size.height
                    <= filters.origin.y + filters.size.height + px(1.),
            "toolbar control escapes filters row: row={filters:?}, selector={selector}, bounds={bounds:?}"
        );
        let control_center = bounds.origin.y + bounds.size.height / 2.;
        assert!(
            (f32::from(control_center) - f32::from(filters_center)).abs() <= 2.,
            "desktop toolbar control should remain on the single row: row={filters:?}, selector={selector}, bounds={bounds:?}"
        );
    }
    for (name, bounds) in [("heading", heading), ("filters", filters)] {
        assert!(
            bounds.origin.x >= header.origin.x
                && bounds.origin.x + bounds.size.width
                    <= header.origin.x + header.size.width + px(1.)
                && bounds.origin.y + bounds.size.height
                    <= header.origin.y + header.size.height + px(1.),
            "updates {name} escapes header: header={header:?}, bounds={bounds:?}"
        );
    }
}

#[gpui_kit::test]
fn updates_empty_filters_have_distinct_stable_state_surfaces(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Updates;
    dashboard.update_groups.clear();
    let window = cx.open_window(gpui_kit::size(px(320.), px(700.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("updates-empty-all").is_some());
    assert!(visual.debug_bounds("updates-empty-unread").is_none());

    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.update_filter = UpdateFilter::Unread;
            cx.notify();
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("updates-empty-all").is_none());
    assert!(visual.debug_bounds("updates-empty-unread").is_some());
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
            let constraints = team_pane_constraints(
                TEAM_DETAIL_INITIAL_WIDTH,
                width,
                layout,
                mode,
                sidebar_collapsed,
            );
            assert!(constraints.available_width >= 0.);
            assert!(constraints.table_min_width >= 0.);
            assert!(
                constraints.table_min_width + constraints.detail_min_width
                    <= constraints.available_width + 0.01
            );
            assert!(constraints.detail_min_width <= constraints.detail_max_width + 0.01);
            assert!(constraints.detail_max_width <= constraints.available_width / 2. + 0.01);
            assert!(
                constraints.table_max_width + constraints.detail_min_width
                    <= constraints.available_width + 0.01
            );
            if width == 1_200. && !sidebar_collapsed {
                assert_eq!(constraints.initial_detail_width, 324.);
            }
            if width == 1_370. {
                assert_eq!(constraints.initial_detail_width, TEAM_DETAIL_INITIAL_WIDTH);
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
        keystroke: gpui_kit::Keystroke::parse(key).expect("keystroke"),
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
        &[],
        None,
        &users,
    )
    .expect("filtered selected issue header");

    assert_eq!(view.id, selected.id);
    assert_eq!(view.key, selected.key.to_string());
}

#[test]
fn selected_issue_lookup_prefers_visible_view_then_domain_team_and_core() {
    let mut domain = sample_issues().into_iter().next().expect("issue");
    let mut team = domain.clone();
    let mut core = domain.clone();
    let selected = domain.id.clone();
    domain.summary = "domain copy".to_owned();
    team.summary = "team copy".to_owned();
    core.summary = "core copy".to_owned();
    let users = sample_users();

    assert_eq!(
        selected_issue_from_sources(
            Some(&selected),
            &[domain.clone()],
            &[team.clone()],
            Some(&core)
        )
        .map(|issue| issue.summary.as_str()),
        Some("domain copy")
    );
    assert_eq!(
        selected_issue_from_sources(Some(&selected), &[], &[team.clone()], Some(&core))
            .map(|issue| issue.summary.as_str()),
        Some("team copy")
    );
    assert_eq!(
        selected_issue_from_sources(Some(&selected), &[], &[], Some(&core))
            .map(|issue| issue.summary.as_str()),
        Some("core copy")
    );

    let mut visible = vec![IssueViewModel::from_domain(&domain, &users)];
    visible[0].summary = "visible copy".to_owned();
    let view = selected_issue_view_from_sources(
        Some(&selected),
        &visible,
        &[domain],
        &[team],
        Some(&core),
        &users,
    )
    .expect("selected view");
    assert_eq!(view.summary, "visible copy");
}

#[test]
fn fetched_detail_updates_matching_domain_and_team_snapshots() {
    let issue = sample_issues().into_iter().next().expect("issue");
    let mut fetched = issue.clone();
    fetched.description_text = Some("Fetched detailed description".to_owned());
    fetched.detail_loaded = true;
    let mut domain_issues = vec![issue.clone()];
    let mut team_issues = vec![issue];

    assert!(update_issue_snapshots(
        &fetched,
        &mut domain_issues,
        &mut team_issues,
    ));
    assert_eq!(domain_issues, vec![fetched.clone()]);
    assert_eq!(team_issues, vec![fetched]);
}

#[test]
fn cached_detail_resolver_finds_team_copy_when_primary_copy_has_no_detail() {
    let mut domain_issue = sample_issues().into_iter().next().expect("issue");
    let mut team_issue = domain_issue.clone();
    domain_issue.description_text = None;
    domain_issue.rich_description = None;
    domain_issue.detail_loaded = false;
    team_issue.description_text = Some("Team-only cached detail".to_owned());
    team_issue.detail_loaded = true;
    let issue_id = team_issue.id.clone();
    let mut dashboard = Dashboard::from_sample_data();
    dashboard.domain_issues = vec![domain_issue];
    dashboard.team_issues = vec![team_issue];
    dashboard.selected_issue = Some(issue_id.clone());
    dashboard.selected_issue_core = None;

    let resolved = dashboard
        .selected_issue_with_cached_detail(Some(&issue_id))
        .expect("Team copy should provide cached detail");
    assert_eq!(
        resolved.description_text.as_deref(),
        Some("Team-only cached detail")
    );
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

#[gpui_kit::test]
fn clearing_idle_remote_lookup_preserves_scroll_but_active_lookup_resets_it(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(900.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|_, cx| {
        dashboard.update(cx, |dashboard, _| {
            let mut offset = dashboard.issues_scroll_handle.base_handle().offset();
            offset.y = px(240.);
            dashboard
                .issues_scroll_handle
                .base_handle()
                .set_offset(offset);
            dashboard.clear_remote_lookup();
            assert_eq!(
                dashboard.issues_scroll_handle.base_handle().offset().y,
                px(240.)
            );
            dashboard.remote_lookup = RemoteLookupState::Loading {
                query: "IX-240".to_owned(),
            };
            dashboard.clear_remote_lookup();
            assert_eq!(
                dashboard.issues_scroll_handle.base_handle().offset().y,
                px(0.)
            );
        });
    });
}

#[gpui_kit::test]
fn assignee_list_uses_fetched_subset_and_rebuilds_after_refresh(cx: &mut gpui_kit::TestAppContext) {
    cx.update(gpui_kit::component::init);
    let window = cx.open_window(gpui_kit::size(px(900.), px(700.)), |_, _| {
        Dashboard::from_sample_data()
    });
    let dashboard = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        dashboard.update(cx, |dashboard, cx| {
            let issue = dashboard
                .selected_issue_view()
                .expect("selected issue")
                .clone();
            let fetched = vec![sample_users().into_iter().next().expect("fixture user")];
            let generation = dashboard
                .issue_edit_flow
                .begin_assignee_loading(issue.id.clone(), "cached".to_owned());
            assert!(dashboard.issue_edit_flow.finish_assignee_loading(
                Some(&issue.id),
                issue.id.clone(),
                issue.key.clone(),
                "cached".to_owned(),
                generation,
                Ok(fetched.clone()),
            ));
            dashboard.ensure_assignee_list(window, cx);
            let list = dashboard.assignee_list.as_ref().expect("assignee list");
            assert_eq!(list.read(cx).delegate().users.len(), 1);
            dashboard.assignee_list = None;
            dashboard.ensure_assignee_list(window, cx);
            let rebuilt = dashboard
                .assignee_list
                .as_ref()
                .expect("rebuilt assignee list");
            assert_eq!(rebuilt.read(cx).delegate().users.len(), 1);
        });
    });
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
fn team_feedback_is_typed_and_never_reports_loading_and_error_together() {
    let states = [
        TeamFeedback::Idle,
        TeamFeedback::Loading("Refreshing team tracker…".to_owned()),
        TeamFeedback::Info("Team tracker refreshed".to_owned()),
        TeamFeedback::Error {
            source: TeamFeedbackErrorSource::Refresh,
            message: "Jira is unavailable".to_owned(),
        },
    ];

    assert!(states[0].display_message().is_none());
    assert!(states[1].is_loading());
    assert!(!states[1].is_error());
    assert!(!states[2].is_loading());
    assert!(!states[2].is_error());
    assert!(states[3].is_error());
    assert!(!states[3].is_loading());
}

#[test]
fn team_feedback_display_copy_preserves_error_context() {
    assert_eq!(
        TeamFeedback::Error {
            source: TeamFeedbackErrorSource::Refresh,
            message: "Jira is unavailable".to_owned(),
        }
        .display_message()
        .as_deref(),
        Some("Team tracker refresh failed · Jira is unavailable")
    );
    assert_eq!(
        TeamFeedback::Error {
            source: TeamFeedbackErrorSource::Save,
            message: "Team tracker could not be saved".to_owned(),
        }
        .display_message()
        .as_deref(),
        Some("Team tracker could not be saved")
    );
    assert_eq!(
        TeamFeedback::Error {
            source: TeamFeedbackErrorSource::Connection,
            message: "Connect Jira before saving the team tracker".to_owned(),
        }
        .display_message()
        .as_deref(),
        Some("Connect Jira before saving the team tracker")
    );
    assert_eq!(
        TeamFeedback::Error {
            source: TeamFeedbackErrorSource::PrimaryRefreshBlocked,
            message: "Jira is unavailable".to_owned(),
        }
        .display_message()
        .as_deref(),
        Some("Team tracker was not refreshed because Jira refresh failed · Jira is unavailable")
    );
}

#[test]
fn team_feedback_alert_label_is_only_present_for_errors() {
    let error = TeamFeedback::Error {
        source: TeamFeedbackErrorSource::Refresh,
        message: "Jira is unavailable".to_owned(),
    };
    assert_eq!(
        error.error_accessible_label().as_deref(),
        Some("Team tracker error · Team tracker refresh failed · Jira is unavailable")
    );
    assert!(
        TeamFeedback::Loading("Refreshing team tracker…".to_owned())
            .error_accessible_label()
            .is_none()
    );
    assert!(
        TeamFeedback::Info("Team tracker refreshed".to_owned())
            .error_accessible_label()
            .is_none()
    );
    assert!(TeamFeedback::Idle.error_accessible_label().is_none());
}

#[gpui_kit::test]
fn team_error_state_is_distinct_from_empty_state_in_dense_and_card_views(
    cx: &mut gpui_kit::TestAppContext,
) {
    cx.update(gpui_kit::component::init);

    let mut dashboard = Dashboard::from_sample_data();
    dashboard.section = Section::Team;
    dashboard.team_members = vec![PersistedTeamMember {
        identifier: "amina".to_owned(),
        account_id: "amina".to_owned(),
        display_name: "Amina Yusuf".to_owned(),
    }];
    dashboard.team_issues.clear();
    dashboard.team_feedback = TeamFeedback::Error {
        source: TeamFeedbackErrorSource::Refresh,
        message: "Jira is unavailable".to_owned(),
    };
    let window = cx.open_window(gpui_kit::size(px(1_370.), px(900.)), |_, _| dashboard);
    let dashboard_entity = window.root(cx).expect("dashboard root");
    let mut visual = VisualTestContext::from_window(window.into(), cx);
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("team-error").is_some());
    assert!(visual.debug_bounds("team-empty").is_none());

    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.team_feedback = TeamFeedback::Info(
                "Team tracker refreshed · fetched 0 · displaying 0 in-progress tickets".to_owned(),
            );
            cx.notify();
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("team-empty").is_some());
    assert!(visual.debug_bounds("team-error").is_none());

    visual.simulate_resize(gpui_kit::size(px(390.), px(800.)));
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.update(|_, cx| {
        dashboard_entity.update(cx, |dashboard, cx| {
            dashboard.team_feedback = TeamFeedback::Error {
                source: TeamFeedbackErrorSource::Refresh,
                message: "Jira is unavailable".to_owned(),
            };
            cx.notify();
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.draw(cx).clear(cx));

    assert!(visual.debug_bounds("team-error").is_some());
    assert!(visual.debug_bounds("team-empty").is_none());
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
