# Meetily Rust 音频子系统技术分析

- 对象：`D:\Meetly`（git main，release v0.4.1）中 `frontend/src-tauri/src/audio/**` 与 `frontend/src-tauri/src/audio_v2/**`
- 方法：只读源码 + 交叉引用检索；所有非显然结论均给出 `path:line` 证据
- 范围外（未读）：`whisper_engine/`、`parakeet_engine/`、`database/`、`summary/`、`frontend/src`
- 说明：行号以本次 read 工具读到的文件内容为准；部分文件 read 报告的总行数与 `Measure-Object -Line` 计数不同（例如 `pipeline.rs` 为 1116 行、`recording_commands.rs` 为 1697 行），本文一律以 read 的行号为准

---

## 1. 模块清单与 `mod.rs` 接线

### 1.1 `audio/mod.rs` 实际声明的模块（`frontend/src-tauri/src/audio/mod.rs:1-52`）

`mod.rs` 用 34 个 `pub mod` 把子系统拆成以下几组（行号为 mod.rs 内行号）：

| 分组 | 模块 | 行号 |
|---|---|---|
| 基础 DSP / 编解码 | `audio_processing`、`decoder`、`encode`、`ffmpeg`、`vad` | 2-6 |
| 设备层 | `devices`、`capture`、`permissions` | 9-11 |
| 设备探测与诊断 | `device_detection`、`diagnostics`、`ffmpeg_mixer` | 14-16 |
| 实时录音核心 | `recording_state`、`pipeline`、`stream`、`recording_manager`、`recording_commands`、`recording_preferences`、`recording_saver`、`incremental_saver` | 19-26 |
| 电平/缓冲池/后处理 | `level_monitor`、`simple_level_monitor`、`buffer_pool`、`post_processor`、`hardware_detector`、`async_logger`、`batch_processor` | 27-33 |
| 系统音频与设备事件 | `system_detector`、`system_audio_commands`、`device_monitor`、`playback_monitor` | 34-37 |
| 转写 | `transcription` | 40 |
| 共享工具 / 常量 | `common`（`pub(crate)`）、`constants` | 43-46 |
| 批量路径 | `retranscription`、`import` | 49-52 |

随后 `mod.rs:54-120` 用大量 `pub use` 把子模块符号提升到 `audio::` 命名空间（设备、采集、系统音频检测、命令、录音组件、诊断、FFmpeg mixer、VAD、decoder、常量）。这是典型的"扁平再导出"风格，导致 `audio::` 成为约 60 个符号的宽接口。

### 1.2 子模块接线（二级 `mod.rs`）

- `audio/devices/mod.rs:4-8` 声明 `discovery / microphone / speakers / configuration / platform`，`devices/mod.rs:11-14` 逐项 re-export（未使用通配符，边界是清晰的）。
- `audio/devices/platform/mod.rs:3-20` 按 `#[cfg(target_os = ...)]` 只编译一个平台文件，并 re-export `configure_windows_audio/get_windows_device`、`configure_macos_audio`、`configure_linux_audio`。
- `audio/capture/mod.rs:3-8` 声明 `microphone / system / backend_config`，`core_audio` 仅在 macOS 编译；`:11-24` re-export。
- `audio/transcription/mod.rs:5-9` 声明 `provider / whisper_provider / parakeet_provider / engine / worker`；`:12-24` re-export trait、两个 provider、engine 函数、worker 入口。

### 1.3 关键发现：`audio_v2` 是完全未被编译的死子系统

`frontend/src-tauri/src/lib.rs:38-56` 的模块清单里**没有** `audio_v2`：

```
38: pub mod analytics;
...
40: pub mod audio;
41: pub mod config;
...
56: pub mod whisper_engine;
```

全仓库检索 `audio_v2|ModernAudioSystem|ModernAudioStream|LegacyBridge` 只命中 `audio_v2/` 自身（`audio_v2/lib.rs:16,21,67`、`audio_v2/stream.rs:29`、`audio_v2/compatibility.rs:10,15` 等），无任何外部引用。因此 `audio_v2/**`（9 文件、1393 行）从不参与编译。

更进一步的证据是它**已经无法编译**：`audio_v2/compatibility.rs:76` 调用 `saver.start_accumulation()`（无参数、返回 sender），而当前 `audio/recording_saver.rs:138-142` 的签名是 `start_accumulation(&mut self, auto_save: bool, receiver)`。这属于 2026 年前的旧 API 残留。该目录内还有 16 处 `TODO`（含 `audio_v2/lib.rs:94,101,108` 三个"Phase 2"未实现的方法体，`audio_v2/limiter.rs:8,20`、`audio_v2/resampler.rs:10,22,27`、`audio_v2/sync.rs:19,31`）。

仓库自己的 `frontend/src-tauri/CLEANUP_PLAN.md:10,14-15,37-40` 已记录此事（"Dual Audio Systems ... indicating migration in progress"、"20+ TODO comments across audio_v2 modules"、"Either complete audio_v2 migration or remove incomplete modules"），但该计划显然未执行。

### 1.4 未被 `mod.rs` 声明、从不编译的孤儿文件

| 文件 | 行数 | 证据 |
|---|---|---|
| `audio/core-old.rs` | 865 | 不在 `audio/mod.rs` 任何 `pub mod` 中；全仓库唯一引用是 `CLEANUP_PLAN.md:9,34` |
| `audio/recording_saver_old.rs` | 349 | 同上；`recording_saver_old.rs:114` 仍是旧的 `start_accumulation(&mut self) -> mpsc::UnboundedSender<AudioChunk>` |
| `audio/recording_commands.rs.backup` | 84 KB | 非 `.rs`，Cargo 永不编译 |
| `audio/stt.rs` | 399 | 不在 `mod.rs`；且 `stt.rs:1-11` 引用 `crate::audio_processing`、`crate::deepgram`、`crate::pyannote`、`crate::vad_engine`、`crate::whisper`、`screenpipe_core`、`candle_transformers` —— 这些模块在当前 crate 中都不存在，加上就会编译失败 |
| `audio/system_audio_stream.rs` | 232 | 不在 `mod.rs`；全仓库仅自引用（`system_audio_stream.rs:9,12,102,143`） |
| `src/lib_old_complex.rs` | 103 KB | 不在 `lib.rs`；仅 `CLEANUP_PLAN.md:9,34` 提及 |

`core-old.rs` 还包含 `cpal::host_from_id(cpal::HostId::ScreenCaptureKit)`（`core-old.rs:264,309,889`）—— 该 `HostId` 变体在 crates.io 的 cpal 0.15.3 中不存在（见 §3.4、§6.6），这进一步说明它是更早期环境的残留。

---

## 2. 音频流水线

### 2.1 端到端数据流（实时路径）

```
cpal 输入回调 (mic, 可能 44.1/16/8 kHz, N 声道)
  └─ AudioCapture::process_audio_data          pipeline.rs:399
       ├─ audio_to_mono                        pipeline.rs:406-410
       ├─ rubato SincFixedIn 持续重采样 ->48k  pipeline.rs:420-522
       ├─ HighPassFilter 80Hz                  pipeline.rs:529-533
       ├─ (RNNoise，编译期常量关断)             pipeline.rs:536-569
       ├─ LoudnessNormalizer (EBU R128)        pipeline.rs:572-584
       └─ RecordingState::send_audio_chunk     pipeline.rs:637 -> recording_state.rs:238
            ↓ tokio mpsc::unbounded_channel
  AudioPipeline::run (单 tokio task)           pipeline.rs:792
       ├─ AudioMixerRingBuffer::add_samples    pipeline.rs:845
       ├─ while can_mix() { extract_window; ProfessionalAudioMixer::mix_window }  pipeline.rs:848-851
       ├─ ContinuousVadProcessor::process_audio (48k -> 16k -> Silero)  pipeline.rs:860
       │    └─ >=800 samples 的 segment -> transcription_sender   pipeline.rs:865-881
       └─ 混合结果 -> recording_sender_for_mixed                   pipeline.rs:894-903
            ↓
  RecordingSaver 累积任务 (tokio::spawn)        recording_saver.rs:178-211
       └─ IncrementalAudioSaver::add_chunk      incremental_saver.rs:54
            └─ 每 30s: encode_single_audio -> ffmpeg aac  incremental_saver.rs:96-101, encode.rs:18
```

对应 CLAUDE.md 描述的"双路径"（录音路径 + 转写路径）在结构上成立，但见 §2.6 的多处细节不符。

### 2.2 线程与并发模型

- **同步原语**：`AudioCapture` 用 6 个 `Arc<std::sync::Mutex<Option<...>>>` 持有重采样器、重采样输入缓冲、RNNoise、HPF、EBU R128（`pipeline.rs:217-225`）。`RecordingState` 用 `AtomicBool` 表状态、`std::sync::Mutex` 保护设备/发送端/统计/时间戳（`recording_state.rs:97-123`）。全仓库音频热路径几乎不用 `RwLock`；CLAUDE.md「Thread Safety and Async Boundaries」所称"Use `Arc<RwLock<T>>` for shared state across async tasks"只在本模块的 `backend_config.rs:100` 与 `batch_processor.rs:16` 出现。
- **回调线程**：`build_stream` 的闭包直接调用 `capture.process_audio_data(data)`（`stream.rs:249-251`），即 **cpal 音频回调线程**上完成 mono 转换、rubato 重采样、两级 DSP、以及一次 `Mutex` 上锁+`mpsc::UnboundedSender::send`。这是标准的"回调里做重活"，是 §6/§7 中多个风险的共同根因。
- **tokio task**：`AudioPipelineManager::start` 里 `tokio::spawn(pipeline.run())`（`pipeline.rs:1022-1024`）；`RecordingSaver::start_accumulation` 里再 spawn 累积任务（`recording_saver.rs:178`）；`SimpleLevelMonitor` / `DeviceMonitor` / transcription worker 各自 spawn（见 §4、§6）。
- **通道全部无界**：`mpsc::unbounded_channel::<AudioChunk>()`（`pipeline.rs:1001`）、transcription 通道（`recording_manager.rs:237`）、recording 通道（`recording_manager.rs:238`）、worker 内部通道（`transcription/worker.rs:75`）、post-processor（`post_processor.rs:36-37`）、async logger（`async_logger.rs:25`）。CLAUDE.md `AudioError::BufferOverflow`（`recording_state.rs:47,64,80`）在无界通道下实际上不可达。

