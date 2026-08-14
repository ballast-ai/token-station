import { useEffect, useMemo, useState } from "react";
import {
  getRequestReceipts,
  type ReceiptCostKind,
  type ReceiptPageView,
  type RequestPlaintextView,
  type ReceiptView,
} from "../api";
import { ReceiptDetails } from "./RecentReceipts";
import { useLocalizedCopy } from "./LanguageProvider";
import { humanizeAppError } from "../errors";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "./ui/select";

const PAGE_SIZE = 20;

interface UsageRequestLogProps {
  since: string;
  agentId: string;
  upstream: string;
  model: string;
  refreshKey: number;
}

function formatTime(timestamp: number, locale: string): string {
  return new Date(timestamp).toLocaleString(locale, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}

function routeOf(receipt: ReceiptView, noRoute: string): string {
  return receipt.routing
    ? `${receipt.routing.upstream}/${receipt.routing.model}`
    : noRoute;
}

function tokenTotal(receipt: ReceiptView): string {
  if (!receipt.usage) return "—";
  return (receipt.usage.input_tokens + receipt.usage.output_tokens).toLocaleString();
}

function costLabel(receipt: ReceiptView, actual: string, estimated: string, unknown: string): string {
  if (receipt.cost_kind === "unknown" || receipt.cost_micros == null) return unknown;
  const prefix = receipt.cost_kind === "actual" ? actual : estimated;
  return `${prefix} $${(receipt.cost_micros / 1_000_000).toFixed(6)}`;
}

function unknownCostReason(
  receipt: ReceiptView,
  missingUsage: string,
  missingPrice: (model: string) => string,
): string {
  if (receipt.usage == null) return missingUsage;
  const model = receipt.routing?.model || receipt.requested_model;
  return missingPrice(model);
}

type SemanticKind =
  | "system"
  | "developer"
  | "user"
  | "assistant"
  | "thinking"
  | "tool-call"
  | "tool-result"
  | "tools"
  | "unrecognized"
  | "stream-end";

interface SemanticBlock {
  kind: SemanticKind;
  content: string;
  detail?: string;
}

type JsonObject = Record<string, unknown>;

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function prettyValue(value: unknown): string {
  if (typeof value === "string") return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function formatJsonSource(source: string): string {
  try {
    return JSON.stringify(JSON.parse(source), null, 2);
  } catch {
    return source;
  }
}

function textParts(value: unknown): Array<{ kind: "text" | "thinking" | "tool-call" | "tool-result"; content: string; detail?: string }> {
  if (typeof value === "string") return value ? [{ kind: "text", content: value }] : [];
  if (Array.isArray(value)) return value.flatMap(textParts);
  if (!isObject(value)) return value == null ? [] : [{ kind: "text", content: String(value) }];

  const type = typeof value.type === "string" ? value.type : "";
  if (type === "thinking" || type === "reasoning") {
    const content = value.thinking ?? value.text ?? value.content;
    return content == null ? [] : [{ kind: "thinking", content: prettyValue(content) }];
  }
  if (type === "tool_use" || type === "function_call") {
    return [{
      kind: "tool-call",
      detail: typeof value.name === "string" ? value.name : undefined,
      content: prettyValue(value.input ?? value.arguments ?? {}),
    }];
  }
  if (type === "tool_result" || type === "function_call_output") {
    return [{
      kind: "tool-result",
      detail: typeof value.tool_use_id === "string" ? value.tool_use_id : undefined,
      content: prettyValue(value.content ?? value.output ?? ""),
    }];
  }
  const text = value.text ?? value.input_text ?? value.output_text;
  if (typeof text === "string") return text ? [{ kind: "text", content: text }] : [];
  if (value.content != null) return textParts(value.content);
  return [];
}

function splitEmbeddedSystemText(content: string): SemanticBlock[] | null {
  const pattern = /<system-reminder>\s*([\s\S]*?)\s*<\/system-reminder>/gi;
  const blocks: SemanticBlock[] = [];
  let cursor = 0;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(content)) !== null) {
    const userText = content.slice(cursor, match.index).trim();
    if (userText) blocks.push({ kind: "user", content: userText });
    if (match[1].trim()) blocks.push({ kind: "system", content: match[1].trim() });
    cursor = match.index + match[0].length;
  }
  if (!blocks.length) return null;
  const trailing = content.slice(cursor).trim();
  if (trailing) blocks.push({ kind: "user", content: trailing });
  return blocks;
}

