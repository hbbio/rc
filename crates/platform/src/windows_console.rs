use std::io;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler};

static ACTIVE_GUARDS: AtomicUsize = AtomicUsize::new(0);
static HANDLER_REGISTRATION: OnceLock<Result<(), Option<i32>>> = OnceLock::new();

/// Keeps Rust Commander's process alive when its foreground child receives a console interrupt.
///
/// Windows invokes the handler on a control-dispatch thread in rc's process. Application-defined
/// console handlers are process-local, so a subsequently spawned child retains its own default
/// handler and still exits on Ctrl-C or Ctrl-Break.
#[derive(Debug)]
#[must_use = "the guard must remain alive while the child owns the console"]
pub struct ParentConsoleControlGuard {
    active: bool,
}

impl ParentConsoleControlGuard {
    /// Installs the process-local handler once and enables it for this scope.
    pub fn acquire() -> io::Result<Self> {
        ensure_handler_registered()?;
        ACTIVE_GUARDS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                active.checked_add(1)
            })
            .map_err(|_| io::Error::other("console control guard count overflowed"))?;
        Ok(Self { active: true })
    }
}

impl Drop for ParentConsoleControlGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        let previous = ACTIVE_GUARDS.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "console control guard count underflowed");
    }
}

fn ensure_handler_registered() -> io::Result<()> {
    match HANDLER_REGISTRATION.get_or_init(register_handler) {
        Ok(()) => Ok(()),
        Err(raw_os_error) => Err(raw_os_error.map_or_else(
            || io::Error::other("failed to register Windows console control handler"),
            io::Error::from_raw_os_error,
        )),
    }
}

fn register_handler() -> Result<(), Option<i32>> {
    // SAFETY: `parent_console_control_handler` has the required system ABI, remains valid for the
    // lifetime of the process, and performs only a lock-free atomic load before returning.
    let registered = unsafe { SetConsoleCtrlHandler(Some(parent_console_control_handler), 1) };
    if registered == 0 {
        Err(io::Error::last_os_error().raw_os_error())
    } else {
        Ok(())
    }
}

unsafe extern "system" fn parent_console_control_handler(control_type: u32) -> i32 {
    let is_interrupt = matches!(control_type, CTRL_C_EVENT | CTRL_BREAK_EVENT);
    i32::from(is_interrupt && ACTIVE_GUARDS.load(Ordering::Acquire) > 0)
}
