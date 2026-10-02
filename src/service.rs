use crate::http::{Http, Transport};
use crate::model::{Candidate, Lyric, Provider, Query, SongRef};
use crate::providers;
use anyhow::{bail, ensure, Context, Result};

#[derive(Debug, Clone)]
pub enum Lookup {
    Song(SongRef),
    Query(Query),
}

#[derive(Debug)]
pub enum Outcome {
    Found { song: SongRef, lyric: String },
    Candidates(Vec<Candidate>),
    Unavailable(Lyric),
}

#[derive(Debug)]
pub struct Resolution {
    pub outcome: Outcome,
    pub diagnostics: Vec<String>,
}

#[derive(Default)]
pub struct Client;

impl Client {
    pub fn resolve(
        &self,
        lookup: Lookup,
        provider: Option<Provider>,
        selection: Option<&SongRef>,
    ) -> Result<Resolution> {
        resolve(&Http::default(), lookup, provider, selection)
    }
}

fn checked_lyric(
    http: &dyn Transport,
    song: &SongRef,
    candidate: Option<&Candidate>,
) -> Result<Lyric> {
    match providers::lyric(http, song, candidate)? {
        Lyric::Synced(text) => crate::lrc::parse(&text),
        other => Ok(other),
    }
}

fn resolution(outcome: Outcome, diagnostics: Vec<String>) -> Resolution {
    Resolution {
        outcome,
        diagnostics,
    }
}

pub(crate) fn resolve(
    http: &dyn Transport,
    lookup: Lookup,
    provider: Option<Provider>,
    selection: Option<&SongRef>,
) -> Result<Resolution> {
    if let Some(song) = selection {
        let song = SongRef::new(song.provider, song.id.clone())?;
        ensure!(
            provider.is_none_or(|p| p == song.provider),
            "选择的候选来源与 --provider 冲突"
        );
        ensure!(
            !matches!(lookup, Lookup::Song(_)),
            "歌曲 ID 输入不能另外选择候选"
        );
        let candidate = if song.provider == Provider::Kugou && !song.id.contains(':') {
            let Lookup::Query(query) = &lookup else {
                unreachable!()
            };
            validate_query(query)?;
            let results = providers::search(http, Provider::Kugou, query)?;
            let mut matches = results
                .candidates
                .into_iter()
                .filter(|candidate| candidate.song == song);
            let candidate = matches
                .next()
                .context("所选酷狗 hash 不在本次搜索结果中；请缩小关键词后重新选择")?;
            ensure!(matches.next().is_none(), "所选酷狗 hash 的搜索元信息不唯一");
            Some(candidate)
        } else {
            None
        };
        return Ok(resolution(
            match checked_lyric(http, &song, candidate.as_ref())? {
                Lyric::Synced(lyric) => Outcome::Found { song, lyric },
                other => Outcome::Unavailable(other),
            },
            Vec::new(),
        ));
    }
    match lookup {
        Lookup::Song(song) => {
            let song = SongRef::new(song.provider, song.id)?;
            ensure!(
                provider.is_none_or(|p| p == song.provider),
                "歌曲来源与 --provider 冲突"
            );
            let native = checked_lyric(http, &song, None);
            match native {
                Ok(Lyric::Synced(lyric)) => {
                    return Ok(resolution(Outcome::Found { song, lyric }, Vec::new()))
                }
                Ok(Lyric::Instrumental) => {
                    return Ok(resolution(
                        Outcome::Unavailable(Lyric::Instrumental),
                        Vec::new(),
                    ))
                }
                result if provider.is_some() => {
                    return Ok(resolution(Outcome::Unavailable(result?), Vec::new()))
                }
                _ => {}
            }
            let mut diagnostics = Vec::new();
            if let Err(error) = &native {
                diagnostics.push(format!("{}: {error:#}", song.provider));
            }
            let candidate = match providers::detail(http, &song) {
                Ok(candidate) => {
                    // QQ detail validates the numeric ID before returning its MID.
                    let verified_qq_mapping = song.provider == Provider::Qq
                        && song.id.starts_with("id:")
                        && candidate.song.provider == Provider::Qq
                        && !candidate.song.id.starts_with("id:");
                    ensure!(
                        candidate.song == song || verified_qq_mapping,
                        "歌曲详情 ID 与请求不符"
                    );
                    candidate
                }
                Err(error) => {
                    if native.is_err() {
                        return Err(error).context("原生歌词失败，且无法读取歌曲元信息进行补查");
                    }
                    diagnostics.push(format!("无法读取歌曲元信息，未跨源猜测: {error:#}"));
                    return Ok(resolution(Outcome::Unavailable(native?), diagnostics));
                }
            };
            let fallback = search(
                http,
                &candidate.query(),
                &Provider::ALL
                    .into_iter()
                    .filter(|p| *p != song.provider)
                    .collect::<Vec<_>>(),
                diagnostics,
            )?;
            if matches!(&fallback.outcome, Outcome::Unavailable(_)) {
                native.context("原生歌词失败，跨源补查未取得可用歌词")?;
            }
            Ok(fallback)
        }
        Lookup::Query(query) => {
            validate_query(&query)?;
            let providers = provider.map_or_else(|| Provider::ALL.to_vec(), |p| vec![p]);
            search(http, &query, &providers, Vec::new())
        }
    }
}

