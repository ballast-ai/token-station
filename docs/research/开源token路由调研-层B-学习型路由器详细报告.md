# 调研详细报告 · 层 B：学习型 / 语义 LLM 路由器

> 本文是 [开源token路由系统全景借鉴.md](./开源token路由系统全景借鉴.md) 第二节的原始调研资料，保留全部事实细节与来源链接。调研日期 2026-07-05。
> 覆盖：vLLM semantic-router · plano（原 archgw）/Arch-Router · RouteLLM · LLMRouter (UIUC) · NVIDIA llm-router · Not Diamond · Azure Model Router · RouterBench · RouteJudge · aurelio semantic-router · 语义缓存类。

---

## 一、vllm-project/semantic-router（vLLM 社区语义路由器）

- **URL**: https://github.com/vllm-project/semantic-router
- **Star**: 4,785 | **语言**: Go（分类核心为 Rust/HF Candle）| **许可证**: Apache-2.0
- **活跃度**: 极高。2025-08 创建，最近 push 2026-07-05（当天），2026-01 发布首个正式版 v0.1 "Iris"

**路由方法论**
- 核心是 **ModernBERT 微调分类器**（非生成式 LLM）：意图/领域分类（MMLU-Pro 约 12K 样本、14 个学科域）+ token 级 PII 检测（Microsoft Presidio 约 50K 样本）+ 越狱检测（公开 jailbreak 数据集）
- v0.1 Iris 架构：**六类信号**（领域分类、关键词正则、嵌入相似度、事实性/幻觉、用户反馈、偏好）汇入可配置决策引擎，插件式挂载语义缓存、越狱检测、PII 防护
- 与 HuggingFace 合作引入**多任务 LoRA**：多个分类任务共享一次 base model 前向，O(n) 降为 O(1)+轻量 adapter
- 特色：**推理模式开关路由**（论文 arXiv:2510.08731 "When to Reason"）——判断查询是否需要 CoT/推理模式，MMLU-Pro 准确率 +10.2pp、延迟 -47.1%、token 消耗 -48.5%

**部署形态**: Envoy **ext_proc gRPC filter**（拦截请求、并行评估信号、改写 header 路由）；也可 `pip install vllm-sr` 独立跑或 Helm 上 K8s。与 vLLM 引擎解耦，官方集成 vLLM production-stack、llm-d、NVIDIA Dynamo、Envoy AI Gateway
**GPU/延迟**: **不需要专用 GPU**——Rust+Candle CPU 推理；配套论文（arXiv:2603.12646）专门做了「无专用 GPU 下 98 倍路由加速」（flash attention + prompt 压缩），路由延迟毫秒级
**训练数据**: MMLU-Pro、Presidio、公开 jailbreak 数据集，**管道开源可自行重训**

---

## 二、katanemo/archgw → 已改名 katanemo/plano（Arch / Arch-Router）

- **URL**: https://github.com/katanemo/plano（archgw 旧地址 301 重定向）
- **Star**: 6,616 | **语言**: Rust | **许可证**: 网关 Apache-2.0；**Arch-Router-1.5B 模型是 Katanemo Research License（非纯开源，商用需注意）**
- **活跃度**: 高，最近 push 2026-06-29

**路由方法论：偏好对齐路由（preference-aligned routing）**
- **Arch-Router-1.5B**（基座 Qwen2.5-1.5B-Instruct，arXiv:2506.16655）：用户在 YAML 里用**自然语言描述** Domain-Action 路由策略（如 `code_generation`、`bug_fixing` + 描述），模型读 prompt + 路由表，输出 `{"route": "bug_fixing"}`，配置把 route 映射到模型
- 关键卖点：**加新模型/新规则只改配置，零重训**；路由决策可解释、可由用户口语化定义。论文报告 93.17% 路由准确率，超 GPT-4o/Claude 约 7pp
- 现默认路由模型已升级为 `plano_orchestrator_v1`（4B）
- 附加能力：函数调用/意图分类（Arch-Function 系列）、guardrail filter chain、OpenTelemetry

