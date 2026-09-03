# Token Station — 25 枚 Logo 独立筛选矩阵

评审日期：2026-08-29

## 结论先行

**Champion：D03-05 Fallback Switch。** 它是 25 枚里对“主路由 / 备用路由切换”表达最直接、单色后仍有独立轮廓、且没有退化为圆角 App tile 的方案。它仍不能原样上线：16 px 时中央负形接近 1 个像素，且存在“台阶 / 座椅 / F”误读。

**Runner-up：D05-04 Relay Flag。** 它的工业控制语义和竞品距离最好，开放轮廓也适合托盘图标；主要问题是单色时仍可能先读成 `E / h / 符文 / 铁路信号臂`。

严格结论：**本轮没有 Production-ready 方案。** Champion 和 Runner-up 是最值得进入一次定向精修的两个母形，不是可直接替换正式图标的成品。

## 评审方法

评审分三轮完成：

1. 先将 25 枚全部去色为纯黑，在白底按 32 px 和 16 px 重渲染，只判断轮廓、负形和像素生存；
2. 再恢复配色，判断“被选择的路由 / 本地控制 / 数据基础设施”是否成立；
3. 最后按 [research.md](./research.md) 中已核验的竞品语汇，复查 OpenRouter 字母构成、CC Switch 中心放射、LiteLLM 火车、Portkey 门户箭头等邻近风险。

这是一轮作者无关、产物优先的设计评审，不是商标清查，也不是用户识别测试。

### 计分口径

五项均为 1–5 分，分数越高越好，总分 25 分，等权：

- **轮廓**：单色轮廓的独特性与可拥有性；
- **小尺寸**：16–32 px 生存能力；
- **产品**：对本地路由、控制、基础设施的贴合度；
- **竞品距**：与已知竞品及拥挤语汇的距离；
- **误读安全**：5 = 误读风险低，1 = 误读风险高。

同分时依次看：产品贴合 → 误读安全 → 小尺寸 → 竞品距离。明显的字母、常见 UI 符号或“大面积近方形 App tile”会触发推荐降级；方形占比已计入轮廓与误读安全，不另设第六项。

## 完整矩阵

