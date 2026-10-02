//! Input classification only. Share URLs are decoded locally, never followed.

use crate::model::{Provider, Query, SongRef};
use anyhow::{bail, ensure, Context, Result};
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Debug, Clone)]
pub enum Input {
    File(PathBuf),
    Song(SongRef),
    Query(Query),
}

/// Classify an OS-native argument without making network requests.
///
/// Auto mode prefers an existing regular file, then a recognized complete share
/// URL or typed ID. Bare decimal IDs default to NetEase unless a provider is
/// specified; use `--input-type query` for numeric song titles.
pub fn parse_input(value: &OsStr, mode: &str, provider: Option<Provider>) -> Result<Input> {
    ensure!(
        matches!(mode, "auto" | "query" | "id" | "file"),
        "未知输入类型: {mode}"
    );
    ensure!(!value.is_empty(), "歌词输入不能为空");
    let path = Path::new(value);
    if mode == "file" {
        require_file(path)?;
        return Ok(Input::File(path.to_owned()));
    }
    if mode == "auto" {
        match fs::metadata(path) {
            Ok(metadata) => {
                ensure!(
                    metadata.is_file(),
                    "输入不是普通音频文件: {}",
                    path.display()
                );
                return Ok(Input::File(path.to_owned()));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                // A dangling symlink is a path error, not a search phrase.
                if fs::symlink_metadata(path).is_ok() {
                    bail!("输入文件不可访问: {}", path.display());
                }
            }
            Err(error) if filename_error_allows_text(path, &error) => {
                // A filename representation limit does not invalidate a textual
                // query/link/ID. Explicit and obvious paths still fail closed.
            }
            Err(error) if error.kind() == ErrorKind::InvalidInput => {
                return Err(error).context("输入包含无效文件路径字符");
            }
            Err(error) => return Err(error).context("检查输入文件失败"),
        }
    }
    let text = value
        .to_str()
        .context("非 UTF-8 输入只能用作已有文件路径")?
        .trim();
    ensure!(!text.is_empty(), "歌词输入不能为空");
    ensure!(text.len() <= 4096, "输入文本过长");
    if mode == "query" {
        ensure!(
            !looks_like_url(text),
            "URL 请使用 auto 或 id 输入类型；仅支持完整歌曲链接"
        );
        return query(text);
    }
    if looks_like_url(text) {
        let song = song_from_url(text)?;
        require_provider(&song, provider)?;
        return Ok(Input::Song(song));
    }
    if let Some((prefix, _)) = text.split_once(':') {
        if matches!(prefix, "netease" | "qq" | "kugou" | "kuwo" | "lrclib") {
            let song: SongRef = text.parse()?;
            require_provider(&song, provider)?;
            return Ok(Input::Song(song));
        }
        // Provider-local typed IDs are valid only with an explicit provider.
        if matches!(
            (provider, prefix),
            (Some(Provider::Qq), "id" | "mid") | (Some(Provider::Kugou), "mix" | "hash" | "lrc")
        ) {
            return Ok(Input::Song(SongRef::new(provider.unwrap(), text)?));
        }
        bail!("未知歌曲前缀或路径: {prefix}；关键词中含冒号时使用 --input-type query，文件使用 --input-type file");
    }
    if mode == "id" {
        return Ok(Input::Song(unqualified_id(text, provider)?));
    }
    if looks_like_path(path) {
        bail!(
            "输入文件不存在: {}；若它是歌名，使用 --input-type query",
            path.display()
        );
    }
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(Input::Song(unqualified_id(text, provider)?));
    }
    if matches!(provider, Some(Provider::Qq))
        && text.len() == 14
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Ok(Input::Song(SongRef::new(Provider::Qq, text)?));
    }
    if matches!(provider, Some(Provider::Kugou))
        && text.len() == 32
        && text.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(Input::Song(SongRef::new(Provider::Kugou, text)?));
    }
    query(text)
}

fn filename_error_allows_text(path: &Path, error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::InvalidFilename
        && path
            .as_os_str()
            .to_str()
            .is_some_and(|text| looks_like_url(text.trim()) || !looks_like_path(path))
}

fn query(text: &str) -> Result<Input> {
    ensure!(text.len() <= 1024, "查询文本过长 (最多 1024 bytes)");
    Ok(Input::Query(Query {
        keywords: text.to_owned(),
        ..Query::default()
    }))
}

fn require_file(path: &Path) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("输入文件不可访问: {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "输入不是普通音频文件: {}",
        path.display()
    );
    Ok(())
}

fn require_provider(song: &SongRef, provider: Option<Provider>) -> Result<()> {
    ensure!(
        provider.is_none_or(|provider| provider == song.provider),
        "歌曲来源与 --provider 冲突"
    );
    Ok(())
}

