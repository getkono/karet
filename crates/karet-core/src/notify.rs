//! Neutral notification model — the "currency" for user-facing messages.
//!
//! Producers and the backend emit a [`Notification`] describing something the user
//! should see (an error, a completed action, an out-of-band condition); the
//! application's notification center owns their lifetime and a renderer (the
//! `Toasts` widget in `karet-widgets`) draws them. Like the other models here, a
//! notification is clock-free: it carries a *relative* [`timeout`](Notification::timeout),
//! never an absolute timestamp, so it stays cheap to serialize and easy to test.

use std::time::Duration;

use crate::model::Severity;
use crate::token::ThemeRole;

/// A monotonic identifier for a notification, assigned by the center on insertion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NotificationId(pub u64);

/// The subsystem a notification originated from — used for an icon/prefix, grouping,
/// and de-duplication.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum NotificationKind {
    /// File input/output (open, save, external change).
    Io,
    /// Version control (status, staging, commit, blame).
    Vcs,
    /// Language server.
    Lsp,
    /// Search / replace.
    Search,
    /// General application / backend condition.
    System,
}

/// A user-facing message. Neutral and clock-free: the `timeout` is a lifetime
/// *relative* to when the center accepts it, so the same value serializes across the
/// client-server seam and drives testable expiry.
///
/// Constructed with literal syntax by the application, so this struct is
/// deliberately not `#[non_exhaustive]`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Notification {
    /// Identity, assigned by the center (`NotificationId(0)` until then).
    pub id: NotificationId,
    /// How prominently to render it, and which color role to use.
    pub severity: Severity,
    /// The originating subsystem.
    pub kind: NotificationKind,
    /// A short, single-line headline.
    pub title: String,
    /// An optional longer body shown beneath the title.
    pub body: Option<String>,
    /// A de-duplication / update key: a new notification with the same tag replaces
    /// the active one instead of stacking (e.g. `"vcs.status"`).
    pub tag: Option<String>,
    /// How long the notification stays before auto-expiring. `None` means it is
    /// persistent and clears only when dismissed.
    pub timeout: Option<Duration>,
    /// Whether the user may dismiss it directly (click / key).
    pub dismissable: bool,
}

/// The UI-chrome color [`ThemeRole`] a notification of the given severity should use.
///
/// Reuses the diagnostic palette: [`Severity::Hint`] maps to the (teal) hint role,
/// which reads as "success".
#[must_use]
pub fn severity_role(severity: Severity) -> ThemeRole {
    match severity {
        Severity::Error => ThemeRole::DiagnosticError,
        Severity::Warning => ThemeRole::DiagnosticWarning,
        Severity::Information => ThemeRole::DiagnosticInfo,
        Severity::Hint => ThemeRole::DiagnosticHint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_maps_to_distinct_roles() {
        assert_eq!(severity_role(Severity::Error), ThemeRole::DiagnosticError);
        assert_eq!(
            severity_role(Severity::Warning),
            ThemeRole::DiagnosticWarning
        );
        assert_eq!(
            severity_role(Severity::Information),
            ThemeRole::DiagnosticInfo
        );
        assert_eq!(severity_role(Severity::Hint), ThemeRole::DiagnosticHint);
    }

    #[test]
    fn notification_builds_with_literal_syntax() {
        let note = Notification {
            id: NotificationId(0),
            severity: Severity::Error,
            kind: NotificationKind::Io,
            title: "save failed".to_string(),
            body: Some("permission denied".to_string()),
            tag: None,
            timeout: None,
            dismissable: true,
        };
        assert_eq!(note.id, NotificationId(0));
        assert!(note.timeout.is_none());
        assert_eq!(note.kind, NotificationKind::Io);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn notification_types_derive_serde() {
        // Compile-time proof the model carries the wire derives under `serde`,
        // without pulling a serialization backend into this dependency-light crate.
        fn assert_serde<T: serde::Serialize + serde::de::DeserializeOwned>() {}
        assert_serde::<Notification>();
        assert_serde::<NotificationId>();
        assert_serde::<NotificationKind>();
    }
}