| 排名 | 候选 | 轮廓 | 小尺寸 | 产品 | 竞品距 | 误读安全 | 总分 | 最主要风险 |
|---:|---|---:|---:|---:|---:|---:|---:|---|
| 1 | [D03-05 Fallback Switch](./direction-03-dual-track-exchange/d03-05-fallback-switch.svg) | 4 | 4 | 5 | 5 | 3 | **21** | 单色后两条路线合成台阶状块面；16 px 中央孔过小，可能读成 F、座椅或阶梯。 |
| 2 | [D05-04 Relay Flag](./direction-05-signal-control/d05-04-relay-flag.svg) | 4 | 4 | 5 | 5 | 2 | **20** | 控制语义强，但仍容易先读成 E、h、符文或铁路信号臂。 |
| 3 | [D01-05 Signal Stitch](./direction-01-switch-cut/d01-05-signal-stitch.svg) | 4 | 4 | 4 | 4 | 3 | **19** | 横带与斜梁容易被读成铁轨枕木、快进箭头或速度线，且横向比例偏长。 |
| 4 | [D04-01 Perforated Unit](./direction-04-token-rack/d04-01-perforated-unit.svg) | 4 | 4 | 4 | 5 | 2 | **19** | 大块填充接近方形 App tile；去色后两侧切口会合成 Z、2、E 或卡片/接口联想。 |
| 5 | [D04-02 Offset Cartridges](./direction-04-token-rack/d04-02-offset-cartridges.svg) | 3 | 4 | 5 | 4 | 2 | **18** | 机架语义成立，但轮廓接近树枝、天线或铁路信号杆；16 px 时模块层次变弱。 |
| 6 | [D01-01 Relay Slash](./direction-01-switch-cut/d01-01-relay-slash.svg) | 3 | 4 | 4 | 4 | 3 | **18** | 容易成为 H、暂停栏或两根铁轨；也与旧正式图标的斜轨 DNA 过近，新鲜度有限。 |
| 7 | [D05-01 Signal Break](./direction-05-signal-control/d05-01-signal-break.svg) | 3 | 3 | 4 | 5 | 3 | **18** | 过度还原后在 16 px 只剩两枚普通短横，识别资产不足，像减号或断开的道路标线。 |
| 8 | [D03-03 Relay Pair](./direction-03-dual-track-exchange/d03-03-relay-pair.svg) | 3 | 5 | 4 | 4 | 2 | **18** | 第一眼是 H、暂停键、磁铁或扣件；紧凑方形占比也偏高。 |
| 9 | [D04-04 Packet Relay](./direction-04-token-rack/d04-04-packet-relay.svg) | 3 | 4 | 4 | 5 | 2 | **18** | 去色后变成三块像素台阶，容易落入 Tetris、积木或传送带图标。 |
| 10 | [D04-05 Meter Cut](./direction-04-token-rack/d04-05-meter-cut.svg) | 3 | 5 | 4 | 5 | 1 | **18** | 16 px 虽清楚，但清楚地读成大写 E 或梳齿；大面积块面也重新回到 App tile 问题。 |
| 11 | [D01-04 Crossing Gate](./direction-01-switch-cut/d01-04-crossing-gate.svg) | 2 | 5 | 4 | 4 | 2 | **17** | 单色后几乎就是 X；可误读为工具、刀剑、交叉路口或关闭符号。 |
| 12 | [D02-02 Private Port](./direction-02-local-aperture/d02-02-private-port.svg) | 2 | 5 | 4 | 4 | 2 | **17** | 局部边界概念成立，但主体像 n、U、门洞、房屋或锁，且块面接近方形底板。 |
| 13 | [D05-03 Detent Lever](./direction-05-signal-control/d05-03-detent-lever.svg) | 3 | 3 | 4 | 5 | 2 | **17** | 齿口在 16 px 合并，容易成为锯齿、梳子、尺、齿条或微型铁轨。 |
| 14 | [D05-02 Gate Latch](./direction-05-signal-control/d05-02-gate-latch.svg) | 2 | 5 | 4 | 5 | 1 | **17** | 极强的大写 L / 裁切角 / 对齐工具读法压过“门闩”语义。 |
| 15 | [D03-04 Platform Transfer](./direction-03-dual-track-exchange/d03-04-platform-transfer.svg) | 3 | 5 | 3 | 4 | 2 | **17** | 小尺寸稳定，但更像楼梯、长凳、椅子或 Tetris 块，缺少本地控制指向。 |
| 16 | [D01-02 Switch Wedge](./direction-01-switch-cut/d01-02-switch-wedge.svg) | 3 | 5 | 4 | 2 | 2 | **16** | 三向中心放射接近分享、螺旋桨与 CC Switch 的中心爆发语汇。 |
| 17 | [D01-03 TS Cut](./direction-01-switch-cut/d01-03-ts-cut.svg) | 3 | 4 | 3 | 4 | 2 | **16** | 负形没有稳定读成路由，反而容易成为 F、TS 字母拼合或未知字体字形。 |
| 18 | [D05-05 Quiet Beacon](./direction-05-signal-control/d05-05-quiet-beacon.svg) | 4 | 3 | 3 | 5 | 1 | **16** | 单色和 16 px 时高度趋近 8、B、磁带盒或电池；整体紧凑方块化，信号语义不足。 |
| 19 | [D02-04 Boundary Slot](./direction-02-local-aperture/d02-04-boundary-slot.svg) | 2 | 5 | 4 | 3 | 1 | **15** | 大面积圆角方块首先是 G、软盘、文件夹或通用 App tile，槽口无法扭转主读法。 |
| 20 | [D02-05 Kernel Fold](./direction-02-local-aperture/d02-05-kernel-fold.svg) | 3 | 4 | 3 | 2 | 2 | **14** | 形态接近返回箭头、链环、回形针与 Portkey 类门户/箭头语汇。 |
| 21 | [D02-01 Loopback Aperture](./direction-02-local-aperture/d02-01-loopback-aperture.svg) | 2 | 5 | 3 | 3 | 1 | **14** | 几乎不可避免地读成 Q、G、刷新环或圆角 App tile；局部色块太小，无法建立独占性。 |
| 22 | [D02-03 Contained Flow](./direction-02-local-aperture/d02-03-contained-flow.svg) | 2 | 5 | 3 | 3 | 1 | **14** | C / G / 括号式容器读法过强，仍是上一轮最需要避免的“圆角方块 + 小色块”。 |
| 23 | [D03-01 Square Crossover](./direction-03-dual-track-exchange/d03-01-square-crossover.svg) | 2 | 5 | 4 | 1 | 1 | **13** | 第一眼是全屏、展开/收起或四箭头 UI 图标，并靠近 CC Switch 的中心放射结构。 |
| 24 | [D03-02 Offset Exchange](./direction-03-dual-track-exchange/d03-02-offset-exchange.svg) | 2 | 5 | 4 | 1 | 1 | **13** | 第一眼是加载旋转器、风扇、四向交换或中心爆发，品牌独占空间不足。 |
| 25 | [D04-03 Slot Register](./direction-04-token-rack/d04-03-slot-register.svg) | 2 | 2 | 3 | 4 | 1 | **12** | 16 px 直接碎成条码、柱状图、暂停栏或像素噪声；既不稳定也不独占。 |