fn unqualified_id(text: &str, provider: Option<Provider>) -> Result<SongRef> {
    let provider = provider.unwrap_or(Provider::Netease);
    if provider == Provider::Qq && text.bytes().all(|byte| byte.is_ascii_digit()) {
        SongRef::new(provider, format!("id:{text}"))
    } else if provider == Provider::Kugou && text.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("酷狗数字 ID 必须明确使用 kugou:mix:数字；裸 ID 使用 32 位 hash");
    } else {
        SongRef::new(provider, text)
    }
}

fn looks_like_url(text: &str) -> bool {
    text.contains("://")
        || text.starts_with("//")
        || text.starts_with("www.")
        || text.starts_with("music.163.com/")
        || text.starts_with("y.music.163.com/")
        || text.starts_with("y.qq.com/")
        || text.starts_with("c.y.qq.com/")
        || text.starts_with("www.kugou.com/")
        || text.starts_with("www.kuwo.cn/")
        || text.starts_with("lrclib.net/")
}

fn looks_like_path(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    path.is_absolute()
        || text.starts_with('.') && (text.starts_with("./") || text.starts_with("../"))
        || text.contains('/')
        || text.contains('\\')
        || path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "mp3"
                        | "flac"
                        | "wav"
                        | "ogg"
                        | "opus"
                        | "m4a"
                        | "mp4"
                        | "aac"
                        | "aiff"
                        | "aif"
                        | "ape"
                        | "wv"
                        | "wma"
                        | "ncm"
                        | "lrc"
                )
            })
}

fn unique_query(url: &Url, key: &str) -> Result<Option<String>> {
    let values: Vec<_> = url
        .query_pairs()
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
        .collect();
    ensure!(values.len() <= 1, "歌曲链接含重复参数 {key}");
    Ok(values.into_iter().next())
}

fn required_query(url: &Url, key: &str) -> Result<String> {
    unique_query(url, key)?.with_context(|| format!("歌曲链接缺少 {key}"))
}

fn path_id<'a>(path: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    path.strip_prefix(prefix)?
        .strip_suffix(suffix)
        .filter(|id| !id.is_empty() && !id.contains('/'))
}

