//! Library side of `ws`: the terminal interface of `ws attach`, the
//! translation of harness hooks (`ws hook`) and the Markdown rendering of a
//! mission report (`ws report --markdown`).

pub mod hook;
pub mod report;
pub mod tui;
