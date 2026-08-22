//! The neutral nbformat-4 model, built to round-trip: every field karet does
//! not interpret is preserved verbatim (flattened `extra` maps), and `source`
//! keeps whichever of its two legal encodings the file used.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

/// A parsed notebook document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notebook {
    /// The nbformat major version (4).
    pub nbformat: u32,
    /// The nbformat minor version (2–5 in the wild).
    pub nbformat_minor: u32,
    /// Document metadata (kernelspec, language_info, …), preserved verbatim.
    #[serde(default)]
    pub metadata: Map<String, Value>,
    /// The cells, document order.
    #[serde(default)]
    pub cells: Vec<Cell>,
    /// Any top-level fields karet does not model, preserved for round-trip.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Notebook {
    /// The document's language for code fences: `language_info.name`, then
    /// `kernelspec.language`, then `python` (Jupyter's overwhelming default).
    #[must_use]
    pub fn language(&self) -> &str {
        let named = |section: &str, key: &str| -> Option<&str> {
            self.metadata.get(section)?.get(key)?.as_str()
        };
        named("language_info", "name")
            .or_else(|| named("kernelspec", "language"))
            .unwrap_or("python")
    }
}

/// One cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    /// The cell id (required from 4.5; absent in older files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The cell kind (`cell_type` on the wire).
    #[serde(rename = "cell_type")]
    pub kind: CellKind,
    /// Cell metadata, preserved verbatim.
    #[serde(default)]
    pub metadata: Map<String, Value>,
    /// The cell text, in whichever encoding the file used.
    #[serde(default)]
    pub source: Source,
    /// Code cells: the execution counter (`Some(None)` = present-but-null,
    /// i.e. not yet executed; `None` = the key is absent, as on markdown
    /// cells).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "double_option"
    )]
    pub execution_count: Option<Option<i64>>,
    /// Code cells: the outputs (`Some(vec![])` = present-but-empty; `None` =
    /// the key is absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<Output>>,
    /// Any cell fields karet does not model (attachments, …).
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A cell's kind (nbformat's `cell_type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellKind {
    /// Executable code.
    Code,
    /// Markdown prose.
    Markdown,
    /// Raw pass-through content.
    Raw,
}

/// Cell/stream text: nbformat allows one joined string or a list of lines
/// (each usually keeping its `\n`). The parsed form is preserved so a
/// round-trip writes what it read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Source {
    /// One joined string.
    Joined(String),
    /// A list of line chunks.
    Lines(Vec<String>),
}

impl Default for Source {
    /// The list form, Jupyter's own on-disk default.
    fn default() -> Self {
        Self::Lines(Vec::new())
    }
}

impl Source {
    /// The joined text, whichever encoding held it.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Joined(text) => text.clone(),
            Self::Lines(lines) => lines.concat(),
        }
    }
}

/// One output of a code cell, tagged by nbformat's `output_type`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "output_type", rename_all = "snake_case")]
pub enum Output {
    /// `stdout`/`stderr` text.
    Stream {
        /// The stream name (`stdout` or `stderr`).
        name: String,
        /// The text, in whichever encoding the file used.
        #[serde(default)]
        text: Source,
        /// Unmodeled fields, preserved.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// The result of the cell's last expression, as a MIME bundle.
    ExecuteResult {
        /// The producing execution counter (nullable per spec).
        execution_count: Option<i64>,
        /// MIME type → content.
        #[serde(default)]
        data: Map<String, Value>,
        /// Per-MIME metadata.
        #[serde(default)]
        metadata: Map<String, Value>,
        /// Unmodeled fields, preserved.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// Rich display output (a MIME bundle without a counter).
    DisplayData {
        /// MIME type → content.
        #[serde(default)]
        data: Map<String, Value>,
        /// Per-MIME metadata.
        #[serde(default)]
        metadata: Map<String, Value>,
        /// Unmodeled fields, preserved.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
    /// An exception, with an ANSI-styled traceback.
    Error {
        /// The exception class name.
        ename: String,
        /// The rendered exception value.
        evalue: String,
        /// Traceback lines (ANSI escapes and all).
        #[serde(default)]
        traceback: Vec<String>,
        /// Unmodeled fields, preserved.
        #[serde(flatten)]
        extra: Map<String, Value>,
    },
}

/// `Option<Option<T>>` over the wire: distinguishes an absent key from a
/// present `null` (a not-yet-executed code cell), which plain `Option`
/// cannot round-trip.
mod double_option {
    use serde::Deserialize;
    use serde::Deserializer;
    use serde::Serialize;
    use serde::Serializer;

    pub fn serialize<S: Serializer>(
        value: &Option<Option<i64>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(serializer),
            // Unreachable in practice: skip_serializing_if drops the None.
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<i64>>, D::Error> {
        Option::<i64>::deserialize(deserializer).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn source_round_trips_both_encodings() {
        let joined: Source = serde_json::from_value(json!("a\nb\n")).unwrap_or_default();
        assert_eq!(joined, Source::Joined("a\nb\n".to_owned()));
        assert_eq!(joined.text(), "a\nb\n");
        let lines: Source = serde_json::from_value(json!(["a\n", "b\n"])).unwrap_or_default();
        assert_eq!(lines.text(), "a\nb\n");
        assert_eq!(
            serde_json::to_value(&lines).unwrap_or_default(),
            json!(["a\n", "b\n"])
        );
    }

    #[test]
    fn execution_count_distinguishes_null_from_absent() -> Result<(), serde_json::Error> {
        let code: Cell = serde_json::from_value(json!({
            "cell_type": "code", "source": [], "outputs": [], "execution_count": null,
            "metadata": {}
        }))?;
        assert_eq!(code.execution_count, Some(None));
        let encoded = serde_json::to_value(&code)?;
        assert_eq!(encoded.get("execution_count"), Some(&Value::Null));
        assert_eq!(encoded.get("outputs"), Some(&json!([])));

        let markdown: Cell = serde_json::from_value(json!({
            "cell_type": "markdown", "source": "hi", "metadata": {}
        }))?;
        assert_eq!(markdown.execution_count, None);
        let encoded = serde_json::to_value(&markdown)?;
        assert_eq!(encoded.get("execution_count"), None);
        assert_eq!(encoded.get("outputs"), None);
        Ok(())
    }

    #[test]
    fn language_prefers_language_info_then_kernelspec() {
        let mut notebook = Notebook {
            nbformat: 4,
            nbformat_minor: 5,
            metadata: Map::new(),
            cells: Vec::new(),
            extra: Map::new(),
        };
        assert_eq!(notebook.language(), "python");
        notebook.metadata.insert(
            "kernelspec".to_owned(),
            json!({"language": "julia", "name": "julia-1.10"}),
        );
        assert_eq!(notebook.language(), "julia");
        notebook
            .metadata
            .insert("language_info".to_owned(), json!({"name": "rust"}));
        assert_eq!(notebook.language(), "rust");
    }
}
