import React, { useCallback, useEffect, useState } from "react";
import App from "./App";
import { getRecoveryState, type RecoveryState } from "./api";
import LaunchScreen, { type LaunchPhase } from "./components/LaunchScreen";
import RecoveryShell from "./components/RecoveryShell";

export const LAUNCH_MINIMUM_MS = 1_250;
export const LAUNCH_EXIT_MS = 420;

function reducedMotionRequested(): boolean {
  return typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function AppBootstrap() {
  const [recovery, setRecovery] = useState<RecoveryState | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [startupSettled, setStartupSettled] = useState(false);
  const [minimumElapsed, setMinimumElapsed] = useState(reducedMotionRequested);
  const [launchPhase, setLaunchPhase] = useState<LaunchPhase | "hidden">("presenting");
  const onStartupSettled = useCallback(() => setStartupSettled(true), []);

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
    if (minimumElapsed) return undefined;
    const timer = window.setTimeout(() => setMinimumElapsed(true), LAUNCH_MINIMUM_MS);
    return () => window.clearTimeout(timer);
  }, [minimumElapsed]);

  useEffect(() => {
    if (launchPhase === "hidden") return undefined;
    if (error || recovery?.mode === "safe") {
      setLaunchPhase("hidden");
      return undefined;
    }
    if (recovery?.mode !== "normal" || !startupSettled || !minimumElapsed) return undefined;
    if (reducedMotionRequested()) {
      setLaunchPhase("hidden");
      return undefined;
    }
    setLaunchPhase("exiting");
    const timer = window.setTimeout(() => setLaunchPhase("hidden"), LAUNCH_EXIT_MS);
    return () => window.clearTimeout(timer);
  }, [error, launchPhase, minimumElapsed, recovery?.mode, startupSettled]);

  if (error) return <RecoveryShell initialError={error} />;
  if (recovery?.mode === "safe") return <RecoveryShell initialState={recovery} />;

  const launchVisible = launchPhase !== "hidden";
  return (
    <>
      {recovery?.mode === "normal" && (
        <div className="launch-app-stage" aria-hidden={launchVisible || undefined}>
          <App onStartupSettled={onStartupSettled} />
        </div>
      )}
      {launchVisible && <LaunchScreen phase={launchPhase} />}
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
