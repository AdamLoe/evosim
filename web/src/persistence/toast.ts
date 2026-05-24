// Autosave failure status toast. Ephemeral, top-center, 6s fade.
// Distinct from in-aquarium event toasts (E.22). v6 §I.

export function showAutosaveFailureToast(): void {
  // Replace existing toast if present (no stack — just one at a time).
  let el = document.getElementById("autosave-toast") as HTMLDivElement | null;
  if (el) el.remove();

  el = document.createElement("div");
  el.id = "autosave-toast";
  el.className = "status-toast";
  el.textContent = "Autosave failed — local storage may be full";
  document.body.appendChild(el);
  setTimeout(() => el?.remove(), 6000);
}
