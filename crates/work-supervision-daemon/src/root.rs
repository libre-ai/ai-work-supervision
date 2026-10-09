use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::Path;

use work_supervision_store::Layout;

use crate::Failure;

const CONFIG_TEMPLATE: &str = "# Work Supervision v0 — private configuration of this root (never commit it).\n\
[repositories]\n\
# name = \"/absolute/path/to/repository\"\n\
\n\
[executor]\n\
profile = \"fake\"\n\
# fake_agent = \"/absolute/path/to/ws-fake-agent\"\n\
\n\
[runs]\n\
idle_after_ms = 2000\n\
grace_ms = 2000\n";

/// Creates a new root: the directory (mode 0700), its sub-directories and a
/// configuration template (mode 0600).
///
/// The root is always a parameter; nothing has a default location. A root
/// inside a git working tree is refused, so that no instance data can end up
/// in a commit.
///
/// # Errors
///
/// `root.not_absolute`, `root.exists`, `root.inside_repository`, `root.io`.
pub fn init_root(root: &Path) -> Result<(), Failure> {
    if !root.is_absolute() {
        return Err(Failure::new("root.not_absolute"));
    }
    if fs::symlink_metadata(root).is_ok() {
        return Err(Failure::new("root.exists"));
    }
    if root
        .ancestors()
        .skip(1)
        .any(|ancestor| ancestor.join(".git").exists())
    {
        return Err(Failure::new("root.inside_repository"));
    }
    let io = |_| Failure::new("root.io");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(root)
        .map_err(io)?;
    let layout = Layout::new(root);
    for directory in [
        root.join("journal"),
        layout.blobs(),
        layout.evidence(),
        layout.worktrees(),
        layout.runs(),
        root.join("run"),
    ] {
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(io)?;
    }
    let mut config = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(layout.config())
        .map_err(io)?;
    config.write_all(CONFIG_TEMPLATE.as_bytes()).map_err(io)?;
    config.sync_all().map_err(io)?;
    Ok(())
}
