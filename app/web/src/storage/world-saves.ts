export type SaveKind = "autosave" | "named" | "imported";

export interface WorldSaveRecord {
  id: string;
  kind: SaveKind;
  name: string;
  artifactJson: string;
  createdAt: number;
  updatedAt: number;
  tick: number;
  population: number;
  runId: string;
}

const DB_NAME = "evosim.world-saves";
const DB_VERSION = 1;
const STORE = "saves";
const AUTOSAVE_ID = "autosave:latest";

let dbPromise: Promise<IDBDatabase> | null = null;

function openDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE)) {
        const store = db.createObjectStore(STORE, { keyPath: "id" });
        store.createIndex("updatedAt", "updatedAt");
        store.createIndex("kind", "kind");
      }
    };
    req.onerror = () => reject(req.error ?? new Error("open IndexedDB failed"));
    req.onsuccess = () => resolve(req.result);
  });
  return dbPromise;
}

function txStore(db: IDBDatabase, mode: IDBTransactionMode): IDBObjectStore {
  return db.transaction(STORE, mode).objectStore(STORE);
}

function requestToPromise<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onerror = () => reject(req.error ?? new Error("IndexedDB request failed"));
    req.onsuccess = () => resolve(req.result);
  });
}

export function metadataFromArtifact(artifactJson: string): {
  tick: number;
  population: number;
  runId: string;
} {
  const parsed = JSON.parse(artifactJson) as {
    kind?: string;
    schema_version?: number;
    error?: string;
    identity?: { run_id?: string };
    state?: { tick?: number; creatures?: { id?: unknown[] } };
  };
  if (parsed.kind !== "evosim.world" || parsed.schema_version !== 1) {
    throw new Error("unsupported world artifact");
  }
  if (parsed.error) throw new Error(parsed.error);
  return {
    tick: Number(parsed.state?.tick ?? 0),
    population: Array.isArray(parsed.state?.creatures?.id)
      ? parsed.state.creatures.id.length
      : 0,
    runId: String(parsed.identity?.run_id ?? "unknown"),
  };
}

export function withAppMetadata(artifactJson: string, appVersion: string): string {
  const parsed = JSON.parse(artifactJson) as {
    meta?: { app_version?: string | null; build_profile?: string | null };
  };
  parsed.meta = {
    ...(parsed.meta ?? {}),
    app_version: appVersion,
    build_profile: "web",
  };
  return JSON.stringify(parsed);
}

export async function putWorldSave(record: WorldSaveRecord): Promise<void> {
  const db = await openDb();
  await requestToPromise(txStore(db, "readwrite").put(record));
}

export async function putAutosave(
  artifactJson: string,
  appVersion: string,
): Promise<WorldSaveRecord> {
  const now = Date.now();
  const withMeta = withAppMetadata(artifactJson, appVersion);
  const meta = metadataFromArtifact(withMeta);
  const record: WorldSaveRecord = {
    id: AUTOSAVE_ID,
    kind: "autosave",
    name: "Autosave",
    artifactJson: withMeta,
    createdAt: now,
    updatedAt: now,
    tick: meta.tick,
    population: meta.population,
    runId: meta.runId,
  };
  await putWorldSave(record);
  return record;
}

export async function putNamedSave(
  artifactJson: string,
  appVersion: string,
  name?: string,
  kind: SaveKind = "named",
): Promise<WorldSaveRecord> {
  const now = Date.now();
  const withMeta = withAppMetadata(artifactJson, appVersion);
  const meta = metadataFromArtifact(withMeta);
  const record: WorldSaveRecord = {
    id: `${kind}:${now}:${Math.random().toString(16).slice(2)}`,
    kind,
    name: name ?? `Save t${meta.tick}`,
    artifactJson: withMeta,
    createdAt: now,
    updatedAt: now,
    tick: meta.tick,
    population: meta.population,
    runId: meta.runId,
  };
  await putWorldSave(record);
  return record;
}

export async function latestWorldSave(): Promise<WorldSaveRecord | null> {
  const db = await openDb();
  const all = await requestToPromise<WorldSaveRecord[]>(txStore(db, "readonly").getAll());
  if (all.length === 0) return null;
  all.sort((a, b) => b.updatedAt - a.updatedAt);
  return all[0] ?? null;
}
