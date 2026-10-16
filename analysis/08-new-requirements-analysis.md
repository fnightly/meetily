# 新需求可行性分析：中文体验 / Windows 按应用录音 / Notion 同步

> 基于仓库 HEAD `a2cb62e8`，与 `00-OVERVIEW.md`、`07-phase1-implementation.md` 同一分析基线。
> 外部 API 结论均附来源链接；内部结论均附 `path:line`。

---

## 一、中文用户体验：**有 4 个真实障碍，其中 1 个是"默认就把中文会议毁掉"**

这不是"缺个语言包"的问题。中文用户在当前版本会遇到一条**从默认配置就开始出错**的链路。

### 1.1 【中→需修正早期判断】默认语言在前后端不一致，存在启动竞态

初读 `lib.rs` 时我判断"默认 `auto-translate` 会把中文会议翻成英文"。**逐层核对后这个结论过重，需要收窄**，但其底层仍是一个真实缺陷。

事实链：

- 后端 `lib.rs:125-127` 的静态默认值是 `"auto-translate"`（注释即写 "default to ... translation to English"）。
- 但**前端默认值是 `'auto'`**：`ConfigContext.tsx:143-149`
  ```ts
  const saved = localStorage.getItem('primaryLanguage');
  return saved || 'auto';      // 首次启动 → 'auto'（只检测、不翻译）
  ```
- 且前端在**启动时会把这个值推给 Rust**：`ConfigContext.tsx:219-220`
  ```ts
  if (selectedLanguage) { invoke('set_language_preference', { language: selectedLanguage }) }
  ```

**收窄后的结论**：正常运行下，前端会立刻把 `'auto'` 写入后端，所以**实际默认是"自动检测、不翻译"，而非"翻译成英文"**。我原先的表述不准确。

**但真实缺陷有两个**：

1. **启动竞态窗口**：`LANGUAGE_PREFERENCE` 是**纯内存静态量，没有任何持久化**（`lib.rs:126`，无 store 写入；全仓仅 `lib.rs:429` 一个写入口，来自 IPC）。因此每次启动后端都从 `"auto-translate"` 开始，直到前端那个 `useEffect` 跑完才被覆盖。**若用户在这几秒内开始录音，首段音频会以 `translate = true` 进入 Whisper**，产出英文——对中文用户就是首段内容被翻译。
2. **默认值本身写错了**：既然前端权威默认是 `'auto'`，后端静态默认就**不该是** `"auto-translate"`。两个默认值不一致是缺陷根源。正确做法是把后端默认也改成 `"auto"`（或更好：按系统语言初始化），使两者一致，竞态窗口即使存在也不会产生翻译。

所以这里仍然值得改，但**理由是"消除前后端默认值不一致 + 竞态"，而不是"默认就翻译"**。改动依旧很小（后端常量），且顺带消除一个跨语言边界的状态同步缺陷。

**关于 `"auto-translate"` 本身**：它是**有意保留的合法选项**（`whisper_engine.rs:568-576`），但 UI 上应明确标注"翻译成英文"，避免中文用户误选后以为是普通转写。


### 1.2 【严重】推荐的默认引擎 Parakeet 完全不支持中文

- `TranscriptSettings.tsx:124` 把 Parakeet 标为 **"Recommended - Real-time / Accurate"**，且它是默认引擎。
- `parakeet_provider.rs:26-31` 明确：语言参数不被支持，只记一条 warn 日志。
- 型号是 NeMo Parakeet TDT 0.6B，**英文专用**。

**后果**：中文用户按 UI 推荐选 Parakeet，得到的是英文模型硬转中文音频的结果——通常是乱码级输出。

**建议**：
1. 在中/日/韩等非英语语言被选中时，**自动切到 localWhisper**，或在 UI 上直接禁用 Parakeet 并说明原因；
2. "Recommended" 标签需要按语言条件化——对中文用户推荐一个不支持的引擎是误导。

### 1.3 【中】token 估算对中文低估约 3 倍，长会议分块会出错

`summary/processor.rs:255-259`：

```rust
pub fn rough_token_count(s: &str) -> usize {
    let char_count = s.chars().count();
    (char_count as f64 * 0.35).ceil() as usize
}
```

`0.35 tokens/char` 是**英文**经验值。中文字符大致 **1 token ≈ 1 个汉字**（BPE 对 CJK 常在 0.6–1.0 tokens/char）。

**后果**：中文转录的实际 token 数是估算值的 **2–3 倍**。叠加 `07-phase1-implementation.md` 已记录的 `token_threshold - 300` 下溢与分块前进量问题，**中文长会议的摘要在内置/Ollama 路径上失败或质量骤降**。

**建议**：按字符集分类估算（CJK 区间用 ~1.0，拉丁用 ~0.35，并按比例混合），而不是全局单一系数。

