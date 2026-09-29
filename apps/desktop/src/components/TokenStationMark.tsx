interface TokenStationMarkProps {
  size?: number | "100%";
  className?: string;
}

/** Shared wordless Token Station product artwork; theme adaptation is CSS-only for recovery safety. */
export default function TokenStationMark({ size = 28, className = "" }: TokenStationMarkProps) {
  const suffix = typeof size === "number" && size <= 24 ? "-small" : "";

  return (
    <span
      className={`token-station-mark${className ? ` ${className}` : ""}`}
      data-testid="token-station-mark"
      aria-hidden="true"
      style={{ width: size, height: size }}
    >
      <img
        className="token-station-mark-light"
        data-testid="station-brand-icon"
        src={`/logo${suffix}.svg`}
        alt=""
      />
      <img className="token-station-mark-dark" src={`/logo${suffix}-dark.svg`} alt="" />
    </span>
  );
}
