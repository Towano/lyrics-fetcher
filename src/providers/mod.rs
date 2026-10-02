use crate::http::Transport;
use crate::model::{Candidate, Lyric, Provider, Query, SongRef};
use anyhow::Result;

pub(crate) mod kugou;
pub(crate) mod kuwo;
pub(crate) mod lrclib;
pub(crate) mod netease;
pub(crate) mod qq;

#[derive(Debug)]
pub(crate) struct Search {
    pub candidates: Vec<Candidate>,
    pub complete: bool,
}

pub(crate) fn search(http: &dyn Transport, provider: Provider, query: &Query) -> Result<Search> {
    match provider {
        Provider::Netease => netease::search(http, query),
        Provider::Qq => qq::search(http, query),
        Provider::Kugou => kugou::search(http, query),
        Provider::Kuwo => kuwo::search(http, query),
        Provider::Lrclib => lrclib::search(http, query),
    }
}

pub(crate) fn detail(http: &dyn Transport, song: &SongRef) -> Result<Candidate> {
    match song.provider {
        Provider::Netease => netease::detail(http, song),
        Provider::Qq => qq::detail(http, song),
        Provider::Kugou => kugou::detail(http, song),
        Provider::Kuwo => kuwo::detail(http, song),
        Provider::Lrclib => lrclib::detail(http, song),
    }
}

pub(crate) fn lyric(
    http: &dyn Transport,
    song: &SongRef,
    candidate: Option<&Candidate>,
) -> Result<Lyric> {
    match song.provider {
        Provider::Netease => netease::lyric(http, song, candidate),
        Provider::Qq => qq::lyric(http, song, candidate),
        Provider::Kugou => kugou::lyric(http, song, candidate),
        Provider::Kuwo => kuwo::lyric(http, song, candidate),
        Provider::Lrclib => lrclib::lyric(http, song, candidate),
    }
}

pub(crate) fn string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
}
