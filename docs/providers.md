# 来源协议与验证边界

本文记录普通同步 LRC 的接入路线、来源限定 ID、参考版本与许可边界。**源码接入、离线 fixture 通过、线上搜索成功、完整线上取词成功是不同状态**。第三方服务可能因地区、限流、客户端参数或接口变化失败，不能把另一平台的歌词成功当作原平台接入通过。

## 能力和在线验证

当前实现与少量只读 HTTPS smoke 的状态如下。尚在接入或未验证的格子不表示已完成。

| 来源 | 搜索 / ID / 详情能力 | 线上结果与限制 |
| --- | --- | --- |
| 网易云 | EAPI 搜索、EAPI 精确详情、原生普通歌词接口已接入；保留旧网易库入口 | 2026-10-01 生产 Transport 的 HTTPS 搜索→详情→LRC 已成功：`晴天` 返回 20 条、`complete=false`，选定 `2652820720`（晴天(深情版) / Lucky小爱）取得 1734 bytes 普通 LRC；另以release CLI `-l netease:186016 --provider netease` 保存 2453 bytes / 62 行，exit 0；同目标重跑 exit 5，文件字节不变。人工指定这些 ID 不代表允许自动选首条 |
| QQ 音乐 | 原生 PC 签名搜索、精确详情与旧普通 LRC 路线已接入；MID 和数字 ID 分开 | HTTPS 搜索 30 条、`complete=false`，MID `0039MnYb0qxYhV`（晴天 / 周杰伦）详情核对成功，普通 LRC 成功样本 `code=0` / `retcode=0`，2547 bytes。另一次暂态异常被拒绝；`retcode`-only 兼容已补离线 fixture。release CLI 已保存 2547 bytes / 68 行（exit 0），重跑目标 exit 5 且文件不变 |
| 酷狗 | HTTPS 歌曲搜索、hash 歌词候选/下载、显式 `lrc:ID:accesskey` 已接入；MixSongID 精确取词与 hash/MixSongID 精确详情尚未实现 | 2026-10-01 release CLI 用 `周杰伦 青花瓷` 搜索 exit 4；原查询加 `--select kugou:055A1AE5D7B2355BBBB52307E99E43A9` 取得多份歌词引用并停止（exit 1）；显式选择列出的歌词 ID `108476647` 后保存 2146 bytes / 67 行（exit 0），重跑 exit 5 且字节不变。此前歌词 ID `274944371` 也已取词成功。hash 选择会重搜元信息，但裸 hash / MixSongID 的无元信息取词仍不支持 |
| 酷我 | 原生搜索、H5 RID 普通歌词及精确详情路线已接入 | `r.s` HTTPS 搜索成功；H5 RID `228908` 的 CLI HTTPS 下载/保存 exit 0，2551 bytes / 63 行；RID `23928868` 的较早样本业务 `status=301`。当前实现以 H5 为主，不请求历史 `mobi.s` 包装/逐字路线 |
| LRCLIB | `/api/search` 与 `/api/get/{id}` 源码已接入，只保存 `syncedLyrics` | HTTPS 搜索成功；样本 20 条可能截断。记录 `36902193` 的 CLI HTTPS 下载/保存 exit 0，2540 bytes / 56 行；不保证覆盖或替客户端聚合中文平台 |

provider 离线测试使用 Fake Transport 和内存响应，标签/文件/CLI 测试使用本地合成数据；最终测试执行记录不由上表的源码能力推断。2026-10-01 provider 审查修复和 release 重建后，已再核实网易 `netease:000186016`、QQ `qq:0039MnYb0qxYhV`、酷我 `kuwo:228908`、LRCLIB `lrclib:36902193` 均 exit 0，字节/行数与上表一致；酷狗也重跑了原查询 → 上表 hash → 明确歌词 ID `108476647` → 保存流程，依次 exit 4 / 1 / 0，2146 bytes / 67 行。五个来源同目标重跑均 exit 5 且字节不变，临时输出已清理。这不等于所有搜索路线和歌曲均在最终版本重跑。随后仅对本地 MP4 属性的 `mdhd` / `stts` 加了版本/边界校验并重跑完整离线检查，未再重复联网样本。

