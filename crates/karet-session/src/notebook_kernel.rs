//! Notebook kernel orchestration (feature `notebook-kernel`): one kernel per
//! notebook path, driven by a dedicated task with a one-in-flight execute
//! queue. Outputs land back in the in-memory notebook model and the preview
//! is re-rendered after every cell (a fresh `DocumentConverted`), so the
//! read-only viewer doubles as the run surface until the cell-native tab
//! lands.

use std::path::Path;
use std::path::PathBuf;

use karet_core::NotificationKind;
use karet_core::Severity;
use karet_notebook::CellKind;
use karet_notebook::Notebook;
use karet_notebook::kernel;
use karet_notebook::kernel::KernelClient;
use karet_notebook::kernel::ZmqTransport;
use tokio::sync::mpsc;

use crate::api::Event;
use crate::api::RequestId;

/// The sender the manager answers on.
type Events = mpsc::UnboundedSender<(Option<RequestId>, Event)>;

/// How long a booting kernel may take before starting is called a failure.
const KERNEL_READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// One job for the kernel task's queue (one cell in flight at a time).
enum Job {
    /// Run every code cell, stopping at the first error.
    RunAll,
    /// Run one cell by index.
    RunCell(usize),
    /// Restart the kernel and mark every cell stale.
    Restart,
}

/// The live kernel session (one at a time; a run on another notebook
/// replaces it).
struct Active {
    path: PathBuf,
    jobs: mpsc::UnboundedSender<Job>,
    connection: kernel::ConnectionInfo,
}

/// Notebook-kernel orchestration owned by the session actor.
pub(crate) struct NotebookKernels {
    supervisor: Option<PathBuf>,
    active: Option<Active>,
    events: Events,
    /// Kernelspec directories override for tests (`None` = the standard set).
    #[cfg(test)]
    spec_dirs: Option<Vec<PathBuf>>,
}

impl NotebookKernels {
    /// Create a manager answering on `events`.
    pub(crate) fn new(supervisor: Option<PathBuf>, events: Events) -> Self {
        Self {
            supervisor,
            active: None,
            events,
            #[cfg(test)]
            spec_dirs: None,
        }
    }

    /// Run all cells (`cell: None`) or one cell of `path`, starting or
    /// switching the kernel as needed.
    pub(crate) fn run(&mut self, path: &Path, cell: Option<usize>) {
        if let Some(active) = self.ensure(path) {
            let _ = active.jobs.send(cell.map_or(Job::RunAll, Job::RunCell));
        }
    }

    /// Ensure a kernel is warming for `path` (the `autoStart` open hook);
    /// queues nothing.
    pub(crate) fn warm(&mut self, path: &Path) {
        let _ = self.ensure(path);
    }

    /// The active kernel for `path`, starting one (empty queue) if needed —
    /// a run on a *different* notebook replaces the previous kernel, whose
    /// task shuts down when its queue closes.
    fn ensure(&mut self, path: &Path) -> Option<&Active> {
        let alive = self
            .active
            .as_ref()
            .is_some_and(|active| active.path == path && !active.jobs.is_closed());
        if !alive {
            self.active = None;
            let Ok(connection) = kernel::local_connection() else {
                self.notify("no loopback port for a kernel connection");
                return None;
            };
            let (jobs_tx, jobs_rx) = mpsc::unbounded_channel();
            let task = KernelTask {
                path: path.to_path_buf(),
                supervisor: self.supervisor.clone(),
                connection: connection.clone(),
                spec_dirs: self.spec_dirs_or_default(),
                events: self.events.clone(),
            };
            tokio::spawn(kernel_task(task, jobs_rx));
            self.active = Some(Active {
                path: path.to_path_buf(),
                jobs: jobs_tx,
                connection,
            });
        }
        self.active.as_ref()
    }

    /// Interrupt the running cell, out of band on the control channel.
    pub(crate) fn interrupt(&self) {
        let Some(active) = &self.active else {
            self.notify("no notebook kernel to interrupt");
            return;
        };
        let connection = active.connection.clone();
        let path = active.path.clone();
        let events = self.events.clone();
        tokio::spawn(async move {
            let outcome = async {
                let mut control = kernel::KernelControl::connect(&connection).await?;
                control.interrupt().await
            }
            .await;
            let text = match outcome {
                Ok(()) => "interrupted".to_owned(),
                Err(error) => format!("interrupt failed: {error}"),
            };
            let _ = events.send((None, Event::NotebookKernelStatus { path, text }));
        });
    }

    /// Restart the kernel of the active notebook, marking cells stale.
    pub(crate) fn restart(&self) {
        let Some(active) = &self.active else {
            self.notify("no notebook kernel to restart");
            return;
        };
        let _ = active.jobs.send(Job::Restart);
    }

