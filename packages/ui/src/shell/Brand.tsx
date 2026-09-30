/// <reference path="../assets.d.ts" />
import iconUrl from "../assets/layrs-icon.svg";

/** The original icon, unmodified, with the wordmark set in Audiowide. */
export function Brand({ compact = false, size = 30 }: { compact?: boolean; size?: number }) {
  return (
    <span className="layrs-brand">
      <img src={iconUrl} alt="" width={size} height={Math.round((size * 104) / 108)} className="layrs-brand__icon" />
      {compact ? <span className="layrs-visually-hidden">Layrs</span> : <span className="layrs-brand__word">layrs</span>}
    </span>
  );
}
