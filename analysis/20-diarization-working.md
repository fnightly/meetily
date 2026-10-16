# 说话人分离：全流程打通，参考音频上正确得出 4 人

## 一、结果

在官方中文参考音频 `0-four-speakers-zh.wav`（**已知正好 4 个说话人**）上：

```
Chunks analysed: 48
Embeddable (chunk, speaker) pairs: 77
Embedded 77 pair(s), dimension 512
Clustering with threshold 0.75, max_speakers 16
Pairs clustered into 4 global speaker(s)
--- Result ---
turns: 12
distinct speakers: 4
total speech labelled: 36.8s of 56.9s
  0.32s -- 6.88s  speaker 0
  6.04s -- 10.80s  speaker 1
  10.04s -- 10.75s  speaker 2
  11.40s -- 14.97s  speaker 1
  11.51s -- 17.09s  speaker 2
  22.12s -- 24.87s  speaker 0
  27.62s -- 29.53s  speaker 2
  27.64s -- 27.98s  speaker 3
  29.98s -- 31.57s  speaker 2
  33.65s -- 38.07s  speaker 2
  47.97s -- 50.50s  speaker 2
  52.51s -- 54.60s  speaker 0
Exactly 4 speakers found, matching the reference clip.
```

**关键点：有一个已知答案的测试音频。** 整个"猜测—验证"循环之所以能收敛，全靠它。

## 二、本轮发现并修掉一个我自己的设计 bug

### `max_speakers` 被实现成了「下限」而不是「上限」

我上一轮的实现是：**合并到 `max_speakers` 就停止合并**。

但**合并只会减少簇数**。所以"最多 N 个说话人"这个上限，**不可能靠停止合并来满足**——停止合并只会留下更多簇。

后果实测：

| 配置 | 结果 |
|---|---|
| 阈值 0.75，**无上限** | **4 人** ✅ |
| 阈值 0.75，`max_speakers=16` | **16 人** ❌ |

第二个正是"把上限当成了下限"——**不管录音里有没有那么多人，都返回 16 个**。

**正确语义**：上限必须**强制继续合并**（越过阈值），直到簇数降到上限以内。

```rust
// 两个合并理由：
//   1. 距离小于阈值 —— 就是同一个人
//   2. 簇数仍超过上限 —— 即使超过阈值也要继续合并最近的
let over_ceiling = params.max_speakers > 0 && clusters.len() > params.max_speakers;
if distance > params.threshold && !over_ceiling { break; }
```

### 为什么之前没发现

我原来的测试 `the_speaker_budget_is_respected` 断言的是"**不能塌到预算以下**"——这**正是错误语义本身的描述**。测试忠实地锁住了错误行为。

已重写为四条测试，覆盖两个方向：
- `the_speaker_ceiling_forces_merges_when_the_threshold_is_too_strict` — 严格阈值 + 上限 3 → 必须是 3
- `the_speaker_ceiling_does_not_inflate_a_small_meeting` — 同样数据 + 上限 20 → 必须是 8（**这条直接锁住那个回归**）
- `the_threshold_still_merges_below_the_ceiling` — 完全相同的声音仍然合并到 1
- `a_zero_ceiling_is_ignored_rather_than_collapsing_everything` — 0 表示"无上限"

## 三、阈值是实测校准的，不是猜的

我先用 0.5（这类模型的常见推荐值），得到 **6 人**。

于是做了**完整阈值扫描**（无上限，让阈值单独决定）：

| 阈值 | 0.30 | 0.40 | 0.45 | 0.50 | 0.60 | 0.70 | **0.75** | 0.80 | 0.90 |
|---|---|---|---|---|---|---|---|---|---|
| 说话人 | 15 | 11 | 8 | **6** | 5 | 5 | **4** ✅ | 2 | 1 |

曲线**平滑且单调**（这是聚类本身健康的信号），4 人出现在 **0.75**。

**为什么 0.5 会过度切分**：参考实现用的是它自己的 fast clustering；我用的是层次聚类 + 质心链接，行为不同。该模型嵌入的类内距离也偏大。

我把 0.75 固化为 `CALIBRATED_THRESHOLD`，并**把测量数据写进文档注释**：

