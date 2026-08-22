//! Pretty TypeScript errors: re-render tsserver's dense one-line messages as
//! markdown, in the spirit of the pretty-ts-errors extension.
//!
//! The whole transformation is text-to-text and pure. The core move is lifting
//! the types tsserver quotes with `'…'` out of the prose: simple names become
//! inline code, structural types become fenced ```type blocks, indented by a
//! small pretty-printer. tsserver truncates long types with `...`, leaving
//! unbalanced braces the fence would render broken — a repair pass appends the
//! missing closers first. Everything else (severity, source, the `ts(2322)`
//! code) is composed by the surfaces that already label raw diagnostics.

use karet_core::Diagnostic;
use karet_core::DiagnosticFormatter;
use karet_core::FormattedDiagnostic;

/// The built-in [`DiagnosticFormatter`] for tsserver-family diagnostics.
pub struct TsErrorFormatter;

/// The `Diagnostic::source` values the TypeScript formatter claims.
const TS_SOURCES: &[&str] = &["typescript", "ts", "tsserver", "deno-ts"];

impl DiagnosticFormatter for TsErrorFormatter {
    fn format(&self, diagnostic: &Diagnostic) -> Option<FormattedDiagnostic> {
        let source = diagnostic.source.as_deref()?;
        TS_SOURCES.contains(&source).then(|| FormattedDiagnostic {
            markdown: prettify(&diagnostic.message),
        })
    }
}

/// Format `diagnostic` through the built-in formatter registry (currently the
/// TypeScript formatter alone; others slot in beside it).
#[must_use]
pub fn format_diagnostic(diagnostic: &Diagnostic) -> Option<FormattedDiagnostic> {
    const FORMATTERS: &[&(dyn DiagnosticFormatter + Sync)] = &[&TsErrorFormatter];
    FORMATTERS
        .iter()
        .find_map(|formatter| formatter.format(diagnostic))
}

/// A quoted segment is rendered as a fenced block (rather than inline code)
/// beyond this length, even when it has no structure to pretty-print.
const FENCE_LENGTH: usize = 50;

/// Re-render one raw tsserver message as markdown.
fn prettify(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    // tsserver nests elaborations ("Types of property 'x' are incompatible.")
    // with two-space indentation; each clause becomes its own paragraph.
    for (i, line) in message.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if i > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&prettify_line(line));
    }
    out
}

/// Lift the `'…'`-quoted segments of one clause into code markup.
fn prettify_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('\'') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('\'') else {
            break;
        };
        out.push_str(&rest[..open]);
        push_code(&mut out, &after[..close]);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The longest run of backticks anywhere in `text`.
///
/// A type can carry backticks of its own — a string-literal type spelling a
/// fence (`"```"`), or a template-literal type — and CommonMark ends a span at
/// the first delimiter run of matching length. Both markups below pick a run
/// one longer than anything inside, which is exactly how nested fences work.
fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// Append `quoted` as inline code or a fenced, pretty-printed block.
fn push_code(out: &mut String, quoted: &str) {
    let structural = quoted.contains('{') || quoted.contains("=>");
    if !structural && quoted.len() <= FENCE_LENGTH {
        // A backtick inside the type needs a longer delimiter, and a space so
        // a leading or trailing backtick is not eaten by the delimiter run.
        let ticks = "`".repeat(longest_backtick_run(quoted) + 1);
        out.push_str(&ticks);
        if quoted.starts_with('`') || quoted.ends_with('`') {
            out.push(' ');
            out.push_str(quoted);
            out.push(' ');
        } else {
            out.push_str(quoted);
        }
        out.push_str(&ticks);
        return;
    }
    let body = pretty_type(&repair_balance(quoted));
    let fence = "`".repeat(longest_backtick_run(&body).max(2) + 1);
    out.push_str("\n\n");
    out.push_str(&fence);
    out.push_str("typescript\n");
    out.push_str(&body);
    out.push('\n');
    out.push_str(&fence);
    out.push_str("\n\n");
}

