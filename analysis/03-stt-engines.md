# Meetily 语音识别（STT）引擎技术分析

分析对象：`D:\Meetly`（git main，release v0.4.1）
分析范围：`frontend/src-tauri/src/whisper_engine/**`、`frontend/src-tauri/src/parakeet_engine/**`、`frontend/src-tauri/Cargo.toml`、`Cargo.toml`、`docs/GPU_ACCELERATION.md`，以及为还原引擎调用链所必需的 `audio/transcription/**`、`audio/pipeline.rs`、`audio/hardware_detector.rs`、`lib.rs`、`config.rs`。

> 说明：本报告只做只读分析，未修改任何既有源文件。文中所有非显然结论均给出 `路径:行号`。

---

## 1. Whisper 引擎

### 1.1 引擎抽象与生命周期

`WhisperEngine` 是一个进程内的全局单例，通过 `static WHISPER_ENGINE: Mutex<Option<Arc<WhisperEngine>>>` 暴露（`frontend/src-tauri/src/whisper_engine/commands.rs:8`）。

内部状态（`whisper_engine.rs:68-82`）：

| 字段 | 作用 |
|---|---|
| `models_dir: PathBuf` | 模型目录，启动时由 Tauri `app_data_dir()/models` 注入 |
| `current_context: Arc<RwLock<Option<WhisperContext>>>` | 已加载的 whisper.cpp 上下文 |
| `current_model: Arc<RwLock<Option<String>>>` | 当前模型名 |
| `available_models: Arc<RwLock<HashMap<String, ModelInfo>>>` | 目录扫描结果的缓存 |
| `last_transcription_was_short` / `short_audio_warning_logged` / `transcription_count` | 日志降噪用的状态位 |
| `active_downloads: Arc<Mutex<HashMap<String, Arc<ActiveDownload>>>>` | 下载所有权（owner）表 |

注意：**并没有** `async-trait` 层面的引擎抽象。真正的抽象只出现在 `audio/transcription/engine.rs:15-19`：

```rust
pub enum TranscriptionEngine {
    Whisper(Arc<WhisperEngine>),      // Direct access (backward compat)
    Parakeet(Arc<ParakeetEngine>),    // Direct access (backward compat)
    Provider(Arc<dyn TranscriptionProvider>),  // Trait-based (preferred for new code)
}
```

也就是说存在**两套并行的抽象**：`audio/transcription/provider.rs:50` 定义了 `TranscriptionProvider` trait，同时 `TranscriptionEngine` 又保留了 `Whisper`/`Parakeet` 两个直连分支。实际加载路径（`audio/transcription/engine.rs:186-220`）走的是**直连枚举分支**，`Provider` 分支在引擎初始化时从未被构造——`WhisperProvider`/`ParakeetProvider` 因此是只写不用的包装层（`audio/transcription/whisper_provider.rs:10`、`parakeet_provider.rs:11`）。

### 1.2 模型目录与存储路径

三种来源，优先级依次为：

1. 显式传入：`new_with_models_dir(Some(dir))`（`whisper_engine.rs:124-126`）。生产路径来自 `set_models_directory()`，即 `app_data_dir()/models`（`commands.rs:16-19`）。
2. debug 构建回退：依次尝试 `./models`、`../models`、`backend/whisper-server-package/models`（`whisper_engine.rs:134-147`）——其中 `backend/` 已在 CLAUDE.md 中被标注为归档目录。
3. release 构建回退：`dirs::data_dir()/Meetily/models`（`whisper_engine.rs:151-156`）。这条路径与 Tauri 的 `app_data_dir()`（Windows 为 `%APPDATA%\<bundle identifier>`）**不是同一个目录**，只是恰好都以 `%APPDATA%` 为根。

模型文件平铺在 `models/` 根下，文件名为 `ggml-{name}.bin`（`commands.rs:82`、`whisper_engine.rs:1102`）。Parakeet 则使用 `models/parakeet/{model}/`（`parakeet_engine.rs:280`），两种布局互不干扰但也不统一。

### 1.3 下载

入口是硬编码 URL 表的 `download_model()`（`whisper_engine.rs:1073-1093`），12 个模型全部指向 `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/`。

下载所有权模型值得肯定：
- `reserve_active_download()` 以互斥方式占用 `model_name`，重复下载直接报错（`whisper_engine.rs:932-946`）。
- 分块流式写盘，进度按“进度 +1% 或每 2 秒”节流上报（`whisper_engine.rs:1200-1213`）。
- 取消通过 `CancellationToken` 在 `tokio::select!` 的 `biased` 分支上生效（`whisper_engine.rs:1147-1152`、`1174-1178`）。
- `finish_download()` 做幂等的所有权校验（`Arc::ptr_eq`）后才发布状态（`whisper_engine.rs:955-1071`），并区分“取消胜出”和“正常完成”两条清理路径，避免取消后重试互相踩踏。

**缺陷：下载客户端没有任何超时。**

```rust
// whisper_engine.rs:1143-1146
let client = Client::builder()
    .user_agent(concat!("Meetily/", env!("CARGO_PKG_VERSION")))
    .build()
```

与 Parakeet 的下载客户端形成鲜明对比（`parakeet_engine.rs:865-871` 设置了 `timeout(3600s)` + `connect_timeout(30s)` + 每块 30 秒读超时）。对端“接受连接后不发包”时，Whisper 下载会无限挂起，只能靠用户手工取消。

### 1.4 校验（含弱点）

两条校验，都不含任何密码学校验和：

1. 文件头魔数（`validate_model_file`，`whisper_engine.rs:846-865`）：检查前 8 字节是否以 `ggml`/`GGUF`/`ggmf`/`lmgg`/`FUGU`/`fmgg` 开头。
2. 文件尺寸下界（`finish_download`，`whisper_engine.rs:972-994`）：`catalog_size_mb * 0.9 * 1024 * 1024`。

目录扫描侧的容差更松：`discover_models()` 用整数除法得到 MB 后与 `expected_min_size_mb = (size_mb as f64 * 0.9) as u64` 比较，并且额外要求 `> 1` MB（`whisper_engine.rs:220-248`）。对 `medium`（目录标称 1463 MB，`config.rs:23`）而言，**缺失约 148 MB 的截断文件仍会被判定为 `Available`**，随后在 `WhisperContext::new_with_params` 阶段失败。由于 whisper.cpp 对 GGML 张量读取不做边界校验，截断文件并不保证是干净的报错退出。

对照项目自身已有的能力：`frontend/src-tauri/build/onnxruntime.rs:1,15,310-341` 对构建期下载的 ONNX Runtime 做了完整 SHA-256 校验（`verify_file`），`sha2` 也已在 `build-dependencies` 中（`frontend/src-tauri/Cargo.toml:62`）。**运行时模型下载完全没有复用这套机制**。

