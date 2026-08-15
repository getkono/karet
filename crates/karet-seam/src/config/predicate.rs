//! Parsing and evaluating `cfg` predicates, with a third answer.
//!
//! A predicate is not simply true or false here. `cfg(target_os = "redox")` evaluated on
//! a Linux host is false; `cfg(some_key_this_index_knows_nothing_about)` is neither — and
//! collapsing the second into "false" would quietly delete code from the view while
//! claiming completeness. So evaluation is three-valued, and the unknown answer
//! propagates through `all`/`any`/`not` by Kleene logic rather than being guessed away.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// Errors parsing a `cfg` predicate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CfgError {
    /// The predicate text was empty.
    #[error("empty cfg predicate")]
    Empty,
    /// A `(` was opened and never closed.
    #[error("unclosed `(` in cfg predicate")]
    Unclosed,
    /// Text remained after a complete predicate.
    #[error("unexpected trailing text `{0}` in cfg predicate")]
    Trailing(String),
    /// A malformed term.
    #[error("malformed cfg term at `{0}`")]
    Malformed(String),
}

/// A parsed `cfg` predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfgPredicate {
    /// `all(…)` — every operand must hold.
    All(Vec<CfgPredicate>),
    /// `any(…)` — at least one operand must hold.
    Any(Vec<CfgPredicate>),
    /// `not(…)` — the operand must not hold.
    Not(Box<CfgPredicate>),
    /// A bare flag such as `unix`, `test`, or `doc`.
    Flag(String),
    /// A `key = "value"` pair such as `feature = "view"`.
    KeyValue {
        /// The configuration key.
        key: String,
        /// The value, with quotes stripped.
        value: String,
    },
}

/// A three-valued truth, because "I do not know" is a distinct and useful answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Truth {
    /// The predicate holds.
    True,
    /// The predicate does not hold.
    False,
    /// The predicate could not be decided from what this index knows.
    Unknown,
}

impl Truth {
    /// Kleene negation: unknown stays unknown.
    ///
    /// Named `negate` rather than `not` so it is never mistaken for `std::ops::Not`,
    /// whose two-valued contract this deliberately does not satisfy.
    #[must_use]
    pub fn negate(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }

    /// Kleene conjunction over a sequence.
    ///
    /// One false operand settles it, even if others are unknown — a gate that cannot open
    /// is closed regardless of what else is uncertain.
    #[must_use]
    pub fn all(values: impl IntoIterator<Item = Self>) -> Self {
        let mut result = Self::True;
        for value in values {
            match value {
                Self::False => return Self::False,
                Self::Unknown => result = Self::Unknown,
                Self::True => {},
            }
        }
        result
    }

    /// Kleene disjunction over a sequence.
    ///
    /// One true operand settles it, mirroring [`all`](Self::all).
    #[must_use]
    pub fn any(values: impl IntoIterator<Item = Self>) -> Self {
        let mut result = Self::False;
        for value in values {
            match value {
                Self::True => return Self::True,
                Self::Unknown => result = Self::Unknown,
                Self::False => {},
            }
        }
        result
    }
}

/// What is known about the compilation environment.
///
/// Anything absent from all three collections evaluates to [`Truth::Unknown`] rather than
/// false, which is what keeps an unrecognized `cfg` key from silently hiding code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CfgEnv {
    /// Enabled Cargo features.
    pub features: BTreeSet<String>,
    /// Bare flags that hold, such as `unix` or `test`.
    pub flags: BTreeSet<String>,
    /// Keys whose value is known, such as `target_os`.
    pub key_values: BTreeMap<String, String>,
    /// Keys that are known to be *fully enumerated*, so an unlisted value is genuinely
    /// false rather than unknown.
    ///
    /// `feature` is always one of these: the manifest lists every feature a package has,
    /// so a `feature = "absent"` gate is decidably off, not indeterminate.
    pub closed_keys: BTreeSet<String>,
}

impl CfgEnv {
    /// An environment that knows nothing, so every predicate is unknown.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable a set of Cargo features, and mark `feature` as fully enumerated.
    #[must_use]
    pub fn with_features(mut self, features: impl IntoIterator<Item = String>) -> Self {
        self.features = features.into_iter().collect();
        self.closed_keys.insert("feature".to_owned());
        self
    }

    /// Set the flags that hold, and mark them enumerated.
    #[must_use]
    pub fn with_flags(mut self, flags: impl IntoIterator<Item = String>) -> Self {
        self.flags = flags.into_iter().collect();
        self
    }

    /// Set a known key, marking that key enumerated.
    #[must_use]
    pub fn with_key(mut self, key: &str, value: &str) -> Self {
        self.key_values.insert(key.to_owned(), value.to_owned());
        self.closed_keys.insert(key.to_owned());
        self
    }

    /// Mark a bare flag as known-absent rather than merely unlisted.
    #[must_use]
    pub fn with_known_flags(mut self, keys: impl IntoIterator<Item = String>) -> Self {
        self.closed_keys.extend(keys);
        self
    }

