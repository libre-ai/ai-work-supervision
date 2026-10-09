//! The private configuration of a root, `config.toml`.
//!
//! It lives in the root, never in a repository. Only a strict subset of TOML
//! is read — comments, `[section]` headers and `key = "string"` or
//! `key = integer` lines in the sections below; anything else is refused with
//! its line number (`config.invalid`), so a typo never silently changes a
//! setting:
//!
//! ```toml
//! [repositories]
//! sample = "/absolute/path/to/a/repository"
//!
//! [executor]
//! profile = "fake"                       # the only profile before C0
//! fake_agent = "/absolute/path/to/ws-fake-agent"
//!
//! [runs]
//! path = "/usr/local/bin:/usr/bin:/bin"  # PATH given to runs
//! idle_after_ms = 2000                   # silence before waiting-input
//! grace_ms = 2000                        # SIGTERM → SIGKILL delay
//!
//! [anchor]
//! path = "/absolute/path/outside/the/root/anchors.v0"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use work_supervision_domain::ExecutorProfile;

use crate::Failure;

/// A parsed configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    repositories: BTreeMap<String, PathBuf>,
    executor: ExecutorProfile,
    fake_agent: Option<PathBuf>,
    path: String,
    idle_after_ms: u64,
    grace_ms: u64,
    anchor: Option<PathBuf>,
}

const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

impl Config {
    /// Reads `config.toml` from `path`.
    ///
    /// # Errors
    ///
    /// `config.unreadable`, `config.invalid`, `config.repository_invalid` and,
    /// for a profile other than `fake`, `agent.real_forbidden_until_c0`.
    pub fn load(path: &Path) -> Result<Self, Failure> {
        let text = std::fs::read_to_string(path).map_err(|_| Failure::new("config.unreadable"))?;
        Self::parse(&text)
    }

    /// Parses the text of a configuration.
    ///
    /// # Errors
    ///
    /// As [`Config::load`].
    pub fn parse(text: &str) -> Result<Self, Failure> {
        let mut config = Self {
            repositories: BTreeMap::new(),
            executor: ExecutorProfile::Fake,
            fake_agent: None,
            path: DEFAULT_PATH.to_owned(),
            idle_after_ms: 2_000,
            grace_ms: 2_000,
            anchor: None,
        };
        let invalid = Failure::new("config.invalid");
        let mut section = String::new();
        for raw in text.lines() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            {
                if !matches!(name, "repositories" | "executor" | "runs" | "anchor") {
                    return Err(invalid);
                }
                name.clone_into(&mut section);
                continue;
            }
            let (key, value) = line.split_once('=').ok_or(invalid)?;
            let (key, value) = (key.trim(), Value::parse(value.trim()).ok_or(invalid)?);
            match (section.as_str(), key, value) {
                ("repositories", name, Value::Text(path)) if is_name(name) => {
                    let path = PathBuf::from(path);
                    if !path.is_absolute() {
                        return Err(Failure::new("config.repository_invalid"));
                    }
                    config.repositories.insert(name.to_owned(), path);
                }
                ("executor", "profile", Value::Text(name)) => {
                    config.executor = ExecutorProfile::from_config_name(&name)?;
                }
                ("executor", "fake_agent", Value::Text(path)) => {
                    let path = PathBuf::from(path);
                    if !path.is_absolute() {
                        return Err(invalid);
                    }
                    config.fake_agent = Some(path);
                }
                ("runs", "path", Value::Text(path)) => config.path = path,
                ("runs", "idle_after_ms", Value::Integer(milliseconds)) => {
                    config.idle_after_ms = milliseconds;
                }
                ("runs", "grace_ms", Value::Integer(milliseconds)) => {
                    config.grace_ms = milliseconds
                }
                ("anchor", "path", Value::Text(path)) => {
                    let path = PathBuf::from(path);
                    if !path.is_absolute() {
                        return Err(invalid);
                    }
                    config.anchor = Some(path);
                }
                _ => return Err(invalid),
            }
        }
        Ok(config)
    }

    /// The repository registered under `name`.
    ///
    /// # Errors
    ///
    /// `config.repository_unknown`.
    pub fn repository(&self, name: &str) -> Result<&Path, Failure> {
        self.repositories
            .get(name)
            .map(PathBuf::as_path)
            .ok_or(Failure::new("config.repository_unknown"))
    }

    /// Every repository, by name.
    #[must_use]
    pub fn repositories(&self) -> Vec<(String, PathBuf)> {
        self.repositories
            .iter()
            .map(|(name, path)| (name.clone(), path.clone()))
            .collect()
    }

    /// The executor profile (always [`ExecutorProfile::Fake`] before C0).
    #[must_use]
    pub const fn executor(&self) -> ExecutorProfile {
        self.executor
    }

    /// Path of the fake agent binary.
    ///
    /// # Errors
    ///
    /// `config.fake_agent_missing`.
    pub fn fake_agent(&self) -> Result<&Path, Failure> {
        self.fake_agent
            .as_deref()
            .ok_or(Failure::new("config.fake_agent_missing"))
    }

    /// `PATH` given to runs.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Silence, in milliseconds, after which a running mission waits for input.
    #[must_use]
    pub const fn idle_after_ms(&self) -> u64 {
        self.idle_after_ms
    }

    /// The private file the journal head is anchored to, outside the root.
    #[must_use]
    pub fn anchor(&self) -> Option<&Path> {
        self.anchor.as_deref()
    }

    /// Delay between `SIGTERM` and `SIGKILL`, in milliseconds.
    #[must_use]
    pub const fn grace_ms(&self) -> u64 {
        self.grace_ms
    }
}

