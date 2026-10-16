# 说话人分离：接入进展与算法订正

## 一、本轮最重要的产出：**我上一轮的算法设计是错的**

上一轮我基于对 pyannote 的**假设**写了 `diarization.rs`，猜测"分段 → 每段一个嵌入 → 聚类"。

下载官方模型包后，包里带着**参考实现**（`speaker-diarization-onnx.py`），我才看到真实算法——**结构完全不同**：

| 我假设的 | 实际的 |
|---|---|
| 模型输出每个说话人的标签 | 输出是 **powerset（幂集）**：每个"活跃说话人的组合"是一个类别 |
| 滑窗步长 1.0 秒 | `window_shift_ratio = 0.1`（窗口的 1/10） |
| 最短区域 0.5 秒 | `min_duration_on = 0.3` 秒（且嵌入阶段跳过 <10 帧 ≈ 0.2 秒的片段） |
| 聚类单位 = 语音区域 | 聚类单位 = **(chunk, speaker) 对** |
| 无需模型元数据 | 需要 7 个元数据字段（`window_size`、`receptive_field_shift` 等） |
| 直接取 argmax 得标签 | argmax → **powerset 映射** → 多标签 |

**powerset 是关键**：分类器不是分别输出"说话人0"和"说话人1"，而是为**每种活跃组合**输出一个类别——所以"两人同时说话"是**一个类别**，而不是两个独立标签。类别 0 是空集（静音），然后是单人，再是两人组合。

例：3 个本地说话人 → `1 + 3 + 3 = 7` 个类别。

## 二、订正后的实现（`audio/diarization.rs`，21 测试）

按真实算法重写，保留可测的纯逻辑部分：

| 函数 | 作用 |
|---|---|
| `SegmentationMeta` | 从 ONNX 元数据读窗口/感受野/类别数，**不硬编码**（不同导出不一样，用错会让时间轴整体拉伸或偏移） |
| `powerset_mapping` | 构建 类别→说话人集合 映射；类别数不够时**报错而不是产出不可达的说话人** |
| `decode_frame` / `decode_chunk` | argmax + 幂集解码；平局取低类别号保证确定性 |
| `merge_adjacent` | 同一说话人、间隔 ≤ `min_duration_off` 的片段合并，**循环到稳定**（合并后可能让第三个进入范围） |
| `drop_short_turns` | 丢弃 < `min_duration_on` 的片段 |

**帧时间要加半个感受野偏移**（`frame_offset_seconds`）——帧是以感受野为中心的，漏掉会让**每个说话人时间提前**。有测试锁住。

## 三、模型已定位并可下载（之前是不可达的）

| 模型 | 大小 | 说明 |
|---|---|---|
| `sherpa-onnx-pyannote-segmentation-3-0.tar.bz2` | **6.64 MB** | 分割（含 `model.onnx` 5.72 MB + `model.int8.onnx` 1.47 MB）**已下载** |
| `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx` | 待下载 | 说话人嵌入 |

**嵌入模型是中文专用**（`zh-cn`），与这个 app 的中文方向一致。

上一轮我记录"未验证 HuggingFace 直连不通"——本轮确认：**经代理 `127.0.0.1:10808`，HuggingFace 与 hf-mirror 都返回 200**，GitHub Releases 下载 6.64 MB 只用 6.8 秒。

## 四、又被自己的测试抓出一个错误

`powerset_mapping_row_count_matches_the_classifier` 失败：我写了 `(classes=4, speakers=3)`，但 4 个类别**只能编码 2 个说话人**（`1 + 2 + 1 = 4`），3 个需要 7 个。

代码的校验是对的，**我的测试数据是错的**。顺带把这个约束变成了显式测试——类别数不足必须报错。

## 五、验证

| 模块 | 结果 |
|---|---|
| `audio::diarization` | **21 pass** |
| `audio::speaker_cluster` | **17 pass** |
| `audio::speaker_turns` | **18 pass** |
| `audio::onnx_model` | **10 pass** |
| `summary::cleanup` | **23 pass** |
| `dictionary` / `transcript_text` | **17 / 17 pass** |
| `summary::chinese` / `processor` | **10 / 37 pass** |
| `audio::formats` / `recordings` | **22 / 28 pass** |
| `cargo check -p meetily` | 0 error |
| **合计** | **210 pass** |

## 六、还没做

| 项 | 状态 |
|---|---|
| `DiarizationModels` 的 ONNX 实现 | **未做**。分割的滑窗 + 批次推理、嵌入模型的输入形状仍需对照真实张量 |
| 嵌入模型下载（`eres2net` 中文） | **未做** |
| 滑动窗口与批次组装 | **未做**。属于 `diarization.rs` 之外的一层 |
| 接到流水线、写入 `transcript.speaker` | **未做** |
| 转写视图按说话人分组 | **未做** |

**准确说法：算法层已按真实参考实现订正并测试，模型已定位并下载了分割模型，但还跑不出说话人标签。**

## 七、下一步的具体形状

1. 用 `onnx_model.rs` 加载 `model.onnx`，**先打印真实的输入/输出张量形状与元数据**，对照 `speaker-diarization-onnx.py` 确认
2. 实现滑窗（`as_strided` 的等价物）+ 批次 32
3. 下载 `eres2net` 中文嵌入模型，确认其输入长度
4. 端到端跑 `0-four-speakers-zh.wav`（官方四说话人中文样本）验证能分出 4 人

第 4 步是关键：**有一个已知答案的中文测试音频**，可以先证明算法对，再接进真实录音。
