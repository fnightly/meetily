# 06 — 前端分析（Next.js + React + TypeScript + Tauri IPC）

> **报告来源与勘误说明**
> 本文件上一版本由 Lead 在时限压力下直接完成。本轮由独立分析者按同一 scope 复核后**合并改写**：保留并标注 Lead 已验证、本轮未复核的条目（主要是 `src-tauri` 子目录内的证据），新增本轮亲自读码/计数得到的发现，并**勘误两条**：
> 1. `frontend/tests/` 并非"没有测试框架"——8 个测试文件的运行器是 **`bun:test`**（`devDependencies` 中有 `@types/bun`），可运行前提是装有 Bun；真正缺失的是 `test` 脚本与 CI 调用。
> 2. "监听器无泄漏"这一绝对结论不成立：`useModalState.ts:168-191` 与 `AnalyticsProvider.tsx:62-137` 各有一处**真实泄漏**。Lead 交叉核对过的 `RecordingControls.tsx` / `TranscriptView.tsx` 两处确实干净。
>
> 复核范围：`frontend/src/**`、`frontend/package.json`、`next.config.js`、`tsconfig.json`、`frontend/tests/**`、`frontend/src-tauri/src/lib.rs`（仅命令注册与命令体）。未读取 `src-tauri/src` 其它子目录、`backend/`、`docs/`。

---

## 1. 应用结构

### 1.1 路由（Next.js App Router，`output: 'export'` 静态导出）

| 路由 | 文件 | 职责 |
|---|---|---|
| `/` | `app/page.tsx`（259 行） | 主页：录音控制条、状态浮层、恢复对话框、实时转写面板 |
| `/meeting-details` | `app/meeting-details/page.tsx`（307）→ `page-content.tsx`（258） | 会议详情：分页转写 + 摘要生成 |
| `/notes/[id]` | `app/notes/[id]/page.tsx` | 笔记编辑（BlockNote/Tiptap），**无任何 Tauri 调用** |
| `/settings` | `app/settings/page.tsx` | 设置页（薄壳） |

`next.config.js:8-11`：`reactStrictMode: false`（注释称"为 BlockNote 兼容"）、`output: 'export'`、`images.unoptimized`。关闭 StrictMode 意味着**双调用类 bug 不会被 StrictMode 提前暴露**，这也是本项目多处 effect/监听器重复注册能长期存活的原因之一。

### 1.2 规模（本轮实测）

| 指标 | 数值 |
|---|---|
| `src/**` TS/TSX 文件 | 171 |
| `src/**` 行数 | 27,813 |
| `components/` | 121 文件，最大 `ModelSettingsModal.tsx` 1287、`Sidebar/index.tsx` 783、`AISummary/index.tsx` 758、`WhisperModelManager.tsx` 729、`ParakeetModelManager.tsx` 629 |
| `contexts/` | 7 |
| `hooks/` | 24（含 `meeting-details/` 6 个） |
| `services/` | 6 |
| `console.log` / `console.error` | 346 / 265（合计 611） |
| `aria-label` / `<button` | 14 / 160 |

### 1.3 录音 → 转写 → 摘要 的组件树

```
app/layout.tsx (RootLayout, 'use client')
└─ AnalyticsProvider            components/AnalyticsProvider.tsx
   └─ RecordingStateProvider    contexts/RecordingStateContext.tsx   ← 录音状态机（真实来源）
      └─ TranscriptProvider     contexts/TranscriptContext.tsx       ← 转写片段 + 缓冲/去重/排序
         └─ ConfigProvider      contexts/ConfigContext.tsx           ← 模型/provider/设备/语言
            └─ OllamaDownloadProvider → OnboardingProvider
               └─ UpdateCheckProvider
                  └─ SidebarProvider   components/Sidebar/SidebarProvider.tsx  ← 会议列表/当前会议/摘要轮询
                     └─ TooltipProvider
                        └─ RecordingPostProcessingProvider  contexts/RecordingPostProcessingProvider.tsx
                           └─ ImportDialogProvider
                              ├─ Sidebar/（列表、搜索、删除、模型设置）
                              └─ MainContent → app/page.tsx
```

`/` 页面内的三段式流水线：

- **录音控制**：`app/page.tsx:229` `<RecordingControls>`（`components/RecordingControls.tsx`）→ `useRecordingStart`（`hooks/useRecordingStart.ts`）→ `recordingService.startRecordingWithDevices`（`services/recordingService.ts:95`）→ `invoke('start_recording_with_devices_and_meeting')`。
- **转写**：Rust `transcript-update` 事件 → `services/transcriptService.ts:58` → `TranscriptContext.tsx:288` 主监听器 → 按 `sequence_id` 缓冲/去重/按 `chunk_start_time` 排序（`TranscriptContext.tsx:181-359`）→ `setTranscripts`。
- **收尾与摘要**：`RecordingControls.stopRecordingAction`（`RecordingControls.tsx:142-186`）先 `invoke('stop_recording')`，随后 `onRecordingStop(true)` → `useRecordingStop`（`hooks/useRecordingStop.ts`）等待转写 → `storageService.saveMeeting`（`services/storageService.ts:44`）落库 → `router.push('/meeting-details?id=…&source=recording')` → `useSummaryGeneration`（`hooks/meeting-details/useSummaryGeneration.ts:364`）`invoke('api_process_transcript')` + `SidebarProvider.startSummaryPolling`（`SidebarProvider.tsx:208-288`，5 秒间隔、最多 200 次）。

---

## 2. 状态管理

7 个 Context 无统一 store；同步模式统一为 **`invoke()` 命令 + Tauri 事件回推 + 前端自 `emit`**（如 `page-content.tsx:110` 用 `emit('model-config-updated')` 让其它 Context 同步）。模式本身合理，问题在"谁拥有真相"上被反复重写。

