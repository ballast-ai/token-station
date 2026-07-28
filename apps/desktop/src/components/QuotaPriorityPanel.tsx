import { useState } from "react";
import { Info } from "lucide-react";
import type { ProviderView } from "../api";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

interface QuotaPriorityPanelProps {
  providers: ProviderView[];
  busy: boolean;
  applying: boolean;
  saveStatus: string;
  onSave: () => void;
}

/**
 * Main panel for quota-first mode: select any number of providers for rotation. Sort by earliest refresh with remaining quota.
 * Use accounts with the earliest expiration first. This mode does not show keyword routing or local-only routing.
 */
export default function QuotaPriorityPanel({
  providers,
  busy,
  applying,
  saveStatus,
  onSave,
}: QuotaPriorityPanelProps) {
  // Providers in quota rotation. TODO (remaining 4a): persist them in upstream.quota_plan.
  const [selected, setSelected] = useState<Set<string>>(new Set());

  const toggle = (name: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(name)) {
        next.delete(name);
      } else {
        next.add(name);
      }
      return next;
    });
  };

  return (
    <div className="quota-ui">
      <Card>
        <CardHeader>
          <div className="flex items-start justify-between gap-4">
            <div>
              <CardTitle className="text-xl">额度优先</CardTitle>
              <CardDescription className="mt-1">
                选择参与轮换的供应商(不限数量)。请求会优先用「最快刷新、且还有额度」的账户,
                把快过期的付费/白嫖额度先用掉,不浪费。
              </CardDescription>
            </div>
            <span className="shrink-0 rounded-full bg-accent px-3 py-1 text-xs font-medium text-accent-foreground">
              全局默认
            </span>
          </div>
        </CardHeader>

        <CardContent className="flex flex-col gap-4">
          <div className="flex items-start gap-2 rounded-lg border border-indigo-100 bg-indigo-50 px-3 py-2.5 text-xs text-slate-600">
            <Info className="mt-0.5 size-4 shrink-0 text-indigo-600" />
            <span>
              若各家剩余额度相差不多,将<strong className="text-slate-900">按你添加供应商的先后顺序</strong>调用。
            </span>
          </div>

          {providers.length === 0 ? (
            <p className="py-6 text-center text-sm text-muted-foreground">
              还没有供应商——先去「添加供应商」接入你的账户。
            </p>
          ) : (
            <div className="flex flex-col gap-2">
              {providers.map((provider) => {
                const isSelected = selected.has(provider.name);
                return (
                  <button
                    key={provider.name}
                    type="button"
                    disabled={busy}
                    onClick={() => toggle(provider.name)}
                    className={[
                      "flex items-center gap-3 rounded-lg border px-4 py-3 text-left transition-colors",
                      isSelected
                        ? "border-primary bg-accent"
                        : "border-border bg-card hover:bg-muted",
                      busy ? "cursor-not-allowed opacity-60" : "cursor-pointer",
                    ].join(" ")}
                  >
                    <span
                      className={[
                        "flex size-5 shrink-0 items-center justify-center rounded-md border",
                        isSelected ? "border-primary bg-primary text-primary-foreground" : "border-input",
                      ].join(" ")}
                    >
                      {isSelected ? "✓" : ""}
                    </span>
                    <span className="flex flex-col">
                      <span className="text-sm font-medium text-foreground">{provider.name}</span>
                      {provider.local ? (
                        <span className="text-xs text-muted-foreground">本地</span>
                      ) : null}
                    </span>
                  </button>
                );
              })}
            </div>
          )}
        </CardContent>

        <div className="flex items-center gap-3 px-6 pb-6">
          <Button disabled={busy || applying} onClick={onSave}>
            {applying ? "应用中…" : "保存并应用"}
          </Button>
          <span className="text-xs text-muted-foreground">{saveStatus}</span>
        </div>
      </Card>
    </div>
  );
}