    fn notify(&self, message: &str) {
        let _ = self.events.send((
            None,
            Event::Notification {
                severity: Severity::Information,
                kind: NotificationKind::System,
                message: message.to_owned(),
            },
        ));
    }

    fn spec_dirs_or_default(&self) -> Vec<PathBuf> {
        #[cfg(test)]
        if let Some(dirs) = &self.spec_dirs {
            return dirs.clone();
        }
        kernel::default_dirs()
    }
}

/// Everything the kernel task needs, bundled.
struct KernelTask {
    path: PathBuf,
    supervisor: Option<PathBuf>,
    connection: kernel::ConnectionInfo,
    spec_dirs: Vec<PathBuf>,
    events: Events,
}

/// The per-kernel task: boot, then drain the job queue sequentially.
async fn kernel_task(ctx: KernelTask, mut jobs: mpsc::UnboundedReceiver<Job>) {
    let status = |text: &str| {
        let _ = ctx.events.send((
            None,
            Event::NotebookKernelStatus {
                path: ctx.path.clone(),
                text: text.to_owned(),
            },
        ));
    };
    let notify = |message: String| {
        let _ = ctx.events.send((
            None,
            Event::Notification {
                severity: Severity::Warning,
                kind: NotificationKind::System,
                message,
            },
        ));
    };

    let mut notebook = match std::fs::read_to_string(&ctx.path)
        .map_err(|error| error.to_string())
        .and_then(|text| karet_notebook::parse(&text).map_err(|error| error.to_string()))
    {
        Ok(notebook) => notebook,
        Err(error) => {
            notify(format!("notebook: {error}"));
            return;
        },
    };

    let specs = kernel::discover_in(&ctx.spec_dirs);
    let requested = notebook
        .metadata
        .get("kernelspec")
        .and_then(|spec| spec.get("name"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let language = notebook.language().to_owned();
    let Some(spec) = kernel::find(&specs, &requested, &language) else {
        notify(format!(
            "no installed kernel for {language} notebooks (install one, e.g. `pip install \
             ipykernel`, or check `jupyter kernelspec list`)"
        ));
        return;
    };

    status(&format!("starting {}", spec.name));
    let Some((mut client, _child, connection_file)) =
        boot(&ctx, spec, &ctx.connection, &notify).await
    else {
        status("kernel failed to start");
        return;
    };
    status("ready");

    while let Some(job) = jobs.recv().await {
        match job {
            Job::RunAll => {
                run_cells(&mut client, &mut notebook, None, &ctx).await;
            },
            Job::RunCell(index) => {
                run_cells(&mut client, &mut notebook, Some(index), &ctx).await;
            },
            Job::Restart => {
                let _ = client.shutdown().await;
                mark_stale(&mut notebook);
                rerender(&ctx, &notebook);
                status("restarting");
                let Some((fresh, child, file)) = boot(&ctx, spec, &ctx.connection, &notify).await
                else {
                    status("kernel failed to restart");
                    return;
                };
                client = fresh;
                let _ = (child, file);
                status("ready");
            },
        }
    }
    let _ = client.shutdown().await;
    let _ = std::fs::remove_file(connection_file);
}

/// Spawn the kernel process (through the supervisor when available) and wait
/// for readiness.
async fn boot(
    ctx: &KernelTask,
    spec: &kernel::KernelSpec,
    connection: &kernel::ConnectionInfo,
    notify: &impl Fn(String),
) -> Option<(KernelClient<ZmqTransport>, tokio::process::Child, PathBuf)> {
    let connection_file = match kernel::write_connection_file(connection) {
        Ok(path) => path,
        Err(error) => {
            notify(format!(
                "notebook: could not write the connection file: {error}"
            ));
            return None;
        },
    };
    let argv = kernel::substitute_argv(&spec.argv, &connection_file.to_string_lossy());
    let (program, args) = argv.split_first()?;
    let mut command = match &ctx.supervisor {
        Some(supervisor) => {
            match karet_supervisor::supervisor::command(
                supervisor,
                program.clone(),
                args.to_vec(),
                ctx.path.parent().unwrap_or(Path::new(".")),
            ) {
                Ok(command) => command,
                Err(error) => {
                    notify(format!("notebook: supervisor launch failed: {error}"));
                    return None;
                },
            }
        },
        None => {
            let mut command = tokio::process::Command::new(program);
            command
                .args(args)
                .current_dir(ctx.path.parent().unwrap_or(Path::new(".")));
            command
        },
    };
    let child = match command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            notify(format!("notebook: could not start {}: {error}", spec.name));
            return None;
        },
    };
    let transport = match ZmqTransport::connect(connection).await {
        Ok(transport) => transport,
        Err(error) => {
            notify(format!("notebook: {error}"));
            return None;
        },
    };
    let mut client = KernelClient::new(transport);
    if let Err(error) = client.wait_ready(KERNEL_READY_TIMEOUT).await {
        notify(format!("notebook: the kernel never became ready: {error}"));
        return None;
    }
    Some((client, child, connection_file))
}

