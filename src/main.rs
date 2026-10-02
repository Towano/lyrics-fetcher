use anyhow::{ensure, Context, Result};
use clap::builder::{OsStringValueParser, PathBufValueParser};
use clap::{Arg, ArgAction, ArgMatches, Command};
use lyrics_fetcher::input::{parse_input, Input};
use lyrics_fetcher::metadata::read_file;
use lyrics_fetcher::model::{Candidate, Lyric, Provider, Query, SongRef};
use lyrics_fetcher::service::{Client, Lookup, Outcome, Resolution};
use lyrics_fetcher::{output_exists, save_lrc};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn command() -> Command {
    Command::new("lyrics-fetcher")
        .version(env!("CARGO_PKG_VERSION"))
        .about("联网查找普通同步 LRC 歌词；只新增文件，不覆盖已有歌词")
        .after_help("示例:\n  lyrics-fetcher -l 123456\n  lyrics-fetcher --lyrics '歌名 歌手'\n  lyrics-fetcher -l './song.flac'\n  lyrics-fetcher -l '歌名' --title '歌名' --artist '歌手' --provider lrclib\n  lyrics-fetcher -l '歌名 歌手' --select 'qq:id:123456'\n\n自动输入: 已有普通文件优先，随后完整平台歌曲链接、来源:ID、裸数字（--provider auto 时默认网易云，显式平台按其 ID 规则）。\n数字歌名请加 --input-type query。QQ 可用 qq:MID 或 qq:id:数字。\n酷狗独立 hash/mix 输入暂无精确歌曲详情支持；hash 候选请保留原关键词命令并添加 --select 'kugou:hash'。\n不执行输入链接，不跟随短链。关键词和无标签文件不会自动选第一首，请按候选提示加 --select。\n默认保存: 音频同目录同 stem 的 .lrc；其他输入为 provider-id.lrc。\n退出码: 0 已保存，1 网络/文件失败，2 参数错误，3 无同步歌词，4 需明确候选，5 目标已存在。")
        .arg(Arg::new("lyrics").short('l').long("lyrics").value_name("INPUT").required(true)
            .value_parser(OsStringValueParser::new())
            .help("音频文件路径、关键词、来源:ID 或完整歌曲链接（支持非 UTF-8 文件路径）"))
        .arg(Arg::new("input-type").long("input-type").default_value("auto")
            .value_parser(["auto", "query", "id", "file"])
            .help("显式解释输入；不存在的路径和未知链接不会作为关键词"))
        .arg(Arg::new("provider").long("provider").default_value("auto")
            .value_parser(["auto", "netease", "qq", "kugou", "kuwo", "lrclib"])
            .help("auto 允许跨平台查询；指定平台则不跨源补查"))
        .arg(Arg::new("title").long("title").value_name("TITLE").value_parser(nonempty_text)
            .help("可信歌名；用于严格匹配，保留 Live/Remix 等版本信息"))
        .arg(Arg::new("artist").long("artist").value_name("ARTIST").action(ArgAction::Append)
            .value_parser(nonempty_text).help("可信歌手，可重复；完整替代文件歌手标签"))
        .arg(Arg::new("album").long("album").value_name("ALBUM").value_parser(nonempty_text)
            .help("可信专辑；给出后候选必须包含且匹配专辑"))
        .arg(Arg::new("duration").long("duration").value_name("SECONDS").value_parser(positive_duration)
            .help("可信时长，有限正数秒；自动匹配容差为 ±2 秒"))
        .arg(Arg::new("select").long("select").value_name("PROVIDER:ID").value_parser(parse_selection)
            .help("显式选择候选的完整来源:ID；用于文件或关键词输入"))
        .arg(Arg::new("output").short('o').long("output").value_name("PATH")
            .value_parser(PathBufValueParser::new()).help("准确输出文件路径；不会自动改扩展名，不覆盖文件或符号链接"))
}

fn nonempty_text(value: &str) -> std::result::Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 1024 {
        Err("元数据不能为空且不得超过 1024 bytes".into())
    } else {
        Ok(value.into())
    }
}

fn positive_duration(value: &str) -> std::result::Result<f64, String> {
    let duration: f64 = value
        .parse()
        .map_err(|_| "时长必须是有限正数秒".to_owned())?;
    if duration.is_finite() && duration > 0.0 {
        Ok(duration)
    } else {
        Err("时长必须是有限正数秒".into())
    }
}

fn parse_selection(value: &str) -> std::result::Result<SongRef, String> {
    value
        .parse()
        .map_err(|error: anyhow::Error| error.to_string())
}