    /// Evaluate a predicate against this environment.
    #[must_use]
    pub fn eval(&self, predicate: &CfgPredicate) -> Truth {
        match predicate {
            CfgPredicate::All(operands) => Truth::all(operands.iter().map(|p| self.eval(p))),
            CfgPredicate::Any(operands) => Truth::any(operands.iter().map(|p| self.eval(p))),
            CfgPredicate::Not(operand) => self.eval(operand).negate(),
            CfgPredicate::Flag(name) => {
                if self.flags.contains(name) {
                    Truth::True
                } else if self.closed_keys.contains(name) {
                    Truth::False
                } else {
                    Truth::Unknown
                }
            },
            CfgPredicate::KeyValue { key, value } => {
                if key == "feature" {
                    return if self.features.contains(value) {
                        Truth::True
                    } else if self.closed_keys.contains("feature") {
                        Truth::False
                    } else {
                        Truth::Unknown
                    };
                }
                match self.key_values.get(key) {
                    Some(known) if known == value => Truth::True,
                    Some(_) => Truth::False,
                    None if self.closed_keys.contains(key) => Truth::False,
                    None => Truth::Unknown,
                }
            },
        }
    }
}

/// Parse a `cfg` predicate from the text inside `cfg(…)`.
///
/// # Errors
/// [`CfgError`] when the text is empty, unbalanced, malformed, or has trailing content.
pub fn parse(text: &str) -> Result<CfgPredicate, CfgError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(CfgError::Empty);
    }
    let (predicate, rest) = parse_term(trimmed)?;
    let rest = rest.trim();
    if !rest.is_empty() {
        return Err(CfgError::Trailing(rest.to_owned()));
    }
    Ok(predicate)
}

/// Parse one term, returning it and the unconsumed remainder.
fn parse_term(text: &str) -> Result<(CfgPredicate, &str), CfgError> {
    let text = text.trim_start();
    let name_end = text
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    let name = &text[..name_end];
    if name.is_empty() {
        return Err(CfgError::Malformed(text.chars().take(16).collect()));
    }
    let rest = text[name_end..].trim_start();

    // `all(…)`, `any(…)`, `not(…)`
    if let Some(inner) = rest.strip_prefix('(') {
        let (operands, after) = parse_operands(inner)?;
        let predicate = match name {
            "all" => CfgPredicate::All(operands),
            "any" => CfgPredicate::Any(operands),
            "not" => CfgPredicate::Not(Box::new(
                operands
                    .into_iter()
                    .next()
                    .ok_or_else(|| CfgError::Malformed("not()".to_owned()))?,
            )),
            other => return Err(CfgError::Malformed(other.to_owned())),
        };
        return Ok((predicate, after));
    }

    // `key = "value"`
    if let Some(after_eq) = rest.strip_prefix('=') {
        let after_eq = after_eq.trim_start();
        let Some(without_quote) = after_eq.strip_prefix('"') else {
            return Err(CfgError::Malformed(after_eq.chars().take(16).collect()));
        };
        let Some(close) = without_quote.find('"') else {
            return Err(CfgError::Malformed(after_eq.chars().take(16).collect()));
        };
        return Ok((
            CfgPredicate::KeyValue {
                key: name.to_owned(),
                value: without_quote[..close].to_owned(),
            },
            &without_quote[close + 1..],
        ));
    }

    Ok((CfgPredicate::Flag(name.to_owned()), rest))
}

