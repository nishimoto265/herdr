use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::state::{
    AppState, ReviewDecisionHitArea, ReviewPanelHitAreas, ReviewPanelState, RuleProposalView,
};
use crate::review_agent::{RuleProposalDecision, RuleProposalDecisionRequest, RuleProposalId};

pub(crate) const REVIEW_PANEL_EXPANDED_WIDTH: u16 = 40;
pub(crate) const REVIEW_PANEL_MIN_WIDTH: u16 = 36;
pub(crate) const REVIEW_PANEL_RAIL_WIDTH: u16 = 3;
pub(crate) const REVIEW_PANEL_MIN_TERMINAL_WIDTH: u16 = 30;
const REVIEW_CARD_MIN_HEIGHT: usize = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReviewCardLayout {
    top: usize,
    height: usize,
    rule_lines: usize,
    target_offset: usize,
    button_offset: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ReviewPanelLayout {
    pub terminal_rect: Rect,
    pub panel_rect: Rect,
    pub rail_rect: Rect,
    pub overlay: bool,
}

pub(crate) fn compute_review_panel_layout(
    area: Rect,
    expanded: bool,
    mobile: bool,
) -> ReviewPanelLayout {
    if area.width == 0 || area.height == 0 {
        return ReviewPanelLayout::default();
    }

    if !expanded {
        let rail_width = REVIEW_PANEL_RAIL_WIDTH.min(area.width);
        let rail_rect = Rect::new(
            area.x.saturating_add(area.width.saturating_sub(rail_width)),
            area.y,
            rail_width,
            area.height,
        );
        return ReviewPanelLayout {
            terminal_rect: area,
            panel_rect: Rect::default(),
            rail_rect,
            overlay: true,
        };
    }

    let can_dock = !mobile
        && area.width >= REVIEW_PANEL_MIN_TERMINAL_WIDTH.saturating_add(REVIEW_PANEL_MIN_WIDTH);
    if can_dock {
        let panel_width = REVIEW_PANEL_EXPANDED_WIDTH
            .min(area.width.saturating_sub(REVIEW_PANEL_MIN_TERMINAL_WIDTH))
            .max(REVIEW_PANEL_MIN_WIDTH);
        let terminal_width = area.width.saturating_sub(panel_width);
        return ReviewPanelLayout {
            terminal_rect: Rect::new(area.x, area.y, terminal_width, area.height),
            panel_rect: Rect::new(
                area.x.saturating_add(terminal_width),
                area.y,
                panel_width,
                area.height,
            ),
            rail_rect: Rect::default(),
            overlay: false,
        };
    }

    let horizontal_margin = if area.width > REVIEW_PANEL_MIN_WIDTH {
        2
    } else {
        0
    };
    let vertical_margin = if area.height > 12 { 1 } else { 0 };
    let panel_width = REVIEW_PANEL_EXPANDED_WIDTH
        .min(area.width.saturating_sub(horizontal_margin * 2))
        .max(1);
    let panel_height = area.height.saturating_sub(vertical_margin * 2).max(1);
    ReviewPanelLayout {
        terminal_rect: area,
        panel_rect: Rect::new(
            area.x
                .saturating_add(area.width.saturating_sub(panel_width)),
            area.y.saturating_add(vertical_margin),
            panel_width,
            panel_height,
        ),
        rail_rect: Rect::default(),
        overlay: true,
    }
}

pub(crate) fn compute_review_panel_hit_areas(
    panel: &ReviewPanelState,
    panel_rect: Rect,
) -> ReviewPanelHitAreas {
    if panel_rect.width < 4 || panel_rect.height < 3 {
        return ReviewPanelHitAreas::default();
    }
    let inner = Rect::new(
        panel_rect.x.saturating_add(1),
        panel_rect.y.saturating_add(1),
        panel_rect.width.saturating_sub(2),
        panel_rect.height.saturating_sub(2),
    );
    let close = Rect::new(
        inner.x.saturating_add(inner.width.saturating_sub(3)),
        inner.y,
        3.min(inner.width),
        1,
    );
    let body_y = inner.y.saturating_add(2);
    let body_bottom = inner.y.saturating_add(inner.height);
    let mut decisions = Vec::new();
    for (proposal, card) in panel
        .proposals
        .iter()
        .zip(review_card_layouts(panel, panel_rect.width))
    {
        let screen_y = i64::from(body_y)
            .saturating_add(saturating_i64(card.top))
            .saturating_sub(saturating_i64(panel.scroll));
        let button_y = screen_y.saturating_add(saturating_i64(card.button_offset));
        if button_y < i64::from(body_y) || button_y >= i64::from(body_bottom) {
            continue;
        }
        let button_y = button_y as u16;
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
            decisions.push(ReviewDecisionHitArea {
                rect,
                request: RuleProposalDecisionRequest {
                    proposal_id: RuleProposalId::new(&proposal.proposal_id),
                    expected_revision: proposal.revision,
                    decision,
                },
            });
        }
    }
    ReviewPanelHitAreas { close, decisions }
}

