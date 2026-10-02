use super::{string, Search};
use crate::http::{decode_base64, json, Request, Transport};
use crate::model::{Candidate, Lyric, Provider, Query, SongRef};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;

pub(crate) fn search(http: &dyn Transport, query: &Query) -> Result<Search> {
    let keywords = query.search_text();
    ensure!(!keywords.trim().is_empty(), "酷狗搜索需要关键词或歌名");
    let body = json(
        &http.send(
            Request::get("https://songsearch.kugou.com/song_search_v2")
                .query("platform", "AndroidFilter")
                .query("keyword", keywords.trim())
                .query("pagesize", 20)
                .query("page", 1)
                .query("iscorrection", 0)
                .query("hifiquality", 0)
                .query("PrivilegeFilter", 0),
        )?,
    )?;
    ensure!(
        number(&body["error_code"]) == Some(0.0),
        "酷狗搜索接口返回错误"
    );
    let rows = body["data"]["lists"]
        .as_array()
        .context("酷狗搜索缺少 data.lists")?;
    let total = number(&body["data"]["total"]).context("酷狗搜索缺少 total")?;
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    for row in rows.iter().take(20) {
        let item = candidate(row)?;
        if seen.insert(item.song.clone()) {
            candidates.push(item);
        }
    }
    Ok(Search {
        candidates,
        complete: total <= rows.len() as f64,
    })
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
}

