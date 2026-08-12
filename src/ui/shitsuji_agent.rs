use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::state::{
    AppState, RuleProposalView, ShitsujiDecisionHitArea, ShitsujiPanelHitAreas, ShitsujiPanelState,
};
use crate::shitsuji_agent::{RuleProposalDecision, RuleProposalDecisionRequest, RuleProposalId};

pub(crate) const SHITSUJI_PANEL_EXPANDED_WIDTH: u16 = 40;
pub(crate) const SHITSUJI_PANEL_MIN_WIDTH: u16 = 36;
/// Display width of the widest collapsed label, `[‹ Rules 99+]`.
pub(crate) const SHITSUJI_PANEL_HANDLE_WIDTH: u16 = 13;
pub(crate) const SHITSUJI_PANEL_MIN_TERMINAL_WIDTH: u16 = 30;
/// `RULE` label and the blank line under it.
const SHITSUJI_CARD_HEADER_ROWS: usize = 2;
/// Blank, `ASSIGNED AGENT`, its value, blank, and the decision row, pinned to the body bottom so
/// the buttons stay visible and clickable no matter how far the rule text scrolls.
const SHITSUJI_CARD_FOOTER_ROWS: usize = 5;
/// Conventional profile id of the bundled shitsuji agent. `shitsuji_agent.backend_profile_id` has no
/// default, so any other configured id is shown verbatim.
const SHITSUJI_AGENT_PROFILE_ID: &str = "shitsuji-agent";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShitsujiCardLayout {
    rule_lines: usize,
    rule_viewport: usize,
    target_offset: usize,
    button_offset: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ShitsujiPanelLayout {
    pub terminal_rect: Rect,
    pub panel_rect: Rect,
    pub overlay: bool,
}

pub(crate) fn compute_shitsuji_panel_layout(
    area: Rect,
    expanded: bool,
    mobile: bool,
) -> ShitsujiPanelLayout {
    if area.width == 0 || area.height == 0 {
        return ShitsujiPanelLayout::default();
    }

    if !expanded {
        return ShitsujiPanelLayout {
            terminal_rect: area,
            panel_rect: Rect::default(),
            overlay: true,
        };
    }

    let can_dock = !mobile
        && area.width >= SHITSUJI_PANEL_MIN_TERMINAL_WIDTH.saturating_add(SHITSUJI_PANEL_MIN_WIDTH);
    if can_dock {
        let panel_width = SHITSUJI_PANEL_EXPANDED_WIDTH
            .min(area.width.saturating_sub(SHITSUJI_PANEL_MIN_TERMINAL_WIDTH))
            .max(SHITSUJI_PANEL_MIN_WIDTH);
        let terminal_width = area.width.saturating_sub(panel_width);
        return ShitsujiPanelLayout {
            terminal_rect: Rect::new(area.x, area.y, terminal_width, area.height),
            panel_rect: Rect::new(
                area.x.saturating_add(terminal_width),
                area.y,
                panel_width,
                area.height,
            ),
            overlay: false,
        };
    }

    let horizontal_margin = if area.width > SHITSUJI_PANEL_MIN_WIDTH {
        2
    } else {
        0
    };
    let vertical_margin = if area.height > 12 { 1 } else { 0 };
    let panel_width = SHITSUJI_PANEL_EXPANDED_WIDTH
        .min(area.width.saturating_sub(horizontal_margin * 2))
        .max(1);
    let panel_height = area.height.saturating_sub(vertical_margin * 2).max(1);
    ShitsujiPanelLayout {
        terminal_rect: area,
        panel_rect: Rect::new(
            area.x
                .saturating_add(area.width.saturating_sub(panel_width)),
            area.y.saturating_add(vertical_margin),
            panel_width,
            panel_height,
        ),
        overlay: true,
    }
}

/// Where the collapsed handle goes, and how many columns its host chrome row loses to it.
///
/// Both come from one call so a row can never reserve columns the handle does not take, or lose
/// the handle to a row that never made room.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ShitsujiPanelHandleSlot {
    pub reserved_width: u16,
    pub rect: Rect,
}

