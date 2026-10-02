//! QQ's legacy plain-LRC endpoint and track fields were checked against
//! Meting (MIT), commit 1c2f4c98eed749200d9d7ff5cab329c4308f4268.
//! PC-search wire schema/signature facts: LX Music ad95d5091c9ed689fa72b5e5c849df65f5a679ce.
//! Independently implemented; no QRC/custom crypt1 decryption is included.

use super::{string, Search};
use crate::http::{decode_base64, json, Request, Transport, MAX_LYRIC_SIZE};
use crate::model::{Candidate, Lyric, Provider, Query, SongRef};
use anyhow::{ensure, Context, Result};
use base64::Engine;
use serde_json::{json as value, Value};
use sha1::{Digest, Sha1};
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: usize = 30;
const SEARCH_MODULE: &str = "music.search.SearchCgiService";

fn checked_song(song: &SongRef) -> Result<()> {
    ensure!(song.provider == Provider::Qq, "不是 QQ 歌曲引用");
    ensure!(
        SongRef::new(Provider::Qq, song.id.clone())?.id == song.id,
        "QQ 歌曲引用未规范化"
    );
    Ok(())
}

fn status(response: &Value) -> Result<()> {
    let code = response
        .get("code")
        .and_then(string)
        .context("QQ 响应缺少 code")?;
    ensure!(code == "0", "QQ 接口返回状态码 {code}");
    Ok(())
}

fn signature(text: &str) -> String {
    // zzc is a wire-request checksum, NOT QQ's lyric encryption.
    let hash = Sha1::digest(text);
    let hex = format!("{hash:x}");
    let bytes = hex.as_bytes();
    // Position 40 in the original wire recipe is beyond the SHA-1 hex
    // digest and contributes an empty string, not a zero byte.
    let first: String = [23, 14, 6, 36, 16, 7, 19]
        .iter()
        .map(|&i| bytes[i] as char)
        .collect();
    let last: String = [16, 1, 32, 12, 19, 27, 8, 5]
        .iter()
        .map(|&i| bytes[i] as char)
        .collect();
    let mask = [
        89, 39, 179, 150, 218, 82, 58, 252, 177, 52, 186, 123, 120, 64, 242, 133, 143, 161, 121,
        179,
    ];
    let mixed: Vec<u8> = hash
        .iter()
        .zip(mask)
        .map(|(byte, mask)| byte ^ mask)
        .collect();
    let middle = base64::engine::general_purpose::STANDARD
        .encode(mixed)
        .replace(['/', '+', '='], "");
    format!("zzc{first}{middle}{last}").to_ascii_lowercase()
}

fn search_request(query: &str) -> Result<Request> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时钟无效")?
        .as_nanos();
    let session = format!("{:x}", Sha1::digest(format!("{query}:{now}")));
    let search_id = format!("{}00000", session[..32].to_ascii_uppercase());
    let payload = value!({
        "comm": {"ct":"19", "cv":"2151", "uin":"0", "tmeAppID":"qqmusic", "tmeLoginType":0},
        SEARCH_MODULE: {
            "module": SEARCH_MODULE, "method": "DoSearchForQQMusicDesktop",
            "param": {"grp":1, "num_per_page":LIMIT, "page_num":1, "query":query,
                "remoteplace":"txt.newclient.top", "search_type":0, "searchid":search_id}
        }
    });
    let sign = signature(&payload.to_string());
    Ok(Request::get("https://u.y.qq.com/cgi-bin/musics.fcg")
        .header("Referer", "https://y.qq.com/")
        .header("User-Agent", "QQMusic 14090508(android 12)")
        .query("sign", sign)
        .json(payload))
}

