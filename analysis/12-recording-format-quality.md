# 录音格式、质量与按应用录制

## 需求

1. 提示录音质量
2. 能够选择具体应用进行录制
3. 能够设置录制的格式（如 mp3 及其他）与音频质量

## 一、格式与质量：设置此前是**完全失效**的

### 发现

`RecordingPreferences.file_format` 字段**早已存在**：

- 有默认值（`"mp4"`）
- 有序列化/反序列化
- 设置页显示它（"File Format: MP4 files"）

但**代码中从未读取它**。全仓库对 `file_format` 的引用只有 3 处，且都在 `recording_preferences.rs` 内部（结构体定义、默认值、日志）：

```
recording_preferences.rs:140:  prefs.save_folder, prefs.auto_save, prefs.file_format,
recording_preferences.rs:151:  preferences.save_folder, preferences.auto_save, preferences.file_format,
```

而真正决定输出的 `encode_single_audio`（`audio/encode.rs`）**硬编码**：

```rust
"-c:a", "aac",
"-b:a", "192k",
"-profile:a", "aac_low",
"-f", "mp4",
```

**结论：无论用户在设置里选什么，录音永远是 AAC 192k 的 MP4。UI 在说谎。** 这比"缺少功能"更糟——用户以为自己改了，实际没有。

### 实现

#### 新增 `audio/formats.rs`（唯一事实来源）

```rust
pub enum RecordingFormat { M4a, Mp3, Wav, Flac, Ogg }
pub enum RecordingQuality { Low, Medium, High, Best }
pub struct AudioEncodingSettings { format, quality }
```

`RecordingFormat` 提供 `id` / `extension` / `label` / `ffmpeg_encoder` / `ffmpeg_muxer` / `is_lossless`；
`AudioEncodingSettings::ffmpeg_codec_args()` 生成编码参数。

支持的格式（全部实测可用）：

| 格式 | 编码器 | 说明 |
|---|---|---|
| M4A | `aac` (AAC-LC) | 默认，兼容性最好 |
| MP3 | `libmp3lame` | 最常被要求的便携格式 |
| WAV | `pcm_s16le` | 无损未压缩，体积最大 |
| FLAC | `flac` | 无损压缩 |
| OGG | `libopus` | 语音场景单位体积质量最高 |

质量档位为**有损格式的码率**：64 / 128 / 192 / 256 kbps，默认 128。
**无损格式（WAV/FLAC）不加 `-b:a`**——给 FLAC 传码率要么被拒绝要么被忽略，省略它才诚实。UI 也因此隐藏质量选择。

#### 兼容旧配置

旧版存的是 `"mp4"`（实际产出的是 AAC 音频）。`RecordingFormat::parse` 把 `mp4`/`aac` 映射到 `M4a`，**不改变已有录音的文件名**。未知值回退默认而非报错——损坏的配置文件不该阻断录音。

#### 转码时机：在结束时做一次，而非逐块编码

checkpoint 保持 AAC/MP4 不变（见下），用户格式在**最后合并后转码一次**：

- checkpoint 逐块编码成任意格式，若是有损格式会**累积多代损失**，且部分容器不便拼接；
- 崩溃恢复路径完全不受影响；
- 只需一次转码，CPU 成本最低。

`finalize()` 因此：
1. 合并 checkpoint → `.merged-intermediate.mp4`（`-c copy`，快）
2. 若目标格式非 M4A：`spawn_blocking` 转码到 `audio.{ext}`，成功后删除中间文件
3. 转码失败时**保留中间文件**并明确报错，说明音频在哪——**宁可留下可用音频，也不静默丢失录音**

#### 格式列表由 Rust 提供给 UI

新增命令 `get_recording_encoding_options`。原 UI 的格式列表与编码器**毫无关联**，这正是它能长期显示一个从不产出的格式的原因；现在 UI 只能提供编码器真正支持的格式。

### 验证（实测 FFmpeg）

用真实 ffmpeg 逐个格式转码，并探测产物：

| 格式 | 体积 | 探测结果 |
|---|---|---|
| m4a | 31.9 KB | `aac (LC) (mp4a), 48000 Hz, mono, 124 kb/s` |
| mp3 | 32.4 KB | `mp3 (mp3float), 48000 Hz, mono, 128 kb/s` |
| wav | 188.1 KB | `pcm_s16le, 48000 Hz, mono, s16, 768 kb/s` |
| flac | 119.3 KB | `flac, 48000 Hz, mono, s32 (24 bit)` |
| ogg | 37.4 KB | `opus, 48000 Hz, mono` |

**质量档位确实生效**（同源对比）：

| 码率 | 体积 | 探测 |
|---|---|---|
| 64k | 16.3 KB | `64 kb/s` |
| 256k | 64.6 KB | `256 kb/s` |

单元测试 15 例：id 往返、旧 `mp4`/`aac` 映射、未知值回退、扩展名唯一、无损格式不加 `-b:a`、有损格式加且值正确、码率单调递增、`from_stored` 容忍损坏配置等。

### 回归检查

`retranscription.rs:144` 的候选列表**已包含** `audio.mp4 / m4a / wav / mp3 / flac / ogg`，所以新格式可被重新转写检索到。

`recover_audio_from_checkpoints`（崩溃恢复）仍输出 `audio.mp4`——**刻意保留**：它用 `-c copy` 无损拼接 checkpoint，属于抢救路径，不应因用户格式偏好而增加失败面。

## 二、按应用录制

### 可行性（已确认）

| 项 | 结果 |
|---|---|
| Windows 版本 | **11 Insider Build 29667** |
| 所需最低版本 | 20348（`AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS`） |
| 结论 | **技术支持** ✅ |

现有 Windows 系统音频走 WASAPI loopback（`audio/devices/platform/windows.rs`），只能录**整个输出设备**。按应用录制需要
`ActivateAudioInterfaceAsync` + `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`（手动 Win32 FFI），外加进程枚举与选择 UI。

**状态：未开始。** 这是本项目最大的一块原生工作（预计 400–700 行 unsafe FFI），且必须做成可降级——
在不支持的 Windows 10 上要退回整设备循环录音并说明原因。

## 三、当前状态汇总

| 项 | 状态 |
|---|---|
| 格式选择（5 种） | ✅ 已实现并实测 |
| 质量选择（4 档） | ✅ 已实现并实测 |
| UI 选择器 | ✅ 已实现，`tsc` 与 `pnpm build` 通过 |
| 按应用录制 | ⏸ 未开始（技术可行，工作量大） |
| 录音质量**提示** | 说明：质量档位已可直接选择并显示码率；若指"录音电平/音量提示"（如增益过低警告），这是**另一项**功能，需要明确 |

### 验证状态

| 项目 | 结果 |
|---|---|
| `cargo check -p meetily` | Finished，0 error |
| `audio::formats` | **15 pass** |
| `http_client` | **12 pass**（+1 ignored 端到端） |
| `storage::config` | **7 pass** |
| `audio::incremental` | 2 pass |
| `retranscription` | 15 pass |
| 前端 `tsc --noEmit` | exit 0 |
| `pnpm build` | ✓ 8/8 静态页 |
| FFmpeg 逐格式实编 | 5/5 有效编码 |

**未做的验证**：端到端实机录音（改设置 → 录一段 → 确认产出文件名与编码）。这需要真实录音，
建议用 `pnpm run tauri:dev` 手动确认一次。
