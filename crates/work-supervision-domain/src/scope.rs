//! Path scopes of missions (`docs/work-supervision/coordination-v0.md`).
//!
//! A scope is a list of repository-relative path prefixes. A prefix covers
//! itself and everything under it, segment by segment: `src/a` covers
//! `src/a/b.rs` but not `src/ab.rs`. A mission without a declared scope covers
//! its whole repository.

use crate::coordination::CoordinationRefusal;

/// Most prefixes in one scope.
pub const MAX_SCOPE_PATHS: usize = 32;
/// Longest prefix, in bytes.
pub const MAX_SCOPE_PATH_BYTES: usize = 256;

/// One validated prefix: `/`-separated segments, none empty, `.` or `..`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopePath(String);

impl ScopePath {
    /// Validates a prefix. A trailing `/` is accepted and dropped.
    ///
    /// # Errors
    ///
    /// [`CoordinationRefusal::FieldInvalid`] (`scope`) for an empty or too long
    /// prefix, a leading `/`, an empty, `.` or `..` segment, a backslash or a
    /// control character.
    pub fn parse(text: &str) -> Result<Self, CoordinationRefusal> {
        let invalid = CoordinationRefusal::FieldInvalid { field: "scope" };
        let trimmed = text.strip_suffix('/').unwrap_or(text);
        if trimmed.is_empty()
            || trimmed.len() > MAX_SCOPE_PATH_BYTES
            || trimmed.starts_with('/')
            || trimmed.chars().any(|c| c.is_control() || c == '\\')
        {
            return Err(invalid);
        }
        if trimmed
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        {
            return Err(invalid);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The prefix.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this prefix covers `path` (itself, or a path under it).
    #[must_use]
    pub fn covers(&self, path: &str) -> bool {
        path == self.0
            || path
                .strip_prefix(self.0.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// The declared scope of a mission; `None` stands for the whole repository.
pub type Scope = Option<Vec<ScopePath>>;

/// Validates a whole scope declaration: 1 to [`MAX_SCOPE_PATHS`] prefixes,
/// returned sorted and without duplicates.
///
/// # Errors
///
/// [`CoordinationRefusal::FieldInvalid`] (`scope`).
pub fn parse_scope<S: AsRef<str>>(paths: &[S]) -> Result<Vec<ScopePath>, CoordinationRefusal> {
    if paths.is_empty() || paths.len() > MAX_SCOPE_PATHS {
        return Err(CoordinationRefusal::FieldInvalid { field: "scope" });
    }
    let mut parsed = paths
        .iter()
        .map(|path| ScopePath::parse(path.as_ref()))
        .collect::<Result<Vec<_>, _>>()?;
    parsed.sort();
    parsed.dedup();
    Ok(parsed)
}

/// Whether two scopes overlap: one prefix of either covers a prefix of the
/// other. An undeclared scope overlaps every scope.
#[must_use]
pub fn overlaps(left: &Scope, right: &Scope) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.iter().any(|a| {
            right
                .iter()
                .any(|b| a.covers(b.as_str()) || b.covers(a.as_str()))
        }),
        _ => true,
    }
}

/// The paths of `changed` that no prefix of `scope` covers, in their order;
/// none when the scope is undeclared.
#[must_use]
pub fn outside<'a>(scope: &Scope, changed: &'a [String]) -> Vec<&'a str> {
    let Some(prefixes) = scope else {
        return Vec::new();
    };
    changed
        .iter()
        .map(String::as_str)
        .filter(|path| !prefixes.iter().any(|prefix| prefix.covers(path)))
        .collect()
}
