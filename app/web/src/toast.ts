// Tiny transient-notice helper. Used by the Settings tab to surface
// "construction-only" changes after Apply / Reset.

const HOST_ID = "toast-host";

export function showToast(message: string, durationMs: number = 3400): void {
  const host = document.getElementById(HOST_ID);
  if (!host) return;
  const el = document.createElement("div");
  el.className = "toast";
  el.textContent = message;
  host.appendChild(el);
  window.setTimeout(() => el.remove(), durationMs);
}
