import type { CSSProperties } from "react";
import TokenStationMark from "./TokenStationMark";

export type LaunchPhase = "presenting" | "exiting";

interface LaunchScreenProps {
  phase: LaunchPhase;
  exitDurationMs: number;
}

/** Independent product launch artwork with a bounded presentation. */
export default function LaunchScreen({ phase, exitDurationMs }: LaunchScreenProps) {
  const status = "Opening Token Station";

  return (
    <section
      className="launch-screen"
      style={{ "--launch-exit-duration": `${exitDurationMs}ms` } as CSSProperties}
      data-phase={phase}
      data-testid="launch-screen"
      role="status"
      aria-label={status}
      aria-live="polite"
      aria-busy="true"
    >
      <div className="launch-coordinate launch-coordinate-top" aria-hidden="true">
        <span>TS</span>
        <span>LOCAL ROUTER</span>
      </div>

      <div className="launch-composition">
        <div className="launch-switch" aria-hidden="true">
          <TokenStationMark size="100%" />
        </div>

        <div className="launch-identity">
          <h1>Token Station</h1>
          <p>LOCAL AI REQUEST ROUTER</p>
        </div>

        <div className="launch-status-line" aria-hidden="true">
          <i />
          <span>{status}</span>
        </div>
      </div>

      <div className="launch-coordinate launch-coordinate-bottom" aria-hidden="true">
        <span>PRIVATE BY DEFAULT</span>
        <span>ON DEVICE</span>
      </div>
    </section>
  );
}