### 2.3 混音环形缓冲 `AudioMixerRingBuffer`

定义 `pipeline.rs:31-36`，实现 `pipeline.rs:38-156`。

- 构造：`let window_ms = 600.0;`（`pipeline.rs:41`），`window_size_samples = 48000*600/1000 = 28800`；`max_buffer_size = window_size_samples * 8 = 230400`（`pipeline.rs:48`）。
- **注释与常量严重不一致**：`pipeline.rs:40` 写"Use 50ms windows for mixing"，实际 600 ms；`pipeline.rs:44-47` 写"max buffer to 400ms ... // 400ms (was 200ms)"，实际 `max = window*8 = 4800 ms`；`pipeline.rs:91` 写"keep only last 200ms"，实际丢弃到 230400 samples。日志语句 `pipeline.rs:50-52` 自身也打印 `max={}ms` 且参数为 `window_ms * 8.0`，即它会诚实地打印 4800ms，而紧邻的注释说 400ms。CLAUDE.md:199 声称"aligned windows (50ms)"，与代码的 600 ms 也不符。
- `add_samples` 使用 `static mut SAMPLE_COUNTER: u64` + `unsafe`（`pipeline.rs:64-71`）。mic 与 system 是两个独立的 cpal 回调线程，**同一个 `static mut` 被两个线程无同步地 `+= 1`**，属于 Rust 定义的数据竞争（UB），其唯一用途是每 200 次打一条日志。
- 溢出时丢弃**最旧**样本（`pipeline.rs:92-97`），system 溢出用 `error!` 但结果同样是静默丢音（`pipeline.rs:85-89`）。
- `can_mix()` 只要**任一路**≥window 就为真（`pipeline.rs:100-103`）；`extract_window()` 对不足的一路 `drain(..)` 后 `resize(window, 0.0)` 零填充（`pipeline.rs:117-131`、`137-151`）。这意味着当 system 流落后时，它会把自己已有的样本"压进"更早的窗口、其余用静音补齐，**system 音频在录音时间轴上前移**，与 mic 产生累积性 A/V 失步。这是设计层面的缺陷，不只是常量问题。

### 2.4 混音器 `ProfessionalAudioMixer`

定义 `pipeline.rs:160-203`，`mix_window` 在 `pipeline.rs:167-202`：

```rust
181:            let sys_scaled = sys * 1.0;
182:            let _mic_scaled = mic * 0.8;  // Reserved for future mic scaling
184:            // Sum without ducking - mic stays at full volume, system slightly reduced
185:            let sum = mic + sys_scaled;
191:            let mixed_sample = if sum_abs > 1.0 {
193:                sum / sum_abs
```

- 注释 `pipeline.rs:178-180` 说"Pre-scale system audio to 70% to leave headroom"，代码是 `sys * 1.0`，且 `_mic_scaled` 完全未使用 —— **没有任何预缩放**。
- 注释 `pipeline.rs:184` 明说 `Sum without ducking`，即**没有 RMS 侧链闪避**。CLAUDE.md:200/277 与 README.md:186「intelligent ducking」「RMS-based ducking」在实时路径上不成立；真正实现 RMS ducking 的 `ffmpeg_mixer.rs:287-308`（`SPEECH_THRESHOLD = 0.01`、增益 0.60/1.0）是死代码（§2.6）。
- `sum / sum_abs` 并非注释所称的"soft scaling ... avoids hard clipping distortion"：它把峰值**精确归一到 ±1.0**，是一种逐窗口自适应增益（对每个越界窗口整体缩放到峰值为 1），会产生电平抽吸/非线性失真；且由于逐 600 ms 窗口独立决策，会引入窗口级增益跳变。真正的软限幅应保留波形形状，此实现不保留。
- 麦克风侧唯一的电平控制是 EBU R128 归一化（`pipeline.rs:310-319, 572-584`），目标 `TARGET_LUFS = -23.0`（`audio_processing.rs:191`），配 `TruePeakLimiter`（`audio_processing.rs:100-137`）。注意该 limiter 的"lookahead"实现为：把样本写入 `buffer[pos]`，同时把该样本自己的增益写入 `gain_reduction[pos]`，输出时读 `buffer[pos+1] * gain_reduction[pos+1]`（`audio_processing.rs:120-136`）。因此它是"延迟 10 ms 的逐样本限幅"，**没有前瞻增益平滑**，与 `audio_processing.rs:99` 的结构体注释所称"lookahead buffer (prevents clipping)"不符（它确实不削顶，但会产生逐样本增益突变）。

### 2.5 VAD 与分段

`vad.rs:21-321`，`ContinuousVadProcessor`（基于 `silero_rs`，`Cargo.toml:104`）。

参数（`vad.rs:36-88`）：`sample_rate = 16000`（`vad.rs:9,41`）、`positive_speech_threshold = 0.50`、`negative_speech_threshold = 0.35`、`pre_speech_pad = 300ms`、`post_speech_pad = 400ms`、`min_speech_time = 250ms`，`redemption_time` 由调用方传入。

redemption 策略是本次分析中少数**有明确设计文档且自洽**的部分：实时路径 `VAD_REDEMPTION_TIME_MS = 500`（`pipeline.rs:27`，并在 `pipeline.rs:744-757` 记录了从 400ms 改为 500ms 的实测依据：26 分钟会议被切成 322 段、中位 3.5 s，47% 的边界落在 0.42–0.75 s 区间），批量路径 `2000`（`import.rs:59`、`retranscription.rs:53`）。`pipeline.rs:1105-1115` 有断言锁住 500 这个值，`retranscription.rs:934-937` 锁住 2000。

**缺陷**：

1. `resample_to_16k`（`vad.rs:119-165`）是自研的"均值滤波 + 线性插值"，而非 sinc：
   - 截止频率那段数学是恒等式：`filter_size = (self.sample_rate as f64 / (cutoff_freq * self.sample_rate as f64)) as usize`（`vad.rs:134`）≈ `1/0.4 = 2`，**与采样率无关**，`cutoff_freq = 0.4`（`vad.rs:130`）实际不起作用，随后 `clamp(1, 5)`（`vad.rs:135`）。即"自适应抗混叠"是假的，永远是 2 抽头（5 点）均值。
   - 48 kHz→16 kHz 三倍抽取只用 5 点滑动平均，阻带衰减不足（均值滤波器第一零点后旁瓣约 −13 dB），低于同文件 `decoder.rs:74-77` 明确弃用线性插值的理由（"lacks an anti-aliasing filter, causing aliasing artifacts that make VAD miss ~99% of speech"）。同一仓库里 decoder 用 rubato sinc、VAD 用线性插值，标准不统一。
   - 该函数**无跨调用相位状态**，每个混音窗口（600 ms）重置相位与滤波器状态，窗口边界会产生不连续。
2. 能量门（`vad.rs:340-355`）注释与代码矛盾：注释 `vad.rs:345-347` 说"New values (0.03/0.08) are more balanced"、"Previous aggressive values (0.08/0.15)"，实际代码是 `if rms < 0.2 || peak < 0.20`（`vad.rs:348`）——三个数字全对不上。
3. `flush()`（`vad.rs:168-238`）用 `real_end_sample = processed_samples + buffer.len()` 与 `session.get_current_speech()` 长度做不变量校验，不足时返回 `Err`（`vad.rs:203-211`）。该错误在 `pipeline.rs:958-960` 只被 `warn!` 掉，即**录音结尾最后一段语音可能静默丢失**。
4. `extract_speech_16k`（`vad.rs:324-361`）使用 400 ms redemption（`vad.rs:325`），既不同于实时 500 也不同于批量 2000，且它已被 `mod.rs:114` 导出但**唯一调用点在死文件 `lib_old_complex.rs:513,547`** —— 实际是死代码。

### 2.6 CLAUDE.md 描述与代码的偏离汇总

