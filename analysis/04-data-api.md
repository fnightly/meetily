# Meetily 数据 / 存储 / API 层技术分析（v0.4.1, git main）

分析范围（只读，未修改任何源文件）：

- `frontend/src-tauri/src/database/**`
- `frontend/src-tauri/src/api/**`
- `frontend/src-tauri/src/{state,config,utils,onboarding,tray}.rs`
- `frontend/src-tauri/migrations/*.sql`
- `frontend/src-tauri/Cargo.toml`、`Cargo.toml`（workspace）
- `frontend/API.md`、`backend/API_DOCUMENTATION.md`（文档对照）

> 说明：为判定"调用点是否存在"和"持久化路径是否完整"，少量引用了 scope 之外文件的**行号证据**（`summary/commands.rs`、`summary/service.rs`、`audio/import.rs`、`audio/retranscription.rs`、`frontend/src/**`）。这些仅作为交叉验证，未对其实质逻辑做分析。

---

## 1. 数据库选型与 schema

### 1.1 选型

SQLite，通过 `sqlx 0.8`（`runtime-tokio`、`sqlite`、`chrono`）访问。单文件数据库，位于 Tauri `app_data_dir`：

- 主库：`meeting_minutes.sqlite`（`frontend/src-tauri/src/database/manager.rs:55-58`）
- 遗留库（仅用于首次迁移检测）：`meeting_minutes.db`（`manager.rs:60-63`）

`Cargo.toml` 中 db 相关依赖只有一行：`sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "chrono"] }`（`frontend/src-tauri/Cargo.toml:148`）。没有 ORM（无 diesel/sea-orm），全部是手写 SQL + `FromRow` 映射。workspace 根 `Cargo.toml` 不含任何数据库依赖，仅声明 `frontend/src-tauri` 与 `llama-helper` 两个成员（`Cargo.toml:3-6`）。

值得注意：**全仓库没有任何 `SqlitePoolOptions` / `SqliteConnectOptions` / `journal_mode` / `busy_timeout` / `max_connections` 配置**（全仓 grep 只命中一条注释性 PRAGMA，见 §1.4）。池行为完全取决于 sqlx 默认值。

### 1.2 表结构（以 migrations 为唯一事实来源）

迁移目录：`frontend/src-tauri/migrations/`，10 个文件，`sqlx::migrate!("./migrations")` 编译期内嵌（`manager.rs:35`）。

**初始 schema**（`20250916100000_initial_schema.sql`）：

| 表 | 列 | 约束 |
|---|---|---|
| `meetings` (`:2-7`) | `id TEXT PK`, `title TEXT NOT NULL`, `created_at TEXT NOT NULL`, `updated_at TEXT NOT NULL` | 无索引 |
| `transcripts` (`:10-19`) | `id TEXT PK`, `meeting_id TEXT NOT NULL`, `transcript TEXT NOT NULL`, `timestamp TEXT NOT NULL`, `summary TEXT`, `action_items TEXT`, `key_points TEXT` | `FK(meeting_id) → meetings(id) ON DELETE CASCADE` |
| `summary_processes` (`:22-35`) | `meeting_id TEXT PK`, `status NOT NULL`, `created_at`, `updated_at`, `error`, `result`, `start_time`, `end_time`, `chunk_count INTEGER DEFAULT 0`, `processing_time REAL DEFAULT 0.0`, `metadata` | `FK → meetings(id) CASCADE` |
| `transcript_chunks` (`:38-48`) | `meeting_id TEXT PK`, `meeting_name`, `transcript_text NOT NULL`, `model NOT NULL`, `model_name NOT NULL`, `chunk_size INTEGER`, `overlap INTEGER`, `created_at NOT NULL` | `FK → meetings(id) CASCADE` |
| `settings` (`:51-60`) | `id TEXT PK`, `provider NOT NULL`, `model NOT NULL`, `whisperModel NOT NULL`, `groqApiKey`, `openaiApiKey`, `anthropicApiKey`, `ollamaApiKey` | 单行表（约定 `id='1'`） |
| `transcript_settings` (`:63-72`) | `id TEXT PK`, `provider NOT NULL`, `model NOT NULL`, `whisperApiKey`, `deepgramApiKey`, `elevenLabsApiKey`, `groqApiKey`, `openaiApiKey` | 单行表（约定 `id='1'`） |

**后续演进（全部为 ADD COLUMN 或表重建）**：

| 迁移 | 变更 |
|---|---|
| `20250920155811_add_openrouter_api_key.sql` | 重建 `settings` 加 `openRouterApiKey TEXT`（`:5-26`） |
| `20251006000000_add_audio_sync_fields.sql` | `meetings.folder_path TEXT`（`:8`）；`transcripts.audio_start_time/audio_end_time/duration REAL`（`:15-17`） |
| `20251010153942_add_ollama_endpoint.sql` | `settings.ollamaEndpoint TEXT`（`:2`） |
| `20251101000000_add_summary_backup.sql` | `summary_processes.result_backup`、`result_backup_timestamp`（`:4-8`） |
| `20251105120000_add_pro_license_custom_openai.sql` | `settings.customOpenAIConfig TEXT`（`:4`）；**`DROP TABLE IF EXISTS licensing` 后重建**（`:7-20`），列：`license_key PK, encrypted_key, signature_hash, activation_date, expiry_date, soft_expiry_date, max_activation_time, duration, generated_on, is_soft_expired` |
| `20251110000000_add_grace_period_to_licensing.sql` | `licensing.grace_period INTEGER NOT NULL DEFAULT 604800`（`:5`） |
| `20251110000001_add_speaker_field.sql` | `transcripts.speaker TEXT`（`:5`） |
| `20251223000000_add_meeting_notes.sql` | 新建 `meeting_notes(meeting_id TEXT PK, notes_markdown, notes_json, created_at NOT NULL, updated_at NOT NULL)` + FK CASCADE（`:2-9`），并建 `idx_meeting_notes_meeting_id`（`:12`） |
| `20251229000000_add_gemini_api_key.sql` | `settings.geminiApiKey TEXT`（`:4`） |

### 1.3 索引与外键实况

**索引**：整个 schema 只有 1 个索引，且是冗余的 —— `idx_meeting_notes_meeting_id ON meeting_notes(meeting_id)`（`20251223000000_add_meeting_notes.sql:12`），而 `meeting_id` 已是该表的 `PRIMARY KEY`（同文件 `:3`），SQLite 会为该 PK 隐式建索引，因此这个显式索引没有任何查询收益。

