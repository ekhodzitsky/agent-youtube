#![cfg_attr(not(test), deny(unsafe_code))]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![deny(unused_must_use)]
mod agent;
mod asr_output;
mod audio;
mod batch;
mod boundaries;
mod checkpoint;
pub mod cli;
pub mod download;
pub mod export;
mod job;
mod memory;
pub mod pipeline;
pub mod process;
mod process_memory;
pub mod protocol;
mod runtime;
pub mod storage;
pub mod time_range;
pub mod transcript;
mod web;

#[cfg(test)]
#[path = "../tests/support/fmp4.rs"]
mod fmp4;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[doc(hidden)]
pub fn run_web_worker() -> Result<()> {
    web::run_worker()
}

/// Dispatch the private Web worker before installing signal handlers or emitting output.
/// Pass arguments excluding argv[0]. Returns false without side effects for ordinary
/// application arguments. A handled worker owns stdin/stdout until EOF; errors must
/// be converted to a nonzero exit without application diagnostics on its IPC stdout.
pub fn dispatch_web_worker(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<bool> {
    let mut args = args.into_iter();
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--internal-web-runtime"))
        || args.next().is_some()
    {
        return Ok(false);
    }
    run_web_worker()?;
    Ok(true)
}
