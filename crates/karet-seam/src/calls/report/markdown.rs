//! The report as one Markdown document.
//!
//! Ordered for a reader with limited attention: what the numbers mean, the summary, the
//! hotspots with their evidence, module coupling, how certain it all is — and only then
//! the full per-function table, as an appendix to cite from.
//!
//! Identities are written as code spans and escaped for where they land: a C++
//! `operator|` would otherwise split a table row, and a Kotlin name in backticks would
//! close its own code span.

use std::fmt::Write as _;

use super::CallReport;
use super::FunctionRow;
use super::Hotspot;

impl CallReport {
    /// The report as Markdown.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Call structure report\n");
        let _ = writeln!(out, "Configuration: {}\n", code(&self.configuration));
        self.how_to_read(&mut out);
        self.summary_section(&mut out);
        self.hotspot_section(&mut out);
        self.coupling_section(&mut out);
        self.ledger_section(&mut out);
        self.function_section(&mut out);
        out
    }

    fn how_to_read(&self, out: &mut String) {
        let t = &self.thresholds;
        let _ = writeln!(
            out,
            "## How to read this\n\n\
             Every function is named by its path, which survives edits that move it within its \
             file; `file:line` is where it starts today.\n\n\
             - **own** — the function's code lines: not blank, not comment-only, not inside a \
             nested named function. Closures count towards the function around them.\n\
             - **inclusive** — own lines of every *distinct* function it reaches, itself \
             included. Recursion is collapsed, so a cycle counts once.\n\
             - **lower / upper** — inclusive through established calls only, and through every \
             plausible call. Calls are resolved by name and scope, not by type; the gap is how \
             much of a number is guesswork.\n\
             - **tree** — own lines plus each callee's tree weight once per call site: how much \
             the function leans on shared code. Tree far above inclusive means heavy reuse.\n\
             - **in / out** — distinct established callers and callees. **ext** — distinct \
             callees outside the indexed code. **depth** — the longest established call chain \
             below it.\n\n\
             Test code is left out of every measure unless the configuration builds tests. \
             Calls through a function value, a pointer, reflection or a macro's arguments are \
             not seen, which is why \"unreachable\" leaves out anything a contract may dispatch \
             to.\n\n\
             Hotspots are ranked against the whole index, whatever this report lists. The \
             thresholds used: hubs have fan-in ≥ {}, heavy entry points reach ≥ {} lines at ≥ \
             {:.1}× their own size, large leaves have ≥ {} own lines (percentile {:.0}%, never \
             below those floors). Cycles, hubs, heavy entry points and large leaves show at \
             most {} each; the unreachable and test-only lists are complete.\n",
            t.hub_fan_in,
            t.heavy_inclusive,
            t.heavy_ratio,
            t.large_leaf_lines,
            t.percentile * 100.0,
            t.top_n
        );
    }

    fn summary_section(&self, out: &mut String) {
        let s = &self.summary;
        let _ = writeln!(
            out,
            "## Summary\n\n| | |\n|---|---|\n| functions | {} |\n| own code lines | {} |\n\
             | modules | {} |\n| packages | {} |\n| recursive functions | {} |\n| call cycles | {} |",
            s.functions, s.code_lines, s.modules, s.packages, s.recursive, s.cycles
        );
        if let Some((path, lines)) = &s.largest {
            let _ = writeln!(out, "| largest reach | {} — {lines} lines |", cell(path));
        }
        out.push('\n');
    }

    fn hotspot_section(&self, out: &mut String) {
        let h = &self.hotspots;
        let _ = writeln!(out, "## Hotspots\n");
        let _ = writeln!(out, "### Call cycles\n");
        if h.cycles.is_empty() {
            let _ = writeln!(out, "None.\n");
        }
        for cycle in &h.cycles {
            let path: Vec<String> = cycle.path.iter().map(|step| code(step)).collect();
            let _ = writeln!(
                out,
                "- {} functions, {} own lines: {}",
                cycle.members.len(),
                cycle.code_lines,
                path.join(" → ")
            );
        }
        if !h.cycles.is_empty() {
            out.push('\n');
        }
        for (title, spots) in [
            ("Hubs — the most callers", &h.hubs),
            (
                "Heavy entry points — reach far beyond their size",
                &h.heavy_entry_points,
            ),
            ("Large leaves — long, calling nothing here", &h.large_leaves),
            ("Unreachable — non-public, no caller at all", &h.unreachable),
            ("Called only from tests", &h.test_only),
        ] {
            let _ = writeln!(out, "### {title}\n");
            if spots.is_empty() {
                let _ = writeln!(out, "None.\n");
            }
            for spot in spots {
                write_spot(out, spot);
            }
        }
    }

    fn coupling_section(&self, out: &mut String) {
        let c = &self.coupling;
        let _ = writeln!(
            out,
            "## Module coupling\n\n\
             Established calls only, counted across the whole index. **Ca** — modules calling \
             in; **Ce** — modules called; **I** — Ce / (Ca + Ce), from 0 (depended on, stable) \
             to 1 (depends on others).\n\n\
             | module | functions | own lines | Ca | Ce | I |\n|---|---:|---:|---:|---:|---:|"
        );
        for m in &c.modules {
            let instability = m
                .instability
                .map_or_else(|| "—".to_owned(), |i| format!("{i:.2}"));
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {instability} |",
                cell(&m.path),
                m.functions,
                m.code_lines,
                m.afferent,
                m.efferent
            );
        }
        let _ = writeln!(
            out,
            "\n### Dependencies between modules\n\n| from | to | calls | callers |\n|---|---|---:|---:|"
        );
        for e in &c.edges {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                cell(&e.from),
                cell(&e.to),
                e.calls,
                e.callers
            );
        }
        let _ = writeln!(
            out,
            "\n### Calls into another module's non-public items\n\n\
             Legal where the language allows it — a Rust child module may call its parent's \
             private functions — but each is a dependency on something not offered as API.\n"
        );
        if c.internal_reach.is_empty() {
            let _ = writeln!(out, "None.");
        }
        for r in &c.internal_reach {
            let _ = writeln!(
                out,
                "- {} → {} ({})",
                code(&r.caller),
                code(&r.callee),
                r.visibility
            );
        }
        let _ = writeln!(out, "\n### Functions leaning on another module\n");
        if c.envious.is_empty() {
            let _ = writeln!(out, "None.");
        }
        for e in &c.envious {
            let _ = writeln!(
                out,
                "- {} calls {} {} times and its own module {} times",
                code(&e.function),
                code(&e.other),
                e.other_calls,
                e.own_calls
            );
        }
        out.push('\n');
    }

    fn ledger_section(&self, out: &mut String) {
        let l = &self.ledger;
        let _ = writeln!(
            out,
            "## Precision ledger\n\n\
             Call sites by how they resolved. Only exact and dispatch sites are in the lower \
             bound; ambiguous ones are in the upper; too-ambiguous (more than 16 candidates), \
             external and unresolved ones are in neither.\n\n\
             | language | functions | sites | exact | dispatch | ambiguous | too ambiguous | external | unresolved | tests told apart by |\n\
             |---|---:|---:|---:|---:|---:|---:|---:|---:|---|"
        );
        for g in &l.languages {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                escape_cell(&g.language),
                g.functions,
                g.sites,
                g.exact,
                g.dispatch,
                g.ambiguous,
                g.too_ambiguous,
                g.external,
                g.unresolved,
                g.test_detection
            );
        }
        let _ = writeln!(
            out,
            "\nAcross the whole index, {} calls sit outside any function and are in no \
             function's numbers.\n",
            l.loose_sites
        );
        if !l.least_certain.is_empty() {
            let _ = writeln!(
                out,
                "Functions whose numbers rest most on guesswork:\n\n| function | sites | ambiguous | unresolved |\n|---|---:|---:|---:|"
            );
            for u in &l.least_certain {
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {} |",
                    cell(&u.path),
                    u.sites,
                    u.ambiguous,
                    u.unresolved
                );
            }
            out.push('\n');
        }
    }

    fn function_section(&self, out: &mut String) {
        let _ = writeln!(
            out,
            "## Appendix: every function\n\nMost inclusive first.\n\n\
             | function | where | own | span | inclusive lower | upper | tree | in | out | ext | depth | cycle |\n\
             |---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"
        );
        for row in &self.functions {
            write_row(out, row);
        }
    }
}

