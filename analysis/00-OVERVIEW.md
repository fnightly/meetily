# Meetily 代码分析总报告

> 分析对象：`https://github.com/fnightly/meetily`（fork）与上游 `https://github.com/Zackriya-Solutions/meetily`
> 本地仓库：`D:\Meetly`　HEAD：`a2cb62e8`（`main`，Release v0.4.1）　提交数：661　Tag：13
> 方法：静态阅读 + 多份独立报告交叉验证；所有非显然结论均带 `path:line` 证据，Lead 对关键结论做了抽样复核（见 §8）。
> 子报告：`02-audio.md`、`03-stt-engines.md`、`04-data-api.md`、`05-summary-llm.md`、`06-frontend.md`

---

## 0. 仓库同步状态（先行结论）

`fnightly/meetily` 是上游的**精确镜像**，三个引用指向同一提交：

| 引用 | SHA |
|---|---|
| `HEAD` / `main` | `a2cb62e827da7ef59f65064c97233efb2313878e` |
| `origin/main`（Zackriya-Solutions/meetily） | `a2cb62e827da7ef59f65064c97233efb2313878e` |
| `fork/main`（fnightly/meetily） | `a2cb62e827da7ef59f65064c97233efb2313878e` |

`git rev-list --left-right --count main...origin/main` 与 `main...fork/main` 结果均为 `0 0`；fork 上只有 `main` 一个分支。
**结论：不存在"fork 特有改动"，本报告对 fork 与上游同时成立。**

本地已完成的同步动作：新增 `fork` 远端、抓取全部分支与 tag（13 个）、`--unshallow` 取回完整历史（661 提交）。工作树干净（仅新增 `analysis/`）。

---

## 1. 项目定位与技术栈

Meetily 是**隐私优先的本地 AI 会议助手**：采集麦克风 + 系统音频 → 本地转写 → 本地/云端 LLM 摘要，宣称"数据不离机"、"100% local processing"。

| 层 | 技术 |
|---|---|
| 桌面壳 | Tauri 2.x（Rust） |
| 前端 | Next.js 14 + React 18 + TypeScript |
| 音频 | cpal、CoreAudio process tap、`rubato` 重采样、Sonic VAD |
| 转写 | whisper-rs（whisper.cpp）+ ONNX Runtime（NeMo Parakeet TDT 0.6B） |
| 存储 | SQLite（sqlx 0.8，手写 SQL，10 个迁移文件） |
| 摘要 | 内置 llama-helper sidecar（stdio JSON Lines）、Ollama、Claude、Groq、OpenRouter、OpenAI 兼容端点 |

规模（排除 `node_modules`/`target`/`.git`，536 文件 / 44.9 MB）：

| 语言 | 文件 | 行数 |
|---|---|---|
| Rust（app + llama-helper） | 144 | ~42,600 |
| TypeScript / TSX | 171 | ~27,800 |
| Python（`backend/`，已归档） | 7 | — |

---

## 2. 架构总览

```
┌──────────── Tauri WebView (Next.js) ────────────┐
│ SidebarProvider / ConfigContext / Transcript-   │
│ Context / OnboardingContext                     │
│ 139 个 invoke() 调用点 ──► Tauri IPC ──┐        │
└────────────────────────────────────────┼────────┘
                                         ▼
┌──────────── Rust core (frontend/src-tauri/src) ─────────────┐
│ lib.rs：约 200 个命令注册 + 生命周期/托盘/退出清理          │
│                                                             │
│ audio/  采集(mic+system)→ring buffer→混音→VAD ─┬► 录音(opus) │
│                                                └► 转写 worker│
│ whisper_engine / parakeet_engine：整段批式 ASR               │
│ database/  SQLite（sqlx）                                    │
│ summary/   sidecar | ollama | anthropic | groq | openai | …  │
└─────────────────────────────────────────────────────────────┘
                        │
          llama-helper 子进程（stdio JSON Lines，非端口）
```

**两点关键澄清**（与直觉/文档不同）：

1. `api/` 模块**不是 HTTP 服务**——无监听器、无路由、无 CORS，只有约 20 个 Tauri 命令（`api/api.rs`，注册于 `lib.rs:715-741`）。`frontend/API.md` 与 `backend/API_DOCUMENTATION.md` 均未描述这层真实命令面。
2. 内置摘要路径是 **stdio JSON Lines 子进程协议，不存在端口/端口冲突**；唯一的真实端口是 Ollama 默认 `11434`（硬编码 5 处：`summary/llm_client.rs:317`、`ollama/ollama.rs:163,282,441`、`ollama/metadata.rs:145`）。