enum Value {
    Text(String),
    Integer(u64),
}

impl Value {
    fn parse(text: &str) -> Option<Self> {
        if let Some(inner) = text
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            // Basic strings without escapes: a backslash or quote inside is refused.
            if inner.contains(['"', '\\']) || inner.chars().any(char::is_control) {
                return None;
            }
            return Some(Self::Text(inner.to_owned()));
        }
        if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) {
            return text.parse().ok().map(Self::Integer);
        }
        None
    }
}

fn strip_comment(line: &str) -> &str {
    // A '#' starts a comment unless it is inside a quoted value.
    let mut quoted = false;
    for (index, character) in line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '#' if !quoted => return line.get(..index).unwrap_or(line),
            _ => {}
        }
    }
    line
}

fn is_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && name.len() <= 64
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn parses_the_documented_subset_and_refuses_the_rest() {
        let config = Config::parse(
            "# c\n[repositories]\nsample = \"/r/sample\" # inline\n[executor]\nprofile = \"fake\"\nfake_agent = \"/bin/agent\"\n[runs]\nidle_after_ms = 300\n",
        )
        .unwrap();
        assert_eq!(
            config.repository("sample").unwrap().to_str(),
            Some("/r/sample")
        );
        assert_eq!(config.idle_after_ms(), 300);
        assert_eq!(config.grace_ms(), 2_000);
        for invalid in [
            "[unknown]\n",
            "[runs]\nidle_after_ms = \"300\"\n",
            "[runs]\nspeed = 3\n",
            "[repositories]\nBad = \"/x\"\n",
            "[repositories]\nx = \"/a\\\\b\"\n",
            "loose = 1\n",
            "[runs]\nidle_after_ms = -1\n",
        ] {
            assert_eq!(
                Config::parse(invalid).unwrap_err().code(),
                "config.invalid",
                "{invalid:?}"
            );
        }
        assert_eq!(
            Config::parse("[repositories]\nx = \"relative\"\n")
                .unwrap_err()
                .code(),
            "config.repository_invalid"
        );
        for real in ["claude", "codex", "/usr/local/bin/claude"] {
            let text = format!("[executor]\nprofile = \"{real}\"\n");
            assert_eq!(
                Config::parse(&text).unwrap_err().code(),
                "agent.real_forbidden_until_c0"
            );
        }
    }
}
