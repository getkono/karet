//! GitHub autolinks in plain text, for the commit-message box.
//!
//! GitHub turns bare references in a commit message into links — `#12`, `GH-12`,
//! `owner/repo#12`, a commit hash, an `@mention`, a URL. This module finds those
//! references in a draft and says what each one points at, so the editor can
//! paint the same links the forge will.
//!
//! # Two deliberate deviations from GitHub
//!
//! **References are linked optimistically.** GitHub only linkifies a `#12` or a
//! hash that actually *resolves* in the repository. An editor scanning a draft
//! offline cannot know that, so a reference that matches the shape is linked; the
//! forge decides whether the target exists when the link is followed.
//!
//! **A bare hash must contain a digit.** `[0-9a-f]{7,40}` also matches ordinary
//! English written in hex letters — `deadbeef`, `facade`, `decade` — and linking
//! those would be worse than missing the occasional hash. Requiring at least one
//! digit costs a real 7-character prefix about 0.13% of the time and removes the
//! whole class of false positives.

use std::ops::Range;

use crate::remote::ForgeKind;
use crate::remote::Remote;

/// One reference found in a draft, and the URL it resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Autolink {
    /// The reference's byte range in the scanned text.
    pub(crate) range: Range<usize>,
    /// The URL the reference points at, already checked by
    /// [`links::safe_external`](crate::links::safe_external).
    pub(crate) url: String,
}

/// The shortest and longest hex prefix Git will accept as an object name.
const SHA_MIN: usize = 7;
const SHA_MAX: usize = 40;
/// The longest a GitHub login can be.
const LOGIN_MAX: usize = 39;
/// More digits than any forge issue number, and short enough that a long run of
/// digits in a diff excerpt is not mistaken for one.
const NUMBER_MAX: usize = 9;

/// Find every autolink in `text`.
///
/// URLs are linked whatever the remote is. The forge-specific forms — `#12`,
/// `GH-12`, `owner/repo#12`, a bare hash, an `@mention` — are linked only when
/// `remote` is a GitHub remote, since that is the only host whose reference
/// syntax this understands.
///
/// Results are ordered by position and never overlap: URLs are matched first and
/// any reference falling inside one is dropped, so a link to
/// `…/issues/12` is one link rather than a link with another link inside it.
pub(crate) fn scan(text: &str, remote: Option<&Remote>) -> Vec<Autolink> {
    let mut links = scan_urls(text);
    if let Some(remote) = remote.filter(|remote| remote.kind == ForgeKind::GitHub) {
        let covered: Vec<Range<usize>> = links.iter().map(|link| link.range.clone()).collect();
        links.extend(scan_references(text, remote, &covered));
        links.sort_by_key(|link| link.range.start);
    }
    links
}

/// Whether a reference may start at byte `at`: at the very start of the text, or
/// after a character that cannot be part of the token before it.
///
/// GitHub's own boundary, and the reason `x@example.com` is not a mention and
/// `v2#1` is not an issue reference. It is deliberately about *token* characters
/// rather than about whitespace: a commit message written in Japanese has no
/// spaces to lean on, and `修正#1` is a reference there just as `fix #1` is in
/// English.
fn opens_at(text: &str, at: usize) -> bool {
    let Some(before) = text[..at].chars().next_back() else {
        return true;
    };
    !before.is_ascii_alphanumeric() && !matches!(before, '_' | '-' | '/' | '@' | '#')
}

/// The byte offset just past the run of `predicate` characters starting at `from`.
fn run_end(text: &str, from: usize, predicate: impl Fn(char) -> bool) -> usize {
    text[from..]
        .char_indices()
        .find(|(_, character)| !predicate(*character))
        .map_or(text.len(), |(offset, _)| from + offset)
}

/// Match a positive issue number at `from`, returning its end offset.
///
/// A leading zero is refused: no forge numbers an issue `#007`, so a match there
/// is far more likely to be a version or an identifier.
fn number_end(text: &str, from: usize) -> Option<usize> {
    let end = run_end(text, from, |character| character.is_ascii_digit());
    let digits = text.get(from..end)?;
    let count = digits.len();
    (count > 0 && count <= NUMBER_MAX && !digits.starts_with('0')).then_some(end)
}