---

## 3. 分领域要点

完整内容见各子报告。

### 3.1 音频子系统 → `02-audio.md`
- 模块接线清晰（`audio/mod.rs` 列出 34 个模块），但实时混音器**没有任何 ducking**（`audio/pipeline.rs:184-185` 自认 "Sum without ducking"），真正的 RMS ducking 在死代码 `audio/ffmpeg_mixer.rs:287-308`。
- 环形缓冲注释全面失真：代码是 600 ms 窗口 / 4800 ms 上限（`pipeline.rs:41,48`），注释与 `CLAUDE.md:199` 都写 50 ms。
- **Windows 系统音频采集在结构上不可用**：无任何 loopback API（`audio/devices/platform/windows.rs:15-23,195`）却对输出设备调 `build_input_stream`（`audio/stream.rs:247`）；本应提供 loopback 的 cpal 补丁因 `[patch.crates-io]` 写在成员 manifest（`frontend/src-tauri/Cargo.toml:212-214`）而从未生效——Cargo 只读工作区根 `Cargo.toml`（18 行，无 `[patch]`），`Cargo.lock:1104-1107` 显示实际是 crates.io cpal 0.15.3。
- 音频回调路径存在明确的 UB 与 abort 风险：`static mut SAMPLE_COUNTER` 在 mic/system 两个回调线程上无同步自增（`pipeline.rs:64-71`）；CoreAudio IO proc 内 `ctx.unwrap()` / `Mutex::lock().unwrap()`（`audio/capture/core_audio.rs:167,319`）。
- 前端显示的音频电平是 `sin()` 模拟值（`audio/simple_level_monitor.rs:50`，经 `lib.rs:318` 接线），真实的 `AudioLevelMonitor` 从未接线（`audio/level_monitor.rs:29`）。

### 3.2 语音识别引擎 → `03-stt-engines.md`
- **没有流式转写**：两个引擎都只有整段批式入口；`transcribe_audio` 与 `transcribe_audio_with_confidence` 是约 120 行重复实现（`whisper_engine/whisper_engine.rs:551-666` / `668-839`）。
- 置信度由**文本长度**推导（`whisper_engine.rs:637-643`），Parakeet 缺省填常量 0.85（`audio/transcription/worker.rs:213`）。
- `is_partial` 仅表示"音频短于 15 秒"（`whisper_engine.rs:611-612`），下游无 partial→final 合并，Parakeet 恒 false（`parakeet_provider.rs:40`）。
- 两个引擎都**算出时间戳后丢弃**（`whisper_engine.rs:793-794`；`parakeet_engine/model.rs:465-468`）。
- **同步推理直接在 async 运行时上执行**，无 `spawn_blocking`、无超时；Parakeet 更以**写锁**覆盖整段 ONNX 推理（`parakeet_engine/parakeet_engine.rs:544-558`）。模型*加载*路径反而知道要用 `spawn_blocking`（`:467`），说明是遗漏而非设计。
- 模型完整性弱：运行时下载**无 SHA-256**（Whisper 只查魔数 + 容忍少 10% 大小；Parakeet 只查精确字节数），而项目在构建期已有 SHA-256 机制（`build/onnxruntime.rs:310-341`）。Whisper 下载客户端**完全没有超时**（`whisper_engine.rs:1143-1146`）。
- Parakeet 执行提供者硬编码为 **CPU**（`parakeet_engine/model.rs:97`），macOS 上 CoreML 未接，与文档"Real time on M4 Max"不符。
- `parallel_processor` / `parallel_commands` 是**完整但零消费者的死子系统**（11 个命令注册于 `lib.rs:672-682`），设计上每个 worker 加载一份完整模型（4×large-v3 ≈ 12 GB）且内存预算参数无消费点，与现行 `NUM_WORKERS = 1`（`worker.rs:74`）直接矛盾。

