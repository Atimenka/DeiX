import {
  AdditiveBlending,
  BoxGeometry,
  BufferGeometry,
  Color,
  EdgesGeometry,
  ExtrudeGeometry,
  Float32BufferAttribute,
  Group,
  IcosahedronGeometry,
  Line,
  LineBasicMaterial,
  LineLoop,
  LineSegments,
  Mesh,
  MeshBasicMaterial,
  Path,
  NormalBlending,
  PerspectiveCamera,
  Points,
  PointsMaterial,
  QuadraticBezierCurve3,
  Raycaster,
  Scene,
  ShaderMaterial,
  Shape,
  SphereGeometry,
  Vector2,
  Vector3,
  WebGLRenderer,
} from 'three';

const VERT = `
uniform float uTime;
uniform vec2 uPointer;
uniform float uSize;
uniform float uPx;
uniform float uBurst;
uniform float uScan;
varying float vMix;
varying float vGlow;
varying float vScan;
float hash(vec3 p){ p = fract(p * 0.3183099 + 0.1); p *= 17.0; return fract(p.x * p.y * p.z * (p.x + p.y + p.z)); }
float noise(vec3 x){
  vec3 i = floor(x); vec3 f = fract(x); f = f * f * (3.0 - 2.0 * f);
  return mix(mix(mix(hash(i), hash(i + vec3(1,0,0)), f.x), mix(hash(i + vec3(0,1,0)), hash(i + vec3(1,1,0)), f.x), f.y),
             mix(mix(hash(i + vec3(0,0,1)), hash(i + vec3(1,0,1)), f.x), mix(hash(i + vec3(0,1,1)), hash(i + vec3(1,1,1)), f.x), f.y), f.z);
}
void main(){
  vec3 dir = normalize(position);
  float n = noise(dir * 1.9 + uTime * 0.22);
  float wave = sin(dir.y * 9.0 + uTime * 1.4) * 0.5 + 0.5;
  float amp = 0.7 + uBurst * 0.9;
  vec3 p = dir * (1.55 + (n - 0.5) * amp + wave * 0.05 * (1.0 + uBurst));
  vec4 mv = modelViewMatrix * vec4(p, 1.0);
  vec4 clip = projectionMatrix * mv;
  float push = smoothstep(0.38, 0.0, distance(clip.xy / clip.w, uPointer));
  mv.xyz += normalize(mat3(modelViewMatrix) * dir) * push * 0.46;
  gl_Position = projectionMatrix * mv;
  vScan = smoothstep(0.26, 0.0, abs(p.y - uScan));
  gl_PointSize = uSize * uPx * (1.0 + push * 1.9 + vScan * 1.1) * (7.0 / -mv.z);
  vMix = n;
  vGlow = push;
}`;

const FRAG = `
uniform vec3 uA;
uniform vec3 uB;
uniform vec3 uC;
uniform float uHue;
uniform float uAlpha;
varying float vMix;
varying float vGlow;
varying float vScan;
void main(){
  float r = length(gl_PointCoord - 0.5);
  if (r > 0.5) discard;
  float soft = smoothstep(0.5, 0.02, r);
  vec3 col = mix(mix(uB, uA, smoothstep(0.35, 0.65, vMix)), uC, uHue);
  col = mix(col, uA, vScan) + vGlow * 0.4;
  gl_FragColor = vec4(col, soft * uAlpha * (0.5 + vGlow * 0.8 + vScan * 0.9));
}`;

const DUST_VERT = `
uniform float uPx;
uniform float uTime;
attribute float aSeed;
varying float vTw;
void main(){
  vec4 mv = modelViewMatrix * vec4(position, 1.0);
  gl_Position = projectionMatrix * mv;
  vTw = 0.55 + 0.45 * sin(uTime * (0.6 + aSeed) + aSeed * 40.0);
  gl_PointSize = (1.2 + aSeed * 2.2) * uPx * (12.0 / -mv.z);
}`;

const DUST_FRAG = `
uniform vec3 uColor;
uniform float uAlpha;
varying float vTw;
void main(){
  float r = length(gl_PointCoord - 0.5);
  if (r > 0.5) discard;
  gl_FragColor = vec4(uColor, smoothstep(0.5, 0.0, r) * uAlpha * vTw);
}`;

const WARP_VERT = `
uniform float uTravel;
uniform float uStreak;
attribute float aEnd;
varying float vA;
void main(){
  float z = mod(position.z + uTravel + 40.0, 46.0) - 40.0;
  z -= aEnd * uStreak;
  vec4 mv = modelViewMatrix * vec4(position.x, position.y, z, 1.0);
  gl_Position = projectionMatrix * mv;
  vA = (1.0 - aEnd * 0.85) * smoothstep(-40.0, -22.0, z) * smoothstep(5.5, 1.5, z);
}`;

const WARP_FRAG = `
uniform vec3 uColor;
uniform float uAlpha;
varying float vA;
void main(){ gl_FragColor = vec4(uColor, vA * uAlpha); }`;

function warpGeometry(count) {
  const pos = new Float32Array(count * 6);
  const end = new Float32Array(count * 2);
  for (let i = 0; i < count; i += 1) {
    const x = (Math.random() * 2 - 1) * 15;
    const y = (Math.random() * 2 - 1) * 9;
    const z = -40 + Math.random() * 46;
    pos.set([x, y, z, x, y, z], i * 6);
    end[i * 2] = 0;
    end[i * 2 + 1] = 1;
  }
  const geo = new BufferGeometry();
  geo.setAttribute('position', new Float32BufferAttribute(pos, 3));
  geo.setAttribute('aEnd', new Float32BufferAttribute(end, 1));
  return geo;
}

const COMET_VERT = `
uniform float uPx;
attribute float aI;
varying float vA;
void main(){
  vec4 mv = modelViewMatrix * vec4(position, 1.0);
  gl_Position = projectionMatrix * mv;
  vA = 1.0 - aI;
  gl_PointSize = (1.5 + vA * vA * 26.0) * uPx * (7.0 / -mv.z);
}`;

const COMET_FRAG = `
uniform vec3 uColor;
uniform float uAlpha;
varying float vA;
void main(){
  float r = length(gl_PointCoord - 0.5);
  if (r > 0.5) discard;
  gl_FragColor = vec4(uColor, smoothstep(0.5, 0.0, r) * uAlpha * vA * vA);
}`;