**缺失索引（高影响）**：以下列全部被 WHERE / JOIN / ORDER BY 使用，但均无索引：

- `transcripts.meeting_id` —— 被 `get_meeting`（`repositories/meeting.rs:78`）、`get_meeting_transcripts_paginated` 的 COUNT（`meeting.rs:145-150`）与分页查询（`meeting.rs:153-163`）、`search_transcripts` 的 JOIN（`repositories/transcript.rs:97-105`）、`retranscription` 的 DELETE（`audio/retranscription.rs:437`）、删除级联（`meeting.rs:262`）反复使用 → 每次都是全表扫描。
- `transcripts.audio_start_time` —— 分页的 `ORDER BY audio_start_time ASC`（`meeting.rs:156`）无索引 → 每页都要排序整表。
- `meeting_notes` 之外的 `summary_processes.meeting_id`（PK，OK）、`transcript_chunks.meeting_id`（PK，OK）—— 这两张表因 `meeting_id` 即主键而天然有索引。
- `meetings.created_at` —— `ORDER BY created_at DESC`（`meeting.rs:12`）无索引。

**外键**：所有 FK 都声明为 `ON DELETE CASCADE`，但 schema 本身不控制 `PRAGMA foreign_keys`；其生效与否取决于连接选项。sqlx 的 `SqliteConnectOptions` 默认将 `foreign_keys` 设为 `ON`，而 `SqlitePool::connect(url)`（`manager.rs:33`）走该默认值，所以级联**在默认构建下是启用的**。但这是一个隐式依赖：代码从未显式声明，一旦有人改用 `SqliteConnectOptions::new()` 手工构造或改动 DSN，级联会静默失效，而删除路径的判断逻辑（见 §2.5）会随之产生孤儿行。

唯一一处 `PRAGMA foreign_keys` 写在迁移文件里（`20250920155811_add_openrouter_api_key.sql:2,28`）。sqlx 把每个迁移脚本作为**单个事务**执行，而 SQLite 的 `PRAGMA foreign_keys` 在事务内是 no-op，因此该 PRAGMA 实际不生效。此处恰好无子表引用 `settings`，故未造成数据问题，但这是一个错误的迁移写法，照抄到有子表的表重建上会出事故。

### 1.4 迁移策略

- 目录式版本化迁移，文件名前缀时间戳；sqlx 以 `_sqlx_migrations` 记录已应用版本 + checksum，已应用的迁移不可修改（改了会导致校验失败）。
- 无 down 迁移，无数据回填脚本，无 `IF NOT EXISTS` 与 `ADD COLUMN` 的统一约定（有的用 `IF NOT EXISTS`，有的不用），风格不一致。
- `PRAGMA journal_mode=WAL` 从未开启，而 `manager.rs:65-67` 却按 WAL 设计清理 `meeting_minutes.sqlite-wal` / `-shm`，`manager.rs:194` 的 `PRAGMA wal_checkpoint(TRUNCATE)` 在非 WAL 模式下是 no-op。注释（`manager.rs:184-188`）声称会"checkpoint WAL 并删除 .wal/.shm"，与实际 journal 模式不符。

---

## 2. DB 层代码

### 2.1 连接管理

`DatabaseManager { pool: SqlitePool }`，派生 `Clone`（`manager.rs:6-9`），通过 `tauri::State<AppState>` 注入命令；`AppState` 只含该字段（`state.rs:3-5`）。**没有 Mutex 包裹数据库**——并发由 sqlx 池内部处理，这也意味着写并发完全依赖 SQLite 的锁与 sqlx 的默认 `busy_timeout`。

池的创建没有任何调优：`SqlitePool::connect(tauri_db_path).await?`（`manager.rs:33`）。既没有限制 `max_connections`（sqlx 默认 10），也没有设 `busy_timeout`（sqlx 默认关闭）。SQLite 同时只允许一个写事务，因此**并发写会直接返回 `SQLITE_BUSY` / "database is locked" 而不是重试等待**。潜在并发写场景是真实存在的：摘要生成（`summary/service.rs`）、重转写（`audio/retranscription.rs:431-461`）、音频导入（`audio/import.rs:701-740`）、录音结束批量落库（`api.rs:976`）。

### 2.2 初始化与遗留库迁移

`new_from_app_handle`（`manager.rs:44-118`）流程：

1. `app_data_dir()`；若不存在则创建（`:50-52`）。
2. 若 `meeting_minutes.sqlite` 不存在且 `meeting_minutes.db` 存在 → `fs::copy` 把遗留 DB 复制为 `.sqlite`（`:19-31`）。
3. 打开池（`:33`），跑迁移（`:35`）。
4. 失败时若错误串含 `malformed`/`corrupt`，则删除 `-wal`/`-shm` 后重试一次（`:78-110`）。

这种"按错误字符串匹配触发恢复"的做法（`:81`）很脆弱：SQLite 的损坏信息随版本变化，非英文 locale 或 sqlx 包装后的消息可能不匹配，导致损坏库无法恢复。同一路径还包括 `is_first_launch`（`:121-130`）与 `import_legacy_database`（`:133-158`，直接把用户选择的 DB 复制成 `meeting_minutes.db` 再走上面的流程，**会覆盖同名文件，无备份**）。

`setup.rs:9-37` 是启动入口：非首次启动时同步 `initialize_database_on_startup` → `app.manage(AppState { db_manager })`（`:33`）；首次启动时**不注册 state**，而是 spawn 一个 delay 500ms 的任务发 `first-launch-detected` 事件（`:19-26`），随后由前端调用 `initialize_fresh_database` / `import_and_initialize_database` 完成注册（`database/commands.rs:164,188`）。

### 2.3 查询构造与 SQL 注入评估

绝大多数语句是常量 SQL + `.bind()` 参数绑定，例如 `meeting.rs:12`、`:65-68`、`transcript.rs:49-61`、`summary.rs:50-55`。

存在 **5 处 `format!` 拼接 SQL**，全部在 `repositories/setting.rs`：

- `save_api_key`：拼 `"{}"` 列名（`:96-105`）
- `get_api_key`：拼 SELECT 列名（`:134-138`）
- `save_transcript_api_key`：拼列名 + **拼 `model` 字面量**（`:194-203`）
- `get_transcript_api_key`：拼列名（`:226-230`）
- `delete_api_key`：拼 SET 列名（`:260-264`）

