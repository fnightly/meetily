# Phase 1 实现记录

> 对应总报告 `00-OVERVIEW.md`。本文件记录本轮实际落地的代码改动、验证方式与遗留项。
> 时间基准：仓库 HEAD `a2cb62e8`（未提交任何 git commit，改动均在工作树）。

---

## 1. 已落地功能

### ① 录音不再实时转写、录完再转写

| 位置 | 改动 |
|---|---|
| `audio/recording_preferences.rs` | 新增 `defer_transcription: bool` 偏好（`#[serde(default)]`，默认 `false`，旧配置自动兼容） |
| `audio/recording_state.rs` | 新增 `defer_transcription: AtomicBool` + `set_defer_transcription` / `is_transcription_deferred` |
| `audio/recording_manager.rs` | `start_recording(...)` 新增 `defer_transcription` 参数，在建管道前发布到 state |
| `audio/recording_commands.rs` | 两条启动路径都读取偏好并透传 |
| `audio/transcription/worker.rs` | `start_transcription_task(...)` 新增 `defer_transcription`；为真时**只 drain 音频段、不做任何 ASR**，并发 `transcription-deferred` 与 `transcription-queue-complete` 事件 |
| `components/RecordingSettings.tsx` | 新增开关「Record only, transcribe afterwards」；`auto_save` 关闭时给出提示 |

设计选择：**不删管道、不做空转**——沿用现有 retranscription 路径对保存后的音频转写。延迟模式下 `chunks_in_queue` 恒为 0，停止流程按既有逻辑直接落库，不引入新的等待。

### ② 录音保存目录可修改

`select_recording_folder` 原先是个**桩**（`Ok(None)`，注释写 "not yet implemented"），现已实现为真实目录选择器并立即持久化到 `recording_preferences.save_folder`；`RecordingSettings` 增加「Change Folder…」按钮。

### ③ Data Storage Locations 可配置（数据库 / 模型 / 笔记 / 录音）

新增模块 `frontend/src-tauri/src/storage/`：

- `config.rs`：`StorageConfig`（四项均可空 = 用默认）、`ResolvedStoragePaths`、`load/save_storage_config`、`resolve_models_dir`。
- `commands.rs`：`get_storage_config`、`set_storage_config`、`pick_storage_directory`、`storage_restart_required`、`open_storage_folder`、`reset_storage_location`、`storage_default_subdirs`。
- 接入点：`database/manager.rs` 的数据库路径改为读 storage 配置（默认仍是 `app_data_dir()/meeting_minutes.sqlite`）；whisper / parakeet 的 `set_models_directory` 改为走 storage 配置；`Cargo.toml` 注册 7 个命令。
- 数据库迁移：改用 `VACUUM INTO`（一致快照，WAL 已折入）写入新位置，失败回退 `wal_checkpoint(TRUNCATE)` + 文件复制（含 `-wal`/`-shm`）；**目标已存在同名库时不覆盖**；复制失败则不落盘新配置，避免指向空库。
- 因连接池在启动时打开，数据库换址**需重启生效**，后端返回 `restart_required`，UI 显式提示。
- UI：新增 `components/StorageLocationsPanel.tsx`（四行，每行可 Change/Open/Reset，带 Default/Custom 标记与重启警示），替换 `PreferenceSettings.tsx` 中原本**整段被注释掉**的只读区块。

### ④ 导出 MD

新增 `lib/meeting-export.ts`：摘要支持三种存储形态（`markdown` 字符串 / BlockNote JSON / 旧版 section-block），转写按 `audio_start_time` 排序并以 `mm:ss` 标注，支持 `summary` / `transcript` / `both` 三种范围；缺失部分显式写明而非留空标题。

接入 `TranscriptButtonGroup` → `TranscriptPanel` → `page-content.tsx` 的 Export 按钮。

**关键正确性决定**：导出时用 `api_get_meeting` **重新读取完整转写**，而不是用 `meetingData.transcripts`——后者是分页加载的，直接导出会把长会议静默截断成用户已滚动到的部分。

### ⑤ 摘要模板可创建 / 编辑 / 定制

