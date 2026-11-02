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
  var mode = 'beauty';
  try {
    var savedMode = localStorage.getItem('deix-mode');
    mode = savedMode === 'femboy' || savedMode === 'plain' ? savedMode : 'beauty';
  } catch (e) {
    mode = 'beauty';
  }
  root.setAttribute('data-mode', mode);
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

  var reduced = false;
  try {
    reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  } catch (e) {
    reduced = false;
  }
  var isHome = path === '/' || path === '/en' || path === '/en/';
  var seen = false;
  try {
    seen = Boolean(sessionStorage.getItem('deix-boot'));
  } catch (e) {
    seen = true;
  }
  if (isHome && !seen && !reduced && mode !== 'plain' && !root.classList.contains('wiped') && !location.hash) {
    try {
      sessionStorage.setItem('deix-boot', '1');
    } catch (e) {
      seen = true;
    }
    root.classList.add('booting');
    var boot = document.createElement('div');
    boot.className = 'boot';
    boot.setAttribute('aria-hidden', 'true');
    boot.innerHTML = '<div class="boot-in"><div class="boot-log"></div><div class="boot-num">0</div><div class="boot-bar"><i></i></div><div class="boot-tip">press ` for a shell</div></div>';
    root.appendChild(boot);
    var log = boot.querySelector('.boot-log');
    var num = boot.querySelector('.boot-num');
    var bar = boot.querySelector('.boot-bar i');
    var steps = [mode === 'femboy' ? 'DEIX OS 0.3.0-beta  x86_64  <3' : 'DEIX OS 0.3.0-beta  x86_64', 'cpu: long mode', 'mm: heap ready', 'vfs: /system erofs ro', 'vfs: /userdata xts-aes-256', 'net: rtl8139 + tls 1.3', 'gui: vbe 32 bpp', 'dinit: pid 1'];
    var n = 0;
    var timer = setInterval(function () {
      if (n < steps.length) {
        var row = document.createElement('div');
        row.textContent = (n ? '[ ok ] ' : '') + steps[n];
        log.appendChild(row);
      }
      n += 1;
      var pct = Math.min(100, Math.round((n / (steps.length + 1)) * 100));
      num.textContent = String(pct);
      bar.style.width = pct + '%';
      if (pct >= 100) {
        clearInterval(timer);
        setTimeout(function () {
          boot.classList.add('out');
          root.classList.remove('booting');
        }, 160);
        setTimeout(function () {
          if (boot.parentNode) boot.parentNode.removeChild(boot);
        }, 1100);
      }
    }, 190);
  }
  if (path === '/en' || path.indexOf('/en/') === 0) {
    root.lang = 'en';
  } else if (path === '/' && !lang && /^en\b/i.test(navigator.language || '')) {
    location.replace('/en' + location.search + location.hash);
  }
})();
