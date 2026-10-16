# 第 5 轮：找到「录音保存位置」设置不生效的真正原因

## 一、起因：顺着「模型目录怎么解析」查下去

我本来想确认 `diarize_meeting` 解析模型目录的那一步（我之前的测试都是直接传路径，绕过了它）。读了 `resolve_models_dir`：

```rust
let base = load_storage_config(app).models_folder(app)?;
Ok(base.join("diarization"))
```

然后读了真实的 `storage_config.json`：

```json
{
  "storage": {
    "database_folder": "D:\\Meetly\\data",
    "models_folder": "D:\\Meetly\\Model",
    "notes_folder": "E:\\Obsidian\\Obs\\meetly",
    "recordings_folder": "D:\\Models"     ← 注意这个
  }
}
```

**模型目录是对的**（`D:\Meetly\Model` + `diarization` = 模型实际所在）。但 `recordings_folder` 是 `D:\Models` —— 而实际录音在 `D:\meetily-recordings`。

## 二、查证：这是一个真实存在的用户可见 bug

`D:\Models` 里只有一个 `ggml-tiny.bin` —— **那是个模型目录，不是录音目录**。

逐层追下去：

| 设置 | 存哪 | 谁读 |
|---|---|---|
| `storage_config.recordings_folder` | storage_config.json | **只有 `resolve()` → 设置界面的显示** |
| `recording_preferences.save_folder` | recording_preferences.json | **录音、录音库** |

**结论：设置面板里那个「录音位置」只用来显示，录音根本不读它。**

代码注释甚至写出了设计意图：

> "Falls back to the recording-preferences default when no explicit storage value is set, so this unified panel and the existing 'recordings folder' setting **cannot disagree about the default**."

它只保证**默认值**不冲突 —— 一旦用户在面板里**显式设置**，两边就会不一致，而且面板这边是**无效**的。

**这正好命中你最初的需求**「recording的save location可以修改到别的目录；Data Storage Locations 也能够自己选择」。你在「Data Storage Locations」里设了 `D:\Models`，它**没有任何效果**。

## 三、修复：两个方向都同步

### 方向 1：设置面板 → 录音偏好

`set_storage_config` 保存后，如果带了录音目录，同时写进录音偏好 —— 这样面板的设置才真正生效。

**只在用户显式传了录音目录时才写**，这样保存其它三个位置不会覆盖在录音设置里做的选择。

### 方向 2：录音偏好 → 设置面板

`set_recording_preferences` 保存后，把 `save_folder` 同步回 storage_config。

**为什么要反向**：否则用户在录音设置里改了目录，设置面板会一直显示旧的显式值 —— 还是不一致。

### 不做成死循环

`set_storage_config` 调用的是**低层**的 `save_recording_preferences`，而反向同步写在 `set_recording_preferences`（命令层）里。两者不会互相调用。

## 四、我自己引入的风险，以及怎么处理

加上同步后，`set_storage_config` 第一次依赖了 store 插件。**如果 store 调用失败，用户就再也保存不了任何存储设置** —— 这是我把一个「显示不一致」的问题升级成「完全不能用」。

所以两处同步都改成**非致命**：

- **设置面板侧**：失败写进 `outcome.detail`，让界面显示「其它位置已保存，但录音目录未能更新」
- **录音偏好侧**：失败只记日志。因为偏好**已经保存成功**了，此时让调用失败等于告诉用户"什么都没存"，是误导

## 五、验证

| 项目 | 结果 |
|---|---|
| `cargo check` | ✅ Finished（29s，无错误） |
| Rust 全量相关模块 | **456 pass，0 failed** |
| 前端 `tsc` / `bun test` | exit 0 / **140 pass** |

**另外这轮确认了模型目录解析是正确的**：`resolve_models_dir` → `D:\Meetly\Model\diarization`，与模型实际位置一致 —— 所以「识别说话人」不会因为找不到模型而失败。

## 六、顺带纠正我上一轮的错误认知

上轮我以为「`cargo check` 不受构建锁影响」。**那只在 dev 会话空闲时成立**。这轮我在 dev 运行时并发跑 cargo，又把构建卡死了。之后改成**先停 dev，再验证**，`cargo check` 稳定 29–35 秒完成。

## 七、⚠️ 你的配置里有这个陈旧值，我没有擅自改

`storage_config.json` 里 `recordings_folder: "D:\Models"`，而实际录音在 `D:\meetily-recordings`。

**我没有改它** —— 那是你的配置。修复之后的行为是：

- 面板现在显示什么，录音就真的去哪里
- 但**当前显示的 `D:\Models` 是错的**，如果你在面板上直接点保存，录音会被改到 `D:\Models`
- 建议你打开「Data Storage Locations」，把录音位置改成你真正想要的目录（例如 `D:\meetily-recordings`），再保存

## 八、还剩什么

| 项 | 状态 |
|---|---|
| **UI 目视确认** | **未做** —— 需要你点一次 |
| 录音偏好与存储面板的**其他三个**位置是否也有同类问题 | 未查（数据库/模型/笔记的路径都只在一个地方设置，风险低） |