| Context | 拥有的状态（关键行） | 与 Rust 的同步方式 |
|---|---|---|
| `RecordingStateContext`（355 行） | 状态机 `RecordingStatus`（`:17-26`）、`isRecording/isPaused/isActive/durations`（`:62-70`） | `recording-started/starting/stopped/paused/resumed` 事件（`:142-213`）+ **500ms 轮询** `get_recording_state`（`:118`）+ 挂载时首同步（`:335-338`） |
| `TranscriptContext`（539 行） | `transcripts[]`、`transcriptsRef`、`meetingTitle`、`currentMeetingId` | `transcript-update`（`:288`）+ `get_transcript_history`（`:371`）+ IndexedDB 旁路写（`:326`） |
| `ConfigContext`（554 行） | `modelConfig`、`transcriptModelConfig`、`selectedDevices`、`selectedLanguage`、`providerApiKeys`、`betaFeatures`、通知/存储路径设置 | 启动 6 个 `invoke` 拉取（`:186,:201,:220,:234,:306,:354`）+ `model-config-updated`（`:330`） |
| `SidebarProvider`（324 行） | `meetings[]`、`currentMeeting`、`sidebarItems`、`isCollapsed`、`isMeetingActive`、`searchResults`、**`serverAddress` / `transcriptServerAddress`**、摘要轮询表 | `api_get_meetings`（`:100`）、`api_search_transcripts`（`:188`）、`api_get_summary`（`:240`） |
| `OnboardingContext`（580 行） | 引导步骤、数据库导入、模型下载编排 | `get_onboarding_status`、`check_first_launch`、`import_and_initialize_database`、`complete_onboarding`（`:333-577`） |
| `OllamaDownloadContext` | Ollama 拉取进度 | 事件 |
| `ImportDialogContext` / `RecordingPostProcessingProvider` | 小范围 UI 状态 | `recording-stop-complete`（`RecordingPostProcessingProvider.tsx:36`） |

### 2.1 重复与冲突状态（本轮新增证据）

1. **`isRecording` 有三份**：`app/page.tsx:26` 的局部 `useState`（传给 `useRecordingStart`/`useRecordingStateSync`）、`RecordingStateContext`（`:62`，被 `RecordingControls.tsx:46` 与 `page.tsx:230` 消费）、`TranscriptContext` 里通过 `recordingState.isRecording` 间接使用（`:366`）。`RecordingStateContext.tsx:9` 的注释自称 "single source of truth"，而 `page.tsx` 仍以局部 state 作为控制流依据 → 两套状态用 1 秒轮询（`useRecordingStateSync.ts:55`）与 500ms 轮询（`RecordingStateContext.tsx:118`）互相"对齐"，本质是收敛而非单一真相。
2. **`transcriptServerAddress` 是死状态**：全仓仅 `SidebarProvider.tsx:87,122,315` 三处写/透出，**零消费者**（grep 全 `src` 仅命中定义处）。
3. **`serverAddress` 只被当作布尔门闩**：`SidebarProvider.tsx:98`、`Sidebar/index.tsx:112,146`、`ModelSettingsModal.tsx` 用法均为 `if (!serverAddress) return;`——真实数据全部走 `invoke`。硬编码值 `'http://localhost:5167'`（`:121`）与 `'http://127.0.0.1:8178/stream'`（`:122`）指向**已归档的 Python FastAPI 与 whisper-server**，在本仓库无监听者。
4. **模型配置被三处各持一份**：`ConfigContext.tsx:102`、`Sidebar/index.tsx:118-139`、`ModelSettingsModal.tsx:266` 都自行 `invoke('api_get_model_config')` 并写入各自 state，仅靠 `model-config-updated` 事件（`page-content.tsx:110`、`Sidebar/index.tsx:202`）做最终一致；中间窗口下三者可见不同 provider/model。
5. **下载进度双份状态**：`WhisperModelManager.tsx:57-68` 自管 `localStorage['downloading-models']`，同时存在全局 `DownloadProgressToastProvider`（`layout.tsx:248`）。
6. **`localStorage` 直读绕过 Context**：`TranscriptView.tsx:132` 直接读 `showConfidenceIndicator`，与 `ConfigContext.tsx:152-158` 并行（同类还有 `MeetingDetailsSplitView.tsx:19`、`useRecentLanguages.ts:25`、`lib/summary-language-preferences.ts:36-75`）。

---

## 3. Rust ↔ JS 契约

### 3.1 命令面总体

- 本轮以正则 `invoke(?:<T>)?\(\s*'<name>'` 对 `src/**` 全量去重：**144 个字面命令名**（另有 3 处经 `commands.initialize/hasAvailableModels/getAvailableModels` 动态转发，`useRecordingStart.ts:88-107`，其目标命令均已字面出现）。Lead 计数为 139，差异属统计口径（含/不含动态转发与重复文件），不影响结论。
- 已注册但前端**零调用**（在 `lib.rs` 注册表中确认存在、全 `src` 无字面匹配）：`api_get_profile`、`api_save_profile`、`api_update_profile`、`test_backend_connection`、`debug_backend_connection`、`save_transcript`（裸命令名）、`is_audio_level_monitoring`、`api_get_template_details`、`api_validate_template`、`api_save_auto_generate_setting`、`reset_onboarding_status_cmd`、`is_retranscription_in_progress_command`、`is_import_in_progress_command`、`whisper_engine::parallel_commands::*`（`get_parallel_processing_status`、`prepare_audio_chunks` 等 10 个）、`notifications::commands::*` 中的 `show_notification`/`clear_notifications`/`get_notification_stats` 等。

### 3.2 主要命令表

