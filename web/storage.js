// Browser storage for settings, saves, save states and picked ROMs.
//
// The emulator asks for files synchronously (Platform::load in Rust), but
// IndexedDB only answers asynchronously. So at startup every stored file
// is read into a Map, reads are served from the Map, and writes update
// the Map at once and reach IndexedDB a moment later ("write-behind"),
// everything written during one frame in one transaction. See
// crates/web/src/storage.rs for the keys.

const DB_NAME = 'gase';
const STORE = 'files';
const ROM_PREFIX = 'rom/';
// Picked ROMs are kept so the recent list works; the app remembers 8.
const MAX_ROMS = 8;

function request(r) {
  return new Promise((resolve, reject) => {
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

function openDb() {
  const r = indexedDB.open(DB_NAME, 1);
  r.onupgradeneeded = () => r.result.createObjectStore(STORE);
  return request(r);
}

/** Read everything into memory; returns the storage object gase.js uses. */
export async function openStorage() {
  const files = new Map(); // key → { data: Uint8Array, used: ms since 1970 }
  let db = null;
  try {
    db = await openDb();
    const store = db.transaction(STORE).objectStore(STORE);
    const [keys, values] = await Promise.all([request(store.getAllKeys()), request(store.getAll())]);
    keys.forEach((key, i) => files.set(key, values[i]));
  } catch (e) {
    console.warn('gase: no IndexedDB, nothing will be kept after this visit', e);
    db = null;
  }

  const pending = new Map(); // key → entry, or null to delete
  let scheduled = false;
  let onError = () => {};

  function flush() {
    scheduled = false;
    if (!db || pending.size === 0) return Promise.resolve();
    const tx = db.transaction(STORE, 'readwrite');
    const store = tx.objectStore(STORE);
    for (const [key, entry] of pending) {
      if (entry) store.put(entry, key);
      else store.delete(key);
    }
    pending.clear();
    return new Promise((resolve) => {
      tx.oncomplete = resolve;
      tx.onerror = tx.onabort = () => {
        onError(tx.error);
        resolve();
      };
    });
  }

  function schedule(key, entry) {
    pending.set(key, entry);
    if (!scheduled && db) {
      scheduled = true;
      setTimeout(flush, 0);
    }
  }

  /** Keep only the most recently used ROMs. */
  function pruneRoms() {
    const roms = [...files].filter(([k]) => k.startsWith(ROM_PREFIX));
    roms.sort((a, b) => b[1].used - a[1].used);
    for (const [key] of roms.slice(MAX_ROMS)) {
      files.delete(key);
      schedule(key, null);
    }
  }

  return {
    persistent: db !== null,
    get(key) {
      const entry = files.get(key);
      if (!entry) return null;
      if (key.startsWith(ROM_PREFIX)) {
        entry.used = Date.now();
        schedule(key, entry);
      }
      return entry.data;
    },
    put(key, data) {
      const entry = { data, used: Date.now() };
      files.set(key, entry);
      schedule(key, entry);
      if (key.startsWith(ROM_PREFIX)) pruneRoms();
    },
    flush,
    set onError(f) {
      onError = f;
    },
  };
}