原命令面只有 list / get / validate，**没有任何写入能力**，而 `loader.rs` 里 `get_custom_templates_dir()` 用 `dirs::data_dir()/Meetily/templates`，与 Tauri 的 `app_data_dir()`（`%APPDATA%\com.meetily.ai`，identifier 为 `com.meetily.ai`）**不是同一个目录**——因此自定义模板目录即使存在也不会被应用发现，模板"定制"实际上无从生效。

改动：

| 位置 | 改动 |
|---|---|
| `templates/loader.rs` | 新增 `set_app_data_dir`；`get_custom_templates_dir()` 优先用 app data dir，并保留旧 `Meetily/templates` 作为**只读回退**；`custom_template_dirs()` 汇总两处 |
| 同上 | 新增 `validate_template_id`（拒绝空/空白/`/`/`\`/`.`，防目录逃逸）、`slugify_template_id`、`unique_template_id`、`save_custom_template`、`delete_custom_template`、`read_custom_template`、`is_custom_template`、`custom_templates_write_dir`、`list_templates_detailed` |
| `templates/mod.rs` | 导出上述 API |
| `lib.rs` | setup 阶段先 `set_app_data_dir(app_data_dir())` 再设 bundled 目录 |
| `summary/template_commands.rs` | 新增 `api_save_template`、`api_delete_template`、`api_duplicate_template`、`api_get_template_full`、`api_get_custom_templates_dir`；`TemplateInfo`/`TemplateDetails` 增加 `is_custom` |
| `lib.rs` | 注册 5 个新命令（原有 3 个保留，签名兼容） |
| `hooks/meeting-details/useTemplates.ts` | 增加 `loadTemplate`/`saveTemplate`/`duplicateTemplate`/`deleteTemplate`/`getCustomTemplatesDir`/`refreshTemplates` 与类型 |
| `components/MeetingDetails/TemplateManagerModal.tsx` | 新增：列表（Custom/Built-in 标记）、新建、编辑（名称/描述/分节增删与上下移/标题/指令/格式/示例格式）、复制、删除（二次确认） |
| `SummaryGeneratorButtonGroup.tsx` / `SummaryPanel.tsx` / `page-content.tsx` | 模板下拉新增「Manage templates…」，打通 `templateActions` |

**保护规则**（后端强制，前端只做提示）：内置模板**不可覆盖也不可删除**，只能"复制后修改"；覆盖已有自定义模板需显式 `overwrite`。模板 id 经 `validate_template_id` 校验，避免 `../` 之类逃逸模板目录。

---

## 2. 顺带修复的两个真实缺陷

### 2.1 P0：停止录音不等转写收尾 → 会议记录静默缺尾部

根因比初版报告更精确：

1. `lib.rs:270-277` 注册的 `get_transcription_status` 是**恒返回零值的桩**；真实现虽在 `audio/recording_commands.rs:1122`，但**既没有 `#[tauri::command]` 也从未注册**。
2. 即使换用真实现也**不能修好**——它的 `chunks_in_queue` 同样硬编码为 0，`is_processing` 读 `IS_RECORDING`（停止时已置 false）。

修法（三处，缺一不可）：

- `recording_commands.rs`：给真实现补 `#[tauri::command]`，并接入真实队列计数：`is_processing = recording || pending > 0`。
- `worker.rs`：新增模块级 `CHUNKS_PENDING` / `LAST_ACTIVITY_MS` 与 `note_chunk_started` / `note_chunk_finished`（饱和递减，防回绕）/ `reset_queue_stats` / `pending_chunk_count` / `millis_since_last_activity`，在派发处 +1、在**每一条**消费完成路径（含两个 `continue` 早退）−1。
- `lib.rs`：删除桩与随之无用的本地 `TranscriptionStatus` 结构，注册真实现。

### 2.2 `[patch.crates-io]` 放错文件导致 cpal 补丁从未生效

`[patch]` 只在工作区根生效，原先写在成员 manifest `frontend/src-tauri/Cargo.toml`。已将 cpal 补丁移到根 `Cargo.toml` 并注明原因；成员处保留说明注释。

> 注：同处的 `esaxx-rs` 补丁**未迁移**——其 git 地址在本机网络下不可达，迁移会导致构建直接失败。仅保留注释记录。

