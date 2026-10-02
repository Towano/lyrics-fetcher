use crate::http::MAX_LYRIC_SIZE;
use crate::model::Lyric;
use anyhow::{ensure, Result};
use serde_json::Value;

pub(crate) fn timestamp(milliseconds: u64) -> String {
    let centiseconds = milliseconds.saturating_add(5) / 10;
    format!(
        "[{:02}:{:02}.{:02}]",
        centiseconds / 6000,
        centiseconds / 100 % 60,
        centiseconds % 100
    )
}

fn time_tag(tag: &str) -> bool {
    let Some((minutes, rest)) = tag.split_once(':') else {
        return false;
    };
    if minutes.is_empty() || minutes.len() > 6 || !minutes.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let (seconds, fraction) = rest
        .split_once(['.', ':'])
        .map_or((rest, None), |(s, f)| (s, Some(f)));
    seconds.len() == 2
        && seconds.bytes().all(|b| b.is_ascii_digit())
        && seconds.parse::<u8>().is_ok_and(|s| s < 60)
        && fraction
            .is_none_or(|f| !f.is_empty() && f.len() <= 3 && f.bytes().all(|b| b.is_ascii_digit()))
}

fn metadata(line: &str) -> bool {
    let Some(tag) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    let Some((key, _)) = tag.split_once(':') else {
        return false;
    };
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphabetic())
}

fn timed_text(mut line: &str) -> Option<&str> {
    let mut count = 0;
    while let Some(after) = line.strip_prefix('[') {
        if !after.as_bytes().first().is_some_and(u8::is_ascii_digit) {
            break;
        }
        let (tag, rest) = after.split_once(']')?;
        if !time_tag(tag) {
            return None;
        }
        count += 1;
        line = rest;
    }
    (count > 0).then_some(line.trim())
}

fn word_tags(line: &str) -> bool {
    line.split('<').skip(1).any(|rest| {
        let Some((tag, _)) = rest.split_once('>') else {
            return false;
        };
        time_tag(tag) || {
            let fields: Vec<_> = tag.split(',').collect();
            (2..=3).contains(&fields.len())
                && fields
                    .iter()
                    .all(|field| !field.is_empty() && field.bytes().all(|b| b.is_ascii_digit()))
        }
    })
}

pub(crate) fn parse(input: &str) -> Result<Lyric> {
    ensure!(input.len() <= MAX_LYRIC_SIZE, "歌词内容超出大小限制");
    let normalized = input
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut lines = Vec::new();
    let mut has_body = false;
    let mut plain = false;
    for raw in normalized.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        ensure!(
            !line.starts_with('<') && !line.starts_with('{'),
            "歌词包含非 LRC 的 HTML/XML/JSON 数据"
        );
        ensure!(!word_tags(line), "不支持逐字歌词格式");
        if metadata(line) {
            lines.push(line.to_owned());
        } else if let Some(text) = timed_text(line) {
            has_body |= !text.is_empty();
            lines.push(line.to_owned());
        } else {
            ensure!(
                !line.starts_with('[') || !line.as_bytes().get(1).is_some_and(u8::is_ascii_digit),
                "歌词时间标签无效"
            );
            plain = true;
        }
    }
    if !has_body {
        return Ok(if plain {
            Lyric::PlainOnly
        } else {
            Lyric::NotFound
        });
    }
    ensure!(!plain, "同步歌词混入无时间标签内容");
    let output = format!("{}\n", lines.join("\n"));
    ensure!(output.len() <= MAX_LYRIC_SIZE, "规范化歌词超出大小限制");
    Ok(Lyric::Synced(output))
}

