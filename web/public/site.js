'use strict';

function el(tag, attrs, children) {
  const node = document.createElement(tag);
  if (attrs) {
    for (const [key, value] of Object.entries(attrs)) {
      if (key === 'class') node.className = value;
      else if (key === 'text') node.textContent = value;
      else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
      else node.setAttribute(key, value);
    }
  }
  for (const child of children || []) node.append(child);
  return node;
}

async function postJson(url, body, extraHeaders) {
  let resp;
  try {
    resp = await fetch(url, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', ...(extraHeaders || {}) },
      body: JSON.stringify(body),
    });
  } catch {
    throw new Error('Нет связи с сервером');
  }
  let data = null;
  try {
    data = await resp.json();
  } catch {
    data = null;
  }
  if (!resp.ok || !data || data.ok === false) {
    const err = new Error((data && data.error) || `Ошибка сервера (${resp.status})`);
    err.status = resp.status;
    throw err;
  }
  return data;
}

function toast(text, kind) {
  let box = document.getElementById('toasts');
  if (!box) {
    box = el('div', { id: 'toasts' });
    document.body.append(box);
  }
  const node = el('div', { class: `toast ${kind || ''}`, text });
  box.append(node);
  setTimeout(() => node.remove(), 4500);
}

function showResult(node, kind, text) {
  node.hidden = false;
  node.className = `result ${kind}`;
  node.textContent = text;
}

for (const node of document.querySelectorAll('[data-year]')) {
  node.textContent = String(new Date().getFullYear());
}
