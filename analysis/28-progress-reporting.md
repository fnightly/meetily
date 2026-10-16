# 第 4 轮：长会议进度反馈

## 一、要解决的问题

一次 14 分钟的会议，分离需要约 10 分钟（0.7× 实时）。这 10 分钟里界面上**只有一个转圈的按钮** —— 看起来就是卡死了。

原来 `diarize_samples` 是一个黑盒：调用它，等，返回。中间过程没有任何出口。

## 二、做法：给服务加一个进度回调，但**不改动原有入口**

```rust
pub fn diarize_samples(samples, models_dir, config) -> Result<...> {
    diarize_samples_with_progress(samples, models_dir, config, None)   // 原入口不变
}

pub fn diarize_samples_with_progress(..., on_progress: Option<&dyn Fn(DiarizationPhase, f32)>)
```

**为什么加一个函数而不是改签名**：`diarize_samples` 有 6 个调用点（1 个命令 + 5 个测试）。加一个平行的带进度入口，让不关心进度的调用方一行都不用改。

### 三个阶段

```rust
pub enum DiarizationPhase { Segmenting, Embedding, Clustering }
```

刻意保持粗糙：界面需要知道**在推进**，不需要一个还要自己平滑的百分比。三个阶段耗时差异极大（绝大部分是 embedding），所以按阶段报告而不是硬切成三等份。

### 报告点

| 位置 | 报告内容 |
|---|---|
| 进入服务时 | `Segmenting 0%` |
| **每个分割批次结束后** | `Segmenting k/n` |
| 进入 embedding | `Embedding 0%` |
| 进入聚类 | `Clustering 0%` |

分段进度是**按批次**报的 —— 长会议会跑很多批次，只报一次"开始"等于没报。

## 三、事件节流：只在整数百分比变化时发事件

分割阶段每批次都报，长会议会有几百个事件。命令侧用 `RefCell` 记住上次发出的 `(阶段, 百分比)`：

```rust
if *last_reported.borrow() == Some((phase, percent)) { return; }
```

**为什么用 `RefCell` 而不是 `let mut`**：回调签名是 `&dyn Fn`（不是 `FnMut`），而运行在单个 blocking 线程上，没有共享，所以内部可变性是安全的。

**踩到的坑**：我第一版用 `let mut last_reported`，编译器直接报：

```
error[E0525]: expected a closure that implements the `Fn` trait,
              but this closure only implements `FnMut`
```

## 四、前端：订阅事件并在按钮上显示

```
正在找出语音… 37%
正在识别声纹… 100%
正在归组说话人… 100%
```

事件名 `diarization-progress`（kebab-case，与项目里 `audio-levels`、`model-config-updated` 一致）。

细节：
- **过滤 `meeting_id`** —— 多窗口时不要显示别的会议进度
- **处理竞态** —— 监听器可能在运行结束后才注册成功，用一个 `cancelled` 标志避免泄漏
- **没有进度时回退**到原来的「正在识别说话人…」，不是空白

## 五、验证

### 真实音频上的分阶段进度

```
Reports: 5
Phases in order: [Segmenting, Embedding, Clustering]
test result: ok. 1 passed
```

**5 次报告**（节流生效：十几秒的片段只发了 5 次），三个阶段按顺序出现，最后的分割报告是 1.0。

### 单元测试（14 条，含 5 条新增）

| 测试 | 保护的东西 |
|---|---|
| `progress_begins_before_any_model_is_loaded` | 第一条报告不能等模型加载完 |
| `a_callback_is_not_required` | 不带回调的入口仍然可用 |
| `progress_never_goes_backwards_within_a_phase` | 进度条不能倒退 |
| `phase_labels_are_non_empty` | 每个阶段都有文案 |
| `phases_serialise_as_snake_case_for_the_interface` | 线上格式 `"segmenting"` 是契约 |

### 端到端仍然通过

```
Separation: 1 speaker(s), 1 turn(s)
Labelled 1 of 1 row(s)
  transcript-76f7e2a7-... -> Some("speaker_1")
The full command path works against a real meeting.
```

### 合计

| 项目 | 结果 |
|---|---|
| Rust 合计 | **449 pass** |
| 前端 `tsc` / `bun test` | exit 0 / **140 pass** |
| i18n 中文覆盖 | ✅ |

## 六、这一轮我犯的两个错误

### 1. 并发 `cargo check` 与 dev 会话**死锁**

我在 `tauri dev` 正在重建时反复跑 `cargo check`，结果 cargo 进程卡住 —— **15 分钟只用了 4.8 秒 CPU**（说明在等锁，不是在编译），没有任何 rustc 子进程。

清理所有进程后，同样的 `cargo check` **35 秒完成**。

> **教训**：dev 会话在重建时不要并发跑 cargo。之前我以为"cargo check 不受锁影响"，那只在 dev 空闲时成立。

### 2. 又一次把测试插进了函数内部

`diarization_e2e.rs` **没有 `mod tests` 包裹**，测试是模块级自由函数。我用"在最后一个 `}` 之前插入"的方式，把新测试插进了上一个测试函数体内，编译报 `can't capture dynamic environment in a fn item`。

（第十行 `&models_dir()` 被当成局部变量，因为外层函数里有个同名局部变量。）

**这和前几轮的错误是同一类**：用"文件末尾的 `}`"当锚点是不可靠的。这次我加了结构校验才发现。

## 七、还剩什么

| 项 | 状态 |
|---|---|
| **UI 目视确认** | **未做** —— 需要你点一次 |
| 阈值 / 说话人上限设置 | 未做 |
| 录音停止后自动分离 | 未做（当前需显式触发，符合"默认关闭"取向） |