#[derive(Debug)]
struct Options {
    input: Input,
    provider: Option<Provider>,
    selection: Option<SongRef>,
    output: Option<PathBuf>,
    title: Option<String>,
    artists: Vec<String>,
    album: Option<String>,
    duration: Option<f64>,
}

fn options(matches: &ArgMatches) -> Result<Options> {
    let source = matches
        .get_one::<String>("provider")
        .context("缺少平台参数")?;
    let provider = if source == "auto" {
        None
    } else {
        Some(source.parse()?)
    };
    let value = matches
        .get_one::<OsString>("lyrics")
        .context("缺少 -l/--lyrics")?;
    let mode = matches
        .get_one::<String>("input-type")
        .context("缺少输入类型")?;
    let input = parse_input(value, mode, provider)?;
    let selection = matches.get_one::<SongRef>("select").cloned();
    let title = matches.get_one::<String>("title").cloned();
    let artists: Vec<_> = matches
        .get_many::<String>("artist")
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let album = matches.get_one::<String>("album").cloned();
    let duration = matches.get_one::<f64>("duration").copied();
    ensure!(artists.len() <= 64, "--artist 最多可重复 64 次");
    if matches!(input, Input::Song(_)) {
        ensure!(
            title.is_none() && artists.is_empty() && album.is_none() && duration.is_none(),
            "歌曲 ID/链接输入不能添加元数据覆盖；要按元数据搜索请使用 --input-type query"
        );
        ensure!(
            selection.is_none(),
            "歌曲 ID/链接输入不能另外 --select；请直接使用选定的来源:ID"
        );
    }
    if let Some(song) = &selection {
        ensure!(
            provider.is_none_or(|provider| song.provider == provider),
            "--select 的平台与 --provider 冲突"
        );
    }
    let output = matches.get_one::<PathBuf>("output").cloned();
    ensure!(
        output
            .as_ref()
            .is_none_or(|path| !path.as_os_str().is_empty()),
        "输出路径不能为空"
    );
    let options = Options {
        input,
        provider,
        selection,
        output,
        title,
        artists,
        album,
        duration,
    };
    if let Input::Query(query) = &options.input {
        let query = merge_metadata(query.clone(), &options);
        ensure!(
            query.search_text().len() <= 1024,
            "搜索文字过长 (最多 1024 bytes)"
        );
    }
    Ok(options)
}

fn merge_metadata(mut query: Query, options: &Options) -> Query {
    if let Some(title) = &options.title {
        query.title = Some(title.clone());
    }
    if !options.artists.is_empty() {
        query.artists = options.artists.clone();
    }
    if let Some(album) = &options.album {
        query.album = Some(album.clone());
    }
    if let Some(duration) = options.duration {
        query.duration = Some(duration);
    }
    // Explicit title/artist metadata is the search identity, not a misleading
    // untagged filename or old free-form input. Keep bare keywords otherwise.
    if options.title.is_some() || (!options.artists.is_empty() && query.title.is_some()) {
        query.keywords.clear();
    }
    query
}

fn default_song_output(song: &SongRef) -> PathBuf {
    PathBuf::from(format!(
        "{}-{}.lrc",
        song.provider,
        song.id.replace(':', "-")
    ))
}

fn early_output(options: &Options) -> Option<PathBuf> {
    options.output.clone().or_else(|| match &options.input {
        Input::File(path) => Some(path.with_extension("lrc")),
        Input::Song(song) => Some(default_song_output(song)),
        Input::Query(_) => options.selection.as_ref().map(default_song_output),
    })
}

fn exists(path: &Path) -> Result<bool> {
    let exists = output_exists(path)?;
    if exists {
        eprintln!("目标已存在，未覆盖: {:?}", path);
    }
    Ok(exists)
}

fn show_candidates(candidates: &[Candidate]) {
    eprintln!(
        "无法唯一确定歌曲或歌词版本（{} 个候选）；未写文件。",
        candidates.len()
    );
    for candidate in candidates {
        let duration = candidate
            .duration
            .filter(|duration| duration.is_finite() && *duration > 0.0)
            .map(|duration| format!("{duration:.2}s"))
            .unwrap_or_else(|| "未知时长".into());
        // Debug formatting escapes control characters in untrusted API text.
        eprintln!(
            "  {} | {:?} | {:?} | 专辑 {:?} | {}",
            candidate.song, candidate.title, candidate.artists, candidate.album, duration
        );
        if candidate.song.provider != Provider::Kugou {
            eprintln!(
                "    独立选择: lyrics-fetcher -l '{}' --provider {}",
                candidate.song, candidate.song.provider
            );
        }
        eprintln!(
            "    文件/关键词输入可重新运行原命令并添加: --select '{}'",
            candidate.song
        );
    }
    eprintln!(
        "候选歌词仍需同步 LRC 校验；--title/--artist/--album/--duration 可提供更强匹配线索。"
    );
}

