/** Non-secret, bounded cache. Packets are reverified before every execution. */
const DB = "layrs.lens-cache.v1";
async function database(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB, 1);
    request.onupgradeneeded = () => request.result.createObjectStore("entries", { keyPath: "key" });
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}
export async function readCache<T>(key: string): Promise<T | null> {
  try {
    const db = await database();
    return await new Promise((resolve, reject) => {
      const tx = db.transaction("entries", "readonly");
      const request = tx.objectStore("entries").get(key);
      request.onsuccess = () => resolve(request.result?.value ?? null);
      request.onerror = () => reject(request.error);
      tx.oncomplete = () => db.close();
    });
  } catch { return null; }
}
export async function writeCache(key: string, value: unknown): Promise<void> {
  try {
    const size = JSON.stringify(value).length;
    if (size > 12 * 1024 * 1024) return;
    const db = await database();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction("entries", "readwrite");
      const store = tx.objectStore("entries");
      const request = store.getAll();
      request.onsuccess = () => {
        const entries = request.result.filter(entry => entry.key !== key).sort((a, b) => a.saved - b.saved);
        let total = entries.reduce((sum, entry) => sum + entry.size, 0) + size;
        while (entries.length >= 128 || total > 32 * 1024 * 1024) {
          const oldest = entries.shift();
          if (!oldest) break;
          total -= oldest.size; store.delete(oldest.key);
        }
        store.put({ key, value, size, saved: Date.now() });
      };
      tx.oncomplete = () => { db.close(); resolve(); };
      tx.onerror = () => { db.close(); reject(tx.error); };
      tx.onabort = () => { db.close(); reject(tx.error); };
    });
  } catch { /* Storage refusal never blocks an online project. */ }
}
