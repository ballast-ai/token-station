import React, { useCallback, useEffect, useState } from "react";
import App, { type StartupOutcome } from "./App";
import { getRecoveryState, type RecoveryState } from "./api";
import LaunchScreen, { type LaunchPhase } from "./components/LaunchScreen";
import RecoveryShell from "./components/RecoveryShell";
import { useLocalizedCopy } from "./components/LanguageProvider";

export const LAUNCH_MINIMUM_MS = 2_400;
export const LAUNCH_EXIT_MS = 300;

function reducedMotionRequested(): boolean {
  return typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function AppBootstrap() {
  const { copy } = useLocalizedCopy();
  const [recovery, setRecovery] = useState<RecoveryState | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [startupOutcome, setStartupOutcome] = useState<StartupOutcome | null>(null);
  const [launchPhase, setLaunchPhase] = useState<LaunchPhase | "hidden">("presenting");
  const onStartupSettled = useCallback((outcome: StartupOutcome) => setStartupOutcome(outcome), []);
  const skipLaunch = Boolean(error || recovery?.mode === "safe"
    || startupOutcome === "actionable-error" || reducedMotionRequested());

  useEffect(() => {
    let disposed = false;
    void getRecoveryState()
      .then((state) => { if (!disposed) setRecovery(state); })
      .catch((caught) => {
        if (!disposed) setError(caught instanceof Error ? caught : new Error(String(caught)));
      });
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    if (skipLaunch) {
      setLaunchPhase("hidden");
      return undefined;
    }
    // Both deadlines belong to the presentation, independent of backend readiness.
    const exitTimer = window.setTimeout(() => setLaunchPhase("exiting"), LAUNCH_MINIMUM_MS);
    const hideTimer = window.setTimeout(() => setLaunchPhase("hidden"), LAUNCH_MINIMUM_MS + LAUNCH_EXIT_MS);
    return () => {
      window.clearTimeout(exitTimer);
      window.clearTimeout(hideTimer);
    };
  }, [skipLaunch]);

  if (error) return <RecoveryShell initialError={error} />;
  if (recovery?.mode === "safe") return <RecoveryShell initialState={recovery} />;

  const launchVisible = launchPhase !== "hidden";
  return (
    <>
      {recovery?.mode === "normal" && (
        <div
          className="launch-app-stage"
          aria-hidden={launchVisible || undefined}
          inert={launchVisible || undefined}
        >
          <App onStartupSettled={onStartupSettled} launchComplete={!launchVisible} />
        </div>
      )}
      {!recovery && !launchVisible && (
        <div className="loading-screen" role="status" aria-live="polite" aria-busy="true">
          <span className="loading-mark" aria-hidden="true"><i /><i /><i /></span>
          <strong>{copy("Opening Token Station", "正在进入 Token Station", "開啟 Token Station", "Token Station を開きます")}</strong>
        </div>
      )}
      {launchVisible && <LaunchScreen phase={launchPhase} exitDurationMs={LAUNCH_EXIT_MS} />}
    </>
  );
}

export class RecoveryBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  render() {
    if (this.state.error) return <RecoveryShell initialError={this.state.error} />;
    return this.props.children;
  }
}
