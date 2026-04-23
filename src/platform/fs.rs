//! Atomic filesystem operations for durable writes and directory replacement.
//!
//! All durable Mars writes should go through this module.

pub use crate::fs::{
    FLAT_SKILL_EXCLUDED_TOP_LEVEL, atomic_install_dir, atomic_install_dir_filtered, atomic_write,
    remove_item,
};

#[cfg(windows)]
pub use crate::fs::clear_readonly;
