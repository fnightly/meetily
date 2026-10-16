# 端到端验证轮 3：完整命令路径在真实数据上跑通

## 一、决定性证据

新增的 `the_command_path_labels_a_real_meeting` 测试跑的是**你的真实数据库 + 真实录音**（在数据库副本上操作，不会改动你的数据）：

```
Working on a copy of the database
Meeting: meeting-a307297d-2c07-40fd-91cf-e749838e2151
Folder:  C:\Users\Administrator\Music\meetily-recordings\Meeting 15_10_26_21_12_43_2026-10-15_13-12
Decoded 6.7s from ...\audio.mp3
Timed transcript rows: 1
Separation: 1 speaker(s), 1 turn(s)
Labelled 1 of 1 row(s)
Rows with a stored label: 1
  transcript-76f7e2a7-... -> Some("speaker_1")
The full command path works against a real meeting.
```

**这是不点按钮能做到的最强验证** —— 它走的是命令**同一条路径**：读同一些行、解码同一个文件、跑同一次分离、调用同一个写库函数。唯一缺的是 Tauri `AppHandle`。

覆盖到的环节：

| 环节 | 验证方式 |
|---|---|
| 从数据库找出有时间的转写行 | 真实 DB |
| 用 `folder_path` 定位音频 | 真实目录 |
| 解码真实 MP3（48k→16k 单声道） | 真实文件 |
| 分离（1 位说话人，独白正确） | 真实推理 |
| 归属 + 写库 | 真实 UPDATE |
| 读回 `speaker_1` | 真实 SELECT |
| 标签形式符合文档约定 | 断言 |

## 二、本轮新增的 DTO 序列化测试（5 条）

这一轮补上了链路最后一层的**契约测试**：

| 测试 | 保护的东西 |
|---|---|
| `a_speaker_label_serialises_for_the_interface` | serde 真的会输出 `speaker` 字段 |
| `an_absent_label_is_omitted_rather_than_null` | 未分离时不产生多余字段 |
| `json_without_a_speaker_still_deserialises` | 旧数据/缓存仍可解析 |
| `a_speaker_label_survives_a_round_trip` | 往返一致 |
| `the_other_timing_fields_are_still_serialised` | 时间字段没被意外丢掉 |

第一条尤其重要：**上一轮发现的问题正是"字段存在但序列化不出去"**，而这类问题在数据库层面完全看不出来。

## 三、顺手修掉的

- `diarization_e2e.rs` 一处未使用导入
- 把 `TranscriptRow` / `apply_speaker_labels` 提为 `pub(crate)`，让端到端测试能调用**真实的**写库函数（而不是复制一份逻辑）
- 磁盘：D: 5.68 → 9 GB（清 `incremental` 4.14 GB + 旧测试 pdb）

## 四、验证

| 项目 | 结果 |
|---|---|
| **完整命令路径（真实 DB + 真实音频）** | ✅ **1 passed** |
| DTO 序列化 | **5 pass** |
| `audio::diarization_commands` | **22 pass** |
| 分离全部模块 | **128 pass** |
| `retranscription` / `speaker_cluster` / `speaker_turns` | **20 / 24 / 18 pass** |
| `fbank` / `onnx_model` | **18 / 10 pass** |
| `summary::cleanup` / `dictionary` / `transcript_text` | **23 / 17 / 17 pass** |
| `summary::chinese` / `processor` | **10 / 37 pass** |
| `formats` / `recordings` | **22 / 28 pass** |
| 前端 `tsc` / `bun test` | exit 0 / **140 pass** |

## 五、还差最后一步：界面上的目视确认

代码层、数据层、算法层现在**全部有证据**。剩下唯一没验证的是**渲染**：

- 说话人标签是否真的画出来
- 是否只在说话人变化时显示
- 设置页的模型区块长什么样

**这三件事我无法自己验证** —— 需要有人在真实界面里看一眼。

另外两个已知的、非阻塞的改进项：
1. 长会议无进度反馈（14 分钟会议约需 10 分钟，只有转圈）
2. 设置里未暴露阈值 / 说话人上限

## 六、关于 D: 磁盘的提醒

`target/debug` 稳定在 ~17 GB（`app_lib.lib` 2 GB + 各种 pdb）。D: 只有 100 GB 且系统占用大，反复逼近上限。

**建议**（需要你决定，我没擅自改）：
- 把 `CARGO_TARGET_DIR` 指到 E:（46 GB 可用），或
- 在 `Cargo.toml` 里给 dev profile 设 `debug = "line-tables-only"`，能把 pdb 从 ~1.2 GB 压到很小（代价是调试栈信息变少）