fn candidate(raw: &Value) -> Result<Candidate> {
    let raw = raw.get("musicData").unwrap_or(raw);
    let mid = raw
        .get("mid")
        .or_else(|| raw.get("songmid"))
        .and_then(Value::as_str)
        .context("QQ 歌曲缺少 MID")?;
    let title = raw
        .get("title")
        .or_else(|| raw.get("name"))
        .or_else(|| raw.get("songname"))
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .context("QQ 歌曲缺少名称")?;
    let artists = raw
        .get("singer")
        .and_then(Value::as_array)
        .context("QQ 歌曲缺少歌手列表")?
        .iter()
        .map(|artist| {
            let name = artist
                .get("name")
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty())
                .context("QQ 歌手名称无效")?;
            decode_entities(name)
        })
        .collect::<Result<Vec<_>>>()?;
    let album = if let Some(album) = raw.get("album").filter(|album| !album.is_null()) {
        Some(decode_entities(
            album
                .get("name")
                .or_else(|| album.get("title"))
                .and_then(Value::as_str)
                .context("QQ 专辑名称字段无效")?,
        )?)
    } else {
        raw.get("albumname")
            .filter(|album| !album.is_null())
            .map(|album| decode_entities(album.as_str().context("QQ 专辑名称字段无效")?))
            .transpose()?
    };
    let duration = raw
        .get("interval")
        .filter(|v| !v.is_null())
        .map(|v| {
            let duration = v.as_f64().context("QQ 歌曲时长无效")?;
            ensure!(duration.is_finite() && duration >= 0.0, "QQ 歌曲时长无效");
            Ok(duration)
        })
        .transpose()?;
    let song = SongRef::new(Provider::Qq, mid)?;
    ensure!(
        song.id == mid && !song.id.starts_with("id:"),
        "QQ 歌曲 MID 字段无效"
    );
    Ok(Candidate {
        song,
        title: decode_entities(title)?,
        artists,
        album,
        duration,
    })
}

pub(crate) fn search(http: &dyn Transport, query: &Query) -> Result<Search> {
    let text = query.search_text();
    ensure!(!text.trim().is_empty(), "QQ 搜索关键词为空");
    let response = json(&http.send(search_request(&text)?)?)?;
    status(&response)?;
    let module = response
        .get(SEARCH_MODULE)
        .or_else(|| response.get("req"))
        .context("QQ 搜索响应缺少模块")?;
    status(module)?;
    let songs = module
        .pointer("/data/body/song/list")
        .and_then(Value::as_array)
        .context("QQ 搜索响应缺少歌曲列表")?;
    let total = module
        .pointer("/data/meta/sum")
        .and_then(string)
        .and_then(|v| v.parse::<u64>().ok())
        .context("QQ 搜索响应缺少有效 total")?;
    ensure!(
        total >= songs.len() as u64,
        "QQ 搜索结果数量与 total 不一致"
    );
    let candidates = songs
        .iter()
        .take(LIMIT)
        .map(candidate)
        .collect::<Result<Vec<_>>>()?;
    let complete = total == candidates.len() as u64;
    Ok(Search {
        candidates,
        complete,
    })
}

pub(crate) fn detail(http: &dyn Transport, song: &SongRef) -> Result<Candidate> {
    checked_song(song)?;
    let param = match song.id.strip_prefix("id:") {
        Some(id) => {
            value!({"song_type":0,"song_mid":"","song_id":id.parse::<u64>().context("QQ 数字 ID 超出范围")?})
        }
        None => value!({"song_type":0,"song_mid":song.id}),
    };
    let payload = value!({"comm":{"ct":"19","cv":"1859","uin":"0"},"req":{
        "module":"music.pf_song_detail_svr","method":"get_song_detail_yqq","param":param
    }});
    let request = Request::get("https://u.y.qq.com/cgi-bin/musicu.fcg")
        .header("Referer", "https://y.qq.com/")
        .json(payload);
    let response = json(&http.send(request)?)?;
    status(&response)?;
    let module = response.get("req").context("QQ 详情响应缺少 req")?;
    status(module)?;
    let raw = module
        .pointer("/data/track_info")
        .context("QQ 详情响应缺少 track_info")?;
    let candidate = candidate(raw)?;
    if song.id.starts_with("id:") {
        let returned = raw
            .get("id")
            .and_then(string)
            .context("QQ 详情缺少数字 ID，无法验证 MID 映射")?;
        let returned = SongRef::new(Provider::Qq, format!("id:{returned}"))?;
        ensure!(returned == *song, "QQ 详情返回的数字 ID 与请求不一致");
    } else {
        ensure!(candidate.song == *song, "QQ 详情返回的 MID 与请求不一致");
    }
    Ok(candidate)
}

