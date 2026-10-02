use super::{string, Search};
use crate::http::{json, Request, Transport, MAX_LYRIC_SIZE};
use crate::model::{Candidate, Lyric, Provider, Query, SongRef};
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write;

pub(crate) fn search(http: &dyn Transport, query: &Query) -> Result<Search> {
    let keywords = query.search_text();
    ensure!(!keywords.trim().is_empty(), "酷我搜索需要关键词或歌名");
    let response = http.send(
        Request::get("https://search.kuwo.cn/r.s")
            .query("client", "kt")
            .query("all", keywords.trim())
            .query("pn", 0)
            .query("rn", 20)
            .query("ft", "music")
            .query("rformat", "json")
            .query("encoding", "utf8")
            .query("newver", 1)
            .query("cluster", 0)
            .query("strategy", 2012)
            .query("vermerge", 1)
            .query("mobi", 1),
    )?;
    let body = search_json(&response)?;
    let total = number(&body["TOTAL"]).context("酷我搜索响应缺少 TOTAL")?;
    ensure!(
        total == 0.0 || string(&body["SHOW"]).as_deref() != Some("0"),
        "酷我搜索接口未提供结果"
    );
    let rows = body["abslist"]
        .as_array()
        .context("酷我搜索响应缺少 abslist")?;
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    for row in rows.iter().take(20) {
        let item = search_candidate(row)?;
        if seen.insert(item.song.clone()) {
            candidates.push(item);
        }
    }
    Ok(Search {
        candidates,
        complete: total <= rows.len() as f64,
    })
}

// The public search endpoint returns single-quoted JSON-like strings. Convert
// only string delimiters, with escaping, then let serde_json validate syntax.
// This is not JavaScript evaluation and never rewrites apostrophes in values.
fn search_json(bytes: &[u8]) -> Result<Value> {
    if let Ok(value) = json(bytes) {
        return Ok(value);
    }
    let text = std::str::from_utf8(bytes).context("酷我搜索响应不是 UTF-8")?;
    let mut chars = text.chars();
    let mut output = String::with_capacity(text.len());
    let mut delimiter = None;
    while let Some(ch) = chars.next() {
        match delimiter {
            None if ch == '\'' || ch == '"' => {
                delimiter = Some(ch);
                output.push('"');
            }
            None => output.push(ch),
            Some(quote) if ch == quote => {
                delimiter = None;
                output.push('"');
            }
            Some(quote) if ch == '\\' => {
                let escaped = chars.next().context("酷我搜索字符串转义不完整")?;
                if quote == '\'' && escaped == '\'' {
                    output.push('\'');
                } else if quote == '\'' && escaped == '"' {
                    output.push_str("\\\"");
                } else {
                    output.push('\\');
                    output.push(escaped);
                }
            }
            Some('\'') if ch == '"' => output.push_str("\\\""),
            Some(_) => output.push(ch),
        }
    }
    ensure!(delimiter.is_none(), "酷我搜索字符串未结束");
    json(output.as_bytes())
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite() && *n >= 0.0)
}

