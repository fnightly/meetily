# 故障记录：Transcription model not ready

## 现象

录音被拒，提示：

> **Transcription model not ready** — Please download a transcription model before recording.

来源：`src/hooks/useRecordingStart.ts:139`（`handleRecordingStart` 的就绪检查）。

## 诊断过程

### 第一层：前端就绪检查

`useRecordingStart.ts:82-98` 的逻辑是：读取配置里的 provider → 映射到该 provider 的命令 → `initialize` 后调 `hasAvailableModels`。

`getProviderCommands()`（`lib/transcription-model-readiness.ts:11-22`）只映射 `localWhisper` 与 `parakeet`，两者都存在，故不是映射缺失。

### 第二层：真实配置

从本机数据库读出实际配置（`sqlite3`）：

```
transcript_settings: provider = localWhisper, model = base
```

不是 `parakeet`，所以"Parakeet 未下载"不是原因。

### 第三层：模型到底在哪

| 目录 | 内容 |
|---|---|
| `%APPDATA%\com.meetily.ai\models`（默认位置） | `ggml-base.bin` **141 MB**、`summary/Qwen3.5-4B-Q4_K_M.gguf` **2.6 GB** |
| `D:\Meetly\Model`（`storage_config.json` 里配置的） | **空**（只有一个空的 `parakeet/` 子目录） |

`transcript_settings` 要求的正是 `base` → `ggml-base.bin`。**模型在，只是应用没在那个目录里找。**

## 根因

这是我在实现功能 ③（可配置存储位置）时引入的 UX 缺陷，**不是模型缺失**：

1. 用户在设置里把 `models_folder` 改成了 `D:\Meetly\Model`；
2. 我当时的 UI 提示只有一行小字"Changing the models location does not move models that were already downloaded"；
3. 该提示不足以阻止误操作，而**后果是录音被完全阻断**，且报错信息（"请下载模型"）把用户指向了错误的方向——用户会反复重新下载，或以为模型损坏。

`storage/commands.rs` 的 `set_storage_config` 对**数据库**做了迁移（`VACUUM INTO`），但对**模型**没有——因为模型动辄数 GB，自动搬迁代价高、且用户可能有意分盘存放。所以"不自动搬"是合理的，**问题在于没有兜底与明确告知**。

## 修复

### 1. 目录解析加兜底（`storage/config.rs`）

`StorageConfig::models_folder()` 现在的解析顺序：

1. 未配置 → 默认目录；
2. 配置目录**含模型** → 用配置目录；
3. 配置目录无模型，但默认目录**有模型** → **回退到默认目录**，并打 `warn` 日志；
4. 两处都没有模型（全新安装）→ 仍用配置目录，让新下载落在用户指定的位置。

新增 `dir_contains_models()`：递归（深度上限 4）查找 `.bin` / `.gguf` / `.onnx`。之所以递归而非检查固定文件名：三种引擎的布局不同——Whisper 是根目录 `ggml-*.bin`，Parakeet 是每模型一个子目录，内置摘要模型是 `summary/*.gguf`。

**这是安全网，不是搬迁方案的替代**：一旦用户在配置目录里下载了任何模型，配置目录即生效。

### 2. UI 明确告知（`StorageLocationsPanel.tsx`）

`StorageSettingsView` 新增 `effective_models_folder` 与 `models_folder_fallback`。当回退发生时，模型那一行显示醒目提示：

> This folder has no models, so Meetily is loading them from `C:\Users\...\models`. Move your models into the folder above (or re-download them) to use it.

用户从此能看到**实际生效的目录**，而不是面对一个"看起来被忽略"的路径。

### 3. 测试（`storage/config.rs`，7 例）

覆盖 Whisper 扁平布局、`summary/` 下的 gguf、`parakeet/` 下的 onnx、空目录与"有无关文件但无模型"（**本次故障的真实形态**）、路径不存在、扩展名大小写不敏感、深度上限。

## 验证

| 项目 | 结果 |
|---|---|
| `cargo check -p meetily` | **Finished，0 error** |
| `storage::config` 测试 | **7 pass / 0 fail** |
| `catalog` 测试 | **20 pass / 0 fail** |
| 前端 `tsc --noEmit` | **exit 0** |

**对真实路径的核验**（用与 `dir_contains_models` 相同的规则）：

```
配置目录 D:\Meetly\Model                 -> 无模型
默认目录 %APPDATA%\com.meetily.ai\models -> 有模型 (2 个, 例: ggml-base.bin)
=> models_folder() 回退到默认目录
```

**模型本身的有效性也已确认**：`WHISPER_MODEL_CATALOG`（`config.rs:21`）声明 `("base", "ggml-base.bin", 142, ...)`，磁盘实际 141 MB，满足发现逻辑的 `>= 期望值 × 0.9` 容差，且魔数校验会另行执行——即回退后该模型会被判为 `Available`，录音应恢复正常。

## 用户的三个选择（现在都能工作）

1. **什么都不做**：回退生效，录音恢复（推荐，因为模型已在默认位置）；
2. **把模型移过去**：把 `%APPDATA%\com.meetily.ai\models` 下的内容移到 `D:\Meetly\Model`（含 `summary/` 子目录），配置目录随即生效；
3. **重置为默认**：在设置面板点模型那一行的 Reset。

## 遗留说明

- 本次修复只处理"配置目录为空"这一种错配。若用户**两个目录都有模型**，以配置目录为准（符合预期）。
- 未处理"模型下载中"与回退的交互：下载目标始终是 `models_folder()` 的返回值，因此回退期间新下载会落进默认目录而非用户配置的目录。这在"配置目录为空且默认目录有模型"时是合理行为，但若用户本意是换盘，需要先手工搬迁——UI 提示已说明这一点。