fn song_from_url(text: &str) -> Result<SongRef> {
    let mut url = Url::parse(text).context("歌曲链接无效；请提供完整 http(s) 歌曲链接")?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "歌曲链接只支持 http(s)，不会执行该链接"
    );
    let authority = text
        .split_once("://")
        .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
        .unwrap_or_default();
    ensure!(
        !authority.contains('@') && !authority.contains(':') && !text.contains('\\'),
        "歌曲链接不能包含用户信息、端口或反斜杠"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.port().is_none(),
        "歌曲链接不能包含用户信息或端口"
    );
    let host = url.host_str().context("歌曲链接缺少域名")?.to_owned();
    if matches!(
        host.as_str(),
        "163cn.tv" | "163cn.com" | "c6.y.qq.com" | "t1.kugou.com" | "kugou.com" | "kuwo.cn"
    ) {
        bail!("不支持短链接跳转；请提供平台完整歌曲链接或 来源:ID");
    }
    ensure!(
        matches!(
            host.as_str(),
            "music.163.com"
                | "y.music.163.com"
                | "y.qq.com"
                | "c.y.qq.com"
                | "www.kugou.com"
                | "www.kuwo.cn"
                | "m.kuwo.cn"
                | "lrclib.net"
        ),
        "不支持的歌曲链接域名: {host}；请提供完整平台歌曲链接或 来源:ID"
    );
    if host == "music.163.com" {
        if let Some(fragment) = url.fragment().map(str::to_owned) {
            ensure!(
                url.path() == "/" && url.query().is_none(),
                "网易云 fragment 歌曲链接格式无效"
            );
            ensure!(
                fragment.starts_with("/song?") || fragment.starts_with("/m/song?"),
                "不支持该网易云页面；请提供完整歌曲链接"
            );
            url = Url::parse(&format!("https://music.163.com{fragment}"))?;
        }
    }
    let path = url.path();
    match host.as_str() {
        "music.163.com" | "y.music.163.com" => {
            ensure!(
                matches!(path, "/song" | "/song/" | "/m/song" | "/m/song/"),
                "不支持该网易云页面；请提供完整歌曲链接"
            );
            SongRef::new(Provider::Netease, required_query(&url, "id")?)
        }
        "y.qq.com" | "c.y.qq.com" => {
            if let Some(mid) = path_id(path, "/n/ryqq/songDetail/", "")
                .or_else(|| path_id(path, "/n/yqq/song/", ".html"))
            {
                return SongRef::new(Provider::Qq, mid);
            }
            ensure!(
                matches!(
                    path,
                    "/n/ryqq/songDetail"
                        | "/n/yqq/song"
                        | "/n/yqq/song.html"
                        | "/base/fcgi-bin/u"
                        | "/r/"
                ),
                "不支持该 QQ 音乐页面；请提供完整歌曲链接"
            );
            match (
                unique_query(&url, "songmid")?,
                unique_query(&url, "songid")?,
            ) {
                (Some(mid), None) => SongRef::new(Provider::Qq, mid),
                (None, Some(id)) => SongRef::new(Provider::Qq, format!("id:{id}")),
                _ => bail!("QQ 歌曲链接需唯一 songmid 或 songid；短链接请换完整歌曲链接"),
            }
        }
        "www.kugou.com" => {
            if let Some(mix) = path_id(path, "/song/", ".html") {
                return SongRef::new(Provider::Kugou, format!("mix:{mix}"));
            }
            ensure!(
                matches!(path, "/song/" | "/song"),
                "不支持该酷狗页面；请使用 kugou:hash 或 kugou:mix:数字"
            );
            if let Some(fragment) = url.fragment().map(str::to_owned) {
                ensure!(
                    url.query().is_none(),
                    "酷狗歌曲链接不能同时包含 query 和 fragment ID"
                );
                let fragment = fragment.strip_prefix('?').unwrap_or(&fragment);
                url.set_query(Some(fragment));
            }
            match (
                unique_query(&url, "hash")?,
                unique_query(&url, "mixsongid")?,
            ) {
                (Some(hash), None) => SongRef::new(Provider::Kugou, hash),
                (None, Some(mix)) => SongRef::new(Provider::Kugou, format!("mix:{mix}")),
                _ => bail!("酷狗歌曲链接需唯一 hash 或 mixsongid"),
            }
        }
        "www.kuwo.cn" | "m.kuwo.cn" => {
            if let Some(id) = path_id(path, "/play_detail/", "") {
                return SongRef::new(Provider::Kuwo, id);
            }
            ensure!(
                matches!(path, "/newh5/singles/songinfoandlrc" | "/yinyue/"),
                "不支持该酷我页面；请提供 kuwo:数字ID"
            );
            SongRef::new(Provider::Kuwo, required_query(&url, "musicId")?)
        }
        "lrclib.net" => {
            let id = path_id(path, "/api/get/", "")
                .context("LRCLIB 链接需 /api/get/数字ID；也可使用 lrclib:数字ID")?;
            SongRef::new(Provider::Lrclib, id)
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(value: &str, provider: Option<Provider>) -> SongRef {
        let Input::Song(song) = parse_input(OsStr::new(value), "auto", provider).unwrap() else {
            panic!("not a song")
        };
        song
    }

    #[test]
    fn recognizes_typed_ids_and_numeric_override() {
        assert_eq!(song("123", None).provider, Provider::Netease);
        assert_eq!(song("123", Some(Provider::Qq)).id, "id:123");
        assert_eq!(song("qq:id:123", None).id, "id:123");
        assert_eq!(song("qq:001ABCabcDEF12", None).id, "001ABCabcDEF12");
        assert_eq!(
            song("kugou:hash:abcdef0123456789abcdef0123456789", None).id,
            "ABCDEF0123456789ABCDEF0123456789"
        );
        assert_eq!(song("kugou:mix:1", None).id, "mix:1");
        assert!(parse_input(OsStr::new("123"), "auto", Some(Provider::Kugou)).is_err());
        assert!(parse_input(OsStr::new("netease:1"), "auto", Some(Provider::Qq)).is_err());
        assert!(matches!(
            parse_input(OsStr::new("123"), "query", None).unwrap(),
            Input::Query(_)
        ));
    }

    #[test]
    fn complete_platform_urls_are_decoded_without_requests() {
        for url in [
            "https://music.163.com/song?id=123",
            "https://music.163.com/#/song?id=123",
            "https://y.music.163.com/m/song?id=123&userid=9",
        ] {
            assert_eq!(song(url, None).id, "123");
        }
        assert_eq!(
            song("https://y.qq.com/n/ryqq/songDetail/001ABCabcDEF12", None).provider,
            Provider::Qq
        );
        assert_eq!(
            song("https://c.y.qq.com/base/fcgi-bin/u?songid=12", None).id,
            "id:12"
        );
        assert_eq!(
            song(
                "https://www.kugou.com/song/#hash=abcdef0123456789abcdef0123456789",
                None
            )
            .provider,
            Provider::Kugou
        );
        assert_eq!(song("https://www.kuwo.cn/play_detail/42", None).id, "42");
        assert_eq!(song("https://lrclib.net/api/get/5", None).id, "5");
    }

    #[test]
    fn rejects_url_confusion_ports_shortlinks_and_unknown_prefixes() {
        for value in [
            "https://music.163.com.evil.test/song?id=1",
            "https://evil.test/song?id=1",
            "https://music.163.com@evil.test/song?id=1",
            "https://@music.163.com/song?id=1",
            "https://music.163.com:443/song?id=1",
            "https://music.163.com:8443/song?id=1",
            "https://127.0.0.1/song?id=1",
            "https://[::1]/song?id=1",
            "file:///tmp/song",
            "https://music.163.com/song?id=1&id=2",
            "https://music.163.com/playlist?id=1",
            "https://163cn.tv/abc",
            "https://c6.y.qq.com/base/fcgi-bin/u?__=abc",
            "spotify:123",
            "https://music.163.com\\@evil.test/song?id=1",
            "//music.163.com/song?id=1",
        ] {
            assert!(
                parse_input(OsStr::new(value), "auto", None).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn file_priority_and_missing_path_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("123");
        fs::write(&path, []).unwrap();
        assert!(matches!(
            parse_input(path.as_os_str(), "auto", None).unwrap(),
            Input::File(_)
        ));
        assert!(parse_input(dir.path().as_os_str(), "auto", None).is_err());
        assert!(parse_input(dir.path().join("missing.wav").as_os_str(), "auto", None).is_err());
        assert!(parse_input(OsStr::new("missing.mp3"), "file", None).is_err());
        assert!(parse_input(OsStr::new("missing.mp3"), "auto", None).is_err());
        assert!(matches!(
            parse_input(OsStr::new("missing.mp3"), "query", None).unwrap(),
            Input::Query(_)
        ));
    }

    #[test]
    fn long_plain_keywords_are_not_limited_by_filename_length() {
        let dir = tempfile::tempdir().unwrap();
        for keywords in ["a".repeat(300), "歌".repeat(300)] {
            assert!(keywords.len() > 255 && keywords.len() <= 1024);
            let Input::Query(query) = parse_input(OsStr::new(&keywords), "auto", None).unwrap()
            else {
                panic!("long keywords were not classified as a query")
            };
            assert_eq!(query.keywords, keywords);
            assert!(parse_input(OsStr::new(&keywords), "file", None).is_err());
            assert!(parse_input(dir.path().join(&keywords).as_os_str(), "auto", None).is_err());
            assert!(parse_input(OsStr::new(&format!("{keywords}.mp3")), "auto", None).is_err());
        }
        assert!(parse_input(OsStr::new(&"a".repeat(1025)), "auto", None).is_err());
        assert!(parse_input(
            OsStr::new(&format!("netease:{}", "1".repeat(300))),
            "auto",
            None
        )
        .is_err());
    }

    #[test]
    fn long_complete_platform_url_is_decoded_without_requests() {
        let url = format!(
            "https://music.163.com/song?id=123&tracking={}",
            "a".repeat(300)
        );
        assert!(url.len() > 255 && url.len() <= 4096);
        assert_eq!(song(&url, None).id, "123");
        assert!(parse_input(OsStr::new(&url), "file", None).is_err());
    }

    #[test]
    fn filename_fallback_is_narrow_and_preserves_path_errors() {
        let invalid_filename = std::io::Error::from(ErrorKind::InvalidFilename);
        for text in [
            "a".repeat(300),
            "歌".repeat(300),
            format!("netease:{}", "1".repeat(300)),
            format!(
                "https://music.163.com/song?id=123&tracking={}",
                "a".repeat(300)
            ),
        ] {
            assert!(filename_error_allows_text(
                Path::new(&text),
                &invalid_filename
            ));
        }
        for text in [
            format!("./{}", "a".repeat(300)),
            format!("missing/{}", "a".repeat(300)),
            format!("missing\\{}", "a".repeat(300)),
            format!("{}.mp3", "a".repeat(300)),
        ] {
            assert!(!filename_error_allows_text(
                Path::new(&text),
                &invalid_filename
            ));
        }
        for kind in [
            ErrorKind::PermissionDenied,
            ErrorKind::InvalidInput,
            ErrorKind::NotADirectory,
            ErrorKind::Other,
        ] {
            let error = std::io::Error::from(kind);
            assert!(!filename_error_allows_text(Path::new("keywords"), &error));
            assert!(!filename_error_allows_text(
                Path::new("https://music.163.com/song?id=123"),
                &error
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_files_are_supported_when_the_filesystem_accepts_them() {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join(std::ffi::OsString::from_vec(b"song\xff.wav".to_vec()));
        if !crate::test_support::create_fixture("non_utf8_input", &path, &[]).unwrap() {
            return;
        }
        assert!(matches!(
            parse_input(path.as_os_str(), "auto", None).unwrap(),
            Input::File(_)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlinks_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("dangling");
        std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
        assert!(parse_input(link.as_os_str(), "auto", None).is_err());
    }
}