fn candidate(row: &Value) -> Result<Candidate> {
    // Audioid and MixSongID identify different objects; neither is a file hash.
    let hash = row["FileHash"].as_str().context("酷狗结果缺少 FileHash")?;
    ensure!(
        hash.len() == 32 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
        "酷狗 FileHash 不是 32 位十六进制 hash"
    );
    let song = SongRef::new(Provider::Kugou, hash)?;
    let title = row["OriSongName"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .context("酷狗结果缺少歌名")?;
    let suffix = row["Suffix"].as_str().unwrap_or_default().trim();
    let title = if suffix.is_empty() {
        title.to_owned()
    } else {
        format!("{title} {suffix}")
    };
    let singers = row["Singers"].as_array().context("酷狗结果缺少 Singers")?;
    let artists = singers
        .iter()
        .map(|singer| {
            singer["name"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(super::kuwo::decode_name)
                .context("酷狗结果歌手名称无效")
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(!artists.is_empty(), "酷狗结果歌手为空");
    Ok(Candidate {
        song,
        title: super::kuwo::decode_name(&title),
        artists,
        album: row["AlbumName"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(super::kuwo::decode_name),
        // song_search_v2 Duration is seconds, not milliseconds.
        duration: number(&row["Duration"]).filter(|n| *n > 0.0),
    })
}

pub(crate) fn detail(_http: &dyn Transport, song: &SongRef) -> Result<Candidate> {
    ensure!(song.provider == Provider::Kugou, "歌曲引用不是酷狗来源");
    SongRef::new(Provider::Kugou, song.id.clone())?;
    bail!("酷狗该精准引用没有已验证的公开 HTTPS 元数据接口；请使用 -l '歌名 歌手' --provider kugou 查询并选择歌曲，不能凭 hash/MixSongID/歌词 ID 猜测跨来源歌曲")
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

fn artist_separator(ch: char) -> bool {
    matches!(ch, '&' | '、' | ',' | '，' | ';' | '；' | '/' | '／')
}

fn artists_match(value: &str, artists: &[String]) -> bool {
    const MAX_ARTISTS: usize = 16;
    const MAX_STATES: usize = 4096;
    const MAX_COMPARISON_BYTES: usize = 1024 * 1024;
    if artists.is_empty()
        || artists.len() > MAX_ARTISTS
        || value.len() > MAX_COMPARISON_BYTES
        || artists.iter().map(String::len).sum::<usize>() > MAX_COMPARISON_BYTES
    {
        return false;
    }
    let value = normalized(value);
    let expected = artists
        .iter()
        .map(|s| normalized(s))
        .collect::<BTreeSet<_>>();
    if expected.contains("") {
        return false;
    }
    if expected.len() == 1 {
        return expected.contains(&value);
    }
    let expected = expected.into_iter().collect::<Vec<_>>();
    let all = ((1u32 << expected.len()) - 1) as u16;
    let mut pending = vec![(0usize, 0u16)];
    let mut seen = BTreeSet::from([(0usize, 0u16)]);
    let mut comparison_bytes = 0usize;
    // Explore complete known names, not punctuation fragments. Memoized states
    // handle overlapping names without unbounded permutation work.
    while let Some((offset, used)) = pending.pop() {
        let rest = &value[offset..];
        for (index, name) in expected.iter().enumerate() {
            let bit = 1u16 << index;
            if used & bit != 0 {
                continue;
            }
            if name.len() > MAX_COMPARISON_BYTES - comparison_bytes {
                return false;
            }
            comparison_bytes += name.len();
            let Some(suffix) = rest.strip_prefix(name.as_str()) else {
                continue;
            };
            let next_used = used | bit;
            if next_used == all {
                if suffix.is_empty() {
                    return true;
                }
                continue;
            }
            let Some(separator) = suffix.chars().next().filter(|c| artist_separator(*c)) else {
                continue;
            };
            let state = (offset + name.len() + separator.len_utf8(), next_used);
            if seen.contains(&state) {
                continue;
            }
            if seen.len() >= MAX_STATES {
                return false;
            }
            seen.insert(state);
            pending.push(state);
        }
    }
    false
}

fn identity_matches(row: &Value, metadata: &Candidate, hash: &str) -> bool {
    if let Some(actual) = row["hash"].as_str().or_else(|| row["FileHash"].as_str()) {
        if !actual.eq_ignore_ascii_case(hash) {
            return false;
        }
    }
    let Some(title) = row["song"].as_str() else {
        return false;
    };
    if normalized(&super::kuwo::decode_name(title)) != normalized(&metadata.title) {
        return false;
    }
    let Some(singer) = row["singer"].as_str() else {
        return false;
    };
    if !artists_match(&super::kuwo::decode_name(singer), &metadata.artists) {
        return false;
    }
    let Some(expected_duration) = metadata.duration.filter(|n| n.is_finite() && *n > 0.0) else {
        return false;
    };
    // Legacy /search takes timelength in seconds, but candidate.duration is
    // milliseconds (verified against the same 269-second song). No heuristic
    // unit switching: a seconds-shaped result fails this millisecond contract.
    let Some(duration_ms) = number(&row["duration"]).filter(|n| *n > 0.0) else {
        return false;
    };
    (duration_ms / 1000.0 - expected_duration).abs() <= 2.0
}

pub(crate) fn lyric(
    http: &dyn Transport,
    song: &SongRef,
    metadata: Option<&Candidate>,
) -> Result<Lyric> {
    ensure!(song.provider == Provider::Kugou, "歌曲引用不是酷狗来源");
    let checked = SongRef::new(Provider::Kugou, song.id.clone())?;
    if let Some(reference) = checked.id.strip_prefix("lrc:") {
        let (id, key) = reference.split_once(':').context("酷狗歌词引用格式无效")?;
        return download(http, id, key);
    }
    ensure!(!checked.id.starts_with("mix:"), "酷狗 MixSongID 没有已验证的公开 HTTPS hash 解析接口；请使用 -l '歌名 歌手' --provider kugou 选择 FileHash");
    let metadata = metadata.context("酷狗 hash 歌词需已验证歌名、歌手和时长；请使用 -l '歌名 歌手' --provider kugou 选择歌曲，或显式 kugou:lrc:歌词ID:accesskey")?;
    ensure!(metadata.song == checked, "酷狗歌词元数据与请求 hash 不匹配");
    let duration = metadata
        .duration
        .filter(|n| n.is_finite() && *n > 0.0 && *n <= 86400.0)
        .context("酷狗歌词匹配需要有效秒数时长")?;
    ensure!(
        !metadata.title.trim().is_empty() && !metadata.artists.is_empty(),
        "酷狗歌词匹配缺少歌名或歌手"
    );
    let body = json(
        &http.send(
            Request::get("https://lyrics.kugou.com/search")
                .query("ver", 1)
                .query("man", "yes")
                .query("client", "pc")
                .query("keyword", &metadata.title)
                .query("hash", &checked.id)
                .query("timelength", duration)
                .query("lrctxt", 1),
        )?,
    )?;
    ensure!(
        number(&body["status"]) == Some(200.0),
        "酷狗歌词搜索接口返回错误"
    );
    if let Some(hash) = body["hash"].as_str() {
        ensure!(
            hash.eq_ignore_ascii_case(&checked.id),
            "酷狗歌词搜索返回 hash 不匹配"
        );
    }
    let rows = body["candidates"]
        .as_array()
        .context("酷狗歌词搜索缺少 candidates")?;
    let mut matches = BTreeSet::new();
    for row in rows {
        if !identity_matches(row, metadata, &checked.id) {
            continue;
        }
        let id = string(&row["id"]).context("酷狗歌词候选缺少 ID")?;
        let key = row["accesskey"]
            .as_str()
            .context("酷狗歌词候选缺少 accesskey")?;
        // This public lyric access key is not an account/session token. Keep
        // it bounded and canonical so error text never embeds API markup.
        let reference = SongRef::new(Provider::Kugou, format!("lrc:{id}:{key}"))?;
        matches.insert(reference.id);
    }
    if matches.is_empty() {
        return Ok(Lyric::NotFound);
    }
    if matches.len() > 1 {
        let choices = matches
            .iter()
            .map(|id| format!("lyrics-fetcher --lyrics kugou:{id}"))
            .collect::<Vec<_>>()
            .join("\n");
        bail!("酷狗返回多份身份匹配的歌词，未默认下载首条；请显式选择歌词引用（accesskey 为公开歌词下载键，非账户 token）：\n{choices}");
    }
    let reference = matches.into_iter().next().unwrap();
    let (id, key) = reference
        .strip_prefix("lrc:")
        .unwrap()
        .split_once(':')
        .unwrap();
    download(http, id, key)
}

fn download(http: &dyn Transport, id: &str, key: &str) -> Result<Lyric> {
    let body = json(
        &http.send(
            Request::get("https://lyrics.kugou.com/download")
                .query("ver", 1)
                .query("client", "pc")
                .query("id", id)
                .query("accesskey", key)
                .query("fmt", "lrc")
                .query("charset", "utf8"),
        )?,
    )?;
    ensure!(
        number(&body["status"]) == Some(200.0),
        "酷狗歌词下载接口返回错误"
    );
    ensure!(
        body["fmt"].as_str() == Some("lrc"),
        "酷狗返回非普通 LRC 格式，未处理 KRC/逐字歌词"
    );
    if let Some(actual) = string(&body["id"]) {
        ensure!(actual == id, "酷狗歌词下载 ID 不匹配");
    }
    let content = body["content"]
        .as_str()
        .context("酷狗歌词下载缺少 content")?;
    crate::lrc::parse(&decode_base64(content)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::Fake;
    use base64::Engine;
    use serde_json::json;
    const HASH: &str = "B3A52A7A958BF0AED0EBFBA2E9A818B7";
    fn metadata() -> Candidate {
        Candidate {
            song: SongRef::new(Provider::Kugou, HASH).unwrap(),
            title: "Example".into(),
            artists: vec!["Artist".into()],
            album: None,
            duration: Some(269.0),
        }
    }
    fn row(id: u64) -> Value {
        json!({"id":id.to_string(),"accesskey":"KEY123","song":"Example","singer":"Artist","duration":269792})
    }
    fn encoded(value: Value) -> Vec<u8> {
        value.to_string().into_bytes()
    }
    fn downloaded() -> Value {
        json!({"status":200,"fmt":"lrc","content":base64::engine::general_purpose::STANDARD.encode("[00:01.00]Example")})
    }

    #[test]
    fn search_keeps_hash_separate_from_audio_and_mix_ids() {
        let http = Fake::new(vec![encoded(
            json!({"error_code":0,"data":{"total":1,"lists":[{"FileHash":HASH.to_lowercase(),"Audioid":123,"MixSongID":456,"OriSongName":"Example","Suffix":"(Live)","Singers":[{"name":"Artist"}],"AlbumName":"Album","Duration":269}]}}),
        )]);
        let found = search(
            &http,
            &Query {
                keywords: "Example".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(found.complete);
        assert_eq!(found.candidates[0].song.id, HASH);
        assert_eq!(found.candidates[0].title, "Example (Live)");
        assert_eq!(found.candidates[0].duration, Some(269.0));
        let requests = http.requests.borrow();
        assert_eq!(
            requests[0].url,
            "https://songsearch.kugou.com/song_search_v2"
        );
        assert!(requests[0]
            .query
            .contains(&("platform".into(), "AndroidFilter".into())));
    }

    #[test]
    fn selects_only_identity_match_not_first_candidate() {
        let mut wrong = row(1);
        wrong["singer"] = json!("Cover Artist");
        let http = Fake::new(vec![
            encoded(json!({"status":200,"candidates":[wrong,row(2)]})),
            encoded(downloaded()),
        ]);
        let item = metadata();
        assert!(matches!(
            lyric(&http, &item.song, Some(&item)).unwrap(),
            Lyric::Synced(_)
        ));
        let requests = http.requests.borrow();
        assert!(requests[0]
            .query
            .contains(&("timelength".into(), "269".into())));
        assert!(requests[1].query.contains(&("id".into(), "2".into())));
        assert!(requests[1].query.contains(&("fmt".into(), "lrc".into())));
    }

    #[test]
    fn ambiguous_matches_list_stable_explicit_selection_without_download() {
        let http = Fake::new(vec![encoded(
            json!({"status":200,"candidates":[row(2),row(1)]}),
        )]);
        let item = metadata();
        let error = lyric(&http, &item.song, Some(&item))
            .unwrap_err()
            .to_string();
        assert!(error.contains("lyrics-fetcher --lyrics kugou:lrc:1:KEY123"));
        assert!(error.contains("lyrics-fetcher --lyrics kugou:lrc:2:KEY123"));
        assert_eq!(http.requests.borrow().len(), 1);
    }

    #[test]
    fn explicit_reference_downloads_without_search_or_metadata() {
        let http = Fake::new(vec![encoded(downloaded())]);
        let song = SongRef::new(Provider::Kugou, "lrc:123:KEY123").unwrap();
        assert!(matches!(
            lyric(&http, &song, None).unwrap(),
            Lyric::Synced(_)
        ));
        assert_eq!(http.requests.borrow().len(), 1);
        assert_eq!(
            http.requests.borrow()[0].url,
            "https://lyrics.kugou.com/download"
        );
    }

    #[test]
    fn rejects_wrong_hash_version_artist_and_time_unit() {
        let item = metadata();
        let mut wrong = row(1);
        wrong["hash"] = json!("00000000000000000000000000000000");
        assert!(!identity_matches(&wrong, &item, HASH));
        let mut wrong = row(1);
        wrong["song"] = json!("Example (Live)");
        assert!(!identity_matches(&wrong, &item, HASH));
        let mut wrong = row(1);
        wrong["duration"] = json!(269);
        assert!(!identity_matches(&wrong, &item, HASH));
        let mut wrong = row(1);
        wrong["duration"] = json!(300000);
        assert!(!identity_matches(&wrong, &item, HASH));
    }

    #[test]
    fn matches_single_full_artist_names_without_splitting_punctuation() {
        for (artist, singer, partials) in [
            (
                "AC/DC",
                " ac/dc ",
                vec!["AC", "DC", "AC & DC", "AC/DC & Guest"],
            ),
            (
                "Earth, Wind & Fire",
                "earth, wind &amp; fire",
                vec!["Earth", "Wind", "Fire", "Earth & Wind & Fire"],
            ),
        ] {
            let mut item = metadata();
            item.artists = vec![artist.into()];
            let mut correct = row(1);
            correct["singer"] = json!(singer);
            assert!(identity_matches(&correct, &item, HASH), "{artist}");
            for partial in partials {
                let mut wrong = correct.clone();
                wrong["singer"] = json!(partial);
                assert!(
                    !identity_matches(&wrong, &item, HASH),
                    "{artist}: {partial}"
                );
            }
        }
    }

    #[test]
    fn matches_collaborations_at_complete_known_artist_boundaries() {
        let mut item = metadata();
        item.artists = vec!["AC/DC".into(), "Earth, Wind & Fire".into(), "Guest".into()];
        for singer in [
            "AC/DC & Earth, Wind &amp; Fire / Guest",
            "Guest、Earth, Wind & Fire；AC/DC",
            "earth, wind & fire， ac/dc ／ guest",
            "AC / DC & Earth, Wind & Fire / Guest",
        ] {
            let mut correct = row(1);
            correct["singer"] = json!(singer);
            assert!(identity_matches(&correct, &item, HASH), "{singer}");
        }
        for singer in [
            "AC/DC",
            "AC & DC & Earth, Wind & Fire / Guest",
            "AC/DC & Earth & Wind & Fire / Guest",
            "AC/DC & Earth, Wind & Fire / Guest & Extra",
            "AC/DC & Earth, Wind & Fire / Guest/Guest",
            "AC/DC & Earth, Wind & Fire / Guesthouse",
            "AC/DC & Earth, Wind & FireGuest",
            "AC/DC & Earth, Wind & Fire / Guest/",
        ] {
            let mut wrong = row(1);
            wrong["singer"] = json!(singer);
            assert!(!identity_matches(&wrong, &item, HASH), "{singer}");
        }
    }

    #[test]
    fn matches_longer_known_artist_names_before_prefixes() {
        let mut item = metadata();
        item.artists = vec!["AC".into(), "AC/DC".into(), "Earth, Wind & Fire".into()];
        for singer in [
            "AC/DC & AC / Earth, Wind & Fire",
            "AC & Earth, Wind & Fire / AC/DC",
        ] {
            let mut correct = row(1);
            correct["singer"] = json!(singer);
            assert!(identity_matches(&correct, &item, HASH), "{singer}");
        }
    }

    #[test]
    fn matches_overlapping_artist_names_without_greedy_selection() {
        let artists = vec!["A".into(), "A&B".into(), "B".into()];
        for singer in ["A&B&A&B", "A&B&B&A", "B&A&A&B", "a&b、a／b"] {
            assert!(artists_match(singer, &artists), "{singer}");
        }
        for singer in ["A&B", "A&B&A", "A&B&A&B&B", "A&B&A&B&", "A&B&Guest"] {
            assert!(!artists_match(singer, &artists), "{singer}");
        }
        assert!(artists_match("乙／甲/乙", &["甲/乙".into(), "乙".into()]));
    }

    #[test]
    fn artist_matching_fails_closed_at_work_limits() {
        let artists = (0..16).map(|n| format!("Artist{n}")).collect::<Vec<_>>();
        assert!(artists_match(&artists.join("&"), &artists));
        assert!(!artists_match(
            &format!("{}&Extra", artists.join("&")),
            &artists
        ));
        let too_many = (0..17).map(|n| format!("Artist{n}")).collect::<Vec<_>>();
        assert!(!artists_match(&too_many.join("&"), &too_many));
        assert!(!artists_match(&"a".repeat(1024 * 1024 + 1), &["a".into()]));
        assert!(!artists_match("a", &["a".repeat(1024 * 1024 + 1)]));
        // Many whole-name segmentations with no valid terminal match exhaust
        // the memoized-state budget instead of exploring every permutation.
        let overlapping = (1..=16).map(|n| vec!["a"; n].join("&")).collect::<Vec<_>>();
        let impossible = format!("{}&x", vec!["a"; 136].join("&"));
        assert!(!artists_match(&impossible, &overlapping));
        assert!(!artists_match("A&B", &["A".into(), "".into()]));
    }

    #[test]
    fn full_artist_matches_still_require_hash_version_and_duration() {
        let mut item = metadata();
        item.artists = vec!["AC/DC".into(), "Earth, Wind & Fire".into()];
        let mut correct = row(1);
        correct["singer"] = json!("AC/DC & Earth, Wind & Fire");
        assert!(identity_matches(&correct, &item, HASH));
        for (key, value) in [
            ("hash", json!("00000000000000000000000000000000")),
            ("song", json!("Example (Live)")),
            ("duration", json!(269)),
            ("duration", json!(300000)),
        ] {
            let mut wrong = correct.clone();
            wrong[key] = value;
            assert!(!identity_matches(&wrong, &item, HASH), "{key}");
        }
        item.duration = None;
        assert!(!identity_matches(&correct, &item, HASH));
    }

    #[test]
    fn downloads_lyrics_for_full_artist_identity_not_partial_names() {
        for (artists, singer, partial) in [
            (vec!["AC/DC"], "AC/DC", "AC"),
            (
                vec!["Earth, Wind & Fire"],
                "Earth, Wind &amp; Fire",
                "Earth",
            ),
            (
                vec!["AC/DC", "Earth, Wind & Fire"],
                "Earth, Wind & Fire / AC/DC",
                "Earth, Wind & Fire / AC",
            ),
        ] {
            let mut item = metadata();
            item.artists = artists.into_iter().map(str::to_owned).collect();
            let mut wrong = row(1);
            wrong["singer"] = json!(partial);
            let mut correct = row(2);
            correct["singer"] = json!(singer);
            let http = Fake::new(vec![
                encoded(json!({"status":200,"candidates":[wrong,correct]})),
                encoded(downloaded()),
            ]);
            assert!(matches!(
                lyric(&http, &item.song, Some(&item)).unwrap(),
                Lyric::Synced(_)
            ));
            let requests = http.requests.borrow();
            assert_eq!(requests.len(), 2);
            assert!(requests[1].query.contains(&("id".into(), "2".into())));
        }
    }

    #[test]
    fn candidate_entities_are_decoded_once_not_recursively() {
        let mut item = metadata();
        item.title = "A &lt; B".into();
        let mut correct = row(1);
        correct["song"] = json!("A &amp;lt; B");
        assert!(identity_matches(&correct, &item, HASH));
        correct["song"] = json!("A &lt; B");
        assert!(!identity_matches(&correct, &item, HASH));
    }

    #[test]
    fn direct_unresolved_refs_fail_without_network() {
        let http = Fake::new(vec![]);
        let item = metadata();
        assert!(lyric(&http, &item.song, None).is_err());
        assert!(detail(&http, &item.song).is_err());
        assert!(lyric(
            &http,
            &SongRef::new(Provider::Kugou, "mix:123").unwrap(),
            None
        )
        .is_err());
        assert!(http.requests.borrow().is_empty());
    }

    #[test]
    fn rejects_krc_base64_errors_and_oversized_output() {
        let mut body = downloaded();
        body["fmt"] = json!("krc");
        assert!(download(&Fake::new(vec![encoded(body)]), "1", "KEY").is_err());
        let mut body = downloaded();
        body["content"] = json!("%%%%");
        assert!(download(&Fake::new(vec![encoded(body)]), "1", "KEY").is_err());
        let mut body = downloaded();
        body["content"] = json!(base64::engine::general_purpose::STANDARD.encode(vec![
            b'a';
            crate::http::MAX_LYRIC_SIZE
                + 1
        ]));
        assert!(download(&Fake::new(vec![encoded(body)]), "1", "KEY").is_err());
    }
}