| 命令 | 前端调用点 | 用途 | 与 `lib.rs` 的一致性 |
|---|---|---|---|
| `start_recording_with_devices_and_meeting` | `services/recordingService.ts:95` | 带设备 + 会议名开始录音 | ✓ 参数 `micDeviceName/systemDeviceName/meetingName` ↔ `lib.rs:364-368` 的 snake_case（Tauri v2 自动 camelCase 映射） |
| `start_recording` | `services/recordingService.ts:80` | 无参启动（**无调用者**，死封装） | ✓ `lib.rs:141-146` 三个 `Option` |
| `stop_recording` | `components/RecordingControls.tsx:151`；`services/recordingService.ts:108` | 停止并落盘 wav | ✓ 两处均传 `{ args: { save_path } }`，匹配 `lib.rs:128-131`+`:202` 的结构体入参 |
| `is_recording` | `services/recordingService.ts:56`；`RecordingControls.tsx:78` | 录音中？ | ✓ `lib.rs:266` |
| `get_recording_state` | `services/recordingService.ts:64` | 500ms 轮询状态 | ✓（注册为 `audio::recording_commands::get_recording_state`） |
| `pause_recording` / `resume_recording` | `RecordingControls.tsx:213/231`（另 `recordingService.ts:118/126` 无人调用） | 暂停/继续 | ✓ |
| `get_recording_meeting_name` | `services/recordingService.ts:72` | 刷新后恢复会议名 | ✓ |
| `get_transcript_history` | `services/transcriptService.ts:39` | 刷新后拉历史片段 | ✓ |
| `get_meeting_folder_path` | `contexts/TranscriptContext.tsx:129` | 恢复 folderPath | ✓ |
| **`get_transcription_status`** | `services/transcriptService.ts:47` ← `hooks/useRecordingStop.ts:166` | 轮询"转写是否结束" | ✗ **注册的是硬编码桩**（见 §3.3） |
| `api_get_meetings` | `Sidebar/SidebarProvider.tsx:100` | 会议列表 | ✓ |
| `api_search_transcripts` | `SidebarProvider.tsx:188` | 侧栏搜索 | ✓ |
| `api_get_summary` | `SidebarProvider.tsx:240`；`meeting-details/page.tsx:199`；`useSummaryGeneration.ts:181/208` | 摘要状态/内容轮询 | ✓ |
| `api_process_transcript` | `useSummaryGeneration.ts:364` | 触发摘要生成 | 注册于 `summary::commands`；参数 `text/model/modelName/meetingId/chunkSize/overlap/customPrompt/templateId/summaryLanguage` 未在本次 scope 内复核 |
| `api_cancel_summary` | `useSummaryGeneration.ts:378,551` | 取消摘要 | 同上 |
| `api_save_transcript` | `services/storageService.ts:44` | **会议落库唯一写路径** | ✓ `{meetingTitle, transcripts, folderPath}` |
| `api_save_meeting_summary` | `hooks/meeting-details/useMeetingData.ts:50` | 保存摘要 | ✓ |
| `api_get_meeting_metadata` / `api_get_meeting_transcripts` | `hooks/usePaginatedTranscripts.ts:86`；`useSummaryGeneration.ts:407/421` | 分页读取 | ✓ |
| `api_get_model_config` / `api_save_model_config` / `api_get_api_key` / `api_get_custom_openai_config` | `ConfigContext.tsx:234/306`；`page-content.tsx:100`；`ModelSettingsModal.tsx:266-293` | 模型与密钥 | ✓ |
| `api_get_transcript_config` / `api_save_transcript_config` | `useRecordingStart.ts:73`；`WhisperModelManager.tsx:323`；`ParakeetModelManager.tsx:260` | 转写引擎选择 | ✓ |
| `get_onboarding_status` | `app/layout.tsx:81`；`OnboardingContext.tsx:344` | 首启判断 | ✓ |
| `builtin_ai_*`（8 个） | `lib/builtin-ai.ts:68-96`；`BuiltInModelManager.tsx:53-248` | 内置摘要模型 | ✗ `builtin_ai_get_models_directory` 未注册（`builtin-ai.ts:96`，无调用者） |
| **`api_get_auto_generate_setting`** | `components/ModelSettingsModal.tsx:320` | 读取"自动摘要"开关 | ✗ **`lib.rs:723` 该注册项被注释掉**（见 §3.3） |
| `open_system_settings` | `PermissionWarning.tsx:37/47`；`onboarding/steps/PermissionsStep.tsx:33/65` | 打开 macOS 隐私面板 | ✓ 但注册项带 `#[cfg(target_os = "macos")]`，非 macOS 下该命令不存在（调用点均有 `isMacOS` 守卫） |
| `open_external_url` | `About.tsx:26`；`AnalyticsConsentSwitch.tsx:150`；`ModelSettingsModal.tsx:739/1236` | 打开外链 | ✓ |
| `read_audio_file` | `hooks/useAudioPlayer.ts:76` | 回放音频读取 | ✓ `{ filePath }` ↔ `lib.rs:280` 的 `file_path` |
| `start_audio_level_monitoring` / `stop_audio_level_monitoring` | `DeviceSelection.tsx:237/250` | 电平监测 | ✓ `{ deviceNames }` ↔ `lib.rs:309-312` |
| `track_*` / `init_analytics` / `is_analytics_enabled` 等 25 个 | `lib/analytics.ts:47-847` | 埋点 | ✓（注册于 `analytics::commands`） |

**参数名核对结论**：在 `lib.rs` 可见范围内**未发现第二处参数名不匹配**；前端统一使用 camelCase 传参，与 Tauri v2 默认转换规则一致（唯一例外是 `stop_recording` 的 `args.save_path` 结构体内部字段，两处调用都正确地用了 snake_case）。本轮的契约不匹配全部集中在**命令注册**层面，而非参数层。

### 3.3 注册层不匹配（本轮的第二个、且是"在用"的失配）

| # | 前端调用 | `lib.rs` 注册 | 影响 |
|---|---|---|---|
| A | `builtin-ai.ts:96` `builtin_ai_get_models_directory` | **不存在** | 无调用者 → 潜在缺陷 |
| B | `ModelSettingsModal.tsx:320` `api_get_auto_generate_setting` | `lib.rs:723` 被 `//` 注释 | **每次打开模型设置弹窗都会 `command not found`**，被 `:323-326` 的 `catch` 吞成 `console.error`，UI 静默保留默认值 → 用户看到的"自动摘要"开关状态**不可信** |

B 与 A 同类但严重度更高：它有真实调用点、有真实用户路径、且失败是静默的。旁证：`api_save_auto_generate_setting`（`lib.rs:724`，同样注释）前端完全无调用 → 说明"自动摘要"这条配置链在 Tauri 迁移中**只迁移了一半**，真正生效的是 `ConfigContext.tsx:161-167` 的 `localStorage['isAutoSummary']`（与 `meeting-details/page.tsx:28` 的消费点一致），DB 侧同名设置已成孤儿。

### 3.4 `get_transcription_status`：注册的是桩（Lead 已验证，本轮复核了 `lib.rs` 侧证据）

- `lib.rs:270-277` 注册的 `get_transcription_status` 是**硬编码桩**：`chunks_in_queue: 0`、`is_processing: false`、`last_activity_ms: 0`。
- Lead 已确认真实现位于 `audio/recording_commands.rs:1106-1112`（读取 `IS_RECORDING`），经 `audio/mod.rs:89` re-export，但**从未进入 `generate_handler!`**（本轮未复核该子目录，采信 Lead 结论）。
- 消费链：`services/transcriptService.ts:47` → `hooks/useRecordingStop.ts:166`。

