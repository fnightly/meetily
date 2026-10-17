//! Optional sync of a finished meeting note to Obsidian or Notion.
//!
//! - [`obsidian`] writes a Markdown file locally and never uses the network;
//! - [`notion`] converts the same Markdown into blocks and calls the Notion API;
//! - [`settings`] persists the non-secret configuration;
//! - [`commands`] is the Tauri surface the meeting page calls.
//!
//! Sync is always triggered per meeting. There is no hook on save and no
//! background job: a meeting only leaves the machine when the user asks for it.

pub mod commands;
pub mod notion;
pub mod obsidian;
pub mod settings;