pub(crate) fn max_review_panel_scroll(panel: &ReviewPanelState, panel_rect: Rect) -> usize {
    let body_height = panel_rect.height.saturating_sub(4) as usize;
    review_card_layouts(panel, panel_rect.width)
        .last()
        .map(|card| card.top.saturating_add(card.height))
        .unwrap_or(0)
        .saturating_sub(body_height)
}

pub(crate) fn selected_review_card_scroll(panel: &ReviewPanelState, panel_rect: Rect) -> usize {
    review_card_layouts(panel, panel_rect.width)
        .get(panel.selected)
        .map(|card| card.top)
        .unwrap_or(0)
}

pub(crate) fn review_panel_page_height(panel_rect: Rect) -> usize {
    panel_rect.height.saturating_sub(4).max(1) as usize
}

pub(super) fn render_review_panel(app: &AppState, frame: &mut Frame) {
    let panel = &app.review_panel;
    let p = &app.palette;
    let panel_rect = app.view.review_panel_rect;
    let rail_rect = app.view.review_panel_rail_rect;

    if rail_rect.width > 0 && rail_rect.height > 0 {
        let pending = panel.proposals.len();
        let badge = if pending > 99 {
            "99+".to_string()
        } else {
            pending.to_string()
        };
        let rail = Paragraph::new(vec![
            Line::from(Span::styled(" R ", Style::default().fg(p.mauve))),
            Line::from(Span::styled(
                format!("{badge:^3}"),
                Style::default().fg(if pending > 0 { p.yellow } else { p.overlay0 }),
            )),
        ])
        .block(
            Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(p.surface1)),
        );
        frame.render_widget(rail, rail_rect);
    }

    if panel_rect.width == 0 || panel_rect.height == 0 {
        return;
    }
    if app.view.review_panel_overlay {
        frame.render_widget(Clear, panel_rect);
    }
    let border_style = if panel.keyboard_focused {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface1)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            format!(" Rule proposals ({}) ", panel.proposals.len()),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ));
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
        Paragraph::new(Span::styled("Review Agent", Style::default().fg(p.mauve))),
        inner,
    );
    frame.render_widget(
        Paragraph::new(Span::styled("[×]", Style::default().fg(p.overlay1))),
        app.view.review_panel_hit_areas.close,
    );

    let body = Rect::new(
        inner.x,
        inner.y.saturating_add(2),
        inner.width,
        inner.height.saturating_sub(2),
    );
    if panel.proposals.is_empty() {
        frame.render_widget(
            Paragraph::new("No pending proposals")
                .style(Style::default().fg(p.overlay0))
                .wrap(Wrap { trim: true }),
            body,
        );
        return;
    }

    for (idx, (proposal, card)) in panel
        .proposals
        .iter()
        .zip(review_card_layouts(panel, panel_rect.width))
        .enumerate()
    {
        render_card(app, frame, body, idx, proposal, card);
    }
}