### 2.3 测试入口缺失

`package.json` 无 `test` 脚本（虽有 10 个 `bun:test` 用例）。已加 `test` / `test:watch` / `typecheck` 三个脚本。

---

## 3. 验证记录（可复现）

| 项目 | 命令 | 结果 |
|---|---|---|
| 前端类型 | `node node_modules/typescript/bin/tsc --noEmit` | **exit 0**（含新增 5 个文件） |
| 前端测试 | `bun test tests` | **65 pass / 0 fail**（原有 45 + 新增 20） |
| Rust 语法 | `rustfmt --edition 2021 --emit stdout <12 个改动文件>` | **0 个解析错误** |
| Rust 类型（storage + templates） | 独立 harness `cargo check` | **exit 0**（真实 tauri / sqlx / tauri-plugin-store API） |
| Rust 单测（templates） | harness `cargo test --lib` | **16 pass / 0 fail**（含我新增的 5 个用例） |

### 3.1 独立验证 harness（重要方法论）

主 crate 因 bindgen/LLVM 问题无法编译，但**我新增的 Rust 代码并非只能靠肉眼核验**。我在
`%TEMP%\meetily-verify` 建了一个验证用 crate，把 `storage/**` 与 `summary/templates/**`
**原样复制进去**，只对 `crate::state::AppState`、`crate::audio::recording_preferences`
两个应用内类型做最小 stub，其余依赖真实 crate（`tauri` 2.12、`sqlx` 0.8、`tauri-plugin-store`
2.5、`tauri-plugin-dialog` 2.8），然后 `cargo check` / `cargo test --lib`。

**这个 harness 立刻抓到一个真实编译错误**，证明它不只是形式：

`storage/config.rs` 调用 `app.store(...)` 但**没有 `use tauri_plugin_store::StoreExt;`**。
`store()` 是 `StoreExt` 提供的 trait 方法，缺少该 import 时编译报
`no method named 'store' found for reference &AppHandle<R>`（提示 "trait `StoreExt` ... not in scope"）。
这正是 `recording_preferences.rs` 之所以要 `use tauri_plugin_store::StoreExt;` 的原因。已修复。

顺带修掉一个 `unused variable: app` 警告（`recordings_folder` 的 `app` 参数当前未使用，
加 `_` 前缀并注明保留原因）。

**仍未覆盖**：`audio/recording_commands.rs`、`audio/transcription/worker.rs`、
`audio/recording_manager.rs`、`audio/recording_state.rs`、`audio/recording_preferences.rs`、
`database/manager.rs`、`lib.rs`、`summary/template_commands.rs`——这些要么依赖 whisper-rs，
要么依赖完整 AppState/音频栈，无法在 harness 中隔离。**其中 `worker.rs` 的队列计数与
`template_commands.rs` 的命令签名是剩余风险最高的两处**，仍需完整 `cargo check -p meetily`。


新增测试 `tests/lib/meeting-export.test.ts`（20 例）覆盖：时间戳格式化（含时/分/无效值）、三种摘要形态（含嵌套子块缩进、任务勾选、行内链接、`_section_order` 顺序）、转写排序（偏移/时钟回退/`sequence_id` 打平局/空段）、三种导出范围、缺失内容的显式措辞、文件名非法字符与后缀。

修复过程中由 tsc 捕获到的两个真实问题（值得记录，因为它们证明类型检查有效）：
1. 我给测试夹具写 `content: 'string'`，而 `BlockNoteBlock.content` 声明为 `any[]`。核对后确认**是生产类型过窄**（本应用与旧摘要格式持久化的就是字符串），故放宽 `content?: any[] | string` 而非改测试。
2. 测试夹具最初缺 `BlockNoteBlock.id`（该字段确实必需）——修正夹具。

---

## 4. 未完成项

### 4.1 Rust 端编译验证：**已解决**

**结果**：`cargo check -p meetily` **通过**（仅剩 7 个既有 warning，无 error）。

