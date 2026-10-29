//! Defer interactive sync interruption until normal permission cleanup runs.
use std::sync::atomic::{AtomicI32, Ordering};

static SIGNAL: AtomicI32 = AtomicI32::new(0);

pub fn interrupted() -> bool {
    SIGNAL.load(Ordering::Relaxed) != 0
}

/// Conventional shell status, retaining the first signal while cleanup runs.
pub fn exit_code() -> Option<i32> {
    let signal = SIGNAL.load(Ordering::Relaxed);
    (signal != 0).then_some(128 + signal)
}

pub fn check() -> anyhow::Result<()> {
    if interrupted() {
        anyhow::bail!("Sync interrupted");
    }
    Ok(())
}

#[cfg(unix)]
extern "C" fn mark_interrupted(signal: libc::c_int) {
    // The handler must not allocate, lock, log, or touch the filesystem.
    let _ = SIGNAL.compare_exchange(0, signal, Ordering::Relaxed, Ordering::Relaxed);
}

pub struct Guard {
    #[cfg(unix)]
    previous: Vec<(libc::c_int, libc::sigaction)>,
}

impl Guard {
    pub fn new() -> anyhow::Result<Self> {
        SIGNAL.store(0, Ordering::Relaxed);
        #[cfg(unix)]
        {
            let mut guard = Self {
                previous: Vec::new(),
            };
            for signal in [libc::SIGINT, libc::SIGTERM] {
                // SAFETY: sigaction structs are initialized, and the handler
                // only stores an atomic flag. Drop restores installed actions.
                unsafe {
                    let mut action: libc::sigaction = std::mem::zeroed();
                    action.sa_sigaction = mark_interrupted as *const () as usize;
                    libc::sigemptyset(&mut action.sa_mask);
                    let mut old = std::mem::zeroed();
                    if libc::sigaction(signal, &action, &mut old) != 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    guard.previous.push((signal, old));
                }
            }
            Ok(guard)
        }
        #[cfg(not(unix))]
        Ok(Self {})
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        #[cfg(unix)]
        for (signal, action) in self.previous.iter().rev() {
            // SAFETY: restore the action captured by successful sigaction.
            unsafe {
                libc::sigaction(*signal, action, std::ptr::null_mut());
            }
        }
    }
}
