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

/**
 * Push a new toast. Drops the oldest if cap is exceeded.
 */
export function pushToast(text: string): void {
  const now = performance.now();
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
