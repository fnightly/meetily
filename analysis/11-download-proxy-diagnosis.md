# 故障记录：模型下载失败（tcp connect error）

## 现象

应用内下载 Whisper 模型时报：

```
Failed to start download: error sending request for url
(https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin):
error trying to connect: tcp connect
```

## 诊断过程（一次被自己误导的经历，值得记录）

### 误判一：以为是镜像地址问题

用户问"能否提供更多下载地址"，我最初按这个方向排查，并用 PowerShell 测了三个源：

| 源 | 结果 |
|---|---|
| huggingface.co | 200 OK |
| hf-mirror.com | 200 OK |
| ModelScope | 404（我没找到正确的公开仓库路径） |

**结论一度是"源都通，所以不是地址问题"——但这个测试本身是错的。**

### 误判二：忽略 PowerShell 自动走代理

后来才意识到：**PowerShell / .NET `HttpWebRequest` 会自动使用 WinHTTP/IE 系统代理**，而我要诊断的 Rust 程序不会。所以"PowerShell 能下载"根本不能证明"应用能下载"。

补做对照实验后真相清晰：

| 方式 | 结果 |
|---|---|
| **经代理** `127.0.0.1:10808` | **200 OK**，465 MB，2424ms |
| **直连** | **超时失败** |

### 真正的根因

```
HKCU\...\Internet Settings
  ProxyEnable = 0x1
  ProxyServer = 127.0.0.1:10808
```

系统代理开启且必须走代理才能到 HuggingFace。而：

- 浏览器 / PowerShell 读系统代理 → 通
- **Rust 的 `reqwest` 不读 Windows 系统代理** → 直连 → 超时

`reqwest 0.11` 只在启用 `system-proxy` 特性时才读 OS 代理，而项目声明的是：

```toml
reqwest = { version = "0.11", features = ["blocking", "multipart", "json", "stream"] }
```

**没有 `system-proxy`。**

## 修复

### 新增 `src/http_client.rs`

代理解析优先级：
1. 环境变量 `HTTPS_PROXY` / `https_proxy` / `HTTP_PROXY` / `http_proxy` / `ALL_PROXY` / `all_proxy`
2. **Windows 注册表系统代理**（`ProxyEnable=1` 时读 `ProxyServer`）——无需用户改任何设置

提供了四个构造函数：
- `client_with_proxy()` — 异步，日常使用（带 User-Agent）
- `client_with_proxy_and_timeout(d)` — 异步 + 总超时
- `blocking_client_with_proxy()` — 同步调用点
- `resolve_proxy_url()` — 供需要自建 builder 的流式下载复用

**两个实现细节**：

1. **`ProxyServer` 有两种格式**：单一 `host:port`，或分协议 `http=...;https=...`。后者优先取 `https` 映射。解析函数是纯函数，可单测。
2. **必须补 `http://` 前缀**：`reqwest` 要求完整 URL，裸 `host:port` 会被拒绝。注册表里存的正是裸形式。

**权限验证**：注册表读取用的是 `reg query`（而非 `fsutil`——后者在另一个功能里实测需要管理员权限）。已验证普通进程可读。

#### 关键陷阱：`REG_DWORD` 是十六进制，`parse::<u32>()` 会静默失败

第一版实现写的是：

```rust
.and_then(|value| value.parse::<u32>().ok())   // "0x1" -> None
.unwrap_or(0)                                   // -> 0 == 未启用
```

`reg query` 对 `REG_DWORD` 返回的是 **`0x1`**，而 `str::parse::<u32>()` **不接受 `0x` 前缀** → 解析失败 → `unwrap_or(0)` → 判定代理未启用 → `resolve_proxy_url()` 返回 `None`。

**最危险的地方在于它是静默的**：单元测试当时**全绿**——因为我只断言了"不 panic"，而返回 `None` 恰好不 panic。修复看起来生效，实际什么都没做。

