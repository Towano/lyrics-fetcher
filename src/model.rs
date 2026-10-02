use anyhow::{bail, ensure, Result};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Provider {
    Netease,
    Qq,
    Kugou,
    Kuwo,
    Lrclib,
}

impl Provider {
    pub const ALL: [Self; 5] = [
        Self::Netease,
        Self::Qq,
        Self::Kugou,
        Self::Kuwo,
        Self::Lrclib,
    ];
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Netease => "netease",
            Self::Qq => "qq",
            Self::Kugou => "kugou",
            Self::Kuwo => "kuwo",
            Self::Lrclib => "lrclib",
        })
    }
}

impl FromStr for Provider {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "netease" => Ok(Self::Netease),
            "qq" => Ok(Self::Qq),
            "kugou" => Ok(Self::Kugou),
            "kuwo" => Ok(Self::Kuwo),
            "lrclib" => Ok(Self::Lrclib),
            _ => bail!("未知歌词来源: {value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SongRef {
    pub provider: Provider,
    pub id: String,
}

pub(crate) fn decimal_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit())
}

fn canonical_decimal_id(value: &str) -> &str {
    let number = value.trim_start_matches('0');
    if number.is_empty() {
        "0"
    } else {
        number
    }
}

impl SongRef {
    pub fn new(provider: Provider, id: impl Into<String>) -> Result<Self> {
        let mut id = id.into();
        match provider {
            Provider::Netease | Provider::Kuwo | Provider::Lrclib => {
                if provider == Provider::Kuwo {
                    id = id.strip_prefix("MUSIC_").unwrap_or(&id).to_owned();
                }
                ensure!(decimal_id(&id), "{provider} 歌曲 ID 无效");
                id = canonical_decimal_id(&id).to_owned();
            }
            Provider::Qq => {
                if let Some(number) = id.strip_prefix("id:") {
                    ensure!(decimal_id(number), "QQ 数字歌曲 ID 无效");
                    id = format!("id:{}", canonical_decimal_id(number));
                } else {
                    id = id.strip_prefix("mid:").unwrap_or(&id).to_owned();
                    ensure!(
                        id.len() == 14 && id.bytes().all(|b| b.is_ascii_alphanumeric()),
                        "QQ MID 应为 14 位字母数字；数字 ID 使用 qq:id:数字"
                    );
                }
            }
            Provider::Kugou => {
                if let Some(lyric) = id.strip_prefix("lrc:") {
                    let (number, key) = lyric
                        .split_once(':')
                        .ok_or_else(|| anyhow::anyhow!("酷狗歌词引用应为 lrc:ID:accesskey"))?;
                    ensure!(
                        decimal_id(number)
                            && !key.is_empty()
                            && key.len() <= 128
                            && key.bytes().all(|b| b.is_ascii_alphanumeric()),
                        "酷狗歌词候选引用无效"
                    );
                    id = format!("lrc:{}:{key}", canonical_decimal_id(number));
                } else if let Some(number) = id.strip_prefix("mix:") {
                    ensure!(decimal_id(number), "酷狗 MixSongID 无效");
                    id = format!("mix:{}", canonical_decimal_id(number));
                } else {
                    id = id.strip_prefix("hash:").unwrap_or(&id).to_owned();
                    ensure!(
                        id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                        "酷狗歌曲需 32 位 hash 或 mix:数字"
                    );
                    id.make_ascii_uppercase();
                }
            }
        }
        Ok(Self { provider, id })
    }
}

impl fmt::Display for SongRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider, self.id)
    }
}

impl FromStr for SongRef {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        let (provider, id) = value
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("歌曲引用应为 来源:ID"))?;
        Self::new(provider.parse()?, id)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    pub keywords: String,
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
}