fn render_card(
    app: &AppState,
    frame: &mut Frame,
    body: Rect,
    idx: usize,
    proposal: &RuleProposalView,
    layout: ReviewCardLayout,
) {
    let panel = &app.review_panel;
    let p = &app.palette;
    let screen_y = i64::from(body.y)
        .saturating_add(saturating_i64(layout.top))
        .saturating_sub(saturating_i64(panel.scroll));
    if screen_y.saturating_add(saturating_i64(layout.height)) <= i64::from(body.y)
        || screen_y >= i64::from(body.y.saturating_add(body.height))
    {
        return;
    }
    let Some(card) = clipped_vertical_rect(
        body.x,
        screen_y,
        body.width,
        saturating_u16(layout.height),
        body,
    ) else {
        return;
    };
    let selected = panel.selected == idx;
    if screen_y >= i64::from(body.y) {
        frame.render_widget(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(if selected { p.accent } else { p.surface0 })),
            card,
        );
    }
    let content_x = body.x.saturating_add(1);
    let content_width = body.width.saturating_sub(2);
    if let Some(rule_rect) = clipped_vertical_rect(
        content_x,
        screen_y.saturating_add(1),
        content_width,
        saturating_u16(layout.rule_lines),
        body,
    ) {
        let hidden_rule_lines = i64::from(rule_rect.y)
            .saturating_sub(screen_y.saturating_add(1))
            .max(0);
        frame.render_widget(
            Paragraph::new(proposal.rule_text.as_str())
                .style(Style::default().fg(p.text))
                .wrap(Wrap { trim: true })
                .scroll((u16::try_from(hidden_rule_lines).unwrap_or(u16::MAX), 0)),
            rule_rect,
        );
    }
    if let Some(target_rect) = clipped_vertical_rect(
        content_x,
        screen_y.saturating_add(saturating_i64(layout.target_offset)),
        content_width,
        1,
        body,
    ) {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Target  ", Style::default().fg(p.overlay0)),
                Span::styled(
                    proposal.target_profile_id.as_str(),
                    Style::default().fg(p.mauve),
                ),
            ])),
            target_rect,
        );
    }
    let hits = compute_review_panel_hit_areas(panel, app.view.review_panel_rect);
    for hit in hits
        .decisions
        .iter()
        .filter(|hit| hit.request.proposal_id.as_str() == proposal.proposal_id)
    {
        let (label, color) = match hit.request.decision {
            RuleProposalDecision::Reject => ("Reject", p.red),
            RuleProposalDecision::Approve => ("Approve", p.green),
        };
        frame.render_widget(
            Paragraph::new(format!("[ {label} ]"))
                .alignment(ratatui::layout::Alignment::Center)
                .style(Style::default().fg(color)),
            hit.rect,
        );
    }
}

fn review_card_layouts(panel: &ReviewPanelState, panel_width: u16) -> Vec<ReviewCardLayout> {
    let text_width = panel_width.saturating_sub(4).max(1);
    let mut top = 0usize;
    panel
        .proposals
        .iter()
        .map(|proposal| {
            let rule_lines = Paragraph::new(proposal.rule_text.as_str())
                .wrap(Wrap { trim: true })
                .line_count(text_width)
                .max(1);
            let height = rule_lines.saturating_add(4).max(REVIEW_CARD_MIN_HEIGHT);
            let card = ReviewCardLayout {
                top,
                height,
                rule_lines,
                target_offset: height.saturating_sub(3),
                button_offset: height.saturating_sub(2),
            };
            top = top.saturating_add(height);
            card
        })
        .collect()
}

fn saturating_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

