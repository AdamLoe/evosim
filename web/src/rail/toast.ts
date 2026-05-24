// Toast stack manager (v6 §B): white bg / dark text / top-right / slide / 4s fade.
// Cap: 6 visible; newest at top (flex-direction: column-reverse on container).
//
// Toasts are suppressed for v1.1 revisit (Events panel is hidden). The
// interface, stack, and tickToasts remain so re-enablement is a one-liner.

interface ToastEntry {
  el: HTMLDivElement;
  expireAt: number;
}

const stack: ToastEntry[] = [];

/**
 * Push a new toast, with optional rate-limiting for Speciation events.
 * Pass isSpeciation=true to apply the 2-second dedup window.
 *
 * No-op while toasts are suppressed for v1.1 revisit.
 */
export function pushToast(_text: string, _isSpeciation = false): void {
  // No-op: toasts suppressed for v1.1 revisit.
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
