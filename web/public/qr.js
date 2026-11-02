/**
 * QR-код для otpauth:// ссылок: байтовый режим, коррекция M, версии 1–10.
 * Строится в браузере, поэтому не зависит от формата картинки от сервера и от CSP.
 */
const QR_EC_PER_BLOCK = [10, 16, 26, 18, 24, 16, 18, 22, 22, 26];
const QR_BLOCKS = [1, 1, 1, 2, 2, 4, 4, 4, 5, 5];
const QR_CODEWORDS = [26, 44, 70, 100, 134, 172, 196, 242, 292, 346];
const QR_ALIGN = [[], [6, 18], [6, 22], [6, 26], [6, 30], [6, 34], [6, 22, 38], [6, 24, 42], [6, 26, 46], [6, 28, 50]];

function qrDataCapacity(version) {
  return QR_CODEWORDS[version - 1] - QR_EC_PER_BLOCK[version - 1] * QR_BLOCKS[version - 1];
}

function qrGfTables() {
  const exp = new Array(512).fill(0);
  const log = new Array(256).fill(0);
  let x = 1;
  for (let i = 0; i < 255; i++) {
    exp[i] = x;
    log[x] = i;
    x <<= 1;
    if (x & 0x100) x ^= 0x11d;
  }
  for (let i = 255; i < 512; i++) exp[i] = exp[i - 255];
  return { exp, log };
}

function qrReedSolomon(data, degree) {
  const { exp, log } = qrGfTables();
  let gen = [1];
  for (let i = 0; i < degree; i++) {
    const next = new Array(gen.length + 1).fill(0);
    for (let j = 0; j < gen.length; j++) {
      next[j] ^= gen[j];
      next[j + 1] ^= gen[j] === 0 ? 0 : exp[log[gen[j]] + i];
    }
    gen = next;
  }
  const rest = new Array(degree).fill(0);
  for (const byte of data) {
    const factor = byte ^ rest.shift();
    rest.push(0);
    if (factor === 0) continue;
    for (let i = 0; i < degree; i++) {
      const coef = gen[i + 1];
      if (coef !== 0) rest[i] ^= exp[log[coef] + log[factor]];
    }
  }
  return rest;
}

function qrCodewords(bytes, version) {
  const capacity = qrDataCapacity(version);
  const bits = [];
  const push = (value, length) => {
    for (let i = length - 1; i >= 0; i--) bits.push((value >>> i) & 1);
  };
  push(4, 4);
  push(bytes.length, version >= 10 ? 16 : 8);
  for (const byte of bytes) push(byte, 8);
  push(0, Math.min(4, capacity * 8 - bits.length));
  while (bits.length % 8) bits.push(0);
  const data = [];
  for (let i = 0; i < bits.length; i += 8) {
    let value = 0;
    for (let j = 0; j < 8; j++) value = (value << 1) | bits[i + j];
    data.push(value);
  }
  for (let pad = 0xec; data.length < capacity; pad ^= 0xec ^ 0x11) data.push(pad);

  const blocks = QR_BLOCKS[version - 1];
  const ecLen = QR_EC_PER_BLOCK[version - 1];
  const shortLen = Math.floor(QR_CODEWORDS[version - 1] / blocks) - ecLen;
  const longFrom = blocks - (QR_CODEWORDS[version - 1] % blocks);
  const dataBlocks = [];
  let offset = 0;
  for (let b = 0; b < blocks; b++) {
    const len = shortLen + (b >= longFrom ? 1 : 0);
    dataBlocks.push(data.slice(offset, offset + len));
    offset += len;
  }
  const ecBlocks = dataBlocks.map((block) => qrReedSolomon(block, ecLen));
  const out = [];
  for (let i = 0; i <= shortLen; i++) {
    for (const block of dataBlocks) if (i < block.length) out.push(block[i]);
  }
  for (let i = 0; i < ecLen; i++) for (const block of ecBlocks) out.push(block[i]);
  return out;
}

function qrBit(value, index) {
  return ((value >>> index) & 1) !== 0;
}

