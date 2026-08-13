export const AGENT_VISIBILITY_STORAGE_KEY = "token-station-hidden-agent-ids";

const MAX_HIDDEN_AGENT_IDS = 64;
const AGENT_ID_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

function isAgentId(value: unknown): value is string {
  return typeof value === "string"
    && value.length <= 64
    && AGENT_ID_PATTERN.test(value);
}

function normalizedAgentIds(values: Iterable<unknown>): string[] {
  const unique = new Set<string>();
  for (const value of values) {
    if (isAgentId(value)) unique.add(value);
  }
  return [...unique]
    .sort((left, right) => left < right ? -1 : left > right ? 1 : 0)
    .slice(0, MAX_HIDDEN_AGENT_IDS);
}

export function updateHiddenAgentIds(
  ids: ReadonlySet<string>,
  agentId: string,
  hidden: boolean,
): Set<string> {
  const next = new Set(normalizedAgentIds(ids));
  if (!isAgentId(agentId)) return next;

  if (!hidden) {
    next.delete(agentId);
    return new Set(normalizedAgentIds(next));
  }
  if (next.has(agentId)) return next;

  if (next.size >= MAX_HIDDEN_AGENT_IDS) {
    const sorted = [...next].sort();
    const evicted = sorted[sorted.length - 1];
    if (evicted) next.delete(evicted);
  }
  next.add(agentId);
  return new Set(normalizedAgentIds(next));
}

export function readHiddenAgentIds(): Set<string> {
  try {
    if (typeof window === "undefined") return new Set();
    const raw = window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY);
    if (raw == null) return new Set();
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed)
      ? new Set(normalizedAgentIds(parsed))
      : new Set();
  } catch {
    return new Set();
  }
}

export function writeHiddenAgentIds(ids: ReadonlySet<string>): boolean {
  try {
    if (typeof window === "undefined") return true;
    const normalized = normalizedAgentIds(ids);
    window.localStorage.setItem(
      AGENT_VISIBILITY_STORAGE_KEY,
      JSON.stringify(normalized),
    );
    return true;
  } catch {
    // UI preferences are best-effort and must never block navigation changes.
    return false;
  }
}
