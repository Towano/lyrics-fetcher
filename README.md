# lyrics-fetcher

独立的 Rust 歌词库与命令行工具。`-l` 和 `--lyrics` 联网查找普通同步 LRC，支持关键词、歌曲 ID/完整链接和本地音频标签作为查询输入。存在歧义时给出候选，不静默下载第一条；已有歌词文件不会覆盖。

接入范围为网易云音乐、QQ 音乐、酷狗、酷我和 LRCLIB；**酷狗 MixSongID 精确取词和精确详情尚未实现，多份可信歌词候选需进一步选择 `kugou:lrc:歌词ID:公开下载键`，不能视为全面支持**。来源协议、许可边界与**实现/联网验证分开的状态表**见 [docs/providers.md](docs/providers.md)。第三方接口不是稳定公开 API，源码与离线 fixture 的通过不代表服务当前可用。

## 构建

需要 Rust/Cargo；锁定依赖的最低声明版本为 Rust 1.89，清单相应声明 `rust-version = "1.89"`。尚未单独运行 Rust 1.89 工具链，推荐使用已安装的 stable。脚本不会安装或升级工具。

Linux/macOS：

```sh
./build.sh
# 也支持以下两种调用
sh build.sh
bash build.sh

./target/release/lyrics-fetcher --help
```

`build.sh` 从脚本所在目录定位 `Cargo.toml`，可在其他工作目录调用，支持中文及空格路径。实际执行：

```sh
cargo build --manifest-path /path/to/lyrics-fetcher/Cargo.toml --locked --release --bin lyrics-fetcher
```

附加参数原样交给 Cargo，不做自己的 `--debug` 参数协议：

```sh
./build.sh --offline
./build.sh --target aarch64-unknown-linux-gnu
./build.sh --target-dir './构建 output'
```

`--offline` 需要依赖已缓存；`--target` 需要对应工具链和链接器已配置。脚本不自动交叉编译、不下载 SDK。需要 debug 构建时直接使用 `cargo build --locked --bin lyrics-fetcher`。

Windows 原生 PowerShell：

```powershell
cargo build --locked --release
.\target\release\lyrics-fetcher.exe --help
```

Windows 也可在已有 Git Bash 中执行脚本。Linux/macOS/Windows 的原生 CI 配置已提供，但添加配置不等于远端矩阵已执行。本机 Droidspaces 是 Android 内核上的 GNU/Linux 用户空间；这不等于原生 Android、Termux 或所有 Linux 发行版已实测支持。

## 最简单的使用

```sh
lyrics-fetcher -l '周杰伦 晴天'
lyrics-fetcher --lyrics '周杰伦 晴天'
```

`-l` 和 `--lyrics` 完全等价，必须带一个输入。关键词本身不一定能唯一确认录音，退出码 `4` 时查看 stderr 的来源限定候选，再用稳定 ID 选择：

```sh
lyrics-fetcher -l '周杰伦 晴天' --select netease:186016
```

不是重新运行后可能变化的 `--select 1`。候选标题、歌手、专辑、版本与时长需要核对；live/remix/cover 等版本不会因为标题相似而自动合并。

## 输入与参数

### 歌曲 ID 和完整链接

```sh
lyrics-fetcher -l netease:186016
lyrics-fetcher -l 186016
lyrics-fetcher -l 'https://music.163.com/#/song?id=186016'
lyrics-fetcher -l qq:0039MnYb0qxYhV --provider qq
lyrics-fetcher -l qq:id:数字歌曲ID
lyrics-fetcher -l kugou:32位十六进制HASH
lyrics-fetcher -l kugou:mix:数字MixSongID
lyrics-fetcher -l kuwo:数字RID
lyrics-fetcher -l lrclib:数字记录ID
```

示例中的中文 ID 占位符必须替换为真实值。QQ MID 为 14 位字母数字，数字歌曲 ID 必须标明 `id:`；酷狗 hash 为 32 位十六进制，MixSongID 必须标明 `mix:`，但当前 MixSongID 精确取词未实现。酷狗歌词服务返回多份可信歌词时，会给出更细的普通歌词引用：

