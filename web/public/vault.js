'use strict';

const VAULT_ITERATIONS = 600000;
const VAULT_PREFIX = 'deix.vault.';

function createVault({ storage, keyStore, subtle, random }) {
  const slot = (id) => VAULT_PREFIX + id;
  const aad = (id) => new TextEncoder().encode(`deix-vault-1:${id}`);
  const emptyData = () => ({ keys: [], labels: {} });

  const toB64 = (bytes) => {
    let text = '';
    for (const byte of bytes) text += String.fromCharCode(byte);
    return btoa(text);
  };
  const fromB64 = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));

  function readBlob(id) {
    const raw = storage.getItem(slot(id));
    if (!raw) return null;
    try {
      const blob = JSON.parse(raw);
      return blob && blob.v === 1 ? blob : null;
    } catch {
      return null;
    }
  }

  async function derive(password, salt, iterations) {
    const base = await subtle.importKey('raw', new TextEncoder().encode(password), 'PBKDF2', false, ['deriveKey']);
    return subtle.deriveKey({ name: 'PBKDF2', salt, iterations, hash: 'SHA-256' }, base, { name: 'AES-GCM', length: 256 }, false, [
      'encrypt',
      'decrypt',
    ]);
  }

  async function seal(id, key, salt, iterations, data) {
    const iv = random(12);
    const plain = new TextEncoder().encode(JSON.stringify(data));
    const sealed = new Uint8Array(await subtle.encrypt({ name: 'AES-GCM', iv, additionalData: aad(id) }, key, plain));
    storage.setItem(
      slot(id),
      JSON.stringify({ v: 1, iter: iterations, salt: toB64(salt), iv: toB64(iv), ct: toB64(sealed) }),
    );
  }

  async function unseal(id, key, blob) {
    const plain = await subtle.decrypt(
      { name: 'AES-GCM', iv: fromB64(blob.iv), additionalData: aad(id) },
      key,
      fromB64(blob.ct),
    );
    const data = JSON.parse(new TextDecoder().decode(plain));
    return { ...emptyData(), ...data };
  }

  async function startNew(id, password, data) {
    const salt = random(16);
    const key = await derive(password, salt, VAULT_ITERATIONS);
    await seal(id, key, salt, VAULT_ITERATIONS, data);
    await keyStore.set(id, key);
  }

  return {
    exists: (id) => readBlob(id) !== null,

    /** 'created' | 'opened' | 'wrong': пароль не подошёл к уже существующим данным. */
    async open(id, password) {
      const blob = readBlob(id);
      if (!blob) {
        await startNew(id, password, emptyData());
        return 'created';
      }
      const key = await derive(password, fromB64(blob.salt), blob.iter);
      try {
        await unseal(id, key, blob);
      } catch {
        return 'wrong';
      }
      await keyStore.set(id, key);
      return 'opened';
    },

    /** Расшифрованные данные или null, если хранилище закрыто. */
    async read(id) {
      const blob = readBlob(id);
      const key = await keyStore.get(id);
      if (!blob || !key) return null;
      try {
        return await unseal(id, key, blob);
      } catch {
        return null;
      }
    },

    async update(id, mutate) {
      const blob = readBlob(id);
      const key = await keyStore.get(id);
      if (!blob || !key) return false;
      let data;
      try {
        data = await unseal(id, key, blob);
      } catch {
        return false;
      }
      mutate(data);
      await seal(id, key, fromB64(blob.salt), blob.iter, data);
      return true;
    },

    /** Перешифровывает данные новым паролем. false, если старый пароль не открывает хранилище. */
    async rekey(id, oldPassword, newPassword) {
      const blob = readBlob(id);
      let data = emptyData();
      if (blob) {
        const key = await derive(oldPassword, fromB64(blob.salt), blob.iter);
        try {
          data = await unseal(id, key, blob);
        } catch {
          return false;
        }
      }
      await startNew(id, newPassword, data);
      return true;
    },

    lock: () => keyStore.clear(),

    async reset(id) {
      storage.removeItem(slot(id));
      await keyStore.clear();
    },
  };
}

function idbKeyStore(idb) {
  const open = () =>
    new Promise((resolve, reject) => {
      const request = idb.open('deix-vault', 1);
      request.onupgradeneeded = () => request.result.createObjectStore('keys');
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
  const run = async (mode, action) => {
    const db = await open();
    return new Promise((resolve, reject) => {
      const tx = db.transaction('keys', mode);
      const request = action(tx.objectStore('keys'));
      tx.oncomplete = () => {
        db.close();
        resolve(request.result);
      };
      tx.onerror = () => {
        db.close();
        reject(tx.error);
      };
    });
  };
  return {
    get: (id) => run('readonly', (store) => store.get(id)),
    set: (id, key) => run('readwrite', (store) => store.put(key, id)),
    clear: () => run('readwrite', (store) => store.clear()),
  };
}

const Vault =
  typeof window !== 'undefined' && window.crypto && window.crypto.subtle && window.indexedDB && window.localStorage
    ? createVault({
        storage: window.localStorage,
        keyStore: idbKeyStore(window.indexedDB),
        subtle: window.crypto.subtle,
        random: (n) => window.crypto.getRandomValues(new Uint8Array(n)),
      })
    : null;

/** Открывает (или создаёт) хранилище паролем, который только что принял вход. Ошибки не мешают входу. */
async function unlockVault(userId, password) {
  if (!Vault || !userId) return;
  try {
    await Vault.open(userId, password);
  } catch (err) {
    console.error('vault unlock failed:', err && err.message);
  }
}

if (typeof module !== 'undefined') module.exports = { createVault, VAULT_ITERATIONS };