pub(crate) fn netease(input: &str) -> Result<Lyric> {
    ensure!(input.len() <= MAX_LYRIC_SIZE, "歌词内容超出大小限制");
    let normalized = input
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut credits = Vec::new();
    let mut body = Vec::new();
    for raw in normalized.lines() {
        let line = raw.trim();
        if line.starts_with('{') {
            let credit: Value = serde_json::from_str(line)?;
            let time = credit
                .get("t")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("网易云署名信息缺少有效时间"))?;
            let parts = credit
                .get("c")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow::anyhow!("网易云署名信息缺少文本列表"))?;
            let text = parts
                .iter()
                .map(|part| {
                    part.get("tx")
                        .and_then(Value::as_str)
                        .ok_or_else(|| anyhow::anyhow!("网易云署名信息文本类型无效"))
                })
                .collect::<Result<Vec<_>>>()?
                .concat();
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
                credits.push(format!("{}{line}", timestamp(time)));
            }
        } else {
            body.push(line);
        }
    }
    match parse(&body.join("\n"))? {
        Lyric::Synced(lyric) => {
            if credits.is_empty() {
                return Ok(Lyric::Synced(lyric));
            }
            parse(&format!("{}\n{lyric}", credits.join("\n")))
        }
        other => Ok(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_multi_tags_and_metadata() {
        assert_eq!(
            parse("\u{feff}[ar:artist]\r\n[00:01][01:02.123]歌词\r\n[01:05.2]\n").unwrap(),
            Lyric::Synced("[ar:artist]\n[00:01][01:02.123]歌词\n[01:05.2]\n".into())
        );
    }
    #[test]
    fn preserves_nonnumeric_brackets_as_timed_body() {
        for line in [
            "[00:01][Chorus]",
            "[00:01][01:02.123][Chorus] words",
            "[00:01][Chorus",
            "[00:01][00:02][Chorus",
            "[00:01][",
            "[00:01][]",
            "[00:01][Chorus][00:99] words",
        ] {
            assert_eq!(parse(line).unwrap(), Lyric::Synced(format!("{line}\n")));
        }
        assert_eq!(parse("[Chorus]").unwrap(), Lyric::PlainOnly);
        assert_eq!(parse("[Chorus").unwrap(), Lyric::PlainOnly);
    }
    #[test]
    fn rejects_malformed_numeric_tags_after_a_timestamp() {
        for line in [
            "[00:01][00:99]words",
            "[00:01][00:02.1111]words",
            "[00:01][01:02][3:bad]words",
            "[00:01][00:02",
            "[00:01][123]words",
        ] {
            assert!(parse(line).is_err(), "{line}");
        }
    }
    #[test]
    fn distinguishes_plain_empty_and_malformed() {
        assert_eq!(parse("普通文字").unwrap(), Lyric::PlainOnly);
        assert_eq!(parse("[ar:artist]\n[00:01.00]").unwrap(), Lyric::NotFound);
        for bad in [
            "[00:99]bad",
            "[00:01]text\nplain",
            "<html>error",
            "{\"code\":400}",
            "[00:01.1111]bad",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn credits_alone_are_not_lyrics() {
        let credits = "{\"t\":1000,\"c\":[{\"tx\":\"作词: writer\"}]}";
        assert_eq!(netease(credits).unwrap(), Lyric::NotFound);
        assert_eq!(
            netease(&format!("{credits}\n[00:02.00]text")).unwrap(),
            Lyric::Synced("[00:01.00]作词: writer\n[00:02.00]text\n".into())
        );
    }
    #[test]
    fn credit_bom_and_embedded_newlines_keep_each_line_timed() {
        let input =
            "\u{feff}{\"t\":0,\"c\":[{\"tx\":\"Writer\\nComposer\\rArranger\"}]}\n[00:01]body";
        assert_eq!(
            netease(input).unwrap(),
            Lyric::Synced(
                "[00:00.00]Writer\n[00:00.00]Composer\n[00:00.00]Arranger\n[00:01]body\n".into()
            )
        );
        assert!(netease("{\"t\":0,\"c\":[{\"tx\":42}]}\n[00:01]body").is_err());
    }
    #[test]
    fn rejects_word_tags_at_arbitrary_offsets() {
        for word in ["<2345,200,0>word", "<999,100>word", "<00:01.234>word"] {
            assert!(parse(&format!("[00:01]{word}")).is_err());
        }
        assert_eq!(
            parse("[00:01]x < y").unwrap(),
            Lyric::Synced("[00:01]x < y\n".into())
        );
        assert_eq!(parse("[00:01][00:02]").unwrap(), Lyric::NotFound);
    }
    #[test]
    fn rounds_timestamps_and_bounds_size() {
        assert_eq!(timestamp(59995), "[01:00.00]");
        assert!(parse(&"a".repeat(MAX_LYRIC_SIZE + 1)).is_err());
    }
}
