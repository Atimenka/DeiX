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
let state = { requests: [], counts: {}, tab: 'pending', timer: null };

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
