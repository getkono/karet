use unicode_width::UnicodeWidthStr;

use super::model::*;
use super::*;

/// A gitignore-aware file tree with per-file-type icons and a git-status overlay.
pub struct FileTree<'a> {
    root: &'a Path,
    status: &'a [(PathBuf, Decoration)],
    badges: &'a [(PathBuf, String)],
    visible: &'a [PathBuf],
    active: Option<&'a Path>,
    cut_paths: &'a [PathBuf],
    explorer_focused: bool,
    hover: Option<usize>,
    icons: IconStyle,
    theme: Option<&'a Theme>,
}

impl<'a> FileTree<'a> {
    /// Build a file tree rooted at `root`.
    #[must_use]
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            status: &[],
            badges: &[],
            visible: &[],
            active: None,
            cut_paths: &[],
            explorer_focused: false,
            hover: None,
            icons: IconStyle::default(),
            theme: None,
        }
    }

    /// Supply the (absolute) row index the mouse is hovering, so it gets a secondary
    /// highlight distinct from the selection.
    #[must_use]
    pub fn hover(mut self, hover: Option<usize>) -> Self {
        self.hover = hover;
        self
    }

    /// Supply the paths of files shown in *other* (non-focused) editor panes — i.e.
    /// each background pane's active tab. Their rows get the accent foreground (a
    /// weaker tier than [`active`](Self::active)). Files that are merely open in a
    /// background tab (not the visible tab of any pane) are intentionally omitted, so
    /// opening a file no longer dims/recolors its explorer row.
    #[must_use]
    pub fn visible(mut self, visible: &'a [PathBuf]) -> Self {
        self.visible = visible;
        self
    }

    /// Supply the path of the focused editor pane's active tab, so its row gets the
    /// strongest highlight (a distinct background plus a bold accent) — the "you are
    /// here" marker VS Code shows for the active file. When collapsed directories
    /// hide the file row, the deepest visible directory ancestor is highlighted.
    #[must_use]
    pub fn active(mut self, active: Option<&'a Path>) -> Self {
        self.active = active;
        self
    }

    /// Supply paths currently marked as cut in the explorer file clipboard.
    #[must_use]
    pub fn cut_paths(mut self, paths: &'a [PathBuf]) -> Self {
        self.cut_paths = paths;
        self
    }

    /// Whether the explorer panel currently holds keyboard focus. The tree's own
    /// selection (cursor / last click) only gets the [`Selection`](ThemeRole::Selection)
    /// background while it does, so it stops competing with the active-file highlight
    /// once focus moves to the editor.
    #[must_use]
    pub fn explorer_focused(mut self, focused: bool) -> Self {
        self.explorer_focused = focused;
        self
    }

    /// Supply a path-keyed status overlay (e.g. from `karet-vcs`).
    #[must_use]
    pub fn status(mut self, status: &'a [(PathBuf, Decoration)]) -> Self {
        self.status = status;
        self
    }

    /// Supply muted, right-aligned badges keyed by directory path.
    #[must_use]
    pub fn badges(mut self, badges: &'a [(PathBuf, String)]) -> Self {
        self.badges = badges;
        self
    }

    /// Choose the icon style (Nerd Font / Unicode / ASCII).
    #[must_use]
    pub fn icons(mut self, icons: IconStyle) -> Self {
        self.icons = icons;
        self
    }

    /// Supply the active theme.
    #[must_use]
    pub fn theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// The status decoration for `path`, if any.
    fn status_for(&self, path: &Path) -> Option<&Decoration> {
        self.status.iter().find(|(p, _)| p == path).map(|(_, d)| d)
    }
}

/// Map a file [`Category`] to the explorer icon-tint role: text-like types share
/// one tint, media and documents another, binaries/archives a third, and everything
/// unrecognized falls back to the neutral [`Foreground`](ThemeRole::Foreground).
fn category_role(category: Category) -> ThemeRole {
    match category {
        Category::Code | Category::Markup | Category::Data | Category::Config | Category::Shell => {
            ThemeRole::FileIconText
        },
        Category::Image | Category::Document => ThemeRole::FileIconMedia,
        Category::Archive | Category::Binary => ThemeRole::FileIconBinary,
        // Unknown — and any future Category variant — stays neutral.
        _ => ThemeRole::Foreground,
    }
}

/// Find the most specific visible row representing the active path.
fn active_row_index(rows: &[FileTreeRow], active: Option<&Path>) -> Option<usize> {
    let active = active?;
    rows.iter()
        .enumerate()
        .filter(|(_, row)| row.path == active || row.is_dir && active.starts_with(&row.path))
        .max_by_key(|(_, row)| row.path.components().count())
        .map(|(index, _)| index)
}