另已用合成的中文/空格路径 WAV + `--select netease:186016` 验证本地音频输入默认同名保存，音频字节保持不变；网易完整 `#/song?id=186016` 链接输入也已通过 CLI 保存。macOS/Windows 原生 CI 与 MSRV 1.89 工具链不在本机环境执行；2026-10-02 已核实首轮远端 Ubuntu、Windows 和 Rust 1.89 检查通过，macOS 非 UTF-8 文件名 fixture 创建失败，测试兼容修复及后续状态见 README 的验证记录。

## 共同规则

- 所有请求通过同一 Transport；生产网络只请求固定白名单 HTTPS 接口，校验 TLS，不跟随重定向、不降级 HTTP、不请求任意输入 URL。
- 每个请求 8 秒，单个 Client 的操作预算 90 秒 / 24 次；响应最多 512 KiB，歌词最多 256 KiB，Base64 解码后的内容同样限长。同步 DNS 不能保证由超时器严格中断，这些预算不是所有系统上的绝对墙钟截止时间。每次 `Client::resolve` 新建操作级 HTTP 状态，预算不跨调用累计。
- 同目录临时文件与不覆盖发布防止覆盖已有目标，但原子性/持久性受文件系统支持影响；没有父目录同步，不保证断电后目录项必然持久。
- HTTP `429` / `503` 报告限流/繁忙及 `Retry-After`，不自动重试；LRCLIB 串行至少间隔 300 ms。
- 平台业务状态码、坏字段、解码错误是错误，不等同于没有歌词。统一区分 `Synced`、`PlainOnly`、`Instrumental`、`NotFound`。
- 只有有合法行级时间标签和正文的普通 LRC 才保存，不把 JSON/HTML/XML、纯文本或逐字标记伪装成普通 LRC。
- 标题、完整歌手集合、可信专辑、版本标识需匹配；时长单位转成秒后比较，当前容差为 ±2 秒。自由关键词或最高分不构成已确认匹配。
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

实际使用的是 EAPI batch 承载搜索和详情，不是旧 `/api/cloudsearch/pc`，也不是把需包装的接口当作明文请求。上述 20 条搜索样本仍为不完整，线上 smoke 的显式选歌不代表可以自动选第一首。

## QQ 音乐

源码接入入口：`src/providers/qq.rs`。

- MID：14 位字母数字，`qq:0039MnYb0qxYhV`，也接受 `qq:mid:MID`。
- 数字歌曲 ID：最多 20 位数字，使用 `qq:id:数字`；两种 ID 不能混用。
- PC 搜索：HTTPS `u.y.qq.com/cgi-bin/musics.fcg`，模块 `music.search.SearchCgiService` / 方法 `DoSearchForQQMusicDesktop`；SHA-1 派生的 `zzc` 请求校验字符串和 JSON 请求体对应，分页从 `page_num=1` 起，`num_per_page=30`。
- 详情：HTTPS `u.y.qq.com/cgi-bin/musicu.fcg`，模块 `music.pf_song_detail_svr` / 方法 `get_song_detail_yqq`，MID 和数字 `song_id` 不混用；数字 ID 详情需能装入 `u64` 并核对返回数字 ID→MID 映射。
- 歌词：HTTPS `c.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg`，校验后的 `songmid` / `g_tk=5381` / `format=json` / `nobase64=0` 等普通 LRC 参数；数字 ID 先读取详情、核对后映射到 MID。
- 响应 JSON 或单个严格 JSONP 包装本地解析，不执行 JavaScript；普通歌词 Base64 UTF-8 再解码 HTML entities。`crypt1` / QRC 不支持，也不使用标准 3DES 假装能解平台自定义格式。
- 歌曲字段 `mid`/`songmid`、`title`/`name`/`songname`、`singer`、`album`/`albumname`、`interval`，时长是秒；搜索 `data.meta.sum` 判断完整性。

生产 HTTPS 搜索与详情 smoke 已通过；最终普通歌词兼容 `retcode`-only 成功状态，若 `code` / `retcode` 同时出现，两者都需为零；该兼容变体只由离线 fixture 验证。2026-10-01 release CLI 使用 `--lyrics qq:0039MnYb0qxYhV --provider qq` 保存 2547 bytes / 68 行，exit 0；同目标重跑 exit 5，文件字节不变。

## 酷狗音乐

源码接入入口：`src/providers/kugou.rs`。