fn write_spot(out: &mut String, spot: &Hotspot) {
    let _ = writeln!(
        out,
        "- **{}** ({}:{}) — {}\n\n  ```text\n{}  ```\n",
        code(&spot.path),
        spot.file,
        spot.line,
        spot.why,
        indent(&spot.excerpt)
    );
}

fn write_row(out: &mut String, row: &FunctionRow) {
    let m = &row.metrics;
    let tree = if m.saturated {
        format!("≥{}", m.tree_weighted)
    } else {
        m.tree_weighted.to_string()
    };
    let cycle = if m.scc_size > 1 {
        format!("{} ({})", m.scc, m.scc_size)
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "| {} | {}:{} | {} | {} | {} | {} | {tree} | {} | {} | {} | {} | {cycle} |",
        cell(&row.path),
        escape_cell(&row.file),
        row.line,
        m.code_lines,
        m.span_lines,
        m.inclusive_lower,
        m.inclusive_upper,
        m.fan_in,
        m.fan_out,
        m.external_apis,
        m.depth
    );
}

/// Text as an inline code span, fenced by more backticks than it contains in a row.
pub(crate) fn code(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    // A span starting or ending with a backtick needs a space to tell it from the fence.
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// A code span safe inside a table cell.
fn cell(text: &str) -> String {
    escape_cell(&code(text))
}

/// Text safe inside a table cell: a bare `|` would end the cell.
fn escape_cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// Indent every line of a block by two spaces, for a list item's fenced code.
fn indent(block: &str) -> String {
    block.lines().map(|line| format!("  {line}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_are_fenced_and_escaped_for_where_they_land() {
        assert_eq!(code("k::run"), "`k::run`");
        // A Kotlin name in backticks needs a longer fence, and padding at the edges.
        assert_eq!(code("pkg::`odd name`"), "`` pkg::`odd name` ``");
        assert_eq!(code("a``b"), "```a``b```");
        // A C++ `operator|` must not split a table row.
        assert_eq!(cell("ui::Flags::operator|"), "`ui::Flags::operator\\|`");
        assert_eq!(escape_cell("a|b"), "a\\|b");
    }
}
