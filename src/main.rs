#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]
#![deny(unused_must_use)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn main() {
    match agent_youtube::dispatch_web_worker(std::env::args_os().skip(1)) {
        Ok(true) => return,
        Ok(false) => {}
        Err(_) => std::process::exit(1),
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    if let Err(error) = ctrlc::set_handler(move || {
        signal.store(true, Ordering::SeqCst);
    }) {
        agent_youtube::protocol::error(&error, false);
        std::process::exit(1);
    }
    if let Err(error) =
        agent_youtube::cli::run(std::env::args_os().skip(1).collect(), cancelled.clone())
    {
        agent_youtube::protocol::error(error.as_ref(), cancelled.load(Ordering::SeqCst));
        std::process::exit(
            if cancelled.load(Ordering::SeqCst)
                && !error
                    .downcast_ref::<agent_youtube::protocol::Failure>()
                    .is_some_and(|e| e.code == "job_deadline_exceeded")
            {
                130
            } else {
                1
            },
        );
    }
}
