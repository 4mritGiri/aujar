//! Aujar daemon - Cross-platform IPC host
//!
//! On Unix platforms, uses Unix domain sockets.
//! On Windows, uses named pipes.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
pub use unix::*;