fn artists(value: &str) -> Vec<String> {
    // Replace the longer escape first so its remaining backslash is not kept.
    let value = value
        .replace(concat!(r"\\", "u0026"), "&")
        .replace(concat!(r"\", "u0026"), "&");
    decode_name(&value)
        .split('&')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

// Decode once: nested entities are not recursively interpreted.
pub(super) fn decode_name(value: &str) -> String {
    let mut rest = value;
    let mut result = String::with_capacity(value.len());
    while let Some(start) = rest.find('&') {
        result.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest
            .char_indices()
            .take_while(|(index, _)| *index <= 16)
            .find_map(|(index, ch)| (ch == ';').then_some(index))
        else {
            result.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "nbsp" => Some(' '),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "lt" => Some('<'),
            "gt" => Some('>'),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|digits| u32::from_str_radix(digits, 16).ok())
                .or_else(|| {
                    entity
                        .strip_prefix('#')
                        .and_then(|digits| digits.parse().ok())
                })
                .and_then(char::from_u32),
        };
        if let Some(ch) = decoded {
            result.push(ch);
            rest = &rest[end + 1..];
        } else {
            result.push('&');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}

fn search_candidate(row: &Value) -> Result<Candidate> {
    let id = string(&row["MUSICRID"]).context("酷我结果缺少 MUSICRID")?;
    let title = row["SONGNAME"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .context("酷我结果缺少歌名")?;
    let singers = artists(row["ARTIST"].as_str().context("酷我结果缺少歌手")?);
    ensure!(!singers.is_empty(), "酷我结果歌手为空");
    Ok(Candidate {
        song: SongRef::new(Provider::Kuwo, id)?,
        title: decode_name(title),
        artists: singers,
        album: row["ALBUM"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(decode_name),
        duration: number(&row["DURATION"]).filter(|n| *n > 0.0),
    })
}

// Verified HTTPS H5 endpoint returns ordinary line timestamps directly. The
// older mobi endpoint can ignore lrcx=0 and return an encrypted word-sync body;
// do not mislabel that body as ordinary LRC or silently downgrade to HTTP.
fn get(http: &dyn Transport, song: &SongRef) -> Result<Value> {
    ensure!(song.provider == Provider::Kuwo, "歌曲引用不是酷我来源");
    let checked = SongRef::new(Provider::Kuwo, song.id.clone())?;
    let body = json(
        &http.send(
            Request::get("https://m.kuwo.cn/newh5/singles/songinfoandlrc")
                .query("musicId", &checked.id),
        )?,
    )?;
    ensure!(
        number(&body["status"]) == Some(200.0),
        "酷我歌曲详情接口缺少成功状态或返回业务错误"
    );
    let info = &body["data"]["songinfo"];
    let actual = string(&info["id"]).context("酷我详情缺少歌曲 ID，无法确认歌曲身份")?;
    ensure!(
        actual.parse::<u64>()? == checked.id.parse::<u64>()?,
        "酷我详情歌曲 ID 不匹配"
    );
    if let Some(rid) = string(&info["musicrId"]) {
        let actual = SongRef::new(Provider::Kuwo, rid)?;
        ensure!(
            actual.id.parse::<u64>()? == checked.id.parse::<u64>()?,
            "酷我详情 MUSICRID 不匹配"
        );
    }
    Ok(body)
}

pub(crate) fn detail(http: &dyn Transport, song: &SongRef) -> Result<Candidate> {
    let body = get(http, song)?;
    detail_candidate(&body)
}

fn detail_candidate(body: &Value) -> Result<Candidate> {
    let row = &body["data"]["songinfo"];
    let title = row["songName"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .context("酷我详情缺少歌名")?;
    let singers = artists(row["artist"].as_str().context("酷我详情缺少歌手")?);
    ensure!(!singers.is_empty(), "酷我详情歌手为空");
    Ok(Candidate {
        song: SongRef::new(Provider::Kuwo, string(&row["id"]).unwrap())?,
        title: decode_name(title),
        artists: singers,
        album: row["album"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(decode_name),
        duration: number(&row["duration"]).filter(|n| *n > 0.0),
    })
}

pub(crate) fn lyric(
    http: &dyn Transport,
    song: &SongRef,
    expected: Option<&Candidate>,
) -> Result<Lyric> {
    if let Some(expected) = expected {
        ensure!(expected.song == *song, "酷我候选歌曲 ID 与请求不匹配");
    }
    let body = get(http, song)?;
    if let Some(expected) = expected {
        let actual = detail_candidate(&body)?;
        ensure!(
            crate::matching::reliable(&expected.query(), &actual),
            "酷我歌词元信息与搜索候选不匹配"
        );
    }
    let rows = match &body["data"]["lrclist"] {
        Value::Null => return Ok(Lyric::NotFound),
        Value::Array(rows) => rows,
        _ => bail!("酷我歌词行格式无效"),
    };
    if rows.is_empty() {
        return Ok(Lyric::NotFound);
    }
    let mut output = String::new();
    let mut timed = false;
    let mut untimed = false;
    for row in rows {
        let text = decode_name(row["lineLyric"].as_str().context("酷我歌词行缺少文本")?);
        ensure!(!text.contains(['\r', '\n']), "酷我单行歌词含换行符");
        // Missing time is plain text, but a present malformed time is an error.
        if row["time"].is_null() {
            untimed |= !text.trim().is_empty();
            continue;
        }
        let seconds = number(&row["time"]).context("酷我歌词时间无效")?;
        ensure!(seconds <= 24.0 * 60.0 * 60.0, "酷我歌词时间超出范围");
        if text.trim().is_empty() {
            continue;
        }
        let millis = (seconds * 1000.0).round() as u64;
        writeln!(
            output,
            "[{:02}:{:02}.{:03}]{}",
            millis / 60_000,
            millis / 1000 % 60,
            millis % 1000,
            text
        )?;
        ensure!(output.len() <= MAX_LYRIC_SIZE, "酷我歌词超出大小限制");
        timed = true;
    }
    if !timed {
        return Ok(if untimed {
            Lyric::PlainOnly
        } else {
            Lyric::NotFound
        });
    }
    ensure!(!untimed, "酷我同步歌词混入无时间标签内容");
    crate::lrc::parse(&output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::Fake;
    use serde_json::json;
    fn fake(value: Value) -> Fake {
        Fake::new(vec![value.to_string().into_bytes()])
    }
    fn song() -> SongRef {
        SongRef::new(Provider::Kuwo, "228908").unwrap()
    }
    fn h5() -> Value {
        json!({"status":200,"data":{"songinfo":{"id":"228908","musicrId":"MUSIC_228908","songName":"Example","artist":"A&B","album":"Album","duration":"269"},"lrclist":[{"time":"0.0","lineLyric":"Example"},{"time":"98.880005","lineLyric":"Words"}]}})
    }

    #[test]
    fn parses_search_strings_without_eval_or_apostrophe_corruption() {
        let input = br#"{'TOTAL':'1','SHOW':'1','abslist':[{'MUSICRID':'MUSIC_228908','SONGNAME':'Don\'t &quot;go&quot;','ARTIST':'A&B','ALBUM':'An&nbsp;album','DURATION':'269'}]}"#;
        let http = Fake::new(vec![input.to_vec()]);
        let found = search(
            &http,
            &Query {
                keywords: "Example".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(found.complete);
        let item = &found.candidates[0];
        assert_eq!(item.title, "Don't \"go\"");
        assert_eq!(item.artists, vec!["A", "B"]);
        assert_eq!(item.album.as_deref(), Some("An album"));
        assert_eq!(item.duration, Some(269.0));
        let requests = http.requests.borrow();
        assert_eq!(requests[0].url, "https://search.kuwo.cn/r.s");
        assert!(requests[0].query.contains(&("pn".into(), "0".into())));
    }

    #[test]
    fn splits_literal_backslash_artist_separators_from_search_and_detail() {
        for count in [1, 2] {
            let separator = format!("{}u0026", "\\".repeat(count));
            let artist = format!("A{separator}B");
            let mut expected_bytes = vec![b'A'];
            expected_bytes.extend(std::iter::repeat_n(92, count));
            expected_bytes.extend_from_slice(b"u0026B");
            assert_eq!(artist.as_bytes(), expected_bytes);
            assert_eq!(artists(&artist), vec!["A", "B"]);

            // JSON escaping doubles each literal backslash on the wire.
            let wire_artist = format!("A{}u0026B", "\\".repeat(count * 2));
            let input = format!(
                "{{'TOTAL':'1','SHOW':'1','abslist':[{{'MUSICRID':'MUSIC_228908','SONGNAME':'Example','ARTIST':'{wire_artist}','ALBUM':'Album','DURATION':'269'}}]}}"
            );
            let parsed = search_json(input.as_bytes()).unwrap();
            assert_eq!(
                parsed["abslist"][0]["ARTIST"].as_str().unwrap().as_bytes(),
                expected_bytes
            );
            let http = Fake::new(vec![input.into_bytes()]);
            let found = search(
                &http,
                &Query {
                    keywords: "Example".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(found.candidates[0].artists, vec!["A", "B"]);
            let mut body = h5();
            body["data"]["songinfo"]["artist"] = json!(artist);
            assert_eq!(
                detail(&fake(body), &song()).unwrap().artists,
                vec!["A", "B"]
            );
        }
        assert_eq!(artists("A&B"), vec!["A", "B"]);
    }

    #[test]
    fn validates_detail_both_ids_and_duration() {
        let result = detail(&fake(h5()), &song()).unwrap();
        assert_eq!(result.duration, Some(269.0));
        assert_eq!(result.artists, vec!["A", "B"]);
        let mut bad = h5();
        bad["data"]["songinfo"]["id"] = json!("1");
        assert!(detail(&fake(bad), &song()).is_err());
        let mut bad = h5();
        bad["data"]["songinfo"]["musicrId"] = json!("MUSIC_1");
        assert!(lyric(&fake(bad), &song(), None).is_err());
    }

    #[test]
    fn rejects_changed_selected_metadata_and_missing_success_status() {
        let expected = detail(&fake(h5()), &song()).unwrap();
        for (key, value) in [
            ("songName", json!("Example (Live)")),
            ("artist", json!("Other")),
            ("album", json!("Other")),
            ("duration", json!("100")),
        ] {
            let mut changed = h5();
            changed["data"]["songinfo"][key] = value;
            assert!(
                lyric(&fake(changed), &song(), Some(&expected)).is_err(),
                "{key}"
            );
        }
        let mut missing = h5();
        missing.as_object_mut().unwrap().remove("status");
        assert!(lyric(&fake(missing), &song(), None).is_err());
        assert_eq!(decode_name(&"&".repeat(256 * 1024)).len(), 256 * 1024);
    }

    #[test]
    fn h5_ordinary_lines_preserve_millisecond_timestamps() {
        let http = fake(h5());
        let Lyric::Synced(text) = lyric(&http, &song(), None).unwrap() else {
            panic!("missing LRC")
        };
        assert!(text.contains("[00:00.000]Example"));
        assert!(text.contains("[01:38.880]Words"));
        assert_eq!(http.requests.borrow().len(), 1);
    }

    #[test]
    fn rejects_business_errors_malformed_time_and_size() {
        let mut bad = h5();
        bad["status"] = json!(301);
        assert!(lyric(&fake(bad), &song(), None).is_err());
        let mut bad = h5();
        bad["data"]["lrclist"][0]["time"] = json!("NaN");
        assert!(lyric(&fake(bad), &song(), None).is_err());
        let mut bad = h5();
        bad["data"]["lrclist"][0]["lineLyric"] = json!("x".repeat(MAX_LYRIC_SIZE + 1));
        assert!(lyric(&fake(bad), &song(), None).is_err());
    }

    #[test]
    fn distinguishes_missing_from_untimed_text() {
        let mut value = h5();
        value["data"]["lrclist"] = json!([]);
        assert_eq!(
            lyric(&fake(value.clone()), &song(), None).unwrap(),
            Lyric::NotFound
        );
        value["data"]["lrclist"] = json!([{"lineLyric":"plain"}]);
        assert_eq!(
            lyric(&fake(value), &song(), None).unwrap(),
            Lyric::PlainOnly
        );
    }

    #[test]
    fn rejects_decoded_newline_entities_before_emitting_lyrics() {
        for separator in [
            "&#10;", "&#13;", "&#xA;", "&#xD;", "&#X0A;", "&#X0D;", "\n", "\r",
        ] {
            let text = format!("Words{separator}[01:00.000]Injected");
            for time in [json!(0), Value::Null] {
                let mut value = h5();
                value["data"]["lrclist"] = json!([{"time":time,"lineLyric":text}]);
                let error = lyric(&fake(value), &song(), None).unwrap_err().to_string();
                assert!(error.contains("单行歌词含换行符"), "{separator}");
            }
        }
    }

    #[test]
    fn lyric_entities_are_decoded_only_once() {
        let mut value = h5();
        value["data"]["lrclist"] = json!([{
            "time":0,
            "lineLyric":"A &amp; B &amp;#10;[01:00.000]Still literal &#x4E2D;"
        }]);
        assert_eq!(
            lyric(&fake(value), &song(), None).unwrap(),
            Lyric::Synced("[00:00.000]A & B &#10;[01:00.000]Still literal 中\n".into())
        );
    }

    #[test]
    fn rejects_mixed_nonempty_timed_and_untimed_body_in_either_order() {
        let timed = json!({"time":0,"lineLyric":"Timed body"});
        for plain in [
            json!({"lineLyric":"Untimed body"}),
            json!({"time":null,"lineLyric":"Untimed body"}),
        ] {
            for rows in [
                json!([timed.clone(), plain.clone()]),
                json!([plain.clone(), timed.clone()]),
            ] {
                let mut value = h5();
                value["data"]["lrclist"] = rows;
                let error = lyric(&fake(value), &song(), None).unwrap_err().to_string();
                assert!(error.contains("同步歌词混入无时间标签内容"));
            }
        }
    }

    #[test]
    fn ignores_empty_lines_without_discarding_untimed_body() {
        let empty = json!([
            {"lineLyric":""},
            {"time":null,"lineLyric":"&nbsp; "},
            {"time":0,"lineLyric":" "}
        ]);
        let mut value = h5();
        value["data"]["lrclist"] = empty.clone();
        assert_eq!(lyric(&fake(value), &song(), None).unwrap(), Lyric::NotFound);

        let mut rows = empty.as_array().unwrap().clone();
        rows.push(json!({"time":1,"lineLyric":"Timed body"}));
        let mut value = h5();
        value["data"]["lrclist"] = json!(rows);
        assert_eq!(
            lyric(&fake(value), &song(), None).unwrap(),
            Lyric::Synced("[00:01.000]Timed body\n".into())
        );

        let mut rows = empty.as_array().unwrap().clone();
        rows.push(json!({"lineLyric":"Untimed body"}));
        let mut value = h5();
        value["data"]["lrclist"] = json!(rows);
        assert_eq!(
            lyric(&fake(value), &song(), None).unwrap(),
            Lyric::PlainOnly
        );

        let mut value = h5();
        value["data"]["lrclist"] = json!([{"time":"NaN","lineLyric":" "}]);
        assert!(lyric(&fake(value), &song(), None).is_err());
    }

    #[test]
    fn entities_are_decoded_once_and_unknowns_are_preserved() {
        assert_eq!(
            decode_name("&amp;lt; &#x4E2D;&#25991; &unknown;"),
            "&lt; 中文 &unknown;"
        );
        assert!(search_json(b"{'x':'unfinished}").is_err());
        assert!(search_json(b"{'x':evil()}").is_err());
        assert_eq!(
            search_json(br#"{'x':'a\\b \"c\"'}"#).unwrap()["x"],
            "a\\b \"c\""
        );
    }
}
