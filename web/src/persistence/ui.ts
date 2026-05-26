// Resume prompt + schema-mismatch modal. v5 §13, v6 §I.

export async function showResumePrompt(opts: {
  day: number;
  seed: string;
  savedAtMs: number;
}): Promise<"resume" | "new"> {
  return new Promise((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "modal-backdrop";

    const card = document.createElement("div");
    card.className = "modal-card";
    const savedAgoMin = Math.floor((Date.now() - opts.savedAtMs) / 60_000);
    card.innerHTML = `
      <h2>Resume world?</h2>
      <p>Day ${opts.day}, seed <code>${escapeHtml(opts.seed)}</code></p>
      <p class="muted">Saved ${savedAgoMin} min ago</p>
      <div class="modal-actions">
        <button data-action="new">New World</button>
        <button data-action="resume" class="primary">Resume</button>
      </div>
    `;
    backdrop.appendChild(card);
    document.body.appendChild(backdrop);

    card.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      const action = t.dataset?.action;
      if (action === "resume" || action === "new") {
        backdrop.remove();
        resolve(action);
      }
    });
  });
}

export async function showSchemaMismatchModal(): Promise<void> {
  return new Promise((resolve) => {
    const backdrop = document.createElement("div");
    backdrop.className = "modal-backdrop";
    const card = document.createElement("div");
    card.className = "modal-card";
    card.innerHTML = `
      <h2>Save format updated</h2>
      <p>Save format updated. Starting fresh.</p>
      <div class="modal-actions">
        <button data-action="new" class="primary">OK</button>
      </div>
    `;
    backdrop.appendChild(card);
    document.body.appendChild(backdrop);
    card.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      if ((t as HTMLElement).dataset?.action === "new") {
        backdrop.remove();
        resolve();
      }
    });
  });
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
