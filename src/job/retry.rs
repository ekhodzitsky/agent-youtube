//! Durable outer retries. New jobs budget waits; legacy jobs retain absolute deadlines.
use super::{read, write};
use crate::{Result, protocol};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn retryable_code(code: &str) -> bool {
    matches!(
        code,
        "youtube_bot_check"
            | "youtube_rate_limited"
            | "youtube_temporary_failure"
            | "http_transport"
            | "tls_random_unavailable"
            | "download_http_temporary"
            | "download_transport"
            | "download_incomplete"
            | "download_recovery_exhausted"
            | "download_source_expired"
            | "download_no_progress"
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    schema_version: u32,
    pub(super) video_id: String,
    pub(super) window_seconds: u64,
    pub(super) deadline: Option<u64>,
    pub(super) next_retry_at: Option<u64>,
    attempts: u64,
    last_error_code: Option<String>,
    pub(super) complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remaining_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    waiting_since: Option<u64>,
}
impl State {
    fn valid(&self) -> bool {
        if self.window_seconds == 0 {
            return false;
        }
        match self.schema_version {
            1 => {
                self.deadline.is_some_and(|at| at > 0)
                    && self.remaining_seconds.is_none()
                    && self.waiting_since.is_none()
            }
            2 => {
                if (self.attempts == 0 && self.last_error_code.is_some())
                    || (self.attempts > 0
                        && !self.last_error_code.as_deref().is_some_and(retryable_code))
                {
                    return false;
                }
                let Some(remaining) = self
                    .remaining_seconds
                    .filter(|left| *left <= self.window_seconds)
                else {
                    return false;
                };
                match (self.next_retry_at, self.waiting_since, self.deadline) {
                    (None, None, None) => true,
                    (Some(next), Some(start), Some(deadline)) => {
                        !self.complete
                            && self.attempts > 0
                            && next > start
                            && start.checked_add(remaining).and_then(|n| n.checked_add(1))
                                == Some(deadline)
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
    pub(super) fn active_deadline(&self) -> Option<u64> {
        self.deadline
            .filter(|_| self.schema_version == 1 && !self.complete)
    }
    pub(super) fn exhausted(&self, time: u64) -> bool {
        !self.complete
            && (self.schema_version == 1 || self.next_retry_at.is_some())
            && (self.deadline.is_some_and(|deadline| time >= deadline)
                || (self.schema_version == 2 && self.remaining_seconds == Some(0)))
    }
    fn remaining_at(&self, time: u64) -> Option<u64> {
        self.remaining_seconds.map(|left| {
            left.saturating_sub(
                self.waiting_since
                    .map_or(0, |start| time.saturating_sub(start)),
            )
        })
    }
    pub(super) fn status(&self, value: &mut serde_json::Value, time: u64) {
        value["next_retry_at"] = serde_json::json!(self.next_retry_at);
        value["deadline"] = serde_json::json!(self.deadline);
        value["retry_policy"] = serde_json::json!(if self.schema_version == 2 {
            "waiting_budget"
        } else {
            "legacy_job_deadline"
        });
        value["retry_budget_remaining_seconds"] = serde_json::json!(self.remaining_at(time));
    }
}
pub(crate) struct Retry {
    path: PathBuf,
    state: Option<State>,
}
pub(super) fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}
pub(super) fn saved(root: &Path) -> Result<Option<State>> {
    match read::<State>(&root.join("job-retry.json")) {
        Ok(state) if state.valid() => Ok(Some(state)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        _ => Err(protocol::failure(
            "job_retry_invalid",
            Some(false),
            "invalid retry schedule; evidence preserved",
        )),
    }
}
impl Retry {
    pub(crate) fn open(options: &crate::pipeline::Options) -> Result<Self> {
        let path = options.work_dir.join("job-retry.json");
        let mut state = saved(&options.work_dir)?;
        let id = crate::pipeline::video_id(&options.url)?;
        if let Some(saved) = &state {
            if saved.video_id != id || saved.window_seconds != options.retry_window_seconds {
                return Err(protocol::failure(
                    "job_retry_invalid",
                    Some(false),
                    "retry URL/window changed; saved policy preserved",
                ));
            }
        } else if options.retry_window_seconds > 0 {
            if super::invocation_retry_window(&options.work_dir, &id)?
                .is_some_and(|window| window > 0)
            {
                return Err(protocol::failure(
                    "job_retry_invalid",
                    Some(false),
                    "enabled retry schedule is missing; refusing to reset its budget",
                ));
            }
            now()?
                .checked_add(options.retry_window_seconds)
                .and_then(|n| n.checked_add(1))
                .ok_or("retry deadline overflow")?;
            state = Some(State {
                schema_version: 2,
                video_id: id,
                window_seconds: options.retry_window_seconds,
                deadline: None,
                next_retry_at: None,
                attempts: 0,
                last_error_code: None,
                complete: false,
                remaining_seconds: Some(options.retry_window_seconds),
                waiting_since: None,
            });
            write(&path, &state)?;
        }
        Ok(Self { path, state })
    }
    pub(crate) fn deadline_error(&self) -> Box<dyn std::error::Error + Send + Sync> {
        let budget = self.state.as_ref().is_some_and(|s| s.schema_version == 2);
        protocol::failure_details(
            "job_deadline_exceeded",
            Some(false),
            if budget {
                "automatic recovery waiting budget exhausted; checkpoint and media preserved"
            } else {
                "job deadline exceeded; checkpoint and media preserved"
            },
            serde_json::json!({"deadline":self.state.as_ref().and_then(|s|s.deadline),"last_error_code":self.state.as_ref().and_then(|s|s.last_error_code.as_deref()),"retry_policy":if budget { "waiting_budget" } else { "legacy_job_deadline" },"retry_budget_remaining_seconds":if budget { Some(0u64) } else { None },"preserved":true,"next_action":"inspect_preserved_artifacts"}),
        )
    }
    pub(crate) fn wait(&mut self, cancelled: &AtomicBool) -> Result<()> {
        loop {
            let Some(state) = &self.state else {
                return Ok(());
            };
            if state.complete {
                return Ok(());
            }
            let time = now()?;
            if state.exhausted(time) {
                return Err(self.deadline_error());
            }
            if cancelled.load(Ordering::SeqCst) {
                return Err(protocol::failure(
                    "interrupted",
                    Some(true),
                    "interrupted; schedule preserved for resume",
                ));
            }
            if state.next_retry_at.is_none_or(|at| time >= at) {
                if state.schema_version == 2 && state.next_retry_at.is_some() {
                    // Persist consumed waiting time before active work. Active downloads,
                    // decode and ASR are not subject to this recovery budget.
                    let remaining = state.remaining_at(time);
                    if let Some(state) = &mut self.state {
                        state.remaining_seconds = remaining;
                        state.next_retry_at = None;
                        state.waiting_since = None;
                        state.deadline = None;
                        write(&self.path, state)?;
                    }
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    pub(crate) fn schedule(&mut self, error: &(dyn std::error::Error + 'static)) -> Result<bool> {
        let Some(state) = &mut self.state else {
            return Ok(false);
        };
        let Some(failure) = error.downcast_ref::<protocol::Failure>() else {
            return Ok(false);
        };
        if failure.retryable != Some(true)
            || !retryable_code(failure.code)
            || (failure.retry_after().is_some() && failure.retry_after_seconds().is_none())
        {
            return Ok(false);
        }
        let delay = failure
            .retry_after_seconds()
            .unwrap_or(60u64.saturating_mul(1u64 << state.attempts.min(4)))
            .max(1);
        let time = now()?;
        if state.schema_version == 2 {
            let remaining = state.remaining_seconds.ok_or("missing recovery budget")?;
            state.deadline = Some(
                time.checked_add(remaining)
                    .and_then(|n| n.checked_add(1))
                    .ok_or("retry deadline overflow")?,
            );
            state.waiting_since = Some(time);
        }
        state.attempts = state
            .attempts
            .checked_add(1)
            .ok_or("job retry counter overflow")?;
        state.last_error_code = Some(failure.code.into());
        // Round upward: a whole-second clock must never shorten Retry-After.
        state.next_retry_at = Some(time.saturating_add(delay).saturating_add(1));
        write(&self.path, state)?;
        let mut progress = serde_json::json!({"state":"waiting","attempt":state.attempts,"cause_code":failure.code});
        state.status(&mut progress, time);
        protocol::progress("waiting", "waiting before automatic job retry", progress);
        Ok(true)
    }
    pub(crate) fn finish(&mut self, complete: bool) -> Result<()> {
        if let Some(state) = &mut self.state {
            state.next_retry_at = None;
            state.complete = complete;
            if state.schema_version == 2 {
                state.deadline = None;
                state.waiting_since = None;
            }
            write(&self.path, state)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (PathBuf, Retry) {
        let (root, checkpoint) = crate::checkpoint::tests::fixture();
        drop(checkpoint);
        let retry = Retry {
            path: root.join("job-retry.json"),
            state: Some(State {
                schema_version: 1,
                video_id: "a2hbJlPxj4M".into(),
                window_seconds: 600,
                deadline: Some(now().unwrap() + 600),
                next_retry_at: None,
                attempts: 0,
                last_error_code: None,
                complete: false,
                remaining_seconds: None,
                waiting_since: None,
            }),
        };
        (root, retry)
    }
    fn budget_fixture() -> (PathBuf, Retry) {
        let (root, mut retry) = fixture();
        let state = retry.state.as_mut().unwrap();
        state.schema_version = 2;
        state.deadline = None;
        state.remaining_seconds = Some(state.window_seconds);
        (root, retry)
    }
    #[test]
    fn budget_wait_is_charged_once_across_restart_and_active_work_has_no_deadline() {
        let (root, mut retry) = budget_fixture();
        let time = now().unwrap();
        let state = retry.state.as_mut().unwrap();
        state.waiting_since = Some(time - 3);
        state.deadline = Some(time - 3 + state.window_seconds + 1);
        state.next_retry_at = Some(time - 1);
        state.attempts = 1;
        state.last_error_code = Some("download_no_progress".into());
        write(&retry.path, state).unwrap();
        let mut restarted = Retry {
            path: retry.path.clone(),
            state: saved(&root).unwrap(),
        };
        restarted.wait(&AtomicBool::new(false)).unwrap();
        let state = restarted.state.as_ref().unwrap();
        assert!(state.remaining_seconds.unwrap() <= 597 && state.remaining_seconds.unwrap() > 0);
        assert!(
            state.deadline.is_none()
                && state.waiting_since.is_none()
                && state.next_retry_at.is_none()
        );
        let before = std::fs::read(&retry.path).unwrap();
        restarted.wait(&AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&retry.path).unwrap(), before);
        let state = restarted.state.as_ref().unwrap();
        let mut status = serde_json::json!({});
        state.status(&mut status, time + 86400);
        assert_eq!(
            status["retry_budget_remaining_seconds"],
            state.remaining_seconds.unwrap()
        );
        assert!(!state.exhausted(time + 86400));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn exhausted_budget_and_long_server_wait_never_reset_on_restart() {
        for remaining in [0, 1] {
            let (root, mut retry) = budget_fixture();
            let state = retry.state.as_mut().unwrap();
            state.remaining_seconds = Some(remaining);
            state.waiting_since = Some(1);
            state.deadline = Some(remaining + 2);
            state.next_retry_at = Some(121);
            state.attempts = 1;
            state.last_error_code = Some("youtube_rate_limited".into());
            write(&retry.path, state).unwrap();
            let before = std::fs::read(&retry.path).unwrap();
            let mut restarted = Retry {
                path: retry.path.clone(),
                state: saved(&root).unwrap(),
            };
            assert!(restarted.wait(&AtomicBool::new(false)).is_err());
            assert_eq!(std::fs::read(&retry.path).unwrap(), before);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn malformed_budget_wait_is_rejected_without_mutation() {
        let (root, mut retry) = budget_fixture();
        let state = retry.state.as_mut().unwrap();
        state.waiting_since = Some(10);
        state.deadline = Some(611);
        state.next_retry_at = Some(12);
        // A pending retry cannot precede its first recorded failure.
        write(&retry.path, state).unwrap();
        let before = std::fs::read(&retry.path).unwrap();
        assert!(saved(&root).is_err());
        assert_eq!(std::fs::read(&retry.path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resume_preserves_policy_if_preflight_never_updated_the_old_invocation() {
        let (root, retry) = fixture();
        write(&retry.path, retry.state.as_ref().unwrap()).unwrap();
        let options = crate::pipeline::Options {
            range: None,
            url: "https://youtu.be/a2hbJlPxj4M".into(),
            work_dir: root.clone(),
            model_dir: root.join("models"),
            model_variant: Default::default(),
            gigastt: "gigastt".into(),
            chunk_seconds: 300,
            boundary_window_ms: 0,
            asr_timeout_seconds: 1800,
            asr_memory_mib: None,
            encoder_threads: None,
            retry_window_seconds: 0,
        };
        super::super::save(&options).unwrap();
        let mut resumed = super::super::resume(&root).unwrap();
        assert_eq!(resumed.retry_window_seconds, 600);
        let before = std::fs::read(&retry.path).unwrap();
        Retry::open(&resumed).unwrap();
        resumed.retry_window_seconds = 601;
        assert!(Retry::open(&resumed).is_err());
        assert_eq!(std::fs::read(&retry.path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn schedule_is_durable_and_never_shortens_server_delay() {
        let (root, mut retry) = fixture();
        let deadline = retry.state.as_ref().unwrap().deadline;
        let before = now().unwrap();
        let error = protocol::failure_details(
            "youtube_rate_limited",
            Some(true),
            "fixture",
            serde_json::json!({"retry_after":"120","retry_after_seconds":120}),
        );
        assert!(retry.schedule(error.as_ref()).unwrap());
        let state = saved(&root).unwrap().unwrap();
        assert!(state.next_retry_at.unwrap() >= before + 121);
        assert_eq!(state.deadline, deadline);
        assert_eq!(state.attempts, 1);
        assert_eq!(
            state.last_error_code.as_deref(),
            Some("youtube_rate_limited")
        );
        assert!(retry.wait(&AtomicBool::new(true)).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn tls_entropy_failure_uses_the_existing_durable_retry_window() {
        let (root, mut retry) = fixture();
        let error = protocol::failure("tls_random_unavailable", Some(true), "sanitized");
        let deadline = retry.state.as_ref().unwrap().deadline;
        let before_schedule = now().unwrap();
        assert!(retry.schedule(error.as_ref()).unwrap());
        let state = saved(&root).unwrap().unwrap();
        assert_eq!(state.deadline, deadline);
        assert_eq!(
            state.last_error_code.as_deref(),
            Some("tls_random_unavailable")
        );
        assert!(state.next_retry_at.unwrap() >= before_schedule + 60);
        let before = std::fs::read(&retry.path).unwrap();
        retry.state = None; // Explicit retry-window=0 must remain disabled.
        assert!(!retry.schedule(error.as_ref()).unwrap());
        assert_eq!(std::fs::read(&retry.path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn permanent_unknown_and_unparseable_cooldowns_are_not_retried() {
        let (root, mut retry) = fixture();
        for error in [
            protocol::failure("youtube_source_unavailable", Some(false), "private"),
            protocol::failure("asr_failed", Some(true), "unknown worker"),
            protocol::failure("tls_certificate_invalid", Some(false), "sanitized"),
            protocol::failure("tls_peer_incompatible", Some(false), "sanitized"),
            protocol::failure_details(
                "youtube_rate_limited",
                Some(true),
                "fixture",
                serde_json::json!({"retry_after":"unparseable"}),
            ),
        ] {
            assert!(!retry.schedule(error.as_ref()).unwrap());
        }
        assert!(!retry.path.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_schedule_is_preserved_and_expiry_is_typed() {
        let (root, mut retry) = fixture();
        std::fs::write(&retry.path, b"bad fixture").unwrap();
        assert!(saved(&root).is_err());
        assert_eq!(std::fs::read(&retry.path).unwrap(), b"bad fixture");
        retry.state.as_mut().unwrap().deadline = Some(1);
        let error = retry.wait(&AtomicBool::new(false)).unwrap_err();
        assert_eq!(
            error.downcast_ref::<protocol::Failure>().unwrap().code,
            "job_deadline_exceeded"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