**部署形态**: 基于 **Envoy 的 AI-native 数据面**（核心贡献者出自 Envoy 团队），Rust filter
**GPU/延迟**: 需跑 1.5B/4B LLM——有 GGUF/Ollama/llama.cpp 量化版，**CPU 边缘部署可行**，GPU 上几十 ms；比 BERT 类分类器重、比 GPT-4 做路由轻得多
**训练数据**: 会话式路由数据集（论文构建，未完全公开；社区 archdata 项目复刻其格式）

---

## 三、lm-sys/RouteLLM

- **URL**: https://github.com/lm-sys/RouteLLM
- **Star**: 5,138 | **语言**: Python | **许可证**: Apache-2.0
- **活跃度**: **已停滞——最后 push 2024-08-10，近两年无提交**，事实上无人维护

**路由方法论**（4 种路由器，二元强/弱模型选择 + 阈值调节成本比例）
1. **矩阵分解**（官方推荐，效果最好）：学 query-model 匹配的隐因子打分
2. **BERT 分类器**：预测哪个模型会赢
3. **相似度加权 Elo 排名**（SW Ranking）：与训练样本嵌入相似度加权 Elo，**免 GPU** 但单次推理最贵
4. **因果 LLM**（Llama-3-8B）：泛化最好、开销最大

**训练数据**: Chatbot Arena 人类偏好对 + GPT-4-judge 合成增强。MT-Bench 上省 85% 成本保 95% GPT-4 质量
**社区替代品**: ulab-uiuc/LLMRouter（算法库继承者）、vLLM semantic-router（生产形态继承者）、RouteJudge（评测继承者）

---

## 四、ulab-uiuc/LLMRouter

- **URL**: https://github.com/ulab-uiuc/LLMRouter
- **Star**: 2,061 | **语言**: Python | **许可证**: MIT
- **活跃度**: 活跃。2025-10 创建，最近 push 2026-05-13，PyPI `llmrouter-lib`

**支持算法清单（16+）**
- 单轮：KNNRouter、SVMRouter、MLPRouter、MFRouter（矩阵分解）、EloRouter、**RouterDC**（对偶对比学习，NeurIPS'24）、**AutoMix**（自校验级联，NeurIPS'24）、**HybridLLM**（成本-质量感知，ICLR'24）
- 图神经网络：**GraphRouter**（ICLR'25）、GMTRouter（多轮交互个性化）、PersonalizedRouter（图上的用户偏好建模）
- 多轮/Agent：**Router-R1**（RL 多轮路由+聚合，NeurIPS'25，需 GPU）、LLMMultiRoundRouter
- 基线：SmallestLLM / LargestLLM / CausalLMRouter

**训练数据**: 内置管道从 11 个 benchmark 生成（MMLU、GPQA、GSM8K、MATH、HumanEval、MBPP、TriviaQA 等），支持多模态
**GPU**: 大部分方法 CPU 可跑，Router-R1 需 GPU（vllm 0.6.3）
**部署**: 定位偏研究库，但附带 OpenAI 兼容 API server、流式响应、多 provider——可轻量生产

---

## 五、其他 2025-2026 值得注意的项目

### NVIDIA-AI-Blueprints/llm-router
- https://github.com/NVIDIA-AI-Blueprints/llm-router | 316 star | Apache-2.0 | 最近 push 2026-05，活跃
- 方法：**双分类策略**——任务分类（代码生成/开放问答/改写…）+ 复杂度分类（推理/创意/领域知识…），用 `nvidia/prompt-task-and-complexity-classifier`（DeBERTa 系），支持自定义微调
- 架构：Rust 路由控制器（OpenAI 兼容代理）+ **Triton 推理服务器**跑分类器
- **需要 GPU**（默认 V100/4GB 起，自训 A10G/24GB），CUDA 12.2+。Docker Compose / Helm

### Not Diamond
- **路由器本体闭源**（SaaS），未开源权重。开源仅 SDK（https://github.com/Not-Diamond/notdiamond-python，92 star）和精选列表 https://github.com/Not-Diamond/awesome-ai-model-routing
- 现状：仍在运营，转向 coding agent 路由；**OpenRouter 的 "Auto Router" 由 Not Diamond 驱动**

