//! Platform boundary for Windows/POSIX-sensitive operations.
//!
//! This module centralizes platform-specific behavior so runtime code can import
//! from `crate::platform` rather than scattering `#[cfg]` branches.
//!
//! Boundary rule: use `crate::platform` for
//! - Filesystem-safe name generation from external identifiers
//! - Global cache root resolution
//! - Cross-process file locks
//! - Durable file writes (atomic writes)
//! - Generated directory replacement and cache publication
//! - External process invocation (git)
//!
//! What stays direct: PathBuf joins under resolved roots, fs::read/read_dir,
//! config reads, content hashing, domain validation in SourceSubpath.

pub mod cache;
pub mod fs;
pub mod lock;
pub mod path_syntax;
pub mod process;

// Re-export commonly used items at the platform level
pub use cache::{global_cache_root, safe_component, safe_component_with_hash};
pub use fs::{atomic_install_dir, atomic_install_dir_filtered, atomic_write};
pub use lock::FileLock;
