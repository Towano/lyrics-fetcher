# lyrics-fetcher

独立的 Rust 歌词获取与 LRC 保存模块，面向含网易云音乐歌曲 ID 的音频转换流程。它不依赖调用方的 `TrackMetadata` 或其他应用类型，可单独复制、测试和集成。

## 接口

```rust
pub fn save_for_track(
    music_id: Option<&str>,
    source: &Path,
    output: &Path,
) -> anyhow::Result<LyricsResult>
```

`music_id` 为网易云歌曲 ID；`source` 是输入音频路径；`output` 是转换后的音频路径。调用方可通过 `lyrics_fetcher::save_for_track(...)` 调用此函数。模块先查找输入文件同目录的同名 `.lrc`，再查找输出文件同目录的同名 `.lrc`。未找到本地歌词时，使用歌曲 ID 请求网易云普通歌词，并将歌词保存到输出音频旁的 `.lrc` 文件。已有目标歌词不会被覆盖。

`LyricsResult` 的结果如下：

- `Local`：使用了本地歌词。
- `Online`：联网获取并保存了歌词。
- `MissingId`：没有本地歌词且未提供歌曲 ID。
- `NoLyrics`：接口未返回有效歌词。
- `OutputExists`：目标 `.lrc` 已存在，未覆盖。

歌曲 ID 会在请求前校验为最多 20 位的十进制数字。网络请求设置 8 秒超时，并限制响应体和歌词大小。接口中的逐行 JSON 作词信息会转换成带时间标签的 LRC 行；只有元数据而没有歌词正文时视为无歌词。

此 crate 仅获取网易云普通 LRC，不包含其他音乐平台、翻译歌词、罗马音歌词或逐字歌词实现。

## 依赖

| Crate | 用途 |
| --- | --- |
| `anyhow` | 请求、解析和文件操作错误上下文 |
| `serde_json` | 解析歌词接口响应 |
| `tempfile` | 在目标目录安全地临时写入，再以不覆盖方式保存 |
| `ureq` | 带超时的 HTTPS 请求 |

## 测试

在仓库根目录运行：

```sh
cargo test
cargo fmt -- --check
```