| CLAUDE.md 断言 | 代码事实 | 证据 |
|---|---|---|
| "professional audio mixing (RMS-based ducking...)" | 实时混音器**无 ducking**（注释自认）；有 ducking 的实现是死代码 | `pipeline.rs:184-185`；`ffmpeg_mixer.rs:287-308` 无人调用 |
| "aligned windows (50ms)" | 600 ms 窗口，最大缓冲 4800 ms | `pipeline.rs:41,48` |
| `capture/microphone.rs # Microphone capture stream` | 4 行占位符 | `capture/microphone.rs:1-4` |
| `core_audio.rs # macOS ScreenCaptureKit integration` | 用的是 CoreAudio process tap（cidre `TapDesc`），**全仓库无任何 ScreenCaptureKit 代码** | `core_audio.rs:91`；检索 `ScreenCaptureKit|SCStream` 只命中命名/注释/死文件 |
| macOS "System Audio: Requires virtual audio device (BlackHole)" | 用 tap，不需要 BlackHole（BlackHole 只出现在 `device_detection.rs:172` 的虚拟设备识别名单与注释里） | `core_audio.rs:91,124-145` |
| Windows "System Audio: Uses WASAPI loopback" | 代码中无 loopback API；只枚举 output endpoint，然后对 output 设备调 `build_input_stream` | `devices/platform/windows.rs:15-23`；`stream.rs:247` |
| Linux "Audio Capture: ALSA/PulseAudio" | 用 `HostId::Alsa` 伪装 PulseAudio；只在 ALSA 输入里找名字含 "monitor" 的设备 | `devices/platform/linux.rs:18-27`；`devices/configuration.rs:158` |
| 引用 `AUDIO_MODULARIZATION_PLAN.md` | 该文件不存在 | `glob **/*MODULARIZATION*` 无结果 |
| "Batch audio metrics using AudioMetricsBatcher" | batcher 存在且被喂数据，但结果**从未被读取** | `pipeline.rs:820-831`；`batch_processor.rs:185-192` 无调用者 |
| "Pre-allocate buffers with AudioBufferPool" | 池只在 `RecordingState` 里被构造与 `clear()`，无任何取用 | `recording_state.rs:108,134,364`；检索 `PooledBuffer` 仅定义与 re-export |
| "Workers: 3"（事件载荷） / "optimized parallel transcription" | `NUM_WORKERS = 1`，且互斥锁包住唯一 receiver，物理上不可能并行 | `recording_commands.rs:474,664`；`transcription/worker.rs:74,76,122-125` |

---

## 3. 各平台采集实现与选择机制

### 3.1 编译期选择

- 设备枚举：`devices/platform/mod.rs:3-20` 用 `#[cfg(target_os=...)]` 只编译一个平台文件；`devices/discovery.rs:13-28` 按同一 cfg 分派 `configure_windows_audio` / `configure_linux_audio` / `configure_macos_audio`。
- 取设备与配置：`devices/configuration.rs:111-176`；Windows 走 `platform::get_windows_device`（`configuration.rs:114-117`），其他平台走 cpal default host（`configuration.rs:119-175`），其中 Linux 的 Output 分支再走 `HostId::Alsa`（`configuration.rs:155-170`）。
- CoreAudio tap 代码只在 macOS 编译：`capture/mod.rs:7-8,17-18`、`stream.rs:144-234`、`core_audio.rs:3,16-17`；非 macOS 提供返回错误的 stub（`core_audio.rs:383-418`）。
- `capture/system.rs` 的非 macOS 分支直接 `bail!`：`system.rs:99-103` `"System audio capture not yet implemented for this platform"`。

### 3.2 运行时选择

`capture/backend_config.rs` 定义 `AudioCaptureBackend::{ScreenCaptureKit, CoreAudio}`（macOS 下才有 `CoreAudio`，`backend_config.rs:10-19`），默认值：macOS = `CoreAudio`（`backend_config.rs:77-83`），非 macOS = `ScreenCaptureKit`（`backend_config.rs:81-82`）。全局实例是 `once_cell::Lazy<Arc<BackendConfig>>` + `RwLock`（`backend_config.rs:99-135`），读取在 `stream.rs:49`、`stream.rs:381-382`。前端可改：`recording_preferences.rs:292-345`（`set_audio_backend`）→ `capture::set_current_backend`（`recording_preferences.rs:331`）；读回时 `load_recording_preferences` 会用当前全局值覆盖持久化值（`recording_preferences.rs:114-118`）。

**选择逻辑的实际后果**（`stream.rs:54-103`）：

```rust
66:        #[cfg(target_os = "macos")]
67:        let use_core_audio = device_type == DeviceType::System
68:            && backend_type == AudioCaptureBackend::CoreAudio;
70:        #[cfg(not(target_os = "macos"))]
71:        let use_core_audio = false;
...
84:        #[cfg(target_os = "macos")]
85:        if use_core_audio { ... create_core_audio_stream ... }
...
92:        let backend_name = if backend_type == AudioCaptureBackend::ScreenCaptureKit {
93:            "ScreenCaptureKit"
94:        } else { "CPAL (default)" };
```

`ScreenCaptureKit` 分支**没有任何实现**，只是把日志字符串写成 "ScreenCaptureKit"，然后落到 `create_cpal_stream`（`stream.rs:102`）。在 macOS 上把 backend 切成 ScreenCaptureKit，system 设备（`DeviceType::Output`）会拿到 `default_output_config()`（`configuration.rs:138-152`），随后 `build_stream` 对**输出设备**调 `device.build_input_stream(...)`（`stream.rs:247`）—— 必然失败。因此"ScreenCaptureKit backend"是一个幽灵后端。

### 3.3 macOS：CoreAudio process tap（唯一真正实现的系统音频采集）

`core_audio.rs:57-151`：取默认输出设备 → `ca::TapDesc::with_mono_global_tap_excluding_processes(&cidre::ns::Array::new())`（`core_audio.rs:91`）→ 建 aggregate device（只挂 tap list，注释说明曾因同时挂 output device 导致回声，`core_audio.rs:113-145`）→ `ca::device_start`（`core_audio.rs:225`）。采样率由 tap 的 ASBD 决定（`core_audio.rs:246-270`），单声道（`stream.rs:172-180` 传 `channels = 1`）。

**线程模型与热路径问题**：

- IO proc 回调 `audio_proc`（`core_audio.rs:158-201`）运行在 CoreAudio 实时线程上：`let ctx = ctx.unwrap();`（`core_audio.rs:167`）在 `extern "C" fn` 内 panic → 跨 FFI 展开（Rust 1.81+ 为 abort）。
- 回调内进入 `process_audio_data`（`core_audio.rs:300-332`）：`ctx.producer.push_slice(data)` 写入 `ringbuf::HeapRb<f32>`（无锁，好），但随后为唤醒消费者获取 `ctx.waker_state.lock().unwrap()`（`core_audio.rs:319`）—— **在实时音频线程上取 `std::sync::Mutex`**，且 `.unwrap()` 在回调里 panic。这是实时线程优先级反转与进程级 abort 的双重风险。
- 环形缓冲 `let buffer_size = 1024 * 128; // 128KB buffer`（`core_audio.rs:261`）：`HeapRb::<f32>::new(131072)` 实为 **512 KiB**，注释少算 4 倍。
- 背压策略：连续 10 次写入不满即置 `should_terminate`（`core_audio.rs:306-312`），`poll_next` 在缓冲排空后返回 `Ready(None)`（`core_audio.rs:356-362`）。流结束后 `stream.rs:222` 只打一条 `info!("⚠️ Stream: Core Audio processing task ended")`，**上层 `AudioCapture`/管道完全不知情**，录音会在无系统音频的情况下继续，直到用户停止。
- 消费端是**逐样本**的 `Stream<Item = f32>`（`core_audio.rs:343-373`）：每取 1 个 f32 就走一次 `poll_next`，缓冲空时取 Mutex、clone waker 并返回 `Pending`。48 kHz 下即每秒 48 000 次 poll/wake；`stream.rs:200-214` 的消费者按 1024 采样聚批，但聚合发生在逐样本 await 之后。

### 3.4 Windows

`devices/platform/windows.rs` 只做两件事：用 `cpal::host_from_id(cpal::HostId::Wasapi)` 枚举 in/out endpoint（`windows.rs:12-38`），以及按名字为设备挑一个 "最兼容" 的配置（`windows.rs:97-256`，F32/2ch 优先、再任意 F32、再 `configs[0].with_max_sample_rate()`）。

- `windows.rs:15` 注释 "Add output devices (including loopback)"、`windows.rs:19-21` "mark output devices specifically for loopback"、`windows.rs:195` "use them in loopback mode" —— **代码里没有任何 loopback 相关 API**。全仓库检索 `loopback|AUDCLNT|IMMDevice|audioclient`：命中仅为注释、`device_detection.rs:175` 的虚拟设备名单、`level_monitor.rs:224` 的"输出设备无法监控"说明，以及 `parakeet_engine.rs:1303` 的测试服务器。
- 因此 Windows 上 system 路径是：`DeviceType::Output` → `get_windows_device` 返回**输出**设备 + `default_output_config()`（`windows.rs:231-250`）→ `create_cpal_stream` → `build_stream` → `build_input_stream`（`stream.rs:247`）。在 crates.io 的 cpal 0.15.3 上这必然报错；只有打了 WASAPI loopback 补丁的 cpal 才可能工作，而该补丁**没有生效**（§6.6）。
- `windows.rs:118,192` 用 `name == base_name || name.contains(base_name)` 匹配设备 —— `contains` 方向反了，短名设备会误匹配到长名设备（例如 base "Microphone" 命中 "Microphone Array (USB)"），且取第一个命中即返回。

### 3.5 Linux

`devices/platform/linux.rs:7-32`：输入设备来自 default host；"PulseAudio monitor sources" 却用 `cpal::host_from_id(cpal::HostId::Alsa)`（`linux.rs:18`），并且只保留名字含 `"monitor"` 的设备（`linux.rs:22`）—— ALSA host 暴露的是 `hw:`/`plughw:` 设备，不含 PulseAudio monitor，故该分支在典型环境枚举不到任何系统音频设备。同样的问题复制在 `devices/configuration.rs:155-170`（`HostId::Alsa` 再按 `default_input_config()` 取配置）。
另外 `linux.rs:11,19` 两个 `?`：任一枚举失败会让整个函数返回 `Err`，连已收集的输入设备一起丢掉（`discovery.rs:19-22` 直接 `?` 上抛）。

`capture/system.rs:99-103` 进一步确认在非 macOS 上 `SystemAudioCapture::start_system_audio_capture` 直接失败；而 `stream.rs` 并不走 `capture/system.rs`，走的是 CPAL 路径（`stream.rs:106-141`），所以 `capture/system.rs` 的 `SystemAudioStream`（`system.rs:115-139`）实际上只服务于已死的 `system_audio_stream.rs:28-29`。

