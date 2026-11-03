//! What a tab reserved for bytes shows while they have not arrived.
//!
//! The shared loading policy (`CLAUDE.md`, "UI loading states"): nothing while
//! the read is young, a muted line once it has been pending past the reveal
//! delay, and the read's failure once one is known — never the placeholder an
//! unreadable file settles at, which would announce a failure that has not
//! happened.

use super::*;

/// The notice for `tab`: `None` for a tab not reserved for bytes, `Some(None)`
/// while its read is too young to announce, and otherwise the line to show with
/// its style.
pub(super) fn notice(tab: &Tab) -> Option<Option<(String, ThemeRole)>> {
    let path = match &tab.kind {
        TabKind::Hex { path, bytes, .. } if bytes.is_empty() => path,
        TabKind::Placeholder { path, .. } => path,
        _ => return None,
    };
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    if let Some(error) = &tab.fill_error {
        return Some(Some((
            format!("Could not read {name}\n{error}"),
            ThemeRole::DiagnosticError,
        )));
    }
    let since = tab.fill_since?;
    Some(
        since
            .visible()
            .then(|| (format!("Loading {name}…"), ThemeRole::Muted)),
    )
}

/// Paint `notice` centred in `area`; nothing at all while it is `None`.
pub(super) fn draw(f: &mut Frame, theme: &Theme, area: Rect, notice: Option<(String, ThemeRole)>) {
    if let Some((text, role)) = notice {
        f.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Center)
                .style(theme.style(role)),
            area,
        );
    }
}
