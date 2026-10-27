//! Terse `git log`-style relative times (`3d ago`), shared by the commit lists,
//! the commit graph and the GitHub surface.

/// A terse `git log`-style relative time (e.g. `3d ago`) for a Unix timestamp.
pub(crate) fn relative_time(secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    relative_time_at(secs, now)
}

/// [`relative_time`] as seen at `now` (seconds since the Unix epoch).
fn relative_time_at(secs: i64, now: i64) -> String {
    let delta = now - secs;
    if delta < 0 {
        return "just now".to_string();
    }
    let (n, unit) = if delta < 60 {
        (delta, "s")
    } else if delta < 3600 {
        (delta / 60, "m")
    } else if delta < 86_400 {
        (delta / 3600, "h")
    } else if delta < 86_400 * 7 {
        (delta / 86_400, "d")
    } else if delta < 86_400 * 30 {
        (delta / (86_400 * 7), "w")
    } else if delta < 86_400 * 365 {
        (delta / (86_400 * 30), "mo")
    } else {
        (delta / (86_400 * 365), "y")
    };
    format!("{n}{unit} ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_time_uses_compact_git_style_units() {
        let now = 40_000_000;
        assert_eq!(relative_time_at(now + 1, now), "just now");
        assert_eq!(relative_time_at(now - 42, now), "42s ago");
        assert_eq!(relative_time_at(now - 120, now), "2m ago");
        assert_eq!(relative_time_at(now - 10_800, now), "3h ago");
        assert_eq!(relative_time_at(now - 172_800, now), "2d ago");
        assert_eq!(relative_time_at(now - 1_209_600, now), "2w ago");
        assert_eq!(relative_time_at(now - 5_184_000, now), "2mo ago");
        assert_eq!(relative_time_at(now - 63_072_000, now), "2y ago");
    }
}