### 3.6 权限

`permissions.rs`：`check_screen_recording_permission()` 在 macOS 与非 macOS **恒返回 `true`**（`permissions.rs:18-30`），其文档注释自认"Always return true - the actual permission dialog is triggered by Core Audio API"（`permissions.rs:23-24`）。于是 `ensure_screen_recording_permission()`（`permissions.rs:64-77`）恒走第一分支返回 true，`request_screen_recording_permission` 永不被调用。`trigger_system_audio_permission` 只能靠"能否创建 tap"推断权限，且注释自认"even with permission denied, tap creation may succeed"（`permissions.rs:108-109`）—— 即无法检测静音式拒绝。

有实际作用的只有麦克风校验 `devices/discovery.rs:112-165`：`spawn_blocking` 内建测试流、等首个回调最多 5 s（`discovery.rs:148-155`），失败即让录音启动硬失败（`recording_commands.rs:290-293`）。

`discovery.rs:31-39` 把 `host.devices()` 中未出现的设备**一律标为 `DeviceType::Output`**—— `host.devices()` 同时包含输入设备，方向信息被丢弃，会产生被标成输出的麦克风条目。

---

## 4. 录音生命周期

### 4.1 状态机（真实实现）

状态分散在三处，靠约定同步，没有单一状态机对象：

| 位置 | 含义 |
|---|---|
| `IS_RECORDING: AtomicBool`（`recording_commands.rs:39`） | 前端可见的"是否在录音" |
| `IS_RECORDING_STOPPING: AtomicBool` + `StoppingGuard`（`recording_commands.rs:46,79-90`） | 停止尾部窗口，防 mic-fallback 误触发 |
| `RECORDING_MANAGER: Mutex<Option<RecordingManager>>`（`recording_commands.rs:103`） | 全局单例 manager |
| `RecordingState`（`recording_state.rs:95-124`） | 每会话状态（设备、audio_sender、暂停计时、错误计数） |
| `IncrementalAudioSaver::checkpoint_buffer`（`incremental_saver.rs:19-26`） | 落盘进度 |

`start_recording` 用 `Acquire`/`Release`（`Ordering::SeqCst`）而非 CAS，靠 `common::acquire_engine_lifecycle_lock()`（`recording_commands.rs:316,506`；`common.rs:10-15`）串行化两个启动入口，才使 line 319/509 的 `load` 检查不构成 TOCTOU —— 因为 `IS_RECORDING` 直到尾部 `finalize_recording_start()`（`recording_commands.rs:95-100,427,614`）才置 true，而该调用发生在持锁期间、锁在其后才释放（`recording_commands.rs:428,615`）。这个不变式没有任何注释或测试保护，属于"靠顺序巧合成立"的设计。

**缺陷（潜在却真实）**：`stop_recording` 在 `stop_streams_and_force_flush()` 返回 `Err` 时直接 `return Err`（`recording_commands.rs:731-734`），此时：
- manager 已在 `recording_commands.rs:701-704` 被 `take()` 出全局并在 return 时 Drop（`recording_manager.rs:613-618` 只调用 `state.cleanup()`）；
- `IS_RECORDING` 仍为 `true`（置 false 在 `recording_commands.rs:1051`，永不执行）；
- `_stopping_guard` 在 return 时把 `IS_RECORDING_STOPPING` 清回 false。

结果：`is_recording()` 永久返回 true（`recording_commands.rs:1101-1103`），此后任何 `start_recording` 都会被 `recording_commands.rs:319-323` 以 "Recording already in progress" 拒绝，只能重启应用。当前 `stop_streams_and_force_flush` 内部把子步骤错误都降级为日志（`recording_manager.rs:349-357`），所以该路径难触发——这正是它未被发现的原因。

### 4.2 启动序列

`RecordingManager::start_recording`（`recording_manager.rs:228-306`）：
1. 建 transcription / recording 两条无界通道（`recording_manager.rs:237-238`）；
2. `state.start_recording()` 记 `recording_start`（`recording_state.rs:147-154`）；
3. `InputDeviceKind::detect(name, 512, 48000)` 对 mic/system 各做一次设备分类（`recording_manager.rs:247-259`；实现见 `device_detection.rs:37-69`）；
4. `pipeline_manager.start(..., 0, 48000, Some(recording_sender), ...)`（`recording_manager.rs:264-274`）—— `target_chunk_duration_ms = 0` 被显式忽略（`pipeline.rs:769-770`），采样率硬编码 48000；
5. `recording_saver.start_accumulation(auto_save, recording_receiver)`（`recording_manager.rs:279`）；
6. `tokio::time::sleep(50ms)`（`recording_manager.rs:286`）；
7. `stream_manager.start_streams(mic, sys, None)`（`recording_manager.rs:290`）—— **第三个参数是 `None`**，即 `AudioCapture.recording_sender` 永远为 None；原始流不进录音文件（与 `pipeline.rs:631-634` 的注释一致，只有混音后音频入盘）；
8. `device_monitor.start_monitoring`（`recording_manager.rs:293-300`），失败仅 warn。

macOS 额外前置：`prepare_audio_for_recording`（`recording_commands.rs:271-295`）先做 300 ms 静音"唤醒"（`recording_manager.rs:53-123`，含 `std::thread::sleep` 但已用 `spawn_blocking` 包裹），再做麦克风回调校验。该路径的注释（`recording_manager.rs:31-51`）记录了 BT 输出导致 60–90 s 静音采集的现象与实验性 workaround。

### 4.3 停止序列

`stop_recording`（`recording_commands.rs:676-1098`，约 420 行）：
1. `manager.take()`（`:701-704`）→ `StoppingGuard::new()`（`:711`）；
2. `manager.stop_streams_and_force_flush()`（`:716` → `recording_manager.rs:335-365`）：先停 device monitor（注释解释 Windows 上否则 WASAPI 轮询会让关闭拖 90 s，`recording_manager.rs:338-339`）→ `state.stop_recording()` → `stop_streams()` → `pipeline_manager.force_flush_and_stop()` → `state.cleanup()`；
3. flush 机制：往管道发 `chunk_id` 为 `u64::MAX` 及 `u64::MAX-1/-2/-3` 的空 chunk（`pipeline.rs:1060-1088`），管道用 `chunk.chunk_id >= u64::MAX - 10` 识别（`pipeline.rs:808`）；中间用 `sleep(20ms)`、`sleep(10ms)`（`pipeline.rs:1075,1091`）这种人为等待来"保证"被处理。这是脆弱的时序约定，但判定条件本身是确定性的。
4. 注销 transcript 监听（`:739-745`）；
5. 等 transcription task，上限 600 s（`:790-805`），并 spawn 一个每 500 ms 发进度的 task，事后 `abort()`（`:768-808`）；
6. 读取 provider 配置（`:826-846`，30 s 超时）→ 卸载 Whisper 或 Parakeet（`:848-900`）；
7. 采集 analytics（`:902-1003`）—— 停止路径里又两次查 DB（`:943-967`）；
8. `manager.save_recording_only(&app)`，5 分钟超时（`:1023-1041`）；
9. `IS_RECORDING = false`（`:1051`）→ 发 `recording-shutdown-progress` / `recording-stopped`（`:1073-1091`）。

**问题**：
- `progress_task` 的 `let last_update = Instant::now()` 在循环外（`recording_commands.rs:769`），所以 `elapsed_seconds` 是一直增长的绝对耗时，与字段名"elapsed"相符但与"progress"语义无关；它每 500 ms 无条件 emit 一次（`:776-785`），本身无害。
- 停止路径包含 **3 次 DB 往返 + 2 次网络 analytics 上报**（`:826-846`、`:943-967`、`:984-1002`），其中 `track_meeting_ended` 无超时包裹（`:984-1002`）。若分析后端不可达，停止会挂在网络超时上，而 `IS_RECORDING` 此刻仍是 true（`:1051` 未执行），前端"停止中"会一直转。
- `stop_streams_only`（`recording_manager.rs:309-332`）与 `stop_recording`（`:394-430`）与 `cleanup_without_save`（`:540-558`）是三条高度重复的停止路径，代码形状几乎相同但顺序/日志不同，属于明确的重复。

### 4.4 落盘

- 累积任务：`recording_saver.rs:178-211`，每收到一个 chunk 先读 `is_saving`，false 即 `break`（`:183-191`）—— 会**丢弃通道中剩余 chunk**。
- `IncrementalAudioSaver`：`checkpoint_interval_samples = sample_rate * 30`（`incremental_saver.rs:44`；由 `recording_saver.rs:233` 传入 48000，即 1 440 000）。每次 `add_chunk` 都对整个缓冲做一次 `iter().map(len).sum()`（`incremental_saver.rs:63-66`），O(n²) 但 n 上限 30 s/chunk，可忽略。
- checkpoint 编码：`encode_single_audio`（`incremental_saver.rs:96-101`）→ `std::process::Command` + `-c:a aac -b:a 192k -movflags +faststart -f mp4`（`encode.rs:37-58`），数据经 `stdin` 管道喂入（`encode.rs:76-83`）。
- 合并：`finalize()` → `merge_checkpoints()` 用 ffmpeg concat demuxer `-c copy`（`incremental_saver.rs:149-215`），列表文件用 `canonicalize()` 的绝对路径（`:166`），完成后 `remove_dir_all(.checkpoints)`（`:137`）。
- 崩溃恢复：`recover_audio_from_checkpoints`（`incremental_saver.rs:240-371`）按文件扫描 + 排序 + concat，时长估算为 `chunk_count * 30.0`（`:286`）—— 对最后一个不满 30 s 的 checkpoint 会高估。
- 元数据/转写写盘：`write_metadata`（`recording_saver.rs:268-277`）与 `write_transcripts_json`（`:280-332`）都是 `std::fs::write` + `std::fs::rename` 的原子写。**`write_transcripts_json` 在每新增一个 segment 时被整文件重写一次**（`recording_saver.rs:112-116` → `:280-331`）：O(n) 次 O(n) 写盘，长会议下是 O(n²) I/O，且是**同步阻塞 I/O**，调用点在 `transcript-update` 事件监听器里（`recording_commands.rs:442-464` → `:460`）。