/// Run one cell (`only`) or every code cell, stopping at the first error.
async fn run_cells(
    client: &mut KernelClient<ZmqTransport>,
    notebook: &mut Notebook,
    only: Option<usize>,
    ctx: &KernelTask,
) {
    let indices: Vec<usize> = notebook
        .cells
        .iter()
        .enumerate()
        .filter(|(index, cell)| {
            cell.kind == CellKind::Code && only.is_none_or(|wanted| wanted == *index)
        })
        .map(|(index, _)| index)
        .collect();
    let total = indices.len();
    for (nth, index) in indices.into_iter().enumerate() {
        let _ = ctx.events.send((
            None,
            Event::NotebookKernelStatus {
                path: ctx.path.clone(),
                text: format!("running cell {}/{total}", nth + 1),
            },
        ));
        let source = notebook.cells[index].source.text();
        let outcome = match client.execute(&source).await {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = ctx.events.send((
                    None,
                    Event::NotebookKernelStatus {
                        path: ctx.path.clone(),
                        text: format!("cell failed: {error}"),
                    },
                ));
                return;
            },
        };
        let cell = &mut notebook.cells[index];
        cell.execution_count = Some(outcome.execution_count);
        cell.outputs = Some(outcome.outputs.clone());
        rerender(ctx, notebook);
        let errored = outcome.errored;
        let _ = ctx.events.send((
            None,
            Event::NotebookCellDone {
                path: ctx.path.clone(),
                cell: index,
                errored,
            },
        ));
        if errored {
            let _ = ctx.events.send((
                None,
                Event::NotebookKernelStatus {
                    path: ctx.path.clone(),
                    text: format!("stopped at cell {} (error)", nth + 1),
                },
            ));
            return;
        }
    }
    let _ = ctx.events.send((
        None,
        Event::NotebookKernelStatus {
            path: ctx.path.clone(),
            text: "idle".to_owned(),
        },
    ));
}

/// Clear every code cell's outputs and counter (a restarted kernel makes
/// them stale).
fn mark_stale(notebook: &mut Notebook) {
    for cell in &mut notebook.cells {
        if cell.kind == CellKind::Code {
            cell.execution_count = Some(None);
            cell.outputs = Some(Vec::new());
        }
    }
}

/// Push a refreshed preview for the in-memory notebook.
fn rerender(ctx: &KernelTask, notebook: &Notebook) {
    let _ = ctx.events.send((
        None,
        Event::DocumentConverted {
            path: ctx.path.clone(),
            markdown: Ok(karet_notebook::to_markdown(notebook)),
        },
    ));
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn run_without_an_installed_kernel_diagnoses() {
        let dir = std::env::temp_dir().join(format!("karet-nbk-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("empty.ipynb");
        let _ = std::fs::write(
            &path,
            r#"{"nbformat": 4, "nbformat_minor": 5, "metadata": {},
               "cells": [{"cell_type": "code", "metadata": {}, "execution_count": null,
                          "source": "1", "outputs": []}]}"#,
        );
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let mut kernels = NotebookKernels::new(None, events_tx);
        kernels.spec_dirs = Some(vec![dir.join("no-kernels-here")]);
        kernels.run(&path, None);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let message = loop {
            let Ok(Some((_, event))) = tokio::time::timeout_at(deadline, events_rx.recv()).await
            else {
                break String::new();
            };
            if let Event::Notification { message, .. } = event {
                break message;
            }
        };
        assert!(message.contains("no installed kernel"), "{message}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_broken_notebook_diagnoses_the_parse() {
        let dir = std::env::temp_dir().join(format!("karet-nbk-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("bad.ipynb");
        let _ = std::fs::write(&path, "not json");
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let mut kernels = NotebookKernels::new(None, events_tx);
        kernels.spec_dirs = Some(Vec::new());
        kernels.run(&path, None);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let message = loop {
            let Ok(Some((_, event))) = tokio::time::timeout_at(deadline, events_rx.recv()).await
            else {
                break String::new();
            };
            if let Event::Notification { message, .. } = event {
                break message;
            }
        };
        assert!(message.contains("notebook:"), "{message}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn interrupt_without_a_kernel_notifies() {
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let kernels = NotebookKernels::new(None, events_tx);
        kernels.interrupt();
        let got = events_rx.recv().await.map(|(_, event)| event);
        assert!(
            matches!(got, Some(Event::Notification { ref message, .. })
                if message.contains("no notebook kernel")),
            "{got:?}"
        );
    }
}