### 3.3 数据与 API 层 → `04-data-api.md`
- 未加密 SQLite（`meeting_minutes.sqlite`），sqlx 0.8 手写 SQL，10 个迁移文件、8 张表。
- **索引近乎缺失**：全库唯一索引还是对主键的冗余索引；`transcripts.meeting_id`、`transcripts.audio_start_time`（分页 ORDER BY）、`meetings.created_at` 全无索引。
- **连接池零配置**：无 `busy_timeout`、无连接数上限、从未显式开启 `foreign_keys`/WAL，而代码注释与 `PRAGMA wal_checkpoint(TRUNCATE)`（`database/manager.rs:194`）假定 WAL 已启用——该 PRAGMA 在默认 journal 模式下是 no-op。并发写（摘要/重转写/导入/落库）下 "database is locked" 是最现实的数据层故障。
- **命令层零鉴权**：`auth_token` 参数仅用于打日志（`api/api.rs:325-330,748-754,936`），`get_auth_token` 是 `#[allow(dead_code)]` 死函数（`api.rs:207`）。
- **所有 API Key 明文存 SQLite 并经 IPC 明文回传**（`database/repositories/setting.rs:105,327-343`；`api.rs:590,700-703,484-493`），且数据库位于 `assetProtocol` scope `$APPDATA/**` 内。
- 两处 **provider 污染 bug**：`save_api_key` 硬编码 `VALUES ('1','openai','gpt-4o-2024-11-20','large-v3',$1)`（`setting.rs:99`）→ 保存 claude/ollama/groq/openrouter 的 key 会把 `provider` 覆写为 `openai`；`save_transcript_api_key` 硬编码 `'parakeet'`（`setting.rs:197-201`）→ 同类覆写。
- **转写非流式入库**：录音中只写浏览器 IndexedDB，停止且转写完成后一次性批量提交；`isCallApi && transcriptionComplete` 不成立则整场不落库（`frontend/src/hooks/useRecordingStop.ts:236`），托盘路径同样依赖前端事件（`tray.rs:94`）。
- **保存非幂等**：`meeting_id`/片段 ID 由后端重新生成，调用方传入的 `segment.id` 被丢弃（`transcript.rs:19,48`），IPC 重试产生重复会议。
- 两处"已建未接"：`meeting_notes` 表（含冗余索引）与 `licensing` 表**无任何 Rust 读写点**；`transcripts.summary/action_items/key_points` 三列从未被使用。
- 生产 panic 面：`lib.rs:586` 数据库初始化失败即 `expect` 崩溃整个应用；`manager.rs:49,125,140`、`tray.rs:27` 的 `expect/unwrap`；`database/commands.rs:188` 竞态下二次 `app.manage(AppState)` 会 panic。
- 遗留死路径：`APP_SERVER_URL = http://localhost:5167`（`api.rs:20`）**在本仓库无任何监听者**（`backend/` 已归档）。

### 3.4 摘要与 LLM 层 → `05-summary-llm.md`
- Provider 抽象是 **enum + 巨型 match，没有 trait**（`summary/llm_client.rs:213-238,301-351`）；新增 provider 需改 5+ 文件，13 参的 `generate_summary` 在 `processor.rs` 重复调用 5 次。
- **并发摘要可能串号（静默数据污染）**：sidecar 协议无请求 ID，"写 stdin"与"抢 stdout 读锁"分两段（`summary/summary_engine/sidecar.rs:352-410`），且 ping（`:413-441`）与 shutdown 残留 `goodbye`（`:487-495`）共用同一响应通道，而 `commands.rs:529-544` 允许并发 → B 的摘要可能写进 A 的记录，无任何报错。
- **每次切换模型泄漏一对 supervisor 循环**：`spawn` 先置 `should_shutdown=true`（`sidecar.rs:279-281`），随后又置回 `false`（`:339`），旧循环永不退出（`:578,626`）。
- **任一请求 900s 超时或用户取消即 `shutdown()` 杀全局 sidecar**（`sidecar.rs:381-388`、`summary/summary_engine/client.rs:206-219`），连带其他会议失败并重载 1–2.6 GB 模型；崩溃后 `is_healthy` 仍为真最长 30s。
- **分块算术缺陷**：`token_threshold - 300` 在 Ollama `num_ctx < 600` 时下溢（`summary/processor.rs:403` + `service.rs:437`，debug panic / release 回绕成单块爆上下文）；`num_ctx ≈ 700–1100` 时每块仅前进个位数字符（`processor.rs:285,313-316`）→ 10 万字符转录可产生数万次 LLM 调用；`rough_token_count`（0.35 tokens/char，`processor.rs:256-259`）对 CJK 低估 2–3 倍，叠加 `llama-helper/src/main.rs:509` 的 `failed to eval`，中文长会议在内置/Ollama 路径上高风险。
- 云侧**零重试、零退避、无 429/Retry-After 处理**（`llm_client.rs:436-485`；分块重试无 sleep `processor.rs:411-466`），且 `token_threshold` 恒为 100000 且云 provider 从不分块（`service.rs:473-476`）。
- `openrouter.rs:42-47` 用 `reqwest::blocking` 且无超时 → **同步 Tauri 命令冻结主线程**。
- 注入与配置面：模板指令未转义进 system prompt（`processor.rs:494-497`）；map/合并 prompt 缺 anti-injection 指令（`processor.rs:218-228` vs `239`）；AI 生成的标题会覆盖用户会议标题（`service.rs:605-615`）；用户 endpoint 未校验即请求（`llm_client.rs:315-330`）。

