//! Asking a child process to stop the way Ctrl+C would.
//!
//! Killing a client outright is not always enough: `docker build` leaves BuildKit compiling
//! for minutes after its client is killed, whereas an interrupt makes the client cancel the
//! build on the other side. So cancellation interrupts first and kills only as a last resort.

/// Delivers an interrupt to the child, returning whether it could be sent.
///
/// Windows has no per-process signal. Children are started in their own process group
/// (`CREATE_NEW_PROCESS_GROUP`, see `process.rs`), and a Ctrl+Break sent to that group reaches
/// only the child and the tools it started, never this app, its terminal or the tooling
/// around it. Go programs such as `docker` treat it as an interrupt and shut down cleanly.
/// The event can only be sent from the child's console, so this process attaches to it for
/// the moment and then goes back to whatever console it had.
#[cfg(windows)]
pub fn interrupt(pid: u32) -> bool {
    use std::sync::Mutex;

    #[link(name = "kernel32")]
    extern "system" {
        fn FreeConsole() -> i32;
        fn AttachConsole(process_id: u32) -> i32;
        fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
    }
    const CTRL_BREAK_EVENT: u32 = 1;
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;

    // Attaching changes this process's console, so only one interrupt may run at a time.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    // SAFETY: plain kernel32 calls with valid arguments; the console is restored afterwards.
    unsafe {
        FreeConsole();
        let sent = AttachConsole(pid) != 0 && GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) != 0;
        FreeConsole();
        // A development build runs from a terminal; give it back. Fails harmlessly without one.
        AttachConsole(ATTACH_PARENT_PROCESS);
        sent
    }
}

#[cfg(not(windows))]
pub fn interrupt(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success())
}