逐条审计结论：**当前不构成注入**。列名来自 `match provider` 的硬编码白名单（`:82-94`、`:120-132`、`:180-192`、`:212-224`、`:246-258`），`_ =>` 分支返回 `sqlx::Error::Protocol`；`save_transcript_api_key` 中拼接的 `crate::config::DEFAULT_PARAKEET_MODEL` 是 `const &str`（`config.rs:12`）。但这是**脆弱的白名单 + 字符串拼装**组合：任一白名单条目被改成带 `;` 或引号的字符串、或未来有人把 provider 参数来源改成可配置项，就会立刻变成注入点。相比之下 `search_transcripts` 的 `format!("%{}%", ...)`（`transcript.rs:95`）拼的是**绑定值**而非 SQL 结构，安全。

真正需要注意的是 `search_transcripts` 未转义 LIKE 通配符：用户输入 `%`（`transcript.rs:95,101`）会被当作通配符，`%` 单字符查询即返回全库 transcript 行（`transcript.rs:97-105`），是廉价的信息枚举/DoS 放大面。

### 2.4 错误处理

错误类型不统一，四种风格并存：

| 层 | 错误类型 | 证据 |
|---|---|---|
| manager | `sqlx::Result`，IO 错误被手工包装成 `sqlx::Error::Io(e)` | `manager.rs:15,26,51` |
| setup/commands | `Result<(), String>`，用 `format!` 拼前缀 | `setup.rs:13,31`；`commands.rs:20,91,159` |
| api 命令 | `Result<_, String>`，`e.to_string()` | `api.rs:350,380,501` |
| summary/import/retranscription | `anyhow::Result` + `anyhow!` | （scope 外交叉验证：`audio/import.rs:701`） |

`save_transcript` 内部把 `sqlx::Error` 直接透传给上层，再由 `api.rs:1001` 转成字符串，因此前端只看到 `Failed to save transcript: <sqlx 内部文本>`，缺乏结构化错误码（区分"约束冲突/锁超时/磁盘满"）。

`SearchRequest`、`GetApiKeyRequest`、`DeleteMeetingRequest`、`SaveMeetingTitleRequest`、`SaveMeetingSummaryRequest`、`SaveTranscriptRequest`、`ApiResponse<T>`、`Meeting`、`Profile`、`ModelConfig`、`TranscriptConfig` 这一批 `api.rs` 中的结构体（`:22-204`）**没有任何引用点**（命令参数是扁平展开的），属于死代码；`SaveModelConfigRequest` 还在 `api.rs:82-91` 与 `repositories/setting.rs:5-15` 被重复定义了两份。

### 2.5 事务使用

事务是手写的三段式（begin / 语句 / commit|rollback），没有 RAII guard，因此**每条 `?` 提前返回都会绕过显式 rollback**，依赖 `Transaction` 的 `Drop` 行为。已观察到的两个 helper：

- `DatabaseManager::with_transaction`（`manager.rs:164-182`）：`f` 返回 `Err` 时显式 `rollback`。
- **`with_transaction` 从未被调用**（全仓 grep 只有定义点）。

实际使用情况：

- `save_transcript`（`transcript.rs:21-80`）：`pool.acquire()` + `conn.begin()`，失败分支显式 rollback（`:40,68`）。正确。
- `get_meeting`（`meeting.rs:60-83`）：**只读查询却开了事务**，且注释/命名混用 `delete_meeting_with_transaction`；`meeting.rs:70-73` 走 `Err(RowNotFound)` 分支时**先 rollback 再返回错误**，`meeting.rs:105-108` 的 `else` 分支实际不可达（前面已 `is_none()` 早返回）。
- `update_meeting_title`（`meeting.rs:179-195`）：`rollback` 后返回 `Ok(false)`。
- `delete_meeting`（`meeting.rs:25-47`）：注意 `:28` 传的是 `&mut transaction`，而 helper 形参类型是 `&mut SqliteConnection`（`:233-236`），靠 `Transaction: DerefMut<Target = SqliteConnection>` 的 deref coercion 通过。
- `summary.rs:26`、`:136`、`:163`、`:194` 中 `update_process_completed/failed/cancelled` 都是**无事务的单条 compare-and-set UPDATE**，用 `WHERE ... AND start_time = ? AND LOWER(status) = 'pending'` 保证"首个终态写入者获胜"（`:140`、`:174`、`:205`），`rows_affected() == 1` 作为成功判据。这是本层里设计最扎实的部分，并配有回归测试（`summary.rs:270-364`）。

---

## 3. 会议 / 转写 / 摘要持久化流程

### 3.1 转写写入：一次性批量事务，逐行 INSERT

**不是**流式/分段落库。真实链路：

1. 录音过程中，前端把每个转写片段写入浏览器 IndexedDB（`frontend/src/services/indexedDBService.ts:230` 每个片段一次 `add`），仅作为崩溃恢复用途。
2. 停止录音且转写完成时，前端把**内存中的全部片段数组**一次性传给 Rust（`frontend/src/hooks/useRecordingStop.ts:241,256-260` → `storageService.saveMeeting` → `frontend/src/services/storageService.ts:44`）。
3. `api_save_transcript` 接收 `transcripts: Vec<serde_json::Value>` 再逐条 `serde_json::from_value` 转成 `TranscriptSegment`（`api.rs:955-962`），失败则整批报错。
4. `TranscriptsRepository::save_transcript`（`transcript.rs:13-83`）在**单个事务**内先插 `meetings`（`:27-36`），再 `for segment in transcripts` **逐条 INSERT**（`:47-71`），最后 commit（`:80`）。

因此：**批处理边界 = 一个事务，但语句粒度 = 每片段一条 INSERT**（`:49-61`）。一场 2 小时会议按 3-5 秒片段计约有 1500-2400 条语句，虽然避免了 fsync 放大，但仍是 N 次 prepare/execute 往返；没有使用多值 `INSERT ... VALUES (...),(...)` 或 `QueryBuilder::push_values`。

`api.rs:947-952` 在生产代码里对第一条片段做 `serde_json::to_string_pretty`（并 `.unwrap_or_default()`），这是纯调试残留，会导致每次保存都多一次完整序列化。

`meeting_id` 由后端生成 `meeting-{uuid}`（`transcript.rs:19`），`transcript_id` 同样重新生成（`:48`）——**调用方传入的 `TranscriptSegment.id`（`api.rs:181`）被丢弃**。这使保存可以被安全重试（不会撞主键），但也意味着**重复调用会产生重复会议**，无法区分"重试"与"新会议"。

另外两条**平行的、绕过 repository 的**写入路径，各自复制了同样的逐条 INSERT 逻辑：音频导入 `audio/import.rs:691-749`（用调用方 `segment.id`）与重转写 `audio/retranscription.rs:431-461`（先 DELETE 再逐条 INSERT，同一事务）。