fn validate_query(query: &Query) -> Result<()> {
    let text = query.search_text();
    ensure!(!text.trim().is_empty(), "查询不能为空");
    ensure!(text.len() <= 1024, "查询文本过长");
    ensure!(query.artists.len() <= 64, "歌手数量过多");
    for field in query
        .title
        .iter()
        .chain(query.album.iter())
        .chain(query.artists.iter())
    {
        ensure!(
            !field.trim().is_empty() && field.len() <= 1024,
            "歌曲元信息为空或过长"
        );
    }
    if let Some(duration) = query.duration {
        ensure!(
            duration.is_finite() && duration > 0.0,
            "歌曲时长必须为有限正数"
        );
    }
    Ok(())
}

fn search(
    http: &dyn Transport,
    query: &Query,
    sources: &[Provider],
    mut diagnostics: Vec<String>,
) -> Result<Resolution> {
    let mut candidates = Vec::new();
    let mut complete = true;
    let mut successes = 0;
    for source in sources {
        match providers::search(http, *source, query) {
            Ok(result) => {
                successes += 1;
                if !result.complete {
                    complete = false;
                    diagnostics.push(format!("{source}: 搜索结果可能截断，需明确选择"));
                }
                for candidate in result.candidates {
                    SongRef::new(candidate.song.provider, candidate.song.id.clone())
                        .context("搜索返回无效歌曲 ID")?;
                    candidates.push(candidate);
                }
            }
            Err(error) => {
                complete = false;
                diagnostics.push(format!("{source}: {error:#}"));
            }
        }
    }
    if successes == 0 {
        bail!("全部歌词来源请求失败: {}", diagnostics.join("; "));
    }
    candidates.sort_by(|a, b| {
        crate::matching::reliable(query, b)
            .cmp(&crate::matching::reliable(query, a))
            .then_with(|| a.song.cmp(&b.song))
    });
    candidates.dedup_by(|a, b| a.song == b.song);
    let strong: Vec<_> = candidates
        .iter()
        .filter(|c| crate::matching::reliable(query, c))
        .collect();
    if complete && strong.len() == 1 {
        let candidate = strong[0];
        return Ok(resolution(
            match checked_lyric(http, &candidate.song, Some(candidate))? {
                Lyric::Synced(lyric) => Outcome::Found {
                    song: candidate.song.clone(),
                    lyric,
                },
                other => Outcome::Unavailable(other),
            },
            diagnostics,
        ));
    }
    if candidates.is_empty() {
        ensure!(
            complete,
            "没有取得可选歌曲，且有来源失败或结果不完整: {}",
            diagnostics.join("; ")
        );
        Ok(resolution(
            Outcome::Unavailable(Lyric::NotFound),
            diagnostics,
        ))
    } else {
        Ok(resolution(Outcome::Candidates(candidates), diagnostics))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::tests::Fake;
    fn record() -> serde_json::Value {
        serde_json::json!({"id":5,"trackName":"Song","artistName":"Artist","albumName":"Album","duration":200.0,"instrumental":false,"syncedLyrics":"[00:01.00]text"})
    }
    #[test]
    fn exact_id_fetches_saves_and_validates() {
        let fake = Fake::new(vec![record().to_string().into_bytes()]);
        let result = resolve(
            &fake,
            Lookup::Song(SongRef::new(Provider::Lrclib, "5").unwrap()),
            Some(Provider::Lrclib),
            None,
        )
        .unwrap();
        assert!(matches!(result.outcome, Outcome::Found { .. }));
        assert_eq!(fake.requests.borrow().len(), 1);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("song.lrc");
        if let Outcome::Found { lyric, .. } = result.outcome {
            assert!(crate::save_lrc(&path, &lyric).unwrap());
            assert!(!crate::save_lrc(&path, &lyric).unwrap());
            assert_eq!(std::fs::read_to_string(path).unwrap(), "[00:01.00]text\n");
        }
    }
    #[test]
    fn keyword_does_not_select_first() {
        let fake = Fake::new(vec![serde_json::json!([record()]).to_string().into_bytes()]);
        let query = Query {
            keywords: "Song Artist".into(),
            ..Default::default()
        };
        let result = resolve(&fake, Lookup::Query(query), Some(Provider::Lrclib), None).unwrap();
        assert!(matches!(result.outcome, Outcome::Candidates(_)));
        assert_eq!(fake.requests.borrow().len(), 1);
    }
    #[test]
    fn structured_unique_candidate_resolves() {
        let fake = Fake::new(vec![
            serde_json::json!([record()]).to_string().into_bytes(),
            record().to_string().into_bytes(),
        ]);
        let query = Query {
            title: Some("Song".into()),
            artists: vec!["Artist".into()],
            duration: Some(200.0),
            ..Default::default()
        };
        let result = resolve(&fake, Lookup::Query(query), Some(Provider::Lrclib), None).unwrap();
        assert!(matches!(result.outcome, Outcome::Found { .. }));
    }
    #[test]
    fn multiple_identical_records_are_ambiguous() {
        let mut other = record();
        other["id"] = 6.into();
        let fake = Fake::new(vec![serde_json::json!([record(), other])
            .to_string()
            .into_bytes()]);
        let query = Query {
            title: Some("Song".into()),
            artists: vec!["Artist".into()],
            ..Default::default()
        };
        let result = resolve(&fake, Lookup::Query(query), Some(Provider::Lrclib), None).unwrap();
        assert!(matches!(result.outcome, Outcome::Candidates(_)));
    }
    #[test]
    fn kugou_selection_requeries_exact_hash_and_passes_metadata() {
        use base64::Engine;
        let hash = "0123456789ABCDEF0123456789ABCDEF";
        let song = SongRef::new(Provider::Kugou, hash).unwrap();
        let query = Query {
            keywords: "Song Artist".into(),
            ..Default::default()
        };
        let search = serde_json::json!({"error_code":0,"data":{"total":1,"lists":[{"FileHash":hash,"OriSongName":"Song","Singers":[{"name":"Artist"}],"Duration":200}]}});
        let lyrics = serde_json::json!({"status":200,"candidates":[{"id":"5","accesskey":"KEY123","song":"Song","singer":"Artist","duration":200000}]});
        let download = serde_json::json!({"status":200,"fmt":"lrc","content":base64::engine::general_purpose::STANDARD.encode("[00:01]text")});
        let fake = Fake::new(vec![
            search.to_string().into_bytes(),
            lyrics.to_string().into_bytes(),
            download.to_string().into_bytes(),
        ]);
        let result = resolve(
            &fake,
            Lookup::Query(query.clone()),
            Some(Provider::Kugou),
            Some(&song),
        )
        .unwrap();
        assert!(matches!(result.outcome, Outcome::Found { .. }));
        assert_eq!(fake.requests.borrow().len(), 3);
        let empty = Fake::new(vec![
            serde_json::json!({"error_code":0,"data":{"total":0,"lists":[]}})
                .to_string()
                .into_bytes(),
        ]);
        assert!(resolve(
            &empty,
            Lookup::Query(query),
            Some(Provider::Kugou),
            Some(&song)
        )
        .is_err());
        assert_eq!(empty.requests.borrow().len(), 1);
    }
    #[test]
    fn qq_numeric_id_can_fallback_using_verified_mid_metadata() {
        let detail = serde_json::json!({"code":0,"req":{"code":0,"data":{"track_info":{"id":42,"mid":"0039MnYb0qxYhV","title":"Song","singer":[{"name":"Artist"}],"album":{"name":"Album"},"interval":200}}}});
        let fake = Fake::new(vec![
            detail.to_string().into_bytes(),
            serde_json::json!({"code":0,"lyric":""})
                .to_string()
                .into_bytes(),
            detail.to_string().into_bytes(),
            serde_json::json!({"code":200,"data":{"resources":[],"totalCount":0}})
                .to_string()
                .into_bytes(),
            serde_json::json!({"error_code":0,"data":{"total":0,"lists":[]}})
                .to_string()
                .into_bytes(),
            serde_json::json!({"TOTAL":"0","abslist":[]})
                .to_string()
                .into_bytes(),
            serde_json::json!([record()]).to_string().into_bytes(),
            record().to_string().into_bytes(),
        ]);
        let result = resolve(
            &fake,
            Lookup::Song(SongRef::new(Provider::Qq, "id:42").unwrap()),
            None,
            None,
        )
        .unwrap();
        let Outcome::Found { song, .. } = result.outcome else {
            panic!("fallback failed: {result:?}")
        };
        assert_eq!(song.provider, Provider::Lrclib);
        assert_eq!(fake.requests.borrow().len(), 8);
    }

    fn netease_fallback_responses(
        native: serde_json::Value,
        lrclib: serde_json::Value,
    ) -> Vec<Vec<u8>> {
        vec![
            native,
            serde_json::json!({"code":200,"songs":[{"id":42,"name":"Song","ar":[{"name":"Artist"}],"al":{"name":"Album"},"dt":200000}]}),
            serde_json::json!({"code":0,"req":{"code":0,"data":{"body":{"song":{"list":[]}},"meta":{"sum":0}}}}),
            serde_json::json!({"error_code":0,"data":{"total":0,"lists":[]}}),
            serde_json::json!({"TOTAL":"0","abslist":[]}),
            lrclib,
        ]
        .into_iter()
        .map(|value| value.to_string().into_bytes())
        .collect()
    }

    #[test]
    fn native_error_survives_complete_empty_fallback() {
        let fake = Fake::new(netease_fallback_responses(
            serde_json::json!({"code":503}),
            serde_json::json!([]),
        ));
        let error = resolve(
            &fake,
            Lookup::Song(SongRef::new(Provider::Netease, "42").unwrap()),
            None,
            None,
        )
        .unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("原生歌词失败，跨源补查未取得可用歌词"));
        assert!(message.contains("网易云接口返回状态码 503"));
        assert_eq!(fake.requests.borrow().len(), 6);
        assert!(fake.responses.borrow().is_empty());
    }

    #[test]
    fn native_error_allows_successful_fallback_recovery() {
        let mut responses = netease_fallback_responses(
            serde_json::json!({"code":503}),
            serde_json::json!([record()]),
        );
        responses.push(record().to_string().into_bytes());
        let fake = Fake::new(responses);
        let result = resolve(
            &fake,
            Lookup::Song(SongRef::new(Provider::Netease, "42").unwrap()),
            None,
            None,
        )
        .unwrap();
        let Outcome::Found { song, lyric } = result.outcome else {
            panic!("fallback failed: {result:?}")
        };
        assert_eq!(song, SongRef::new(Provider::Lrclib, "5").unwrap());
        assert_eq!(lyric, "[00:01.00]text\n");
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].contains("网易云接口返回状态码 503"));
        assert_eq!(fake.requests.borrow().len(), 7);
        assert!(fake.responses.borrow().is_empty());
    }

    #[test]
    fn native_error_allows_fallback_candidates() {
        let mut other = record();
        other["id"] = 6.into();
        let fake = Fake::new(netease_fallback_responses(
            serde_json::json!({"code":503}),
            serde_json::json!([record(), other]),
        ));
        let result = resolve(
            &fake,
            Lookup::Song(SongRef::new(Provider::Netease, "42").unwrap()),
            None,
            None,
        )
        .unwrap();
        let Outcome::Candidates(candidates) = result.outcome else {
            panic!("missing fallback candidates: {result:?}")
        };
        assert_eq!(candidates.len(), 2);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(fake.requests.borrow().len(), 6);
        assert!(fake.responses.borrow().is_empty());
    }

    #[test]
    fn native_error_survives_unavailable_fallback_lyric() {
        for unavailable in [Lyric::NotFound, Lyric::PlainOnly, Lyric::Instrumental] {
            let mut lyrics = record();
            lyrics["syncedLyrics"] = serde_json::Value::Null;
            if unavailable == Lyric::PlainOnly {
                lyrics["plainLyrics"] = "plain words".into();
            }
            if unavailable == Lyric::Instrumental {
                lyrics["instrumental"] = true.into();
            }
            let mut responses = netease_fallback_responses(
                serde_json::json!({"code":503}),
                serde_json::json!([record()]),
            );
            responses.push(lyrics.to_string().into_bytes());
            let fake = Fake::new(responses);
            let error = resolve(
                &fake,
                Lookup::Song(SongRef::new(Provider::Netease, "42").unwrap()),
                None,
                None,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("网易云接口返回状态码 503"));
            assert_eq!(fake.requests.borrow().len(), 7);
            assert!(fake.responses.borrow().is_empty());
        }
    }

    #[test]
    fn native_not_found_with_complete_empty_fallback_is_unavailable() {
        let fake = Fake::new(netease_fallback_responses(
            serde_json::json!({"code":200,"uncollected":true}),
            serde_json::json!([]),
        ));
        let result = resolve(
            &fake,
            Lookup::Song(SongRef::new(Provider::Netease, "42").unwrap()),
            None,
            None,
        )
        .unwrap();
        assert!(matches!(
            result.outcome,
            Outcome::Unavailable(Lyric::NotFound)
        ));
        assert!(result.diagnostics.is_empty());
        assert_eq!(fake.requests.borrow().len(), 6);
    }

    #[test]
    fn raw_decimal_song_and_selection_fields_are_canonicalized() {
        for select in [false, true] {
            let fake = Fake::new(vec![
                serde_json::json!({"code":200,"id":"00042","lrc":{"lyric":"[00:01]text"}})
                    .to_string()
                    .into_bytes(),
            ]);
            let raw = SongRef {
                provider: Provider::Netease,
                id: "00042".into(),
            };
            let lookup = if select {
                Lookup::Query(Query {
                    keywords: "Song".into(),
                    ..Default::default()
                })
            } else {
                Lookup::Song(raw.clone())
            };
            let result = resolve(
                &fake,
                lookup,
                Some(Provider::Netease),
                select.then_some(&raw),
            )
            .unwrap();
            let Outcome::Found { song, .. } = result.outcome else {
                panic!("missing native lyric: {result:?}")
            };
            assert_eq!(song.id, "42");
            assert!(fake.requests.borrow()[0]
                .query
                .contains(&("id".into(), "42".into())));
        }
        for invalid in ["000000000000000000042", "42&evil=1"] {
            let fake = Fake::new(vec![]);
            let raw = SongRef {
                provider: Provider::Netease,
                id: invalid.into(),
            };
            assert!(resolve(
                &fake,
                Lookup::Song(raw.clone()),
                Some(Provider::Netease),
                None
            )
            .is_err());
            assert!(resolve(
                &fake,
                Lookup::Query(Query {
                    keywords: "Song".into(),
                    ..Default::default()
                }),
                Some(Provider::Netease),
                Some(&raw)
            )
            .is_err());
            assert!(fake.requests.borrow().is_empty());
        }
    }

    #[test]
    fn mismatched_provider_rejected_before_network() {
        let fake = Fake::new(vec![]);
        assert!(resolve(
            &fake,
            Lookup::Song(SongRef::new(Provider::Netease, "1").unwrap()),
            Some(Provider::Qq),
            None
        )
        .is_err());
        assert!(fake.requests.borrow().is_empty());
    }
}
