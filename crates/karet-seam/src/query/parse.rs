//! Turning query text into terms, with positioned failures.

use std::ops::Range;

use super::Query;
use super::QueryError;
use super::Term;
use super::TermKind;
use crate::edge::EdgeKind;
use crate::id::SeamPath;
use crate::lang::SeamLanguage;
use crate::model::LENSES;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;

/// Split query text into `(span, text)` tokens, honouring quoted phrases.
fn tokenize(text: &str) -> Vec<(Range<usize>, &str)> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
            continue;
        }
        let start = index;
        let mut in_quotes = false;
        while index < bytes.len() {
            match bytes[index] {
                b'"' => in_quotes = !in_quotes,
                b if b.is_ascii_whitespace() && !in_quotes => break,
                _ => {},
            }
            index += 1;
        }
        tokens.push((start..index, &text[start..index]));
    }
    tokens
}

/// Parse query text into a [`Query`].
///
/// # Errors
/// [`QueryError`] for an unrecognized term, an unparseable value, or a misused
/// `config:` directive — each carrying the byte range that produced it.
pub fn parse(text: &str) -> Result<Query, QueryError> {
    let mut query = Query::default();
    let mut config_span: Option<Range<usize>> = None;

    for (span, token) in tokenize(text) {
        let (negated, body_offset) = match token.strip_prefix('!') {
            Some(_) => (true, 1usize),
            None => (false, 0usize),
        };
        let body = &token[body_offset..];
        let body_span = span.start + body_offset..span.end;
        if body.is_empty() {
            return Err(QueryError::new("`!` needs a term to negate", span));
        }

        // `config:` sets the evaluation context rather than filtering, so negating it or
        // giving it twice is a contradiction rather than a narrowing.
        if let Some(name) = body.strip_prefix("config:") {
            if negated {
                return Err(QueryError::new(
                    "`config:` selects which configuration to evaluate under, so it cannot be negated",
                    span,
                ));
            }
            if let Some(previous) = &config_span {
                return Err(QueryError::new(
                    format!(
                        "only one `config:` may be given; the first is at byte {}",
                        previous.start
                    ),
                    body_span,
                ));
            }
            if name.is_empty() {
                return Err(QueryError::new(
                    "`config:` needs a configuration name",
                    body_span,
                ));
            }
            config_span = Some(body_span);
            query.configuration = Some(name.to_owned());
            continue;
        }

        query.terms.push(Term {
            negated,
            kind: parse_term(body, &body_span)?,
            span,
        });
    }
    Ok(query)
}

/// Parse one non-`config:` term.
fn parse_term(body: &str, span: &Range<usize>) -> Result<TermKind, QueryError> {
    if let Some(phrase) = body.strip_prefix('"') {
        return Ok(TermKind::Phrase(
            phrase.strip_suffix('"').unwrap_or(phrase).to_owned(),
        ));
    }

    let Some((prefix, value)) = body.split_once(':') else {
        // A bare word is a fuzzy name match — the common case, and never an error.
        return Ok(TermKind::Name(body.to_owned()));
    };

    if value.is_empty() {
        return Err(QueryError::new(
            format!("`{prefix}:` needs a value"),
            span.clone(),
        ));
    }

    match prefix {
        "lens" => Lens::from_name(value).map(TermKind::Lens).ok_or_else(|| {
            unknown(
                "lens",
                value,
                span,
                LENSES.iter().map(|l| l.name().to_owned()),
            )
        }),
        "vis" => Visibility::from_name(value)
            .map(TermKind::Visibility)
            .ok_or_else(|| {
                unknown(
                    "visibility level",
                    value,
                    span,
                    Visibility::all().iter().map(|v| v.name().to_owned()),
                )
            }),
        "kind" => NodeKind::from_name(value)
            .map(TermKind::Kind)
            .ok_or_else(|| {
                unknown(
                    "node kind",
                    value,
                    span,
                    NodeKind::all().iter().map(|k| k.name().to_owned()),
                )
            }),
        "in" => value
            .parse::<SeamPath>()
            .map(TermKind::In)
            .map_err(|error| {
                QueryError::new(format!("`in:` needs a node path ({error})"), span.clone())
            }),
        "cfg" => Ok(TermKind::Cfg(value.to_owned())),
        "pivot" => parse_pivot(value, span),
        // `<lens>:<subtype>` — the lens name doubles as a prefix.
        other => match Lens::from_name(other) {
            Some(lens) => {
                let known = known_subtypes(lens);
                // A subtype no registered language can emit would match nothing while
                // looking like it worked, so it is an error like any other unknown term.
                // With no language compiled in there is nothing to check against, and
                // guessing would be worse than accepting.
                if !known.is_empty() && !known.iter().any(|candidate| candidate == value) {
                    return Err(unknown(
                        &format!("{} facet", lens.name()),
                        value,
                        span,
                        known,
                    ));
                }
                Ok(TermKind::Facet {
                    lens,
                    subtype: value.to_owned(),
                })
            },
            None => Err(unknown("term", other, span, known_prefixes())),
        },
    }
}