function parseInputSemantics(body: string): SemanticBlock[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return [{ kind: "unrecognized", content: body }];
  }
  if (!isObject(parsed)) return [{ kind: "unrecognized", content: prettyValue(parsed) }];

  const blocks: SemanticBlock[] = [];
  for (const value of [parsed.system, parsed.instructions]) {
    for (const part of textParts(value)) {
      blocks.push({ kind: part.kind === "thinking" ? "thinking" : "system", content: part.content });
    }
  }
  const messages = Array.isArray(parsed.messages)
    ? parsed.messages
    : parsed.input != null ? (Array.isArray(parsed.input) ? parsed.input : [{ role: "user", content: parsed.input }]) : [];
  for (const message of messages) {
    if (!isObject(message)) continue;
    const role = typeof message.role === "string" ? message.role : "user";
    for (const part of textParts(message.content ?? message)) {
      if (role === "user" && part.kind === "text") {
        const embedded = splitEmbeddedSystemText(part.content);
        if (embedded) {
          blocks.push(...embedded);
          continue;
        }
      }
      const kind: SemanticKind = part.kind === "thinking"
        ? "thinking"
        : part.kind === "tool-call"
          ? "tool-call"
          : part.kind === "tool-result"
            ? "tool-result"
            : role === "system"
              ? "system"
              : role === "developer"
                ? "developer"
                : role === "assistant"
                  ? "assistant"
                  : role === "tool"
                    ? "tool-result"
                    : "user";
      blocks.push({ kind, content: part.content, detail: part.detail });
    }
  }
  if (Array.isArray(parsed.tools) && parsed.tools.length) {
    const tools = parsed.tools.map((tool) => {
      if (!isObject(tool)) return prettyValue(tool);
      const definition = isObject(tool.function) ? tool.function : tool;
      const name = typeof definition.name === "string" ? definition.name : "未命名工具";
      const description = typeof definition.description === "string" ? definition.description : "";
      return description ? `${name} — ${description}` : name;
    });
    blocks.push({ kind: "tools", detail: String(parsed.tools.length), content: tools.join("\n") });
  }
  return blocks.length ? blocks : [{ kind: "unrecognized", content: prettyValue(parsed) }];
}

function ssePayloads(body: string): { found: boolean; values: unknown[] } {
  const values: unknown[] = [];
  let found = false;
  for (const frame of body.replace(/\r\n?/g, "\n").split(/\n{2,}/)) {
    const dataLines = frame.split("\n")
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).replace(/^ /, ""));
    if (!dataLines.length) continue;
    found = true;
    const payload = dataLines.join("\n");
    if (payload === "[DONE]") {
      values.push(payload);
      continue;
    }
    try {
      values.push(JSON.parse(payload));
    } catch {
      values.push(payload);
    }
  }
  return { found, values };
}