根因不是缺 LLVM，而是**依赖版本过期**：`whisper-rs-sys 0.11.1` 钉住 `bindgen 0.69`（2023 年），该版本无法用 LLVM 22 解析 `whisper_full_params` 的嵌套匿名 struct，于是把结构生成为只有 `_address` 的不透明类型，导致 `whisper-rs` 自身编译失败——**与本次改动无关，但阻塞了整个 crate 的类型检查**。

**解法**：升级到官方为 clang 22 兼容而发布的版本（[voxtype PR #269 "Bump whisper-rs to 0.16.0 for clang 22 compatibility"](https://github.com/peteonrails/voxtype/pull/269) 佐证了这一动机）：

```
whisper-rs 0.13.2 → 0.16.0     （whisper-rs-sys 0.11.1 → 0.15.0）
```

该升级同时**可行且必要**：三个 `whisper-rs` 声明的 feature（`raw-api` / `metal` / `coreml`）在 0.16.0 中全部存在。

**连带完成的 API 迁移**（0.16 的破坏性变更，共 5 类）：

| 0.13 API | 0.16 API |
|---|---|
| `set_suppress_non_speech_tokens(b)` | `set_suppress_nst(b)` |
| `WhisperContext::new_with_params(&Cow<str>, ..)` | 需 `AsRef<Path>`，直接传 `&PathBuf` |
| `state.full(params, data)?` 返回 `Result` | 仍返回 `Result`；但 `full_n_segments()` 从 `Result<c_int>` 变为**裸 `c_int`**，原 `?` 失效 |
| `state.full_get_segment_text_lossy(i) -> Result<String>` | `state.get_segment(i as i32) -> Option<WhisperSegment>`，再 `.to_str_lossy()` 得 `Cow<str>` |
| `state.full_get_segment_t0/t1(i)` | `segment.start_timestamp()` / `end_timestamp()`（`i64`） |

迁移过程中由编译器捕获到 2 处我自己的机械替换错误（`Option` 上误用 `Ok/Err` 模式、以及重复 `.unwrap_or(0)`），已修正。

**踩到的第二个环境坑**：`cargo check` 还会失败于 app 自身 build script——

```
resource path `binaries\llama-helper-x86_64-pc-windows-msvc.exe` doesn't exist
```

`frontend/src-tauri/binaries/` 是 **gitignored 的构建产物目录**，其中 sidecar 必须先行构建并放入：

```
cargo build -p llama-helper --release
copy target\release\llama-helper.exe frontend\src-tauri\binaries\llama-helper-x86_64-pc-windows-msvc.exe
```

另外 `whisper.cpp` 的 C++ 源码在 `whisper-rs 0.13.2` 下需要 `CL=/utf-8`（MSVC 默认代码页把 UTF-8 注释当成非法字面量后缀，报 `C3688`）。
**升级到 0.16.0 后该 workaround 已不再需要**——我强制删除 `whisper-rs-sys` 的全部构建目录后、不带 `CL=/utf-8` 重新编译验证通过（`whisper-rs-sys 0.15.0` 耗时约 11 分钟）。保留此说明是为了让人遇到同样 `C3688` 时知道根因，而不是把它当成必须步骤。

**完整可复现验证命令**：

```
cargo build -p llama-helper --release
copy target\release\llama-helper.exe frontend\src-tauri\binaries\llama-helper-x86_64-pc-windows-msvc.exe
cargo check -p meetily
cargo test -p meetily --lib
```

**风险提示（需你知晓）**：升级 `whisper-rs` 改动的是**转写核心依赖**。它未经实机转写验证——`cargo check` 只证明类型与 API 正确，不证明行为一致。建议在真实音频上回归一次转写质量；`set_suppress_nst`、`no_speech_thold`、`max_len` 等参数语义在 0.16 中是否与 0.13 完全一致，我**无法仅凭类型检查确认**。

#### Rust 测试现状：263 个用例，14 个失败，**经查全部为既有/环境问题**

`cargo test -p meetily --lib` 跑通（此前从未能运行），结果：**249 通过 / 14 失败**。逐类归因如下——**没有一个是本次改动引入的**：

| 失败类别 | 数量 | 归因 | 证据 |
|---|---|---|---|
| `audio::vad::*` | 8 | **ONNX Runtime 未初始化**：`ort` 用 `load-dynamic`，运行时由 `lib.rs` setup 里的 `ort::init_from(...)` 提供；单测不经过 app setup → `ort` 在 `lib.rs:168` panic | panic 位置在 `ort-2.0.0-rc.10/src/lib.rs:168`，与转写/VAD 逻辑无关 |
| `parakeet_engine::*` | 4 | 同上（Parakeet 即 ONNX 路径），测试自带本地 HTTP 服务但止步于 ORT 初始化 | 同上 |
| `audio::device_detection::*` | 1 | **既有浮点断言缺陷**：`3840 帧 @48kHz × 2 = 159.999996ms`，测试用 `assert_eq!(timeout, Duration::from_millis(160))` 精确相等 | `device_detection.rs:486`；该文件**不引用 `ort` 也不引用 `whisper`**，故与我方改动完全无关 |
| `analytics::*` | 1 | 埋点会话测试，环境相关 | `analytics/analytics.rs::ending_session_completes_and_emits_an_event` |
| `summary::llm_client::*` | 1 | legacy Ollama 重试/取消时序测试 | `summary/llm_client.rs::cancellation_during_failed_legacy_ollama_retry_body_returns_promptly` |

**关键证据**：`device_detection` 那条在不依赖 `ort`/`whisper` 的文件里失败，说明失败集合与 whisper-rs 升级**无因果关系**（否则纯数学模块不会受影响）。

**本次我新增的 Rust 测试全部通过**：

```
database::repositories::meeting_notes::tests   → 6 passed
database::notes_commands::tests                → 5 passed
summary::templates::loader::tests              → 17 passed（含 5 个我新增）
```

若要修那 14 个，需单独排期：VAD/Parakeet 类需要一个测试期 ORT 初始化夹具（或在测试里 `#[ignore]`），`device_detection` 类把精确相等等价改为毫秒取整比较。



### 4.2 Phase 2（进行中）

**⑦⑧ meeting notes 指定位置 + 图片：后端已完成**

`meeting_notes` 表此前零代码读写（"已建未接"），现补齐：

| 位置 | 改动 |
|---|---|
| `database/repositories/meeting_notes.rs` | 新增：`MeetingNote` / `NoteBlock`（`text`/`bullet`/`heading`/`image`）/ `NoteInput`；`get`（无记录时返回空文档而非 `None`，把"没有笔记"当正常态）／`save`（upsert，同时写 `notes_json` 与派生的 `notes_markdown`）／`delete`／`has_content`；`blocks_to_markdown` + `markdown_to_blocks` 双向转换（JSON 损坏时从 markdown 恢复，笔记不会因格式问题丢失） |
| `database/repositories/mod.rs` | 注册模块 |
| `database/notes_commands.rs` | 新增 7 个命令：`api_get_meeting_note`、`api_save_meeting_note`、`api_delete_meeting_note`、`api_meeting_has_note`、`api_get_note_images_dir`、`api_save_note_image`、`api_delete_note_image` |
| `lib.rs` | 注册上述 7 个命令 |

**关于图片存储的两个设计决定**：

1. **图片落盘、不入库**：写入 `notes_folder/images/`（跟随 ③ 的可配置笔记目录，因此移动笔记位置会连带图片）。若把图片做成 data URL 存进 `notes_json`，几张照片就会显著膨胀 SQLite 并拖慢每次笔记加载。
2. **不引入 base64 依赖**：前端直接传字节数组（`Vec<u8>`），避免为一个解码需求新增 crate。

安全与健壮性处理：
- 文件名 `sanitize_file_name` 剥离路径分隔符与不安全字符；**加时间戳前缀**，避免两次粘贴 `image.png` 互相覆盖掉已被笔记引用的图片；
- 单图上限 25 MB；
- `api_delete_note_image` **强制校验目标路径 canonicalize 后仍位于笔记图片目录内**，否则拒绝删除——因为路径来自笔记内容，不校验等于提供了任意文件删除原语；
- 笔记 id 复用数据层既有的空值校验。

**Rust 单测**（写于 `meeting_notes.rs`，6 例）：markdown↔blocks 往返一致（含图片与标题）、无 alt 图片保留、空笔记渲染为空、标题层级 clamp、空行忽略、**畸形图片语法降级为文本而非静默丢弃**。

**⑧ 的前端（笔记页）：已完成**

`app/notes/[id]/page.tsx` 原本是**硬编码示例数据**（`sampleData` 里 4 条假笔记，零 Tauri 调用），已整体重写为真实编辑器：

- 载入：`api_get_meeting_note` + `api_get_meeting_metadata`（元数据失败不阻塞编辑）；
- 编辑：四类块（`heading`(H1–H3) / `text` / `bullet` / `image`），可增删、可改图片题注；
- 保存：显式保存（非每次按键触发），带 `dirty` 指示与 `beforeunload` 拦截，避免丢改动；
- 图片：新增/删除，删除时同时清理磁盘文件（后端校验目录包含关系）。

**顺带修复一个使功能不可达的真实缺陷**：`Sidebar/index.tsx:582` 用
`item.id.includes('-') ? '/meeting-details' : '/notes/{id}'` 决定去向，但会议 id 形如
`meeting-<uuid>`（`transcript.rs:19`、`import.rs:697` 证实）**必然含连字符**，因此该三元
表达式的 `/notes/` 分支**永远不可达**——笔记功能在 UI 上根本进不去。已在侧栏每个会议项
新增独立的「笔记」按钮（`StickyNote` 图标，`router.push('/notes/{id}')`）。

**图片显示方式的取舍**：最初按常规用 `convertFileSrc`（asset 协议），但 `tauri.conf.json`
的 `assetProtocol.scope` 只有 `$APPDATA/**`，而笔记目录是 ③ 引入的**用户可配置路径**——
笔记放在别处时图片会显示为空白。改为新增 `api_read_note_image` 命令读字节并转 data URL，
并施加与删除相同的包含性校验。代价是图片经 IPC 传输，换来"笔记放哪里图片都能显示"。

后端为此内置了一个**极简 base64 编码器**（附 RFC 4648 测试向量），以避免为单一用途引入 base64 依赖。

**⑥ cloud ASR 框架 + 多语言补全：已完成（按约定不真正上传）**

原先 provider 集合是**散落的 `match` 分支 + 前端硬编码 map**：`engine.rs` 有两处 `match config.provider.as_str()`，`TranscriptSettings.tsx` 有一份 `modelOptions`。加一个 provider 要改多处，且**没有任何地方记录 provider 支持什么语言**——Parakeet 被标成 "Recommended" 却只支持英文，用户无从知晓。

改为**数据驱动 + 单一真源**：

| 位置 | 改动 |
|---|---|
| `audio/transcription/catalog.rs` | 新增 `PROVIDERS` 目录（6 个 provider：parakeet / localWhisper / deepgram / elevenLabs / groq / openai），每项声明 `kind`（local/cloud）、`status`（available/not_implemented）、`supported_languages`、`requires_api_key`、`models`、`notes`、`api_key_url` |
| 同上 | `ensure_selectable`（拒绝未实现 provider）、`validate_language`（按 provider 校验语言并**给出替代建议**）、`is_cloud`、`is_implemented`、`default_provider_id` |
| `audio/transcription/provider.rs` | `TranscriptionProvider` 新增两个**带默认实现**的方法：`kind()`（默认 Local）与 `supports_language()`（默认全通过）——默认值保证既有实现零改动，且"新 provider 不会因漏写而静默变成云" |
| `audio/transcription/engine.rs` | 两条路径入口加 `ensure_selectable`；验证路径再加 `validate_language`。这使**未实现 provider 在录音开始前就被拒绝**，而非依赖前端自觉。顺带修掉 3 处谎报 "defaulting to parakeet" 的日志（实际走的是 Whisper 分支） |
| `audio/transcription/provider_commands.rs` | 新增 4 个命令：`api_list_speech_providers`、`api_get_active_speech_settings`、`api_validate_speech_selection`、`api_get_provider_languages`；`api_get_transcript_api_key` 返回空串表示"未配置"，此处归一化为 `None` |
| `lib.rs` | 注册上述 4 个命令 |
| `hooks/useSpeechProviders.ts` | 新增：拉取目录 + `validateSelection` + `languagesFor` |
| `components/TranscriptSettings.tsx` | provider 下拉改为**遍历目录**（原先 4 个云端选项被注释掉，且带错误的模型名，如 Groq 写成 `llama-3.3-70b-versatile`）；未实现的置灰标注 "not available yet"；内联展示所选 provider 的能力说明与 provider/语言不匹配警告 |

**核心安全性质（测例锁定）**：所有 `cloud` provider 的 `status` 均为 `NotImplemented`，`ensure_selectable` 一律拒绝，因此**当前不可能有音频被上传**——这不是文档承诺，而是被测例固定的行为。

**多语言补全**：`validate_language` 让"Parakeet + 中文"这类组合在设置界面即报错并**建议改用 localWhisper**，而不是等录音后产出不可用文本。

**Rust 单测**：`catalog.rs` 11 例，覆盖默认 provider 可用、**所有云 provider 均不可选**、本地 provider 免 key、Parakeet 拒绝中文且建议 Whisper、英文大小写/地区变体接受、Whisper 接受任意语言、`auto`/空值视为无限制、未知 provider 报错含有效 id、provider id 唯一性。

### 4.3 功能清单状态：**8 项全部实现**

①②③④⑤（Phase 1）与 ⑥⑦⑧（Phase 2）均已落地。剩余的是**实机验证**与既有测试修复（见 §4.1 末的测试现状小节）。

### 4.4 笔记路由改造（修复 `pnpm build` 失败）

**故障**：`pnpm run tauri:build` 在 `beforeBuildCommand: pnpm build` 阶段失败：

```
Error: Page "/notes/[id]" is missing "generateStaticParams()" so it cannot be used with "output: export" config.
```

**根因**：`frontend/next.config.js:9` 是 `output: 'export'`（纯静态导出），动态路由段**必须在构建时枚举出全部路径**。而笔记的 id 是运行时才产生的会议 id，无法在构建时知道——原版的 `generateStaticParams()` 只是返回 4 个硬编码假 id（`team-sync-dec-26` 等），恰好服务于那时页面里的 `sampleData` 假数据。我上一轮把假数据换成真实后端调用时删掉了它，构建随即暴露。

**修法**：改为查询参数路由，与仓库中已验证的 `/meeting-details?id=…` 完全一致：

| 变更 | 说明 |
|---|---|
| `src/app/notes/[id]/page.tsx` → 删除 | 动态段不再需要枚举 |
| `src/app/notes/page.tsx` 新增 | `useSearchParams()` + `Suspense` 包裹 |
| `src/app/notes/MeetingNotesContent.tsx` 新增 | 原编辑器内容，`useParams` → `useSearchParams` |
| `Sidebar/index.tsx:631` | `/notes/${id}` → `/notes?id=${encodeURIComponent(id)}` |

`useSearchParams()` **必须包在 `Suspense` 里**，否则静态导出在收集页面数据阶段会失败——`meeting-details` 同样如此，其 fallback 也是 spinner。

**顺带清理**：`Sidebar/index.tsx` 中原有的 `item.id.includes('-') ? '/meeting-details' : '/notes/{id}'` 三元表达式，因会议 id 形如 `meeting-<uuid>` **必然含连字符**，其 `/notes/` 分支从来不可达。已简化为单一去向（笔记由每行的独立按钮进入）。

**验证**：
- `pnpm build` **通过**，`✓ Generating static pages (8/8)`，路由表出现 `○ /notes 4.18 kB`；
- 导出产物 `out/notes.html`（11.6 KB）生成，`out/` 下已无 `[id]` 目录；
- `tauri.conf.json` 的 `frontendDist: ../out` 指向该目录，确认 Tauri 会消费此产物；
- `tsc --noEmit` exit 0，`bun test` 65/65。

**一个值得记录的取舍**：静态 HTML 里烘焙的是 `Suspense` 的 fallback（spinner），而非组件内"未选择会议"的空态——React 不会预渲染 Suspense 边界内的内容。这与 `/meeting-details` 的行为一致，客户端 hydrate 后立即接管，故未额外处理；已在代码注释中写明原因，避免后来者误以为 spinner 是 bug。