### 4.5 设备断连

- 监控：`AudioDeviceMonitor::monitor_loop`（`device_monitor.rs:272-398`）轮询 `list_audio_devices()`，`check_interval = 2s`（`:280`）；macOS 额外注册 CoreAudio `HW_DEVICES` 属性监听以即时唤醒（`:283`，回调 `:127-137`，`DeviceChangeListenerGuard` 故意泄漏 ~100 字节以规避 use-after-free，`:104-123` 有详细说明）。
- **死代码**：`next_interval` 计算了"有设备缺失时 2 s、全部在线时 5 s"的自适应轮询（`device_monitor.rs:387-396`），但 `check_interval` 是不可变的 `let`（`:280`），`select!` 用的始终是它（`:296`）——**自适应策略从未生效，监控在整个录音期间恒以 2 s 全量枚举设备**。这正是 `recording_manager.rs:338-339` 注释所称"continuous WASAPI polling ... 90+ seconds"关闭慢的成因。
- 阈值：BT 3 次、有线 2 次（`device_monitor.rs:63-70`）；BT 判定是独立于 `device_detection` 的第二套名字启发式（`device_monitor.rs:50-52`，只有 airpods/bluetooth/wireless 三个关键词），与 `device_detection.rs:120-168` 的三级名单重复且不一致。
- 事件消费：`spawn_device_event_processor`（`recording_commands.rs:1424-1468`）只对麦克风断连动作，重连按产品决策忽略（`:1455-1460`）。
- 恢复：`trigger_mic_fallback_to_default`（`recording_commands.rs:1483-1697`）用 `MIC_SWAP_IN_PROGRESS` CAS 防重入（`:1504-1513`）+ `SwapGuard`（`:1517-1523`）+ 3 次预算（`:1285-1286`）；`do_mic_swap` 分三段锁（`:1337-1407`），明确不在持 `RECORDING_MANAGER` 时做 cpal I/O，并有注释解释（`:1351-1355`）。`session_live` 用 `Arc::ptr_eq` 校验会话身份（`:62-69`），并有"不得在持锁时调用"的显式约定（`:58-61`）。这部分是整子系统里工程质量最高的代码。
- 错误聚合：`RecordingState::report_error` 在可恢复错误累计 ≥10 或总错误 ≥15 时直接 `stop_recording()`（`recording_state.rs:275-278,293-296`）。注意 `AudioError::DeviceDisconnected` 被标为**可恢复**（`recording_state.rs:56`），因此断连 10 次后录音会自行终止而不通知用户去换设备；`user_message` 只走一次 `recording-error` 事件（`recording_commands.rs:399-401`）。

---

## 5. 导入与重转写

### 5.1 `import.rs`（1323 行）

流程：`select_and_validate_audio_command`（`:918-954`）/ `validate_audio_file_command`（`:957-961`）→ `start_import_audio_command`（`:964-990`，spawn 后立即返回）→ `start_import`（`:255-309`，`ImportGuard` CAS）→ `run_import`（`:312-675`）：

1. 建 meeting 目录 `create_meeting_folder(&base, &title, false)`（`:344`）；
2. `std::fs::copy` 到目标（`:360-363`，已包 `spawn_blocking`）；
3. `decode_audio_file_with_progress`（`:385-389`，`spawn_blocking`）；
4. `to_whisper_format_with_progress`（`:413-417`，`spawn_blocking`）；
5. `get_speech_chunks_with_progress(..., 2000ms, cb)`（`:434-455`，`spawn_blocking`）；
6. 25 s 以上 segment 用 `split_segment_at_silence` 切分（`:526-543`）；
7. 逐段 `transcribe_audio_with_confidence` / `transcribe_audio`（`:583-597`）；
8. `create_meeting_with_transcripts` 单事务写 meetings + transcripts（`:691-749`）；
9. 写 `transcripts.json` / `metadata.json`（`:652-665`）。

**缺陷**：
- **在 async 上下文里做全文件阻塞解码**：`validate_audio_file` 在元数据取不到时长时 `decode_audio_file(path)?`（`:173-182`）。它被 `select_and_validate_audio_command`（async）与 `validate_audio_file_command`（async）直接调用（`:941`、`:960`），**没有 `spawn_blocking`**。对一个 20 GB 上限的文件，这会在 tokio 工作线程上阻塞数分钟并把全部 PCM 载入内存，仅为了拿一个时长。注意 `extract_duration_from_metadata` 在 MP3 等无 `n_frames` 的格式上必然失败（`:239-242`），所以回退路径是常规路径。
- 20 GB 上限（`:62`）与**不可取消**的复制：复制期间不检查 `IMPORT_CANCELLED`（`:360-372`，检查在复制之后）。
- `parakeet_engine.as_ref().unwrap()` / `whisper_engine.as_ref().unwrap()`（`:584,591`）依赖"engine 为 None ⇔ total_segments == 0 ⇒ processable_segments 为空"这条间接不变式；一旦 `total_segments > 0` 而某分支使 engine 为 None，就是 panic（同一模式的 `retranscription.rs:373,380` 亦然）。
- 取消时 `remove_dir_all(&meeting_folder)`（`:370,401,427,505,554,627`）会**连带删除目录里其它内容**，且未校验 `meeting_folder` 确为本次创建的目录（`create_meeting_folder` 返回的是新建路径，风险低但无防护）。

### 5.2 `retranscription.rs`（1054 行）

流程与 import 高度平行：`find_audio_file`（`:142-170`，先试 10 个固定名再扫扩展名）→ `decode_audio_file`（`:202-206`）→ `to_whisper_format`（`:222-226`）→ VAD（`:242-264`）→ 分段转写（`:343-401`）→ **事务内 `DELETE`+逐条 `INSERT` transcripts**（`:433-461`）→ 写 `transcripts.json`/`metadata.json`（`:472-490`）。解码、重采样、VAD 都正确使用了 `spawn_blocking`（`:202,222,242`），这一点比 import 的校验路径规范。

**缺陷**：
- **TOCTOU**：`start_retranscription_command` 先查 `RETRANSCRIPTION_IN_PROGRESS`（`:791-793`）再 `tauri::async_runtime::spawn`（`:799`）并**立刻返回 `Ok(RetranscriptionStarted)`**（`:817-820`）。真正的 `ImportGuard` 式 CAS 在被 spawn 的任务里才执行（`:100`）。两次快速调用会双双收到"已启动"，第二个任务随后立即失败，错误只写日志（`:812-814`）。
- 序列化错误：`-c copy` 拼接的 checkpoint 与 `audio.mp4` 的时长语义在 `write_retranscription_metadata` 里被覆盖为**重新解码得到的** `duration_seconds`（`:740`），若 HE-AAC 等场景下 `decode_audio_file` 修正过采样率（`decoder.rs:527-539`），该值才正确；这依赖 decoder 的修正逻辑，无测试覆盖该组合。
- 与 import.rs 的**大段重复**：VAD 段统计日志（`import.rs:460-484` vs `retranscription.rs:269-293`）、25 s 切分（`import.rs:523-543` vs `retranscription.rs:314-334`）、逐段转写循环（`import.rs:552-611` vs `retranscription.rs:343-401`）、`get_or_init_whisper/parakeet`（`import.rs:752-841` vs `retranscription.rs:523-680`）、metadata 写入（`import.rs:880-911` vs `retranscription.rs:725-768`）几乎逐行同构，只有 `common.rs` 里的四个工具函数被真正共享（`common.rs:20-212`）。

### 5.3 `decoder.rs`（869 行）与 ffmpeg 依赖

- 库选择：`symphonia 0.5.4`（features：aac/isomp4/mp3/flac/ogg/vorbis/pcm/wav，`Cargo.toml:138`），`FFMPEG_ONLY_EXTENSIONS = ["mkv","webm","wma"]`（`decoder.rs:23`）走 ffmpeg 预转 WAV（`decoder.rs:276-389`，`pcm_s16le`，`:327`），临时文件用 `tempfile::TempPath` 自动回收（`:293-299`，`:406-421` 有说明）。
- **内存无上界**：`decode_audio_file` 把整文件解成 `Vec<f32> all_samples`（`decoder.rs:479,546`），`to_whisper_format` 又 `self.samples.clone()`（`decoder.rs:63`）。2 小时 48 kHz 立体声约 2.7 GB，克隆后峰值约 4.1 GB；retranscription 还会把它 move 进两个 `spawn_blocking`（`retranscription.rs:222,242`）。`SampleBuffer` 只按**第一个**解码包的 `capacity` 分配（`decoder.rs:515-541`），后续包容量变化依赖 symphonia 内部行为。
- 大文件重采样：阈值 `LARGE_FILE_THRESHOLD = 14_400_000` samples（`decoder.rs:78`，即 48 kHz 5 分钟），超过走 `chunked_resample_with_progress`——60 s/块、100 ms 交叉淡化、`rayon` 并行（`:126-228`）。合并逻辑（`:174-192`）用「前一块输出尾部」与「当前块重采样结果头部」做线性淡化，其正确性依赖 `resample()` 不引入群延迟，而 `resample()` 每次新建 `SincFixedIn` 且 `chunk_size = input.len()`（`audio_processing.rs:580-589`），**未做延迟补偿**；`decoder.rs:730-761` 的测试只断言最大样本差 < 0.15，属较宽松的容忍。
- `resample_audio` 在失败时**静默返回未重采样的原始数据**（`audio_processing.rs:598-606`：`Err(e) => { debug!(...); input.to_vec() }`），调用方 `decoder.rs:95` 与 `worker.rs:407` 都无法感知，会得到采样率不匹配的音频而无任何错误。

