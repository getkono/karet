use super::*;

/// Read-only sides and view state for a dedicated three-way merge-conflict editor.
pub(crate) struct MergeConflictState {
    /// Current-branch content (Git index stage 2), once loaded.
    pub(crate) current: Option<TextBuffer>,
    /// Incoming-branch content (Git index stage 3), once loaded.
    pub(crate) incoming: Option<TextBuffer>,
    /// Independent renderer state for the read-only current side.
    pub(crate) current_editor: EditorState,
    /// Independent renderer state for the read-only incoming side.
    pub(crate) incoming_editor: EditorState,
    /// When side loading began, for the shared delayed-loading policy.
    pub(crate) loading_since: Pending,
    /// Backend load failure, if the path stopped being a text conflict.
    pub(crate) error: Option<String>,
}

impl MergeConflictState {
    /// Reserve a conflict view while the two committed sides load.
    #[must_use]
    pub(crate) fn loading() -> Self {
        Self {
            current: None,
            incoming: None,
            current_editor: EditorState::new(),
            incoming_editor: EditorState::new(),
            loading_since: Pending::start(),
            error: None,
        }
    }

    /// Install both committed sides after the backend answers.
    pub(crate) fn finish(&mut self, current: String, incoming: String) {
        self.current = Some(TextBuffer::from_text(&current));
        self.incoming = Some(TextBuffer::from_text(&incoming));
        self.error = None;
    }
}
