use anyhow::{bail, Result};
use regex::Regex;

/// A single match condition applied to a file path.
enum FilterPattern {
    Substring(String),
    Regex(Regex),
}

/// A file-path filter built from one or more conditions.
///
/// Call [`FileFilter::matches`] with a path; backslashes are normalised to
/// forward slashes before comparison.  A path matches when **any** pattern
/// matches (logical OR).  When `invert` is set the result is flipped.
pub struct FileFilter {
    patterns: Vec<FilterPattern>,
    invert: bool,
}

impl FileFilter {
    /// Build a filter from a colon-separated list of literal substrings.
    ///
    /// Returns an error if any condition is empty.
    pub fn from_substrings(raw: &str, invert: bool) -> Result<Self> {
        let parts: Vec<&str> = raw.split(':').collect();
        for part in &parts {
            if part.is_empty() {
                bail!("filter condition is invalid: empty condition in '{raw}'");
            }
        }
        Ok(Self {
            patterns: parts
                .into_iter()
                .map(|s| FilterPattern::Substring(s.to_owned()))
                .collect(),
            invert,
        })
    }

    /// Build a filter from a colon-separated list of regex patterns.
    ///
    /// Returns an error if any pattern is empty or not a valid regular expression.
    pub fn from_regexes(raw: &str, invert: bool) -> Result<Self> {
        let parts: Vec<&str> = raw.split(':').collect();
        let mut patterns = Vec::with_capacity(parts.len());
        for part in &parts {
            if part.is_empty() {
                bail!("filter-regex condition is invalid: empty pattern in '{raw}'");
            }
            let re = Regex::new(part)
                .map_err(|e| anyhow::anyhow!("invalid regex pattern '{}': {}", part, e))?;
            patterns.push(FilterPattern::Regex(re));
        }
        Ok(Self { patterns, invert })
    }

    /// Return `true` if `path` matches this filter.
    ///
    /// Backslashes in `path` are converted to forward slashes before matching.
    pub fn matches(&self, path: &str) -> bool {
        let normalized = path.replace('\\', "/");
        let any_match = self.patterns.iter().any(|p| match p {
            FilterPattern::Substring(s) => normalized.contains(s.as_str()),
            FilterPattern::Regex(r) => r.is_match(&normalized),
        });
        if self.invert { !any_match } else { any_match }
    }
}
