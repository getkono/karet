//! Persistent application tracing.

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use color_eyre::eyre::OptionExt;
use color_eyre::eyre::eyre;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_appender::rolling::RollingFileAppender;
use tracing_subscriber::EnvFilter;

const RETAINED_LOG_FILES: usize = 7;

fn directory() -> color_eyre::Result<PathBuf> {
    let directories = directories::ProjectDirs::from("", "getkono", "karet")
        .ok_or_eyre("platform application directories are unavailable")?;
    let state = directories
        .state_dir()
        .unwrap_or_else(|| directories.data_local_dir());
    Ok(state.join("logs"))
}

/// Initialize daily application logs and return the guard that flushes them.
pub(crate) fn init() -> color_eyre::Result<WorkerGuard> {
    init_in(&directory()?)
}

/// Print every existing application log path, or a blue informational message
/// when the log directory contains none.
pub(crate) fn report_paths() -> color_eyre::Result<()> {
    report_paths_in(
        &directory()?,
        &mut std::io::stdout().lock(),
        &mut std::io::stderr().lock(),
    )
}

fn report_paths_in(
    directory: &Path,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> color_eyre::Result<()> {
    let mut paths = match std::fs::read_dir(directory) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name == "karet.log" || name.starts_with("karet.log.")
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    paths.sort();
    if paths.is_empty() {
        writeln!(
            stderr,
            "\u{1b}[34minfo:\u{1b}[0m no karet log files exist in {}",
            directory.display()
        )?;
    } else {
        for path in paths {
            writeln!(stdout, "{}", path.display())?;
        }
    }
    Ok(())
}

fn init_in(directory: &Path) -> color_eyre::Result<WorkerGuard> {
    std::fs::create_dir_all(directory)?;
    let appender = build_appender(directory)?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(filter)
        .with_writer(writer)
        .try_init()
        .map_err(|error| eyre!("install tracing subscriber: {error}"))?;
    Ok(guard)
}

fn build_appender(directory: &Path) -> color_eyre::Result<RollingFileAppender> {
    Ok(rolling::Builder::new()
        .rotation(rolling::Rotation::DAILY)
        .filename_prefix("karet.log")
        .max_log_files(RETAINED_LOG_FILES)
        .build(directory)?)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn report_paths_prints_existing_logs_in_order() -> color_eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let older = directory.path().join("karet.log.2026-07-29");
        let newer = directory.path().join("karet.log.2026-07-30");
        std::fs::write(&newer, "new")?;
        std::fs::write(&older, "old")?;
        std::fs::write(directory.path().join("unrelated.txt"), "ignore")?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        report_paths_in(directory.path(), &mut stdout, &mut stderr)?;

        assert_eq!(
            String::from_utf8(stdout)?,
            format!("{}\n{}\n", older.display(), newer.display())
        );
        assert!(stderr.is_empty());
        Ok(())
    }

    #[test]
    fn report_paths_prints_blue_info_when_no_log_exists() -> color_eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        report_paths_in(directory.path(), &mut stdout, &mut stderr)?;

        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr)?;
        assert!(stderr.contains("\u{1b}[34minfo:\u{1b}[0m"));
        assert!(stderr.contains("no karet log files exist"));
        assert!(stderr.contains(&directory.path().display().to_string()));
        Ok(())
    }

    #[test]
    fn rolling_appender_writes_to_the_requested_log_directory() -> color_eyre::Result<()> {
        let directory = tempfile::tempdir()?;
        let mut appender = build_appender(directory.path())?;
        writeln!(appender, "persisted warning")?;
        appender.flush()?;
        drop(appender);

        let contents = std::fs::read_dir(directory.path())?
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .collect::<String>();
        assert!(contents.contains("persisted warning"));
        Ok(())
    }
}
