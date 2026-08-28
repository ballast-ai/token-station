export type LaunchPhase = "presenting" | "exiting";

interface LaunchScreenProps {
  phase: LaunchPhase;
}

/** Independent product launch artwork shown while the first local scan settles. */
export default function LaunchScreen({ phase }: LaunchScreenProps) {
  const status = "Opening Token Station";

  return (
    <section
      className="launch-screen"
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
          <svg viewBox="0 0 256 256" focusable="false">
            <path fill="currentColor" d="M42 185 91 140h21l-50 45H42ZM88 197l57-57h21l-58 57H88ZM100 113l42-44h21l-43 44h-20ZM157 113l31-31h20l-30 31h-21Z" />
            <path fill="#f04b2f" d="M58 197 180 60h20L78 197H58Z" />
            <rect className="launch-symbol-bar" x="88" y="121" width="80" height="14" />
          </svg>
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