**危害链条**（`useRecordingStop.ts:163-236`）：桩恒为"无处理、队列空"，`if (!status.is_processing && status.chunks_in_queue === 0)` 在**第一次轮询即成立** → `transcriptionComplete = true` → 跳过 `:204-209` 的超时分支、只固定 `await 4000ms` → `:236` 的 `if (isCallApi && transcriptionComplete == true)` 恒真 → `:256` 用"此刻已有的片段"落库。`:158` 的 `listen('transcription-complete')` 抢不过轮询，成为死代码。**结论：停止录音不会等转写队列收尾，会议记录静默缺失尾部片段。**

### 3.5 事件面

前端 `listen` 清单（本轮全量提取，共 22 个事件名 30 处调用点）：

| 事件 | 前端监听点 | 用途 |
|---|---|---|
| `transcript-update` | `services/transcriptService.ts:58` → `TranscriptContext.tsx:288` | 实时转写主通道 |
| `transcription-complete` | `useRecordingStop.ts:158`；`transcriptService.ts:69` | 停止流程等待（§3.4 中实际失效） |
| `transcript-error` | `RecordingControls.tsx:255`；`transcriptService.ts:89` | 旧式错误串 |
| `transcription-error` | `RecordingControls.tsx:277`；`useModalState.ts:136`；`transcriptService.ts:78` | 结构化错误（`actionable` 分支） |
| `recording-started` / `-starting` / `-stopped` / `-paused` / `-resumed` | `recordingService.ts:137/146/155/166/175` | 状态机驱动（`RecordingStateContext.tsx:142-213`） |
| `recording-stop-complete` | `RecordingPostProcessingProvider.tsx:36` | 托盘/快捷键停止后的统一后处理 |
| `recording-stopped`（第二监听点） | `useRecordingStop.ts:89` | 取 `folder_path`/`meeting_name` 存 sessionStorage |
| `speech-detected` | `RecordingControls.tsx:306`；`TranscriptView.tsx:155` | VAD 反馈 |
| `chunk-drop-warning` | `useModalState.ts:109`；`recordingService.ts:184` | 音频丢块告警 |
| `mic-device-switched` / `-unavailable` / `-swap-failed` / `-recovery-exhausted` | `recordingService.ts:204-239` → `RecordingStateContext.tsx:262-313` | 麦克风热插拔（注释说明"其它地方都不监听"，属实） |
| `model-config-updated` | `ConfigContext.tsx:330`；`Sidebar/index.tsx:167`；`SummaryModelSettings.tsx:85`；`useModelConfiguration.ts:92` | **前端自 emit**（`page-content.tsx:110`、`Sidebar/index.tsx:202`） |
| `model-download-complete` / `parakeet-model-download-complete` | `useModalState.ts:173`；`transcriptService.ts:100/111` | 下载完成 |
| `builtin-ai-download-progress` | `BuiltInModelManager.tsx:81` | 内置模型下载 |
| `audio-levels` | `DeviceSelection.tsx:134` | 电平表 |
| `request-recording-toggle` | `app/layout.tsx:111` | 托盘 → 录音开关 |
| `tauri://drag-enter` / `-leave` / `-drop` | `app/layout.tsx:168/180/191` | 拖入音频文件（Beta） |

**Rust 侧单侧事件**：Lead 从 `src-tauri/src` 全量提取 30 个 emit 名，与上述 22 个交叉，得 **14 个无任何前端监听者**：`audio-status`、`database-initialized`、`first-audio-detected`、`first-launch-detected`、`microphone-recovered`、`recording-error`、`recording-saved`、`recording-startup-progress`、`system-audio-started`、`system-audio-stopped`、`transcription-progress`、`transcription-queue-complete`、`transcription-started`、`transcription-warning`。其中 `recording-error`、`transcription-warning`、`recording-saved` 的缺失意味着**错误/完成信号不触达用户**；`transcription-queue-complete` 无监听与 §3.4 相互印证——真正的"队列排空"信号前端拿得到却没人用。

> 限制说明：本轮 scope 不允许读取 `src-tauri/src` 子目录，故 §3.5 的事件名清单采信 Lead 的提取结果，仅与之交叉的前端 22 项为本轮独立实测。

---

## 4. Hooks / Services / 直接 HTTP / 埋点

### 4.1 录音相关 hooks

| Hook | 行数 | 职责 | 观察 |
|---|---|---|---|
| `useRecordingStart.ts` | 412 | 手动 + 自动（sessionStorage `autoStartRecording`）+ 侧栏自定义事件三入口启动 | **三份近乎重复的启动逻辑**（`:119-208`、`:211-306`、`:309-406`），校验/埋点/toast 文案逐段复制；`:281`、`:379` 用阻塞式 `alert()`；`:50` 有正确的同步 latch `isStartingRef` |
| `useRecordingStop.ts` | 470 | 停止后处理：等转写 → flush → 落库 → 跳转 → 埋点 | `stopInProgressRef`（`:73`）是**实例级**守卫；`:440-455` 把 handler 挂到 `window.handleRecordingStop` 供 Rust 回调 |
| `useRecordingStateSync.ts` | 71 | 1 秒轮询 `is_recording` 对齐页面局部 state | `:50` 以 `(window as any).__TAURI__` 判存在性——Tauri 2 默认不注入该全局，条件大概率为假 → **此 hook 的同步逻辑可能整体失效**（不影响主流程，因为 `RecordingStateContext` 另有 500ms 轮询） |
| `RecordingPostProcessingProvider.tsx` | 61 | 全局挂一份 `useRecordingStop`，监听 `recording-stop-complete` | 见 §5.2 双实例问题 |
| `useTranscriptRecovery.ts` | ~210 | 崩溃恢复（IndexedDB + 音频 checkpoint） | `:63` `has_audio_checkpoints`、`:203` `cleanup_checkpoints` |

### 4.2 meeting-details hooks

