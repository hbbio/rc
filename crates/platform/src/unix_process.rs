use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Limit files created by this child and its descendants. Existing process
/// limits are untouched. Core dumps are disabled to avoid unbounded crash files.
pub fn limit_child_file_size(command: &mut Command, bytes: u64) -> io::Result<()> {
    let bytes = libc::rlim_t::try_from(bytes).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "child file limit is too large")
    })?;
    let file_limit = libc::rlimit {
        rlim_cur: bytes,
        rlim_max: bytes,
    };
    let core_limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: the post-fork hook only calls setrlimit and captures errno on
    // failure. It does not allocate, lock, inspect the environment, or unwind.
    // Both pointers refer to initialized values captured by the closure.
    unsafe {
        command.pre_exec(move || {
            if libc::setrlimit(libc::RLIMIT_FSIZE, &file_limit) != 0
                || libc::setrlimit(libc::RLIMIT_CORE, &core_limit) != 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}