impl Query {
    pub fn search_text(&self) -> String {
        if !self.keywords.trim().is_empty() {
            self.keywords.clone()
        } else {
            format!(
                "{} {}",
                self.artists.join(" "),
                self.title.as_deref().unwrap_or_default()
            )
            .trim()
            .to_owned()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub song: SongRef,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
}

impl Candidate {
    pub fn query(&self) -> Query {
        Query {
            keywords: String::new(),
            title: Some(self.title.clone()),
            artists: self.artists.clone(),
            album: self.album.clone(),
            duration: self.duration,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lyric {
    Synced(String),
    PlainOnly,
    Instrumental,
    NotFound,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizes_every_decimal_reference_form() {
        for (provider, input, expected) in [
            (Provider::Netease, "00042", "42"),
            (Provider::Kuwo, "00042", "42"),
            (Provider::Kuwo, "MUSIC_00042", "42"),
            (Provider::Lrclib, "00042", "42"),
            (Provider::Qq, "id:00042", "id:42"),
            (Provider::Kugou, "mix:00042", "mix:42"),
            (Provider::Kugou, "lrc:00042:Key007", "lrc:42:Key007"),
            (Provider::Netease, "000", "0"),
            (Provider::Kuwo, "MUSIC_000", "0"),
            (Provider::Lrclib, "000", "0"),
            (Provider::Qq, "id:000", "id:0"),
            (Provider::Kugou, "mix:000", "mix:0"),
            (Provider::Kugou, "lrc:000:Key007", "lrc:0:Key007"),
        ] {
            let song = SongRef::new(provider, input).unwrap();
            assert_eq!(song.id, expected, "{provider}:{input}");
            assert_eq!(
                format!("{provider}:{input}").parse::<SongRef>().unwrap(),
                song
            );
            assert_eq!(SongRef::new(provider, song.id.clone()).unwrap(), song);
        }
    }

    #[test]
    fn decimal_validation_precedes_normalization_without_a_u64_limit() {
        for (provider, prefix, suffix) in [
            (Provider::Netease, "", ""),
            (Provider::Kuwo, "", ""),
            (Provider::Kuwo, "MUSIC_", ""),
            (Provider::Lrclib, "", ""),
            (Provider::Qq, "id:", ""),
            (Provider::Kugou, "mix:", ""),
            (Provider::Kugou, "lrc:", ":Key007"),
        ] {
            let normalized_prefix = if provider == Provider::Kuwo {
                ""
            } else {
                prefix
            };
            for number in ["99999999999999999999", "00000000000000000000"] {
                let song = SongRef::new(provider, format!("{prefix}{number}{suffix}")).unwrap();
                let expected = if number.starts_with('0') { "0" } else { number };
                assert_eq!(song.id, format!("{normalized_prefix}{expected}{suffix}"));
            }
            for invalid in ["", "000000000000000000000", "00x42", "+42", "42 "] {
                assert!(
                    SongRef::new(provider, format!("{prefix}{invalid}{suffix}")).is_err(),
                    "{provider}:{prefix}{invalid}{suffix}"
                );
            }
        }
    }

    #[test]
    fn decimal_normalization_preserves_mid_hash_and_access_key() {
        for mid in ["001CLC7W2Gpz4J", "00000000000042"] {
            assert_eq!(SongRef::new(Provider::Qq, mid).unwrap().id, mid);
            assert_eq!(
                SongRef::new(Provider::Qq, format!("mid:{mid}")).unwrap().id,
                mid
            );
        }
        assert_eq!(
            SongRef::new(Provider::Kugou, "hash:000000abcdef0123456789abcdef0123")
                .unwrap()
                .id,
            "000000ABCDEF0123456789ABCDEF0123"
        );
        assert_eq!(
            SongRef::new(Provider::Kugou, "lrc:00042:00aBc123")
                .unwrap()
                .id,
            "lrc:42:00aBc123"
        );
        for key in ["", "bad-key", "bad:key"] {
            assert!(SongRef::new(Provider::Kugou, format!("lrc:00042:{key}")).is_err());
        }
        assert!(SongRef::new(Provider::Kugou, format!("lrc:00042:{}", "a".repeat(129))).is_err());
    }
}
