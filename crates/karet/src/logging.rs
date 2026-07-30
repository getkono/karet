//! Persistent application tracing.

use std::path::Path;

use color_eyre::eyre::OptionExt;
use color_eyre::eyre::eyre;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_appender::rolling::RollingFileAppender;
use tracing_subscriber::EnvFilter;

const RETAINED_LOG_FILES: usize = 7;

/// Initialize daily application logs and return the guard that flushes them.
pub(crate) fn init() -> color_eyre::Result<WorkerGuard> {
    let directories = directories::ProjectDirs::from("", "getkono", "karet")
        .ok_or_eyre("platform application directories are unavailable")?;
    let state = directories
        .state_dir()
        .unwrap_or_else(|| directories.data_local_dir());
    init_in(&state.join("logs"))
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
