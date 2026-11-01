'use strict';

const STATUS_LABEL = { pending: tr('На рассмотрении'), approved: tr('Одобрена'), consumed: tr('Токен выдан'), denied: tr('Отклонена') };
const TABS = [
  ['pending', tr('Ожидают')],
  ['approved', tr('Одобренные')],
  ['consumed', tr('Токен выдан')],
  ['denied', tr('Отклонённые')],
  ['all', tr('Все')],
];

const $ = (id) => document.getElementById(id);
let state = { requests: [], counts: {}, tab: 'pending', timer: null, section: 'requests', releases: [], updates: [] };
const MAX_IMAGE = 50 * 1024 * 1024;

function api(body) {
  return postJson('/api/admin', body);
}

function showDenied(title, text, actions) {
  clearInterval(state.timer);
  $('panel-view').hidden = true;
  $('denied-view').hidden = false;
  $('denied-title').textContent = title;
  $('denied-text').textContent = text;
  $('denied-actions').replaceChildren(...actions);
}

const loginButton = () => el('a', { class: 'btn primary', href: lp('/login?next=/admin'), text: tr('Войти') });
const securityButton = () => el('a', { class: 'btn primary', href: lp('/account#security'), text: tr('Настроить 2FA') });
const relogButton = () =>
  el('button', {
    class: 'btn primary',
    type: 'button',
    text: tr('Войти заново с кодом'),
    onclick: async () => {
      try {
        await authApi({ action: 'logout' });
      } finally {
        location.assign(lp('/login?next=/admin'));
      }
    },
  });

function denyMfa(user) {
  if (user && user.mfa) {
    showDenied(tr('Нужен вход с кодом'), tr('Эта сессия открыта без кода из приложения. Войдите заново и введите код.'), [relogButton()]);
  } else {
    showDenied(tr('Включите 2FA'), tr('Панель Owner открывается только с двухфакторной аутентификацией. Подключите её в аккаунте, затем вернитесь сюда.'), [securityButton()]);
  }
}

function showPanel() {
  $('denied-view').hidden = true;
  $('panel-view').hidden = false;
  renderSections();
  refresh();
  clearInterval(state.timer);
  state.timer = setInterval(refresh, 15000);
}

function fmt(iso) {
  return iso ? new Date(iso).toLocaleString(LOCALE) : '—';
}

function renderCounters() {
  const box = $('counters');
  box.replaceChildren();
  for (const key of ['pending', 'approved', 'consumed', 'denied']) {
    box.append(
      el('div', { class: 'counter' }, [
        el('b', { text: String(state.counts[key] || 0) }),
        el('span', { text: STATUS_LABEL[key] }),
      ]),
    );
  }
}

function renderTabs() {
  const box = $('tabs');
  box.replaceChildren();
  for (const [key, label] of TABS) {
    box.append(
      el('button', {
        class: `tab${state.tab === key ? ' active' : ''}`,
        type: 'button',
        text: label,
        onclick: () => {
          state.tab = key;
          render();
        },
      }),
    );
  }
}

async function decide(row, decision) {
  try {
    await api({ action: 'decide', device_id: row.device_id, decision });
    toast(decision === 'approve' ? tr('Заявка одобрена') : tr('Заявка отклонена'), 'ok');
    await refresh();
  } catch (err) {
    toast(err.message, 'err');
    refresh();
  }
}

async function removeRow(row) {
  if (!confirm(tr('Удалить заявку {id}?', { id: row.device_id }))) return;
  try {
    await api({ action: 'remove', device_id: row.device_id });
    toast(tr('Заявка удалена'), 'ok');
    await refresh();
  } catch (err) {
    toast(err.message, 'err');
  }
}

function actionButtons(row) {
  const buttons = [];
  if (row.status === 'pending' || row.status === 'denied') {
    buttons.push(el('button', { class: 'btn good small', type: 'button', text: tr('Одобрить'), onclick: () => decide(row, 'approve') }));
  }
  if (row.status === 'pending' || row.status === 'approved') {
    buttons.push(
      el('button', {
        class: 'btn bad small',
        type: 'button',
        text: row.status === 'approved' ? tr('Отозвать') : tr('Отклонить'),
        onclick: () => decide(row, 'deny'),
      }),
    );
  }
  buttons.push(el('button', { class: 'btn small', type: 'button', text: tr('Удалить'), onclick: () => removeRow(row) }));
  return buttons;
}

