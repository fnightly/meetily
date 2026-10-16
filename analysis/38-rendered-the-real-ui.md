# 第 14 轮：终于把真实界面渲染出来了 —— 并因此抓到一个会整树崩溃的缺陷

## 一、换了工具就成功了

前几轮我用**裸 Chrome** 截图，挂在沙箱的命名管道限制上。这轮改用项目自带的 `browser` 技能（基于 CDP + `ws`），**立刻就通了**：

```
Starting Chrome on port 9222...
Chrome launched (PID: 2984)
✅ 9222 在监听
```

## 二、关键思路：把 Tauri 的 IPC 打桩

页面是客户端渲染的，所有数据都经过 `invoke`。而 `invoke` 只是：

```js
async function invoke(cmd, args = {}, options) {
    return window.__TAURI_INTERNALS__.invoke(cmd, args, options);
}
```

**所以只要在页面加载前替换 `window.__TAURI_INTERNALS__`，就能让真实的 React 组件用真实形状的数据渲染出来。**

我写了一个脚本 `scripts/cdpshot.cjs`，通过 `Page.addScriptToEvaluateOnNewDocument` 在导航前注入打桩（必须早于应用 bundle，否则 React 挂载时桩还没就位），然后截图并导出页面可见文本。

## 三、逐步逼近：每一轮都暴露一个未打桩的命令

| 迭代 | 现象 | 原因 |
|---|---|---|
| 1 | 停在「Loading Preferences...」 | `plugin:store\|get` 返回 shape 不对（应返回 `[value, exists]` 元组） |
| 2 | 树被错误边界拆掉 | **`RecordingStateContext` 读到 null 崩溃** ← 见下 |
| 3 | 又崩 | `ConfigContext` 的 `models.map` 无保护 |
| 4 | **成功渲染完整设置页** | 补上集合类默认值 |

**第 4 次迭代拿到了完整页面文本**：

```
返回 设置 通用 录音 词典 转写 纪要 实验功能
Data Storage Locations
Database   D:\Meetly\data\meeting_minutes.sqlite
Models     D:\Meetly\Model
Meeting Notes      E:\notes
Meeting Recordings D:\meetily-recordings
```

**注意最后一行** —— 那正是我第 5 轮修过的「面板只用于显示」的那个设置，现在显示的就是录音实际去的位置。

## 四、抓到的真实缺陷

```
Unhandled Runtime Error
TypeError: Cannot read properties of null (reading 'is_recording')
  src\contexts\RecordingStateContext.tsx (95:35)
```

源码：

```tsx
try {
  const backendState = await recordingService.getRecordingState();

  setState(prev => ({
    ...prev,
    isRecording: backendState.is_recording,   // ← 在 render 阶段执行
    ...
  }));
} catch (error) {
  // 以为这里能兜住
}
```

**问题在于 `try/catch` 给人虚假的安全感。**

React 的 `setState(updater)` 中的 **updater 函数是在 render 阶段被调用的**，不在 `await` 那一刻。所以：

- `try` 保护的是 `await` 本身
- **updater 里的字段读取抛错时，已经离开了 `try` 的作用域**
- 结果：**未捕获的运行时错误 → 整棵树被错误边界重建**

**修复**：在把值交给 updater **之前**做判空 —— 这样才有意义。

```tsx
if (!backendState) {
  console.warn('[RecordingStateContext] Backend returned no recording state; keeping the current one');
  return;
}
```

后端命令目前总是返回对象，所以这是**潜在**缺陷而非现网故障。但"一个 null 就让整个应用崩掉、而 catch 又兜不住"这个失效模式是真实的。

## 五、同时发现的**一类**模式

继续往下点「转写」页签时，又撞到同一个模式：

```
TypeError: Cannot read properties of null (reading 'find')
  src\components\TranscriptSettings.tsx (41:40)
  const selectedProvider = providers.find((p) => p.id === uiProvider);
```

同类的还有 `ConfigContext` 的 `models.map`。

**我的打桩对未处理的命令返回 `null`，于是把这些"假定的非空返回"逐个暴露出来。**

**我没有把这些全部改掉** —— 在真实后端下它们不会触发，逐个加保护属于防御性硬化，而且可能掩盖真实错误。我修了 `RecordingStateContext` 这一处，因为它的 `try/catch` 是**误导性的**（读者会以为已经兜住了）。

## 六、验证

| 项目 | 结果 |
|---|---|
| 设置页完整渲染 | ✅ 可见文本含全部四个存储位置 |
| 前端 `tsc` / `bun test` | exit 0 / **185 pass** |
| 提交 | ✅ `742ef6e` |
| 调试 Chrome | 已关闭（8 个进程），9222 已释放 |
| 应用 | PID 28220，前端 HTTP 200 |

## 七、为什么停留在「设置页」而没有截到说话人分离面板

设置页「转写」页签**在到达我的组件之前**就崩了 —— 崩在 `TranscriptSettings.tsx`（列表里排在我的组件**前面**的既有组件）。

要再看下去，需要继续为 `useSpeechProviders` 等打桩。我评估后停手了：

- 这是一条**没有明确终点**的打桩链（每个上下文都有自己的命令形状）
- 已经拿到了足够的证据：**真实页面能渲染、我能读到它的文本、并且因此抓到了一个真实缺陷**
- 组件级渲染已由 **25 条渲染测试**覆盖，与"能不能看见"无关

## 八、留下的东西

1. **`scripts/cdpshot.cjs`** —— 一个可复用的界面渲染与截图工具。它把 Tauri IPC 打桩，让任何页面能在浏览器里渲染出来。**这是这个项目以前没有的能力**，下次要看界面不必再靠裸 Chrome 碰运气。
2. **`RecordingStateContext` 的修复**（已提交）
3. **一类模式的记录**：多处代码假定后端返回非空，而失效模式是整树崩溃

## 九、目标状态

「确认程序处于可 ready 使用的状态」这一项，本轮**实质性推进了** —— 我第一次真正看到了渲染出来的界面（设置页），而不只是组件的单元渲染。

但**仍未看到说话人分离面板本身**。要看到它，要么：
- 继续为「转写」页签的上下文打桩（有明确工作量，无明确终点）
- 或者**你点一次**（30 秒）
