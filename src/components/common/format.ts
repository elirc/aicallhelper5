/** "m:ss" for a millisecond duration (negative clamps to 0). */
export function formatClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${s.toString().padStart(2, "0")}`;
}

/** Seconds remaining before the core's auto-stop, or null outside the last 30 s. */
export function countdownSeconds(deadlineMs: number, now: number): number | null {
  const left = deadlineMs - now;
  if (left > 30_000 || left <= 0) return null;
  return Math.ceil(left / 1000);
}