### 5.4 外部进程处理（ffmpeg）

`ffmpeg.rs`：

- 路径查找优先级：exe 同目录 → `which("ffmpeg")` → macOS `~/.local/bin` → CWD → exe 的 `../Resources`（macOS）/`lib`（Linux）→ **联网下载安装**（`ffmpeg.rs:24-162`，安装见 `:164-190`，`download_ffmpeg_package`/`unpack_ffmpeg`）。
- 结果缓存在 `static FFMPEG_PATH: Lazy<Option<PathBuf>>`（`ffmpeg.rs:18`），即首次调用触发查找/下载，之后进程内固定。
- **副作用**：macOS 安装路径逻辑会**向用户的 `~/.bashrc`、`~/.bash_profile`、`~/.zshrc` 追加 `export PATH="$HOME/.local/bin:$PATH"`**（`ffmpeg.rs:210-221`）。对一个自称 privacy-first 的桌面应用，未经同意改写用户 shell 配置是需要修正的行为。
- `sidecar_dir().map_err(|e| e.to_string()).unwrap()`（`ffmpeg.rs:128`）—— 可失败路径上的 `unwrap`。
- 首次调用可能发生在**录音路径内**（`encode.rs:30` 的 `find_ffmpeg_path()` 由 checkpoint 编码触发，`incremental_saver.rs:172-173` 由 finalize 触发），若 ffmpeg 缺失，一次网络下载会阻塞在积累任务里。

`encode.rs`（阻塞调用）：`command.spawn().expect("Failed to spawn FFmpeg process")`（`:74`）、`ffmpeg.stdin.take().expect(...)`（`:76`）、`ffmpeg.wait_with_output().unwrap()`（`:83`）、`output_path.to_str().unwrap()`（`:57`）。该函数是**同步阻塞**的，且在 `recording_saver.rs:197-198` 于持有 `AsyncMutex` 的情况下被调用 —— 见 §6.2。

`incremental_saver.rs`：`merge_checkpoints` 标为 `async fn` 却用 `std::process::Command::output()`（`:198`），`recover_audio_from_checkpoints`（`#[tauri::command] async fn`）同样（`:332`），`list_file.to_str().unwrap()`／`output.to_str().unwrap()`（`:184,187`）、`concat_file_path.to_str().unwrap()`（`:318`）。Windows 上用 `creation_flags(CREATE_NO_WINDOW)` 抑制控制台弹窗（`:191-196`、`:325-330`），考虑周到。

---

## 6. 代码质量评估

### 6.1 规模与复杂度热点

| 文件 | 行数 | 说明 |
|---|---|---|
| `recording_commands.rs` | 1697 | `stop_recording` 单函数约 420 行；含 UI 进度、DB、analytics、模型卸载、热插拔恢复 |
| `import.rs` | 1323 | 与 retranscription 大段重复 |
| `pipeline.rs` | 1116 | 混合了缓冲、混音、DSP、VAD 分发、流构建 |
| `retranscription.rs` | 1054 | 同上 |
| `core-old.rs` | 865（死） | —— |
| `decoder.rs` | 869 | —— |
| `vad.rs` | 779 | 含自研重采样与大量测试 |
| `recording_commands.rs.backup` | 84 KB（死） | —— |

`audio/` 共 57 个 `.rs`（含 4 个死文件、1 个占位符），约 18 000 行；`audio_v2/` 另 1393 行从不编译。

### 6.2 异步上下文中的阻塞调用

| 位置 | 问题 |
|---|---|
| `recording_saver.rs:196-199` 持 `AsyncMutex` 调 `add_chunk` → `incremental_saver.rs:69-71` → `encode_single_audio`（`encode.rs:74-83` 阻塞 spawn+wait） | 每 30 s 阻塞 tokio 工作线程（数百 ms 级），期间 `stop_and_save` 的 `saver_arc.lock().await`（`recording_saver.rs:378`）也无法推进 |
| `incremental_saver.rs:198`、`:332` | `async fn` 内 `std::process::Command::output()` |
| `import.rs:941,960` → `import.rs:179` | async command 内全文件阻塞解码 |
| `stream.rs:342` | `AudioStream::stop`（被 async 停止路径调用）内 `std::thread::sleep(50ms)` |
| `recording_saver.rs:273,310,323`、`:396,425` | `stop_and_save`（async）内同步 `std::fs::write/rename` |
| `recording_saver.rs:112-116` | 事件监听器里每个 segment 触发一次全量文件重写 |
| `recording_commands.rs:1051` 之前 | 停止路径上 3 次 DB + 1 次无超时的 analytics 上报 |
| `discovery.rs:89` | `std::thread::sleep(500ms)`（同步 fn，由文件对话框/权限命令路径间接调用） |
| `permissions.rs:106` | `std::thread::sleep(500ms)`；但 `trigger_system_audio_permission_command` 已用 `spawn_blocking` 包住（`permissions.rs:139-141`），是正确做法 |

### 6.3 `unwrap` / `expect` / panic 于可失败路径（节选，均为实际可达或明确脆弱）

- `encode.rs:74` `expect("Failed to spawn FFmpeg process")`、`:76` `expect`、`:83` `unwrap`、`:57` `unwrap`。若 spawn 失败（缺 ffmpeg/权限），**累积任务的 `tokio::spawn` 会 panic 而无人观察 JoinHandle**（`recording_saver.rs:178`），后续录音将静默不再产生任何 checkpoint。
- `core_audio.rs:167` `ctx.unwrap()` 与 `:319` `waker_state.lock().unwrap()`：位于 `extern "C" fn audio_proc` 内 → 跨 FFI panic（abort）。
- `core_audio.rs:116` `tap.uid().unwrap()`（初始化路径）。
- `ffmpeg.rs:128`、`incremental_saver.rs:184,187,318`、`audio_processing.rs:649`（`.expect("Failed to create valid path")`）。
- `buffer_pool.rs:91,96,101` 三处 `expect("Buffer should always be available")`。
- `device_monitor.rs:229,238` 紧接 `push` 的 `last().unwrap()`。
- `import.rs:584,591`、`retranscription.rs:373,380` 的 `as_ref().unwrap()`（§5.1）。
- `ffmpeg_mixer.rs:283` `assert_eq!(mic.len(), system.len(), ...)` —— 位于死代码中，但是潜在的 panic 型 API 契约。
- `batch_processor.rs:155-156` `metrics.first().unwrap()/last().unwrap()`（已由 `is_empty` 前置，安全）。
- `audio_processing.rs:594` `waves_out.into_iter().next().unwrap()`（`process()` 返回 Ok 即非空，安全）。

反过来，批量路径用 `unwrap_or_else(|e| e.into_inner())` 处理 poisoned mutex（`common.rs:31,40`、`retranscription.rs:530,632`、`import.rs:759,805`），而 `engine.rs:194,232,316`、`recording_state.rs`（如 `:149,163`）与 `recording_commands.rs:416,702,854,878,1125,1159,1185,1197,1226,1238,1251,1340,1391` 用裸 `.unwrap()` —— 同一子系统内策略不统一（一旦某处 panic 导致 mutex 中毒，其余裸 `unwrap` 会把一次局部故障放大为连锁 panic）。

### 6.4 竞态与锁

1. `pipeline.rs:64-71` `static mut` 数据竞争（§2.3）—— 唯一一处明确的 UB。
2. `level_monitor.rs:48-55`：`start_monitoring` 先把 `is_monitoring` 置 false（`:48-51`）再置 true（`:55`），两步之间**没有任何 `.await`**，而旧发射任务只在每轮 100 ms 循环顶部检查该标志（`:88`）。因此连续两次 start 会让**两个发射任务同时存活**，各自向 `audio-levels` 每 100 ms 推一份数据。旧任务不会退出（`level_monitor.rs` 无 abort/JoinHandle 保留）。不过该模块整体未被使用（§6.5），且被真正使用的 `simple_level_monitor` 有 100 ms `sleep`（`simple_level_monitor.rs:36`）规避了同样的问题。
3. `recording_saver.rs:178` spawn 的任务在 `:214` 置 `is_saving = true` **之后**才可能观察到数据：任务先停在 `receiver.recv().await`（`:181`），而 chunk 只可能来自尚未启动的流（`recording_manager.rs:286-290`），所以当前顺序是安全的——但这依赖"启动流之前恰好有 50 ms sleep"。若移除该 sleep 或改为先启动流，任务会立刻因 `is_saving == false` 而 `break`（`:189-191`），**整场录音不再产生任何音频文件且无任何报错**。
4. `device_monitor.rs:316-333` 的 mailbox 与 `monitored_devices` 更新在轮询周期内完成，`notify_mic_swapped`（`:204-208`）用 `std::sync::Mutex` 且不跨 await 持有，无问题。
5. `pipeline.rs:432-465` 嵌套获取 `resampler_input_buffer` 与 `resampler` 两把锁；`:500` 单独获取前者。顺序一致，暂无死锁，但缺文档约定。
6. `recording_commands.rs:58-61` 明确记录"`session_live` 会锁 `RECORDING_MANAGER`，不得在持锁时调用"，是少见的显式锁纪律；但 `do_mic_swap` 的三段锁（`:1339-1349,1390-1404`）与 `session_live` 的调用点分散在多个函数中，纪律靠人遵守而非类型保证。