/// Find every bare `http`/`https` URL.
fn scan_urls(text: &str) -> Vec<Autolink> {
    let mut links = Vec::new();
    let mut at = 0usize;
    while at < text.len() {
        let Some(offset) = text[at..].find("http") else {
            break;
        };
        let start = at + offset;
        let rest = &text[start..];
        let scheme = if rest.starts_with("https://") {
            "https://"
        } else if rest.starts_with("http://") {
            "http://"
        } else {
            at = start + "http".len();
            continue;
        };
        if !opens_at(text, start) {
            at = start + scheme.len();
            continue;
        }
        let end = run_end(text, start, |character| {
            !character.is_whitespace() && !character.is_control()
        });
        let end = trim_url_end(&text[start..end]) + start;
        // A URL with nothing after its scheme is not a link, just the word.
        if end > start + scheme.len()
            && let Some(url) = crate::links::safe_external(&text[start..end])
        {
            links.push(Autolink {
                range: start..end,
                url: url.to_string(),
            });
        }
        at = end.max(start + scheme.len());
    }
    links
}

/// Trim the trailing characters a URL at the end of a sentence collects.
///
/// Sentence punctuation always goes; a closing bracket goes only when the URL
/// does not open it itself, so `…/Foo_(bar)` survives while `(see …/Foo)` does not.
fn trim_url_end(url: &str) -> usize {
    let mut end = url.len();
    loop {
        let trimmed = &url[..end];
        let Some(last) = trimmed.chars().next_back() else {
            return end;
        };
        let drop = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '<' | '>' => true,
            ')' => unbalanced(trimmed, '(', ')'),
            ']' => unbalanced(trimmed, '[', ']'),
            '}' => unbalanced(trimmed, '{', '}'),
            _ => false,
        };
        if !drop {
            return end;
        }
        end -= last.len_utf8();
    }
}

/// Whether `text` closes `close` more often than it opens `open`.
fn unbalanced(text: &str, open: char, close: char) -> bool {
    text.matches(close).count() > text.matches(open).count()
}

/// Find every GitHub-specific reference outside the ranges in `covered`.
fn scan_references(text: &str, remote: &Remote, covered: &[Range<usize>]) -> Vec<Autolink> {
    let mut links = Vec::new();
    let mut at = 0usize;
    while at < text.len() {
        if let Some(range) = covered.iter().find(|range| range.contains(&at)) {
            at = range.end;
            continue;
        }
        let Some(character) = text[at..].chars().next() else {
            break;
        };
        let matched = match character {
            '#' => issue_at(text, at, remote),
            '@' => mention_at(text, at, remote),
            _ if character.is_ascii_alphanumeric() => word_at(text, at, remote),
            _ => None,
        };
        match matched {
            Some(link) => {
                at = link.range.end;
                links.push(link);
            },
            None => at += character.len_utf8(),
        }
    }
    links
}

/// Match `#12`, or the `owner/repo#12` whose `#` sits at `at`.
fn issue_at(text: &str, at: usize, remote: &Remote) -> Option<Autolink> {
    let end = number_end(text, at + 1)?;
    let number = text.get(at + 1..end)?;
    // A cross-repository reference carries its own `owner/repo` before the `#`.
    if let Some((start, repo)) = repo_before(text, at) {
        return issue(start..end, &remote.host, repo, number);
    }
    if !opens_at(text, at) {
        return None;
    }
    issue(at..end, &remote.host, &remote.repo_path, number)
}

/// The `owner/repo` immediately before the `#` at `at`, when there is one.
fn repo_before(text: &str, at: usize) -> Option<(usize, &str)> {
    let name =
        |character: char| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.');
    let repo_start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, character)| name(*character))
        .last()
        .map(|(offset, _)| offset)?;
    let owner_end = repo_start.checked_sub(1)?;
    if text.get(owner_end..repo_start) != Some("/") {
        return None;
    }
    let owner_start = text[..owner_end]
        .char_indices()
        .rev()
        .take_while(|(_, character)| name(*character))
        .last()
        .map(|(offset, _)| offset)?;
    if !opens_at(text, owner_start) {
        return None;
    }
    Some((owner_start, text.get(owner_start..at)?))
}

/// Match an `@mention` starting at `at`.
fn mention_at(text: &str, at: usize, remote: &Remote) -> Option<Autolink> {
    if !opens_at(text, at) {
        return None;
    }
    let end = run_end(text, at + 1, |character| {
        character.is_ascii_alphanumeric() || character == '-'
    });
    let login = text.get(at + 1..end)?;
    // GitHub logins are alphanumeric with single interior hyphens.
    if login.is_empty()
        || login.len() > LOGIN_MAX
        || login.starts_with('-')
        || login.ends_with('-')
        || login.contains("--")
    {
        return None;
    }
    link(at..end, format!("https://{}/{login}", remote.host))
}

