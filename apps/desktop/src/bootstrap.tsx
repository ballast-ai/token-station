import React, { useEffect, useState } from "react";
import App from "./App";
import { getRecoveryState, type RecoveryState } from "./api";
import RecoveryShell from "./components/RecoveryShell";
import { useLocalizedCopy } from "./components/LanguageProvider";

export function AppBootstrap() {
  const { copy } = useLocalizedCopy();
  const [recovery, setRecovery] = useState<RecoveryState | null>(null);
  const [error, setError] = useState<Error | null>(null);

  useEffect(() => {
    let disposed = false;
    void getRecoveryState()
      .then((state) => { if (!disposed) setRecovery(state); })
      .catch((caught) => {
        if (!disposed) setError(caught instanceof Error ? caught : new Error(String(caught)));
      });
    return () => { disposed = true; };
  }, []);

  if (error) return <RecoveryShell initialError={error} />;
  if (!recovery) {
    return (
      <div className="loading-screen">
        <span className="loading-mark" aria-hidden="true"><i /><i /><i /></span>
        <strong>{copy(
          "Checking local data compatibility",
          "正在检查本地数据兼容性",
        )}</strong>
      </div>
    );
  }
  if (recovery.mode === "safe") return <RecoveryShell initialState={recovery} />;
  return <App />;
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
