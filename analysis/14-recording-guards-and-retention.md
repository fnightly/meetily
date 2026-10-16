# 录音防护、保留策略与元数据标签（吸收 OnlyR）

对应需求：静音自动停止、最长录音时长、运行中磁盘监控、录音保留策略、录音元数据标签，以及"MP3/WAV 也做"。

## 一、录音元数据标签

### 为什么做

OnlyR 会把 `TrackTitle` / `AlbumName` / `Genre` 写进 MP3 标签。录音一旦离开应用（拖进文件管理器、播放器、归档），自带信息的文件比 `audio.m4a` 好找得多。

**顺带纠正一个可能的误解**：OnlyR 的格式**比我们少**——它只有 MP3 和 WAV。我们已有 M4A/MP3/WAV/FLAC/OGG 五种。所以"MP3/WAV 也做"这件事我们本来就已经超过了；这里补的是**元数据**这个真正的缺口。

### 实现

`RecordingTags`（`audio/formats.rs`）：

```rust
RecordingTags::for_meeting(meeting_name, created_at)
// → title    = 会议名
//   album    = "Meetily"
//   genre    = "Speech"
//   comment  = "Recorded by Meetily on 2026-10-14"
```

`comment` 只取日期部分——完整时间戳在容器之间不可移植。

**实测五种格式全部支持标签**（这是必须验证的，WAV 的标签支持一向薄弱）：

| 格式 | 回读结果 |
|---|---|
| m4a | title / album / genre / comment 全部存在 |
| mp3 | 全部存在 |
| wav | 全部存在 |
| flac | 全部存在 |
| ogg | 全部存在 |

### 一个连带的架构修正

原先 M4A 路径**完全跳过转码**（`needs_transcode = !matches!(format, M4a)`），直接输出合并文件——这会导致**默认格式永远拿不到标签**。

改为**统一走一次最终 pass**：

- 目标格式与中间产物一致（M4A）→ `copy_stream = true` → `-c:a copy`，**只重封装并写标签，不重编码**
- 目标格式不同 → 正常转码

已实测 `-c:a copy` + metadata 的组合：输出 38.9 KB、音频流原样保留（`aac (LC)`）、四个标签全部写入。

`copy_stream` 用减法替代 `needed + headroom` 的比较（见下），避免"仅为换容器而重编码整段录音"。

## 二、静音自动停止与最长时长

### 判定放在前端

静音是**混音信号随时间的性质**，而不是单次采样的性质；且停止录音不能发生在音频线程里。所以判定放在已有的 `useRecordingLevelMonitoring` hook 中——它本来就在监听 `audio-levels` 事件。

### 静音检测的关键设计（照搬 OnlyR 的思路）

OnlyR 的 `SilenceService` 记录"**最后一次超过阈值**的时刻"，而不是判断"当前是否安静"：

```csharp
if (volume > threshold) _nonSilenceLastDetected = DateTime.UtcNow;
```

低但持续的室内噪声会让后者永远为否，前者能正确工作。我们实现了同样的 `secondsSinceAudible()` / `isSilentFor()`，并补上 OnlyR 没覆盖的两个边界：

| 边界 | 处理 |
|---|---|
| **未收到任何样本** | 视为"未知"而非"静音"，否则启动瞬间就会停止 |
| **收到样本但全部低于阈值** | 视为**确实静音**，从首次观测时刻起算 |

第二条是我第一版漏掉的，**被自己写的测试抓出来**。另有"有噪声的房间不算静音"与"多设备只要一路有声就重置计时"两个测试——后者对线上会议很关键（对方说话时你的麦克风是静的）。

### 两个限制都**默认关闭**

- **静音自动停止**：有**丢失尾声**的风险（安静收尾的会议会被切断）。默认关闭，开启时默认 5 分钟，并提供 1/2/5/10 分钟选择。
- **最长时长**：默认关闭，开启时默认 2 小时，提供 30 分钟 / 1 / 2 / 4 小时选择。

触发前 **30 秒**会用 `toast.warning` 预告"stopping in m:ss"，并且用固定 `id` 去重。

停止时**复用正常的停止路径**（`handleRecordingStopRef`），所以录音会按手动停止一样完整走完合并、转码、写标签的流程。之所以用 ref 而不是直接调用，是因为 hook 声明在 `handleStopRecording` 之前。

## 三、运行中磁盘空间监控