### 3.5 前端 → `06-frontend.md`
- 4 个 App Router 页面、7 个 Context、24 个 hooks、6 个 service，共 171 文件 / ~27.8k 行。
- **最严重缺陷（P0，本次分析新发现）**：`lib.rs:270-277` 注册的 `get_transcription_status` 是**恒返回零值的桩**，真实现位于 `audio/recording_commands.rs:1106-1112` 却**从未注册**（`lib.rs:616` 注册的是本地桩）。而 `hooks/useRecordingStop.ts:164-181` 正是靠轮询这个命令来"等待转写完成"——桩恒为 `is_processing:false, chunks_in_queue:0`，导致**第一次轮询就判定完成**，只等 4 秒便 flush 落库。**后果：录音停止时不等转写收尾，会议记录静默缺少尾部内容。**修复只需把 `lib.rs:616` 换成 `audio::recording_commands::get_transcription_status`。
- **30 个 Rust emit 事件中 14 个前端零监听者**，含 `recording-error`、`transcription-warning`、`transcription-queue-complete`——错误与完成信号被丢弃。
- 139 个被调用命令中 `builtin_ai_get_models_directory`（`lib/builtin-ai.ts:96`）未注册；但无调用者，属潜在缺陷。
- 弱点：165 处 `onClick` 无键盘等价物（可访问性）；611 处 `console.*` 留在生产；10 个测试文件用 `bun:test` 但**无 `test` 脚本、CI 从不执行**（`package.json:103` 仅有 `@types/bun`）。
- **2 处确定性监听器泄漏**（此处已修正早前"无泄漏"的误判）：`useModalState.ts:168-191` 的 async cleanup 被丢弃且随依赖变化线性增长；`AnalyticsProvider.tsx:126` 的 `beforeunload` 未清理。
- `SidebarProvider.tsx:121-122` 仍硬编码已归档服务的 `localhost:5167` / `127.0.0.1:8178`，且仍在 CSP 白名单内。

---

## 4. 跨领域系统性主题

单点缺陷之上，有 7 个贯穿全局的模式。它们比任何单个 bug 更能解释这个代码库当前的状态。

### 4.1 文档宣称与实现之间系统性偏离

`CLAUDE.md`、`README.md` 和源码注释描述的是一套**并不存在**的实现：

| 文档/注释说法 | 代码实际 |
|---|---|
| "professional audio mixing（RMS-based ducking）"（`CLAUDE.md:199` 区块） | 实时路径无任何 ducking，`pipeline.rs:184-185` 自认 "Sum without ducking" |
| 环形缓冲 50 ms 窗口 / 400 ms 上限（`pipeline.rs:40,44-47`） | 600 ms / 4800 ms（`pipeline.rs:41,48`） |
| macOS 用 ScreenCaptureKit 采集系统音频（`CLAUDE.md:117,329`） | `stream.rs:84-102` 的 ScreenCaptureKit 分支是空壳，实际用 CoreAudio process tap（`core_audio.rs:91`） |
| Windows 用 WASAPI loopback | 全仓无 loopback API，输出设备被交给 `build_input_stream` |
| "4x faster Parakeet"、M4 Max 实时 | Parakeet 只有 CPU execution provider（`model.rs:97`） |
| "pre-scaled 70%"（`pipeline.rs:178-181`） | 代码是 `sys * 1.0` |

这类偏离的危险在于：**维护者和贡献者会依据文档做出错误判断**（例如照着 `CLAUDE.md` 去调 ducking 参数、或以为 Windows 系统音频可用只需修 bug）。

### 4.2 "已建成但未接线"是主要失败模式

最严重的功能缺口不是"没写"，而是"写了但没接上"：

| 已建成 | 状态 |
|---|---|
| `parallel_processor` + 11 个 Tauri 命令 | 前端零调用，死子系统 |
| `meeting_notes` 表 + 索引 | 无 repo、无命令、无结构体 |
| `licensing` 表 | 无任何 Rust 读写点 |
| `transcripts.summary/action_items/key_points` 列 | 从未使用 |
| 真实 `AudioLevelMonitor` | 未接线；前端接的是 `sin()` 假数据 |
| `ffmpeg_mixer` 的 RMS ducking / 限幅 | 死代码（`RNNOISE_APPLY_ENABLED = false`） |
| `AudioMetricsBatcher` | 结果无人读取且无界增长 |
| `device_monitor` 自适应轮询 | 未生效，全程 2 s 全量枚举 |
| `ModelStatus::Error` | 永不构造 |
| `get_auth_token`、profile 三命令、`test_backend_connection` | 死代码/无调用点 |
| `audio_v2/**`（1220 行） | 未注册，且因 API 过时**已无法编译** |

