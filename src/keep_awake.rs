//! Prevent the system from entering sleep or hibernation during long-running
//! operations (downloading or patching).
//!
//! On Windows, [`SetThreadExecutionState`] with `ES_CONTINUOUS |
//! ES_SYSTEM_REQUIRED` tells the power manager that this thread is performing
//! an activity that should keep the display and system awake.  When the guard
//! is dropped the previous state is restored, allowing the system to sleep
//! normally again.
//!
//! On non-Windows targets the guard is a no-op.

/// RAII guard that prevents the system from sleeping while it exists.
///
/// Create one at the start of a long-running operation; drop it (or let it
/// fall out of scope) when the operation finishes.
///
/// ```ignore
/// let _awake = KeepAwake::new();
/// // ... long-running work ...
/// // _awake is dropped here → system can sleep again.
/// ```
pub struct KeepAwake {
    #[cfg(windows)]
    _private: (),
}

impl KeepAwake {
    /// Prevent the system from sleeping until this guard is dropped.
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            imp::set_awake();
        }
        Self {
            #[cfg(windows)]
            _private: (),
        }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            imp::restore();
        }
    }
}

#[cfg(windows)]
mod imp {
    // ES_CONTINUOUS = 0x80000000, ES_SYSTEM_REQUIRED = 0x00000001
    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    extern "system" {
        /// Informs the system that this thread has an activity in progress
        /// that should prevent sleep.  Returns the previous execution state.
        fn SetThreadExecutionState(esFlags: u32) -> u32;
    }

    /// Tell Windows to stay awake (continuous system-required).
    pub fn set_awake() {
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        }
    }

    /// Restore the default power-management behaviour (allow sleep).
    pub fn restore() {
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS);
        }
    }
}
