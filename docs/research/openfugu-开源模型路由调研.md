# OpenFugu 开源模型路由项目调研

> 调研日期：2026-07-08
> 项目地址：https://github.com/trotsky1997/OpenFugu
> 许可证：Apache-2.0（项目代码）；训练出的 Conductor 权重受 Llama 3.2 社区许可约束
> 定位：对 Sakana AI 商业产品 Fugu 的开源逆向复现，与 Sakana 无官方关联

## 一、项目定位

Fugu 是 Sakana AI 的商业产品，对外宣传是"一个模型"，实际是一个**学习型模型路由/编排策略**：用一个极小的协调器，按每条查询把工作分派给一池前沿 LLM（GPT、Claude、Gemini、DeepSeek、GLM 等），再以单一模型的接口形态返回答案。

Sakana 的产品和训练权重是闭源的。OpenFugu 做的事情是：

1. 从两篇论文 + 公开发布的产物中逆向出完整机制；
2. 用 Sakana 发布的真实权重验证逆向结论；
3. 从零训练自己的路由器（Conductor）；
4. 通过 OpenAI 兼容端点对外提供服务。

## 二、核心机制

### 2.1 路由器架构（TRINITY / Fugu-mini）

- **骨干**：冻结的 Qwen3-0.6B，仅用于提取隐藏状态，从不生成面向用户的文本。
- **路由决策**：提取查询倒数第二个 token 的隐藏状态 h ∈ R^1024，经过一个**无偏置线性头**映射为各工作模型的 logits，softmax 后选最高分模型作答：

  π_θ(i|s) = softmax(W_head · h)_i

- **可训练参数极少**：约 **19,456 个**（10,240 头部权重 + 9,216 SVF 偏移），路由一次前向传播完成，几乎不增加延迟。
- **SVF（奇异值微调）**：骨干的 9 个权重矩阵（嵌入层、第 26 层的 7 个变换、LM 头）冻结正交分解，只学习奇异值缩放 σ̃_k = (1+δ_k)σ_k，加能量保留归一化。
- **角色系统（学术版 TRINITY）**：头部输出 L+3 = 10 个 logits，前 7 个选工作模型，后 3 个经独立 softmax 选角色（Worker / Thinker / Verifier）。生产版简化为只选模型。
- **输入格式**：原始文本输入，非 chat 模板（逆向验证确认）。

### 2.2 Conductor / Fugu-Ultra（进阶版）

- 用一个 **7B 模型（Conductor）** 替换逐轮选择器，一次性生成完整**工作流 DAG**：
  - 拆分子任务列表；
  - 给每步分配工作模型；
  - 控制各步骤可见的上游输出（工作流内隔离、跨工作流共享）。
- DAG 拓扑排序后执行，支持多智能体协调。

### 2.3 自适应池

支持动态 k-of-n 池子选择——工作模型池可以是任意子集，路由器对池变化有适应能力。

## 三、四阶段工作流（read → run → train → serve）

| 阶段 | 内容 | 产物/结果 |
|------|------|-----------|
| **read** | 从论文 + 发布产物逆向完整数学模型 | `docs/HOW_FUGU_IS_IMPLEMENTED.md`，带证据评级的技术文档 |
| **run** | 用 Sakana 发布的真实权重（`model_iter_60.npy`）验证复现 | 37 个测试用例上 95% 模型选择准确率、100% 角色准确率（统计显著性 P ≈ 10^-8 ~ 10^-12） |
| **train** | 从零训练 TRINITY 和 Conductor | TRINITY 数代内收敛到最优路由；Conductor 奖励 1.21 → 1.64 |
| **serve** | litellm 暴露 OpenAI 兼容 `/v1/chat/completions` | 单一模型接口的多模型编排，隐藏内部路由循环 |

## 四、训练方法

两阶段流程：

1. **监督预热**：在可验证数据集上，把各工作模型的性能分数经温度 softmax 软化为目标分布，用 KL 散度训练头部和 SVF 参数。
2. **进化策略精化**：用可分离 CMA-ES（sep-CMA-ES）直接优化端到端任务完成率 J(θ) = E_τ[R(τ)]。配置：33 个候选、16 精英、60 次迭代。对角化限制下实际表现为标量步长自适应的各向同性策略。

Conductor 用 **GRPO** 在 ToolScale 数据集上训练。全程**无梯度反传大模型**——这是成本极低的关键。

## 五、关键性能指标

- 训练出的路由器相对池中**最佳单一模型 +107%**（按查询级路由的收益）。
- 自适应池测试：模拟场景下达到 oracle 性能的 **94%**。
- 递归测试：第二轮与第一轮相当（见 results/）。

## 六、部署使用

```bash
pip install -r requirements.txt
python scripts/fetch_artifacts.py   # 拉取 Qwen3 + 权重 + 测试集
export FUGU_MODEL=... FUGU_VECTOR=... FUGU_FIXTURE=...
python openfugu/serve.py --slot-models "<模型列表>" --port 8088
curl localhost:8088/v1/chat/completions   # 像普通 OpenAI 接口一样调用
```

技术栈：纯 Python；torch / transformers / trl / litellm。

项目结构：

- `docs/` — 数学推导与架构证据
- `openfugu/` — mini.py（TRINITY）、ultra.py（Conductor DAG）、serve.py（服务端）
- `train/` — TRINITY、Conductor、递归/自适应变体三类训练脚本
- `eval/` — 编排效能评估、端到端验证
- `pipeline/` — 训练→服务→验证一体化命令
- `scripts/` — 权重/产物获取脚本

## 七、对 token-station 的借鉴意义

1. **"单一入口 + 后端多模型路由"技术门槛极低**：19.5K 可训练参数、0.6B 冻结骨干就能做按查询路由，路由本身近零延迟、近零成本。token 中转站做"智能路由降本"的技术底座已被验证可行。
2. **成本结构差异**：便宜模型能答的路由给便宜模型，难题才走贵模型——与纯转发的成本结构完全不同，是定价/毛利设计的关键变量（对照 `../features/业务模式与定价划分.md` 与 `../features/智能路由能力.md`）。
3. **对外单一模型形态**：serve 阶段隐藏内部路由循环，用户只看到一个 OpenAI 兼容端点——这与「路由透明与用户信任」议题直接相关（对照 `../features/路由透明与用户信任.md`）：Fugu 商业产品选择了完全不透明的路线，OpenFugu 的逆向恰恰说明不透明路由会被外部还原。
4. **训练成本可控**：无梯度进化算法 + 小参数量，意味着中转站自训路由器（基于自己流量的真实反馈）是可负担的，可与层 B 学习型路由器调研（`开源token路由调研-层B-学习型路由器详细报告.md`）合并评估。

## 八、同名/同类项目辨析

- **walidboulanouar/maestro** — 自称 "open-source Fugu" 的 LLM 编排大脑，另一个独立实现。
- **BicaMindLabs/open-sakanafugu（FuguNano）** — 多智能体编码工作流：9 个国产 LLM 做实现者（各自独立 Claude Code 实例）+ Codex 做独立审查者 + 有界审查-修复循环。
- **Google Project Fugu** — 完全无关，是浏览器 Web 能力 API 项目，注意不要混淆。

## 参考来源

- 项目主页：https://github.com/trotsky1997/OpenFugu
- 实现机制文档：https://github.com/trotsky1997/OpenFugu/blob/main/docs/HOW_FUGU_IS_IMPLEMENTED.md