### 4.3 约 7,100 行从不编译的 Rust 代码

已精确统计：`lib_old_complex.rs`(2314) + `recording_commands.rs.backup`(1822) + `core-old.rs`(865) + `stt.rs`(362) + `recording_saver_old.rs`(349) + `system_audio_stream.rs`(194) + `audio_v2/**`(1220) = **7,126 行**，约占 Rust 代码的 **17%**。

它们全部被 git 跟踪（含一个 4.4 MB 的 `frontend/vs_buildtools.exe`），却因未被 `mod` 声明而从不参与编译——编译器、clippy、CI 都看不到它们，于是**永远不会腐烂报警，只会持续误导读者**。仓库里甚至有一份 `frontend/src-tauri/CLEANUP_PLAN.md` 列出了其中一部分，但没有执行。

### 4.4 复制-分叉式重复实现及其漂移

多处"复制一份再各自改"：

- `transcribe_audio` vs `transcribe_audio_with_confidence`（约 120 行重复）。
- `whisper_engine/commands.rs` vs `parakeet_engine/commands.rs` 近乎逐行重复（含同名静态量），且行为已漂移：Whisper 缺 Parakeet 的下载前置 `discover_models`，可能"文件已下载但状态仍 Missing"。
- `openai`/`groq`/`anthropic` 三份 `CacheEntry` + `clear_cache` 逐字复制，且 `clear_cache` 未注册为命令 → 改 key 后最长 5 分钟不刷新。
- `simple_level_monitor`（假的）与 `level_monitor`（真的）两套并存。

重复本身是成本，**漂移**才是缺陷来源。

### 4.5 并发与共享状态：缺少请求标识和锁纪律

- **协议层**：sidecar 响应流无请求 ID，generate/ping/shutdown 三个方向共享同一通道（`sidecar.rs:352-410,413-441,487-495`）。
- **跨 FFI 层**：`static mut` 双线程自增（`pipeline.rs:64-71`）；音频回调内 `unwrap()` panic 会跨 `extern "C"` 变成 abort。
- **锁粒度**：Parakeet 写锁覆盖整段推理；checkpoint 编码持 `AsyncMutex` 同步等 ffmpeg，而 `stop_and_save` 要等同一把锁。
- **资源生命周期**：supervisor 循环泄漏；"任一请求超时即杀全局进程"是反向的隔离失败——不是隔离不足，而是共享过度。

### 4.6 隐私叙事与实现之间的账

项目以"privacy-first / 100% local"作为核心卖点，实际存在三处需要用户知情才能接受的实现：

1. **匿名遥测默认开启**：`init_analytics` 硬编码 PostHog key 与 `enabled: true`（`analytics/commands.rs:12-16`），上报到 `https://us.i.posthog.com`；隐私层面做得不错的是有敏感字段白名单（`analytics/analytics.rs:9-28`），且可在 UI 关闭。但"100% local, no data leaves your machine"与"默认上报遥测"需要同时说明。
2. **密钥明文**：所有第三方 API Key 明文存 SQLite 并明文经 IPC 回传。
3. **WebView 权限过大**：capability 授予 `fs:read-all`、`fs:write-all`、`fs:read-all` 级别的文件系统访问（`tauri.conf.json:44-52`），覆盖 `$APPDATA` 的 asset scope。命令层又零鉴权，因此 WebView 一旦被注入，代价是"整个用户可读文件系统 + 全部明文密钥"，而不是仅应用数据。

**需要精确表述的是**：`open_external_url` 在 Windows 上走 `cmd /C start <url>`（`api.rs:1148-1165`）确实具备命令注入形态，但**当前 4 个调用点全部传硬编码常量 URL**，因此是**加固缺口而非在用的漏洞**；同理 `fs:read-all` 在"WebView 内容可信"的前提下不是活跃漏洞。把它们说成"已可利用"会夸大结论。

### 4.7 无质量门禁的 CI

`.github/workflows/` 有 10 个文件，构建矩阵覆盖三平台（`build.yml` 经 `tauri-action` 真实编译 Rust + 前端），但：

- `pr-main-check.yml`（唯一的 PR 门禁）**只校验 `tauri.conf.json` 里的版本号格式**；
- 全仓**没有任何 `cargo test` / `cargo clippy` / `cargo fmt` / `pnpm test` / `pnpm lint` 步骤**（已对全部 workflow 文本 grep 确认）。

