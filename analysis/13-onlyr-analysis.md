# OnlyR 功能分析与吸收建议

参考对象：[AntonyCorbett/OnlyR](https://github.com/AntonyCorbett/OnlyR)（MIT，Windows，C#/WPF/NAudio）

## 一、OnlyR 实际上有什么

刻意极简：主界面就是一个录音按钮 + 音量表。但它把"**稳定地把音频录下来**"这件事做得很扎实。

### 录音配置（`OnlyR.Core/Recorder/RecordingConfig.cs`、`Services/Options/Options.cs`）

| 选项 | 值 |
|---|---|
| Codec | **仅 MP3 / WAV** |
| Mp3BitRate | 可选 |
| SampleRate / ChannelCount | 可选（单声道/立体声） |
| UseLoopbackCapture | 录系统声音 |
| MaxRecordingTimeSeconds | **最长录音时长** |
| MaxSilenceTimeSeconds + SilenceAsVolumePercentage | **静音自动停止** |
| FadeOut | 停止时淡出 |
| StartRecordingOnLaunch | 启动即录 |

值得注意：**Codec 只有 MP3 和 WAV**——比我们的 5 种格式（M4A/MP3/WAV/FLAC/OGG）**更少**。所以格式这块我们已经超过它。

### MP3 元数据写入（`RecordingConfig.cs`）

```
TrackTitle / AlbumName / Genre  →  写入 MP3 tag
```

录音本身带上标题/专辑/流派标签。我们用 ffmpeg 就能做到，成本很低。

### 三个"录音卫生"服务（这是它真正的价值所在）

| 服务 | 作用 |
|---|---|
| `Services/AudioSilence` | 静音计时，配合 `MaxSilenceTimeSeconds` 自动停止 |
| `Services/PurgeRecordings` | **保留策略**：按 `RecordingsLifeTimeDays` 自动删除过期录音，并清理空目录 |
| `Services/RecordingCopies` | 把录音**复制到移动盘**，完成后**安全弹出该盘** |
| `Exceptions/NoSpaceException.cs` | **磁盘空间不足检测** |
| `Services/RecordingDestination` | 分层目录 `年/月/日` + 序号文件名，规避同名 |
| `Services/Snackbar` | 轻量非阻塞提示 |

### 静音检测实现（`SilenceService.cs`，全文仅 3 个方法）

```csharp
public int GetSecondsOfSilence() =>
    _nonSilenceLastDetected == default ? 0
    : (int)(DateTime.UtcNow - _nonSilenceLastDetected).TotalSeconds;

public void ReportVolume(int volumeLevelAsPercentage) {
    if (volumeLevelAsPercentage > Options.SilenceAsVolumePercentage)
        _nonSilenceLastDetected = DateTime.UtcNow;
}
```

**关键设计**：记录"**最后一次超过阈值**的时刻"，而不是判断"当前是否安静"。

这点很重要——低但持续的室内噪声会让"当前是否安静"永远为否，但"距上次超过阈值多久"能正确工作。我把它作为我们实现的参照。

### 音量表（`VolumeMeter/VduControl.cs`）

分段 LED 条（`_levelsCount` 块）：顶部 1/7 红、其下 1/4 黄、其余绿。

**这从标准 VU 表的角度确认了我们一个 bug**：我们的电平条把**低电平染成绿色、高电平染成红色**——正好反了。已修。

## 二、已吸收并落地

### 1. 修正电平表配色（`AudioLevelMeter.tsx`）

- 旧：`<0.3 绿`、`<0.7 黄`、`否则红` → 正常语音（对数刻度下约 60%）显示为**警告色**，而接近静音却显示**绿色**。
- 新：`≥0.9 红`、`≥0.7 黄`、`<0.1 灰（无信号）`、其余绿。

与 OnlyR 的 VDU 分段逻辑一致（阈值在标尺顶部）。

### 2. 录音电平/增益提示（新功能，对应你的需求）

| 文件 | 作用 |
|---|---|
| `src/lib/audio-levels.ts` | 滑动窗口分析 + 静音计时 |
| `src/components/RecordingLevelMonitor.tsx` | 录音中电平表 + 告警 |
| `src/hooks/useRecordingLevelMonitoring.ts` | 录音期间启停电平监控 |
| `tests/lib/audio-levels.test.ts` | 29 例测试 |

判决类型：`silent` / `quiet` / `clipping` / `healthy`。

**按设备分别诊断**——这正是线上会议最需要的：麦克风正常但**输出设备选错**时，能明确告诉你是哪一路没有声音：

> No audio is reaching (Speakers). Check the device is not muted and that it is the one actually playing or capturing the meeting.

阈值：静音 < −50 dBFS、偏低 < −40 dBFS（取窗口**均值**，避免句间停顿误报）、削波 peak ≥ 0.98。

### 3. 静音时长追踪（为自动停止铺路）

按 OnlyR 的思路实现 `secondsSinceAudible()` / `isSilentFor(threshold)`，并处理了两处它没有覆盖的边界：

- **未收到任何样本 ≠ 静音**：设备不上报是"未知"，不能触发自动停止（否则启动瞬间就会停）。
- **收到样本但全部低于阈值 = 确实静音**：从首次观测时刻起算。这一条是我写的第一版漏掉的，被自己的测试抓出来。

另有"有噪声的房间不算静音"与"多设备只要一路有声就重置计时"两个测试——后者对线上会议很关键（对方说话时你的麦克风是静的）。

### 4. 顺带修复：系统音频轨此前**无法显示电平**

`level_monitor.rs` 对输出设备直接返回 `"Output device monitoring not supported yet"`。而 cpal 补丁在 `data_flow() == eRender` 时会自动加 `AUDCLNT_STREAMFLAGS_LOOPBACK`，**输出设备上本就能 `build_input_stream`**（录音路径就是这么做的）。

结果是：电平表永远只能显示麦克风，系统音频轨拿不到数据。已改为统一走 `build_input_stream`，输出设备经 WASAPI loopback 监控。

## 三、建议吸收的清单（按性价比排序）

### 第一梯队：直接对应软件的真实痛点

| 功能 | 为什么值得做 | 成本 |
|---|---|---|
| **静音自动停止** | 会后忘记停录是真实场景；省磁盘、省电。基础已就绪（`isSilentFor`） | 中 |
| **最长录音时长上限** | 同样的跑飞保护，比静音检测更简单直接 | 低 |
| **保留策略**（超 N 天自动清理 + 清空目录） | 磁盘被录音填满是本项目的实际故障模式（我们已经因为磁盘满失败过一次） | 中 |
| **磁盘空间不足检测** | 已有下载前检查；录音是长时间写入，更需要运行中监控 | 低 |

### 第二梯队：提升可用性

| 功能 | 说明 |
|---|---|
| **MP3/WAV 元数据标签** | 用 ffmpeg `-metadata` 写入标题/日期/会议名，音频文件自带信息 |
| **录音复制到移动盘 + 安全弹出** | 对"录完就拔盘"的现场录音场景有用；仅当你有这需求 |
| **启动即录（StartRecordingOnLaunch）** | 配合你的"快速录音"诉求；但要谨慎，误启动会录到私人对话 |

### 不建议吸收

| 功能 | 原因 |
|---|---|
| MP3/WAV 两种格式 | **我们已有 5 种，比它多** |
| 命令行录音 | 与 GUI 应用定位不符 |
| `年/月/日` 分层目录 | 我们按会议名建文件夹且带 `metadata.json` + 增量 checkpoint，更契合会议场景 |
| `MaxRecordingsInOneFolder` | 我们不复用文件名，无此需求 |

### 需要你决定的一点

**静音自动停止有丢失尾声的风险**。建议实现成：默认关闭，开启时给一个较保守的阈值（如 5 分钟），并在停止前 N 秒提示"即将因静音停止"。

## 四、验证

| 项目 | 结果 |
|---|---|
| `audio-levels` 测试 | **29 pass / 0 fail** |
| 前端 bun 测试（全量） | **94 pass / 0 fail** |
| `tsc --noEmit` | exit 0 |
| `cargo check -p meetily` | Finished，0 error |
| Rust 关键模块测试 | formats 15、http_client 12、storage::config 7、incremental 2、retranscription 15、catalog 22、summary::processor 37 —— **全通过** |
| `pnpm build` | 通过（8/8 静态页） |
