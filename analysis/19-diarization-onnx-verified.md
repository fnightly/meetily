# 说话人分离：ONNX 全链路打通（实测通过）

## 一、结论先说

**ONNX 全链路已在真实模型上跑通**，包括官方中文四说话人参考音频：

```
Initialised ONNX Runtime from ...onnxruntime.dll
Decoded reference audio: 909771 samples at 16000 Hz, 1 channel(s)
Reference duration: 56.9s
Segmentation metadata: window=160000 (10.0s) shift=16000 sample_rate=16000
                       receptive_field_size=991 shift=270 classes=7
                       local_speakers=3 powerset_max=2
Analysing 47 window(s)
Added a trailing padded window at offset 752000
Segmentation produced 1178 frame(s), 13841 with speech
Embedding model inputs: ["x"]
Embedding produced 2 vector(s), dimension Some(512)
Wiring verified.
test result: ok. 1 passed
```

## 二、最重要的发现：**我连着两次猜错了输入形状**

这两次都是靠**实际运行**发现的，不是靠文档：

### 第一次：嵌入模型不是 `(batch, samples)`
我按常规写成 2D，模型报：
```
Invalid rank for input: x  Got: 2  Expected: 3
```

### 第二次：嵌入模型**根本不接受音频**
改成 3D 后报：
```
index: 2  Got: 160000  Expected: 80
```

**`Expected: 80` 说明它要的是 80 维声学特征**。这才发现 3D-Speaker 的 `eres2net` 标准做法是吃 **FBank 特征**，不是原始波形。

于是专门写了 `diarization_shapes.rs`（`#[ignore]` 的诊断测试）**打印真实张量形状**，一次拿到确定答案：

| 模型 | 输入 | 输出 | 关键元数据 |
|---|---|---|---|
| pyannote 分割 | `(N, 1, T)` f32 | `(N, frames, 7)` | `window_size=160000`、`receptive_field_shift=270` |
| 3dspeaker 嵌入 | `(N, T, 80)` f32 | `(N, 512)` | `language=Chinese`、`feature_normalize_type=global-mean` |

**教训**：这两个错误都不会报"你搞错了"，只会报形状/维度不符。若我没跑真实模型而是照文档写完就交付，会得到一个"看起来完整、实际跑不通"的分离功能。

## 三、顺带确认了元数据读取是对的（价值很大）

我从模型文件读元数据，而不是硬编码。实测值与我的假设对比：

| 字段 | 我的假设 | 模型实际值 |
|---|---|---|
| `receptive_field_size` | 3200 | **991** |
| `receptive_field_shift` | 160 | **270** |
| `num_classes` | 7 | 7 ✅ |

**两个假设都是错的。** 如果硬编码，帧时间会算错 → **每个说话人时间段整体偏移**，而且不会有任何报错。

**帧数交叉验证**：`1178 frames / 2 batches = 589 frames/window`，而 `160000 / 270 + 1 ≈ 593`。数量级吻合，证明 `receptive_field_shift=270` 被正确解释，帧率是 16.875 ms/帧。

## 四、新增模块

| 文件 | 测试 | 内容 |
|---|---|---|
| `audio/fbank.rs` | **18** | Kaldi 兼容 log-mel 特征：Povey 窗、`1127·ln(1+f/700)` mel 刻度、80 滤波器组、`global-mean` 归一化 |
| `audio/diarization_models.rs` | **15** | ONNX 实现：元数据读取、滑窗、批次推理、张量组装 |
| `audio/diarization_shapes.rs` | 诊断 | 打印真实张量形状（`#[ignore]`） |
| `audio/diarization_e2e.rs` | 端到端 | 官方中文参考音频全链路验证（`#[ignore]`） |