`useMeetingData.ts`（107）、`useSummaryGeneration.ts`（595）、`useMeetingOperations.ts`、`useModelConfiguration.ts`、`useTemplates.ts`、`useCopyOperations.ts`。
`useSummaryGeneration.ts` 是本目录质量最高的一段代码：`generationIdRef` 世代号（`:106`）+ `visibleMeetingIdRef`（`:104`）+ `mountedRef`（`:103`）三重守卫，取消/重生成/切换会议都有明确分支（`:377-380` 迟到结果会主动 `api_cancel_summary`）。`handleGenerateSummary`（`:459`）前置校验 provider（ollama 有模型 / builtin-ai 就绪）也是到位的。

### 4.3 直接 HTTP / WebSocket

全 `src/**` 检索 `fetch(`、`axios`、`XMLHttpRequest`、`new WebSocket(`、`EventSource(`、`http://`：

- **零处直接 HTTP 调用**（唯一命中是 `hooks/usePaginatedTranscripts.ts:192` 的 `refetch()` 字面误匹配）。
- 出现的 `http://` 全部是 (a) SVG `xmlns` 声明、(b) 外链常量、(c) 表单占位符、(d) `SidebarProvider.tsx:121-122` 的两处死地址。
- **结论：前端数据面 100% 走 Tauri IPC**，与 CLAUDE.md "不再需要独立 FastAPI 层"一致；但 `SidebarProvider.tsx:121-122` 的硬编码地址（Lead：仍在 `tauri.conf.json:30` 的 CSP `connect-src` 白名单内）属于迁移残留，会误导后来者以为存在 HTTP 层。

### 4.4 埋点

- 唯一入口 `lib/analytics.ts`（~850 行），全部经 `invoke('track_event' | 'track_*')` 转发给 Rust，前端不直接出网。
- 默认**关闭**且带一次性迁移：`AnalyticsProvider.tsx:34-53`（`analyticsOptedIn` 默认 false + `analyticsDefaultOffMigrationV1` 强制回写 false），符合"隐私优先"定位；`AnalyticsConsentSwitch.tsx` 提供开关与数据说明弹窗（`AnalyticsDataModal.tsx`）。
- 覆盖面很宽：启动/会话（`analytics.ts:115-160`）、录音起停、会议删除、设置变更、摘要生成/重生成/模型切换/自定义 prompt（`:788-847`），以及 UI 级 `trackButtonClick`（`RecordingControls.tsx:388/444`、`SidebarProvider.tsx:172`、`useRecordingStart.ts:137-382`）。埋点调用点密度高于测试覆盖密度，属本项目"可观测性不平衡"的一个侧影。
- 缺陷：`AnalyticsProvider.tsx:62-137` 内注册的 `beforeunload` 监听器（`:126`）不在任何 effect cleanup 内（`initAnalytics2` 的返回值被丢弃，`:58`）→ 监听器泄漏，且会话结束上报只在页面卸载路径上碰运气。

---

## 5. 具体工程缺陷

### 5.1 【P0】停止录音不等转写收尾 → 静默数据缺失
同 §3.4。要点：`lib.rs:270-277` 桩 → `useRecordingStop.ts:164-181` 首次轮询即判完成 → `:236` 条件恒真 → `:256` 以当时的 `transcriptsRef.current` 落库。修复面极小（把 `lib.rs:270-277` 删掉，注册真实现），收益极大。

**附带的数据丢失路径**（本轮补充）：落库**完全依赖前端**在进程内持有的 `transcriptsRef`（`useRecordingStop.ts:241`）与 sessionStorage 中的 `folder_path/meeting_name`（`:244-245`，来源是 `:96-99` 的 `recording-stopped` 事件）。若用户在"停止 → 落库"窗口内关闭应用/崩溃：
- `sessionStorage` 随进程消失；
- 落库前的片段只存在于 IndexedDB 旁路（`TranscriptContext.tsx:326`，且为 fire-and-forget `.catch(console.warn)` 不重试）；
- 恢复完全依赖 `useTranscriptRecovery` + checkpoint（`app/page.tsx:70-106`）。
即"会议是否被持久化"由**一个前端事件 + 4 秒固定等待**决定，而非由 Rust 侧的音频/转写完成状态决定。

### 5.2 【P1】`useRecordingStop` 被实例化两次，去重守卫是实例级的 → 重复落库风险
- 实例 A：`app/page.tsx:46`（主页按钮路径）。
- 实例 B：`RecordingPostProcessingProvider.tsx:28`（`layout.tsx:245` 全局挂载，主页同样存在）。
- 守卫：`useRecordingStop.ts:73` 的 `stopInProgressRef` 是 `useRef` → **每个实例各一把锁**。
- 结果：一次停止可能由"主页按钮回调"（`RecordingControls.tsx:162`）与"`recording-stop-complete` 事件"（`RecordingPostProcessingProvider.tsx:36-41`）两条路同时进入 `handleRecordingStop(true)`，两边都执行 `storageService.saveMeeting`（`:256`）。是否产生重复会议取决于 Rust `api_save_transcript` 是否有幂等键；前端**没有**任何跨实例互斥。
- 同时每个实例都会在停止时临时 `listen('transcription-complete')`（`:158`）与常驻 `listen('recording-stopped')`（`:89`），日志噪音翻倍。

### 5.3 【P1】监听器泄漏（勘误 Lead 的"无泄漏"结论）
| 位置 | 问题 |
|---|---|
| `hooks/useModalState.ts:168-191` | `setupDownloadListeners` 是 async，其**返回的 cleanup 被丢弃**（effect 未 `return`）→ `model-download-complete`（`:173`）永不 `unlisten`；且依赖 `[transcriptModelConfig, hideModal]`，模型配置每变一次就再注册一个 → 监听器按配置变更次数线性增长 |
| `components/AnalyticsProvider.tsx:62-137` | `initAnalytics2` 内 `window.addEventListener('beforeunload', …)`（`:126`）不在 effect cleanup 中，返回值被丢弃（`:58`） |
| 对照（Lead 已验证、本轮采信） | `RecordingControls.tsx:311-331`、`TranscriptView.tsx:154-155`、`RecordingStateContext.tsx:251-329`（含 `cancelled` 竞态守卫）、`app/layout.tsx:164-208`（含 `cleanedUpRef`）**清理正确** |

结论应表述为："**大部分**监听器清理正确，但 `useModalState` 与 `AnalyticsProvider` 各有一处确定性泄漏"，而非"不存在泄漏"。