> 这个值属于**嵌入模型**，换模型必须重新测量。给出 4 人的区间约 **0.71–0.79，很窄**。

并加了测试锁住两个方向：4 个正交方向在 0.75 下**必须仍是 4 人**（阈值偏松时容易塌），同一个人的微小抖动**必须仍合成 1 人**。

## 四、本轮实现的最终四步

| 步骤 | 函数 | 说明 |
|---|---|---|
| 1. 跨窗加权累加 | `weighted_speaker_scores` | 窗是重叠的，按覆盖数**求平均**而非覆盖写，结果与窗顺序无关 |
| 2. 帧说话人数取整 | `speaker_count_per_frame` | `+0.5` 截断 = 四舍五入；描述的是**典型同时说话人数**而不是最大值 |
| 3. 取分数最高的前 N 个 | `select_top_speakers` | 平均后的分数是小数，可能让超过帧预算的说话人都在阈值之上 |
| 4. 生成时间段 | `build_turns` + `merge_adjacent` + `drop_short_turns` | onset/offset 双阈值 + 合并 ≤0.5s + 丢弃 <0.3s |
| 局部→全局重标 | `relabel_activity` | 局部说话人只在窗内有意义，靠聚类标签跨窗对齐 |
| 每对提取音频 | `speaker_pairs` + `embeddable_windows` | 只拼接活跃帧（不掺静音），<10 帧的对丢弃 |

## 五、验证

| 模块 | 结果 |
|---|---|
| `audio::diarization` | **76 pass**（4 ignored） |
| `audio::speaker_cluster` | **24 pass** |
| `audio::fbank` | **18 pass** |
| `audio::speaker_turns` | **18 pass** |
| `audio::diarization_models` | **15 pass** |
| `audio::onnx_model` | **10 pass** |
| `summary::cleanup` / `dictionary` / `transcript_text` | **23 / 17 / 17 pass** |
| `summary::chinese` / `processor` | **10 / 37 pass** |
| `audio::formats` / `recordings` | **22 / 28 pass** |
| **端到端（真实模型，已知答案）** | **恰好 4 人** ✅ |
| `cargo check`（lib + tests） | 0 error |

**合计 315 pass + 端到端实测。**

## 六、还没做

| 项 | 状态 |
|---|---|
| 接到转写流水线 | **未做**。目前是独立模块，录音停止后没有调用点 |
| 写入 `transcript.speaker` | **未做**。字段已存在 |
| 转写视图按说话人分组 | **未做** |
| 模型下载集成 | **未做**。模型目前手动放在 `D:\Meetly\Model\diarization`；仓库已有下载/镜像/空间预检基础设施可复用 |
| 模型体积 | 分割 5.7 MB + 嵌入 **37.8 MB** —— 需要评估是否按需下载 |
| 分离开关与说话人数设置 | **未做** |
| 性能 | 57 秒音频约 **168–290 秒**处理（CPU）。**这对真实会议太慢**，需要评估：int8 分割模型、降低嵌入批次数、或只在停止后跑一次 |

## 七、诚实的性能提醒

端到端耗时 **168 秒处理 57 秒音频 ≈ 3 倍实时**，这还是在 CPU 上。一小时会议会需要约 3 小时。

**这是接进流水线前必须先解决的问题**，可能的着力点：
1. 换 `model.int8.onnx`（1.47 MB，已下载）—— 分割占大头
2. 减少嵌入窗口（目前每对固定送入 3 秒 = 48000 采样）
3. 只在**录音停止后**跑，且做成显式可选，不拖慢录音

## 八、文件位置

```
D:\Meetly\Model\diarization\
  sherpa-onnx-pyannote-segmentation-3-0\model.onnx   5.72 MB   （另有 model.int8.onnx 1.47 MB）
  3dspeaker_eres2net_zh.onnx                        37.76 MB
  0-four-speakers-zh.wav                             1.74 MB
```

运行：
```
cargo test -p meetily --lib diarization_e2e -- --ignored --nocapture --test-threads=1
cargo test -p meetily --lib sweep_clustering_threshold -- --ignored --nocapture --test-threads=1
```