fn clipped_vertical_rect(x: u16, y: i64, width: u16, height: u16, clip: Rect) -> Option<Rect> {
    let start = y.max(i64::from(clip.y));
    let end = y
        .saturating_add(i64::from(height))
        .min(i64::from(clip.y.saturating_add(clip.height)));
    if start >= end {
        return None;
    }
    Some(Rect::new(x, start as u16, width, (end - start) as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(id: &str, revision: u64) -> RuleProposalView {
        RuleProposalView {
            proposal_id: id.to_string(),
            rule_text: format!("Rule {id}"),
            target_profile_id: "review-agent".to_string(),
            revision,
        }
    }

    #[test]
    fn auto_and_manual_visibility_transitions_are_sticky() {
        let mut panel = ReviewPanelState::default();
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
        let mut panel = ReviewPanelState::default();
        panel.replace_proposals(vec![proposal("one", 1)]);
        panel.open_manually();

        panel.replace_proposals_after_decision(Vec::new());
        assert_eq!(
            panel.visibility,
            crate::app::state::ReviewPanelVisibility::Auto
        );
        assert!(!panel.is_expanded());
        assert!(!panel.keyboard_focused);

        panel.open_manually();
        assert!(panel.is_expanded());
        panel.replace_proposals(Vec::new());
        assert!(panel.is_expanded());
    }

    #[test]
    fn docked_layout_keeps_terminal_and_panel_disjoint() {
        let layout = compute_review_panel_layout(Rect::new(20, 0, 100, 30), true, false);
        assert!(!layout.overlay);
        assert!(layout.terminal_rect.width >= REVIEW_PANEL_MIN_TERMINAL_WIDTH);
        assert!(layout.panel_rect.width >= REVIEW_PANEL_MIN_WIDTH);
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
            let layout = compute_review_panel_layout(area, true, mobile);
            assert!(layout.overlay);
            assert_eq!(layout.terminal_rect, area);
            assert!(layout.panel_rect.width <= area.width);
        }
    }

    #[test]
    fn collapsed_rail_overlays_without_resizing_terminal() {
        let area = Rect::new(26, 0, 74, 24);
        let layout = compute_review_panel_layout(area, false, false);
        assert_eq!(layout.terminal_rect, area);
        assert_eq!(layout.rail_rect.width, REVIEW_PANEL_RAIL_WIDTH);
        assert_eq!(
            layout.rail_rect.x + layout.rail_rect.width,
            area.x + area.width
        );
    }

    #[test]
    fn decision_hit_areas_preserve_id_revision_and_do_not_overlap() {
        let mut panel = ReviewPanelState::default();
        panel.replace_proposals(vec![proposal("one", 7)]);
        let hits = compute_review_panel_hit_areas(&panel, Rect::new(80, 0, 40, 20));
        assert_eq!(hits.decisions.len(), 2);
        assert_eq!(hits.decisions[0].request.proposal_id.as_str(), "one");
        assert_eq!(hits.decisions[0].request.expected_revision, 7);
        let left = hits.decisions[0].rect;
        let right = hits.decisions[1].rect;
        assert!(left.x + left.width <= right.x);
    }

    #[test]
    fn scrolled_card_hit_areas_follow_document_offset() {
        let mut panel = ReviewPanelState::default();
        panel.replace_proposals(vec![proposal("one", 1), proposal("two", 1)]);
        panel.scroll = 6;
        let hits = compute_review_panel_hit_areas(&panel, Rect::new(80, 0, 40, 20));
        let first = hits
            .decisions
            .iter()
            .find(|hit| hit.request.proposal_id.as_str() == "one")
            .expect("first card remains partially visible");
        let second = hits
            .decisions
            .iter()
            .find(|hit| hit.request.proposal_id.as_str() == "two")
            .expect("second card is visible");
        assert_eq!(first.rect.y, 4);
        assert_eq!(second.rect.y, 13);
    }

    #[test]
    fn vertical_clip_handles_cards_scrolled_above_origin() {
        let clip = Rect::new(10, 3, 20, 10);
        assert_eq!(
            clipped_vertical_rect(10, -3, 20, 9, clip),
            Some(Rect::new(10, 3, 20, 3))
        );
        assert_eq!(clipped_vertical_rect(10, -9, 20, 6, clip), None);
    }

    #[test]
    fn long_rule_wraps_into_variable_card_and_moves_buttons_and_scroll_limit() {
        let mut panel = ReviewPanelState::default();
        panel.replace_proposals(vec![RuleProposalView {
            proposal_id: "long".to_string(),
            rule_text:
                "verify every relevant provider boundary before approving the proposed rule "
                    .repeat(12),
            target_profile_id: "review-agent".to_string(),
            revision: 3,
        }]);

        let narrow = review_card_layouts(&panel, 20)[0];
        let wide = review_card_layouts(&panel, 40)[0];
        assert!(narrow.rule_lines > wide.rule_lines);
        assert!(narrow.height > wide.height);

        let tall_rect = Rect::new(0, 0, 20, saturating_u16(narrow.height.saturating_add(6)));
        let approve = compute_review_panel_hit_areas(&panel, tall_rect)
            .decisions
            .into_iter()
            .find(|hit| hit.request.decision == RuleProposalDecision::Approve)
            .expect("long rule approve button");
        assert_eq!(
            approve.rect.y,
            3u16.saturating_add(saturating_u16(narrow.button_offset))
        );

        let viewport = Rect::new(0, 0, 20, 12);
        assert_eq!(
            max_review_panel_scroll(&panel, viewport),
            narrow.height.saturating_sub(8)
        );
    }

    #[test]
    fn scrolling_long_rule_renders_lines_hidden_above_viewport() {
        let mut app = AppState::test_new();
        app.review_panel.replace_proposals(vec![RuleProposalView {
            proposal_id: "scroll".to_string(),
            rule_text: "HEAD\nMIDDLE\nTAIL".to_string(),
            target_profile_id: "review-agent".to_string(),
            revision: 1,
        }]);
        app.review_panel.scroll = 2;
        app.view.review_panel_rect = Rect::new(0, 0, 40, 12);
        app.view.review_panel_hit_areas =
            compute_review_panel_hit_areas(&app.review_panel, app.view.review_panel_rect);

        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 12))
            .expect("test terminal");
        terminal
            .draw(|frame| render_review_panel(&app, frame))
            .expect("render review panel");
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.contains("MIDDLE"));
        assert!(content.contains("TAIL"));
    }
}
