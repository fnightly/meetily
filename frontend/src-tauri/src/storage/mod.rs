//! User-configurable storage locations (database, models, notes, recordings).
//!
//! See [`config`] for resolution rules and [`commands`] for the Tauri surface.

pub mod commands;
pub mod config;

pub use config::{
    load_storage_config, resolve_models_dir, save_storage_config, StorageConfig,
    STORAGE_CONFIG_STORE,
};
