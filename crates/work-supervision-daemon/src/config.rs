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
//!
//! [forge]                                # coordination-v0.md, « Forge signals »
//! mode = "observe"                       # or "active"; observe when absent
//!
//! [forge.senders]                        # who is admitted; nobody when absent
//! lead = "login-1"                       # entry name (journalled) = forge login (never journalled)
//! ci = "github-actions[bot]"
//! ```
//!
//! An admitted signal records the entry name, never the login: the owner
//! chooses a name that is not the login itself (refused when equal).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use work_supervision_domain::ExecutorProfile;
use work_supervision_domain::forge::{AllowedSender, ForgeLogin, ForgeMode, SenderEntry};

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
    forge_mode: ForgeMode,
    forge_senders: Vec<AllowedSender>,
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
            forge_mode: ForgeMode::Observe,
            forge_senders: Vec::new(),
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
                if !matches!(
                    name,
                    "repositories" | "executor" | "runs" | "anchor" | "forge" | "forge.senders"
                ) {
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
                ("forge", "mode", Value::Text(mode)) => {
                    config.forge_mode = ForgeMode::parse(&mode).map_err(|_| invalid)?;
                }
                ("forge.senders", entry, Value::Text(login)) => {
                    let allowed = sender(entry, &login).ok_or(invalid)?;
                    let taken = config
                        .forge_senders
                        .iter()
                        .any(|known| known.entry == allowed.entry || known.login == allowed.login);
                    if taken {
                        return Err(invalid);
                    }
                    config.forge_senders.push(allowed);
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

    /// What an admitted forge signal that triggers does (`observe` unless
    /// configured `active`).
    #[must_use]
    pub const fn forge_mode(&self) -> ForgeMode {
        self.forge_mode
    }

    /// The sender list: whose signals are admitted, under which entry name.
    /// Empty unless configured: every signal is then ignored.
    #[must_use]
    pub fn forge_senders(&self) -> &[AllowedSender] {
        &self.forge_senders
    }
}

/// One `[forge.senders]` line: an entry name and the login it admits. An
/// entry named as its login would journal the login, so it is refused.
fn sender(entry: &str, login: &str) -> Option<AllowedSender> {
    let allowed = AllowedSender {
        entry: SenderEntry::parse(entry).ok()?,
        login: ForgeLogin::parse(login).ok()?,
    };
    (allowed.entry.as_str() != allowed.login.as_str()).then_some(allowed)
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
    use work_supervision_domain::forge::ForgeMode;

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
        // Forge signals: observe and nobody admitted unless configured.
        assert_eq!(config.forge_mode(), ForgeMode::Observe);
        assert!(config.forge_senders().is_empty());
        let forge = Config::parse(
            "[forge]\nmode = \"active\"\n[forge.senders]\nlead = \"Reviewer-1\"\nci = \"github-actions[bot]\"\n",
        )
        .unwrap();
        assert_eq!(forge.forge_mode(), ForgeMode::Active);
        assert_eq!(
            forge
                .forge_senders()
                .iter()
                .map(|allowed| (allowed.entry.as_str(), allowed.login.as_str()))
                .collect::<Vec<_>>(),
            [("lead", "reviewer-1"), ("ci", "github-actions[bot]")]
        );
        assert!(
            Config::parse("[forge.senders]\n")
                .unwrap()
                .forge_senders()
                .is_empty()
        );
        for invalid in [
            "[forge]\nmode = \"auto\"\n",
            "[forge]\nmode = 1\n",
            "[forge]\nsenders = \"reviewer-1\"\n",
            "[forge]\nkey = \"x\"\n",
            "[forge.senders]\nlead = \"two words\"\n",
            "[forge.senders]\nLead = \"reviewer-1\"\n",
            "[forge.senders]\nlead = 1\n",
            // The same entry twice, the same login twice.
            "[forge.senders]\nlead = \"a\"\nlead = \"b\"\n",
            "[forge.senders]\nlead = \"a\"\nsecond = \"A\"\n",
            // An entry named as its login would journal the login.
            "[forge.senders]\nreviewer-1 = \"Reviewer-1\"\n",
        ] {
            assert_eq!(
                Config::parse(invalid).unwrap_err().code(),
                "config.invalid",
                "{invalid:?}"
            );
        }
        for real in ["claude", "codex", "/usr/local/bin/claude"] {
            let text = format!("[executor]\nprofile = \"{real}\"\n");
            assert_eq!(
                Config::parse(&text).unwrap_err().code(),
                "agent.real_forbidden_until_c0"
            );
        }
    }
}
