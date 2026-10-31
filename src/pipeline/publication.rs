//! Recovery of durable publication is independent of the acquisition retry deadline.
use super::*;

impl Pipeline {
    pub(super) fn recover_published(&self, mut checkpoint: Checkpoint) -> Result<Outcome> {
        let output = self.options.work_dir.join("transcript.json");
        crate::job::save(&self.options)?;
        checkpoint.recover()?;
        if !checkpoint.completed("transcript.json")? {
            return Err("missing completed transcript".into());
        }
        let header =
            transcript::read_streaming(std::io::BufReader::new(fs::File::open(&output)?), |_| {
                if self.runner.cancelled.load(Ordering::SeqCst) {
                    Err("interrupted; run the same command to resume".into())
                } else {
                    Ok(())
                }
            })?;
        if header.range.map(|r| r.requested) != self.options.range {
            return Err("published range differs from invocation".into());
        }
        checkpoint.validate_duration(header.duration)?;
        let manifest = checkpoint.ensure_manifest(&self.runner.cancelled)?;
        checkpoint.cleanup_media().map_err(|e| crate::protocol::failure_details("cleanup_pending", Some(true), e, serde_json::json!({"transcript":output,"manifest":manifest,"transcript_published":true,"cleanup_complete":false})))?;
        crate::protocol::progress(
            "complete",
            "complete: verified existing transcript; no recognition repeated; temporary media removed",
            serde_json::json!({"reused":true,"cleanup_complete":true}),
        );
        Ok(Outcome {
            range: header.range,
            transcript: output,
            manifest,
            reused: true,
        })
    }
}