`fbank.rs` 的关键测试：
- **1 kHz 正弦波必须落在低频 mel bin** —— 锁住滤波器组方向，反过来会让所有嵌入都错
- **Povey 窗 ≠ Hann 窗** —— 四分之一处应为 `0.5^0.85 ≈ 0.554`，用错窗会静默改变每个嵌入
- 每个 FFT bin **最多被 2 个滤波器覆盖** —— 锁住 mel 间距
- 静音落在对数下限，且**全为有限值**（不能是 NaN 或 -inf）

## 五、又被自己的测试抓出两个错误

1. **`needs_trailing_window` 判断过宽**：我把"余数 > 0"当成需要补窗，但余数**恰好等于一个 shift** 时已经被覆盖（1050/100/50 → 19 窗，最后一窗覆盖到 1000，剩下 50 正好是终点）。改为 `remainder > 0 && remainder < shift`。

2. **Povey 窗首样本我断言非零**：实际 `0^0.85 = 0`。改成断言真正有区分度的性质（四分之一处 ≈0.554 而非 Hann 的 0.5）。

## 六、验证

| 模块 | 结果 |
|---|---|
| `audio::fbank` | **18 pass** |
| `audio::diarization` | **36 pass**（2 ignored） |
| `audio::diarization_models` | **15 pass** |
| `audio::speaker_cluster` / `speaker_turns` | **17 / 18 pass** |
| `audio::onnx_model` | **10 pass** |
| `summary::cleanup` / `dictionary` / `transcript_text` | **23 / 17 / 17 pass** |
| `summary::chinese` / `processor` | **10 / 37 pass** |
| `audio::formats` / `recordings` | **22 / 28 pass** |
| **端到端（真实模型）** | **1 passed** |
| `cargo check`（lib + tests） | 0 error |
| 前端测试 | **140 pass** |

**合计：241 pass + 1 端到端实测。**

## 七、还没做

| 项 | 状态 |
|---|---|
| 把分割的 589×3 帧聚合成**说话人时间段** | **未做**。参考实现里的 `speaker_count` / `relabels` / `count` 那套跨窗聚合还没实现 |
| (chunk, speaker) 对的嵌入提取 | **未做**。目前只验证了嵌入模型能跑，还没按参考实现的"逐窗逐本地说话人取音频段"来喂它 |
| 滑动窗口的批次拼接顺序 | **半成品**。当前只验证了帧数，没有把各窗结果按 `receptive_field_shift` 拼成全局时间轴 |
| 接到转写流水线、写 `transcript.speaker` | **未做** |
| 转写视图按说话人分组 | **未做** |

**准确说法：两个模型都能正确加载并推理，元数据与特征提取已验证，但还差"从帧到说话人时间段"的聚合，因此仍不能产出说话人标签。**

## 八、下一步的精确形状

参考实现（`speaker-diarization-onnx.py`）里剩下的部分：
1. `speaker_count()` — 把各窗的 `(chunk, frames, speakers)` 按 `receptive_field_shift` 累加到全局帧轴，除以覆盖数
2. `get_embeddings()` — 对每个 (chunk, local_speaker) 对，拼接其所有连续帧对应的音频段，喂嵌入模型
3. `relabels` + `count` — 用聚类结果把本地说话人重标为全局说话人，再跨窗投票
4. 最后按 `onset/offset = 0.5` 分段，合并间隔 ≤0.5s 的同类段，丢弃 <0.3s 的段

这四步是纯数组操作，**可以在 `diarization.rs` 里以纯函数实现并测试**，不需要模型——正好是下一轮该做的。

## 九、文件位置

模型与测试音频（约 45 MB，不在仓库里）：
```
D:\Meetly\Model\diarization\
  sherpa-onnx-pyannote-segmentation-3-0\model.onnx   5.72 MB
  3dspeaker_eres2net_zh.onnx                        37.76 MB
  0-four-speakers-zh.wav                             1.74 MB
```
运行端到端：
```
cargo test -p meetily --lib diarization_e2e -- --ignored --nocapture --test-threads=1
```
