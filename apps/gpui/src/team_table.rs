//! A virtualized table of currently in-progress team tickets.
//!
//! The delegate owns only presentation rows. Jira state remains in the domain and the caller is
//! responsible for replacing the rows after a cache refresh. This keeps table sorting and
//! selection deterministic while allowing the dashboard to map a selected row back to its stable
//! [`jira_domain::IssueId`].

use std::cmp::Ordering;

use gpui::{App, Context, IntoElement, ParentElement, Styled, Window, div, px};
use gpui_component::ActiveTheme as _;
use gpui_component::table::{Column, ColumnSort, TableDelegate, TableState};
use jira_domain::{Issue, IssueId, Timestamp, UpdateEvent, User};

use crate::presentation::{IdentityDirectory, describe_update_with_directory, format_timestamp};

const COLUMN_COUNT: usize = 7;

/// Display-ready values for one row in the team ticket table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeamTicketRow {
    /// Stable Jira identity used when opening the issue from a selected row.
    pub issue_id: IssueId,
    pub key: String,
    pub summary: String,
    pub assignee: String,
    pub status: String,
    pub latest_update: String,
    /// Exact localized timestamp, including date, time, and UTC offset.
    pub last_updated: String,
    /// The unformatted instant used for sorting and diagnostics.
    pub last_updated_at: Timestamp,
    /// Compact elapsed time from `last_updated_at` to the injected clock.
    pub elapsed: String,
    /// Elapsed seconds, useful for callers that need their own accessibility wording.
    pub elapsed_seconds: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SortColumn {
    Key,
    Summary,
    Assignee,
    Status,
    LatestUpdate,
    LastUpdated,
    Elapsed,
}

impl SortColumn {
    fn from_index(index: usize) -> Option<Self> {
        Some(match index {
            0 => Self::Key,
            1 => Self::Summary,
            2 => Self::Assignee,
            3 => Self::Status,
            4 => Self::LatestUpdate,
            5 => Self::LastUpdated,
            6 => Self::Elapsed,
            _ => return None,
        })
    }
}

/// Table delegate for in-progress team tickets.
pub struct TeamTicketTableDelegate {
    rows: Vec<TeamTicketRow>,
    sort_column: SortColumn,
    sort_order: ColumnSort,
}

impl TeamTicketTableDelegate {
    /// Builds rows from the current issue cache, event cache, and user directory.
    ///
    /// Status category matching is intentionally repeated here instead of relying on an upstream
    /// filter: stale or mixed-case cached data must never leak a non-in-progress issue into this
    /// surface.
    pub fn new(issues: &[Issue], events: &[UpdateEvent], users: &[User], now: Timestamp) -> Self {
        let mut this = Self {
            rows: Vec::new(),
            sort_column: SortColumn::LastUpdated,
            sort_order: ColumnSort::Ascending,
        };
        this.replace_rows(issues, events, users, now);
        this
    }

    /// Rebuilds and default-sorts the rows. The oldest activity is first so the stalest work is
    /// visible without any interaction.
    pub fn replace_rows(
        &mut self,
        issues: &[Issue],
        events: &[UpdateEvent],
        users: &[User],
        now: Timestamp,
    ) {
        let mut identities = IdentityDirectory::from_users(users);
        for issue in issues {
            identities.include_issue(issue);
        }

        self.rows = issues
            .iter()
            .filter(|issue| {
                issue
                    .status
                    .category
                    .as_deref()
                    .is_some_and(|category| category.trim().eq_ignore_ascii_case("in progress"))
            })
            .map(|issue| build_row(issue, events, &identities, now))
            .collect();
        self.sort_rows();
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[TeamTicketRow] {
        &self.rows
    }

    /// Maps the table's selected row to the stable Jira identity.
    pub fn issue_id_for_row(&self, row_ix: usize) -> Option<IssueId> {
        self.rows.get(row_ix).map(|row| row.issue_id.clone())
    }

    fn sort_rows(&mut self) {
        let column = self.sort_column;
        let descending = matches!(self.sort_order, ColumnSort::Descending);
        self.rows.sort_by(|left, right| {
            let ordering = compare_rows(left, right, column);
            let ordering = if descending {
                ordering.reverse()
            } else {
                ordering
            };
            ordering.then_with(|| left.issue_id.cmp(&right.issue_id))
        });
    }
}

/// Extension methods for the GPUI table entity used by dashboard integration.
pub trait TeamTicketTableStateExt: Sized {
    /// Replace the delegate's rows and refresh its column layout in one operation.
    fn replace_team_ticket_rows(
        &mut self,
        issues: &[Issue],
        events: &[UpdateEvent],
        users: &[User],
        now: Timestamp,
        cx: &mut Context<Self>,
    );