/// `chrome_min_content_width` is what the host row needs for its own controls. A row that cannot
/// spare the handle keeps all of its columns and the handle falls back to the top row of
/// `terminal_rect` — the only path where it overlaps terminal content.
pub(crate) fn shitsuji_panel_handle_slot(
    panel: &ShitsujiPanelState,
    chrome_row: Rect,
    chrome_min_content_width: u16,
    terminal_rect: Rect,
) -> ShitsujiPanelHandleSlot {
    if panel.is_expanded() {
        return ShitsujiPanelHandleSlot::default();
    }
    let chrome_fits = chrome_row.width > 0
        && chrome_row.height > 0
        && chrome_row.width >= SHITSUJI_PANEL_HANDLE_WIDTH.saturating_add(chrome_min_content_width);
    let (host, reserved_width) = if chrome_fits {
        (
            Rect::new(chrome_row.x, chrome_row.y, chrome_row.width, 1),
            SHITSUJI_PANEL_HANDLE_WIDTH,
        )
    } else if terminal_rect.width > 0 && terminal_rect.height > 0 {
        (
            Rect::new(terminal_rect.x, terminal_rect.y, terminal_rect.width, 1),
            0,
        )
    } else {
        return ShitsujiPanelHandleSlot::default();
    };
    let width = SHITSUJI_PANEL_HANDLE_WIDTH.min(host.width);
    ShitsujiPanelHandleSlot {
        reserved_width,
        rect: Rect::new(
            host.x.saturating_add(host.width.saturating_sub(width)),
            host.y,
            width,
            1,
        ),
    }
}

pub(crate) fn compute_shitsuji_panel_hit_areas(
    panel: &ShitsujiPanelState,
    panel_rect: Rect,
) -> ShitsujiPanelHitAreas {
    if panel_rect.width < 4 || panel_rect.height < 3 {
        return ShitsujiPanelHitAreas::default();
    }
    let inner = Rect::new(
        panel_rect.x.saturating_add(1),
        panel_rect.y.saturating_add(1),
        panel_rect.width.saturating_sub(2),
        panel_rect.height.saturating_sub(2),
    );
    let close = Rect::new(
        inner.x.saturating_add(inner.width.saturating_sub(3)),
        panel_rect.y,
        3.min(inner.width),
        1,
    );
    let body_y = inner.y.saturating_add(2);
    let mut decisions = Vec::new();
    if let (Some(proposal), Some(card)) = (
        panel.proposals.get(panel.selected),
        selected_shitsuji_card_layout(panel, panel_rect),
    ) {
        if shitsuji_panel_body_height(panel_rect) > 0 {
            let button_y = body_y.saturating_add(saturating_u16(card.button_offset));
            let gap = 1;
            let button_width = inner.width.saturating_sub(gap) / 2;
            let reject = Rect::new(inner.x, button_y, button_width, 1);
            let approve = Rect::new(
                inner.x.saturating_add(button_width).saturating_add(gap),
                button_y,
                inner.width.saturating_sub(button_width).saturating_sub(gap),
                1,
            );
            for (rect, decision) in [
                (reject, RuleProposalDecision::Reject),
                (approve, RuleProposalDecision::Approve),
            ] {
                decisions.push(ShitsujiDecisionHitArea {
                    rect,
                    request: RuleProposalDecisionRequest {
                        proposal_id: RuleProposalId::new(&proposal.proposal_id),
                        expected_revision: proposal.revision,
                        decision,
                    },
                });
            }
        }
    }
    ShitsujiPanelHitAreas { close, decisions }
}

pub(crate) fn max_shitsuji_panel_scroll(panel: &ShitsujiPanelState, panel_rect: Rect) -> usize {
    selected_shitsuji_card_layout(panel, panel_rect)
        .filter(|card| card.rule_viewport > 0)
        .map(|card| card.rule_lines.saturating_sub(card.rule_viewport))
        .unwrap_or(0)
}

pub(crate) fn shitsuji_panel_page_height(panel_rect: Rect) -> usize {
    shitsuji_rule_viewport_height(panel_rect).max(1)
}

/// ratatui draws block titles left, then center, then right, so each one overdraws the previous.
/// The centered position is dropped when `[›]` would clip it, and the left heading is shortened
/// until it stops before whatever follows it.
fn heading_for_title_width(panel_width: u16, position_len: usize) -> &'static str {
    let title_width = usize::from(panel_width.saturating_sub(2));
    let next_label_start = if position_shown(panel_width, position_len) {
        title_width.saturating_sub(position_len) / 2
    } else {
        title_width.saturating_sub(3)
    };
    ["Rule proposals", "Rules"]
        .into_iter()
        .find(|heading| heading.len() < next_label_start)
        .unwrap_or("")
}

fn position_shown(panel_width: u16, position_len: usize) -> bool {
    if position_len == 0 {
        return false;
    }
    let title_width = usize::from(panel_width.saturating_sub(2));
    let start = title_width.saturating_sub(position_len) / 2;
    start.saturating_add(position_len) < title_width.saturating_sub(3)
}