### 3.2 摘要存储

摘要走 `summary_processes` 单行状态机，不写 `transcripts.summary`：

- 启动生成：`create_or_reset_process` 把行置 `PENDING`，并把旧 `result` 复制到 `result_backup`（`summary.rs:97-116`，关键在 `:105-107`）。
- 全量转写文本单独存 `transcript_chunks`，用 `INSERT ... ON CONFLICT(meeting_id) DO UPDATE` upsert（`transcript_chunk.rs:24-45`）。
- 完成：`update_process_completed` 写 `result` JSON，清空 backup（`summary.rs:136-152`）；失败/取消：`result = COALESCE(result_backup, result)` 并在比较交换成功后回滚到备份（`summary.rs:163-185`、`:187-215`）。
- 另有 `update_meeting_summary`（`summary.rs:21-74`）直接改写 `result`，被 `summary/commands.rs:112` 使用；以及 `get_summary_data_for_meeting`（`summary.rs:80-86`）用 `summary_processes JOIN transcript_chunks ON meeting_id` 取数——因为两边 `meeting_id` 都是主键，这个 JOIN 等价于对 `summary_processes` 的过滤，属于无意义的 JOIN。

注意 `transcripts.summary` / `action_items` / `key_points` 三列（`20250916100000_initial_schema.sql:15-17`）**在整个代码库中从未被写入或读取**，是初始 schema 的历史残留（`models.rs:31-33` 仍映射它们）。

### 3.3 设置 / 配置持久化

`settings`（模型/LLM 侧）与 `transcript_settings`（转写侧）都是**约定 `id='1'` 的单行表**，用 upsert 写入：

- `save_model_config`（`setting.rs:41-68`）：`INSERT ... VALUES ('1', ...) ON CONFLICT(id) DO UPDATE`。
- `get_model_config` / `get_transcript_config` 用 `SELECT * ... LIMIT 1`（`setting.rs:35`、`:146`）。

配置既有 SQLite 也有 JSON 键值存储，边界不清晰：

| 内容 | 位置 | 证据 |
|---|---|---|
| LLM/转写 provider、model、各 API Key、ollamaEndpoint、customOpenAIConfig | SQLite `settings` / `transcript_settings` | `setting.rs` 全文 |
| onboarding 进度 | `onboarding-status.json`（tauri-plugin-store） | `onboarding.rs:50,88,116` |
| 录音偏好（保存目录、格式、设备） | `recording_preferences.json` | `audio/recording_preferences.rs:100,148` |
| 分析同意 | `analytics.json`（前端 Store） | `frontend/src/lib/analytics.ts:171-172` |
| 通知偏好 | `preferences.json` | `frontend/src/lib/recordingNotification.tsx:15-16` |
| 遗留后端地址 | `frontend/src-tauri/config/backend_config.json`（**无任何 Rust 读取点**） | 见 §4.4 |

### 3.4 会议列表 / 详情读取路径

- `api_get_meetings` → `SELECT * FROM meetings ORDER BY created_at DESC`，**无分页**（`meeting.rs:12-14`）。侧边栏一次拉全量（`frontend/src/components/Sidebar/SidebarProvider.tsx:100`）。
- `api_get_meeting` 一次取回会议 + **全部** transcript（`meeting.rs:78`）、无 LIMIT，再由 `api.rs:86-96` 丢弃 `transcript_chunks`/speaker 等字段重打包成 `MeetingTranscript`（只保留 id/text/timestamp/三个时间字段）——**`transcripts.speaker` 列（`20251110000001_add_speaker_field.sql:5`）在 API 响应中没有出口**，前端拿不到说话人来源。
- 分页读取 `get_meeting_transcripts_paginated`（`meeting.rs:132-166`）：先 `SELECT COUNT(*)`（`:145-150`）再 `SELECT * ... ORDER BY audio_start_time ASC LIMIT ? OFFSET ?`（`:153-163`）。COUNT 与列表查询是两次独立全表扫描（无索引），且 `OFFSET` 深分页随数据量线性劣化。
- `limit`/`offset` 在 `api.rs:846-852` **没有任何范围校验**：调用方传 `limit = i64::MAX` 会触发一次全量物化。
- 搜索走 `LOWER(t.transcript) LIKE ?`（`transcript.rs:97-105`）：对 `LOWER()` 结果做前缀通配符匹配，即使建了索引也无法使用。

---

## 4. `api/` 模块

### 4.1 它到底是什么

`frontend/src-tauri/src/api/` 只有 3 个文件：`mod.rs`（5 行，re-export）、`commands.rs`（2 行注释："Commands are now directly in api.rs"）、`api.rs`（1383 行）。

**结论：该模块不监听任何网络端口，不注册 HTTP 路由，不实现 HTTP 服务端。** 它是一个 Tauri 命令集合（`#[tauri::command]`），前端通过 Tauri IPC 调用：

- `api_get_meetings`（`api.rs:322`）、`api_search_transcripts`（`:356`）、`api_get_meeting`（`:781`）、`api_get_meeting_metadata`（`:813`）、`api_get_meeting_transcripts`（`:846`）、`api_save_meeting_title`（`:900`）、`api_save_transcript`（`:930`）、`api_delete_meeting`（`:744`）、`open_meeting_folder`（`:1008`）
- `api_get_model_config`（`:467`）、`api_save_model_config`（`:517`）、`api_get_api_key`（`:574`）、`api_delete_api_key`（`:717`）、`api_get/save_transcript_config`（`:600,650`）、`api_get_transcript_api_key`（`:687`）
- `api_save/get_custom_openai_config`（`:1172,1246`）、`api_test_custom_openai_connection`（`:1274`）
- `api_get_profile`（`:386`）、`api_save_profile`（`:406`）、`api_update_profile`（`:433`）、`test_backend_connection`（`:1079`）、`debug_backend_connection`（`:1111`）、`open_external_url`（`:1149`）

全部在 `lib.rs:715-741` 注册。Cargo 依赖中没有 axum/warp/hyper/tower-http/actix（`frontend/src-tauri/Cargo.toml:65-161`），Tauri 插件只有 notification / store / dialog / updater / process / single-instance（`lib.rs:466-470`），**不存在本地 HTTP 监听器**。

### 4.2 认证 / 授权

作为 IPC 命令，访问控制由 Tauri 能力系统 + 窗口作用域决定（`tauri.conf.json:38-78`，`windows: ["main"]`）。这**不是**传统 authn/authz：

