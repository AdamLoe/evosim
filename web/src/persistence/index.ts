// Main-thread persistence client. Spawns the IDB worker (plain ES module,
// no wasm). v5 §13, v6 §I.

import PersistenceWorker from "./worker?worker";

export interface CameraState {
  zoom: number;
  cx: number;
  cy: number;
}

export interface SavedRow {
  key: "current";
  json: string;
  tick: number;
  seed: string;
  saved_at_ms: number;
  /** F.26 + v6 §C: camera persisted alongside the wasm save string. May be
   * absent on rows written by older builds that wrapped camera into `json`
   * as a `{wasm, camera}` envelope. */
  camera?: CameraState;
}

export class PersistenceClient {
  private worker = new PersistenceWorker();
  private onErrorCb?: (code: string, message: string) => void;
  private onSavedCb?: (tick: number) => void;
  private onSavingCb?: () => void;

  constructor() {
    // Default message handler routes saved/error; overridden transiently by
    // loadCurrent / deleteCurrent to capture their one-shot responses.
    this.worker.onmessage = (ev: MessageEvent) => {
      const m = ev.data as { type: string; tick?: number; code?: string; message?: string };
      if (m?.type === "saved") this.onSavedCb?.(m.tick!);
      else if (m?.type === "error") this.onErrorCb?.(m.code ?? "Unknown", m.message ?? "");
    };
  }

  setHandlers(opts: {
    onSaving?: () => void;
    onSaved?: (tick: number) => void;
    onError?: (code: string, message: string) => void;
  }): void {
    this.onSavingCb = opts.onSaving;
    this.onSavedCb = opts.onSaved;
    this.onErrorCb = opts.onError;
  }

  save(json: string, seed: string, tick: number, camera?: CameraState): void {
    // Camera is passed as a structured-cloneable object so the main thread
    // never has to JSON.parse/stringify the (large) wasm save string just to
    // wrap a few floats into an envelope.
    this.onSavingCb?.();
    this.worker.postMessage({ type: "save", json, seed, tick, camera });
  }

  loadCurrent(): Promise<SavedRow | null> {
    return new Promise((resolve, reject) => {
      const savedHandler = this.worker.onmessage;
      this.worker.onmessage = (ev: MessageEvent) => {
        const m = ev.data as { type: string; row?: SavedRow; code?: string; message?: string };
        if (m?.type === "loaded") {
          this.worker.onmessage = savedHandler;
          resolve(m.row ?? null);
        } else if (m?.type === "error") {
          this.worker.onmessage = savedHandler;
          reject(new Error(`${m.code}: ${m.message}`));
        }
      };
      this.worker.postMessage({ type: "load" });
    });
  }

  deleteCurrent(): Promise<void> {
    return new Promise((resolve, reject) => {
      const savedHandler = this.worker.onmessage;
      this.worker.onmessage = (ev: MessageEvent) => {
        const m = ev.data as { type: string; code?: string; message?: string };
        if (m?.type === "deleted") {
          this.worker.onmessage = savedHandler;
          resolve();
        } else if (m?.type === "error") {
          this.worker.onmessage = savedHandler;
          reject(new Error(`${m.code}: ${m.message}`));
        }
      };
      this.worker.postMessage({ type: "delete" });
    });
  }
}