### 6.5 死代码 / 未接线（除 §1.3、§1.4 外）

| 项 | 证据 |
|---|---|
| `FFmpegAudioMixer`、`SourceBuffer`、`AudioMixer`（含唯一的 RMS ducking 实现） | 仅 `ffmpeg_mixer.rs` 自身与 `mod.rs:112` re-export、其自身测试（`ffmpeg_mixer.rs:479`）引用 |
| `RNNOISE_APPLY_ENABLED = false` | `ffmpeg_mixer.rs:20`；`pipeline.rs:285` 与 `:536` 两个分支因此恒不进入，`NoiseSuppressionProcessor`（`audio_processing.rs:237-341`）整段是死代码；`pipeline.rs:543-565` 的 RNNoise 健康监控全部不可达 |
| `normalize_v2`、`spectral_subtraction`、`average_noise_spectrum` | 仅定义（`audio_processing.rs:60,405,466`），无调用者 |
| `AudioLevelMonitor`（真实电平） | 定义 `level_monitor.rs:29`、re-export `mod.rs:95`；无调用者。被接线的是 `simple_level_monitor`（`lib.rs:318`） |
| **假的电平数据被送到前端** | `simple_level_monitor.rs:50` `let fake_level = (counter.sin().abs() * 0.8) as f32;` 注释写"Simulate varying levels"（`:41,50`），随后 `emit("audio-levels", ...)`（`:70`）；`lib.rs:309-318` 把它接给 `start_audio_level_monitoring`。即 UI 上的电平表是正弦波模拟值，且 `device_type` 硬编码为 `"input"`（`:55`） |
| `system_audio_stream.rs`（`SystemAudioStreamManager`/`EnhancedAudioStreamManager`） | 不在 `mod.rs`，无外部引用 |
| `diagnostics.rs` 全部 5 个函数 | `mod.rs:106-109` re-export，无调用者 |
| `hardware_detector.rs`（`HardwareProfile`/`AdaptiveWhisperConfig`） | `mod.rs:98` re-export，除自身测试（`:281-321`）外无调用者 |
| `post_processor.rs`（`PostProcessor`） | `mod.rs:97` re-export，无调用者 |
| `async_logger.rs` 与 `async_debug!/async_info!/async_warn!` | 宏定义（`:104-131`）无使用点；`init_async_logger`/`get_async_logger` 无外部调用 |
| `AudioBufferPool` / `PooledBuffer` | 只在 `RecordingState` 里构造与清理（`recording_state.rs:108,134,364,384`），从未 `get_buffer` |
| `AudioMetricsBatcher` 的结果 | `pipeline.rs:783` 构造、`:820-831` 每 chunk 喂入；`get_summaries/clear_summaries`（`batch_processor.rs:185-192`）无调用者 → `results: Arc<RwLock<Vec<_>>>`（`batch_processor.rs:35`）**在整个录音期间无界增长**（每次混合窗口推入一条 summary），且每个 chunk 额外做一次全样本 `abs().sum()`（`pipeline.rs:821`）纯为无人读取的统计 |
| `extract_speech_16k` | `mod.rs:114` 导出，唯一调用在死文件 `lib_old_complex.rs:513,547` |
| `worker.rs:585` `format_recording_time` | `#[allow(dead_code)]` 标注 |
| `RecordingState::get_buffer_pool`、`hardware_detector`、`system_detector` 的 macOS 实现 | 均无调用者；`system_detector` 的 macOS 部分（`system_detector.rs:74-356`）反而比其 stub（`:383-399`）长得多，却只能经 `system_audio_commands.rs:39-70` 到达，而该命令仅"开始监控系统音频占用"这一外围功能 |

### 6.6 构建与依赖（直接影响音频行为）

- **`[patch.crates-io]` 位置错误，补丁未生效**：`frontend/src-tauri/Cargo.toml:212-214` 声明
  ```
  212: [patch.crates-io]
  213: cpal = { git = "https://github.com/RustAudio/cpal", rev = "51c3b43" }
  214: esaxx-rs = { git = "https://github.com/thewh1teagle/esaxx-rs.git", branch = "feat/dynamic-msvc-link" }
  ```
  但 Cargo 只读取**工作区根** manifest 的 `[patch]`；根是 `D:\Meetly\Cargo.toml`（`:1-6` 声明 `members = ["frontend/src-tauri", "llama-helper"]`），全文 18 行、无 `[patch]` 段。`Cargo.lock:1104-1107` 证实生效版本是 crates.io 的 `cpal 0.15.3`（带 checksum `873dab07...`），`:1944-1947` 同样是 crates.io 的 `esaxx-rs 0.1.10`。也就是说：**为 Windows WASAPI loopback / macOS ScreenCaptureKit host 而引入的 cpal 分支从未被编译进去**，这解释了 §3.4 的 loopback 缺失与 §3.2 的幽灵后端；同时 `core-old.rs:264` 使用的 `cpal::HostId::ScreenCaptureKit` 在 0.15.3 中不存在，佐证该文件与补丁同代。
- 其他 Git 依赖：`cidre` rev 固定（`Cargo.toml:173`）；`silero_rs` rev 固定（`:104`）；`ffmpeg-sidecar` 跟 **`branch = "main"`**（`:146`），锁定在 `Cargo.lock:2016-2018` 的 `b0f48a51`，但 `Cargo.lock` 位于仓库根而 `frontend/src-tauri` 下无独立 lock —— 任何一次 `cargo update` 都可能改变 ffmpeg 安装行为，而该行为包含联网下载与改写 shell 配置（§5.4）。
- `ort` 重复声明：全局 `ort = "2.0.0-rc.10"`（`:107`）与 Windows 专属 `features = ["load-dynamic"]`（`:189`）。VAD 依赖的 `silero_rs` 也要 ONNX Runtime，`vad.rs:37` 与 `recording_commands.rs:325,515` 都调用 `crate::ensure_onnx_runtime_available()`，说明运行时可用性在多个入口被重复检查。
- `rubato` 在依赖图中出现两个版本（`Cargo.lock:5261-5265` = 0.15.0、`:5273-5277` = 0.16.2），音频代码使用 0.15 API（`audio_processing.rs:580-586`、`pipeline.rs:357-363`），存在版本漂移风险。

### 6.7 重复代码

1. `import.rs` 与 `retranscription.rs` 的五段同构流程（§5.2）。
2. 三套系统音频实现：`capture/system.rs`（cpal/macOS CoreAudio 桥）、`capture/core_audio.rs`（真正在用）、`audio/system_audio_stream.rs`（死）、`audio/system_detector.rs`（占用检测，另一目的）。
3. 两套"内置设备"匹配：`devices/microphone.rs:25-67` 与 `devices/speakers.rs:61-115` 结构相同、模式列表不同（后者多 `"speakers"` 等）。
4. 两套 BT 判定：`device_detection.rs:116-188`（3 级 17 个模式）与 `device_monitor.rs:50-52`（3 个关键词）与 `playback_monitor.rs:52-60,97-102,136-141`（三平台各一套关键词），再加 `recording_commands.rs:927-940` 的 `classify_device_type`（第四套）。
5. 两套电平监控（`level_monitor.rs` 355 行 vs `simple_level_monitor.rs` 92 行），`AudioLevelData`/`AudioLevelUpdate` 结构体逐字重复（`level_monitor.rs:14-27` vs `simple_level_monitor.rs:7-20`）。
6. 两套 `TranscriptSegment`：`recording_saver.rs:15-25`（有 `sequence_id`/`display_time`/`confidence`）与 `crate::api::TranscriptSegment`（`import.rs:3`、`common.rs:1` 使用，字段为 `Option<f64>`）。
7. 两套 `TruePeakLimiter`/归一化：`audio_processing.rs:100-137` 与 `audio_v2/limiter.rs`、`audio_v2/normalizer.rs`。
8. 三条几乎相同的停止路径（§4.3）。

### 6.8 文档与注释可信度

本子系统注释常包含有价值的实测背景（例如 `pipeline.rs:744-757` 的分段统计、`core_audio.rs:119-123` 的回声根因、`device_monitor.rs:113-119` 的 use-after-free 权衡、`recording_manager.rs:31-51` 的 BT 唤醒现象），但也有相当比例与代码不符：`pipeline.rs:40,44-47,91,178-180,187-189`、`vad.rs:117-118,129-130,345-347`、`audio_processing.rs:272,308-310`、`core_audio.rs:261`、`audio_processing.rs:99`、`recording_saver.rs:22`（`display_time` 声称 `"[02:15]"` 格式，实际在 `recording_commands.rs:452` 被赋成 `update.timestamp` 即墙上时钟 `"14:30:05"`）、`system_audio_stream.rs:150-151` 与 `devices/speakers.rs:52,56`（都提 ScreenCaptureKit，代码里没有）。CLAUDE.md 与 README.md 也复述了其中若干不实描述（§2.6），并引用了一个不存在的 `AUDIO_MODULARIZATION_PLAN.md`。

---

## 7. 按严重度排序的技术风险