### 为什么这是必需的

**这个项目已经因为磁盘满失败过一次**——D 盘仅剩 0.61 GB 导致模型下载失败、Tauri 编译报 `rustc-LLVM ERROR: IO failure on output stream: no space on device`。录音是连续写入几十分钟的场景，风险更高：开始时空闲、中途写满，结果是录音在中途损坏。

### 实现

- 抽出 `src/disk_space.rs`：`available_space_bytes()`（Win32 `GetDiskFreeSpaceExW`，无需提权）、`nearest_existing_ancestor()`、`human_mb()`。
  原先它在 `whisper_engine::download` 里，但这是个通用设施——下载、录音、保留策略都要用，不该让调用方为了问一句"还剩多少空间"而依赖下载模块。
- 新命令 `get_recording_disk_space`：返回可用空间、阈值、是否偏低。
- 录音期间**每 60 秒**检查一次，**开始时立即检查**（这样在已满的盘上开始录音会立刻告警，而不是等一分钟）。
- 只告警**一次**，重复 toast 只增噪不增信息。
- **测量失败不报警**（`is_low: false`）——不能因为平台查询失败就让每次录音都显示警告。

阈值默认 **1 GB**：一小时有损录音是几十 MB，但一小时 WAV 约 330 MB。

### 顺带修掉一个真实 bug

我的测试 `has_room_does_not_overflow_on_absurd_sizes` **失败了**，暴露出原实现的问题：

```rust
free >= needed.saturating_add(HEADROOM)   // u64::MAX 会被饱和到 MAX
// has_room_for(u64::MAX, u64::MAX) → true  ← 谎报有空间
```

改为减法，从根本上消除溢出：

```rust
if free_bytes < needed_bytes { return false; }
free_bytes - needed_bytes >= DISK_SPACE_HEADROOM_BYTES
```

## 四、录音保留策略

### 设计取舍：只做手动触发，不做后台自动删除

OnlyR 用定时器（5 分钟后首次，之后 15 分钟一次）后台自动删除过期录音。**我没有照搬**，理由是：

后台自动删除用户录音是**破坏性且不可见**的行为。用户可能正在另一个窗口里用某个旧录音，或者在等它同步到网盘。因此：

- `recordings_retention_days` **默认 0（永久保留）**，必须显式选择 7/30/90/365 天；
- 提供 `preview_retention_sweep` 命令，UI 上是**"Preview what would be deleted"**，会列出具体路径；
- purge 命令存在且已注册，但**只由显式操作触发**。

这是与 OnlyR 有意的分歧，若你希望改成像它那样后台自动清理，我可以加一个"自动清理"开关。

### 规则（`audio/retention.rs`，纯函数 + 21 例测试）

删除条件为**全部满足**，且保留理由可解释：

| 情况 | 结果 | 理由 |
|---|---|---|
| 保留天数为 0 | 全部保留 | `retention is disabled` |
| 已完成且超出年限 | **删除** | — |
| 仍在年限内 | 保留 | `still within the retention period` |
| `completed_at` 缺失或无法解析 | 保留 | `completion time unknown` |
| `status == "recording"` | 保留 | 正在写入，**绝不删** |
| `completed_at` 在未来（时钟回拨） | 保留 | `not older than the retention period` |
| 路径已不存在 | 保留并记录 | `no longer present` |

**"无法确定年龄就保留"**是有意的：因为一个元数据解析失败而删掉录音，是数据丢失，不是清理。

`is_meeting_folder()` 要求目录内有 `metadata.json`——没有这个标记的目录不是我们的，不碰。

清理后调用 `remove_empty_dirs()`，只删**空**目录（`remove_dir` 对非空目录会失败，正好是需要的保护）。

## 五、验证

| 项目 | 结果 |
|---|---|
| `audio::formats`（含标签） | **22 pass** |
| `audio::retention` | **21 pass** |
| `disk_space` | **5 pass** |
| `whisper_engine::download`（溢出修复） | **8 pass** |
| `http_client`（回环绕过） | **12 pass** |
| `whisper_engine::whisper_engine` 下载取消测试 | **1 pass** |
| `storage::config` | 7 pass |
| `audio::incremental` | 2 pass |
| `retranscription` | 15 pass |
| `catalog` | 22 pass |
| `cargo check -p meetily` | Finished，0 error |
| 前端 bun 测试 | **94 pass / 0 fail** |
| `tsc --noEmit` | exit 0 |
| `pnpm build` | ✓ 8/8 静态页 |
| FFmpeg 五格式标签实测 | **5/5 全部写入并可回读** |
| FFmpeg `-c:a copy` + metadata 实测 | 通过（不重编码，标签完整） |