- hash：32 位十六进制，`kugou:HASH` / `kugou:hash:HASH`；校验后统一大写。
- 普通歌词候选引用：`kugou:lrc:歌词ID:accesskey`，ID/公开下载键必须成对保留，key 最多 128 位字母数字。这是精确歌词下载引用，不是歌曲 MID，也不是账号凭据。
- MixSongID：`kugou:mix:数字`；Audioid、MixSongID、hash 是不同字段，不把数字混当 hash。
- 搜索：HTTPS `songsearch.kugou.com/song_search_v2`；`keyword` 搜索，返回 `FileHash`、`Audioid`、`MixSongID` 等，业务 `status=1` / `error_code=0`。
- 歌词候选：HTTPS `lyrics.kugou.com/search`，`ver=1` / `man=yes` / `client=pc` / `hash` / `timelength` / `lrctxt=1`，业务 `status=200`；下载用候选的 `id` 与 `accesskey` 成对保留。
- 下载：同域 `/download`，`fmt=lrc` / `charset=utf8`，只解码 Base64 UTF-8 普通 LRC，不输出 KRC。
- 时间单位：旧 `timelength` 是秒；歌词候选 `duration` 是毫秒，不能把约 239 秒当 239000 秒或反之。
- 候选可能没有 hash，也可能对同一查询返回多份歌词；必须核对标题、完整歌手集合及 ±2 秒时长，不能仅 `candidates[0]`。歌手名称内的标点保留，合作歌手只在完整已知名称边界匹配，重叠名称通过有界状态搜索消歧；最多 16 个歌手、4096 个状态和 1 MiB 前缀比较工作，超限拒绝匹配。多份可信歌词会明确停止并列出 `lyrics-fetcher --lyrics kugou:lrc:歌词ID:accesskey` 命令，不默认下载首条；当前此歌词层歧义作为错误报告（退出码 1），不是歌曲层候选的退出码 4。

少量线上样本 `周杰伦 青花瓷` 的歌曲搜索返回 hash `37A8F50A9EC3B267C3CC6BEC633D9C4A`、Audioid `339796`、MixSongID `32218352`；歌词搜索 `timelength=239` 的候选 `duration=239386`，说明单位不同。此样本提供协议字段证据，不表示所有候选同一版本或已验证自动下载。

MixSongID/hash 的精确详情缺少已核实固定 HTTPS 路线。关键词或文件输入加 `--select kugou:HASH` 时，会只重搜酷狗，用原查询找回精确 hash 的服务端元信息，再核对歌词版本；所选 hash 不在返回页中则明确失败，需缩小关键词。该集成已有离线 transport 回归和上述线上 CLI 分步选歌→选词→保存证据；直接 hash/MixSongID 的无元信息取词仍不支持。

## 酷我音乐

源码接入入口：`src/providers/kuwo.rs`。

- RID：最多 20 位十进制，`kuwo:数字`；合法 `MUSIC_` 前缀只做平台内正规化。
- 搜索：HTTPS `search.kuwo.cn/r.s`，`client=kt` / `all=关键词` / `pn=0` / `rn` / `ft=music` / `rformat=json`。页码从 0 起。
- 线上响应可能是单引号对象而非严格 JSON，需平台专用有边界解析，不把服务端文本当代码执行。
- 当前歌词/详情路线：HTTPS `m.kuwo.cn/newh5/singles/songinfoandlrc?musicId=...`，核对 `data.songinfo.id` 和可选 `musicrId`，业务成功才使用元信息与 `lrclist`；歌名、歌手、专辑、秒级时长用于候选/补查。
- `lrclist` 的 `time` 是秒，转为普通 LRC 行时间；保留长度上限并验证正文，不输出逐字格式。歌词是否含翻译取决于平台返回内容，当前接口未单独请求翻译。
- RID `23928868` 的 H5 虽然 HTTP 200，业务 `status=301`，不能当成功；另一 RID `228908` H5 返回业务成功及原生普通行歌词，证明接口并非所有 ID 一律不可用。
- 历史 `mlyric.kuwo.cn/mobi.s` 即使请求 `lrcx=0` 仍可能返回 `lrcx=1` 的包装/逐字数据；**当前生产实现不走该路线，不承诺解包逐字歌词**。调研中能解包某份响应不代表 CLI 实现了此协议。

当前 H5 已有成功和业务失败两种线上样本，RID `228908` 已完成 CLI HTTPS 取词/临时保存（exit 0，2551 bytes / 63 行），测试文件已清理。对失败 ID 不编造详情、不用未知元信息自动跨源补查。

