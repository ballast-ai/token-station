import type { AppView } from "./components/AppShell";

const WORKSPACE_TARGETS = new Set<AppView>([
  "home",
  "add-provider",
  "logs",
  "settings",
]);

/** Keep native-menu payloads on the same explicit navigation boundary as UI actions. */
export function resolveStatusMenuNavigation(
  target: unknown,
  knownAgentIds: ReadonlySet<string>,
): AppView | null {
  if (typeof target !== "string") return null;
  if (WORKSPACE_TARGETS.has(target as AppView)) return target as AppView;
  if (!target.startsWith("agent:")) return null;
  const agentId = target.slice("agent:".length);
  return knownAgentIds.has(agentId) ? `agent:${agentId}` : null;
}