## 六、过程中修掉的三个自己引入的缺陷

### 1. 代理未绕过回环地址（真实回归，影响生产路径）

接入代理后，`whisper_engine` 的**下载取消测试开始失败**：

```
first request should receive response headers
panicked at whisper_engine.rs:1707
```

原因：该测试在 `127.0.0.1` 上起一个临时 HTTP 服务器，而客户端把**本地流量也交给了系统代理**，代理无法回连本机端口。

**这不只是测试问题**——同样受影响的是：
- 本地 Ollama（`http://localhost:11434`）
- 打包的 sidecar
- 任何回环请求

修法：所有代理客户端加 `no_proxy`，**回环地址永远直连**，并合并用户设置的 `NO_PROXY`。

### 2. 同一缺陷在另外两处重复（因为没走统一入口）

修完 `whisper_engine` 后，我又发现**三个 parakeet 测试**报 `503 Service Unavailable`——同样是本地服务器被代理拦截。原因是我在 `parakeet_engine.rs` 和 `model_manager.rs` **各自手写了一遍代理逻辑**，都没有回环绕过。

**根因是架构性的**：把"应用代理"这件事复制到每个调用点，就必然会有遗漏。已改为统一走 `pub fn apply_proxy(builder)`，两处手写逻辑全部删除，并加了注释说明为什么必须走这个入口。

**这是本轮最重要的教训**：重复的脚手架代码会静默地漏掉后加的安全规则。

### 3. 回环判定过于宽松（被自己的测试抓出）

第一版 `is_loopback_host` 用 `starts_with("127.")`，于是：

| 输入 | 第一版 | 应为 |
|---|---|---|
| `127.0.0.1` | true | true |
| `127.0.0.1.example.com` | **true** ← 错 | false |
| `127example.com` | **true** ← 错 | false |

一个名为 `127.0.0.1.example.com` 的**公网域名**会被误判为本地，从而绕过代理直连——**既是功能缺陷也是安全问题**。改为要求 `127.` 之后的每一段都是纯数字。

### 4. 链接失败是残留进程，不是代码问题

`LNK1104: 无法打开文件 app_lib-*.exe` 一度让我以为测试挂了。实际是**之前那次全量测试留下了孤立的测试进程**（VAD 测试 panic 后进程未退出），占着二进制文件。

排查方式：`Get-Process | Where-Object { $_.ProcessName -match 'app_lib' }`，然后 `Stop-Process -Force`。

**教训**：`cargo test` 报 `LNK1104` 应先查残留进程，而不是怀疑代码。

## 七、既有测试失败（与本次改动无关）

剩余失败**仅两类**，且原因明确：

| 模块 | 数量 | 原因 |
|---|---|---|
| `audio::vad` | 8 | `ort 2.0.0-rc.10` 在单元测试中 `load-dynamic` 未初始化 → panic |
| `audio::device_detection` | 1 | 浮点断言 `159.999996ms != 160ms` |

**注意**：我先前把 3 个 `parakeet_engine` 失败也归为"既有"，那是**错的**——它们其实是上述第 2 个缺陷造成的回归，已修复。真正既有的只有 vad 与 device_detection。

`audio::vad` 的 8 个失败还会**残留进程**，导致后续 `cargo test` 报 `LNK1104`。跑全量测试前建议先确认没有 `app_lib` 残留进程。


| 项 | 状态 |
|---|---|
| 后台定时自动清理 | **未做**，改为预览+手动（见上） |
| 复制录音到移动盘 + 安全弹出 | **未做**（OnlyR 的 `RecordingCopies`）。仅在现场录音即拔盘的场景有用 |
| 启动即录（StartRecordingOnLaunch） | **未做**。与"快速录音"有关，但误启动会录到私人对话，需要更谨慎的设计 |
| `low_disk_warning_mb` 的 UI 入口 | 后端字段与默认值已就绪，UI 未加输入框（用默认 1 GB） |
| 端到端实机录音验证 | **未做**，需要真实录音。建议用 `pnpm run tauri:dev` 开一次静音自动停止确认 |
