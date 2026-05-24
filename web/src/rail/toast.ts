// Toast stack manager (v6 §B): white bg / dark text / top-right / slide / 4s fade.
// Cap: 6 visible; newest at top (flex-direction: column-reverse on container).

interface ToastEntry {
  id: number;
  el: HTMLDivElement;
  expireAt: number;
}

const stack: ToastEntry[] = [];
let nextId = 0;
const MAX_TOASTS = 6;

// Rate-limit speciation toasts: only show the first within a 2-second window.
// Events still all land in the log; this only suppresses the UX pop-up burst.
const SPECIATION_TOAST_WINDOW_MS = 2000;
let lastSpeciationToastAt = -Infinity;

/**
 * Push a new toast, with optional rate-limiting for Speciation events.
 * Pass isSpeciation=true to apply the 2-second dedup window.
 */
export function pushToast(text: string, isSpeciation = false): void {
  const now = performance.now();
  if (isSpeciation) {
    if (now - lastSpeciationToastAt < SPECIATION_TOAST_WINDOW_MS) {
      return; // suppress; event is already in the log
    }
    lastSpeciationToastAt = now;
  }
  // Enforce cap: remove oldest (index 0 = oldest in stack order).
  while (stack.length >= MAX_TOASTS) {
    const oldest = stack.shift()!;
    oldest.el.remove();
  }

  const el = document.createElement("div");
  el.className = "toast";
  el.textContent = text;
  const container = document.getElementById("toast-stack");
  if (!container) return;
  container.appendChild(el);

  // Force reflow then add is-visible class for slide-in animation.
  void el.offsetWidth;
  el.classList.add("is-visible");

  stack.push({ id: nextId++, el, expireAt: now + 4000 });
}

/**
 * Tick toasts: start fade-out 200ms before expiry, remove after expiry.
 * Called each frame from pollRail.
 */
export function tickToasts(nowMs: number): void {
  for (let i = stack.length - 1; i >= 0; i--) {
    const t = stack[i];
    if (nowMs > t.expireAt - 200 && !t.el.classList.contains("is-leaving")) {
      t.el.classList.remove("is-visible");
      t.el.classList.add("is-leaving");
    }
    if (nowMs > t.expireAt) {
      t.el.remove();
      stack.splice(i, 1);
    }
  }
}
