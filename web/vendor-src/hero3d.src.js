import {
  AdditiveBlending,
  BufferGeometry,
  Color,
  Float32BufferAttribute,
  Group,
  IcosahedronGeometry,
  LineBasicMaterial,
  LineLoop,
  NormalBlending,
  PerspectiveCamera,
  Points,
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
  vec3 p = dir * (1.55 + (n - 0.5) * 0.7);
  vec4 mv = modelViewMatrix * vec4(p, 1.0);
  vec4 clip = projectionMatrix * mv;
  float push = smoothstep(0.38, 0.0, distance(clip.xy / clip.w, uPointer));
  mv.xyz += normalize(mat3(modelViewMatrix) * dir) * push * 0.42;
  gl_Position = projectionMatrix * mv;
  gl_PointSize = uSize * uPx * (1.0 + push * 1.8) * (7.0 / -mv.z);
  vMix = n;
  vGlow = push;
}`;

const FRAG = `
uniform vec3 uA;
uniform vec3 uB;
uniform float uAlpha;
varying float vMix;
varying float vGlow;
void main(){
  float r = length(gl_PointCoord - 0.5);
  if (r > 0.5) discard;
  float soft = smoothstep(0.5, 0.05, r);
  vec3 col = mix(uA, uB, smoothstep(0.3, 0.7, vMix)) + vGlow * 0.45;
  gl_FragColor = vec4(col, soft * uAlpha * (0.5 + vGlow * 0.7));
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

function cssColor(name, fallback) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  try {
    return new Color(value || fallback);
  } catch {
    return new Color(fallback);
  }
}

/** Монтирует сцену в контейнер; возвращает { dispose }. Бросает исключение, если WebGL недоступен. */
export function mount(container, options = {}) {
  const still = Boolean(options.still);
  const canvas = document.createElement('canvas');
  canvas.className = 'hero-gl';
  canvas.setAttribute('aria-hidden', 'true');
  const renderer = new WebGLRenderer({ canvas, alpha: true, antialias: false, powerPreference: 'default' });
  renderer.setClearColor(0x000000, 0);
  container.prepend(canvas);

  const scene = new Scene();
  const camera = new PerspectiveCamera(45, 1, 0.1, 50);
  camera.position.z = 7;
  const group = new Group();
  scene.add(group);

  const uniforms = {
    uTime: { value: 0 },
    uPointer: { value: new Vector2(9, 9) },
    uSize: { value: 5.2 },
    uPx: { value: 1 },
    uA: { value: new Color() },
    uB: { value: new Color() },
    uAlpha: { value: 1 },
  };
  const material = new ShaderMaterial({ uniforms, vertexShader: VERT, fragmentShader: FRAG, transparent: true, depthWrite: false });
  const cloud = new Points(new IcosahedronGeometry(1, 12), material);
  group.add(cloud);

  const ringMaterial = new LineBasicMaterial({ transparent: true, opacity: 0.35, depthWrite: false });
  const ringA = new LineLoop(circle(2.35, 220), ringMaterial);
  ringA.rotation.set(1.15, 0.2, 0.3);
  const ringB = new LineLoop(circle(2.9, 260), ringMaterial);
  ringB.rotation.set(0.45, 0, -0.5);
  group.add(ringA, ringB);

  function applyTheme() {
    const light = document.documentElement.getAttribute('data-theme') === 'light';
    uniforms.uA.value.copy(cssColor('--accent', '#8b5cf6'));
    uniforms.uB.value.copy(cssColor('--accent-2', '#22d3ee'));
    material.blending = light ? NormalBlending : AdditiveBlending;
    uniforms.uAlpha.value = light ? 0.75 : 0.95;
    ringMaterial.color.copy(uniforms.uA.value);
    ringMaterial.opacity = light ? 0.4 : 0.32;
    material.needsUpdate = true;
  }
  applyTheme();
  const themeWatch = new MutationObserver(applyTheme);
  themeWatch.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });

  let width = 1;
  let height = 1;
  function resize() {
    const box = container.getBoundingClientRect();
    width = Math.max(1, Math.round(box.width));
    height = Math.max(1, Math.round(box.height));
    const ratio = Math.min(window.devicePixelRatio || 1, 1.75);
    renderer.setPixelRatio(ratio);
    renderer.setSize(width, height, false);
    uniforms.uPx.value = ratio;
    camera.aspect = width / height;
    const halfWidth = Math.tan((camera.fov * Math.PI) / 360) * camera.position.z * camera.aspect;
    group.position.x = width > 900 ? halfWidth * 0.5 : 0;
    camera.updateProjectionMatrix();
  }
  const sizeWatch = new ResizeObserver(resize);
  sizeWatch.observe(container);
  resize();

  const pointer = new Vector2(9, 9);
  const target = new Vector2(0, 0);
  const smooth = new Vector2(0, 0);
  function onMove(event) {
    const box = container.getBoundingClientRect();
    target.set(((event.clientX - box.left) / box.width) * 2 - 1, -(((event.clientY - box.top) / box.height) * 2 - 1));
    pointer.copy(target);
  }
  function onLeave() {
    pointer.set(9, 9);
    target.set(0, 0);
  }
  window.addEventListener('pointermove', onMove, { passive: true });
  document.addEventListener('pointerleave', onLeave);

  let visible = true;
  const view = new IntersectionObserver((entries) => {
    visible = entries[0].isIntersecting;
  });
  view.observe(container);

  let raf = 0;
  let last = performance.now();
  let slow = 0;
  let frames = 0;
  let disposed = false;

  function frame(now) {
    raf = requestAnimationFrame(frame);
    if (!visible || document.hidden) {
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
    smooth.lerp(target, 0.06);
    uniforms.uTime.value = time;
    uniforms.uPointer.value.lerp(pointer, 0.18);
    group.rotation.y = time * 0.07 + smooth.x * 0.5;
    group.rotation.x = -smooth.y * 0.3;
    ringA.rotation.z = 0.3 + time * 0.05;
    ringB.rotation.y = time * -0.04;
    const fade = Math.max(0, 1 - window.scrollY / (container.clientHeight * 0.85));
    uniforms.uAlpha.value = (document.documentElement.getAttribute('data-theme') === 'light' ? 0.75 : 0.95) * fade;
    ringMaterial.opacity = (document.documentElement.getAttribute('data-theme') === 'light' ? 0.4 : 0.32) * fade;
    group.position.y = Math.min(window.scrollY * 0.004, 2);
    renderer.render(scene, camera);
  }

  function dispose() {
    if (disposed) return;
    disposed = true;
    cancelAnimationFrame(raf);
    window.removeEventListener('pointermove', onMove);
    document.removeEventListener('pointerleave', onLeave);
    themeWatch.disconnect();
    sizeWatch.disconnect();
    view.disconnect();
    cloud.geometry.dispose();
    ringA.geometry.dispose();
    ringB.geometry.dispose();
    material.dispose();
    ringMaterial.dispose();
    renderer.dispose();
    canvas.remove();
  }

  if (still) draw(2.5);
  else raf = requestAnimationFrame(frame);
  return { dispose };
}
