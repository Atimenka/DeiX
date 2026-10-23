'use strict';

async function copySum(sha256) {
  try {
    await navigator.clipboard.writeText(sha256);
    toast(tr('Контрольная сумма скопирована'), 'ok');
  } catch (err) {
    toast(tr('Не удалось скопировать: выделите сумму вручную'), 'err');
  }
}

function releaseCard(row) {
  const published = row.published_at ? new Date(row.published_at).toLocaleDateString(LOCALE) : '';
  return el('article', { class: 'panel release' }, [
    el('div', { class: 'toolbar' }, [
      el('h3', { class: 'title-sm' }, [
        el('span', { class: 'mono', text: `v${row.version}` }),
        document.createTextNode(row.title ? ` — ${row.title}` : ''),
      ]),
      el('span', { class: 'hint', text: published }),
    ]),
    row.notes ? el('p', { class: 'release-notes', text: row.notes }) : document.createTextNode(''),
    el('div', { class: 'release-sum' }, [
      el('span', { class: 'hint', text: 'SHA-256' }),
      el('code', { class: 'sum', text: row.sha256 }),
      el('button', { class: 'btn small', type: 'button', text: tr('Копировать'), onclick: () => copySum(row.sha256) }),
    ]),
    el('div', { class: 'row-gap' }, [
      el('a', {
        class: 'btn primary',
        href: `/api/releases?download=${encodeURIComponent(row.version)}`,
        text: tr('Скачать {file} ({size})', { file: row.file_name, size: fmtSize(row.size_bytes) }),
      }),
      el('span', { class: 'hint', text: tr('Скачиваний: {n}', { n: row.downloads }) }),
    ]),
  ]);
}

async function load() {
  const box = document.getElementById('releases');
  try {
    const resp = await fetch('/api/releases', { headers: { 'x-deix-lang': LANG } });
    const data = await resp.json().catch(() => null);
    if (!resp.ok || !data) throw new Error((data && data.error) || tr('Не удалось загрузить список версий'));
    box.replaceChildren(...data.releases.map(releaseCard));
    document.getElementById('releases-empty').hidden = data.releases.length > 0;
  } catch (err) {
    box.replaceChildren(el('div', { class: 'result err', text: err.message || tr('Не удалось загрузить список версий') }));
  }
}

load();