function parseOutputSemantics(body: string): SemanticBlock[] {
  const stream = ssePayloads(body);
  let values = stream.values;
  if (!stream.found) {
    try {
      values = [JSON.parse(body)];
    } catch {
      return [{ kind: "unrecognized", content: body }];
    }
  }

  let thinking = "";
  let output = "";
  const tools = new Map<string, { name?: string; arguments: string }>();
  const unrecognized: string[] = [];
  let sawEnd = false;

  const appendParts = (parts: ReturnType<typeof textParts>) => {
    for (const part of parts) {
      if (part.kind === "thinking") thinking += part.content;
      else if (part.kind === "text") output += part.content;
      else if (part.kind === "tool-call") {
        const key = `content-${tools.size}`;
        tools.set(key, { name: part.detail, arguments: part.content });
      } else if (part.kind === "tool-result") {
        unrecognized.push(part.content);
      }
    }
  };

  for (const value of values) {
    if (value === "[DONE]") {
      sawEnd = true;
      continue;
    }
    if (!isObject(value)) {
      unrecognized.push(prettyValue(value));
      continue;
    }
    let recognized = false;
    const index = typeof value.index === "number" ? String(value.index) : "0";
    if (isObject(value.content_block) && value.content_block.type === "tool_use") {
      const initialInput = value.content_block.input;
      tools.set(index, {
        name: typeof value.content_block.name === "string" ? value.content_block.name : undefined,
        arguments: isObject(initialInput) && Object.keys(initialInput).length === 0
          ? ""
          : prettyValue(initialInput ?? ""),
      });
      recognized = true;
    }
    if (isObject(value.delta)) {
      const delta = value.delta;
      if (delta.type === "thinking_delta" && typeof delta.thinking === "string") {
        thinking += delta.thinking;
        recognized = true;
      }
      if ((delta.type === "text_delta" || delta.type === "output_text_delta") && typeof delta.text === "string") {
        output += delta.text;
        recognized = true;
      }
      if (delta.type === "input_json_delta" && typeof delta.partial_json === "string") {
        const tool = tools.get(index) ?? { arguments: "" };
        tool.arguments += delta.partial_json;
        tools.set(index, tool);
        recognized = true;
      }
    }
    if (Array.isArray(value.choices)) {
      for (const choice of value.choices) {
        if (!isObject(choice)) continue;
        const message = isObject(choice.delta) ? choice.delta : isObject(choice.message) ? choice.message : null;
        if (!message) continue;
        if (typeof message.content === "string") output += message.content;
        else appendParts(textParts(message.content));
        const reasoning = message.reasoning_content ?? message.reasoning;
        if (typeof reasoning === "string") thinking += reasoning;
        if (Array.isArray(message.tool_calls)) {
          for (const call of message.tool_calls) {
            if (!isObject(call)) continue;
            const callIndex = typeof call.index === "number" ? String(call.index) : String(tools.size);
            const fn = isObject(call.function) ? call.function : call;
            const tool = tools.get(callIndex) ?? { arguments: "" };
            if (typeof fn.name === "string") tool.name = fn.name;
            if (typeof fn.arguments === "string") tool.arguments += fn.arguments;
            tools.set(callIndex, tool);
          }
        }
        recognized = true;
      }
    }
    if (typeof value.type === "string" && value.type.startsWith("response.")) {
      if (typeof value.delta === "string") {
        if (value.type.includes("reasoning")) thinking += value.delta;
        else if (value.type.includes("output_text")) output += value.delta;
        else if (value.type.includes("function_call_arguments")) {
          const key = typeof value.item_id === "string" ? value.item_id : index;
          const tool = tools.get(key) ?? { arguments: "" };
          tool.arguments += value.delta;
          tools.set(key, tool);
        }
        recognized = true;
      }
    }
    if (Array.isArray(value.content)) {
      appendParts(textParts(value.content));
      recognized = true;
    }
    if (Array.isArray(value.output)) {
      for (const item of value.output) {
        if (!isObject(item)) continue;
        if (item.type === "function_call") {
          const key = typeof item.call_id === "string" ? item.call_id : String(tools.size);
          tools.set(key, {
            name: typeof item.name === "string" ? item.name : undefined,
            arguments: prettyValue(item.arguments ?? ""),
          });
        } else appendParts(textParts(item.content ?? item));
      }
      recognized = true;
    }
    const controlEvent = typeof value.type === "string" && /^(message_|content_block_)/.test(value.type);
    if (!recognized && !controlEvent) unrecognized.push(prettyValue(value));
  }

  const blocks: SemanticBlock[] = [];
  if (thinking) blocks.push({ kind: "thinking", content: thinking });
  if (output) blocks.push({ kind: "assistant", content: output });
  for (const tool of tools.values()) {
    let args = tool.arguments;
    try {
      args = JSON.stringify(JSON.parse(args), null, 2);
    } catch {
      // Partial tool arguments remain visible exactly as received.
    }
    blocks.push({ kind: "tool-call", detail: tool.name, content: args || "{}" });
  }
  for (const content of unrecognized) blocks.push({ kind: "unrecognized", content });
  if (sawEnd) blocks.push({ kind: "stream-end", content: "[DONE]" });
  return blocks.length ? blocks : [{ kind: "unrecognized", content: prettyValue(values) }];
}

