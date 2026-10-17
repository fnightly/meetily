# 界面审查与合并方案

审查基于**实际渲染出来的界面**（用 `scripts/cdpshot.cjs` 把 Tauri IPC 打桩后在浏览器里渲染截图），不是只看代码。

---

## 一、发现的缺陷（按严重程度）

### 1. `PLACEHOLDER_SKIP` 直接渲染给用户 ✅ 已修

录音库页的头部原本长这样：

```
目录: D:\meetily-recordings    PLACEHOLDER_SKIP 3 · 40.1 MB of audio
```

源码：

```tsx
<span className="ml-2">
  PLACEHOLDER_SKIP
  {view.recordings.length === 1 ? '' : 's'} · {formatBytes(view.total_bytes)} of audio
</span>
```

**本该填录音数量，结果占位符没被替换就发布了。** 而且 `library.summary` 这个键**两种语言都早就存在**（`{count} 个录音 · 共 {size}`），只是没被用上。

已修，现在渲染为 `3 个录音 · 共 40.1 MB`。

### 2. 侧边栏**完全没有加载态和空状态**

`Sidebar/index.tsx` 里搜不到 `isLoading` / `Skeleton` / 空提示 —— `meetings` 是什么就直接渲染什么。首帧会是一片空白，用户无法区分「正在加载」和「一个会议都没有」。

对比：`RecordingsLibrary` 三态齐全（loading / empty / folder-missing）。**同一件事两个列表做法不一致。**

### 3. 文案中英混杂

| 类型 | 数量 | 例子 |
|---|---|---|
| 硬编码英文 placeholder | **33** | `placeholder="Select model"`、`"Enter your API key"` |
| 硬编码英文 toast | **12** | `toast.error("Meeting title cannot be empty")` |
| 录音库汇总行 | 1（已修） | `... of audio` |

集中处：`ModelSettingsModal`(11)、`TemplateManagerModal`(5)、`MeetingNotesContent`(4)、`TranscriptSettings`(3)。

**中文用户会看到英文提示**，而项目的默认语言就是中文。

### 4. 录音库页有两个标题、两段说明

```
录音文件                              ← 页面 h1
磁盘上保存的全部录音。可以逐个转写、生成纪要、重命名、移动或删除。

录音库                                ← 组件自己的标题
磁盘上保存的全部录音，最新的在前。包含没有对应会议记录的录音，所以不会有任何遗漏。
```

**同一个意思说两遍**，占掉屏幕顶部一大块。

### 5. 页面上有 `1 error` 开发浮层残留

Next 的开发错误浮层在有历史错误时会常驻左下角（我截图里能看到）。开发环境正常，但发布构建不该有 —— 这条只是提醒，**不是缺陷**。

---

## 二、合并方案（侧边栏合成一个列表）

### 数据

| 来源 | 命令 | 关键字段 |
|---|---|---|
| 会议 | `api_get_meetings` | `id`、`title`、`folder_path` |
| 录音 | `list_saved_recordings` | `folder_path`、`meeting_name`、`created_at`、`duration_seconds`、`audio_bytes`、`has_transcript`、`has_summary` |

**用 `folder_path` 精确匹配**作为去重键 —— 两边都有这个字段且语义一致。

> `api_get_meetings` 的 Rust 结构（`MeetingMetadata`）**本来就有 `folder_path`**，只是 TS 侧的 `CurrentMeeting` 把它丢了。所以**不需要改 Rust**，加宽 TS 类型即可。

### 三种条目

| 情况 | 渲染 | 点击 |
|---|---|---|
| 有会议 + 有录音 | **一行**（合并） | 打开会议详情 |
| 只有会议（录音被移走/删除） | 一行 | 打开会议详情 |
| 只有录音（未导入为会议） | 一行 | 打开录音库 |

### 行内信息（用户要的「信息密度」）

现在是**只有标题**。合并后每行应显示：

```
产品周会                                    ● 转写  ● 纪要
10月15日 21:12 · 30:50 · 17.5 MB
```

—— 日期、时长、体积来自 `RecordingEntry`；转写/纪要徽章来自 `has_transcript` / `has_summary`。

### 无障碍与键盘

- 列表用 `ul`/`li`，或给容器 `role="list"`
- 行是**可聚焦**的（`tabIndex` 或真 `button`），回车可打开
- 「打开笔记」这类次要动作**不能只靠 hover 出现** —— 键盘用户看不到
- 每个按钮有 `aria-label`（现在多数是 `title`，屏幕阅读器支持不一致）

---

## 三、优先级建议

| 优先级 | 事项 | 理由 |
|---|---|---|
| **P0** | `PLACEHOLDER_SKIP` | 已修 |
| **P0** | 列表合并 | 用户明确要求 |
| **P1** | 侧边栏加载态 + 空状态 | 首屏白屏，且与录音库不一致 |
| **P1** | 行内信息密度 | 用户明确要求；数据已经在手，只是没显示 |
| **P2** | 45 处硬编码英文 | 默认语言是中文，属于明显的不一致 |
| **P2** | 录音库页去掉重复标题 | 视觉噪音 |
| **P3** | 键盘/aria | 基础已可用，但次要动作仅 hover 可及 |

---

## 四、我**没有**做的判断

以下几处是设计取舍，我选了保守方案，如果与你的预期不符请指出：

1. **合并后「录音库」入口取消** —— 因为列表已经包含全部录音。但 `/recordings` 页面保留（仍可直达），所以功能没有丢失。
2. **只有录音、没有会议的条目点击后去录音库**，而不是自动创建会议 —— 自动创建会往数据库里写东西，不该由一次点击决定。
3. **没有自动把录音导入成会议** —— 同样是「不擅自写数据」。
