#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_console;

#[cfg(windows)]
pub use windows_console::ParentConsoleControlGuard;
