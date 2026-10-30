//! Author-published description timestamps; never infer chapters from audio.
use super::{Downloader, WEB_USER_AGENT, embedded_json};
use crate::{Result, protocol};
use serde_json::{Value, json};
use std::sync::atomic::AtomicBool;

const DESCRIPTION_LIMIT: usize = 64 * 1024;
const CHAPTER_LIMIT: usize = 2048;
const TITLE_LIMIT: usize = 4096;

impl Downloader {
    /// Fetch one watch page without selecting media, minting tokens or running ASR.
    pub fn chapters(&self, url: &str, cancelled: &AtomicBool) -> Result<Value> {
        let id = crate::pipeline::video_id(url)
            .map_err(|error| protocol::context(error, "invalid_arguments"))?;
        let (page, _) = self.watch_page(&id, &self.http.origin, WEB_USER_AGENT, cancelled)?;
        let player = embedded_json(&page, "ytInitialPlayerResponse", "playabilityStatus")
            .map_err(|_| invalid_metadata())?;
        extract(&player, &id)
    }
}

fn invalid_metadata() -> Box<dyn std::error::Error + Send + Sync> {
    protocol::failure(
        "youtube_metadata_invalid",
        Some(false),
        "watch page lacks valid video metadata",
    )
}

fn invalid_chapters() -> Box<dyn std::error::Error + Send + Sync> {
    protocol::failure(
        "youtube_chapters_invalid",
        Some(false),
        "description timestamps must have titles, increase strictly and lie within the recording",
    )
}

fn extract(player: &Value, id: &str) -> Result<Value> {
    if player["playabilityStatus"]["status"] != "OK" {
        let bot = player["playabilityStatus"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.to_ascii_lowercase().contains("bot"));
        return Err(protocol::failure(
            if bot {
                "youtube_bot_check"
            } else {
                "youtube_source_unavailable"
            },
            Some(bot),
            "video metadata is unavailable; no chapter absence is inferred",
        ));
    }
    let details = &player["videoDetails"];
    if details["videoId"].as_str() != Some(id) {
        return Err(invalid_metadata());
    }
    if details["isLive"].as_bool() == Some(true)
        || player
            .pointer("/microformat/playerMicroformatRenderer/liveBroadcastDetails/isLiveNow")
            .and_then(Value::as_bool)
            == Some(true)
    {
        return Err(protocol::failure(
            "youtube_live_unsupported",
            Some(false),
            "use a completed recording",
        ));
    }
    let duration = details["lengthSeconds"]
        .as_str()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(invalid_metadata)?;
    // An absent field is not evidence that the author supplied no timestamps.
    let description = details["shortDescription"]
        .as_str()
        .ok_or_else(invalid_metadata)?;
    if description.len() > DESCRIPTION_LIMIT {
        return Err(protocol::failure(
            "resource_limit",
            Some(false),
            "video description exceeds the chapter extraction limit",
        ));
    }
    let mut chapters = Vec::new();
    let mut previous = None;
    for line in description.lines() {
        // Permit decorative bullets, emoji and opening brackets before a timestamp.
        let line = line.trim_start_matches(|c: char| !c.is_alphanumeric());
        let end = line
            .find(|c: char| !c.is_ascii_digit() && c != ':')
            .unwrap_or(line.len());
        let stamp = &line[..end];
        let Some(start) = timestamp(stamp)? else {
            continue;
        };
        let tail = &line[end..];
        if tail.starts_with(|c: char| c.is_alphanumeric()) {
            continue;
        }
        let title = tail
            .trim_start_matches(|c: char| c.is_whitespace() || ")]-–—:|".contains(c))
            .trim_end();
        if title.is_empty()
            || title.len() > TITLE_LIMIT
            || start >= duration
            || previous.is_some_and(|value| start <= value)
        {
            return Err(invalid_chapters());
        }
        if chapters.len() == CHAPTER_LIMIT {
            return Err(protocol::failure(
                "resource_limit",
                Some(false),
                "too many published timestamps",
            ));
        }
        chapters.push(json!({"start_seconds":start,"title":title}));
        previous = Some(start);
    }
    Ok(
        json!({"video_id":id,"title":details["title"].as_str(),"duration_seconds":duration,
        "source":"video_description","status":if chapters.is_empty() {"not_found"} else {"available"},"chapters":chapters}),
    )
}