- 受影响命令都带一个 `auth_token: Option<String>` 参数，但**仅用于打日志**。例如 `api_get_meetings`（`api.rs:325-330`）、`api_delete_meeting`（`:748-754`）、`api_save_transcript`（`:936`）。参数前缀为下划线表示完全不使用者有 `_auth_token`（`:470`、`:525`、`:578`、`:603` 等）。
- `get_auth_token`（`api.rs:207-230`）会从 `store.json` 读 `authToken` 并打印前 20 个字符（`:217-219`），但该函数标注 `#[allow(dead_code)]`（`:207`）且**无调用点**。
- `make_api_request` 确实会带 `Authorization: Bearer`（`:262-267`），但它服务于外部后端调用（§4.4），并且在没有 token 时只是打一条 `log_warn` 后继续发**未认证请求**（`:266`）。

也就是说：**应用内不存在任何身份验证或权限区分；任何能在 WebView 里执行 JS 的一方（包括被注入的第三方脚本）都能调用全部命令，包括读取明文 API Key 和删除任意会议。**

### 4.3 CORS / 请求校验 / localhost 假设

- **CORS：不存在**。没有 HTTP 服务端，`tauri.conf.json:25-31` 只有 CSP（`default-src 'self'`，`connect-src` 白名单 `localhost:11434/5167/8178` 与 `api.ollama.ai`）。
- **请求校验：极弱且不均衡**。做校验的只有 `api_save_custom_openai_config`（endpoint/model 非空、`http(s)://` 前缀、temperature ∈ [0,2]、top_p ∈ [0,1]、max_tokens ≥ 1，`api.rs:1189-1216`）与 `api_test_custom_openai_connection`（前缀校验，`:1287-1289`）。**未做校验的**：`api_save_transcript` 的 `meeting_title`（`:933`，可空串/超长）、`transcripts` 数组长度与单段文本长度（`:934`）、`api_get_meeting_transcripts` 的 `limit/offset`（`:849-850`）、`api_save_meeting_title` 的 `title`（`:904`）、`open_external_url` 的 `url`（`:1149`）。
- **localhost 假设**：`const APP_SERVER_URL: &str = "http://localhost:5167"`（`api.rs:20`），由 `get_server_address` 硬编码返回（`:233-236`）。从这两点看代码**假定**存在一个监听 5167 的本机服务。但该服务在本仓库中**不存在**：`grep` 全仓仅命中 3 处常量/配置文件（`api.rs:20`、`tauri.conf.json:30`、`frontend/src-tauri/config/backend_config.json:4`），没有任何 `TcpListener` / `bind` / 路由注册；`backend/` 是已归档的 FastAPI（`backend/API_DOCUMENTATION.md:1-4`）。

### 4.4 与 "后端" 有关的死路径（重要发现）

`make_api_request` 唯一的消费者是 5 个 profile 命令：`api_get_profile`（`api.rs:401` → `/get-profile`）、`api_save_profile`（`:421` → `/save-profile`）、`api_update_profile`（`:455` → `/update-profile`），加上 `test_backend_connection`（`:1090` → `/docs`）和 `debug_backend_connection`（`:1128` → `/docs`）。

对 `frontend/src` 全量 grep 后确认：**没有任何前端代码调用 `api_get_profile` / `api_save_profile` / `api_update_profile` / `test_backend_connection` / `debug_backend_connection`**。因此这套"POST 到 localhost:5167 的 FastAPI"的代码是**遗留死代码**：即便被调用也只会连接失败，因为该端口无监听。同理 `frontend/src-tauri/config/backend_config.json`（`whisperEndpoint:8178` / `fastApiEndpoint:5167`）在整个 Rust 代码中**没有任何读取点**（grep `backend_config` 只命中 `audio/capture/backend_config.rs` 的同名模块与一个测试名，与之无关）。

### 4.5 与文档的一致性对照

| 文档声明 | 代码实际 | 判定 |
|---|---|---|
| `frontend/API.md:8-11`："不再需要手工启动的 whisper-server HTTP 服务；Next.js UI 通过 Tauri commands/events 与 Rust 核心通信；本地转写在桌面应用内完成" | 成立。`api/` 全是 Tauri 命令，无 HTTP 服务，转走在 app 内 | ✅ 一致 |
| `backend/API_DOCUMENTATION.md:8-10`：FastAPI 不再是活动 API，形态为 Tauri 命令 | 成立 | ✅ 一致 |
| `backend/API_DOCUMENTATION.md:20-22`："归档的 FastAPI 是未认证的、CORS 面向开发的，不能当作生产 API" | 归档代码确实如此；但**该描述不能用来解释当前风险**——真正未认证的是本应用的 Tauri 命令层（§4.2） | ⚠️ 文档正确但易误导 |
| 两份文档都未描述实际存在的命令面（约 20 个 `api_*` 命令、参数、错误语义） | `api.rs` 是唯一"文档" | ⚠️ 文档缺口 |
| 代码仍保留 `APP_SERVER_URL=http://localhost:5167`、`/get-profile` 等 FastAPI 路径与 `backend_config.json` | 与"已归档"的叙述矛盾，属残留 | ⚠️ 不一致（死代码与文档叙事冲突） |

---

## 5. 配置 / onboarding / 托盘

### 5.1 配置位置与格式

- **SQLite**：`<app_data_dir>/meeting_minutes.sqlite`（`manager.rs:55-58`），无加密、无权限收紧（仅靠 OS 用户目录默认 ACL）。
- **tauri-plugin-store JSON**（同目录，`app_data_dir`）：
  - `onboarding-status.json`（`onboarding.rs:50`）
  - `recording_preferences.json`（`audio/recording_preferences.rs:100`）
  - `store.json`（仅被死代码 `get_auth_token` 读取，`api.rs:209`）
- **前端 Store**（`@tauri-apps/plugin-store`）：`analytics.json`（`frontend/src/lib/analytics.ts:172`、`AnalyticsProvider.tsx:34`）、`preferences.json`（`recordingNotification.tsx:15`、`RecordingSettings.tsx:65`）。同一份应用配置被 4 类不同存储机制切分，且都在 `app_data_dir` 下**明文 JSON**。
- `frontend/src-tauri/config/backend_config.json`：无读取点（§4.4）。

### 5.2 默认值

集中在 `config.rs`：`DEFAULT_WHISPER_MODEL = "large-v3-turbo"`（`:8`）、`DEFAULT_PARAKEET_MODEL = "parakeet-tdt-0.6b-v3-int8"`（`:12`）、`WHISPER_MODEL_CATALOG` 共 12 个模型（`:18-36`）。