### 1.5 GPU 特性门控与回退

编译期后端判定（`whisper_engine/acceleration.rs:13-25`）：

```rust
if cfg!(feature = "cuda") { Cuda }
else if cfg!(feature = "vulkan") { Vulkan }
else if cfg!(feature = "hipblas") { HipBlas }
else if cfg!(target_os = "macos") || cfg!(feature = "metal") { Metal }
else { Cpu }
```

运行期决策（`acceleration.rs:61-80`）：`use_gpu = backend != Cpu`；`flash_attn` 仅对 Metal/CUDA 且性能档为 High/Ultra 时开启；Vulkan/HipBlas 永不启用 flash attention。该处有 4 个单元测试覆盖（`acceleration.rs:86-143`）。

上下文参数落地点（`whisper_engine.rs:320-331`）：

```rust
let context_param = WhisperContextParameters {
    use_gpu: acceleration.use_gpu,
    gpu_device: acceleration.gpu_device,
    flash_attn: acceleration.flash_attn,
    ..Default::default()
};
```

字段名与 whisper-rs 0.13.2 的 `WhisperContextParameters { use_gpu, flash_attn, gpu_device, dtw_parameters }` 一致（[docs.rs/whisper-rs/0.13.2](https://docs.rs/whisper-rs/0.13.2/whisper_rs/struct.WhisperContextParameters.html)），实现无误。

**特性与依赖的错配（重要）**：`frontend/src-tauri/Cargo.toml:38-53` 声明了 `cuda/vulkan/hipblas/metal/coreml/openblas/openmp` 七个特性，但：

- macOS 目标下 `whisper-rs` 被**无条件**带上 `metal` + `coreml`（`Cargo.toml:179`），所以 macOS 上 `--features metal` 之类是空操作，`WhisperCompiledBackend::current()` 仅凭 `cfg!(target_os = "macos")` 就返回 Metal。
- Windows/Linux 目标下 `whisper-rs` 只带 `raw-api`（`Cargo.toml:188`、`200`），`cuda`/`vulkan`/`hipblas` 通过 Cargo 特性统一生效（已验证 whisper-rs 0.13.2 确实声明了这些特性：`cuda = ["whisper-rs-sys/cuda", "_gpu"]` 等，见 [docs.rs 特性页](https://docs.rs/crate/whisper-rs/0.13.2/features)）。
- **`openmp` 与 `openblas` 在 Windows/Linux 上是死特性**：没有任何依赖声明会激活它们，因此 `whisper_engine.rs:169-170`、`181-182` 的两条 `#[cfg(feature = ...)]` 日志在 Windows/Linux 上永不编译进去。

还有一处**接口存在但未接线**的降级逻辑：`adaptive_config.use_gpu` 在 `WhisperEngine::load_model()` 中根本没有被读取（`whisper_engine.rs:318-331` 只用 `performance_tier`），因此 `PerformanceTier::Low` 时“强制 CPU 以免弱机 GPU 开销”的意图（`audio/hardware_detector.rs:242-248`）不会生效——只要二进制里编了 GPU 后端，就一定会走 GPU。同理 `max_threads` 也是死配置（见 1.7）。

`docs/GPU_ACCELERATION.md` 与实际状态有三处偏差：
- 第 11 行把 CoreML 描述为独立的加速后端；实际它只是 macOS 上与 Metal 一并开启的编译特性（`Cargo.toml:179`），代码中没有任何 CoreML 的运行时决策。
- 第 17 行描述的 `scripts/auto-detect-gpu.js` 自动探测与代码中的 `WhisperCompiledBackend::current()`（纯编译期 `cfg!`）是两套机制，文档未说明后者才是实际生效者。
- 第 13 行列出的 OpenBLAS 在 Windows/Linux 默认构建中并未启用（见上）。

### 1.6 转写：流式 vs 批处理

**不存在真正的流式转写。** 引擎只有两个批量入口：
- `transcribe_audio()` → `String`（`whisper_engine.rs:668`）
- `transcribe_audio_with_confidence()` → `(String, f32, bool)`（`whisper_engine.rs:551`）

两者是约 120 行的**整段复制**：参数配置、时间戳开关、日志状态机几乎逐行相同（对比 `whisper_engine.rs:561-604` 与 `678-726`，及 `626-665` 与 `785-838`）。差异仅在：前者额外计算“置信度”，后者带完整的短音频日志抑制。这是本模块最大的重复代码块。

Parakeet 引擎则只有一个 `transcribe_audio()`（`parakeet_engine.rs:543`），且不返回置信度——前端展示置信度时 Whisper 与 Parakeet 行为必然不同。

### 1.7 参数与时间戳

关键解码参数（`whisper_engine.rs:561-603`，`with_confidence` 版本）：

| 参数 | 值 | 说明 |
|---|---|---|
| `SamplingStrategy` | `BeamSearch { beam_size: adaptive_config.beam_size, patience: 1.0 }` | beam size 由硬件档决定（Windows 恒为 2；其余 5/3/2/1） |
| `set_no_timestamps(true)` | — | 刻意关闭时间戳 token，规避 whisper.cpp 的 chunk 丢弃启发式 |
| `set_token_timestamps(true)` | — | 但保留 token 级时间戳 |
| `set_no_speech_thold(0.55)` | — | 注释记录了从 0.75 下调的原因 |
| `set_max_len(200)`、`set_single_segment(false)` | — | |
| `temperature` | 0.1~0.4（按档） | 与注释中“0.3”不一致处见 `whisper_engine.rs:709` |

`set_no_timestamps(true)` + `set_token_timestamps(true)` 的组合是**自相矛盾**的配置：关闭段级时间戳后又请求 token 级时间戳，注释也承认“Keep for any timestamp-aware features”，但代码中没有任何地方消费 token 时间戳。段级 t0/t1 虽被读取（`whisper_engine.rs:793-794`），却赋给 `_start_time`/`_end_time` 并仅用于 >30 秒时的 trace 日志——**时间戳实际上被丢弃**。

`max_threads` 完全没接线：

```rust
// whisper_engine.rs:605-609
if let Some(_max_threads) = adaptive_config.max_threads {
    // Note: whisper.cpp may or may not expose thread control through params
    // Removed debug log to reduce I/O overhead in transcription hot path
}
```

whisper-rs 没有暴露 `n_threads` 的 `FullParams` 开关（`pcm_to_mel`/`encode`/`decode` 才收 `threads`），所以真实线程数由 whisper.cpp 默认值决定。文档与 `AdaptiveWhisperConfig.max_threads` 在 CPU 核数较少或较多时都不会改变行为。

### 1.8 置信度是启发式而非模型概率

```rust
// whisper_engine.rs:637-643
let segment_confidence = if segment_length > 0.0 {
    (segment_length / 100.0).min(0.9) + 0.1 // 0.1 to 1.0 confidence based on text length
} else { 0.1 };
```

置信度**仅由文本字符数决定**，与 token 概率无关。whisper-rs 明明提供 `full_get_token_prob()`（见 [WhisperState 文档](https://docs.rs/whisper-rs/0.13.2/whisper_rs/struct.WhisperState.html)）却未使用。该值随后被塞进 `TranscriptUpdate.confidence`（`audio/transcription/worker.rs:213`），而 Parakeet 路径下该字段退化为常量 `0.85`。前端把它当置信度展示时会得到误导性数值。

### 1.9 部分/最终转写的实际语义

```rust
// whisper_engine.rs:611-612
let duration_seconds = audio_data.len() as f64 / 16000.0;
let is_partial = duration_seconds < 15.0; // Consider chunks under 15s as partial
```

`is_partial` 只表示“这次输入短于 15 秒”，**完全不表示“这是未完成的中间结果”**。而 live 路径送入引擎的是 VAD 已经闭合的完整语音段（`audio/pipeline.rs:860-885`），典型长度远小于 15 秒，因此**几乎所有 live 转写都被标记为 partial**。

更关键的是：这个标记在后端和后端之外都没有任何消费逻辑。`TranscriptUpdate.is_partial` 被前端写入缓冲（`frontend/src/contexts/TranscriptContext.tsx:312`）并按 `sequence_id` 排序，没有“用 final 覆盖 partial”的替换路径。也就是说 Meetily 当前**没有 partial→final 的合并机制**，每条 VAD 段都是一条独立、永久追加的转写条目。

`is_partial` 的跨引擎语义也不一致：Whisper 恒按 15 秒阈值动态判定，Parakeet 恒为 `false`（`audio/transcription/parakeet_provider.rs:40`、`worker.rs:495`），Provider 分支则转发底层结果（`worker.rs:526,543`）。

### 1.10 语言处理

```rust
// whisper_engine.rs:570-576（with_confidence 版本同 687-693）
let (language_code, should_translate) = match language.as_deref() {
    Some("auto") | None => (None, false),
    Some("auto-translate") => (None, true),
    Some(lang) => (Some(lang), false),
};
```

`WhisperEngine` 自身不持有语言状态，每次调用从全局 `LANGUAGE_PREFERENCE` 读取（`audio/transcription/worker.rs:441` → `lib.rs:443-445`）。**该全局默认值是 `"auto-translate"`**（`lib.rs:124-126`）：

```rust
// Global language preference storage (default to "auto-translate" for automatic translation to English)
static LANGUAGE_PREFERENCE: std::sync::LazyLock<StdMutex<String>> =
    std::sync::LazyLock::new(|| StdMutex::new("auto-translate".to_string()));
```

即：用户未在设置里显式选择语言之前，**所有 Whisper 转写都会被翻译成英文**。这是一个隐私/正确性上都需要明确告知的默认值（前端在 `frontend/src/contexts/ConfigContext.tsx:220,485` 才会写这个全局量）。Parakeet 不支持语言参数，仅在 provider 层打警告（`parakeet_provider.rs:29-34`）。

### 1.11 模型卸载/重载

- `load_model()` 会在加载新模型前先 `unload_model()`（`whisper_engine.rs:304-313`），并按名去重避免重复加载。
- `unload_model()`（`whisper_engine.rs:380-391`）**没有任何串行化锁**（Parakeet 有 `model_lifecycle_lock`，`parakeet_engine.rs:267,443,519`），因此与 `load_model` 并发时可能出现“卸载把刚加载的模型清掉”。
- 录制结束时按 provider 卸载（`audio/recording_commands.rs:848-900`）；批处理（导入/重转写）结束后统一卸载（`audio/common.rs:20-47`，且在录制中会跳过卸载）。
- `engine.rs:287` 在“已加载模型与配置不符”时会先 `unload_model()` 再重载。

卸载/重载的代价很高：`large-v3` 目录标称 2951 MB（`config.rs:25`），每次重载都是完整的磁盘读 + 反量化。

### 1.12 parallel_processor 设计（整块未接线）

`parallel_processor.rs` 实现了一个资源感知的 worker 池：`ChunkQueue{pending, processing, completed, failed, retry_queue}`（`:93-99`）、信号量限流（`:83,126`）、资源监控任务自动暂停/恢复（`:372-413`）、chunk 级重试（`:286-291`）、`TranscriptionResult` 事件流（`:39-48`）。

设计上的两处真实问题：

1. **每个 worker 独立加载一份模型。** `create_worker()` 为每个 worker 新建 `WhisperEngine`：

```rust
// parallel_processor.rs:223-228
let mut engine_guard = engine_ref.write().await;
let engine = WhisperEngine::new().map_err(...)?;
engine.load_model(&model_name).await...;
```

   `WhisperEngine::new()` 走的是**无 models_dir 的路径**（`whisper_engine.rs:111-113` → `new_with_models_dir(None)`），在 release 下落到 `dirs::data_dir()/Meetily/models`（`whisper_engine.rs:151-156`）。4 个 worker × `large-v3` ≈ **12 GB 常驻内存**，而 `memory_budget_mb` 默认 512 MB（`parallel_processor.rs:65`）在代码里从未被用于任何内存核算。

2. **未被任何代码调用。** 全仓库检索 `initialize_parallel_processor`、`start_parallel_processing`、`prepare_audio_chunks` 在 `frontend/src` 中零命中；Rust 侧除 `lib.rs:471` 注册 state、`lib.rs:672-682` 注册命令与 `parallel_commands.rs` 自身外无消费者。真正跑起来的转写池明确是串行的：

```rust
// audio/transcription/worker.rs:74
const NUM_WORKERS: usize = 1; // Serial processing ensures transcripts emit in chronological order
```

即：**并行的旧设计（无限流、每 worker 一份模型）仍在 IPC 上暴露，而现行实现是 1 worker 串行。** 这是典型的“加了没接 / 已接已废”双份代码。

---

## 2. Parakeet 引擎

### 2.1 它是什么

**ONNX Runtime 上的 NVIDIA NeMo Parakeet TDT 0.6B**，不是 sherpa、也不是 NeMo 运行时本身。证据：
- `frontend/src-tauri/Cargo.toml:107`：`ort = { version = "2.0.0-rc.10" }`（ONNX Runtime 的 Rust 绑定）；Windows 下另加 `load-dynamic`（`Cargo.toml:189`，动态加载 DLL）。
- `model.rs:65-67` 依次加载 `encoder-model.int8.onnx`、`decoder_joint-model.int8.onnx`、`nemo128.onnx` 三个会话。
- `model.rs:15-18` 定义 `SUBSAMPLING_FACTOR = 8`、`WINDOW_SIZE = 0.01`、`TDT_DURATIONS = [0,1,2,3,4]`，`decode_sequence()` 按 TDT（Token-and-Duration Transducer）规则拆分 `vocab_logits` / `duration_logits`（`model.rs:377-425`）。

模型定义（`parakeet_engine.rs:120-139`）：

| 模型 | 大小 | 来源 |
|---|---|---|
| `parakeet-tdt-0.6b-v3-int8` | 670 MB | `https://meetily.towardsgeneralintelligence.com/models/parakeet-tdt-0.6b-v3-onnx` |
| `parakeet-tdt-0.6b-v2-int8` | 661 MB | `https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/0bbb45a3...` |

v3 走**厂商自建域名**，v2 走 Hugging Face。这是两个引擎中唯一的自托管分发点。

### 2.2 模型生命周期

目录为 `models/parakeet/{model_name}/`（`parakeet_engine.rs:280`），每个模型是一个包含 4 个文件的目录（`parakeet_engine.rs:106-118`）。生命周期显著比 Whisper 严谨：

- **加载有串行化锁**：`model_lifecycle_lock: Mutex<()>`（`:267`），`load_model`（`:443`）与 `unload_model`（`:519`）都持锁；原生加载放到 `spawn_blocking`（`:467`），不阻塞 async 运行时。
- **下载支持断点续传**：逐 artifact 比对本地字节数决定 `Range: bytes={n}-`（`:913-914`），并校验 206/`Content-Range`、416、200 三种响应（`:919-972`、`validate_partial_response`/`validate_unsatisfied_response`）。
- **写入用 8 MB `BufWriter`**（`:993`），每块有 30 秒读超时且取消时先 flush 保留已下载部分（`:1000-1017`）。
- **发现/下载并发一致性**：用 `ActiveDownloadState.revision` 做乐观重试（`:337-411`）。

### 2.3 推理路径

调用链：`ParakeetEngine::transcribe_audio`（`parakeet_engine.rs:543`）→ `ParakeetModel::transcribe_samples`（`model.rs:477`）→ `recognize_batch`（`model.rs:327`）→ `preprocess` → `encode` → `decode_sequence` → `decode_tokens`。

每一步都是**同步阻塞调用**，直接跑在 async 上下文中：

```rust
// parakeet_engine.rs:544-559
let mut model_guard = self.current_model.write().await;   // 写锁覆盖整个推理
let model = model_guard.as_mut()...;
let result = model.transcribe_samples(audio_data)...;      // 同步 ONNX 推理，无 await
```

两个后果：
1. ONNX 会话只被写锁保护，**同一引擎的转写完全无法并发**（Whisper 用的是读锁 + 每次新建 state，可以并发）。
2. 推理期间 async 运行时的 worker 线程被独占，且持锁时间取决于音频长度——`parakeet_load_model` / `unload_model` 会被长时间阻塞。

### 2.4 与 Whisper 路径的差异

| 维度 | Whisper | Parakeet |
|---|---|---|
| 运行时 | whisper.cpp（C/C++，`whisper-rs` 0.13.2） | ONNX Runtime 2.0.0-rc.10（`ort`） |
| 执行提供者 | 编译期 metal/cuda/vulkan/hipblas | **硬编码 CPU only**（`model.rs:97`） |
| 模型存储 | `models/ggml-*.bin` 平铺 | `models/parakeet/{model}/` 目录 |
| 下载来源 | Hugging Face ggerganov | 自建域名（v3）+ Hugging Face（v2） |
| 完整性校验 | 魔数 + 90% 尺寸 | **精确字节数**（`parakeet_engine.rs:418`） |
| 断点续传 | 不支持（总是从头覆盖） | 支持（HTTP Range） |
| 下载超时 | **无** | 3600s / 30s connect / 30s 读 |
| 推理并发 | 读锁，可并发 | 写锁，串行 |
| 语言 | 支持 auto/auto-translate/指定语言 | 不支持（`parakeet_provider.rs:29`） |
| 置信度 | 启发式伪置信度 | 无（`None`） |
| partial 标记 | 时长 < 15s 判 true | 恒 false |
| 时间戳 | 读取后丢弃 | 计算后丢弃（见下） |
| 卸载串行化 | 无锁 | `model_lifecycle_lock` |

**Parakeet 也丢时间戳**：`TimestampedResult` 同时携带 `timestamps: Vec<f32>` 与 `tokens: Vec<String>`（`model.rs:24-28`），但引擎只返回文本：

```rust
// parakeet_engine.rs:561-563
log::debug!("Parakeet transcription result: '{}'", result.text);
Ok(result.text)
```

### 2.5 两引擎的代码重复

`commands.rs` 是本项目最严重的重复：`whisper_engine/commands.rs`（568 行）与 `parakeet_engine/commands.rs`（577 行）结构逐段对应，包含同一份 `MODELS_DIR` 静态量、同名 `set_models_directory()` / `get_models_directory()`、同构的 init/load/get_current/is_loaded/has_available/validate_ready/transcribe/download/cancel/delete/open_folder 命令，甚至重复实现了同名的私有静态量：

- `whisper_engine/commands.rs:11` `static MODELS_DIR: Mutex<Option<PathBuf>>`
- `parakeet_engine/commands.rs:11` `static MODELS_DIR: Mutex<Option<PathBuf>>`

以及 `open_models_folder` / `open_parakeet_models_folder` 两份 40 行的 `#[cfg(target_os=...)]` 分支（`whisper_engine/commands.rs:529-568` vs `parakeet_engine/commands.rs:537-576`）。

`*_validate_model_ready_with_config()`（`whisper_engine/commands.rs:280-385` vs `parakeet_engine/commands.rs:230-344`）也是近 100 行的平行实现，差异仅在于 provider 字符串、模型筛选偏好（Parakeet 优先 int8）和错误文案。

内部的重复还有：`WhisperEngine::transcribe_audio` 与 `transcribe_audio_with_confidence`（§1.6）；`WhisperEngine::discover_models` 与 `commands::discover_models_standalone`（§4）。

---

## 3. 命令面与接线

### 3.1 Whisper 命令（15 个，`whisper_engine/commands.rs`）

`whisper_init`(:41)、`whisper_get_available_models`(:55)、`whisper_load_model`(:127)、`whisper_get_current_model`(:181)、`whisper_is_model_loaded`(:195)、`whisper_has_available_models`(:209)、`whisper_validate_model_ready`(:234)、`whisper_transcribe_audio`(:388)、`whisper_get_models_directory`(:407)、`whisper_download_model`(:422)、`whisper_cancel_download`(:495)、`whisper_delete_corrupted_model`(:512)、`open_models_folder`(:530)；另有非命令的 `whisper_validate_model_ready_with_config`(:280)。注册处：`lib.rs:644-655`、`lib.rs:814`。

### 3.2 Parakeet 命令（15 个，`parakeet_engine/commands.rs`）

`parakeet_init`(:41)、`parakeet_get_available_models`(:55)、`parakeet_load_model`(:72)、`parakeet_get_current_model`(:126)、`parakeet_is_model_loaded`(:140)、`parakeet_has_available_models`(:154)、`parakeet_validate_model_ready`(:179)、`parakeet_transcribe_audio`(:347)、`parakeet_get_models_directory`(:364)、`parakeet_download_model`(:379)、`parakeet_retry_download`(:499)、`parakeet_cancel_download`(:483)、`parakeet_delete_corrupted_model`(:520)、`open_parakeet_models_folder`(:538)；加 `parakeet_validate_model_ready_with_config`(:230)。注册处：`lib.rs:657-670`。

### 3.3 并行处理命令（11 个，均已注册但无消费者）

`parallel_commands.rs:26-263` 的 11 个命令注册于 `lib.rs:672-682`。前端零引用（见 §1.12），state 在 `lib.rs:471` 注册。

### 3.4 启动与 provider 选择

`lib.rs:541-558` 在 setup 中分别注入两个引擎的 models_dir 并各自 `spawn` 初始化（未加载模型，只建 engine 对象）。录制开始时按 transcript config 决定引擎：

- `localWhisper` → `whisper_validate_model_ready_with_config`（`audio/transcription/engine.rs:91-113`）
- `parakeet` → `parakeet_validate_model_ready_with_config`（`engine.rs:114-137`）
- **无配置或读取失败时默认 parakeet**（`engine.rs:71-87`、`167-183`），默认模型 `DEFAULT_PARAKEET_MODEL = "parakeet-tdt-0.6b-v3-int8"`（`config.rs:12`）。

### 3.5 转写 worker 的实际接线

`audio/recording_commands.rs:404,431` 与 `:591,618` 两处把 pipeline 的 `transcription_receiver` 交给 `transcription::start_transcription_task()`。该任务：
- 初始化引擎（`worker.rs:59`），失败则 emit `transcription-error` 并直接返回（`worker.rs:61-70`）。
- 起 1 个 worker（`worker.rs:74`），chunk 来自 VAD 段（`audio/pipeline.rs:869-875`，`sample_rate: 16000` 硬编码，`timestamp = segment.start_timestamp_ms/1000`）。
- 无 speech 时短路返回空（`worker.rs:28-30`）；空文本不落库（`worker.rs:220`）。
- 时间戳直接复用 VAD 段的相对秒数（`worker.rs:194-195`），上游 `VAD_REDEMPTION_TIME_MS = 500`（`audio/pipeline.rs:27`）。

注意 `whisper_transcribe_audio` / `parakeet_transcribe_audio` 这两个把 `Vec<f32>` 当参数的命令**没有任何 Rust 侧调用者**：前端包装函数存在（`frontend/src/lib/whisper.ts:305`、`parakeet.ts:189`）但从未被调用。它们把整段音频以 JSON 数组经 IPC 传输，实用性很低且扩大了命令面（详见 §4）。

---

## 4. 性能相关设计

### 4.1 线程模型

- 模型加载：Parakeet 使用 `spawn_blocking`（`parakeet_engine.rs:467`）；Whisper 的 `WhisperContext::new_with_params` 直接在 async 上下文中执行（`whisper_engine.rs:348`）。
- **推理全部未用 `spawn_blocking`**（全仓库 `spawn_blocking` 命中仅 17 处，全部在音频解码/设备枚举/加载路径，`grep` 结果见 `audio/import.rs:385,413,434`、`audio/retranscription.rs:202,222,242`、`audio/recording_manager.rs:120,138`、`device/discovery.rs:117`、`permissions.rs:139`、`parakeet_engine.rs:467`）。Whisper 的 `state.full(...)`（`whisper_engine.rs:620,775`）与 Parakeet 的 `transcribe_samples`（`parakeet_engine.rs:558`）都直接占用 tokio worker 线程。
- `audio/transcription/worker.rs:123-125` 通过 `Arc<Mutex<Receiver>>` 串行取 chunk，配合 `NUM_WORKERS = 1`（`:74`）确保时间顺序，但也意味着**转写吞吐完全等于单线程推理吞吐**。
- VAD 走 ONNX（`audio/vad.rs:37,66`），与 Parakeet 共享 ONNX Runtime 动态库；VAD 会话与 Parakeet 会话的线程策略不同（VAD 由 `silero_rs` 决定）。

### 4.2 批处理与缓冲区

- live 路径按 VAD 段切分，段最小 800 samples（50 ms @16 kHz，`audio/pipeline.rs:865`、`:935`），**没有段长上限**（`audio/pipeline.rs:19-26` 的注释明确说明无界连续语音是已知问题 #756）。
- 批处理路径（导入/重转写）切成 25 秒子段并在静音处切分（`audio/import.rs:526,537`、`audio/retranscription.rs:317,328`）。
- 因此**同一引擎在 live 与批处理下的输入长度分布完全不同**：Parakeet 在批处理下有 25 秒上界保护，在 live 下没有；Whisper 在 live 下同样没有上界（whisper.cpp 内部按 30 秒窗口滑动，风险较 Parakeet 小）。
- 下载侧缓冲：Parakeet `BufWriter::with_capacity(8*1024*1024)`（`parakeet_engine.rs:993`）；Whisper 直接 `file.write_all(chunk)`（`whisper_engine.rs:1189`），无显式缓冲。

### 4.3 量化

- Whisper 目录提供 f16 与 q5_0/q5_1 量化档（`config.rs:20-35`），量化版普遍只有 f16 的 1/3~1/2；默认模型 `DEFAULT_WHISPER_MODEL = "large-v3-turbo"`（`config.rs:8`，1549 MB，非量化）。
- Parakeet 只有 int8 一种量化（`QuantizationType::Int8` 为 `Default`，`parakeet_engine.rs:21-25`；两个目录条目都是 int8），且 `init_session(..., try_quantized=true)` 在 `.int8.onnx` 缺失时会静默回退到非量化 `.onnx`（`model.rs:100-113`）——但目录校验要求精确文件名与精确字节数（`parakeet_engine.rs:413-429`），因此这条回退路径在实际部署中**不可能被触发**，属死分支。

### 4.4 内存占用与 OOM / 阻塞 UI 风险

- 常驻内存的主要来源是模型：`large-v3` 2951 MB 权重（+ KV/状态）；`large-v3-q5_0` 1031 MB；Parakeet int8 约 670 MB。
- 引擎对象本身在启动时就创建（`lib.rs:544-558`），但**不加载权重**，因此启动开销可忽略。
- `ParallelProcessor` 若被启用，会按 worker 数**成倍**加载模型（`parallel_processor.rs:223-228`），且其内存预算参数不具备约束力（`:65` vs 无任何核算代码）。
- `system_monitor.rs` 的守护逻辑（70% 内存阈值、`calculate_safe_worker_count`）只服务 `ParallelProcessor`（`system_monitor.rs:26-35,117-189`），而 `ParallelProcessor` 无人调用 → **现行转写路径没有任何内存保护**。
- 阻塞 UI：Tauri 命令/setup 之外的重活跑在 async 运行时；由于转写是同步调用（§4.1），单次推理会占据一个 tokio worker 约等于“音频时长 × RTF”的时间。这不会冻结 WebView 主线程，但会挤占运行时线程池；在 `audio/import.rs`、`audio/retranscription.rs` 的逐段循环中大量 await 与同步推理交替，可能造成事件循环抖动。

### 4.5 其他热点

- `model.rs:371-375` 每个解码步都对 logits 做 `as_slice()`；`:318-324` 每一步做 3 次 `to_owned()`；`:349-426` 的 TDT 循环每步分配一次 decoder state（`:354` 的 `create_decoder_state` 会读取会话输入形状）。
- `WhisperEngine::clean_repetitive_text` 及三个辅助函数（`whisper_engine.rs:402-548`）在**每次转写结果上全量执行**：按词拆分、滑窗 2~5 词短语去重、再做一次 HashMap 统计。对短段开销可忽略，但这是纯文本启发式后处理，会改变输出（见 §5）。

---

## 5. 真实缺陷清单

### D1【P0】同步引擎推理阻塞 async 运行时，且锁覆盖整段推理
- `whisper_engine.rs:620` / `:775`：`state.full(params, &audio_data)` 在 async fn 内同步执行，无 `spawn_blocking`、无超时。
- `parakeet_engine.rs:544-558`：`current_model` 的**写锁**覆盖整个 ONNX 推理。
- 对比：模型加载路径知道要用 `spawn_blocking`（`parakeet_engine.rs:467`），说明这是遗漏而非设计。

### D2【P0】Whisper 下载客户端无任何超时
- `whisper_engine.rs:1143-1146`：`Client::builder().user_agent(...).build()`，无 `timeout` / `connect_timeout` / 读超时。
- 对照 Parakeet `parakeet_engine.rs:865-871` 的完整超时设置。对端挂起时下载无限期占用 `active_downloads` 槽位（该槽位还会阻止重试，`whisper_engine.rs:934-936`）。

### D3【P1】模型完整性只有魔数 + 90% 尺寸，且项目已有 SHA-256 能力却未使用
- 弱校验：`whisper_engine.rs:846-865`（魔数）、`:972-994` 与 `:220-248`（0.9 倍尺寸）。
- Parakeet 更严（精确字节数，`parakeet_engine.rs:418`）但同样是长度校验，同样无摘要。
- 项目自身能力：`build/onnxruntime.rs:15,310-341` 对构建期制品做 SHA-256，`sha2` 已在 `Cargo.toml:62`。
- 后果：`medium` 允许少 148 MB 仍判 `Available`，随后在 whisper.cpp 内触发未定义行为级别的张量读取。

### D4【P1】live 路径对 VAD 段长无上界
- `audio/pipeline.rs:19-27`：捕获策略只有 500 ms 静默闭合与 300/400 ms 前后填充，**没有最大段长**；注释自述“with continuous audio a 2000ms redemption keeps one VAD segment open indefinitely”，且 “Bounded live segmentation during continuous speech is tracked separately in #756”。
- 无界段会一次性送入 `ParakeetModel::transcribe_samples`（`parakeet_engine.rs:558`），使 encoder/解码内存与耗时随段长线性增长。

### D5【P1】两个引擎都计算了时间戳却全部丢弃
- Whisper：`whisper_engine.rs:793-794` 取到的 `t0/t1` 仅用于 `duration_seconds > 30.0` 的 trace 日志（`:799-802`）。
- Parakeet：`model.rs:465-468` 计算 `float_timestamps`，`parakeet_engine.rs:561-563` 只返回 `.text`。
- 上层的 `audio_start_time`/`audio_end_time` 完全来自 VAD 段边界（`audio/transcription/worker.rs:194-195`），与 ASR 自身的对齐结果无关。

### D6【P2】置信度不是模型置信度
- `whisper_engine.rs:637-643`：由字符数推导。whisper-rs 的 `full_get_token_prob()` 未使用。
- `audio/transcription/worker.rs:213` 把 `None` 兜底成常量 `0.85`，使 Parakeet 路径展示出无意义的“置信度”。

### D7【P2】Parakeet 只有 CPU 执行提供者，与文档宣称的加速不符
- `model.rs:97`：`let providers = vec![CPUExecutionProvider::default().build()];` —— 硬编码，无条件。
- `parakeet_engine/mod.rs:9` 文档称 “Real time on M4 Max”，但在 macOS 上也未启用 CoreML EP。
- `model.rs:91,125-129`：`intra_threads` 参数永远是 `None`（`new()` 三处调用均传 `None`），`with_intra_threads` 分支为死代码，线程数交给 onnxruntime 默认值；同时 `.with_parallel_execution(true)`（`model.rs:123`）在单 worker 下意义有限。

### D8【P2】整块并行处理子系统是死代码，且其设计有 OOM 风险
- 无调用者：`frontend/src` 对 `initialize_parallel_processor`/`start_parallel_processing`/`prepare_audio_chunks` 零命中；`lib.rs:471` 仍注册 state、`lib.rs:672-682` 仍暴露 11 个命令。
- 每 worker 一份模型：`parallel_processor.rs:223-228`；且用 `WhisperEngine::new()`（无 models_dir）→ `whisper_engine.rs:111-113,151-156` 走 release 回退目录。
- `memory_budget_mb`（`parallel_processor.rs:65`）无任何消费点。
- 与 `worker.rs:74` 的 `NUM_WORKERS = 1` 直接矛盾（注释写明为保序而串行）。

### D9【P2】`is_partial` 语义错误且无下游消费者
- `whisper_engine.rs:611-612`：仅按 <15 s 判定。
- 无 partial→final 合并：`frontend/src/contexts/TranscriptContext.tsx:288-337` 只做 `sequence_id` 去重与排序，`is_partial` 仅被写入对象（`:312`）。
- 跨引擎不一致：Parakeet 恒 false（`parakeet_provider.rs:40`、`worker.rs:495`）。

### D10【P2】默认语言是 `auto-translate`
- `lib.rs:124-126` 默认 `"auto-translate"` → `whisper_engine.rs:570-576` 的 `should_translate = true`。用户未显式设置前，所有 Whisper 输出被翻译为英文。

### D11【P2】`ModelStatus::Error` 在引擎内部永不构造
- 两个引擎都在 `load_model`/`delete_model` 中匹配 `ModelStatus::Error`（`whisper_engine.rs:371-373,406,926-928`；`parakeet_engine.rs:504-506,607-613`），但全模块搜索没有任何构造点（"Error" 命中的全是字符串或 `DownloadCancelled`）。

### D12【P2】目录扫描的 `discover_models_standalone` 校验比引擎路径更弱
- `whisper_engine/commands.rs:93-101`：只要求 `文件大小 >= 1 MB`，**不做魔数校验**，与 `whisper_engine.rs:228` 的 `validate_model_file` 判定标准不一致；同一份模型在这两条代码路径下可能得到不同的 `ModelStatus`。
- 同时该函数还重复实现了整个 catalog 遍历（`:91-118`），是 `WhisperEngine::discover_models`（`whisper_engine.rs:210-294`）的复制品。
- Parakeet 侧**没有对应实现**：`parakeet_get_available_models` 在引擎未初始化时直接报错（`parakeet_engine/commands.rs:66-68`），而 Whisper 侧会回退到目录扫描（`whisper_engine/commands.rs:67-70`）。两引擎行为不一致。

### D13【P3】Whisper 下载要求先 `discover_models()`，而 Parakeet 显式补了这一前置步骤
- `whisper_engine/commands.rs:451` 直接调用 `download_model`；`WhisperEngine::download_model` 自身不扫描目录，若 `available_models` 为空，则 `finish_download` 中 `models.get_mut(model_name)` 为 `None`（`whisper_engine.rs:1058-1066`），**状态永远不发布为 `Available`**，文件已下载但 UI 仍显示 Missing，直到下一次 `discover_models`。
- Parakeet 明确处理了这一点：`parakeet_engine/commands.rs:418-423` 先 `discover_models()`。

### D14【P3】错误/失败路径被静默吞掉
- `whisper_engine/commands.rs:468-474`：取消被当作 `Ok(())` 返回。注释解释是有意的，但调用方无法区分“成功”与“取消”。
- `whisper_engine.rs:632-635`、`:788-791`：段文本读取失败 `continue`，无日志、无计数，最终可能返回空串而无任何线索。
- `parakeet_engine/commands.rs:420-423`：`discover_models` 失败只 warn 后继续。

### D15【P3】两处 `expect` 在 fallible 路径上
- `whisper_engine/commands.rs:16-17`、`parakeet_engine/commands.rs:16-17`：`app.path().app_data_dir().expect("Failed to get app data dir")` —— 在 setup 阶段 panic 会直接终止应用启动。
- `parakeet_engine.rs:1143`：`result.expect("successful transfer must carry final progress")` —— 依赖前置 `result.is_ok()` 保护，属于可接受但脆弱的写法。

### D16【P3】两处参数无界、可能异常分配
- `model.rs:168`：`let mut vocab = vec![String::new(); max_id + 1];` —— `max_id` 来自 `vocab.txt` 的解析结果，未做上界校验；一个损坏/恶意 vocab 文件可以让进程尝试分配巨大容器。
- `audio/import.rs:558`、`retranscription.rs:350`：进度计算用 `processable_count`（retranscription 侧未 `max(1)`）作除数；空集合时 `i` 循环不执行，实际不会除零，但两处写法不一致。

### D17【P3】死代码与注释掉的模块
- `whisper_engine/_stderr_suppressor.rs`：整文件被注释（`:13-83`），模块声明也被注释（`whisper_engine/mod.rs:7,15`），调用点同样注释（`whisper_engine.rs:345,617`）。
- `whisper_engine/mod.rs:1-15` 用 `pub use xxx::*` 通配再导出全部子模块，配合未使用的 `stderr_suppressor` 预留，模块边界很松。
- `audio/recording_commands.rs.backup`（含旧引擎调用）仍在源码树中（`grep` 命中 `:156,1316,1531`），属仓库卫生问题。

### D18【P3】两个引擎的删除/目录语义不一致
- Whisper `delete_model` 删单个 `.bin` 文件（`whisper_engine.rs:887,909`）。
- Parakeet `delete_model` 删整个模型目录 `remove_dir_all`（`parakeet_engine.rs:590`）。
- 两者共用同一个父目录（`models/`），`open_models_folder` 打开的是父目录，会把 Parakeet 子目录也暴露在同一视图里（`whisper_engine/commands.rs:530` vs `parakeet_engine/commands.rs:538-541`）。

### D19【P3】`transcribe_audio` 类命令把音频当 JSON 数组走 IPC
- `whisper_engine/commands.rs:388`、`parakeet_engine/commands.rs:347` 接受 `Vec<f32>`；前端包装 `frontend/src/lib/whisper.ts:305`、`parakeet.ts:189` 存在但零调用。
- 这些命令仍在 `lib.rs:651,664` 注册，属于无用且高开销的暴露面。

### D20【P3】`system_monitor` 的读锁阻塞与除零隐患
- `system_monitor.rs:67-72`：`refresh_system_info` 持有 `system` 写锁期间执行 `tokio::time::sleep(200ms)`，会阻塞所有 `get_current_resources` 读锁。
- `system_monitor.rs:85-87`：`system.cpus().len()` 未做零值保护（实测不会为 0，但属未防御除零）。
- 该模块整体只为无人调用的 `ParallelProcessor` 服务（§D8）。

---

## 6. 风险排序（按严重度降序）

| # | 风险 | 证据 |
|---|---|---|
| 1 | **同步 ASR 推理直接在 async 运行时上执行**，最坏情况等于单段音频的处理时长（live 路径无段长上界 → 上界不可预估），Parakeet 还以写锁覆盖整段推理，加载/卸载会被拖住 | `whisper_engine.rs:620,775`；`parakeet_engine.rs:544-558`；`audio/pipeline.rs:19-27` |
| 2 | **模型文件无密码学完整性校验**：Whisper 允许少 10% 仍标 `Available`，两个引擎都无摘要；项目在构建期已有 SHA-256 机制却未复用到运行时下载 | `whisper_engine.rs:220-248,846-865,972-994`；`parakeet_engine.rs:413-429`；`build/onnxruntime.rs:310-341` |
| 3 | **Whisper 下载无超时**，可无限挂起并长期占用互斥的下载槽，阻止重试 | `whisper_engine.rs:1143-1146` vs `parakeet_engine.rs:865-871` |
| 4 | **Parakeet 无 GPU（只有 CPU EP）**，与 `mod.rs` 文档宣称的 “Real time on M4 Max” 及项目 GPU 叙事不符；macOS 上 CoreML 完全未接 | `model.rs:97`；`parakeet_engine/mod.rs:9` |
| 5 | **并行处理子系统是死代码却仍暴露**：11 个命令无消费者，且设计上每 worker 加载一份完整模型（4×large-v3 ≈ 12 GB），内存预算参数无实际作用 | `parallel_commands.rs:26-263`；`lib.rs:471,672-682`；`parallel_processor.rs:65,223-228`；`worker.rs:74` |
| 6 | **live 路径 VAD 段无长度上界**，连续语音（如播客/系统音频）可产出任意长单段 | `audio/pipeline.rs:19-27`（自述 #756） |
| 7 | **partial/final 机制名存实亡**：<15 s 即标 partial，下游无合并逻辑，跨引擎语义还不一致 | `whisper_engine.rs:611-612`；`TranscriptContext.tsx:288-337`；`parakeet_provider.rs:40` |
| 8 | **时间戳在两端都被算出来又丢掉**，播放同步完全依赖 VAD 边界而非 ASR 对齐 | `whisper_engine.rs:793-794`；`model.rs:465-468` + `parakeet_engine.rs:561-563` |
| 9 | **`transcribe_audio` 与 `transcribe_audio_with_confidence` 约 120 行重复**，且置信度是字符数启发式（上层还会兜底成 0.85） | `whisper_engine.rs:551-666` vs `668-839`；`whisper_engine.rs:637-643`；`worker.rs:213` |
| 10 | **默认 `auto-translate`**：用户未设置前所有 Whisper 输出被强制翻译成英文 | `lib.rs:124-126`；`whisper_engine.rs:570-576` |
| 11 | **两个 `commands.rs` 近乎逐行重复**（含同名 `MODELS_DIR` 静态量），且 Whisper 缺 Parakeet 的下载前置发现步骤，导致下载成功但状态不更新 | `whisper_engine/commands.rs:11,451` vs `parakeet_engine/commands.rs:11,418-423` |
| 12 | **`ModelStatus::Error` 死枚举、`discover_models_standalone` 弱校验**、`openmp`/`openblas` 死特性等零散的死代码与不一致 | `whisper_engine.rs:371-373`；`parakeet_engine.rs:504-506`；`whisper_engine/commands.rs:93-101`；`Cargo.toml:52-53,188,200` |

---

## 摘要要点

- Whisper 侧 `WhisperEngine` 为进程内单例，模型平铺存于 `app_data_dir()/models/ggml-*.bin`；`load_model` 前会先卸载旧模型，但 `unload_model` 没有 lifecycle 锁（`whisper_engine.rs:380-391`），与 Parakeet 的 `model_lifecycle_lock`（`parakeet_engine.rs:267`）不对称。
- 编译期后端门控在 `acceleration.rs:13-25`（cuda→vulkan→hipblas→metal→cpu），`flash_attn` 只对 Metal/CUDA 的 High/Ultra 档开启（`acceleration.rs:68-71`）；但 `adaptive_config.use_gpu` 与 `max_threads` 均未接线（`whisper_engine.rs:318-331`、`:605-609`），`openmp`/`openblas` 在 Windows/Linux 是死特性（`Cargo.toml:52-53,188,200`）。
- 无流式转写：只有整段批处理入口，`transcribe_audio` 与 `transcribe_audio_with_confidence` 是约 120 行重复实现（`whisper_engine.rs:551-666` / `668-839`）；置信度由字符数推导（`:637-643`），时间戳取到后丢弃（`:793-794`）。
- `is_partial` 仅表示“音频短于 15 秒”（`whisper_engine.rs:611-612`），下游无 partial→final 合并，Parakeet 恒为 false（`parakeet_provider.rs:40`），跨引擎语义不一致。
- 语言默认值是 `"auto-translate"`（`lib.rs:124-126`），用户未设置前所有 Whisper 输出被翻译为英文；Parakeet 完全不支持语言参数（`parakeet_provider.rs:29-34`）。
- Parakeet 是 ONNX Runtime（`ort` 2.0.0-rc.10）+ NeMo Parakeet TDT 0.6B，加载 `encoder/decoder_joint/nemo128` 三个 `.onnx`；执行提供者硬编码为 CPU（`model.rs:97`），macOS 上 CoreML 未接，`intra_threads` 参数永远为 `None`（`:125-129`）。
- Parakeet 生命周期明显更严谨：支持 HTTP Range 断点续传（`parakeet_engine.rs:913-972`）、精确字节数校验（`:418`）、8 MB 写缓冲（`:993`）、30 秒读超时（`:1006`）、乐观 revision 重试（`:337-411`）。
- 两引擎共同缺陷：模型下载**无 SHA-256**（Whisper 只有魔数 + 90% 尺寸容差 `whisper_engine.rs:220-248,972-994`；Parakeet 只有精确长度 `parakeet_engine.rs:418`），而项目在 `build/onnxruntime.rs:310-341` 已有校验能力；Whisper 下载客户端连超时都没有（`whisper_engine.rs:1143-1146`）。
- 性能关键问题：ASR 推理未用 `spawn_blocking`、直接在 async 运行时上同步执行（`whisper_engine.rs:620,775`；`parakeet_engine.rs:544-558`），且 Parakeet 以写锁覆盖整段推理。
- live 路径对 VAD 段长无上界（`audio/pipeline.rs:19-27`，自述 #756），批处理路径才切 25 秒子段（`audio/import.rs:526`、`audio/retranscription.rs:317`）；`large-v3` 权重 2951 MB（`config.rs:25`），而唯一的内存守护模块只服务无人调用的并行子系统。
- `parallel_processor` + `parallel_commands` 是完整但**零消费者**的死子系统（`lib.rs:471,672-682`；前端无引用），其设计为每 worker 加载一份完整模型（`parallel_processor.rs:223-228`），预算参数 `memory_budget_mb` 无消费点（`:65`），与现行 `NUM_WORKERS = 1`（`worker.rs:74`）直接矛盾。
- 其他真实缺陷：`ModelStatus::Error` 永不构造（`whisper_engine.rs:371-373`；`parakeet_engine.rs:504-506`）；Whisper 下载缺少 Parakeet 那样的前置 `discover_models`，可能“文件已下载但状态仍 Missing”（`whisper_engine/commands.rs:451` vs `parakeet_engine/commands.rs:418-423`）；`discover_models_standalone` 只按 1 MB 判定且不做魔数校验（`whisper_engine/commands.rs:93-101`）；完整报告见本文件 `D:\Meetly\analysis\03-stt-engines.md`。
