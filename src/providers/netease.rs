//! Protocol facts checked against Meting (MIT), commit
//! 1c2f4c98eed749200d9d7ff5cab329c4308f4268, and LX Music's search
//! request schema at ad95d5091c9ed689fa72b5e5c849df65f5a679ce.
//! This is an independent implementation using RustCrypto's standard AES.

use super::{string, Search};
use crate::http::{json, Request, Transport};
use crate::model::{Candidate, Lyric, Provider, Query, SongRef};
use aes::cipher::{BlockEncrypt, KeyInit};
use anyhow::{ensure, Context, Result};
use md5::{Digest, Md5};
use serde_json::{json as value, Value};

const LIMIT: usize = 30;
const SEARCH_PATH: &str = "/api/search/song/list/page";
const DETAIL_PATH: &str = "/api/v3/song/detail";

fn checked_song(song: &SongRef) -> Result<()> {
    ensure!(song.provider == Provider::Netease, "不是网易云歌曲引用");
    SongRef::new(Provider::Netease, song.id.clone())?;
    Ok(())
}

fn status(response: &Value) -> Result<()> {
    let code = response
        .get("code")
        .and_then(string)
        .context("网易云响应缺少 code")?;
    ensure!(code == "200", "网易云接口返回状态码 {code}");
    Ok(())
}

fn eapi_params(path: &str, data: &Value) -> String {
    let text = data.to_string();
    let digest = format!(
        "{:x}",
        Md5::digest(format!("nobody{path}use{text}md5forencrypt"))
    );
    let mut bytes = format!("{path}-36cd479b6b5-{text}-36cd479b6b5-{digest}").into_bytes();
    let padding = 16 - bytes.len() % 16;
    bytes.resize(bytes.len() + padding, padding as u8);
    let cipher = aes::Aes128::new_from_slice(b"e82ckenh8dichen8").expect("fixed AES-128 key");
    for block in bytes.chunks_exact_mut(16) {
        cipher.encrypt_block(aes::cipher::generic_array::GenericArray::from_mut_slice(
            block,
        ));
    }
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn eapi(http: &dyn Transport, path: &str, data: Value) -> Result<Value> {
    // The batch endpoint dispatches using the path inside the EAPI envelope.
    // HTTPS is mandatory even though some reference clients still use HTTP.
    let request = Request::get("https://interface.music.163.com/eapi/batch")
        .header("Referer", "https://music.163.com/")
        .header("Origin", "https://music.163.com")
        .header("Cookie", "os=pc; appver=2.5.2.197409; channel=netease")
        .form(&[("params", eapi_params(path, &data))]);
    let response = json(&http.send(request)?)?;
    status(&response)?;
    Ok(response)
}

fn candidate(raw: &Value) -> Result<Candidate> {
    let raw = raw.pointer("/baseInfo/simpleSongData").unwrap_or(raw);
    let id = raw
        .get("id")
        .and_then(string)
        .context("网易云歌曲缺少 ID")?;
    let title = raw
        .get("name")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .context("网易云歌曲缺少名称")?
        .to_owned();
    let artists = raw
        .get("ar")
        .or_else(|| raw.get("artists"))
        .and_then(Value::as_array)
        .context("网易云歌曲缺少歌手列表")?
        .iter()
        .map(|artist| {
            artist
                .get("name")
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty())
                .map(str::to_owned)
                .context("网易云歌手名称无效")
        })
        .collect::<Result<Vec<_>>>()?;
    let album = raw
        .get("al")
        .or_else(|| raw.get("album"))
        .filter(|album| !album.is_null())
        .map(|album| {
            album
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .context("网易云专辑名称字段无效")
        })
        .transpose()?;
    let duration = raw
        .get("dt")
        .or_else(|| raw.get("duration"))
        .filter(|v| !v.is_null())
        .map(|v| {
            let ms = v.as_f64().context("网易云歌曲时长无效")?;
            ensure!(ms.is_finite() && ms >= 0.0, "网易云歌曲时长无效");
            Ok(ms / 1000.0)
        })
        .transpose()?;
    Ok(Candidate {
        song: SongRef::new(Provider::Netease, id)?,
        title,
        artists,
        album,
        duration,
    })
}

