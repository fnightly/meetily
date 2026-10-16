# 06 — 前端技术分析（Next.js 14 + React 18 + TypeScript + Tauri IPC）

> **本文件说明**：这是一份独立完成的完整分析，覆盖 `frontend/src/**`、`frontend` 根配置文件、`frontend/tests/**`，并以 `frontend/src-tauri/src/lib.rs` 作为命令/事件注册面的唯一参照。
> 写作前该路径已存在一版由协调者撰写的报告（内容不同），已备份至 `analysis/06-frontend.coordinator-snapshot.md`，未删除。
> 所有 `path:line` 均以仓库根 `D:\Meetly\` 为基准；所有计数均为本轮实测（口径见各表）。**未修改任何既有源文件。**

---

## 1. 应用结构

### 1.1 Next.js 14 App Router 布局

| 项目 | 值 | 证据 |
| --- | --- | --- |
| 路由模式 | App Router，页面与 layout 均为 `'use client'` | `frontend/src/app/layout.tsx:1`、`frontend/src/app/page.tsx:1` |
| 静态导出 | `output: 'export'` + `images.unoptimized` | `frontend/next.config.js:9-12` |
| React StrictMode | **关闭**（注释："Disabled for BlockNote compatibility"） | `frontend/next.config.js:8` |
| Dev 端口 | 3118 | `frontend/package.json:7` |
| 路径别名 | `@/*` → `./src/*` | `frontend/tsconfig.json:21-23` |
| ProseMirror 单实例 | webpack `resolve.alias` 强制 13 个 prosemirror 包解析到 `@tiptap/pm` 下的同一份 | `frontend/next.config.js:27-45` |

关闭 `reactStrictMode` 的代价值得单独指出：StrictMode 的双挂载/双调用会提前暴露「effect 重复注册且 cleanup 不完整」这类缺陷（本项目 §5.4 正是此类），关闭后这些缺陷可以长期存活到生产。`layout.tsx:36-38` 与 `RecordingStateContext.tsx:252-253` 的注释都显示作者清楚双挂载问题，却选择了全局关闭而非局部规避。

**页面清单（4 个 `page.tsx`）**

| 路由 | 文件 | 行数 | 职责 |
| --- | --- | --- | --- |
| `/` | `frontend/src/app/page.tsx` | 259 | 录音主界面：录音条、实时转录面板、恢复对话框、状态遮罩 |
| `/meeting-details` | `frontend/src/app/meeting-details/page.tsx` + `page-content.tsx` | 307 + 258 | 单会议详情（`?id=` 查询参数），含摘要自动生成判定 |
| `/settings` | `frontend/src/app/settings/page.tsx` | 135 | 5 个 Tab：General / Recordings / Transcription / Summary / Beta |
| `/notes/[id]` | `frontend/src/app/notes/[id]/page.tsx` | 189 | **纯静态假数据占位页**，与 SQLite 完全无关 |

`/notes/[id]` 的假数据性质很重要：`frontend/src/app/notes/[id]/page.tsx:19-27` 用 `generateStaticParams` 硬编码 4 个 id，`:31-127` 硬编码 4 段会议内容，`:174-183` 用字符串拼接 + `dangerouslySetInnerHTML` 渲染 Markdown。而 `frontend/src/components/Sidebar/index.tsx:581-583` 真的会路由到 `/notes/${item.id}`（当 id 不含 `-` 时），即存在一条通往假数据的真实入口。

### 1.2 规模（本轮实测）

| 指标 | 数值 | 口径 |
| --- | --- | --- |
| `src/` 下 TS/TSX 文件 | 171 | `Get-ChildItem -Recurse -Include *.ts,*.tsx` |
| `src/` 总行数 | 30,994 | `(Get-Content).Count`，含空行 |
| `components/` 文件 | 104 | — |
| `contexts/` 文件 | 7 | — |
| `hooks/` 文件 | 23（含 `meeting-details/` 6） | — |
| `services/` 文件 | 6 | — |
| `tests/` 文件 | 10（1,074 行） | — |
| `console.log/error/warn/info` | **657 处** | 全 `src` 检索 |

**最大文件排行**：`ModelSettingsModal.tsx` 1287 · `Sidebar/index.tsx` 882 · `lib/analytics.ts` 857 · `AISummary/index.tsx` 758 · `WhisperModelManager.tsx` 729 · `OnboardingContext.tsx` 652 · `ParakeetModelManager.tsx` 629 · `useSummaryGeneration.ts` 595 · `ConfigContext.tsx` 554 · `TranscriptContext.tsx` 539 · `RecordingControls.tsx` 524。

（注意：`Get-Content | Measure-Object -Line` 会**跳过空行**，用它统计会低估约 10%；上表用 `.Count`。同一文件两种口径差异示例：`Sidebar/index.tsx` 真实的 882 行会被 `-Line` 报成 783。）

### 1.3 组件清单（按功能分组）

**A. 录音与音频设备**

- `RecordingControls.tsx`（524 行；start/pause/resume/stop 主控件 + 3 个 Tauri 事件监听）
- `AudioLevelMeter.tsx`（导出 `AudioLevelMeter`、`CompactAudioLevelMeter`）
- `AudioBackendSelector.tsx`、`DeviceSelection.tsx`（导出 `SelectedDevices`、`AudioDevice`、`AudioLevelUpdate`）
- `RecordingSettings.tsx`、`RecordingStatusBar.tsx`
- `PermissionWarning.tsx`、`ComplianceNotification.tsx`
- **死文件（0 字节）**：`AudioPlayer.tsx`；**死组件**：`BluetoothPlaybackWarning.tsx`（零引用）

**B. 转录显示**

- `TranscriptView.tsx`（337 行，非虚拟化旧版）、`VirtualizedTranscriptView.tsx`（394 行，现役）
- `ConfidenceIndicator.tsx`、`LanguageSelection.tsx`、`LanguagePickerPopover.tsx`
- `ChunkProgressDisplay.tsx`（导出 `ChunkProgressDisplay`、`ChunkProgressMini`，均零引用）
- `TranscriptRecovery/TranscriptRecovery.tsx`

**C. 摘要与编辑器**

- `AISummary/index.tsx`（758 行：块级选择、拖选、上下文菜单、undo/redo、导出 Markdown）
- `AISummary/Section.tsx`(124)、`AISummary/Block.tsx`(267)、`AISummary/BlockNoteSummaryView.tsx`(245)
- `BlockNoteEditor/Editor.tsx`（被 `BlockNoteSummaryView.tsx:14` 经 `next/dynamic` 且 `ssr:false` 加载）；`BlockNoteEditor/BasicBlockNoteTest.tsx` **零引用**
- `EditableTitle.tsx`、`EmptyStateSummary.tsx`、`SummaryLanguageSettings.tsx`

**D. 设置与模型管理**

- `ModelSettingsModal.tsx`（**1287 行，全仓最大**）
- `WhisperModelManager.tsx`（导出名为 `ModelManager`）、`ParakeetModelManager.tsx`、`BuiltInModelManager.tsx`
- `TranscriptSettings.tsx`、`SummaryModelSettings.tsx`、`PreferenceSettings.tsx`、`BetaSettings.tsx`
- `SettingTabs.tsx`、`AnalyticsConsentSwitch.tsx`、`AnalyticsDataModal.tsx`

**E. 侧边栏 / 导航 / 布局**

- `Sidebar/index.tsx`(882)、`Sidebar/SidebarProvider.tsx`(324)
- `MainContent/index.tsx`、`Logo.tsx`、`Info.tsx`、`About.tsx`；`MainNav/index.tsx`(19) **零引用**

**F. 导入 / 数据库迁移 / 更新 / 引导**

- `ImportAudio/{ImportAudioDialog,ImportDropOverlay}.tsx`
- `DatabaseImport/{LegacyDatabaseImport,HomebrewDatabaseDetector}.tsx` — **两者共同孤立**（后者仅被前者引用）
- `UpdateCheckProvider.tsx`、`UpdateDialog.tsx`、`UpdateNotification.tsx`
- `onboarding/**`（`OnboardingFlow`、`OnboardingContainer`、4 个 step、3 个 shared 组件）

**G. shadcn/ui 基础件**：`components/ui/**` 22 个文件 + `components/molecules/form-components/*` 三个表单件（**全部零引用**）。

### 1.4 录音 → 转录 → 摘要 的组件树

```
app/layout.tsx (RootLayout, 'use client')
├─ AnalyticsProvider                            layout.tsx:236
├─ RecordingStateProvider                       layout.tsx:237   ← 录音状态机（设计上的真源）
├─ TranscriptProvider                           layout.tsx:238   ← 转录缓冲/去重/排序
├─ ConfigProvider                               layout.tsx:239   ← 模型/provider/设备/语言
├─ OllamaDownloadProvider                       layout.tsx:240
├─ OnboardingProvider                           layout.tsx:241
├─ UpdateCheckProvider                          layout.tsx:242
├─ SidebarProvider                              layout.tsx:243   ← 会议列表/当前会议/摘要轮询
├─ TooltipProvider                              layout.tsx:244
├─ RecordingPostProcessingProvider              layout.tsx:245   ← 全局再挂一份 useRecordingStop
└─ ImportDialogProvider                         layout.tsx:246
    ├─ OnboardingFlow                (未完成引导)          layout.tsx:252
    ├─ <div class="flex">
    │   ├─ Sidebar                                        layout.tsx:255
    │   └─ MainContent → {children}                       layout.tsx:256
    │       ├─ app/page.tsx (:24)  ── 录音主页
    │       │   ├─ SettingsModals                         :194
    │       │   ├─ TranscriptRecovery                     :201
    │       │   ├─ TranscriptPanel (app/_components)      :210
    │       │   │   └─ VirtualizedTranscriptView    _components/TranscriptPanel.tsx:108
    │       │   │       ├─ RecordingStatusBar       VirtualizedTranscriptView.tsx:232
    │       │   │       ├─ ConfidenceIndicator      VirtualizedTranscriptView.tsx:95
    │       │   │       └─ useTranscriptStreaming   :161
    │       │   ├─ RecordingControls (fixed bottom)       :229
    │       │   └─ StatusOverlays                         :251
    │       └─ app/meeting-details/page.tsx (:297, Suspense)
    │           └─ MeetingDetailsContent (:23)
    │               └─ PageContent (key=meetingId)  page.tsx:273
    │                   └─ MeetingDetailsSplitView  page-content.tsx:196
    │                       ├─ [transcript] MeetingDetails/TranscriptPanel  :203
    │                       │   └─ VirtualizedTranscriptView  :83
    │                       │       └─ TranscriptButtonGroup  :71
    │                       │           └─ RetranscribeDialog  :90
    │                       └─ [summary] MeetingDetails/SummaryPanel  :224
    │                           ├─ SummaryGeneratorButtonGroup  :7
    │                           │   └─ ModelSettingsModal (dialog)
    │                           ├─ SummaryUpdaterButtonGroup
    │                           ├─ BlockNoteSummaryView
    │                           │   └─ dynamic(BlockNoteEditor/Editor)
    │                           └─ EmptyStateSummary
    └─ ImportDropOverlay / ConditionalImportDialog        layout.tsx:260-265
```

三段流水线（均在 `/`）：

1. **录音控制**：`app/page.tsx:229` `<RecordingControls>` → `useRecordingStart`（`frontend/src/hooks/useRecordingStart.ts`）→ `recordingService.startRecordingWithDevices`（`frontend/src/services/recordingService.ts:95`）→ `invoke('start_recording_with_devices_and_meeting')`。
2. **转录**：Rust `transcript-update` → `frontend/src/services/transcriptService.ts:58` → `frontend/src/contexts/TranscriptContext.tsx:288` 主监听器 → 按 `sequence_id` 缓冲/去重、按 `chunk_start_time` 排序（`:181-359`）→ `setTranscripts`。
3. **收尾与摘要**：`RecordingControls.stopRecordingAction`（`frontend/src/components/RecordingControls.tsx:142-186`）先 `invoke('stop_recording')`，再 `onRecordingStop(true)` → `useRecordingStop`（`frontend/src/hooks/useRecordingStop.ts`）→ `storageService.saveMeeting`（`frontend/src/services/storageService.ts:44`）→ `router.push('/meeting-details?id=…&source=recording')` → `useSummaryGeneration`（`frontend/src/hooks/meeting-details/useSummaryGeneration.ts:364`）`invoke('api_process_transcript')` + `SidebarProvider.startSummaryPolling`（`frontend/src/components/Sidebar/SidebarProvider.tsx:208-288`，5 秒间隔、上限 200 次）。

---

## 2. 状态管理

需求点名 4 个 Context，仓库实际有 7 个（另有 `RecordingStateContext`、`RecordingPostProcessingProvider`、`ImportDialogContext`、`OllamaDownloadContext`）。

### 2.1 SidebarProvider（`frontend/src/components/Sidebar/SidebarProvider.tsx`，324 行）

拥有：`currentMeeting`、`isCollapsed`、`meetings`、`sidebarItems`、`isMeetingActive`、`searchResults`、`isSearching`、`serverAddress`、`transcriptServerAddress`，以及 `summaryPollsRef: Map<meetingId, SummaryPoll>`（`:88`）。

- `fetchMeetings` **被 `serverAddress` 门控**：`:98` `if (serverAddress)` 才发 `api_get_meetings`。而 `serverAddress` 由 `:119-125` 硬编码为 `'http://localhost:5167'`（`:121`），`transcriptServerAddress` 为 `'http://127.0.0.1:8178/stream'`（`:122`）。
- `sidebarItems` 由两个 effect 分别写入同一 state：`:144-149`（依赖 `[pathname]`）与 `:152-154`（依赖 `[meetings]`），都执行 `setSidebarItems(baseItems)`，而 `baseItems` 在渲染体内重建（`:127`）。重复写入 + 潜在旧值覆盖。
- `summaryPollsRef` 轮询实现质量高：`inFlight` 重入保护（`:218`）、`processId` 一致性校验（`:242`）、`maxPolls=200` 超时（`:215`、`:224-238`）、卸载清 `clearInterval`（`:290-293`）。
- `searchResults` 类型是 `any[]`（`:84`），消费端 `frontend/src/components/Sidebar/index.tsx:265` 假设元素含 `.id` 与 `.matchContext`。

**`transcriptServerAddress` 是纯死状态**：全仓仅 `SidebarProvider.tsx:87,122,315` 三处（定义、写入、透出），**零消费者**。两个地址字符串指向已归档的 Python FastAPI / whisper-server，但通过 `if (serverAddress)` 实际充当了「配置加载完成」的布尔门闩（`:98`、`frontend/src/components/Sidebar/index.tsx:112`、`:146`）——用假地址承担真职责。

### 2.2 ConfigContext（`frontend/src/contexts/ConfigContext.tsx`，554 行）

拥有：`modelConfig`、`transcriptModelConfig`、`providerApiKeys`、`selectedDevices`、`selectedLanguage`、`showConfidenceIndicator`、`isAutoSummary`、`betaFeatures`、`models`、`notificationSettings`、`storageLocations`、`isModelConfigLoading`。

与 Rust 的同步：

| 方向 | 机制 | 证据 |
| --- | --- | --- |
| Rust → JS | `configService.getModelConfig()` → `api_get_model_config` | `:234` |
| Rust → JS | `configService.getTranscriptConfig()` → `api_get_transcript_config` | `:201` |
| Rust → JS | 4 个 `api_get_api_key` 并发拉取（claude/groq/openai/openrouter） | `:303-309` |
| Rust → JS | `get_notification_settings`、`get_database_directory`、`whisper_get_models_directory`、`get_default_recordings_folder_path` | `:436,446,447,448` |
| JS → Rust | `set_language_preference`（挂载 + 变更） | `:220`、`:485` |
| JS → Rust | `set_notification_settings` | `:470` |
| 双向 | `model-config-updated` 事件 → `setModelConfig` | `:330-332` |

**已确认的状态冲突**：

1. **`transcriptModelConfig` 有三处独立加载，默认值互斥**：
   - `ConfigContext.tsx:112-116` 默认 `{provider:'parakeet', model:'parakeet-tdt-0.6b-v3-int8'}`
   - `ConfigContext.tsx:198-215` 挂载时从 `api_get_transcript_config` 载入
   - `frontend/src/app/settings/page.tsx:38-46` **也**从同一命令载入，provider 回退 `'localWhisper'`、model 回退 `'large-v3'`

   两者并发写同一个 Context state，后返回者胜出；当 DB 返回空值时，结果取决于 IO 竞态。更严重的是**可用 provider 集合不一致**：`frontend/src/lib/transcription-model-readiness.ts:11-22` 的 `PROVIDER_COMMANDS` 只含 `localWhisper` 与 `parakeet`，而 `frontend/src/components/TranscriptSettings.tsx:53-60` 的 UI 还提供 `deepgram`/`elevenLabs`/`groq`/`openai`。选中后者后，`useRecordingStart.ts:85-93` 得到 `commands === null`，只打一行 `console.error` 并返回 `false` —— 用户得到一个「无法录音且不说明原因」的状态。

2. **模型默认值在多处硬编码，且与 `constants/modelDefaults.ts` 不一致且该文件零引用**：
   - `frontend/src/constants/modelDefaults.ts:10` 声明 `DEFAULT_WHISPER_MODEL = 'large-v3-turbo'`
   - 但 `frontend/src/app/settings/page.tsx:43`、`frontend/src/contexts/ConfigContext.tsx:105`、`frontend/src/app/meeting-details/page.tsx:105` 三处写死 `'large-v3'`
   - `modelDefaults.ts` 在整个 `src/` 中**零引用**（全量检索确认），修改它无任何效果

3. **`modelConfig` 有 4 个独立副本**：`ConfigContext`、`ModelSettingsModal`、`SummaryModelSettings`、`Sidebar/index.tsx:67-73`。靠 `model-config-updated` 手动广播同步。该事件由前端 `emit`（`page-content.tsx:110`、`Sidebar/index.tsx:202`、`SummaryModelSettings.tsx:116`、`useModelConfiguration.ts:147`），被 4 处 `listen`（`ConfigContext.tsx:330`、`Sidebar/index.tsx:167`、`SummaryModelSettings.tsx:85`、`useModelConfiguration.ts:92`）。`ConfigContext.tsx:332` 用 `setModelConfig(event.payload)` **整体替换**，因此若某个发射点的 payload 缺字段（如 `customOpenAI*`），会静默丢失该 provider 的配置。

4. **`useMemo` 实际上从不命中缓存**：`modelOptions` 是渲染体内重建的普通对象（`:370-378`），却被列入 `useMemo` 依赖数组（`:532`）。由于它每次渲染身份都不同，`useMemo` 永不返回缓存值 → Context value 每次渲染都是新对象 → **所有 `useConfig()` 消费者在每个 ConfigProvider 渲染时被迫重渲染**。这是全仓影响面最大的性能缺陷，也是「用一个不稳定的普通对象做 memo 依赖」的教科书反例。

### 2.3 TranscriptContext（`frontend/src/contexts/TranscriptContext.tsx`，539 行）

拥有：`transcripts`、`meetingTitle`、`currentMeetingId`、`transcriptsRef`、`transcriptContainerRef` 及缓冲区逻辑。

**转录缓冲与顺序保证**（`:181-359`）设计是可取的：`Map<sequence_id, Transcript>` + `lastProcessedSequence` 顺序抽取（`:192-199`）+ 100ms 陈旧兜底（`:203`）+ 基于 `chunk_start_time`/`sequence_id` 双重排序（`:231-237`）+ sequence_id 去重（`:248-253`）。

但有三处隐患：

1. `:216` 用 `parseInt(transcript.id.split('-')[0])` 从字符串主键反推时间戳，而 id 在 `:307` 被构造为 `` `${Date.now()}-${transcriptCounter++}` ``。把时间语义编码进字符串主键再解析回来，脆弱且无法类型检查。
2. 该 effect 依赖 `[currentMeetingId]`（`:359`），而 `transcriptBuffer`/`lastProcessedSequence`/`transcriptCounter` 是 **effect 局部变量**（`:183-186`）。`currentMeetingId` 只在 `recording-started` 时设置（`:99`），若它在录音中途变化，监听器重建会**清空未处理的缓冲区**，已到达但未落库的片段静默丢失。
3. `:341` 在监听器装配失败时用阻塞式 `alert()`。

**重复的 `isRecording`**：`frontend/src/app/page.tsx:26` 保留局部 `isRecording` 与全局 `RecordingStateContext` 并存。`page.tsx:42` 把局部值喂给 `useRecordingStateSync`，`page.tsx:230` 把局部值喂给 `RecordingControls` 的 `isRecording` prop，而 `RecordingControls` 内部又读全局 `useRecordingState()` 取 `isPaused`（`RecordingControls.tsx:46-48`）：**同一组件同时消费两套录音状态**。

### 2.4 RecordingStateContext（`frontend/src/contexts/RecordingStateContext.tsx`，355 行）

设计意图中的单一真源（`:7-14` 注释明确说明）：

- `RecordingStatus` 8 态枚举（`:17-26`）
- 挂载时 `syncWithBackend()` 修复刷新后状态错位（`:335-338`）
- 录音期间 500ms 轮询 `get_recording_state`（`:118`）
- 5 个生命周期事件 + 4 个麦克风热插拔事件监听；`:251-329` 用 `cancelled` 标志**正确规避**了 StrictMode/HMR 下 `listen()` 异步未决议时的监听器泄漏（`:272/287/299/313`）。**这是全仓事件监听写法最正确的一段代码。**

缺陷：`setStatus` 的 `useCallback` 依赖写成 `[state.status, state.isRecording, state.isPaused]`（`:83`），但函数体从未读取这些值——`:76` 的 `console.log` 引用了 `state.status`，仅此而已。日志不该成为依赖：它使 `setStatus` 每次状态/录音位变化都重建，进而击穿 `contextValue` 的 `useMemo`（`:341-348`），并级联到所有依赖 `setStatus` 的 `useCallback`（`useRecordingStart`、`useRecordingStop`、`RecordingControls`）。

### 2.5 OnboardingContext（`frontend/src/contexts/OnboardingContext.tsx`，652 行）

拥有：`currentStep`、`parakeetDownloaded/Progress/ProgressInfo`、`summaryModelDownloaded/Progress/ProgressInfo`、`selectedSummaryModel`、`recommendedSummaryModel`、`databaseExists`、`isBackgroundDownloading`、`permissions`、`permissionsSkipped`。

- 数据库现状判定与迁移在 `:158-215` 内联实现（Homebrew 探测 → 默认 legacy 路径 → `initialize_fresh_database`），**与被孤立的 `DatabaseImport/LegacyDatabaseImport.tsx` 功能重叠**——迁移 UI 组件实际已被内联逻辑取代但未删除。
- 自动保存有 1s 防抖 + `isCompletingRef` 竞态保护（`:217-234`、`:443-450`），且有清晰注释解释为何跳过保存。这段是刻意设计与正确实现。
- `:151-155` 的挂载 effect 依赖 `[]`，调用三个组件作用域函数，其中 `initializeSummaryModelSelection` 的默认参数 `selectedSummaryModel`（`:111`）在空依赖下恒为初始值 `''`。
- `:559` 把 `parakeet_get_available_models` 断言为 `any[]`，`:560` 用 `typeof m.status === 'object' ? 'Downloading' in m.status : m.status === 'Downloading'` 做鸭子类型判断，绕过后端的 tag 枚举。
- `:300-329` 的 `builtin-ai-download-progress` 监听依赖 `[selectedSummaryModel]`，清理用 `unlisten.then(fn => fn())`。promise 最终会 resolve 所以 `.then` 必然执行，不构成泄漏；但 `selectedSummaryModel` 快速连续变更时会短暂出现多个监听器同时活跃。

### 2.6 其余 Context

| Context | 拥有 | 备注 |
| --- | --- | --- |
| `RecordingPostProcessingProvider` | 无自身 state，仅复用 `useRecordingStop` | `:23-24` 传入两个**空函数**作为 `setIsRecording`/`setIsRecordingDisabled`（因为 `useRecordingStop.ts:45` 实际用全局 Context），签名已过时；更严重的是它使第二个 `useRecordingStop` 实例常驻，见 §5.2 |
| `OllamaDownloadContext` | `downloadProgress: Map`、`downloadingModels: Set` | `contextValue` 在渲染体内新建（`:144-149`），未 `useMemo`，所有消费者每次渲染都更新 |
| `ImportDialogContext` | 仅 `openImportDialog` | 干净，含 beta 门控（`:29-34`） |

### 2.7 其它状态源（绕过 Context）

- `frontend/src/components/TranscriptView.tsx:132` 直读 `localStorage.getItem('showConfidenceIndicator')`，与 `ConfigContext.tsx:152-158` 并行持有同一偏好，靠自定义事件 `confidenceIndicatorChanged`（`ConfigContext.tsx:387` / `TranscriptView.tsx:145`）同步。
- `frontend/src/components/MeetingDetails/MeetingDetailsSplitView.tsx:19,29` 自管 localStorage 布局比例。
- `frontend/src/components/WhisperModelManager.tsx:57-68` 自管 `localStorage['downloading-models']`，同时全局还有 `DownloadProgressToastProvider`（`layout.tsx:248`）维护同一语义的下载状态。
- `frontend/src/hooks/useProcessingProgress.ts:206,212,230` 自管 `localStorage['transcription_progress']`（该 hook 本身零引用）。

---

## 3. Rust ↔ JS 契约

### 3.1 前端调用的 Tauri 命令

下表覆盖前端全部 `invoke()` 调用点（含经 `services/*` 间接调用）。共约 **144 个去重命令名**。

| # | 命令 | 调用方 file:line | 用途 | lib.rs 注册 |
| --- | --- | --- | --- | --- |
| 1 | `api_get_meetings` | `components/Sidebar/SidebarProvider.tsx:100`、`services/storageService.ts:65` | 会议列表 | `lib.rs:715` |
| 2 | `api_get_meeting` | `services/storageService.ts:57` | 单会议 | `lib.rs:729` |
| 3 | `api_get_meeting_metadata` | `hooks/usePaginatedTranscripts.ts:86` | 分页元数据 | `lib.rs:730` |
| 4 | `api_get_meeting_transcripts` | `hooks/usePaginatedTranscripts.ts:110`、`hooks/meeting-details/useSummaryGeneration.ts:407,421`、`hooks/meeting-details/useCopyOperations.ts:31,45` | 分页/全量转录 | `lib.rs:731` |
| 5 | `api_save_transcript` | `services/storageService.ts:44` | 保存会议+转录（**唯一落库写路径**） | `lib.rs:733` |
| 6 | `api_delete_meeting` | `components/Sidebar/index.tsx:329` | 删除会议 | `lib.rs:728` |
| 7 | `api_save_meeting_title` | `components/Sidebar/index.tsx:387` | 改标题 | `lib.rs:732` |
| 8 | `api_search_transcripts` | `components/Sidebar/SidebarProvider.tsx:188` | 全文检索 | `lib.rs:716` |
| 9 | `api_get_model_config` | `services/configService.ts:62`、`components/Sidebar/index.tsx:118`、`components/ModelSettingsModal.tsx:266`、`app/meeting-details/page.tsx:86` | 读摘要模型配置 | `lib.rs:720` |
| 10 | `api_save_model_config` | `components/Sidebar/index.tsx:188`、`app/meeting-details/page-content.tsx:100`、`app/meeting-details/page.tsx:102` | 写摘要模型配置 | `lib.rs:721` |
| 11 | `api_get_api_key` | `contexts/ConfigContext.tsx:306`、`components/Sidebar/index.tsx:123`、`components/ModelSettingsModal.tsx:208,273` | 读 provider key | `lib.rs:722` |
| 12 | `api_get_transcript_config` | `services/configService.ts:54`、`components/Sidebar/index.tsx:152`、`hooks/useRecordingStart.ts:73`、`app/settings/page.tsx:38` | 读转录配置 | `lib.rs:725` |
| 13 | `api_save_transcript_config` | `components/Sidebar/index.tsx:222`、`components/WhisperModelManager.tsx:323`、`components/ParakeetModelManager.tsx:260` | 写转录配置 | `lib.rs:726` |
| 14 | `api_get_transcript_api_key` | `components/TranscriptSettings.tsx:45` | 读转录服务 key | `lib.rs:727` |
| 15 | `api_get_custom_openai_config` | `services/configService.ts:78`、`components/ModelSettingsModal.tsx:293` | 读自定义 OpenAI | `lib.rs:740` |
| 16 | `api_save_custom_openai_config` | `services/configService.ts:87` | 写自定义 OpenAI | `lib.rs:739` |
| 17 | `api_test_custom_openai_connection` | `services/configService.ts:109`、`components/ModelSettingsModal.tsx:680` | 连通性测试 | `lib.rs:741` |
| 18 | **`api_get_auto_generate_setting`** | `components/ModelSettingsModal.tsx:320` | 读自动摘要开关 | ❌ **未注册**（`lib.rs:723` 被 `//` 注释） |
| 19 | `api_get_summary` | `app/meeting-details/page.tsx:199`、`components/Sidebar/SidebarProvider.tsx:240`、`hooks/meeting-details/useSummaryGeneration.ts:181,208` | 轮询摘要 | `lib.rs:744` |
| 20 | `api_process_transcript` | `hooks/meeting-details/useSummaryGeneration.ts:364` | 触发摘要生成 | `lib.rs:743` |
| 21 | `api_cancel_summary` | `hooks/meeting-details/useSummaryGeneration.ts:378,551` | 取消摘要 | `lib.rs:751` |
| 22 | `api_save_meeting_summary` | `hooks/meeting-details/useMeetingData.ts:50` | 保存摘要 | `lib.rs:745` |
| 23 | `api_get_meeting_summary_language` | `lib/summary-language-preferences.ts:103` | 摘要语言 | `lib.rs:746` |
| 24 | `api_save_meeting_summary_language` | `lib/summary-language-preferences.ts:125` | 摘要语言 | `lib.rs:747` |
| 25 | `api_get_meeting_detected_summary_language` | `lib/summary-language-preferences.ts:160` | 检测语言缓存 | `lib.rs:748` |
| 26 | `api_save_meeting_detected_summary_language` | `lib/summary-language-preferences.ts:180` | 检测语言缓存 | `lib.rs:749` |
| 27 | `api_detect_transcript_summary_language` | `lib/summary-language-preferences.ts:204` | 语言检测 | `lib.rs:750` |
| 28 | `api_list_templates` | `hooks/meeting-details/useTemplates.ts:18` | 模板列表 | `lib.rs:753` |
| 29 | `start_recording_with_devices_and_meeting` | `services/recordingService.ts:95` | 开始录音 | `lib.rs:686` |
| 30 | `stop_recording` | `components/RecordingControls.tsx:151`、`services/recordingService.ts:108` | 停止录音 | `lib.rs:614` |
| 31 | `is_recording` | `services/recordingService.ts:56`、`components/RecordingControls.tsx:78` | 状态查询 | `lib.rs:615` |
| 32 | `get_recording_state` | `services/recordingService.ts:64` | 完整录音状态（500ms 轮询） | `lib.rs:694` |
| 33 | `get_recording_meeting_name` | `services/recordingService.ts:72` | 当前会议名 | `lib.rs:698` |
| 34 | `pause_recording` | `components/RecordingControls.tsx:213`、`services/recordingService.ts:118` | 暂停 | `lib.rs:691` |
| 35 | `resume_recording` | `components/RecordingControls.tsx:231`、`services/recordingService.ts:126` | 恢复 | `lib.rs:692` |
| 36 | `get_transcript_history` | `services/transcriptService.ts:39` | 刷新后同步历史 | `lib.rs:697` |
| 37 | **`get_transcription_status`** | `services/transcriptService.ts:47` ← `hooks/useRecordingStop.ts:166` | 转录队列状态 | `lib.rs:616`（**硬编码桩，见 §3.3**） |
| 38 | `get_meeting_folder_path` | `contexts/TranscriptContext.tsx:129`、`hooks/useTranscriptRecovery.ts:132` | 会议文件夹 | `lib.rs:695` |
| 39 | `get_audio_devices` | `components/DeviceSelection.tsx:57`、`hooks/usePermissionCheck.ts:24,65` | 设备枚举 | `lib.rs:683` |
| 40 | `trigger_microphone_permission` | `components/onboarding/steps/PermissionsStep.tsx:43` | 麦克风授权 | `lib.rs:684` |
| 41 | `trigger_system_audio_permission_command` | `PermissionsStep.tsx:77` | 系统音频授权 | `lib.rs:802` |
| 42 | `start_audio_level_monitoring` | `components/DeviceSelection.tsx:237` | 电平监测 | `lib.rs:687` |
| 43 | `stop_audio_level_monitoring` | `components/DeviceSelection.tsx:250` | 电平监测 | `lib.rs:688` |
| 44 | `get_active_audio_output` | `components/BluetoothPlaybackWarning.tsx:35` | 蓝牙提醒 | `lib.rs:700` |
| 45 | `get_audio_backend_info` | `components/AudioBackendSelector.tsx:36` | 后端信息 | `lib.rs:774` |
| 46 | `get_current_audio_backend` | `components/AudioBackendSelector.tsx:41` | 当前后端 | `lib.rs:772` |
| 47 | `set_audio_backend` | `components/AudioBackendSelector.tsx:61` | 切换后端 | `lib.rs:773` |
| 48 | `get_recording_preferences` | `services/configService.ts:70`、`components/RecordingSettings.tsx:41` | 设备偏好 | `lib.rs:766` |
| 49 | `get_default_recordings_folder_path` | `components/RecordingSettings.tsx:47`、`contexts/ConfigContext.tsx:448` | 录制目录 | `lib.rs:768` |
| 50 | `set_language_preference` | `contexts/ConfigContext.tsx:220,485` | 语言同步 | `lib.rs:776` |
| 51 | `get_notification_settings` | `contexts/ConfigContext.tsx:436` | 通知设置 | `lib.rs:778` |
| 52 | `set_notification_settings` | `contexts/ConfigContext.tsx:470` | 通知设置 | `lib.rs:779` |
| 53 | `get_database_directory` | `contexts/ConfigContext.tsx:446` | 存储位置 | `lib.rs:812` |
| 54 | `whisper_get_models_directory` | `contexts/ConfigContext.tsx:447` | 模型目录 | `lib.rs:652` |
| 55 | `open_models_folder` | `lib/whisper.ts:334`、`components/PreferenceSettings.tsx:120` | 打开目录 | `lib.rs:814` |
| 56 | `open_parakeet_models_folder` | `lib/parakeet.ts:218` | 打开目录 | `lib.rs:670` |
| 57 | `open_recordings_folder` | `components/PreferenceSettings.tsx:117` | 打开目录 | `lib.rs:769` |
| 58 | `open_external_url` | `components/About.tsx:26`、`components/AnalyticsConsentSwitch.tsx:150`、`components/ModelSettingsModal.tsx:739,1236` | 外链 | `lib.rs:737` |
| 59 | `open_system_settings` | `PermissionsStep.tsx:33,65` | macOS 系统设置 | `lib.rs:822`（`#[cfg(target_os="macos")]`） |
| 60 | `whisper_init` | `lib/whisper.ts:286`、`hooks/useRecordingStart.ts:88`（经 `getProviderCommands`） | 初始化 | `lib.rs:644` |
| 61 | `whisper_get_available_models` | `hooks/useTranscriptionModels.ts:50` | 模型列表 | `lib.rs:645` |
| 62 | `whisper_has_available_models` | `hooks/useRecordingStart.ts:89`（经映射） | 就绪判定 | `lib.rs:649` |
| 63 | `whisper_download_model` / `whisper_cancel_download` | `lib/whisper.ts:290,318` | 下载/取消 | `lib.rs:653,654` |
| 64 | `whisper_load_model`/`whisper_get_current_model`/`whisper_is_model_loaded`/`whisper_validate_model_ready`/`whisper_transcribe_audio`/`whisper_delete_corrupted_model` | `lib/whisper.ts:294-334` | Whisper 生命周期 | `lib.rs:646,647,648,650,651,655` |
| 65 | `parakeet_init` | `contexts/OnboardingContext.tsx:391`、`onboarding/steps/DownloadProgressStep.tsx:343` | 初始化 | `lib.rs:657` |
| 66 | `parakeet_get_available_models` | `hooks/useTranscriptionModels.ts:66`、`OnboardingContext.tsx:559` | 模型列表 | `lib.rs:658` |
| 67 | `parakeet_has_available_models` | `OnboardingContext.tsx:392`、`DownloadProgressStep.tsx:344` | 就绪判定 | `lib.rs:662` |
| 68 | `parakeet_download_model` / `parakeet_retry_download` | `OnboardingContext.tsx:541,577`、`DownloadProgressStep.tsx:85` | 下载/重试 | `lib.rs:666,667` |
| 69 | `parakeet_cancel_download`/`parakeet_load_model`/`parakeet_get_current_model`/`parakeet_is_model_loaded`/`parakeet_validate_model_ready`/`parakeet_transcribe_audio`/`parakeet_get_models_directory`/`parakeet_delete_corrupted_model` | `lib/parakeet.ts:170-214` | Parakeet 生命周期 | `lib.rs:668,659,660,661,663,664,665,669` |
| 70 | `builtin_ai_list_models` | `components/BuiltInModelManager.tsx:53`、`ModelSettingsModal.tsx:510`、`lib/builtin-ai.ts:68` | 内置模型列表 | `lib.rs:757` |
| 71 | `builtin_ai_is_model_ready` | `OnboardingContext.tsx:118,405,488`、`useSummaryGeneration.ts:486` | 就绪判定 | `lib.rs:762` |
| 72 | `builtin_ai_get_recommended_model` | `OnboardingContext.tsx:113,401,484` | 推荐模型 | `lib.rs:764` |
| 73 | `builtin_ai_download_model` | `BuiltInModelManager.tsx:206`、`OnboardingContext.tsx:141`、`DownloadProgressStep.tsx:134` | 下载 | `lib.rs:759` |
| 74 | `builtin_ai_cancel_download` / `builtin_ai_delete_model` | `BuiltInModelManager.tsx:234,248` | 取消/删除 | `lib.rs:760,761` |
| 75 | `builtin_ai_get_model_info` / `builtin_ai_get_available_summary_model` | `lib/builtin-ai.ts:72,80` | 元信息 | `lib.rs:758,763` |
| 76 | **`builtin_ai_get_models_directory`** | `lib/builtin-ai.ts:96` | 模型目录 | ❌ **未注册** |
| 77 | `get_ollama_models` | `contexts/ConfigContext.tsx:186`、`ModelSettingsModal.tsx:434`、`app/meeting-details/page.tsx:56`、`useSummaryGeneration.ts:472` | Ollama 模型 | `lib.rs:708` |
| 78 | `pull_ollama_model`/`delete_ollama_model`/`get_ollama_model_context` | `ModelSettingsModal.tsx` 等 | Ollama 管理 | `lib.rs:709,710,711` |
| 79 | `get_openai_models`/`get_anthropic_models`/`get_groq_models`/`get_openrouter_models` | `ModelSettingsModal.tsx:494,534,552,570` | 云端模型列表 | `lib.rs:712,713,714,765` |
| 80 | `toggle_console`/`show_console`/`hide_console` | `components/ConsoleToggle.tsx:14,27,40` | 控制台（组件死代码） | `lib.rs:705,706,707` |
| 81 | `check_first_launch` | `OnboardingContext.tsx:161,333` | 首启判定 | `lib.rs:804` |
| 82 | `check_default_legacy_database` / `detect_legacy_database` | `OnboardingContext.tsx:200`、`LegacyDatabaseImport.tsx:37` | 旧库探测 | `lib.rs:807,806` |
| 83 | `check_homebrew_database` | `HomebrewDatabaseDetector.tsx:37`、`OnboardingContext.tsx:182` | Homebrew 探测 | `lib.rs:808` |
| 84 | `select_legacy_database_path` | `LegacyDatabaseImport.tsx:27` | 选择路径 | `lib.rs:805` |
| 85 | `import_and_initialize_database` | `LegacyDatabaseImport.tsx:64`、`HomebrewDatabaseDetector.tsx:60`、`OnboardingContext.tsx:189,203` | 导入 | `lib.rs:809` |
| 86 | `initialize_fresh_database` | `LegacyDatabaseImport.tsx:88`、`OnboardingContext.tsx:213` | 新建 | `lib.rs:810` |
| 87 | `get_onboarding_status` | `app/layout.tsx:81`、`OnboardingContext.tsx:344` | 引导状态 | `lib.rs:816` |
| 88 | `save_onboarding_status_cmd` | `OnboardingContext.tsx:453` | 保存引导 | `lib.rs:817` |
| 89 | `complete_onboarding` | `OnboardingContext.tsx:498` | 完成引导 | `lib.rs:819` |
| 90 | `start_retranscription_command` / `cancel_retranscription_command` | `MeetingDetails/RetranscribeDialog.tsx:217,236` | 重新转录 | `lib.rs:824,825` |
| 91 | `select_and_validate_audio_command` / `validate_audio_file_command` | `hooks/useImportAudio.ts:166,191` | 选文件/校验 | `lib.rs:828,829` |
| 92 | `start_import_audio_command` / `cancel_import_command` | `useImportAudio.ts:229,253` | 导入 | `lib.rs:830,831` |
| 93 | `has_audio_checkpoints` / `recover_audio_from_checkpoints` / `cleanup_checkpoints` | `hooks/useTranscriptRecovery.ts:63,143,203` | 检查点 | `lib.rs:704,702,703` |
| 94 | `init_analytics` / `disable_analytics` / `is_analytics_enabled` | `lib/analytics.ts:47,60,73` | 分析 | `lib.rs:619,620,629` |
| 95 | `track_event` | `lib/analytics.ts:87,659,699,721` | 通用埋点 | `lib.rs:621` |
| 96 | `identify_user` / `start_analytics_session` / `end_analytics_session` / `is_analytics_session_active` | `lib/analytics.ts:100,115,130,160` | 会话 | `lib.rs:622,630,631,634` |
| 97 | `track_daily_active_user` / `track_user_first_launch` | `lib/analytics.ts:140,150` | 埋点 | `lib.rs:632,633` |
| 98 | `track_meeting_started` / `track_recording_started` / `track_recording_stopped` / `track_meeting_deleted` | `lib/analytics.ts:541,551,561,571` | 埋点 | `lib.rs:623,624,625,626` |
| 99 | `track_settings_changed` / `track_feature_used` | `lib/analytics.ts:581,591` | 埋点 | `lib.rs:627,628` |
| 100 | `track_summary_generation_completed` / `track_summary_regenerated` / `track_model_changed` / `track_custom_prompt_used` | `lib/analytics.ts:788,809,827,847` | 埋点 | `lib.rs:636,637,638,639` |
| 101 | `track_analytics_enabled` / `track_analytics_disabled` / `track_analytics_transparency_viewed` | `AnalyticsConsentSwitch.tsx:111,120,61` | 埋点 | `lib.rs:641,642,643` |
| 102 | `read_audio_file` | `hooks/useAudioPlayer.ts:76` | 读音频字节 | `lib.rs:617` |

### 3.2 前端监听的事件

| 事件 | 前端监听位置 | 后端发射点（活跃代码） |
| --- | --- | --- |
| `recording-started` | `services/recordingService.ts:137` | `audio/recording_commands.rs:471,658` |
| `recording-starting` | `recordingService.ts:146` | `recording_commands.rs:351,541` |
| `recording-stopped` | `recordingService.ts:155`、`useRecordingStop.ts:89` | `recording_commands.rs:1084` |
| `recording-paused` | `recordingService.ts:166` | `recording_commands.rs:1131` |
| `recording-resumed` | `recordingService.ts:175` | `recording_commands.rs:1165` |
| `recording-stop-complete` | `RecordingPostProcessingProvider.tsx:36` | `tray.rs:94,190` |
| `transcript-update` | `transcriptService.ts:58` | `whisper_engine/transcription/worker.rs:220` 等 8 处 |
| **`transcription-complete`** | `transcriptService.ts:69`、`useRecordingStop.ts:158` | ❌ **活跃代码零发射点**（仅 `lib_old_complex.rs:1103,1297`） |
| **`transcript-error`** | `transcriptService.ts:89`、`RecordingControls.tsx:255` | ❌ **活跃代码零发射点**（仅 `lib_old_complex.rs:992`） |
| `transcription-error` | `transcriptService.ts:78`、`RecordingControls.tsx:277`、`useModalState.ts:136` | `worker.rs:63`、`recording_commands.rs:139,339,529` |
| **`chunk-drop-warning`** | `recordingService.ts:184`、`useModalState.ts:109` | ❌ **零发射点**（仅 `lib_old_complex.rs:692`） |
| `speech-detected` | `recordingService.ts:195`、`RecordingControls.tsx:306`、`TranscriptView.tsx:155` | `worker.rs:182` |
| `mic-device-switched` | `recordingService.ts:204` | `recording_commands.rs:1301,1316` |
| `mic-unavailable` | `recordingService.ts:216` | `recording_commands.rs:229` |
| `mic-swap-failed` | `recordingService.ts:227` | `recording_commands.rs:1324` |
| `mic-recovery-exhausted` | `recordingService.ts:239` | `recording_commands.rs:1560,1595,1613,1691` |
| `audio-levels` | `components/DeviceSelection.tsx:134` | `audio/level_monitor.rs:107`、`simple_level_monitor.rs:70` |
| **`model-download-complete`** | `transcriptService.ts:100`、`useModalState.ts:173` | ❌ **零发射点**（仅 `lib_old_complex.rs:2312`） |
| `model-config-updated` | `ConfigContext.tsx:330`、`Sidebar/index.tsx:167`、`SummaryModelSettings.tsx:85`、`useModelConfiguration.ts:92` | **前端自发自收**（`page-content.tsx:110`、`Sidebar/index.tsx:202`、`SummaryModelSettings.tsx:116`、`useModelConfiguration.ts:147`） |
| `parakeet-model-download-progress` | `OnboardingContext.tsx:239`、`ParakeetModelManager.tsx:97`、`DownloadProgressToast.tsx:226`、`DownloadProgressStep.tsx:200` | `parakeet_engine/commands.rs:402,449` |
| `parakeet-model-download-complete` | `OnboardingContext.tsx:271`、`transcriptService.ts:111`、`ParakeetModelManager.tsx:146`、`DownloadProgressToast.tsx:251` | `parakeet_engine/commands.rs:433` |
| `parakeet-model-download-error` | `OnboardingContext.tsx:282`、`ParakeetModelManager.tsx:187`、`DownloadProgressToast.tsx:270`、`DownloadProgressStep.tsx:243` | `parakeet_engine/commands.rs:466` |
| `builtin-ai-download-progress` | `OnboardingContext.tsx:308`、`BuiltInModelManager.tsx:81`、`DownloadProgressToast.tsx:306`、`DownloadProgressStep.tsx:272` | `summary/summary_engine/commands.rs:160,179,199,237` |
| `ollama-model-download-progress`/`-complete`/`-error` | `OllamaDownloadContext.tsx:52,76,104` | `ollama.rs:380,424,318/345/403` |
| `retranscription-progress`/`-complete`/`-error` | `RetranscribeDialog.tsx:131,146,173` | `audio/retranscription.rs:511,117,128` |
| `import-progress`/`-complete`/`-error` | `hooks/useImportAudio.ts:86,101,133` | `audio/import.rs:680,289,300` |
| **`request-recording-toggle`** | `app/layout.tsx:111` | ❌ **全仓零发射点**（含 Rust 侧） |
| `tauri://drag-enter`/`-leave`/`-drop` | `layout.tsx:168,180,191` | Tauri 内建 |

### 3.3 契约缺口

**A. 被调用但未注册（运行期 command-not-found）**

| 命令 | 调用点 | 证据 |
| --- | --- | --- |
| `api_get_auto_generate_setting` | `frontend/src/components/ModelSettingsModal.tsx:320` | `frontend/src-tauri/src/lib.rs:723-724` 明确注释掉注册；Rust 侧检索 `auto_generate_setting` **零结果**（无实现）。调用点 `:319-326` 有 `try/catch`，因此表现为「每次打开模型设置弹窗一条 `console.error` + 开关回退默认值」——**用户看到的「自动摘要」开关状态不可信**。真正生效的值来自 `frontend/src/contexts/ConfigContext.tsx:161-167` 的 `localStorage['isAutoSummary']`。配套的 `api_save_auto_generate_setting`（`lib.rs:724`）前端完全无调用 → 这条配置链在迁移中**只迁了一半** |
| `builtin_ai_get_models_directory` | `frontend/src/lib/builtin-ai.ts:96` | `lib.rs:757-764` 注册了 8 个 `builtin_ai_*` 命令，不含此命令；Rust 侧检索零结果。所在 `BuiltInAIAPI` 类**整体零引用**，属死封装 |

**B. `get_transcription_status` 注册的是硬编码桩（P0）**

`frontend/src-tauri/src/lib.rs:270-277`：

```rust
#[tauri::command]
fn get_transcription_status() -> TranscriptionStatus {
    TranscriptionStatus { chunks_in_queue: 0, is_processing: false, last_activity_ms: 0 }
}
```

消费链：`frontend/src/services/transcriptService.ts:47` → `frontend/src/hooks/useRecordingStop.ts:166`。危害见 §5.1。

**C. 已注册但前端从未调用（约 60 条）**

- 整个 `whisper_engine::parallel_commands::*`（`lib.rs:672-682`，11 条）
- 整个 `audio::system_audio_commands::*`（`lib.rs:793-798`，6 条）与 `audio::permissions::check_screen_recording_permission_command`、`request_screen_recording_permission_command`（`lib.rs:800-801`）
- `api_get_profile`/`api_save_profile`/`api_update_profile`（`lib.rs:717-719`）— 个人资料功能前端完全未接
- `test_backend_connection`/`debug_backend_connection`（`lib.rs:735-736`）— FastAPI 时代遗留
- `save_transcript`（`lib.rs:618`）、`start_recording`（`lib.rs:613`）、`start_recording_with_devices`（`lib.rs:685`）、`is_audio_level_monitoring`（`lib.rs:689`）、`is_recording_paused`（`lib.rs:693`）、`reset_onboarding_status_cmd`（`lib.rs:818`）、`open_database_folder`（`lib.rs:813`）、`api_get_template_details`/`api_validate_template`（`lib.rs:754,755`）

**D. 事件面反向缺口（P0/P1）**

`frontend/src-tauri/src/lib_old_complex.rs` 承载了 `transcription-complete`、`transcript-error`、`chunk-drop-warning`、`model-download-complete` 四个事件的全部发射代码，而该文件**未被任何模块声明**：`lib.rs:37-56` 的 `pub mod` 列表不含它，全仓检索 `lib_old_complex` 零命中（包括 `Cargo.toml` 与所有 `.rs`），它不参与编译。因此：

- `transcription-complete`：`useRecordingStop.ts:158` 依赖它作为「转录完成」信号，但活跃代码从不发射。
- 真正的替代信号是 `frontend/src-tauri/src/audio/transcription/worker.rs:339` 的 **`transcription-queue-complete`** —— 前端**零监听**。这是明确的双向断链：**后端在发、前端没听；前端在听、后端没发。**
- `chunk-drop-warning`（音频丢块告警）、`transcript-error`、`model-download-complete`（Whisper 下载完成自动关弹窗）、`request-recording-toggle`（托盘录音开关）四条 UX 链路同理失效。

**E. `model-config-updated`：前端自发自收的组件总线**

4 处 `emit` + 4 处 `listen`，全部在前端。这意味着「Rust 是配置唯一真源」的假设不成立，且 `ConfigContext.tsx:332` 的整体替换语义会在 payload 不完整时静默丢字段。

**F. Tauri 参数命名约定（澄清，非缺陷）**

Tauri v2.6.3 的 `#[tauri::command]` 默认 `ArgumentCase::Camel`（`tauri-macros-2.6.3/src/command/wrapper.rs:51` 默认值，`:506-508` 的转换逻辑 `key.to_lower_camel_case()`）。因此 Rust 的 snake_case 形参期望前端传 camelCase。前端所有调用（`meetingId`、`sourcePath`、`meetingFolder`、`modelName`、`micDeviceName` 等）与之一致，**未发现参数名不匹配**。

唯一例外是 `stop_recording` 的 `args: { save_path }`（`recordingService.ts:109`、`RecordingControls.tsx:152-154`）：`RecordingArgs`（`lib.rs:128-131`）是无 `rename_all` 的结构体，字段名保持 snake_case —— 两处调用都正确。

**G. 参数在传递链中被剥离（前端内部）**

`frontend/src/components/AISummary/Section.tsx:114-116` 调用 `onBlockNavigate(block.id, direction, cursorPosition)`（3 参），而 `frontend/src/components/AISummary/index.tsx:813` 传入 `(blockId, direction) => handleBlockNavigate(blockId, direction)`（2 参）。TS 允许少参赋值故不报错，`cursorPosition` 被静默丢弃——`Section.tsx:23` 声明了第三个参数，`index.tsx:125` 的 `handleBlockNavigate` 也不接收它。同类问题还有 `onCreateNewBlock`（`Section.tsx:24` 三参 vs `index.tsx:814` 传入三参，一致）与 `onKeyDown`（`Section.tsx:18` 三参 vs `index.tsx:808` 的 `handleKeyDown` 两参，`newBlockContent` 被丢弃）。

---

## 4. Hooks 与服务层

### 4.1 `useSummaryGeneration`（`frontend/src/hooks/meeting-details/useSummaryGeneration.ts`，595 行）

全仓并发控制质量最高的一段，也是唯一做了代际隔离的地方：

- `generationIdRef` 单调递增（`:323`、`:300`、`:538`、`:559`）；所有异步回调入口做三重校验 `mountedRef && visibleMeetingIdRef.current === meeting.id && generationId === generationIdRef.current`（`:158`、`:175`、`:190`、`:217`、`:360`、`:376`、`:555`）。
- `activeProcessIdRef` 跟踪当前进程，卸载时精确停止对应轮询（`:285-287`），不会误停别的会议。
- 竞态处理：`processSummary` 开头把上一次未完成 attempt 记为 `cancelled`（`:318-321`）；若 `api_process_transcript` 返回时代际已失效，主动 `api_cancel_summary` 回收后端进程（`:377-380`）。
- 失败/取消时用 `api_get_summary` 恢复上一份摘要（`:181-199`、`:208-231`），避免「重新生成失败导致旧摘要消失」。
- `pollingResultRef`（`:278-279`）让监听器始终调用最新闭包。
- 埋点去重：`trackedAttemptRef.finished` 保证一次生成只上报一次（`:139-142`）。
- `handleGenerateSummary`（`:459-500`）的前置校验到位：ollama 检查模型列表非空、`builtin-ai` 检查 `builtin_ai_is_model_ready`，失败时给出可操作提示并调用 `onOpenModelSettings`。

可指摘处：

1. `processSummary` 的 deps（`:388-399`）含 `onMeetingUpdated`、`setAiSummary`、`updateMeetingTitle`，但函数体内**从不使用**它们（只用 `finishGeneration`、`failGeneration`、`modelConfig`、`selectedTemplate`、`meeting.id`、`meeting.created_at`、`startSummaryPolling`）。多余 deps 使回调身份不稳定，级联让 `handleGenerateSummary`（`:506-515`）与 `frontend/src/app/meeting-details/page-content.tsx:161-186` 的自动生成 effect 每次渲染重跑判定。
2. `fetchAllTranscripts`（`:402-434`）先 `limit:1` 取总数，再 `limit: totalCount` 全量拉取（`:407-425`）。长会议会把全部转录一次性载入内存并拼成单个字符串；虽然 `api_process_transcript` 接收 `chunkSize: 40000`（`:369`）把分块推给后端，前端仍承担全量内存峰值。

### 4.2 其它 hooks

| Hook | 行数 | 职责 | 观察 |
| --- | --- | --- | --- |
| `usePaginatedTranscripts` | 223 | 分页拉取元数据+转录，100 条/页 | 设计良好：`requestIdRef` + `activeMeetingIdRef` 双校验（`:61-64`）、`offsetRef`、按 id 去重（`:125-126`）、`loadMore` 100ms 防抖（`:153-157`）、`isLoadingRef` 重入门控（`:159`）、`reset()` 作废未决请求（`:67-79`） |
| `useRecordingStart` | 412 | 手动/自动（sessionStorage `autoStartRecording`）/侧栏自定义事件三入口启动 | `:52` 的同步 `isStartingRef` latch 正确地防住双击重入（异步状态守卫做不到）。但三份启动逻辑近乎逐行重复（`:119-208`、`:211-306`、`:309-406`），约 280 行；`:281`、`:379` 用阻塞式 `alert()` |
| `useRecordingStop` | 470 | 停止后处理：等转录 → flush → 落库 → 跳转 → 埋点 | 见 §5.1（P0）。`stopInProgressRef`（`:73`）是**实例级**守卫，见 §5.2（P1） |
| `useModalState` | 200 | 6 个 modal 状态 + 3 个监听 | `:168-191` 的监听 effect **不返回清理函数** → 监听器泄漏，见 §5.4 |
| `useRecordingStateSync` | 71 | 1s 轮询 `is_recording` 同步页面局部 state | `:50` 用 `(window as any).__TAURI__` 作门控。Tauri v2 默认不注入该全局（`frontend/src-tauri/tauri.conf.json` 的 `app` 段无 `withGlobalTauri`），因此**该 effect 整体不执行**：`isRecordingDisabled` 恒为 `false`、后端状态同步永不发生。正确键是 `__TAURI_INTERNALS__`，见 `frontend/src/hooks/usePlatform.ts:8` 的 `declare global` |
| `useImportAudio` | 283 | 音频导入 | 监听用 `cleanedUpRef` 正确规避异步注册泄漏（`:81`、`:93-96`、`:124-128`、`:144-148`），**写法正确** |
| `useTranscriptRecovery` | 248 | IndexedDB + 音频 checkpoint 恢复 | `:50` 变量名 `secondsAgo` 与 `:47` 注释都说「15 秒阈值」，代码实际是 `2 * 1000`（2 秒）→ 注释与实现不符 |
| `usePermissionCheck` | 86 | 麦克风/系统音频可用性 | `checkPermissions` 未 `useCallback`（`:19`），每次渲染新引用；`requestPermissions` 内 `setTimeout` 无清理（`:68-70`）。且把「有 Output 设备」等同于「有系统音频权限」（`:32-33`），语义偏弱 |
| `useAutoScroll` | 180 | 自动滚动 | 仅 `VirtualizedTranscriptView.tsx:150` 使用 |
| `useTranscriptStreaming` | — | 打字机效果 | 仅 `VirtualizedTranscriptView.tsx:161` 使用 |
| **死 hooks** | — | `useAudioPlayer`、`useNavigation`、`useProcessingProgress`、`useModelConfiguration` | 全量检索确认零外部引用 |

### 4.3 services/ 层的 HTTP 使用

**结论：前端不发起任何 HTTP 或 WebSocket 请求。**

全量检索 `fetch(`、`axios`、`XMLHttpRequest`、`new WebSocket(`、`EventSource(` 在 `frontend/src/**` 中**零命中**（唯一的 `fetch(` 字面匹配是 `frontend/src/hooks/usePaginatedTranscripts.ts:192` 的 `refetch()`）。

所有网络 IO 都经 Tauri 命令下沉到 Rust（Ollama、OpenAI、Anthropic、Groq、OpenRouter、HuggingFace 模型下载）。CSP 侧面印证：`frontend/src-tauri/tauri.conf.json` 的 `connect-src` 仅允许 `'self'`、`http://localhost:11434`（Ollama）、`http://localhost:5167`、`http://localhost:8178`、`https://api.ollama.ai`。

前端出现的 `http://` 字符串全部是：(a) SVG `xmlns` 声明、(b) 外链常量（`About.tsx:26` 等经 `open_external_url` 打开）、(c) 输入框占位符（`ModelSettingsModal.tsx:956,1149`）、(d) `SidebarProvider.tsx:121-122` 的两处死地址。

services/ 共 6 个模块，全部是 `invoke`/`listen` 薄封装（自述见各文件头，如 `configService.ts:4-5`「Pure 1-to-1 wrapper - no error handling changes」）：

| 服务 | 行数 | 内容 |
| --- | --- | --- |
| `configService.ts` | 118 | 5 个命令封装，干净 |
| `storageService.ts` | 70 | 3 个命令封装；`Meeting` 接口含 `[key: string]: any`（`:24`） |
| `recordingService.ts` | 246 | 9 命令 + 9 事件；`:28-44` 的 `withStartTimeout` 120s 超时保护是必要且写得好的补丁（`:30-32` 的注释解释了为何超时不自动调 `stop_recording`） |
| `transcriptService.ts` | 118 | 2 命令 + 5 事件 |
| `updateService.ts` | 136 | Tauri updater 封装；`:43-45` 并发检查抛异常、`:48-57` 24h 节流 |
| `indexedDBService.ts` | 449 | 浏览器侧恢复存储；`saveTranscript` 的 `transcript: any`（`:230`）与 `StoredTranscript` 的 `[key: string]: any`（`:29`）是类型逃逸口。函数式写法则规范：`saveMeetingMetadata`（`:100-116`）与 `saveTranscript`（`:230-271`）都「失败静默、不打断录音」并注明原因 |

### 4.4 analytics / lib 使用

`frontend/src/lib/analytics.ts`（857 行）是单例静态类，默认导出 + 具名导出（`:21`、`:857`）。

- **默认隐私友好**：`frontend/src/components/AnalyticsProvider.tsx:34-53` 的 `analyticsOptedIn` 默认 `false`，并有一次强制回写迁移（`analyticsDefaultOffMigrationV1`）。
- 初始化幂等（`analytics.ts:30-56`，`initialized` + `initializationPromise` 双标志）。
- **本地状态分散在两个位置**：Tauri `plugin-store` 的 `analytics.json`（`:172`、`:201`、`:224`、`:309`、`:323`、`:343`、`:356`、`:368`、`:396`、`:502`）与 `sessionStorage`（`:188-192`、`:213-216`）降级。同一份 `total_meetings` 在 `frontend/src/hooks/useRecordingStop.ts:382-383` 又经 `Store.load('analytics.json')` **第三次**读取。
- 埋点统一走 `invoke('track_event', {eventName, properties})`；`properties` 类型限定 `Record<string, string>`（`:3-5`），因此所有数值在调用点手动 `.toString()`（`:449`、`:452-455`）。不一致点：`trackSummaryGenerationCompleted` 走专用命令传**原始数字**（`:788-794`），`trackMeetingCompleted` 走字符串化的 `track_event`（`:447-458`）。
- `reportedTranscriptionErrorCodes`（`:28`）做「每会话每类错误只报一次」去重（`:686-695`），设计合理；`getTranscriptionErrorCode`（`:673-684`）把错误分桶为 `auth_rejected`/`ort_failed`/`transcription_failed`，并对 `cooling down` 特判返回 `null`。
- `trackBackendConnection`（`:649-671`）的 `waitForInitialization` 是**忙轮询**（`:641-643`，100ms 间隔查 `initialized` 标志），最坏阻塞 5s。

`lib/` 其余：`summary-content.ts`、`summary-languages.ts`、`summary-language-preferences.ts`、`blocknote-markdown.ts`、`onboarding-summary-model.ts`、`transcription-model-readiness.ts`、`whisper.ts`(293)、`parakeet.ts`(189)、`builtin-ai.ts`(98，**死**)、`utils.ts`、`recordingNotification.tsx`。

---

## 5. UX / 工程缺陷

### 5.1 effect 依赖缺失/错误（逐个核对函数体）

| 位置 | 问题 | 后果 |
| --- | --- | --- |
| `frontend/src/components/AISummary/index.tsx:77-85` | deps 仅 `[summary]`，闭包读 `history`、`currentHistoryIndex`、`isUndoRedoing` | 撤销/重做栈使用陈旧快照；连续编辑后 undo 跳到错误版本，可能静默丢弃用户摘要编辑 |
| `frontend/src/contexts/ConfigContext.tsx:490-539` | `useMemo` 依赖数组含 `modelOptions`，而它是渲染体内重建的普通对象（`:370-378`） | **`useMemo` 永不命中缓存** → Context value 每次渲染都是新对象 → 所有 `useConfig()` 消费者被迫重渲染 |
| `frontend/src/contexts/ConfigContext.tsx:218-228` | deps `[]`，函数体读 `selectedLanguage` | 注释自称「修复启动时不同步 bug」，但空依赖只用**初始值**（来自 localStorage 惰性初始化 `:143-149`）。首帧后语言被改写时 Rust 端收不到 |
| `frontend/src/contexts/ConfigContext.tsx:327-348` | deps `[]`，函数体调用 `updateProviderApiKey` | 闭包捕获首次渲染的引用；该 setter 的 `useCallback` 无 deps 故实际稳定，危害低 |
| `frontend/src/contexts/TranscriptContext.tsx:363-405` | deps 仅 `[recordingState.isRecording]`，函数体读 `transcripts.length`（`:366`） | 刷新后若转录在 `isRecording` 变 true 之后才到达，`transcripts.length === 0` 判定会永久错过同步窗口 |
| `frontend/src/contexts/TranscriptContext.tsx:181-359` | deps 仅 `[currentMeetingId]`；`transcriptBuffer`/`lastProcessedSequence`/`transcriptCounter` 是 effect 局部变量（`:183-186`） | 监听器重建即清空未处理缓冲，记录中途变化会静默丢片段 |
| `frontend/src/contexts/RecordingStateContext.tsx:75-83` | `setStatus` deps 含 `state.status/isRecording/isPaused`，仅 `:76` 的日志用到 | 每次状态变化重建 `setStatus` → 击穿 `contextValue` 的 `useMemo`（`:341-348`）→ 全量消费者重渲染 |
| `frontend/src/hooks/useRecordingStop.ts:423-438` | deps 含 `meetings`（`:435`），函数体未使用 | 侧栏会议列表变化即改变 `handleRecordingStop` 身份 → `RecordingPostProcessingProvider.tsx:58` 的 effect 重注册监听器 |
| `frontend/src/components/RecordingControls.tsx:248-332` | deps `[onRecordingStop, onTranscriptionError]`，而 `onRecordingStop` 在 `frontend/src/app/page.tsx:231` 是**行内箭头函数** | 父组件每次渲染都产生新函数 → 3 个 Tauri 监听器被反复注销重注册，期间事件可能丢失 |
| `frontend/src/hooks/meeting-details/useSummaryGeneration.ts:388-399` | deps 含 `onMeetingUpdated`、`setAiSummary`、`updateMeetingTitle`，函数体未使用 | 回调身份不稳定 → 级联使 `page-content.tsx:176-186` 的自动生成 effect 每次渲染重跑判定 |
| `frontend/src/components/Sidebar/index.tsx:89-95` | deps `[expandedFolders]` 且函数体内 `setExpandedFolders(newExpanded)` | 自触发：每次变化都再 set 一次（`if` 守卫使其收敛，但多一次渲染） |
| `frontend/src/contexts/OnboardingContext.tsx:151-155` | deps `[]`，调用三个组件作用域函数 | `initializeSummaryModelSelection` 的默认参数 `selectedSummaryModel`（`:111`）在空依赖下恒为初始 `''` |
| `frontend/src/components/SummaryGeneratorButtonGroup.tsx:61-75` | deps `[onOpenModelSettings]`，而它来自 `page-content.tsx:81` 的行内函数 | 每次渲染都重新注册 dialog 打开器 |

另外 `frontend/src/app/page.tsx:106` 的依赖含 `status`（状态机全量），使恢复检查在每个状态跃迁时重跑。

### 5.2 【P0】停止录音不等转录收尾 → 静默数据缺失

`frontend/src/hooks/useRecordingStop.ts:155-209`：

```ts
let transcriptionComplete = false;
const unlistenComplete = await listen('transcription-complete', () => { transcriptionComplete = true; });  // :158
while (elapsedTime < MAX_WAIT_TIME && !transcriptionComplete) {
  const status = await transcriptService.getTranscriptionStatus();     // :166
  if (!status.is_processing && status.chunks_in_queue === 0) {          // :170
    transcriptionComplete = true; break;                                // :171-173
  }
  ...
}
```

而 `frontend/src-tauri/src/lib.rs:270-277` 的 `get_transcription_status` **恒返回** `{chunks_in_queue: 0, is_processing: false, last_activity_ms: 0}`。因此：

- 第一次迭代即 `break`（`:170-173`），60s 上限与 500ms 间隔从未真正等待。
- `transcription-complete` 在活跃 Rust 中无发射点（§3.3D），`:158` 的监听器是死代码。
- **唯一等待变成 `:208` 的 `await sleep(4000)`**。若实际转录落后超过 4 秒（长录音、低配机器、大模型），`:241` 的 `transcriptsRef.current` 缺尾部片段，`:256` 的 `api_save_transcript` 落库不完整。
- 更糟的是 `:297` 紧接着 `markMeetingAsSaved()`，把 IndexedDB 恢复路径也关闭 → **数据不可挽回地丢失**。
- 兜底信号其实存在：`frontend/src-tauri/src/audio/transcription/worker.rs:339` 发射 `transcription-queue-complete`，前端零监听。

**附带的数据丢失路径**：落库完全依赖前端进程内的 `transcriptsRef`（`:241`）与 sessionStorage 的 `folder_path`/`meeting_name`（`:244-245`，来源 `:96-99`）。用户在「停止 → 落库」窗口内关窗/崩溃时，sessionStorage 随进程消失，落库前片段只存在于 IndexedDB 旁路（`TranscriptContext.tsx:326`，且是 fire-and-forget `.catch(console.warn)` 不重试）。即「会议是否被持久化」由**一个前端事件 + 4 秒固定等待**决定，而非由 Rust 侧的音频/转录完成状态决定。

### 5.3 【P1】`useRecordingStop` 被实例化两次，守卫是实例级的 → 重复落库风险

- 实例 A：`frontend/src/app/page.tsx:46`（主页按钮路径）。
- 实例 B：`frontend/src/contexts/RecordingPostProcessingProvider.tsx:28`（`layout.tsx:245` 全局挂载，主页同样存在）。
- 守卫：`frontend/src/hooks/useRecordingStop.ts:73` 的 `stopInProgressRef` 是 `useRef` → **每个实例各一把锁**。

结果：一次停止可能由「主页按钮回调」（`RecordingControls.tsx:162` → `page.tsx:231`）与「`recording-stop-complete` 事件」（`RecordingPostProcessingProvider.tsx:36-41`）两条路同时进入 `handleRecordingStop(true)`，两边都执行 `storageService.saveMeeting`（`:256`）。是否产生重复会议取决于 Rust `api_save_transcript` 是否有幂等键；前端**没有任何跨实例互斥**。同时每个实例都会在停止时临时 `listen('transcription-complete')`（`:158`）并常驻 `listen('recording-stopped')`（`:89`），日志噪音翻倍。

### 5.4 【P1】监听器泄漏

| 位置 | 问题 | 判定 |
| --- | --- | --- |
| `frontend/src/hooks/useModalState.ts:168-191` | `setupDownloadListeners()` 是 async 且 effect 直接调用（`:190`），**既不 `return` 清理函数也无 `cancelled` 守卫**；`:185-187` 返回的 `unlisteners.forEach(...)` 无人接收。`model-download-complete`（`:173`）永不 `unlisten`；依赖数组 `[transcriptModelConfig, hideModal]` 使模型配置每变一次就再注册一个 → **监听器随配置变更次数线性增长** | **确定性泄漏** |
| `frontend/src/components/AnalyticsProvider.tsx:62-137` | `initAnalytics2` 内 `window.addEventListener('beforeunload', …)`（`:126`）不在任何 cleanup 中，`:58` 丢弃了返回值 | **确定性泄漏** |
| `frontend/src/components/BuiltInModelManager.tsx:77-199` | `let unlisten` 声明在 effect 外、`await listen(...)` 赋值在 async 函数内（`:81`）；组件在 promise 决议前卸载时 cleanup（`:194-198`）看到 `undefined` 直接跳过 | 窄窗口泄漏 |
| `frontend/src/components/WhisperModelManager.tsx:190-313` | 同上模式（有 `initialized` 状态兜底） | 窄窗口泄漏 |
| `frontend/src/components/DeviceSelection.tsx:129-156` | `let unlisten` + async 赋值（`:134`） | 窄窗口泄漏 |
| `frontend/src/contexts/RecordingStateContext.tsx:251-329` | `cancelled` 标志（`:272/287/299/313`） | **正确** |
| `frontend/src/hooks/useImportAudio.ts:79-158` | `cleanedUpRef` | **正确** |
| `frontend/src/app/layout.tsx:160-209` | `unlisteners[]` + `cleanedUpRef` | **正确** |
| `frontend/src/components/onboarding/steps/DownloadProgressStep.tsx:197-296` | `unlistenX.then(fn => fn())` | **正确** |
| `frontend/src/components/shared/DownloadProgressToast.tsx:223-342` | `unlistenX.then(fn => fn())` | **正确** |
| `frontend/src/components/ParakeetModelManager.tsx:88-254` | `registeredUnlisteners` 数组 + `listenersReady` 状态（`:245`） | **正确** |

结论应表述为「**大部分**清理正确，但 `useModalState` 与 `AnalyticsProvider` 各有一处确定性泄漏」，而非「不存在泄漏」，也不是「普遍泄漏」。

**重复监听同一事件**：

- `transcription-error` 被 3 处监听：`RecordingControls.tsx:277`、`useModalState.ts:136`、`services/transcriptService.ts:78`（封装层）。一次错误会触发 `Analytics.trackTranscriptionError`（`RecordingControls.tsx:283`）+ 一个 toast（`useModalState.ts:145`）+ 可能一次 `onRecordingStop(false)`（`RecordingControls.tsx:295`）+ 一次 modal（`useModalState.ts:142`）。行为叠加但互不知情。
- `speech-detected` 被 3 处监听：`RecordingControls.tsx:306`、`TranscriptView.tsx:155`、封装层 `recordingService.ts:195`。两个组件各自 `setState`。
- `model-config-updated` 4 发 4 收。

### 5.5 未处理的 Promise 拒绝 / 假成功 / 阻塞式 UI

- **`frontend/src/contexts/TranscriptContext.tsx:470-472`**：`navigator.clipboard.writeText(fullTranscript)` **未 await、未 catch**，随后无条件 `toast.success("Transcript copied to clipboard")`。剪贴板被拒（无权限、窗口未聚焦）时既产生未处理拒绝，又给用户**假成功**。
- `frontend/src/hooks/useRecordingStop.ts:447-449`：`(window as any).handleRecordingStop` 直接调用 async 函数且不接 promise → Rust 回调路径上的失败无人接收（该桥在 Rust 侧已零调用，见 §5.9）。
- `frontend/src/contexts/RecordingPostProcessingProvider.tsx:41`：`handleRecordingStop(event.payload)` 未 `await` 也未 `.catch`；错误边界完全依赖 `useRecordingStop` 内部的 `try/catch`（`useRecordingStop.ts:138-422`）。
- **`setupListener().then(...)` 缺 `.catch`（4 处）**：`ConfigContext.tsx:343`、`Sidebar/index.tsx:176`、`SummaryModelSettings.tsx:86`、`useModelConfiguration.ts:93`。若 `listen()` 拒绝，产生 unhandled rejection 且 cleanup 变量保持 `undefined`（静默无监听器）。
- `frontend/src/hooks/usePermissionCheck.ts:68-70`：`setTimeout(() => { checkPermissions(); }, 1000)` 内 promise 未 catch（内部有 try/catch，安全但依赖内部实现）。
- **阻塞式 `alert()` 共 10 处**：`RecordingControls.tsx:82,218,236`、`useRecordingStart.ts:281,379`、`TranscriptContext.tsx:341`、`TranscriptRecovery.tsx:89,109`、`PermissionsStep.tsx:35,67`。其中 `TranscriptContext.tsx:341` 在**转录监听器装配失败**这一核心链路错误上弹 alert，会遮挡录音界面；而项目已引入 `sonner` 的 `toast` 作为统一方案。
- **轮询叠加**：500ms（`RecordingStateContext.tsx:118` 录音状态）+ 1000ms（`useRecordingStateSync.ts:55`，实际未启动）+ 5000ms（`SidebarProvider.tsx:286` 摘要）+ 停止时 500ms（`useRecordingStop.ts:153`，实际一次即退）+ 300ms 纯装饰波形（`page.tsx:182`，`setBarHeights` 每 300ms 造成一次重渲染）。

### 5.6 类型与规模

- `frontend/tsconfig.json:7` 设了 `"strict": true`，但 `skipLibCheck: true`（`:6`），且无 `noUncheckedIndexedAccess` / `exactOptionalPropertyTypes`。
- `any` 相关写法（`: any`、`<any>`、`as any`、`any[]`、`(window as any)`、`@ts-ignore`、`@ts-expect-error`、`@ts-nocheck`、`eslint-disable`）共 **61 处**，分布 31 个文件。Top：`ModelSettingsModal.tsx` 6、`useTranscriptRecovery.ts` 5、`useImportAudio.ts` 4、`Sidebar/index.tsx` 4。
- **零 `@ts-ignore` / `@ts-expect-error` / `@ts-nocheck`**（已确认）——所有放宽都用 `any` 表达，这是相对克制的做法。

集中点名的漏洞：

| 位置 | 问题 |
| --- | --- |
| `frontend/src/app/meeting-details/page-content.tsx:38` | `meeting: any` — 会议详情页顶层 props 失去类型，`meeting.folder_path`（`:219`）等字段无保护 |
| `frontend/src/app/meeting-details/page-content.tsx:46` | `segments?: any[]` — 分页片段类型被抹掉 |
| `frontend/src/hooks/meeting-details/useMeetingData.ts:10` | `meeting: any` |
| `frontend/src/components/BuiltInModelManager.tsx:180` | `as any` 强制把 `{type:'error', progress:0}` 塞进 `ModelStatus`（该 union 的 error 分支要求 `Error: string`） |
| `frontend/src/components/BuiltInModelManager.tsx:81` | `listen('builtin-ai-download-progress', (event: any) => ...)` |
| `frontend/src/hooks/useRecordingStateSync.ts:50` | `(window as any).__TAURI__` — 键名错误 |
| `frontend/src/hooks/useRecordingStop.ts:447,453` | `(window as any).handleRecordingStop` |
| `frontend/src/components/Sidebar/index.tsx:437,443` | `(window as any).openSettings` |
| `frontend/src/services/indexedDBService.ts:29,230` | `[key: string]: any` + `transcript: any` |
| `frontend/src/types/index.ts:54,74,75,87` | `ApiResponse.data: any[]`、`BlockNoteBlock.props: Record<string, any>`、`content?: any[]`、`SummaryDataResponse[key: string]: any` |
| `frontend/src/components/Sidebar/SidebarProvider.tsx:84` | `useState<any[]>([])` 的 `searchResults` |
| `frontend/src/contexts/OnboardingContext.tsx:559` | `invoke<any[]>('parakeet_get_available_models')` |

**巨型组件**：`ModelSettingsModal.tsx` 1287、`Sidebar/index.tsx` 882、`AISummary/index.tsx` 758、`WhisperModelManager.tsx` 729、`ParakeetModelManager.tsx` 629 —— 单个文件混合表单、校验、下载、provider 分支与事件同步。`useRecordingStart.ts` 的三份重复启动逻辑是「同一语义三处实现」的典型：`:185`/`:272`/`:370` 的 `'already in progress'` 分支、文案与埋点完全平行，任何一处修 bug 都极易漏改另两处。

**657 处 `console.*`**（含大量 emoji 调试输出，如 `TranscriptContext.tsx:290-297`、`RecordingControls.tsx:249`）留在生产代码中。

### 5.7 无障碍基础

统计口径：`frontend/src/**/*.tsx`。

| 指标 | 数量 |
| --- | --- |
| `<button>` 元素 | 160 |
| 其中带 `aria-label` 或 `title` | 28（17.5%） |
| `aria-label` 出现次数 | 14 |
| `role=` | 11 |
| `aria-live` | **2** |
| `tabIndex` | 4 |
| `<img>` | 2（均有 `alt`：`About.tsx:57`、`Logo.tsx:25`） |

1. **核心控件缺可访问名**：`frontend/src/components/RecordingControls.tsx:386-400`（开始录音）、`:411-433`（暂停/恢复）、`:442-457`（停止）三个按钮**只含图标**（`<Mic>/<Pause>/<Play>/<Square>`），可访问名依赖 Radix `TooltipContent`（`:402/435/459`）而非 `aria-label`；未设 `role`/`aria-pressed`，屏幕阅读器只能播报「button」，且**无法区分暂停与继续是同一按钮的状态切换**。
2. **侧栏图标按钮同样缺失**：`frontend/src/components/Sidebar/index.tsx:461-467`、`:476-486`、`:496-507`、`:511-520`、`:529-535`（五个 Tooltip 包裹的图标按钮）、`:666-676`（折叠按钮）、`:796-803`（导入按钮）。
3. **状态变化不播报**：`RecordingControls.tsx:338-342` 的 "Processing recording..." spinner 无 `role="status"`/`aria-live`；实时转录是持续更新的动态内容（`VirtualizedTranscriptView.tsx:100-106`）却无任何 `aria-live` 区域，屏幕阅读器用户无法感知新片段。
4. **键盘不可达的块选择**：`frontend/src/components/AISummary/index.tsx:158-180` 的 `handleBlockMouseDown`/`handleBlockMouseEnter`/`handleBlockMouseUp` 仅绑定鼠标事件；`Section.tsx:101-103` 也只传 `onMouseDown/onMouseEnter/onMouseUp`。多块选择、Shift 区间选择完全无键盘等价操作。`:406-432` 的全局 `keydown` 只处理 Ctrl/Cmd+Z/C 与 Delete，且 `:428-431` 在 `selectedBlocks.length > 1` 时 `preventDefault()` 拦截 Delete —— 用户在 textarea 内编辑多块时，标准删除行为被劫持。
5. **自定义右键菜单无语义**：`AISummary/index.tsx:673-698` 用 `<div>`+`<button>` 手搓上下文菜单，无 `role="menu"`/`role="menuitem"`、无 `aria-orientation`、无 Esc 关闭（关闭依赖 `:477-483` 的 document click）。
6. **正面项**：`RecordingControls.tsx:499` 的关闭按钮有 `aria-label="Close alert"`；`EmptyStateSummary` 有 `role="alert"`（被 `tests/lib/empty-state-summary.test.tsx:12` 断言）。

### 5.8 工程配置缺陷

| 位置 | 问题 |
| --- | --- |
| `frontend/package.json:28` | `"lint": "next lint"`，但 `devDependencies`（`:98-115`）**既无 `eslint` 也无 `eslint-config-next`** → `pnpm run lint` 必然失败 |
| `frontend/eslint.config.mjs:3,13` | 依赖 `@eslint/eslintrc` 与 `next/core-web-vitals`/`next/typescript`，同样未声明为依赖 |
| `frontend/package.json:5` | `"main": "electron/main.js"` — `frontend/electron/` 目录**不存在**，是 Electron→Tauri 迁移残留 |
| `frontend/package.json:9` | `"export": "next export"` — Next 14 已移除该命令（`output: 'export'` 由 `next build` 处理） |
| `frontend/package.json` 全部 | **无 `test` script，无任何测试运行器依赖**；`devDependencies` 只有 `@types/bun@1.3.14`（`:103`），无 `bun`/`vitest`/`jest` |
| `frontend/src/config/api.ts` | **0 字节空文件** |
| `frontend/src/components/AudioPlayer.tsx` | **0 字节空文件**（其配套 hook `useAudioPlayer.ts` 也无人引用） |
| `frontend/next-env.d.ts` | 不存在，但 `frontend/tsconfig.json:25` 却 include 它；需 `next build` 生成，缺失时 `next/image` 等全局类型不可用 |
| `frontend/tsconfig.json:25-29` | `include` 的 `**/*.ts(x)` 会纳入 `tests/**`，`exclude` 只排除 `node_modules`、`src-tauri`，未排除 `tests/` |
| `frontend/tailwind.config.js:4-8` | `content` 只声明 `./src/pages`、`./src/components`、`./src/app`；`src/hooks`、`src/lib`、`src/contexts` 若含类名字符串不会被扫描（当前它们确实不含，属潜在陷阱） |

### 5.9 死代码清单（全量检索确认）

**零字节**：`frontend/src/components/AudioPlayer.tsx`、`frontend/src/config/api.ts`

**未被任何模块引用**：

- `frontend/src/components/DatabaseImport/LegacyDatabaseImport.tsx` 与 `HomebrewDatabaseDetector.tsx`（后者仅被前者引用，两者共同孤立；功能已被 `OnboardingContext.tsx:158-215` 内联取代）
- `frontend/src/components/BlockNoteEditor/BasicBlockNoteTest.tsx`
- `frontend/src/components/ConsoleToggle.tsx`、`CustomDialog.tsx`、`ModelDownloadProgress.tsx`、`BluetoothPlaybackWarning.tsx`
- `frontend/src/components/molecules/form-components/{form-input-item,form-input-switch,form-select-item}.tsx`
- `frontend/src/lib/builtin-ai.ts`（含 §3.3A 的未注册命令）
- `frontend/src/constants/modelDefaults.ts`
- `frontend/src/components/MainNav/index.tsx`
- 死 hooks：`useAudioPlayer`、`useNavigation`、`useProcessingProgress`、`useModelConfiguration`
- 死组件（仅被死文件引用）：`ChunkProgressDisplay`、`AudioLevelMeter`、`RecordingStatusBar` 系列需按调用点核对；已确认零引用的是 `ChunkProgressDisplay`/`ChunkProgressMini`

**导入但未使用（19 处）**：`CheckCircleIcon`、`ClipboardDocumentCheckIcon`（`AISummary/index.tsx:7`）、`CheckCircle2`×2、`Loader2`、`StickyNote`、`Input`（`Sidebar/index.tsx:4,32`）、`Settings`、`Button`、`useRef`、`ProcessRequest`、`RecordingPreferences`、`X`、`useCallback`、`useState`×2、`Transcript`、`ChunkStatus`、**`MessageToast`（`Sidebar/index.tsx:28` import 但从未使用 → `MessageToast.tsx` 成为孤立组件）**。

**死桥接**：`(window as any).openSettings`（`Sidebar/index.tsx:437`）、`(window as any).handleRecordingStop`（`useRecordingStop.ts:447`）— 两者在 Rust 侧**零调用点**。托盘改用 `window.eval("window.location.assign('/settings')")`（`frontend/src-tauri/src/tray.rs:47`）与 `window.eval("sessionStorage.setItem('autoStartRecording','true')")`（`tray.rs:110`）。

**死事件监听**：`layout.tsx:109-128` 的 `request-recording-toggle` 处理逻辑（全仓含 Rust 零发射）。

**死服务方法**：`recordingService.startRecording()`（`:79-81`）、`recordingService.stopRecording()`（`:107-111`）、`transcriptService.onModelDownloadComplete()`（`:99-103`）、`transcriptService.onTranscriptError()`（`:88-92`）、`transcriptService.onTranscriptionComplete()`（`:68-70`，`useRecordingStop` 绕过它直接 `listen`）、`BuiltInAIAPI` 全类、`updateService.wasCheckedRecently()`、`indexedDBService.getTranscriptCount()`、`ConfigContext.loadPreferences` 之外的 `isLoadingPreferences` 消费情况需另行核对。

### 5.10 代码自述与实现的分歧

| 位置 | 自述 | 实现 |
| --- | --- | --- |
| `frontend/src/contexts/RecordingStateContext.tsx:9` | "single source of truth for recording state" | `frontend/src/app/page.tsx:26` 另有局部 `isRecording` 并驱动控制流（§2.3） |
| `CLAUDE.md`（Frontend State Management） | "Sidebar Context：Global state for **recording status**" | 录音状态实际在 `RecordingStateContext`；`SidebarProvider.tsx:91` 只**读**它 |
| `CLAUDE.md`（Frontend Performance） | "React state updates batched via **Sidebar context**"、"Transcript rendering **virtualized**"、"Audio level monitoring throttled to 60fps" | 状态分散在 7 个 Context；虚拟化仅在详情页分页路径启用（`page-content.tsx:211`），主页 `TranscriptPanel` 走的是 `VirtualizedTranscriptView`（`app/_components/TranscriptPanel.tsx:108`）但 `page.tsx` 主路径用的是同一组件，需按页面区分；电平节流在 Rust 侧（`level_monitor.rs`），前端不参与 |
| `frontend/src/hooks/useRecordingStateSync.ts:10-13` | "Polls backend every 1 second to detect recording state changes" | 外层有 `(window as any).__TAURI__` 门槛（`:50`），Tauri 2 下为假 → 文档描述的轮询根本没启动 |
| `frontend/src/hooks/useTranscriptRecovery.ts:47` | 注释称 "Older than 15 seconds" | 代码是 `Date.now() - (2 * 1000)`（2 秒） |
| `frontend/src/components/RecordingControls.tsx:233` | `onTranscriptReceived={() => {}} // Not actually used` | 属实，但**保留了死 prop**（`SummaryResponse` 类型仍被 import，`:7`） |
| `frontend/package.json:5` | `"main": "electron/main.js"` | 仓库无 Electron，Tauri 2 项目 → 陈旧字段 |
| `frontend/src/services/configService.ts:16` | `@deprecated Use providerApiKeys from ConfigContext instead` | 但 `ModelConfig.apiKey` 仍在 4 处被读写（`Sidebar/index.tsx:126`、`ModelSettingsModal.tsx:211` 等），弃用标记未落地 |
| `frontend/src/components/TranscriptSettings.tsx:54-55` | `localWhisper: [] // Model selection handled by ModelManager component` | 属实；但同表的 `deepgram`/`elevenLabs`/`groq`/`openai` 选项对应的 provider 在 `getProviderCommands` 中不存在（§2.2） |

---

## 6. 测试现状核查

### 6.1 实际有什么

`frontend/tests/` 共 **10 个文件 / 1,074 行**。运行器是 **`bun:test`**（不是 jest/vitest）：9 个文件首行导入 `bun:test`，1 个 `.mjs` 用 `node:assert` + `typescript.transpileModule` 手搓 TS 加载器。

| 文件 | 行数 | 运行器 | 覆盖对象 |
| --- | --- | --- | --- |
| `tests/hooks/meeting-details-refresh.test.tsx` | 173 | `bun:test` | 会议切换时摘要刷新 |
| `tests/hooks/paginated-transcripts.test.tsx` | 158 | `bun:test` | `usePaginatedTranscripts` 请求序列与竞态 |
| `tests/hooks/summary-generation.test.tsx` | 273 | `bun:test` | `useSummaryGeneration` 轮询/取消/恢复 |
| `tests/lib/analytics.test.ts` | 88 | `bun:test` | `Analytics.trackSummaryGenerationCompleted` 秒数取整与 duration 省略 |
| `tests/lib/blocknote-markdown.test.ts` | 72 | `bun:test` | `blocksToMarkdownSafely` 成功/抛错两路 |
| `tests/lib/empty-state-summary.test.tsx` | 19 | `bun:test` | `EmptyStateSummary` SSR 标记含 `role="alert"` |
| `tests/lib/onboarding-summary-model.test.mjs` | 94 | `node:assert` | `resolveOnboardingSummaryModelStatus` |
| `tests/lib/summary-content.test.ts` | 32 | `bun:test` | `hasVisibleSummaryContent` / `parseSummaryContent` |
| `tests/lib/summary-language-preferences.test.js` | 130 | `bun:test` | 摘要语言偏好读写（自建 localStorage stub） |
| `tests/lib/transcription-model-readiness.test.ts` | 35 | `bun:test` | `getProviderCommands` / `hasDownloadingModel` |

测试技术：`react-test-renderer` 18.3.1（`package.json:111`）+ `bun:test` 的 `mock.module` 做模块级打桩。

质量细节值得肯定：`tests/hooks/summary-generation.test.tsx:9-10` 明确注释「Bun shares module mocks between test files; restore application modules after this suite」并在 `:11-17` 做 `afterAll` 恢复；`tests/lib/analytics.test.ts:8-9` 同样注明「模块必须在 Tauri invoke mock 注册后再 import」。作者清楚 bun 的 mock 全局性风险并做了处理。

### 6.2 但实际等同于「从未运行」

- **`package.json` 无 `test` script**：`:6-29` 共 21 个 script（`dev`/`build`/`export`/`start`/`tauri*` 13 个/`lint`），无一个测试相关 → `pnpm test` 不存在，团队无法用项目约定的包管理器运行。
- **无任何测试运行器依赖**：`devDependencies` 只有 `@types/bun@1.3.14`（`:103`），没有 `bun` 本体。
- **无 `bunfig.toml`、无 `vitest.config.*`、无 `jest.config.*`、无 `tests/setup*`**（全量检索确认）。
- 因此要跑必须外部安装 Bun 并执行 `bun test tests/`，**CI 与 pnpm 工作流均不覆盖**。git 历史 `e29aa31 fix(tests): resolve Bun TypeScript errors` 佐证测试是通过独立 bun 流程维护的。

### 6.3 覆盖率现实

| 维度 | 数值 |
| --- | --- |
| `src/` TS/TSX 文件 | 171 |
| `src/` 总行数 | 30,994 |
| 测试触及的源码模块 | 9（`analytics`、`blocknote-markdown`、`summary-content`、`transcription-model-readiness`、`EmptyStateSummary`、`summary-language-preferences`、`onboarding-summary-model`、`useSummaryGeneration`、`usePaginatedTranscripts`） |
| 模块覆盖比例 | ≈ 5.3%（9 / 171） |
| 测试总行数 / 源码总行数 | 1,074 / 30,994 ≈ **3.5%** |

**完全没有测试覆盖的关键路径**：

1. 录音生命周期——`useRecordingStart`(412)、`useRecordingStop`(470)、`RecordingStateContext`(355)、`RecordingControls`(524)。§5.2/§5.3 的 P0/P1 正在这一空白区。
2. 上下文层——`SidebarProvider`(324)、`ConfigContext`(554)、`TranscriptContext`(539)、`OnboardingContext`(652) 全部零测试。§2 的所有状态冲突都在这里。
3. 事件契约——没有任何测试断言「前端监听的事件名在后端有发射点」或反向。§3.3D 的 5 个死事件与 §3.3A 的 2 个未注册命令本可被一个契约测试拦住。
4. services 层——6 个服务 0 测试，包括有真实逻辑的 `withStartTimeout`（`recordingService.ts:33-44`）与 449 行的 `indexedDBService`。
5. 组件渲染——仅 `EmptyStateSummary` 一个有测试；`ModelSettingsModal`(1287)、`Sidebar`(882)、`AISummary`(758) 无任何渲染测试。
6. 无障碍——无 axe/aria 断言；唯一沾边的是 `tests/lib/empty-state-summary.test.tsx:12` 断言 `role="alert"` 字符串存在。

**结构性隐患**：`tests/hooks/summary-generation.test.tsx:9-10` 自述的「mock 跨文件共享」意味着用例之间存在**顺序耦合**，一旦并行或分片运行会互相污染。

**测试风格评价**：偏保守但质量不低——不渲染大型组件树，只测纯函数与两个 hook；用 `react-test-renderer`+`act` 而非 jsdom+testing-library；对模块 mock 的全局污染有显式恢复。所以问题不在测试质量，而在**测试范围与代码体量严重不匹配**，且**没有运行入口**。

---

## 7. 技术风险排序

| # | 风险 | 影响 | 证据 |
| --- | --- | --- | --- |
| **1** | **停止录音时「等待转录完成」逻辑完全失效**：`get_transcription_status` 恒返回零值桩，`transcription-complete` 在活跃 Rust 中零发射点，唯一等待是固定 4 秒 sleep。长录音/慢机器下会把不完整转录写入 SQLite，并紧接着 `markMeetingAsSaved()` 关闭 IndexedDB 恢复通道 | **数据丢失（不可恢复）** | `frontend/src/hooks/useRecordingStop.ts:158,166,170-173,208,241,297`；`frontend/src-tauri/src/lib.rs:270-277`；真实信号 `frontend/src-tauri/src/audio/transcription/worker.rs:339`（`transcription-queue-complete`）前端零监听 |
| **2** | **前端调用两个不存在的 Tauri 命令**：`api_get_auto_generate_setting` 从未实现且注册被注释（用户在设置弹窗看到的「自动摘要」开关状态不可信）；`builtin_ai_get_models_directory` 未实现未注册 | 功能静默失效 + 运行期错误 | `frontend/src/components/ModelSettingsModal.tsx:320`；`frontend/src/lib/builtin-ai.ts:96`；`frontend/src-tauri/src/lib.rs:723-724`；Rust 侧 `auto_generate_setting` 检索零结果 |
| **3** | **5 个前端监听的事件在活跃 Rust 中零发射点**（`transcription-complete`、`chunk-drop-warning`、`transcript-error`、`model-download-complete`、`request-recording-toggle`）→ 音频丢块告警、旧式转录错误、Whisper 下载完成自动关弹窗、托盘录音开关四条 UX 链路全是死代码 | 4 条 UX 链路静默失效；掩盖风险 1 | `frontend/src/hooks/useModalState.ts:109,173`；`frontend/src/hooks/useRecordingStop.ts:158`；`frontend/src/components/RecordingControls.tsx:255`；`frontend/src/app/layout.tsx:111`；仅 `frontend/src-tauri/src/lib_old_complex.rs:692,992,1103,2312` 有发射，而该文件**未被任何模块声明**（`lib.rs:37-56` 的 `pub mod` 列表不含它，全仓零引用） |
| **4** | **`useRecordingStop` 被实例化两次（`app/page.tsx:46` 与 `RecordingPostProcessingProvider.tsx:28`），去重守卫是实例级 `useRef`** → 主页按钮路径与 `recording-stop-complete` 事件路径可并发进入 `handleRecordingStop(true)` 并各自 `saveMeeting` | 重复落库风险；日志噪音翻倍 | `frontend/src/app/page.tsx:46`；`frontend/src/contexts/RecordingPostProcessingProvider.tsx:28,36-41`；`frontend/src/hooks/useRecordingStop.ts:73,256` |
| **5** | **`useModalState.ts:168-191` 的 `model-download-complete` 监听器确定性泄漏**：effect 无返回值、无 `cancelled` 守卫，且依赖 `[transcriptModelConfig, hideModal]` 使监听器随配置变更**线性增长**；另有 `AnalyticsProvider.tsx:126` 的 `beforeunload` 未清理 | 内存/回调堆积，同一事件被多次处理 | `frontend/src/hooks/useModalState.ts:168-191`；`frontend/src/components/AnalyticsProvider.tsx:58,126`（对比正确写法 `frontend/src/contexts/RecordingStateContext.tsx:251-329`） |
| **6** | **录音状态三源并存 + 一处同步已失效**：`app/page.tsx:26` 局部 `isRecording`、`RecordingStateContext` 全局、`useRecordingStateSync` 的 1s 轮询（因 `window.__TAURI__` 键名错误整体不执行）。`RecordingControls` 同时消费局部 `isRecording` 与全局 `isPaused`，且 `isRecordingDisabled` 恒为 `false` | 刷新后状态错位；停止后可立即重录；UI 与真实状态短暂不一致 | `frontend/src/app/page.tsx:26,42,230`；`frontend/src/components/RecordingControls.tsx:46-48`；`frontend/src/hooks/useRecordingStateSync.ts:50`；正确键见 `frontend/src/hooks/usePlatform.ts:8` |
| **7** | **`transcriptModelConfig` 双源并发写入，默认值互斥，且 UI 提供的 provider 集合大于引擎支持的集合**：`getProviderCommands()` 只认 `localWhisper`/`parakeet`，而设置页还提供 `deepgram`/`elevenLabs`/`groq`/`openai`；选中后者后录音被静默拒绝 | 设置页与主界面显示不同 provider；用户可选中一条无法录音的路径且无解释 | `frontend/src/contexts/ConfigContext.tsx:112-116,198-215`；`frontend/src/app/settings/page.tsx:38-46`；`frontend/src/lib/transcription-model-readiness.ts:11-22`；`frontend/src/hooks/useRecordingStart.ts:92-93`；`frontend/src/components/TranscriptSettings.tsx:53-60` |
| **8** | **`ConfigContext` 的 `useMemo` 因 `modelOptions` 每次渲染重建而永不命中缓存** → Context value 每次渲染都是新对象 → 所有 `useConfig()` 消费者被迫重渲染 | 全应用级性能退化 | `frontend/src/contexts/ConfigContext.tsx:370-378`（定义）、`:490-539`（`useMemo` 与依赖数组含 `modelOptions`） |
| **9** | **`AISummary` 撤销/重做使用陈旧历史栈**：`useEffect` deps 仅 `[summary]`，闭包读 `history`/`currentHistoryIndex`/`isUndoRedoing` | 连续编辑后 Ctrl+Z 跳到错误版本，可能静默丢弃用户摘要编辑 | `frontend/src/components/AISummary/index.tsx:77-85`（配合 `:87-103`） |
| **10** | **落库完全由前端进程内状态 + 4 秒固定等待决定**：崩溃/关窗即丢尾部；IndexedDB 旁路是 fire-and-forget 不重试；`sessionStorage` 随进程消失 | 崩溃场景下会议数据不可恢复 | `frontend/src/hooks/useRecordingStop.ts:241,244-260`；`frontend/src/contexts/TranscriptContext.tsx:326`；`frontend/src/app/page.tsx:70-106` |
| **11** | **无测试运行入口**：`package.json` 无 `test` script、无运行器依赖；`lint` 引用的 eslint 也未安装。10 个 `bun:test` 文件（3.5% 行覆盖）在 CI 与 pnpm 工作流中从不执行 | 回归只能靠手工；本表 1–10 项均无 CI 拦截 | `frontend/package.json:6-29,98-115`；`frontend/eslint.config.mjs:3,13`；`frontend/tests/**` |
| **12** | **可访问性缺口集中在核心操作**：160 个 `<button>` 仅 28 个有 `aria-label`/`title`；录音三按钮为纯图标、无 `aria-pressed`；实时转录无 `aria-live`（全仓仅 2 处）；`AISummary` 多块选择纯鼠标、键盘不可达且 Delete 被全局劫持；10 处阻塞式 `alert()` | 键盘/屏幕阅读器用户无法完成录音控制与摘要编辑 | `frontend/src/components/RecordingControls.tsx:338-342,386-457`；`frontend/src/components/Sidebar/index.tsx:461-535,666-676`；`frontend/src/components/AISummary/index.tsx:158-180,406-432,673-698` |
| **13** | **重复监听同一事件、互不知情**：`transcription-error` 3 处、`speech-detected` 3 处、`model-config-updated` 4 发 4 收。一次错误同时触发埋点、toast、modal 与 `onRecordingStop(false)`；`ConfigContext.tsx:332` 用不完整 payload 整体替换 `modelConfig` 会丢失 provider 自定义字段 | 行为叠加、难调试；配置字段静默丢失 | `frontend/src/components/RecordingControls.tsx:277`；`frontend/src/hooks/useModalState.ts:136`；`frontend/src/services/transcriptService.ts:78`；`frontend/src/contexts/ConfigContext.tsx:330-338` |
| **14** | **错误类型放宽集中在最复杂组件**：`page-content.tsx:38,46` 的 `meeting: any` / `segments?: any[]`；`BuiltInModelManager.tsx:180` 用 `as any` 伪造 `ModelStatus`；61 处 `any` 集中在 31 个文件 | 契约漂移不可见（§3.3G 的参数剥离本可被类型捕获） | `frontend/src/app/meeting-details/page-content.tsx:38,46`；`frontend/src/components/BuiltInModelManager.tsx:81,180`；`frontend/src/hooks/meeting-details/useMeetingData.ts:10`；`frontend/src/services/indexedDBService.ts:29,230` |
| **15** | **废弃代码与陈旧配置散落**：`serverAddress` 硬编码 `http://localhost:5167` 仅作门控、`transcriptServerAddress` 零消费者；`constants/modelDefaults.ts` 零引用而模型默认值三处硬编码为不一致的 `large-v3`；`package.json:5` 指向不存在的 `electron/main.js`；`src/config/api.ts` 与 `AudioPlayer.tsx` 为 0 字节；`LegacyDatabaseImport`/`HomebrewDatabaseDetector` 整组孤立；`lib/builtin-ai.ts` 整类死代码；657 处 `console.*` | 误导后续维护（改 `modelDefaults.ts` 无效果）；FastAPI 时代的「后端地址」概念持续污染架构认知 | `frontend/src/components/Sidebar/SidebarProvider.tsx:98,119-125`；`frontend/src/constants/modelDefaults.ts:10` vs `frontend/src/app/settings/page.tsx:43`、`frontend/src/contexts/ConfigContext.tsx:105`、`frontend/src/app/meeting-details/page.tsx:105`；`frontend/package.json:5`；`frontend/src/lib/builtin-ai.ts:66-97` |

---

## 摘要要点

- 架构：Next.js 14 App Router 全客户端渲染 + `output:'export'`，4 个页面（`/`、`/meeting-details`、`/settings`、`/notes/[id]` 是硬编码假数据占位页，但侧栏 `Sidebar/index.tsx:581-583` 有真实入口），11 层 Provider 嵌套，`reactStrictMode` 被全局关闭（`frontend/next.config.js:8`）。
- 规模（本轮实测口径 `(Get-Content).Count`，注意 `Measure-Object -Line` 会漏算空行而低估约 10%）：`src/**` 171 个 TS/TSX、30,994 行、**657 处 `console.*`**；`components/` 104、`contexts/` 7、`hooks/` 23、`services/` 6；最大文件 `ModelSettingsModal.tsx` 1287 行。
- 状态层实有 7 个 Context（需求点名的 4 个之外还有 `RecordingStateContext`、`RecordingPostProcessingProvider`、`ImportDialogContext`、`OllamaDownloadContext`），`RecordingStateContext` 才是设计上的单一真源，但 `frontend/src/app/page.tsx:26` 的局部 `isRecording` 与之并存，`RecordingControls` 同时消费两套。
- 契约硬伤（P0）：`api_get_auto_generate_setting`（`frontend/src/components/ModelSettingsModal.tsx:320`）与 `builtin_ai_get_models_directory`（`frontend/src/lib/builtin-ai.ts:96`）在前端被调用但 Rust 未实现且未注册；前者有真实用户路径且失败静默，真正生效的「自动摘要」是 `ConfigContext.tsx:161-167` 的 localStorage。
- 契约硬伤（P0）：`get_transcription_status` 注册的是恒返回零值的桩（`lib.rs:270-277`），加上 5 个前端监听的事件在活跃 Rust 中零发射（`transcription-complete`、`chunk-drop-warning`、`transcript-error`、`model-download-complete`、`request-recording-toggle`，发射代码只在未被任何模块声明的 `lib_old_complex.rs` 中）。
- 最严重缺陷（P0，数据丢失）：`frontend/src/hooks/useRecordingStop.ts:158-208` 的「等待转录完成」因桩实现与死事件而失效，唯一等待是固定 4 秒，随后 `:297` 关闭 IndexedDB 恢复通道；真正的完成信号 `transcription-queue-complete`（`src-tauri/src/audio/transcription/worker.rs:339`）前端零监听。
- P1：`useRecordingStop` 被实例化两次（`app/page.tsx:46` 与 `RecordingPostProcessingProvider.tsx:28`），`stopInProgressRef` 是实例级 `useRef` → 主页按钮路径与 `recording-stop-complete` 路径可并发 `saveMeeting`。
- P1：监听器泄漏两处确定性存在——`frontend/src/hooks/useModalState.ts:168-191`（async cleanup 被丢弃且监听器随 `transcriptModelConfig` 变化线性增长）、`frontend/src/components/AnalyticsProvider.tsx:126`（`beforeunload` 未清理）；同时应承认 `RecordingStateContext.tsx:251-329`、`useImportAudio.ts:79-158`、`layout.tsx:160-209` 等处写法正确。
- 性能：`frontend/src/contexts/ConfigContext.tsx:370-378` 的 `modelOptions` 每次渲染重建却被列入 `useMemo` 依赖（`:490-539`），导致 Context value 永不缓存，所有 `useConfig()` 消费者被迫随之重渲染；`RecordingStateContext.tsx:75-83` 把日志用的 `state.*` 放进 `setStatus` 的 deps，同样击穿 memo。
- 请求路径干净且参数命名正确：`frontend/src/**` 中 `fetch`/`axios`/`XMLHttpRequest`/`WebSocket` 零命中，数据面 100% 走 Tauri IPC；Tauri v2.6.3 默认 `ArgumentCase::Camel`（`tauri-macros-2.6.3/src/command/wrapper.rs:51`），前端 camelCase 传参与后端 snake_case 形参正确对应，未发现参数名不匹配。
- 类型与无障碍：`strict:true` 且零 `@ts-ignore/@ts-expect-error`，但 61 处 `any` 集中在最复杂组件（`page-content.tsx:38,46` 用 `meeting: any`，`BuiltInModelManager.tsx:180` 用 `as any` 伪造 `ModelStatus`）；160 个 `<button>` 仅 28 个有 `aria-label`/`title`，`aria-live` 全仓仅 2 处，`AISummary` 多块选择纯鼠标、Delete 被全局 `preventDefault` 劫持。
- 测试现实与配置熵：测试运行器是 `bun:test`（10 个文件 / 1,074 行覆盖 9 个模块，≈5.3% 模块、3.5% 行），真正的问题是 `package.json` **无 `test` script、无运行器依赖**、无测试配置且 `lint` 引用的 eslint 未安装；另有 `package.json:5` 指向不存在的 `electron/main.js`、`constants/modelDefaults.ts` 零引用而模型默认值三处硬编码为不一致的 `large-v3`、`src/config/api.ts` 与 `AudioPlayer.tsx` 为 0 字节、`LegacyDatabaseImport`/`HomebrewDatabaseDetector` 与 `lib/builtin-ai.ts` 整组孤立、`transcriptServerAddress` 零消费者、`useRecordingStateSync.ts:50` 的 `window.__TAURI__` 键名错误使该轮询整体失效。

---

## 附录 A — 合并与勘误说明（Lead 记录）

本文件由两轮独立分析合并而成，另附 Lead 的独立复核。为免后续误引，明确以下三条：

**A1. 已确认被合并的快照**：Lead 在时限压力下自写的 330 行版本已保留为 `06-frontend.coordinator-snapshot.md`，未丢弃。本文件（806 行）为更完整版本，冲突处以后者为准。

**A2. 对"云转写 provider 可选择"的修正（Lead 独立复核）**：本报告正文有一条"用户能选中一条永远无法录音的路径"的判断，**该判断不成立，需按下列事实收窄**：

- `components/TranscriptSettings.tsx:126-129` 的四个云端选项（`deepgram`/`elevenLabs`/`groq`/`openai`）**整段被 `{/* … */}` 注释掉**，`<SelectContent>` 中实际只有 `parakeet`（`:124`）与 `localWhisper`（`:125`）两项。
- 因此用户在**当前 UI 上无法选择**云转写 provider，不存在"选中后静默失败"的在用路径。
- 但残留脚手架仍存在且可误导后续开发：`:53-60` 的 `modelOptions` 仍列出 `deepgram`/`elevenLabs`/`groq`/`openai` 的模型名，`:61` 的 `requiresApiKey` 仍会为它们取 API Key，`:13` 与 `components/LanguageSelection.tsx:121` 的类型联合仍包含这四个值；而 `lib/transcription-model-readiness.ts:11-22` 的 `PROVIDER_COMMANDS` 只映射 `localWhisper`/`parakeet`。
- 结论应表述为：**"云转写是一条已铺好 UI 与 Key 管理、但被注释关闭且后端无路由的死支路"**，而非"用户可选中的在用故障"。这一状态恰好是功能⑥"预留"的现有基线——Phase 2 接入时应复用而非重建 `modelOptions`/`requiresApiKey`/`api_get_transcript_api_key` 这条链。

**A3. Lead 独立验证补充**：`.github/workflows/*.yml` 中按**词边界**检索 `bun` 为 **0 命中**（早前一次得到"138 命中"是 `ubuntu-latest` 的子串噪声），确认 `bun:test` 用例从未进入 CI；`package.json` 内 `bun` 仅出现一次（`:103` 的 `@types/bun`），全仓无 `bunfig.toml`/`bun.lockb`，构建脚本（`build.ps1`/`clean_run*.sh`）亦不使用 bun。
