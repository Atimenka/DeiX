'use strict';

document.getElementById('gallery').addEventListener('click', async (event) => {
  const button = event.target.closest('button[data-copy]');
  if (!button) return;
  try {
    await navigator.clipboard.writeText(button.dataset.copy);
    toast(tr('Команда скопирована'), 'ok');
  } catch (err) {
    toast(tr('Не удалось скопировать: выделите команду вручную'), 'err');
  }
});
