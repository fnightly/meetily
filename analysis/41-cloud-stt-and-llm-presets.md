# 41 · 云端转写全链路 + 国内大模型预设 + 设置页重做

## 目标
让 Meetily 成为「录音 → 转写 → 纪要」一站式工具，本地与云端模型均可用于转写和总结。

## 1. 云端语音转写（原为 NotImplemented → 已实现）
- 新增 `src-tauri/src/audio/transcription/cloud_provider.rs`：`CloudSpeechProvider`
  - OpenAI 兼容 `/audio/transcriptions` 接口，PCM16 16kHz WAV multipart 上传
  - 支持：**硅基流动 SiliconFlow**（SenseVoiceSmall / TeleSpeechASR，国内直连）、**Groq**（whisper-large-v3-turbo / v3）、**OpenAI**（gpt-4o-mini-transcribe / gpt-4o-transcribe / whisper-1）
  - 429/5xx 自动重试 3 次（退避），90s 超时；HTTP 错误转成可操作提示（Key 无效、额度不足、模型不存在等）
  - 自动清理 SenseVoice `<|zh|><|NEUTRAL|>` 等标签；语言代码归一为 ISO；词典术语作为 `prompt` 提升专有名词识别（≤400 字符，OpenAI/Groq）
  - 新命令 `api_test_speech_provider`：设置页一键测试连接
- 打通三条链路：实时录音（`engine.rs`）、导入音频（`import.rs`）、重新转写（`retranscription.rs`）
- `catalog.rs`：语言自动回退**只在本地引擎间切换**，绝不会静默把音频发往云端
- 数据库新增 `siliconflowApiKey` 列（迁移 `20261017000000_add_siliconflow_transcript_key.sql`）

## 2. 纪要大模型：国内/海外一键预设
- 新增 `src/lib/llm-presets.ts`：DeepSeek、通义千问、Kimi、智谱 GLM、硅基流动、豆包/火山方舟、Gemini；本地 LM Studio / llama.cpp / vLLM
- 模型选择改为分组下拉：本地·数据不出本机 / 云端·国内 / 云端·海外 / 自定义
- 预设卡片：隐私提示（本地 vs 云端）、获取 API Key 链接、推荐模型 chips；切换厂商自动清空旧 Key，防止串用

## 3. 转写设置界面重做
- 本地/云端分组卡片，状态徽标（使用中 / 仅英文 / 已保存 Key / 即将支持）
- 云端面板：隐私提示、模型选择、API Key 显隐、获取 Key、测试连接、设为转写引擎
- 导入/重新转写对话框自动列出已配置的云端引擎（☁️）

## 4. 中文化
- 新增 `tr()`（React 外可用）+ 约 130 条中英文案；录音开始/停止、纪要生成、复制、侧边栏等核心流程 toast 全部中文化；纪要设置页标题/说明中文化

## 验证
- `cargo check --lib` 通过，无新增警告；`cargo test --lib audio::transcription` 34/34 通过（含 8 个云端转写单测）
- `tsc --noEmit` 通过；`bun test` 201/201 通过
- 浏览器渲染截图核验转写设置、纪要模型分组下拉

## 已知限制
- 云端转写未用真实 API Key 联调（字段依据各家官方文档）；Deepgram、ElevenLabs 仍标记为「即将支持」
- 自定义 OpenAI 兼容配置同一时间只保存一套（切换厂商需重新填 Key）
- 其它非核心页面仍有约 60 条英文 toast 待中文化
