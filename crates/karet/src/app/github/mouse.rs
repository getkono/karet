//! Mouse handling for GitHub dashboard tabs.

use super::*;

impl App {
    /// Handle a click or wheel gesture within the active GitHub dashboard table.
    pub(in crate::app) fn github_mouse(&mut self, mouse: MouseEvent) -> bool {
        if self
            .tabs
            .get(self.active)
            .is_some_and(|tab| matches!(tab.kind, TabKind::Github(GithubViewState::PullRequest(_))))
        {
            return self.github_pull_request_mouse(mouse);
        }
        let point = (mouse.column, mouse.row);
        let Some((section_hit, query_hit, auth_hit, table_rect, first_visible, row_count)) =
            self.active_dashboard_mut().map(|dashboard| {
                (
                    dashboard.section_hits.iter().find_map(|(section, rect)| {
                        rect_contains(*rect, point).then_some(*section)
                    }),
                    rect_contains(dashboard.query_rect, point),
                    rect_contains(dashboard.auth_rect, point),
                    dashboard.table_rect,
                    dashboard.first_visible,
                    dashboard.row_count(),
                )
            })
        else {
            return false;
        };

        if let Some(section) = section_hit {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                self.set_github_section(section);
            }
            return true;
        }
        if query_hit {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                && let Some(dashboard) = self.active_dashboard_mut()
                && dashboard.section != GithubSection::Actions
            {
                dashboard.query_focused = true;
            }
            return true;
        }
        if auth_hit {
            if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
                && let Some(dashboard) = self.active_dashboard_mut()
                && !dashboard.auth.can_write
                && dashboard.login_pending.is_none()
            {
                dashboard.login_editing = true;
                dashboard.login_token.clear();
                dashboard.error = None;
            }
            return true;
        }
        if !rect_contains(table_rect, point) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => self.github_move_cursor(3, false),
            MouseEventKind::ScrollUp => self.github_move_cursor(-3, false),
            MouseEventKind::Down(MouseButton::Left) => {
                let row = first_visible
                    + usize::from(mouse.row.saturating_sub(table_rect.y)) / DASHBOARD_ROW_HEIGHT;
                if row < row_count {
                    let modified = mouse
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::SHIFT);
                    let open = !modified && self.click_streak(mouse.column, mouse.row) >= 2;
                    if let Some(dashboard) = self.active_dashboard_mut() {
                        if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                            let (start, end) = if dashboard.cursor <= row {
                                (dashboard.cursor, row)
                            } else {
                                (row, dashboard.cursor)
                            };
                            dashboard.selected.extend(start..=end);
                        } else if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                            if !dashboard.selected.remove(&row) {
                                dashboard.selected.insert(row);
                            }
                        } else {
                            dashboard.selected.clear();
                            dashboard.selected.insert(row);
                        }
                        dashboard.cursor = row;
                    }
                    if open {
                        self.open_github_selection();
                    }
                }
            },
            _ => {},
        }
        true
    }
}
