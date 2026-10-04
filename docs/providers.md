---
kind: reference
lang: zh-CN
---

# 来源协议与验证边界

本文记录普通同步 LRC 的接入路线、来源限定 ID、参考版本与许可边界。实现能力由源码和离线 fixture 验证；线上搜索、完整取词与文件保存需要独立的联网证据。日期化结果统一记录在[项目状态文档](status.md)，不把另一平台成功当作原平台接入通过。

## 接入能力

| 来源 | 搜索、ID 与详情 | 普通同步歌词与限制 |
| --- | --- | --- |
| [网易云](#网易云音乐) | EAPI 搜索和精确详情；十进制歌曲 ID | 原生普通歌词接口；保留旧库入口 |
| [QQ 音乐](#qq-音乐) | PC 签名搜索和精确详情；MID 与数字 ID 分开 | 旧普通 LRC 接口；数字 ID 先核对详情并映射到 MID；不支持 QRC |
| [酷狗](#酷狗音乐) | 歌曲搜索；hash、MixSongID 与 Audioid 分开 | hash 候选/下载和显式 `lrc:ID:accesskey`；MixSongID 精确取词及 hash/MixSongID 精确详情未实现；多份可信歌词不自动选择 |
| [酷我](#酷我音乐) | 原生搜索及 H5 RID 精确详情 | H5 普通行歌词；不请求历史 `mobi.s` 包装/逐字路线 |
| [LRCLIB](#lrclib) | `/api/search` 和 `/api/get/{id}` | 只保存 `syncedLyrics`；搜索到达返回上限时视为可能不完整 |

联网成功样本、业务失败样本和最后执行日期见[联网样本记录](status.md#2026-10-01-联网样本)。这张能力表不保证服务在线可用或覆盖所有歌曲。

## 共同规则

- 来源限定 ID 在请求前校验；十进制部分最多 20 位，校验后去除前导零，全零保留为 `0`。QQ MID、酷狗 hash 和公开下载键不按十进制处理。部分详情/取词路线还要求数值能装入 `u64`，见各来源限制。
- 所有请求通过同一 Transport；生产网络只请求固定白名单 HTTPS 接口，校验 TLS，不跟随重定向、不降级 HTTP、不请求任意输入 URL。
- 每个请求 8 秒，单个 Client 的操作预算 90 秒 / 24 次；响应最多 512 KiB，歌词最多 256 KiB，Base64 解码后的内容同样限长。同步 DNS 不能保证由超时器严格中断，这些预算不是所有系统上的绝对墙钟截止时间。每次 `Client::resolve` 新建操作级 HTTP 状态，预算不跨调用累计。
- 同目录临时文件与不覆盖发布防止覆盖已有目标，但原子性/持久性受文件系统支持影响；没有父目录同步，不保证断电后目录项必然持久。
- HTTP `429` / `503` 报告限流/繁忙及 `Retry-After`，不自动重试；LRCLIB 串行至少间隔 300 ms。
- 平台业务状态码、坏字段、解码错误是错误，不等同于没有歌词。统一区分 `Synced`、`PlainOnly`、`Instrumental`、`NotFound`。
- 只有有合法行级时间标签和正文的普通 LRC 才保存，不把 JSON/HTML/XML、纯文本或逐字标记伪装成普通 LRC。
- 标题、完整歌手集合、可信专辑、版本标识需匹配；时长单位转成秒后比较，容差为 ±2 秒。自由关键词或最高分不构成已确认匹配。
- `--provider auto` 允许多来源查询；明确来源只查询该来源。跨源查询只能使用已核实元信息，不能跨平台复用数字 ID。搜索不完整、来源部分失败或多条候选均可要求用户显式选择。

## 网易云音乐

源码：`src/providers/netease.rs`；兼容入口在 `src/lib.rs`。

- ID：最多 20 位十进制数字，`netease:186016`；不指定来源的裸数字默认网易。ID 在请求前校验。
- 搜索：HTTPS `interface.music.163.com/eapi/batch`，EAPI 内部路径 `/api/search/song/list/page`；`keyword` / `needCorrect=1` / `channel=typing` / `offset=0` / `scene=normal` / `total=true` / `limit=30`。
- 详情：同一 HTTPS batch 端点，EAPI 内部路径 `/api/v3/song/detail`；`c` 是包含校验后 ID 的数组 JSON 字符串，返回 ID 必须与请求一致。输入验证允许最多 20 位数字，但该详情路线还要求数值能装入 `u64`，超限会报错。
- EAPI 是标准 AES-128-ECB / PKCS#7 + MD5 的协议包装；不是明文 GET，也不是自行实现 AES 底层。请求对象只序列化一次用于签名/包装。只发送静态客户端标记，不发送用户账号 Cookie。
- 歌词：HTTPS `music.163.com/api/song/lyric/v1`，发送校验后 `id` 和普通歌词相关版本参数；业务 `code=200`，使用 `lrc.lyric`。
- 元信息：搜索 `/data/resources` 内的 `baseInfo/simpleSongData`；详情常用 `id`、`name`、`ar`/`artists`、`al`/`album`、`dt`/`duration`，时长为毫秒，除以 1000。
- `nolyric=true` 与 `uncollected=true` 分开处理；网易行级 JSON 作词信息可转换，但只有作者信息而没有歌词正文不算同步歌词。
- 搜索 `data.totalCount` 判断完整性；有剩余结果时不会因第一页仅有一个好候选就自动认定唯一。

实际使用的是 EAPI batch 承载搜索和详情，不是旧 `/api/cloudsearch/pc`，也不是把需包装的接口当作明文请求。线上样本见[验证记录](status.md#2026-10-01-联网样本)，显式选歌不代表可以自动选第一首。

## QQ 音乐

源码接入入口：`src/providers/qq.rs`。

- MID：14 位字母数字，`qq:0039MnYb0qxYhV`，也接受 `qq:mid:MID`。
- 数字歌曲 ID：最多 20 位数字，使用 `qq:id:数字`；两种 ID 不能混用。
- PC 搜索：HTTPS `u.y.qq.com/cgi-bin/musics.fcg`，模块 `music.search.SearchCgiService` / 方法 `DoSearchForQQMusicDesktop`；SHA-1 派生的 `zzc` 请求校验字符串和 JSON 请求体对应，分页从 `page_num=1` 起，`num_per_page=30`。
- 详情：HTTPS `u.y.qq.com/cgi-bin/musicu.fcg`，模块 `music.pf_song_detail_svr` / 方法 `get_song_detail_yqq`，MID 和数字 `song_id` 不混用；数字 ID 详情需能装入 `u64` 并核对返回数字 ID→MID 映射。
- 歌词：HTTPS `c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg`，校验后的 `songmid` / `g_tk=5381` / `format=json` / `nobase64=0` 等普通 LRC 参数；数字 ID 先读取详情、核对后映射到 MID。
- 响应 JSON 或单个严格 JSONP 包装本地解析，不执行 JavaScript；普通歌词 Base64 UTF-8 再解码 HTML entities。`crypt1` / QRC 不支持，也不使用标准 3DES 假装能解平台自定义格式。
- 歌曲字段 `mid`/`songmid`、`title`/`name`/`songname`、`singer`、`album`/`albumname`、`interval`，时长是秒；搜索 `data.meta.sum` 判断完整性。

普通歌词响应兼容 `retcode`-only 成功状态；若 `code` / `retcode` 同时出现，两者都需为零。该兼容变体由离线 fixture 验证，线上取词证据见[联网样本记录](status.md#2026-10-01-联网样本)。

## 酷狗音乐

源码接入入口：`src/providers/kugou.rs`。

- hash：32 位十六进制，`kugou:HASH` / `kugou:hash:HASH`；校验后统一大写。
- 普通歌词候选引用：`kugou:lrc:歌词ID:accesskey`，ID/公开下载键必须成对保留，key 最多 128 位字母数字。这是精确歌词下载引用，不是歌曲 MID，也不是账号凭据。
- MixSongID：`kugou:mix:数字`；Audioid、MixSongID、hash 是不同字段，不把数字混当 hash。
- 搜索：HTTPS `songsearch.kugou.com/song_search_v2`；`keyword` 搜索，返回 `FileHash`、`Audioid`、`MixSongID` 等，业务 `status=1` / `error_code=0`。
- 歌词候选：HTTPS `lyrics.kugou.com/search`，`ver=1` / `man=yes` / `client=pc` / `hash` / `timelength` / `lrctxt=1`，业务 `status=200`；下载用候选的 `id` 与 `accesskey` 成对保留。
- 下载：同域 `/download`，`fmt=lrc` / `charset=utf8`，只解码 Base64 UTF-8 普通 LRC，不输出 KRC。
- 时间单位：旧 `timelength` 是秒；歌词候选 `duration` 是毫秒，不能把约 239 秒当 239000 秒或反之。
- 候选可能没有 hash，也可能对同一查询返回多份歌词；必须核对标题、完整歌手集合及 ±2 秒时长，不能仅 `candidates[0]`。歌手名称内的标点保留，合作歌手只在完整已知名称边界匹配，重叠名称通过有界状态搜索消歧；最多 16 个歌手、4096 个状态和 1 MiB 前缀比较工作，超限拒绝匹配。多份可信歌词会明确停止并列出 `lyrics-fetcher --lyrics kugou:lrc:歌词ID:accesskey` 命令，不默认下载首条；歌词层歧义报告为错误，退出码见 [README](../README.md#保存与退出码)。

MixSongID/hash 的精确详情缺少已核实固定 HTTPS 路线。关键词或文件输入加 `--select kugou:HASH` 时，会只重搜酷狗，用原查询找回精确 hash 的服务端元信息，再核对歌词版本；所选 hash 不在返回页中则明确失败，需缩小关键词。该路线有离线 Transport 回归；日期化选歌→选词→保存证据见[联网样本](status.md#2026-10-01-联网样本)。直接 hash/MixSongID 的无元信息取词不支持。

## 酷我音乐

源码接入入口：`src/providers/kuwo.rs`。

- RID：最多 20 位十进制，`kuwo:数字`；合法 `MUSIC_` 前缀只做平台内正规化。详情/取词的响应身份核对还要求请求 RID 能装入 `u64`，超限报错。
- 搜索：HTTPS `search.kuwo.cn/r.s`，`client=kt` / `all=关键词` / `pn=0` / `rn` / `ft=music` / `rformat=json`。页码从 0 起。
- 线上响应可能是单引号对象而非严格 JSON，需平台专用有边界解析，不把服务端文本当代码执行。
- 当前歌词/详情路线：HTTPS `m.kuwo.cn/newh5/singles/songinfoandlrc?musicId=...`，核对 `data.songinfo.id` 和可选 `musicrId`，业务成功才使用元信息与 `lrclist`；歌名、歌手、专辑、秒级时长用于候选/补查。
- `lrclist` 的 `time` 是秒，转为普通 LRC 行时间；保留长度上限并验证正文，不输出逐字格式。歌词是否含翻译取决于平台返回内容，当前接口未单独请求翻译。
- H5 即使 HTTP 200，也必须核对业务状态；业务失败时不编造详情、不用未知元信息自动跨源补查。
- 历史 `mlyric.kuwo.cn/mobi.s` 即使请求 `lrcx=0` 仍可能返回 `lrcx=1` 的包装/逐字数据；**生产实现不走该路线，不承诺解包逐字歌词**。调研中能解包某份响应不代表 CLI 实现了此协议。

H5 成功与业务失败样本见[联网验证记录](status.md#2026-10-01-联网样本)。

## LRCLIB

源码：`src/providers/lrclib.rs`。

- ID：最多 20 位十进制平台记录 ID，`lrclib:数字`，不是网易/QQ 歌曲 ID；详情/取词的响应身份核对还要求请求 ID 能装入 `u64`，超限报错。
- 搜索：HTTPS `lrclib.net/api/search`，可以使用 `q` 或 `track_name` / `artist_name` / `album_name` 等字段；`q` 会覆盖结构化条件，客户端必须自行核对返回元信息。
- 详情/歌词：HTTPS `/api/get/{id}`；返回记录 ID、`trackName`、`artistName`、`albumName`、秒级 `duration`、`instrumental`、`syncedLyrics`、`plainLyrics`。
- 只保存 `syncedLyrics`；只有 `plainLyrics` 不编造时间标签，`instrumental=true` 单独报告。
- 搜索最多 20 条，无分页；到 20 条标记可能不完整，不能推断只有一个匹配。`get` 的记录返回也不是关键词全局唯一匹配证明。
- 客户端按秒比较时长，容差 ±2 秒；请求串行间隔至少 300 ms，限流时不自动重试。

搜索截断和精确记录下载的线上样本见[联网验证记录](status.md#2026-10-01-联网样本)，不据此自动选中首条。

## 支持的完整歌曲链接

链接在本地解析，只有提取出的来源限定 ID 会进入固定 HTTPS 接口。允许输入 `http://` 完整链接只是解析历史分享格式，**不会发送明文 HTTP 请求**。拒绝端口、用户信息、未知域名、未知页面和短链；以查询参数提取 ID 的路线拒绝重复 ID 参数，路径直接包含 ID 的路线不读取附带 query。

| 来源 | 接受的 host 与主要路径 |
| --- | --- |
| 网易 | `music.163.com` / `y.music.163.com` 的 `/song`、`/m/song` + `id`；`music.163.com/#/song?id=...` / `#/m/song?id=...` |
| QQ | `y.qq.com` / `c.y.qq.com` 的 `/n/ryqq/songDetail/MID`、`/n/yqq/song/MID.html`；限定页面可带唯一 `songmid` 或 `songid`，包括 `/base/fcgi-bin/u` / `/r/` |
| 酷狗 | `www.kugou.com/song/MixSongID.html`；`/song` 或 `/song/` 的 query/fragment 里唯一 `hash` 或 `mixsongid`，不同时接受两套 ID |
| 酷我 | `www.kuwo.cn` / `m.kuwo.cn` 的 `/play_detail/RID`；`/newh5/singles/songinfoandlrc` 或 `/yinyue/` + `musicId` |
| LRCLIB | `lrclib.net/api/get/ID` |

准确条件以 `src/input.rs` 为准。`163cn.tv`、`c6.y.qq.com`、`t1.kugou.com` 等短链不跟随，需换完整链接或 `来源:ID`。域名相似、存在官方子串不等于白名单。

## 固定参考和许可边界

参考项目不是运行时依赖，不要求部署它们的服务器。协议事实用于独立实现；不得复制/翻译 GPL-only 或带附加限制的源代码，再以本仓库无许可或宽松许可的名义分发。

| 项目与固定版本 | 路径 / 用途 | 许可边界 |
| --- | --- | --- |
| [lyswhut/lx-music-desktop](https://github.com/lyswhut/lx-music-desktop/tree/ad95d5091c9ed689fa72b5e5c849df65f5a679ce) `ad95d5091c9ed689fa72b5e5c849df65f5a679ce` | `src/renderer/utils/musicSdk/wy/musicSearch.js`、`wy/utils/index.js`、`tx/musicSearch.js`、`tx/utils/index.js`、`tx/musicInfo.js`、`kg/musicSearch.js`、`kg/lyric.js`、`kg/musicInfo.js`、`kw/musicSearch.js`、`kw/lyric.js`、`kw/util.js`；平台请求字段与格式对照 | Apache-2.0 **之外还有补充限制**，不能仅按普通 Apache 项目处理；本项目不直接复制实现 |
| [L-1124/QQMusicApi](https://github.com/L-1124/QQMusicApi/tree/ba95861ee9391f5b5f60f89caa8d4de4af160c8b) `ba95861ee9391f5b5f60f89caa8d4de4af160c8b` | `qqmusic_api/modules/lyric.py`；QQ 请求与响应协议对照 | 文件级 GPL 边界，只参考协议事实，不直接移植签名/QRC 算法 |
| [Rain120/qq-music-api](https://github.com/Rain120/qq-music-api/tree/d05420bf098bd2769866eba81cfd48a6d0c6f50c) `d05420bf098bd2769866eba81cfd48a6d0c6f50c` | QQ 数字歌曲 ID 精确详情协议对照 | MIT；只提取协议事实，不直接复制实现，不采用 QRC 解密 |
| [metowolf/Meting](https://github.com/metowolf/Meting/tree/1c2f4c98eed749200d9d7ff5cab329c4308f4268) `1c2f4c98eed749200d9d7ff5cab329c4308f4268` | `src/providers/netease.js`、`src/providers/tencent.js`；网易 EAPI 和 QQ 旧普通 LRC 协议事实 | MIT；固定 commit 与许可证经接入负责人远程核实，独立实现，不直接复用代码 |
| [tranxuanthang/lrclib](https://github.com/tranxuanthang/lrclib/tree/05ad8590f6fc4d47a2d74e70f4915273df20f63c) `05ad8590f6fc4d47a2d74e70f4915273df20f63c` | `server/src/router.rs`，`api/search`、`api/get/:id` 路由与服务端行为 | MIT；阅读对照不表示复制代码 |
| [tranxuanthang/lrclib-homepage](https://github.com/tranxuanthang/lrclib-homepage/tree/f37c07042be1af5fdcc7932d090af32141089751) `f37c07042be1af5fdcc7932d090af32141089751` | 官方 API 文档，参数、匹配与限流建议；服务端代码纠正文档差异 | MIT；文档不能单独证明某首中文歌曲可取 |

网易与 QQ 的固定对照文件列在上表。只提取协议事实，不直接复制实现、不采用 QRC 解密，也不移植 GPL 参考中的算法。未采用的历史参考及核实缺口见[调查记录](status.md#历史参考调查)。

本仓库尚无项目 license，不在此自动选择 MIT/Apache/GPL。若未来直接复用允许使用的代码，应先按文件级许可保留版权、声明和必要 notice；服务协议与歌词内容的使用/再分发权另行判断。