## Top 7

| 顺位 | 候选 | 保留理由 | 进入下一轮前必须解决的问题 |
|---:|---|---|---|
| 1 | **D03-05 Fallback Switch** | 产品含义、开放轮廓、小尺寸和竞品距离最均衡。 | 放大中央负形；去除 F / 台阶 / 座椅读法；确认单色时仍像两路切换。 |
| 2 | **D05-04 Relay Flag** | 最像“本地控制硬件”，开放且远离主流 AI logo 套路。 | 打破 E / h / 铁路信号臂；保留非对称控制瞬间。 |
| 3 | **D01-05 Signal Stitch** | 有明确的“多路被本地横带统一控制”语义，轮廓不方块化。 | 去除铁轨、快进箭头和速度线联想。 |
| 4 | **D04-01 Perforated Unit** | 数据单元 / 插槽 / 本地基础设施语义最完整。 | 降低大块方形占比，避免单色时变成 Z / 2 / E / 接口卡。 |
| 5 | **D04-02 Offset Cartridges** | “本地机架管理多个模型/提供商”的隐喻清楚。 | 去树形、天线和铁路信号杆读法，强化共享主脊而非分支。 |
| 6 | **D01-01 Relay Slash** | 与现有品牌迁移成本最低，选路动作清楚。 | 必须显著摆脱 H / 暂停栏 / 双轨和旧图换形感。 |
| 7 | **D05-01 Signal Break** | 极简、开放、竞品距离大，具备做托盘图标的潜力。 | 增加不依赖颜色的独有结构，否则 16 px 只剩两个减号。 |

## Bottom 5

| 候选 | 淘汰原因 |
|---|---|
| **D04-03 Slot Register** | 16 px 崩解，条码/图表/暂停栏误读同时存在。 |
| **D03-02 Offset Exchange** | 中心放射与旋转器/风扇是拥挤语汇，和 CC Switch 距离不足。 |
| **D03-01 Square Crossover** | 基本等同于全屏/展开 UI 符号，无法形成品牌资产。 |
| **D02-03 Contained Flow** | 圆角方块与 C/G 字母占据第一读法，重现旧方案问题。 |
| **D02-01 Loopback Aperture** | Q/G/刷新环和通用 App tile 误读过强，局部蓝色无法补救。 |

## 五个方向的独立判断

| 方向 | 判断 |
|---|---|
| D1 切轨印记 | 最容易延续现有资产，但“铁轨 / 箭头 / H / X”风险集中；只有 05 和 01 值得继续。 |
| D2 本地孔径 | 方向假设成立，造型执行失败最系统性：大量圆角近方形、C/G/Q 和安全软件/通用 App tile 读法。 |
| D3 双轨换线 | 产品语义最强，但 01/02 掉入中心放射与 UI 符号；05 是明显例外，也是本轮冠军。 |
| D4 令牌机架 | 本地基础设施感最好，但容易变成 E、条码、图表、像素阶梯和厚重方块；01/02 可作次级母形。 |
| D5 精密信号 | 竞品距离整体最好，但工业构件很容易被读成 L/E/h、齿条、铁路信号或符文；04 最值得精修。 |

## 下一步决策门槛

只建议精修 **D03-05** 与 **D05-04**，不要继续平均打磨 25 枚：

1. 各做 3 个只改变负形和端部关系的变体，不增加颜色、节点或装饰；
2. 先以纯黑 16 px 过关，再看双色版本；
3. 在不展示名称的情况下做 5 秒识别测试，记录第一联想是否为路由/控制/基础设施；
4. 对冠军候选再做正式的图形近似检索与商标清查；本矩阵不能替代法律结论。