// Parse JSON or exactly one JSONP call, never execute JavaScript and never
// truncate by a fixed callback-name length. Embedded ')' inside JSON strings
// is safe because only the final wrapper parenthesis is removed.
fn json_or_jsonp(body: &[u8]) -> Result<Value> {
    let text = std::str::from_utf8(body)
        .context("QQ 响应不是 UTF-8")?
        .trim_start_matches('\u{feff}')
        .trim();
    if text.starts_with('{') {
        return json(text.as_bytes());
    }
    let (callback, rest) = text.split_once('(').context("QQ 响应不是 JSON/JSONP")?;
    let callback = callback.trim();
    ensure!(
        !callback.is_empty() && callback.len() <= 128,
        "QQ JSONP 回调无效"
    );
    let mut bytes = callback.bytes();
    let first = bytes.next().unwrap();
    ensure!(
        first.is_ascii_alphabetic() || matches!(first, b'_' | b'$'),
        "QQ JSONP 回调无效"
    );
    ensure!(
        bytes.all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'$')),
        "QQ JSONP 回调无效"
    );
    let rest = rest
        .trim()
        .strip_suffix(';')
        .unwrap_or(rest.trim())
        .trim_end();
    let payload = rest.strip_suffix(')').context("QQ JSONP 包装无效")?.trim();
    let response = json(payload.as_bytes())?;
    ensure!(response.is_object(), "QQ JSONP 数据不是对象");
    Ok(response)
}

fn decode_entities(input: &str) -> Result<String> {
    ensure!(input.len() <= MAX_LYRIC_SIZE, "QQ 文本超出大小限制");
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        rest = &rest[start..];
        let entity = rest
            .bytes()
            .take(17)
            .position(|byte| byte == b';')
            .map(|end| (&rest[1..end], end));
        let decoded = match entity {
            Some((name, end)) => {
                let character = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some(' '),
                    _ if name.starts_with('#') => {
                        let digits = &name[1..];
                        let number = if let Some(hex) = digits
                            .strip_prefix('x')
                            .or_else(|| digits.strip_prefix('X'))
                        {
                            u32::from_str_radix(hex, 16).ok()
                        } else {
                            digits.parse::<u32>().ok()
                        };
                        Some(
                            char::from_u32(number.context("QQ 数字 HTML 实体无效")?)
                                .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
                                .context("QQ HTML 实体不是有效字符")?,
                        )
                    }
                    _ => None,
                };
                character.map(|character| (character, end + 1))
            }
            None => None,
        };
        if let Some((character, consumed)) = decoded {
            output.push(character);
            rest = &rest[consumed..];
        } else {
            output.push('&');
            rest = &rest[1..];
        }
    }
    output.push_str(rest);
    ensure!(output.len() <= MAX_LYRIC_SIZE, "QQ 解码文本超出大小限制");
    Ok(output)
}