也就是说：仓库里有 46 个 Rust `#[test]` 和 10 个前端测试文件，**在 CI 上永远不会执行**。前端测试的运行器其实是 `bun:test`（`package.json:103` 有 `@types/bun`），**装有 Bun 即可 `bun test tests/` 本地运行**；但 `package.json` 没有 `test` 脚本，且全部 workflow 中按词边界检索 `bun` 为 **0 命中**（此前 grep 到的 138 处是 `ubuntu-latest` 的子串噪声）。第 4.1–4.3 节的所有漂移、死代码与重复，都是这个门禁缺口的直接后果。

---

## 5. 风险排序

跨 5 个子报告统一排序。**"可利用性"一列区分真实漏洞与加固缺口**，避免把理论风险当成已发生的事实。

### P0 — 会静默损坏数据或让应用不可恢复

| # | 风险 | 证据 | 用户可见后果 |
|---|---|---|---|
| 1 | **停止录音不等转写收尾 → 会议记录静默缺尾部**：注册的是恒零值的 `get_transcription_status` 桩，轮询第一次即判"完成"，仅等 4 秒落库 | `lib.rs:270-277,616`；`audio/recording_commands.rs:1106-1112`；`useRecordingStop.ts:164-181,236` | 录音看似成功，会议记录缺少最后一段内容，**无任何提示** |
| 2 | **并发摘要串号**：sidecar 无请求 ID + 分阶段抢 stdout 锁 | `sidecar.rs:352-410`；`commands.rs:529-544` | B 会议的摘要被写进 A 的会议记录，**无任何报错** |
| 3 | **整场会议可能完全不落库**：落库依赖前端 `isCallApi && transcriptionComplete`，托盘路径亦然 | `useRecordingStop.ts:236`；`tray.rs:94` | 录音看似成功，结束后会议凭空消失 |
| 4 | **录音停止失败 → 应用永久拒绝新录音**：`Err` 早退分支不清 `IS_RECORDING` 且 manager 已被 `take()` | `recording_commands.rs:731-734` vs `:1051`、`:319-323` | 只能重启应用；当前被上游错误吞噬掩盖（`recording_manager.rs:349-357`） |
| 5 | **数据库初始化失败即崩溃**：`expect` 无降级 | `lib.rs:586` | 首次启动/迁移失败 = 应用打不开 |

### P1 — 结构性缺陷，影响核心承诺

| # | 风险 | 证据 |
|---|---|---|
| 5 | **Windows 系统音频采集结构上不可用**，而文档称已支持；本应使能的 cpal 补丁因放错 manifest 位置从未生效 | `windows.rs:15-23,195`；`stream.rs:247`；`frontend/src-tauri/Cargo.toml:212-214` + 根 `Cargo.toml:1-18` + `Cargo.lock:1104-1107` |
| 6 | **同步 ASR 推理阻塞 async 运行时**，Parakeet 以写锁覆盖整段推理；live 路径 VAD 段长无上界 | `whisper_engine.rs:620,775`；`parakeet_engine.rs:544-558`；`audio/pipeline.rs:19-27` |
| 7 | **中文/长转录的分块退化**：token 估算对 CJK 低估 2–3 倍 + `num_ctx<600` 时 `-300` 下溢 + 每块仅前进个位数字符 | `processor.rs:256-259,285,313-316,403`；`service.rs:437`；`main.rs:509` |
| 8 | **sidecar 生命周期缺陷**：每次换模型泄漏一对 supervisor 循环；任一超时/取消即杀全局进程 | `sidecar.rs:279-281,339,381-388,578,626` |
| 9 | **模型下载无密码学校验**（Whisper 容忍少 10%；两引擎都无 SHA-256），且 Whisper 下载无超时 | `whisper_engine.rs:220-248,972-994,1143-1146`；`parakeet_engine.rs:413-429` |
| 10 | **音频回调路径 UB/abort**：`static mut` 双线程自增；CoreAudio IO proc 内 `unwrap()` | `pipeline.rs:64-71`；`core_audio.rs:167,319` |
| 11 | **前端电平表是 `sin()` 假数据**，用户据此判断麦克风是否工作 | `simple_level_monitor.rs:50`，经 `lib.rs:318` 接线 |
| 12 | **系统音频与麦克风累积失步**：落后声道 `drain+零填充` 把样本压缩进更早时间窗 | `pipeline.rs:137-151`，叠加 `:92-97` |

### P2 — 安全加固、健壮性与可维护性

