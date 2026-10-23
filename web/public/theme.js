(function () {
  var root = document.documentElement;
  var theme = 'dark';
  var lang = '';
  try {
    theme = localStorage.getItem('deix-theme') === 'light' ? 'light' : 'dark';
    lang = localStorage.getItem('deix-lang') || '';
  } catch (e) {
    theme = 'dark';
  }
  root.setAttribute('data-theme', theme);
  root.classList.add('js');
  try {
    if (sessionStorage.getItem('deix-wipe')) {
      sessionStorage.removeItem('deix-wipe');
      root.classList.add('wiped');
    }
  } catch (e) {
    root.classList.remove('wiped');
  }
  var path = location.pathname;
  if (path === '/en' || path.indexOf('/en/') === 0) {
    root.lang = 'en';
  } else if (path === '/' && !lang && /^en\b/i.test(navigator.language || '')) {
    location.replace('/en' + location.search + location.hash);
  }
})();
