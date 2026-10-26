//! The visualization lenses the session serves: today the workspace's
//! package-dependency graph.

use super::*;

impl Session {
    /// Build the workspace package-dependency graph and emit it, or surface a failure
    /// (no lockfile / parse error) as a notification.
    #[cfg(feature = "viz")]
    pub(super) fn emit_dependency_graph(&mut self, id: RequestId) {
        let Some(root) = self.config.roots.first() else {
            self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::System,
                    message: "dependency graph: no workspace root is open".to_string(),
                },
            );
            return;
        };
        match crate::viz::dependency_graph(root) {
            Ok(view) => {
                let title = root
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string();
                self.emit(
                    Some(id),
                    Event::GraphReady {
                        kind: crate::api::GraphKind::Dependency,
                        title,
                        view,
                    },
                );
            },
            Err(e) => self.emit(
                Some(id),
                Event::Notification {
                    severity: Severity::Error,
                    kind: NotificationKind::System,
                    message: format!("dependency graph: {e}"),
                },
            ),
        }
    }

    /// Without the `viz` feature the dependency-graph lens is absent; say so
    /// instead of silently ignoring the request.
    #[cfg(not(feature = "viz"))]
    pub(super) fn emit_dependency_graph(&mut self, id: RequestId) {
        self.emit(
            Some(id),
            Event::Notification {
                severity: Severity::Error,
                kind: karet_core::NotificationKind::System,
                message: "dependency graph support is not compiled in (feature `viz`)".to_string(),
            },
        );
    }
}
