use super::{string, Search};
use crate::http::{json, Request, Transport, MAX_LYRIC_SIZE};
use crate::model::{Candidate, Lyric, Provider, Query, SongRef};
use anyhow::{ensure, Context, Result};
use serde_json::Value;

const SEARCH: &str = "https://lrclib.net/api/search";

pub(crate) fn search(http: &dyn Transport, query: &Query) -> Result<Search> {
    let mut request = Request::get(SEARCH);
    // LRCLIB's q overrides structured fields: never send both forms.
    if !query.keywords.trim().is_empty() {
        request = request.query("q", query.keywords.trim());
    } else {
        let title = query.title.as_deref().unwrap_or_default().trim();
        ensure!(!title.is_empty(), "LRCLIB 搜索需要关键词或歌名");
        request = request.query("track_name", title);
        if !query.artists.is_empty() {
            request = request.query("artist_name", query.artists.join(" "));
        }
        if let Some(album) = query.album.as_deref().filter(|a| !a.trim().is_empty()) {
            request = request.query("album_name", album.trim());
        }
    }
    let body = json(&http.send(request)?)?;
    let rows = body.as_array().context("LRCLIB 搜索结果不是数组")?;
    let candidates = rows.iter().take(20).map(candidate).collect::<Result<_>>()?;
    // This endpoint has no pagination and returns at most twenty matches.
    Ok(Search {
        candidates,
        complete: rows.len() < 20,
    })
}

