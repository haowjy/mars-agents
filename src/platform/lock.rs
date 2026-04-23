//! Cross-process file locking.
//!
//! Uses flock on Unix, LockFileEx on Windows.

pub use crate::fs::FileLock;
