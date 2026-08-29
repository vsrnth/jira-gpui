use super::*;

const DETAIL_SIDEBAR_MIN_WIDTH: f32 = 320.;
pub(super) const TEAM_DETAIL_INITIAL_WIDTH: f32 = 480.;
pub(super) const TEAM_DENSE_TABLE_WIDTH: f32 = 596.;
const TEAM_WIDE_TABLE_WIDTH: f32 = 1_190.;
const TEAM_WIDE_TABLE_MIN_VIEWPORT: f32 = 1_920.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TeamTableMode {
    Cards,
    DenseTable,
    WideTable,
}

pub(super) fn team_table_mode_for_width(width: f32) -> TeamTableMode {
    if width >= TEAM_WIDE_TABLE_MIN_VIEWPORT {
        TeamTableMode::WideTable
    } else if width >= 1_200. {
        TeamTableMode::DenseTable
    } else {
        TeamTableMode::Cards
    }
}

pub(super) fn team_table_min_width(mode: TeamTableMode, layout: LayoutMode) -> f32 {
    match mode {
        TeamTableMode::Cards => layout.issue_list_range().0,
        TeamTableMode::DenseTable => TEAM_DENSE_TABLE_WIDTH,
        TeamTableMode::WideTable => TEAM_WIDE_TABLE_WIDTH,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TeamPaneConstraints {
    pub(super) available_width: f32,
    pub(super) table_min_width: f32,
    pub(super) table_max_width: f32,
    pub(super) detail_min_width: f32,
    pub(super) detail_max_width: f32,
    pub(super) initial_detail_width: f32,
}

pub(super) fn team_pane_constraints(
    requested_detail_width: f32,
    viewport_width: f32,
    layout: LayoutMode,
    table_mode: TeamTableMode,
    sidebar_collapsed: bool,
) -> TeamPaneConstraints {
    let available_width = (viewport_width
        - crate::responsive::sidebar_width_for_viewport(layout, sidebar_collapsed, viewport_width)
        - 2. * layout.list_padding())
    .max(0.);
    let table_min_width = team_table_min_width(table_mode, layout);
    // A desktop viewport normally has room for both minimums. Bound the
    // native panel floor at very small or transitional viewports so a
    // malformed min..max range can never reach gpui-component.
    let bounded_table_min_width = table_min_width.min(available_width);
    let detail_max_width = (available_width - bounded_table_min_width)
        .max(0.)
        .min(available_width / 2.);
    let detail_min_width = DETAIL_SIDEBAR_MIN_WIDTH
        .min(available_width)
        .min(detail_max_width);
    let table_max_width = (available_width - detail_min_width)
        .max(bounded_table_min_width)
        .min(available_width);
    let initial_detail_width = requested_detail_width.clamp(detail_min_width, detail_max_width);

    TeamPaneConstraints {
        available_width,
        table_min_width: bounded_table_min_width,
        table_max_width,
        detail_min_width,
        detail_max_width,
        initial_detail_width,
    }
}

impl Dashboard {
    pub(super) fn render_team(
        &self,
        layout: LayoutMode,
        table_mode: TeamTableMode,
        viewport_width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mobile = layout.is_mobile();
        let configured = !self.team_members.is_empty();
        let show_mobile_detail = mobile && self.mobile_detail_open;
        let table = self.team_table.clone();
        let (_, displayed_team_count) = team_issue_counts(&self.team_issues);
        let team_loading =
            configured && displayed_team_count == 0 && self.team_feedback.is_loading();
        let team_error = configured && displayed_team_count == 0 && self.team_feedback.is_error();
        let team_feedback_message = self.team_feedback.display_message();
        let team_feedback_error = self.team_feedback.is_error();
        let team_feedback_error_label = self.team_feedback.error_accessible_label();
        let mobile_rows = self
            .team_issues
            .iter()
            .filter(|issue| {
                issue
                    .status
                    .category
                    .as_deref()
                    .is_some_and(|category| category.trim().eq_ignore_ascii_case("in progress"))
            })
            .map(|issue| IssueViewModel::from_domain(issue, &self.users))
            .collect::<Vec<_>>();
        v_flex()
            .size_full()
            .min_w_0()
            .p(gpui::rems(layout.list_padding() / 16.))
            .gap_3()
            .child(
                v_flex()
                    .min_w_0()
                    .gap_1()
                    .child(div().text_base().font_semibold().child("In-progress team tickets"))
                    .child(div().min_w_0().whitespace_normal().text_sm().text_color(cx.theme().muted_foreground).child(
                        "All visible Jira issues whose status category is In Progress and whose assignee is one of the configured accounts. Jira permissions still apply.",
                    ))
                    .child(div().min_w_0().whitespace_normal().text_xs().text_color(cx.theme().muted_foreground).child(
                        if configured { team_summary(displayed_team_count, self.team_members.len()) } else { "No team members configured · no Jira request will be made".to_owned() },
                    ))
                    .when_some(team_feedback_message, |this, feedback| {
                        this.child(
                            v_flex()
                                .id(if team_feedback_error && !team_error {
                                    "team-error"
                                } else {
                                    "team-feedback"
                                })
                                .min_w_0()
                                .whitespace_normal()
                                .text_xs()
                                .debug_selector(move || {
                                    if team_feedback_error && !team_error {
                                        "team-error".to_owned()
                                    } else {
                                        "team-feedback".to_owned()
                                    }
                                })
                                .text_color(if team_feedback_error {
                                    cx.theme().danger
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .role(if team_feedback_error {
                                    gpui::accesskit::Role::Alert
                                } else {
                                    gpui::accesskit::Role::Status
                                })
                                .aria_label(
                                    team_feedback_error_label
                                        .unwrap_or_else(|| feedback.clone()),
                                )
                                .child(feedback),
                        )
                    }),
            )
            .child(
                h_flex()
                    .h_full()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .when(mobile, |this| this.flex_col())
                    .child(if show_mobile_detail {
                        v_flex()
                            .h_full()
                            .flex_1()
                            .min_h_0()
                            .p_3()
                            .gap_2()
                            .child(
                                Button::new("team-back-to-table")
                                    .ghost()
                                    .label("Back to team tickets")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mobile_detail_open = false;
                                        cx.notify();
                                    })),
                            )
                    .child(
                                self.issue_detail(layout, cx),
                            )
                            .into_any_element()
                    } else {
                        let team_table =
                        if matches!(table_mode, TeamTableMode::Cards) {
                            v_flex()
                                .id("team-table")
                                .debug_selector(|| "team-table".to_owned())
                                .accessibility_id("team-table")
                                .role(gpui::accesskit::Role::Group)
                                .aria_label("In-progress team tickets")
                                .h_full()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .overflow_y_scrollbar()
                                .border_1()
                                .border_color(cx.theme().border)
                                .when(!configured, |this| {
                                    this.child(
                                        v_flex()
                                            .id("team-unconfigured")
                                            .role(gpui::accesskit::Role::Status)
                                            .aria_label("Team tracker is not configured")
                                            .items_center()
                                            .gap_2()
                                            .p_6()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Configure a team to see in-progress tickets here.")
                                            .child(
                                                Button::new("configure-team-mobile")
                                                    .compact()
                                                    .primary()
                                                    .label("Configure team")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.section = Section::Settings;
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                })
                                .when(configured && mobile_rows.is_empty(), |this| {
                                    let status_id = if team_loading {
                                        "team-loading"
                                    } else if team_error {
                                        "team-error"
                                    } else {
                                        "team-empty"
                                    };
                                    let status_label = if team_loading {
                                        "Loading in-progress team tickets"
                                    } else if team_error {
                                        "Team tracker error"
                                    } else {
                                        "No in-progress team tickets found"
                                    };
                                    let status_label = if team_error {
                                        format!(
                                            "{status_label} · {}",
                                            self.team_feedback
                                                .display_message()
                                                .unwrap_or_else(|| {
                                                    "Unable to refresh team tickets".to_owned()
                                                })
                                        )
                                    } else {
                                        status_label.to_owned()
                                    };
                                    this.child(
                                        v_flex()
                                            .id(status_id)
                                            .debug_selector(move || status_id.to_owned())
                                            .role(if team_error {
                                                gpui::accesskit::Role::Alert
                                            } else {
                                                gpui::accesskit::Role::Status
                                            })
                                            .aria_label(status_label)
                                            .items_center()
                                            .gap_2()
                                            .p_6()
                                            .text_sm()
                                            .text_color(if team_error {
                                                cx.theme().danger
                                            } else {
                                                cx.theme().muted_foreground
                                            })
                                            .when(team_loading, |this| {
                                                this.child(Spinner::new()).child(
                                                    div().child("Loading team tickets…"),
                                                )
                                            })
                                            .when(team_error, |this| {
                                                this.child(self.team_feedback.display_message().unwrap_or_else(
                                                    || "Unable to refresh team tickets".to_owned(),
                                                ))
                                            })
                                            .when(!team_loading && !team_error, |this| {
                                                this.child("No in-progress team tickets found.")
                                            }),
                                    )
                                })
                                .children(
                                    mobile_rows
                                        .iter()
                                        .map(|issue| self.issue_row(issue, layout, cx)),
                                )
                                .into_any_element()
                        } else {
                            v_flex()
                                .id("team-table")
                                .debug_selector(|| "team-table".to_owned())
                                .accessibility_id("team-table")
                                .role(gpui::accesskit::Role::Group)
                                .aria_label("In-progress team tickets")
                                .h_full()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .border_1()
                                .border_color(cx.theme().border)
                                .when(!configured, |this| {
                                    this.child(
                                        v_flex()
                                            .id("team-unconfigured")
                                            .role(gpui::accesskit::Role::Status)
                                            .aria_label("Team tracker is not configured")
                                            .items_center()
                                            .gap_2()
                                            .p_6()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child("Configure a team to see in-progress tickets here.")
                                            .child(
                                                Button::new("configure-team")
                                                    .compact()
                                                    .primary()
                                                    .label("Configure team")
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.section = Section::Settings;
                                                        cx.notify();
                                                    })),
                                            ),
                                    )
                                })
                                .when(
                                    configured
                                        && displayed_team_count == 0
                                        && !matches!(table_mode, TeamTableMode::Cards),
                                    |this| {
                                        let status_id = if team_loading {
                                            "team-loading"
                                        } else if team_error {
                                            "team-error"
                                        } else {
                                            "team-empty"
                                        };
                                        let status_label = if team_loading {
                                            "Loading in-progress team tickets"
                                        } else if team_error {
                                            "Team tracker error"
                                        } else {
                                            "No in-progress team tickets found"
                                        };
                                        let status_label = if team_error {
                                            format!(
                                                "{status_label} · {}",
                                                self.team_feedback
                                                    .display_message()
                                                    .unwrap_or_else(|| {
                                                        "Unable to refresh team tickets".to_owned()
                                                    })
                                            )
                                        } else {
                                            status_label.to_owned()
                                        };
                                        this.child(
                                            v_flex()
                                                .id(status_id)
                                                .debug_selector(move || status_id.to_owned())
                                                .role(if team_error {
                                                    gpui::accesskit::Role::Alert
                                                } else {
                                                    gpui::accesskit::Role::Status
                                                })
                                                .aria_label(status_label)
                                                .items_center()
                                                .gap_2()
                                                .p_6()
                                                .text_sm()
                                                .text_color(if team_error {
                                                    cx.theme().danger
                                                } else {
                                                    cx.theme().muted_foreground
                                                })
                                                .when(team_loading, |this| {
                                                    this.child(Spinner::new()).child(
                                                        div().child("Loading team tickets…"),
                                                    )
                                                })
                                                .when(team_error, |this| {
                                                    this.child(self.team_feedback.display_message().unwrap_or_else(
                                                        || "Unable to refresh team tickets".to_owned(),
                                                    ))
                                                })
                                                .when(!team_loading && !team_error, |this| {
                                                    this.child("No in-progress team tickets found.")
                                                }),
                                        )
                                    },
                                )
                                .when(
                                    configured
                                        && displayed_team_count > 0
                                        && table.is_some()
                                        && !matches!(table_mode, TeamTableMode::Cards),
                                    |this| {
                                    this.child(
                                        DataTable::new(table.as_ref().expect("team table exists")),
                                    )
                                })
                                .into_any_element()
                        };
                        if mobile {
                            team_table
                        } else {
                            let constraints = team_pane_constraints(
                                TEAM_DETAIL_INITIAL_WIDTH,
                                viewport_width,
                                layout,
                                table_mode,
                                self.sidebar_collapsed,
                            );
                            let state = self
                                .team_panes_state
                                .as_ref()
                                .expect("team panes state should be initialized for desktop");
                            // gpui-component's resizable API requires native pixel ranges;
                            // the surrounding dashboard layout remains rem-based.
                            div()
                                .id("team-panes")
                                .debug_selector(|| "team-panes".to_owned())
                                .h_full()
                                .w_full()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .child(
                                    h_resizable("team-panes-group")
                                        .with_state(state)
                                        .child(
                                            resizable_panel()
                                                .size(px(
                                                    constraints.available_width
                                                        - constraints.initial_detail_width,
                                                ))
                                                .size_range(
                                                    px(constraints.table_min_width)
                                                        ..px(constraints.table_max_width),
                                                )
                                                .child(team_table),
                                        )
                                        .child(
                                            resizable_panel()
                                                .size(px(constraints.initial_detail_width))
                                                .size_range(
                                                    px(constraints.detail_min_width)
                                                        ..px(constraints.detail_max_width),
                                                )
                                                .flex_none()
                                                .child(
                                                    v_flex()
                                                        .id("team-detail")
                                                        .h_full()
                                                        .w_full()
                                                        .min_h_0()
                                                        .min_w_0()
                                                        .debug_selector(|| "team-detail".to_owned())
                                                        .p_4()
                                                        .gap_2()
                                                        .overflow_x_hidden()
                                                        .border_1()
                                                        .border_color(cx.theme().border)
                                                        .child(self.issue_detail(layout, cx)),
                                                ),
                                        ),
                                )
                                .into_any_element()
                        }
                    })
            )
    }
}
