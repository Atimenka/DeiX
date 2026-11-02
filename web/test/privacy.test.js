'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const root = path.join(__dirname, '..');
const pub = path.join(root, 'public');
const read = (file) => fs.readFileSync(file, 'utf8');
const pages = {
  ru: read(path.join(pub, 'privacy.html')),
  en: read(path.join(pub, 'en', 'privacy.html')),
};

function everywhere(name, items) {
  for (const [lang, html] of Object.entries(pages)) {
    const missing = items.filter((item) => !html.includes(item));
    assert.deepEqual(missing, [], `${name} missing from the ${lang} policy`);
  }
}

test('policy lists every cookie the API sets', () => {
  const cookies = new Set();
  for (const name of fs.readdirSync(path.join(root, 'api'))) {
    for (const m of read(path.join(root, 'api', name)).matchAll(/['"`](deix_[a-z]+)=?/g)) cookies.add(m[1]);
  }
  assert.ok(cookies.size >= 3, 'no cookies found');
  everywhere('cookie', [...cookies]);
});

test('policy lists every public table created by the migrations', () => {
  const dir = path.join(root, '..', 'supabase', 'migrations');
  const tables = new Set();
  for (const name of fs.readdirSync(dir)) {
    for (const m of read(path.join(dir, name)).matchAll(/create table if not exists public\.([a-z_]+)/g)) tables.add(m[1]);
  }
  assert.ok(tables.size >= 8, 'no tables found');
  everywhere('table', [...tables].map((t) => `<code>${t}</code>`));
});

test('policy lists every browser storage key the site uses', () => {
  const keys = new Set();
  for (const name of fs.readdirSync(pub).filter((n) => n.endsWith('.js'))) {
    for (const m of read(path.join(pub, name)).matchAll(/(?:localStorage|sessionStorage)\.(?:getItem|setItem|removeItem)\('(deix-[a-z]+)'/g)) keys.add(m[1]);
  }
  assert.ok(keys.size >= 3, 'no storage keys found');
  everywhere('storage key', [...keys]);
});

test('every table-of-contents link points to a section', () => {
  for (const [lang, html] of Object.entries(pages)) {
    const ids = new Set([...html.matchAll(/\sid="([^"]+)"/g)].map((m) => m[1]));
    const links = [...html.matchAll(/href="#([^"]+)"/g)].map((m) => m[1]);
    assert.ok(links.length >= 20, `${lang}: table of contents is too short`);
    assert.deepEqual(links.filter((id) => !ids.has(id)), [], `${lang}: dangling anchors`);
  }
});
