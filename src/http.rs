use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::io::Read;
use std::time::{Duration, Instant};

pub(crate) const MAX_RESPONSE_SIZE: u64 = 512 * 1024;
pub(crate) const MAX_LYRIC_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub url: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub content_type: Option<String>,
}

impl Request {
    pub fn get(url: &str) -> Self {
        Self {
            url: url.into(),
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
            content_type: None,
        }
    }
    pub fn query(mut self, key: &str, value: impl ToString) -> Self {
        self.query.push((key.into(), value.to_string()));
        self
    }
    pub fn header(mut self, key: &str, value: &str) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }
    pub fn json(mut self, value: Value) -> Self {
        self.body = Some(value.to_string());
        self.content_type = Some("application/json".into());
        self
    }
    pub fn form(mut self, fields: &[(&str, String)]) -> Self {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in fields {
            serializer.append_pair(key, value);
        }
        self.body = Some(serializer.finish());
        self.content_type = Some("application/x-www-form-urlencoded".into());
        self
    }
}

pub(crate) trait Transport {
    fn send(&self, request: Request) -> Result<Vec<u8>>;
}

pub(crate) struct Http {
    agent: ureq::Agent,
    started: Instant,
    requests: std::cell::Cell<u32>,
    lrclib_last: std::cell::Cell<Option<Instant>>,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(Duration::from_secs(8))
                .build(),
            started: Instant::now(),
            requests: std::cell::Cell::new(0),
            lrclib_last: std::cell::Cell::new(None),
        }
    }
}

pub(crate) fn bounded_read(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut body)
        .context("读取响应失败")?;
    ensure!(
        body.len() as u64 <= limit,
        "响应超出大小限制 ({limit} bytes)"
    );
    Ok(body)
}

impl Transport for Http {
    fn send(&self, request: Request) -> Result<Vec<u8>> {
        let url = url::Url::parse(&request.url)?;
        ensure!(
            url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none(),
            "请求不是有效 HTTPS 端点"
        );
        ensure!(
            matches!(
                url.host_str(),
                Some(
                    "music.163.com"
                        | "interface.music.163.com"
                        | "interface3.music.163.com"
                        | "c.y.qq.com"
                        | "u.y.qq.com"
                        | "songsearch.kugou.com"
                        | "lyrics.kugou.com"
                        | "wwwapi.kugou.com"
                        | "search.kuwo.cn"
                        | "mlyric.kuwo.cn"
                        | "m.kuwo.cn"
                        | "lrclib.net"
                )
            ),
            "不允许的请求域名"
        );
        ensure!(self.requests.get() < 24, "本次查询请求次数已达上限");
        let remaining = Duration::from_secs(90)
            .checked_sub(self.started.elapsed())
            .context("本次查询超过总时限")?;
        ensure!(!remaining.is_zero(), "本次查询超过总时限");
        if url.host_str() == Some("lrclib.net") {
            if let Some(last) = self.lrclib_last.get() {
                if let Some(wait) = Duration::from_millis(300).checked_sub(last.elapsed()) {
                    ensure!(wait < remaining, "本次查询超过总时限");
                    std::thread::sleep(wait);
                }
            }
            self.lrclib_last.set(Some(Instant::now()));
        }
        let remaining = Duration::from_secs(90)
            .checked_sub(self.started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .context("本次查询超过总时限")?;
        self.requests.set(self.requests.get() + 1);
        let mut call = if request.body.is_some() {
            self.agent.post(&request.url)
        } else {
            self.agent.get(&request.url)
        };
        call = call.timeout(remaining.min(Duration::from_secs(8))).set(
            "User-Agent",
            concat!(
                "lyrics-fetcher/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/Towano/lyrics-fetcher)"
            ),
        );
        for (key, value) in request.query {
            call = call.query(&key, &value);
        }
        for (key, value) in request.headers {
            call = call.set(&key, &value);
        }
        if let Some(content_type) = request.content_type {
            call = call.set("Content-Type", &content_type);
        }
        let response = match request.body {
            Some(body) => call.send_string(&body),
            None => call.call(),
        };
        let response = match response {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) => {
                if code == 429 || code == 503 {
                    bail!(
                        "服务限流或繁忙 (HTTP {code})，Retry-After: {}；未自动重试",
                        response.header("Retry-After").unwrap_or("未提供")
                    );
                }
                bail!("服务返回 HTTP {code}");
            }
            Err(error) => return Err(error).context("HTTPS 请求失败"),
        };
        ensure!(
            (200..300).contains(&response.status()),
            "接口重定向或非成功 HTTP {}，未跟随",
            response.status()
        );
        bounded_read(response.into_reader(), MAX_RESPONSE_SIZE)
    }
}

pub(crate) fn json(body: &[u8]) -> Result<Value> {
    serde_json::from_slice(body).context("接口响应 JSON 无效")
}

pub(crate) fn decode_base64(value: &str) -> Result<String> {
    use base64::Engine;
    ensure!(
        value.len() <= MAX_RESPONSE_SIZE as usize,
        "Base64 内容超出大小限制"
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .context("歌词 Base64 无效")?;
    ensure!(decoded.len() <= MAX_LYRIC_SIZE, "解码歌词超出大小限制");
    String::from_utf8(decoded).context("歌词不是 UTF-8")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) struct Fake {
        pub responses: std::cell::RefCell<std::collections::VecDeque<Vec<u8>>>,
        pub requests: std::cell::RefCell<Vec<Request>>,
    }
    impl Fake {
        pub fn new(responses: Vec<Vec<u8>>) -> Self {
            Self {
                responses: std::cell::RefCell::new(responses.into()),
                requests: Default::default(),
            }
        }
    }
    impl Transport for Fake {
        fn send(&self, request: Request) -> Result<Vec<u8>> {
            self.requests.borrow_mut().push(request);
            self.responses
                .borrow_mut()
                .pop_front()
                .context("unexpected request")
        }
    }
    #[test]
    fn refuses_unsafe_endpoints_and_exhausted_budgets_before_io() {
        let http = Http::default();
        for url in [
            "http://music.163.com/api/song/lyric/v1",
            "https://127.0.0.1/",
            "https://music.163.com.evil.invalid/",
            "https://user:password@music.163.com/",
            "https://music.163.com:8443/",
        ] {
            assert!(http.send(Request::get(url)).is_err());
        }
        assert_eq!(http.requests.get(), 0);
        http.requests.set(24);
        assert!(http
            .send(Request::get("https://music.163.com/api/song/lyric/v1"))
            .is_err());
        let mut expired = Http::default();
        expired.started = Instant::now() - Duration::from_secs(91);
        assert!(expired
            .send(Request::get("https://lrclib.net/api/get/1"))
            .is_err());
        assert_eq!(expired.requests.get(), 0);
    }

    #[test]
    fn bounds_reader() {
        assert!(bounded_read(&b"1234"[..], 3).is_err());
        assert_eq!(bounded_read(&b"123"[..], 3).unwrap(), b"123");
    }
    #[test]
    fn rejects_invalid_and_oversize_base64() {
        assert!(decode_base64("%%%%").is_err());
        use base64::Engine;
        let input =
            base64::engine::general_purpose::STANDARD.encode(vec![b'a'; MAX_LYRIC_SIZE + 1]);
        assert!(decode_base64(&input).is_err());
    }
}
