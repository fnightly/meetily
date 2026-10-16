# 第 12 轮：量化了性能问题，并**决定不做**这项优化

## 一、先测量，再决定

我提出过「embedding 阶段重复计算约 4 倍」。但这只是从代码注释推的**估计**，所以这轮先写了测量。

### 测量结果

```
Windows:           48
Pairs:             77
Samples embedded:  3750911 (234.4 s)     ← 送进 embedding 模型的音频总时长
Audio length:       909771 (56.9 s)      ← 而录音只有 56.9 秒
Distinct coverage: ~48 s
Redundancy factor: ~4.9x
Same chunk+speaker repeated: 0           ← 没有可以简单去重的完全重复项
Longest 10% of pairs hold 21% of the embedded samples
Median pair: 43360 samples (2.71 s)
```

**估计是准的：4.9×。** 57 秒的录音，embedding 模型实际处理了 **234 秒**音频。

## 二、但测量结果同时说明：这个优化不能顺手做

### 1. 没有"完全重复"可以简单去掉

```
Same chunk+speaker repeated: 0
```

重复来自**时间上重叠的不同窗口**（window_size=10s，shift=1s，相邻窗口重叠 9 秒）。它们的 frames 边界不同，样本集合**不完全相同**。所以不能靠"去重 key"解决，必须改成"按时间合并连续活动帧"这类真正的算法改动。

### 2. 这会**使聚类阈值校准失效**

`speaker_cluster.rs` 里的注释：

> `CALIBRATED_THRESHOLD = 0.75` …
> **This value belongs to the embedding model, so it must be re-measured if the model changes.**
> **The band that yields four is roughly 0.71–0.79, which is narrow**

**关键点**：把 77 个窗口级 embedding 换成约 15 个连续 span 的 embedding，聚类看到的**分布完全不同** —— 0.75 这个值是针对**当前粒度**测出来的。改动之后：

- 0.75 很可能不再给出 4 位说话人
- 而且**可接受区间只有 0.08 宽**（0.71–0.79），所以不是"稍微调一下"就行

也就是说，这不是一个性能改动，而是**一次需要重新校准 + 重新验证的研究任务**。

## 三、决定：不做

**理由不是"太难"，而是"做一半更糟"。**

- 这是**性能**改进，不是正确性修复 —— 功能本身是对的
- 0.7× 实时 + 进度反馈，对一个**录完之后的**后台任务是可接受的
- 我剩余的工作预算不足以完成"改算法 → 重新扫阈值 → 重新验证 4 人 → 确认真实录音仍正确"这一整条链
- **半途而废会让功能比现在更差**（阈值失配 + 校准注释失效）

## 四、留下了什么给下一次

诊断测试已保留为 `#[ignore]` 测试（`diarization_e2e.rs:1255`），下次可以直接跑出同样的数字，不需要重新发明测量方法。

要做这项优化的话，完整的验证清单是：

1. 改 `speaker_pairs`：按时间合并同一说话人的连续活动帧
2. 跑 `sweep_clustering_threshold` 重新找阈值
3. 更新 `CALIBRATED_THRESHOLD` 及其注释里的区间
4. 跑 `the_full_pipeline_separates_the_four_reference_speakers`（必须仍是 4）
5. 跑 `a_four_speaker_recording_is_labelled_into_four_speakers`（必须仍是 4 个标签）
6. 跑 `real_recording_separates_without_erroring`（独白仍须是 1）
7. 对比优化前后的耗时

## 五、验证

| 项目 | 结果 |
|---|---|
| 冗余度量（真实模型 + 参考音频） | ✅ 4.9×，234.4s / 56.9s |
| 诊断测试 | ✅ 保留为 `#[ignore]`，可重复运行 |
| 应用 | PID 16060，前端 HTTP 200 |

## 六、关于"ready to use"的现状

到这一轮为止：

| 维度 | 状态 |
|---|---|
| 功能完整性 | ✅ 安装（下载）→ 分离 → 标注 → 显示 → 导出/复制，全链路 |
| 正确性 | ✅ 484 Rust + 185 前端测试；真实音频、真实数据库、真实网络 |
| 已知缺陷 | ✅ 本次会话发现并修复 6 个（含 2 个"看起来做完了实际没用"） |
| **性能** | ⚠️ 0.7× 实时（已量化，已定位，已说明修复路径与风险） |
| **人工确认** | ❌ 未做 —— 需要你 |