初始化默认值有**两条路径且不一致**：

- 首次安装（前端调用 `initialize_fresh_database`）：summary provider `builtin-ai` + 系统推荐模型（失败回退 `"qwen3.5:2b"`，`commands.rs:193-205`），transcription provider `parakeet` + `DEFAULT_PARAKEET_MODEL`（`commands.rs:208-214`）。
- 首次启动但走 onboarding 完成：provider `builtin-ai`，`whisperModel` 硬编码 `"large-v3"` 而非 `DEFAULT_WHISPER_MODEL`（`onboarding.rs:182-188`）。

`api_get_transcript_config` 在无记录时返回 `parakeet` + `DEFAULT_PARAKEET_MODEL`（`api.rs:634-641`），与 `initialize_fresh_database` 一致。

`RecordingPreferences::default()`（`audio/recording_preferences.rs:28-40`）默认 `auto_save=true`、`file_format="mp4"`、目录按平台取 Music/Movies/Documents 下的 `meetily-recordings`（`:43-77`）。

### 5.3 旧配置迁移

只有两条：

1. **遗留 SQLite**：`meeting_minutes.db` → `meeting_minutes.sqlite`（`manager.rs:19-31`），支持三种探测：用户直选 `.db`、目录下的 `meeting_minutes.db`、`<dir>/backend/meeting_minutes.db`（`database/commands.rs:48-83`），以及默认目录与 Homebrew 路径 `/usr/local/var/meetily/meeting_minutes.db`（`frontend/src/contexts/OnboardingContext.tsx:180`）。
2. **onboarding schema 演进**：`selected_summary_model` 用 `#[serde(default)]` 兼容旧 JSON（`onboarding.rs:24-25`），并有专门回归测试 `onboarding_status_deserializes_without_selected_summary_model`（`onboarding.rs:228-245`）。

**没有任何针对旧版 `store.json` / `preferences.json` / 旧 provider 字符串的迁移**：旧配置若反序列化失败，一律静默回退默认值（`onboarding.rs:66-69`、`recording_preferences.rs:121-124`），用户会丢失已填设置且无提示。

### 5.4 密钥处理

**结论：所有 API Key 都以明文存放在未加密的 SQLite 文件中。**

- 列本身是 `TEXT`：`groqApiKey`、`openaiApiKey`、`anthropicApiKey`、`ollamaApiKey`（`20250916100000_initial_schema.sql:56-59`）、`openRouterApiKey`（`20250920155811:14`）、`geminiApiKey`（`20251229000000:4`）；转写侧 `whisperApiKey`、`deepgramApiKey`、`elevenLabsApiKey`、`groqApiKey`、`openaiApiKey`（`:67-71`）。
- 写入无任何加密/封装：`SettingsRepository::save_api_key` 直接 `bind(api_key)`（`setting.rs:105`）；`save_custom_openai_config` 把含 `api_key` 的整个结构体 `serde_json::to_string` 后明文存 `customOpenAIConfig`（`setting.rs:327-343`）。
- 读取路径把明文回吐给前端：`api_get_api_key` 返回 `Ok(key.unwrap_or_default())`（`api.rs:590`）、`api_get_transcript_api_key`（`:700-703`）、`api_get_model_config` 把 key 塞进 `ModelConfig.api_key` 返回（`:484-493`）。
- **且会写进日志**：`api_save_model_config` 打印 provider/model/whisperModel/ollamaEndpoint（`api.rs:527-533`，未打印 key 本身），但 `save_api_key` 拼出的 INSERT 语句含真实列名（`setting.rs:96-105`）；更严重的是 `make_api_request` 会把响应体前 200 字符写日志（`api.rs:302-310`），若后端返回 profile 类数据可能含 PII。
- 数据库本身没有加密扩展（无 SQLCipher），`assetProtocol.scope` 允许 `$APPDATA/**`（`tauri.conf.json:32-37`），即数据库文件位于被 asset 协议覆盖的目录内。

### 5.5 托盘

`tray.rs` 只做录音状态与窗口控制，**不直接持久化任何数据**：

- `toggle_recording_handler` 自主拼接保存路径 `app_data_dir/recording-{timestamp}.wav`（`tray.rs:75-76`）后调用 `stop_recording`，再 `emit("recording-stop-complete")`（`:94`）触发前端去写 SQLite —— 即**托盘路径下数据库落库完全依赖前端 JS 收到事件后执行**，若窗口未加载/事件丢失则本次会议不入库（与 §3.1 的"仅录音结束时批量入库"叠加，构成实质丢失风险）。
- `check_can_record` 读 onboarding 状态决定菜单可用性（`tray.rs:267-291`）；`update_tray_menu` 用固定 100ms sleep 后异步刷新（`:212-220`），是时序猜测而非状态订阅。

---

## 6. 真实缺陷与风险

按严重度排序，均为可在源码定位的具体问题。

### A. 安全相关

1. **API Key 明文落盘 + 无加密 + 直通 WebView**：`setting.rs:105`、`setting.rs:327-343`、`api.rs:590`、`api.rs:700-703`、`api.rs:484-493`。任何本机进程、任何备份/同步工具、以及任何在 WebView 中取得执行权的脚本都可直接读取全部第三方密钥。
2. **命令层零鉴权**：`auth_token` 只被打印（`api.rs:325-330`、`:748-754`、`:936`），`get_auth_token` 是 `#[allow(dead_code)]` 的死函数（`api.rs:207-230`）。配合 `tauri.conf.json:44-52` 授予 WebView `fs:read-all` / `fs:write-all` / `fs:allow-download-*`，一旦 WebView 内容被注入，攻击面是"整个用户可读文件系统 + 全部明文密钥 + 任意会议删除（`api.rs:744`）"，而不只是应用数据。
3. **`open_external_url` 以 shell 方式启动 URL**：Windows 下 `Command::new("cmd").args(&["/C", "start", &url])`（`api.rs:1152-1153`），`url` 由命令参数直接传入（`:1149`），无 scheme 白名单、无转义。`cmd.exe` 的 `/C` 解析规则（`&`、`|`、`^`、`%VAR%`）使构造性 URL 具备命令注入条件。同文件 `open_meeting_folder`（`:1049-1052`）把来自数据库的 `folder_path` 交给 `explorer`，信任边界更低。
4. **`assetProtocol` scope = `$APPDATA/**`（`tauri.conf.json:34-36`）**，而数据库与全部密钥就在该目录（`manager.rs:55-58`）。作用域显著超出必要范围（实际只需录音/模型目录）。
5. **命令式 WebView 注入**：`tray.rs:47`、`:110-111`、`:206-208` 使用 `window.eval("window.location.assign(...)")` 等字符串求值；当前字符串是常量，风险低，但这是注入面。