function render() {
  renderCounters();
  renderTabs();
  const rows = state.requests.filter((r) => state.tab === 'all' || r.status === state.tab);
  const body = $('rows');
  body.replaceChildren();
  $('empty').hidden = rows.length > 0;
  for (const row of rows) {
    const decided = row.decided_at ? `${fmt(row.decided_at)}${row.decided_by ? ` · ${row.decided_by}` : ''}` : '';
    body.append(
      el('tr', null, [
        el('td', { class: 'devcell' }, [
          el('span', { class: 'mono', text: row.device_id }),
          el('small', { text: row.hostname || tr('без названия') }),
        ]),
        el('td', null, [el('span', { text: row.requester || tr('анонимно (из ОС)') })]),
        el('td', null, [el('span', { class: `badge ${row.status}`, text: STATUS_LABEL[row.status] || row.status })]),
        el('td', null, [el('span', { class: 'badge', text: row.source === 'web' ? tr('сайт') : tr('ОС') })]),
        el('td', null, [el('div', { class: 'note-text', text: row.note || '—' })]),
        el('td', null, [el('div', { text: fmt(row.created_at) }), el('small', { class: 'hint', text: decided })]),
        el('td', { class: 'actions' }, actionButtons(row)),
      ]),
    );
  }
}

async function refresh() {
  try {
    const data = await api({ action: 'list' });
    state.requests = data.requests;
    state.counts = data.counts;
    $('open-switch').checked = data.requests_open;
    $('open-label').textContent = data.requests_open ? tr('Приём заявок открыт') : tr('Приём заявок закрыт');
    $('updated').textContent = tr('Обновлено: {time}', { time: new Date().toLocaleTimeString(LOCALE) });
    render();
  } catch (err) {
    if (err.code === 'mfa_required') {
      denyMfa(await fetchMe());
      return;
    }
    if (err.status === 401 || err.status === 403) {
      showDenied(tr('Нужен вход'), tr('Сессия закончилась. Войдите снова.'), [loginButton()]);
      return;
    }
    toast(err.message, 'err');
  }
}

function renderSections() {
  const box = $('section-tabs');
  box.replaceChildren();
  for (const [key, label] of [['requests', tr('Заявки')], ['releases', tr('Версии ОС')], ['updates', tr('Обновления ОС')], ['manage', tr('Управление')]]) {
    box.append(
      el('button', {
        class: `tab${state.section === key ? ' active' : ''}`,
        type: 'button',
        text: label,
        onclick: () => {
          state.section = key;
          renderSections();
          if (key === 'releases') loadReleases();
          if (key === 'updates') loadUpdates();
          if (key === 'manage') loadManage();
        },
      }),
    );
  }
  $('requests-section').hidden = state.section !== 'requests';
  $('releases-section').hidden = state.section !== 'releases';
  $('updates-section').hidden = state.section !== 'updates';
  $('manage-section').hidden = state.section !== 'manage';
}

function releaseBadge(row) {
  if (!row.ready) return ['pending', tr('Загрузка не завершена')];
  return row.published ? ['approved', tr('Опубликована')] : ['consumed', tr('Черновик')];
}

function renderReleases() {
  const body = $('rel-rows');
  body.replaceChildren();
  $('rel-empty').hidden = state.releases.length > 0;
  for (const row of state.releases) {
    const [kind, label] = releaseBadge(row);
    const buttons = [];
    if (row.ready) {
      buttons.push(
        el('button', {
          class: `btn small ${row.published ? '' : 'good'}`,
          type: 'button',
          text: row.published ? tr('Скрыть') : tr('Опубликовать'),
          onclick: () => setPublished(row, !row.published),
        }),
      );
    }
    buttons.push(el('button', { class: 'btn bad small', type: 'button', text: tr('Удалить'), onclick: () => removeRelease(row) }));
    body.append(
      el('tr', null, [
        el('td', { class: 'devcell' }, [el('span', { class: 'mono', text: row.version }), el('small', { text: row.title || '—' })]),
        el('td', { class: 'devcell' }, [
          el('span', { class: 'mono', text: row.file_name }),
          el('small', { text: `${fmtSize(row.size_bytes)} · SHA-256 ${row.sha256.slice(0, 12)}…`, title: row.sha256 }),
        ]),
        el('td', null, [el('span', { class: `badge ${kind}`, text: label })]),
        el('td', null, [el('span', { text: String(row.downloads) })]),
        el('td', null, [el('div', { text: fmt(row.created_at) }), el('small', { class: 'hint', text: row.created_by || '' })]),
        el('td', { class: 'actions' }, buttons),
      ]),
    );
  }
}

async function loadReleases() {
  try {
    state.releases = (await api({ action: 'releases' })).releases;
    renderReleases();
  } catch (err) {
    toast(err.message, 'err');
  }
}