impl StatefulWidget for FileTree<'_> {
    type State = FileTreeState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut FileTreeState) {
        state.ensure_built(self.root);
        let height = area.height as usize;
        if area.width == 0 || height == 0 {
            return;
        }

        // Keep the cursor within the viewport.
        let cursor = state.selection.cursor();
        if cursor < state.offset {
            state.offset = cursor;
        } else if cursor >= state.offset + height {
            state.offset = cursor + 1 - height;
        }

        let fallback;
        let theme = match self.theme {
            Some(theme) => theme,
            None => {
                fallback = Theme::dark();
                &fallback
            },
        };
        let fg = theme.role(ThemeRole::Foreground);
        let guide = theme.role(ThemeRole::IndentGuide);
        let muted = theme.role(ThemeRole::Muted);
        let accent = theme.role(ThemeRole::LineNumberActive);
        let active_row = active_row_index(&state.rows, self.active);

        for (i, row) in state
            .rows
            .iter()
            .enumerate()
            .skip(state.offset)
            .take(height)
        {
            let y = area.y + u16::try_from(i - state.offset).unwrap_or(0);
            // Which editor(s) show this file drives the highlight: the focused pane's
            // active file is strongest, a file visible in another pane is weaker.
            let is_active = active_row == Some(i);
            let is_visible = self.visible.iter().any(|p| p == &row.path);
            let selected = state.selection.is_selected(i);
            let cut = self.cut_paths.iter().any(|p| p == &row.path);
            // Background precedence: the focused pane's active file ("you are here")
            // wins; then the explorer's own cursor — but only while the explorer holds
            // focus, so the last-clicked row doesn't linger once you return to editing;
            // then the transient mouse-hover accent.
            let row_bg = if is_active {
                Some(ThemeRole::ActiveEditorRow)
            } else if selected {
                if self.explorer_focused {
                    Some(ThemeRole::Selection)
                } else {
                    Some(ThemeRole::HoverHighlight)
                }
            } else if self.hover == Some(i) {
                Some(ThemeRole::HoverHighlight)
            } else {
                None
            };
            if let Some(role) = row_bg {
                buf.set_style(
                    Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    },
                    Style::default().bg(theme.role(role).to_ratatui()),
                );
            }

            // Layout: indent, an expand chevron (directories only), then the
            // type icon (folder / per-file-type), then the label. Files leave the
            // chevron column blank so names stay aligned under directories.
            let chev = if row.is_dir {
                chevron(row.expanded, self.icons)
            } else {
                ' '
            };
            let icon = if row.is_symlink {
                UiIcon::Symlink.glyph(self.icons)
            } else if row.is_dir {
                directory_icon(row.expanded, self.icons).unwrap_or(' ')
            } else {
                icon_for_path(&row.path, self.icons)
            };

            // Foreground precedence: the focused pane's active file is accented and
            // bold; a file visible in another pane is accented; gitignored entries
            // recede to a readable muted grey (VS Code style); everything else — a
            // merely-open background tab included — is normal.
            let (row_fg, label_style) = if cut {
                (muted, Style::default().fg(muted.to_ratatui()))
            } else if is_active {
                (
                    accent,
                    Style::default()
                        .fg(accent.to_ratatui())
                        .add_modifier(Modifier::BOLD),
                )
            } else if is_visible {
                (accent, Style::default().fg(accent.to_ratatui()))
            } else if row.ignored {
                (muted, Style::default().fg(muted.to_ratatui()))
            } else {
                (fg, Style::default().fg(fg.to_ratatui()))
            };
            // The type icon is tinted by file Category (text / media / binary /
            // neutral); directories follow the row color, and gitignored entries
            // recede to muted so the whole row dims together.
            let icon_color = if row.ignored || cut {
                muted
            } else if row.is_dir || row.is_symlink {
                row_fg
            } else {
                theme.role(category_role(category_for_path(&row.path)))
            };
            // Indent guides: one vertical rule per ancestor depth level. Rows are
            // flattened depth-first, so a rule at each ancestor column draws a
            // continuous line down every expanded directory's children.
            let mut spans = Vec::with_capacity(row.depth as usize + 3);
            for _ in 0..row.depth {
                spans.push(Span::styled(
                    "\u{2502} ", // "│ " — box-drawing rule + spacer, 2 cells per level
                    Style::default().fg(guide.to_ratatui()),
                ));
            }
            spans.push(Span::styled(
                format!("{chev} "),
                Style::default().fg(row_fg.to_ratatui()),
            ));
            spans.push(Span::styled(
                format!("{icon} "),
                Style::default().fg(icon_color.to_ratatui()),
            ));
            if row.editing {
                push_editing_spans(
                    &mut spans,
                    state.editing.as_ref(),
                    accent.to_ratatui(),
                    theme.role(ThemeRole::Selection).to_ratatui(),
                );
            } else {
                spans.push(Span::styled(row.label.clone(), label_style));
                if let Some(dec) = self.status_for(&row.path)
                    && let DecorationKind::GutterMarker { glyph } = &dec.kind
                {
                    let color = dec.role.map_or(fg, |r| theme.role(r));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        glyph.to_string(),
                        Style::default().fg(color.to_ratatui()),
                    ));
                }
            }

            buf.set_line(area.x, y, &Line::from(spans), area.width);
            if let Some((_, badge)) = self.badges.iter().find(|(path, _)| path == &row.path) {
                let badge_width = u16::try_from(badge.width()).unwrap_or(u16::MAX);
                if badge_width > 0 && badge_width.saturating_add(1) < area.width {
                    let x = area.right().saturating_sub(badge_width);
                    buf.set_stringn(
                        x.saturating_sub(1),
                        y,
                        " ",
                        1,
                        Style::default().fg(muted.to_ratatui()),
                    );
                    buf.set_stringn(
                        x,
                        y,
                        badge,
                        badge.width(),
                        Style::default().fg(muted.to_ratatui()),
                    );
                }
            }
        }
    }
}
