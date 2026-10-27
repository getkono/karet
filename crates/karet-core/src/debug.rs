//! Neutral debugger inspection model: call-stack frames, variable scopes,
//! variables, and the stream a piece of debuggee output arrived on.
//!
//! A debug-adapter client (`karet-dap`) produces these; the backend carries
//! them to the presentation unchanged. Positions are 0-based like the rest of
//! the toolkit. The two adapter handles are distinct newtypes — a
//! [`FrameId`] names a frame, a [`VariablesRef`] names a set of children — so
//! passing one where the other belongs does not compile.

use std::path::PathBuf;

/// The adapter's handle for one stack frame: what scope and evaluate
/// requests are addressed to. Valid only while the thread stays stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FrameId(pub i64);

/// The adapter's handle for a set of children: a scope's variables, or a
/// structured value's fields. Zero (or negative) means "no children"; see
/// [`VariablesRef::has_children`]. Valid only while the thread stays stopped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VariablesRef(pub i64);

impl VariablesRef {
    /// The handle a leaf carries: nothing to fetch.
    pub const NONE: Self = Self(0);

    /// Whether the handle names children that can be fetched. The protocol
    /// reserves zero for "none"; a negative handle is malformed and treated
    /// the same way.
    #[must_use]
    pub fn has_children(self) -> bool {
        self.0 > 0
    }
}

/// One frame of a stopped thread's call stack, top frame first in a stack.
///
/// Constructed with literal syntax by producers and tests, so this struct is
/// deliberately not `#[non_exhaustive]`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StackFrame {
    /// The handle scope and evaluate requests take.
    pub id: FrameId,
    /// The frame name (usually a function).
    pub name: String,
    /// The 0-based line the frame is stopped at.
    pub line: u32,
    /// The 0-based column.
    pub column: u32,
    /// The source file, when the adapter reports a path.
    pub source_path: Option<PathBuf>,
}

/// One variables scope of a frame (Locals, Registers, …).
///
/// Deliberately not `#[non_exhaustive]` (see [`StackFrame`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DebugScope {
    /// The scope name.
    pub name: String,
    /// The handle that fetches the scope's variables.
    pub variables_reference: VariablesRef,
    /// Whether fetching the scope is expensive, so it is fetched only on
    /// demand rather than eagerly.
    pub expensive: bool,
}

/// One variable within a scope or a structured parent.
///
/// Deliberately not `#[non_exhaustive]` (see [`StackFrame`]).
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DebugVariable {
    /// The variable name.
    pub name: String,
    /// Its rendered value.
    pub value: String,
    /// The type, when the adapter reports one.
    pub ty: Option<String>,
    /// The handle that fetches its children; [`VariablesRef::NONE`] for a
    /// leaf.
    pub variables_reference: VariablesRef,
}

/// The stream a piece of debugger output arrived on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DebugOutputCategory {
    /// The adapter's own console messages; the protocol's default when an
    /// adapter names no category.
    #[default]
    Console,
    /// The debuggee's standard output.
    Stdout,
    /// The debuggee's standard error.
    Stderr,
    /// Any other category the adapter names (`"important"`, `"telemetry"`,
    /// an adapter-specific one), kept verbatim.
    Other(String),
}

impl DebugOutputCategory {
    /// Classify a protocol category name. Matching is exact, as the protocol
    /// spells the well-known names in lower case.
    #[must_use]
    pub fn from_wire(name: &str) -> Self {
        match name {
            "console" => Self::Console,
            "stdout" => Self::Stdout,
            "stderr" => Self::Stderr,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The protocol category name this value was classified from.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Console => "console",
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Other(name) => name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_positive_references_have_children() {
        assert!(!VariablesRef::NONE.has_children());
        assert!(!VariablesRef::default().has_children());
        assert!(!VariablesRef(-3).has_children());
        assert!(VariablesRef(1).has_children());
    }

    #[test]
    fn output_categories_round_trip_their_wire_names() {
        for name in ["console", "stdout", "stderr", "important", ""] {
            assert_eq!(DebugOutputCategory::from_wire(name).as_str(), name);
        }
        assert_eq!(
            DebugOutputCategory::from_wire("stderr"),
            DebugOutputCategory::Stderr
        );
        assert_eq!(
            DebugOutputCategory::from_wire("important"),
            DebugOutputCategory::Other("important".to_owned())
        );
    }

    #[test]
    fn output_category_matching_is_exact() {
        assert_eq!(
            DebugOutputCategory::from_wire("STDERR"),
            DebugOutputCategory::Other("STDERR".to_owned())
        );
    }

    #[test]
    fn output_category_defaults_to_console() {
        assert_eq!(DebugOutputCategory::default(), DebugOutputCategory::Console);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn handles_serialize_as_bare_numbers_and_models_round_trip() -> Result<(), serde_json::Error> {
        let frame = StackFrame {
            id: FrameId(41),
            name: "main".to_owned(),
            line: 2,
            column: 0,
            source_path: Some(PathBuf::from("src/main.rs")),
        };
        let json = serde_json::to_value(&frame)?;
        assert_eq!(json["id"], serde_json::json!(41));
        assert_eq!(serde_json::from_value::<StackFrame>(json)?, frame);

        let variable = DebugVariable {
            name: "v".to_owned(),
            value: "Vec(2)".to_owned(),
            ty: None,
            variables_reference: VariablesRef(9),
        };
        let json = serde_json::to_string(&variable)?;
        assert_eq!(serde_json::from_str::<DebugVariable>(&json)?, variable);

        let category = DebugOutputCategory::Other("important".to_owned());
        let json = serde_json::to_string(&category)?;
        assert_eq!(serde_json::from_str::<DebugOutputCategory>(&json)?, category);
        Ok(())
    }
}
