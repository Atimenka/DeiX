/* Движение сайта: плавная прокрутка, анимации при прокрутке, 3D-сцена героя, наклон карточек, конфетти.
   Все библиотеки лежат в /vendor и подгружаются только там, где нужны. Без JS и при
   prefers-reduced-motion сайт остаётся полностью рабочим. */
(function () {
  'use strict';

  const root = document.documentElement;
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
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
    return [css.getPropertyValue('--accent').trim() || '#8b5cf6', css.getPropertyValue('--accent-2').trim() || '#22d3ee', '#ec4899', '#fbbf24'];
  }

  let confettiFire = null;

  /** Праздничный всплеск после удачного действия (регистрация, заявка, привязка). Без Web Worker: CSP его запрещает. */
  window.deixCelebrate = function celebrate() {
    if (reduce) return;
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
        return gsap.from(self.lines, { yPercent: 112, duration: 1.1, ease: 'expo.out', stagger: 0.12, delay: 0.1 });
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

  function hero3d() {
    const hero = document.querySelector('.hero');
    if (!hero || window.innerWidth < 760 || !webglOk()) return;
    const start = () =>
      load('/vendor/three-hero.js')
        .then(() => {
          const scene = window.DeixHero.mount(hero, {
            onSlow: () => root.classList.remove('has-gl'),
          });
          root.classList.add('has-gl');
          return scene;
        })
        .catch(() => root.classList.remove('has-gl'));
    if ('requestIdleCallback' in window) window.requestIdleCallback(start, { timeout: 1800 });
    else setTimeout(start, 400);
  }

  function panels(gsap) {
    const targets = document.querySelectorAll('.auth-card:not(.reveal), .page-head:not(.reveal), .panel:not(.reveal):not(.rel-panel)');
    if (!targets.length) return;
    gsap.from(targets, { y: 22, opacity: 0, duration: 0.8, ease: 'power3.out', stagger: 0.07, clearProps: 'transform,opacity' });
  }

  async function init() {
    watchNav();
    if (reduce) return;
    root.classList.add('motion');
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
        hero3d();
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
