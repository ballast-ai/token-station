import { type FrontendDiagnosticInput, recordFrontendDiagnostic } from "./api";

const MESSAGE_LIMIT = 4096;
const STACK_LIMIT = 12_000;
const COMPONENT_LIMIT = 6000;

function bounded(value: string, limit: number): string {
  if (value.length <= limit) return value;
  return `${value.slice(0, Math.max(0, limit - 12))}…[truncated]`;
}

export function redactDiagnosticText(value: string): string {
  return value
    .replace(/\bBearer\s+[a-z0-9._~+/-]{3,}/gi, "Bearer [REDACTED]")
    .replace(/\b(?:sk|pk)[-_][a-z0-9_-]{4,}/gi, "[REDACTED]")
    .replace(/\bAIza[0-9A-Za-z_-]{12,}/g, "[REDACTED]")
    .replace(/((?:api[_-]?key|access[_-]?token|token|password|secret|authorization)\s*[:=]\s*["']?)[^"'\s,;}]{3,}/gi, "$1[REDACTED]")
    .replace(/(["']?(?:request[_-]?body|body|prompt|content|input|tool[_-]?input|toolInput|arguments|query|search[_-]?(?:term|query))["']?\s*[:=]\s*)[\s\S]*/gi, "$1[REDACTED]");
}

function errorFields(value: unknown): { message: string; stack: string | null } {
  if (value instanceof Error) {
    return { message: value.message || value.name, stack: value.stack ?? null };
  }
  if (typeof value === "string") return { message: value, stack: null };
  if (value && typeof value === "object") {
    const candidate = value as { message?: unknown; stack?: unknown };
    return {
      message: typeof candidate.message === "string" ? candidate.message : "未知运行时异常",
      stack: typeof candidate.stack === "string" ? candidate.stack : null,
    };
  }
  return { message: String(value), stack: null };
}

export function diagnosticInput(
  kind: FrontendDiagnosticInput["kind"],
  value: unknown,
  componentStack: string | null = null,
): FrontendDiagnosticInput {
  const { message, stack } = errorFields(value);
  return {
    kind,
    message: bounded(redactDiagnosticText(message), MESSAGE_LIMIT),
    stack: stack ? bounded(redactDiagnosticText(stack), STACK_LIMIT) : null,
    component_stack: componentStack
      ? bounded(redactDiagnosticText(componentStack), COMPONENT_LIMIT)
      : null,
  };
}

export function persistFrontendDiagnostic(input: FrontendDiagnosticInput): void {
  void recordFrontendDiagnostic(input).catch(() => undefined);
}

/** Installs process-lifetime listeners. The returned function is useful in tests. */
export function installGlobalDiagnostics(): () => void {
  const onError = (event: ErrorEvent) => {
    persistFrontendDiagnostic(
      diagnosticInput("window_error", event.error ?? event.message),
    );
  };
  const onUnhandledRejection = (event: PromiseRejectionEvent) => {
    persistFrontendDiagnostic(diagnosticInput("unhandled_rejection", event.reason));
  };
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onUnhandledRejection);
  return () => {
    window.removeEventListener("error", onError);
    window.removeEventListener("unhandledrejection", onUnhandledRejection);
  };
}