function circle(radius, segments) {
  const pos = [];
  for (let i = 0; i < segments; i += 1) {
    const a = (i / segments) * Math.PI * 2;
    pos.push(Math.cos(a) * radius, 0, Math.sin(a) * radius);
  }
  const geo = new BufferGeometry();
  geo.setAttribute('position', new Float32BufferAttribute(pos, 3));
  return geo;
}

function dustGeometry(count) {
  const pos = new Float32Array(count * 3);
  const seed = new Float32Array(count);
  for (let i = 0; i < count; i += 1) {
    const r = 9 + Math.random() * 26;
    const t = Math.random() * Math.PI * 2;
    const u = Math.random() * 2 - 1;
    const s = Math.sqrt(1 - u * u);
    pos[i * 3] = r * s * Math.cos(t);
    pos[i * 3 + 1] = r * u * 0.8;
    pos[i * 3 + 2] = r * s * Math.sin(t) - 6;
    seed[i] = Math.random();
  }
  const geo = new BufferGeometry();
  geo.setAttribute('position', new Float32BufferAttribute(pos, 3));
  geo.setAttribute('aSeed', new Float32BufferAttribute(seed, 1));
  return geo;
}

function cssColor(name, fallback) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  try {
    return new Color(value || fallback);
  } catch {
    return new Color(fallback);
  }
}

const smooth = (a, b, x) => {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
};

/* Ключевые точки пути сферы по странице: [прогресс, x (доля половины ширины), масштаб, прозрачность] */
const PATH = [
  [0, 0.52, 1.2, 1],
  [0.2, -0.62, 0.7, 0.42],
  [0.45, -0.62, 0.78, 0.4],
  [0.7, 0.62, 0.72, 0.4],
  [0.9, 0.0, 1.0, 0.5],
  [1, 0.0, 1.3, 0.6],
];

function samplePath(p) {
  for (let i = 0; i < PATH.length - 1; i += 1) {
    const a = PATH[i];
    const b = PATH[i + 1];
    if (p <= b[0]) {
      const t = smooth(a[0], b[0], p);
      return [a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t];
    }
  }
  const last = PATH[PATH.length - 1];
  return [last[1], last[2], last[3]];
}

/**
 * Фиксированная 3D-сцена на всю страницу.
 * options: still (один кадр, без анимации), dim (приглушённая, для страниц с формами),
 * lite (меньше точек), onSlow (вызывается, если устройство не тянет).
 */
