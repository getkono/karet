//! Keyboard handling for GitHub dashboard tabs.

use super::*;

impl App {
    /// Handle dashboard and form keys before the ordinary editor keymap.
    pub(in crate::app) fn github_key(&mut self, key: KeyEvent) -> bool {
        if self.focus != Focus::Editor {
            return false;
        }
        if self.view != View::GitHub || !self.github.is_active() {
            return false;
        }
        if self.github_form_key(key) {
            return true;
        }
        if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return self.refresh_active_github();
        }
        if self.github_pull_request_key(key) {
            return true;
        }
        if self.active_dashboard_mut().is_none() {
            // A detail page scrolls *itself*. `self.scroll_lines` walks the active
            // tab, which is a document the GitHub view is drawn over — scrolling it
            // would move something the user cannot see.
            return match key.code {
                KeyCode::Esc => self.close_github_page(),
                KeyCode::Down | KeyCode::Char('j') => self.scroll_github_page(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll_github_page(-1),
                KeyCode::PageDown => self.scroll_github_page(12),
                KeyCode::PageUp => self.scroll_github_page(-12),
                KeyCode::Home => self.scroll_github_page_edge(true),
                KeyCode::End => self.scroll_github_page_edge(false),
                _ => false,
            };
        }
        let query_focused = self.active_dashboard_mut().is_some_and(|d| d.query_focused);
        let login_editing = self
            .active_dashboard_mut()
            .is_some_and(|dashboard| dashboard.login_editing);
        if login_editing {
            match key.code {
                KeyCode::Esc => {
                    if let Some(dashboard) = self.active_dashboard_mut() {
                        dashboard.login_editing = false;
                        dashboard.login_token.clear();
                    }
                },
                KeyCode::Enter => self.submit_github_login(),
                KeyCode::Backspace => {
                    if let Some(dashboard) = self.active_dashboard_mut() {
                        dashboard.login_token.pop();
                    }
                },
                KeyCode::Char(character)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    if let Some(dashboard) = self.active_dashboard_mut() {
                        dashboard.login_token.push(character);
                    }
                },
                _ => {},
            }
            return true;
        }
        if query_focused {
            match key.code {
                KeyCode::Esc => self.active_dashboard_mut().map(|d| d.query_focused = false),
                KeyCode::Enter => {
                    if let Some(d) = self.active_dashboard_mut() {
                        d.query_focused = false;
                        d.reset_navigation();
                    }
                    self.request_github_section();
                    None
                },
                KeyCode::Backspace => self.active_dashboard_mut().map(|d| {
                    d.query.pop();
                }),
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.active_dashboard_mut().map(|d| d.query.push(c))
                },
                _ => None,
            };
            return true;
        }
        match key.code {
            KeyCode::Char('1') => self.set_github_section(GithubSection::Issues),
            KeyCode::Char('2') => self.set_github_section(GithubSection::PullRequests),
            KeyCode::Char('3') => self.set_github_section(GithubSection::Actions),
            KeyCode::Char('/') => {
                if let Some(d) = self.active_dashboard_mut() {
                    d.query_focused = true;
                }
            },
            KeyCode::Char('r') => self.request_github_section(),
            KeyCode::Char('l') => {
                if let Some(dashboard) = self.active_dashboard_mut()
                    && !dashboard.auth.can_write
                    && dashboard.login_pending.is_none()
                {
                    dashboard.login_editing = true;
                    dashboard.login_token.clear();
                    dashboard.error = None;
                }
            },
            KeyCode::Char('n') => self.open_github_creation_form(),
            KeyCode::Down | KeyCode::Char('j') => {
                self.github_move_cursor(1, key.modifiers.contains(KeyModifiers::SHIFT));
            },
            KeyCode::Up | KeyCode::Char('k') => {
                self.github_move_cursor(-1, key.modifiers.contains(KeyModifiers::SHIFT));
            },
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(d) = self.active_dashboard_mut() {
                    d.selected = (0..d.row_count()).collect();
                }
            },
            KeyCode::Char(' ') => {
                if let Some(d) = self.active_dashboard_mut()
                    && !d.selected.remove(&d.cursor)
                {
                    d.selected.insert(d.cursor);
                }
            },
            KeyCode::Enter => self.open_github_selection(),
            _ => return false,
        }
        true
    }
}
