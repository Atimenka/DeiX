/* Движение сайта: плавная прокрутка, анимации при прокрутке, 3D-сцена героя, наклон карточек, конфетти.
   Все библиотеки лежат в /vendor и подгружаются только там, где нужны. Без JS и при
   prefers-reduced-motion сайт остаётся полностью рабочим. */
(function () {
  'use strict';

  const root = document.documentElement;
  const mode = root.getAttribute('data-mode') || 'beauty';
  const plain = mode === 'plain';
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches || plain;
  const finePointer = window.matchMedia('(hover: hover) and (pointer: fine)').matches;
  const page = document.body.dataset.page || '';
  const loaded = {};

  function load(src) {
    if (!loaded[src]) {
      loaded[src] = new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = src;
        script.onload = resolve;
        script.onerror = () => reject(new Error(`не загрузился ${src}`));
        document.head.append(script);
      });
    }
    return loaded[src];
  }

  function accentColors() {
    const css = getComputedStyle(root);
    return [css.getPropertyValue('--accent').trim() || '#ff3b30', css.getPropertyValue('--accent-2').trim() || '#f3f2ee', '#ff7a3c', '#ffffff'];
  }

  let confettiFire = null;
  let scene = null;

  /** Праздничный всплеск после удачного действия (регистрация, заявка, привязка). Без Web Worker: CSP его запрещает. */
  window.deixCelebrate = function celebrate() {
    if (reduce) return;
    if (scene && scene.pulse) scene.pulse();
    load('/vendor/confetti.min.js')
      .then(() => {
        if (!confettiFire) {
          const canvas = document.createElement('canvas');
          canvas.className = 'confetti-canvas';
          canvas.setAttribute('aria-hidden', 'true');
          document.body.append(canvas);
          confettiFire = window.confetti.create(canvas, { resize: true, useWorker: false });
        }
        return confettiFire({ particleCount: 110, spread: 80, startVelocity: 38, origin: { y: 0.72 }, colors: accentColors(), disableForReducedMotion: true });
      })
      .catch(() => undefined);
  };

  function watchNav() {
    const nav = document.querySelector('.nav');
    if (!nav) return;
    let ticking = false;
    function update() {
      ticking = false;
      nav.classList.toggle('nav-solid', window.scrollY > 24);
    }
    window.addEventListener(
      'scroll',
      () => {
        if (!ticking) {
          ticking = true;
          requestAnimationFrame(update);
        }
      },
      { passive: true },
    );
    update();
  }

  /** Число в тексте карточки «~32 000» → анимированный счётчик с тем же форматом. */
  function parseCounter(text) {
    const m = /^(~?)(\d[\d\s\u00a0.,]*\d|\d)(.*)$/.exec(text.trim());
    if (!m) return null;
    const token = m[2];
    const decimal = /^\d+([.,])\d{1,2}$/.exec(token);
    if (decimal) {
      return { prefix: m[1], suffix: m[3], value: parseFloat(token.replace(',', '.')), decimals: token.length - token.indexOf(decimal[1]) - 1, point: decimal[1], group: '' };
    }
    const group = /[\s\u00a0.,]/.exec(token);
    return { prefix: m[1], suffix: m[3], value: parseInt(token.replace(/[\s\u00a0.,]/g, ''), 10), decimals: 0, point: '', group: group ? group[0] : '' };
  }

  function formatCounter(info, value) {
    let body = info.decimals ? value.toFixed(info.decimals).replace('.', info.point) : String(Math.round(value));
    if (info.group) body = body.replace(/\B(?=(\d{3})+(?!\d))/g, info.group);
    return `${info.prefix}${body}${info.suffix}`;
  }

  function counters(gsap, ScrollTrigger) {
    document.querySelectorAll('.stat b').forEach((node) => {
      const info = parseCounter(node.textContent);
      if (!info || !Number.isFinite(info.value)) return;
      const final = node.textContent;
      node.style.minWidth = `${node.offsetWidth}px`;
      ScrollTrigger.create({
        trigger: node,
        start: 'top 92%',
        once: true,
        onEnter: () => {
          const state = { v: 0 };
          node.textContent = formatCounter(info, 0);
          gsap.to(state, {
            v: info.value,
            duration: 1.8,
            ease: 'power3.out',
            onUpdate: () => {
              node.textContent = formatCounter(info, state.v);
            },
            onComplete: () => {
              node.textContent = final;
            },
          });
        },
      });
    });
  }

  function headings(gsap, SplitText) {
    document.querySelectorAll('section.block h2').forEach((h2) => {
      h2.classList.add('in');
      SplitText.create(h2, {
        type: 'lines',
        mask: 'lines',
        autoSplit: true,
        onSplit(self) {
          return gsap.from(self.lines, { yPercent: 105, duration: 0.9, ease: 'expo.out', stagger: 0.09, scrollTrigger: { trigger: h2, start: 'top 88%', once: true } });
        },
      });
    });
  }

  function heroTitle(gsap, SplitText) {
    const h1 = document.querySelector('.hero h1');
    if (!h1) return;
    h1.classList.add('in');
    SplitText.create(h1, {
      type: 'lines',
      mask: 'lines',
      autoSplit: true,
      onSplit(self) {
        return gsap.from(self.lines, { yPercent: 112, duration: 1.1, ease: 'expo.out', stagger: 0.12, delay: root.classList.contains('booting') ? 1.75 : 0.1 });
      },
    });
  }

  /** Параллакс окон «рабочего стола» по указателю и прокрутке. Свойство translate не конфликтует с CSS-анимацией float. */
  function desktopParallax() {
    const desktop = document.querySelector('.desktop');
    if (!desktop || !finePointer) return;
    const layers = [
      [desktop.querySelector('.term'), 14],
      [desktop.querySelector('.mon'), 26],
      [desktop.querySelector('.os-toast'), 38],
      [desktop.querySelector('.dock'), 20],
    ].filter(([node]) => node);
    const pos = { x: 0, y: 0 };
    const goal = { x: 0, y: 0 };
    let running = false;
    function tick() {
      pos.x += (goal.x - pos.x) * 0.08;
      pos.y += (goal.y - pos.y) * 0.08;
      for (const [node, depth] of layers) node.style.translate = `${(pos.x * depth).toFixed(2)}px ${(pos.y * depth).toFixed(2)}px`;
      if (Math.abs(goal.x - pos.x) > 0.001 || Math.abs(goal.y - pos.y) > 0.001) requestAnimationFrame(tick);
      else running = false;
    }
    window.addEventListener(
      'pointermove',
      (event) => {
        goal.x = (event.clientX / window.innerWidth - 0.5) * 2;
        goal.y = (event.clientY / window.innerHeight - 0.5) * 2;
        if (!running) {
          running = true;
          requestAnimationFrame(tick);
        }
      },
      { passive: true },
    );
  }

  function magnetic() {
    if (!finePointer) return;
    document.querySelectorAll('.btn.large, .hero .btn').forEach((btn) => {
      btn.addEventListener('pointermove', (event) => {
        const box = btn.getBoundingClientRect();
        const dx = (event.clientX - (box.left + box.width / 2)) / box.width;
        const dy = (event.clientY - (box.top + box.height / 2)) / box.height;
        btn.style.translate = `${(dx * 10).toFixed(1)}px ${(dy * 8).toFixed(1)}px`;
      });
      btn.addEventListener('pointerleave', () => {
        btn.style.translate = '';
      });
    });
  }

  function tilt() {
    if (!finePointer || !window.VanillaTilt) return;
    const cards = document.querySelectorAll('main .card, main .stat');
    window.VanillaTilt.init(Array.from(cards), { max: 5, speed: 500, perspective: 1000, scale: 1.015, glare: false });
  }

  function smoothScroll(gsap, ScrollTrigger) {
    if (!window.Lenis) return null;
    const lenis = new window.Lenis({ lerp: 0.11, smoothWheel: true });
    lenis.on('scroll', ScrollTrigger.update);
    gsap.ticker.add((time) => lenis.raf(time * 1000));
    gsap.ticker.lagSmoothing(0);
    document.addEventListener('click', (event) => {
      const link = event.target instanceof Element ? event.target.closest('a[href]') : null;
      if (!link || event.defaultPrevented || event.metaKey || event.ctrlKey || event.shiftKey || event.button !== 0) return;
      const url = new URL(link.href, location.href);
      if (url.origin !== location.origin || url.pathname.replace(/\/$/, '') !== location.pathname.replace(/\/$/, '') || !url.hash) return;
      const target = document.querySelector(url.hash);
      if (!target) return;
      event.preventDefault();
      lenis.scrollTo(target, { offset: -84, duration: 1.2 });
      history.replaceState(null, '', url.hash);
    });
    return lenis;
  }

  function webglOk() {
    try {
      const canvas = document.createElement('canvas');
      return Boolean(canvas.getContext('webgl2') || canvas.getContext('webgl'));
    } catch {
      return false;
    }
  }

  const SCENE_PAGES = ['home', 'download', 'login', 'register', 'request', 'verify', '404'];

  function mountScene() {
    if (plain || !SCENE_PAGES.includes(page) || !webglOk()) return;
    const lite = window.innerWidth < 760 || (navigator.deviceMemory || 8) <= 4 || (navigator.hardwareConcurrency || 8) <= 4;
    const start = () =>
      load('/vendor/three-scene.js?v=7')
        .then(() => {
          scene = window.DeixScene.mount({
            still: reduce,
            dim: page !== 'home',
            lite,
            onSlow: () => {
              scene = null;
              root.classList.remove('has-gl');
            },
          });
          root.classList.add('has-gl');
        })
        .catch(() => root.classList.remove('has-gl'));
    if ('requestIdleCallback' in window) window.requestIdleCallback(start, { timeout: 1500 });
    else setTimeout(start, 300);
  }

  /** Интерактивная 3D-модель в разделе главной; список рядом работает и без WebGL. */
  function mountModel(hostId, listId, factory, initial) {
    const host = document.getElementById(hostId);
    const list = document.getElementById(listId);
    if (!host || !list) return;
    const box = host.closest('.stack');
    const items = Array.from(list.querySelectorAll('.stack-item'));
    let view = null;
    const mark = (index) => {
      items.forEach((item) => item.classList.toggle('on', Number(item.dataset.i) === index));
      if (view) view.setActive(index);
    };
    items.forEach((item) => {
      const index = Number(item.dataset.i);
      item.addEventListener('pointerenter', () => mark(index));
      item.addEventListener('focus', () => mark(index));
    });
    mark(initial);
    if (!webglOk()) return;
    box.classList.add('has-3d');
    const lite = window.innerWidth < 760 || (navigator.deviceMemory || 8) <= 4;
    const start = () =>
      load('/vendor/three-scene.js?v=7')
        .then(() => {
          view = window.DeixScene[factory](host, { still: reduce, lite, initial, onHover: mark });
        })
        .catch(() => box.classList.remove('has-3d'));
    if ('IntersectionObserver' in window) {
      const watch = new IntersectionObserver((entries) => {
        if (!entries[0].isIntersecting) return;
        watch.disconnect();
        start();
      }, { rootMargin: '500px 0px' });
      watch.observe(host);
    } else start();
  }

  /** Объёмный логотип: без WebGL остаётся плоская надпись. */
  function mountSolo(hostId, factory, extra) {
    const host = document.getElementById(hostId);
    if (!host || !webglOk()) return;
    const box = host.closest('.logo-stage');
    box.classList.add('has-3d');
    const lite = window.innerWidth < 760 || (navigator.deviceMemory || 8) <= 4;
    const start = () =>
      load('/vendor/three-scene.js?v=7')
        .then(() => {
          window.DeixScene[factory](host, { still: reduce, lite, ...extra });
        })
        .catch(() => box.classList.remove('has-3d'));
    if ('IntersectionObserver' in window) {
      const watch = new IntersectionObserver((entries) => {
        if (!entries[0].isIntersecting) return;
        watch.disconnect();
        start();
      }, { rootMargin: '500px 0px' });
      watch.observe(host);
    } else start();
  }

  /** Эффекты «расшифровки»: подписи разделов проявляются из случайных символов. */
  function scramble() {
    const glyphs = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789#%/<>';
    const labels = Array.from(document.querySelectorAll('.eyebrow'));
    if (!labels.length || !('IntersectionObserver' in window)) return;
    const watch = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        watch.unobserve(entry.target);
        const el = entry.target;
        const text = el.textContent;
        const started = performance.now();
        const tick = (now) => {
          const done = Math.min(1, (now - started) / 650);
          let out = '';
          for (let i = 0; i < text.length; i += 1) {
            out += text[i] === ' ' || i < done * text.length ? text[i] : glyphs[Math.floor(Math.random() * glyphs.length)];
          }
          el.textContent = out;
          if (done < 1) requestAnimationFrame(tick);
          else el.textContent = text;
        };
        requestAnimationFrame(tick);
      });
    }, { threshold: 0.6 });
    labels.forEach((label) => watch.observe(label));
  }

  /** Красная шторка при переходе между страницами сайта. */
  function wipe() {
    const veil = document.createElement('div');
    veil.className = 'wipe';
    veil.setAttribute('aria-hidden', 'true');
    document.body.append(veil);
    window.addEventListener('pageshow', () => veil.classList.remove('go'));
    document.addEventListener('click', (event) => {
      const link = event.target.closest && event.target.closest('a[href]');
      if (!link || event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      if (link.target || link.hasAttribute('download')) return;
      const url = new URL(link.href, location.href);
      if (url.origin !== location.origin || url.pathname === location.pathname || url.pathname.startsWith('/api/')) return;
      event.preventDefault();
      try {
        sessionStorage.setItem('deix-wipe', '1');
      } catch {
        /* без хранилища шторка работает только на уходе */
      }
      veil.classList.add('go');
      setTimeout(() => {
        location.href = url.href;
      }, 360);
    });
  }

  /** Скролл-сцена: окно ОС разворачивается, шаги dev-режима зажигаются по мере прокрутки, лента ускоряется. */
  function scrollStory(gsap, ScrollTrigger) {
    const desktop = document.querySelector('.desktop');
    const hero = document.querySelector('.hero');
    if (desktop && hero && window.innerWidth > 900) {
      gsap.to(desktop, { transformPerspective: 1100, rotationX: 14, rotationY: -13, y: -36, scale: 0.93, ease: 'none', scrollTrigger: { trigger: hero, start: 'top top', end: 'bottom top', scrub: 0.6 } });
    }
    const steps = document.querySelector('.steps');
    if (steps) {
      const items = Array.from(steps.querySelectorAll('.step'));
      ScrollTrigger.create({
        trigger: steps,
        start: 'top 78%',
        end: 'bottom 55%',
        scrub: true,
        onUpdate: (self) => {
          steps.style.setProperty('--p', self.progress.toFixed(3));
          items.forEach((item, index) => item.classList.toggle('on', self.progress >= index / items.length - 0.02));
        },
      });
    }
    const track = document.querySelector('.ticker-track');
    if (track) {
      let rate = 1;
      let goal = 1;
      ScrollTrigger.create({
        onUpdate: (self) => {
          goal = (self.direction >= 0 ? 1 : -1) * (1 + Math.min(Math.abs(self.getVelocity()) / 350, 7));
        },
      });
      gsap.ticker.add(() => {
        goal += ((goal >= 0 ? 1 : -1) - goal) * 0.04;
        rate += (goal - rate) * 0.12;
        for (const animation of track.getAnimations()) animation.playbackRate = rate;
      });
    }
  }

  /** Кольцо и точка следуют за курсором и увеличиваются над кликабельным. */
  function cursor() {
    if (!finePointer || page === 'admin') return;
    const dot = document.createElement('div');
    const ring = document.createElement('div');
    dot.className = 'cursor-dot';
    ring.className = 'cursor-ring';
    dot.setAttribute('aria-hidden', 'true');
    ring.setAttribute('aria-hidden', 'true');
    document.body.append(ring, dot);
    const pos = { x: -100, y: -100 };
    const lag = { x: -100, y: -100 };
    let running = false;
    function tick() {
      lag.x += (pos.x - lag.x) * 0.18;
      lag.y += (pos.y - lag.y) * 0.18;
      ring.style.translate = `${lag.x.toFixed(1)}px ${lag.y.toFixed(1)}px`;
      dot.style.translate = `${pos.x.toFixed(1)}px ${pos.y.toFixed(1)}px`;
      if (Math.abs(pos.x - lag.x) > 0.1 || Math.abs(pos.y - lag.y) > 0.1) requestAnimationFrame(tick);
      else running = false;
    }
    window.addEventListener(
      'pointermove',
      (event) => {
        pos.x = event.clientX;
        pos.y = event.clientY;
        root.classList.add('has-cursor');
        const hot = event.target instanceof Element && event.target.closest('a, button, summary, input, select, textarea, label, .tab, .card');
        ring.classList.toggle('hot', Boolean(hot));
        if (!running) {
          running = true;
          requestAnimationFrame(tick);
        }
      },
      { passive: true },
    );
    document.addEventListener('pointerleave', () => root.classList.remove('has-cursor'));
  }

  /** Свет под курсором в сетке возможностей, плитках и шагах. */
  function spotlight() {
    if (!finePointer) return;
    const groups = document.querySelectorAll('.bento, .steps, .stats');
    groups.forEach((group) => {
      const items = Array.from(group.children);
      group.addEventListener(
        'pointermove',
        (event) => {
          items.forEach((item) => {
            const box = item.getBoundingClientRect();
            item.style.setProperty('--mx', `${(event.clientX - box.left).toFixed(0)}px`);
            item.style.setProperty('--my', `${(event.clientY - box.top).toFixed(0)}px`);
          });
        },
        { passive: true },
      );
    });
  }

  /** Огромная строка между разделами наклоняется по скорости прокрутки. */
  function megaSkew(gsap) {
    const track = document.querySelector('.mega-track');
    if (!track) return;
    const setSkew = gsap.quickSetter(track, 'skewX', 'deg');
    let lastY = window.scrollY;
    let velocity = 0;
    let skew = 0;
    gsap.ticker.add(() => {
      velocity = velocity * 0.9 + (window.scrollY - lastY) * 0.1;
      lastY = window.scrollY;
      const target = Math.max(-16, Math.min(16, -velocity * 0.9));
      skew += (target - skew) * 0.15;
      setSkew(skew);
    });
  }

  /** Скрытый шелл: клавиша ` или Ctrl+K. */
  function shell() {
    const base = location.pathname === '/en' || location.pathname.startsWith('/en/') ? '/en' : '';
    const pages = { home: '/', download: '/download', request: '/request', login: '/login', register: '/register', account: '/account' };
    const facts = [
      'OS      DeiX OS 0.3.0-beta',
      'Kernel  Rust no_std, x86_64 long mode',
      '/system EROFS, read-only',
      '/user   ext2/4 over XTS-AES-256',
      'Net     RTL8139, own TLS 1.3 stack',
      'GUI     VBE 32 bpp',
      'Init    dinit (PID 1)',
    ];
    let box = null;
    let out = null;
    let input = null;
    let rain = null;
    const past = [];
    let cursorAt = 0;

    function print(text, cls) {
      const row = document.createElement('div');
      row.textContent = text;
      if (cls) row.className = cls;
      out.append(row);
      out.scrollTop = out.scrollHeight;
    }

    function go(path) {
      const a = document.createElement('a');
      a.href = path;
      a.hidden = true;
      document.body.append(a);
      a.click();
      a.remove();
    }

    function matrix() {
      if (rain) return;
      rain = document.createElement('canvas');
      rain.className = 'sh-rain';
      box.prepend(rain);
      const ctx = rain.getContext('2d');
      const w = (rain.width = window.innerWidth);
      const h = (rain.height = window.innerHeight);
      const cols = Math.ceil(w / 16);
      const drops = Array.from({ length: cols }, () => Math.random() * -40);
      const chars = '01DEIX<>/{}#';
      const started = performance.now();
      const draw = (now) => {
        if (!rain) return;
        ctx.fillStyle = 'rgba(11,11,12,0.14)';
        ctx.fillRect(0, 0, w, h);
        ctx.fillStyle = accentColors()[0];
        ctx.font = '15px monospace';
        drops.forEach((y, i) => {
          ctx.fillText(chars[Math.floor(Math.random() * chars.length)], i * 16, y * 16);
          drops[i] = y * 16 > h && Math.random() > 0.97 ? 0 : y + 1;
        });
        if (now - started < 9000) requestAnimationFrame(draw);
        else {
          rain.remove();
          rain = null;
        }
      };
      requestAnimationFrame(draw);
    }

    const commands = {
      help: () => print('help  about  neofetch  ls  open <page>  theme  mode <beauty|femboy|plain>  lang  github  matrix  clear  exit'),
      mode: (arg) => {
        const key = (arg || '').trim();
        if (!['beauty', 'femboy', 'plain'].includes(key)) return print(`mode: now ${mode}. usage: mode beauty | femboy | plain`, 'sh-err');
        if (key === mode) return print(`mode: already ${key}`);
        print(`mode: ${key}, reloading...`);
        try {
          localStorage.setItem('deix-mode', key);
        } catch {
          return print('mode: storage is blocked, cannot save', 'sh-err');
        }
        return setTimeout(() => location.reload(), 300);
      },
      femboy: () => commands.mode('femboy'),
      about: () => print('DeiX is a small operating system written from scratch in Rust. This page is its website.'),
      neofetch: () => facts.forEach((line) => print(line, 'sh-fact')),
      ls: () => print(Object.keys(pages).join('  ')),
      open: (arg) => {
        const key = (arg || '').trim();
        if (!pages[key]) return print(`open: no such page: ${key || '(none)'}`, 'sh-err');
        print(`opening ${key}...`);
        return setTimeout(() => go(base + (pages[key] === '/' ? (base ? '/en' : '/') : pages[key])), 200);
      },
      theme: () => {
        const btn = document.querySelector('.theme-toggle');
        if (btn) btn.click();
        print(`theme: ${root.getAttribute('data-theme')}`);
      },
      lang: () => {
        try {
          localStorage.setItem('deix-lang', base ? 'ru' : 'en');
        } catch {
          /* язык не сохранится */
        }
        go(base ? '/' : '/en');
      },
      github: () => {
        const link = document.querySelector('a[href*="github.com"]');
        if (!link) return print('github: link not found on this page', 'sh-err');
        print(link.href);
        return window.open(link.href, '_blank', 'noopener');
      },
      matrix,
      sudo: () => print('this website has no root. the kernel does.', 'sh-err'),
      clear: () => {
        out.textContent = '';
      },
      exit: () => close(),
    };

    function run(line) {
      print(`$ ${line}`, 'sh-cmd');
      const [name, ...rest] = line.trim().split(/\s+/);
      if (!name) return;
      if (Object.prototype.hasOwnProperty.call(commands, name)) commands[name](rest.join(' '));
      else print(`${name}: command not found. try help`, 'sh-err');
    }

    function close() {
      if (!box) return;
      box.classList.remove('on');
      rain = null;
      document.body.classList.remove('sh-open');
      setTimeout(() => {
        if (box.classList.contains('on')) return;
        box.classList.remove('show');
        const canvas = box.querySelector('canvas');
        if (canvas) canvas.remove();
      }, 300);
    }

    function open() {
      if (!box) {
        box = document.createElement('div');
        box.className = 'sh';
        box.setAttribute('role', 'dialog');
        box.setAttribute('aria-label', 'DeiX shell');
        const win = document.createElement('div');
        win.className = 'sh-win';
        const bar = document.createElement('div');
        bar.className = 'sh-bar';
        bar.textContent = 'deix — shell';
        out = document.createElement('div');
        out.className = 'sh-out';
        const row = document.createElement('label');
        row.className = 'sh-row';
        const prompt = document.createElement('span');
        prompt.textContent = '$';
        input = document.createElement('input');
        input.className = 'sh-in';
        input.spellcheck = false;
        input.autocomplete = 'off';
        input.setAttribute('aria-label', 'command');
        row.append(prompt, input);
        win.append(bar, out, row);
        box.append(win);
        document.body.append(box);
        box.addEventListener('pointerdown', (event) => {
          if (event.target === box) close();
        });
        input.addEventListener('keydown', (event) => {
          if (event.key === 'Enter') {
            const line = input.value;
            input.value = '';
            if (line.trim()) past.push(line);
            cursorAt = past.length;
            run(line);
          } else if (event.key === 'ArrowUp') {
            event.preventDefault();
            cursorAt = Math.max(0, cursorAt - 1);
            input.value = past[cursorAt] || '';
          } else if (event.key === 'ArrowDown') {
            event.preventDefault();
            cursorAt = Math.min(past.length, cursorAt + 1);
            input.value = past[cursorAt] || '';
          } else if (event.key === 'Tab') {
            event.preventDefault();
            const hit = Object.keys(commands).filter((name) => name.startsWith(input.value));
            if (hit.length === 1) input.value = hit[0];
          }
        });
        print('DeiX shell. type help.');
      }
      document.body.classList.add('sh-open');
      box.classList.add('show');
      box.getBoundingClientRect();
      box.classList.add('on');
      input.focus();
    }

    document.addEventListener('keydown', (event) => {
      const typing = event.target instanceof Element && event.target.closest('input, textarea, select, [contenteditable]');
      if (event.key === 'Escape') return close();
      if (typing) return;
      if (event.key === '`' || ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'k')) {
        event.preventDefault();
        if (box && box.classList.contains('on')) close();
        else open();
      }
    });
  }

  /** Режим «Фембой»: плывущие сердечки и звёздочки, сердечки по клику. */
  function femFx() {
    if (reduce || mode !== 'femboy') return;
    const glyphs = ['♡', '✦', '♥', '✧', '♡', '✿'];
    const colors = ['var(--accent)', 'var(--accent-2)', '#ffffff', 'var(--accent-soft)'];
    const layer = document.createElement('div');
    layer.className = 'fem-fx';
    layer.setAttribute('aria-hidden', 'true');
    const count = window.innerWidth < 760 ? 9 : 18;
    for (let i = 0; i < count; i += 1) {
      const node = document.createElement('i');
      node.textContent = glyphs[i % glyphs.length];
      node.style.setProperty('--x', `${(Math.random() * 100).toFixed(1)}vw`);
      node.style.setProperty('--s', `${(14 + Math.random() * 20).toFixed(0)}px`);
      node.style.setProperty('--c', colors[i % colors.length]);
      node.style.setProperty('--d', `${(14 + Math.random() * 14).toFixed(1)}s`);
      node.style.setProperty('--w', `${(-Math.random() * 24).toFixed(1)}s`);
      node.style.setProperty('--dx', `${(Math.random() * 80 - 40).toFixed(0)}px`);
      layer.append(node);
    }
    document.body.append(layer);
    document.addEventListener('pointerdown', (event) => {
      if (!(event.target instanceof Element) || event.target.closest('input, textarea, select, .sh')) return;
      for (let i = 0; i < 6; i += 1) {
        const node = document.createElement('i');
        node.className = 'fem-pop';
        node.setAttribute('aria-hidden', 'true');
        node.textContent = glyphs[(i + Math.floor(Math.random() * 3)) % glyphs.length];
        node.style.setProperty('--x', `${event.clientX}px`);
        node.style.setProperty('--y', `${event.clientY}px`);
        node.style.setProperty('--dx', `${(Math.random() * 120 - 60).toFixed(0)}px`);
        node.style.setProperty('--dy', `${(-30 - Math.random() * 90).toFixed(0)}px`);
        node.style.setProperty('--s', `${(14 + Math.random() * 14).toFixed(0)}px`);
        node.style.setProperty('--c', colors[i % colors.length]);
        document.body.append(node);
        setTimeout(() => node.remove(), 950);
      }
    });
  }

  /** Фон светится под курсором, текст hero чуть поворачивается вслед. */
  function followPointer() {
    if (!finePointer) return;
    const lead = document.querySelector('.hero-grid > div:first-child');
    let frame = 0;
    let ev = null;
    const apply = () => {
      frame = 0;
      root.style.setProperty('--px', `${ev.clientX}px`);
      root.style.setProperty('--py', `${ev.clientY}px`);
      if (lead) {
        lead.style.setProperty('--tx', ((ev.clientX / window.innerWidth) * 2 - 1).toFixed(3));
        lead.style.setProperty('--ty', ((ev.clientY / window.innerHeight) * 2 - 1).toFixed(3));
      }
    };
    window.addEventListener(
      'pointermove',
      (event) => {
        ev = event;
        if (!frame) frame = requestAnimationFrame(apply);
      },
      { passive: true },
    );
  }

  function panels(gsap) {
    const targets = document.querySelectorAll('.auth-card:not(.reveal), .page-head:not(.reveal), .panel:not(.reveal):not(.rel-panel)');
    if (!targets.length) return;
    gsap.from(targets, { y: 22, opacity: 0, duration: 0.8, ease: 'power3.out', stagger: 0.07, clearProps: 'transform,opacity' });
  }

  async function init() {
    watchNav();
    mountScene();
    if (page === 'home') {
      mountModel('stack3d', 'stack-list', 'mountStack', 1);
      mountModel('disk3d', 'disk-list', 'mountDisk', 3);
      mountSolo('logo3d', 'mountLogo', { onFlip: () => scene && scene.pulse() });
      mountSolo('net3d', 'mountGlobe', {});
    }
    if (reduce) return;
    root.classList.add('motion');
    cursor();
    wipe();
    scramble();
    spotlight();
    followPointer();
    femFx();
    shell();
    const marketing = page === 'home' || page === 'download';
    try {
      await load('/vendor/gsap.min.js');
      const gsap = window.gsap;
      if (marketing) {
        await Promise.all([load('/vendor/ScrollTrigger.min.js'), load('/vendor/SplitText.min.js'), load('/vendor/lenis.min.js'), load('/vendor/tilt.min.js')]);
        gsap.registerPlugin(window.ScrollTrigger, window.SplitText);
        smoothScroll(gsap, window.ScrollTrigger);
        tilt();
      }
      if (page === 'home') {
        heroTitle(gsap, window.SplitText);
        headings(gsap, window.SplitText);
        counters(gsap, window.ScrollTrigger);
        desktopParallax();
        scrollStory(gsap, window.ScrollTrigger);
        megaSkew(gsap);
      } else if (page !== 'admin') {
        panels(gsap);
      }
      magnetic();
      if (marketing) window.ScrollTrigger.refresh();
    } catch (err) {
      console.warn('motion:', err.message);
    }
  }

  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', () => requestAnimationFrame(init));
  else requestAnimationFrame(init);
})();
