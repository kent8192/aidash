// Once the sign-in helper is served from this origin, Firebase writes `firebase:`
// keys (and may open its IndexedDB store) here. Aidash keeps no GCIP state
// between attempts, so remove all of it after every attempt.
const PREFIX = "firebase:";
const DATABASE = "firebaseLocalStorageDb";

export async function clearGcipStorage() {
  for (const storage of [localStorage, sessionStorage]) {
    const keys = Array.from({ length: storage.length }, (_, index) =>
      storage.key(index),
    );
    for (const key of keys)
      if (key?.startsWith(PREFIX)) storage.removeItem(key);
  }
  await new Promise<void>((resolve) => {
    const request = indexedDB.deleteDatabase(DATABASE);
    // A blocked deletion still completes once the remaining connection closes.
    request.onsuccess = request.onerror = request.onblocked = () => resolve();
  });
}