1. **录音停止失败会导致应用级卡死状态（只能重启）**。`stop_recording` 在 `Err` 分支提前返回时不清 `IS_RECORDING`，manager 已被取走并 Drop（`recording_commands.rs:731-734` vs `:1051`、`recording_commands.rs:701-704`、`recording_manager.rs:613-618`）。此后 `start_recording` 永久被 "Recording already in progress" 拒绝（`recording_commands.rs:319-323`）。当前因 `stop_streams_and_force_flush` 吞掉子错误而难触发（`recording_manager.rs:349-357`），属"沉默的定时炸弹"，修复成本极低（`defer` 式清理）。
2. **Windows 上系统音频采集在结构上不可用，且被 CLAUDE.md/README 描述为已实现**。无任何 loopback API（`devices/platform/windows.rs:15-23`、`:195`），Output 设备被交给 `build_input_stream`（`stream.rs:247`），而本该提供 loopback 的 cpal 补丁因 `[patch]` 放错文件而未生效（`frontend/src-tauri/Cargo.toml:212-214` + 根 `D:\Meetly\Cargo.toml:1-18` + `Cargo.lock:1104-1107` 显示 crates.io 0.15.3）。同理 macOS 的 "ScreenCaptureKit" 后端是空壳（`stream.rs:84-102`、`backend_config.rs:10-19`）。
3. **系统音频与麦克风的累积失步**：`extract_window` 对落后的一路 `drain(..)+resize(0.0)`（`pipeline.rs:137-151`），把该路已有样本压缩进更早的时间窗。系统音频在录音/转写时间轴上被系统性前移；叠加 4.8 s 上限的"丢最旧"策略（`pipeline.rs:92-97`，注释谎称 400 ms/200 ms），长会议下 system 轨会漂移。
4. **实时线程上的锁与 panic**：CoreAudio IO proc 内 `ctx.unwrap()`（`core_audio.rs:167`）与 `Mutex::lock().unwrap()`（`core_audio.rs:319`），panic 跨 `extern "C"` 即 abort；同时 `static mut SAMPLE_COUNTER` 在 mic/system 两个回调线程上无同步自增（`pipeline.rs:64-71`）是明确的 UB。三者都在音频回调路径上。
5. **热路径阻塞会级联成关闭挂起**：checkpoint 编码是持有 `AsyncMutex` 的同步 ffmpeg spawn/wait（`recording_saver.rs:196-199` → `encode.rs:74-83`），`stop_and_save` 需等同一把锁（`recording_saver.rs:378`）；`Stop` 尾部的 3 次 DB + 无超时 analytics 上报（`recording_commands.rs:826-846,943-1002`）也在 `IS_RECORDING` 仍为 true 的窗口内。
6. **内存无上界**：`decode_audio_file` 全量载入 + `to_whisper_format` 全量克隆（`decoder.rs:479,546,63`），配合 20 GB 导入上限（`import.rs:62`）与"仅取时长"的阻塞全解码（`import.rs:173-182`，在 async 命令内直接调用，`import.rs:941,960`）——一次校验即可耗尽内存并卡死运行时。
7. **断连与静音降级对用户不可见**：CoreAudio 背压终止后只打一条 info 日志（`core_audio.rs:356-362`、`stream.rs:222`），录音在无系统音频下继续；`AudioDeviceDisconnected` 被归为可恢复（`recording_state.rs:56`），累计 10 次即静默停止录音（`recording_state.rs:275-278`）；transcription 引擎初始化失败后所有音频段被静默丢弃（`worker.rs:59-71` + `pipeline.rs:877-881`），而录音仍在进行。
8. **前端显示伪造电平数据**：`lib.rs:309-318` 接的是 `simple_level_monitor`，其数据源是 `sin()` 模拟值（`simple_level_monitor.rs:50`），`device_type` 硬编码 `"input"`（`:55`），而真实的 `AudioLevelMonitor` 从未被接线（`level_monitor.rs:29`，`mod.rs:95` 仅 re-export）。用户会据此判断麦克风是否工作，等于给出错误反馈。
9. **音频质量与"专业化"描述不符**：实时路径无 ducking（`pipeline.rs:184-185`）、system 增益注释与代码不符（`pipeline.rs:178-181`）、`sum/sum_abs` 逐窗口归一化引入增益抽吸（`pipeline.rs:190-196`）、RNNoise 编译期关闭（`ffmpeg_mixer.rs:20`）、真正的 RMS ducking 与 `audio_v2` 的限幅/归一化都在死代码里。VAD 侧的 48k→16k 是 5 点均值 + 线性插值且无跨窗口状态（`vad.rs:119-165`），与 decoder 弃用线性插值的理由（`decoder.rs:74-77`）自相矛盾。
10. **无人读取的指标导致无界增长与白费算力**：`AudioMetricsBatcher` 的结果从未被读取（`batch_processor.rs:185-192` 无调用者），`results` 随录音时长线性增长（`batch_processor.rs:35,56,68`），每个 chunk 还额外做一次全样本 `abs().sum()`（`pipeline.rs:821`）。
11. **批量路径的一致性缺口**：`retranscription.rs:791-820` 的 TOCTOU 让前端在任务已失败时仍看到"已启动"；`import.rs:584,591` 与 `retranscription.rs:373,380` 的 `as_ref().unwrap()` 靠间接不变式；`resolve_*` + `import`/`retranscription` 的取消语义各自为政（import 会 `remove_dir_all`，retranscription 不会删除半写状态，`:472-490` 只 warn）。
12. **可维护性/供应链**：约 18 000 行中约 1 500 行从不编译（`core-old.rs`、`recording_saver_old.rs`、`recording_commands.rs.backup`、`stt.rs`、`system_audio_stream.rs`、`lib_old_complex.rs`），再加 1 393 行的 `audio_v2`（`CLEANUP_PLAN.md:10-15` 已列出但未执行）；`ffmpeg-sidecar` 跟随 `branch = "main"`（`Cargo.toml:146`）且其代码路径会在 macOS 上改写用户 shell 配置（`ffmpeg.rs:210-221`）；`device_monitor` 的 5 s/2 s 自适应轮询是死代码（`device_monitor.rs:387-396`），使整个录音期间维持 2 s 全量设备枚举（`device_monitor.rs:280,296`）。

---

## 摘要要点

- 模块接线清晰（`audio/mod.rs:2-52` 显式列出 34 个模块），但 `audio_v2/**`（1393 行）从未在 `lib.rs:38-56` 注册、且因 API 过时（`audio_v2/compatibility.rs:76` vs `recording_saver.rs:138`）已无法编译。
- 另有 5 个从不编译的遗留文件：`core-old.rs`、`recording_saver_old.rs`、`recording_commands.rs.backup`、`stt.rs`（引用 7 个不存在的 crate 模块，`stt.rs:1-11`）、`system_audio_stream.rs`。
- 实时混音器**没有任何 ducking**（`pipeline.rs:184-185` 自认 "Sum without ducking"），`sys * 1.0` 与"预缩放 70%"注释矛盾（`pipeline.rs:178-181`），真正的 RMS ducking 在死代码 `ffmpeg_mixer.rs:287-308` 中。
- 环形缓冲注释全面失真：代码为 600 ms 窗口 / 4800 ms 上限（`pipeline.rs:41,48`），注释写 50 ms / 400 ms / 200 ms（`pipeline.rs:40,44-47,91`），CLAUDE.md:199 也写 50 ms。
- `extract_window` 对落后声道 `drain+零填充`（`pipeline.rs:137-151`），导致系统音频在时间轴上被前移，与麦克风产生累积失步。
- 实时回调路径存在明确 UB 与 abort 风险：`static mut SAMPLE_COUNTER` 双线程自增（`pipeline.rs:64-71`）、CoreAudio IO proc 内 `ctx.unwrap()` 与 `Mutex::lock().unwrap()`（`core_audio.rs:167,319`）。
- Windows 系统音频采集结构上不可用：无 loopback API（`devices/platform/windows.rs:15-23,195`）却对输出设备调 `build_input_stream`（`stream.rs:247`）；本应提供支持的 cpal 补丁因 `[patch.crates-io]` 写在成员 manifest 而非工作区根（`frontend/src-tauri/Cargo.toml:212-214` vs `D:\Meetly\Cargo.toml:1-18`）而未生效（`Cargo.lock:1104-1107` 为 crates.io 0.15.3）。
- 后端概念亦有幽灵项：`ScreenCaptureKit` 分支无任何实现，只改日志字符串后落到 CPAL（`stream.rs:84-102`），macOS 实际用的是 CoreAudio process tap（`core_audio.rs:91`），与 CLAUDE.md 的 ScreenCaptureKit/BlackHole 描述（CLAUDE.md:117,329）不符。
- `stop_recording` 的 `Err` 早退路径不清 `IS_RECORDING`（`recording_commands.rs:731-734` vs `:1051`）且 manager 已被取走，会使应用此后永久拒绝新录音（`recording_commands.rs:319-323`），只能重启。
- 异步上下文阻塞集中且互相级联：checkpoint 编码持 `AsyncMutex` 同步等待 ffmpeg（`recording_saver.rs:196-199` → `encode.rs:74-83`）、`merge_checkpoints` 用 `async fn` 包 `Command::output()`（`incremental_saver.rs:198`）、导入校验在 async 命令里全文件阻塞解码仅取时长（`import.rs:941,960,173-182`）、停止尾部有 3 次 DB 与无超时的 analytics 上报（`recording_commands.rs:826-1002`）。
- 内存无上界：`decode_audio_file` 全量载入并再克隆一份（`decoder.rs:479,546,63`），配合 20 GB 导入上限（`import.rs:62`）与不可取消的复制（`import.rs:360-372`）。
- 多处"性能/UX 特性"实为死代码或伪造数据：音频电平由 `sin()` 模拟值驱动（`simple_level_monitor.rs:50`，经 `lib.rs:318` 接线）、真实 `AudioLevelMonitor` 未接线（`level_monitor.rs:29`）、`RNNOISE_APPLY_ENABLED = false`（`ffmpeg_mixer.rs:20`）、`AudioMetricsBatcher` 结果无人读取且无界增长（`batch_processor.rs:35,185-192`）、`device_monitor` 的 2 s/5 s 自适应轮询未生效（`device_monitor.rs:280,387-396`）、转写"3 workers"事件载荷与 `NUM_WORKERS = 1` 矛盾（`recording_commands.rs:474` vs `worker.rs:74`）。
