interface TokenStationMarkProps {
  size?: number;
  className?: string;
}

/** Shared wordless Token Station product artwork; theme adaptation is CSS-only for recovery safety. */
export default function TokenStationMark({ size = 28, className = "" }: TokenStationMarkProps) {
  return (
    <span
      className={`token-station-mark${className ? ` ${className}` : ""}`}
      data-testid="token-station-mark"
      aria-hidden="true"
      style={{ width: size, height: size }}
    >
      <img data-testid="station-brand-icon" src="/icon.png" alt="" />
    </span>
  );
}
