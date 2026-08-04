//! Neutral models for current-buffer source attribution.

/// Commit metadata shown for an attributed line.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BlameCommit {
    /// Full hexadecimal commit id.
    pub hash: String,
    /// Commit author's display name.
    pub author: String,
    /// Commit author time in seconds since the Unix epoch.
    pub author_time: i64,
}

/// Attribution of a current-buffer line.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn commit() -> BlameCommit {
        BlameCommit {
            hash: "1234567890".to_string(),
            author: "Ada".to_string(),
            author_time: 1_768_435_200,
        }
    }

    #[cfg(feature = "serde")]
    #[test]
    fn models_round_trip_through_serde() -> Result<(), serde_json::Error> {
        let attribution = BlameAttribution::Commit(commit());
        let json = serde_json::to_string(&attribution)?;
        assert_eq!(
            serde_json::from_str::<BlameAttribution>(&json)?,
            attribution
        );
        Ok(())
    }
}