pub(super) fn render_shitsuji_panel(app: &AppState, frame: &mut Frame) {
    let panel = &app.shitsuji_panel;
    let p = &app.palette;
    let panel_rect = app.view.shitsuji_panel_rect;
    let handle_rect = app.view.shitsuji_panel_handle_rect;

    if handle_rect.width > 0 && handle_rect.height > 0 {
        let pending = panel.proposals.len();
        let label = if pending == 0 {
            "[‹ Rules]".to_string()
        } else if pending > 99 {
            "[‹ Rules 99+]".to_string()
        } else {
            format!("[‹ Rules {pending}]")
        };
        // Pad to the full rect so every cell is painted: the headless retained path treats
        // handle_rect as an overlay and skips it when patching pane output, so any cell the label
        // does not cover would keep stale terminal content.
        let label = format!("{label:>width$}", width = usize::from(handle_rect.width));
        frame.render_widget(
            Paragraph::new(Span::styled(
                label,
                Style::default().fg(if pending > 0 { p.yellow } else { p.text }),
            )),
            handle_rect,
        );
    }

    if panel_rect.width == 0 || panel_rect.height == 0 {
        return;
    }
    if app.view.shitsuji_panel_overlay {
        frame.render_widget(Clear, panel_rect);
    }
    let border_style = if panel.keyboard_focused {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface1)
    };
    let position = if panel.proposals.is_empty() {
        String::new()
    } else {
        format!(
            "{} / {}",
            panel.selected.saturating_add(1),
            panel.proposals.len()
        )
    };
    let position = if position_shown(panel_rect.width, position.chars().count()) {
        position
    } else {
        String::new()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title_top(
            Line::from(Span::styled(
                heading_for_title_width(panel_rect.width, position.chars().count()),
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            ))
            .left_aligned(),
        )
        .title_top(Line::from(Span::styled(position, Style::default().fg(p.overlay1))).centered())
        .title_top(
            Line::from(Span::styled("[›]", Style::default().fg(p.overlay1))).right_aligned(),
        );
    frame.render_widget(block, panel_rect);

    let inner = Rect::new(
        panel_rect.x.saturating_add(1),
        panel_rect.y.saturating_add(1),
        panel_rect.width.saturating_sub(2),
        panel_rect.height.saturating_sub(2),
    );
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled("Shitsuji Agent", Style::default().fg(p.mauve))),
        inner,
    );

    let body = Rect::new(
        inner.x,
        inner.y.saturating_add(2),
        inner.width,
        inner.height.saturating_sub(2),
    );
    if panel.proposals.is_empty() {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "No pending rules",
                    Style::default().fg(p.text).add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "New proposals will appear here.",
                    Style::default().fg(p.overlay0),
                )),
            ])
            .wrap(Wrap { trim: true }),
            body,
        );
        return;
    }

    if let (Some(proposal), Some(card)) = (
        panel.proposals.get(panel.selected),
        selected_shitsuji_card_layout(panel, panel_rect),
    ) {
        render_card(app, frame, body, proposal, card);
    }
}

fn render_card(
    app: &AppState,
    frame: &mut Frame,
    body: Rect,
    proposal: &RuleProposalView,
    layout: ShitsujiCardLayout,
) {
    let panel = &app.shitsuji_panel;
    let p = &app.palette;
    let content_x = body.x.saturating_add(1);
    let content_width = body.width.saturating_sub(2);
    // The footer offsets saturate together once the body cannot hold every row, so each label is
    // drawn only while it still sits above the row below it. The decision row always wins.
    let show_rule_label = layout.target_offset > 0;
    let show_target_label = layout.target_offset < layout.button_offset;
    let show_target_value = layout.target_offset.saturating_add(1) < layout.button_offset;
    if let Some(label_rect) =
        card_row_rect(body, 0, content_x, content_width).filter(|_| show_rule_label)
    {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "RULE",
                Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
            )),
            label_rect,
        );
    }
    if layout.rule_viewport > 0 {
        frame.render_widget(
            Paragraph::new(proposal.rule_text.as_str())
                .style(Style::default().fg(p.text))
                .wrap(Wrap { trim: true })
                .scroll((u16::try_from(panel.scroll).unwrap_or(u16::MAX), 0)),
            Rect::new(
                content_x,
                body.y
                    .saturating_add(saturating_u16(SHITSUJI_CARD_HEADER_ROWS)),
                content_width,
                saturating_u16(layout.rule_viewport),
            ),
        );
    }
    if let Some(target_rect) = card_row_rect(body, layout.target_offset, content_x, content_width)
        .filter(|_| show_target_label)
    {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "ASSIGNED AGENT",
                Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
            )),
            target_rect,
        );
    }
    if let Some(target_value_rect) = card_row_rect(
        body,
        layout.target_offset.saturating_add(1),
        content_x,
        content_width,
    )
    .filter(|_| show_target_value)
    {
        frame.render_widget(
            Paragraph::new(Span::styled(
                assigned_agent_label(&proposal.target_profile_id),
                Style::default().fg(p.mauve),
            )),
            target_value_rect,
        );
    }
    for hit in app.view.shitsuji_panel_hit_areas.decisions.iter() {
        let (label, color) = match hit.request.decision {
            RuleProposalDecision::Reject => ("[ R  Reject ]", p.red),
            RuleProposalDecision::Approve => ("[ A  Approve ]", p.green),
        };
        frame.render_widget(
            Paragraph::new(label)
                .alignment(ratatui::layout::Alignment::Center)
                .style(Style::default().fg(color)),
            hit.rect,
        );
    }
}