### 5.4 状态更新竞态 / effect 依赖
- `TranscriptContext.tsx:85-178` 的 `useEffect` 依赖 `[currentMeetingId]`，但回调里读 `currentMeetingId`（`:148`）：每次 `currentMeetingId` 变化都会**注销并重注册两个监听器**，且在 `recording-stopped` 与 `currentMeetingId` 更新的竞态窗口内，用的是闭包里的旧值 → `:150-155` 的 folderPath 回写可能落空。
- `TranscriptContext.tsx:363-405`：`[recordingState.isRecording]` 依赖 + 读 `transcripts.length === 0`（`:366`）但未列入依赖 → 属于故意的"只在状态翻转时跑一次"，可接受但易被后续修改破坏。
- `app/page.tsx:106` 依赖含 `status`（状态机全量），恢复检查会在每个状态跃迁时重跑。
- `RecordingStateContext.tsx:75-83` 的 `setStatus` 把 `state.status/isRecording/isPaused` 放进 deps 只为打日志（`:76`），导致**每次状态/录音位变化都重建 `setStatus`**，进而击穿 `contextValue` 的 `useMemo`（`:341-348`）→ 所有 `useRecordingState()` 消费者被动重渲染。日志不该成为依赖。
- `useRecordingStop.ts:423-438` 依赖列表含 `meetings`（`:435`）却未在函数体使用 → 只要侧栏会议列表变化，`handleRecordingStop` 身份即变，`RecordingPostProcessingProvider.tsx:58` 的 effect 随之重注册监听器。

### 5.5 未处理的 Promise 拒绝 / 阻塞式 UI
- `TranscriptContext.tsx:470`：`navigator.clipboard.writeText(fullTranscript)` **未 await、未 catch**，随后无条件 `toast.success("Transcript copied to clipboard")`（`:472`）→ 剪贴板被拒（无权限/非聚焦窗口）时既有未处理拒绝，又给用户**假成功**。
- `useRecordingStop.ts:447-449`：`(window as any).handleRecordingStop` 直接调用 async 函数且不接 promise → Rust 侧回调路径上的失败无人接收。
- `AnalyticsProvider.tsx:139`：`initAnalytics().catch(console.error)` 只留日志。
- 阻塞式 `alert()`（阻断事件循环，Tauri webview 主线程）：`RecordingControls.tsx:82,218,236`、`useRecordingStart.ts:281,379`、`TranscriptContext.tsx:341`、`TranscriptRecovery.tsx:89,109`、`PermissionsStep.tsx:35,67`。其中 `TranscriptContext.tsx:341` 在**转写监听器装配失败**时弹 alert——这是核心链路错误，用 alert 而非 `toast`/持久错误 UI，且会遮挡录音界面。
- 轮询叠加：500ms（录音状态）+ 1000ms（`useRecordingStateSync`）+ 5000ms（摘要）+ 停止时 500ms（转写状态，实际一次即退）+ `app/page.tsx:174` 300ms 波形随机数（纯装饰性定时器，`setBarHeights` 每 300ms 造成一次重渲染）。

### 5.6 无障碍（a11y）
- `aria-label` 仅 14 处 vs `<button>` 160 处（Lead 另计 165 处 `onClick` 无键盘等价物、`role` 11、`tabIndex` 4）。
- 主控件的可访问名缺失：`RecordingControls.tsx:386-400`（开始）、`:411-433`（暂停/继续）、`:442-457`（停止）三个按钮**只含图标**（`<Mic>/<Pause>/<Play>/<Square>`），可访问名来自 Radix `TooltipContent`（`:402/435/459`）的 `aria-describedby`，而非可靠的 label；`role`/`aria-pressed` 也未设置 → 屏幕阅读器只能播报"button"，且**无法区分暂停与继续是同一个按钮的状态切换**。
- `RecordingControls.tsx:340` 的"Processing recording..." spinner 无 `role="status"`/`aria-live` → 状态变化不播报。
- 侧栏/设备选择等自研可点区域依赖鼠标（Lead §4.5）。
- 正面项：`RecordingControls.tsx:499` 的关闭按钮写了 `aria-label="Close alert"`；`EmptyStateSummary` 有 `role="alert"`（`tests/lib/empty-state-summary.test.tsx:15` 断言）。

### 5.7 类型与规模
- `tsconfig.json:7` `"strict": true`，`@ts-ignore/@ts-expect-error/@ts-nocheck` 计数 0（Lead 验证）；但 25 处 `as any` / 28 处 `: any`，集中在后端返回结构（`SidebarProvider.tsx:84,101`、`meeting-details/page.tsx:56,86`、`ModelSettingsModal.tsx:266,510`）。
- 巨型组件：`ModelSettingsModal.tsx` 1287、`Sidebar/index.tsx` 783、`AISummary/index.tsx` 758、`WhisperModelManager.tsx` 729、`ParakeetModelManager.tsx` 629（单个组件混合表单/校验/下载/provider 分支/事件同步）。
- `hooks/useRecordingStart.ts` 三份重复启动逻辑（§4.1）是"同一语义三处实现"的典型：任何一处修 bug 都极易漏改另外两处（如 `:185`/`:272`/`:370` 三处 `'already in progress'` 分支文案与埋点完全平行）。

