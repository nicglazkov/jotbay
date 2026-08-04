//! Spawning child processes without a console window appearing.
//!
//! On Windows a console subprocess started by a windowed parent allocates its
//! own console, so `git`, `gh` and `powershell` each flash a black window and
//! steal focus. First run is the worst case — capabilities, clone, identity,
//! shortcuts and scheduling in a row, each with its own flash.
//!
//! `git.rs` had this from the start; nothing else did, because nothing else was
//! called from a GUI until first-run setup arrived and then started registering
//! schedulers and making shortcuts.

use std::process::Command;

/// A `Command` that never allocates a console on Windows.
///
/// Use this for every child process in the core. The flag is inert everywhere
/// else, so there is no reason to spawn any other way.
pub fn quiet(program: &str) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}