/// Parse `pivot:<edge>:<node path>`.
fn parse_pivot(value: &str, span: &Range<usize>) -> Result<TermKind, QueryError> {
    let Some((edge_name, target)) = value.split_once(':') else {
        return Err(QueryError::new(
            "`pivot:` needs an edge kind and a node path, as `pivot:<edge>:<path>`",
            span.clone(),
        ));
    };
    let edge = EdgeKind::from_name(edge_name).ok_or_else(|| {
        unknown(
            "edge kind",
            edge_name,
            span,
            EdgeKind::all().iter().map(|k| k.name().to_owned()),
        )
    })?;
    let target = target.parse::<SeamPath>().map_err(|error| {
        QueryError::new(
            format!("`pivot:` needs a node path ({error})"),
            span.clone(),
        )
    })?;
    Ok(TermKind::Pivot { edge, target })
}

/// Every prefix a term may start with, for suggestions.
fn known_prefixes() -> impl Iterator<Item = String> {
    ["lens", "vis", "kind", "in", "cfg", "config", "pivot"]
        .into_iter()
        .map(str::to_owned)
        .chain(LENSES.iter().map(|lens| lens.name().to_owned()))
}

/// An "unrecognized X" failure carrying the nearest valid names.
fn unknown(
    what: &str,
    value: &str,
    span: &Range<usize>,
    candidates: impl IntoIterator<Item = String>,
) -> QueryError {
    let candidates: Vec<String> = candidates.into_iter().collect();
    QueryError::new(format!("unknown {what} `{value}`"), span.clone())
        .with_suggestions(closest(value, &candidates))
}

/// The candidates nearest to `value`, best first, capped at three.
///
/// Prefix and substring hits come first because they are what a typo usually produces;
/// otherwise fall back to edit distance.
fn closest(value: &str, candidates: &[String]) -> Vec<String> {
    let lowered = value.to_lowercase();
    let mut scored: Vec<(usize, &String)> = candidates
        .iter()
        .map(|candidate| {
            let lower = candidate.to_lowercase();
            let score = if lower == lowered {
                0
            } else if lower.starts_with(&lowered) || lowered.starts_with(&lower) {
                1
            } else if lower.contains(&lowered) || lowered.contains(&lower) {
                2
            } else {
                3 + edit_distance(&lowered, &lower)
            };
            (score, candidate)
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    scored
        .into_iter()
        // Beyond this the "suggestion" is noise rather than help.
        .filter(|(score, _)| *score <= 6)
        .take(3)
        .map(|(_, candidate)| candidate.clone())
        .collect()
}

/// Levenshtein distance, for ranking near-miss names.
fn edit_distance(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b_chars.len()).collect();
    let mut current = vec![0usize; b_chars.len() + 1];
    for (i, a_char) in a.chars().enumerate() {
        current[0] = i + 1;
        for (j, b_char) in b_chars.iter().enumerate() {
            let cost = usize::from(a_char != *b_char);
            current[j + 1] = (previous[j] + cost)
                .min(previous[j + 1] + 1)
                .min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous.last().copied().unwrap_or(0)
}

/// One language's subtypes for a lens.
#[allow(dead_code, reason = "unused when no language feature is enabled")]
fn subtypes_of(language: &dyn SeamLanguage, lens: Lens) -> Vec<String> {
    language
        .subtypes()
        .iter()
        .filter(|(l, _)| *l == lens)
        .map(|(_, subtype)| subtype.name().to_owned())
        .collect()
}

/// Every facet subtype *any* registered language can emit, for `<lens>:<subtype>`
/// suggestions and validation.
///
/// Unioned across languages, not taken from one: a Python `substitution:protocol` is as
/// valid a term as a Rust `substitution:dyn`, and validating against Rust alone would
/// reject the other language's own vocabulary.
#[must_use]
pub fn known_subtypes(lens: Lens) -> Vec<String> {
    let mut out = Vec::new();
    #[cfg(feature = "lang-rust")]
    out.extend(subtypes_of(&crate::lang::rust::Rust, lens));
    #[cfg(feature = "lang-python")]
    out.extend(subtypes_of(&crate::lang::python::Python, lens));
    #[cfg(feature = "lang-javascript")]
    out.extend(subtypes_of(&crate::lang::typescript::TypeScript, lens));
    #[cfg(feature = "lang-swift")]
    out.extend(subtypes_of(&crate::lang::swift::Swift, lens));
    let _ = lens;
    out.sort();
    out.dedup();
    out
}