async function setPublished(row, published) {
  try {
    await api({ action: 'release_publish', id: row.id, published });
    toast(published ? tr('Версия опубликована') : tr('Версия скрыта'), 'ok');
    await loadReleases();
  } catch (err) {
    toast(err.message, 'err');
  }
}

async function removeRelease(row) {
  if (!confirm(tr('Удалить версию {version}? Файл будет удалён из хранилища.', { version: row.version }))) return;
  try {
    await api({ action: 'release_remove', id: row.id });
    toast(tr('Версия удалена'), 'ok');
    await loadReleases();
  } catch (err) {
    toast(err.message, 'err');
  }
}

async function sha256Hex(file) {
  const digest = await crypto.subtle.digest('SHA-256', await file.arrayBuffer());
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('');
}

function uploadFile(url, file, onProgress) {
  return new Promise((resolve, reject) => {
    const form = new FormData();
    form.append('cacheControl', '3600');
    form.append('', file);
    const xhr = new XMLHttpRequest();
    xhr.open('PUT', url);
    xhr.upload.addEventListener('progress', (event) => {
      if (event.lengthComputable) onProgress(Math.round((event.loaded / event.total) * 100));
    });
    xhr.addEventListener('load', () => {
      if (xhr.status >= 200 && xhr.status < 300) resolve();
      else reject(new Error(tr('Хранилище отклонило файл ({status})', { status: xhr.status })));
    });
    xhr.addEventListener('error', () => {
      reject(new Error(tr('Не удалось загрузить файл в хранилище. Если supabase.co недоступен в вашей сети, включите VPN.')));
    });
    xhr.send(form);
  });
}

async function submitRelease(event) {
  event.preventDefault();
  const result = $('rel-result');
  const progress = $('rel-progress');
  const button = $('rel-submit');
  const file = $('rel-file').files[0];
  if (!file) {
    showResult(result, 'err', tr('Выберите файл образа'));
    return;
  }
  if (file.size > MAX_IMAGE) {
    showResult(result, 'err', tr('Файл больше 50 МиБ: это предел хранилища'));
    return;
  }
  const publish = $('rel-publish').checked;
  button.disabled = true;
  progress.hidden = true;
  try {
    showResult(result, '', tr('Считаем контрольную сумму SHA-256…'));
    const sha256 = await sha256Hex(file);
    showResult(result, '', tr('Готовим загрузку…'));
    const prepared = await api({
      action: 'release_prepare',
      version: $('rel-version').value.trim(),
      title: $('rel-title').value,
      notes: $('rel-notes').value,
      filename: file.name,
      size: file.size,
      sha256,
    });
    progress.value = 0;
    progress.hidden = false;
    await uploadFile(prepared.upload_url, file, (percent) => {
      progress.value = percent;
      showResult(result, '', tr('Загружаем файл: {percent}%', { percent }));
    });
    showResult(result, '', tr('Проверяем файл…'));
    await api({ action: 'release_finalize', id: prepared.id, publish });
    showResult(result, 'ok', publish ? tr('Версия загружена и опубликована.') : tr('Версия загружена как черновик.'));
    $('release-form').reset();
    await loadReleases();
  } catch (err) {
    showResult(result, 'err', err.message);
    await loadReleases();
  } finally {
    progress.hidden = true;
    button.disabled = false;
  }
}

$('release-form').addEventListener('submit', submitRelease);
$('rel-refresh-btn').addEventListener('click', loadReleases);

function renderUpdates() {
  const body = $('upd-rows');
  body.replaceChildren();
  $('upd-empty').hidden = state.updates.length > 0;
  for (const row of state.updates) {
    const [kind, label] = releaseBadge(row);
    const buttons = [];
    if (row.ready) {
      buttons.push(
        el('button', {
          class: `btn small ${row.published ? '' : 'good'}`,
          type: 'button',
          text: row.published ? tr('Скрыть') : tr('Опубликовать'),
          onclick: () => setUpdatePublished(row, !row.published),
        }),
      );
    }
    buttons.push(el('button', { class: 'btn bad small', type: 'button', text: tr('Удалить'), onclick: () => removeUpdate(row) }));
    body.append(
      el('tr', null, [
        el('td', { class: 'devcell' }, [el('span', { class: 'mono', text: row.version }), el('small', { text: row.channel })]),
        el('td', { class: 'devcell' }, [el('span', { class: 'mono', text: String(row.build) }), el('small', { text: fmtSize(row.payload_size) })]),
        el('td', null, [el('span', { class: `badge ${kind}`, text: label })]),
        el('td', null, [el('span', { text: String(row.downloads) })]),
        el('td', null, [el('div', { text: fmt(row.created_at) }), el('small', { class: 'hint', text: row.created_by || '' })]),
        el('td', { class: 'actions' }, buttons),
      ]),
    );
  }
}