### 1.4 【中】界面全英文，且没有 i18n 框架；字体不含中文字形

- **无 i18n**：`frontend/package.json` 中 `i18n`/`intl`/`locale`/`translat` **零命中**；全部 UI 文案硬编码英文字面量。
- **字体**：`layout.tsx:4,33` 只加载 `Source_Sans_3`，`tailwind.config.js` 的 `fontFamily.sans` 只有 `var(--font-source-sans-3)`——**Source Sans 3 不含 CJK 字形**。中文会回退到系统字体，导致中英文混排的字重/行高不一致，观感直接降级。

**建议**：先做**字体栈**（低成本、立即见效）：`-apple-system, "Segoe UI", "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", sans-serif` 作为 fallback。i18n 框架（next-intl 等）是独立工程，建议单独排期。

### 1.5 已有但未被利用的中文能力

- **摘要语言已支持中文**：`lib/summary-languages.ts` 含 `zh`/`zh-tw` 等 30+ 语言，与 `processor.rs` 的 `language_name_from_code` 对应。
- **Whisper 支持中文**：`LanguageSelection.tsx:12` 有 ISO 639-1 清单，显式选择 `zh` 即可正常中文转写。

**结论**：中文能力"零件齐全但默认配置错误"。**最重要的不是开发新功能，而是改默认值 + 改引擎推荐逻辑。**

---

## 二、Windows 按特定软件录音：**可行，但有一个硬版本门槛**

### 2.1 先说一个更基础的结论：当前 Windows 系统音频采集**根本没实现**

`audio/capture/system.rs:99-103`：

```rust
#[cfg(not(target_os = "macos"))]
{
    // For non-macOS platforms, you would implement WASAPI/ALSA loopback here
    anyhow::bail!("System audio capture not yet implemented for this platform")
}
```

非 macOS **直接报错返回**。这与 `07-phase1-implementation.md` 中"Windows 系统音频结构上不可用"一致——而"按应用录音"是建立在这之上的，**必须先把整机 loopback 做出来**。

### 2.2 整机 loopback：升级 cpal 到 0.16 即可，无须自研

调研到的权威佐证：`tauri-plugin-system-audio` 的 `loopback.rs` 源码注释写明——

> cpal 0.16 has built-in loopback support on Windows: opening an input stream on a render (output) device transparently sets the WASAPI flag `AUDCLNT_STREAMFLAGS_LOOPBACK`. See `cpal-0.16.0/src/host/wasapi/device.rs:561` ... **No cpal feature flag is required.**

这正好解释了现有代码为什么坏：项目锁定 **cpal 0.15.3**，且 `Cargo.toml` 里那个"提供 WASAPI loopback 支持"的 git 补丁因写在成员 manifest 而从未生效（已在 Phase 1 修正到工作区根）。

**建议**：把 cpal 升到 **0.16**（loopback 原生支持），并**移除 cpal git 补丁**——升级比维护一个未固定的 git rev 更稳。然后 `system.rs` 的 Windows 分支照该插件的方式实现（默认输出设备 + `build_input_stream` + 重采样到 16 kHz 单声道）。

**这条路能解决"录到电脑里的声音"（会议、网课、视频），但录的是整机混音，不是"只录某个软件"。**

### 2.3 按应用录音：需要 Windows 11（Build 20348+）的 Process Loopback API

