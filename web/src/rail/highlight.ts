// Highlight ring state. Maps creature_id (stable u64 as number) to
// expiry wall-clock ms. Timing is wall-clock per v6 §B (4s toast, 1.5s ring).

export const highlights: Map<number, number> = new Map();

// Sentinel used when a creature is selected in the inspector (infinite highlight).
export const HIGHLIGHT_PERMANENT = Number.MAX_SAFE_INTEGER;

/**
 * Add a 1.5s highlight for a creature (v6 §B: 1.5 seconds wall-clock).
 */
export function addHighlight(creatureId: number, durationMs = 1500): void {
  highlights.set(creatureId, performance.now() + durationMs);
}

/**
 * Remove expired highlights. Called each frame from pollRail.
 */
export function pruneHighlights(nowMs: number): void {
  for (const [id, exp] of highlights) {
    if (nowMs >= exp) highlights.delete(id);
  }
}
