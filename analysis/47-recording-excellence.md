# 47 · 录音效率、准确性与分应用录音

盘点用户提出的六项，逐项给出**当前真实状态**和**落地路线**。

---

## 一、先录音后转录 ✅ **已实现**

不是要做，是已经有了：

| 位置 | 内容 |
|---|---|
| `recording_preferences.rs` | `defer_transcription` 偏好 |
| `recording_commands.rs:352` | 读取偏好并传给 `start_recording` |
| `RecordingState.defer_transcription` | 注释：「captures audio only and skips realtime transcription」 |
| `RecordingSettings.tsx:155` | 开关 `handleDeferTranscriptionToggle` |

**默认值**需要确认 —— 如果是**开启**，那「减少系统负担」这条已经达成；如果默认关闭，用户需手动开。建议：**默认开启**，因为录音时用户通常只需要「录下来」，实时字幕是额外负担。

> 待确认：`RecordingPreferences::default()` 里 `defer_transcription` 的值。

---

## 二、分应用录音 ❌ **未实现，且是唯一的大工程**

### 当前架构的硬限制

```
cpal 0.15.3 → WASAPI 默认设备 loopback → 整个系统的混音
```

**cpal 不提供进程级 loopback**。要做到「只录某个应用的声音」，必须绕开 cpal 直接调 WASAPI COM。

### 技术路线（Windows 10 2004+）

```
1. 枚举有音频会话的进程
   IMMDeviceEnumerator → 默认输出设备 → IAudioSessionManager2
   → IAudioSessionEnumerator → IAudioSessionControl2::GetProcessId
   → OpenProcess → QueryFullProcessImageName → 进程名

2. 进程级 loopback 激活
   ActivateAudioInterfaceAsync(
     VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
     IID_IAudioClient,
     params = AUDIOCLIENT_ACTIVATION_PARAMS {
       ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
       ProcessLoopbackParams: {
         TargetProcessId,
         ProcessTreeScope: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
       }
     })

3. 之后与现有整系统 loopback 相同：IAudioClient → IAudioCaptureClient
```

### 需要新增的依赖

`windows` crate 的feature：`Win32_Media_Audio`、`Win32_System_Com`、`Win32_System_Threading`、`Win32_System_ProcessStatus`。

### 工程量估计

| 部分 | 估计 |
|---|---|
| 进程音频会话枚举 | ~120 行 + 单测 |
| 进程 loopback 激活与采集 | ~250 行 unsafe COM |
| 与麦克风混音（复用现有 pipeline） | 复用，改动小 |
| 进程选择 UI（下拉/搜索） | ~150 行前端 |
| **合计** | **约 1–2 天** |

### 风险

1. **无法在无应用播放音频时验证** —— 必须真的开一个播放器/会议软件来测
2. 某些应用（如微信、部分 Electron 应用）的音频会话进程与 UI 进程不同，需要**按进程树**包含子进程，否则录不到
3. Windows 10 2004 以下不支持，需降级到整系统 loopback
4. 会议软件（腾讯会议 / 飞书 / Zoom）常有自己的音频引擎进程，用户看到的名字可能不是他以为的那个

### 建议的落地顺序

1. **先做进程音频会话枚举**（只读、可测、无风险）→ UI 能显示「当前正在发声的应用」
2. 再做进程 loopback 采集 → 真正只录一个应用
3. 最后接进现有 pipeline（与麦克风混音）

**不要跳过第 1 步直接做第 2 步** —— 枚举能立刻验证「我们看到的应用列表是不是用户理解的那个」，避免采集做完才发现选错了进程。

---

## 三、录音准确性与稳定性 —— 已有基础，可针对性加强

| 已有 | 位置 |
|---|---|
| 设备热插拔检测 | `device_monitor.rs`（含 `consecutive_missing` 计数） |
| 麦克风断开回退 | `recording_commands.rs` 的 `IS_RECORDING_STOPPING` / StoppingGuard |
| 噪声抑制 / 高通 / EBU R128 归一化 | `pipeline.rs` |
| 蓝牙设备重采样修正 | `pipeline.rs`（16k/44.1k → 48k） |

**可能的加强点**（未验证，仅列出）：
- 设备切换时缓冲区是否会出现空洞/重复
- `consecutive_missing` 的阈值是否会导致误判（短暂抖动就被当成断开）

---

## 四、模型的转化能力 —— 现状

| 引擎 | 状态 |
|---|---|
| 本地 Whisper | ✅ `ggml-small.bin`(465MB) / `ggml-tiny.bin`(74MB) 已下载 |
| 本地 Parakeet | ✅ provider 存在 |
| 云端 | ✅ OpenAI / Groq / 硅基流动 |
| 火山引擎 | ❌ 需自有协议 + 双凭据 |

**提升方向**（按性价比）：
1. **换更大的本地模型** —— 当前是 `small`，`medium`/`large-v3` 明显更准，代价是速度与内存
2. **已有的词典功能**（`dictionary.rs`）作为 whisper `prompt` 注入，提升专有名词识别 —— 已实现，确认是否默认启用
3. **后处理修正**（`post_processor.rs` + `transcript_text.rs`）已能清理重复/标点噪音

---

## 五、减少系统负担 —— 可做但未做的

| 想法 | 现状 |
|---|---|
| 录音时不跑实时转写 | ✅ `defer_transcription` 已存在，**确认默认值** |
| 空闲时卸载模型 | ✅ `residency.rs` 有 `start_idle_reaper` |
| 本地模型按需加载 | ✅ 同上 |

---

## 六、参考其他录音 App 的功能

| 功能 | 我们 | 备注 |
|---|---|---|
| 分应用录音 | ❌ | 见第二节 |
| 录音暂停/继续 | ✅ | |
| 电平表 | ✅ | 刚修成真实电平 |
| 静音自动停止 | ✅ | `silenceStopSeconds` |
| 最长时长限制 | ✅ | `maxRecordingSeconds` |
| 磁盘空间监控 | ✅ | |
| 降噪 | ✅ | |
| 音频格式/质量选择 | ✅ | MP3/WAV |
| 说话人分离 | ✅ | silero + 嵌入聚类 |
| 实时字幕 | ✅ | |
| 录音后选择是否转写 | ✅ | `defer_transcription` |
| 跨设备同步 | ⚠️ | 只有 Obsidian/Notion 单向导出 |

---

## 结论

**六项里只有「分应用录音」是真正缺失的大功能**，其余要么已完成，要么是小幅加强。

建议优先做分应用录音的**第 1 步（进程音频会话枚举）**：风险低、可立即验证、且是后续采集的必要前提。
