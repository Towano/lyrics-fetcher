# AGENTS.md

## 项目范围

- 本仓库维护独立 Rust crate 与 CLI `lyrics-fetcher`，联网检索网易云音乐、QQ 音乐、酷狗、酷我和 LRCLIB 的普通同步 LRC，并安全保存到文件。
- CLI 使用 `-l` / `--lyrics`，接受关键词、来源限定歌曲 ID、支持的歌曲链接和本地音频文件；有歧义时显示候选，由 `--select 来源:ID` 明确选择，不静默取首条。
- 保持公开接口不依赖调用方的歌曲元数据结构或具体应用；不要在本仓库加入 `ncm-audio` 集成代码。保留原 `save_for_track` 和 `LyricsResult` 的兼容性。
- 不实现账号登录、音频解密、翻译、罗马音或逐字歌词输出。第三方协议变化或接口受限必须明确报告，不用另一来源冒充原生平台支持。

## 实现约定

- 网络请求统一走受限 Transport，保留来源限定歌曲 ID 校验、HTTPS 固定域名白名单、请求超时、总时限、请求次数以及响应体和歌词大小限制。不得跟随任意输入 URL、关闭 TLS 校验或自动降级到 HTTP。
- Base64 解码及解压后的歌词同样限长；区分同步歌词、纯文本、纯音乐、未找到和网络/协议错误，不给纯文本编造时间标签。
- 跨来源检索依赖已核实的标题、歌手、专辑、版本和时长，不跨平台复用歌曲 ID；关键词匹配证据不足时必须要求选择。
- 歌词文件写入需保持同目录临时文件、同步和不覆盖发布，避免先检查存在后直接写入的竞态。已有目标（含符号链接）不得覆盖，不擅自创建父目录。
- 本地音频只读取必要标签和时长，不上传、不修改音频，不读取封面；保留 `Path` / `OsString` 的跨平台路径语义。
- 默认启用 `cli` feature；库使用方能通过 `default-features = false` 排除 CLI 与音频标签依赖。
- 单元和集成测试使用本地 fixture 或假 Transport / Cargo，不依赖真实歌词服务或外部网络。少量只读联网 smoke 与常规测试分开，记录各来源结果，不扫站。
- 新增依赖前先确认现有依赖无法满足需求，并保持模块实现精简。参考开源协议时记录固定 commit、文件及许可边界；不复制受限算法，也不擅自为本仓库选择 license。
- 本地构建脚本用 POSIX sh，兼容 sh/bash，从脚本位置定位仓库，原样透传 Cargo 参数；不安装工具、修改全局配置或自动配置交叉编译。

## 验证

在仓库根目录运行：

```sh
cargo fmt -- --check
cargo test --locked
cargo test --locked --no-default-features --lib
cargo build --locked --release --bin lyrics-fetcher
sh -n build.sh
bash -n build.sh
```

- bash 缺失时只跳过 bash 专属检查，不安装工具；Unix shell 集成测试在 Windows 原生环境以 `cfg(unix)` 跳过。
- CI 使用 Linux/macOS/Windows 原生 runner。提交说明区分本机已执行验证、联网 smoke 和尚未执行的远端 CI；Droidspaces 的 GNU/Linux 用户空间验证不等于原生 Android/Termux 支持。