export function mount(options = {}) {
  const still = Boolean(options.still);
  const dim = Boolean(options.dim);
  const canvas = document.createElement('canvas');
  canvas.className = 'scene-gl';
  canvas.setAttribute('aria-hidden', 'true');
  const renderer = new WebGLRenderer({ canvas, alpha: true, antialias: false, powerPreference: 'default' });
  renderer.setClearColor(0x000000, 0);
  const bg = document.querySelector('.bg');
  if (bg && bg.parentNode) bg.after(canvas);
  else document.body.prepend(canvas);

  const scene = new Scene();
  const camera = new PerspectiveCamera(45, 1, 0.1, 80);
  camera.position.z = 7;
  const group = new Group();
  scene.add(group);

  const uniforms = {
    uTime: { value: 0 },
    uPointer: { value: new Vector2(9, 9) },
    uSize: { value: 7.2 },
    uPx: { value: 1 },
    uBurst: { value: 0 },
    uScan: { value: 0 },
    uHue: { value: 0 },
    uA: { value: new Color() },
    uB: { value: new Color() },
    uC: { value: new Color() },
    uAlpha: { value: 1 },
  };
  const material = new ShaderMaterial({ uniforms, vertexShader: VERT, fragmentShader: FRAG, transparent: true, depthWrite: false });
  const cloud = new Points(new IcosahedronGeometry(1, options.lite ? 8 : 12), material);
  group.add(cloud);

  const coreMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.5, depthWrite: false });
  const core = new LineSegments(new EdgesGeometry(new IcosahedronGeometry(0.92, 1)), coreMaterial);
  group.add(core);

  const ringMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.35, depthWrite: false });
  const ringA = new LineLoop(circle(2.35, 220), ringMaterial);
  ringA.rotation.set(1.15, 0.2, 0.3);
  const ringB = new LineLoop(circle(2.9, 260), ringMaterial);
  ringB.rotation.set(0.45, 0, -0.5);
  group.add(ringA, ringB);

  const scanMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.8, depthWrite: false });
  const scanRing = new LineLoop(circle(1, 120), scanMaterial);
  group.add(scanRing);

  const dustUniforms = { uPx: { value: 1 }, uTime: { value: 0 }, uColor: { value: new Color() }, uAlpha: { value: 0.8 } };
  const dustMaterial = new ShaderMaterial({ uniforms: dustUniforms, vertexShader: DUST_VERT, fragmentShader: DUST_FRAG, transparent: true, depthWrite: false });
  const dust = new Points(dustGeometry(options.lite ? 700 : 1700), dustMaterial);
  scene.add(dust);

  const warpUniforms = { uTravel: { value: 0 }, uStreak: { value: 0.05 }, uColor: { value: new Color() }, uAlpha: { value: 0.5 } };
  const warpMaterial = new ShaderMaterial({ uniforms: warpUniforms, vertexShader: WARP_VERT, fragmentShader: WARP_FRAG, transparent: true, depthWrite: false });
  const warp = new LineSegments(warpGeometry(options.lite ? 260 : 620), warpMaterial);
  warp.frustumCulled = false;
  scene.add(warp);

  const packetGeometry = new EdgesGeometry(new BoxGeometry(0.17, 0.17, 0.17));
  const packetMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.9, depthWrite: false });
  const packets = [];
  for (let i = 0; i < 7; i += 1) {
    const orbit = new Group();
    orbit.rotation.set(0.25 + i * 0.37, i * 0.9, i * 0.5);
    const box = new LineSegments(packetGeometry, packetMaterial);
    box.position.x = 2.2 + (i % 3) * 0.32;
    orbit.add(box);
    orbit.userData.speed = 0.25 + (i % 4) * 0.11;
    group.add(orbit);
    packets.push({ orbit, box });
  }

  const shockMaterial = new LineBasicMaterial({ transparent: true, opacity: 0, depthWrite: false });
  const shockRing = new LineLoop(circle(1, 160), shockMaterial);
  shockRing.rotation.x = 1.15;
  group.add(shockRing);

  const TRAIL = 56;
  const cometPos = new Float32Array(TRAIL * 3);
  const cometIdx = new Float32Array(TRAIL);
  for (let i = 0; i < TRAIL; i += 1) cometIdx[i] = i / TRAIL;
  const cometGeometry = new BufferGeometry();
  const cometAttr = new Float32BufferAttribute(cometPos, 3);
  cometGeometry.setAttribute('position', cometAttr);
  cometGeometry.setAttribute('aI', new Float32BufferAttribute(cometIdx, 1));
  const cometUniforms = { uPx: { value: 1 }, uColor: { value: new Color() }, uAlpha: { value: 0 } };
  const cometMaterial = new ShaderMaterial({ uniforms: cometUniforms, vertexShader: COMET_VERT, fragmentShader: COMET_FRAG, transparent: true, depthWrite: false });
  const comet = new Points(cometGeometry, cometMaterial);
  comet.frustumCulled = false;
  scene.add(comet);

  let light = false;
  function applyTheme() {
    light = document.documentElement.getAttribute('data-theme') === 'light';
    uniforms.uA.value.copy(cssColor('--accent', '#ff3b30'));
    cometUniforms.uColor.value.copy(cssColor('--accent', '#ff3b30'));
    cometMaterial.blending = light ? NormalBlending : AdditiveBlending;
    cometMaterial.needsUpdate = true;
    warpUniforms.uColor.value.copy(light ? cssColor('--accent', '#ff3b30') : cssColor('--accent-2', '#f3f2ee'));
    warpMaterial.blending = light ? NormalBlending : AdditiveBlending;
    warpMaterial.needsUpdate = true;
    packetMaterial.color.copy(cssColor('--accent', '#ff3b30'));
    shockMaterial.color.copy(cssColor('--accent', '#ff3b30'));
    scanMaterial.color.copy(cssColor('--accent', '#ff3b30'));
    uniforms.uB.value.copy(cssColor('--accent-2', '#f3f2ee'));
    uniforms.uC.value.set(light ? '#ff3b30' : '#ff7a3c');
    dustUniforms.uColor.value.copy(light ? uniforms.uA.value : uniforms.uB.value);
    material.blending = light ? NormalBlending : AdditiveBlending;
    dustMaterial.blending = light ? NormalBlending : AdditiveBlending;
    coreMaterial.color.copy(uniforms.uB.value);
    ringMaterial.color.copy(uniforms.uA.value);
    material.needsUpdate = true;
    dustMaterial.needsUpdate = true;
  }
  applyTheme();
  const themeWatch = new MutationObserver(() => {
    applyTheme();
    if (still) draw(2.5);
  });
  themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });

  let width = 1;
  let height = 1;
  let halfWidth = 1;
  function resize() {
    width = Math.max(1, window.innerWidth);
    height = Math.max(1, window.innerHeight);
    const ratio = Math.min(window.devicePixelRatio || 1, options.lite ? 1.25 : 1.6);
    renderer.setPixelRatio(ratio);
    renderer.setSize(width, height, false);
    uniforms.uPx.value = ratio;
    dustUniforms.uPx.value = ratio;
    cometUniforms.uPx.value = ratio;
    camera.aspect = width / height;
    halfWidth = Math.tan((camera.fov * Math.PI) / 360) * camera.position.z * camera.aspect;
    camera.updateProjectionMatrix();
    if (still) draw(2.5);
  }
  window.addEventListener('resize', resize);
  resize();

  const pointer = new Vector2(9, 9);
  const target = new Vector2(0, 0);
  const eased = new Vector2(0, 0);
  function onMove(event) {
    target.set((event.clientX / width) * 2 - 1, -((event.clientY / height) * 2 - 1));
    pointer.copy(target);
  }
  function onLeave() {
    pointer.set(9, 9);
    target.set(0, 0);
  }
  window.addEventListener('pointermove', onMove, { passive: true });
  document.addEventListener('pointerleave', onLeave);

  let burst = 0;
  let shock = 0;
  function pulse() {
    burst = 1;
    shock = 1;
  }
  function onPress(event) {
    if (still || !(event.target instanceof Element)) return;
    if (event.target.closest('a, button, input, select, textarea, summary, label, .stack-view, .logo-stage, .sh')) return;
    pulse();
  }
  window.addEventListener('pointerdown', onPress, { passive: true });
  let lastScrollY = window.scrollY;
  let scrollSpeed = 0;
  let travel = 0;

  let progress = 0;
  let progressSmooth = 0;
  function readScroll() {
    const room = Math.max(1, document.documentElement.scrollHeight - window.innerHeight);
    progress = Math.min(1, Math.max(0, window.scrollY / room));
  }
  readScroll();
  progressSmooth = progress;
  function onScroll() {
    readScroll();
    if (still) draw(2.5);
  }
  window.addEventListener('scroll', onScroll, { passive: true });

  let raf = 0;
  let last = performance.now();
  let slow = 0;
  let frames = 0;
  let disposed = false;

  function frame(now) {
    raf = requestAnimationFrame(frame);
    if (document.hidden) {
      last = now;
      return;
    }
    const dt = Math.min(now - last, 100);
    last = now;
    frames += 1;
    if (frames > 40 && frames <= 160) {
      slow += dt > 36 ? 1 : 0;
      if (frames === 160 && slow > 70) {
        dispose();
        if (options.onSlow) options.onSlow();
        return;
      }
    }
    draw(now / 1000);
  }

  function draw(time) {
    eased.lerp(target, 0.06);
    const dy = still ? 0 : window.scrollY - lastScrollY;
    lastScrollY = window.scrollY;
    scrollSpeed += (dy - scrollSpeed) * 0.2;
    travel += scrollSpeed * 0.02 + 0.012;
    burst = Math.max(burst, Math.min(0.85, Math.abs(scrollSpeed) / 60));
    warpUniforms.uTravel.value = travel;
    warpUniforms.uStreak.value = 0.06 + Math.min(11, Math.abs(scrollSpeed) * 0.22);
    camera.position.x = eased.x * 0.55;
    camera.position.y = eased.y * 0.35;
    camera.lookAt(0, 0, 0);
    const vh = halfWidth / camera.aspect;
    const live = pointer.x < 5;
    const hx = live ? pointer.x * halfWidth : cometPos[0];
    const hy = live ? pointer.y * vh : cometPos[1];
    cometPos[0] += (hx - cometPos[0]) * 0.5;
    cometPos[1] += (hy - cometPos[1]) * 0.5;
    cometPos[2] = 0.4;
    for (let i = 1; i < TRAIL; i += 1) {
      cometPos[i * 3] += (cometPos[(i - 1) * 3] - cometPos[i * 3]) * 0.22;
      cometPos[i * 3 + 1] += (cometPos[(i - 1) * 3 + 1] - cometPos[i * 3 + 1]) * 0.22;
      cometPos[i * 3 + 2] = 0.4;
    }
    cometAttr.needsUpdate = true;
    cometUniforms.uAlpha.value += ((live && !still ? (light ? 0.9 : 1) : 0) - cometUniforms.uAlpha.value) * 0.1;
    scene.rotation.z += (Math.max(-0.14, Math.min(0.14, -scrollSpeed * 0.004)) - scene.rotation.z) * 0.1;
    progressSmooth += (progress - progressSmooth) * (still ? 1 : 0.08);
    burst *= 0.94;
    shock *= 0.968;
    uniforms.uTime.value = time;
    dustUniforms.uTime.value = time;
    uniforms.uBurst.value = burst;
    const scanY = Math.sin(time * 0.9) * 1.55;
    uniforms.uScan.value = scanY;
    scanRing.position.y = scanY;
    scanRing.scale.setScalar(Math.sqrt(Math.max(0.04, 2.6 - scanY * scanY)));
    uniforms.uPointer.value.lerp(pointer, 0.18);

    const wide = width > 900;
    const [px, scale, alpha] = samplePath(progressSmooth);
    group.position.x = wide ? halfWidth * px : 0;
    group.position.y = wide ? 0 : 0.9 * (1 - smooth(0, 0.2, progressSmooth));
    group.scale.setScalar(wide ? scale : scale * 0.78);
    group.rotation.y = time * 0.07 + eased.x * 0.5 + progressSmooth * Math.PI * 3;
    group.rotation.x = -eased.y * 0.3 + Math.sin(progressSmooth * Math.PI * 2) * 0.25;
    core.rotation.y = -time * 0.18;
    core.rotation.x = time * 0.09;
    ringA.rotation.z = 0.3 + time * 0.05;
    ringB.rotation.y = time * -0.04;
    dust.rotation.y = time * 0.01 + progressSmooth * 1.2;
    dust.position.y = progressSmooth * 3.5;

    packets.forEach((p, i) => {
      p.orbit.rotation.y += p.orbit.userData.speed * 0.012;
      p.box.rotation.set(time * 0.6 + i, time * 0.8, 0);
    });
    const shockScale = 1 + (1 - shock) * 8;
    shockRing.scale.setScalar(shockScale);
    shockMaterial.opacity = Math.min(1, shock * 1.2);
    warpUniforms.uAlpha.value = (light ? 0.5 : 0.55) * (dim ? 0.5 : 1);
    packetMaterial.opacity = (wide ? 0.9 : 0.3) * alpha;
    const base = (dim ? 0.45 : 1) * (wide ? 1 : 0.22) * alpha;
    uniforms.uAlpha.value = (light ? 0.8 : 0.95) * base;
    uniforms.uHue.value = smooth(0.35, 0.95, progressSmooth) * 0.7;
    coreMaterial.opacity = (light ? 0.5 : 0.5) * base;
    ringMaterial.opacity = (light ? 0.42 : 0.34) * base;
    dustUniforms.uAlpha.value = (light ? 0.55 : 0.85) * (dim ? 0.6 : 1);
    renderer.render(scene, camera);
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    cancelAnimationFrame(raf);
    window.removeEventListener('resize', resize);
    window.removeEventListener('scroll', onScroll);
    window.removeEventListener('pointerdown', onPress);
    window.removeEventListener('pointermove', onMove);
    document.removeEventListener('pointerleave', onLeave);
    themeWatch.disconnect();
    cloud.geometry.dispose();
    core.geometry.dispose();
    ringA.geometry.dispose();
    ringB.geometry.dispose();
    dust.geometry.dispose();
    material.dispose();
    coreMaterial.dispose();
    ringMaterial.dispose();
    scanMaterial.dispose();
    scanRing.geometry.dispose();
    dustMaterial.dispose();
    warp.geometry.dispose();
    cometGeometry.dispose();
    cometMaterial.dispose();
    warpMaterial.dispose();
    packetGeometry.dispose();
    packetMaterial.dispose();
    shockRing.geometry.dispose();
    shockMaterial.dispose();
    renderer.dispose();
    canvas.remove();
  }

  if (still) draw(2.5);
  else raf = requestAnimationFrame(frame);
  return { dispose, pulse };
}

