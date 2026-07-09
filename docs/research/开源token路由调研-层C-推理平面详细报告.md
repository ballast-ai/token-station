# 调研详细报告 · 层 C：推理平面 / 集群内路由

> 本文是 [开源token路由系统全景借鉴.md](../../../token-station/docs/research/开源token路由系统全景借鉴.md) 第三节的原始调研资料，保留全部事实细节与来源链接。调研日期 2026-07-05。
> 场景锚点：IDC 2 台 8×H200 节点跑 SGLang（TP8+EP8，GLM-5.2 FP8），每节点一个完整副本，前置推理网关做副本级 LB、健康检查、队列水位上报（见 [glm-5.2-部署架构.md](../architecture/glm-5.2-部署架构.md)）。
> 覆盖：SGL Model Gateway · llm-d · NVIDIA Dynamo · vLLM production-stack · AIBrix · Gateway API Inference Extension · KServe · Ray Serve。

---

## 零、结论速览

- **最契合、最轻量的是 SGLang 自带的 router（现已更名 SGL Model Gateway）**：单 Rust 二进制，天然理解 SGLang worker，cache_aware 开箱即用，自带重试/熔断/限流/Prometheus，可直接替换或增强自研网关。
- **cache-aware 路由在 2 副本规模收益有限但非零**：官方 benchmark 收益随 worker 数增长（4-8+ 显著），2 worker 时主要收益来自多轮对话/共享 system prompt 的前缀命中（各家数据：TTFT 降 40%-60%、吞吐 +1.9x~3x，均为前缀重复度高 workload、多为 4+ 副本测得）。多轮对话为主值得上；单轮短请求为主 power_of_two 就够。
- llm-d / Dynamo / AIBrix 都是 K8s 原生重型方案，2 节点 IDC 裸机性价比低，作为规模化后的演进方向。

---

## 一、SGLang Router → SGL Model Gateway（推荐）

- **仓库**：https://github.com/sgl-project/sglang（目录 `sgl-model-gateway/`，无独立仓库；原 `sgl-router` 目录已更名）
- **元数据**：主仓 29,909 stars | 路由器本体 Rust | Apache-2.0 | 主仓每日更新（最新 push 2026-07-05）
- **安装**：`pip install sglang-router`（PyPI 最新 0.3.2，2026-01-15）；Docker `lmsysorg/sgl-model-gateway:latest`；或 cargo 编译
- **路由策略**：`random` / `round_robin` / `cache_aware`（默认）/ `power_of_two` / `bucket`（动态负载分桶），支持 DP-aware 调度、IGW 多模型模式下按模型覆盖策略

