'use strict';

const STORE_KEY = 'deix.deviceId';
const idInput = document.getElementById('device-id');
const statusInput = document.getElementById('status-id');
const submitResult = document.getElementById('submit-result');
const statusResult = document.getElementById('status-result');

const saved = localStorage.getItem(STORE_KEY);
if (saved) {
  idInput.value = saved;
  statusInput.value = saved;
}

document.getElementById('submit-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const button = document.getElementById('submit-btn');
  button.disabled = true;
  try {
    const id = idInput.value.trim().toLowerCase();
    const data = await postJson('/api/request', {
      action: 'submit',
      device_id: id,
      name: document.getElementById('name').value,
      note: document.getElementById('note').value,
      website: document.getElementById('website').value,
    });
    localStorage.setItem(STORE_KEY, id);
    statusInput.value = id;
    showResult(submitResult, data.result === 'created' ? 'ok' : '', data.message);
  } catch (err) {
    showResult(submitResult, 'err', err.message);
  } finally {
    button.disabled = false;
  }
});

document.getElementById('status-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  try {
    const data = await postJson('/api/request', {
      action: 'status',
      device_id: statusInput.value.trim().toLowerCase(),
    });
    showResult(statusResult, data.status === 'approved' || data.status === 'consumed' ? 'ok' : data.status === 'denied' ? 'err' : '', data.message);
  } catch (err) {
    showResult(statusResult, 'err', err.message);
  }
});