### 5.8 代码自述与实现的分歧（CLAUDE.md / 注释）
| 位置 | 自述 | 实现 |
|---|---|---|
| `CLAUDE.md`（Frontend State Management） | "Sidebar Context：Global state for **recording status**" | 录音状态实际在 `RecordingStateContext`（`RecordingStateContext.tsx:17-49`）；`SidebarProvider.tsx:91` 只是**读**它 |
| `RecordingStateContext.tsx:9` 注释 | "single source of truth for recording state" | `app/page.tsx:26` 另有局部 `isRecording` 并驱动控制流（§2.1） |
| `CLAUDE.md`（Frontend Performance） | "React state updates batched via **Sidebar context**"、"Transcript rendering **virtualized**"、"Audio level monitoring throttled to 60fps" | 状态分散在 7 个 Context；分页/虚拟化仅会议详情页启用（`page-content.tsx:211` `usePagination`，主页 `TranscriptPanel` 仍走 `TranscriptView`），`VirtualizedTranscriptView.tsx` 非主路径；电平节流在 Rust 侧 |
| `CLAUDE.md`（Essential Commands） | `pnpm run dev` → Next.js dev server（port 3118） | ✓ 一致（`package.json` `dev`） |
| `package.json:5` | `"main": "electron/main.js"` | 仓库无 Electron 目录，Tauri 2 项目 → 陈旧字段 |
| `RecordingControls.tsx:45` 注释 | "Use global recording state context for pause state (syncs with tray operations)" | 属实（`RecordingStateContext.tsx:194-213`） |
| `useRecordingStateSync.ts:10-13` 注释 | "Polls backend every 1 second to detect recording state changes" | 逻辑外层有 `(window as any).__TAURI__` 门槛（`:50`），Tauri 2 下大概率为假 → 文档描述的轮询很可能根本没启动 |
| `RecordingControls.tsx:233`（调用点）/ `page.tsx:233` | `onTranscriptReceived={() => {}} // Not actually used by RecordingControls` | 属实，但**保留了死 prop**（`SummaryResponse` 类型仍被 import，`RecordingControls.tsx:7`） |
| `next.config.js:8` | `reactStrictMode: false, // Disabled for BlockNote compatibility` | 与 `layout.tsx:36-38` 等处的"HMR/StrictMode 双挂载"防御注释相互印证，但代价是全局失去 StrictMode 检查 |

---

## 6. 测试现实

### 6.1 事实（本轮更正 Lead 与任务简报）

- `frontend/tests/` 有 **10 个测试文件**：`hooks/` 3 个（meeting-details-refresh 173 行、paginated-transcripts 158、summary-generation 273）、`lib/` 7 个（analytics 88、blocknote-markdown 72、empty-state-summary 19、onboarding-summary-model 94、summary-content 32、summary-language-preferences 130、transcription-model-readiness 35）。
- **运行器是 `bun:test`，不是 jest/vitest**：8 个 `.ts/.tsx/.js` 文件首行均 `import { … } from 'bun:test'`（如 `tests/lib/summary-content.test.ts:1`），`package.json:103` 有 `"@types/bun": "1.3.14"`；`tests/lib/onboarding-summary-model.test.mjs` 则用 `node:test` 风格的手写 `assert` + `typescript.transpileModule` 加载 TS（`:1-20`）。断言库为 Bun 自带 `expect`，另有 `react-test-renderer@18.3.1` + `@types/react-test-renderer` 用于组件测试。
- 因此简报中"没有 jest/vitest 依赖 → 测试无法运行"的推论**不成立**：这些测试在装有 Bun 的机器上可用 `bun test tests/` 直接运行。

### 6.2 但实际状态等同于"不存在"

- `package.json:6-29` 脚本区只列 `dev/build/export/start/tauri*/lint`，**没有 `test` 脚本** → `pnpm test` 不存在，团队无法用项目约定的包管理器运行。
- 全 `.github/workflows/*.yml` 检索 `bun`：**零命中**（仅 `ubuntu-*` runner 的字符串噪声命中了 grep）→ **这 10 个文件从未在 CI 执行过**；唯一质量相关脚本 `lint` 也不在任何 workflow 中被调用。
- 覆盖面对照：10 个文件 ≈ 1,000 行测试 vs 27,813 行实现（≈3.6%），且测试集中在纯函数（`summary-content`、`blocknote-markdown`、`transcription-model-readiness`）与 3 个 hook；`useRecordingStop`（§5.1/§5.2 的 P0/P1 所在）、`RecordingControls`、`SidebarProvider`、`TranscriptContext` 缓冲排序逻辑**全部无测试**。
- 结构性隐患（若将来接上 Bun）：`tests/hooks/summary-generation.test.tsx:7-13` 自己注明 "Bun shares module mocks between test files"，说明这些用例依赖 `mock.module` 的全局副作用与 `afterAll` 手工还原，**顺序耦合**，并行/分片运行会互相污染。

---

## 7. 风险排序

| # | 风险 | 证据 | 严重度 |
|---|---|---|---|
| 1 | **停止录音不等转写收尾 → 会议记录静默缺失尾部**（`get_transcription_status` 桩使轮询首次即判完成） | `lib.rs:270-277`（桩）vs Lead：`audio/recording_commands.rs:1106`（从未注册）；`useRecordingStop.ts:164-181,236,256` | **P0** |
| 2 | **`api_get_auto_generate_setting` 注册被注释 → 模型设置弹窗每次打开都命令失败且静默**；"自动摘要"DB 配置链只迁移了一半 | `ModelSettingsModal.tsx:320` vs `lib.rs:723-724`；实际生效值来自 `ConfigContext.tsx:161-167` 的 localStorage | **P0**（用户可见的配置不可信） |
| 3 | **`useRecordingStop` 双实例 + 实例级守卫 → 停止可能重复落库**（主页按钮路径与 `recording-stop-complete` 路径并发） | `app/page.tsx:46`；`RecordingPostProcessingProvider.tsx:28,36`；`useRecordingStop.ts:73,256` | **P1** |
| 4 | **会议落库完全由前端进程内状态 + 4 秒固定等待决定**，崩溃/关窗即丢尾部；IndexedDB 旁路为 fire-and-forget 不重试 | `useRecordingStop.ts:241,244-260`；`TranscriptContext.tsx:326`；`app/page.tsx:70-106` | **P1** |
| 5 | **14 个 Rust 事件无前端监听**，含 `recording-error`/`transcription-warning`/`recording-saved` → 错误与完成信号不触达用户 | Lead 的 Rust emit 清单 vs §3.5 前端 22 事件（`transcription-queue-complete` 与 #1 互证） | **P1** |
| 6 | **监听器泄漏 ≥2 处**：`useModalState.ts:168-191`（cleanup 被丢弃 + 随配置变更线性增长）、`AnalyticsProvider.tsx:126`（beforeunload 未清理） | 同左；对照 `RecordingControls.tsx:324-331` 的正确写法 | **P1** |
| 7 | 未处理拒绝 + 假成功：`navigator.clipboard.writeText` 未 await/catch 却无条件报成功 | `TranscriptContext.tsx:470-472` | **P2** |
| 8 | 可访问性：160 个 `<button>` 仅 14 个 `aria-label`；录音主控件为纯图标按钮且无 `aria-pressed`/`aria-live` | `RecordingControls.tsx:386-462`；计数 | **P2** |
| 9 | `builtin_ai_get_models_directory` 调用未注册命令（当前无调用者，潜在） | `src/lib/builtin-ai.ts:96` vs `lib.rs:757-764` 注册区 | **P2**（潜在） |
| 10 | 611 处 `console.*`（含 emoji 调试输出）留在生产代码；`alert()` 8 处阻断 UI | 计数；`RecordingControls.tsx:82,218,236`；`useRecordingStart.ts:281,379`；`TranscriptContext.tsx:341` | **P2** |
| 11 | 硬编码已归档服务的 `localhost:5167`/`127.0.0.1:8178`（`transcriptServerAddress` 零消费者），且仍在 CSP 白名单 | `SidebarProvider.tsx:121-122`；Lead：`tauri.conf.json:30`、`api/api.rs:20` | **P2** |
| 12 | **10 个 `bun:test` 测试无 `test` 脚本、CI 从不执行**（`lint` 亦从不执行）；mock 全局共享导致顺序耦合 | `package.json:6-33,103`；`tests/**` 首行；`.github/workflows/*` 无 `bun` | **P2** |
| 13 | `setStatus` 把日志用的 `state.*` 放进 deps → 击穿 `contextValue` memo，全量消费者重渲染；`handleRecordingStop` 依赖无关的 `meetings` 导致全局监听器重注册 | `RecordingStateContext.tsx:75-83,341-348`；`useRecordingStop.ts:435` | **P3** |
| 14 | 三份重复的录音启动逻辑（412 行 hook 内）+ 巨型组件阻碍演进 | `useRecordingStart.ts:119-208,211-306,309-406`；1287/783/758/729 行 | **P3** |

