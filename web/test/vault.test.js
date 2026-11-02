'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const nodeCrypto = require('node:crypto');
const { createVault } = require('../public/vault.js');

function memoryVault(shared) {
  const items = shared ? shared.items : new Map();
  const keys = shared ? shared.keys : new Map();
  const storage = {
    getItem: (k) => (items.has(k) ? items.get(k) : null),
    setItem: (k, v) => items.set(k, v),
    removeItem: (k) => items.delete(k),
  };
  const keyStore = {
    get: async (id) => keys.get(id),
    set: async (id, key) => keys.set(id, key),
    clear: async () => keys.clear(),
  };
  const vault = createVault({
    storage,
    keyStore,
    subtle: nodeCrypto.webcrypto.subtle,
    random: (n) => nodeCrypto.webcrypto.getRandomValues(new Uint8Array(n)),
  });
  return { vault, items, keys };
}

const ID = '00000000-0000-0000-0000-000000000001';

test('vault: stores data encrypted, readable only after unlock', async () => {
  const { vault, items } = memoryVault();
  assert.equal(await vault.open(ID, 'secret-pass'), 'created');
  assert.equal(await vault.update(ID, (d) => d.keys.push({ device_id: 'a'.repeat(32), created_at: 'now' })), true);
  assert.equal((await vault.read(ID)).keys[0].device_id, 'a'.repeat(32));

  const blob = items.get(`deix.vault.${ID}`);
  assert.equal(blob.includes('a'.repeat(32)), false);
  assert.equal(blob.includes('device_id'), false);

  await vault.lock();
  assert.equal(await vault.read(ID), null);
  assert.equal(await vault.update(ID, () => {}), false);
});

test('vault: the right password reopens it, a wrong one does not', async () => {
  const shared = memoryVault();
  await shared.vault.open(ID, 'secret-pass');
  await shared.vault.update(ID, (d) => {
    d.labels.x = 'laptop';
  });
  await shared.vault.lock();

  assert.equal(await shared.vault.open(ID, 'wrong-pass'), 'wrong');
  assert.equal(await shared.vault.read(ID), null);
  assert.equal(await shared.vault.open(ID, 'secret-pass'), 'opened');
  assert.equal((await shared.vault.read(ID)).labels.x, 'laptop');
});

test('vault: every save uses a fresh IV and tampering is detected', async () => {
  const { vault, items } = memoryVault();
  await vault.open(ID, 'secret-pass');
  const first = JSON.parse(items.get(`deix.vault.${ID}`));
  await vault.update(ID, (d) => d.keys.push({ device_id: 'b'.repeat(32), created_at: 'now' }));
  const second = JSON.parse(items.get(`deix.vault.${ID}`));
  assert.notEqual(first.iv, second.iv);
  assert.equal(first.salt, second.salt);

  second.ct = Buffer.from(Buffer.from(second.ct, 'base64').map((b, i) => (i === 0 ? b ^ 1 : b))).toString('base64');
  items.set(`deix.vault.${ID}`, JSON.stringify(second));
  assert.equal(await vault.read(ID), null);
});

test('vault: data is bound to the account id', async () => {
  const { vault, items } = memoryVault();
  await vault.open(ID, 'secret-pass');
  const other = '00000000-0000-0000-0000-000000000002';
  items.set(`deix.vault.${other}`, items.get(`deix.vault.${ID}`));
  assert.equal(await vault.open(other, 'secret-pass'), 'wrong');
});

test('vault: password change re-encrypts the data', async () => {
  const { vault } = memoryVault();
  await vault.open(ID, 'old-pass-1');
  await vault.update(ID, (d) => {
    d.labels.x = 'pc';
  });
  assert.equal(await vault.rekey(ID, 'not-the-old-one', 'new-pass-2'), false);
  assert.equal(await vault.rekey(ID, 'old-pass-1', 'new-pass-2'), true);
  await vault.lock();
  assert.equal(await vault.open(ID, 'old-pass-1'), 'wrong');
  assert.equal(await vault.open(ID, 'new-pass-2'), 'opened');
  assert.equal((await vault.read(ID)).labels.x, 'pc');
});

test('vault: reset removes the data and the key', async () => {
  const { vault, items, keys } = memoryVault();
  await vault.open(ID, 'secret-pass');
  await vault.reset(ID);
  assert.equal(items.size, 0);
  assert.equal(keys.size, 0);
  assert.equal(vault.exists(ID), false);
});
