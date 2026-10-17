//! Workspace features layered on top of meetings: speaker names, tags and
//! projects, action items, full-text search, meeting Q&A, and backup/restore.
//!
//! Each submodule owns its tables (see migration `20261101000000`) and exposes
//! Tauri commands registered in `lib.rs`.

pub mod action_items;
pub mod ask;
pub mod backup;
pub mod labels;
pub mod search;
pub mod speakers;
pub mod text;

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
