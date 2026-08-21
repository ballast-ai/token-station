import { ArrowRight } from "lucide-react";
import type { StateView } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";

export default function RevisionChain({ state }: { state: StateView }) {
  const { copy } = useLocalizedCopy();
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  const runningRevision = state.serve.running_revision;
  const savedApplied = runtimeHealthy && runningRevision === state.saved_revision;
  const draftLabel = state.config_dirty ? copy("Draft", "草稿", "草案", "下書き") : copy("Saved", "已保存", "已儲存", "保存済み");
  const draftRevision = state.config_dirty ? state.draft_revision : state.saved_revision;
  const middleLabel = state.serve.phase === "starting"
    ? copy("Applying", "正在应用", "應用中", "適用中")
    : savedApplied
      ? copy("Applied", "已应用", "已應用", "適用済み")
      : copy("Pending apply", "待应用", "待應用", "適用待ち");
  const runtimeLabel = runtimeHealthy ? copy("Running", "运行中", "執行中", "実行中") : copy("Not running", "未运行", "未執行", "実行されていません");
  const accessible = copy(
    `${draftLabel} revision ${draftRevision}; ${middleLabel}; ${runtimeLabel}${runningRevision == null ? "" : ` revision ${runningRevision}`}`,
    `${draftLabel} revision ${draftRevision}；${middleLabel}；${runtimeLabel}${runningRevision == null ? "" : ` revision ${runningRevision}`}`, `${draftLabel} 版本 ${draftRevision}；${middleLabel}；${runtimeLabel}${runningRevision == null ? "" : ` revision ${runningRevision}`}`, `${draftLabel} バージョン ${draftRevision}；${middleLabel}；${runtimeLabel}${runningRevision == null ? "" : ` revision ${runningRevision}`}`
  );

  return (
    <div className="revision-chain" data-testid="revision-chain" aria-label={accessible}>
      <span className={`revision-step ${state.config_dirty ? "draft" : "saved"}`}>
        <small>{draftLabel}</small>
        <strong>rev {draftRevision}</strong>
      </span>
      <ArrowRight aria-hidden="true" />
      <span className={`revision-step ${savedApplied ? "applied" : "pending"}`}>
        <small>{middleLabel}</small>
        <strong>{state.serve.phase === "starting" ? "…" : savedApplied ? copy("Synced", "同步", "已同步", "同期済み") : copy("Waiting", "等待", "等待中", "待機中")}</strong>
      </span>
      <ArrowRight aria-hidden="true" />
      <span className={`revision-step ${runtimeHealthy ? "running" : "stopped"}`}>
        <small>{runtimeLabel}</small>
        <strong>{runningRevision == null ? "—" : `rev ${runningRevision}`}</strong>
      </span>
    </div>
  );
}
