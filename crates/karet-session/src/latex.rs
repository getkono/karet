//! External LaTeX root discovery, compilation, cancellation, and diagnostics.

use std::collections::HashSet;
use std::hash::Hash;
use std::hash::Hasher;
use std::io::Read;
use std::io::Seek;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use karet_core::Diagnostic;
use karet_core::LineCol;
use karet_core::Range;
use karet_core::Severity;
use tokio::sync::mpsc::UnboundedSender;

use crate::api::DocumentId;
use crate::api::Event;
use crate::api::RequestId;
use crate::cancellation::Cancellation;
use crate::config::schema::Latex;

/// One independent build request. Builds are serialized to avoid competing TeX
/// processes writing the same auxiliary directory.
pub(crate) struct LatexJob {
    pub id: Option<RequestId>,
    pub doc: DocumentId,
    pub source: PathBuf,
    pub workspace: Option<PathBuf>,
    pub settings: Latex,
    pub cancel: Cancellation,
}

/// Start the session's serialized external-build worker.
pub(crate) fn spawn(events: UnboundedSender<(Option<RequestId>, Event)>) -> mpsc::Sender<LatexJob> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("karet-latex".to_owned())
        .spawn(move || {
            while let Ok(job) = rx.recv() {
                run(job, &events);
            }
        });
    tx
}

fn run(job: LatexJob, events: &UnboundedSender<(Option<RequestId>, Event)>) {
    if job.cancel.is_cancelled() {
        return;
    }
    let root = match discover_root(&job.source) {
        Ok(root) => root,
        Err(error) => {
            emit(job, events, PathBuf::new(), None, Vec::new(), Some(error));
            return;
        },
    };
    let output_dir = output_directory(job.workspace.as_deref(), &root, &job.settings);
    let outcome = compile(&root, &output_dir, &job.settings, &job.cancel);
    if job.cancel.is_cancelled() {
        return;
    }
    match outcome {
        Ok(output) => {
            let diagnostics = parse_diagnostics(&output, &job.source);
            let expected = expected_pdf(&root, &output_dir);
            let pdf = expected.is_file().then_some(expected);
            let error = if output.success && pdf.is_some() {
                None
            } else if output.success {
                Some(format!(
                    "LaTeX command succeeded but did not produce {}",
                    expected_pdf(&root, &output_dir).display()
                ))
            } else {
                Some(last_meaningful_line(&output.text).map_or_else(
                    || "LaTeX build failed; inspect compiler diagnostics".to_owned(),
                    |line| format!("LaTeX build failed: {line}"),
                ))
            };
            emit(job, events, root, pdf, diagnostics, error);
        },
        Err(error) => emit(job, events, root, None, Vec::new(), Some(error)),
    }
}

fn emit(
    job: LatexJob,
    events: &UnboundedSender<(Option<RequestId>, Event)>,
    root: PathBuf,
    pdf: Option<PathBuf>,
    diagnostics: Vec<Diagnostic>,
    error: Option<String>,
) {
    if !job.cancel.is_cancelled() {
        let _ = events.send((
            job.id,
            Event::LatexBuildFinished {
                doc: job.doc,
                root,
                pdf,
                diagnostics,
                error,
            },
        ));
    }
}

/// Follow TeXShop/LaTeX Workshop compatible `% !TeX root = …` magic comments.
/// Chaining is allowed for included files, but cycles and excessive chains fail.
fn discover_root(source: &Path) -> Result<PathBuf, String> {
    let mut current = canonical_existing(source)?;
    let mut visited = HashSet::new();
    for _ in 0..8 {
        if !visited.insert(current.clone()) {
            return Err(format!(
                "cyclic % !TeX root directives include {}",
                current.display()
            ));
        }
        let text = std::fs::read_to_string(&current)
            .map_err(|error| format!("could not read {}: {error}", current.display()))?;
        let Some(relative) = root_directive(&text) else {
            return Ok(current);
        };
        let parent = current
            .parent()
            .ok_or_else(|| format!("{} has no parent directory", current.display()))?;
        current = canonical_existing(&parent.join(relative))?;
    }
    Err("% !TeX root chain exceeds 8 files".to_owned())
}

fn canonical_existing(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path)
        .map_err(|error| format!("could not resolve LaTeX source {}: {error}", path.display()))
}

fn root_directive(text: &str) -> Option<&str> {
    text.lines().take(40).find_map(|line| {
        let comment = line.trim_start().strip_prefix('%')?.trim_start();
        let (key, value) = comment.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("!tex root")
            .then(|| value.trim().trim_matches('"'))
            .filter(|value| !value.is_empty())
    })
}

fn output_directory(workspace: Option<&Path>, root: &Path, settings: &Latex) -> PathBuf {
    if settings.output_directory.trim().is_empty() {
        let identity = workspace.or_else(|| root.parent()).unwrap_or(root);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        identity.hash(&mut hasher);
        let project_cache = directories::ProjectDirs::from("", "getkono", "karet")
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .unwrap_or_else(std::env::temp_dir);
        return project_cache
            .join("latex")
            .join(format!("{:016x}", hasher.finish()));
    }
    let configured = PathBuf::from(&settings.output_directory);
    if configured.is_absolute() {
        configured
    } else {
        workspace
            .or_else(|| root.parent())
            .unwrap_or_else(|| Path::new("."))
            .join(configured)
    }
}

