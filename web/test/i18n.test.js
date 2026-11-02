'use strict';

const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const assert = require('node:assert/strict');

const root = path.join(__dirname, '..');
const pub = path.join(root, 'public');
const CYRILLIC = /[А-Яа-яЁё]/;

function read(file) {
  return fs.readFileSync(file, 'utf8');
}

function codeLines(source) {
  return source.split('\n').filter((line) => !/^\s*(\/\/|\/\*|\*)/.test(line));
}

test('every API message with Russian text has an English translation', () => {
  const { translate } = require('../api/_i18n');
  const missing = [];
  for (const name of fs.readdirSync(path.join(root, 'api'))) {
    if (!name.endsWith('.js') || name === '_i18n.js' || name === '_mail.js') continue;
    for (const line of codeLines(read(path.join(root, 'api', name)))) {
      if (!/HttpError\(|^\s*\w+: '|^\s*'[^']+': '|message:/.test(line) || /subject:|text:|html:/.test(line)) continue;
      for (const m of line.matchAll(/(['`])((?:(?!\1).)*[А-Яа-яЁё](?:(?!\1).)*)\1/g)) {
        const sample = m[2].replace(/\$\{[^}]+\}/g, '8');
        if (translate(sample, 'en') === sample && CYRILLIC.test(sample)) missing.push(`${name}: ${sample}`);
      }
    }
  }
  assert.deepEqual(missing, []);
});

test('every tr() string in browser scripts has an English translation', () => {
  const i18n = read(path.join(pub, 'i18n.js'));
  const table = new Function(`${i18n.slice(i18n.indexOf('const EN = {'), i18n.indexOf('/** Перевод строки'))}; return EN;`)();
  const missing = [];
  for (const name of fs.readdirSync(pub)) {
    if (!name.endsWith('.js') || name === 'i18n.js') continue;
    for (const m of read(path.join(pub, name)).matchAll(/\btr\('((?:[^'\\]|\\.)*)'/g)) {
      const key = m[1].replace(/\\'/g, "'");
      if (!(key in table)) missing.push(`${name}: ${key}`);
    }
  }
  assert.deepEqual(missing, []);
  const unused = Object.keys(table).filter((key) => !fs.readdirSync(pub).some((n) => n.endsWith('.js') && n !== 'i18n.js' && read(path.join(pub, n)).includes(`'${key.replace(/'/g, "\\'")}'`)));
  assert.deepEqual(unused, []);
});

test('English pages mirror the Russian ones and contain no Russian text', () => {
  for (const name of fs.readdirSync(path.join(pub, 'en'))) {
    const en = read(path.join(pub, 'en', name));
    const ru = read(path.join(pub, name));
    assert.match(en, /<html lang="en"/, name);
    assert.ok(!CYRILLIC.test(en), `${name} still has Russian text`);
    const ids = (html) => [...html.matchAll(/\sid="([^"]+)"/g)].map((m) => m[1]).sort();
    assert.deepEqual(ids(en), ids(ru), `${name}: element ids differ`);
    const scripts = (html) => [...html.matchAll(/<script src="([^"]+)"/g)].map((m) => m[1]);
    assert.deepEqual(scripts(en), scripts(ru), `${name}: scripts differ`);
    assert.ok(!/<a [^>]*href="\/(request|register|login|account|admin)\b/.test(en), `${name}: link leaves the English version`);
  }
});

test('every Russian page has an English twin', () => {
  const skip = new Set(['404.html']);
  for (const name of fs.readdirSync(pub).filter((n) => n.endsWith('.html') && !skip.has(n) && !n.startsWith('google'))) {
    assert.ok(fs.existsSync(path.join(pub, 'en', name)), `en/${name} is missing`);
  }
});
