# Сторонние библиотеки сайта

Все файлы лежат в репозитории: CSP сайта запрещает внешние скрипты, стили и шрифты, а шага сборки у проекта нет.

| Файл | Библиотека | Версия | Лицензия |
|---|---|---|---|
| `gsap.min.js`, `ScrollTrigger.min.js`, `SplitText.min.js` | GSAP | 3.15.0 | GSAP Standard License (https://gsap.com/standard-license): бесплатна, в том числе для коммерческого использования; нельзя использовать, чтобы сделать конкурирующий визуальный конструктор анимаций |
| `lenis.min.js` | Lenis | 1.3.27 | MIT |
| `tilt.min.js` | vanilla-tilt | 1.8.1 | MIT |
| `confetti.min.js` | canvas-confetti | 1.9.4 | ISC |
| `three-scene.js` | three.js (r186, с tree-shaking) + сцена `web/vendor-src/hero3d.src.js` | 0.186.1 | MIT |
| `fonts/inter-*.woff2` | Inter Variable (@fontsource-variable/inter) | — | SIL OFL 1.1 |
| `fonts/jbmono-*.woff2` | JetBrains Mono Variable (@fontsource-variable/jetbrains-mono) | — | SIL OFL 1.1 |

`three-scene.js` собирается из исходников командой `web/vendor-src/build.sh` (нужны npm-пакеты `three` и `esbuild` во временной папке; они не входят в `web/package.json`).

Авторские права: GSAP © GreenSock; Lenis © darkroom.engineering; vanilla-tilt © Șandor Sergiu; canvas-confetti © Kiril Vatev; three.js © 2010–2026 three.js authors; Inter © The Inter Project Authors; JetBrains Mono © The JetBrains Mono Project Authors. Тексты лицензий MIT/ISC разрешают распространение при сохранении этого уведомления.