关键 API 与门槛（[Microsoft Learn: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS](https://learn.microsoft.com/en-us/windows/win32/api/audioclientactivationparams/ns-audioclientactivationparams-audioclient_process_loopback_params)）：

```
typedef struct AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
  DWORD                 TargetProcessId;
  PROCESS_LOOPBACK_MODE ProcessLoopbackMode;
} AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS;
```

| 要求项 | 值 |
|---|---|
| **最低系统** | **Windows 10 Build 20348** |
| 头文件 | `audioclientactivationparams.h` |
| 激活方式 | `ActivateAudioInterfaceAsync` + `VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` |

**必须准确传达的限制**：Build 20348 实际对应 **Windows Server 2022 与 Windows 11 21H2+**。普通 Windows 10（最高 19045）**不支持**。所以这个功能**只能面向 Windows 11 用户**，且需优雅降级到整机 loopback + 提示。

能力上它比"按进程"更强：`TargetProcessId` 指定进程名后，**该进程及其子进程**的渲染流都会被包含或排除（`Include`/`Exclude` 两种模式），足以覆盖"只录腾讯会议/钉钉/Teams"这类诉求。

**Rust 实现路径**：需要 `ActivateAudioInterfaceAsync` 的 COM 回调 + `IAudioClient` 手工接线。`windows` crate 目前**未自动生成**这部分绑定（见 [microsoft/windows-rs#2995](https://github.com/microsoft/windows-rs/issues/2995) "Win32 Application Specific Audio Loopback"），需要手工声明 FFI 结构体与接口。属于**中等规模的原生代码**（估计 400–700 行 unsafe Rust + 平台分支），且与现有 cpal 麦克风路径需要一个混音/选择层。

**结论**：可行、有官方文档与可参考实现（OBS 的 [win-capture-audio](https://github.com/eoNaho/win-capture-audio) 走同一 API），但**不是小改动**，且用户覆盖受 Windows 11 限制。

---

## 三、同步 Notion：**完全可行，是三者中最标准的一项**

Notion REST API 能力充足（API 版本 `2025-09-03`，官方 [block types](https://developers.notion.com/reference/block) 与 [page create](https://developers.notion.com/reference/post-page) 语义）：

### 3.1 建议的机制

1. **认证**：Notion internal integration token（`Authorization: Bearer`，必需 `Notion-Version` 头）。用户需在 Notion 侧把目标页面/数据库**共享给该 integration**，否则一律 404。
2. **配置项**（存入现有 `settings` 表，与其它 API Key 同机制）：
   - `notionToken`
   - `notionParentType`（`page_id` / `data_source_id`）
   - `notionParentId`
3. **同步内容**：会议标题 → 页面标题；摘要 → `heading_2` + 段落/列表块；转写 → 折叠块或分段段落。
4. **幂等**：在 `meetings` 表加 `notion_page_id`，重复同步走 `PATCH /v1/pages/{id}` 更新而非重复建页。

### 3.2 必须处理的三个现实约束（否则一定踩坑）

| 约束 | 数值 | 影响 |
|---|---|---|
| **速率限制** | 平均 **3 req/s**（429 + `Retry-After`） | 逐条转写同步会被限流，需批量+指数退避 |
| **单请求 blocks** | **100 个/请求** | 长会议必须分页追加（`PATCH /blocks/{id}/children`） |
| **rich_text 单块** | **2000 字符** | 长段落必须切分，否则 400 |

**推论**：长会议的转写**不能一次性同步**，需要 100 块/2000 字符的分片器 + 退避重试。这一层是本功能的主要工程量，而非 API 调用本身。

### 3.3 与既有代码的契合度

现有 `summary/llm_client.rs` 已有 reqwest 客户端与 provider 模式可复用；导出侧我在 Phase 1 写的 `lib/meeting-export.ts` 已经能把会议渲染成 markdown，**可作为"markdown → Notion blocks"转换的输入**，不必重写渲染逻辑。

---

## 四、建议优先级

按"投入产出比"排序，而非按你列出的顺序：

| 优先级 | 事项 | 工作量 | 理由 |
|---|---|---|---|
| **P0** | Parakeet 对中文禁用/自动切换 + 修正 "Recommended" 标签 | 小 | 推荐引擎**完全不支持中文**，这是主动误导，且用户无法自行判断 |
| **P0** | 后端语言默认值 `auto-translate` → `auto`（与前端一致） | **~1 行** | 消除前后端默认值不一致与启动竞态；竞态下中文首段会被翻译 |
| **P1** | `rough_token_count` 按字符集分类 | 小 | 决定中文摘要**能否可靠工作**（当前低估 2–3 倍） |
| **P1** | CJK 字体栈 | 小 | 中英混排观感立即改善，零架构改动 |
| **P1** | Notion 同步 | 中 | 标准 API，机制清晰，主要成本在分片+退避 |
| **P2** | cpal 0.15.3 → 0.16，实现 Windows 整机 loopback | 中 | 补上"Windows 根本没有系统音频"这个洞 |
| **P2** | i18n 框架 + 中文界面 | 大 | 全量文案抽取，建议独立排期 |
| **P3** | Windows 11 按应用录音（Process Loopback） | 中大 | 需手工 FFI；仅覆盖 Win11；依赖 P2 先落地 |

**关键判断**：Parakeet 的中文问题与 token 估算问题是**"功能对中文是否成立"的问题**，而不是体验优化；Notion 与按应用录音是**新增能力**。因此若目标是"让中文用户能用"，应先把前四项（均为小时级）做完，再谈录音与同步。


---

## 五、待确认项（需你决定）

1. **默认语言改成 `auto` 是否符合产品意图？** 若上游有意以英文为默认输出，则应改为在**首次启动按系统语言**设定（中文系统 → `zh`），而不是全局 `auto`。
2. **按应用录音的目标系统**：只支持 Windows 11（Build 20348+），还是要求兼容 Windows 10（则只能做整机 loopback）？
3. **Notion 同步的形态**：每次会议手动"同步到 Notion"，还是自动（摘要完成后自动创建页面）？后者需要处理失败重试与用户授权边界。
