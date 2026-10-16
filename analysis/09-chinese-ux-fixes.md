# 中文用户体验修复记录

> 目标：解决 `08-new-requirements-analysis.md` 中确认的三项中文问题。
> 基线：仓库 HEAD `a2cb62e8` + 前两轮的功能实现（工作树未提交）。

---

## ① Parakeet 不支持中文却作为默认/推荐引擎

**问题**：Parakeet（NeMo TDT 0.6B）是英文专用模型，却是默认引擎且 UI 标为 "Recommended"。中文用户按推荐录音后得到不可用输出，且无从判断原因。前一轮已让它**在设置页报错**，但那是"阻止"而非"解决"。

**做法**：新增 `catalog::select_provider_for_language(configured, language, is_ready)`——纯函数，返回能处理该语言的 provider：

1. 配置的 provider 若支持该语言 → 保持不变（尊重用户选择）；
2. 否则选目录中第一个**可用且支持该语言**的 provider（目录顺序使多语言请求自然落到 Whisper）；
3. 都不行 → `None`（不假装有解）。

`is_ready` 回调让调用方能排除"本机模型未下载"的 provider，避免"修好了语言问题却换到跑不起来的引擎"。

**接线**（`worker.rs`）：录音任务启动时按当前语言偏好计算 provider，若与配置不同则通过 `provider_override` 参数**仅对本次会话生效**（不改写用户保存的设置），并发 `transcription-provider-switched` 事件。

**告知用户**（`RecordingControls.tsx`）：监听该事件并弹出 toast，说明"X 不支持该语言，本次改用 Y"，8 秒时长。**静默换引擎是更糟的选择**——用户会只察觉"转写结果和平时不一样"却找不到原因。

`engine.rs` 新增 `get_or_init_transcription_engine_for(app, provider_override)` 与 `configured_provider_id(app)`（后者避免调用方重复实现 config 读取与默认值逻辑）。

**测试**：`catalog.rs` 新增 9 例，覆盖：英文+Parakeet 不切换、中文/日文/韩文/德文/阿拉伯文/zh-tw 均切到 Whisper、替代引擎未就绪时返回 `None`（不假装有解）、优先保留配置的 provider、忽略未就绪的配置 provider、**绝不返回未实现的 provider**（云）、无语言提示时不切换、全都不就绪时返回 `None`。

**已知边界**：`is_ready` 在 worker 中当前传 `|_| true`，即"可用且实现"即可，模型是否已下载交给引擎初始化自带的校验（会给出可操作的错误）。这是个刻意的取舍：真正的就绪检查需要跨模块查询各引擎的模型状态，成本高于收益；而失败路径本来就是清晰的。

---

## ② token 估算对中文低估 2–3 倍

**问题**（`summary/processor.rs`）：`rough_token_count` 用全局 `0.35 tokens/char`——英文经验值。中文一个字约 1 token，因此被低估约 3 倍。

更严重的是 `chunk_text` 把该常数**取倒数**当"每 token 字符数"（`1.0/0.35 ≈ 2.857`）来切块，于是同一个 token 预算对中文会塞进 2–3 倍内容，长中文会议直接超出模型上下文。

**做法**：按字符集分类估算（`token_weight`）：

| 字符类别 | 权重 |
|---|---|
| CJK 汉字（含扩展 A–F）、假名、谚文 | **1.0** |
| 空白 | 0.0 |
| ASCII 字母数字 | 0.25 |
| ASCII 标点 | 0.5 |
| 其他非 ASCII（带音标拉丁、西里尔、阿拉伯…） | 0.5 |
| 星平面表情符号 | 1.0 |

`chunk_text` 重写为**按权重累计**决定窗口边界，不再依赖单一字符/Token 比率；重叠量也按片段自身的脚本构成换算（`overlap_chars_for`），使"重叠 N token"对中英文是等量的**内容**。

顺带修掉旧实现的字符/字节索引换算：原代码用 `chars[..start_char].iter().map(len_utf8).sum()` 反复求和（O(n²)），改为一次 `char_indices()` 收集。

**测试**：`summary::processor` 37 例（新增 8 例、改写 2 例）。新增覆盖：
- 中文按 ~1 token/字；
- 英文显著低于 1 token/字；
- 空白不计数；
- 内容增多则估算不减（单调性）；
- 中英混排估算落在纯中文与纯英文之间；
- 同一 token 预算下中文需要**更多**分块（这正是修复的实际后果）；
- 分块在无重叠时拼接可**完整还原原文**（旧实现的字符比率切法会在边界丢字）；
- 多字节字符切片不破坏 UTF-8。

**改写的 2 例说明**：`chunk_text_keeps_unicode_boundaries`、`chunk_text_progresses_when_overlap_matches_window` 原先断言的是**旧的字符比率语义**（如 `chunk_text("abcd",1,1) == ["abc","bcd"]`）。新语义下 4 个拉丁字符仅值 1 token，故整段为一个 chunk——这是刻意变更，已改写为断言新语义下的正确行为（完整覆盖、不丢内容），而非放松断言。

---

## ③ 字体栈不含 CJK 字形

**问题**：`tailwind.config.js` 的 `fontFamily.sans` 只有 `var(--font-source-sans-3)`，而 Source Sans 3 **不含 CJK 字形**。中文回退到系统逐字选择，导致中英混排字重与行高不一致。

**做法**：补齐跨平台 CJK 回退链，保持 Latin 仍由 Source Sans 3 渲染：

```
var(--font-source-sans-3), -apple-system, BlinkMacSystemFont,
"Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei",
"Noto Sans CJK SC", "Source Han Sans SC", "WenQuanYi Micro Hei",
"Hiragino Sans GB", sans-serif
```

**验证**：`font-sans` 已应用于 `<body>`（`layout.tsx:235`）；全仓 CSS 无竞争性 `font-family` 声明；用 node 解析 Tailwind 配置确认最终值正确合并（Tailwind 的 `extend` 是替换数组而非追加，故必须写全，不能只写新增项）。

---

## 验证汇总

| 项目 | 结果 |
|---|---|
| `cargo check -p meetily` | **Finished，0 error** |
| `catalog` 测试 | **20 pass / 0 fail**（新增 9） |
| `summary::processor` 测试 | **37 pass / 0 fail**（新增 8、改写 2） |
| 前端 `tsc --noEmit` | **exit 0** |
| 前端 `bun test` | **65 pass / 0 fail** |

## 未做

- **未实机走查**：三项均已验证到代码与单测层，但未在打包后的应用里用中文语音实际录音验证端到端效果（尤其 ① 的自动切换与 toast、② 的摘要质量）。
- **未做 i18n**：界面文案仍全英文。这是独立工程（需引入框架并抽取全部字符串），不在本次三项范围内。