/**
 * Общая основа интерактивных 3D-моделей на страницах: холст, камера, вращение перетаскиванием,
 * наведение на части модели, пауза вне экрана и по смене темы.
 * options: className, lite, still, fit(camera, aspect), pickables(), onHover(index), onTheme(), update(time, stage).
 */
function createStage(container, options) {
  const still = Boolean(options.still);
  const canvas = document.createElement('canvas');
  canvas.className = options.className;
  canvas.setAttribute('aria-hidden', 'true');
  const renderer = new WebGLRenderer({ canvas, alpha: true, antialias: true, powerPreference: 'default' });
  renderer.setClearColor(0x000000, 0);
  container.append(canvas);
  const scene = new Scene();
  const camera = new PerspectiveCamera(32, 1, 0.1, 60);
  const group = new Group();
  scene.add(group);

  const stage = { scene, group, camera, renderer, canvas, still, yaw: 0.7, tilt: new Vector2(), hover: -1, visible: still, light: false };
  const raycaster = new Raycaster();
  const ndc = new Vector2();
  let dragging = false;
  let lastX = 0;
  let velocity = 0;
  let raf = 0;
  let disposed = false;

  function pick(event) {
    const rect = canvas.getBoundingClientRect();
    ndc.set(((event.clientX - rect.left) / rect.width) * 2 - 1, -(((event.clientY - rect.top) / rect.height) * 2 - 1));
    stage.tilt.copy(ndc);
    raycaster.setFromCamera(ndc, camera);
    const hit = raycaster.intersectObjects(options.pickables(), false)[0];
    const index = hit ? hit.object.userData.index : -1;
    if (index !== stage.hover) {
      stage.hover = index;
      canvas.classList.toggle('hit', index >= 0);
      if (index >= 0 && options.onHover) options.onHover(index);
    }
  }
  function onDown(event) {
    dragging = true;
    lastX = event.clientX;
    canvas.classList.add('drag');
    canvas.setPointerCapture(event.pointerId);
  }
  function onMove(event) {
    if (dragging) {
      velocity = (event.clientX - lastX) * 0.011;
      stage.yaw += velocity;
      lastX = event.clientX;
      request();
    } else pick(event);
  }
  function onUp(event) {
    dragging = false;
    canvas.classList.remove('drag');
    if (canvas.hasPointerCapture(event.pointerId)) canvas.releasePointerCapture(event.pointerId);
  }
  canvas.addEventListener('pointerdown', onDown);
  canvas.addEventListener('pointermove', onMove);
  canvas.addEventListener('pointerup', onUp);
  canvas.addEventListener('pointercancel', onUp);

  function draw(time) {
    if (!dragging) {
      velocity *= 0.94;
      stage.yaw += velocity + (still || options.spring ? 0 : 0.0035);
      if (options.spring) stage.yaw -= stage.yaw * 0.03;
    }
    options.update(time, stage);
    group.rotation.y = stage.yaw + (stage.yawOffset || 0);
    renderer.render(scene, camera);
  }
  function loop(now) {
    raf = requestAnimationFrame(loop);
    if (!document.hidden) draw(now / 1000);
  }
  function request() {
    if (still && !disposed) draw(0);
  }
  stage.request = request;

  function applyTheme() {
    stage.light = document.documentElement.getAttribute('data-theme') === 'light';
    options.onTheme(stage);
    request();
  }
  const themeWatch = new MutationObserver(applyTheme);
  themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });

  function resize() {
    const width = Math.max(1, container.clientWidth);
    const height = Math.max(1, container.clientHeight);
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, options.lite ? 1.25 : 2));
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    options.fit(camera, camera.aspect);
    camera.updateProjectionMatrix();
    request();
  }
  const sizeWatch = new ResizeObserver(resize);
  sizeWatch.observe(container);

  const viewWatch = new IntersectionObserver((entries) => {
    stage.visible = entries[0].isIntersecting;
    if (still) return;
    if (stage.visible && !raf) raf = requestAnimationFrame(loop);
    else if (!stage.visible && raf) {
      cancelAnimationFrame(raf);
      raf = 0;
    }
  });
  viewWatch.observe(container);

  stage.dispose = (free) => {
    if (disposed) return;
    disposed = true;
    cancelAnimationFrame(raf);
    viewWatch.disconnect();
    sizeWatch.disconnect();
    themeWatch.disconnect();
    free();
    renderer.dispose();
    canvas.remove();
  };
  applyTheme();
  resize();
  return stage;
}

