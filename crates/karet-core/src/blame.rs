//! Neutral models for current-buffer source attribution.

/// How blame should scope attribution around the current line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[non_exhaustive]
pub enum BlameMode {
    /// Attribute only the current line.
    #[default]
    Line,
    /// Attribute every contributing group in the enclosing semantic block.
    Semantic,
}

/// An inclusive, zero-based range of buffer lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BlameLineRange {
    /// First line in the range.
    pub start: u32,
    /// Last line in the range.
    pub end: u32,
}

impl BlameLineRange {
    /// Whether `line` falls inside the range.
    #[must_use]
    pub const fn contains(self, line: u32) -> bool {
        self.start <= line && line <= self.end
    }
}

/// Commit metadata shown for an attributed line or semantic group.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BlameCommit {
    /// Full hexadecimal commit id.
    pub hash: String,
    /// Commit author's display name.
    pub author: String,
    /// Commit author date in ISO-8601 form when available.
    pub date: String,
    /// Full commit message.
    pub message: String,
}

impl BlameCommit {
    /// The abbreviated seven-character id used in compact displays.
    #[must_use]
    pub fn short_hash(&self) -> &str {
        &self.hash[..self.hash.len().min(7)]
    }

    /// The first line of the commit message.
    #[must_use]
    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }
}

/// Attribution of a current-buffer line range.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[non_exhaustive]
pub enum BlameAttribution {
    /// The range is unchanged from, or uniquely mapped to, this commit.
    Commit(BlameCommit),
    /// The range is new, changed, or cannot be mapped without ambiguity.
    Uncommitted,
}

/// One consecutive current-buffer range with a shared attribution.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BlameHunk {
    /// Current-buffer line range covered by the hunk.
    pub lines: BlameLineRange,
    /// Commit or uncommitted attribution for the range.
    pub attribution: BlameAttribution,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit() -> BlameCommit {
        BlameCommit {
            hash: "1234567890".to_string(),
            author: "Ada".to_string(),
            date: "2026-07-20".to_string(),
            message: "subject\n\nbody".to_string(),
        }
    }

    #[test]
    fn line_range_contains_its_inclusive_edges() {
        let range = BlameLineRange { start: 2, end: 4 };
        assert!(range.contains(2));
        assert!(range.contains(4));
        assert!(!range.contains(5));
    }

    #[test]
    fn commit_has_compact_display_helpers() {
        let commit = commit();
        assert_eq!(commit.short_hash(), "1234567");
        assert_eq!(commit.summary(), "subject");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn models_round_trip_through_serde() -> Result<(), serde_json::Error> {
        let hunk = BlameHunk {
            lines: BlameLineRange { start: 1, end: 3 },
            attribution: BlameAttribution::Commit(commit()),
        };
        let json = serde_json::to_string(&hunk)?;
        assert_eq!(serde_json::from_str::<BlameHunk>(&json)?, hunk);
        Ok(())
    }
}