pub(crate) fn search(http: &dyn Transport, query: &Query) -> Result<Search> {
    let text = query.search_text();
    ensure!(!text.trim().is_empty(), "网易云搜索关键词为空");
    let response = eapi(
        http,
        SEARCH_PATH,
        value!({
            "keyword": text, "needCorrect": "1", "channel": "typing",
            "offset": 0, "scene": "normal", "total": true, "limit": LIMIT,
        }),
    )?;
    let resources = response
        .pointer("/data/resources")
        .and_then(Value::as_array)
        .context("网易云搜索响应缺少 resources")?;
    let total = response
        .pointer("/data/totalCount")
        .and_then(string)
        .and_then(|v| v.parse::<u64>().ok())
        .context("网易云搜索响应缺少有效 totalCount")?;
    ensure!(
        total >= resources.len() as u64,
        "网易云搜索结果数量与 totalCount 不一致"
    );
    let candidates = resources
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
    let id = song.id.parse::<u64>().context("网易云数字 ID 超出范围")?;
    let response = eapi(
        http,
        DETAIL_PATH,
        value!({"c": value!([{"id": id, "v": 0}]).to_string()}),
    )?;
    let songs = response
        .get("songs")
        .and_then(Value::as_array)
        .context("网易云详情响应缺少 songs")?;
    ensure!(songs.len() == 1, "网易云详情未返回唯一歌曲");
    let candidate = candidate(&songs[0])?;
    ensure!(
        candidate.song == *song,
        "网易云详情返回的歌曲 ID 与请求不一致"
    );
    Ok(candidate)
}

