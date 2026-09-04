import { useEffect, useId, useMemo, useRef, useState } from "react";
import { ChevronDown, Maximize2 } from "lucide-react";
import {
  getRequestReceipts,
  type ReceiptCostKind,
  type ReceiptPageView,
  type HttpHeaderView,
  type HttpRequestView,
  type HttpResponseView,
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";

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

function tokenTotal(receipt: ReceiptView, reported: (total: string) => string): string {
  if (!receipt.usage) return "—";
  const total = (receipt.usage.input_tokens + receipt.usage.output_tokens).toLocaleString();
  return receipt.usage_semantics === "provider_reported_v1" ? reported(total) : total;
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

type HttpChangeKind = "kept" | "modified" | "added" | "removed" | "redacted";

interface HttpChange {
  kind: HttpChangeKind;
  path: string;
  before?: string;
  after?: string;
}

function shortValue(value: unknown): string {
  let rendered: string;
  try {
    rendered = typeof value === "string" ? value : prettyValue(value);
  } catch {
    rendered = "<complex value>";
  }
  return rendered.length > 140 ? `${rendered.slice(0, 137)}…` : rendered;
}

export function diffJson(beforeText: string, afterText: string): HttpChange[] {
  let before: unknown;
  let after: unknown;
  try {
    before = JSON.parse(beforeText);
    after = JSON.parse(afterText);
  } catch {
    return beforeText === afterText ? [] : [{ kind: "modified", path: "body", before: shortValue(beforeText), after: shortValue(afterText) }];
  }
  const changes: HttpChange[] = [];
  const pending: Array<{ left: unknown; right: unknown; path: string; depth: number }> = [
    { left: before, right: after, path: "body", depth: 0 },
  ];
  let visited = 0;
  while (pending.length > 0 && changes.length < 100) {
    const current = pending.pop();
    if (!current || Object.is(current.left, current.right)) continue;
    visited += 1;
    if (current.depth > 256 || visited > 10_000) {
      return [{ kind: "modified", path: "body", before: "<comparison limit reached>", after: "<comparison limit reached>" }];
    }
    const { left, right, path, depth } = current;
    if (Array.isArray(left) && Array.isArray(right)) {
      const length = Math.max(left.length, right.length);
      for (let index = length - 1; index >= 0; index -= 1) {
        const child = `${path}[${index}]`;
        if (index >= left.length) changes.push({ kind: "added", path: child, after: shortValue(right[index]) });
        else if (index >= right.length) changes.push({ kind: "removed", path: child, before: shortValue(left[index]) });
        else pending.push({ left: left[index], right: right[index], path: child, depth: depth + 1 });
      }
      continue;
    }
    if (isObject(left) && isObject(right)) {
      const keys = [...new Set([...Object.keys(left), ...Object.keys(right)])];
      for (let index = keys.length - 1; index >= 0; index -= 1) {
        const key = keys[index];
        const child = path ? `${path}.${key}` : key;
        if (!(key in left)) changes.push({ kind: "added", path: child, after: shortValue(right[key]) });
        else if (!(key in right)) changes.push({ kind: "removed", path: child, before: shortValue(left[key]) });
        else pending.push({ left: left[key], right: right[key], path: child, depth: depth + 1 });
      }
      continue;
    }
    changes.push({ kind: "modified", path: path || "body", before: shortValue(left), after: shortValue(right) });
  }
  return changes;
}

function headerMap(headers: HttpHeaderView[]): Map<string, HttpHeaderView> {
  return new Map(headers.map((header) => [header.name.toLowerCase(), header]));
}

function requestChanges(source: HttpRequestView, target: HttpRequestView): HttpChange[] {
  const changes: HttpChange[] = [];
  if (source.method !== target.method) {
    changes.push({ kind: "modified", path: "method", before: source.method, after: target.method });
  }
  if (source.url !== target.url) {
    changes.push({ kind: "modified", path: "url", before: source.url, after: target.url });
  }
  const beforeHeaders = headerMap(source.headers);
  const afterHeaders = headerMap(target.headers);
  for (const [name, header] of beforeHeaders) {
    const next = afterHeaders.get(name);
    if (!next) changes.push({ kind: "removed", path: `header.${name}`, before: header.value });
    else if (next.redacted) changes.push({ kind: "redacted", path: `header.${name}`, before: "<redacted>", after: "<redacted>" });
    else if (header.value !== next.value) changes.push({ kind: "modified", path: `header.${name}`, before: header.value, after: next.value });
  }
  for (const [name, header] of afterHeaders) {
    if (!beforeHeaders.has(name)) {
      changes.push({ kind: header.redacted ? "redacted" : "added", path: `header.${name}`, after: header.value });
    }
  }
  changes.push(...diffJson(source.body, target.body));
  return changes;
}

function HttpPacketInspector({
  direction,
  request,
  response,
  headerKinds = new Map<string, HttpChangeKind>(),
}: {
  direction: "request" | "response";
  request?: HttpRequestView;
  response?: HttpResponseView;
  headerKinds?: Map<string, HttpChangeKind>;
}) {
  const { copy } = useLocalizedCopy();
  const isRequest = direction === "request";
  if ((isRequest && !request) || (!isRequest && !response)) {
    return (
      <div className="http-packet-empty" role="status">
        {isRequest
          ? copy("No request was captured", "未采集到请求", "未擷取到請求", "リクエストは取得されていません")
          : copy("No response was captured", "未采集到响应", "未擷取到回應", "レスポンスは取得されていません")}
      </div>
    );
  }
  const headers = request?.headers ?? response?.headers ?? [];
  const body = request?.body ?? response?.body ?? "";
  const truncated = request?.body_truncated ?? response?.body_truncated ?? false;
  const headerLabel = isRequest
    ? copy("Request headers", "请求头", "請求標頭", "リクエストヘッダー")
    : copy("Response headers", "响应头", "回應標頭", "レスポンスヘッダー");
  const bodyLabel = isRequest
    ? copy("Request body", "请求体", "請求本文", "リクエスト本文")
    : copy("Response body", "响应体", "回應本文", "レスポンス本文");
  return (
    <div className="http-packet-inspector">
      <section className="http-packet-section headers" role="region" aria-label={headerLabel}>
        <div className="http-packet-section-title">
          <strong>{headerLabel}</strong>
          <span>{copy(`${headers.length} entries`, `${headers.length} 项`, `${headers.length} 項`, `${headers.length} 件`)}</span>
        </div>
        <div className="http-header-list">
          {headers.length ? headers.map((header, index) => {
            const kind = header.redacted ? "redacted" : headerKinds.get(header.name.toLowerCase()) ?? "kept";
            return (
              <div className={`http-header-row ${kind}`} key={`${header.name}-${index}`}>
                <code>{header.name}</code>
                <code>{header.value}</code>
                {kind !== "kept" && <small>{copy(
                  kind === "redacted" ? "Redacted" : kind === "added" ? "Added" : kind === "modified" ? "Modified" : "Kept",
                  kind === "redacted" ? "已脱敏" : kind === "added" ? "新增" : kind === "modified" ? "修改" : "保留",
                  kind === "redacted" ? "已脫敏" : kind === "added" ? "新增" : kind === "modified" ? "修改" : "保留",
                  kind === "redacted" ? "編集済み" : kind === "added" ? "追加" : kind === "modified" ? "変更" : "保持",
                )}</small>}
              </div>
            );
          }) : <span className="http-empty">{isRequest
            ? copy("No request headers captured", "未采集到请求头", "未擷取到請求標頭", "リクエストヘッダーは取得されていません")
            : copy("No response headers captured", "未采集到响应头", "未擷取到回應標頭", "レスポンスヘッダーは取得されていません")}</span>}
        </div>
      </section>
      <section className="http-packet-section body" role="region" aria-label={bodyLabel}>
        <div className="http-packet-section-title">
          <strong>{bodyLabel}</strong>
          {truncated && <em>{copy("Truncated", "已截断", "已截斷", "切り捨て")}</em>}
        </div>
        <pre>{body ? formatJsonSource(body) : isRequest
          ? copy("Empty request body", "请求体为空", "請求本文為空", "リクエスト本文は空です")
          : copy("Empty response body", "响应体为空", "回應本文為空", "レスポンス本文は空です")}</pre>
      </section>
    </div>
  );
}

function HttpChangeList({ changes }: { changes: HttpChange[] }) {
  const { copy } = useLocalizedCopy();
  if (!changes.length) {
    return <div className="http-change-empty">{copy("No semantic changes detected", "未检测到语义改动", "未偵測到語義變更", "意味上の変更は検出されませんでした")}</div>;
  }
  return (
    <details className="http-change-list">
      <summary>
        <strong>{copy("Changes made by Token Station", "Token Station 改动", "Token Station 變更", "Token Station による変更")}</strong>
        <span className="http-change-summary-meta">
          <span>{copy(`${changes.length} changes`, `${changes.length} 项`, `${changes.length} 項`, `${changes.length} 件`)}</span>
          <span className="http-change-disclosure" aria-hidden="true"><ChevronDown /></span>
        </span>
      </summary>
      <div>
        {changes.map((change, index) => (
          <div className={`http-change-row ${change.kind}`} key={`${change.path}-${index}`}>
            <span>{copy(
              change.kind === "added" ? "Added" : change.kind === "removed" ? "Removed" : change.kind === "redacted" ? "Redacted" : "Modified",
              change.kind === "added" ? "新增" : change.kind === "removed" ? "删除" : change.kind === "redacted" ? "脱敏" : "修改",
              change.kind === "added" ? "新增" : change.kind === "removed" ? "刪除" : change.kind === "redacted" ? "脫敏" : "修改",
              change.kind === "added" ? "追加" : change.kind === "removed" ? "削除" : change.kind === "redacted" ? "編集" : "変更",
            )}</span>
            <code>{change.path}</code>
            <p>
              {change.before != null && <del>{change.before}</del>}
              {change.after != null && <ins>{change.after}</ins>}
            </p>
          </div>
        ))}
      </div>
    </details>
  );
}

function HttpTraceInspector({ plaintext }: { plaintext: RequestPlaintextView }) {
  const { copy } = useLocalizedCopy();
  const trace = plaintext.http_trace;
  const source = trace?.agent_request;
  const defaultConversationKey = trace?.upstream_exchanges[0]
    ? `upstream-${trace.upstream_exchanges[0].ordinal}`
    : "client";
  const [selectedConversationKey, setSelectedConversationKey] = useState(defaultConversationKey);
  const [direction, setDirection] = useState<"request" | "response">("request");
  useEffect(() => {
    setSelectedConversationKey(defaultConversationKey);
    setDirection("request");
  }, [defaultConversationKey, plaintext.request_id]);
  if (!trace || !source) return null;

  interface HttpConversation {
    key: string;
    title: string;
    peer: string;
    request: HttpRequestView;
    response?: HttpResponseView;
    changes?: HttpChange[];
    headerKinds?: Map<string, HttpChangeKind>;
  }

  const conversations: HttpConversation[] = [{
    key: "client",
    title: copy("Client conversation", "客户端会话", "用戶端會話", "クライアント会話"),
    peer: "Agent ↔ Token Station",
    request: source,
    response: trace.agent_response ?? undefined,
  }];

  for (const exchange of trace.upstream_exchanges) {
    const changes = requestChanges(source, exchange.request);
    const headerKinds = new Map<string, HttpChangeKind>();
    for (const change of changes) {
      if (change.path.startsWith("header.") && change.kind !== "removed") {
        headerKinds.set(change.path.slice(7), change.kind);
      }
    }
    conversations.push({
      key: `upstream-${exchange.ordinal}`,
      title: copy(
        `Upstream attempt ${exchange.ordinal}`,
        `上游尝试 ${exchange.ordinal}`,
        `上游嘗試 ${exchange.ordinal}`,
        `アップストリーム試行 ${exchange.ordinal}`,
      ),
      peer: `Token Station ↔ ${exchange.upstream}`,
      request: exchange.request,
      response: exchange.response ?? undefined,
      changes,
      headerKinds,
    });
  }

  const selected = conversations.find((conversation) => conversation.key === selectedConversationKey)
    ?? conversations[0];
  const startLine = direction === "request"
    ? `${selected.request.method} ${selected.request.url}`
    : selected.response
      ? `HTTP ${selected.response.status}`
      : copy("No response", "暂无响应", "暫無回應", "レスポンスなし");

  return (
    <div
      className="http-trace-inspector"
      role="region"
      aria-label={copy("HTTP execution trace", "HTTP 执行链路", "HTTP 執行鏈路", "HTTP 実行トレース")}
    >
      <div className="http-conversation-workbench">
        <div
          className="http-conversation-list"
          role="listbox"
          aria-label={copy("HTTP conversations", "HTTP 会话", "HTTP 會話", "HTTP 会話")}
        >
          <strong>{copy(
            `${conversations.length} conversations`,
            `${conversations.length} 个会话`,
            `${conversations.length} 個會話`,
            `${conversations.length} 件の会話`,
          )}</strong>
          {conversations.map((conversation) => {
            const selectedItem = conversation.key === selected.key;
            const statusClass = conversation.response
              && conversation.response.status >= 200
              && conversation.response.status < 400
              ? "success"
              : "error";
            return (
              <button
                type="button"
                role="option"
                aria-selected={selectedItem}
                className="http-conversation-item"
                key={conversation.key}
                onClick={() => {
                  setSelectedConversationKey(conversation.key);
                  setDirection("request");
                }}
              >
                <span>
                  <strong>{conversation.title}</strong>
                  <small>{conversation.peer}</small>
                </span>
                {conversation.changes?.length ? <small>{copy(
                  `${conversation.changes.length} transformations`,
                  `${conversation.changes.length} 项转换`,
                  `${conversation.changes.length} 項轉換`,
                  `${conversation.changes.length} 件の変換`,
                )}</small> : null}
                <em className={statusClass}>{conversation.response
                  ? `HTTP ${conversation.response.status}`
                  : copy("No response", "暂无响应", "暫無回應", "レスポンスなし")}</em>
              </button>
            );
          })}
        </div>
        <div className="http-conversation-detail">
          <header>
            <div>
              <strong>{selected.title}</strong>
              <span>{selected.peer}</span>
            </div>
            <div
              className="http-direction-tabs"
              role="tablist"
              aria-label={copy("HTTP direction", "HTTP 方向", "HTTP 方向", "HTTP 方向")}
            >
              <button type="button" role="tab" aria-selected={direction === "request"} onClick={() => setDirection("request")}>
                {copy("Request", "请求", "請求", "リクエスト")}
              </button>
              <button type="button" role="tab" aria-selected={direction === "response"} onClick={() => setDirection("response")}>
                {copy("Response", "响应", "回應", "レスポンス")}
              </button>
            </div>
          </header>
          <code className="http-conversation-start-line">{startLine}</code>
          <HttpPacketInspector
            direction={direction}
            request={direction === "request" ? selected.request : undefined}
            response={direction === "response" ? selected.response : undefined}
            headerKinds={direction === "request" ? selected.headerKinds : undefined}
          />
          {direction === "request" && selected.changes && <HttpChangeList changes={selected.changes} />}
        </div>
      </div>
    </div>
  );
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
  useEffect(() => {
    setViewMode("parsed");
  }, [plaintext?.request_id]);
  if (error) {
    return (
      <section className="request-plaintext" aria-label={copy("Plaintext input and output", "明文输入输出", "明文輸入輸出", "プレーンテキスト入出力")}>
        <div className="request-plaintext-state error-text" role="alert">
          {copy(`Failed to read plaintext: ${error}`, `明文读取失败：${error}`, `明文讀取失敗：${error}`, `プレーンテキストの読み込みに失敗：${error}`)}
        </div>
      </section>
    );
  }
  if (!plaintext) {
    return (
      <section className="request-plaintext" aria-label={copy("Plaintext input and output", "明文输入输出", "明文輸入輸出", "プレーンテキスト入出力")}>
        <div className="request-plaintext-state" role="status">
          {copy(
            "No plaintext is available for this request. It may predate this feature, have failed to write, or have been removed by retention.",
            "此请求没有可用明文。它可能早于本功能、写入失败或已被保留策略清理。", "此請求沒有可用明文。它可能早於本功能、寫入失敗或已被保留策略清理。", "このリクエストには利用可能なプレーンテキストがありません。これはこの機能以前のもの、書き込み失敗、または保持ポリシーにより削除された可能性があります。"
          )}
        </div>
      </section>
    );
  }
  const panels = [
    {
      key: "input",
      label: copy("Plaintext input", "明文输入", "明文輸入", "プレーンテキスト入力"),
      body: plaintext.input,
      truncated: plaintext.input_truncated,
      blocks: parseInputSemantics(plaintext.input),
    },
    {
      key: "output",
      label: copy("Plaintext output", "明文输出", "明文輸出", "プレーンテキスト出力"),
      body: plaintext.output,
      truncated: plaintext.output_truncated,
      blocks: parseOutputSemantics(plaintext.output),
    },
  ];
  const semanticLabel = (block: SemanticBlock) => {
    switch (block.kind) {
      case "system": return copy("System prompt", "系统提示词", "系統提示詞", "システムプロンプト");
      case "developer": return copy("Developer prompt", "开发者提示词", "開發者提示詞", "開発者プロンプト");
      case "user": return copy("User input", "用户输入", "使用者輸入", "ユーザー入力");
      case "assistant": return copy("Assistant output", "助手输出", "助手輸出", "アシスタント出力");
      case "thinking": return copy("Assistant thinking", "助手思考", "助手思考", "アシスタントの思考");
      case "tool-call": return block.detail
        ? copy(`Tool call · ${block.detail}`, `工具调用 · ${block.detail}`, `工具呼叫 · ${block.detail}`, `ツール呼び出し · ${block.detail}`)
        : copy("Tool call", "工具调用", "工具呼叫", "ツール呼び出し");
      case "tool-result": return block.detail
        ? copy(`Tool result · ${block.detail}`, `工具返回 · ${block.detail}`, `工具返回 · ${block.detail}`, `ツール結果 · ${block.detail}`)
        : copy("Tool result", "工具返回", "工具返回", "ツール結果");
      case "tools": return copy(`Tool definitions · ${block.detail} total`, `工具定义 · ${block.detail} 个`, `工具定義 · ${block.detail} 個`, `ツール定義 · ${block.detail} 個`);
      case "stream-end": return copy("Stream completed", "流式结束", "流式結束", "ストリーム終了");
      default: return copy("Unrecognized content", "未识别内容", "未識別內容", "未識別コンテンツ");
    }
  };
  return (
    <section className="request-plaintext" aria-label={copy("Plaintext input and output", "明文输入输出", "明文輸入輸出", "プレーンテキスト入出力")}>
      <header>
        <div className="request-plaintext-title">
          <strong>{copy("Content", "内容", "內容", "コンテンツ")}</strong>
          <span>{copy("Input and output", "输入与输出", "輸入與輸出", "入力と出力")}</span>
        </div>
        <div
          className="request-plaintext-mode"
          role="group"
          aria-label={copy("Body display mode", "正文显示方式", "正文顯示方式", "本文表示モード")}
        >
          <button
            type="button"
            aria-pressed={viewMode === "parsed"}
            onClick={() => setViewMode("parsed")}
          >
            {copy("Parsed", "解析视图", "解析檢視", "解析ビュー")}
          </button>
          <button
            type="button"
            aria-pressed={viewMode === "source"}
            onClick={() => setViewMode("source")}
          >
            {copy("Source", "原文", "原文", "原文")}
          </button>
        </div>
      </header>
      <div className="request-plaintext-grid">
        {panels.map((panel) => (
          <div className="request-plaintext-panel" key={panel.key}>
            <div className="request-plaintext-label">
              <span>{panel.key.toUpperCase()}</span>
              {panel.truncated && <em>{copy("Truncated", "已截断", "已截斷", "切り捨てられた")}</em>}
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
                )) : <span className="request-semantic-empty">{copy("Empty body", "空正文", "空本體", "空本文")}</span>}
                {panel.truncated && (
                  <div className="request-semantic-truncated">{copy("Body truncated", "正文已截断", "本體已截斷", "本文が切り捨てられています")}</div>
                )}
              </div>
            ) : (
              <pre
                className="request-plaintext-scroll"
                role="region"
                aria-label={panel.label}
                tabIndex={0}
              >
                {panel.body ? formatJsonSource(panel.body) : copy("Empty body", "空正文", "空本體", "空本文")}
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
      "上游未返回 Token，无法估算成本。", "上游未返回 Token，無法估算成本。", "上流から Token が返されなかったため、コストの推定ができません。"
    ),
    (model) => copy(`No price configured for model: ${model}`, `缺少模型价格：${model}`, `缺少模型價格：${model}`, `モデルの価格が設定されていません：${model}`),
  );
  if (kind === "unknown" || receipt.cost_micros == null) {
    return (
      <span className="usage-log-cost unknown" title={reason}>
        {compact ? copy("Unknown", "未知", "未知", "不明") : reason}
      </span>
    );
  }
  return (
    <span className={`usage-log-cost ${kind}`}>
      {costLabel(
        receipt,
        copy("Actual", "实际", "實際", "実際"),
        copy("Estimated", "估算", "估算", "推定"),
        copy("Cost unknown", "成本未知", "費用未知", "費用は不明"),
      )}
    </span>
  );
}

function ReceiptDetail({
  receipt,
  plaintext,
  plaintextError,
}: {
  receipt: ReceiptView;
  plaintext?: RequestPlaintextView;
  plaintextError?: string;
}) {
  const { language, copy } = useLocalizedCopy();
  const cancelled = receipt.status === 499;
  const success = receipt.status >= 200 && receipt.status < 400 && receipt.error_code == null;
  const hasHttpTrace = Boolean(plaintext?.http_trace?.agent_request);
  const [activeView, setActiveView] = useState<"content" | "http" | "routing">(
    hasHttpTrace ? "http" : "content",
  );
  const tabPrefix = useId();
  useEffect(() => {
    setActiveView(hasHttpTrace ? "http" : "content");
  }, [hasHttpTrace, receipt.request_id]);
  const detailViews = [
    { key: "content" as const, label: copy("Content", "内容", "內容", "コンテンツ") },
    ...(hasHttpTrace ? [{ key: "http" as const, label: "HTTP" }] : []),
    { key: "routing" as const, label: copy("Routing", "路由", "路由", "ルーティング") },
  ];
  return (
    <div className="usage-log-expanded">
      <section className="request-detail-overview" aria-label={copy("Request overview", "请求概览", "請求概覽", "リクエスト概要")}>
        <div className="request-detail-outcome">
          <strong className={success ? "success" : cancelled ? "cancelled" : "error"}>
            {cancelled ? copy("Cancelled", "已取消", "已取消", "キャンセル") : `HTTP ${receipt.status}`}
          </strong>
          <span>{receipt.latency_ms.toLocaleString(language)} ms</span>
          <span>{copy(
            `${receipt.attempts} upstream attempts`,
            `${receipt.attempts} 次上游尝试`,
            `${receipt.attempts} 次上游嘗試`,
            `${receipt.attempts} 回のアップストリーム試行`,
          )}</span>
        </div>
        <dl
          className="usage-log-facts"
          aria-label={copy("Request audit summary", "请求审计摘要", "請求稽核摘要", "リクエスト監査の概要")}
        >
          <div className="request-detail-fact request-detail-fact--identity"><dt>{copy("Request ID", "请求 ID", "請求 ID", "リクエスト ID")}</dt><dd><code>{receipt.request_id}</code></dd></div>
          <div className="request-detail-fact request-detail-fact--identity"><dt>{copy("Requested model", "请求模型", "請求模型", "リクエストモデル")}</dt><dd>{receipt.requested_model}</dd></div>
          <div className="request-detail-fact request-detail-fact--technical"><dt>{copy("Protocol", "协议", "協議", "プロトコル")}</dt><dd>{receipt.protocol}</dd></div>
          <div className="request-detail-fact request-detail-fact--technical"><dt>{copy("Endpoint", "端点", "端點", "エンドポイント")}</dt><dd>{receipt.request_method ?? "—"} · {receipt.path_kind ?? "unknown"}</dd></div>
          <div className="request-detail-fact request-detail-fact--technical"><dt>{copy("Transport", "传输", "傳輸", "トランスポート")}</dt><dd>{receipt.stream ? copy("Streaming", "流式", "流式", "ストリーム") : copy("Non-streaming", "非流式", "非流式", "非ストリーム")}</dd></div>
        {receipt.price_version != null && (
            <div className="request-detail-fact"><dt>{copy("Price version", "价格版本", "價格版本", "価格バージョン")}</dt><dd>v{receipt.price_version}</dd></div>
        )}
        </dl>
        <div className="request-detail-measures">
          {receipt.usage && (
            <div className="usage-log-token-facts">
              <span>{receipt.usage_semantics === "provider_reported_v1"
                ? copy("Input reported", "上游输入", "上游輸入", "報告された入力")
                : copy("Input", "输入", "輸入", "入力")} <strong>{receipt.usage.input_tokens.toLocaleString(language)}</strong></span>
              <span>{copy("Output", "输出", "輸出", "出力")} <strong>{receipt.usage.output_tokens.toLocaleString(language)}</strong></span>
              <span>{copy("Cache read", "缓存读", "快取讀取", "キャッシュ読み込み")} <strong>{receipt.usage.cache_read_tokens.toLocaleString(language)}</strong></span>
              <span>{copy("Cache write", "缓存写", "快取寫入", "キャッシュ書き込み")} <strong>{receipt.usage.cache_write_tokens.toLocaleString(language)}</strong></span>
              <span>{copy("Reasoning", "推理", "推理", "推論")} <strong>{receipt.usage.reasoning_tokens.toLocaleString(language)}</strong></span>
              {receipt.usage_semantics === "provider_reported_v1" && (
                <small>{copy(
                  "Historical provider-reported input may exclude cache tokens; totals are not canonical.",
                  "历史上游输入可能不含缓存 Token；总量并非规范总量。",
                  "歷史上游輸入可能不含快取 Token；總量並非規範總量。",
                  "過去のプロバイダー報告入力にはキャッシュトークンが含まれない場合があり、合計は正規化済みではありません。",
                )}</small>
              )}
            </div>
          )}
          <CostState receipt={receipt} />
        </div>
      </section>
      <div className="request-detail-navigation">
        <div
          role="tablist"
          aria-label={copy("Request detail views", "请求详情视图", "請求詳情檢視", "リクエスト詳細ビュー")}
        >
          {detailViews.map((view) => (
            <button
              type="button"
              role="tab"
              id={`${tabPrefix}-${view.key}-tab`}
              aria-controls={`${tabPrefix}-${view.key}-panel`}
              aria-selected={activeView === view.key}
              key={view.key}
              onClick={() => setActiveView(view.key)}
            >
              {view.label}
            </button>
          ))}
        </div>
        <span>{copy("Retained for 7 days by default", "默认保留 7 天", "預設保留 7 天", "デフォルトで7日間保持")}</span>
      </div>
      <section
        className="request-detail-view"
        role="tabpanel"
        id={`${tabPrefix}-${activeView}-panel`}
        aria-labelledby={`${tabPrefix}-${activeView}-tab`}
      >
        {activeView === "content" && <RequestPlaintext plaintext={plaintext} error={plaintextError} />}
        {activeView === "http" && plaintext && <HttpTraceInspector plaintext={plaintext} />}
        {activeView === "routing" && <ReceiptDetails receipt={receipt} />}
      </section>
    </div>
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
  const [selectedReceiptId, setSelectedReceiptId] = useState<string | null>(null);
  const requestLogRef = useRef<HTMLElement>(null);
  const receiptTriggerRef = useRef<HTMLButtonElement | null>(null);

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
  const selectedReceipt = data?.items.find((receipt) => receipt.request_id === selectedReceiptId);

  useEffect(() => {
    if (selectedReceiptId && data && !selectedReceipt) {
      setSelectedReceiptId(null);
    }
  }, [data, selectedReceipt, selectedReceiptId]);

  const range = useMemo(() => {
    if (!total) return copy("0 items", "0 条", "0 項", "0 項");
    const start = (page - 1) * PAGE_SIZE + 1;
    return `${start}–${Math.min(total, start + PAGE_SIZE - 1)} / ${total}`;
  }, [copy, page, total]);

  return (
    <section ref={requestLogRef} className="usage-request-log" tabIndex={-1}>
      <header>
        <div>
          <h2>{copy("Request log", "请求日志", "請求紀錄", "リクエストログ")}</h2>
          <p>{copy(
            "Complete local receipts · Plaintext bodies are retained separately for 7 days by default",
            "完整本地 Receipt · 正文明文独立存储，默认保留 7 天", "完整本機回執 · 純文字內文會獨立儲存，預設保留 7 天", "完全なローカルレシート · 平文の本文は別途保存され、既定では7日間保持されます"
          )}</p>
        </div>
        <div className="usage-log-tools">
          <div className="usage-log-filter">
            <span>{copy("Status", "状态", "狀態", "ステータス")}</span>
            <Select
              value={status || "all"}
              onValueChange={(next) => setStatus(next === "all" ? "" : (next as typeof status))}
            >
              <SelectTrigger
                className="usage-log-status-select"
                size="sm"
                aria-label={copy("Request status", "请求状态", "請求狀態", "リクエストステータス")}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                <SelectItem value="all">{copy("All statuses", "全部状态", "全部狀態", "すべてのステータス")}</SelectItem>
                <SelectItem value="success">{copy("Success", "成功", "成功", "成功")}</SelectItem>
                <SelectItem value="error">{copy("Error", "失败", "錯誤", "失敗")}</SelectItem>
              </SelectContent>
            </Select>
          </div>
          <strong>{range}</strong>
        </div>
      </header>

      {loading && !data && (
        <div className="usage-log-state">{copy("Loading request log…", "正在读取请求日志…", "正在讀取請求紀錄…", "リクエストログを読み込んでいます…")}</div>
      )}
      {error && (
        <div className="usage-log-state error-text">
          {copy(`Failed to load request log: ${error}`, `请求日志读取失败：${error}`, `請求紀錄讀取失敗：${error}`, `リクエストログの読み込みに失敗しました：${error}`)}
        </div>
      )}
      {!loading && !error && data?.items.length === 0 && (
        <div className="usage-log-state">{copy(
          "No requests match the current filters.",
          "当前筛选范围没有请求日志。", "目前沒有符合篩選條件的請求。", "現在のフィルター条件に一致するリクエストはありません。"
        )}</div>
      )}

      {data && data.items.length > 0 && (
        <>
          <div className="usage-log-head" aria-hidden="true">
            <span>{copy("Time", "时间", "時間", "時間")}</span>
            <span>{copy("Agent / Route", "Agent / 路由", "Agent / 路由", "Agent / ルーティング")}</span>
            <span>{copy("Status", "状态", "狀態", "ステータス")}</span>
            <span>Token</span>
            <span>{copy("Latency", "延迟", "延遲", "レイテンシー")}</span>
            <span>{copy("Cost", "成本", "成本", "コスト")}</span>
          </div>
          <div className={`usage-log-list ${loading ? "refreshing" : ""}`}>
            {data.items.map((receipt) => {
              const cancelled = receipt.status === 499;
              const success = receipt.status >= 200
                && receipt.status < 400
                && receipt.error_code == null;
              return (
                <button
                  className="usage-log-row"
                  key={receipt.request_id}
                  type="button"
                  onClick={(event) => {
                    receiptTriggerRef.current = event.currentTarget;
                    setSelectedReceiptId(receipt.request_id);
                  }}
                >
                  <span className="sr-only">{copy(
                    `Open request details ${receipt.request_id}. `,
                    `打开请求详情 ${receipt.request_id}。`,
                    `開啟請求詳情 ${receipt.request_id}。`,
                    `リクエスト詳細を開く ${receipt.request_id}。`,
                  )}</span>
                  <span className="usage-log-open-mark" aria-hidden="true"><Maximize2 /></span>
                  <time dateTime={new Date(receipt.started_at_ms).toISOString()}>
                    {formatTime(receipt.started_at_ms, language)}
                  </time>
                  <span className="usage-log-route">
                    <small>{receipt.agent_id ?? copy("Home", "主页", "主頁", "ホーム")}</small>
                    <strong>{routeOf(receipt, copy("No route", "未产生路由", "未產生路由", "ルーティングが生成されませんでした"))}</strong>
                  </span>
                  <span className={`usage-log-status ${success ? "success" : cancelled ? "" : "error"}`}>
                    {cancelled ? copy("Cancelled", "已取消", "已取消", "キャンセル") : `HTTP ${receipt.status}`}
                  </span>
                  <span>{tokenTotal(receipt, (total) => copy(
                    `${total} reported`,
                    `${total} 上游上报`,
                    `${total} 上游上報`,
                    `${total} 報告値`,
                  ))}</span>
                  <span className="usage-log-latency">{receipt.latency_ms.toLocaleString()} ms</span>
                  <CostState receipt={receipt} compact />
                </button>
              );
            })}
          </div>
          <footer className="usage-log-pagination">
            <button
              type="button"
              disabled={page <= 1 || loading}
              onClick={() => setPage((value) => Math.max(1, value - 1))}
            >
              {copy("Previous", "上一页", "上一頁", "前ページ")}
            </button>
            <span>{copy(`Page ${page} of ${totalPages}`, `第 ${page} / ${totalPages} 页`, `第 ${page} / ${totalPages} 頁`, `第 ${page} / ${totalPages} ページ`)}</span>
            <button
              type="button"
              aria-label={copy("Next page", "下一页", "下一頁", "次ページ")}
              disabled={page >= totalPages || loading}
              onClick={() => setPage((value) => Math.min(totalPages, value + 1))}
            >
              {copy("Next", "下一页", "下一頁", "次ページ")}
            </button>
          </footer>
          <Dialog
            open={Boolean(selectedReceipt)}
            onOpenChange={(open) => !open && setSelectedReceiptId(null)}
          >
            {selectedReceipt && (
              <DialogContent
                className="request-detail-dialog"
                closeLabel={copy("Close", "关闭", "關閉", "閉じる")}
                onCloseAutoFocus={(event) => {
                  event.preventDefault();
                  if (receiptTriggerRef.current?.isConnected) receiptTriggerRef.current.focus();
                  else requestLogRef.current?.focus();
                }}
              >
                <DialogHeader>
                  <DialogTitle>{copy("Request details", "请求详情", "請求詳情", "リクエスト詳細")}</DialogTitle>
                  <DialogDescription>
                    {formatTime(selectedReceipt.started_at_ms, language)} · {routeOf(selectedReceipt, copy("No route", "未产生路由", "未產生路由", "ルーティングが生成されませんでした"))}
                  </DialogDescription>
                </DialogHeader>
                <div className="request-detail-dialog-body">
                  <ReceiptDetail
                    receipt={selectedReceipt}
                    plaintext={data.plaintext_by_request_id?.[selectedReceipt.request_id]}
                    plaintextError={data.plaintext_errors_by_request_id?.[selectedReceipt.request_id]}
                  />
                </div>
              </DialogContent>
            )}
          </Dialog>
        </>
      )}
    </section>
  );
}
