# 端到端验证轮：把命令的写库路径变成可测的

## 一、本轮做的事：让「写入」这半边可测试

`diarize_meeting` 之前把「归属 + 写库」内联在命令体里，**无法测试** —— 命令需要 Tauri `AppHandle`，测试拿不到。

抽出 `apply_speaker_labels(pool, rows, turns)`：

```rust
// 命令现在只剩：解码 → 分离 → 调用这个函数
let labelled = apply_speaker_labels(pool, &rows, &outcome.turns).await?;
```

**职责分清了**：产生时间段 与 把时间段变成数据库行，是两件事，只有后者需要数据库。

这样测试能用内存数据库直接跑真实的写库代码，而不是重新实现一遍逻辑（**重新实现逻辑的测试证明不了原代码对**）。

## 二、本轮抓到的真实缺陷：测试表结构与真实表不一致

第一版测试立刻失败了：

```
read back: ColumnNotFound("summary")
```

我从真实数据库读出建表语句对比，发现我的测试表**缺了三列**（`summary`、`action_items`、`key_points`）。

**这个失败本身很有价值**：如果测试表比真实表"简单"，它就**永远抓不到"模型读了表里没有的列"**这类问题。现在测试表用的是**真实 DDL 逐字复制**。

（顺带：这正是我上一轮修复的那个 bug 的同类 —— `Transcript` 模型与 `transcripts` 表之间的列不匹配。）

## 三、现在写库路径被验证覆盖的点

22 条测试，全部通过：

| 验证点 | 说明 |
|---|---|
| 标签写入并能通过 `Transcript` 模型读回 | 端到端往返 |
| 标签是**一基**的 | 泄漏零基索引会显示成"说话人 0" |
| 跨两个时间段的片段取**重叠较大**者 | 平局取低索引 |
| **无时间信息的行保持未标注** | 不默认给第一个说话人 |
| 无时间段时**不写任何东西** | 没有可分离内容时不得重标 |
| **只改传入的行** | 其它会议不受影响 |
| 重新标注是**覆盖**不是累积 | |
| 落在间隙的片段取**最近**说话人 | 不留空洞 |
| **转写文本不被改动** | 分离只加标注 |

## 四、真实录音上的分离（真实音频路径）

```
Recording folder: ...\Meeting 15_10_26_21_12_43_2026-10-15_13-12
Located audio:    ...\audio.mp3
Decoded 6.7s (106666 samples at 16 kHz mono) from 48000 Hz / 1 channel(s)
Result: 1 speaker(s), 1 turn(s)
  1.84s -- 6.70s  speaker 0
```

**独白录音正确得出 1 位说话人** —— 既没多也没少。`find_audio_file` 也确认能找到命令实际要用的文件。

## 五、修掉的可用性缺口

数据库里 **3 个会议只有 1 个可测试**：

| 会议 | 带时间转写 | 按钮 |
|---|---|---|
| Meeting 16_10_26_22_27_48 | 0 | 禁用 |
| **Meeting 15_10_26_21_12_43** | **1** | **可用** |
| Meeting 14_10_26_14_29_53 | 0 | 禁用 |

原因：用户开了**延后转写**，新录音没有转写段落。而分离是**给转写段落加标注**，没转写就无事可标。

原先的 tooltip 只说"运行较慢"，**没解释为什么禁用**。已改为：

> 「请先转写这个会议 —— 说话人标签是附加在转写段落上的。」

## 六、顺带确认与更正的

### 1. 更正我之前的错误建议

日志证明 **`tauri dev` 会自动重编译 Rust**：

```
Info File src-tauri\src\audio\diarization_commands.rs changed. Rebuilding application...
    Finished `dev` profile in 1m 16s
```

我之前等 150 秒没反应就说"必须 Ctrl+C 重启" —— **观察窗口太短**。实际 1–3 分钟。

### 2. 「录音保存位置」修复已在真实使用中生效

用户 22:27 的录音落在 `D:\meetily-recordings\...`（**配置的位置**），而不是默认的 `C:\Users\Administrator\Music\`。

### 3. 磁盘回收 2.8 GB

`target/debug/incremental` 是 4.06 GB 的纯编译缓存。D: 6.05 → 8.85 GB。

## 七、验证

| 项目 | 结果 |
|---|---|
| `audio::diarization_commands`（含数据库路径） | **22 pass** |
| `audio::diarization` 及其子模块 | **128 pass**（5 ignored） |
| `audio::speaker_cluster` / `speaker_turns` / `fbank` | **24 / 18 / 18 pass** |
| `audio::retranscription`（标签迁移） | **20 pass** |
| `summary::cleanup` / `dictionary` / `transcript_text` | **23 / 17 / 17 pass** |
| `summary::chinese` / `processor` | **10 / 37 pass** |
| `formats` / `recordings` | **22 / 28 pass** |
| `cargo check`（lib + tests） | 0 error |
| 前端 `tsc` / `bun test` / `build` | exit 0 / **140 pass** / ✓ 9/9 |
| 真实录音分离 | ✅ 独白→1 人 |

## 八、还剩什么

| 项 | 状态 |
|---|---|
| **UI 交互实测**（点击按钮 → 标签显示 → 分组） | **未做** —— 需要用户在运行的界面里操作 |
| 长会议无进度反馈 | 未做（14 分钟会议约需 10 分钟，只有转圈） |
| 设置里暴露阈值 / 说话人上限 | 未做 |
| 录音停止后自动分离 | 未做（当前需显式触发，符合"默认关闭"取向） |

**所有能在没有界面交互的情况下验证的路径，现在都验证过了**：模型加载、特征提取、分离算法、真实音频、音频查找、数据库读写。剩下的只有"点下去看到什么"。