---

## 摘要要点

- 前端 171 个 TS/TSX、27,813 行、611 处 `console.*`；4 个 App Router 页面、7 个 Context、24 个 hooks、6 个 service；数据面 100% 走 Tauri IPC，**零处直接 HTTP/WebSocket**。
- **P0-1**：`lib.rs:270-277` 的 `get_transcription_status` 是恒返回 `is_processing:false, chunks_in_queue:0` 的桩（真实现从未注册），`useRecordingStop.ts:164-181` 的"等转写完成"轮询**第一次即命中**，只等 4 秒就 flush，`:236` 落库条件恒真 → 会议记录静默缺尾部。
- **P0-2（本轮新增，与已知失配同类但在用）**：`ModelSettingsModal.tsx:320` 调用 `api_get_auto_generate_setting`，而该注册项在 `lib.rs:723` 被注释掉 → 每次打开模型设置弹窗都 `command not found`，被 `:323-326` 吞掉；真正生效的"自动摘要"是 `ConfigContext.tsx:161-167` 的 localStorage。
- **P1-1（本轮新增）**：`useRecordingStop` 被实例化两次（`app/page.tsx:46` 与 `RecordingPostProcessingProvider.tsx:28`），去重守卫 `stopInProgressRef`（`useRecordingStop.ts:73`）是实例级的 → 主页按钮路径与 `recording-stop-complete` 路径可并发调用 `saveMeeting`（`:256`），前端无跨实例互斥。
- **P1-2（勘误）**：监听器并非"无泄漏"——`useModalState.ts:168-191` 的 async cleanup 被丢弃且随 `transcriptModelConfig` 变化线性增长，`AnalyticsProvider.tsx:126` 的 `beforeunload` 未清理；`RecordingControls.tsx:324-331`、`TranscriptView.tsx:154-155`、`RecordingStateContext.tsx:251-329` 三处确属正确。
- **命令契约**：本轮实测 144 个去重字面命令名（Lead 计 139，口径差异）；注册层失配共 2 处——`builtin_ai_get_models_directory`（`lib/builtin-ai.ts:96`，无调用者）与 `api_get_auto_generate_setting`（在用）；`lib.rs` 可见范围内**未发现参数名不匹配**（前端统一 camelCase，`stop_recording` 的 `args.save_path` 两处调用均正确）。
- **事件契约**：前端 22 个事件名 / 30 处 `listen`；Lead 提取的 30 个 Rust emit 中 **14 个前端零监听**（含 `recording-error`、`transcription-warning`、`recording-saved`、`transcription-queue-complete`），后者与 P0-1 互为印证。
- **状态管理**：`isRecording` 存在三份（`page.tsx:26` 局部 / `RecordingStateContext:62` / `TranscriptContext:366` 间接），靠 500ms+1000ms 双轮询收敛；`transcriptServerAddress` 零消费者；模型配置在 `ConfigContext`/`Sidebar`/`ModelSettingsModal` 各持一份，仅靠前端自 emit 的 `model-config-updated` 最终一致。
- **测试现实（勘误）**：10 个测试文件用的是 **`bun:test`**（`package.json:103` 有 `@types/bun`），装有 Bun 即可 `bun test` 运行；真正的问题是 `package.json` 无 `test` 脚本、**CI 全 workflow 无 `bun` 命中**、`lint` 亦从不执行 → 等同于从未运行；且用例依赖 `mock.module` 全局共享（`tests/hooks/summary-generation.test.tsx:7-13`）存在顺序耦合。
- **其它缺陷**：`navigator.clipboard.writeText` 未 await 却报成功（`TranscriptContext.tsx:470-472`）；`setStatus` 把日志依赖塞进 deps 击穿 context memo（`RecordingStateContext.tsx:75-83`）；`useRecordingStateSync.ts:50` 用 `window.__TAURI__` 判存在性在 Tauri 2 下大概率为假；`useRecordingStart.ts` 三份重复启动逻辑 + 8 处阻塞式 `alert()`。
- **a11y**：160 个 `<button>` 仅 14 个 `aria-label`；录音主控件（`RecordingControls.tsx:386-462`）为纯图标按钮、无 `aria-pressed`，处理中 spinner 无 `aria-live`。
- **代码自述分歧**：CLAUDE.md 称 SidebarProvider 持有 recording status（实为 `RecordingStateContext`）、称状态经 Sidebar context 批量更新（实为 7 个 Context 分散）、称转写已虚拟化（仅详情页分页路径）；`package.json:5` 的 `"main": "electron/main.js"` 为陈旧字段。