    /// Resolve the current row selection to an issue ID, if any.
    fn selected_team_ticket_issue_id(&self) -> Option<IssueId>;
}

impl TeamTicketTableStateExt for TableState<TeamTicketTableDelegate> {
    fn replace_team_ticket_rows(
        &mut self,
        issues: &[Issue],
        events: &[UpdateEvent],
        users: &[User],
        now: Timestamp,
        cx: &mut Context<Self>,
    ) {
        self.delegate_mut().replace_rows(issues, events, users, now);
        self.refresh(cx);
        cx.notify();
    }

    fn selected_team_ticket_issue_id(&self) -> Option<IssueId> {
        self.selected_row()
            .and_then(|row_ix| self.delegate().issue_id_for_row(row_ix))
    }
}

impl TableDelegate for TeamTicketTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        COLUMN_COUNT
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        match col_ix {
            0 => Column::new("key", "Ticket")
                .width(px(100.))
                .sortable()
                .fixed_left(),
            1 => Column::new("summary", "Summary").width(px(280.)).sortable(),
            2 => Column::new("assignee", "Assignee")
                .width(px(150.))
                .sortable(),
            3 => Column::new("status", "Status").width(px(125.)).sortable(),
            4 => Column::new("latest_update", "Latest update")
                .width(px(260.))
                .sortable(),
            5 => Column::new("last_updated", "Last updated")
                .width(px(190.))
                .sort(ColumnSort::Ascending),
            6 => Column::new("elapsed", "Age").width(px(85.)).sortable(),
            _ => Column::default(),
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        if let Some(column) = SortColumn::from_index(col_ix) {
            self.sort_column = column;
            self.sort_order = if matches!(sort, ColumnSort::Default) {
                ColumnSort::Ascending
            } else {
                sort
            };
            self.sort_rows();
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };

        let value = match col_ix {
            0 => row.key.as_str(),
            1 => row.summary.as_str(),
            2 => row.assignee.as_str(),
            3 => row.status.as_str(),
            4 => row.latest_update.as_str(),
            5 => row.last_updated.as_str(),
            6 => row.elapsed.as_str(),
            _ => "",
        };
        let mut cell = div().size_full().truncate().child(value.to_owned());
        cell = match col_ix {
            0 => cell.text_color(cx.theme().blue).text_sm(),
            3 => cell.text_color(cx.theme().green).text_sm(),
            6 => cell
                .text_color(age_color(row.elapsed_seconds, cx))
                .text_xs(),
            5 => cell.text_color(cx.theme().muted_foreground).text_xs(),
            _ => cell.text_color(cx.theme().foreground).text_sm(),
        };
        cell.into_any_element()
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        let Some(row) = self.rows.get(row_ix) else {
            return String::new();
        };
        match col_ix {
            0 => row.key.clone(),
            1 => row.summary.clone(),
            2 => row.assignee.clone(),
            3 => row.status.clone(),
            4 => row.latest_update.clone(),
            5 => row.last_updated.clone(),
            6 => row.elapsed.clone(),
            _ => String::new(),
        }
    }
}

fn build_row(
    issue: &Issue,
    events: &[UpdateEvent],
    identities: &IdentityDirectory,
    now: Timestamp,
) -> TeamTicketRow {
    let latest_event = events
        .iter()
        .filter(|event| event.issue_id == issue.id)
        .max_by(|left, right| {
            left.occurred_at
                .cmp(&right.occurred_at)
                .then_with(|| left.id.cmp(&right.id))
        });
    // An event older than the current Jira snapshot cannot safely describe the latest state: the
    // issue may have changed again before the cache observed `updated_at`.
    let latest_update_event = latest_event.filter(|event| event.occurred_at >= issue.updated_at);
    let latest_update = latest_update_event.map_or_else(
        || current_state_fallback(issue),
        |event| describe_update_with_directory(event, identities),
    );
    let last_updated_at = latest_event
        .map(|event| event.occurred_at.max(issue.updated_at))
        .unwrap_or(issue.updated_at);
    let elapsed_seconds = elapsed_seconds(last_updated_at, now);

    TeamTicketRow {
        issue_id: issue.id.clone(),
        key: issue.key.to_string(),
        summary: issue.summary.clone(),
        assignee: identities.display(issue.assignee.as_ref(), "Unassigned"),
        status: issue.status.name.clone(),
        latest_update,
        last_updated: format_timestamp(last_updated_at),
        last_updated_at,
        elapsed: format_elapsed(elapsed_seconds),
        elapsed_seconds,
    }
}

