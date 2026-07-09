# 开源 Token 路由系统全景：三个层次、可借鉴什么、明确不选什么

> 场景：继 [OmniRoute 分析](./omniroute-开源系统借鉴分析.md) 之后，对开源 token/LLM 路由生态做一次全景扫描，回答「还有哪些系统可以借鉴」。
> 核心结论：**没有任何一个开源系统能整体覆盖我们的三层架构，但每一层都有明确的「首选借鉴对象」：推理平面直接采用 SGL Model Gateway（零改造契合 SGLang 双副本）；质量侧路由整体复用 vLLM semantic-router 的形态与训练管道；计费网关不整体采用任何开源，抄 LiteLLM 的七层预算模型、Bifrost 的健康分负载均衡与「计费不进关键路径」、new-api 的倍率计费体系。另有四个死亡/变故案例（TensorZero、Helicone、Portkey、RouteLLM）给出的教训与我们的自研决策互为印证。**
>
> 调研日期 2026-07-05（所有 star 数、活跃度、许可证均为当日 GitHub API 实测）。配套：[glm-5.2-计费接入架构.md](../../../glm5.2-platform/docs/architecture/glm-5.2-计费接入架构.md)、[glm-5.2-智能路由能力.md](../features/glm-5.2-智能路由能力.md)、[glm-5.2-部署架构.md](../../../glm5.2-platform/docs/architecture/glm-5.2-部署架构.md)、[omniroute-开源系统借鉴分析.md](./omniroute-开源系统借鉴分析.md)。
> 详细调研资料（每系统的完整事实清单与来源）：[层A·计费网关](../../../glm5.2-platform/docs/research/开源token路由调研-层A-计费网关详细报告.md) · [层B·学习型路由器](./开源token路由调研-层B-学习型路由器详细报告.md) · [层C·推理平面](../../../glm5.2-platform/docs/research/开源token路由调研-层C-推理平面详细报告.md)。

---

## 零、先对齐坐标系：三个层次的「路由」

「token 路由」在开源生态里其实是三个互不替代的问题，对应我们架构的三段：

| 层 | 问题 | 我们架构中的位置 | 本文对应章节 |
|----|------|----------------|------------|
| **A. 计费/多租户网关** | 谁能调、扣谁的钱、满了怎么办 | 腾讯云 APISIX/Higress + 结算引擎 | 第一节 |
| **B. 质量侧智能路由** | 这个 query 该用哪个模型 | 四层路由（规则>hint>学习型>级联） | 第二节 |
| **C. 推理平面路由** | 定了模型，派给哪个 GPU 副本 | IDC 推理网关/Router | 第三节 |

（OmniRoute 所在的「供给侧路由」——模型已定、派给哪个供应商/key——见 [前文](./omniroute-开源系统借鉴分析.md)，本文不重复。）

---

## 一、层 A：计费 / 多租户网关

层 A 属**计费接入平面**，落在独立项目 glm5.2-platform。此处只留结论摘要，完整格局对比（LiteLLM / new-api / Bifrost / Higress 等八系统）、可抄设计与死亡案例教训见 glm5.2-platform 的 [开源token路由调研-层A-计费网关详细报告](../../../glm5.2-platform/docs/research/开源token路由调研-层A-计费网关详细报告.md)。

- **首选借鉴对象**：LiteLLM 的七层预算模型 + `budget_duration` 自动重置（映射四种 plan.type）、Bifrost 的连续健康分 adaptive LB + 「计费不进关键路径」（验证 Redis 预扣 + CKafka 异步落账）、new-api 的倍率计费词汇表（含缓存倍率，扩展 ASR/TTS）、Higress 的 ai-quota 插件直接扛准入层粗粒度配额。
- **不整体采用任何开源计费网关**：TensorZero/Helicone/Portkey 的死亡与变故印证「计费/配额必须是网关一等公民、不能外包」，支持自研结算引擎的决策。
- **与路由器的接口**：LiteLLM 的三类语义化 fallback 分离（容量型 vs 能力型失败）启示——glm5.2-platform 的内部溢出决策要区分失败类型，这与 token-station 的质量侧路由是两套机制。

---

## 二、层 B：质量侧智能路由（四层框架的弹药库）

### 2.1 格局一览

