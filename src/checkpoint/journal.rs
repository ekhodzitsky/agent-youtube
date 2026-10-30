//! Durable change records. Only the atomically published head authorizes replay.
use super::{
    Batch, Publication, State, media_artifact, validate_download_prefix, validate_publication,
    validate_scratch,
};
use crate::{Result, storage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Head {
    sequence: u64,
    bytes: u64,
    sha256: String,
}
impl Default for Head {
    fn default() -> Self {
        Self {
            sequence: 0,
            bytes: 0,
            sha256: format!("{:x}", Sha256::digest([])),
        }
    }
}
impl Head {
    fn validate(&self) -> Result<()> {
        if !valid_digest(&self.sha256)
            || (self.sequence == 0 && *self != Self::default())
            || self.sequence > self.bytes / 41
        {
            return Err("invalid checkpoint journal head".into());
        }
        Ok(())
    }
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Anchor {
    pub directory: String,
    head: Head,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Change {
    Scratch {
        names: Vec<String>,
    },
    Intents {
        entries: BTreeMap<String, Publication>,
    },
    Published {
        names: Vec<String>,
    },
    Download {
        prefix: String,
    },
    RetireDownload,
    BeginBatch {
        batch: Batch,
    },
    RetireBatch,
    ClearBatches,
    Cleanup,
}
impl Change {
    fn apply(self, state: &mut State, root: &Path) -> Result<()> {
        match self {
            Self::Scratch { names } => {
                for name in names {
                    validate_scratch(&name)?;
                    state.scratch.insert(name);
                }
            }
            Self::Intents { entries } => {
                for (name, publication) in entries {
                    validate_publication(&name, &publication.pending)?;
                    if !valid_digest(&publication.digest) {
                        return Err("invalid journal artifact digest".into());
                    }
                    if name == "media/source.audio" {
                        let prefix = state
                            .download_prefix
                            .as_deref()
                            .ok_or("missing journal download namespace")?;
                        validate_download_prefix(prefix)?;
                        if !super::download_output(&publication.pending, prefix) {
                            return Err("journal publication outside download namespace".into());
                        }
                    }
                    if state.publications.insert(name, publication).is_some() {
                        return Err("duplicate journal publication intent".into());
                    }
                }
            }
            Self::Published { names } => {
                for name in names {
                    let publication = state
                        .publications
                        .remove(&name)
                        .ok_or("journal completion has no intent")?;
                    state.artifacts.insert(name, publication.digest);
                    state.scratch.remove(&publication.pending);
                }
            }
            Self::Download { prefix } => {
                validate_download_prefix(&prefix)?;
                if state.download_prefix.replace(prefix).is_some() {
                    return Err("duplicate journal download namespace".into());
                }
            }
            Self::RetireDownload => {
                if state.retired_downloads.len() >= 2
                    || state.artifacts.contains_key("media/source.audio")
                    || state.publications.contains_key("media/source.audio")
                {
                    return Err("invalid journal acquisition retirement".into());
                }
                state.retired_downloads.push(
                    state
                        .download_prefix
                        .take()
                        .ok_or("journal retirement has no download")?,
                );
            }
            Self::BeginBatch { batch } => {
                batch.dirs(root)?;
                if state.batch.replace(batch).is_some() {
                    return Err("duplicate journal batch".into());
                }
            }
            Self::RetireBatch => state.retired_batches.push(
                state
                    .batch
                    .take()
                    .ok_or("journal retirement has no batch")?,
            ),
            Self::ClearBatches => {
                state.batch = None;
                state.retired_batches.clear();
            }
            Self::Cleanup => {
                if !state.artifacts.contains_key("transcript.json") {
                    return Err("journal cleanup has no transcript".into());
                }
                state.artifacts.retain(|name, _| !media_artifact(name));
                state.scratch.clear();
                state.download_prefix = None;
                state.retired_downloads.clear();
            }
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    sequence: u64,
    previous: String,
    change: Change,
}

pub(super) struct Journal {
    directory: PathBuf,
    file: File,
    head: Head,
    snapshot_head: Head,
    snapshot_bytes: u64,
    healthy: bool,
}

fn owned_file(path: &Path) -> Result<()> {
    storage::reject_nonregular(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(path)?.nlink() != 1 {
            return Err("refusing hard-linked checkpoint journal file".into());
        }
    }
    Ok(())
}
fn directory(root: &Path, name: &str) -> Result<PathBuf> {
    let fields: Vec<_> = name
        .strip_prefix("state.")
        .and_then(|s| s.strip_suffix(".journal"))
        .ok_or("invalid checkpoint journal namespace")?
        .split('.')
        .collect();
    if fields.len() != 3
        || fields
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("invalid checkpoint journal namespace".into());
    }
    let path = root.join(name);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("checkpoint journal must be a real directory".into());
    }
    Ok(path)
}

impl Journal {
    pub fn create(root: &Path, sequence: u64) -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let name = format!("state.{}.{stamp}.{sequence}.journal", std::process::id());
        let path = root.join(name);
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        let file = storage::private_file()
            .read(true)
            .open(path.join("events"))?;
        file.sync_all()?;
        let head = Head::default();
        storage::atomic_write(&path.join("head.json"), &serde_json::to_vec(&head)?, false)?;
        storage::sync_parent(&path)?;
        Ok(Self {
            directory: path,
            file,
            snapshot_head: head.clone(),
            head,
            snapshot_bytes: 0,
            healthy: true,
        })
    }
    pub fn load(
        root: &Path,
        anchor: &Anchor,
        snapshot_bytes: u64,
        state: &mut State,
    ) -> Result<Self> {
        let path = directory(root, &anchor.directory)?;
        for file in ["head.json", "events"] {
            owned_file(&path.join(file))?;
        }
        let head: Head = serde_json::from_str(&storage::read_text(&path.join("head.json"), 1024)?)?;
        head.validate()?;
        anchor.head.validate()?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.join("events"))?;
        if head.bytes < anchor.head.bytes
            || head.sequence < anchor.head.sequence
            || file.metadata()?.len() < head.bytes
        {
            return Err("checkpoint journal is shorter than its committed head".into());
        }
        crate::memory::require(
            (head.bytes - anchor.head.bytes).saturating_mul(16),
            &path,
            "checkpoint_replay",
        )?;
        file.seek(SeekFrom::Start(anchor.head.bytes))?;
        let mut seen = anchor.head.clone();
        while seen.bytes < head.bytes {
            let mut length = [0; 8];
            let mut digest = [0; 32];
            if head.bytes - seen.bytes < 40 {
                return Err("truncated committed journal frame".into());
            }
            file.read_exact(&mut length)?;
            file.read_exact(&mut digest)?;
            let length = u64::from_le_bytes(length);
            if length == 0 || length > head.bytes - seen.bytes - 40 {
                return Err("invalid committed journal frame length".into());
            }
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(usize::try_from(length)?)
                .map_err(|error| {
                    crate::memory::allocation_error(&path, "checkpoint_replay", error)
                })?;
            bytes.resize(usize::try_from(length)?, 0);
            file.read_exact(&mut bytes)?;
            let actual = Sha256::digest(&bytes);
            if actual[..] != digest[..] {
                return Err("checkpoint journal checksum mismatch".into());
            }
            let record: Record = serde_json::from_slice(&bytes)?;
            if record.version != 1
                || seen.sequence.checked_add(1) != Some(record.sequence)
                || record.previous != seen.sha256
            {
                return Err("checkpoint journal sequence/hash chain mismatch".into());
            }
            record.change.apply(state, root)?;
            seen = Head {
                sequence: record.sequence,
                bytes: seen.bytes + 40 + length,
                sha256: format!("{actual:x}"),
            };
        }
        if seen != head {
            return Err("checkpoint journal head does not match committed records".into());
        }
        Ok(Self {
            directory: path,
            file,
            head,
            snapshot_head: anchor.head.clone(),
            snapshot_bytes,
            healthy: true,
        })
    }
    pub fn anchor(&self) -> Result<Anchor> {
        if !self.healthy {
            return Err(crate::protocol::failure_details(
                "checkpoint_reopen_required",
                Some(true),
                "checkpoint journal requires reopen after failed persistence",
                serde_json::json!({"path":self.directory.to_string_lossy(),"preserved":true}),
            ));
        }
        Ok(Anchor {
            directory: self
                .directory
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("invalid journal directory")?
                .into(),
            head: self.head.clone(),
        })
    }
    pub fn append(&mut self, change: Change) -> Result<()> {
        self.anchor()?;
        let result = (|| -> Result<()> {
            let sequence = self
                .head
                .sequence
                .checked_add(1)
                .ok_or("journal sequence overflow")?;
            let bytes = serde_json::to_vec(&Record {
                version: 1,
                sequence,
                previous: self.head.sha256.clone(),
                change,
            })?;
            let digest = Sha256::digest(&bytes);
            let end = self
                .head
                .bytes
                .checked_add(40)
                .and_then(|n| n.checked_add(bytes.len() as u64))
                .ok_or("journal length overflow")?;
            // Only bytes beyond the durable head are uncommitted. A load validates
            // the entire committed suffix before any tail may be replaced.
            self.file.set_len(self.head.bytes)?;
            self.file.seek(SeekFrom::Start(self.head.bytes))?;
            self.file.write_all(&(bytes.len() as u64).to_le_bytes())?;
            self.file.write_all(&digest)?;
            self.file.write_all(&bytes)?;
            self.file.sync_all()?;
            let head = Head {
                sequence,
                bytes: end,
                sha256: format!("{digest:x}"),
            };
            let cursor = serde_json::to_vec(&head)?;
            storage::atomic_write(&self.directory.join("head.json"), &cursor, true)?;
            #[cfg(test)]
            super::SAVED_BYTES.fetch_add(
                (40 + bytes.len() + cursor.len()) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
            self.head = head;
            Ok(())
        })();
        if result.is_err() {
            self.healthy = false;
        }
        result
    }
    pub fn needs_snapshot(&self) -> bool {
        self.head.bytes - self.snapshot_head.bytes >= self.snapshot_bytes.max(64 * 1024)
    }
    pub fn snapshotted(&mut self, bytes: u64) {
        self.snapshot_head = self.head.clone();
        self.snapshot_bytes = bytes;
    }
    pub fn invalidate(&mut self) {
        self.healthy = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksummed_records_still_validate_paths_and_state_transitions() {
        for change in [
            Change::Scratch {
                names: vec!["../foreign".into()],
            },
            Change::Published {
                names: vec!["raw/000000.json".into()],
            },
            Change::RetireBatch,
            Change::Cleanup,
            Change::BeginBatch {
                batch: Batch {
                    namespace: "batch.1.2.3".into(),
                    indices: vec![],
                },
            },
            Change::Download {
                prefix: "../foreign".into(),
            },
        ] {
            let (root, mut checkpoint) = super::super::tests::fixture();
            checkpoint.journal.as_mut().unwrap().append(change).unwrap();
            let snapshot = fs::read(root.join("state.json")).unwrap();
            assert!(super::super::Checkpoint::load(&root).is_err());
            assert_eq!(fs::read(root.join("state.json")).unwrap(), snapshot);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn valid_frame_hash_cannot_hide_a_broken_predecessor_chain() {
        let (root, mut checkpoint) = super::super::tests::fixture();
        let journal = checkpoint.journal.as_mut().unwrap();
        let bytes = serde_json::to_vec(&Record {
            version: 1,
            sequence: 1,
            previous: "0".repeat(64),
            change: Change::Scratch {
                names: vec!["raw/.agent-youtube-1-2-3-000000.json".into()],
            },
        })
        .unwrap();
        let digest = Sha256::digest(&bytes);
        journal
            .file
            .write_all(&(bytes.len() as u64).to_le_bytes())
            .unwrap();
        journal.file.write_all(&digest).unwrap();
        journal.file.write_all(&bytes).unwrap();
        journal.file.sync_all().unwrap();
        let head = Head {
            sequence: 1,
            bytes: bytes.len() as u64 + 40,
            sha256: format!("{digest:x}"),
        };
        storage::atomic_write(
            &journal.directory.join("head.json"),
            &serde_json::to_vec(&head).unwrap(),
            true,
        )
        .unwrap();
        drop(checkpoint);
        let error = super::super::Checkpoint::load(&root).err().unwrap();
        assert!(error.to_string().contains("chain mismatch"));
        fs::remove_dir_all(root).unwrap();
    }
}
