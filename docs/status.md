---
kind: changelog
lang: zh-CN
---

# 项目状态与验证记录

按日期记录执行结果，不代替 [CLI 用法](../README.md)或[来源协议](providers.md)。本机离线检查、对应提交的远端 CI、真实服务联网样本分别记录；通过其中一项不代表其他项已验证。

## 2026-10-04 状态核实

- 本地 HEAD 与远端 `main` 均为 [`dd0583a1bb9696106e3f312b3efd3244b3c4a53b`](https://github.com/Towano/lyrics-fetcher/commit/dd0583a1bb9696106e3f312b3efd3244b3c4a53b)，没有发现更新的远端提交。
- 该提交在功能分支和 `main` 的原生平台 CI、Rust 1.89 编译检查均通过；macOS 的部分非 UTF-8 文件名 fixture 因文件系统限制跳过，详见[远端 CI](#远端-ci)。
- 本机完整离线检查通过，详见[本机离线验证](#本机离线验证)。
- GitHub 开放 PR、开放 Issue、Release 和 tag 均为 0；release 构建通过不代表已发布安装包。
- 本次未请求真实歌词服务。最后保留的线上证据是 [2026-10-01 联网样本](#2026-10-01-联网样本)，不能视为 2026-10-04 的服务可用性验证。

### 文档检查

使用已安装的 seiso 0.3.0 为 README、来源协议、项目指南声明文档类型，新增本状态记录集中日期化证据；项目指南的约束正文未改。

`seiso check --no-cache` 和 `seiso check --preview --no-cache` 检查四份 Markdown 均无诊断，`git diff --check` 通过。没有规则抑制、排除配置或新增 CI 门禁；preview 仅作本次人工复核，不等同于稳定规则保证。

### 本机离线验证

执行日期：2026-10-04。代码提交：`dd0583a`；本次只修改文档。

环境为 Droidspaces / aarch64，Android 内核上的 GNU/Linux 用户空间，Rust/Cargo 1.98.1。rustup 未配置默认工具链，但已有 stable；仅为本次命令设置 `RUSTUP_TOOLCHAIN=stable` 和 `CARGO_NET_OFFLINE=true`，不安装工具、不修改默认配置、不下载依赖。

在仓库根目录执行，以下命令均 exit 0：

| 检查 | 命令 | 结果 |
| --- | --- | --- |
| 格式 | `cargo fmt -- --check` | 通过 |
| 默认 feature | `cargo test --locked` | 库 130、CLI 单元 10、脚本 5、真实 CLI 进程 4 项通过；另有 16 个离线 CLI 子进程场景通过、0 跳过 |
| 关闭 CLI | `cargo test --locked --no-default-features --lib` | 库 120 项通过 |
| release CLI | `cargo build --locked --release --bin lyrics-fetcher` | 通过 |
| POSIX sh 语法 | `sh -n build.sh` | 通过 |
| bash 语法 | `bash -n build.sh` | 通过 |

真实 CLI 进程测试执行 Cargo 测试 profile 的二进制；release 构建单独记录，不把两者混写。测试使用本地 fixture、假 Transport 和假 Cargo，不验证真实服务的 HTTP/TLS。

本机未单独执行 Rust 1.89 工具链、clippy 或 macOS/Windows 原生测试；原生平台结果来自下列远端日志。本次未重跑历史的异地中文/空格路径实际构建，也未验证原生 Android/Termux。

### 远端 CI

核实日期：2026-10-04；最新对应 `main` 的运行是 [37038559254](https://github.com/Towano/lyrics-fetcher/actions/runs/37038559254)，代码提交 `dd0583a`。最后一个 job 于 2026-10-02 17:12:19 UTC 完成，run 于 17:12:21 UTC 更新，整体成功。

| Job | 结果 | 已执行范围 |
| --- | --- | --- |
| [Ubuntu](https://github.com/Towano/lyrics-fetcher/actions/runs/37038559254/job/110942703800) | 通过 | 格式、默认 feature 测试、关闭 CLI 的库测试、release CLI 构建 |
| [macOS](https://github.com/Towano/lyrics-fetcher/actions/runs/37038559254/job/110942703470) | 通过，含 fixture 跳过 | 同上；非 UTF-8 文件名受 `EILSEQ` 限制 |
| [Windows](https://github.com/Towano/lyrics-fetcher/actions/runs/37038559254/job/110942703971) | 通过 | 同上；Unix 专属测试不编译 |
| [Rust 1.89](https://github.com/Towano/lyrics-fetcher/actions/runs/37038559254/job/110942703801) | 通过 | Ubuntu 上的 `cargo check --locked --all-targets` 和 `cargo check --locked --no-default-features --lib`；不是测试运行或 release 构建 |

三个原生平台 job 使用 Rust 1.99.0。日志统计如下，平台条件编译会改变测试数量：

| 平台 | 默认库 | CLI 单元 | 脚本 | 真实 CLI 进程 | 离线 CLI 子进程 | 关闭 CLI 的库 |
| --- | --- | --- | --- | --- | --- | --- |
| Ubuntu | 130 | 10 | 5 | 4 | 16 通过 / 0 跳过 | 120 |
| macOS | 130，含 fixture 提前返回 | 10，含 fixture 部分跳过 | 5 | 4 | 15 通过 / 1 跳过 | 120，含 fixture 提前返回 |
| Windows | 122 | 7 | 0 | 4 | 14 通过 / 0 跳过 | 113 |

macOS 日志明确跳过默认库的 `non_utf8_input`、`non_utf8_audio_metadata`、`non_utf8_stem_collision`，CLI 单元的 `non_utf8_cli_paths` 文件 fixture 部分，pipeline 的 `non_utf8_output`；关闭 CLI 的库再次跳过 `non_utf8_input`、`non_utf8_stem_collision`。

跳过条件由 [`tests/support/mod.rs`](../tests/support/mod.rs) 严格限定为 macOS、非 UTF-8 路径、创建返回 `EILSEQ`（错误码 92）；其他错误仍失败。库/CLI 单元测试打印提示后提前返回，因此 libtest 的 `0 ignored` 不等于文件 fixture 全部执行。pipeline 自行汇总为 **15 个通过、1 个跳过**。断链符号链接测试仍执行，原始 `OsString` 参数断言在 fixture 创建前执行。Windows 的 `0 skipped` 也不代表拥有全部 Unix 场景。

此前两次运行保留为历史证据：

| 日期（UTC） | 运行与提交 | 结果 |
| --- | --- | --- |
| 2026-10-02 | [36955394496](https://github.com/Towano/lyrics-fetcher/actions/runs/36955394496)，`2013255c6dc60ae77e6fdefdc4b40bc03d115213`，功能分支 | Ubuntu、Windows、Rust 1.89 通过；macOS 因非 UTF-8 fixture 创建 `EILSEQ`，库测试 124 通过、3 失败，后续无 CLI 测试及 release 构建未执行 |
| 2026-10-02 | [36958775015](https://github.com/Towano/lyrics-fetcher/actions/runs/36958775015)，`dd0583a`，功能分支 | 修复后四个 job 均通过；macOS 的 fixture 跳过与后续 `main` 运行一致；最后 job 于 03:13:11 UTC 完成 |

旧失败不代表修复提交仍失败；绿色 CI 也不代表五来源在线端到端覆盖、全部歌曲可取词或原生 Android/Termux 支持。[CI 配置](../.github/workflows/ci.yml) 未部署或上传发布产物；本次核实未触发新的 CI。

## 历史本机记录

以下内容迁自原 README 和来源文档，是当时的执行记录；未补填无法核实的提交 SHA，不把历史数量当成本次统计。

- **2026-10-01**：Droidspaces / aarch64，Rust 1.98.1；格式通过，默认库 127、CLI 单元 8、脚本 5、真实 CLI 进程 4 项及 16 个离线子进程场景通过；关闭 CLI 的库 117 项通过。release 构建、sh/bash 语法、异地中文/空格目录的实际 sh/bash 构建与 `git diff --check` 通过。未运行缺失的 clippy，也未在本机执行 Rust 1.89 或原生 macOS/Windows。
- **2026-10-02 修复后**：本机默认库 130、CLI 单元 10、脚本 5、真实 CLI 进程 4 项及 16 个离线子进程场景通过，pipeline 0 跳过；关闭 CLI 的库 120 项通过。macOS 非 UTF-8 fixture 的兼容修复随后由上述远端运行验证。

## 2026-10-01 联网样本

本节迁自原来源文档，保留当时的少量只读 HTTPS smoke 和 release CLI 结果，2026-10-04 没有重新联网执行。人工指定样本 ID 不代表允许自动选择搜索首条，也不保证所有歌曲、搜索路线或最终提交都重跑过。

| 来源 | 当时的搜索 / 详情证据 | 当时的取词 / 保存结果 |
| --- | --- | --- |
| 网易云 | 生产 Transport 的 `晴天` 搜索返回 20 条，`complete=false`；显式选择 `2652820720`（晴天(深情版) / Lucky小爱）取得 1734 bytes 普通 LRC | release CLI `-l netease:186016 --provider netease` 保存 2453 bytes / 62 行，exit 0 |
| QQ 音乐 | HTTPS 搜索返回 30 条，`complete=false`；MID `0039MnYb0qxYhV`（晴天 / 周杰伦）详情核对成功 | 普通 LRC 样本 `code=0` / `retcode=0`；release CLI 保存 2547 bytes / 68 行，exit 0；另一次暂态异常被拒绝，`retcode`-only 变体仅补离线 fixture |
| 酷狗 | release CLI 查询 `周杰伦 青花瓷` 返回歌曲候选，exit 4；原查询加 `--select kugou:055A1AE5D7B2355BBBB52307E99E43A9` 返回多份歌词引用并停止，exit 1 | 显式选择列出的歌词 ID `108476647` 后保存 2146 bytes / 67 行，exit 0；此前歌词 ID `274944371` 也取词成功；不构成裸 hash/MixSongID 无元信息取词支持 |
| 酷我 | `r.s` HTTPS 搜索成功；H5 RID `228908` 返回业务成功和普通行歌词；较早的 RID `23928868` 虽 HTTP 200，但业务 `status=301` | RID `228908` 的 CLI 下载/保存 2551 bytes / 63 行，exit 0；业务失败不当作成功，不编造元信息跨源补查 |
| LRCLIB | HTTPS 搜索成功；`track_name=Yellow` / `artist_name=Coldplay` 样本返回 20 条，可能截断 | 记录 `36902193` 的 CLI 下载/保存 2540 bytes / 56 行，exit 0 |

provider 审查修复和 release 重建后，又核实网易 `netease:000186016`、QQ `qq:0039MnYb0qxYhV`、酷我 `kuwo:228908`、LRCLIB `lrclib:36902193` 均 exit 0，字节/行数与表中一致。酷狗重跑原查询 → 表中 hash → 明确歌词 ID `108476647` → 保存，依次 exit 4 / 1 / 0。五来源同目标重跑均 exit 5 且文件字节不变，临时输出已清理。

另有两项 CLI 样本：合成中文/空格路径 WAV 加 `--select netease:186016`，默认同名 `.lrc` 保存成功且音频字节不变；网易完整 `#/song?id=186016` 链接保存成功。之后只对本地 MP4 属性的 `mdhd` / `stts` 加了版本/边界校验并重跑完整离线检查，未再重复联网样本。

酷狗较早的协议字段样本为 hash `37A8F50A9EC3B267C3CC6BEC633D9C4A`、Audioid `339796`、MixSongID `32218352`；歌词查询 `timelength=239` 的候选 `duration=239386`，用于核对秒与毫秒的区别，不证明所有候选是同一录音版本或可自动下载。

## 历史参考调查

原接入文档记录：`ELDment/Meting-Fixed` 的 `src/Meting.php` 是 MIT 多源协议参考，但计划所列 `4a386c206483c95f2d1c9567328b6ea6ca6c7e22` 在当时的远程文件/commit 查询中未找到，**该固定版本未核实、未采用**。原记录未单独注明查询日期，2026-10-04 未复查；不能把它当作实现或线上可用性证据。已采用的固定协议参考及许可边界见 [providers.md](providers.md#固定参考和许可边界)。
