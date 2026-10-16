# 第 6 轮：四位说话人端到端测试抓到一个「报 4 显示 3」的缺陷

## 一、为什么非要做这个测试

你的三个会议里，**只有一个同时有音频和带时间的转写，而且是 6.7 秒的独白** —— 正确结果就是"1 位说话人"。

**这意味着「多位说话人」这条路径从来没有在你的数据上被验证过。** 算法单元测试用的是合成 turn，每个说话人都至少拥有一段；而真实的归属过程可能会漏掉某个人。

所以我用参考音频（`0-four-speakers-zh.wav`，上游标注为 4 位中文说话人）搭了一个真实的转写行集合，走**完整的标注路径**。

## 二、第一次运行就失败了

```
Separation: 4 speaker(s), 12 turn(s)
Transcript rows: 28
Labelled 28 of 28 row(s)
Distinct labels: ["speaker_1", "speaker_2", "speaker_3"]     ← 只有 3 个！
assertion failed: expected 4 distinct labels, got [...]
```

**分离找到了 4 位说话人，但转写里只有 3 个标签。**

加上每个说话人的明细后，原因很清楚：

```
speaker_1: 3 turn(s), 11.4s
speaker_2: 2 turn(s),  7.5s
speaker_3: 6 turn(s), 16.7s
speaker_4: 1 turn(s),  0.3s     ← 总共 0.3 秒
```

**说话人 4 只有 0.3 秒语音**，永远不可能在"重叠最多者胜出"里赢过任何一段 2 秒的转写行。

## 三、这算 bug 吗？两件事要分开看

**归属算法本身没有错。** 一个只有 0.3 秒语音的说话人赢得不了一段话。强行把某段分给他，是把真实语音标错人 —— 那更糟。

**真正的 bug 是界面上的自相矛盾**：

- 命令返回的 `speaker_count` 用的是**分离找到的簇数**（4）
- 于是 toast 说「识别出 **4** 位说话人」
- 而转写里只有 `speaker_1..3`

**用户会看到"4 位"，然后在正文里找不到第 4 个人。**

## 四、修复：报告「真正写进转写的说话人」

`apply_speaker_labels` 原来只返回一个计数，现在返回：

```rust
pub(crate) struct LabelOutcome {
    pub labelled: usize,
    /// 至少拥有一段转写的说话人（按序）
    pub speakers: std::collections::BTreeSet<u32>,
}
```

命令改用 `speakers.len()` 作为 `speaker_count` —— **用户看到什么，就报什么**。

如果分离找到的簇多于实际标注的，记一条日志：

```
2 speaker(s) found but only 1 could be matched to a segment
```

**这是"报告事实"而不是"掩盖差异"** —— 差异仍然可见，只是不再对用户做出转写无法兑现的承诺。

## 五、修正后的测试

```
Separation: 4 speaker(s), 12 turn(s)
Distinct labels: ["speaker_1", "speaker_2", "speaker_3"]
Note: separation found 4 speakers but only 3 own a segment; the reported count matches the transcript.
test result: ok. 1 passed
```

新的断言是**用户可见的不变量**，而不是我原先那个过强的假设：

| 断言 | 保护的东西 |
|---|---|
| `labelled_speakers == distinct.len()` | **报告的数量必须等于写进数据库的标签数** |
| 标签是从 1 开始连续的 | 界面渲染不会出现编号空缺 |
| `distinct.len() > 1` | 四人对话不能被归成一个说话人 |
| 没有未标注的段落 | 对话中间不能出现空洞 |
| 全部 28 段都被标注 | 每段都该归到某人 |

## 六、顺带：三个路径设置都查了一遍

上一轮发现 `recordings_folder` 只用于显示，这轮把同一张面板的另外三个也查了：

| 设置 | 谁读 | 结论 |
|---|---|---|
| `models_folder` | `resolve_models_dir`（分离）、whisper 引擎 | ✅ 一致 |
| `notes_folder` | `notes_commands.rs:93`（同一解析器） | ✅ 一致 |
| `database_folder` | DB 管理器（日志确认路径匹配） | ✅ 一致 |
| `recordings_folder` | 原来只有显示 → **上一轮已修** | ✅ 现在一致 |

## 七、验证

| 项目 | 结果 |
|---|---|
| **四位说话人端到端标注** | ✅ **1 passed**（56.9s 音频，4 人，28 段） |
| 真实录音（独白）端到端 | ✅ 1 passed |
| 完整命令路径（真实 DB） | ✅ 1 passed |
| Rust 合计 | **456 pass，0 failed** |
| 前端 `tsc` / `bun test` | exit 0 / **140 pass** |

## 八、这一轮的教训

**单元测试测的是我想得到的输入。** 我给 `apply_speaker_labels` 写的 9 条测试全都用"每个说话人都至少有一段"的合成数据 —— 于是**全都通过**，而真实数据下会漏掉说话人。

真正暴露问题的是**用真实音频、真实分布去跑完整链路**。

（另外一个教训：我这个文件连续第三次把测试插错位置。`diarization_e2e.rs` **没有 `mod tests` 包裹**，必须**追加到文件末尾**，不能用"最后一个 `}` 之前"当锚点。这次我在插入前先检查了文件结构。）

## 九、还剩什么

| 项 | 状态 |
|---|---|
| **UI 目视确认** | **未做** —— 需要你点一次 |
| 说话人 4 这类「有语音但无段落」的情况 | 已如实报告，未强行归属（这是有意为之） |