fn expected_pdf(root: &Path, output_dir: &Path) -> PathBuf {
    let name = root
        .file_stem()
        .map_or_else(|| "document.pdf".into(), |stem| stem.to_owned());
    output_dir.join(name).with_extension("pdf")
}

struct CompilerOutput {
    success: bool,
    text: String,
}

fn compile(
    root: &Path,
    output_dir: &Path,
    settings: &Latex,
    cancel: &Cancellation,
) -> Result<CompilerOutput, String> {
    std::fs::create_dir_all(output_dir).map_err(|error| {
        format!(
            "could not create LaTeX output directory {}: {error}",
            output_dir.display()
        )
    })?;
    let file_dir = root.parent().unwrap_or_else(|| Path::new("."));
    let replace = |value: &str| {
        value
            .replace("{file}", &root.to_string_lossy())
            .replace("{fileDir}", &file_dir.to_string_lossy())
            .replace("{outputDir}", &output_dir.to_string_lossy())
    };
    let mut stdout =
        tempfile::tempfile().map_err(|error| format!("could not capture LaTeX output: {error}"))?;
    let mut stderr =
        tempfile::tempfile().map_err(|error| format!("could not capture LaTeX errors: {error}"))?;
    let stdout_child = stdout
        .try_clone()
        .map_err(|error| format!("could not capture LaTeX output: {error}"))?;
    let stderr_child = stderr
        .try_clone()
        .map_err(|error| format!("could not capture LaTeX errors: {error}"))?;
    let mut child = std::process::Command::new(&settings.command)
        .args(settings.args.iter().map(|argument| replace(argument)))
        .current_dir(file_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_child))
        .stderr(Stdio::from(stderr_child))
        .spawn()
        .map_err(|error| {
            format!(
                "could not start LaTeX compiler `{}`: {error}",
                settings.command
            )
        })?;
    let deadline =
        Instant::now() + Duration::from_millis(settings.timeout_ms.clamp(1_000, 600_000));
    let status = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("LaTeX build cancelled".to_owned());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "LaTeX compiler exceeded its {} ms timeout",
                settings.timeout_ms.clamp(1_000, 600_000)
            ));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                return Err(format!("could not wait for LaTeX compiler: {error}"));
            },
        }
    };
    let mut text = read_limited(&mut stdout, "output")?;
    let errors = read_limited(&mut stderr, "errors")?;
    if !text.is_empty() && !text.ends_with('\n') && !errors.is_empty() {
        text.push('\n');
    }
    text.push_str(&errors);
    Ok(CompilerOutput {
        success: status.success(),
        text,
    })
}

fn read_limited(file: &mut std::fs::File, stream: &str) -> Result<String, String> {
    const LIMIT: u64 = 2 * 1024 * 1024;
    file.rewind()
        .map_err(|error| format!("could not seek LaTeX {stream}: {error}"))?;
    let mut bytes = Vec::new();
    file.take(LIMIT)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read LaTeX {stream}: {error}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn parse_diagnostics(output: &CompilerOutput, source: &Path) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut seen = HashSet::new();
    for line in output.text.lines() {
        let Some((prefix, message)) = line.split_once(": ") else {
            continue;
        };
        let Some((file, line_number)) = prefix.rsplit_once(':') else {
            continue;
        };
        let Ok(line_number) = line_number.parse::<u32>() else {
            continue;
        };
        if Path::new(file).file_name() != source.file_name() {
            continue;
        }
        let line_number = line_number.saturating_sub(1);
        let message = message.trim();
        if message.is_empty() || !seen.insert((file.to_owned(), line_number, message.to_owned())) {
            continue;
        }
        diagnostics.push(Diagnostic {
            range: Range {
                start: LineCol::new(line_number, 0),
                end: LineCol::new(line_number, 1),
            },
            severity: if output.success {
                Severity::Warning
            } else {
                Severity::Error
            },
            message: message.to_owned(),
            source: Some("latex".to_owned()),
            code: None,
            tags: Vec::new(),
            related: Vec::new(),
        });
    }
    diagnostics
}