/// Match a bare word at `at`: `GH-12`, or a commit hash.
fn word_at(text: &str, at: usize, remote: &Remote) -> Option<Autolink> {
    if !opens_at(text, at) {
        return None;
    }
    let end = run_end(text, at, |character| {
        character.is_ascii_alphanumeric() || character == '-'
    });
    // A word followed by `#` is the `owner/repo` half of a cross-repository
    // reference (or the noise before one); the `#` branch owns it.
    if text[end..].starts_with('#') || text[end..].starts_with('/') {
        return None;
    }
    let word = text.get(at..end)?;
    if let Some(digits) = word
        .strip_prefix("GH-")
        .or_else(|| word.strip_prefix("gh-"))
        .or_else(|| word.strip_prefix("Gh-"))
        && number_end(word, word.len() - digits.len()) == Some(word.len())
    {
        return issue(at..end, &remote.host, &remote.repo_path, digits);
    }
    is_hash(word)
        .then(|| {
            link(
                at..end,
                format!("https://{}/{}/commit/{word}", remote.host, remote.repo_path),
            )
        })
        .flatten()
}

/// Whether `word` looks like a commit hash — see the module docs on the
/// digit requirement.
fn is_hash(word: &str) -> bool {
    (SHA_MIN..=SHA_MAX).contains(&word.len())
        && word.bytes().all(|byte| byte.is_ascii_hexdigit())
        && word.bytes().all(|byte| !byte.is_ascii_uppercase())
        && word.bytes().any(|byte| byte.is_ascii_digit())
}

/// Build an issue link. GitHub redirects `/issues/n` to a pull request of the
/// same number, so one route serves both.
fn issue(range: Range<usize>, host: &str, repo_path: &str, number: &str) -> Option<Autolink> {
    link(range, format!("https://{host}/{repo_path}/issues/{number}"))
}

