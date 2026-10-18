(function () {
  var theme = 'dark';
  try {
    theme = localStorage.getItem('deix-theme') === 'light' ? 'light' : 'dark';
  } catch (e) {
    theme = 'dark';
  }
  document.documentElement.setAttribute('data-theme', theme);
  document.documentElement.classList.add('js');
})();
