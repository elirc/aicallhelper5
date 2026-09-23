import { useApp } from "../app/view";

export const PROTECTED_TEXT = "Hidden from screen capture";
export const UNKNOWN_TEXT = "Screen-share protection not confirmed yet";
export const UNPROTECTED_TEXT = "Windows would not hide this window — it may be visible in screen shares";

/**
 * Screen-share protection verdict (spec §11). Shown in every view. Only
 * `protected` — which only Windows' read-back can produce — claims protection.
 */
export function ProtectionBadge({ compact = false }: { compact?: boolean }) {
  const { view } = useApp();
  const cls = `protection-badge${compact ? " protection-badge--compact" : ""}`;
  if (view.protection === "protected") {
    return (
      <span className={`${cls} protection-badge--ok`} data-testid="protection-badge" title={PROTECTED_TEXT}>
        <span className="protection-badge__icon" aria-hidden="true">●</span>
        <span className="protection-badge__text">{PROTECTED_TEXT}</span>
      </span>
    );
  }
  if (view.protection === "unprotected") {
    return (
      <span className={`${cls} protection-badge--bad`} role="alert" data-testid="protection-badge" title={UNPROTECTED_TEXT}>
        <span className="protection-badge__icon" aria-hidden="true">▲</span>
        <span className="protection-badge__text">{UNPROTECTED_TEXT}</span>
      </span>
    );
  }
  return (
    <span className={`${cls} protection-badge--unknown`} data-testid="protection-badge" title={UNKNOWN_TEXT}>
      <span className="protection-badge__icon" aria-hidden="true">◌</span>
      <span className="protection-badge__text">{UNKNOWN_TEXT}</span>
    </span>
  );
}