fn candidate(row: &Value) -> Result<Candidate> {
    let id = string(&row["id"]).context("LRCLIB 结果缺少 ID")?;
    let title = row["trackName"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .context("LRCLIB 结果缺少歌名")?
        .to_owned();
    let artist = row["artistName"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .context("LRCLIB 结果缺少歌手")?
        .to_owned();
    let duration = row["duration"]
        .as_f64()
        .filter(|n| n.is_finite() && *n > 0.0);
    Ok(Candidate {
        song: SongRef::new(Provider::Lrclib, id)?,
        title,
        artists: vec![artist],
        album: row["albumName"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned),
        duration,
    })
}

fn get(http: &dyn Transport, song: &SongRef) -> Result<Value> {
    ensure!(
        song.provider == Provider::Lrclib,
        "歌曲引用不是 LRCLIB 来源"
    );
    let checked = SongRef::new(Provider::Lrclib, song.id.clone())?;
    let body = json(&http.send(Request::get(&format!(
        "https://lrclib.net/api/get/{}",
        checked.id
    )))?)?;
    let actual = string(&body["id"]).context("LRCLIB 返回缺少歌曲 ID")?;
    ensure!(
        actual.parse::<u64>()? == checked.id.parse::<u64>()?,
        "LRCLIB 返回歌曲 ID 不匹配"
    );
    Ok(body)
}

pub(crate) fn detail(http: &dyn Transport, song: &SongRef) -> Result<Candidate> {
    candidate(&get(http, song)?)
}

pub(crate) fn lyric(
    http: &dyn Transport,
    song: &SongRef,
    expected: Option<&Candidate>,
) -> Result<Lyric> {
    if let Some(expected) = expected {
        let reference = SongRef::new(Provider::Lrclib, song.id.clone())?;
        let candidate_ref = SongRef::new(expected.song.provider, expected.song.id.clone())?;
        ensure!(
            candidate_ref.provider == Provider::Lrclib
                && candidate_ref.id.parse::<u64>()? == reference.id.parse::<u64>()?,
            "LRCLIB 候选歌曲 ID 不匹配"
        );
    }
    let body = get(http, song)?;
    if let Some(expected) = expected {
        let actual = candidate(&body)?;
        ensure!(
            actual.title == expected.title
                && actual.artists == expected.artists
                && actual.album == expected.album
                && actual.duration == expected.duration,
            "LRCLIB 返回元信息与搜索候选不一致，拒绝使用可能过期的候选"
        );
    }
    for field in ["syncedLyrics", "plainLyrics"] {
        ensure!(
            body[field].is_null() || body[field].is_string(),
            "LRCLIB {field} 类型无效"
        );
        if let Some(text) = body[field].as_str() {
            ensure!(text.len() <= MAX_LYRIC_SIZE, "LRCLIB 歌词超出大小限制");
        }
    }
    ensure!(
        body["instrumental"].is_null() || body["instrumental"].is_boolean(),
        "LRCLIB instrumental 类型无效"
    );
    if body["instrumental"].as_bool().unwrap_or(false) {
        return Ok(Lyric::Instrumental);
    }
    if let Some(text) = body["syncedLyrics"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
    {
        ensure!(text.len() <= MAX_LYRIC_SIZE, "LRCLIB 歌词超出大小限制");
        return crate::lrc::parse(text);
    }
    if let Some(text) = body["plainLyrics"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
    {
        ensure!(text.len() <= MAX_LYRIC_SIZE, "LRCLIB 歌词超出大小限制");
        return Ok(Lyric::PlainOnly);
    }
    Ok(Lyric::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::Fake;
    use serde_json::json;

    fn row(id: u64) -> Value {
        json!({"id":id,"trackName":"Example","artistName":"Artist","albumName":"Album",
               "duration":180.5,"instrumental":false,"syncedLyrics":"[00:01.00]Example","plainLyrics":"Example"})
    }
    fn fake(value: Value) -> Fake {
        Fake::new(vec![value.to_string().into_bytes()])
    }
    fn song() -> SongRef {
        SongRef::new(Provider::Lrclib, "12").unwrap()
    }

    #[test]
    fn structured_search_and_seconds() {
        let http = fake(json!([row(12)]));
        let query = Query {
            title: Some("Example".into()),
            artists: vec!["Artist".into()],
            album: Some("Album".into()),
            ..Default::default()
        };
        let found = search(&http, &query).unwrap();
        assert!(found.complete);
        assert_eq!(found.candidates[0].duration, Some(180.5));
        let requests = http.requests.borrow();
        assert_eq!(requests[0].url, SEARCH);
        assert_eq!(
            requests[0].query,
            vec![
                ("track_name".into(), "Example".into()),
                ("artist_name".into(), "Artist".into()),
                ("album_name".into(), "Album".into())
            ]
        );
    }

    #[test]
    fn free_text_overrides_structured_query() {
        let http = fake(json!([]));
        let query = Query {
            keywords: "anything".into(),
            title: Some("ignored".into()),
            artists: vec!["ignored".into()],
            ..Default::default()
        };
        assert!(search(&http, &query).unwrap().complete);
        assert_eq!(
            http.requests.borrow()[0].query,
            vec![("q".into(), "anything".into())]
        );
    }

    #[test]
    fn full_search_is_incomplete_even_without_next_page() {
        let http = fake(Value::Array((1..=20).map(row).collect()));
        let found = search(
            &http,
            &Query {
                keywords: "Example".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(found.candidates.len(), 20);
        assert!(!found.complete);
    }

    #[test]
    fn rejects_detail_and_lyric_id_mismatch() {
        assert!(detail(&fake(row(13)), &song()).is_err());
        assert!(lyric(&fake(row(13)), &song(), None).is_err());
    }

    #[test]
    fn returns_only_synced_lyrics_with_one_get() {
        let http = fake(row(12));
        assert!(matches!(
            lyric(&http, &song(), None).unwrap(),
            Lyric::Synced(_)
        ));
        let requests = http.requests.borrow();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url, "https://lrclib.net/api/get/12");
    }

    #[test]
    fn distinguishes_plain_instrumental_and_missing() {
        let mut value = row(12);
        value["syncedLyrics"] = Value::Null;
        assert_eq!(
            lyric(&fake(value.clone()), &song(), None).unwrap(),
            Lyric::PlainOnly
        );
        value["instrumental"] = json!(true);
        assert_eq!(
            lyric(&fake(value.clone()), &song(), None).unwrap(),
            Lyric::Instrumental
        );
        value["instrumental"] = json!(false);
        value["plainLyrics"] = Value::Null;
        assert_eq!(lyric(&fake(value), &song(), None).unwrap(), Lyric::NotFound);
    }

    #[test]
    fn rejects_bad_lyric_types_instead_of_silent_not_found() {
        for field in ["syncedLyrics", "plainLyrics", "instrumental"] {
            let mut value = row(12);
            value[field] = json!({"invalid":"type"});
            assert!(lyric(&fake(value), &song(), None).is_err(), "{field}");
        }
        for field in ["syncedLyrics", "plainLyrics"] {
            let mut value = row(12);
            value[field] = json!(false);
            assert!(lyric(&fake(value), &song(), None).is_err(), "{field}");
        }
    }

    #[test]
    fn selected_candidate_requires_consistent_identity_and_metadata() {
        let expected = candidate(&row(12)).unwrap();
        assert!(matches!(
            lyric(&fake(row(12)), &song(), Some(&expected)).unwrap(),
            Lyric::Synced(_)
        ));
        for (field, value) in [
            ("trackName", json!("Example (Live)")),
            ("artistName", json!("Other Artist")),
            ("albumName", json!("Other Album")),
            ("duration", json!(120.0)),
        ] {
            let mut changed = row(12);
            changed[field] = value;
            assert!(
                lyric(&fake(changed), &song(), Some(&expected)).is_err(),
                "{field}"
            );
        }
        let wrong = candidate(&row(13)).unwrap();
        let http = Fake::new(vec![]);
        assert!(lyric(&http, &song(), Some(&wrong)).is_err());
        assert!(http.requests.borrow().is_empty());
    }

    #[test]
    fn rejects_oversized_lyrics_and_malformed_search() {
        let mut value = row(12);
        value["syncedLyrics"] = json!("x".repeat(MAX_LYRIC_SIZE + 1));
        assert!(lyric(&fake(value), &song(), None).is_err());
        assert!(search(
            &fake(json!({"error":"busy"})),
            &Query {
                keywords: "Example".into(),
                ..Default::default()
            }
        )
        .is_err());
    }
}