```sh
lyrics-fetcher --lyrics 'kugou:lrc:歌词ID:公开下载键' --provider kugou -o './song.lrc'
```

`accesskey` 是该接口的公开歌词下载键，不是账号 token；应使用程序列出的成对 ID/下载键，不猜测。直接 hash 需要可信歌名、歌手和时长，不能把一个裸 hash 当作已掌握元信息。

网易、酷我和 LRCLIB 使用各自的最多 20 位十进制 ID，ID 不能跨平台互用。十进制部分校验后去除前导零，全零保留为 `0`；QQ MID、酷狗 hash 和下载键不按十进制处理。`--provider auto` 时裸数字默认网易；指定 `qq` 时转换成 `qq:id:数字`，指定 `kuwo` / `lrclib` 时归对应来源；`kugou` 拒绝裸数字，需明确 `mix:`（当前不支持取词）。数字歌名使用：

```sh
lyrics-fetcher -l '22' --input-type query
```

仅解析支持的官方完整歌曲链接，提取 ID 后请求固定平台接口；不会请求用户输入的任意 URL，也不会自动跟随短链接。完整链接的准确 host/path 范围以 CLI 帮助和 [来源说明](docs/providers.md) 为准。未知 URL、非法 ID、未知来源前缀和来源冲突在联网前报错，不退化成关键词搜索。

### 本地音频

```sh
lyrics-fetcher --lyrics './音乐/My song.flac'
lyrics-fetcher -l './音乐/My song.flac' --select netease:186016
```

CLI 只在本地读取音频的标题、歌手、专辑和时长，不上传、不修改音频，不读取封面；标签库单项分配上限为 1 MiB（不是整个进程内存上限）。MP3/FLAC/M4A/OGG/WAV 等由 `lofty` 支持的格式可用于标签查询；具体容器和编码仍取决于标签库实际支持。损坏音频、目录、明确但不存在的文件路径和 `.ncm` 等不支持格式报错。文件名只作低置信度线索，标签不足不会自动选中搜索首条。

M4A/MP4 按实际内容识别，仅选择读取 iTunes `©nam` / `©ART` / `©alb` 的 UTF-8 或 UTF-16BE 文本，封面和其他负载通过 seek 跳过；音频属性仍由禁用标签读取的 `lofty` 提供。该路线限制原子数量/深度和文本大小，当前明确拒绝属性区域的扩展/零大小原子及非标准 QuickTime meta 布局；不支持 keyed/freeform 元信息。超大封面不会再触发整个 `ilst` 的分配限制，但不承诺覆盖所有 MP4 变体。

### 明确输入类型、来源与元信息

```sh
lyrics-fetcher -l '周杰伦 晴天' --input-type query --provider netease
lyrics-fetcher -l 186016 --input-type id --provider netease
lyrics-fetcher -l './song.flac' --input-type file
lyrics-fetcher -l '晴天' --title '晴天' --artist '周杰伦' --album '叶惠美' --duration 269
lyrics-fetcher -l '合作歌曲' --artist '歌手甲' --artist '歌手乙'
```

| 参数 | 作用 |
| --- | --- |
| `-l`, `--lyrics INPUT` | 联网查询输入，保留本地路径的 `OsString` 语义 |
| `--input-type auto\|query\|id\|file` | 默认 `auto`；强制解析类别，避免数字歌名等歧义 |
| `--provider auto\|netease\|qq\|kugou\|kuwo\|lrclib` | 默认 `auto`；指定来源时只使用该来源 |
| `--title TITLE` | 提供歌曲标题用于可靠匹配 |
| `--artist ARTIST` | 提供歌手，可重复；替换文件标签的整组歌手 |
| `--album ALBUM` | 提供专辑 |
| `--duration SECONDS` | 提供秒级有限正时长 |
| `--select PROVIDER:ID` | 精确选择来源限定候选，可保留音频输入的默认输出路径 |
| `-o`, `--output PATH` | 指定准确的目标文件路径，不自动改扩展名 |