fn current_state_fallback(issue: &Issue) -> String {
    format!(
        "Jira issue updated · currently {}",
        issue.status.name.trim()
    )
}

fn compare_rows(left: &TeamTicketRow, right: &TeamTicketRow, column: SortColumn) -> Ordering {
    match column {
        SortColumn::Key => left
            .key
            .to_ascii_lowercase()
            .cmp(&right.key.to_ascii_lowercase()),
        SortColumn::Summary => left
            .summary
            .to_ascii_lowercase()
            .cmp(&right.summary.to_ascii_lowercase()),
        SortColumn::Assignee => left
            .assignee
            .to_ascii_lowercase()
            .cmp(&right.assignee.to_ascii_lowercase()),
        SortColumn::Status => left
            .status
            .to_ascii_lowercase()
            .cmp(&right.status.to_ascii_lowercase()),
        SortColumn::LatestUpdate => left
            .latest_update
            .to_ascii_lowercase()
            .cmp(&right.latest_update.to_ascii_lowercase()),
        SortColumn::LastUpdated => left.last_updated_at.cmp(&right.last_updated_at),
        // Age is a derived value, so its ascending order means freshest first. This is
        // intentionally different from the default LastUpdated sort, which is oldest first.
        SortColumn::Elapsed => left.elapsed_seconds.cmp(&right.elapsed_seconds),
    }
}

fn elapsed_seconds(last_updated_at: Timestamp, now: Timestamp) -> i64 {
    (now - last_updated_at).whole_seconds().max(0)
}

fn format_elapsed(seconds: i64) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;
    const MONTH: i64 = 30 * DAY;
    const YEAR: i64 = 365 * DAY;

    if seconds < MINUTE {
        format!("{seconds}s")
    } else if seconds < HOUR {
        format!("{}m", seconds / MINUTE)
    } else if seconds < DAY {
        format!("{}h", seconds / HOUR)
    } else if seconds < WEEK {
        format!("{}d", seconds / DAY)
    } else if seconds < MONTH {
        format!("{}w", seconds / WEEK)
    } else if seconds < YEAR {
        format!("{}mo", seconds / MONTH)
    } else {
        format!("{}y", seconds / YEAR)
    }
}

