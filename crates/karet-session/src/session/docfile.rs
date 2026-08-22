//! Reading a document off disk and writing it back: format classification,
//! the per-document serialization settings, and the save normalization.
//!
//! Split out of `session.rs` to keep that file under the workspace code-line
//! ceiling — a pure relocation, no behaviour change.

use super::*;

/// How many leading bytes to sample when classifying a document's on-disk format.
const CLASSIFY_HEAD: usize = 8192;

/// Load `path` into an editable buffer, decoding a known binary format (CBOR) to
/// text, and report the [`DocFormat`] to re-encode with on save.
///
/// The buffer records the on-disk fingerprint of the *original* bytes so the
/// file-watcher can still recognize the editor's own writes.
pub(super) fn load_document(path: &Path) -> Result<(TextBuffer, DocFormat), DocumentLoadError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(DocumentLoadError::Missing);
        },
        Err(error) => return Err(LoadError::Io(error.to_string()).into()),
    };
    // Format detection ignores the size guard: once the session is asked to open a
    // document it must decode it correctly regardless of size (the guard is an
    // app-level *routing* choice), so a large CBOR still decodes rather than being
    // mistaken for plain text.
    #[cfg(feature = "cbor")]
    {
        let head = &bytes[..bytes.len().min(CLASSIFY_HEAD)];
        if classify_ignoring_size(path, head) == FileKind::Cbor {
            let text =
                karet_cbor::decode_to_text(&bytes).map_err(|_| DocumentLoadError::Undecodable)?;
            let mut buffer = TextBuffer::from_text(&text);
            buffer.record_disk_state(path, &bytes);
            return Ok((buffer, DocFormat::Cbor));
        }
    }
    let mut buffer = TextBuffer::from_bytes(&bytes)?;
    buffer.record_disk_state(path, &bytes);
    Ok((buffer, DocFormat::Text))
}

pub(super) fn resolve_document_settings(
    path: &Path,
    language: Option<&str>,
    settings: &crate::config::Settings,
) -> (DocumentSettings, Option<String>) {
    let (resolved, editorconfig_error) =
        match crate::editorconfig::resolve(path, language, settings) {
            Ok(resolved) => (resolved, None),
            Err(error) => (
                crate::editorconfig::defaults(language, settings),
                Some(format!("EditorConfig: {error}")),
            ),
        };
    let language_error = (settings.spellcheck.enabled && resolved.spelling_language.is_none())
        .then(|| {
            format!(
                "spell-checking is enabled for {}, but no supported language resolved; use en_US or en_GB",
                path.display()
            )
        });
    let error = match (editorconfig_error, language_error) {
        (Some(editorconfig), Some(language)) => Some(format!("{editorconfig}; {language}")),
        (Some(error), None) | (None, Some(error)) => Some(error),
        (None, None) => None,
    };
    (resolved, error)
}

pub(super) fn apply_serialization_settings(buffer: &mut TextBuffer, settings: DocumentSettings) {
    match settings.line_ending {
        Some(DocumentLineEnding::Lf) => buffer.set_eol(TextEol::Lf),
        Some(DocumentLineEnding::Crlf) => buffer.set_eol(TextEol::Crlf),
        None => {},
    }
    match settings.encoding {
        Some(DocumentEncoding::Utf8) => buffer.set_encoding(Encoding::Utf8),
        Some(DocumentEncoding::Utf8Bom) => buffer.set_encoding(Encoding::Utf8Bom),
        None => {},
    }
}

pub(super) fn normalize_text_for_save(text: &str, settings: DocumentSettings) -> String {
    let mut normalized = String::with_capacity(text.len().saturating_add(1));
    for segment in text.split_inclusive('\n') {
        if let Some(line) = segment.strip_suffix('\n') {
            if settings.trim_trailing_whitespace {
                normalized.push_str(line.trim_end_matches([' ', '\t']));
            } else {
                normalized.push_str(line);
            }
            normalized.push('\n');
        } else {
            // The specification trims whitespace preceding a newline. Whitespace
            // at EOF has no following newline and is therefore preserved.
            normalized.push_str(segment);
        }
    }
    if settings.insert_final_newline && !normalized.is_empty() && !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    normalized
}

/// Save `doc` to disk, re-encoding a decoded binary format (CBOR) from its edit
/// text. A CBOR encode error (e.g. malformed diagnostic notation after editing)
/// leaves the file untouched and surfaces as a save failure. Returns
/// [`TextError::Conflict`] distinctly (rather than a generic IO error) so the
/// caller can prompt the user instead of just reporting a failure.
pub(super) fn save_document(doc: &mut Document) -> Result<(), TextError> {
    let result = match (doc.format, doc.must_create) {
        (DocFormat::Text, false) => doc.buffer.save(&doc.path).map(|_| ()),
        (DocFormat::Text, true) => doc.buffer.save_new(&doc.path).map(|_| ()),
        #[cfg(feature = "cbor")]
        (DocFormat::Cbor, must_create) => {
            let text = doc.buffer.text();
            let bytes =
                karet_cbor::encode_from_text(&text).map_err(|e| TextError::Io(e.to_string()))?;
            if must_create {
                doc.buffer.save_new_bytes(&doc.path, &bytes).map(|_| ())
            } else {
                doc.buffer.save_bytes(&doc.path, &bytes).map(|_| ())
            }
        },
    };
    if result.is_ok() {
        doc.must_create = false;
    }
    result
}