/// Parse a comma-separated operand list up to the matching `)`.
fn parse_operands(text: &str) -> Result<(Vec<CfgPredicate>, &str), CfgError> {
    let mut operands = Vec::new();
    let mut rest = text.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix(')') {
            return Ok((operands, after));
        }
        if rest.is_empty() {
            return Err(CfgError::Unclosed);
        }
        let (operand, remainder) = parse_term(rest)?;
        operands.push(operand);
        rest = remainder.trim_start();
        if let Some(after_comma) = rest.strip_prefix(',') {
            rest = after_comma.trim_start();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(name: &str) -> CfgPredicate {
        CfgPredicate::Flag(name.to_owned())
    }

    fn kv(key: &str, value: &str) -> CfgPredicate {
        CfgPredicate::KeyValue {
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    #[test]
    fn parses_a_bare_flag() -> Result<(), CfgError> {
        assert_eq!(parse("unix")?, flag("unix"));
        Ok(())
    }

    #[test]
    fn parses_a_key_value_pair() -> Result<(), CfgError> {
        assert_eq!(parse("feature = \"view\"")?, kv("feature", "view"));
        assert_eq!(parse("target_os=\"linux\"")?, kv("target_os", "linux"));
        Ok(())
    }

    #[test]
    fn parses_nested_combinators() -> Result<(), CfgError> {
        let parsed = parse("all(unix, not(test), any(feature = \"a\", feature = \"b\"))")?;
        assert_eq!(
            parsed,
            CfgPredicate::All(vec![
                flag("unix"),
                CfgPredicate::Not(Box::new(flag("test"))),
                CfgPredicate::Any(vec![kv("feature", "a"), kv("feature", "b")]),
            ])
        );
        Ok(())
    }

    #[test]
    fn rejects_malformed_predicates() {
        assert_eq!(parse(""), Err(CfgError::Empty));
        assert_eq!(parse("   "), Err(CfgError::Empty));
        assert_eq!(parse("all(unix"), Err(CfgError::Unclosed));
        assert!(matches!(parse("unix junk"), Err(CfgError::Trailing(_))));
        assert!(matches!(
            parse("feature = view"),
            Err(CfgError::Malformed(_))
        ));
        assert!(matches!(parse("bogus(unix)"), Err(CfgError::Malformed(_))));
    }

    #[test]
    fn kleene_conjunction_lets_one_false_settle_it() {
        assert_eq!(Truth::all([Truth::True, Truth::True]), Truth::True);
        assert_eq!(Truth::all([Truth::True, Truth::Unknown]), Truth::Unknown);
        // A gate that cannot open is closed, whatever else is uncertain.
        assert_eq!(Truth::all([Truth::Unknown, Truth::False]), Truth::False);
        assert_eq!(Truth::all([]), Truth::True);
    }

    #[test]
    fn kleene_disjunction_lets_one_true_settle_it() {
        assert_eq!(Truth::any([Truth::False, Truth::True]), Truth::True);
        assert_eq!(Truth::any([Truth::False, Truth::Unknown]), Truth::Unknown);
        assert_eq!(Truth::any([Truth::Unknown, Truth::True]), Truth::True);
        assert_eq!(Truth::any([]), Truth::False);
    }

    #[test]
    fn negating_unknown_stays_unknown() {
        assert_eq!(Truth::Unknown.negate(), Truth::Unknown);
        assert_eq!(Truth::True.negate(), Truth::False);
        assert_eq!(Truth::False.negate(), Truth::True);
    }

    #[test]
    fn a_feature_gate_is_decided_because_the_manifest_enumerates_features() -> Result<(), CfgError>
    {
        let env = CfgEnv::new().with_features(["view".to_owned()]);
        assert_eq!(env.eval(&parse("feature = \"view\"")?), Truth::True);
        // Absent from an enumerated set means genuinely off, not merely unknown.
        assert_eq!(env.eval(&parse("feature = \"other\"")?), Truth::False);
        Ok(())
    }

    #[test]
    fn an_unenumerated_feature_set_leaves_gates_undecided() -> Result<(), CfgError> {
        let env = CfgEnv::new();
        assert_eq!(env.eval(&parse("feature = \"view\"")?), Truth::Unknown);
        Ok(())
    }

    #[test]
    fn a_known_key_decides_both_ways() -> Result<(), CfgError> {
        let env = CfgEnv::new().with_key("target_os", "linux");
        assert_eq!(env.eval(&parse("target_os = \"linux\"")?), Truth::True);
        assert_eq!(env.eval(&parse("target_os = \"redox\"")?), Truth::False);
        Ok(())
    }

    #[test]
    fn an_unknown_key_is_never_guessed_to_be_false() -> Result<(), CfgError> {
        // This is the whole point: silently treating an unrecognized key as false would
        // delete code from the view while the header still claimed completeness.
        let env = CfgEnv::new().with_key("target_os", "linux");
        assert_eq!(env.eval(&parse("some_vendor_key")?), Truth::Unknown);
        assert_eq!(env.eval(&parse("nonstandard = \"x\"")?), Truth::Unknown);
        Ok(())
    }

    #[test]
    fn flags_are_decided_only_once_declared_enumerated() -> Result<(), CfgError> {
        let env = CfgEnv::new()
            .with_flags(["unix".to_owned()])
            .with_known_flags(["unix".to_owned(), "windows".to_owned(), "test".to_owned()]);
        assert_eq!(env.eval(&parse("unix")?), Truth::True);
        assert_eq!(env.eval(&parse("windows")?), Truth::False);
        assert_eq!(env.eval(&parse("test")?), Truth::False);
        Ok(())
    }

    #[test]
    fn unknown_propagates_through_a_nested_predicate() -> Result<(), CfgError> {
        let env = CfgEnv::new()
            .with_features(["view".to_owned()])
            .with_flags(["unix".to_owned()])
            .with_known_flags(["unix".to_owned()]);
        // `unix` is true, the vendor key is unknown, so the conjunction is unknown.
        assert_eq!(env.eval(&parse("all(unix, vendor_thing)")?), Truth::Unknown);
        // But adding a decidably-false operand settles it.
        assert_eq!(
            env.eval(&parse("all(unix, vendor_thing, feature = \"absent\")")?),
            Truth::False
        );
        Ok(())
    }
}