function qrMaskTest(mask, x, y) {
  switch (mask) {
    case 0: return (x + y) % 2 === 0;
    case 1: return y % 2 === 0;
    case 2: return x % 3 === 0;
    case 3: return (x + y) % 3 === 0;
    case 4: return (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0;
    case 5: return ((x * y) % 2) + ((x * y) % 3) === 0;
    case 6: return (((x * y) % 2) + ((x * y) % 3)) % 2 === 0;
    default: return (((x + y) % 2) + ((x * y) % 3)) % 2 === 0;
  }
}

function qrPenalty(grid) {
  const size = grid.length;
  let score = 0;
  const runs = (line) => {
    let total = 0;
    let run = 1;
    for (let i = 1; i < size; i++) {
      if (line[i] === line[i - 1]) {
        run++;
      } else {
        if (run >= 5) total += run - 2;
        run = 1;
      }
    }
    return total + (run >= 5 ? run - 2 : 0);
  };
  for (let i = 0; i < size; i++) {
    score += runs(grid[i]);
    score += runs(grid.map((row) => row[i]));
  }
  for (let y = 0; y < size - 1; y++) {
    for (let x = 0; x < size - 1; x++) {
      const c = grid[y][x];
      if (c === grid[y][x + 1] && c === grid[y + 1][x] && c === grid[y + 1][x + 1]) score += 3;
    }
  }
  const pattern = [true, false, true, true, true, false, true, false, false, false, false];
  const matches = (get, start) => {
    for (let k = 0; k < 11; k++) if (get(start + k) !== pattern[k]) return false;
    return true;
  };
  const matchesRev = (get, start) => {
    for (let k = 0; k < 11; k++) if (get(start + k) !== pattern[10 - k]) return false;
    return true;
  };
  for (let a = 0; a < size; a++) {
    for (let s = 0; s + 11 <= size; s++) {
      const row = (i) => grid[a][i];
      const col = (i) => grid[i][a];
      if (matches(row, s) || matchesRev(row, s)) score += 40;
      if (matches(col, s) || matchesRev(col, s)) score += 40;
    }
  }
  let dark = 0;
  for (const row of grid) for (const cell of row) if (cell) dark++;
  const percent = (dark * 100) / (size * size);
  score += Math.floor(Math.abs(percent - 50) / 5) * 10;
  return score;
}

function qrMatrix(text) {
  const bytes = Array.from(new TextEncoder().encode(text));
  let version = 1;
  while (version <= 10 && qrDataCapacity(version) < bytes.length + (version >= 10 ? 3 : 2)) version++;
  if (version > 10) throw new Error('Слишком длинная строка для QR-кода');
  const size = 17 + 4 * version;
  const grid = Array.from({ length: size }, () => new Array(size).fill(false));
  const fixed = Array.from({ length: size }, () => new Array(size).fill(false));
  const put = (x, y, dark) => {
    if (x < 0 || y < 0 || x >= size || y >= size) return;
    grid[y][x] = dark;
    fixed[y][x] = true;
  };

  for (let i = 0; i < size; i++) {
    put(6, i, i % 2 === 0);
    put(i, 6, i % 2 === 0);
  }
  for (const [cx, cy] of [[3, 3], [size - 4, 3], [3, size - 4]]) {
    for (let dy = -4; dy <= 4; dy++) {
      for (let dx = -4; dx <= 4; dx++) {
        const dist = Math.max(Math.abs(dx), Math.abs(dy));
        put(cx + dx, cy + dy, dist !== 2 && dist !== 4);
      }
    }
  }
  const align = QR_ALIGN[version - 1];
  for (let i = 0; i < align.length; i++) {
    for (let j = 0; j < align.length; j++) {
      const corner = (i === 0 && j === 0) || (i === 0 && j === align.length - 1) || (i === align.length - 1 && j === 0);
      if (corner) continue;
      for (let dy = -2; dy <= 2; dy++) {
        for (let dx = -2; dx <= 2; dx++) put(align[i] + dx, align[j] + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
      }
    }
  }
  const drawFormat = (mask) => {
    const data = mask;
    let rest = data;
    for (let i = 0; i < 10; i++) rest = (rest << 1) ^ ((rest >>> 9) * 0x537);
    const bits = ((data << 10) | rest) ^ 0x5412;
    for (let i = 0; i <= 5; i++) put(8, i, qrBit(bits, i));
    put(8, 7, qrBit(bits, 6));
    put(8, 8, qrBit(bits, 7));
    put(7, 8, qrBit(bits, 8));
    for (let i = 9; i < 15; i++) put(14 - i, 8, qrBit(bits, i));
    for (let i = 0; i < 8; i++) put(size - 1 - i, 8, qrBit(bits, i));
    for (let i = 8; i < 15; i++) put(8, size - 15 + i, qrBit(bits, i));
    put(8, size - 8, true);
  };
  drawFormat(0);
  if (version >= 7) {
    let rest = version;
    for (let i = 0; i < 12; i++) rest = (rest << 1) ^ ((rest >>> 11) * 0x1f25);
    const bits = (version << 12) | rest;
    for (let i = 0; i < 18; i++) {
      const a = size - 11 + (i % 3);
      const b = Math.floor(i / 3);
      put(a, b, qrBit(bits, i));
      put(b, a, qrBit(bits, i));
    }
  }

  const words = qrCodewords(bytes, version);
  let index = 0;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    for (let vert = 0; vert < size; vert++) {
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const y = ((right + 1) & 2) === 0 ? size - 1 - vert : vert;
        if (!fixed[y][x] && index < words.length * 8) {
          grid[y][x] = qrBit(words[index >>> 3], 7 - (index & 7));
          index++;
        }
      }
    }
  }

  let best = null;
  let bestScore = Infinity;
  for (let mask = 0; mask < 8; mask++) {
    const trial = grid.map((row, y) => row.map((cell, x) => (fixed[y][x] ? cell : cell !== qrMaskTest(mask, x, y))));
    const saved = grid.map((row) => row.slice());
    for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) grid[y][x] = trial[y][x];
    drawFormat(mask);
    const score = qrPenalty(grid);
    if (score < bestScore) {
      bestScore = score;
      best = grid.map((row) => row.slice());
    }
    for (let y = 0; y < size; y++) for (let x = 0; x < size; x++) grid[y][x] = saved[y][x];
  }
  return best;
}

function qrSvg(text) {
  const grid = qrMatrix(text);
  const size = grid.length;
  const ns = 'http://www.w3.org/2000/svg';
  const svg = document.createElementNS(ns, 'svg');
  svg.setAttribute('viewBox', `0 0 ${size + 8} ${size + 8}`);
  svg.setAttribute('shape-rendering', 'crispEdges');
  const back = document.createElementNS(ns, 'rect');
  back.setAttribute('width', String(size + 8));
  back.setAttribute('height', String(size + 8));
  back.setAttribute('fill', '#ffffff');
  const path = document.createElementNS(ns, 'path');
  let d = '';
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) if (grid[y][x]) d += `M${x + 4} ${y + 4}h1v1h-1z`;
  }
  path.setAttribute('d', d);
  path.setAttribute('fill', '#000000');
  svg.append(back, path);
  return svg;
}

if (typeof module !== 'undefined') module.exports = { qrMatrix };