pub(crate) fn lyric(
    http: &dyn Transport,
    song: &SongRef,
    _candidate: Option<&Candidate>,
) -> Result<Lyric> {
    checked_song(song)?;
    let request = Request::get("https://music.163.com/api/song/lyric/v1")
        .header("Referer", "https://music.163.com/")
        .query("id", &song.id)
        .query("lv", 0)
        .query("tv", 0)
        .query("rv", 0)
        .query("kv", 0)
        .query("yv", 0)
        .query("ytv", 0)
        .query("yrv", 0)
        .query("cp", "false");
    let response = json(&http.send(request)?)?;
    status(&response)?;
    if let Some(id) = response.get("id") {
        let id = SongRef::new(
            Provider::Netease,
            string(id).context("网易云歌词返回的歌曲 ID 无效")?,
        )?;
        ensure!(id == *song, "网易云歌词返回的歌曲 ID 与请求不一致");
    }
    for key in ["nolyric", "uncollected"] {
        if let Some(flag) = response.get(key) {
            ensure!(flag.is_boolean(), "网易云歌词 {key} 字段无效");
        }
    }
    if response.get("nolyric").and_then(Value::as_bool) == Some(true) {
        return Ok(Lyric::Instrumental);
    }
    if response.get("uncollected").and_then(Value::as_bool) == Some(true) {
        return Ok(Lyric::NotFound);
    }
    let lyric = response
        .pointer("/lrc/lyric")
        .and_then(Value::as_str)
        .context("网易云歌词缺少有效 lrc.lyric，且未返回无歌词标记")?;
    crate::lrc::netease(lyric)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::Fake;

    fn song() -> SongRef {
        SongRef::new(Provider::Netease, "42").unwrap()
    }
    fn record(id: u64) -> Value {
        value!({"id": id, "name": "Song", "ar": [{"name": "Singer"}], "al": {"name": "Album"}, "dt": 125000})
    }
    fn fake(value: Value) -> Fake {
        Fake::new(vec![value.to_string().into_bytes()])
    }

    #[test]
    fn search_tracks_total_and_nested_song_schema() {
        let http = fake(
            value!({"code":200,"data":{"resources":[{"baseInfo":{"simpleSongData":record(42)}}],"totalCount":2}}),
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
        assert_eq!(result.candidates[0].duration, Some(125.0));
        let requests = http.requests.borrow();
        assert_eq!(
            requests[0].url,
            "https://interface.music.163.com/eapi/batch"
        );
        assert!(requests[0].body.as_ref().unwrap().starts_with("params="));
        let http = fake(value!({"code":"200","data":{"resources":[],"totalCount":0}}));
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
    fn search_does_not_hide_invalid_schema_or_status() {
        for response in [
            value!({"code":401}),
            value!({"data":{"resources":[],"totalCount":0}}),
            value!({"code":200,"data":{"resources":[],"totalCount":"bad"}}),
            value!({"code":200,"data":{"resources":[{}],"totalCount":1}}),
        ] {
            assert!(search(
                &fake(response),
                &Query {
                    keywords: "Song".into(),
                    ..Default::default()
                }
            )
            .is_err());
        }
    }

    #[test]
    fn detail_checks_returned_id() {
        let http = fake(value!({"code":200,"songs":[record(42)]}));
        assert_eq!(detail(&http, &song()).unwrap().title, "Song");
        assert!(detail(&fake(value!({"code":200,"songs":[record(43)]})), &song()).is_err());
        assert!(detail(&fake(value!({"code":200,"songs":[]})), &song()).is_err());
    }

    #[test]
    fn detail_normalizes_returned_decimal_identity() {
        let numeric = SongRef::new(Provider::Netease, "00042").unwrap();
        let mut raw = record(42);
        raw["id"] = value!("00042");
        assert_eq!(
            detail(&fake(value!({"code":200,"songs":[raw]})), &numeric)
                .unwrap()
                .song,
            song()
        );
        let mut wrong = record(43);
        wrong["id"] = value!("00043");
        assert!(detail(&fake(value!({"code":200,"songs":[wrong]})), &numeric).is_err());
    }

    #[test]
    fn lyric_converts_credits_and_classifies_absence() {
        let http = fake(
            value!({"code":200,"lrc":{"lyric":"{\"t\":0,\"c\":[{\"tx\":\"作词: X\"}]}\n[00:01.00]歌词"}}),
        );
        assert_eq!(
            lyric(&http, &song(), None).unwrap(),
            Lyric::Synced("[00:00.00]作词: X\n[00:01.00]歌词\n".into())
        );
        assert_eq!(
            lyric(&fake(value!({"code":200,"nolyric":true})), &song(), None).unwrap(),
            Lyric::Instrumental
        );
        assert_eq!(
            lyric(
                &fake(value!({"code":200,"uncollected":true})),
                &song(),
                None
            )
            .unwrap(),
            Lyric::NotFound
        );
        assert_eq!(
            lyric(
                &fake(value!({"code":200,"lrc":{"lyric":"plain"}})),
                &song(),
                None
            )
            .unwrap(),
            Lyric::PlainOnly
        );
        assert!(lyric(
            &fake(value!({"code":200,"lrc":{"lyric":12}})),
            &song(),
            None
        )
        .is_err());
        assert!(lyric(
            &fake(value!({"lrc":{"lyric":"[00:01]text"}})),
            &song(),
            None
        )
        .is_err());
    }

    #[test]
    fn lyric_normalizes_returned_decimal_identity_and_rejects_mismatches() {
        let numeric = SongRef::new(Provider::Netease, "00042").unwrap();
        for returned in [value!(42), value!("42"), value!("00042")] {
            let http = fake(value!({"code":200,"id":returned,"lrc":{"lyric":"[00:01]text"}}));
            assert_eq!(
                lyric(&http, &numeric, None).unwrap(),
                Lyric::Synced("[00:01]text\n".into())
            );
            assert!(http.requests.borrow()[0]
                .query
                .contains(&("id".into(), "42".into())));
        }
        for returned in [
            value!(43),
            value!("00043"),
            value!("000000000000000000042"),
            value!("42x"),
            value!(""),
        ] {
            assert!(lyric(
                &fake(value!({"code":200,"id":returned,"lrc":{"lyric":"[00:01]text"}})),
                &numeric,
                None
            )
            .is_err());
        }
    }

    #[test]
    fn lyric_compares_zero_numeric_identity() {
        let requested = SongRef::new(Provider::Netease, "000").unwrap();
        assert_eq!(
            lyric(
                &fake(value!({"code":200,"id":"0000","uncollected":true})),
                &requested,
                None
            )
            .unwrap(),
            Lyric::NotFound
        );
    }

    #[test]
    fn invalid_public_song_fields_fail_before_network() {
        let http = Fake::new(vec![]);
        assert!(lyric(
            &http,
            &SongRef {
                provider: Provider::Netease,
                id: "42&evil=1".into()
            },
            None
        )
        .is_err());
        assert!(detail(&http, &SongRef::new(Provider::Kuwo, "42").unwrap()).is_err());
        assert!(http.requests.borrow().is_empty());
    }

    #[test]
    fn rejects_malformed_optional_metadata_and_lyric_flags() {
        for response in [
            value!({"code":200,"id":true,"lrc":{"lyric":"[00:01]text"}}),
            value!({"code":200}),
            value!({"code":200,"lrc":null}),
            value!({"code":200,"nolyric":"true"}),
            value!({"code":200,"uncollected":1}),
            value!({"code":200,"lrc":{"lyric":"x".repeat(crate::http::MAX_LYRIC_SIZE + 1)}}),
        ] {
            assert!(lyric(&fake(response), &song(), None).is_err());
        }
        for album in [value!(42), value!({"name":false}), value!({})] {
            let mut raw = record(42);
            raw["al"] = album;
            assert!(candidate(&raw).is_err());
        }
    }

    #[test]
    fn eapi_aes_envelope_has_known_answer() {
        // Independently generated with OpenSSL AES-128-ECB / PKCS#7.
        let encrypted = eapi_params("/api/test", &value!({"x":1}));
        assert_eq!(encrypted, "4DC723619A991588865191FD2F319BADAADC72592913FF72DFBC1BC9424CDA069E978D425F818C71D01CBD140BE0E49F57B986419D00275F41501849FC541582E8CCB7AE8E98F504F80252B0A9B85D60");
    }
}