function RequestPlaintext({
  plaintext,
  error,
}: {
  plaintext?: RequestPlaintextView;
  error?: string;
}) {
  const { copy } = useLocalizedCopy();
  const [viewMode, setViewMode] = useState<"parsed" | "source">("parsed");
  if (error) {
    return (
      <section className="request-plaintext" aria-label={copy("Plaintext input and output", "明文输入输出")}>
        <div className="request-plaintext-state error-text" role="alert">
          {copy(`Failed to read plaintext: ${error}`, `明文读取失败：${error}`)}
        </div>
      </section>
    );
  }
  if (!plaintext) {
    return (
      <section className="request-plaintext" aria-label={copy("Plaintext input and output", "明文输入输出")}>
        <div className="request-plaintext-state" role="status">
          {copy(
            "No plaintext is available for this request. It may predate this feature, have failed to write, or have been removed by retention.",
            "此请求没有可用明文。它可能早于本功能、写入失败或已被保留策略清理。",
          )}
        </div>
      </section>
    );
  }
  const panels = [
    {
      key: "input",
      label: copy("Plaintext input", "明文输入"),
      body: plaintext.input,
      truncated: plaintext.input_truncated,
      blocks: parseInputSemantics(plaintext.input),
    },
    {
      key: "output",
      label: copy("Plaintext output", "明文输出"),
      body: plaintext.output,
      truncated: plaintext.output_truncated,
      blocks: parseOutputSemantics(plaintext.output),
    },
  ];
  const semanticLabel = (block: SemanticBlock) => {
    switch (block.kind) {
      case "system": return copy("System prompt", "系统提示词");
      case "developer": return copy("Developer prompt", "开发者提示词");
      case "user": return copy("User input", "用户输入");
      case "assistant": return copy("Assistant output", "助手输出");
      case "thinking": return copy("Assistant thinking", "助手思考");
      case "tool-call": return block.detail
        ? copy(`Tool call · ${block.detail}`, `工具调用 · ${block.detail}`)
        : copy("Tool call", "工具调用");
      case "tool-result": return block.detail
        ? copy(`Tool result · ${block.detail}`, `工具返回 · ${block.detail}`)
        : copy("Tool result", "工具返回");
      case "tools": return copy(`Tool definitions · ${block.detail} total`, `工具定义 · ${block.detail} 个`);
      case "stream-end": return copy("Stream completed", "流式结束");
      default: return copy("Unrecognized content", "未识别内容");
    }
  };
  return (
    <section className="request-plaintext" aria-label={copy("Plaintext input and output", "明文输入输出")}>
      <header>
        <div className="request-plaintext-title">
          <strong>{copy("Plaintext input and output", "明文输入输出")}</strong>
          <span>{copy("Retained for 7 days by default", "默认保留 7 天")}</span>
        </div>
        <div
          className="request-plaintext-mode"
          role="group"
          aria-label={copy("Body display mode", "正文显示方式")}
        >
          <button
            type="button"
            aria-pressed={viewMode === "parsed"}
            onClick={() => setViewMode("parsed")}
          >
            {copy("Parsed", "解析视图")}
          </button>
          <button
            type="button"
            aria-pressed={viewMode === "source"}
            onClick={() => setViewMode("source")}
          >
            {copy("Source", "原文")}
          </button>
        </div>
      </header>
      <div className="request-plaintext-grid">
        {panels.map((panel) => (
          <div className="request-plaintext-panel" key={panel.key}>
            <div className="request-plaintext-label">
              <span>{panel.key.toUpperCase()}</span>
              {panel.truncated && <em>{copy("Truncated", "已截断")}</em>}
            </div>
            {viewMode === "parsed" ? (
              <div
                className="request-plaintext-scroll request-semantic-view"
                role="region"
                aria-label={panel.label}
                tabIndex={0}
              >
                {panel.body ? panel.blocks.map((block, index) => (
                  <section className={`request-semantic-block ${block.kind}`} key={`${block.kind}-${index}`}>
                    <div>{semanticLabel(block)}</div>
                    <pre>{block.content}</pre>
                  </section>
                )) : <span className="request-semantic-empty">{copy("Empty body", "空正文")}</span>}
                {panel.truncated && (
                  <div className="request-semantic-truncated">{copy("Body truncated", "正文已截断")}</div>
                )}
              </div>
            ) : (
              <pre
                className="request-plaintext-scroll"
                role="region"
                aria-label={panel.label}
                tabIndex={0}
              >
                {panel.body ? formatJsonSource(panel.body) : copy("Empty body", "空正文")}
              </pre>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}

function CostState({
  receipt,
  compact = false,
}: {
  receipt: ReceiptView;
  compact?: boolean;
}) {
  const { copy } = useLocalizedCopy();
  const kind: ReceiptCostKind = receipt.cost_kind;
  const reason = unknownCostReason(
    receipt,
    copy(
      "The upstream did not return token usage, so cost cannot be estimated.",
      "上游未返回 Token，无法估算成本。",
    ),
    (model) => copy(`No price configured for model: ${model}`, `缺少模型价格：${model}`),
  );
  if (kind === "unknown" || receipt.cost_micros == null) {
    return (
      <span className="usage-log-cost unknown" title={reason}>
        {compact ? copy("Unknown", "未知") : reason}
      </span>
    );
  }
  return (
    <span className={`usage-log-cost ${kind}`}>
      {costLabel(
        receipt,
        copy("Actual", "实际"),
        copy("Estimated", "估算"),
        copy("Cost unknown", "成本未知"),
      )}
    </span>
  );
}

export default function UsageRequestLog({
  since,
  agentId,
  upstream,
  model,
  refreshKey,
}: UsageRequestLogProps) {
  const { language, copy } = useLocalizedCopy();
  const [status, setStatus] = useState<"" | "success" | "error">("");
  const [page, setPage] = useState(1);
  const [data, setData] = useState<ReceiptPageView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");

  useEffect(() => {
    setPage(1);
  }, [since, agentId, upstream, model, status]);

  useEffect(() => {
    let active = true;
    setLoading(true);
    setError("");
    getRequestReceipts({
      since,
      agentId: agentId || null,
      upstream: upstream || null,
      model: model || null,
      status: status || null,
      page,
      pageSize: PAGE_SIZE,
    })
      .then((next) => {
        if (active) setData(next);
      })
      .catch((caught) => {
        if (active) setError(humanizeAppError(caught));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [agentId, model, page, refreshKey, since, status, upstream]);

  const total = data?.total ?? 0;
  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE));
  const range = useMemo(() => {
    if (!total) return copy("0 items", "0 条");
    const start = (page - 1) * PAGE_SIZE + 1;
    return `${start}–${Math.min(total, start + PAGE_SIZE - 1)} / ${total}`;
  }, [copy, page, total]);

  return (
    <section className="usage-request-log">
      <header>
        <div>
          <h2>{copy("Request log", "请求日志")}</h2>
          <p>{copy(
            "Complete local receipts · Plaintext bodies are retained separately for 7 days by default",
            "完整本地 Receipt · 正文明文独立存储，默认保留 7 天",
          )}</p>
        </div>
        <div className="usage-log-tools">
          <div className="usage-log-filter">
            <span>{copy("Status", "状态")}</span>
            <Select
              value={status || "all"}
              onValueChange={(next) => setStatus(next === "all" ? "" : (next as typeof status))}
            >
              <SelectTrigger
                className="usage-log-status-select"
                size="sm"
                aria-label={copy("Request status", "请求状态")}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                <SelectItem value="all">{copy("All statuses", "全部状态")}</SelectItem>
                <SelectItem value="success">{copy("Success", "成功")}</SelectItem>
                <SelectItem value="error">{copy("Error", "失败")}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <strong>{range}</strong>
        </div>
      </header>

      {loading && !data && (
        <div className="usage-log-state">{copy("Loading request log…", "正在读取请求日志…")}</div>
      )}
      {error && (
        <div className="usage-log-state error-text">
          {copy(`Failed to load request log: ${error}`, `请求日志读取失败：${error}`)}
        </div>
      )}
      {!loading && !error && data?.items.length === 0 && (
        <div className="usage-log-state">{copy(
          "No requests match the current filters.",
          "当前筛选范围没有请求日志。",
        )}</div>
      )}

      {data && data.items.length > 0 && (
        <>
          <div className="usage-log-head" aria-hidden="true">
            <span>{copy("Time", "时间")}</span>
            <span>{copy("Agent / Route", "Agent / 路由")}</span>
            <span>{copy("Status", "状态")}</span>
            <span>Token</span>
            <span>{copy("Latency", "延迟")}</span>
            <span>{copy("Cost", "成本")}</span>
          </div>
          <div className={`usage-log-list ${loading ? "refreshing" : ""}`}>
            {data.items.map((receipt) => {
              const cancelled = receipt.status === 499;
              const success = receipt.status >= 200
                && receipt.status < 400
                && receipt.error_code == null;
              return (
                <details className="usage-log-row" key={receipt.request_id}>
                  <summary>
                    <time dateTime={new Date(receipt.started_at_ms).toISOString()}>
                      {formatTime(receipt.started_at_ms, language)}
                    </time>
                    <span className="usage-log-route">
                      <small>{receipt.agent_id ?? copy("Unknown Agent", "未知 Agent")}</small>
                      <strong>{routeOf(receipt, copy("No route", "未产生路由"))}</strong>
                    </span>
                    <span className={`usage-log-status ${success ? "success" : cancelled ? "" : "error"}`}>
                      {cancelled ? copy("Cancelled", "已取消") : `HTTP ${receipt.status}`}
                    </span>
                    <span>{tokenTotal(receipt)}</span>
                    <span>{receipt.latency_ms.toLocaleString()} ms</span>
                    <CostState receipt={receipt} compact />
                  </summary>
                  <div className="usage-log-expanded">
                    <div className="usage-log-facts">
                      <span><small>{copy("Request ID", "请求 ID")}</small><code>{receipt.request_id}</code></span>
                      <span><small>{copy("Requested model", "请求模型")}</small><strong>{receipt.requested_model}</strong></span>
                      <span><small>{copy("Protocol", "协议")}</small><strong>{receipt.protocol}</strong></span>
                      <span>
                        <small>{copy("Endpoint", "端点")}</small>
                        <strong>{receipt.request_method ?? "—"} · {receipt.path_kind ?? "unknown"}</strong>
                      </span>
                      <span>
                        <small>{copy("Transport", "传输")}</small>
                        <strong>{receipt.stream ? copy("Streaming", "流式") : copy("Non-streaming", "非流式")}</strong>
                      </span>
                      {receipt.price_version != null && (
                        <span><small>{copy("Price version", "价格版本")}</small><strong>v{receipt.price_version}</strong></span>
                      )}
                    </div>
                    {receipt.usage && (
                      <div className="usage-log-token-facts">
                        <span>{copy("Input", "输入")} <strong>{receipt.usage.input_tokens.toLocaleString(language)}</strong></span>
                        <span>{copy("Output", "输出")} <strong>{receipt.usage.output_tokens.toLocaleString(language)}</strong></span>
                        <span>{copy("Cache read", "缓存读")} <strong>{receipt.usage.cache_read_tokens.toLocaleString(language)}</strong></span>
                        <span>{copy("Cache write", "缓存写")} <strong>{receipt.usage.cache_write_tokens.toLocaleString(language)}</strong></span>
                        <span>{copy("Reasoning", "推理")} <strong>{receipt.usage.reasoning_tokens.toLocaleString(language)}</strong></span>
                      </div>
                    )}
                    <CostState receipt={receipt} />
                    <ReceiptDetails receipt={receipt} />
                    <RequestPlaintext
                      plaintext={data.plaintext_by_request_id?.[receipt.request_id]}
                      error={data.plaintext_errors_by_request_id?.[receipt.request_id]}
                    />
                  </div>
                </details>
              );
            })}
          </div>
          <footer className="usage-log-pagination">
            <button
              type="button"
              disabled={page <= 1 || loading}
              onClick={() => setPage((value) => Math.max(1, value - 1))}
            >
              {copy("Previous", "上一页")}
            </button>
            <span>{copy(`Page ${page} of ${totalPages}`, `第 ${page} / ${totalPages} 页`)}</span>
            <button
              type="button"
              aria-label={copy("Next page", "下一页")}
              disabled={page >= totalPages || loading}
              onClick={() => setPage((value) => Math.min(totalPages, value + 1))}
            >
              {copy("Next", "下一页")}
            </button>
          </footer>
        </>
      )}
    </section>
  );
}