fn assigned_agent_label(target_profile_id: &str) -> &str {
    if target_profile_id == SHITSUJI_AGENT_PROFILE_ID {
        "Shitsuji Agent"
    } else {
        target_profile_id
    }
}

fn shitsuji_panel_body_height(panel_rect: Rect) -> usize {
    usize::from(panel_rect.height.saturating_sub(4))
}

fn shitsuji_rule_viewport_height(panel_rect: Rect) -> usize {
    shitsuji_panel_body_height(panel_rect)
        .saturating_sub(SHITSUJI_CARD_HEADER_ROWS)
        .saturating_sub(SHITSUJI_CARD_FOOTER_ROWS)
}

fn selected_shitsuji_card_layout(
    panel: &ShitsujiPanelState,
    panel_rect: Rect,
) -> Option<ShitsujiCardLayout> {
    let text_width = panel_rect.width.saturating_sub(4).max(1);
    let proposal = panel.proposals.get(panel.selected)?;
    let rule_lines = Paragraph::new(proposal.rule_text.as_str())
        .wrap(Wrap { trim: true })
        .line_count(text_width)
        .max(1);
    let body_height = shitsuji_panel_body_height(panel_rect);
    Some(ShitsujiCardLayout {
        rule_lines,
        rule_viewport: shitsuji_rule_viewport_height(panel_rect),
        target_offset: body_height.saturating_sub(4),
        button_offset: body_height.saturating_sub(1),
    })
}

fn card_row_rect(body: Rect, offset: usize, x: u16, width: u16) -> Option<Rect> {
    let offset = saturating_u16(offset);
    (offset < body.height).then(|| Rect::new(x, body.y.saturating_add(offset), width, 1))
}

fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_buffer(app: &AppState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
                .expect("test terminal");
        terminal
            .draw(|frame| render_shitsuji_panel(app, frame))
            .expect("render shitsuji panel");
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
        buffer.content().iter().map(|cell| cell.symbol()).collect()
    }

    fn proposal(id: &str, revision: u64) -> RuleProposalView {
        RuleProposalView {
            proposal_id: id.to_string(),
            rule_text: format!("Rule {id}"),
            target_profile_id: "shitsuji-agent".to_string(),
            revision,
        }
    }

    #[test]
    fn auto_and_manual_visibility_transitions_are_sticky() {
        let mut panel = ShitsujiPanelState::default();
        assert!(!panel.is_expanded());

        panel.replace_proposals(vec![proposal("one", 1)]);
        assert!(panel.is_expanded());
        panel.replace_proposals(Vec::new());
        assert!(!panel.is_expanded());

        panel.open_manually();
        assert!(panel.is_expanded());
        panel.replace_proposals(Vec::new());
        assert!(panel.is_expanded());

        panel.close_manually();
        panel.replace_proposals(vec![proposal("two", 2), proposal("three", 1)]);
        assert!(!panel.is_expanded());
        assert_eq!(panel.proposals.len(), 2);
    }

    #[test]
    fn deciding_last_proposal_collapses_but_manual_empty_view_can_reopen() {
        let mut panel = ShitsujiPanelState::default();
        panel.replace_proposals(vec![proposal("one", 1)]);
        panel.open_manually();

        panel.replace_proposals_after_decision(Vec::new());
        assert_eq!(
            panel.visibility,
            crate::app::state::ShitsujiPanelVisibility::Auto
        );
        assert!(!panel.is_expanded());
        assert!(!panel.keyboard_focused);

        panel.open_manually();
        assert!(panel.is_expanded());
        panel.replace_proposals(Vec::new());
        assert!(panel.is_expanded());
    }

    #[test]
    fn replacing_selected_proposal_resets_its_scroll() {
        let mut panel = ShitsujiPanelState::default();
        panel.replace_proposals(vec![proposal("one", 1), proposal("two", 1)]);
        panel.scroll = 5;

        panel.replace_proposals(vec![proposal("two", 1)]);

        assert_eq!(panel.selected, 0);
        assert_eq!(panel.scroll, 0);
    }

    #[test]
    fn a_surviving_selection_follows_its_proposal_and_keeps_its_scroll() {
        let mut panel = ShitsujiPanelState::default();
        panel.replace_proposals(vec![proposal("one", 1), proposal("two", 1)]);
        panel.selected = 1;
        panel.scroll = 5;

        panel.replace_proposals(vec![
            proposal("zero", 1),
            proposal("one", 1),
            proposal("two", 1),
        ]);

        assert_eq!(panel.selected, 2);
        assert_eq!(panel.scroll, 5);
    }

    #[test]
    fn docked_layout_keeps_terminal_and_panel_disjoint() {
        let layout = compute_shitsuji_panel_layout(Rect::new(20, 0, 100, 30), true, false);
        assert!(!layout.overlay);
        assert!(layout.terminal_rect.width >= SHITSUJI_PANEL_MIN_TERMINAL_WIDTH);
        assert!(layout.panel_rect.width >= SHITSUJI_PANEL_MIN_WIDTH);
        assert_eq!(
            layout.terminal_rect.x + layout.terminal_rect.width,
            layout.panel_rect.x
        );
    }

    #[test]
    fn narrow_and_mobile_layouts_use_overlay_without_shrinking_terminal() {
        for (area, mobile) in [
            (Rect::new(20, 0, 60, 20), false),
            (Rect::new(0, 0, 44, 20), true),
        ] {
            let layout = compute_shitsuji_panel_layout(area, true, mobile);
            assert!(layout.overlay);
            assert_eq!(layout.terminal_rect, area);
            assert!(layout.panel_rect.width <= area.width);
        }
    }

    #[test]
    fn collapsed_layout_leaves_the_terminal_whole_and_defers_handle_placement() {
        let area = Rect::new(26, 0, 74, 24);
        let layout = compute_shitsuji_panel_layout(area, false, false);
        assert_eq!(layout.terminal_rect, area);
        assert_eq!(layout.panel_rect, Rect::default());
    }

    #[test]
    fn a_collapsed_handle_sits_at_the_right_of_the_chrome_row() {
        let tab_bar = Rect::new(26, 4, 74, 1);
        let terminal = Rect::new(26, 5, 74, 20);
        let handle =
            shitsuji_panel_handle_slot(&ShitsujiPanelState::default(), tab_bar, 17, terminal).rect;

        assert_eq!(handle.width, SHITSUJI_PANEL_HANDLE_WIDTH);
        assert_eq!(handle.height, 1);
        assert_eq!(handle.y, tab_bar.y);
        assert_eq!(handle.x + handle.width, tab_bar.x + tab_bar.width);
        assert!(!handle.intersects(terminal));
    }

    #[test]
    fn a_chrome_row_too_narrow_to_spare_the_handle_keeps_all_of_its_columns() {
        let panel = ShitsujiPanelState::default();
        let terminal = Rect::new(0, 1, 40, 20);
        let min_content = 17;

        let roomy = Rect::new(0, 0, SHITSUJI_PANEL_HANDLE_WIDTH + min_content, 1);
        let roomy_slot = shitsuji_panel_handle_slot(&panel, roomy, min_content, terminal);
        assert_eq!(roomy_slot.reserved_width, SHITSUJI_PANEL_HANDLE_WIDTH);
        assert_eq!(roomy_slot.rect.y, roomy.y);

        let cramped = Rect {
            width: roomy.width - 1,
            ..roomy
        };
        let cramped_slot = shitsuji_panel_handle_slot(&panel, cramped, min_content, terminal);
        assert_eq!(
            cramped_slot.reserved_width, 0,
            "a row that cannot spare the handle must keep its columns"
        );
        assert_eq!(
            cramped_slot.rect.y, terminal.y,
            "the handle falls back to the terminal top row instead"
        );
    }

    #[test]
    fn a_missing_chrome_row_pins_the_handle_to_the_terminal_top_row() {
        let terminal = Rect::new(26, 5, 74, 20);
        for height in [1u16, 2, 7, 20, 40] {
            let terminal = Rect { height, ..terminal };
            let handle = shitsuji_panel_handle_slot(
                &ShitsujiPanelState::default(),
                Rect::default(),
                17,
                terminal,
            )
            .rect;
            assert_eq!(
                handle.y, terminal.y,
                "height {height} moved the handle off the top row"
            );
            assert_eq!(handle.x + handle.width, terminal.x + terminal.width);
        }
    }

    #[test]
    fn handle_reservation_is_the_handle_width_until_the_panel_expands() {
        let mut panel = ShitsujiPanelState::default();
        let chrome = Rect::new(0, 0, 60, 1);
        let terminal = Rect::new(0, 1, 60, 20);
        assert_eq!(
            shitsuji_panel_handle_slot(&panel, chrome, 17, terminal).reserved_width,
            SHITSUJI_PANEL_HANDLE_WIDTH
        );

        panel.open_manually();
        let expanded = shitsuji_panel_handle_slot(&panel, chrome, 17, terminal);
        assert_eq!(expanded.reserved_width, 0);
        assert_eq!(expanded.rect, Rect::default());
    }

    #[test]
    fn collapsed_handles_render_pending_count_and_a_readable_empty_label() {
        let mut app = AppState::test_new();
        app.view.shitsuji_panel_handle_rect = Rect::new(27, 0, 13, 1);
        app.shitsuji_panel
            .replace_proposals(vec![proposal("one", 1), proposal("two", 1)]);

        let pending = render_buffer(&app, 40, 5);
        assert!(buffer_text(&pending).contains("[‹ Rules 2]"));
        assert_eq!(pending[(29, 0)].style().fg, Some(app.palette.yellow));

        app.shitsuji_panel.replace_proposals(Vec::new());
        let empty = render_buffer(&app, 40, 5);
        assert!(buffer_text(&empty).contains("[‹ Rules]"));
        assert!(!buffer_text(&empty).contains("[‹ Rules 0]"));
        assert_eq!(empty[(31, 0)].style().fg, Some(app.palette.text));
    }

    #[test]
    fn collapsed_handle_fits_the_capped_pending_count() {
        let mut app = AppState::test_new();
        app.view.shitsuji_panel_handle_rect = Rect::new(0, 0, SHITSUJI_PANEL_HANDLE_WIDTH, 1);
        app.shitsuji_panel.replace_proposals(
            (0..100)
                .map(|idx| proposal(&format!("proposal-{idx}"), 1))
                .collect(),
        );

        let content = buffer_text(&render_buffer(&app, SHITSUJI_PANEL_HANDLE_WIDTH, 1));
        assert_eq!(content, "[‹ Rules 99+]");
    }

    #[test]
    fn a_short_handle_label_still_paints_every_cell_of_its_rect() {
        let mut app = AppState::test_new();
        app.view.shitsuji_panel_handle_rect = Rect::new(0, 0, SHITSUJI_PANEL_HANDLE_WIDTH, 1);
        app.shitsuji_panel.replace_proposals(Vec::new());

        let content = buffer_text(&render_buffer(&app, SHITSUJI_PANEL_HANDLE_WIDTH, 1));
        assert_eq!(content, "    [‹ Rules]");
    }

    #[test]
    fn decision_hit_areas_preserve_id_revision_and_do_not_overlap() {
        let mut panel = ShitsujiPanelState::default();
        panel.replace_proposals(vec![proposal("one", 7)]);
        let panel_rect = Rect::new(80, 3, 40, 20);
        let hits = compute_shitsuji_panel_hit_areas(&panel, panel_rect);
        assert_eq!(hits.decisions.len(), 2);
        assert_eq!(hits.close.y, panel_rect.y);
        assert_eq!(hits.decisions[0].request.proposal_id.as_str(), "one");
        assert_eq!(hits.decisions[0].request.expected_revision, 7);
        let left = hits.decisions[0].rect;
        let right = hits.decisions[1].rect;
        assert!(left.x + left.width <= right.x);
    }

    #[test]
    fn decision_hit_areas_only_target_selected_proposal() {
        let mut panel = ShitsujiPanelState::default();
        panel.replace_proposals(vec![proposal("one", 1), proposal("two", 1)]);
        panel.selected = 1;
        let hits = compute_shitsuji_panel_hit_areas(&panel, Rect::new(80, 0, 40, 20));
        assert_eq!(hits.decisions.len(), 2);
        assert!(hits
            .decisions
            .iter()
            .all(|hit| hit.request.proposal_id.as_str() == "two"));
    }

    #[test]
    fn expanded_panel_renders_only_selected_proposal_and_decision_colors() {
        let mut app = AppState::test_new();
        app.shitsuji_panel
            .replace_proposals(vec![proposal("one", 4), proposal("two", 7)]);
        app.shitsuji_panel.selected = 1;
        app.view.shitsuji_panel_rect = Rect::new(0, 0, 40, 20);
        app.view.shitsuji_panel_hit_areas =
            compute_shitsuji_panel_hit_areas(&app.shitsuji_panel, app.view.shitsuji_panel_rect);

        let buffer = render_buffer(&app, 40, 20);
        let content = buffer_text(&buffer);
        let top_row = (0..40).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
        let first_inner_row = (0..40).map(|x| buffer[(x, 1)].symbol()).collect::<String>();
        assert!(content.contains("Rule proposals"));
        assert!(content.contains("2 / 2"));
        assert!(content.contains("[›]"));
        assert!(top_row.contains("Rule proposals"));
        assert!(top_row.contains("2 / 2"));
        assert!(top_row.contains("[›]"));
        assert_eq!(buffer[(1, 0)].symbol(), "R");
        let close = app.view.shitsuji_panel_hit_areas.close;
        assert_eq!(buffer[(close.x, close.y)].symbol(), "[");
        assert!(first_inner_row.contains("Shitsuji Agent"));
        assert!(!first_inner_row.contains("2 / 2"));
        assert!(!first_inner_row.contains("[›]"));
        assert!(content.contains("RULE"));
        assert!(content.contains("Rule two"));
        assert!(!content.contains("Rule one"));
        assert!(content.contains("ASSIGNED AGENT"));
        assert!(content.contains("[ R  Reject ]"));
        assert!(content.contains("[ A  Approve ]"));

        let reject = app
            .view
            .shitsuji_panel_hit_areas
            .decisions
            .iter()
            .find(|hit| hit.request.decision == RuleProposalDecision::Reject)
            .expect("reject hit")
            .rect;
        let approve = app
            .view
            .shitsuji_panel_hit_areas
            .decisions
            .iter()
            .find(|hit| hit.request.decision == RuleProposalDecision::Approve)
            .expect("approve hit")
            .rect;
        assert!((reject.x..reject.x + reject.width)
            .any(|x| buffer[(x, reject.y)].style().fg == Some(app.palette.red)));
        assert!((approve.x..approve.x + approve.width)
            .any(|x| buffer[(x, approve.y)].style().fg == Some(app.palette.green)));
        for rect in [reject, approve] {
            for x in rect.x..rect.x.saturating_add(rect.width) {
                let style = buffer[(x, rect.y)].style();
                assert!(matches!(
                    style.bg,
                    None | Some(ratatui::style::Color::Reset)
                ));
                assert!(!style.add_modifier.contains(Modifier::REVERSED));
            }
        }
    }

    #[test]
    fn minimum_expanded_width_keeps_header_labels_disjoint() {
        let mut app = AppState::test_new();
        app.shitsuji_panel.replace_proposals(
            (0..100)
                .map(|idx| proposal(&format!("proposal-{idx}"), 1))
                .collect(),
        );
        app.shitsuji_panel.selected = 99;
        app.view.shitsuji_panel_rect = Rect::new(0, 0, SHITSUJI_PANEL_MIN_WIDTH, 12);
        app.view.shitsuji_panel_hit_areas =
            compute_shitsuji_panel_hit_areas(&app.shitsuji_panel, app.view.shitsuji_panel_rect);

        let buffer = render_buffer(&app, SHITSUJI_PANEL_MIN_WIDTH, 12);
        let top_row = (0..SHITSUJI_PANEL_MIN_WIDTH)
            .map(|x| buffer[(x, 0)].symbol())
            .collect::<String>();
        assert!(buffer_text(&buffer).contains("Shitsuji Agent"));
        assert_eq!(top_row, "┌Rules───────100 / 100──────────[›]┐");
    }

    #[test]
    fn header_heading_shrinks_only_when_the_position_would_overdraw_it() {
        assert_eq!(
            heading_for_title_width(SHITSUJI_PANEL_MIN_WIDTH, 0),
            "Rule proposals"
        );
        assert_eq!(
            heading_for_title_width(SHITSUJI_PANEL_EXPANDED_WIDTH, "1 / 9".len()),
            "Rule proposals"
        );
        assert_eq!(
            heading_for_title_width(SHITSUJI_PANEL_MIN_WIDTH, "100 / 100".len()),
            "Rules"
        );
        // Width 12 drops the position entirely, which frees the left side for the short heading.
        assert_eq!(heading_for_title_width(12, "10 / 10".len()), "Rules");
        assert_eq!(heading_for_title_width(6, 0), "");
    }

    #[test]
    fn a_position_that_the_chevron_would_clip_is_dropped() {
        assert!(position_shown(SHITSUJI_PANEL_MIN_WIDTH, "100 / 100".len()));
        assert!(!position_shown(16, "100 / 100".len()));
        assert!(!position_shown(SHITSUJI_PANEL_MIN_WIDTH, 0));
    }

    #[test]
    fn manually_open_empty_panel_explains_where_rules_will_appear() {
        let mut app = AppState::test_new();
        app.shitsuji_panel.open_manually();
        app.view.shitsuji_panel_rect = Rect::new(0, 0, 40, 12);
        app.view.shitsuji_panel_hit_areas =
            compute_shitsuji_panel_hit_areas(&app.shitsuji_panel, app.view.shitsuji_panel_rect);

        let content = buffer_text(&render_buffer(&app, 40, 12));
        assert!(content.contains("No pending rules"));
        assert!(content.contains("New proposals will appear here."));
    }

    #[test]
    fn narrow_wrapping_lengthens_the_rule_and_its_scroll_range() {
        let mut panel = ShitsujiPanelState::default();
        panel.replace_proposals(vec![RuleProposalView {
            proposal_id: "long".to_string(),
            rule_text:
                "verify every relevant provider boundary before approving the proposed rule "
                    .repeat(12),
            target_profile_id: "shitsuji-agent".to_string(),
            revision: 3,
        }]);

        let narrow_rect = Rect::new(0, 0, 20, 24);
        let wide_rect = Rect::new(0, 0, 40, 24);
        let narrow = selected_shitsuji_card_layout(&panel, narrow_rect).expect("narrow layout");
        let wide = selected_shitsuji_card_layout(&panel, wide_rect).expect("wide layout");
        assert!(narrow.rule_lines > wide.rule_lines);
        assert_eq!(narrow.rule_viewport, wide.rule_viewport);
        assert!(
            max_shitsuji_panel_scroll(&panel, narrow_rect)
                > max_shitsuji_panel_scroll(&panel, wide_rect)
        );
    }

    #[test]
    fn decision_row_stays_pinned_to_the_body_bottom_for_a_rule_taller_than_the_viewport() {
        let mut panel = ShitsujiPanelState::default();
        panel.replace_proposals(vec![RuleProposalView {
            proposal_id: "long".to_string(),
            rule_text: "wrap this rule across several drawer lines. ".repeat(6),
            target_profile_id: "shitsuji-agent".to_string(),
            revision: 3,
        }]);
        let panel_rect = Rect::new(0, 0, 40, 12);
        let card = selected_shitsuji_card_layout(&panel, panel_rect).expect("layout");
        assert!(card.rule_lines > card.rule_viewport);
        assert!(max_shitsuji_panel_scroll(&panel, panel_rect) > 0);

        let hits = compute_shitsuji_panel_hit_areas(&panel, panel_rect);
        assert_eq!(hits.decisions.len(), 2);
        let body_bottom = panel_rect.y + panel_rect.height - 2;
        for hit in &hits.decisions {
            assert_eq!(hit.rect.y, body_bottom);
        }
    }

    #[test]
    fn a_rule_viewport_squeezed_to_nothing_reports_no_scroll_range() {
        let mut panel = ShitsujiPanelState::default();
        panel.replace_proposals(vec![RuleProposalView {
            proposal_id: "long".to_string(),
            rule_text: "wrap this rule across several drawer lines. ".repeat(6),
            target_profile_id: "shitsuji-agent".to_string(),
            revision: 3,
        }]);
        let panel_rect = Rect::new(0, 0, 40, 10);

        let card = selected_shitsuji_card_layout(&panel, panel_rect).expect("layout");
        assert_eq!(card.rule_viewport, 0);
        assert_eq!(max_shitsuji_panel_scroll(&panel, panel_rect), 0);
        assert_eq!(
            compute_shitsuji_panel_hit_areas(&panel, panel_rect)
                .decisions
                .len(),
            2
        );
    }

    #[test]
    fn scrolling_a_long_rule_hides_the_lines_above_the_viewport() {
        let mut app = AppState::test_new();
        app.shitsuji_panel.replace_proposals(vec![RuleProposalView {
            proposal_id: "scroll".to_string(),
            rule_text: "HEAD\nMIDDLE\nTAIL".to_string(),
            target_profile_id: "shitsuji-agent".to_string(),
            revision: 1,
        }]);
        app.view.shitsuji_panel_rect = Rect::new(0, 0, 40, 13);
        app.shitsuji_panel.scroll =
            max_shitsuji_panel_scroll(&app.shitsuji_panel, app.view.shitsuji_panel_rect);
        app.view.shitsuji_panel_hit_areas =
            compute_shitsuji_panel_hit_areas(&app.shitsuji_panel, app.view.shitsuji_panel_rect);
        assert_eq!(app.shitsuji_panel.scroll, 1);

        let content = buffer_text(&render_buffer(&app, 40, 13));
        assert!(content.contains("MIDDLE"));
        assert!(content.contains("TAIL"));
        assert!(!content.contains("HEAD"));
    }

    #[test]
    fn a_body_too_short_for_every_row_still_keeps_the_decision_row() {
        for panel_height in 5..=12u16 {
            let mut app = AppState::test_new();
            app.shitsuji_panel
                .replace_proposals(vec![proposal("one", 1)]);
            app.view.shitsuji_panel_rect = Rect::new(0, 0, 40, panel_height);
            app.view.shitsuji_panel_hit_areas =
                compute_shitsuji_panel_hit_areas(&app.shitsuji_panel, app.view.shitsuji_panel_rect);

            let buffer = render_buffer(&app, 40, panel_height);
            let decision_row = (0..40)
                .map(|x| buffer[(x, panel_height - 2)].symbol())
                .collect::<String>();
            assert!(
                decision_row.contains("[ R  Reject ]") && decision_row.contains("[ A  Approve ]"),
                "panel height {panel_height} lost the decision row: {decision_row:?}"
            );
        }
    }

    #[test]
    fn a_non_default_profile_id_is_shown_verbatim() {
        assert_eq!(assigned_agent_label("shitsuji-agent"), "Shitsuji Agent");
        assert_eq!(assigned_agent_label("custom-reviewer"), "custom-reviewer");
    }
}
