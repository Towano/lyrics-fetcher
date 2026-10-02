use anyhow::{bail, ensure, Context, Result};
use http::Transport;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::Builder;

mod http;
pub mod input;
mod lrc;
pub mod matching;
#[cfg(feature = "cli")]
pub mod metadata;
pub mod model;
mod providers;
pub mod service;

const MAX_LYRIC_SIZE: usize = http::MAX_LYRIC_SIZE;

#[derive(Debug, PartialEq, Eq)]
pub enum LyricsResult {
    Local,
    Online,
    MissingId,
    NoLyrics,
    OutputExists,
}

pub fn save_for_track(
    music_id: Option<&str>,
    source: &Path,
    output: &Path,
) -> Result<LyricsResult> {
    let destination = output.with_extension("lrc");
    if output_exists(&destination)? {
        return Ok(LyricsResult::OutputExists);
    }
    for local in matching_lrc(source).into_iter().chain(matching_lrc(output)) {
        if let Ok(file) = fs::File::open(&local) {
            let contents = http::bounded_read(file, MAX_LYRIC_SIZE as u64)
                .with_context(|| format!("读取本地歌词 {} 失败", local.display()))?;
            if !contents.is_empty() {
                return Ok(if write_new(&destination, &contents)? {
                    LyricsResult::Local
                } else {
                    LyricsResult::OutputExists
                });
            }
        }
    }

    let Some(music_id) = music_id else {
        return Ok(LyricsResult::MissingId);
    };
    let Some(contents) = fetch_lyric(music_id)? else {
        return Ok(LyricsResult::NoLyrics);
    };
    Ok(if write_new(&destination, contents.as_bytes())? {
        LyricsResult::Online
    } else {
        LyricsResult::OutputExists
    })
}

pub fn output_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("无法检查歌词路径 {}", path.display())),
    }
}

pub fn save_lrc(path: &Path, lyric: &str) -> Result<bool> {
    let model::Lyric::Synced(lyric) = lrc::parse(lyric)? else {
        bail!("没有有效普通同步 LRC，未保存");
    };
    write_new(path, lyric.as_bytes())
}

fn matching_lrc(audio: &Path) -> Vec<PathBuf> {
    let Some(stem) = audio.file_stem() else {
        return Vec::new();
    };
    let parent = audio
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut matches = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_str()?;
            let candidate_stem = path.file_stem()?;
            let same_stem = match (candidate_stem.to_str(), stem.to_str()) {
                (Some(candidate), Some(original)) => candidate.eq_ignore_ascii_case(original),
                _ => candidate_stem == stem,
            };
            (extension.eq_ignore_ascii_case("lrc")
                && same_stem
                && fs::metadata(&path).ok()?.is_file())
            .then_some(path)
        })
        .collect::<Vec<_>>();
    // Exact stems win on case-sensitive filesystems; never replace their empty
    // lyric with a differently cased song's lyric. Extensions remain flexible.
    if matches.iter().any(|path| path.file_stem() == Some(stem)) {
        matches.retain(|path| path.file_stem() == Some(stem));
    }
    matches.sort();
    matches
}

fn fetch_lyric(music_id: &str) -> Result<Option<String>> {
    ensure!(
        !music_id.is_empty()
            && music_id.len() <= 20
            && music_id.bytes().all(|byte| byte.is_ascii_digit()),
        "网易云歌曲 ID 无效"
    );
    let request = http::Request::get("https://music.163.com/api/song/lyric/v1")
        .query("tv", 0)
        .query("lv", 0)
        .query("rv", 0)
        .query("kv", 0)
        .query("yv", 0)
        .query("ytv", 0)
        .query("yrv", 0)
        .query("cp", "false")
        .query("id", music_id);
    let body = http::Http::default()
        .send(request)
        .context("网易云歌词请求失败")?;
    parse_lyric_response(&body)
}

fn parse_lyric_response(body: &[u8]) -> Result<Option<String>> {
    let response: Value = serde_json::from_slice(body).context("网易云歌词响应 JSON 无效")?;
    let code = response
        .get("code")
        .and_then(Value::as_i64)
        .or_else(|| response.get("code").and_then(Value::as_str)?.parse().ok());
    let code = code.context("网易云歌词接口缺少有效状态码")?;
    if code != 200 {
        bail!("网易云歌词接口返回状态码 {code}");
    }
    let Some(lyric) = response
        .pointer("/lrc/lyric")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|lyric| !lyric.is_empty())
    else {
        return Ok(None);
    };
    let model::Lyric::Synced(lyric) = lrc::netease(lyric)? else {
        return Ok(None);
    };
    ensure!(lyric.len() <= MAX_LYRIC_SIZE, "歌词内容超出大小限制");
    Ok(Some(lyric))
}