fn last_meaningful_line(output: &str) -> Option<&str> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_directive_is_case_insensitive_and_limited_to_comments() {
        let text = "\\documentclass{article}\n% !TEX root = ../main.tex\n";
        assert_eq!(root_directive(text), Some("../main.tex"));
        assert_eq!(
            root_directive("% !TeX root = \"main file.tex\"\n"),
            Some("main file.tex")
        );
        assert_eq!(root_directive("!TeX root = nope.tex\n"), None);
    }

    #[test]
    fn root_discovery_follows_chains_and_rejects_cycles() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let chapter = dir.path().join("chapter.tex");
        let main = dir.path().join("main.tex");
        std::fs::write(&chapter, "% !TeX root = main.tex\nchapter")?;
        std::fs::write(&main, "\\documentclass{article}\n")?;
        assert_eq!(discover_root(&chapter)?, std::fs::canonicalize(&main)?);

        std::fs::write(&main, "% !TeX root = chapter.tex\n")?;
        assert!(discover_root(&chapter).is_err());
        Ok(())
    }

    #[test]
    fn compiler_lines_become_deduplicated_source_diagnostics() {
        let output = CompilerOutput {
            success: false,
            text: "./main.tex:12: Undefined control sequence.\n\
                   ./main.tex:12: Undefined control sequence.\n\
                   ./chapter.tex:3: Missing $ inserted.\n"
                .to_owned(),
        };
        let diagnostics = parse_diagnostics(&output, Path::new("main.tex"));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].range.start, LineCol::new(11, 0));
        assert_eq!(diagnostics[0].severity, Severity::Error);
    }

    #[test]
    fn output_path_and_placeholders_are_predictable() {
        let settings = Latex {
            output_directory: "build/latex".to_owned(),
            ..Latex::default()
        };
        let output = output_directory(
            Some(Path::new("/workspace")),
            Path::new("/workspace/book/main.tex"),
            &settings,
        );
        assert_eq!(output, PathBuf::from("/workspace/build/latex"));
        assert_eq!(
            expected_pdf(Path::new("main.tex"), &output),
            output.join("main.pdf")
        );
    }

    #[test]
    fn default_output_directory_is_outside_the_workspace() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempfile::tempdir()?;
        let output = output_directory(
            Some(workspace.path()),
            &workspace.path().join("main.tex"),
            &Latex::default(),
        );
        assert!(!output.starts_with(workspace.path()));
        assert!(output.components().any(|part| part.as_os_str() == "latex"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn compiler_receives_placeholders_as_literal_argv() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let source_dir = dir.path().join("source with spaces");
        let output_dir = dir.path().join("output with spaces");
        std::fs::create_dir_all(&source_dir)?;
        let root = source_dir.join("main.tex");
        std::fs::write(&root, "\\documentclass{article}\n")?;
        let settings = Latex {
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf '%s\\n' \"$1\" \"$2\" \"$3\"".to_owned(),
                "karet-test".to_owned(),
                "{file}".to_owned(),
                "{fileDir}".to_owned(),
                "{outputDir}".to_owned(),
            ],
            ..Latex::default()
        };
        let hub = crate::cancellation::CancellationHub::default();
        let output = compile(&root, &output_dir, &settings, &hub.register(RequestId(1)))?;

        assert!(output.success);
        let lines: Vec<&str> = output.text.lines().collect();
        assert_eq!(
            lines,
            vec![
                root.to_string_lossy(),
                source_dir.to_string_lossy(),
                output_dir.to_string_lossy()
            ]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_terminates_a_running_compiler() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("main.tex");
        let output_dir = dir.path().join("out");
        std::fs::write(&root, "\\documentclass{article}\n")?;
        let settings = Latex {
            command: "sleep".to_owned(),
            args: vec!["5".to_owned()],
            ..Latex::default()
        };
        let hub = crate::cancellation::CancellationHub::default();
        let token = hub.register(RequestId(9));
        let started = Instant::now();
        let error = std::thread::scope(|scope| {
            let build = scope.spawn(|| compile(&root, &output_dir, &settings, &token));
            std::thread::sleep(Duration::from_millis(50));
            hub.cancel(RequestId(9));
            build.join().ok().and_then(Result::err)
        });

        assert!(error.is_some_and(|error| error.contains("cancelled")));
        assert!(started.elapsed() < Duration::from_secs(2));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn successful_job_emits_the_generated_pdf_and_warnings()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("main.tex");
        let output_dir = dir.path().join("out");
        std::fs::write(&root, "\\documentclass{article}\n")?;
        let settings = Latex {
            command: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "printf '%s' '%PDF-1.4' > \"$1/main.pdf\"; printf './main.tex:2: Overfull hbox\\n'"
                    .to_owned(),
                "karet-test".to_owned(),
                "{outputDir}".to_owned(),
            ],
            output_directory: output_dir.to_string_lossy().into_owned(),
            ..Latex::default()
        };
        let hub = crate::cancellation::CancellationHub::default();
        let (events, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run(
            LatexJob {
                id: Some(RequestId(3)),
                doc: DocumentId(4),
                source: root,
                workspace: Some(dir.path().to_path_buf()),
                settings,
                cancel: hub.register(RequestId(3)),
            },
            &events,
        );

        let event = rx.try_recv().ok();
        assert!(matches!(
            event,
            Some((
                Some(RequestId(3)),
                Event::LatexBuildFinished {
                    doc: DocumentId(4),
                    pdf: Some(pdf),
                    diagnostics,
                    error: None,
                    ..
                }
            )) if pdf == output_dir.join("main.pdf") && diagnostics.len() == 1
        ));
        Ok(())
    }
}
