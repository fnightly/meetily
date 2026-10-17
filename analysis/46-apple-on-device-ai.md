# 46 · macOS / iOS 调用本机 AI 能力（技术方案）

> **约束说明**：本文是在 Windows 上写的。**没有 macOS 机器、没有 Xcode、没有 Apple Silicon 真机**，所以以下内容是基于官方文档的**架构方案**，不是已验证的实现。文末列出了必须在 Mac 上验证的清单。

---

## 一、「本机 AI 能力」在 Apple 平台上指什么

| 能力 | 适用系统 | 访问方式 |
|---|---|---|
| **Foundation Models 框架**（Apple Intelligence 端侧模型，约 3B） | macOS 26+ / iOS 26+，Apple Silicon | **Swift API** `FoundationModels` |
| Core ML 自定义模型 | macOS / iOS | Swift / ObjC |
| Vision / Speech 框架 | 全平台 | Swift / ObjC |

用户要的「本机 AI」= **Foundation Models**：不联网、不装 Ollama、系统自带，这正好契合约会纪要这种「隐私优先」场景。

---

## 二、为什么不能直接从 Rust 调

`FoundationModels` 是 **Swift-only** 框架，没有 C / ObjC 头文件，因此：

- Rust 无法直接 link 它
- 现有的 `llm_client.rs` 是纯 HTTP 客户端（Ollama / OpenAI 兼容 / Claude 等），**这条路走不通** —— 本机模型不监听端口

必须在中间加一层 Swift。

---

## 三、三种接入方式

### 方案 A：Swift sidecar 可执行文件（**仅 macOS**）

写一个小的 Swift 命令行程序，读 stdin、写 stdout，内部调 Foundation Models。Rust 侧像现在的 `sidecar.rs` 一样拉起它。

```
Rust ──stdin/stdout──> apple-fm-sidecar (Swift) ──> FoundationModels
```

| 优点 | 缺点 |
|---|---|
| 复用已有的 `sidecar.rs` 机制 | **iOS 不允许 fork 子进程**，只解决 macOS |
| Swift 代码与主程序解耦 | 多一个二进制要签名/公证 |
| 崩溃不拖垮主程序 | 进程启动开销（~50-200ms） |

### 方案 B：Tauri 移动端插件（**macOS + iOS 通用**）✅ 推荐

写一个 Tauri plugin，Swift 侧实现 `invoke` 处理，Rust 侧通过 `tauri::plugin` 调用。Tauri 2 的移动端插件机制天生支持 Swift。

```
前端 ──invoke('plugin:apple-ai|generate')──> Rust 插件壳 ──> Swift ──> FoundationModels
```

| 优点 | 缺点 |
|---|---|
| **macOS 和 iOS 一套代码** | 需要写插件壳（Rust + Swift 各一层） |
| 进程内调用，无启动开销 | Swift 侧要处理异步/取消 |
| 符合 Tauri 2 官方移动端路线 | 只能在 Mac 上构建 |

### 方案 C：App 内嵌本地 HTTP 服务

Swift 侧起一个 `127.0.0.1` 的 HTTP 服务，Rust 走现有 `llm_client.rs` 的 HTTP 路径。

| 优点 | 缺点 |
|---|---|
| **Rust 侧零改动**（当成一个 localhost 的 OpenAI 兼容端点） | 多一个监听端口（安全面） |
| 复用 `llm_client.rs` 全部逻辑 | iOS 后台容易被系统挂起，端口可能失效 |
| 调试方便（curl 就能测） | 多一层序列化开销 |

---

## 四、推荐方案

**macOS 与 iOS 统一走方案 B（Tauri 插件）**，理由：

1. iOS 是明确要做的未来目标，**方案 A 在 iOS 上根本不可用**，选 A 等于要维护两套
2. 方案 C 的 localhost 端口在 iOS 上不可靠（后台挂起、端口回收）
3. 插件机制是 Tauri 2 对移动端的官方答案，macOS 也能用

### 分层落点

```
frontend/src/lib/llm-presets.ts      + 一个「Apple 本机模型」预设（kind: 'device'）
frontend/src-tauri/src/summary/llm_client.rs
                                     + 一个 provider 分支：不发 HTTP，改调插件
frontend/src-tauri/src/apple_ai/     (新) Rust 插件壳
frontend/src-tauri/src-tauri/plugins/apple-ai/
    Cargo.toml / src/lib.rs          (Rust 侧插件定义)
    ios/Sources/AppleAiPlugin.swift  (iOS)
    macos/Sources/AppleAiPlugin.swift(macOS)
                                     Swift 侧调 FoundationModels
```

### 关键接口

Rust 侧暴露一个与现有 LLM 调用同形的接口，让 `llm_client.rs` 无感知：

```rust
// 与 HTTP provider 同形：给 prompt，返回文本
pub async fn generate(prompt: &str, system: Option<&str>) -> Result<String, String>
```

---

## 五、必须在 Mac 上验证的清单

以下每一条我都**无法在 Windows 上验证**，实施时必须逐项确认：

| # | 待验证 | 为什么重要 |
|---|---|---|
| 1 | `FoundationModels` 的确切最低系统版本 | 文档写 macOS 26 / iOS 26，需确认真机 |
| 2 | `SystemLanguageModel.default.availability` 的实际返回值 | 设备不支持 / 未开启 Apple Intelligence 时必须降级 |
| 3 | 单次请求的 **上下文窗口上限** | 会议转写可能远超，需要分片策略 |
| 4 | 中文摘要质量是否够用 | 3B 模型，可能不如 qwen2.5:1.5b |
| 5 | 是否需要 `com.apple.developer.foundation-models` entitlement | 影响签名与上架 |
| 6 | iOS 后台调用是否被限制 | 影响「录完自动生成纪要」 |
| 7 | Swift 侧流式输出能否透传到前端 | 长文本生成体验 |
| 8 | Tauri 移动端插件能否在 iOS 上跑通 | 整条路的地基 |

---

## 六、降级矩阵（设计要点）

本机模型不可用时**必须能优雅降级**，不能让用户卡住：

| 情况 | 降级路径 |
|---|---|
| macOS < 26 / iOS < 26 | 回退 Ollama（已在做）或云端 |
| Apple Silicon 但未开启 Apple Intelligence | 同上，并在设置里明确说明原因 |
| 转写超过上下文窗口 | 分片摘要再合并（现有长文本逻辑可能可复用） |

设置页应显示一行状态，例如：

```
Apple 本机模型 · 不可用（需要 macOS 26 及以上并开启 Apple Intelligence）
```

---

## 七、建议的实施顺序

1. **先在 Mac 上跑通最小验证**：一个 Swift 命令行程序，调 Foundation Models 输出一段中文。这一步能一次性回答验证清单的 1/2/4/5
2. 把该 Swift 代码包成 **Tauri 插件**，iOS 上跑通「点按钮 → 返回文本」
3. Rust 侧加 provider 分支 + 前端加预设 + 降级矩阵
4. 再做分片、流式等体验优化

**不要把 2 和 3 并行做** —— 第 1 步如果发现本机模型中文摘要质量不够，整个方案就要重新评估。

---

## 八、与现有功能的关系

- **转写**不受影响：Foundation Models 只做**文本生成**，不做语音识别。macOS/iOS 的本机**语音**能力是 `Speech` 框架，那是另一件事（可后续单独做）
- **现有 Ollama / 云端路径全部保留**，Apple 本机模型是**新增的一个选项**，不是替换
- 隐私提示必须写清楚：本机模型 = 数据不出设备，这是它相对云端的核心卖点