fn write_new(path: &Path, contents: &[u8]) -> Result<bool> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = Builder::new()
        .prefix(".lyrics-fetcher-")
        .tempfile_in(parent)
        .with_context(|| format!("无法在 {} 创建歌词临时文件", parent.display()))?;
    temp.write_all(contents)?;
    temp.as_file_mut().sync_all()?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(true),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => {
            Err(error.error).with_context(|| format!("无法保存歌词文件 {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn create_distinct_case_variant(path: &Path, contents: &[u8]) -> Result<bool> {
        // Probe the actual filesystem, not the OS: macOS/Windows volumes may
        // be case-sensitive, and Unix volumes may be case-insensitive.
        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        file.write_all(contents)?;
        Ok(true)
    }

    #[test]
    fn parses_timed_lyric_response() -> Result<()> {
        let body =
            r#"{"code":200,"lrc":{"version":1,"lyric":"[00:01.00]第一句\n[00:02.00]第二句"}}"#
                .as_bytes();
        let lyric = parse_lyric_response(body)?.unwrap();
        assert_eq!(lyric, "[00:01.00]第一句\n[00:02.00]第二句\n");
        Ok(())
    }

    #[test]
    fn converts_credit_json_to_timed_lrc_lines() -> Result<()> {
        let body = r#"{"code":200,"lrc":{"version":1,"lyric":"{\"t\":0,\"c\":[{\"tx\":\"作词: \"},{\"tx\":\"Mili\"}]}\n[00:01.00]第一句"}}"#.as_bytes();
        let lyric = parse_lyric_response(body)?.unwrap();
        assert_eq!(lyric, "[00:00.00]作词: Mili\n[00:01.00]第一句\n");
        Ok(())
    }

    #[test]
    fn rejects_non_success_response() {
        assert!(parse_lyric_response(br#"{"code":404}"#).is_err());
    }

    #[test]
    fn missing_lyrics_returns_none() -> Result<()> {
        assert_eq!(
            parse_lyric_response(br#"{"code":200,"nolyric":true}"#)?,
            None
        );
        let credits_only = r#"{"code":200,"lrc":{"lyric":"{\"t\":0,\"c\":[{\"tx\":\"作词: \"},{\"tx\":\"Mili\"}]}"}}"#.as_bytes();
        assert_eq!(parse_lyric_response(credits_only)?, None);
        Ok(())
    }

    #[test]
    fn invalid_music_id_is_rejected_before_request() {
        assert!(fetch_lyric("not-a-number").is_err());
    }

    #[test]
    fn prefers_local_lrc_and_saves_it_beside_output() -> Result<()> {
        let source_dir = tempdir()?;
        let output_dir = tempdir()?;
        let source = source_dir.path().join("song.ncm");
        let local = source_dir.path().join("song.LRC");
        let output = output_dir.path().join("song.flac");
        let expected = b"[00:01.00]local lyrics\n";
        fs::write(&local, expected)?;

        assert_eq!(
            save_for_track(Some("invalid-id"), &source, &output)?,
            LyricsResult::Local
        );
        assert_eq!(fs::read(output.with_extension("lrc"))?, expected);
        Ok(())
    }

    #[test]
    fn existing_output_short_circuits_invalid_id() -> Result<()> {
        let dir = tempdir()?;
        let output = dir.path().join("song.flac");
        fs::write(output.with_extension("lrc"), b"keep")?;
        assert_eq!(
            save_for_track(Some("invalid"), &dir.path().join("missing.flac"), &output)?,
            LyricsResult::OutputExists
        );
        Ok(())
    }

    #[test]
    fn empty_input_lrc_does_not_hide_output_side_lrc() -> Result<()> {
        let input_dir = tempdir()?;
        let output_dir = tempdir()?;
        let source = input_dir.path().join("source.flac");
        let output = output_dir.path().join("target.flac");
        let local = output_dir.path().join("target.LRC");
        let destination = output.with_extension("lrc");
        fs::write(source.with_extension("lrc"), b"")?;
        fs::write(&local, b"local bytes")?;
        let distinct_destination = create_distinct_case_variant(&destination, b"probe")?;
        let expected = if distinct_destination {
            fs::remove_file(&destination)?;
            LyricsResult::Local
        } else {
            // target.LRC already occupies target.lrc on this filesystem.
            LyricsResult::OutputExists
        };
        assert_eq!(save_for_track(None, &source, &output)?, expected);
        assert_eq!(fs::read(&destination)?, b"local bytes");
        assert_eq!(fs::read(&local)?, b"local bytes");
        Ok(())
    }

    #[test]
    fn empty_first_extension_does_not_hide_other_local_candidate() -> Result<()> {
        let input_dir = tempdir()?;
        let output_dir = tempdir()?;
        let source = input_dir.path().join("song.flac");
        let output = output_dir.path().join("target.flac");
        let empty = input_dir.path().join("song.LRC");
        let usable = input_dir.path().join("song.lrc");
        fs::write(&empty, [])?;
        if !create_distinct_case_variant(&usable, b"usable")? {
            assert!(fs::read(&empty)?.is_empty());
            return Ok(());
        }
        assert_eq!(save_for_track(None, &source, &output)?, LyricsResult::Local);
        assert_eq!(fs::read(output.with_extension("lrc"))?, b"usable");
        assert!(fs::read(&empty)?.is_empty());
        assert_eq!(fs::read(&usable)?, b"usable");
        Ok(())
    }

    #[test]
    fn exact_case_stem_wins_over_different_recording() -> Result<()> {
        let input_dir = tempdir()?;
        let output_dir = tempdir()?;
        let source = input_dir.path().join("song.flac");
        let output = output_dir.path().join("target.flac");
        let different = input_dir.path().join("Song.lrc");
        let exact = input_dir.path().join("song.lrc");
        fs::write(&different, b"wrong")?;
        if !create_distinct_case_variant(&exact, b"right")? {
            assert_eq!(fs::read(&different)?, b"wrong");
            return Ok(());
        }
        assert_eq!(save_for_track(None, &source, &output)?, LyricsResult::Local);
        assert_eq!(fs::read(output.with_extension("lrc"))?, b"right");
        assert_eq!(fs::read(&different)?, b"wrong");
        assert_eq!(fs::read(&exact)?, b"right");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn local_regular_file_symlink_is_read_only() -> Result<()> {
        let dir = tempdir()?;
        let original = dir.path().join("original.txt");
        let source = dir.path().join("song.flac");
        let output = dir.path().join("target.flac");
        fs::write(&original, b"linked")?;
        std::os::unix::fs::symlink(&original, source.with_extension("lrc"))?;
        assert_eq!(save_for_track(None, &source, &output)?, LyricsResult::Local);
        assert_eq!(fs::read(output.with_extension("lrc"))?, b"linked");
        assert_eq!(fs::read(original)?, b"linked");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn invalid_utf8_stems_do_not_collide() -> Result<()> {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempdir()?;
        let source = dir
            .path()
            .join(std::ffi::OsString::from_vec(b"song\xff.flac".to_vec()));
        let wrong = dir
            .path()
            .join(std::ffi::OsString::from_vec(b"song\xfe.lrc".to_vec()));
        fs::write(wrong, b"wrong")?;
        assert_eq!(
            save_for_track(None, &source, &dir.path().join("target.flac"))?,
            LyricsResult::MissingId
        );
        Ok(())
    }

    #[test]
    fn bounds_local_lyric() -> Result<()> {
        let dir = tempdir()?;
        let source = dir.path().join("source.flac");
        let output = dir.path().join("output.flac");
        fs::write(source.with_extension("lrc"), vec![b'a'; MAX_LYRIC_SIZE + 1])?;
        assert!(save_for_track(None, &source, &output).is_err());
        assert!(!output.with_extension("lrc").exists());
        Ok(())
    }

    #[test]
    fn rejects_missing_code_and_plain_online_lyrics() -> Result<()> {
        assert!(parse_lyric_response(br#"{"lrc":{"lyric":"[00:01]text"}}"#).is_err());
        assert_eq!(
            parse_lyric_response(br#"{"code":200,"lrc":{"lyric":"plain"}}"#)?,
            None
        );
        Ok(())
    }

    #[test]
    fn concurrent_saves_never_overwrite() -> Result<()> {
        let dir = tempdir()?;
        let path = std::sync::Arc::new(dir.path().join("race.lrc"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    write_new(&path, format!("winner{i}").as_bytes()).unwrap()
                })
            })
            .collect();
        let winners = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(winners, 1);
        assert!(fs::read_to_string(&*path)?.starts_with("winner"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_an_existing_target() -> Result<()> {
        let dir = tempdir()?;
        let target = dir.path().join("song.lrc");
        std::os::unix::fs::symlink(dir.path().join("missing"), &target)?;
        assert!(output_exists(&target)?);
        assert!(!save_lrc(&target, "[00:01]text")?);
        assert!(fs::symlink_metadata(&target)?.file_type().is_symlink());
        Ok(())
    }

    #[test]
    fn existing_output_lrc_is_never_overwritten() -> Result<()> {
        let source_dir = tempdir()?;
        let output_dir = tempdir()?;
        let source = source_dir.path().join("song.ncm");
        let local = source_dir.path().join("song.lrc");
        let output = output_dir.path().join("song.flac");
        let destination = output.with_extension("lrc");
        fs::write(&local, b"local lyrics\n")?;
        fs::write(&destination, b"keep existing lyrics\n")?;

        assert_eq!(
            save_for_track(None, &source, &output)?,
            LyricsResult::OutputExists
        );
        assert_eq!(fs::read(destination)?, b"keep existing lyrics\n");
        Ok(())
    }
}