**cache_aware 机制**（[SGLang v0.4 博客](https://www.lmsys.org/blog/2024-12-04-sglang-v0-4/)）
- 路由器本地维护每个 worker 的**近似 radix tree**（字符级，不做 tokenization），懒更新，开销极低
- 新请求对每个 worker 子树做最长前缀匹配，命中率超 `--cache-threshold`（默认 0.3）→ 路由到命中最高的 worker
- **负载均衡回退**：worker 间负载差超 `--balance-abs-threshold`（默认 64）**且**比值超 `--balance-rel-threshold`（默认 1.5）时，放弃缓存亲和回退 shortest-queue——即「负载均衡优先于缓存亲和」
- 其他参数：`--eviction-interval-secs 120`、`--max-tree-size`
- **官方数据**：最高 **1.9x 吞吐、3.8x 缓存命中率**——多 worker（10+）、前缀重复 workload 下测得，**收益随 worker 数放大**，2 worker 按比例打折

**与本架构的契合度**
- 「每节点一个完整 TP8 副本」= router 眼中 2 个 regular worker：`--worker-urls http://node1:port http://node2:port --policy cache_aware` 一行起，**零改造**
- **PD 分离**：原生支持（`--pd-disaggregation --prefill ... --decode ...`），prefill/decode 可各配独立策略（如 prefill 用 cache_aware、decode 用 power_of_two）——未来跨节点 PD 分离不用换路由器
- **可靠性**：健康检查（间隔/成败阈值可调）、重试（指数退避+抖动，408/429/5xx）、每 worker 熔断器（Closed→Open→Half-Open）、token-bucket 限流+排队
- **可观测/负载信号**：40+ Prometheus 指标（默认 :29000），含 `smg_worker_requests_active`、TTFT/TPOT 直方图；worker 层 SGLang server 自身暴露 `sglang_num_queue_reqs`（等待队列深度，**溢出决策核心信号**）、`sglang_num_running_reqs`、token_usage、cache hit rate（注意 v0.5.4+ 指标前缀从 `sglang:` 改为 `sglang_`，见 [issue #12618](https://github.com/sgl-project/sglang/issues/12618)）；`/workers` API 返回每 worker 的 `load` 与 `is_healthy`，上层网关可直接轮询做溢出决策
- **动态 worker 管理**：统一 `/workers` CRUD API（旧 add/remove_worker 已废弃），K8s service discovery 可选（裸机不需要）
- **HA 注意点**：多 router 副本各自维护独立 radix tree，官方文档提示会降 10-20% 缓存命中率；radix tree 跨副本同步（gRPC mesh）在 [roadmap #10341](https://github.com/sgl-project/sglang/issues/10341)，尚未完成
- **加分项**：gRPC 直连 SGLang scheduler 模式（Rust 本地 tokenize/tool parser，绕过 worker HTTP 层）；reasoning parser 已列入 GLM4/GLM4.7 支持（**GLM-5.2 需确认 parser 是否跟上**）

---

## 二、llm-d

- **仓库**：https://github.com/llm-d/llm-d | 3,702 stars | Shell/Go（多仓库组织）| Apache-2.0 | 活跃（push 2026-07-04），**CNCF Sandbox（2026-03）**，Red Hat/Google/IBM 主导
- **版本**：v0.8.1（2026-06-26）；v0.7 引入 GA 的预测延迟调度（predicted latency scheduling）
- **架构**：K8s 原生，"well-lit paths" = 推理调度（EPP 智能路由）/ 分层前缀缓存（KV offload 到 CPU/磁盘）/ 宽 EP / PD 分离；底层引擎以 **vLLM 为一等公民**，SGLang 仅文档提及、无成熟 well-lit path
- **与 IGW 的关系**：llm-d 的 inference scheduler 就是 Gateway API Inference Extension 的 EPP 实现；**2026 年起 GIE 上游的 EPP、InferenceObjective、Body Based Router 已迁移到 llm-d 仓库继续开发**，两者事实合流
- **benchmark**（官方 README）：前缀缓存感知路由 2x TTFT、3x 输出吞吐；预测调度降 40% 延迟——均为多副本 K8s 集群数据
- **适用性**：强依赖 K8s + Gateway API + Envoy（kgateway/Istio），vLLM 中心。2 节点裸机 + SGLang 不适用，除非未来迁 K8s 且换 vLLM。

---

## 三、NVIDIA Dynamo

- **仓库**：https://github.com/ai-dynamo/dynamo | 7,416 stars | Rust | Apache-2.0（附第三方 NOTICE，GitHub 显示 NOASSERTION）| 非常活跃（push 2026-07-05），stable v1.2.1（2026-06）
- **架构**：定位「推理引擎之上的编排层」——KV-aware Router（按 KV block 重叠度选 worker，事件驱动，从引擎上报 KV events 构建全局视图，**比 SGLang 的字符级近似树更精确**）、SLA-driven Planner（预测扩缩容）、KVBM（KV 多级卸载 GPU→CPU→SSD→远端）、NIXL 传输层、PD 分离
- **SGLang 兼容性**（官方支持矩阵）：PD 分离 ✅ / KV-aware routing ✅ / SLA Planner ✅ / KVBM 🚧 / 多模态 ✅——三大后端（vLLM/SGLang/TRT-LLM）中支持度居中
- **官方数据**：KV-aware routing 带来 **2x TTFT**；GB200 上 DeepSeek R1 7x 吞吐/GPU（后者是 PD 分离+大规模数字）
- **部署**：`uv pip install ai-dynamo[sglang]` 本地可跑（etcd+NATS 依赖）；K8s 走 operator/CRD
- **适用性**：可不上 K8s 且真支持 SGLang，KV 路由最精确；但引入 etcd/NATS/Dynamo runtime 一整套组件，2 副本场景是「用航母护渔船」。扩到 4+ 节点、做 PD 分离 + SLA 自动扩缩时最值得重估。

---

## 四、vLLM production-stack 与 AIBrix

**production-stack**：https://github.com/vllm-project/production-stack | 2,439 stars | Python | Apache-2.0 | 活跃度中等（0.1.11，2026-05；push 2026-06-29）
- 路由：round-robin、session-ID 亲和、prefix-aware（开发中）；无 LoRA 感知路由；Helm/K8s；Grafana 面板含队列/KV 使用率指标
- **vLLM 专用**，路由能力本清单最弱，对本场景无用。

**AIBrix**：https://github.com/vllm-project/aibrix | 4,930 stars | Go | Apache-2.0 | 活跃（v0.7.0，2026-06-18；字节跳动主导）
- 路由策略最丰富（基于 Envoy Gateway，可按请求 header 切换）：`prefix-cache`、`prefix-cache-preble`（**Preble 算法：前缀命中×负载混合打分**）、`least-kv-cache`、`least-request`、`power-of-two`、SLO 系列、`vtc-basic`（用户级 token 公平性）、`pd`、session-affinity
- 附加：高密度 LoRA 动态加载与感知路由、分布式 KV cache（跨引擎复用）、LLM 定制 autoscaler、异构 GPU SLO 调度
- **限制**：K8s + Envoy Gateway 强绑定，vLLM 中心（文档无 SGLang 支持）。路由策略设计值得借鉴（尤其 Preble 混合打分），系统本身不适合。

---

## 五、Gateway API Inference Extension（GIE / IGW）

- **仓库**：https://github.com/kubernetes-sigs/gateway-api-inference-extension | 704 stars | Go | Apache-2.0 | v1.5.0（2026-04-19）
- **机制**：基于 Envoy ext-proc 的 **EPP（Endpoint Picker）**——网关把请求交给 EPP，EPP 按 **queue depth、KV cache 利用率、prefix cache 状态、LoRA 亲和**打分选 endpoint；核心 CRD `InferencePool`
- **状态**：**已 GA**（InferencePool v1）；原 EPP/InferenceObjective/BBR 已迁入 llm-d 社区维护，上游只保留轻量 LWEPP 和 API 规范；网关实现：Envoy Gateway、kgateway、GKE Gateway、Istio
- **模型服务器**：协议化——「任何实现 metrics 协议的 server」，vLLM 一等，SGLang 需适配
- **适用性**：这是「标准/协议」而非可独立落地的产品，绑定 K8s Gateway API。价值在**调度信号协议设计**（queue depth + KV utilization + prefix affinity 三信号打分）可直接抄进自研网关。

---

## 六、简评：KServe 与 Ray Serve

- **KServe**（https://github.com/kserve/kserve，5,647 stars，CNCF Incubating）：v0.16 引入 `LLMInferenceService` CRD，**路由层直接复用 llm-d + GIE**（自身不造轮子）；官方数据：开启前缀感知后 3x 输出吞吐、2x TTFT（[KServe 博客](https://kserve.github.io/website/blog/cloud-native-ai-inference-kserve-llm-d)）。= llm-d 的企业封装，不必单独看。
- **Ray Serve LLM**（https://github.com/ray-project/ray，43,117 stars）：`PrefixCacheAffinityRouter`——先比较副本队列长度，负载均衡时才做前缀亲和；[Anyscale benchmark](https://www.anyscale.com/blog/ray-serve-faster-first-token-custom-routing)：32B 模型 **TTFT -60%、端到端吞吐 +40%、输入 token 处理 +2.5x**（前缀重复数据集）。仅已在 Ray 生态时值得看；为 2 节点引入 Ray 不划算。

---

## 七、对比表

| 系统 | Stars | 语言 | 许可证 | KV/前缀感知机制 | SGLang 支持 | PD 分离 | 部署重量 | 2 节点裸机契合度 |
|---|---|---|---|---|---|---|---|---|
| SGL Model Gateway | (主仓 29.9k) | Rust | Apache-2.0 | 路由器侧近似 radix tree（字符级、懒更新）+ 负载回退 | **原生** | ✅ 双策略 | **单二进制** | ★★★★★ |
| NVIDIA Dynamo v1.2 | 7.4k | Rust | Apache-2.0 | 引擎上报 KV events 的精确 block 级路由 | ✅（KVBM 除外） | ✅ | 重（etcd+NATS） | ★★★ |
| llm-d v0.8 | 3.7k | Go/Shell | Apache-2.0 | EPP 打分（prefix + queue + KV util）+ 预测延迟 | 弱（vLLM 中心） | ✅ | 重（K8s+GW API） | ★ |
| AIBrix v0.7 | 4.9k | Go | Apache-2.0 | prefix-cache / Preble 混合打分 / least-kv-cache | ✗ | ✅ | 重（K8s+Envoy GW） | ★ |
| GIE v1.5 (GA) | 0.7k | Go | Apache-2.0 | EPP 协议（queue/KV/LoRA 信号） | 需适配 | 经 llm-d | 重（K8s） | ★（协议可借鉴） |
| production-stack | 2.4k | Python | Apache-2.0 | session 亲和；prefix-aware 开发中 | ✗ | roadmap | 中（Helm） | ★ |
| Ray Serve LLM | (43.1k) | Python | Apache-2.0 | PrefixCacheAffinityRouter（队列优先） | ✗（vLLM） | ✅(Anyscale) | 重（Ray 集群） | ★★ |

---

## 八、落地建议（按优先级）

1. **直接上 SGL Model Gateway 替换/垫在自研 Router 之下**：`--policy cache_aware --worker-urls http://node1 http://node2`，保留云上网关做计费/租户/溢出，把副本级 LB、健康检查、重试熔断下沉给它。改造成本一天以内。默认参数（cache-threshold 0.3 / balance-abs 64 / balance-rel 1.5）对 2 副本基本合理，`balance_abs_threshold` 可按并发水位调低使回退更敏感。
2. **溢出信号采集**：上层网关拉两路——router 的 `/workers`（每 worker load/health）+ 各节点 SGLang `/metrics`（`sglang_num_queue_reqs`、`sglang_num_running_reqs`、token_usage）。以 `num_queue_reqs` 持续 > 阈值作为溢出触发，这是 GIE/AIBrix/Ray 共同采用的第一信号。
3. **先测后信**：用真实流量重放对比 `cache_aware` vs `power_of_two`。2 副本下 cache-aware 理论收益上限低（每副本本来就有 50% 概率命中自己的 RadixAttention 缓存）；只有多轮对话/长共享 system prompt 占比高时，60%/2x 这类数字才可能部分兑现。
4. **暂不引入** llm-d / AIBrix / GIE（K8s 绑定、vLLM 中心）；**Dynamo 列入观察名单**，触发条件：扩到 ≥4 节点、跨节点 PD 分离或 SLA 自动扩缩。
5. **风险提示**：多 router 副本做 HA 时 radix tree 不同步掉 10-20% 命中率（官方自述）；GLM-5.2 的 reasoning/tool parser 在 gRPC 模式下需验证（文档目前列到 GLM4.7）。

---

## 参考

[SGLang v0.4 博客（cache-aware LB, 1.9x/3.8x）](https://www.lmsys.org/blog/2024-12-04-sglang-v0-4/) | [SGL Model Gateway README](https://github.com/sgl-project/sglang/tree/main/sgl-model-gateway) | [SGLang Router Roadmap #10341](https://github.com/sgl-project/sglang/issues/10341) | [SGLang Production Metrics](https://docs.sglang.io/references/production_metrics.html) | [指标前缀变更 #12618](https://github.com/sgl-project/sglang/issues/12618) | [llm-d](https://github.com/llm-d/llm-d) | [llm-d 智能调度指南](https://llm-d.ai/docs/guide/Installation/inference-scheduling) | [NVIDIA Dynamo](https://github.com/ai-dynamo/dynamo) | [production-stack](https://github.com/vllm-project/production-stack) | [AIBrix](https://github.com/vllm-project/aibrix) | [AIBrix Gateway 插件文档](https://aibrix.readthedocs.io/latest/features/gateway-plugins.html) | [Gateway API Inference Extension](https://github.com/kubernetes-sigs/gateway-api-inference-extension) | [KServe+llm-d 博客](https://kserve.github.io/website/blog/cloud-native-ai-inference-kserve-llm-d) | [KServe LLMInferenceService](https://kserve.github.io/website/docs/model-serving/generative-inference/llmisvc/llmisvc-overview) | [Ray PrefixCacheAffinityRouter](https://docs.ray.io/en/latest/serve/llm/prefix-aware-request-router.html) | [Anyscale 60% TTFT 博客](https://www.anyscale.com/blog/ray-serve-faster-first-token-custom-routing) | [Red Hat: KServe+llm-d](https://developers.redhat.com/articles/2026/04/21/kserve-llm-d-optimized-gen-ai-inference)
