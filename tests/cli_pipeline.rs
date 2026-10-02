//! Child-process coverage of the production CLI, resolver, providers and save path.
//! This harness=false target includes the exact sources to access crate-private
//! Transport without exporting it or adding fixture hooks to the shipped binary.
//! It runs its own scenarios, not another registration of the library unit tests.
// harness=false omits included #[test] functions, leaving their helpers/imports.
#![allow(dead_code, unused_imports)]

extern crate self as lyrics_fetcher;
include!("../src/lib.rs");

mod cli {
    include!("../src/main.rs");

    pub(super) fn offline(
        arguments: impl IntoIterator<Item = OsString>,
        fake: &crate::offline::Fake,
    ) -> ExitCode {
        entry_with_runner(arguments, |options| {
            run_with_resolver(options, |lookup, provider, selection| {
                crate::service::resolve(fake, lookup, provider, selection)
            })
        })
    }
}

mod offline {
    use crate::http::{Request, Transport};
    use anyhow::{Context, Result};
    use serde_json::{json, Value};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode};

    const CHILD: &str = "--offline-cli-child";
    const LRC: &[u8] = "[ar:艺术家]\n[00:01.00]第一句\n[00:02.50]second\n".as_bytes();
    const KEPT: &[u8] = b"keep existing bytes\n";

    pub(super) struct Fake {
        responses: RefCell<VecDeque<Vec<u8>>>,
        requests: RefCell<Vec<Request>>,
    }

    impl Fake {
        fn new(responses: Vec<Value>) -> Self {
            Self {
                responses: RefCell::new(
                    responses
                        .into_iter()
                        .map(|response| response.to_string().into_bytes())
                        .collect(),
                ),
                requests: RefCell::new(Vec::new()),
            }
        }

        fn ledger(&self) -> Value {
            json!({
                "requests": self.requests.borrow().iter().map(|request| json!({
                    "url": request.url,
                    "query": request.query,
                    "headers": request.headers,
                    "body": request.body,
                    "content_type": request.content_type,
                })).collect::<Vec<_>>(),
                "remaining_responses": self.responses.borrow().len(),
            })
        }
    }

    impl Transport for Fake {
        fn send(&self, request: Request) -> Result<Vec<u8>> {
            self.requests.borrow_mut().push(request);
            // Exhaustion is an error, never a fall-through to real HTTP.
            self.responses
                .borrow_mut()
                .pop_front()
                .context("unexpected offline CLI request")
        }
    }

    fn record() -> Value {
        json!({
            "id": 5, "trackName": "Song", "artistName": "Artist",
            "albumName": "Album", "duration": 200.0, "instrumental": false,
            "syncedLyrics": "\u{feff}[ar:艺术家]\r\n [00:01.00]第一句 \r\n[00:02.50]second\r\n",
            "plainLyrics": "第一句\nsecond",
        })
    }

    fn responses(scenario: &str) -> Vec<Value> {
        let mut row = record();
        match scenario {
            "structured_query" => vec![json!([row.clone()]), row],
            "tagged_file" => {
                row["duration"] = json!(1.0);
                vec![json!([row.clone()]), row]
            }
            "keyword_candidates" => vec![json!([row])],
            "wrong_returned_id" => {
                row["id"] = json!(6);
                vec![row]
            }
            "malformed_lrc" => {
                row["syncedLyrics"] = json!("[00:99]invalid timestamp");
                vec![row]
            }
            "plain_only" => {
                row["syncedLyrics"] = Value::Null;
                vec![row]
            }
            "instrumental" => {
                row["instrumental"] = json!(true);
                vec![row]
            }
            "not_found" => {
                row["syncedLyrics"] = Value::Null;
                row["plainLyrics"] = Value::Null;
                vec![row]
            }
            "id_short" | "id_long" | "selected_candidate" | "existing_short" | "existing_long"
            | "invalid_input_id" => vec![row],
            #[cfg(unix)]
            "existing_symlink" | "non_utf8_output" => vec![row],
            other => panic!("unknown offline CLI scenario: {other}"),
        }
    }

    fn request(url: &str, query: &[(&str, &str)]) -> Value {
        json!({
            "url": url, "query": query, "headers": [],
            "body": null, "content_type": null,
        })
    }

    struct Scenario {
        name: &'static str,
        arguments: Vec<OsString>,
        destination: PathBuf,
        seed: Option<&'static [u8]>,
        expected: Option<&'static [u8]>,
        status: i32,
        stderr: String,
        requests: Vec<Value>,
        remaining: usize,
        audio: Option<(PathBuf, Vec<u8>)>,
        #[cfg(unix)]
        symlink: bool,
    }

    fn scenario(name: &'static str, arguments: &[&str], destination: &str) -> Scenario {
        Scenario {
            name,
            arguments: arguments.iter().map(OsString::from).collect(),
            destination: PathBuf::from(destination),
            seed: None,
            expected: Some(LRC),
            status: 0,
            stderr: String::new(),
            requests: vec![request("https://lrclib.net/api/get/5", &[])],
            remaining: 0,
            audio: None,
            #[cfg(unix)]
            symlink: false,
        }
    }

    fn path_stdout(path: &Path) -> Vec<u8> {
        #[cfg(unix)]
        let mut bytes = {
            use std::os::unix::ffi::OsStrExt;
            path.as_os_str().as_bytes().to_vec()
        };
        #[cfg(not(unix))]
        let mut bytes = path.to_string_lossy().as_bytes().to_vec();
        bytes.push(b'\n');
        bytes
    }

    fn check(scenario: Scenario) -> bool {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join(&scenario.destination);
        #[cfg(unix)]
        if scenario.name == "non_utf8_output" {
            if !crate::test_support::create_fixture(scenario.name, &destination, &[]).unwrap() {
                return false;
            }
            std::fs::remove_file(&destination).unwrap();
        }
        if let Some((path, bytes)) = &scenario.audio {
            std::fs::write(directory.path().join(path), bytes).unwrap();
        }
        if let Some(bytes) = scenario.seed {
            std::fs::write(&destination, bytes).unwrap();
        }
        #[cfg(unix)]
        if scenario.symlink {
            std::os::unix::fs::symlink(directory.path().join("absent.lrc"), &destination).unwrap();
        }
        let ledger = directory.path().join("requests.json");
        let output = Command::new(std::env::current_exe().unwrap())
            .arg(CHILD)
            .arg(scenario.name)
            .arg(&ledger)
            .args(&scenario.arguments)
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(scenario.status),
            "{} stderr: {}",
            scenario.name,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = if scenario.status == 0 {
            path_stdout(&scenario.destination)
        } else {
            Vec::new()
        };
        assert_eq!(output.stdout, stdout, "{} stdout", scenario.name);
        assert_eq!(
            output.stderr,
            scenario.stderr.as_bytes(),
            "{} stderr",
            scenario.name
        );
        let actual: Value = serde_json::from_slice(&std::fs::read(&ledger).unwrap()).unwrap();
        assert_eq!(
            actual,
            json!({"requests": scenario.requests, "remaining_responses": scenario.remaining}),
            "{} request ledger",
            scenario.name
        );
        let written = match scenario.expected {
            Some(bytes) => {
                assert_eq!(
                    std::fs::read(&destination).unwrap(),
                    bytes,
                    "{} bytes",
                    scenario.name
                );
                true
            }
            None => {
                #[cfg(unix)]
                if scenario.symlink {
                    assert!(std::fs::symlink_metadata(&destination)
                        .unwrap()
                        .file_type()
                        .is_symlink());
                    assert_eq!(
                        std::fs::read_link(&destination).unwrap(),
                        directory.path().join("absent.lrc")
                    );
                    assert!(!directory.path().join("absent.lrc").exists());
                    true
                } else {
                    assert!(
                        std::fs::symlink_metadata(&destination).is_err(),
                        "{} wrote a file",
                        scenario.name
                    );
                    false
                }
                #[cfg(not(unix))]
                {
                    assert!(
                        std::fs::symlink_metadata(&destination).is_err(),
                        "{} wrote a file",
                        scenario.name
                    );
                    false
                }
            }
        };
        let mut files = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        files.sort();
        let mut expected_files = vec![OsString::from("requests.json")];
        if let Some((path, bytes)) = scenario.audio {
            assert_eq!(
                std::fs::read(directory.path().join(&path)).unwrap(),
                bytes,
                "{} modified audio",
                scenario.name
            );
            expected_files.push(path.into_os_string());
        }
        if written {
            expected_files.push(scenario.destination.into_os_string());
        }
        expected_files.sort();
        assert_eq!(
            files, expected_files,
            "{} unexpected file or leftover temporary",
            scenario.name
        );
        true
    }

    fn tagged_wav() -> Vec<u8> {
        fn synchsafe(size: usize) -> [u8; 4] {
            [
                (size >> 21 & 127) as u8,
                (size >> 14 & 127) as u8,
                (size >> 7 & 127) as u8,
                (size & 127) as u8,
            ]
        }
        fn chunk(id: &[u8; 4], bytes: &[u8], body: &mut Vec<u8>) {
            body.extend_from_slice(id);
            body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            body.extend_from_slice(bytes);
            if bytes.len() % 2 != 0 {
                body.push(0);
            }
        }
        let mut frames = Vec::new();
        for (key, text) in [(b"TIT2", "Song"), (b"TPE1", "Artist"), (b"TALB", "Album")] {
            let content = [vec![3], text.as_bytes().to_vec()].concat(); // ID3v2.4 UTF-8.
            frames.extend_from_slice(key);
            frames.extend_from_slice(&synchsafe(content.len()));
            frames.extend_from_slice(&[0, 0]);
            frames.extend_from_slice(&content);
        }
        let mut tag = b"ID3\x04\x00\x00".to_vec();
        tag.extend_from_slice(&synchsafe(frames.len()));
        tag.extend_from_slice(&frames);
        let mut body = b"WAVE".to_vec();
        let format = [
            1_u16.to_le_bytes().to_vec(), // PCM, mono, 8 kHz, 8 bits.
            1_u16.to_le_bytes().to_vec(),
            8000_u32.to_le_bytes().to_vec(),
            8000_u32.to_le_bytes().to_vec(),
            1_u16.to_le_bytes().to_vec(),
            8_u16.to_le_bytes().to_vec(),
        ]
        .concat();
        chunk(b"fmt ", &format, &mut body);
        chunk(b"ID3 ", &tag, &mut body);
        chunk(b"data", &vec![128; 8000], &mut body); // One second of silence.
        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&(body.len() as u32).to_le_bytes());
        wav.extend_from_slice(&body);
        wav
    }

    fn parent() {
        let mut scenarios = Vec::new();
        for (name, flag) in [("id_short", "-l"), ("id_long", "--lyrics")] {
            scenarios.push(scenario(
                name,
                &[flag, "lrclib:0005", "--provider", "lrclib"],
                "lrclib-5.lrc",
            ));
        }
        let mut structured = scenario(
            "structured_query",
            &[
                "--lyrics",
                "untrusted hint",
                "--title",
                "Song",
                "--artist",
                "Artist",
                "--album",
                "Album",
                "--duration",
                "200",
                "--provider",
                "lrclib",
                "--output",
                "中文 structured.exact",
            ],
            "中文 structured.exact",
        );
        structured.requests.insert(
            0,
            request(
                "https://lrclib.net/api/search",
                &[
                    ("track_name", "Song"),
                    ("artist_name", "Artist"),
                    ("album_name", "Album"),
                ],
            ),
        );
        scenarios.push(structured);
        let mut audio = scenario(
            "tagged_file",
            &["--lyrics", "误导 filename.wav", "--provider", "lrclib"],
            "误导 filename.lrc",
        );
        audio.audio = Some((PathBuf::from("误导 filename.wav"), tagged_wav()));
        audio.requests.insert(
            0,
            request(
                "https://lrclib.net/api/search",
                &[
                    ("track_name", "Song"),
                    ("artist_name", "Artist"),
                    ("album_name", "Album"),
                ],
            ),
        );
        scenarios.push(audio);

        let mut candidates = scenario(
            "keyword_candidates",
            &[
                "-l",
                "Song Artist",
                "--provider",
                "lrclib",
                "-o",
                "候选.lrc",
            ],
            "候选.lrc",
        );
        candidates.status = 4;
        candidates.expected = None;
        candidates.requests = vec![request(
            "https://lrclib.net/api/search",
            &[("q", "Song Artist")],
        )];
        candidates.stderr = concat!(
            "无法唯一确定歌曲或歌词版本（1 个候选）；未写文件。\n",
            "  lrclib:5 | \"Song\" | [\"Artist\"] | 专辑 Some(\"Album\") | 200.00s\n",
            "    独立选择: lyrics-fetcher -l 'lrclib:5' --provider lrclib\n",
            "    文件/关键词输入可重新运行原命令并添加: --select 'lrclib:5'\n",
            "候选歌词仍需同步 LRC 校验；--title/--artist/--album/--duration 可提供更强匹配线索。\n",
        )
        .into();
        scenarios.push(candidates);
        scenarios.push(scenario(
            "selected_candidate",
            &[
                "--lyrics",
                "Song Artist",
                "--provider",
                "lrclib",
                "--select",
                "lrclib:5",
                "--output",
                "中文 selected.exact",
            ],
            "中文 selected.exact",
        ));

        for (name, flag) in [("existing_short", "-l"), ("existing_long", "--lyrics")] {
            let mut existing = scenario(
                name,
                &[
                    flag,
                    "lrclib:5",
                    "--provider",
                    "lrclib",
                    "--output",
                    "中文 existing.lrc",
                ],
                "中文 existing.lrc",
            );
            existing.seed = Some(KEPT);
            existing.expected = Some(KEPT);
            existing.status = 5;
            existing.stderr = "目标已存在，未覆盖: \"中文 existing.lrc\"\n".into();
            existing.requests.clear();
            existing.remaining = 1;
            scenarios.push(existing);
        }
        for (name, input, status, stderr, remaining) in [
            (
                "wrong_returned_id",
                "lrclib:5",
                1,
                "错误: LRCLIB 返回歌曲 ID 不匹配\n",
                0,
            ),
            (
                "malformed_lrc",
                "lrclib:5",
                1,
                "错误: 歌词时间标签无效\n",
                0,
            ),
            (
                "invalid_input_id",
                "lrclib:bad",
                2,
                "参数错误: lrclib 歌曲 ID 无效\n",
                1,
            ),
            (
                "plain_only",
                "lrclib:5",
                3,
                "仅有纯文本歌词，没有可保存的同步 LRC。\n",
                0,
            ),
            (
                "instrumental",
                "lrclib:5",
                3,
                "该歌曲标记为纯音乐，没有可保存的同步 LRC。\n",
                0,
            ),
            ("not_found", "lrclib:5", 3, "未找到同步 LRC 歌词。\n", 0),
        ] {
            let mut rejected = scenario(
                name,
                &[
                    "-l",
                    input,
                    "--provider",
                    "lrclib",
                    "--output",
                    "must-not-write.lrc",
                ],
                "must-not-write.lrc",
            );
            rejected.status = status;
            rejected.stderr = stderr.into();
            rejected.expected = None;
            rejected.remaining = remaining;
            if remaining != 0 {
                rejected.requests.clear();
            }
            scenarios.push(rejected);
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let mut symlink = scenario(
                "existing_symlink",
                &[
                    "--lyrics",
                    "lrclib:5",
                    "--provider",
                    "lrclib",
                    "-o",
                    "symlink.lrc",
                ],
                "symlink.lrc",
            );
            symlink.symlink = true;
            symlink.expected = None;
            symlink.status = 5;
            symlink.stderr = "目标已存在，未覆盖: \"symlink.lrc\"\n".into();
            symlink.requests.clear();
            symlink.remaining = 1;
            scenarios.push(symlink);
            let mut native = scenario(
                "non_utf8_output",
                &["-l", "lrclib:5", "--provider", "lrclib", "--output"],
                "unused.lrc",
            );
            let path = OsString::from_vec(b"track\xff.lrc".to_vec());
            native.destination = PathBuf::from(&path);
            native.arguments.push(path);
            scenarios.push(native);
        }
        let total = scenarios.len();
        let mut passed = 0;
        for scenario in scenarios {
            if check(scenario) {
                passed += 1;
            }
        }
        println!(
            "offline CLI pipeline: {passed} child-process scenarios passed; {} skipped",
            total - passed
        );
    }

    pub(super) fn main() -> ExitCode {
        let mut arguments = std::env::args_os();
        let executable = arguments.next().unwrap();
        if arguments.next().as_deref() == Some(OsStr::new(CHILD)) {
            let scenario = arguments.next().unwrap().into_string().unwrap();
            let ledger = PathBuf::from(arguments.next().unwrap());
            let fake = Fake::new(responses(&scenario));
            let status = crate::cli::offline(std::iter::once(executable).chain(arguments), &fake);
            std::fs::write(ledger, fake.ledger().to_string()).unwrap();
            status
        } else {
            parent();
            ExitCode::SUCCESS
        }
    }
}

fn main() -> std::process::ExitCode {
    offline::main()
}
