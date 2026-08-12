//! Debounced, token-aware spell checking for editable documents.
//!
//! This module is the *worker*: it owns the per-document debounce loop and the
//! dictionary cache. The pure text-in/diagnostics-out pass lives in [`check`], so a
//! bulk workspace scan can share it without going through this queue.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::time::Duration;
use std::time::Instant;

use karet_core::Diagnostic;
use karet_syntax::Highlights;
use spellbook::Dictionary;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::DocumentId;
use crate::api::SpellingLanguage;
use crate::config::schema::Spellcheck;

pub(crate) mod check;
mod context;
pub(crate) mod scope;

use check::SpellInput;

/// One immutable document version queued for checking.
pub(crate) struct SpellJob {
    pub doc: DocumentId,
    pub version: u64,
    pub language: Option<&'static str>,
    /// The stable per-language settings selector (`karet-filetype`'s
    /// `config_selector`) — the machine key for language-aware behavior. The
    /// display `language` above is presentation-only and never dispatched on.
    pub language_selector: Option<&'static str>,
    pub spelling_language: SpellingLanguage,
    pub text: String,
    pub highlights: Arc<Highlights>,
    pub syntax_error_lines: Arc<Vec<(u32, u32)>>,
    pub settings: Spellcheck,
}

/// A spell-check result tagged with the exact source version it describes.
pub(crate) struct SpellResult {
    pub doc: DocumentId,
    pub version: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub error: Option<String>,
}

struct Pending {
    due: Instant,
    job: SpellJob,
}

/// Start the coalescing worker. Every document owns its own debounce deadline;
/// newer jobs replace older versions without delaying unrelated documents.
pub(crate) fn spawn() -> (Sender<SpellJob>, tokio_mpsc::UnboundedReceiver<SpellResult>) {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let (results_tx, results_rx) = tokio_mpsc::unbounded_channel();
    let _ = std::thread::Builder::new()
        .name("karet-spell".to_owned())
        .spawn(move || run(&jobs_rx, &results_tx));
    (jobs_tx, results_rx)
}

fn run(jobs: &Receiver<SpellJob>, results: &tokio_mpsc::UnboundedSender<SpellResult>) {
    let mut pending: HashMap<DocumentId, Pending> = HashMap::new();
    let mut dictionaries: HashMap<SpellingLanguage, Result<Dictionary, String>> = HashMap::new();
    loop {
        let now = Instant::now();
        let wait = pending
            .values()
            .map(|pending| pending.due.saturating_duration_since(now))
            .min();
        let received = match wait {
            Some(wait) => jobs.recv_timeout(wait),
            None => match jobs.recv() {
                Ok(job) => Ok(job),
                Err(_) => break,
            },
        };
        match received {
            Ok(job) => {
                let delay = Duration::from_millis(job.settings.debounce_ms.clamp(50, 5_000));
                pending.insert(
                    job.doc,
                    Pending {
                        due: Instant::now() + delay,
                        job,
                    },
                );
            },
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                let ready: Vec<DocumentId> = pending
                    .iter()
                    .filter(|(_, pending)| pending.due <= now)
                    .map(|(doc, _)| *doc)
                    .collect();
                for doc in ready {
                    if let Some(pending) = pending.remove(&doc) {
                        let result = check_job(pending.job, &mut dictionaries);
                        if results.send(result).is_err() {
                            return;
                        }
                    }
                }
            },
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn check_job(
    job: SpellJob,
    dictionaries: &mut HashMap<SpellingLanguage, Result<Dictionary, String>>,
) -> SpellResult {
    let dictionary = dictionaries
        .entry(job.spelling_language)
        .or_insert_with(|| load_dictionary(job.spelling_language));
    match dictionary {
        Ok(dictionary) => SpellResult {
            doc: job.doc,
            version: job.version,
            diagnostics: check_text(&job, dictionary),
            error: None,
        },
        Err(error) => SpellResult {
            doc: job.doc,
            version: job.version,
            diagnostics: Vec::new(),
            error: Some(error.clone()),
        },
    }
}

/// Run the shared checking pass over one queued document version. The editor path
/// always wants suggestions — the quick-fix menu recovers them from the message.
fn check_text(job: &SpellJob, dictionary: &Dictionary) -> Vec<Diagnostic> {
    check::check(
        &SpellInput {
            text: &job.text,
            language: job.language,
            language_selector: job.language_selector,
            spelling_language: job.spelling_language,
            highlights: job.highlights.as_ref(),
            syntax_error_lines: job.syntax_error_lines.as_ref(),
            settings: &job.settings,
            suggest: true,
        },
        dictionary,
    )
}

/// Load `language`'s Hunspell dictionary from the conventional search roots.
pub(crate) fn load_dictionary(language: SpellingLanguage) -> Result<Dictionary, String> {
    load_dictionary_from_roots(language, dictionary_roots())
}

fn load_dictionary_from_roots(
    language: SpellingLanguage,
    roots: impl IntoIterator<Item = PathBuf>,
) -> Result<Dictionary, String> {
    let locale = language.locale();
    for root in roots {
        let aff_path = root.join(format!("{locale}.aff"));
        let dic_path = root.join(format!("{locale}.dic"));
        let aff = match std::fs::read(&aff_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "could not read spell-check dictionary {}: {error}",
                    aff_path.display()
                ));
            },
        };
        let dic = match std::fs::read(&dic_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "could not read spell-check dictionary {}: {error}",
                    dic_path.display()
                ));
            },
        };
        let (aff, dic) = decode_dictionary_files(&aff, &dic).map_err(|error| {
            format!(
                "spell-check dictionary {locale} is invalid at {}: {error}",
                root.display()
            )
        })?;
        return Dictionary::new(&aff, &dic).map_err(|error| {
            format!(
                "spell-check dictionary {locale} is invalid at {}: {error}",
                root.display()
            )
        });
    }
    Err(format!(
        "spell-check dictionary {locale} was not found; install Hunspell dictionaries or copy {locale}.aff and {locale}.dic into {}",
        user_dictionary_dir().map_or_else(
            || "the karet data dictionary directory".to_owned(),
            |path| path.display().to_string()
        )
    ))
}