| 项目 | Stars | 许可证 | 状态 | 方法 | 需 GPU |
|------|-------|--------|------|------|--------|
| [vLLM semantic-router](https://github.com/vllm-project/semantic-router) | 4.8k | Apache-2.0 | 极活跃（v0.1 "Iris"） | ModernBERT 多信号分类 + 多任务 LoRA | **否**（Rust/Candle CPU） |
| [plano（原 archgw）](https://github.com/katanemo/plano) | 6.6k | 网关 Apache-2.0 / **模型 Research License** | 活跃 | Arch-Router 1.5B/4B 生成式偏好对齐 | CPU 量化可行 |
| [RouteLLM](https://github.com/lm-sys/RouteLLM) | 5.1k | Apache-2.0 | **停滞 2 年** | MF/BERT/Elo，二元强弱 + 阈值旋钮 | 部分否 |
| [LLMRouter (UIUC)](https://github.com/ulab-uiuc/LLMRouter) | 2.1k | MIT | 活跃 | 16+ 算法库（KNN→GraphRouter→Router-R1） | 多数否 |
| [NVIDIA llm-router](https://github.com/NVIDIA-AI-Blueprints/llm-router) | 0.3k | Apache-2.0 | 活跃 | DeBERTa 任务+复杂度双分类 | 是（Triton） |
| [aurelio semantic-router](https://github.com/aurelio-labs/semantic-router) | 3.7k | MIT | 活跃 | 嵌入 + 示例语句意图匹配 | 否，<100ms |
| [RouterBench](https://github.com/withmartian/routerbench) | 0.2k | MIT | 数据集仍有效 | 405K 条多模型评测结果 | — |

Not Diamond、Azure Model Router 均闭源（OpenRouter 的 Auto Router 即 Not Diamond 驱动），对 BYOK 模式无用。

### 2.2 对四层路由的逐层借鉴

**第 1 层（用户规则）**：
- **Arch-Router 的 Domain-Action YAML 范式是目前最好的「用户自定义规则」交互设计**——用户用自然语言描述路由策略（`bug_fixing: "修 bug 和报错分析"` → 映射到模型），加新规则零重训。即使不用它的模型（Research License，商用内嵌需授权），**配置 schema 值得照抄**，正好落在 [智能路由能力.md](../features/glm-5.2-智能路由能力.md) ① 层「声明式规则」的升级版；
- **aurelio semantic-router 补语义匹配**：用户给几句示例话术即成一条 route，本地嵌入免 GPU、MIT、无外呼——BYOK 隐私模式可直接内嵌进开源客户端，比正则鲁棒。

**第 2 层（客户端 hint）**：
- 借鉴 plano 的**模型语义别名**：客户端传 `model: fast/smart/cheap` 而非具体模型 ID，平台侧解析——这同时是套餐差异化的载体（不同套餐对同一别名解析出不同档位）。

**第 3 层（学习型路由）——本次调研最大收获**：
- **vLLM semantic-router 就是我们规划中「第 3 阶段学习型路由」的现成工程载体**：Apache-2.0、ModernBERT 分类器 CPU 毫秒级推理（**不破 BYOK 隐私，可蒸馏分发给开源客户端**）、Envoy ext_proc 部署形态与我们网关兼容、训练管道开源（可用自有流量重训分类头）、还内置 PII/越狱检测和语义缓存。相比之下 RouteLLM 已停维护，只保留其「单阈值旋钮调成本比例」的产品思想（套餐参数化）；
- 它的**推理模式开关路由**（判断 query 是否需要 CoT，token -48%）对 GLM-5.2 这类混合推理模型是独有的省钱杠杆——我们自营模型可控制 thinking 开关，这个信号纯 API 转售商用不了；
- **离线选型用 LLMRouter + RouterBench**：405K 条现成评测数据 + 16 种算法横评，不烧 API 费就能确定生产算法。

**第 4 层（级联）**：
- **AutoMix**（NeurIPS'24，LLMRouter 内有实现）：小模型先答 + 自校验，低置信升级——级联层的 SOTA 开源实现，与我们「自营 GLM logprob 信号」的优势叠加；
- 借鉴 vLLM SR 的**反馈信号回流**：级联触发记录（小模型答错被升级）直接成为第 3 层分类器的训练标签——这就是智能路由文档里「数据飞轮」的具体实现路径，且只用元数据不碰内容。

---

## 三、层 C：推理平面路由（IDC 内 2×8×H200）

层 C 属**推理部署平面**，落在独立项目 glm5.2-platform。此处只留结论摘要，完整格局对比（SGL Model Gateway / Dynamo / llm-d / AIBrix / GIE 五系统）与逐条落地建议见 glm5.2-platform 的 [开源token路由调研-层C-推理平面详细报告](../../../glm5.2-platform/docs/research/开源token路由调研-层C-推理平面详细报告.md)。

- **直接采用 SGL Model Gateway**（原 sgl-router，Apache-2.0、原生 SGLang、单 Rust 二进制）替换自研推理网关的 LB 部分，副本级 LB / 健康检查 / 重试熔断全下沉，自研 Router 收缩为「水位上报 + 与云上网关对接」。
- **溢出信号标准化**：以 `/workers` load/health + 各节点 `sglang_num_queue_reqs` 队列深度作为第一信号，与 glm5.2-platform 的 [计费接入架构](../../../glm5.2-platform/docs/architecture/glm-5.2-计费接入架构.md) §3.1 的队列水位双阈值对接——这是层 C（glm5.2-platform）与 token-station 之间的**契约接口**。
- **cache_aware 先测后信**：官方 1.9x 吞吐是 10+ worker 测的，2 副本收益打对折，用真实流量重放对比再定；Dynamo/GIE 的三信号打分协议列入观察名单。

---

## 四、汇总：借鉴落点表

| 借鉴点 | 来源 | 落入文档/模块 |
|--------|------|--------------|
| 七层预算 + budget_duration 周期重置 + soft_budget | LiteLLM | 结算引擎（plan.type 统一原语）|
| 容量型/能力型失败分离的 fallback 链 | LiteLLM | 计费接入架构 §3 溢出决策 |
| 连续健康分 adaptive LB + 自动回流 | Bifrost | 网关溢出模块（替代二值禁用）|
| 内存扣减 + 异步落账（验证既定设计） | Bifrost | Redis 预扣 + CKafka（保持不变）|
| 倍率计费词汇表 + 缓存倍率 + 按次计费 | new-api | rate_plan 扩展（ASR/TTS/缓存折扣）|
| ai-quota / ai-token-ratelimit 插件 | Higress | 网关准入层粗粒度配额 |
| 自然语言路由规则 YAML schema | Arch-Router/plano | 四层路由第 1 层 |
| 示例话术语义匹配（本地、免 GPU） | aurelio semantic-router | 第 1 层 + 开源客户端内嵌 |
| 模型语义别名（fast/smart/cheap） | plano | 第 2 层 + 套餐差异化 |
| ModernBERT 分类器 + 开源训练管道 + 推理开关路由 | vLLM semantic-router | 第 3 层的工程载体 |
| 单阈值成本旋钮（思想） | RouteLLM | 套餐参数 |
| AutoMix 自校验级联 + 反馈标签回流 | LLMRouter / vLLM SR | 第 4 层 + 数据飞轮 |
| 离线算法横评（RouterBench 405K 数据） | RouterBench + LLMRouter | 第 3 层选型实验 |
| cache_aware LB + 队列水位信号 + PD 分离预留 | SGL Model Gateway | IDC 推理网关（直接采用）|
| queue/KV/前缀三信号打分协议 | GIE | 自研网关的水位上报协议 |

## 五、明确不选清单

| 项 | 原因 |
|----|------|
| fork new-api / one-api 代码 | AGPL 传染（new-api）+ 停更（one-api）；只抄计费词汇表 |
| TensorZero / Helicone ai-gateway / RouteLLM 作依赖 | 均已死亡或停滞 |
| Portkey Gateway 作依赖 | 收购后维护不明；只抄 conditional routing 条件表达式与 Usage Policies 设计 |
| Kong AI Gateway | token 级限流锁企业版 |
| Envoy AI Gateway / llm-d / AIBrix | 强绑 K8s（且后两者 vLLM 中心），当前 2 节点裸机不适用 |
| Arch-Router 模型权重直接内嵌 | Research License，商用需向 Katanemo 确认授权；schema 可抄 |
| NVIDIA llm-router | 需 Triton + N 卡跑分类器，与「路由器必须轻量/CPU」冲突 |

---

## 参考

**层 A**：[LiteLLM](https://github.com/BerriAI/litellm) · [new-api](https://github.com/QuantumNous/new-api) · [one-api](https://github.com/songquanpeng/one-api) · [done-hub](https://github.com/deanxv/done-hub) · [uni-api](https://github.com/yym68686/uni-api) · [Bifrost](https://github.com/maximhq/bifrost) · [Portkey Gateway 2.0](https://portkey.ai/blog/gateway-2-0/) · [Higress ai-quota](https://higress.cn/en/docs/latest/plugins/ai/api-consumer/ai-quota/) · [TensorZero 停运（HN）](https://news.ycombinator.com/item?id=48518120)
**层 B**：[vLLM semantic-router](https://github.com/vllm-project/semantic-router) · [When to Reason 论文](https://arxiv.org/abs/2510.08731) · [plano / Arch-Router](https://github.com/katanemo/plano) · [Arch-Router 论文](https://arxiv.org/abs/2506.16655) · [LLMRouter](https://github.com/ulab-uiuc/LLMRouter) · [RouterBench](https://github.com/withmartian/routerbench) · [aurelio semantic-router](https://github.com/aurelio-labs/semantic-router) · [RouteJudge 路由方法分类学](https://arxiv.org/pdf/2606.18774)
**层 C**：[SGL Model Gateway](https://github.com/sgl-project/sglang/tree/main/sgl-model-gateway) · [SGLang v0.4 cache-aware LB](https://www.lmsys.org/blog/2024-12-04-sglang-v0-4/) · [NVIDIA Dynamo](https://github.com/ai-dynamo/dynamo) · [llm-d](https://github.com/llm-d/llm-d) · [AIBrix](https://github.com/vllm-project/aibrix) · [Gateway API Inference Extension](https://github.com/kubernetes-sigs/gateway-api-inference-extension)
