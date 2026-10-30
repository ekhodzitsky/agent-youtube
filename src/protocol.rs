//! Versioned public CLI records. Video-derived content is untrusted evidence.
use crate::Result;
use serde_json::{Value, json};
use std::{
    fmt,
    io::{self, Write},
};

#[derive(Debug)]
pub struct Failure {
    pub code: &'static str,
    pub retryable: Option<bool>,
    message: String,
    details: Value,
}
impl Failure {
    pub fn http_status(&self) -> Option<u64> {
        self.details["http_status"].as_u64()
    }
    pub fn details(&self) -> &Value {
        &self.details
    }
    pub fn retry_after(&self) -> Option<&str> {
        self.details["retry_after"].as_str()
    }
    pub fn retry_after_seconds(&self) -> Option<u64> {
        self.details["retry_after_seconds"].as_u64()
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
pub(crate) fn with_attempts(
    mut error: Box<dyn std::error::Error + Send + Sync>,
    attempts: u32,
) -> Box<dyn std::error::Error + Send + Sync> {
    if let Some(failure) = error.downcast_mut::<Failure>() {
        if !failure.details.is_object() {
            failure.details = json!({});
        }
        failure.details["attempts"] = json!(attempts);
        failure.details["retries_exhausted"] = json!(true);
    }
    error
}
pub fn failure(
    code: &'static str,
    retryable: Option<bool>,
    message: impl ToString,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(Failure {
        code,
        retryable,
        message: message.to_string(),
        details: Value::Null,
    })
}
pub fn failure_details(
    code: &'static str,
    retryable: Option<bool>,
    message: impl ToString,
    details: Value,
) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(Failure {
        code,
        retryable,
        message: message.to_string(),
        details,
    })
}
pub fn context(
    error: Box<dyn std::error::Error + Send + Sync>,
    code: &'static str,
) -> Box<dyn std::error::Error + Send + Sync> {
    if error.is::<Failure>() {
        error
    } else {
        failure(code, (code == "invalid_arguments").then_some(false), error)
    }
}
pub fn event(message: impl fmt::Display) {
    progress("diagnostic", message, json!({}));
}
pub(crate) fn progress(stage: &str, message: impl fmt::Display, fields: Value) {
    let mut record =
        json!({"schema_version":1,"event":"progress","stage":stage,"message":message.to_string()});
    if let (Some(record), Some(fields)) = (record.as_object_mut(), fields.as_object()) {
        for (key, value) in fields {
            if !record.contains_key(key) {
                record.insert(key.clone(), value.clone());
            }
        }
    }
    let _ = writeln!(io::stderr().lock(), "{}", record);
}
pub fn success(command: &str, result: Value) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(
        &mut out,
        &json!({"schema_version":1,"ok":true,"command":command,"result":result}),
    )?;
    writeln!(out)?;
    Ok(())
}
fn error_record(error: &(dyn std::error::Error + 'static), cancelled: bool) -> Value {
    let typed = error.downcast_ref::<Failure>();
    let cancelled = cancelled
        && !typed.is_some_and(|e| {
            matches!(
                e.code,
                "process_teardown_failed" | "web_runtime_teardown_failed" | "job_deadline_exceeded"
            )
        });
    let code = if cancelled {
        "interrupted"
    } else {
        typed.map_or("operation_failed", |e| e.code)
    };
    let retryable = if cancelled {
        Some(true)
    } else {
        typed.and_then(|e| e.retryable)
    };
    json!({"schema_version":1,"ok":false,"error":{"code":code,"retryable":retryable,"message":error.to_string(),"details":typed.map(|e| &e.details)}})
}
pub fn error(error: &(dyn std::error::Error + 'static), cancelled: bool) {
    let record = error_record(error, cancelled);
    let _ = writeln!(io::stdout().lock(), "{record}");
    let _ = writeln!(
        io::stderr().lock(),
        "{}",
        json!({"schema_version":1,"event":"error","error":record["error"]})
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_does_not_hide_unverified_teardown() {
        let failure = failure_details(
            "process_teardown_failed",
            None,
            "worker may still be running",
            json!({"group_stopped":false}),
        );
        let record = error_record(failure.as_ref(), true);
        assert_eq!(record["error"]["code"], "process_teardown_failed");
        assert_eq!(record["error"]["retryable"], Value::Null);
        assert_eq!(record["error"]["details"]["group_stopped"], false);
        let web = super::failure(
            "web_runtime_teardown_failed",
            None,
            "worker stop unverified",
        );
        assert_eq!(
            error_record(web.as_ref(), true)["error"]["code"],
            "web_runtime_teardown_failed"
        );
        assert_eq!(
            error_record(web.as_ref(), true)["error"]["retryable"],
            Value::Null
        );
        assert_eq!(
            error_record(&std::io::Error::other("cancelled"), true)["error"]["code"],
            "interrupted"
        );
    }

    #[test]
    fn low_disk_space_has_machine_readable_capacity_details() {
        let error = crate::storage::DiskSpace {
            available_bytes: 4096,
            allocation_unit: 4096,
            available_files: None,
        }
        .require(std::path::Path::new("chunks"), 8192)
        .unwrap_err();
        let failure = error.downcast_ref::<Failure>().unwrap();
        assert_eq!(failure.code, "insufficient_disk_space");
        assert_eq!(failure.retryable, Some(true));
        assert_eq!(
            failure.details,
            json!({"path":"chunks","required_bytes":8192,"available_bytes":4096,"allocation_unit_bytes":4096,"stage":"prepare_chunks"})
        );
    }
    #[cfg(unix)]
    #[test]
    fn decode_capacity_and_late_quota_failures_keep_typed_stage_details() {
        let path = std::path::Path::new("chunks");
        let error = crate::storage::DiskSpace {
            available_bytes: 0,
            allocation_unit: 4096,
            available_files: None,
        }
        .require_at(path, 4096, "decode_audio")
        .unwrap_err();
        assert_eq!(
            error.downcast_ref::<Failure>().unwrap().details["stage"],
            "decode_audio"
        );
        for code in [libc::ENOSPC, libc::EDQUOT] {
            let error = crate::storage::audio_write_error(
                Box::new(ryf::WavError::from(std::io::Error::from_raw_os_error(code))),
                path,
                "decode_audio",
            );
            let failure = error.downcast_ref::<Failure>().unwrap();
            assert_eq!(failure.code, "insufficient_disk_space");
            assert_eq!(failure.retryable, Some(true));
            assert_eq!(failure.details["stage"], "decode_audio");
        }
        let error = crate::storage::audio_write_error(
            std::io::Error::from_raw_os_error(libc::EIO).into(),
            path,
            "decode_audio",
        );
        assert!(!error.is::<Failure>());
    }
}