### B. 数据完整性

6. **`meeting_notes` 表建了但整条链路不存在**：迁移 `20251223000000_add_meeting_notes.sql:2-12` 建表建索引，全仓 grep `meeting_notes` / `notes_markdown` / `notes_json` **只命中迁移文件本身**；无 repo、无命令、无 `models.rs` 结构体。索引 `idx_meeting_notes_meeting_id`（`:12`）本身是对 PK 的冗余索引。属"已建未接"。
7. **删除会议不清 `meeting_notes`**：`delete_meeting_with_transaction` 按序删 `transcript_chunks` → `summary_processes` → `transcripts` → `meetings`（`meeting.rs:250-271`），**未包含 `meeting_notes`**。当前依赖 FK CASCADE 兜底；一旦 FK 失效（见 §1.3 的隐式依赖）就会留下孤儿行。
8. **表无界增长、无保留策略**：`transcripts`（每片段一行，无上限）、`transcript_chunks.transcript_text`（**整场会议全文单行**，`transcript_chunk.rs:38`）、`summary_processes.result`（整份摘要 JSON，`:143`）都不会被裁剪；`get_meetings` 无分页全量返回（`meeting.rs:12-14`），前端一次拉全量（`SidebarProvider.tsx:100`）。没有任何 VACUUM / 清理任务（`manager.rs` 只有 checkpoint）。
9. **保存失败即静默丢数据**：仅当 `isCallApi && transcriptionComplete == true` 才落库（`frontend/src/hooks/useRecordingStop.ts:236`），且没有失败重试或"退回 IndexedDB"分支；托盘路径同样依赖前端事件（`tray.rs:94`）。IndexedDB 只是"恢复 UI"，不会自动补写 SQLite。
10. **保存不可幂等 / 无去重**：`save_transcript` 每次生成新 `meeting-{uuid}`（`transcript.rs:19`），调用方传入的 `segment.id` 被丢弃（`:48`）。网络/IPC 重试将产生重复会议而非覆盖。
11. **`provider` 字段被无关写入污染**（语义 bug，两条）：
    - `save_api_key` 的 INSERT 硬编码 `VALUES ('1', 'openai', 'gpt-4o-2024-11-20', 'large-v3', $1)`（`setting.rs:99`）——保存 claude/ollama/groq/openrouter 的 key 时，会把 `provider` 覆盖为 `'openai'`、`model` 覆盖为 `'gpt-4o-2024-11-20'`。
    - `save_transcript_api_key` 硬编码 `VALUES ('1', 'parakeet', '{}', $1)`（`setting.rs:197-201`）——保存 deepgram/elevenLabs/groq/openai 的 key 时把 provider 覆盖为 `'parakeet'`。
    两者都会污染 `SELECT * FROM settings` 读回的 `provider`（影响 `api.rs:484` 选择哪一个 key 列）。
12. **`custom-openai` 保存后 provider 与配置不一致**：upsert 只更新 `customOpenAIConfig`（`setting.rs:333-344`），不改 `provider`/`model`；而 `get_api_key` 走 `custom-openai` 分支读 JSON（`:115-118`），与 `get_model_config` 读到的 `provider` 列可能不同源。

### C. 并发 / 锁

13. **未配置 `busy_timeout` 与连接数**（全仓无 `SqlitePoolOptions`，`manager.rs:33`）。SQLite 单写者模型下，摘要生成（`summary.rs:97`/`:136`）、重转写（`audio/retranscription.rs:432-461`）、导入（`audio/import.rs:701-740`）、录音落库（`api.rs:976`）并存时，会直接抛 "database is locked" 而非等待重试。**这是当前最可能触发用户可见失败的数据层问题。**
14. **事务跨语句持有连接**：`save_transcript`（`transcript.rs:21-80`）在一个事务里执行 N 条 INSERT，期间独占一条池连接；`import.rs`/`retranscription.rs` 同理。池默认上限 10 且无超时，长时间事务会挤占连接。
15. **无死锁证据，但存在锁顺序同一性风险**：跨表事务的加锁顺序不统一——`save_transcript` 先 meetings 后 transcripts（`transcript.rs:27,49`），`update_meeting_summary` 先 meetings 再 summary_processes（`summary.rs:28,50`），`update_meeting_name` 先 meetings 再 transcript_chunks（`meeting.rs:209,222`）。SQLite 的库级/表级写锁让当前代码尚不会自死锁，但顺序不一致使未来启用更细粒度并发或 WAL + 多进程访问时容易出现互锁，属应先统一的隐患。

### D. `unwrap` / `expect` / panic 面

16. **`expect` 会 panic 的位置（生产路径）**：
    - `manager.rs:49`、`:125`、`:140`：`app_data_dir().expect("failed to get app data dir")`。路径解析失败（权限/环境异常）直接 panic；同文件的 `is_first_launch` 与 `import_legacy_database` 都走这里。
    - `setup.rs:24`：`emit("first-launch-detected").expect(...)`。
    - `lib.rs:586`：启动时 `initialize_database_on_startup(...).expect("Failed to initialize database")` —— 数据库初始化失败会让**整个应用启动崩溃**，而不是降级。
    - `tray.rs:27`：`app.default_window_icon().unwrap()`；图标资源缺失即 panic。
17. **`summary.rs:51` 的 `&result_json.unwrap()`**：紧邻 `:43-48` 已判 `is_err()`，当前安全，但把已判定的 `Result` 用 `unwrap` 再取，属可被后续重构破坏的模式（应改用 `if let Ok(..)`）。
18. 其余 `unwrap` 集中在 `summary.rs:224-362` 的 `#[cfg(test)]` 中，不构成生产风险。

### E. 锁 / 状态注册

19. **`app.manage(AppState)` 可能被调用两次**：首次启动路径下 `setup.rs:15-26` 不注册 state；但前端 `initializeDatabaseInBackground` 与 `setup` 中的 `block_on` 存在竞态（`frontend/src/contexts/OnboardingContext.tsx:151-215` vs `setup.rs:11-35`）。理论上若窗口在 `setup` 完成前完成 `check_first_launch`（窗口在 `setup` 期间已可见），前端与 backend 各自走"不存在 → 创建"分支，则 `commands.rs:188` 会对已被 `setup.rs:33` 注册的类型再次 `manage`。Tauri 对同一类型重复 `manage` 会 panic。当前被时序"恰好"掩盖（两者都会重新检查文件存在性），但这是**依赖执行顺序的正确性**，不是显式防护。
20. **首次启动的 500ms 猜测**：`setup.rs:20-26` 用 `sleep(500ms)` 等 React 监听就绪。慢机器上监听未注册即丢事件，只能靠前端轮询兜底。

