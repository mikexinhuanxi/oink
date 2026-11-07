# Mos 共存问题上游反馈

发布位置：[oinkvm/oink Issue #16](https://github.com/oinkvm/oink/issues/16#issuecomment-6704229809)。以下为已发布的技术反馈正文。

---

补充一个与本 Issue 及 #7 相同方向的 **Mos 共存复现、AI 辅助排查和本机验证结果**。我看到此前已有“退出 Mos 后恢复”的反馈，因此补充到这里，不另开重复 Issue，也不直接向上游提交 PR。

我需要保留 Mos 的本地平滑滚动。以下参考修改目前只在自己的 fork 中，用于提供可复现上下文和代码定位，供你判断是否适合维护。

## 环境

- 原始问题版本：Mac / Windows 均为官方 Oink **0.3.3**。
- Mac：Apple Silicon，**macOS 27.0.1**。
- Windows：Oink 当前设备信息仅返回 `Windows`；具体 Windows 版本及 OS Build 尚未核对，不能当作本 Issue 标题中的 25H2 环境。
- 鼠标：**迈从 G3 V2（Maicong G3 V2）**。
- Mac 安装 **Mos 4.2.1**，开启平滑、滚动反转，横向/纵向反转均开启；本例没有配置应用例外，allowlist 关闭。

## 复现与对照

1. 保持 Mos 开启，将鼠标接在 Mac 上，经 Oink 控制 Windows。Mac 本地向下滚动时，Windows 的方向相反。
2. 在 Windows 端切换“鼠标滚轮反转”，上述 Mac 源鼠标的方向不变。这一开关属于连接在该电脑上的鼠标，因此这一步本身不是 Windows 反转逻辑失效的证据。
3. 将鼠标接在 Windows 上，经 Oink 控制 Mac，出现滚轮不响应。
4. 完全退出 Mac 上的 Mos，重新测试：**两个方向均恢复正常**。

这是同一套设备的用户实测对照；没有完成逐应用、所有 Mos 选项或触控板的完整测试矩阵。

| 版本 / 状态 | Mac 鼠标 → Windows | Windows 鼠标 → Mac |
| --- | --- | --- |
| 官方 0.3.3，Mos 开启 | 与 Mac 本地方向相反 | 滚轮不响应 |
| 官方 0.3.3，退出 Mos | 正常 | 正常 |
| 参考修改，Mos 开启 | 用户确认正常 | 用户确认正常 |

## AI 辅助排查结论及参考实现

结合退出 Mos 的对照和源码，问题与 Mos 的共存高度相关。具体事件处理机制来自源码分析；原始问题阶段没有保存完整的逐事件失效追踪，因此下面的处理链解释不等同于已经抓到所有丢失事件的系统 trace。

- Oink 在 **Session event tap** 采集。Mos 4.2.1 在 **AnnotatedSession event tap** 调整方向并平滑滚动，部分原始滚轮会被消费，平滑帧直接发往目标 PID。源机本地事件继续经过 Mos，但发往 Windows 的原始事件已在更早的位置被 Oink 转发，这可以解释方向差异。
- 参考修改保留 Session 采集边界，只在发往远端的普通离散鼠标滚轮副本上应用 Mos 的横/纵反转，同时处理加速后的行量与 raw wheel steps。本地原生事件仍交给 Mos，因此本地平滑滚动保留；没有向 Windows 复刻或传输 Mos 的平滑曲线。
- 接收端先用 Oink 原标记确认自己的注入事件，再复制事件来源并设置 Mos 4.2.1 的 processed-scroll marker，让 Mos 跳过对此类已处理远端滚轮的再次消费、反转和平滑。兼容回环识别同时要求本进程 PID 与 scroll 类型，真实 Mos 进程的帧仍视为外部事件。
- 本机 CoreGraphics 检查发现，读取来源标记后直接改 `EventSourceUserData` 可能仍保留缓存值；复制来源后再更新标记可生效，并验证不会修改键盘/指针共用的来源对象。相关原生回归检查包含载荷与共享来源完整性。
- 主队列每 500 ms 读取 Mos 偏好并检查其启用的滚轮 tap；仅安装 Mos、退出 Mos 或禁用 tap 时不启用适配。事件回调不做偏好文件 IO。已知目标 PID 的应用规则可应用，未知目标采用全局/allowlist 回退。

理解 #7 中提到的多个输入软件处理顺序不稳定的问题。这是 **针对已核对 Mos 4.2.1 实现的受限方案**，不是对任意滚动软件/版本/拦截器组合的稳定兼容保证。尤其 processed-scroll marker 属于 Mos 内部实现约定，不是官方互操作 API，未来 Mos 更新可能需要重新核对。

## 脱敏诊断摘录与验证

以下摘录来自**修改后的本机测试版**，不冒充原版失效时的逐事件诊断包。省略了设备名称、设备指纹、路径、网络地址、IPC 凭据及剪贴板相关数据：

```text
Mos scroll compatibility updated active=true reverse_horizontal=true reverse_vertical=true
input permission preflights changed ax_trusted=true iohid_listen=0
permissions: accessibility=Granted, input_monitoring=Granted, local_network=Granted
```

- 原生输入模块：**422 passed，2 个原有测试 ignored**；新增 7 项 Mos 相关回归检查。
- 路由模块：**454 passed**。
- 前端：**99 个文件 / 1,455 tests passed**，类型检查及生产构建通过。
- Mac 后台、UI、CLI 以一致的 BuildIdentity 完整编译，测试版本为 **0.3.3-inhouse.101**；打包后 CLI ↔ 后台 IPC、原生窗口实际打开及隔离启动/退出检查通过。
- 测试版重新注册系统权限后，后台报告辅助功能与输入监控为 `Granted`，本地键鼠指示正常。**用户最终确认：Mos 保持开启，两种鼠标连接方向的滚轮均正常。**
- 工具链：Rust 1.99.0；前端使用锁定依赖，验证环境为 Node 26.7.0 / npm 11.19.0（本机覆写了仓库的 engine 检查）。

本次未保留包含最初 0.3.3 复现过程的完整 `manual.zip`，因此没有把修复后的当前日志包装成原始故障证据。若仍需要事件级诊断，应在明确的复现场景下另行采集。

本机测试安装包采用临时签名和独立 bundle identifier，未上传到上游；这里提供对应源码、验证结果和适配限制。

## 源码参考

- 我 fork 内部的参考 PR（不是上游 PR）：https://github.com/mikexinhuanxi/oink/pull/1
- 适配核心：https://github.com/mikexinhuanxi/oink/blob/dda8d258b875e0d96801e518cca1db2541f1558e/crates/oink-input/src/macos/mos.rs
- 采集与接收边界接入：https://github.com/mikexinhuanxi/oink/blob/dda8d258b875e0d96801e518cca1db2541f1558e/crates/oink-input/src/macos/tap.rs
- 适配范围和本机验证说明：https://github.com/mikexinhuanxi/oink/blob/dda8d258b875e0d96801e518cca1db2541f1558e/docs/mos-compatibility.md
- 核对的 Mos 4.2.1 源码：
  - https://github.com/Caldis/Mos/blob/0ff06a4e787abd5d8568e31865481e75abe550b4/Mos/ScrollCore/ScrollCore.swift
  - https://github.com/Caldis/Mos/blob/0ff06a4e787abd5d8568e31865481e75abe550b4/Mos/ScrollCore/ScrollUtils.swift

这份反馈和参考代码希望补充现有问题的定位依据，不要求维护者直接合并或审阅 fork PR。如果项目继续建议关闭外部滚动软件，也希望上述复现对照能帮助遇到同类问题的用户判断原因。
