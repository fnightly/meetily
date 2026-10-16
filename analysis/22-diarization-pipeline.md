# 说话人分离：接入流水线（模型获取 + 运行命令）

## 一、本轮完成

| 模块 | 测试 | 内容 |
|---|---|---|
| `audio/diarization_service.rs` | **9** | 端到端流水线编排（分割→嵌入→聚类→重标→时间段） |
| `audio/diarization_commands.rs` | **6** | 新增 `diarize_meeting`：解码 → 分离 → 归属 → 写 `speaker` |
| `audio/diarization_catalog.rs` | **16** | 模型清单、URL、完整性判定 |
| 合计（分离相关） | **143** | |

**注册了 4 个命令**：
- `get_diarization_model_status`
- `download_diarization_models`
- `get_diarization_models_folder`
- `diarize_meeting`

## 二、模型来源：改用直链，**零新依赖**

分割模型上游**只以 `.tar.bz2` 发布**。照搬就要引入 `tar` + `bzip2` 两个依赖。

我查到 HuggingFace 上有**解压后的直链**，并做了逐字节验证：

```
HF 下载     : 5992913 bytes
本地(包解压) : 5992913 bytes
SHA256 一致 : True
```

嵌入模型同样验证：

```
HF 下载     : 39593761 bytes
目录声明值   : 39593761 bytes
SHA256 一致 : True
```

两个模型**都在 HuggingFace**，所以现有的 `hf-mirror.com` 镜像覆盖方式直接适用。

## 三、发现并修掉一个**静默丢数据**的问题

写好了 `speaker` 列，我顺手检查了读取路径，发现：

```
meeting.rs:78   SELECT * FROM transcripts WHERE meeting_id = ?
models.rs:26    pub struct Transcript { id, meeting_id, transcript, timestamp,
                                        summary, action_items, key_points,
                                        audio_start_time, audio_end_time, duration }
```

**`Transcript` 结构体没有 `speaker` 字段**，而 sqlx 按列名映射 —— 所以 `SELECT *` 拿到的 `speaker` 列**被静默丢弃**，前端永远看不到。

**分离会把标签写进数据库，然后前端什么都看不到，且没有任何报错。**

已修复：Rust 结构体与 TS 类型都加上 `speaker`。

（这类问题的特征是**没有任何报错**，只有"功能不生效"。如果不是顺手检查读取路径，这个 bug 会一直存在到用户报告"分离没反应"。）

## 四、几个刻意的设计决定

### 1. 服务是**同步**的，用 `spawn_blocking` 调用

每个阶段都是 CPU 密集的 ONNX 或数组运算，可能跑几分钟。做成 `async` 只会把它拉到 async 运行时上**堵住其它任务**。调用方（`diarize_meeting`）用 `spawn_blocking` 包住，与 `retranscription` 的做法一致。

### 2. 分割没找到语音就**不加载嵌入模型**

先用不需要嵌入的 `turns_from_chunks` 做一次基线判断：

```rust
let frame_turns = turns_from_chunks(...);
if frame_turns.is_empty() {
    // 静音录音：直接返回，不加载 38 MB 模型
    return Ok(empty_outcome);
}
```

静音录音会省下 38 MB 模型加载和全部嵌入计算。

### 3. 只写 `speaker` 列

`UPDATE transcripts SET speaker = ? WHERE id = ?` —— 文本、时间戳、id 全不动。所以：

- 分离**可以重复运行**，不会破坏其它数据
- **不需要重新转写**音频

### 4. 没有时间信息的片段**跳过，不猜**

归属需要 `audio_start_time` / `audio_end_time`。缺这两个字段的片段无法定位到时间轴，**保持原状**而不是随便给个标签。

### 5. 标签存为 `speaker_1`

语言中性、可排序，UI 想怎么显示都行。

### 6. 标签前先检查模型是否就绪

模型缺失时直接返回**缺失摘要**（告诉用户要下载什么），而不是让 ONNX 抛一个看不懂的解析错误。

## 五、测试

| 模块 | 结果 |
|---|---|
| 分离相关（4 模块） | **143 pass** |
| 全量 touched 模块 | **270 pass** |
| `cargo check`（lib + tests） | 0 error |
| 前端 `tsc` / bun | exit 0 / **140 pass** |
| **端到端（已知 4 人参考音频）** | **恰好 4 人** ✅ |

## 六、⚠️ 一个需要指出的交互缺陷（尚未修）

`retranscription.rs:437` 用的是 **DELETE + INSERT**：

```rust
sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
// 然后 INSERT（不带 speaker 列）
```

**所以在分离之后重新转写，会把所有说话人标签清空。**

这个交互目前没有任何保护。可能的修法：
1. 重新转写时保留 `speaker`（按时间重叠迁移）
2. 或者重新转写后提示"说话人标签已清除，需要重新分离"

我倾向第 1 种，但需要判断迁移的正确性（转写分段可能变化）。

## 七、还没做

| 项 | 状态 |
|---|---|
| 前端 UI：触发按钮、按说话人分组、设置开关 | **未做** |
| 设置里暴露阈值 / 说话人上限 | **未做** |
| 模型下载的 UI 入口 | **未做**（命令已就绪） |
| 录音停止后自动分离 | **未做**（当前是显式命令，符合"默认关闭"的取向） |
| 重新转写清空标签 | **已发现，未修** |

## 八、下一步

前端是最后一块：设置页加「说话人分离」区块（模型状态 + 下载按钮 + 开关），转写面板按说话人分组显示。命令层已经全部就绪。