fn age_color(seconds: i64, cx: &Context<TableState<TeamTicketTableDelegate>>) -> gpui::Hsla {
    if seconds >= 14 * 24 * 60 * 60 {
        cx.theme().red
    } else if seconds >= 7 * 24 * 60 * 60 {
        cx.theme().yellow
    } else {
        cx.theme().muted_foreground
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample_data::{sample_issues, sample_updates, sample_users};
    use jira_domain::{ChangeValue, EventId, IssueKey, UpdateKind};
    use time::macros::datetime;

    #[test]
    fn filters_category_case_insensitively() {
        let mut issues = sample_issues();
        issues[0].status.category = Some(" IN PROGRESS ".to_owned());
        let rows = TeamTicketTableDelegate::new(
            &issues,
            &[],
            &sample_users(),
            datetime!(2026-08-19 00:00 UTC),
        );
        assert_eq!(rows.rows().len(), 3);
        let keys: Vec<_> = rows.rows().iter().map(|row| row.key.as_str()).collect();
        assert_eq!(keys, vec!["DESK-171", "DESK-176", "DESK-184"]);
    }

    #[test]
    fn selects_newest_event_and_uses_safe_wording() {
        let issues = sample_issues();
        let mut events = sample_updates();
        events.push(UpdateEvent::new(
            EventId::new("newer").unwrap(),
            issues[0].site_id.clone(),
            issues[0].id.clone(),
            IssueKey::new("DESK-184").unwrap(),
            UpdateKind::SummaryChanged {
                old: ChangeValue::Text("old".into()),
                new: ChangeValue::Text("new".into()),
            },
            datetime!(2026-08-18 00:00 UTC),
            vec![],
        ));
        let table = TeamTicketTableDelegate::new(
            &issues,
            &events,
            &sample_users(),
            datetime!(2026-08-19 00:00 UTC),
        );
        let row = table
            .rows()
            .iter()
            .find(|row| row.key == "DESK-184")
            .unwrap();
        assert_eq!(row.latest_update, "Summary: old → new");
        assert_eq!(row.last_updated_at, datetime!(2026-08-18 00:00 UTC));
        assert!(!row.latest_update.contains("account"));
    }

    #[test]
    fn quiet_baseline_describes_current_state() {
        let issues = sample_issues();
        let table = TeamTicketTableDelegate::new(
            &issues,
            &[],
            &sample_users(),
            datetime!(2026-08-19 00:00 UTC),
        );
        let row = table
            .rows()
            .iter()
            .find(|row| row.key == "DESK-184")
            .unwrap();
        assert_eq!(
            row.latest_update,
            "Jira issue updated · currently In Progress"
        );
    }

    #[test]
    fn stale_event_does_not_claim_to_describe_newer_issue_state() {
        let mut issue = sample_issues()[0].clone();
        issue.updated_at = datetime!(2026-08-18 00:00 UTC);
        let stale_event = sample_updates().into_iter().next().unwrap();
        let table = TeamTicketTableDelegate::new(
            &[issue],
            &[stale_event],
            &sample_users(),
            datetime!(2026-08-19 00:00 UTC),
        );
        let row = &table.rows()[0];
        assert_eq!(
            row.latest_update,
            "Jira issue updated · currently In Progress"
        );
        assert_eq!(row.last_updated_at, datetime!(2026-08-18 00:00 UTC));
    }

    #[test]
    fn age_boundaries_and_future_timestamps_are_clamped() {
        let issues = sample_issues();
        let now = datetime!(2026-08-19 00:00 UTC);
        let table = TeamTicketTableDelegate::new(&issues, &[], &sample_users(), now);
        let row = table
            .rows()
            .iter()
            .find(|row| row.key == "DESK-184")
            .unwrap();
        assert_eq!(row.elapsed, "2d");
        assert_eq!(format_elapsed(59), "59s");
        assert_eq!(format_elapsed(60), "1m");
        assert_eq!(format_elapsed(3_600), "1h");
        assert_eq!(format_elapsed(86_400), "1d");

        let mut future = issues[0].clone();
        future.status.category = Some("in progress".to_owned());
        future.updated_at = datetime!(2026-08-20 00:00 UTC);
        let table = TeamTicketTableDelegate::new(&[future], &[], &sample_users(), now);
        assert_eq!(table.rows()[0].elapsed_seconds, 0);
        assert_eq!(table.rows()[0].elapsed, "0s");
    }

    #[test]
    fn default_and_explicit_sorting_are_deterministic() {
        let issues = sample_issues();
        let mut table = TeamTicketTableDelegate::new(
            &issues,
            &[],
            &sample_users(),
            datetime!(2026-08-19 00:00 UTC),
        );
        assert_eq!(table.rows()[0].key, "DESK-171");
        table.sort_column = SortColumn::Key;
        table.sort_order = ColumnSort::Ascending;
        table.sort_rows();
        let ascending: Vec<_> = table.rows().iter().map(|row| row.key.as_str()).collect();
        assert_eq!(ascending, vec!["DESK-171", "DESK-176", "DESK-184"]);
        table.sort_order = ColumnSort::Descending;
        table.sort_rows();
        assert_eq!(table.rows()[0].key, "DESK-184");

        table.sort_column = SortColumn::Elapsed;
        table.sort_order = ColumnSort::Ascending;
        table.sort_rows();
        assert_eq!(table.rows()[0].key, "DESK-184");
        table.sort_order = ColumnSort::Descending;
        table.sort_rows();
        assert_eq!(table.rows()[0].key, "DESK-171");
    }

    #[test]
    fn row_selection_maps_to_stable_issue_id() {
        let issues = sample_issues();
        let table = TeamTicketTableDelegate::new(
            &issues,
            &[],
            &sample_users(),
            datetime!(2026-08-19 00:00 UTC),
        );
        let selected = table.issue_id_for_row(0).unwrap();
        assert_eq!(selected, IssueId::new("10171").unwrap());
        assert!(table.issue_id_for_row(99).is_none());
    }
}