fn print_output(path: &Path) -> Result<()> {
    let mut stdout = io::stdout().lock();
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        stdout.write_all(path.as_os_str().as_bytes())?;
    }
    #[cfg(not(unix))]
    stdout.write_all(path.to_string_lossy().as_bytes())?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn run(options: Options) -> Result<u8> {
    run_with_resolver(options, |lookup, provider, selection| {
        Client::default().resolve(lookup, provider, selection)
    })
}

fn run_with_resolver(
    options: Options,
    resolve: impl FnOnce(Lookup, Option<Provider>, Option<&SongRef>) -> Result<Resolution>,
) -> Result<u8> {
    let output = early_output(&options);
    if let Some(path) = &output {
        if exists(path)? {
            return Ok(5);
        }
    }
    let lookup = match &options.input {
        Input::Song(song) => Lookup::Song(song.clone()),
        Input::Query(query) => Lookup::Query(merge_metadata(query.clone(), &options)),
        Input::File(path) => {
            let query = merge_metadata(read_file(path)?, &options);
            if query.title.is_none() || query.artists.is_empty() {
                eprintln!("文件标签不足；文件名仅作为低可信搜索线索，不会自动选第一首。");
            }
            Lookup::Query(query)
        }
    };
    let result = resolve(lookup, options.provider, options.selection.as_ref())?;
    for diagnostic in result.diagnostics {
        eprintln!("{diagnostic}");
    }
    match result.outcome {
        Outcome::Found { song, lyric } => {
            let destination = output.unwrap_or_else(|| default_song_output(&song));
            if exists(&destination)? {
                return Ok(5);
            }
            if save_lrc(&destination, &lyric)? {
                print_output(&destination)?;
                Ok(0)
            } else {
                eprintln!("目标已存在，未覆盖: {:?}", destination);
                Ok(5)
            }
        }
        Outcome::Candidates(candidates) => {
            show_candidates(&candidates);
            Ok(4)
        }
        Outcome::Unavailable(kind) => {
            eprintln!(
                "{}",
                match kind {
                    Lyric::PlainOnly => "仅有纯文本歌词，没有可保存的同步 LRC。",
                    Lyric::Instrumental => "该歌曲标记为纯音乐，没有可保存的同步 LRC。",
                    Lyric::NotFound => "未找到同步 LRC 歌词。",
                    Lyric::Synced(_) => "服务未返回可保存的同步歌词。",
                }
            );
            Ok(3)
        }
    }
}

fn main() -> ExitCode {
    entry_with_runner(std::env::args_os(), run)
}

