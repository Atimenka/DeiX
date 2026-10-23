import {
  AdditiveBlending,
  BoxGeometry,
  BufferGeometry,
  Color,
  EdgesGeometry,
  Float32BufferAttribute,
  Group,
  IcosahedronGeometry,
  LineBasicMaterial,
  LineLoop,
  LineSegments,
  Mesh,
  MeshBasicMaterial,
  NormalBlending,
  PerspectiveCamera,
  Points,
  Raycaster,
  Scene,
  ShaderMaterial,
  Vector2,
  WebGLRenderer,
} from 'three';

const VERT = `
uniform float uTime;
uniform vec2 uPointer;
uniform float uSize;
uniform float uPx;
uniform float uBurst;
varying float vMix;
varying float vGlow;
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
  gl_PointSize = uSize * uPx * (1.0 + push * 1.9) * (7.0 / -mv.z);
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
void main(){
  float r = length(gl_PointCoord - 0.5);
  if (r > 0.5) discard;
  float soft = smoothstep(0.5, 0.02, r);
  vec3 col = mix(mix(uA, uB, smoothstep(0.3, 0.7, vMix)), uC, uHue) + vGlow * 0.5;
  gl_FragColor = vec4(col, soft * uAlpha * (0.5 + vGlow * 0.8));
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

  const dustUniforms = { uPx: { value: 1 }, uTime: { value: 0 }, uColor: { value: new Color() }, uAlpha: { value: 0.8 } };
  const dustMaterial = new ShaderMaterial({ uniforms: dustUniforms, vertexShader: DUST_VERT, fragmentShader: DUST_FRAG, transparent: true, depthWrite: false });
  const dust = new Points(dustGeometry(options.lite ? 700 : 1700), dustMaterial);
  scene.add(dust);

  let light = false;
  function applyTheme() {
    light = document.documentElement.getAttribute('data-theme') === 'light';
    uniforms.uA.value.copy(cssColor('--accent', '#8b5cf6'));
    uniforms.uB.value.copy(cssColor('--accent-2', '#22d3ee'));
    uniforms.uC.value.set(light ? '#db2777' : '#f472b6');
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
  function pulse() {
    burst = 1;
  }

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
    progressSmooth += (progress - progressSmooth) * (still ? 1 : 0.08);
    burst *= 0.94;
    uniforms.uTime.value = time;
    dustUniforms.uTime.value = time;
    uniforms.uBurst.value = burst;
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

    const base = (dim ? 0.45 : 1) * (wide ? 1 : 0.3) * alpha;
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
    dustMaterial.dispose();
    renderer.dispose();
    canvas.remove();
  }

  if (still) draw(2.5);
  else raf = requestAnimationFrame(frame);
  return { dispose, pulse };
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

/**
 * Интерактивный разборный стек DeiX в контейнере: тянуть — вращать, наведение подсвечивает слой.
 * options: still (рисовать только по событиям), lite (ниже разрешение), onHover(index) — слой под курсором.
 * Возвращает { setActive(index), dispose }.
 */
export function mountStack(container, options = {}) {
  const still = Boolean(options.still);
  const canvas = document.createElement('canvas');
  canvas.className = 'stack-gl';
  canvas.setAttribute('aria-hidden', 'true');
  const renderer = new WebGLRenderer({ canvas, alpha: true, antialias: true, powerPreference: 'default' });
  renderer.setClearColor(0x000000, 0);
  container.append(canvas);

  const scene = new Scene();
  const camera = new PerspectiveCamera(32, 1, 0.1, 60);
  const group = new Group();
  scene.add(group);

  const slabs = STACK.map(([w, d, h], index) => {
    const geometry = new BoxGeometry(w, h, d);
    const fill = new MeshBasicMaterial({ transparent: true, opacity: 0.16, depthWrite: false });
    const edge = new LineBasicMaterial({ transparent: true, opacity: 0.8 });
    const mesh = new Mesh(geometry, fill);
    const edgeGeometry = new EdgesGeometry(geometry);
    mesh.add(new LineSegments(edgeGeometry, edge));
    mesh.userData.index = index;
    group.add(mesh);
    return { mesh, fill, edge, geometry, edgeGeometry, glow: 0 };
  });

  const railPos = [];
  for (const [x, z] of [[-0.85, -0.55], [0.85, -0.55], [0.85, 0.55], [-0.85, 0.55]]) railPos.push(x, -1, z, x, 1, z);
  const railGeometry = new BufferGeometry();
  railGeometry.setAttribute('position', new Float32BufferAttribute(railPos, 3));
  const railMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.28 });
  const rails = new LineSegments(railGeometry, railMaterial);
  group.add(rails);

  let light = false;
  function applyTheme() {
    light = document.documentElement.getAttribute('data-theme') === 'light';
    const low = cssColor('--accent-2', '#22d3ee');
    const mid = cssColor('--accent', '#8b5cf6');
    const high = new Color('#f472b6');
    slabs.forEach((slab, i) => {
      const t = i / (STACK.length - 1);
      const color = t < 0.5 ? low.clone().lerp(mid, t * 2) : mid.clone().lerp(high, (t - 0.5) * 2);
      slab.fill.color.copy(color);
      slab.edge.color.copy(color);
    });
    railMaterial.color.copy(mid);
    request();
  }
  const themeWatch = new MutationObserver(applyTheme);
  themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });

  let width = 1;
  let height = 1;
  function resize() {
    width = Math.max(1, container.clientWidth);
    height = Math.max(1, container.clientHeight);
    renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, options.lite ? 1.25 : 2));
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    camera.position.set(0, 0, 10 * Math.max(1, 4.9 / (5.73 * camera.aspect)));
    camera.updateProjectionMatrix();
    request();
  }
  const sizeWatch = new ResizeObserver(resize);
  sizeWatch.observe(container);

  const raycaster = new Raycaster();
  const ndc = new Vector2();
  const tilt = new Vector2();
  let dragging = false;
  let lastX = 0;
  let velocity = 0;
  let yaw = 0.7;
  let hover = -1;
  let active = options.initial ?? -1;
  let explode = still ? 1 : 0;

  function pick(event) {
    const rect = canvas.getBoundingClientRect();
    ndc.set(((event.clientX - rect.left) / rect.width) * 2 - 1, -(((event.clientY - rect.top) / rect.height) * 2 - 1));
    tilt.set(ndc.x, ndc.y);
    raycaster.setFromCamera(ndc, camera);
    const hit = raycaster.intersectObjects(slabs.map((slab) => slab.mesh), false)[0];
    const index = hit ? hit.object.userData.index : -1;
    if (index !== hover) {
      hover = index;
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
      yaw += velocity;
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
    explode += ((visible || still ? 1 : 0) - explode) * (still ? 1 : 0.05);
    if (!dragging) {
      velocity *= 0.94;
      yaw += velocity + (still ? 0 : 0.0035);
    }
    const gap = 0.3 + explode * 0.58;
    slabs.forEach((slab, i) => {
      slab.glow += ((i === active || i === hover ? 1 : 0) - slab.glow) * (still ? 1 : 0.14);
      slab.mesh.position.set(slab.glow * 0.45, (i - (STACK.length - 1) / 2) * gap, 0);
      slab.mesh.scale.setScalar(1 + slab.glow * 0.04);
      slab.fill.opacity = (light ? 0.24 : 0.14) + slab.glow * (light ? 0.36 : 0.3);
      slab.edge.opacity = 0.55 + slab.glow * 0.45;
    });
    rails.scale.y = ((STACK.length - 1) / 2) * gap + 0.1;
    group.rotation.y = yaw;
    group.rotation.x = 0.5 - tilt.y * 0.12;
    group.position.y = Math.sin(time * 0.8) * (still ? 0 : 0.06);
    renderer.render(scene, camera);
  }

  let raf = 0;
  let visible = still;
  let disposed = false;
  function loop(now) {
    raf = requestAnimationFrame(loop);
    if (!document.hidden) draw(now / 1000);
  }
  function request() {
    if (still && !disposed) draw(0);
  }
  const viewWatch = new IntersectionObserver((entries) => {
    visible = entries[0].isIntersecting;
    if (still) return;
    if (visible && !raf) raf = requestAnimationFrame(loop);
    else if (!visible && raf) {
      cancelAnimationFrame(raf);
      raf = 0;
    }
  });
  viewWatch.observe(container);

  function setActive(index) {
    active = index;
    request();
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    cancelAnimationFrame(raf);
    viewWatch.disconnect();
    sizeWatch.disconnect();
    themeWatch.disconnect();
    slabs.forEach((slab) => {
      slab.geometry.dispose();
      slab.edgeGeometry.dispose();
      slab.fill.dispose();
      slab.edge.dispose();
    });
    railGeometry.dispose();
    railMaterial.dispose();
    renderer.dispose();
    canvas.remove();
  }

  applyTheme();
  resize();
  return { setActive, dispose };
}