fn timestamp(value: &str) -> Result<Option<u64>> {
    let parts: Vec<_> = value.split(':').collect();
    if !(2..=3).contains(&parts.len())
        || parts[0].is_empty()
        || parts
            .iter()
            .any(|part| !part.bytes().all(|byte| byte.is_ascii_digit()))
        || parts[1..].iter().any(|part| part.len() != 2)
    {
        return Ok(None);
    }
    let mut total = 0_u64;
    for (index, part) in parts.iter().enumerate() {
        let number = part.parse::<u64>().map_err(|_| invalid_chapters())?;
        if index > 0 && number >= 60 {
            return Err(invalid_chapters());
        }
        total = total
            .checked_mul(60)
            .and_then(|value| value.checked_add(number))
            .ok_or_else(invalid_chapters)?;
    }
    Ok(Some(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(description: &str) -> Value {
        json!({"playabilityStatus":{"status":"OK"},"videoDetails":{
            "videoId":"a2hbJlPxj4M","title":"Author title","lengthSeconds":"20000","shortDescription":description}})
    }

    #[test]
    fn description_shapes_preserve_titles_and_multi_hour_timestamps() {
        let value = extract(&player("Contents\n⌨️ (0:00) Introduction\n- 1:45 – Setup\n[4:20:43] Интерпретатор\nhttps://example.org"), "a2hbJlPxj4M").unwrap();
        assert_eq!(value["status"], "available");
        assert_eq!(
            value["chapters"],
            json!([
            {"start_seconds":0,"title":"Introduction"},
            {"start_seconds":105,"title":"Setup"},
            {"start_seconds":15643,"title":"Интерпретатор"}])
        );
        assert_eq!(timestamp("90:00").unwrap(), Some(5400));
    }

    #[test]
    fn absence_is_not_confused_with_missing_metadata_or_auto_chapters() {
        let mut page = player("No author timestamps. Aspect ratio 16:9.");
        page["playerOverlays"] = json!({"chapterRenderer":{"title":{"simpleText":"Automatic chapter"},"timeRangeStartMillis":0}});
        let value = extract(&page, "a2hbJlPxj4M").unwrap();
        assert_eq!(value["status"], "not_found");
        assert_eq!(value["chapters"], json!([]));
        page["videoDetails"]
            .as_object_mut()
            .unwrap()
            .remove("shortDescription");
        assert!(extract(&page, "a2hbJlPxj4M").is_err());
    }

    #[test]
    fn invalid_or_ambiguous_lists_are_never_silently_reordered_or_truncated() {
        for description in [
            "0:00 Intro\n0:00 Duplicate",
            "1:00 Later\n0:00 Earlier",
            "0:99 Bad",
            "0:00",
            "5:33:20 End",
            "999999999999999999999:00 Overflow",
        ] {
            let error = extract(&player(description), "a2hbJlPxj4M").unwrap_err();
            assert_eq!(
                error.downcast_ref::<protocol::Failure>().unwrap().code,
                "youtube_chapters_invalid"
            );
        }
        assert!(extract(&player("0:00 Intro"), "different01").is_err());
        let mut live = player("0:00 Intro");
        live["videoDetails"]["isLive"] = json!(true);
        assert!(extract(&live, "a2hbJlPxj4M").is_err());
    }

    #[test]
    fn oversized_descriptions_and_lists_fail_and_denial_does_not_echo_page_data() {
        for description in [
            "x".repeat(DESCRIPTION_LIMIT + 1),
            (0..=CHAPTER_LIMIT)
                .map(|n| format!("{}:{:02} Item\n", n / 60, n % 60))
                .collect(),
        ] {
            let error = extract(&player(&description), "a2hbJlPxj4M").unwrap_err();
            assert_eq!(
                error.downcast_ref::<protocol::Failure>().unwrap().code,
                "resource_limit"
            );
        }
        let mut denied = player("0:00 Intro");
        denied["playabilityStatus"] =
            json!({"status":"LOGIN_REQUIRED","reason":"bot check secret-session"});
        let error = extract(&denied, "a2hbJlPxj4M").unwrap_err();
        assert_eq!(
            error.downcast_ref::<protocol::Failure>().unwrap().code,
            "youtube_bot_check"
        );
        assert!(!error.to_string().contains("secret-session"));
    }
}