async function loadUpdates() {
  try {
    state.updates = (await api({ action: 'updates' })).updates;
    renderUpdates();
  } catch (err) {
    toast(err.message, 'err');
  }
}

async function setUpdatePublished(row, published) {
  try {
    await api({ action: 'update_publish', id: row.id, published });
    toast(published ? tr('Обновление опубликовано') : tr('Обновление скрыто'), 'ok');
    await loadUpdates();
  } catch (err) {
    toast(err.message, 'err');
  }
}

async function removeUpdate(row) {
  if (!confirm(tr('Удалить обновление {build}? Нагрузка будет удалена из хранилища.', { build: row.build }))) return;
  try {
    await api({ action: 'update_remove', id: row.id });
    toast(tr('Обновление удалено'), 'ok');
    await loadUpdates();
  } catch (err) {
    toast(err.message, 'err');
  }
}

async function submitUpdate(event) {
  event.preventDefault();
  const result = $('upd-result');
  const progress = $('upd-progress');
  const button = $('upd-submit');
  const manifestFile = $('upd-manifest').files[0];
  const payloadFile = $('upd-payload').files[0];
  if (!manifestFile || !payloadFile) {
    showResult(result, 'err', tr('Выберите файлы манифеста и нагрузки'));
    return;
  }
  const publish = $('upd-publish').checked;
  button.disabled = true;
  progress.hidden = true;
  try {
    showResult(result, '', tr('Готовим загрузку…'));
    const prepared = await api({ action: 'update_prepare', manifest_file: await manifestFile.text() });
    if (prepared.payload_size !== payloadFile.size) {
      throw new Error(tr('Размер нагрузки не совпадает с манифестом'));
    }
    progress.value = 0;
    progress.hidden = false;
    await uploadFile(prepared.upload_url, payloadFile, (percent) => {
      progress.value = percent;
      showResult(result, '', tr('Загружаем файл: {percent}%', { percent }));
    });
    showResult(result, '', tr('Проверяем файл…'));
    await api({ action: 'update_finalize', id: prepared.id, publish });
    showResult(result, 'ok', publish ? tr('Обновление загружено и опубликовано.') : tr('Обновление загружено как черновик.'));
    $('update-form').reset();
    await loadUpdates();
  } catch (err) {
    showResult(result, 'err', err.message);
    await loadUpdates();
  } finally {
    progress.hidden = true;
    button.disabled = false;
  }
}

$('update-form').addEventListener('submit', submitUpdate);
$('upd-refresh-btn').addEventListener('click', loadUpdates);

$('refresh-btn').addEventListener('click', refresh);

$('open-switch').addEventListener('change', async (event) => {
  const open = event.target.checked;
  try {
    await api({ action: 'set_open', open });
    toast(open ? tr('Приём заявок открыт') : tr('Приём заявок закрыт'), 'ok');
    await refresh();
  } catch (err) {
    event.target.checked = !open;
    toast(err.message, 'err');
  }
});

const FLAG_LABELS = {
  dev_mode: tr('Dev-режим'),
  cloud_sync: tr('Облачная синхронизация'),
  cloud_clipboard: tr('Общий буфер обмена'),
  os_updates: tr('Обновления ОС'),
  media_player: tr('Медиаплеер'),
  wallpapers: tr('Встроенные обои'),
  widgets: tr('Виджеты рабочего стола'),
  '2048': tr('Пасхалка 2048'),
};

const ROLE_LABELS = {
  owner: tr('Owner'),
  admin: tr('Администратор'),
  moderator: tr('Модератор'),
  tester: tr('Тестировщик'),
  user: tr('Пользователь'),
};

function flagLabel(name) {
  return FLAG_LABELS[name] || name;
}

function roleLabel(role) {
  return ROLE_LABELS[role] || role;
}

async function loadManage() {
  try {
    const data = await api({ action: 'dev_mode_list' });
    $('mg-dev-mode').checked = data.dev_mode;
    $('mg-dev-mode-label').textContent = data.dev_mode ? tr('Dev-режим активен') : tr('Dev-режим выключен');
    renderFlags(data.flags);
    renderRoles(data.user_roles);
  } catch (err) {
    if (err.code === 'mfa_required') {
      denyMfa(await fetchMe());
      return;
    }
    if (err.status === 401 || err.status === 403) {
      showDenied(tr('Нужен вход'), tr('Сессия закончилась. Войдите снова.'), [loginButton()]);
      return;
    }
    toast(err.message, 'err');
  }
}