| # | 风险 | 证据 | 可利用性 |
|---|---|---|---|
| 13 | API Key 明文存盘 + 明文 IPC 回传 | `setting.rs:105,327-343`；`api.rs:590,700-703` | 本机任意进程可读；配合下一条放大 |
| 14 | 命令层零鉴权 + WebView 授予 `fs:read-all`/`fs:write-all` | `api.rs:325-330`；`tauri.conf.json:44-52` | 需先注入 WebView；当前无已知注入路径 |
| 15 | `open_external_url` 走 `cmd /C start <url>` 无白名单 | `api.rs:1148-1165` | **仅加固缺口**：现有 4 个调用点全是硬编码常量 |
| 16 | 匿名遥测默认开启（硬编码 PostHog key） | `analytics/commands.rs:12-16` | 设计选择，但有隐私叙事张力 |
| 17 | 连接池零配置（无 WAL/busy_timeout/上限），而代码假定 WAL 已启用 | `manager.rs:33,194` | 并发写下 "database is locked" |
| 18 | 索引近乎全缺（`transcripts.meeting_id`、`audio_start_time`、`meetings.created_at`） | `repositories/meeting.rs:145,156` | 会议多时前端全量拉取变慢（`SidebarProvider.tsx:100` 无分页） |
| 19 | 云 provider 无重试/退避/429 处理；`openrouter.rs` 阻塞主线程 | `llm_client.rs:436-485`；`openrouter.rs:42-47` | 限流即失败；UI 冻结 |
| 20 | 保存非幂等：IPC 重试产生重复会议 | `transcript.rs:19,48` | 数据重复 |
| 21 | 两处 provider 覆写 bug | `setting.rs:99,197-201` | 保存一个 provider 的 key 会污染另一个的读取 |
| 22 | CI 无测试/lint 门禁；前端有 `bun:test` 用例但无 `test` 脚本、CI 零调用 | `pr-main-check.yml`；`package.json` 无 `test` 脚本；workflow 中 `bun` 词边界 0 命中 | —— |
| 23 | 7,126 行从不编译的遗留代码 + 4.4 MB 二进制入库 | 见 §4.3 | 误导维护者，永不被编译器发现 |
| 24 | 内存无上界：全量解码 + 全量克隆，配合 20 GB 导入上限 | `decoder.rs:479,546,63`；`import.rs:62` | 一次校验即可耗尽内存 |

---

## 6. 工程健康度

| 维度 | 评价 | 依据 |
|---|---|---|
| 架构分层 | **良好** | 音频/转写/存储/摘要分模块清晰，`audio/mod.rs` 显式列出 34 个模块；Tauri 命令/事件边界统一 |
| 异步/并发正确性 | **弱** | 同步推理阻塞运行时、写锁覆盖整段推理、`static mut` UB、supervisor 循环泄漏、协议无请求 ID |
| 错误处理 | **弱** | 442 处 `.unwrap()`、148 处 `.expect()`；关键路径 `lib.rs:586` 直接崩溃；多处错误被吞（`recording_manager.rs:349-357`） |
| 安全 | **中偏弱** | 密钥明文、命令零鉴权、WebView 权限过大；正面是有遥测敏感字段白名单、updater 有签名公钥 |
| 测试 | **很差** | 46 个 Rust test + 10 个 `bun:test` 用例**全部不在 CI 中运行**；无 `test` 脚本 |
| CI/质量门禁 | **很差** | PR 门禁仅校验版本号格式；无 clippy/fmt/test/lint |
| 文档准确性 | **差** | `CLAUDE.md`/README/注释与实现系统性偏离（§4.1） |
| 依赖管理 | **中** | workspace 结构规范，但有 `branch = "main"` 跟随浮动分支（`Cargo.toml:146`）；`[patch]` 放错位置导致静默失效 |
| 死代码控制 | **差** | 7,126 行从不编译；多张表/多个模块"已建未接" |

**综合**：这是一个**架构意图良好、工程纪律薄弱**的代码库。分层是清晰的、模块划分是合理的、部分子系统（如 Parakeet 的下载续传与 revision 重试、SQLite 摘要的 compare-and-set）有真正的工程水准；但缺少质量门禁使漂移、死代码与重复不断累积，而并发与错误处理的纪律不足，导致缺陷以"静默损坏数据"而非"崩溃"的形式出现——后者更难发现。

---

## 7. 结论

