# 48 · 进程 loopback 采集的实现阻碍（未完成）

尝试实现分应用录音的最后一步（`ActivateAudioInterfaceAsync` 进程级 loopback），
**未完成，已回退**。工作区保持干净可编译。

记录这轮遇到的**具体阻碍**，下次直接从这里继续，不用重新踩。

---

## 已完成的前置（已提交）

- `audio/app_audio.rs` — `list_audio_apps()`，进程音频会话枚举
- 进程树归并：`Weixin` + `WeChatAppEx` → 一行 `Weixin`，`process_ids: [18344, 22548]`
- 命令 `list_audio_apps` 已注册

---

## 这轮卡住的四个点

### 1. `#[implement]` 需要 `windows-core` 作为直接依赖

`windows` crate 的 `#[implement(IActivateAudioInterfaceCompletionHandler)]` 宏
展开成 `::windows_core::...` 路径。只依赖 `windows`（通过 `windows::core` 间接可达）
不够，会报 `cannot find windows_core in the crate root`。

**解法**：`Cargo.toml` 加 `windows-core = "0.61"`（锁文件里已有 0.61.2）。

### 2. `PROPVARIANT` 在 `Com::StructuredStorage`，不在 `Variant`

```rust
// 错
use windows::Win32::System::Variant::PROPVARIANT;
// 对
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
```

需要 feature `Win32_System_Com_StructuredStorage`（已确认可启用）。

### 3. `ActivateCompleted` 的签名是 `Ref<>` 不是 `Option<&>`

```rust
// 引擎报的：expected `Ref<'_, ...>`, found `Option<&...>`
fn ActivateCompleted(
    &self,
    activateoperation: Ref<'_, IActivateAudioInterfaceAsyncOperation>,
) -> windows::core::Result<()>
```

`Ref<T>` 是 `windows-core` 里表示「可为 null 的 COM 引用」的类型，不是 Rust 的 `&T` 或
`Option<&T>`。**这是最后没解决的点之一**，需要确认 `Ref` 的构造与解引用方式。

### 4. `PROPVARIANT` 的字段是方法，不是字段

```rust
// 报错：attempted to take value of method `vt`；no field `blob` on `PROPVARIANT_0`
variant.vt = VT_BLOB;
variant.Anonymous.blob.cbSize = ...;
```

windows-rs 0.61 的 `PROPVARIANT` 不是裸联合体：
- `vt` 是**方法**（`variant.vt()` / 或通过 `From` 构造）
- 联合体成员名不是 `blob`

**需要查的**：`PROPVARIANT` 在 0.61 里如何表示 `VT_BLOB`。
可能是 `PROPVARIANT::from(...)`、或者 `windows_core::PROPVARIANT` 有专门的 blob setter，
或者需要用 `PROPVARIANT_SetBlob` 之类的辅助函数。**这是另一个没解决的点。**

---

## 下次的起手式

```rust
// 1. Cargo.toml
windows-core = "0.61"
windows = { version = "0.61", features = [
    "Win32_Foundation", "Win32_Media_Audio", "Win32_System_Com",
    "Win32_System_Com_StructuredStorage", "Win32_System_Threading",
    "Win32_System_Variant", "Win32_System_Diagnostics_ToolHelp",
] }

// 2. 先只写「构造 PROPVARIANT」这一件事，写个单测断言字节
//    确认 vt=VT_BLOB、cbSize=sizeof(AUDIOCLIENT_ACTIVATION_PARAMS)、指针正确
//    这一步过了，再碰 ActivateAudioInterfaceAsync

// 3. 回调用 Ref<'_, T>，不是 Option<&T>
```

**建议顺序**：第 2 步单独验证。`PROPVARIANT` 布局错了，WASAPI 会在很深的地方
返回一个毫无信息量的 HRESULT，debug 成本极高。

---

## 数据布局参考（这部分是对的，没被验证卡住）

```rust
AUDIOCLIENT_ACTIVATION_PARAMS {
    ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
    Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
        ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
            TargetProcessId: pid,
            ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
        },
    },
}
```

> 字段名 `ProcessLoopbackMode` 与 `ProcessTreeScope` 二选一 —— 文档里两种写法都出现过，
> 编译一次就能确认哪个是 0.61 的。

捕获格式：48kHz / 单声道 / f32（`WAVEFORMATEX` wFormatTag=3）。
流标志：`AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_LOOPBACK`。

---

## 结论

**不是设计问题，是 API 绑定的具体细节没配平。** 前置全部就绪，剩下的是一次
有充足上下文时能在 1–2 小时内收掉的收尾工作。
