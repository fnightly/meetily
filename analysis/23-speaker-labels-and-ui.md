# 说话人分离：标签迁移修复 + 设置界面

## 一、修掉了重新转写清空标签的数据丢失

上一轮发现：`retranscription.rs` 用 **DELETE + INSERT**，会把分离写入的说话人标签全部清空。

**修法：按时间重叠迁移**（而不是按位置），复用已有的 `speaker_turns::attribute_segments`（18 个测试）。

流程变成：

```
读取旧标签（speaker IS NOT NULL）
  → DELETE + INSERT 新转写（带 speaker 列）
  → 用新旧时间段重叠把旧说话人映射到新片段
```

**为什么按重叠而不是按序号**：重新转写产生的分段边界不同（VAD 结果会变），序号完全不可靠。重叠匹配是唯一稳健的做法。

### 关键函数：`parse_speaker_label`

```rust
fn parse_speaker_label(label: &str) -> Option<u32> {
    label.strip_prefix("speaker_")?.parse::<u32>().ok()
        .map(|n| n.saturating_sub(1))
}
```

**解析失败必须返回 `None` 而不是默认值** —— 否则一个无法识别的标签会被**静默当作 speaker 1**，把整场会议重新归到错误的人名下。

5 条测试覆盖：`speaker_1`→0、`speaker_12`→11、大小写敏感、无前缀、负数、空串、`speaker_0` 不下溢。

`audio::retranscription` 测试 **15 → 20**。

## 二、设置界面

新增 `SpeakerSeparationSettings.tsx`：

| 元素 | 说明 |
|---|---|
| 模型状态 | ✅ 已就绪 / ⚠️ 尚未下载 |
| 下载按钮 | 显示所需体积；**下载中禁用** |
| 磁盘不足 | 显示后端的具体信息（后端在写任何东西前就拒绝） |
| 模型目录 | 显示路径 + 一键打开 |
| 重新下载 | 已就绪时也提供，便于替换损坏文件 |

**放在「转写」标签页**，因为分离是**给转写加标注**，不是产生转写。

**「对某个会议运行分离」没放在设置里** —— 它属于它要标注的那份转写旁边，不属于设置。

### 新增命令

`open_diarization_models_folder` —— 会先创建目录（首次下载前目录不存在，直接打开会失败）。

## 三、i18n

19 个新键，中英双语。`Chinese covers every English key` 测试通过——**中文是默认语言，缺一个键就意味着大多数用户看到英文**。

## 四、过程中反复踩的坑（记录一下）

这一轮我**又踩了三次**同一类问题：**多行字符串替换因行尾符不同而静默失败**。

- `insert-command.ps1`：PowerShell `$lines[$i..]` 语法错误（缺 range 上界）
- `add-label-helper.ps1`：3 个锚点只命中 1 个（缩进/行尾不符）
- `fix-retranscribe-labels.ps1`：主锚点完全没命中

**教训已经很清楚**：涉及多行文本的编辑，**不要用脚本拼字符串**，要么用 `edit` 工具，要么**按行号操作**（`ReadAllLines` → 改索引 → `WriteAllLines`）。后者更稳，因为不依赖行尾符。后半段我改用了行号方式，一次就成。

## 五、验证

| 模块 | 结果 |
|---|---|
| `audio::retranscription` | **20 pass**（含标签迁移） |
| `audio::diarization_catalog` / `_service` / `_commands` | **16 / 9 / 6 pass** |
| `audio::diarization_models` / `speaker_cluster` / `speaker_turns` | **15 / 24 / 18 pass** |
| `audio::fbank` / `onnx_model` | **18 / 10 pass** |
| `summary::cleanup` / `dictionary` / `transcript_text` | **23 / 17 / 17 pass** |
| `summary::chinese` / `processor` | **10 / 37 pass** |
| `formats` / `recordings` | **22 / 28 pass** |
| **合计** | **290 pass** |
| `cargo check`（lib + tests） | 0 error |
| 前端 `tsc` / bun / build | exit 0 / **140 pass** / ✓ 9/9 |

## 六、还没做

| 项 | 状态 |
|---|---|
| **转写面板按说话人分组** | **未做** —— 这是用户能看到的最终效果 |
| 在会议详情里放「识别说话人」按钮 | **未做**（`diarize_meeting` 命令已就绪，只缺调用入口） |
| 录音停止后自动分离 | **未做**（当前必须显式触发，符合"默认关闭"取向） |
| 设置里暴露阈值 / 说话人上限 | **未做** |
| 未实机验证 | 模型下载命令、标签迁移、设置界面**都没在真实应用里跑过** |

**注意最后一条**：命令层和界面都完成了，但整条链路（下载 → 运行 → 标签显示 → 重新转写保留）**尚未端到端实测**。这是下一步最该做的事。
