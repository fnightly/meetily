# 44 · 功能优化（三批合并交付）

目标：把 Meetily 从“录音 + 纪要”补齐为“可检索、可追踪、可迁移”的会议工作台，并针对中文投资场景提升纪要质量。

## 一、交付清单

| # | 功能 | 用户可见变化 | 关键实现 |
|---|------|-------------|---------|
| 1 | 说话人命名 | 转写中点“说话人 N”即可命名（如“张总”），最近用过的名字一键复用；名字同步到复制、导出、纪要生成 | 表 `meeting_speakers`；`workspace/speakers.rs`；`lib/speaker-names.tsx`（Context）；`SpeakerRenameDialog`；纪要输入改为 `[mm:ss] 张总: …` |
| 2 | 全文搜索 | 侧栏搜索覆盖标题/标签/纪要/笔记/转写，按会议分组展示命中片段并高亮；点转写命中直接跳到该秒并闪烁定位 | FTS5 trigram 外部内容表 `transcripts_fts` + 触发器；<3 字回退 LIKE；`api_search_meetings`；详情页支持 `?t=秒&q=词` |
| 3 | 本地识别词典 | 自定义词典同时作用于 Whisper 识别提示（initial prompt，≤160 字）与识别后纠错（Whisper / Parakeet / 导入 / 重转写） | `dictionary.rs` 活动词典镜像 + `apply_active_corrections` |
| 4 | 中文模板 + 推荐 | 新增 5 个中文模板：周会、客户访谈、招聘面试、投决会、项目路演/尽调；按标题与开场转写自动推荐并在列表标“推荐”；内置英文模板名本地化 | `templates/zh_*.json`；`lib/template-recommend.ts`；`useTemplates.recommendFor` |
| 5 | 云端长会议分块并行 | 云模型也按上下文窗口（60%）自动分块，3 路并发；本地模型保持串行 | `summary/processor.rs`：`cloud_token_threshold`、`chunk_concurrency_for`、`join_all` |
| 6 | 直接生成目标语言 | 选择目标语言时一次生成，不再“先英文再翻译”；无需归一化时跳过英文归一化轮 | `build_final_report_system_prompt_in(…, target_language)`；`needs_english_normalization` |
| 7 | 会议问答 | 单场（详情页“问答”页签）与跨会议（侧栏“问答”）提问；回答带 [S1] 引用，点引用跳到原话时间点 | `workspace/ask.rs`：关键词检索转写+纪要 → 用已配置的纪要模型回答；`AskPanel` |
| 8 | 标签 / 项目 | 会议头部可设项目与多个标签；侧栏筛选条按标签/项目过滤 | 表 `meeting_tags`、`meeting_projects`；`MeetingLabels` |
| 9 | 备份 / 恢复 | 设置 →“数据与备份”：一键打包 ZIP（数据库、词典、笔记，可选录音）；恢复后提示一键重启生效 | `workspace/backup.rs`：`VACUUM INTO` 一致快照；恢复先落 `restore-pending`，启动时原子替换；录音解压到 `restored-<时间>/` 并改写路径 |
| 10 | 待办追踪 | 纪要生成后自动提取待办（表格/列表，识别负责人与截止）；详情页“待办”页签、全局“待办”页、首页“未完成待办”卡片；逾期标红 | 表 `action_items`、`action_item_imports`；`lib/action-items.ts`；`ActionItemsPanel`；旧纪要首次启动时回填 |

清理：删除 `recording_commands.rs.backup`；`psychatric_session.json` 更正为 `psychiatric_session.json`（旧 id 仍可读取）；自动纪要开关改存应用 Store（保留 localStorage 兜底）；模板相关 toast、转写“加载更多”等文案本地化。

## 二、验证

- Rust：`cargo check` 通过；`cargo test --lib` 734 项全过（含新增 workspace/search/ask/backup/labels/action_items/dictionary/templates/分块与直接生成测试）。
- 前端：`tsc --noEmit` 0 错误；`bun test` 242 项全过（新增待办解析、模板推荐、说话人命名、复制使用命名）；`next build` 通过；中英文 i18n 键 0 缺失。
- 截图（模拟后端数据）：`analysis/v4-*.png`——详情页（命名/定位高亮/标签/推荐模板）、待办页签、问答页签、命名对话框、全局待办、全局问答、侧栏搜索、备份设置。

## 三、已知限制

1. 未在真实 macOS / Windows 包上端到端运行；截图基于模拟数据。
2. Parakeet 不支持提示词，词典只做识别后纠错；Whisper 提示词有长度上限，词条多时取前 160 字。
3. 问答是关键词检索 + LLM，非向量语义检索；同义改写的问题召回可能不全。问答会把相关片段发给已配置的纪要模型（界面已提示）。
4. 恢复备份会替换当前会议库，需要重启生效；建议恢复前先备份。
5. 待办提取依赖纪要中有“待办/行动项/Next Steps”等小节，自由文本中的承诺不会被识别。
6. 云端分块并发为 3，触发上游限流时会按原有重试逻辑退避。