function renderFlags(flags) {
  const box = $('mg-flags-list');
  box.replaceChildren();
  for (const flag of flags) {
    box.append(
      el('label', { class: 'switch' }, [
        el('input', {
          type: 'checkbox',
          checked: flag.enabled,
          onchange: (event) => toggleFlag(flag.name, event.target.checked),
        }),
        el('span', { text: flagLabel(flag.name) }),
        el('small', { class: 'hint', text: flag.description || '' }),
      ]),
    );
  }
}

async function toggleFlag(name, enabled) {
  try {
    await api({ action: 'flags_set', name, enabled });
    toast(enabled ? tr('Флаг включён') : tr('Флаг выключен'), 'ok');
  } catch (err) {
    toast(err.message, 'err');
    await loadManage();
  }
}

function renderRoles(userRoles) {
  const body = $('mg-roles-rows');
  body.replaceChildren();
  $('mg-roles-empty').hidden = userRoles.length > 0;
  for (const row of userRoles) {
    body.append(
      el('tr', null, [
        el('td', { class: 'devcell' }, [
          el('span', { class: 'mono', text: row.profiles ? row.profiles.username : row.user_id.slice(0, 8) + '…' }),
        ]),
        el('td', null, [el('span', { text: row.profiles ? row.profiles.email || '—' : '—' })]),
        el('td', null, [el('span', { class: 'badge', text: roleLabel(row.role_name) })]),
        el('td', null, [el('div', { text: fmt(row.assigned_at) }), el('small', { class: 'hint', text: row.assigned_by || '' })]),
        el('td', { class: 'actions' }, [
          el('button', {
            class: 'btn bad small',
            type: 'button',
            text: tr('Снять'),
            onclick: () => removeRole(row),
          }),
        ]),
      ]),
    );
  }
}

async function removeRole(row) {
  if (!confirm(tr('Снять роль {role} у пользователя {user}?', { role: row.role_name, user: row.profiles ? row.profiles.username : row.user_id }))) return;
  try {
    await api({ action: 'roles_remove', user_id: row.user_id });
    toast(tr('Роль снята'), 'ok');
    await loadManage();
  } catch (err) {
    toast(err.message, 'err');
  }
}

async function submitRoleForm(event) {
  event.preventDefault();
  const result = $('mg-role-result');
  const input = $('mg-user-id');
  let userId = input.value.trim();
  const role = $('mg-role-select').value;
  // If it's not a UUID, try to look up by username
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(userId)) {
    try {
      const users = await api({ action: 'list' });
      // Can't look up by username from list, need a different approach
      // Let's just show error for non-UUID
      showResult(result, 'err', tr('Введите корректный UUID пользователя'));
      return;
    } catch {
      showResult(result, 'err', tr('Введите корректный UUID пользователя'));
      return;
    }
  }
  const submitBtn = $('mg-role-submit');
  submitBtn.disabled = true;
  try {
    await api({ action: 'roles_assign', user_id: userId, role_name: role });
    showResult(result, 'ok', tr('Роль назначена'));
    input.value = '';
    await loadManage();
  } catch (err) {
    showResult(result, 'err', err.message);
  } finally {
    submitBtn.disabled = false;
  }
}

$('mg-role-form').addEventListener('submit', submitRoleForm);
$('mg-refresh-btn').addEventListener('click', loadManage);
$('mg-dev-mode').addEventListener('change', async (event) => {
  const enabled = event.target.checked;
  try {
    await api({ action: 'dev_mode_set', enabled });
    toast(enabled ? tr('Dev-режим активирован') : tr('Dev-режим деактивирован'), 'ok');
    $('mg-dev-mode-label').textContent = enabled ? tr('Dev-режим активен') : tr('Dev-режим выключен');
  } catch (err) {
    event.target.checked = !enabled;
    toast(err.message, 'err');
  }
});

(async () => {
  const user = await fetchMe();
  if (!user) {
    showDenied(tr('Нужен вход'), tr('Панель доступна только владельцу проекта. Войдите в аккаунт Owner.'), [loginButton()]);
  } else if (user.role !== 'owner') {
    showDenied(tr('Нет доступа'), tr('Ваш аккаунт не имеет прав Owner.'), [el('a', { class: 'btn', href: lp('/account'), text: tr('В аккаунт') })]);
  } else if (user.aal !== 'aal2') {
    denyMfa(user);
  } else {
    showPanel();
  }
})();
