import type { AgentView } from "./api";

/** Management survives gateway stops. Connection status describes runtime reachability. */
export function hasManagedInstallation(agent: AgentView | undefined): boolean {
  return Boolean(agent?.installations.some((installation) => installation.managed)
    || agent?.status === "CONNECTED");
}