## LRCLIB

源码：`src/providers/lrclib.rs`。

- ID：平台记录十进制 ID，`lrclib:数字`，不是网易/QQ 歌曲 ID。
- 搜索：HTTPS `lrclib.net/api/search`，可以使用 `q` 或 `track_name` / `artist_name` / `album_name` 等字段；`q` 会覆盖结构化条件，客户端必须自行核对返回元信息。
- 详情/歌词：HTTPS `/api/get/{id}`；返回记录 ID、`trackName`、`artistName`、`albumName`、秒级 `duration`、`instrumental`、`syncedLyrics`、`plainLyrics`。
- 只保存 `syncedLyrics`；只有 `plainLyrics` 不编造时间标签，`instrumental=true` 单独报告。
- 搜索最多 20 条，无分页；到 20 条标记可能不完整，不能推断只有一个匹配。`get` 的记录返回也不是关键词全局唯一匹配证明。
- 客户端按秒比较时长，容差 ±2 秒；请求串行间隔至少 300 ms，限流时不自动重试。

少量 HTTPS `track_name=Yellow` / `artist_name=Coldplay` 搜索返回 20 条，证明上限和截断风险，未据此自动选中首条。

## 支持的完整歌曲链接

链接在本地解析，只有提取出的来源限定 ID 会进入固定 HTTPS 接口。允许输入 `http://` 完整链接只是解析历史分享格式，**不会发送明文 HTTP 请求**。拒绝端口、用户信息、重复 ID 参数、未知域名、未知页面和短链。

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
| [L-1124/QQMusicApi](https://github.com/L-1124/QQMusicApi/tree/ba95861ee9391f5b5f60f89caa8d4de4af160c8b) `ba95861ee9391f5b5f60f89caa8d4de4af160c8b` | `qqmusic_api/modules/lyric.py`；近期 QQ 请求与响应协议对照 | 文件级 GPL 边界，只参考协议事实，不直接移植签名/QRC 算法 |
| [metowolf/Meting](https://github.com/metowolf/Meting/tree/1c2f4c98eed749200d9d7ff5cab329c4308f4268) `1c2f4c98eed749200d9d7ff5cab329c4308f4268` | `src/providers/netease.js`、`src/providers/tencent.js`；网易 EAPI 和 QQ 旧普通 LRC 协议事实 | MIT；固定 commit 与许可证经接入负责人远程核实，独立实现，不直接复用代码 |
| [tranxuanthang/lrclib](https://github.com/tranxuanthang/lrclib/tree/05ad8590f6fc4d47a2d74e70f4915273df20f63c) `05ad8590f6fc4d47a2d74e70f4915273df20f63c` | `server/src/router.rs`，`api/search`、`api/get/:id` 路由与服务端行为 | MIT；阅读对照不表示复制代码 |
| [tranxuanthang/lrclib-homepage](https://github.com/tranxuanthang/lrclib-homepage/tree/f37c07042be1af5fdcc7932d090af32141089751) `f37c07042be1af5fdcc7932d090af32141089751` | 官方 API 文档，参数、匹配与限流建议；服务端代码纠正文档差异 | MIT；文档不能单独证明某首中文歌曲可取 |

历史 `ELDment/Meting-Fixed` 的 `src/Meting.php` 为 MIT 多源协议参考，但计划所列 `4a386c206483c95f2d1c9567328b6ea6ca6c7e22` 在本次远程文件/commit 查询中未找到，**该固定版本未核实、未采用**，不能作为当前实现或线上可用性证据。

网易与 QQ 对照文件已按接入负责人报告逐项列在上表；QQ 数字详情另参考 MIT [Rain120/qq-music-api](https://github.com/Rain120/qq-music-api/tree/d05420bf098bd2769866eba81cfd48a6d0c6f50c) 的固定提交 `d05420bf098bd2769866eba81cfd48a6d0c6f50c`。只提取协议事实，不直接复制实现、不采用 QRC 解密，也不移植 GPL 参考中的算法。

本仓库尚无项目 license，不在此自动选择 MIT/Apache/GPL。若未来直接复用允许使用的代码，应先按文件级许可保留版权、声明和必要 notice；服务协议与歌词内容的使用/再分发权另行判断。
