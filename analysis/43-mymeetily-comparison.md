# MyMeetily 代码分析：可以吸收什么

仓库：`https://github.com/fnightly/MyMeetily`（本地克隆在 `D:\_ref\MyMeetily`）

## 它是什么

**Go + React + TypeScript + Wails v2** 的 Windows 桌面会议助手，全程离线。

| | MyMeetily | 我们（Meetily） |
|---|---|---|
| 技术栈 | Go + React + Wails v2 | Rust + Next.js + Tauri 2 |
| 安装包 | ~22 MB | ~150 MB |
| 模块数 | 87 个文件 / ~4k 行 Go | 大得多 |
| LLM | Ollama | Ollama / Claude / Groq / OpenRouter |
| VAD | **能量 VAD** | **silero（神经网络）** |

**结论先说**：技术栈不同，**代码不能直接搬**。而且它在大多数维度上比我们**简单**，不是更全。真正值得吸收的只有 **2 件事**，另外 **4 件事我们已经做得更好（不要白改）**。

---

## ✅ 值得吸收（2 项）

### 1. 把音频诊断指标暴露到界面 ⭐ 最高价值

它的 `internal/audio/livetranscribe.go` 里有一个 `LiveTranscribeDiag`：

```go
AudioPackets   // 收到的音频包数
VoiceFrames    // 判定为有人声的帧
SilentFrames   // 判定为静音的帧
SentencesCut   // 切出的句子数
LastRMS        // 最近一次 RMS 电平
WhisperRunning // 正在跑的 whisper 进程数
```

用原子计数器更新，UI 随时 `Snapshot()` 取一份展示。

**为什么值得吸收 —— 我们正好缺这块：**

我们**已经算了同类指标**，但**只留在 Rust 内部，前端一个都看不到**：

```
batch_processor.rs:109  pub struct AudioMetricsBatcher
batch_processor.rs:123  pub struct AudioMetricsSummary
```

前端 grep `audio-metrics` / `dropped` / `vad_rate` → **零命中**。

**这直接关系到你刚报的那个问题**：音量条是假的、显示「等待音频输入…」，如果界面能显示「音频包 0 / 人声帧 0 / RMS 0」，一眼就能判断是**没采到音频**还是**只是显示坏了**，不用猜。

**建议做法**：录屏页加一个可折叠的「录音诊断」，把已有的 `AudioMetricsSummary` 通过事件发到前端。改动小，收益是让人能自己诊断。

---

### 2. 能量 VAD 作为 silero 的回退

它是纯能量的、极便宜的判据，参数是实测调过的：

```go
vadWindowMs  = 30    // 30ms 分析窗
vadThreshold = 0.015 // RMS 阈值 —— 注释写 "low enough for consumer mics"

vadSilenceMs  = 1000.0 // 连续静音 1s 判定句子边界
vadMinUtterMs = 800.0  // 短于 0.8s 的片段丢弃
```

我们用的是 **silero**，质量更好但更重，且**依赖模型文件**（`vad.rs` 里 `silero_rs::VadSession`）。

**值得吸收的理由**：silero 模型缺失或加载失败时，有一个 30 行就能实现的能量 VAD 兜底，比「没有 VAD」好得多。它的阈值针对**消费级麦克风**调过，正好适配我们用户的实际设备。

> 注：它这个 VAD 还有个小 bug —— `_ = samplesPerFrame * bytesPerSample * lt.cfg.Channels` 算出来直接丢弃了（下划线赋值），说明原本想做基于帧字节数的精确切分但没接上。吸收思路即可，别照抄。

---

## ❌ 我们已经有、或更强（不要白改）

这几点我一开始以为是我们缺的，**查证后发现相反**：

| 能力 | MyMeetily | 我们 |
|---|---|---|
| **中文标点/无内容段过滤** | `shouldSkipTranscriptText` trim 简单标点集 | **更强**：`transcript_text.rs` 有完整 CJK 标点集（`。，！？；：、…—「」『』（）【】` 等），注释还点明了「Whisper 在静音处输出句号」「重复循环」两种中文特有失败模式 |
| **相邻重复段去重** | `cleanSegments` 用 `lastKey` 去重 | **已有**：`transcript_text.rs:161 dedupe_adjacent` |
| **导出格式** | MD + HTML | **更多**：`md / txt / docx / html / srt / vtt / json` 共 7 种 |
| **中文纪要防幻觉** | prompt 里写「信息缺失不要编造，写'未知'」 | **已有同等约束**：`chinese.rs`「信息缺失时写'未提及'，不要编造」 |
| **重新生成纪要** | `RetrySummary`（从已存转写重跑摘要） | **已有**（录音库「重新生成纪要」） |

> 有一处**我们自己要修**：`CHINESE_SUMMARY_OUTLINE`（`chinese.rs:124`）只在测试里被引用，**生产代码没用**，编译器一直在报 `never used`。要么接上，要么删。

---

## ⚪ 架构参考（可以看，但不建议现在动）

### 声明式流水线（eino graph）

它用 CloudWeGo 的 [eino](https://github.com/cloudwego/eino) 把处理流程写成图：

```
START → AudioPreprocess → ASR → Summarize → FormatOutput → END
```

价值在于：**每个节点独立可测**、**状态是显式结构体**（`MeetingState`）、**失败可定位到节点**。

我们的流水线是命令式的、能跑但较散。**现在不建议重构成图** —— 收益是架构美观，成本是大改，而我们刚经历过一次启动崩溃，稳定性优先。

### 打包体积（22 MB vs 150 MB）

它是 Go + 复用系统 WebView2。**这个差距追不回来**，也不用追：我们的大头是 bundled ONNX Runtime + FFmpeg + whisper，这些是**本地推理能力**的代价。它把 whisper 交给外部 `whisper.cpp` 进程，等于把体积转嫁给了用户自己装。

---

## 建议的落地顺序

| 优先级 | 事项 | 成本 | 收益 |
|---|---|---|---|
| **P1** | 音频诊断指标发到前端（可折叠面板） | 小 | 用户能自己判断「是不是没采到音频」，正是你刚遇到的问题 |
| **P2** | 修 `CHINESE_SUMMARY_OUTLINE` 死代码 | 很小 | 消除编译警告，或接上真正的中文大纲 |
| **P3** | 能量 VAD 兜底 | 中 | silero 不可用时仍有分段能力 |
| — | 重构为 eino 式图流水线 | 大 | **不建议现在做** |

---

## 我确认过的事实边界

- 本地克隆了 `main` 分支（depth 1），**没有**构建或运行它，所以它的功能描述来自**读代码**，不是实际使用验证。
- 「我们的实现更强」的判断来自 grep + 读源码，**没有**跑对应测试做行为级验证。