fn entry_with_runner<I, T>(arguments: I, run: impl FnOnce(Options) -> Result<u8>) -> ExitCode
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = match command().try_get_matches_from(arguments) {
        Ok(matches) => matches,
        Err(error) => {
            let success = matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            );
            if let Err(print_error) = error.print() {
                eprintln!("无法输出参数帮助: {print_error}");
            }
            return ExitCode::from(if success { 0 } else { 2 });
        }
    };
    let options = match options(&matches) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("参数错误: {error:#}");
            return ExitCode::from(2);
        }
    };
    match run(options) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("错误: {error:#}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(arguments: &[&str]) -> Result<Options> {
        let matches = command().try_get_matches_from(arguments)?;
        options(&matches)
    }

    #[test]
    fn accepts_short_long_aliases_and_repeatable_artists() {
        for flag in ["-l", "--lyrics"] {
            let options = parse(&[
                "lyrics-fetcher",
                flag,
                "song",
                "--title",
                "Song",
                "--artist",
                "Alice",
                "--artist",
                "Bob",
                "--duration",
                "180.5",
            ])
            .unwrap();
            assert_eq!(options.artists, ["Alice", "Bob"]);
            assert_eq!(options.duration, Some(180.5));
            let Input::Query(query) = &options.input else {
                panic!("expected query")
            };
            let merged = merge_metadata(query.clone(), &options);
            assert!(merged.keywords.is_empty());
            assert_eq!(merged.search_text(), "Alice Bob Song");
        }
    }

    #[test]
    fn artist_only_override_preserves_the_song_search_hint() {
        let options = parse(&["lyrics-fetcher", "-l", "Song", "--artist", "Alice"]).unwrap();
        let Input::Query(query) = &options.input else {
            panic!("expected query")
        };
        let merged = merge_metadata(query.clone(), &options);
        assert_eq!(merged.keywords, "Song");
        assert_eq!(merged.artists, ["Alice"]);
        assert!(merged.title.is_none());
    }

    #[test]
    fn existing_file_output_short_circuits_invalid_audio_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("broken.wav");
        let destination = source.with_extension("lrc");
        std::fs::write(&source, b"invalid audio").unwrap();
        std::fs::write(&destination, b"keep").unwrap();
        let matches = command()
            .try_get_matches_from([
                OsString::from("lyrics-fetcher"),
                OsString::from("-l"),
                source.into_os_string(),
            ])
            .unwrap();
        assert_eq!(run(options(&matches).unwrap()).unwrap(), 5);
        assert_eq!(std::fs::read(destination).unwrap(), b"keep");
    }

    #[test]
    fn rejects_invalid_duration_and_missing_input() {
        assert!(command().try_get_matches_from(["lyrics-fetcher"]).is_err());
        for duration in ["NaN", "inf", "0", "-1", "nonsense"] {
            assert!(parse(&["lyrics-fetcher", "-l", "song", "--duration", duration]).is_err());
        }
        assert!(parse(&["lyrics-fetcher", "-l", "song", "--title", " "]).is_err());
        assert!(parse(&["lyrics-fetcher", "-l", "song", "--provider", "invalid"]).is_err());
        assert!(parse(&["lyrics-fetcher", "-l", "song", "--select", "qq:invalid"]).is_err());
    }

    #[test]
    fn rejects_song_metadata_and_selection_conflicts() {
        assert!(parse(&["lyrics-fetcher", "-l", "netease:1", "--title", "Song"]).is_err());
        assert!(parse(&["lyrics-fetcher", "-l", "netease:1", "--select", "lrclib:5"]).is_err());
        assert!(parse(&[
            "lyrics-fetcher",
            "-l",
            "song",
            "--provider",
            "qq",
            "--select",
            "lrclib:5"
        ])
        .is_err());
        assert!(parse(&[
            "lyrics-fetcher",
            "-l",
            "123",
            "--input-type",
            "query",
            "--title",
            "123"
        ])
        .is_ok());
    }

    #[test]
    fn defaults_are_safe_and_explicit_output_is_exact() {
        assert_eq!(
            default_song_output(&SongRef::new(Provider::Qq, "id:123").unwrap()),
            Path::new("qq-id-123.lrc")
        );
        assert_eq!(
            default_song_output(&SongRef::new(Provider::Kugou, "mix:12").unwrap()),
            Path::new("kugou-mix-12.lrc")
        );
        let options = parse(&[
            "lyrics-fetcher",
            "-l",
            "song",
            "--select",
            "lrclib:5",
            "-o",
            "exact.file",
        ])
        .unwrap();
        assert_eq!(early_output(&options), Some(PathBuf::from("exact.file")));
    }

    #[test]
    fn existing_output_exits_before_network() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("song.lrc");
        std::fs::write(&path, b"keep").unwrap();
        let matches = command()
            .try_get_matches_from([
                OsString::from("lyrics-fetcher"),
                OsString::from("-l"),
                OsString::from("netease:1"),
                OsString::from("-o"),
                path.as_os_str().to_owned(),
            ])
            .unwrap();
        assert_eq!(run(options(&matches).unwrap()).unwrap(), 5);
        assert_eq!(std::fs::read(path).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn preserves_non_utf8_cli_input_and_output_paths() {
        use std::os::unix::ffi::OsStringExt;
        let directory = tempfile::tempdir().unwrap();
        let input = directory
            .path()
            .join(OsString::from_vec(b"track\xff.wav".to_vec()));
        let output = directory
            .path()
            .join(OsString::from_vec(b"track\xfe.lrc".to_vec()));
        std::fs::write(&input, []).unwrap();
        let matches = command()
            .try_get_matches_from([
                OsString::from("lyrics-fetcher"),
                OsString::from("--lyrics"),
                input.as_os_str().to_owned(),
                OsString::from("--output"),
                output.as_os_str().to_owned(),
            ])
            .unwrap();
        let options = options(&matches).unwrap();
        let Input::File(path) = options.input else {
            panic!("expected file")
        };
        assert_eq!(path, input);
        assert_eq!(options.output, Some(output));
    }
}
