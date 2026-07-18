    use karet_vcs::StatusKind;

    use super::*;
    use crate::keymap::SidebarPanel;

    fn change(path: &str, status: StatusKind) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            old_path: None,
            status,
            is_binary: false,
            old: String::new(),
            new: "x\n".to_string(),
        }
    }

    fn app() -> App {
        App::new(
            PathBuf::from("."),
            vec![change("a.rs", StatusKind::Modified)],
            vec![change("b.rs", StatusKind::Modified)],
            false,
        )
    }

    fn test_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir =
            std::env::temp_dir().join(format!("karet-{name}-{}-{unique}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn write_file(root: &Path, rel: &str, contents: &[u8]) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, contents);
    }

    fn select_explorer_path(app: &mut App, path: &Path) {
        app.explorer.ensure_built(&app.root);
        let Some(idx) = app.explorer.rows().iter().position(|row| row.path == path) else {
            panic!("missing explorer path {}", path.display());
        };
        app.explorer.select_visible(idx);
    }

    fn refresh_count(backend: &RecordingBackend) -> usize {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter(|(_, command)| matches!(command, SessionCommand::RefreshVcs))
                    .count()
            })
            .unwrap_or_default()
    }

    fn retarget_commands(backend: &RecordingBackend) -> Vec<(DocumentId, PathBuf)> {
        backend
            .sent
            .lock()
            .map(|sent| {
                sent.iter()
                    .filter_map(|(_, command)| match command {
                        SessionCommand::RetargetDocument { doc, path } => {
                            Some((*doc, path.clone()))
                        },
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    struct RecordingBackend {
        next: std::sync::atomic::AtomicU64,
        sent: std::sync::Mutex<Vec<(RequestId, SessionCommand)>>,
    }

    impl RecordingBackend {
        fn new() -> Self {
            Self {
                next: std::sync::atomic::AtomicU64::new(1),
                sent: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl Backend for RecordingBackend {
        fn send(&self, id: RequestId, command: SessionCommand) -> Result<(), BackendError> {
            if let Ok(mut sent) = self.sent.lock() {
                sent.push((id, command));
            }
            Ok(())
        }

        fn next_id(&self) -> RequestId {
            RequestId(self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
        }
    }

