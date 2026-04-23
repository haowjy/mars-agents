#![allow(clippy::result_large_err)]

pub mod cli;
pub mod config;
pub mod diagnostic;
pub mod discover;
pub mod error;
pub mod frontmatter;
pub mod fs;
pub mod hash;
pub mod local_source;
pub mod lock;
pub mod merge;
pub mod models;
pub mod platform;
pub mod reconcile;
pub mod resolve;
pub mod source;
pub mod sync;
pub mod target_sync;
pub mod types;
pub mod validate;