元信息参数用于关键词/音频文件输入；明确 ID/链接不允许覆盖歌曲元信息或另外 `--select`，需直接改为选定的 `来源:ID`。显式 `--title`，或已有可信标题时的 `--artist` 覆盖，会优先构造元信息搜索；只有歌手、没有可信标题时仍保留原关键词线索。

自动分类顺序是已存在的普通本地文件、支持的完整链接、来源限定 ID、裸数字（`auto` 默认网易，指定平台时使用该平台的 ID 规则）、关键词。显式 `--input-type query` 可把长得像 ID 的输入当作歌曲名。自由关键词不会被任意按空格强拆为歌手与标题。

`--provider auto` 才允许多来源搜索/补查。明确 ID 先定位原平台歌曲，缺同步歌词时可凭核实元信息补查其他来源，不拿原平台 ID 去遍历其他平台。纯音乐、纯文本歌词和普通同步歌词分别处理，不伪造时间标签。

## 保存与退出码

- 音频文件输入默认保存到同目录、同文件名的 `.lrc`。
- 关键词输入默认保存到当前目录的 `<选定来源>-<歌曲ID>.lrc`；ID/链接输入保留输入引用的默认文件名，即使 `auto` 从另一来源补查成功也不改名。不把不可信歌曲标题用作路径；类型化 ID 内的冒号替换成 `-`，如 `qq:id:123` 对应 `qq-id-123.lrc`。
- `--output` 指定准确文件路径，不自动改扩展名或创建父目录；CLI 明确联网查询，不因旁边存在其他本地歌词改为复制。
- 目标已存在时不覆盖，包括符号链接。先检查可以避免不必要的请求，最终依靠同目录临时写入、`sync_all` 与 `persist_noclobber` 防止并发覆盖。
- 文件系统决定发布原子性和崩溃后的持久性；未同步父目录，不承诺断电后必然保留目录项或跨所有文件系统绝对原子。Unix 新文件保留 tempfile 的 `0600` 权限；共享库需要按自己的权限策略在保存后调整，不自动扩大读取权限。
- stdout 只输出成功保存的路径；候选、限制和错误写 stderr，适合脚本判断退出码。

| 退出码 | 含义 |
| --- | --- |
| `0` | 保存成功，或显示 help/version |
| `1` | 网络、协议、解码、音频读取或文件操作失败 |
| `2` | 参数、输入格式或来源组合错误 |
| `3` | 没有可保存的同步 LRC：未找到、只有纯文本或纯音乐 |
| `4` | 匹配有歧义，需要 `--select 来源:ID` |
| `5` | 目标已存在，未覆盖 |

多个来源部分失败会保留诊断；网络错误不应被解释为“无歌词”。自动检索不是保证全网唯一匹配，更不是逐个平台尝试同一个数字 ID。酷狗歌词层的多份匹配目前返回错误码 `1` 并列出 `kugou:lrc:歌词ID:公开下载键`，不同于歌曲搜索候选的退出码 `4`；使用列出的精确歌词引用再次下载。

## 网络与资源边界

生产请求只允许固定 HTTPS 域名，校验证书，不跟随重定向，不自动降级到 HTTP，不使用第三方代理或发送账号 Cookie。

- 单次请求超时：8 秒；整个查询预算：90 秒，最多 24 次请求。同步 DNS 解析不能保证由这些计时器严格中断，因此不是所有系统上的绝对墙钟截止时间。
- 每个响应最多 512 KiB；歌词最多 256 KiB，包括 Base64 解码后的大小。当前酷我用 H5 普通行歌词，不接入旧压缩/逐字路线。
- LRCLIB 请求串行，间隔至少 300 ms。
- HTTP `429` / `503` 返回限流/繁忙诊断和 `Retry-After`，不自动重试。
- 本项目只保存普通同步 LRC，不支持翻译、罗马音、逐字 KRC/QRC 输出、登录、音频解密或下载。

