//! `wsd`, the Work Supervision v0 daemon, and the client of its socket.
//!
//! The daemon is the single writer of a root: it holds the journal lock, the
//! PTY of every run and the worktree manager, and applies the requests of
//! `ws` received on `run/wsd.sock` — a Unix socket of mode 0600 inside a
//! 0700 directory, whose peers must run as the daemon's own user. It opens no
//! network port.
//!
//! On start it follows the recovery order of the plan (§2.7): the journal is
//! verified by the independent verifier, a torn tail is quarantined, the
//! projection catches up, unconfirmed worktree intents are reconciled, runs
//! left without a PTY are recorded as interrupted, and the mission steps a
//! crash between two entries left undone are completed.

mod anchor;
mod clock;
mod config;
mod coordination;
mod core;
mod failure;
mod fault;
mod peer;
mod protocol;
mod root;
mod server;

pub use anchor::{Anchor, check_anchor};
pub use clock::now;
pub use config::Config;
pub use failure::Failure;
pub use fault::FAULT_POINTS;
pub use peer::{authorize_peer, peer_uid};
pub use protocol::{Client, ClientError, MAX_MESSAGE_BYTES};
pub use root::init_root;
pub use server::serve;
