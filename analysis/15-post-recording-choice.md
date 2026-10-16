# 录后可选转写/纪要、启动分析

对应需求：录后可选是否转写与生成纪要、本地/云端模型选择、快速启动。

## 一、先说结论：其中两项本来就已经有了

在动手前我查了现状，避免重复造轮子：

| 需求 | 实际情况 |
|---|---|
| **本地/云端模型选择** | **纪要侧已完整**：`ollama` / `groq` / `claude` / `openai` / `openrouter` / `builtin-ai` / `custom-openai` 全部已实现（`ModelSettingsModal.tsx`） |
| **转写侧 provider** | 已有 `localWhisper` / `parakeet` + 云端预留（`catalog.rs`，云 provider 一律 `NotImplemented` 且不可选） |
| **快速启动** | 启动流程**已经基本是异步的**，没有明显阻塞 |

所以我这轮的实际工作集中在**真正的缺口：录音结束时没有任何选择**。

## 二、录后选择：修掉一个真实的数据可达性缺陷

### 发现的问题

`useRecordingStop.handleRecordingStop` 的保存分支原本是：

```ts
if (isCallApi && transcriptionComplete == true) {
  // 保存会议到数据库
} else {
  setStatus(RecordingStatus.IDLE);   // ← 什么都不存
}
```

而"延后转写"模式（`defer_transcription`）下**录音期间不产生任何转写片段**，于是：

- 音频文件正常写入磁盘 ✅
- **但数据库里没有这条会议记录** ❌

后果：录音在 UI 里**不可见、打不开、无法重新转写**。这个模式此前等于把音频扔进磁盘就忘了。

### 顺带发现的第二个问题：延后模式会白等

停止流程里有一段等待转写完成的轮询：

```ts
const unlistenComplete = await listen('transcription-complete', ...);
while (elapsedTime < MAX_WAIT_TIME && !transcriptionComplete) { ... }
```

但**活跃代码里没有任何地方发出 `transcription-complete` 事件**——它只存在于已废弃的 `lib_old_complex.rs`。所以这个监听永远不触发，只能靠轮询里的其他条件退出。延后模式下更是完全没有东西可等，却仍要走一遍并多等 4 秒的"迟到的转写片段"宽限期。

### 实现

**1. 后端把本次会话的转写模式告诉前端**（`recording_commands.rs`）

`recording-stopped` 事件新增 `transcription_deferred` 字段。取自**会话状态**（`manager.get_state().is_transcription_deferred()`）而不是偏好设置——用户可能在录音过程中改了设置，停止流程必须反映**这次录音实际发生的事**。

**2. 前端在延后模式下保存会议**（空转写列表）

先确认了后端允许空转写：`TranscriptsRepository::save_transcript` **先插 meeting 行再循环插 transcript**，空列表只是不插 transcript，meeting 行照常创建。所以保存是安全的。

**3. 延后模式跳过等待**，不再白等那段宽限期。

**4. 停止后弹出选择对话框**（`PostRecordingDialog.tsx`）

| 选项 | 行为 |
|---|---|
| **Transcribe and summarise** | 先 `start_retranscription_command`，等完成，再交给会议页 |
| **Transcribe** | 同上，不生成纪要 |
| **Summarise** | 已有文本时只生成纪要（此时不显示"重新转写"，避免丢弃已有工作） |
| **Keep audio only** | 不做后续处理，会议页里随时可以再转写 |

判定逻辑是纯函数（`src/lib/post-recording.ts`，13 例测试），包括：
- 没有音频**也没有文本**时不弹窗（对空录音提问只会让人困惑）
- 已有文本时**不提供重新转写**（重复转写会丢掉用户已有的结果）
- 每个选项必带 label 与 description

**5. 纪要生成交给会议页**

我没有在 home 页重复实现纪要调用。`handlePostRecordingChoice` 完成后导航到 `/meeting-details?id=...&source=recording`，由会议页的既有 `shouldAutoGenerate` 逻辑配合 `isAutoSummary` 设置决定是否生成。这样用户不会被问两次（一次"要不要纪要"，一次设置里的"自动生成纪要"）。

## 三、快速启动：查了，但没改（附诚实说明）

### 现状核查

| 启动项 | 状态 |
|---|---|
| ONNX Runtime 初始化 | setup 内同步，但只是 `ort::init_from`，很轻 |
| 通知系统 | `spawn`，非阻塞 |
| `whisper_init` / `parakeet_init` | `spawn`，非阻塞 |
| `init_model_manager_at_startup` | `spawn`，非阻塞 |
| **数据库初始化** | **`block_on` 同步等待** ← 唯一同步阻塞项 |

关键点：**`tauri.conf.json` 里定义了窗口，Tauri 在 `setup` 返回前就已创建窗口**。所以 `block_on` 阻塞的是 setup 的完成，不是窗口的出现。用户看到窗口的时间不受它影响。

`initialize_database_on_startup` 本身也只是建连接池 + `app.manage()`，不做迁移扫描。

**所以我没有改动启动流程。** 没有测量数据支撑的改动只是凭感觉重构，风险大于收益。

### 如果你觉得启动仍然慢，我建议按这个顺序查（需要有数据）

1. **先测**：记录窗口出现（`Application setup complete`）到首页可交互的时间，以及各 `spawn` 任务各自的耗时。没有这两个数字，任何优化都是猜。
2. **最可能的可疑点**：四个 `spawn` 任务在启动时同时抢 CPU（ONNX 初始化 + 两个引擎 + ModelManager init），可能与 webview 首屏渲染争抢。可以考虑串行化或延迟到空闲时。
3. **其次**：`init_model_manager_at_startup` 会在启动时扫模型目录。若目录里有大模型，这一步可能不便宜，而它完全可以延迟到首次生成纪要时（代码注释也写着"will be lazy-initialized on first use"，说明本就允许懒加载）。

## 四、验证

| 项目 | 结果 |
|---|---|
| 前端 bun 测试 | **107 pass / 0 fail**（新增 13 例） |
| `post-recording` 测试 | **13 pass** |
| `tsc --noEmit` | exit 0 |
| `pnpm build` | ✓ 8/8 静态页 |
| `cargo check -p meetily` | Finished，0 error |
| `audio::formats` / `retention` / `disk_space` / `http_client` / `whisper_engine::download` / `storage::config` / `retranscription` | 22 / 21 / 5 / 16 / 8 / 7 / 15 —— **全通过** |
| 空转写列表保存会议 | 已核对后端实现：**先插 meeting 行再循环插 transcript**，空列表安全 |

## 五、一个诚实的缺口

**录后选择的端到端流程没有实机验证。** 需要真实录音才能走完：开延后转写 → 录一段 → 停止 → 看到对话框 → 选"转写" → 确认音频被转写。

类型检查、单测、构建都通过，但**这条路径依赖真实录音、真实音频文件和 Rust 命令**，静态验证覆盖不到。建议用 `pnpm run tauri:dev` 实测一次。