**是靠端到端测试才暴露的**。我加了一个 `#[ignore]` 的真实网络测试，打印解析结果：

```
# 修复前
resolved proxy: None
status: 200 OK          # ← 这个 200 是直连侥幸成功，会掩盖问题

# 修复后
resolved proxy: Some("http://127.0.0.1:10808")
status: 200 OK
```

注意修复前那次 `status: 200`——如果只看状态码就会误判为"已经好了"。**打印中间状态（解析到的代理）才是有效的验证手段。**

修法：新增纯函数 `parse_reg_dword`，同时支持 `0x` 十六进制与十进制，并补了 4 个测试（`0x1`、`0X1`、`0xF`、十进制、空白、非数字 token）。

### 顺带修正另一处解析脆弱点

读 `ProxyServer` 时原实现取该行最后一个空白分隔字段。正常输出为：

```
    ProxyServer    REG_SZ    127.0.0.1:10808
```

末字段是值本身，没问题。但加了一道 `filter(|v| *v != "REG_SZ")` 防御：若某个 Windows 语言/版本的输出把类型名放在行尾，就能避免把 `REG_SZ` 当成代理值。

### 接入范围：19 处

| 文件 | 处数 | 说明 |
|---|---|---|
| `whisper_engine.rs` | 1 | 模型下载（用户实际踩到的路径） |
| `parakeet_engine.rs` | 1 | 模型下载，保留原 3600s 超时 |
| `model_manager.rs` | 1 | 内置摘要模型下载，保留原 3600s 超时 |
| `ollama.rs` / `metadata.rs` | 4 | 本地 11434，但仍统一 |
| `anthropic.rs` / `openai.rs` / `groq.rs` / `openrouter.rs` | 4 | 云端 provider，**同样受影响** |
| `llm_client.rs` | 3 | 摘要的云端调用 |
| `service.rs` | 1 | 摘要主流程 |
| `api.rs` | 3 | 后端连通性测试、自定义 OpenAI 端点测试 |

**注意**：这意味着修复前**云端 LLM provider 与自定义 OpenAI 端点也是坏的**——不只是模型下载。用户大概率还没测到那一步。

### 顺带修正

- 移除三处因改造而失效的 `use reqwest::Client;`（`ollama.rs`、`metadata.rs`、`openrouter.rs`）——留着会是编译警告。
- `openrouter::get_openrouter_models` 是**同步 `#[command]` + blocking client 且无超时**，会冻结主线程且可能无限挂起。已加 20 秒超时。（其"同步命令"的结构性问题未动，属于独立议题。）

## 验证

| 项目 | 结果 |
|---|---|
| 代理检测（实测注册表） | 提取 `127.0.0.1:10808` → `http://127.0.0.1:10808` ✅ |
| `reg query` 需要管理员权限吗 | **不需要**，普通进程可读 ✅ |
| 经代理 vs 直连对照 | 代理 200 OK / 直连超时 ✅ |
| 代理 URL 解析单测 | 12 例（裸 host:port、已有 scheme、分协议映射、空白、空值、hex/十进制 dword） |
| **端到端真实网络测试** | `resolved proxy: Some("http://127.0.0.1:10808")` + `status: 200 OK` ✅ |
| `cargo check -p meetily` | **Finished，0 error** |

端到端测试默认 `#[ignore]`，在有代理的机器上显式运行：

```powershell
cargo test -p meetily --lib http_client -- --ignored --nocapture
```


## 用户无需任何配置

修复后应用会**自动**读系统代理（`127.0.0.1:10808`），不必设置环境变量。

若想临时绕过（不必等重编）——`reqwest` 会读 `HTTPS_PROXY`：

```powershell
$env:HTTPS_PROXY = "http://127.0.0.1:10808"   # 必须带 http:// 前缀
cd D:\Meetly\frontend
pnpm run tauri:dev
```

环境变量必须在**启动应用的同一终端**设置，因为由进程继承。
