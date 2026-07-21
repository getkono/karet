//! Debounced, token-aware spell checking for editable documents.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::Sender;
use std::time::Duration;
use std::time::Instant;

use karet_core::Diagnostic;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Severity;
use karet_core::StandardToken;
use karet_core::TokenId;
use karet_syntax::Highlights;
use spellbook::Dictionary;
use tokio::sync::mpsc as tokio_mpsc;

use crate::api::DocumentId;
use crate::api::SpellingLanguage;
use crate::config::schema::Spellcheck;

/// One immutable document version queued for checking.
pub(crate) struct SpellJob {
    pub doc: DocumentId,
    pub version: u64,
    pub language: Option<&'static str>,
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

fn load_dictionary(language: SpellingLanguage) -> Result<Dictionary, String> {
    let locale = language.locale();
    for root in dictionary_roots() {
        let aff_path = root.join(format!("{locale}.aff"));
        let dic_path = root.join(format!("{locale}.dic"));
        let (Ok(aff), Ok(dic)) = (
            std::fs::read_to_string(&aff_path),
            std::fs::read_to_string(&dic_path),
        ) else {
            continue;
        };
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

fn check_text(job: &SpellJob, dictionary: &Dictionary) -> Vec<Diagnostic> {
    let custom: HashSet<String> = job
        .settings
        .words
        .iter()
        .map(|word| word.to_lowercase())
        .collect();
    let document = is_prose_document(job.language) && job.settings.documents;
    let line_index = LineIndex::new(&job.text);
    let mut diagnostics = Vec::new();
    for (start, end, word) in words(&job.text) {
        if !scope_allows(job, start, end, document)
            || should_skip_context(&job.text, start, end)
            || custom.contains(&word.to_lowercase())
            || dictionary.check(word)
            || should_skip_proper_name(word)
        {
            continue;
        }
        let mut suggestions = Vec::new();
        dictionary
            .suggester()
            .with_ngram_suggestions(false)
            .suggest(word, &mut suggestions);
        suggestions.truncate(3);
        let message = if suggestions.is_empty() {
            format!("Unknown word “{word}”")
        } else {
            format!("Unknown word “{word}”; try {}", suggestions.join(", "))
        };
        diagnostics.push(Diagnostic {
            range: Range {
                start: line_index.position(start),
                end: line_index.position(end),
            },
            severity: Severity::Warning,
            message,
            source: Some("karet-spell".to_owned()),
            code: Some(job.spelling_language.locale().to_owned()),
            tags: Vec::new(),
            related: Vec::new(),
        });
    }
    diagnostics
}

fn is_prose_document(language: Option<&str>) -> bool {
    language.is_some_and(|language| {
        matches!(
            language.to_ascii_lowercase().as_str(),
            "markdown" | "plain text" | "asciidoc" | "restructuredtext" | "tex"
        )
    })
}

fn scope_allows(job: &SpellJob, start: usize, end: usize, document: bool) -> bool {
    let token = token_at(job.highlights.as_ref(), start);
    if document {
        return !matches!(
            token,
            Some(token)
                if token == StandardToken::MarkupRaw.id()
                    || token == StandardToken::MarkupLink.id()
                    || is_code_token(token)
        );
    }
    if job.settings.comments
        && token.is_some_and(|token| is_comment_token(token) || is_markup_prose_token(token))
    {
        return true;
    }
    if job.settings.strings && token == Some(TokenId::STRING) {
        return true;
    }
    job.settings.identifiers
        && job.syntax_error_lines.is_empty()
        && token.is_some_and(is_identifier_token)
        && end > start
}

fn token_at(highlights: &Highlights, byte: usize) -> Option<TokenId> {
    let spans = highlights.all();
    let before = spans.partition_point(|span| span.span.start.0 <= byte);
    spans[..before]
        .iter()
        .rev()
        .find(|span| byte < span.span.end.0)
        .map(|span| span.token)
}

fn is_comment_token(token: TokenId) -> bool {
    token == TokenId::COMMENT
        || token == StandardToken::CommentDoc.id()
        || token == StandardToken::CommentMark.id()
}

fn is_markup_prose_token(token: TokenId) -> bool {
    matches!(
        token,
        token if token == StandardToken::MarkupHeading.id()
            || token == StandardToken::MarkupBold.id()
            || token == StandardToken::MarkupItalic.id()
            || token == StandardToken::MarkupQuote.id()
            || token == StandardToken::MarkupStrikethrough.id()
    )
}

fn is_code_token(token: TokenId) -> bool {
    token == TokenId::KEYWORD
        || token == TokenId::FUNCTION
        || token == TokenId::TYPE
        || token == TokenId::VARIABLE
        || token == TokenId::CONSTANT
        || token == TokenId::NUMBER
        || token == TokenId::OPERATOR
        || token == TokenId::STRING
}

fn is_identifier_token(token: TokenId) -> bool {
    token == TokenId::TYPE
        || token == TokenId::FUNCTION
        || token == StandardToken::Method.id()
        || token == StandardToken::Property.id()
}

fn should_skip_context(text: &str, start: usize, end: usize) -> bool {
    let chunk_start = text[..start]
        .rfind(char::is_whitespace)
        .map_or(0, |index| index + 1);
    let chunk_end = text[end..]
        .find(char::is_whitespace)
        .map_or(text.len(), |index| end + index);
    let chunk = &text[chunk_start..chunk_end];
    chunk.contains("://")
        || chunk.contains('@')
        || chunk.contains("::")
        || chunk.contains('_')
        || chunk.contains('\\')
        || chunk.chars().any(|character| character.is_ascii_digit())
}

fn should_skip_proper_name(word: &str) -> bool {
    const COMMON_MISSPELLINGS: &[&str] = &[
        "accomodate",
        "definately",
        "occured",
        "recieve",
        "seperate",
        "teh",
    ];
    let lower = word.to_lowercase();
    if COMMON_MISSPELLINGS.contains(&lower.as_str()) {
        return false;
    }
    let mut characters = word.chars();
    let title_case = characters.next().is_some_and(char::is_uppercase)
        && characters
            .clone()
            .all(|character| !character.is_uppercase());
    let internal_uppercase = characters.any(char::is_uppercase);
    title_case || internal_uppercase || (word.len() > 1 && word.chars().all(char::is_uppercase))
}

fn words(text: &str) -> Vec<(usize, usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        let word_character = character.is_alphabetic()
            || (character == '\''
                && start.is_some()
                && text[index + character.len_utf8()..]
                    .chars()
                    .next()
                    .is_some_and(char::is_alphabetic));
        match (start, word_character) {
            (None, true) => start = Some(index),
            (Some(word_start), false) => {
                words.push((word_start, index, &text[word_start..index]));
                start = None;
            },
            _ => {},
        }
    }
    if let Some(start) = start {
        words.push((start, text.len(), &text[start..]));
    }
    words
}

struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.match_indices('\n')
                .map(|(index, _)| index.saturating_add(1)),
        );
        Self { text, starts }
    }

    fn position(&self, byte: usize) -> LineCol {
        let line = self.starts.partition_point(|start| *start <= byte) - 1;
        let column = self.starts.get(line).map_or(0, |start| {
            u32::try_from(self.text[*start..byte].chars().count()).unwrap_or(u32::MAX)
        });
        LineCol::new(line as u32, column)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use karet_syntax::LayeredHighlighter;
    use karet_treesitter::LayeredParser;
    use karet_treesitter::language_id_from_path;

    use super::*;

    const AFF: &str = "SET UTF-8\nSFX S Y 1\nSFX S 0 s .\n";
    const DIC: &str = "8\nhello\nworld/S\nthis\nis\na\ncomment\nreceive\ntext\n";

    fn dictionary() -> Option<Dictionary> {
        Dictionary::new(AFF, DIC).ok()
    }

    fn job(text: &str, language: &'static str) -> SpellJob {
        let settings = Spellcheck {
            enabled: true,
            ..Spellcheck::default()
        };
        let path = if language == "Markdown" {
            Path::new("notes.md")
        } else {
            Path::new("source.rs")
        };
        let highlights = language_id_from_path(path)
            .and_then(|language| {
                let mut parser = LayeredParser::new();
                parser.parse(language, text).ok()
            })
            .map_or_else(Highlights::default, |tree| {
                LayeredHighlighter::new().highlight(&tree, text)
            });
        SpellJob {
            doc: DocumentId(1),
            version: 1,
            language: Some(language),
            spelling_language: SpellingLanguage::EnglishUnitedStates,
            text: text.to_owned(),
            highlights: Arc::new(highlights),
            syntax_error_lines: Arc::default(),
            settings,
        }
    }

    #[test]
    fn markdown_checks_prose_but_skips_code_and_links() {
        let Some(dictionary) = dictionary() else {
            return;
        };
        let text = "hello wrld `mistke` https://example.test/badd\n";
        let diagnostics = check_text(&job(text, "Markdown"), &dictionary);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("wrld"));
    }

    #[test]
    fn source_checks_comments_only_and_respects_custom_words() {
        let Some(dictionary) = dictionary() else {
            return;
        };
        let text = "let misspeled = 1; // this commment is Karet\n";
        let mut request = job(text, "Rust");
        request.settings.words.push("commment".to_owned());
        assert!(check_text(&request, &dictionary).is_empty());
    }

    #[test]
    fn optional_string_and_identifier_scopes_are_independent() {
        let Some(dictionary) = dictionary() else {
            return;
        };
        let text = "fn misspeled() { let value = \"wrld\"; }\n";
        let mut request = job(text, "Rust");
        assert!(check_text(&request, &dictionary).is_empty());

        request.settings.strings = true;
        let strings = check_text(&request, &dictionary);
        assert_eq!(strings.len(), 1);
        assert!(strings[0].message.contains("wrld"));

        request.settings.strings = false;
        request.settings.identifiers = true;
        let identifiers = check_text(&request, &dictionary);
        assert_eq!(identifiers.len(), 1);
        assert!(identifiers[0].message.contains("misspeled"));

        request.syntax_error_lines = Arc::new(vec![(0, 0)]);
        assert!(
            check_text(&request, &dictionary).is_empty(),
            "identifier linting pauses while syntax is invalid"
        );
    }

    #[test]
    fn prose_scope_can_be_disabled_without_affecting_the_feature_toggle() {
        let Some(dictionary) = dictionary() else {
            return;
        };
        let mut request = job("hello wrld\n", "Markdown");
        request.settings.documents = false;
        assert!(check_text(&request, &dictionary).is_empty());
    }

    #[test]
    fn common_misspelling_is_not_hidden_as_a_proper_name() {
        let Some(dictionary) = dictionary() else {
            return;
        };
        let diagnostics = check_text(&job("Recieve this\n", "Markdown"), &dictionary);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("Recieve"));
    }

    #[test]
    fn word_tokenizer_keeps_internal_apostrophes_and_utf8_offsets() {
        assert_eq!(
            words("élan isn't end"),
            vec![(0, 5, "élan"), (6, 11, "isn't"), (12, 15, "end")]
        );
    }

    #[test]
    fn prose_document_names_match_the_filetype_registry() {
        for language in [
            "Markdown",
            "Plain Text",
            "reStructuredText",
            "AsciiDoc",
            "TeX",
        ] {
            assert!(is_prose_document(Some(language)), "{language}");
        }
        assert!(!is_prose_document(Some("Rust")));
        assert!(!is_prose_document(None));
    }
}