/* Слои системы снизу вверх: ширина, глубина, высота плиты */
const STACK = [
  [2.1, 1.5, 0.2],
  [2.5, 1.8, 0.3],
  [3.1, 2.1, 0.26],
  [3.1, 2.1, 0.26],
  [2.7, 1.9, 0.22],
  [3.3, 2.2, 0.12],
];

/** Градиент слоёв: от нейтрального (accent-2) к акценту. */
function ramp(t) {
  return cssColor('--accent-2', '#f3f2ee').lerp(cssColor('--accent', '#ff3b30'), t);
}

/**
 * Разборный стек DeiX: тянуть — вращать, наведение подсвечивает слой.
 * options: still, lite, initial, onHover(index). Возвращает { setActive, dispose }.
 */
export function mountStack(container, options = {}) {
  let active = options.initial ?? -1;
  let explode = options.still ? 1 : 0;
  const slabs = STACK.map(([w, d, h], index) => {
    const geometry = new BoxGeometry(w, h, d);
    const fill = new MeshBasicMaterial({ transparent: true, opacity: 0.16, depthWrite: false });
    const edge = new LineBasicMaterial({ transparent: true, opacity: 0.8 });
    const mesh = new Mesh(geometry, fill);
    const edgeGeometry = new EdgesGeometry(geometry);
    mesh.add(new LineSegments(edgeGeometry, edge));
    mesh.userData.index = index;
    return { mesh, fill, edge, geometry, edgeGeometry, glow: 0 };
  });
  const railPos = [];
  for (const [x, z] of [[-0.85, -0.55], [0.85, -0.55], [0.85, 0.55], [-0.85, 0.55]]) railPos.push(x, -1, z, x, 1, z);
  const railGeometry = new BufferGeometry();
  railGeometry.setAttribute('position', new Float32BufferAttribute(railPos, 3));
  const railMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.3 });
  const rails = new LineSegments(railGeometry, railMaterial);

  const stage = createStage(container, {
    className: 'stack-gl',
    lite: options.lite,
    still: options.still,
    fit: (camera, aspect) => camera.position.set(0, 0, 10 * Math.max(1, 4.9 / (5.73 * aspect))),
    pickables: () => slabs.map((slab) => slab.mesh),
    onHover: options.onHover,
    onTheme: () => {
      slabs.forEach((slab, i) => {
        const color = ramp(i / (STACK.length - 1));
        slab.fill.color.copy(color);
        slab.edge.color.copy(color);
      });
      railMaterial.color.copy(cssColor('--accent', '#ff3b30'));
    },
    update: (time, st) => {
      explode += ((st.visible || st.still ? 1 : 0) - explode) * (st.still ? 1 : 0.05);
      const gap = 0.3 + explode * 0.58;
      slabs.forEach((slab, i) => {
        slab.glow += ((i === active || i === st.hover ? 1 : 0) - slab.glow) * (st.still ? 1 : 0.14);
        slab.mesh.position.set(slab.glow * 0.45, (i - (STACK.length - 1) / 2) * gap, 0);
        slab.mesh.scale.setScalar(1 + slab.glow * 0.04);
        slab.fill.opacity = (st.light ? 0.2 : 0.12) + slab.glow * (st.light ? 0.4 : 0.34);
        slab.edge.opacity = 0.55 + slab.glow * 0.45;
      });
      rails.scale.y = ((STACK.length - 1) / 2) * gap + 0.1;
      st.group.rotation.x = 0.5 - st.tilt.y * 0.12;
      st.group.position.y = Math.sin(time * 0.8) * (st.still ? 0 : 0.06);
    },
  });
  stage.group.add(rails, ...slabs.map((slab) => slab.mesh));
  stage.request();
  return {
    setActive(index) {
      active = index;
      stage.request();
    },
    dispose: () =>
      stage.dispose(() => {
        slabs.forEach((slab) => {
          slab.geometry.dispose();
          slab.edgeGeometry.dispose();
          slab.fill.dispose();
          slab.edge.dispose();
        });
        railGeometry.dispose();
        railMaterial.dispose();
      }),
  };
}