1. **项目定位清晰、架构分层合理**，核心链路（采集→VAD→转写→存储→摘要）在代码上是完整闭合的，不是玩具工程。
2. **最大风险不是"缺功能"，而是"已建成但未接线"和"文档与实现脱节"**。§4.2 列出的 11 项"已建未接"和 §4.1 的 6 处文档偏离，共同解释了为什么项目看起来比实际完备。
3. **最该优先修的是 5 个 P0**：转写状态桩导致会议记录静默缺尾部（一行注册改动即可修）、并发摘要串号（静默写错记录）、落库依赖前端事件（整场丢失）、`stop_recording` 错误路径（应用永久不可用）、DB 初始化崩溃。它们共同特征是"用户数据静默丢失或应用不可恢复"，而非性能问题。
4. **最高杠杆的工程投资是给 CI 加门禁**。`pr-main-check.yml` 目前只校验版本号；加上 `cargo clippy -- -D warnings`、`cargo test`、`pnpm lint` 就能立刻暴露 442 处 `unwrap`、7,126 行死代码中的失效项，以及两套引擎的行为漂移。
5. **对"隐私优先"的评估应分项而非笼统**：本地转写/本地摘要的承诺在代码层面是成立的（无上传音频的路径）；但"默认开启的匿名遥测"、"明文密钥"、"过大的 WebView 文件权限"三项需要用户在知情前提下接受。
6. **fork 与上游无差异**，本报告结论对两者同时适用；若要分析 fork 特有改动，需 fork 作者先推入自己的提交。

---

## 8. 分析可信度说明

**Lead 亲自抽样复核并确认的结论**（非仅依赖子报告）：

| 结论 | 复核方式 |
|---|---|
| fork 与上游完全同步 | `git rev-parse` 三方比对 + `rev-list --left-right --count` 均为 `0 0` |
| **转写状态桩导致 P0 数据丢失** | 读 `lib.rs:270-277`（恒零桩）与 `:616`（注册的正是该桩）；读 `recording_commands.rs:1106-1112`（真实现含 `IS_RECORDING`，从未注册）；读 `useRecordingStop.ts:164-181`（第一次轮询即 break）与 `:236`（落库条件） |
| **14 个事件无监听者** | 从 Rust 全量提取 emit 事件名（30 个）与前端 `listen<T>('…')` 事件名（22 个）交叉比对，再对 `frontend/src` 逐个做字符串存在性确认 |
| `save_api_key` 覆写 provider/model | 直接读 `setting.rs:88-108`，确认 `VALUES ('1','openai','gpt-4o-2024-11-20','large-v3',$1)` |
| sidecar supervisor 循环泄漏 | 直接读 `sidecar.rs:270-349`，确认 `shutdown()` 后 `should_shutdown` 又被置回 `false` |
| cpal `[patch]` 未生效 | 直接读成员 manifest `:212-214`、根 `Cargo.toml`（无 `[patch]`）、`Cargo.lock` 实为 crates.io 0.15.3 |
| Windows 无 loopback | grep 全仓 loopback + 读 `windows.rs:15-23,195` + `stream.rs:247` |
| 电平表是假数据 | 直接读 `simple_level_monitor.rs:41-50`，注释写着 "create fake level data to test the UI" |
| CI 无测试门禁 | grep 全部 workflow 文本，确认零 `cargo test`/`clippy`/`pnpm test`/`lint`；按词边界检索 `bun` 为 0 命中（138 处为 `ubuntu` 子串噪声） |
| 7,126 行从不编译 | 逐文件统计行数 + 确认无 `mod` 声明、无任何引用 |
| `open_external_url` 仅传常量 | grep 出 4 个调用点，全部为硬编码 URL |
| **监听器泄漏（已修正）** | 首轮据 47 处 `listen`/196 处 unlisten 的比值误判为"无泄漏"；第二轮读码确认 `useModalState.ts:168-191` 与 `AnalyticsProvider.tsx:126` 各有一处真实泄漏 |
| `api_get_auto_generate_setting` 契约失配 | 读 `lib.rs:723-724` 确认两行均被 `//` 注释；读 `ModelSettingsModal.tsx:320` 确认在调用 |

**未做运行时验证的部分**（均为静态阅读结论）：未编译本项目（需 Rust 工具链 + 平台构建依赖），未运行任何测试或基准，未实机验证 Windows/macOS 音频行为。因此：

- "Windows 系统音频不可用"是基于**代码中不存在 loopback API** 的强静态推断；运行时表现还取决于所用 cpal 补丁是否真含 loopback 支持。子报告已给出该推断的完整证据链。
- 所有性能类结论（如"数万次 LLM 调用"）是**基于代码算术的推演**，非实测。
- 安全类结论已按 §5 的"可利用性"列区分严重程度，未将加固缺口表述为在用漏洞。