### Microsoft / Azure Foundry Model Router
- **闭源**。Azure 上的「可部署路由模型」，仅开源示例代码。**不可自托管**，对 BYOK 隐私模式无用

### RouterBench（评测基准）
- https://github.com/withmartian/routerbench | 167 star | MIT | 代码 2024-06 后不更新，数据集仍是标准基准
- **405,000+ 条推理结果**（多模型 × 多 benchmark），HF 数据集 `withmartian/routerbench`。可离线训练/评测自己的路由器而无需烧 API 费

### RouteJudge（2026-06 新出）
- arXiv:2606.18774，https://github.com/LAMDA-Model-Reuse/RouteJudge（刚发布）
- 价值在分类学：把路由方法分为**规则/回归/分类/排名/图/奖励模型/级联/非参数**八族并可复现对比，选型时值得读论文

### 语义缓存类
- zilliztech/GPTCache（8,085 star，MIT）：**半停滞**（最后 push 2025-07）；codefuse-ai/ModelCache（942 star，蚂蚁系）替代。注意 vLLM semantic-router **已内置语义缓存**，无需另拼

---

## 六、aurelio-labs/semantic-router（用于「用户自定义规则层」）

- https://github.com/aurelio-labs/semantic-router | 3,666 star | Python | MIT | 活跃（2026-05）
- 方法：定义 `Route` + 若干示例 utterances → 嵌入 → 来询与各 route 相似度匹配 + 可自动调优的阈值。**不是 LLM 选型路由，是意图路由库**
- **完全本地可跑**：HuggingFaceEncoder/FastEmbed/llama.cpp，无外呼；**sub-100ms**，免 GPU
- **结论：很适合第一层**——用户用几句示例定义「这类请求走 X 模型」，比正则鲁棒、比训练分类器零成本，每条 route 可单独调阈值（不匹配落到下一层）

---

## 七、对比表

| 项目 | Star | 许可证 | 活跃 | 路由方法 | 需 GPU | 路由延迟 | 本地/边缘 |
|---|---|---|---|---|---|---|---|
| vLLM semantic-router | 4,785 | Apache-2.0 | ★★★ | ModernBERT 多信号分类 + LoRA 多任务 | **否**（Rust/Candle CPU） | 毫秒级 | ✅ 优 |
| plano (原 archgw) | 6,616 | 网关 Apache-2.0 / 模型 Research License | ★★★ | Arch-Router 1.5B/4B 生成式偏好对齐 | 推荐（CPU GGUF 可行） | 几十 ms（GPU） | ✅ 可（量化） |
| RouteLLM | 5,138 | Apache-2.0 | ✝ 停滞 2 年 | MF / BERT / SW-Elo / 因果 LLM，二元强弱 | 部分方法否 | 视方法 | ✅ |
| LLMRouter (UIUC) | 2,061 | MIT | ★★ | 16+ 算法（KNN→GraphRouter→Router-R1） | 多数否 | 视方法 | ✅ |
| NVIDIA llm-router | 316 | Apache-2.0 | ★★ | DeBERTa 任务+复杂度双分类 | **是**（Triton） | 低 | ⚠️ 需 N 卡 |
| Not Diamond | — | 闭源 | 运营中 | 学习型（细节不公开） | — | API 调用 | ❌ |
| Azure model router | — | 闭源 | 运营中 | 训练的路由 LM | — | — | ❌ |
| RouterBench | 167 | MIT | 停滞（数据集有效） | 评测基准，405K 结果 | 否 | — | ✅ |
| aurelio semantic-router | 3,666 | MIT | ★★ | 嵌入 + 示例语句 route 匹配 | 否 | <100ms | ✅ 优 |

---

## 八、对四层路由架构的可借鉴点