/* Сектора диска: угол в градусах. Схема раскладки, не масштаб. */
const DISK = [28, 40, 22, 130, 120];
const DISK_GAP = 4;

function sector(r0, r1, a0, a1) {
  const shape = new Shape();
  const steps = Math.max(6, Math.round(Math.abs(a1 - a0) / 3));
  for (let i = 0; i <= steps; i += 1) {
    const a = a0 + ((a1 - a0) * i) / steps;
    const x = Math.cos(a) * r1;
    const y = Math.sin(a) * r1;
    if (i === 0) shape.moveTo(x, y);
    else shape.lineTo(x, y);
  }
  for (let i = steps; i >= 0; i -= 1) {
    const a = a0 + ((a1 - a0) * i) / steps;
    shape.lineTo(Math.cos(a) * r0, Math.sin(a) * r0);
  }
  shape.closePath();
  return new ExtrudeGeometry(shape, { depth: 0.2, bevelEnabled: false });
}

/**
 * Диск DeiX: области раскладки секторами, «головка» чтения наводится на выбранную область.
 * options: still, lite, initial, onHover(index). Возвращает { setActive, dispose }.
 */
export function mountDisk(container, options = {}) {
  let active = options.initial ?? -1;
  let head = 0;
  let cursor = 0;
  const mids = [];
  const parts = DISK.map((deg, index) => {
    const a0 = (cursor * Math.PI) / 180;
    cursor += deg;
    const a1 = (cursor * Math.PI) / 180;
    cursor += DISK_GAP;
    mids.push((a0 + a1) / 2);
    const geometry = sector(1.1, 2.5, a0, a1);
    const fill = new MeshBasicMaterial({ transparent: true, opacity: 0.2, depthWrite: false });
    const edge = new LineBasicMaterial({ transparent: true, opacity: 0.8 });
    const mesh = new Mesh(geometry, fill);
    mesh.rotation.x = -Math.PI / 2;
    const edgeGeometry = new EdgesGeometry(geometry, 20);
    mesh.add(new LineSegments(edgeGeometry, edge));
    mesh.userData.index = index;
    return { mesh, fill, edge, geometry, edgeGeometry, glow: 0 };
  });
  const hubGeometry = circle(0.55, 64);
  const hubMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.7 });
  const hub = new LineLoop(hubGeometry, hubMaterial);
  const armGeometry = new BufferGeometry();
  armGeometry.setAttribute('position', new Float32BufferAttribute([0.6, 0.35, 0, 2.85, 0.35, 0], 3));
  const armMaterial = new LineBasicMaterial({ transparent: true, opacity: 1 });
  const arm = new LineSegments(armGeometry, armMaterial);
  const tipGeometry = new BoxGeometry(0.16, 0.16, 0.16);
  const tipMaterial = new MeshBasicMaterial();
  const tip = new Mesh(tipGeometry, tipMaterial);
  tip.position.set(2.85, 0.35, 0);
  const headGroup = new Group();
  headGroup.add(arm, tip);

  const stage = createStage(container, {
    className: 'stack-gl',
    lite: options.lite,
    still: options.still,
    fit: (camera, aspect) => camera.position.set(0, 0, 11 * Math.max(1, 5.6 / (6.3 * aspect))),
    pickables: () => parts.map((part) => part.mesh),
    onHover: options.onHover,
    onTheme: () => {
      parts.forEach((part, i) => {
        const color = ramp(i / (DISK.length - 1));
        part.fill.color.copy(color);
        part.edge.color.copy(color);
      });
      const accent = cssColor('--accent', '#ff3b30');
      hubMaterial.color.copy(accent);
      armMaterial.color.copy(accent);
      tipMaterial.color.copy(accent);
    },
    update: (time, st) => {
      const target = active >= 0 ? mids[active] : mids[0];
      let delta = target - head;
      while (delta > Math.PI) delta -= Math.PI * 2;
      while (delta < -Math.PI) delta += Math.PI * 2;
      head += delta * (st.still ? 1 : 0.08);
      headGroup.rotation.y = -head;
      parts.forEach((part, i) => {
        part.glow += ((i === active || i === st.hover ? 1 : 0) - part.glow) * (st.still ? 1 : 0.14);
        part.mesh.position.y = part.glow * 0.3;
        part.fill.opacity = (st.light ? 0.2 : 0.12) + part.glow * (st.light ? 0.4 : 0.34);
        part.edge.opacity = 0.55 + part.glow * 0.45;
      });
      headGroup.position.y = 0.2 + parts[Math.max(active, 0)].glow * 0.3;
      st.group.rotation.x = 0.85 - st.tilt.y * 0.1;
      st.group.position.y = Math.sin(time * 0.7) * (st.still ? 0 : 0.05);
    },
  });
  stage.group.add(hub, headGroup, ...parts.map((part) => part.mesh));
  stage.request();
  return {
    setActive(index) {
      active = index;
      stage.request();
    },
    dispose: () =>
      stage.dispose(() => {
        parts.forEach((part) => {
          part.geometry.dispose();
          part.edgeGeometry.dispose();
          part.fill.dispose();
          part.edge.dispose();
        });
        hubGeometry.dispose();
        hubMaterial.dispose();
        armGeometry.dispose();
        armMaterial.dispose();
        tipGeometry.dispose();
        tipMaterial.dispose();
      }),
  };
}


const LOGO_VERT = `
varying vec3 vN;
varying vec3 vW;
void main(){
  vN = normalize(normalMatrix * normal);
  vec4 w = modelMatrix * vec4(position, 1.0);
  vW = w.xyz;
  gl_Position = projectionMatrix * viewMatrix * w;
}`;

const LOGO_FRAG = `
uniform vec3 uBase;
uniform vec3 uAccent;
uniform float uScan;
uniform float uGlow;
varying vec3 vN;
varying vec3 vW;
void main(){
  float l = dot(normalize(vN), normalize(vec3(0.35, 0.7, 0.62))) * 0.5 + 0.5;
  float band = smoothstep(0.55, 0.0, abs(vW.x - uScan));
  vec3 c = uBase * (0.65 + 0.75 * l) + uAccent * (band * 0.85 + uGlow * 0.5);
  gl_FragColor = vec4(c, 1.0);
}`;

