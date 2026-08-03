import { ArrowRight } from "lucide-react";
import type { StateView } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";

export default function RevisionChain({ state }: { state: StateView }) {
  const { copy } = useLocalizedCopy();
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  const runningRevision = state.serve.running_revision;
  const savedApplied = runtimeHealthy && runningRevision === state.saved_revision;
  const draftLabel = state.config_dirty ? copy("Draft", "草稿") : copy("Saved", "已保存");
  const draftRevision = state.config_dirty ? state.draft_revision : state.saved_revision;
  const middleLabel = state.serve.phase === "starting"
    ? copy("Applying", "正在应用")
    : savedApplied
      ? copy("Applied", "已应用")
      : copy("Pending apply", "待应用");
  const runtimeLabel = runtimeHealthy ? copy("Running", "运行中") : copy("Not running", "未运行");
  const accessible = copy(
    `${draftLabel} revision ${draftRevision}; ${middleLabel}; ${runtimeLabel}${runningRevision == null ? "" : ` revision ${runningRevision}`}`,
    `${draftLabel} revision ${draftRevision}；${middleLabel}；${runtimeLabel}${runningRevision == null ? "" : ` revision ${runningRevision}`}`,
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
        <strong>{state.serve.phase === "starting" ? "…" : savedApplied ? copy("Synced", "同步") : copy("Waiting", "等待")}</strong>
      </span>
      <ArrowRight aria-hidden="true" />
      <span className={`revision-step ${runtimeHealthy ? "running" : "stopped"}`}>
        <small>{runtimeLabel}</small>
        <strong>{runningRevision == null ? "—" : `rev ${runningRevision}`}</strong>
      </span>
    </div>
  );
}