/// Append the closers of any `{`/`(`/`[` left open — tsserver truncates long
/// types with `...`, and an unbalanced fence renders broken. Surplus closers
/// are left alone: removal could only guess which one was wrong.
fn repair_balance(type_text: &str) -> String {
    let mut stack = Vec::new();
    for ch in type_text.chars() {
        match ch {
            '{' => stack.push('}'),
            '(' => stack.push(')'),
            '[' => stack.push(']'),
            '}' | ')' | ']' if stack.last() == Some(&ch) => {
                stack.pop();
            },
            _ => {},
        }
    }
    let mut repaired = type_text.to_owned();
    while let Some(closer) = stack.pop() {
        repaired.push(closer);
    }
    repaired
}

/// Indent-based type pretty-printer: object-literal braces open a nested
/// block, and `;`-separated members inside them go one per line. Parenthesized
/// signatures stay inline — breaking parameter lists helps nobody.
fn pretty_type(type_text: &str) -> String {
    if !type_text.contains('{') {
        return type_text.to_owned();
    }
    let mut out = String::with_capacity(type_text.len() * 2);
    let mut depth = 0usize;
    let mut parens = 0usize;
    let mut chars = type_text.chars().peekable();
    let newline = |out: &mut String, depth: usize| {
        while out.ends_with(' ') {
            out.pop();
        }
        out.push('\n');
        out.push_str(&"  ".repeat(depth));
    };
    while let Some(ch) = chars.next() {
        match ch {
            '{' if parens == 0 => {
                out.push('{');
                depth += 1;
                newline(&mut out, depth);
                // Swallow the space tsserver prints after `{`.
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
            },
            '}' if parens == 0 => {
                depth = depth.saturating_sub(1);
                newline(&mut out, depth);
                out.push('}');
            },
            ';' if depth > 0 && parens == 0 => {
                out.push(';');
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                // A member separator, unless the `}` right behind it closes
                // the block (the printer's own newline handles that).
                if chars.peek() != Some(&'}') {
                    newline(&mut out, depth);
                }
            },
            '(' => {
                parens += 1;
                out.push('(');
            },
            ')' => {
                parens = parens.saturating_sub(1);
                out.push(')');
            },
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use karet_core::Range;
    use karet_core::Severity;

    use super::*;

    fn ts_diag(message: &str) -> Diagnostic {
        Diagnostic {
            range: Range::default(),
            severity: Severity::Error,
            message: message.to_owned(),
            source: Some("typescript".to_owned()),
            code: Some("2322".to_owned()),
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    #[test]
    fn other_sources_pass_through_unformatted() {
        let mut diagnostic = ts_diag("Type 'string' is wrong.");
        diagnostic.source = Some("rustc".to_owned());
        assert_eq!(format_diagnostic(&diagnostic), None);
        diagnostic.source = None;
        assert_eq!(format_diagnostic(&diagnostic), None);
    }

    #[test]
    fn simple_quoted_names_become_inline_code() {
        let got = prettify("Type 'string' is not assignable to type 'number'.");
        assert_eq!(got, "Type `string` is not assignable to type `number`.");
    }

    #[test]
    fn structural_types_become_fenced_blocks() {
        let got =
            prettify("Type '{ name: string; age: number; }' is not assignable to type 'Person'.");
        assert!(
            got.contains("```typescript\n{\n  name: string;\n  age: number;\n}\n```"),
            "{got}"
        );
        assert!(got.contains("`Person`"));
    }

    #[test]
    fn nested_elaborations_become_paragraphs() {
        let raw = "Type 'A' is not assignable to type 'B'.\n  Types of property 'age' are incompatible.\n    Type 'string' is not assignable to type 'number'.";
        let got = prettify(raw);
        assert_eq!(got.matches("\n\n").count(), 2, "{got}");
        assert!(got.contains("Types of property `age` are incompatible."));
    }

    #[test]
    fn function_signatures_fence_without_breaking_parameters() {
        let got = prettify(
            "Argument of type '(x: number) => string' is not assignable to parameter of type '(x: string) => string'.",
        );
        assert!(
            got.contains("```typescript\n(x: number) => string\n```"),
            "{got}"
        );
    }

    #[test]
    fn truncated_types_are_repaired_before_fencing() {
        let got = prettify("Type '{ a: { b: { c: string; ...' is missing properties.");
        let fence = got.split("```").nth(1).unwrap_or_default();
        let opens = fence.matches('{').count();
        let closes = fence.matches('}').count();
        assert_eq!(opens, closes, "{got}");
    }

    #[test]
    fn a_type_containing_backticks_still_produces_a_closable_fence() {
        // `type Config = { fence: "```" }` is ordinary TypeScript, and CommonMark
        // ends a fence at the first run of matching length — so the delimiter has
        // to be longer than anything inside, or the popup renders broken.
        let got = prettify(
            r#"Type 'string' is not assignable to type '{ fence: "```"; lang: string; }'."#,
        );
        assert!(got.contains("````typescript"), "{got}");
        assert!(
            fences_balanced(&got),
            "the fence must close after the type:\n{got}"
        );

        // Deeper runs push the delimiter further out.
        let deep = prettify(r#"Type 'Y' is not assignable to type '{ f: "`````"; }'."#);
        assert!(deep.contains("``````typescript"), "{deep}");
        assert!(fences_balanced(&deep), "{deep}");
    }

    #[test]
    fn a_backticked_name_is_wrapped_without_swallowing_its_ticks() {
        // A template-literal type is short and unstructured, so it takes the
        // inline-code path; a leading or trailing backtick needs padding.
        let got = prettify("Type 'A' is not assignable to type '`x`'.");
        assert!(got.contains("`` `x` ``"), "{got}");
    }

    /// Walk `md` the way CommonMark does: a fence opens on a run of three or
    /// more backticks and closes only on a run at least as long.
    fn fences_balanced(md: &str) -> bool {
        let mut open: Option<usize> = None;
        for line in md.lines() {
            let trimmed = line.trim_start();
            let run = trimmed.chars().take_while(|&c| c == '`').count();
            if run < 3 {
                continue;
            }
            match open {
                None => open = Some(run),
                Some(n) if run >= n && trimmed[run..].trim().is_empty() => open = None,
                Some(_) => {},
            }
        }
        open.is_none()
    }

    #[test]
    fn repair_is_order_aware_and_ignores_surplus_closers() {
        assert_eq!(repair_balance("{ ( ["), "{ ( [])}");
        assert_eq!(repair_balance("a } b"), "a } b");
        // A mismatched closer is surplus under strict nesting: both opens
        // still get their closers appended.
        assert_eq!(repair_balance("([)"), "([)])");
        assert_eq!(repair_balance("balanced()"), "balanced()");
    }

    #[test]
    fn unterminated_quote_leaves_the_tail_verbatim() {
        let got = prettify("Cannot find name 'foo. Did you mean bar?");
        assert_eq!(got, "Cannot find name 'foo. Did you mean bar?");
    }

    #[test]
    fn deep_nesting_indents_per_level() {
        let got = prettify("Type '{ a: { b: string; }; c: number; }' is bad.");
        assert!(
            got.contains("{\n  a: {\n    b: string;\n  };\n  c: number;\n}"),
            "{got}"
        );
    }

    #[test]
    fn long_flat_types_fence_without_pretty_printing() {
        let long = "\"a\" | \"b\" | \"c\" | \"d\" | \"e\" | \"f\" | \"g\" | \"h\" | \"i\"";
        let got = prettify(&format!("Type 'x' is not assignable to type '{long}'."));
        assert!(
            got.contains(&format!("```typescript\n{long}\n```")),
            "{got}"
        );
    }

    #[test]
    fn formatter_registry_serves_typescript_sources() {
        let formatted = format_diagnostic(&ts_diag(
            "Property 'x' does not exist on type 'Y'. Did you mean 'z'?",
        ));
        let markdown = formatted.map(|f| f.markdown).unwrap_or_default();
        assert_eq!(
            markdown,
            "Property `x` does not exist on type `Y`. Did you mean `z`?"
        );
    }
}
