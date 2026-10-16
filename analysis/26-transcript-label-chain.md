# 端到端验证轮 2：找到并修掉「标签到不了界面」的完整断链

## 一、这一轮找到的问题比上一轮严重

上一轮我修了 `Transcript` 模型缺 `speaker` 字段。这一轮顺着**读取路径**往下查，发现标签要经过**四道关卡**，而我之前只修了一道：

| 层 | 位置 | 状态 |
|---|---|---|
| 1 | `Transcript` 模型（数据库读取） | 上一轮已修 |
| 2 | **`MeetingTranscript` DTO**（API 实际返回的类型） | **本轮修** |
| 3 | `meeting.rs:88` 的第二处 DTO 构造 | **本轮修**（编译器报错暴露） |
| 4 | `convertTranscriptsToSegments`（分页 hook） | **本轮修** |
| 5 | `app/_components/TranscriptPanel.tsx` 的另一份映射 | **本轮修** |

**如果不修第 2 条，前一轮的修复完全没有效果** —— 标签会正确写入数据库、正确读出、然后**被 DTO 丢掉**，界面上什么都看不到。

而且第 4 条还有一个更隐蔽的地方：会议详情页用的是 **`usePagination={true}`**，走的是 `usePaginatedTranscripts` 里的映射，**根本不会经过我在 `TranscriptPanel` 里改的那段**。

> 换句话说：**改对了源码，但改的不是实际执行的那条路径。**

## 二、我是怎么发现的

**不是靠猜，是靠顺着数据流往下读。**

改完 `TranscriptPanel` 的映射后，我检查了它的调用方式，发现：

```tsx
const convertedSegments = useMemo(() => {
    if (usePagination && segments) {
      return segments;      // ← 会议详情走这条
    }
    return transcripts.map(...)  // ← 我改的是这条
}, ...);
```

于是去找 `segments` 的来源 → `usePaginatedTranscripts` → 发现另一份映射。

再继续往下 → `api_get_meeting_transcripts` → 发现 `MeetingTranscript` DTO。

**每一层都"看起来对"，但组合起来标签到不了界面。**

## 三、第二处 DTO 构造是编译器找到的

加完 `MeetingTranscript.speaker` 字段后，`cargo check` 直接报：

```
error[E0063]: missing field `speaker` in initializer of
               `api::api::MeetingTranscript`
  --> database/repositories/meeting.rs:88:26
```

**这是 Rust 结构体字面量的好处**：加字段会让所有构造点编译失败，不可能漏掉。前端那边（TypeScript 的可选字段）就没这个保护 —— 所以我必须手工搜出全部映射。

## 四、完整的链路现在是通的

```
diarize_meeting 写入 transcripts.speaker
   ↓  Transcript 模型（有 speaker ✓）
   ↓  MeetingTranscript DTO（有 speaker ✓）
   ↓  convertTranscriptsToSegments（有 speaker ✓）
   ↓  TranscriptSegmentData.speaker（类型有 ✓）
   ↓  VirtualizedTranscriptView：说话人变化时渲染标签 ✓
```

## 五、验证

| 项目 | 结果 |
|---|---|
| `cargo check`（经 dev 会话重建） | ✅ Finished（1m 34s），应用重启运行 |
| 前端 `tsc` | exit 0 |
| 前端 `bun test` | **140 pass** |
| 客户端 bundle 含 `speakers.title` / `说话人分离` / `speakers.runNeedsTranscript` / `get_diarization_model_status` | ✅ |
| 会议详情 bundle 含 `diarize_meeting` / `handleIdentifySpeakers` / `identify_speakers` | ✅ |
| 数据库写库路径（22 条） | ✅ 上一轮已验证 |

**说明 `cargo check` 直接跑会报 `os error 32`** —— dev 会话持有构建锁。构建成功的证据来自 dev 会话自身的日志（`Finished dev profile in 1m 34s` + 应用重启）。

## 六、这一轮得到的教训

1. **"改对了"不等于"改对了地方"** —— 有分支的调用路径必须顺着实际执行的那条走一遍。
2. **Rust 的穷尽性检查是资产**：加字段会强制更新所有构造点。TS 的可选字段没有这个保护，只能靠搜索 + 人工判断。
3. **同一个概念在项目里有 5 处映射**（3 处前端、2 处后端）。这类重复是 bug 的温床，但本次不重构 —— 先保证行为正确。

## 七、还剩什么

| 项 | 状态 |
|---|---|
| **UI 交互实测**（用户点击 → 看标签） | **未做** —— 需要在界面里操作 |
| 长会议无进度反馈 | 未做 |
| 阈值 / 说话人上限设置 | 未做 |
