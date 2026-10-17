'use strict';

const STATUS_LABEL = { pending: 'На рассмотрении', approved: 'Одобрена', consumed: 'Токен выдан', denied: 'Отклонена' };
const submitResult = document.getElementById('submit-result');

async function loadMine() {
  const box = document.getElementById('mine');
  try {
    const data = await postJson('/api/request', { action: 'mine' });
    box.replaceChildren();
    if (!data.requests.length) {
      box.append(el('p', { class: 'hint', text: 'Заявок пока нет.' }));
      return;
    }
    for (const row of data.requests) {
      box.append(
        el('div', { class: 'mine-item' }, [
          el('div', null, [
            el('span', { class: 'mono', text: row.device_id }),
            document.createTextNode(' '),
            el('span', { class: `badge ${row.status}`, text: STATUS_LABEL[row.status] || row.status }),
          ]),
          el('div', { class: 'hint', text: row.message }),
        ]),
      );
    }
  } catch (err) {
    box.replaceChildren(el('p', { class: 'hint', text: err.message }));
  }
}

(async () => {
  const user = await fetchMe();
  if (!user) {
    document.getElementById('need-login').hidden = false;
    return;
  }
  document.getElementById('request-view').hidden = false;
  loadMine();
})();

document.getElementById('submit-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const button = document.getElementById('submit-btn');
  button.disabled = true;
  try {
    const data = await postJson('/api/request', {
      action: 'submit',
      device_id: document.getElementById('device-id').value.trim().toLowerCase(),
      name: document.getElementById('name').value,
      note: document.getElementById('note').value,
    });
    showResult(submitResult, data.result === 'created' ? 'ok' : '', data.message);
    loadMine();
  } catch (err) {
    showResult(submitResult, 'err', err.message);
  } finally {
    button.disabled = false;
  }
});

document.getElementById('reload-btn').addEventListener('click', loadMine);
