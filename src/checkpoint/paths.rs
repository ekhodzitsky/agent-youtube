//! Pure ownership and path validation for checkpoint artifacts.
use super::*;

pub(super) fn validate_publication(name: &str, pending: &str) -> Result<()> {
    validate_relative(name)?;
    validate_relative(pending)?;
    let destination = Path::new(name);
    let scratch = Path::new(pending);
    if destination.parent() != scratch.parent() {
        return Err("publication scratch must be adjacent to its artifact".into());
    }
    let allowed = matches!(
        name,
        "source.json"
            | "download.json"
            | "recognition.json"
            | "coverage.json"
            | "manifest.json"
            | "transcript.json"
            | "media/video.mkv"
            | "media/source.audio"
            | "media/audio.wav"
            | "media/audio.json"
    ) || ["chunks/", "raw/"].iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|tail| {
            let (stem, extension) = tail.split_once('.').unwrap_or_default();
            chunk_index(stem)
                && if *prefix == "chunks/" {
                    matches!(extension, "wav" | "resume.json")
                } else {
                    extension == "json"
                }
        })
    });
    if !allowed {
        return Err("unsupported publication artifact".into());
    }
    if name == "media/source.audio"
        && pending.rsplit_once('.').is_some_and(|(prefix, _)| {
            validate_download_prefix(prefix).is_ok() && download_output(pending, prefix)
        })
    {
        return Ok(());
    }
    if name == "media/video.mkv" && pending == "media/download.mkv" {
        return Ok(());
    }
    let filename = scratch
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid scratch filename")?;
    let Some(rest) = filename.strip_prefix(".agent-youtube-") else {
        return Err("publication scratch lacks ownership prefix".into());
    };
    let fields: Vec<_> = rest.splitn(4, '-').collect();
    if fields.len() != 4
        || fields[..3]
            .iter()
            .any(|field| field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("invalid scratch ownership identity".into());
    }
    let expected = destination
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid artifact filename")?;
    let partial = if let Some((stem, ext)) = expected.rsplit_once('.') {
        format!("{stem}.partial.{ext}")
    } else {
        expected.to_owned()
    };
    if fields[3] != expected && fields[3] != partial {
        return Err("scratch filename does not match publication artifact".into());
    }
    Ok(())
}
pub(super) fn validate_relative(name: &str) -> Result<()> {
    if name.is_empty()
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("invalid checkpoint artifact path".into());
    }
    Ok(())
}

pub(super) fn chunk_index(stem: &str) -> bool {
    stem.len() >= 6 && stem.bytes().all(|b| b.is_ascii_digit()) && stem.parse::<usize>().is_ok()
}

pub(super) fn media_artifact(name: &str) -> bool {
    matches!(
        name,
        "media/source.audio" | "media/video.mkv" | "media/audio.wav" | "media/audio.json"
    ) || name
        .strip_prefix("chunks/")
        .and_then(|n| {
            n.strip_suffix(".wav")
                .or_else(|| n.strip_suffix(".resume.json"))
        })
        .is_some_and(chunk_index)
}
pub(super) fn validate_download_prefix(name: &str) -> Result<()> {
    let fields: Vec<_> = name
        .strip_prefix("media/download.")
        .ok_or("invalid download namespace")?
        .split('.')
        .collect();
    if fields.len() != 3
        || fields
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("invalid download namespace".into());
    }
    Ok(())
}
pub(super) fn download_output(name: &str, prefix: &str) -> bool {
    name.strip_prefix(&format!("{prefix}.")).is_some_and(|ext| {
        !ext.is_empty() && ext.len() <= 16 && ext.bytes().all(|b| b.is_ascii_alphanumeric())
    })
}
pub(super) fn validate_scratch(name: &str) -> Result<()> {
    validate_relative(name)?;
    let path = Path::new(name);
    let parent = path.parent().unwrap_or(Path::new(""));
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("invalid scratch path")?;
    let fields: Vec<_> = filename
        .strip_prefix(".agent-youtube-")
        .ok_or("invalid scratch ownership")?
        .splitn(4, '-')
        .collect();
    if fields.len() != 4
        || fields[..3]
            .iter()
            .any(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("invalid scratch ownership".into());
    }
    let suffix = fields[3];
    if parent == Path::new("media") && suffix == "download-path.txt" {
        return Ok(());
    }
    let target = suffix.replace(".partial.", ".");
    let target = parent.join(target);
    validate_publication(target.to_str().ok_or("invalid scratch path")?, name)
}
