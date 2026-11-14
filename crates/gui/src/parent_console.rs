//! The console of whoever started us, borrowed.
//!
//! The executable is built for Windows' GUI subsystem (`main.rs`), so that
//! opening it from Explorer or a shortcut opens a window and not a window and
//! a black console behind it. A GUI-subsystem program has no console of its
//! own, though, and this one prints: `--help`, `--devices`, `--accounts`, and
//! everything `--answer` says. Started from a shell, it attaches to the
//! shell's console instead, and `println!` goes where it always went.
//!
//! Two things are different from a console program, and both are Windows':
//!
//! * the shell does not wait for a GUI-subsystem program, so its prompt comes
//!   back at once and the output arrives after it. `start /wait binmodem ...`
//!   in cmd, or `binmodem ... | Out-Host` in PowerShell, waits;
//! * output that is redirected or piped goes where it was sent and needs none
//!   of this, which is how the scripts in this repository read it.

/// Attach to the parent process's console, if it has one. Does nothing
/// anywhere but Windows, and nothing when there is no such console: the
/// program was started from Explorer, or its output is already redirected.
// The one foreign call in the program: the workspace denies `unsafe`, and
// this is the exception it is denied in order to make visible.
#[allow(unsafe_code)]
pub fn attach() {
    #[cfg(windows)]
    {
        /// "Use the console of the parent of the current process"
        /// (`AttachConsole`, wincon.h).
        const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        // SAFETY: AttachConsole takes a process identifier by value and
        // touches nothing of ours. It fails, returning zero, when the parent
        // has no console or this process already has one, and either way
        // there is nothing to undo.
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}