## Rust 库

保持独立 crate，不依赖调用方的 `TrackMetadata` 或具体应用，不包含 `ncm-audio` 集成。默认 feature `cli` 包含 `clap` / `lofty`；只用库时可以排除这两项依赖：

```toml
[dependencies]
lyrics-fetcher = { path = "../lyrics-fetcher", default-features = false }
```

兼容原入口：

```rust
pub fn save_for_track(
    music_id: Option<&str>,
    source: &std::path::Path,
    output: &std::path::Path,
) -> anyhow::Result<LyricsResult>
```

`music_id` 仍是网易歌曲 ID；`source` 是输入音频路径，`output` 是输出音频路径。旧入口先检查目标是否存在，再尝试输入/输出音频旁的同名本地 `.lrc`，没有可用本地歌词才联网。`LyricsResult` 保留 `Local`、`Online`、`MissingId`、`NoLyrics`、`OutputExists`；不把新 CLI 的严格同步匹配规则偷偷施加到旧的本地字节复制接口。

新多来源能力位于 `model` / `service` 模块，使用独立的 `Provider`、`SongRef`、`Query`、`Candidate`、`Lyric` 模型。`service::Client::resolve` 接受 `Lookup` 与可选来源/候选，返回 `Resolution`，结果是 `Outcome::Found`、`Candidates` 或 `Unavailable`；`save_lrc` 验证并安全保存同步歌词。各来源的 `Synced`、`PlainOnly`、`Instrumental`、`NotFound` 与请求错误分开，应用不必使用 CLI 或音频标签类型。

## 验证

```sh
cargo fmt -- --check
cargo test --locked
cargo test --locked --no-default-features --lib
cargo build --locked --release --bin lyrics-fetcher
sh -n build.sh
bash -n build.sh
```

`cargo test` 的单元/集成测试只使用本地 fixture、假 Transport 和假 Cargo，不依赖真实服务或外网；Cargo 下载缺失依赖与测试本身访问服务是两件事。shell 测试仅在 Unix 启用，bash 不存在时跳过 bash 子项，不安装工具。

CLI 有两层进程覆盖：`tests/cli.rs` 运行实际发布二进制，检查帮助、参数错误、已有输出和坏音频；`tests/cli_pipeline.rs` 是包含原生产源码的独立测试可执行程序，用仅测试可见的假 Transport 在子进程中走参数解析、resolver、LRCLIB、LRC 校验和保存，包括合成带标签 WAV。后者不测试实际 HTTP/TLS，也不冒充所有来源的线上验证；生产二进制没有 fixture 环境变量或任意 URL 注入开关。

本机最终离线验证（2026-10-01，Droidspaces / aarch64，Rust 1.98.1）：格式检查通过；默认 feature 的 127 项库测试、8 项 CLI 单元测试、5 项脚本测试和 4 项实际二进制进程测试通过，另有 16 个离线 CLI 子进程场景通过；关闭 CLI 的 117 项库测试通过；release 构建、sh/bash 语法、异地中文/空格目录的实际 sh/bash 构建和 `git diff --check` 通过。没有安装/运行缺失的 clippy，也未单独运行 Rust 1.89 或 macOS/Windows 原生矩阵。

`.github/workflows/ci.yml` 在 Ubuntu、macOS、Windows 原生执行格式检查、默认 feature 测试、关闭 CLI 的库测试及 release 构建；另有 Ubuntu / Rust 1.89 的 locked 依赖编译检查。权限仅 `contents: read`，不部署、不上传发布产物。实际验证结果与各平台尚未执行的事项应以交付记录和来源状态表为准，不能由这份命令清单推断“已通过”。

## 参考与许可

固定版本的开源参考、接口格式和复用限制见 [docs/providers.md](docs/providers.md)。协议对照不等于复制源代码的许可，也不等于获得歌词内容的再分发权。本仓库尚未选择项目 license，不会因为参考项目的 MIT/Apache/GPL 许可而自动采用其中一种。