/// Build an autolink, dropping any target the OSC 8 guard refuses.
fn link(range: Range<usize>, url: String) -> Option<Autolink> {
    crate::links::safe_external(&url)?;
    Some(Autolink { range, url })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::parse_remote;

    fn github() -> Remote {
        parse_remote("git@github.com:getkono/karet.git").unwrap_or_else(|| unreachable!())
    }

    fn gitlab() -> Remote {
        parse_remote("https://gitlab.com/g/r.git").unwrap_or_else(|| unreachable!())
    }

    /// The linked substrings and their targets.
    fn found(text: &str, remote: Option<&Remote>) -> Vec<(String, String)> {
        scan(text, remote)
            .into_iter()
            .map(|link| (text[link.range].to_string(), link.url))
            .collect()
    }

    /// Just the linked substrings.
    fn hits(text: &str, remote: Option<&Remote>) -> Vec<String> {
        found(text, remote)
            .into_iter()
            .map(|(text, _)| text)
            .collect()
    }

    #[test]
    fn a_closing_keyword_is_irrelevant_the_reference_is_what_links() {
        let repo = github();
        assert_eq!(
            found("Closes #1", Some(&repo)),
            [(
                "#1".to_string(),
                "https://github.com/getkono/karet/issues/1".to_string()
            )]
        );
        // GitHub links the bare reference wherever it appears, so "Ref #1",
        // "see #1" and a naked "#1" all behave the same.
        assert_eq!(hits("Ref #1", Some(&repo)), ["#1"]);
        assert_eq!(hits("#1", Some(&repo)), ["#1"]);
        assert_eq!(hits("(#1)", Some(&repo)), ["#1"]);
    }

    #[test]
    fn the_other_reference_forms_resolve_to_their_own_repositories() {
        let repo = github();
        assert_eq!(
            found("GH-12", Some(&repo)).first().map(|hit| hit.1.clone()),
            Some("https://github.com/getkono/karet/issues/12".to_string())
        );
        assert_eq!(hits("gh-12", Some(&repo)), ["gh-12"], "case-insensitive");
        assert_eq!(
            found("fixes owner/repo#3", Some(&repo)),
            [(
                "owner/repo#3".to_string(),
                "https://github.com/owner/repo/issues/3".to_string()
            )]
        );
        assert_eq!(
            found("thanks @octo-cat", Some(&repo)),
            [(
                "@octo-cat".to_string(),
                "https://github.com/octo-cat".to_string()
            )]
        );
        assert_eq!(
            found("see 0deadbe", Some(&repo)),
            [(
                "0deadbe".to_string(),
                "https://github.com/getkono/karet/commit/0deadbe".to_string()
            )]
        );
    }

    #[test]
    fn a_reference_must_stand_on_its_own() {
        let repo = github();
        // Not preceded by a word character: an e-mail address is not a mention,
        // and a fragment is not an issue.
        assert!(hits("Co-authored-by: A <a@b.com>", Some(&repo)).is_empty());
        assert!(hits("x#1", Some(&repo)).is_empty());
        // A number is required, and it may not be padded.
        assert!(hits("# 1", Some(&repo)).is_empty());
        assert!(hits("#abc", Some(&repo)).is_empty());
        assert!(hits("#0abc", Some(&repo)).is_empty());
        assert!(hits("#01", Some(&repo)).is_empty());
        assert!(hits("@", Some(&repo)).is_empty());
        assert!(hits("@-bad", Some(&repo)).is_empty());
    }

    #[test]
    fn a_hash_must_look_like_a_hash_and_not_like_a_word() {
        let repo = github();
        // The digit requirement: hex letters alone spell too many real words.
        assert!(hits("deadbeef", Some(&repo)).is_empty());
        assert!(hits("facade", Some(&repo)).is_empty());
        assert!(hits("decade", Some(&repo)).is_empty());
        // Length bounds, and no capitals (git prints hashes lowercase).
        assert!(hits("0deadb", Some(&repo)).is_empty(), "six is too short");
        assert_eq!(hits("0deadbe", Some(&repo)), ["0deadbe"]);
        let forty = format!("0{}", "a".repeat(39));
        assert_eq!(hits(&forty, Some(&repo)), *std::slice::from_ref(&forty));
        assert!(hits(&format!("{forty}a"), Some(&repo)).is_empty(), "41");
        assert!(hits("0DEADBE", Some(&repo)).is_empty());
    }

    #[test]
    fn urls_are_linked_whatever_the_remote_is_and_trim_their_sentence() {
        assert_eq!(
            hits("see https://example.com/a", None),
            ["https://example.com/a"]
        );
        assert_eq!(
            hits("see https://example.com/a.", None),
            ["https://example.com/a"]
        );
        assert_eq!(
            hits("(see http://example.com/a)", None),
            ["http://example.com/a"]
        );
        // A URL that opens its own parenthesis keeps it.
        assert_eq!(
            hits("https://e.com/Foo_(bar)", None),
            ["https://e.com/Foo_(bar)"]
        );
        assert!(hits("https://", None).is_empty(), "a scheme is not a link");
        assert!(hits("nothttps://e.com/a", None).is_empty(), "mid-word");
    }

    #[test]
    fn a_reference_inside_a_url_is_part_of_the_url_not_a_second_link() {
        let repo = github();
        assert_eq!(
            hits("https://github.com/o/r/issues/12#note", Some(&repo)),
            ["https://github.com/o/r/issues/12#note"]
        );
        assert_eq!(
            hits("https://github.com/o/r/commit/0deadbeef", Some(&repo)),
            ["https://github.com/o/r/commit/0deadbeef"]
        );
    }

    #[test]
    fn only_a_github_remote_gets_the_reference_forms() {
        let other = gitlab();
        let text = "Closes #1 thanks @you 0deadbe https://example.com/a";
        assert_eq!(hits(text, Some(&other)), ["https://example.com/a"]);
        assert_eq!(hits(text, None), ["https://example.com/a"]);
        assert_eq!(hits(text, Some(&github())).len(), 4);
    }

    #[test]
    fn results_are_ordered_and_never_overlap() {
        let repo = github();
        let links = scan("#1 and @you and https://e.com/x and 0deadbe", Some(&repo));
        assert_eq!(links.len(), 4);
        for (previous, link) in links.iter().zip(links.iter().skip(1)) {
            assert!(
                previous.range.end <= link.range.start,
                "{previous:?} overlaps {link:?}"
            );
        }
    }

    #[test]
    fn a_multibyte_draft_never_splits_a_character() {
        let repo = github();
        // Byte offsets must land on character boundaries whatever precedes them.
        let text = "修正 #1 界@you 界https://e.com/x";
        for link in scan(text, Some(&repo)) {
            assert!(text.is_char_boundary(link.range.start), "{link:?}");
            assert!(text.is_char_boundary(link.range.end), "{link:?}");
        }
        assert_eq!(hits(text, Some(&repo)).len(), 3);
    }
}