pub(crate) fn lyric(
    http: &dyn Transport,
    song: &SongRef,
    _candidate: Option<&Candidate>,
) -> Result<Lyric> {
    checked_song(song)?;
    // A numeric ID is not a MID: resolve and verify it before using the legacy
    // lyric API. A caller-supplied candidate cannot prove that mapping.
    let resolved;
    let song = if song.id.starts_with("id:") {
        resolved = detail(http, song)?;
        &resolved.song
    } else {
        song
    };
    let request = Request::get("https://c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg")
        .header("Referer", "https://y.qq.com/portal/player.html")
        .query("songmid", &song.id)
        .query("g_tk", 5381)
        .query("loginUin", 0)
        .query("hostUin", 0)
        .query("format", "json")
        .query("inCharset", "utf8")
        .query("outCharset", "utf-8")
        .query("platform", "yqq")
        .query("nobase64", 0);
    let response = json_or_jsonp(&http.send(request)?)?;
    ensure!(
        response.get("code").is_some() || response.get("retcode").is_some(),
        "QQ 歌词响应缺少业务状态码"
    );
    for key in ["code", "retcode"] {
        if let Some(code) = response.get(key) {
            ensure!(
                string(code).as_deref() == Some("0"),
                "QQ 歌词 {key} 非成功或字段无效"
            );
        }
    }
    if let Some(mid) = response.get("songmid") {
        ensure!(
            mid.as_str() == Some(song.id.as_str()),
            "QQ 歌词返回的 MID 与请求不一致"
        );
    }
    for key in ["crypt", "crypt1", "qrc"] {
        if let Some(flag) = response.get(key) {
            ensure!(
                string(flag).as_deref() == Some("0") || flag == &Value::Bool(false),
                "QQ 返回加密/逐字歌词；不支持 {key}"
            );
        }
    }
    let lyric = response
        .get("lyric")
        .and_then(Value::as_str)
        .context("QQ 响应缺少有效 lyric；旧 LRC 接口可能已受限")?;
    if lyric.is_empty() {
        return Ok(Lyric::NotFound);
    }
    let lyric = decode_entities(
        &decode_base64(lyric).context("QQ 旧 LRC 不是有效 Base64 UTF-8；不支持 crypt1/QRC")?,
    )?;
    // The legacy API uses this untimed sentinel for an instrumental. Do not
    // infer instrumentals from arbitrary empty/plain text or timed lyrics.
    let mut lines = lyric.lines().filter(|line| !line.trim().is_empty());
    let first_line: String = lines
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if first_line == "纯音乐请欣赏"
        && lines.all(|line| {
            matches!(
                line.trim(),
                "未经许可,不得翻唱或使用" | "未经许可，不得翻唱或使用"
            )
        })
    {
        return Ok(Lyric::Instrumental);
    }
    crate::lrc::parse(&lyric)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::Fake;

    fn song() -> SongRef {
        SongRef::new(Provider::Qq, "001CLC7W2Gpz4J").unwrap()
    }
    fn record() -> Value {
        value!({"id":42,"mid":song().id,"title":"Song &amp; More","singer":[{"name":"Singer"}],"album":{"name":"Album"},"interval":125})
    }
    fn fake(value: Value) -> Fake {
        Fake::new(vec![value.to_string().into_bytes()])
    }
    fn detail_body(record: Value) -> Value {
        value!({"code":0,"req":{"code":0,"data":{"track_info":record}}})
    }
    fn b64(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(text)
    }

    #[test]
    fn signed_search_checks_total_and_fields() {
        let http = fake(
            value!({"code":0,SEARCH_MODULE:{"code":0,"data":{"body":{"song":{"list":[record()]}},"meta":{"sum":2}}}}),
        );
        let result = search(
            &http,
            &Query {
                keywords: "Song".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!result.complete);
        assert_eq!(result.candidates[0].song, song());
        assert_eq!(result.candidates[0].title, "Song & More");
        assert_eq!(result.candidates[0].duration, Some(125.0));
        let requests = http.requests.borrow();
        let request = &requests[0];
        assert_eq!(request.url, "https://u.y.qq.com/cgi-bin/musics.fcg");
        assert_eq!(
            request.query[0].1,
            signature(request.body.as_ref().unwrap())
        );
        let body: Value = serde_json::from_str(request.body.as_ref().unwrap()).unwrap();
        assert_eq!(body[SEARCH_MODULE]["method"], "DoSearchForQQMusicDesktop");
        let http = fake(
            value!({"code":0,"req":{"code":0,"data":{"body":{"song":{"list":[]}},"meta":{"sum":0}}}}),
        );
        assert!(
            search(
                &http,
                &Query {
                    keywords: "nothing".into(),
                    ..Default::default()
                }
            )
            .unwrap()
            .complete
        );
    }

    #[test]
    fn search_rejects_business_failure_and_partial_schema() {
        for body in [
            value!({"code":1}),
            value!({"code":0,"req":{"code":-1}}),
            value!({"code":0,"req":{"code":0,"data":{"body":{"song":{"list":[]}}}}}),
        ] {
            assert!(search(
                &fake(body),
                &Query {
                    keywords: "Song".into(),
                    ..Default::default()
                }
            )
            .is_err());
        }
    }

    #[test]
    fn detail_checks_both_id_types() {
        assert_eq!(
            detail(&fake(detail_body(record())), &song()).unwrap().song,
            song()
        );
        let numeric = SongRef::new(Provider::Qq, "id:42").unwrap();
        let http = fake(detail_body(record()));
        assert_eq!(detail(&http, &numeric).unwrap().song, song());
        let request: Value =
            serde_json::from_str(http.requests.borrow()[0].body.as_ref().unwrap()).unwrap();
        assert_eq!(request["req"]["param"]["song_id"], 42);
        assert_eq!(request["req"]["param"]["song_mid"], "");
        let mut wrong = record();
        wrong["id"] = value!(43);
        assert!(detail(&fake(detail_body(wrong)), &numeric).is_err());
        let mut wrong = record();
        wrong["mid"] = value!("001CLC7W2Gpz4K");
        assert!(detail(&fake(detail_body(wrong)), &song()).is_err());
    }

    #[test]
    fn detail_normalizes_numeric_identity_and_rejects_mismatches() {
        let numeric = SongRef::new(Provider::Qq, "id:00042").unwrap();
        assert_eq!(numeric.id, "id:42");
        for returned in [value!(42), value!("42"), value!("00042")] {
            let mut raw = record();
            raw["id"] = returned;
            let http = fake(detail_body(raw));
            assert_eq!(detail(&http, &numeric).unwrap().song, song());
            let request: Value =
                serde_json::from_str(http.requests.borrow()[0].body.as_ref().unwrap()).unwrap();
            assert_eq!(request["req"]["param"]["song_id"], 42);
        }
        for returned in [
            value!("00043"),
            value!("000000000000000000042"),
            value!("id:42"),
            value!("42x"),
        ] {
            let mut raw = record();
            raw["id"] = returned;
            assert!(detail(&fake(detail_body(raw)), &numeric).is_err());
        }
    }

    #[test]
    fn detail_compares_zero_numeric_identity() {
        let mut raw = record();
        raw["id"] = value!("000");
        assert_eq!(
            detail(
                &fake(detail_body(raw)),
                &SongRef::new(Provider::Qq, "id:0000").unwrap()
            )
            .unwrap()
            .song,
            song()
        );
    }

    #[test]
    fn numeric_lyric_resolves_to_mid() {
        let http = Fake::new(vec![
            detail_body(record()).to_string().into_bytes(),
            value!({"code":0,"retcode":0,"lyric":b64("[00:01]text")})
                .to_string()
                .into_bytes(),
        ]);
        assert_eq!(
            lyric(&http, &SongRef::new(Provider::Qq, "id:42").unwrap(), None).unwrap(),
            Lyric::Synced("[00:01]text\n".into())
        );
        let requests = http.requests.borrow();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].query.contains(&("songmid".into(), song().id)));
        assert!(!requests[1].query.iter().any(|(_, v)| v == "id:42"));
    }

    #[test]
    fn lyric_accepts_json_and_variable_jsonp_and_decodes_entities_once() {
        let body = value!({"code":0,"lyric":b64("[00:01.00]A &amp; B &#x1D11E; &#39; &amp;lt; )")});
        for bytes in [
            body.to_string().into_bytes(),
            format!(" callback_long_name ({body}); \n").into_bytes(),
            format!("x({body})").into_bytes(),
        ] {
            let http = Fake::new(vec![bytes]);
            assert_eq!(
                lyric(&http, &song(), None).unwrap(),
                Lyric::Synced("[00:01.00]A & B 𝄞 ' &lt; )\n".into())
            );
        }
    }

    #[test]
    fn jsonp_is_not_executable_or_truncated_arbitrarily() {
        for bad in [
            "x({\"code\":0});alert(1)",
            "a.b({\"code\":0})",
            "x({\"code\":0})junk",
            "x({\"code\":0});;",
            "x([1])",
            "x({\"code\":0},1)",
            "<html>error</html>",
        ] {
            assert!(json_or_jsonp(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn lyric_reports_encrypted_bad_status_and_bad_fields() {
        for response in [
            value!({"code":-1901}),
            value!({"lyric":b64("[00:01]text")}),
            value!({"code":0,"lyric":42}),
            value!({"code":0,"lyric":"%%%%"}),
            value!({"code":0,"crypt":1,"lyric":"0123456789"}),
            value!({"code":0,"retcode":-1,"lyric":""}),
            value!({"code":0,"songmid":"001CLC7W2Gpz4K","lyric":""}),
        ] {
            assert!(lyric(&fake(response), &song(), None).is_err());
        }
        assert_eq!(
            lyric(&fake(value!({"code":0,"lyric":""})), &song(), None).unwrap(),
            Lyric::NotFound
        );
        assert_eq!(
            lyric(
                &fake(value!({"code":0,"lyric":b64("ordinary text")})),
                &song(),
                None
            )
            .unwrap(),
            Lyric::PlainOnly
        );
        assert!(lyric(
            &fake(value!({"code":0,"lyric":b64("<QrcInfos>encrypted</QrcInfos>")})),
            &song(),
            None
        )
        .is_err());
    }

    #[test]
    fn limits_and_invalid_song_fail_closed() {
        assert!(decode_entities(&"x".repeat(MAX_LYRIC_SIZE + 1)).is_err());
        let http = Fake::new(vec![]);
        assert!(lyric(
            &http,
            &SongRef {
                provider: Provider::Qq,
                id: "invalid".into()
            },
            None
        )
        .is_err());
        assert!(detail(&http, &SongRef::new(Provider::Netease, "42").unwrap()).is_err());
        assert!(http.requests.borrow().is_empty());
    }

    #[test]
    fn rejects_cipher_flags_and_malformed_optional_metadata() {
        for key in ["crypt", "crypt1", "qrc"] {
            let mut response = value!({"code":0,"lyric":b64("[00:01]text")});
            response[key] = value!(1);
            assert!(lyric(&fake(response), &song(), None).is_err());
        }
        for album in [value!(42), value!({"name":false}), value!({})] {
            let mut raw = record();
            raw["album"] = album;
            assert!(candidate(&raw).is_err());
        }
        for mid in ["id:42", "mid:001CLC7W2Gpz4J"] {
            let mut raw = record();
            raw["mid"] = value!(mid);
            assert!(candidate(&raw).is_err());
        }
        let mut raw = record();
        raw.as_object_mut().unwrap().remove("id");
        assert!(detail(
            &fake(detail_body(raw)),
            &SongRef::new(Provider::Qq, "id:42").unwrap()
        )
        .is_err());
        assert!(lyric(
            &fake(value!({"code":0,"lyric":b64(&"x".repeat(MAX_LYRIC_SIZE + 1))})),
            &song(),
            None
        )
        .is_err());
    }

    #[test]
    fn legacy_retcode_only_and_invalid_html_entities() {
        assert_eq!(
            lyric(
                &fake(value!({"retcode":0,"lyric":b64("[00:01]text")})),
                &song(),
                None
            )
            .unwrap(),
            Lyric::Synced("[00:01]text\n".into())
        );
        for response in [
            value!({"retcode":-1,"lyric":""}),
            value!({"code":1,"retcode":0,"lyric":""}),
            value!({"code":false,"retcode":0,"lyric":""}),
        ] {
            assert!(lyric(&fake(response), &song(), None).is_err());
        }
        for bad in ["&#0;", "&#xD800;", "&#x110000;", "&#xBADZZ;"] {
            assert!(decode_entities(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn repeated_ampersands_are_not_quadratically_scanned() {
        let input = "&".repeat(MAX_LYRIC_SIZE);
        assert_eq!(decode_entities(&input).unwrap(), input);
    }

    #[test]
    fn distinguishes_instrumental_sentinel_from_plain_and_timed_text() {
        for text in [
            "纯音乐 请欣赏",
            "纯音乐                       请欣赏\n未经许可,不得翻唱或使用",
        ] {
            assert_eq!(
                lyric(&fake(value!({"code":0,"lyric":b64(text)})), &song(), None).unwrap(),
                Lyric::Instrumental
            );
        }
        assert_eq!(
            lyric(
                &fake(value!({"code":0,"lyric":b64("[00:01]纯音乐请欣赏")})),
                &song(),
                None
            )
            .unwrap(),
            Lyric::Synced("[00:01]纯音乐请欣赏\n".into())
        );
        assert_eq!(
            lyric(
                &fake(value!({"code":0,"lyric":b64("纯音乐请欣赏\nother words")})),
                &song(),
                None
            )
            .unwrap(),
            Lyric::PlainOnly
        );
    }

    #[test]
    fn signature_has_independent_known_answer() {
        assert_eq!(
            signature("{\"x\":1}"),
            "zzc6f21c193gnpt7ieaz67am9x6sgdjj2ypqc7b4946c"
        );
    }
}
