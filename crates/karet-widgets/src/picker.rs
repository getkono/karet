//! An incremental fuzzy picker over labeled items.
//!
//! The one picker every palette-style surface uses: type to filter (nucleo
//! fuzzy matching with smart case, via `karet-fuzzy`), best match first, with
//! the selection on the shared [`ListSelection`] model. What accepting an item
//! *does* is the consumer's choice of `T`.

use karet_fuzzy::Matcher;

use crate::select::ListSelection;

/// An incremental fuzzy picker over labeled items of type `T`.
pub struct Picker<T> {
    title: String,
    query: String,
    items: Vec<(String, T)>,
    /// Indices into `items` for the current query, best match first.
    filtered: Vec<usize>,
    selection: ListSelection,
    matcher: Matcher,
}

impl<T> Picker<T> {
    /// Build a picker titled `title` over `items` (label + value).
    #[must_use]
    pub fn new(title: impl Into<String>, items: Vec<(String, T)>) -> Self {
        let filtered: Vec<usize> = (0..items.len()).collect();
        Self {
            title: title.into(),
            query: String::new(),
            selection: ListSelection::new(items.len()),
            items,
            filtered,
            matcher: Matcher::new(),
        }
    }

    /// The picker title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The current query string.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The visible (filtered) row labels, best match first.
    #[must_use]
    pub fn rows(&self) -> Vec<&str> {
        self.filtered
            .iter()
            .map(|&i| self.items[i].0.as_str())
            .collect()
    }

    /// The visible (filtered) row values, aligned with [`rows`](Self::rows).
    #[must_use]
    pub fn values(&self) -> Vec<&T> {
        self.filtered.iter().map(|&i| &self.items[i].1).collect()
    }

    /// The selected row index within the filtered list.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selection.cursor()
    }

    /// The currently-selected value, if any row is visible.
    #[must_use]
    pub fn accepted(&self) -> Option<&T> {
        self.filtered
            .get(self.selected())
            .map(|&i| &self.items[i].1)
    }

    /// Move the selection up.
    pub fn select_up(&mut self) {
        self.selection.move_by(-1);
    }

    /// Move the selection down, clamped to the filtered list.
    pub fn select_down(&mut self) {
        self.selection.move_by(1);
    }

    /// Append a character to the query and refilter.
    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    /// Remove the last query character and refilter.
    pub fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    /// Append pasted text to the query and refilter.
    pub fn push_str(&mut self, text: &str) {
        self.query.push_str(text);
        self.refilter();
    }

    /// Re-rank the items for the current query and reset the selection to the
    /// best match.
    fn refilter(&mut self) {
        let labels: Vec<&str> = self.items.iter().map(|(label, _)| label.as_str()).collect();
        self.filtered = self.matcher.rank_indices(&self.query, &labels);
        self.selection.set_len(self.filtered.len());
        self.selection.move_to(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker() -> Picker<u32> {
        Picker::new(
            "Test",
            vec![
                ("app.rs".to_string(), 1),
                ("main.rs".to_string(), 2),
                ("Makefile".to_string(), 3),
            ],
        )
    }

    #[test]
    fn empty_query_shows_everything_in_input_order() {
        let p = picker();
        assert_eq!(p.rows(), vec!["app.rs", "main.rs", "Makefile"]);
        assert_eq!(p.accepted(), Some(&1));
    }

    #[test]
    fn typing_filters_fuzzily_and_resets_the_selection() {
        let mut p = picker();
        p.select_down();
        p.push_char('m');
        p.push_char('a');
        // Both "main.rs" and "Makefile" match "ma"; "app.rs" does not.
        assert!(p.rows().contains(&"main.rs"));
        assert!(p.rows().contains(&"Makefile"));
        assert!(!p.rows().contains(&"app.rs"));
        assert_eq!(p.selected(), 0, "a new query re-selects the best match");
    }

    #[test]
    fn fuzzy_subsequences_match_out_of_adjacency_but_not_out_of_order() {
        let mut p = picker();
        p.push_str("ars"); // a…r…s is a subsequence of app.rs
        assert!(p.rows().contains(&"app.rs"));
        let mut p = picker();
        p.push_str("sra"); // reversed: no match
        assert!(p.rows().is_empty());
        assert!(p.accepted().is_none());
    }

    #[test]
    fn backspace_restores_wider_matches() {
        let mut p = picker();
        p.push_str("main");
        assert_eq!(p.rows(), vec!["main.rs"]);
        p.pop_char();
        p.pop_char();
        p.pop_char();
        p.pop_char();
        assert_eq!(p.rows().len(), 3);
    }

    #[test]
    fn selection_clamps_to_the_filtered_list() {
        let mut p = picker();
        p.select_down();
        p.select_down();
        p.select_down();
        assert_eq!(p.selected(), 2, "clamped to the last row");
        p.select_up();
        assert_eq!(p.selected(), 1);
    }
}