### F. 迁移与 schema 卫生

21. **`DROP TABLE IF EXISTS licensing`（`20251105120000_add_pro_license_custom_openai.sql:7`）**：非幂等且带破坏性。虽然 sqlx 会跳过已应用版本，但任何手动重放（导入旧库、回滚 `_sqlx_migrations`、批量修复）都会**直接抹掉用户授权数据**。
22. **`INSERT INTO settings_new SELECT *, NULL AS openRouterApiKey FROM settings`（`:18-20`）**：按位置复制列，依赖两张表列序完全一致。后续任何 `ADD COLUMN` 落在中间都会静默错位。
23. **迁移内 `PRAGMA foreign_keys` 在事务中无效**（`:2`、`:28`，见 §1.3）。
24. **`is_soft_expired INTEGER DEFAULT 0` 的注释声明了 3 个取值 `0/1/2`**（`20251105120000:19`），但代码里没有任何约束或枚举；`licensing` 表在整个 Rust 层**无任何读写点**（grep `licensing` 只命中迁移与 summary 无关模块），是"已建未接"的第二张表。
25. **`meetings` 无 `updated_at` 索引而列表按 `created_at` 排序**（`meeting.rs:12`），与 §1.3 的索引缺失共同构成长列表性能问题。

### G. 日志 / 隐私

26. **明文日志**：`api.rs:302-310` 记录响应体前 200 字符；`api.rs:947-952` 生产路径打印首条转写段（`log_debug`，release 下 `log::set_max_level(Info)`（`lib.rs:448`）会压制，但 debug 构建会输出会议内容）；`api.rs:362-366` 打印完整搜索 query 与结果数。
27. **`grep` 级不可达残留**：`APP_SERVER_URL` + profile 命令 + `backend_config.json`（§4.4）会误导后续维护者以为存在后端依赖，也会持续污染 `tauri.conf.json` 的 CSP 白名单（`tauri.conf.json:30` 仍放行 5167）。

---

## 分析边界与未验证项

- 本次为**纯静态分析**，未编译、未运行、未执行任何测试，也未对数据库做任何写入。
- `busy_timeout` / `foreign_keys` / WAL 的结论基于"全仓无连接选项配置"这一事实 + sqlx 0.8 默认值推导，未通过运行时实测确认（无 `SqlitePoolOptions` 使用点可查）。
- `commands.rs:188` 二次 `app.manage(AppState)` 会 panic 这一判断依据 Tauri 2 的 `manage` 语义，本次未在运行的应用上复现（受执行顺序掩盖，难以稳定触发）。
- 部分调用点证据取自 scope 之外的文件（`summary/commands.rs`、`summary/service.rs`、`audio/import.rs`、`audio/retranscription.rs`、`frontend/src/**`），仅用于交叉验证"是否存在调用点"，未分析其内部逻辑。

---

## 摘要要点

- 数据库为未加密 SQLite（`meeting_minutes.sqlite`，sqlx 0.8 手写 SQL），核心表：`meetings`/`transcripts`/`summary_processes`/`transcript_chunks`/`settings`/`transcript_settings`/`licensing`/`meeting_notes`（`migrations/*.sql`）。
- 索引近乎缺失：全库唯一索引 `idx_meeting_notes_meeting_id` 还是对主键的冗余索引，`transcripts.meeting_id`、`audio_start_time`、`meetings.created_at` 全无索引（`meeting.rs:145,156`；`20251223000000:12`）。
- 池无任何配置：无 `busy_timeout`、无连接数上限、从未显式开启 `foreign_keys`/WAL，而代码注释与 `wal_checkpoint`（`manager.rs:194`）假定 WAL 已启用 —— **并发写下 "database is locked" 是最现实的数据层故障**。
- `api/` 模块**不是 HTTP 服务**，不含监听器/CORS/路由，只有约 20 个 Tauri 命令（`api.rs`，注册于 `lib.rs:715-741`）；文档 `frontend/API.md` 与此一致，但完全没有描述这层真实命令面。
- 命令层**零鉴权**：`auth_token` 参数只用于打日志（`api.rs:325-330,748-754`），`get_auth_token` 是 `#[allow(dead_code)]` 死函数（`api.rs:207`）；`limit/offset`、会议标题、`open_external_url` 的 URL 等入口均无校验。
- **所有 API Key 明文存于 SQLite 并经 IPC 明文回传**（`setting.rs:105,327-343`；`api.rs:590,700-703,484-493`），数据库位于 `assetProtocol` 覆盖的 `$APPDATA/**` 内（`tauri.conf.json:34-36`）。
- 转写**不是流式入库**：录音中仅写浏览器 IndexedDB，停止且转写完成后一次性批量提交，单事务内**逐片段一条 INSERT**（`transcript.rs:47-71`）；`isCallApi && transcriptionComplete` 不成立则本次会议完全不落库（`useRecordingStop.ts:236`）。
- 保存非幂等：会议 ID 与片段 ID 均由后端重新生成，调用方传入的 `segment.id` 被丢弃（`transcript.rs:19,48`），重试会产生重复会议。
- 两处 provider 污染 bug：`save_api_key` 硬编码把 provider 写成 `'openai'`、model 写成 `'gpt-4o-2024-11-20'`（`setting.rs:99`）；`save_transcript_api_key` 硬编码写成 `'parakeet'`（`setting.rs:197-201`）。
- 已建未接的死结构：`meeting_notes` 表与其索引无任何读写点；`licensing` 表无任何 Rust 读写点；`transcripts.summary/action_items/key_points` 三列从未被使用。
- 生产 panic 面（`lib.rs:586` 数据库初始化失败即崩溃；`manager.rs:49,125,140`、`tray.rs:27` 的 `expect/unwrap`；`commands.rs:188` 竞态下二次 `app.manage(AppState)`）与遗留死路径（`api.rs:20` 的 `localhost:5167` 服务并不存在，profile/`test_backend_connection` 无前端调用点，`config/backend_config.json` 无读取点，却仍留在 CSP 白名单 `tauri.conf.json:30`）。