**第 1 层（用户规则）**
- 直接用 **aurelio semantic-router**：每条规则几句示例话术即成 Route，本地嵌入免 GPU，MIT，BYOK 隐私无外呼
- 借鉴 **Arch-Router 的 Domain-Action YAML 配置范式**：用户用自然语言描述规则而非写正则/示例集——即使不用它的模型，配置 schema 值得抄
- 借鉴 vLLM SR 的 **keyword 正则信号**作为规则层兜底精确匹配

**第 2 层（客户端 hint）**
- 借鉴 vLLM SR 的 **Envoy ext_proc header 改写**机制：hint 走 header，路由层只 mutate header 不动 body，与网关解耦
- 借鉴 plano 的**模型语义别名**设计：客户端传 `model: fast/smart/cheap`，平台侧解析

**第 3 层（学习型路由）**
- **首选整体复用 vLLM semantic-router**：Apache-2.0、CPU 可跑（BYOK 隐私关键）、Envoy 原生、自带 PII/越狱/语义缓存/推理开关，训练管道开源可用自有流量重训 ModernBERT 分类头
- 用 **LLMRouter (UIUC)** 做离线选型实验：RouterBench 数据集 + 真实流量回放，横评 16 种算法再定生产方案
- RouteLLM 的**阈值调成本比例**思想保留：连续 α 参数控制「多少流量上 SOTA」，运营侧随预算实时拧

**第 4 层（级联）**
- 借鉴 **AutoMix**（LLMRouter 内有实现）：小模型先答 + 自我一致性/自校验打分，低置信升级——级联层的 SOTA 开源实现
- 借鉴 vLLM SR 的**反馈信号**：级联触发（小模型答错被升级）记录为训练标签，回流微调第 3 层分类器，形成数据飞轮

**事实纠正**：资料里若写 "katanemo/archgw"，它已改名 **katanemo/plano**，默认路由模型从 Arch-Router-1.5B 升级为 4B 的 plano_orchestrator_v1；Arch-Router 权重为 Research License，商业化自托管平台直接内嵌需先确认授权。

---

## 参考

- [vLLM Semantic Router GitHub](https://github.com/vllm-project/semantic-router) / [v0.1 Iris 发布博客](https://vllm.ai/blog/2026-01-05-vllm-sr-iris) / [When to Reason 论文](https://arxiv.org/abs/2510.08731) / [98x Faster Routing Without GPU](https://arxiv.org/pdf/2603.12646) / [vLLM 官方博客](https://blog.vllm.ai/2025/09/11/semantic-router.html)
- [katanemo/plano](https://github.com/katanemo/plano) / [Arch-Router-1.5B HuggingFace](https://huggingface.co/katanemo/Arch-Router-1.5B) / [Arch-Router 论文](https://arxiv.org/abs/2506.16655) / [VentureBeat 报道](https://venturebeat.com/ai/new-1-5b-router-model-achieves-93-accuracy-without-costly-retraining)
- [lm-sys/RouteLLM](https://github.com/lm-sys/RouteLLM) / [论文](https://arxiv.org/pdf/2406.18665) / [LMSYS 博客](https://www.lmsys.org/blog/2024-07-01-routellm/)
- [ulab-uiuc/LLMRouter](https://github.com/ulab-uiuc/LLMRouter)
- [NVIDIA llm-router Blueprint](https://github.com/NVIDIA-AI-Blueprints/llm-router)
- [Not Diamond](https://www.notdiamond.ai/) / [awesome-ai-model-routing](https://github.com/Not-Diamond/awesome-ai-model-routing) / [OpenRouter Auto Router](https://openrouter.ai/docs/guides/routing/routers/auto-router)
- [Azure Foundry Model Router 文档](https://learn.microsoft.com/en-us/azure/foundry/openai/concepts/model-router)
- [RouterBench](https://github.com/withmartian/routerbench) / [论文](https://arxiv.org/abs/2403.12031)
- [RouteJudge 论文](https://arxiv.org/pdf/2606.18774) / [GitHub](https://github.com/LAMDA-Model-Reuse/RouteJudge)
- [aurelio-labs/semantic-router](https://github.com/aurelio-labs/semantic-router)
- [动态路由与级联综述 (2026)](https://arxiv.org/html/2603.04445v1)
