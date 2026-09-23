/**
 * Copies text: async Clipboard API first, then a hidden-textarea
 * `execCommand("copy")` fallback (WebView2 can deny the async API when the
 * window is not focused). Resolves true on success.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (typeof navigator !== "undefined" && navigator.clipboard && typeof navigator.clipboard.writeText === "function") {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall through to the textarea fallback
  }
  return fallbackCopy(text);
}

function fallbackCopy(text: string): boolean {
  if (typeof document === "undefined") return false;
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.setAttribute("aria-hidden", "true");
  ta.className = "visually-hidden-input";
  const active = document.activeElement as HTMLElement | null;
  document.body.appendChild(ta);
  try {
    ta.focus();
    ta.select();
    return typeof document.execCommand === "function" ? document.execCommand("copy") : false;
  } catch {
    return false;
  } finally {
    ta.remove();
    active?.focus?.();
  }
}
