// IDB persistence worker. Plain ES-module (no wasm) — receives save/load/delete
// messages from the main thread, writes to IndexedDB, posts back results.
// v5 §13, v6 §I. DECISIONS: JSON-encode on main thread; worker only writes to IDB.

let dbPromise: Promise<IDBDatabase> | null = null;

function openDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const req = indexedDB.open("evosim", 1);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains("worlds")) {
        db.createObjectStore("worlds", { keyPath: "key" });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IDB open failed"));
  });
  return dbPromise;
}

interface CameraState {
  zoom: number;
  cx: number;
  cy: number;
}

self.onmessage = async (ev: MessageEvent) => {
  const msg = ev.data as {
    type: string;
    json?: string;
    seed?: string;
    tick?: number;
    camera?: CameraState;
  };
  if (msg?.type === "save") {
    try {
      const db = await openDb();
      const tx = db.transaction("worlds", "readwrite");
      const store = tx.objectStore("worlds");
      const row = {
        key: "current",
        json: msg.json!,
        tick: msg.tick!,
        seed: msg.seed!,
        saved_at_ms: Date.now(),
        camera: msg.camera,
      };
      const putReq = store.put(row);
      putReq.onerror = () => {
        const err = putReq.error;
        (self as unknown as Worker).postMessage({
          type: "error",
          message: String(err?.message ?? err),
          code: err?.name ?? "UnknownError",
        });
      };
      tx.oncomplete = () =>
        (self as unknown as Worker).postMessage({ type: "saved", tick: msg.tick });
      tx.onerror = () => {
        const err = tx.error;
        (self as unknown as Worker).postMessage({
          type: "error",
          message: String(err?.message ?? err),
          code: err?.name ?? "TransactionError",
        });
      };
    } catch (e) {
      (self as unknown as Worker).postMessage({
        type: "error",
        message: String((e as Error).message ?? e),
        code: "OpenError",
      });
    }
  } else if (msg?.type === "load") {
    try {
      const db = await openDb();
      const tx = db.transaction("worlds", "readonly");
      const store = tx.objectStore("worlds");
      const req = store.get("current");
      req.onsuccess = () =>
        (self as unknown as Worker).postMessage({ type: "loaded", row: req.result ?? null });
      req.onerror = () =>
        (self as unknown as Worker).postMessage({
          type: "error",
          message: String(req.error?.message ?? req.error),
          code: req.error?.name ?? "LoadError",
        });
    } catch (e) {
      (self as unknown as Worker).postMessage({
        type: "error",
        message: String((e as Error).message ?? e),
        code: "OpenError",
      });
    }
  } else if (msg?.type === "delete") {
    try {
      const db = await openDb();
      const tx = db.transaction("worlds", "readwrite");
      const store = tx.objectStore("worlds");
      store.delete("current");
      tx.oncomplete = () => (self as unknown as Worker).postMessage({ type: "deleted" });
      tx.onerror = () =>
        (self as unknown as Worker).postMessage({
          type: "error",
          message: String(tx.error?.message ?? tx.error),
          code: tx.error?.name ?? "DeleteError",
        });
    } catch (e) {
      (self as unknown as Worker).postMessage({
        type: "error",
        message: String((e as Error).message ?? e),
        code: "OpenError",
      });
    }
  }
};
