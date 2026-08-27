use super::*;

const DETAIL_SIDEBAR_MIN_WIDTH: f32 = 320.;
pub(super) const TEAM_DETAIL_RESIZE_HANDLE_WIDTH: f32 = 8.;
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

pub(super) fn clamped_team_detail_width(
    requested: f32,
    viewport_width: f32,
    layout: LayoutMode,
    table_mode: TeamTableMode,
    sidebar_collapsed: bool,
) -> f32 {
    let content_width = (viewport_width
        - crate::responsive::effective_sidebar_width(layout, sidebar_collapsed)
        - 2. * layout.list_padding())
    .max(0.);
    let table_min_width = team_table_min_width(table_mode, layout);
    let max_width = ((content_width - TEAM_DETAIL_RESIZE_HANDLE_WIDTH - table_min_width).max(0.))
        .min(content_width / 2.);
    let min_width = DETAIL_SIDEBAR_MIN_WIDTH.min(max_width);
    requested.clamp(min_width, max_width)
}

struct DetailSidebarResize;

impl Render for DetailSidebarResize {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl Dashboard {
    pub(super) fn render_team(
        &self,
        layout: LayoutMode,
        table_mode: TeamTableMode,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mobile = layout.is_mobile();
        let configured = !self.team_members.is_empty();
        let show_mobile_detail = mobile && self.mobile_detail_open;
        let table = self.team_table.clone();
        let (_, displayed_team_count) = team_issue_counts(&self.team_issues);
        let team_loading = configured
            && displayed_team_count == 0
            && (self.team_task.is_some() || self.team_feedback_loading);
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
            .p(px(layout.list_padding()))
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
                    .when_some(self.team_feedback.clone(), |this, feedback| {
                        this.child(
                            div()
                                .min_w_0()
                                .whitespace_normal()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(feedback),
                        )
                    }),
            )
            .child(
                h_flex()
                    .h_full()
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
                        if matches!(table_mode, TeamTableMode::Cards) {
                            v_flex()
                                .id("team-table")
                                .debug_selector(|| "team-table".to_owned())
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
                                    this.child(
                                        v_flex()
                                            .items_center()
                                            .gap_2()
                                            .p_6()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .when(team_loading, |this| {
                                                this.child(Spinner::new()).child(
                                                    div().child("Loading team tickets…"),
                                                )
                                            })
                                            .when(!team_loading, |this| {
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
                                .h_full()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .border_1()
                                .border_color(cx.theme().border)
                                .when(!configured, |this| {
                                    this.child(
                                        v_flex()
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
                                        this.child(
                                            v_flex()
                                                .items_center()
                                                .gap_2()
                                                .p_6()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .when(team_loading, |this| {
                                                    this.child(Spinner::new()).child(
                                                        div().child("Loading team tickets…"),
                                                    )
                                                })
                                                .when(!team_loading, |this| {
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
                        }
                    })
                    .when(!mobile, |this| {
                        this.child(
                            div()
                                .id("team-detail-resize-handle")
                                .h_full()
                                .w(px(8.))
                                .flex_shrink_0()
                                .cursor(gpui::CursorStyle::ResizeColumn)
                                .hover(|style| style.bg(cx.theme().muted))
                                .on_drag(DetailSidebarResize, |_, _, _, cx| {
                                    cx.new(|_| DetailSidebarResize)
                                }),
                        )
                        .child(
                            v_flex()
                                .id("team-detail")
                                .h_full()
                                .flex_1()
                                .min_h_0()
                                .min_w_0()
                                .debug_selector(|| "team-detail".to_owned())
                                .w(self.detail_sidebar_width)
                                .flex_shrink_0()
                                .p_4()
                                .gap_2()
                                .overflow_x_hidden()
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(self.issue_detail(layout, cx)),
                        )
                    })
                    .on_drag_move(cx.listener(|this, event: &DragMoveEvent<DetailSidebarResize>, _, cx| {
                        let container_right = event.bounds.right();
                        let max_width = (event.bounds.size.width / 2.).max(px(DETAIL_SIDEBAR_MIN_WIDTH));
                        let requested = container_right - event.event.position.x;
                        let clamped = requested.clamp(
                            px(DETAIL_SIDEBAR_MIN_WIDTH),
                            max_width,
                        );
                        if clamped != this.detail_sidebar_width {
                            this.detail_sidebar_width = clamped;
                            cx.notify();
                        }
                    })),
            )
    }
}