fn decode_dictionary_files(aff: &[u8], dic: &[u8]) -> Result<(String, String), String> {
    let encoding = aff
        .split(|byte| *byte == b'\n')
        .find_map(|line| {
            let mut words = line
                .split(u8::is_ascii_whitespace)
                .filter(|word| !word.is_empty());
            let key = words.next()?;
            key.eq_ignore_ascii_case(b"SET")
                .then(|| words.next())
                .flatten()
        })
        .unwrap_or(b"UTF-8");

    if encoding.eq_ignore_ascii_case(b"UTF-8") || encoding.eq_ignore_ascii_case(b"UTF8") {
        let aff = String::from_utf8(aff.to_vec())
            .map_err(|error| format!("affix file is not valid UTF-8: {error}"))?;
        let dic = String::from_utf8(dic.to_vec())
            .map_err(|error| format!("word list is not valid UTF-8: {error}"))?;
        return Ok((aff, dic));
    }
    if encoding.eq_ignore_ascii_case(b"ISO8859-1")
        || encoding.eq_ignore_ascii_case(b"ISO-8859-1")
        || encoding.eq_ignore_ascii_case(b"LATIN1")
    {
        let decode_latin1 = |bytes: &[u8]| bytes.iter().copied().map(char::from).collect();
        return Ok((decode_latin1(aff), decode_latin1(dic)));
    }

    Err(format!(
        "unsupported character encoding {}",
        String::from_utf8_lossy(encoding)
    ))
}

fn dictionary_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(paths) = std::env::var_os("DICPATH") {
        roots.extend(std::env::split_paths(&paths));
    }
    if let Some(path) = user_dictionary_dir() {
        roots.push(path);
    }
    if let Some(base) = directories::BaseDirs::new() {
        roots.push(base.home_dir().join(".local/share/hunspell"));
        roots.push(base.home_dir().join("Library/Spelling"));
    }
    roots.extend([
        PathBuf::from("/usr/share/hunspell"),
        PathBuf::from("/usr/share/myspell"),
        PathBuf::from("/usr/local/share/hunspell"),
        PathBuf::from("/Library/Spelling"),
    ]);
    roots
}

fn user_dictionary_dir() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("", "getkono", "karet")?
            .data_local_dir()
            .join("dictionaries"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_loader_decodes_latin1_hunspell_files() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(
            dir.path().join("en_GB.aff"),
            b"SET ISO8859-1\nTRY abcdefghijklmnopqrstuvwxyz\xe9\n",
        )?;
        std::fs::write(dir.path().join("en_GB.dic"), b"2\nhello\ncaf\xe9\n")?;

        let dictionary = load_dictionary_from_roots(
            SpellingLanguage::EnglishUnitedKingdom,
            [dir.path().to_path_buf()],
        )
        .map_err(std::io::Error::other)?;

        assert!(dictionary.check("hello"));
        assert!(dictionary.check("café"));
        Ok(())
    }

    #[test]
    fn dictionary_loader_reports_unsupported_encoding() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("en_GB.aff"), b"SET KOI8-R\n")?;
        std::fs::write(dir.path().join("en_GB.dic"), b"1\nhello\n")?;

        let error = load_dictionary_from_roots(
            SpellingLanguage::EnglishUnitedKingdom,
            [dir.path().to_path_buf()],
        )
        .err()
        .unwrap_or_default();

        assert!(error.contains("unsupported character encoding KOI8-R"));
        Ok(())
    }
}
