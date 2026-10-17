# 43 · UI / UX 重设计

## 设计定位
Meetily = **本地优先的 AI 会议助手**。设计原则：
1. **一个主动作**：任何页面都能一键开始录音（红色只用于录音）。
2. **安静可信**：石板灰中性色 + 单一品牌蓝；紫色专属 AI 纪要；隐私状态可见。
3. **待办驱动**：首页直接告诉用户"哪些会议还没出纪要/没转写"。
4. **全中文**：本轮把主流程残留英文全部本地化（中英双语）。

## 设计系统（globals.css / tailwind）
- 令牌：`--primary` 品牌蓝 221 83% 53%、`--record` 录音红、slate 中性色、圆角 10px。
- 组件类：`surface-card`（卡片）、`surface-card-hover`、`nav-item` / `rail-item`（导航）、`chip`（状态标签）、`eyebrow`（分组标题）。
- 动效：`animate-rec-pulse`（录音中脉冲）、`animate-ring-breathe`（录音按钮呼吸光环），遵守 `prefers-reduced-motion`。
- 语义色：转写=蓝、纪要=紫、待处理=琥珀、本地隐私=绿、云端=天蓝。

## 页面
| 页面 | 改动 |
|---|---|
| 侧边栏 | 展开态：品牌区（Logo+标语+收起按钮）→ 开始录音主按钮 + 导入图标 → 首页/录音库导航 → 会议列表（搜索、计数、紧凑两行条目、转写/纪要圆点、悬停/键盘聚焦显示操作）→ 底部设置/关于/版本。收起态：图标导航条 + 当前页指示条 + 底部展开按钮。去掉了压在内容上的浮动折叠按钮。 |
| 首页（空闲） | 新增仪表盘：问候语+日期；录音主卡片（录音设备、系统声音、识别引擎、语言一览 + 96px 呼吸录音按钮）；快捷入口（导入音频/录音库/模型与模板）；最近会议（状态标签、待生成纪要提示）；本周概览（会议数、时长、待办跳转）；隐私卡片（本地/云端自动切换文案）。 |
| 首页（录音中） | 顶部固定栏：录音状态+计时胶囊、段数、会议标题、识别语言/复制；聆听中的波形空状态；底部控制栏改为半透明悬浮条。 |
| 会议详情 | 新增会议标题栏：返回、可原地重命名的标题、日期/时长/段数、纪要状态、笔记与导出主按钮。转写栏改为"标题+计数 / 图标工具"，不再溢出；纪要栏左标题右操作，生成按钮紫色；纪要空状态说明"要点/决策/待办"；补充背景输入框加标签。 |
| 录音库 | 统一页头；4 个图标统计卡；目录与刷新合并为一行；搜索/筛选/排序/批量操作放入吸顶卡片；筛选改为深色胶囊；条目改为卡片+状态色图标。 |
| 设置 | 统一页头；顶部标签改为左侧竖向导航 + 右侧内容区（带当前分组说明）；去掉重复标题；卡片样式统一；存储位置、通知、录音保存位置等全部中文。 |

## 新增/修改文件
- 新增：`components/home/HomeDashboard.tsx`、`lib/home-overview.ts`（+测试）、`components/layout/PageHeader.tsx`、`components/MeetingDetails/MeetingHeader.tsx`
- 主要修改：Sidebar、Logo、Info、page.tsx、TranscriptPanel(首页/详情)、RecordingStatusBar、VirtualizedTranscriptView、SummaryPanel、Summary*ButtonGroup、EmptyStateSummary、MeetingDetailsSplitView、RecordingsLibrary、settings/page、RecordingSettings、StorageLocationsPanel、PreferenceSettings、RecordingControls、RecordingLevelMonitor、recordings.ts（日期随界面语言）、i18n.ts（约 150 个新键，中英齐全）

## 验证
- `tsc --noEmit` 0 错误；`bun test` 228 项全部通过（新增首页概览 4 项）。
- `next build` 静态导出构建通过。
- 用 Playwright + Tauri IPC 桩逐页截图核对：首页（展开侧栏）、录音中、会议详情、录音库、设置（通用/录音/转写）。

## 未覆盖 / 后续
- 深色模式未适配（令牌已预留 `.dark`）。
- 引导页（onboarding）、模板管理、重新转写对话框等次级弹窗仍是旧样式，部分文案仍为英文。
- 未在真机 macOS/Windows 窗口（含红绿灯区域）中走查。