function logoShape(letter) {
  const shape = new Shape();
  if (letter === 'I') {
    shape.moveTo(0, 0);
    shape.lineTo(0.24, 0);
    shape.lineTo(0.24, 1);
    shape.lineTo(0, 1);
    return { shape, width: 0.24 };
  }
  if (letter === 'E') {
    [[0, 0], [0.72, 0], [0.72, 0.2], [0.24, 0.2], [0.24, 0.4], [0.62, 0.4], [0.62, 0.6], [0.24, 0.6], [0.24, 0.8], [0.72, 0.8], [0.72, 1], [0, 1]].forEach(([x, y], i) => (i ? shape.lineTo(x, y) : shape.moveTo(x, y)));
    return { shape, width: 0.72 };
  }
  if (letter === 'X') {
    [[0, 0], [0.26, 0], [0.46, 0.38], [0.66, 0], [0.92, 0], [0.6, 0.5], [0.92, 1], [0.66, 1], [0.46, 0.62], [0.26, 1], [0, 1], [0.32, 0.5]].forEach(([x, y], i) => (i ? shape.lineTo(x, y) : shape.moveTo(x, y)));
    return { shape, width: 0.92 };
  }
  shape.moveTo(0, 0);
  shape.lineTo(0.4, 0);
  shape.absarc(0.4, 0.5, 0.5, -Math.PI / 2, Math.PI / 2, false);
  shape.lineTo(0, 1);
  shape.closePath();
  const hole = new Path();
  hole.moveTo(0.24, 0.2);
  hole.lineTo(0.4, 0.2);
  hole.absarc(0.4, 0.5, 0.3, -Math.PI / 2, Math.PI / 2, false);
  hole.lineTo(0.24, 0.8);
  hole.closePath();
  shape.holes.push(hole);
  return { shape, width: 0.9 };
}

/**
 * Объёмные буквы DEIX: свет пробегает по граням, буква под курсором выезжает вперёд,
 * клик переворачивает её. options: still, lite, onFlip().
 */
export function mountLogo(container, options = {}) {
  const GAP = 0.32;
  const defs = Array.from('DEIX').map(logoShape);
  const total = defs.reduce((sum, d) => sum + d.width, 0) + GAP * (defs.length - 1);
  let cursor = -total / 2;
  const parts = defs.map((def, index) => {
    const geometry = new ExtrudeGeometry(def.shape, { depth: 0.36, bevelEnabled: true, bevelThickness: 0.03, bevelSize: 0.025, bevelSegments: 1, curveSegments: 16 });
    geometry.translate(-def.width / 2, -0.5, -0.18);
    const material = new ShaderMaterial({
      vertexShader: LOGO_VERT,
      fragmentShader: LOGO_FRAG,
      uniforms: { uBase: { value: new Color() }, uAccent: { value: new Color() }, uScan: { value: 0 }, uGlow: { value: 0 } },
    });
    const mesh = new Mesh(geometry, material);
    mesh.userData.index = index;
    const edgeGeometry = new EdgesGeometry(geometry, 28);
    const edgeMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.95 });
    mesh.add(new LineSegments(edgeGeometry, edgeMaterial));
    const holder = new Group();
    holder.position.x = cursor + def.width / 2;
    cursor += def.width + GAP;
    holder.add(mesh);
    return { holder, mesh, geometry, material, edgeGeometry, edgeMaterial, glow: 0, spin: 1 };
  });

  const stage = createStage(container, {
    className: 'stack-gl',
    lite: options.lite,
    still: options.still,
    spring: true,
    fit: (camera, aspect) => {
      camera.position.set(0, 0, Math.max(3.6, 8.6 / aspect));
      camera.lookAt(0, 0, 0);
    },
    pickables: () => parts.map((part) => part.mesh),
    onTheme: (st) => {
      const accent = cssColor('--accent', '#ff3b30');
      parts.forEach((part) => {
        part.material.uniforms.uBase.value.set(st.light ? '#d9d6cf' : '#3a3a40');
        part.material.uniforms.uAccent.value.copy(accent);
        part.edgeMaterial.color.copy(st.light ? cssColor('--text', '#0c0c0c') : accent);
      });
    },
    update: (time, st) => {
      const scan = st.still ? 0.2 : Math.sin(time * 0.9) * (total / 2 + 0.6);
      st.yawOffset = st.still ? 0.28 : Math.sin(time * 0.45) * 0.3 + st.tilt.x * 0.25;
      st.group.rotation.x = -st.tilt.y * 0.2 + 0.08;
      parts.forEach((part, i) => {
        part.glow += ((st.hover === i ? 1 : 0) - part.glow) * (st.still ? 1 : 0.16);
        part.holder.position.z = part.glow * 0.55;
        part.holder.position.y = st.still ? 0 : Math.sin(time * 1.1 + i * 0.9) * 0.05;
        if (part.spin < 1) part.spin = Math.min(1, part.spin + 0.022);
        const e = 1 - (1 - part.spin) ** 3;
        part.holder.rotation.y = e * Math.PI * 2;
        part.material.uniforms.uScan.value = scan;
        part.material.uniforms.uGlow.value = part.glow;
      });
    },
  });
  stage.yaw = 0;
  stage.group.add(...parts.map((part) => part.holder));
  const onClick = () => {
    const part = parts[stage.hover];
    if (!part || part.spin < 1) return;
    part.spin = 0;
    if (options.onFlip) options.onFlip();
  };
  stage.canvas.addEventListener('click', onClick);
  stage.request();
  return {
    setActive() {},
    dispose: () =>
      stage.dispose(() => {
        stage.canvas.removeEventListener('click', onClick);
        parts.forEach((part) => {
          part.geometry.dispose();
          part.edgeGeometry.dispose();
          part.material.dispose();
          part.edgeMaterial.dispose();
        });
      }),
  };
}


function fibonacciSphere(count, radius) {
  const pos = new Float32Array(count * 3);
  const golden = Math.PI * (3 - Math.sqrt(5));
  for (let i = 0; i < count; i += 1) {
    const y = 1 - (i / (count - 1)) * 2;
    const r = Math.sqrt(1 - y * y);
    const a = golden * i;
    pos.set([Math.cos(a) * r * radius, y * radius, Math.sin(a) * r * radius], i * 3);
  }
  return pos;
}

