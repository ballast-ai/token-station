import { useEffect, useState } from "react";
import type { ProviderView } from "../api";
import { listChatGPTAccounts } from "../chatgptPlanApi";
import { listCodexAccounts } from "../codexAccountsApi";
import { providerPlanKind, type PlanConnectionKind } from "../planConnections";

interface AccountSummary { id: string; label: string; email: string | null; state: string }
interface Summaries {
  codex: AccountSummary[] | null;
  chatgpt: AccountSummary[] | null;
}

/** Read safe account metadata only. Never infer a live session from configured credentials. */
export function usePlanAccountSummaries(providers: ProviderView[]) {
  const [summaries, setSummaries] = useState<Summaries>({ codex: null, chatgpt: null });
  useEffect(() => {
    let stopped = false;
    setSummaries({ codex: null, chatgpt: null });
    for (const kind of ["codex", "chatgpt"] as const) {
      if (!providers.some(provider => providerPlanKind(provider) === kind)) continue;
      const read = kind === "codex" ? listCodexAccounts : listChatGPTAccounts;
      void read().then(accounts => {
        if (!stopped) setSummaries(current => ({ ...current, [kind]: accounts }));
      }).catch(() => { /* Keep the connection state unknown if metadata cannot be loaded. */ });
    }
    return () => { stopped = true; };
  }, [providers]);
  return (kind: PlanConnectionKind, accountId: string | undefined) => ({
    account: summaries[kind]?.find(account => account.id === accountId),
    loaded: summaries[kind] !== null,
  });
}
