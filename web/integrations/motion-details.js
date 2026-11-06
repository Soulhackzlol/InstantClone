// ---------------------------------------------------------------- motion details

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

// Spinner on a button while `fn` runs; a second click is ignored.
async function busy(btn, fn){
  if (!btn || busyEls.has(btn)) return undefined;
  busyEls.add(btn);
  btn.classList.add('is-busy');
  btn.disabled = true;
  btn.setAttribute('aria-busy', 'true');
  try {
    return await fn();
  } finally {
    busyEls.delete(btn);
    btn.classList.remove('is-busy');
    btn.removeAttribute('aria-busy');
    btn.disabled = !!btn._wantDisabled;
    delete btn._wantDisabled;
  }
}

// Destructive buttons ask on the button itself: the first click arms it,
// a second within three seconds confirms. True when confirmed.
function armConfirm(btn, label){
  if (btn.hasAttribute('data-armed')) return true;
  const before = btn.innerHTML;
  btn.setAttribute('data-armed', '');
  btn.classList.add('ig-armed');
  btn.innerHTML = esc(label);
  setTimeout(() => {
    if (!btn.isConnected || !btn.hasAttribute('data-armed')) return;
    btn.removeAttribute('data-armed');
    btn.classList.remove('ig-armed');
    btn.innerHTML = before;
  }, 3000);
  return false;
}

// The live element for a selector inside the open modal, never one that is
// fading out.
function q(sel){
  if (!Modal.el) return null;
  return [...Modal.el.querySelectorAll(sel)].find(el => !el.closest('[data-leaving]')) || null;
}

// A toast with Undo, for actions that are easy to regret.
// `adds`: the Undo takes away integrations just added (see dropAddUndos).
function undoToast(text, undo, adds){ actionToast(text, 'Undo', undo, adds); }
// A toast with one button: Undo, or the obvious next step ("Edit copy").
function actionToast(text, label, act, adds){
  const rail = $('toast-rail');
  if (!rail){ toast(text, 'ok'); return; }
  const el = document.createElement('div');
  el.className = 'ic-toast ok ig-undo' + (adds ? ' ig-undo-adds' : '');
  el.innerHTML = `<span class="ic-toast-icon">✓</span><span class="ig-undo-text"></span><button type="button" class="ig-undo-btn"></button>
    <i class="ig-undo-bar" style="animation-duration:${UNDO_MS}ms"></i>`;
  el.querySelector('.ig-undo-text').textContent = text;
  el.querySelector('.ig-undo-btn').textContent = label;
  let used = false;
  const leave = () => {
    if (!el.isConnected || el.classList.contains('leaving')) return;
    el.classList.add('leaving');
    setTimeout(() => el.remove(), 350);
  };
  el.icLeave = leave;
  el.querySelector('button').addEventListener('click', async () => {
    if (used) return;
    used = true;
    leave();
    await act();
  });
  rail.appendChild(el);
  // Pointing at it holds the countdown (the bar pauses in CSS too).
  let left = UNDO_MS, since = Date.now(), timer = setTimeout(leave, left);
  el.addEventListener('pointerenter', () => { clearTimeout(timer); left -= Date.now() - since; });
  el.addEventListener('pointerleave', () => { since = Date.now(); timer = setTimeout(leave, Math.max(left, 600)); });
}
const UNDO_MS = 8000;
// An integration's editor opening: an Undo of an add still up could take
// away what it edits, so it goes.
function dropAddUndos(){
  document.querySelectorAll('#toast-rail .ig-undo-adds').forEach(el => { if (el.icLeave) el.icLeave(); });
}

// A secret field (topic, webhook link, headers, secret address): hidden
// like a password until the user asks to see it.
function secretField(id, control, label){
  const shown = S.revealed.has(id);
  // The label names the field: clicking it focuses it. Without `for`, the
  // Show button inside it would be its control, and a click on the words
  // would show the secret, maybe on stream.
  const fid = 'sf-' + String(id).replace(/[^A-Za-z0-9_-]/g, '_');
  control = control.replace(/<(input|textarea)\b/, `<$1 id="${fid}"`);
  return `<div class="dff ig-secret-field${shown ? ' shown' : ''}" data-field="${esc(id)}"><label for="${fid}">${label}
    <button type="button" class="ig-reveal" data-act="reveal" data-field="${esc(id)}" aria-pressed="${shown}">
      <span class="ig-eye">${svg('eye')}${svg('eyeOff')}</span><span data-tick>${shown ? 'Hide' : 'Show'}</span></button></label>${control}</div>`;
}

function parseJson(text){
  try {
    const v = JSON.parse(text);
    return v !== null && typeof v === 'object' ? v : undefined;
  } catch(_){ return undefined; }
}
// Every value in a JSON answer, with the path a variable uses to reach it.
// `usable`: the path fits a variable name (letters, digits, _).
function jsonLeaves(root){
  const out = [];
  const visit = (v, path) => {
    if (out.length >= 200) return;
    if (v !== null && typeof v === 'object'){
      (Array.isArray(v) ? v.slice(0, 25).map((x, i) => [i, x]) : Object.entries(v)).forEach(([k, x]) => visit(x, path.concat(k)));
      return;
    }
    out.push({path:path.join('.'), text:v === null ? '' : String(v), usable:path.length > 0 && path.every(k => /^[A-Za-z0-9_]+$/.test(String(k)))});
  };
  visit(root, []);
  return out;
}
// The answer as a tree; every value is a button that picks its variable.
function jsonTreeHtml(root, saveAs){
  let budget = 400;
  const node = (v, path, key, depth, index) => {
    if (budget-- <= 0) return '';
    const label = key === null ? '' : `<span class="k">${esc(key)}</span>`;
    // The first level eases in one row after another.
    const order = depth === 1 ? ` style="--i:${index}"` : '';
    if (v !== null && typeof v === 'object'){
      const all = Array.isArray(v) ? v.length : Object.keys(v).length;
      const entries = Array.isArray(v) ? v.slice(0, 25).map((x, i) => [i, x]) : Object.entries(v).slice(0, 60);
      const kids = entries.map(([k, x], i) => node(x, path.concat(k), String(k), depth + 1, i)).join('')
        + (all > entries.length ? `<div class="ig-jmore">… ${all - entries.length} more</div>` : '');
      if (key === null) return kids || '<div class="muted">(empty)</div>';
      return `<details class="ig-jnode"${order} ${depth < 2 ? 'open' : ''}><summary>${svg('chev')}${label}<span class="ig-jcount">${Array.isArray(v) ? `[${all}]` : `{${all}}`}</span></summary><div class="ig-jkids">${kids}</div></details>`;
    }
    const name = `${saveAs}.json.${path.join('.')}`;
    const usable = path.every(k => /^[A-Za-z0-9_]+$/.test(String(k))) && name.length <= 64;
    const text = v === null ? 'null' : String(v);
    const shown = typeof v === 'string' ? `"${text.length > 80 ? text.slice(0, 80) + '…' : text}"` : text;
    const kind = v === null ? 'null' : typeof v;
    return usable
      ? `<button class="ig-jleaf"${order} data-act="b-pick" data-token="${esc(name)}" data-sample="${esc(text.slice(0, 200))}" title="{${esc(name)}}">${label}<span class="v ${kind}">${esc(shown)}</span><span class="ig-juse">Use</span></button>`
      : `<div class="ig-jleaf off"${order} title="This name has characters a variable can't reach">${label}<span class="v ${kind}">${esc(shown)}</span></div>`;
  };
  return node(root, [], null, 0, 0);
}

