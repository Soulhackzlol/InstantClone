// ---------------------------------------------------------------- motion

// A changed number or short value rolls in instead of snapping: up when a
// number grew, down when it shrank. Only for elements marked data-tick.
function tick(el, was, now){
  if (!el || !el.hasAttribute('data-tick')) return;
  const a = parseFloat(was), b = parseFloat(now);
  const from = !isNaN(a) && !isNaN(b) && b < a ? '-45%' : '45%';
  el.animate([
    {transform:`translateY(${from})`, opacity:0, filter:'blur(2px)'},
    {transform:'none', opacity:1, filter:'blur(0)'},
  ], {duration:260, easing:EASE});
}

// A secret field decodes letter by letter when shown and folds back into
// dots, from the end, when hidden. Each letter scrambles for a moment, then
// settles. It is drawn on a layer over the field (which morph owns and
// leaves untouched); one animation frame loop, writing only what changed,
// and typing into the field ends it at once.
const SCRAMBLE = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789#%&*+=?';
const DECODE_MAX = 300;          // longer values just switch
const decoders = new WeakMap();  // field -> stop()

function decodeField(control, reveal){
  if (!control || reduced()) return;
  const host = control.closest('.ig-secret-field');
  const chars = [...(control.value || '')];
  if (!host || !chars.length || chars.length > DECODE_MAX) return;
  if (decoders.has(control)) decoders.get(control)();
  const layer = decodeLayer(control, host);
  const cells = [];
  for (const ch of chars){
    if (ch === '\n'){ layer.appendChild(document.createElement('br')); continue; }
    const span = layer.appendChild(document.createElement('span'));
    cells.push({span, ch, state:-1});
  }
  layer.scrollTop = control.scrollTop;
  layer.scrollLeft = control.scrollLeft;
  const n = cells.length;
  const step = Math.min(24, 480 / n), settle = 130;
  const total = (n - 1) * step + settle;
  const started = performance.now();
  let frame = 0;
  const stop = () => {
    cancelAnimationFrame(frame);
    control.removeEventListener('input', stop);
    control.classList.remove('ig-decoding');
    decoders.delete(control);
    layer.remove();
  };
  const draw = now => {
    const t = now - started;
    // Scrambled letters change 20 times a second, not every frame.
    const beat = Math.floor(t / 50);
    cells.forEach((c, i) => {
      const at = (reveal ? i : n - 1 - i) * step;
      const state = t < at ? (reveal ? 0 : 2) : t < at + settle ? 1 : (reveal ? 2 : 0);
      const text = state === 0 ? '•' : state === 2 ? c.ch : SCRAMBLE[(i * 7 + beat * 13) % SCRAMBLE.length];
      if (c.span.textContent !== text) c.span.textContent = text;
      if (c.state !== state){ c.state = state; c.span.className = 's' + state; }
    });
    if (t < total) frame = requestAnimationFrame(draw); else stop();
  };
  control.classList.add('ig-decoding');
  control.addEventListener('input', stop);
  decoders.set(control, stop);
  frame = requestAnimationFrame(draw);
}

// The layer the decode draws on: the field's exact box and type metrics,
// so letters land where the field's own letters are.
function decodeLayer(control, host){
  const cs = getComputedStyle(control);
  const box = control.getBoundingClientRect(), at = host.getBoundingClientRect();
  const layer = document.createElement('div');
  layer.className = 'ig-decode' + (control.tagName === 'TEXTAREA' ? ' area' : '');
  layer.setAttribute('aria-hidden', 'true');
  layer.setAttribute('data-persist', '');
  Object.assign(layer.style, {
    left:(box.left - at.left) + 'px', top:(box.top - at.top) + 'px', width:box.width + 'px', height:box.height + 'px',
  });
  for (const prop of ['fontFamily', 'fontSize', 'fontWeight', 'letterSpacing', 'lineHeight', 'paddingTop', 'paddingRight',
    'paddingBottom', 'paddingLeft', 'borderTopWidth', 'borderRightWidth', 'borderBottomWidth', 'borderLeftWidth']){
    layer.style[prop] = cs[prop];
  }
  return host.appendChild(layer);
}
