#![deny(unsafe_code, unsafe_op_in_unsafe_fn)]

#[cfg(unix)]
#[allow(unsafe_code)]
mod unix_process;

#[cfg(unix)]
pub use unix_process::limit_child_file_size;

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows_console;

#[cfg(windows)]
pub use windows_console::ParentConsoleControlGuard;