/**
 * Иллюстрация сети: точечный глобус, узлы и дуги, по которым бегут пакеты.
 * options: still, lite.
 */
export function mountGlobe(container, options = {}) {
  const dotsGeometry = new BufferGeometry();
  dotsGeometry.setAttribute('position', new Float32BufferAttribute(fibonacciSphere(options.lite ? 1400 : 2600, 1), 3));
  const dotsMaterial = new PointsMaterial({ size: 0.022, transparent: true, opacity: 0.8, depthWrite: false });
  const dots = new Points(dotsGeometry, dotsMaterial);

  const maskGeometry = new SphereGeometry(0.985, 40, 28);
  const maskMaterial = new MeshBasicMaterial({ colorWrite: false });
  const mask = new Mesh(maskGeometry, maskMaterial);
  mask.renderOrder = -1;

  const gridGeometry = new EdgesGeometry(new IcosahedronGeometry(1.012, 3), 1);
  const gridMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.1, depthWrite: false });
  const grid = new LineSegments(gridGeometry, gridMaterial);

  const nodeCount = 16;
  const nodes = Array.from({ length: nodeCount }, (_, i) => {
    const y = ((i * 0.618 + 0.13) % 1) * 1.7 - 0.85;
    const a = i * 2.399963;
    const r = Math.sqrt(1 - y * y);
    return new Vector3(Math.cos(a) * r, y, Math.sin(a) * r);
  });
  const nodePos = new Float32Array(nodeCount * 3);
  nodes.forEach((n, i) => nodePos.set([n.x * 1.012, n.y * 1.012, n.z * 1.012], i * 3));
  const nodeGeometry = new BufferGeometry();
  nodeGeometry.setAttribute('position', new Float32BufferAttribute(nodePos, 3));
  const nodeMaterial = new PointsMaterial({ size: 0.075, transparent: true, depthWrite: false });
  const nodePoints = new Points(nodeGeometry, nodeMaterial);

  const links = [];
  for (let i = 0; i < nodeCount; i += 1) {
    links.push([i, (i * 5 + 3) % nodeCount]);
    if (i % 3 === 0) links.push([i, (i + 7) % nodeCount]);
  }
  const arcs = links
    .filter(([a, b]) => a !== b)
    .map(([a, b]) => {
      const from = nodes[a];
      const to = nodes[b];
      const lift = 1.15 + from.distanceTo(to) * 0.42;
      const mid = from.clone().add(to).normalize().multiplyScalar(lift);
      return new QuadraticBezierCurve3(from.clone().multiplyScalar(1.012), mid, to.clone().multiplyScalar(1.012));
    });
  const arcMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.42, depthWrite: false });
  const arcLines = arcs.map((curve) => {
    const geo = new BufferGeometry().setFromPoints(curve.getPoints(48));
    return new Line(geo, arcMaterial);
  });

  const packetPos = new Float32Array(arcs.length * 3);
  const packetGeometry = new BufferGeometry();
  const packetAttr = new Float32BufferAttribute(packetPos, 3);
  packetGeometry.setAttribute('position', packetAttr);
  const packetMaterial = new PointsMaterial({ size: 0.085, transparent: true, depthWrite: false });
  const packets = new Points(packetGeometry, packetMaterial);
  const scratch = new Vector3();

  const ringMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.35, depthWrite: false });
  const equator = new LineLoop(circle(1.34, 200), ringMaterial);
  equator.rotation.set(0.4, 0, 0.3);
  const equatorB = new LineLoop(circle(1.62, 220), ringMaterial);
  equatorB.rotation.set(-0.5, 0, -0.2);

  const stage = createStage(container, {
    className: 'stack-gl',
    lite: options.lite,
    still: options.still,
    fit: (camera, aspect) => {
      camera.position.set(0, 0, 4.9 * Math.max(1, 1.15 / aspect));
      camera.lookAt(0, 0, 0);
    },
    pickables: () => [],
    onTheme: (st) => {
      const accent = cssColor('--accent', '#ff3b30');
      const text = cssColor('--text', '#f3f2ee');
      dotsMaterial.color.copy(st.light ? text : cssColor('--accent-2', '#f3f2ee'));
      dotsMaterial.opacity = st.light ? 0.55 : 0.7;
      dotsMaterial.blending = st.light ? NormalBlending : AdditiveBlending;
      gridMaterial.color.copy(text);
      nodeMaterial.color.copy(accent);
      arcMaterial.color.copy(accent);
      packetMaterial.color.copy(st.light ? text : cssColor('--accent-2', '#f3f2ee'));
      ringMaterial.color.copy(accent);
      [dotsMaterial, packetMaterial, nodeMaterial].forEach((m) => {
        m.needsUpdate = true;
      });
    },
    update: (time, st) => {
      st.group.rotation.x = 0.35 - st.tilt.y * 0.15;
      st.yawOffset = st.still ? 0 : time * 0.05;
      equator.rotation.z = 0.3 + time * 0.08;
      equatorB.rotation.y = time * -0.06;
      nodeMaterial.size = 0.075 + Math.sin(time * 3) * 0.015;
      arcs.forEach((curve, i) => {
        const t = st.still ? (i * 0.17) % 1 : (time * (0.12 + (i % 5) * 0.03) + i * 0.173) % 1;
        curve.getPoint(t, scratch);
        packetPos.set([scratch.x, scratch.y, scratch.z], i * 3);
      });
      packetAttr.needsUpdate = true;
    },
  });
  stage.group.add(mask, dots, grid, nodePoints, packets, equator, equatorB, ...arcLines);
  stage.request();
  return {
    setActive() {},
    dispose: () =>
      stage.dispose(() => {
        dotsGeometry.dispose();
        dotsMaterial.dispose();
        maskGeometry.dispose();
        maskMaterial.dispose();
        gridGeometry.dispose();
        gridMaterial.dispose();
        nodeGeometry.dispose();
        nodeMaterial.dispose();
        arcLines.forEach((line) => line.geometry.dispose());
        arcMaterial.dispose();
        packetGeometry.dispose();
        packetMaterial.dispose();
        equator.geometry.dispose();
        equatorB.geometry.dispose();
        ringMaterial.dispose();
      }),
  };
}
