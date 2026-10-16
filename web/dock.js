// InstantClone OBS dock: a configurable stack of widgets driven by a saved
// layout. The layout is per-slot (?dock=<id>), persisted server-side so it
// survives OBS clearing its browser cache, with a localStorage mirror for
// instant paint. dock.html holds the markup+CSS; this file is all behaviour.

const $ = id => document.getElementById(id);
const PHASE_TO_STATE = { idle: 'off', preparing: 'arming', ready: 'armed', active: 'active' };
const EASE = 'cubic-bezier(.32,.72,0,1)';
// The longest delay anything can arm, config::MAX_DELAY_MS in the backend.
const DELAY_MAX_S = 3600;
const FORM = { 'content-type': 'application/x-www-form-urlencoded' };
let s = null, lastVal = '0.0', pending = null, pendingTimer = 0;

// Guarded writes. The state stream lands about 4 times a second and most
// pushes change nothing the dock shows; writing only real changes keeps
// running animations from restarting and leaves an idle dock untouched.
function setText(el, t) { if (el.textContent !== t) el.textContent = t; }
function setHtml(el, h) { if (el._html !== h) { el._html = h; el.innerHTML = h; } }
function setHidden(el, h) { if (el.hidden !== h) el.hidden = h; }
function setClass(el, c) { if (el.className !== c) el.className = c; }
function setTitle(el, t) { if (el.title !== t) el.title = t; }
function setDisabled(el, d) { if (el.disabled !== d) el.disabled = d; }
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
function motionOK() { return !document.hidden && !reducedMotion.matches; }
// "4:03" for a millisecond countdown.
function fmtClock(ms) {
  const secs = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
}
// A delay in the dock's one unit, seconds: the number, the field and every
// label agree (Activate 900s under a 900), whatever the length.
function fmtDelay(ms) {
  return Math.round(ms / 1000) + 's';
}

// What a dock opened by a link without the access token sees once a
// dashboard password is on: every call answers 401.
const AUTH_MSG = "This dock link is missing its access token. Copy the dock URL again from the dashboard's OBS tab.";
let authBlocked = false;
async function fetchJ(u, o) {
  try {
    const r = await fetch(u, o);
    const j = r.headers.get('content-type')?.includes('json') ? await r.json().catch(() => null) : null;
    noteAuth(r.status === 401);
    return { ok: r.ok, status: r.status, j };
  } catch (_) { return { ok: false, status: 0, j: null }; }
}
// The message for a failed call: the server's own reason when it gave one.
function failText(r, fallback) {
  if (r.status === 401) return AUTH_MSG;
  if (r.status === 0) return "Can't reach InstantClone. Is it running?";
  return r.j?.error || fallback;
}
function noteAuth(blocked) {
  if (authBlocked === blocked) return;
  authBlocked = blocked;
  paintNote();
}
// One toast at a time. Errors stay long enough to read twice, and a hover
// holds any toast until the pointer leaves.
function toast(msg, kind) {
  const t = $('toast'), k = kind || 'info';
  t.className = 'show ' + k;
  t.firstElementChild.textContent = { ok: '✓', err: '!' }[k] || 'i';
  t.lastElementChild.textContent = msg;
  t._ms = k === 'err' ? 4500 : 2200;
  armToast(t);
}
function armToast(t) {
  clearTimeout(t._h);
  if (!t._hover) t._h = setTimeout(() => t.classList.remove('show'), t._ms);
}
$('toast').addEventListener('mouseenter', e => { e.currentTarget._hover = true; clearTimeout(e.currentTarget._h); });
$('toast').addEventListener('mouseleave', e => { e.currentTarget._hover = false; armToast(e.currentTarget); });
// Safe in text and in either kind of quoted attribute, as integrations.js's.
function esc(v) {
  return String(v == null ? '' : v).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
// OBS's dock browser can refuse the async clipboard (no focus, no
// permission); the older copy command still works there.
async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return true; } catch (_) {}
  const ta = document.createElement('textarea');
  ta.value = text; ta.setAttribute('readonly', ''); ta.style.cssText = 'position:fixed;opacity:0;pointer-events:none';
  document.body.appendChild(ta); ta.select();
  let ok = false;
  try { ok = document.execCommand('copy'); } catch (_) {}
  ta.remove();
  return ok;
}

// ---------------------------------------------------------------- config ----

const params = new URLSearchParams(location.search);
// The dock slot this page is (?dock=<id>). Sanitised to match the server's
// allowed key charset so a hand-typed slot can round-trip to storage.
const SLOT = ((params.get('dock') || 'default').replace(/[^a-zA-Z0-9_-]/g, '').slice(0, 40)) || 'default';
// The access token this dock was opened with, carried into every dock link
// it copies so a password-protected dashboard lets those docks in too.
const TOKEN = (params.get('token') || '').replace(/[^a-zA-Z0-9]/g, '');
const DOCKS_MAX = 64;   // config::MAX_DOCKS

// Widget catalog: id -> label + default options (on + per-widget knobs).
const WIDGETS = {
  status:   { label: 'Status bar',    def: { on: true } },
  phase:    { label: 'Phase chip',    def: { on: true, style: 'chip' } },
  source:   { label: 'Live pill',     def: { on: true, style: 'pill' } },
  number:   { label: 'Delay number',  def: { on: true, units: true, step: 5, controls: true, size: 'md', tint: false } },
  bar:      { label: 'Buffer bar',    def: { on: true, pct: false, thick: false, mono: false } },
  egress:   { label: 'Egress glance', def: { on: true, dests: true, bitrate: true, codec: false } },
  tips:     { label: 'Hint text',     def: { on: true, errorsOnly: false } },
  dest:     { label: 'Destinations',  def: { on: false, confirm: true, view: 'rows', active: false, stats: false, brand: true } },
  profiles: { label: 'Delay profiles',def: { on: false, arm: false, value: true, view: 'chips' } },
  stats:    { label: 'Health stats',  def: { on: false, cpu: true, mem: true, recon: true, bitrate: false, cuts: false, view: 'row' } },
  behavior: { label: 'Auto behavior', def: { on: false } },
  settings: { label: 'Settings',      def: { on: false } },
  overlays: { label: 'Overlays',      def: { on: false, autohide: false, search: true, group: true } },
  // `only`: the one group shown, or '' for all.
  events:   { label: 'Events',        def: { on: false, skip: true, times: true, only: '' } },
  activate: { label: 'Action button', def: { on: true, confirmcut: false, safecut: true } },
};
// Every widget that has stored options. phase + source are sub-widgets of the
// status bar; bar rides along with the number screen it lives in.
const ALL = ['status', 'phase', 'source', 'number', 'bar', 'egress', 'tips', 'dest', 'profiles', 'stats', 'behavior', 'settings', 'overlays', 'events', 'activate'];
// Reorderable rows = the top-level containers the user can drag around.
const ROWS = ['status', 'number', 'profiles', 'egress', 'tips', 'dest', 'stats', 'behavior', 'settings', 'overlays', 'events', 'activate'];
// DOM container per row.
const WEL = {
  status: 'w-top', number: 'w-screen', profiles: 'w-profiles', egress: 'w-eg', tips: 'w-tip', dest: 'w-dests',
  stats: 'w-stats', behavior: 'w-behavior', settings: 'w-settings', overlays: 'w-overlays', events: 'w-events', activate: 'w-cta',
};
// Presets = which widgets are on. Options keep their defaults / last tweak.
const ALL_ON = ['status', 'phase', 'source', 'number', 'bar', 'egress', 'tips', 'dest', 'profiles', 'stats', 'behavior', 'settings', 'overlays', 'events', 'activate'];
const PRESETS = {
  full:      { label: 'Full',         on: ['status', 'phase', 'source', 'number', 'bar', 'egress', 'tips', 'dest', 'activate'] },
  minimal:   { label: 'Minimal',      on: ['status', 'phase', 'source', 'number', 'activate'] },
  delay:     { label: 'Delay only',   on: ['number', 'activate'] },
  dest:      { label: 'Destinations', on: ['status', 'phase', 'source', 'dest'] },
  control:   { label: 'Control',      on: ['status', 'source', 'behavior', 'settings', 'overlays'] },
  events:    { label: 'Events',       on: ['status', 'source', 'events'] },
  dashboard: { label: 'Dashboard',    on: ALL_ON },
};
// Editable options per row. An option targets another widget's store via `w`
// (phase/source live under the status bar; bar under the number screen),
// renders as a segmented mode picker via `modes`/`sel`, else a switch.
const OPTS = {
  status: [
    { k: 'on', label: 'Phase chip', w: 'phase' },
    { k: 'style', label: 'Phase', w: 'phase', modes: [['chip', 'Chip'], ['dot', 'Dot']] },
    { k: 'on', label: 'Live pill', w: 'source' },
    { k: 'style', label: 'Live', w: 'source', modes: [['pill', 'Pill'], ['dot', 'Dot']] },
  ],
  number: [
    { k: 'size', label: 'Size', modes: [['sm', 'S'], ['md', 'M'], ['lg', 'L']] },
    { k: 'step', label: 'Step', sel: [1, 5, 10] },
    { k: 'controls', label: '+/- buttons' }, { k: 'units', label: 'Unit' }, { k: 'tint', label: 'Accent number' },
    { k: 'on', label: 'Buffer bar', w: 'bar' }, { k: 'pct', label: 'Show %', w: 'bar' },
    { k: 'thick', label: 'Thick bar', w: 'bar' }, { k: 'mono', label: 'Accent bar', w: 'bar' },
  ],
  egress: [{ k: 'dests', label: 'Dest count' }, { k: 'bitrate', label: 'Bitrate' }, { k: 'codec', label: 'Codec' }],
  tips:   [{ k: 'errorsOnly', label: 'Errors only' }],
  dest:   [
    { k: 'view', label: 'Layout', modes: [['rows', 'Rows'], ['icons', 'Icons']] },
    { k: 'brand', label: 'Platform colors' },
    { k: 'confirm', label: 'Confirm tap' }, { k: 'stats', label: 'Show bitrate' }, { k: 'active', label: 'Active only' },
  ],
  profiles: [
    { k: 'view', label: 'Layout', modes: [['chips', 'Chips'], ['list', 'List']] },
    // Off, a tap only picks the delay and the main button confirms it.
    { k: 'arm', label: 'Apply on tap' }, { k: 'value', label: 'Show delay' },
  ],
  stats:  [
    { k: 'view', label: 'Layout', modes: [['row', 'Row'], ['tiles', 'Tiles']] },
    { k: 'cpu', label: 'CPU' }, { k: 'mem', label: 'RAM' }, { k: 'recon', label: 'Reconnects' },
    { k: 'bitrate', label: 'Bitrate' }, { k: 'cuts', label: 'Cuts' },
  ],
  overlays: [
    { k: 'search', label: 'Search bar' }, { k: 'group', label: 'Folders' }, { k: 'autohide', label: 'Copy with autohide off' },
  ],
  events: [{ k: 'skip', label: 'Skip button' }, { k: 'times', label: 'Times' }],
  activate: [{ k: 'safecut', label: 'Cut after airs' }, { k: 'confirmcut', label: 'Confirm cut' }],
};

function defaultCfg() {
  const w = {};
  for (const id of ALL) w[id] = { ...WIDGETS[id].def };
  return { v: 1, name: '', preset: 'full', density: 'comfy', accent: 'cyan', order: [...ROWS], w };
}
// Accent presets: [--accent, --accent-ink]. Ink is the dark text drawn on
// filled accent buttons, tuned per hue so it stays legible.
const ACCENTS = {
  cyan:   ['#5ac8fa', '#04121c'],
  green:  ['#34d06a', '#04140a'],
  purple: ['#a78bfa', '#140a24'],
  amber:  ['#e0a33a', '#1c1204'],
  pink:   ['#f472b6', '#24040f'],
  blue:   ['#6aa8ff', '#04101c'],
};
// A dock's display name: what the Docks list calls it. The slot id in its
// URL never changes, so renaming can't strand the OBS dock that uses it.
function cleanName(v) { return String(v || '').replace(/[\u0000-\u001f]/g, '').trim().slice(0, 40); }
// Keep only known row ids, drop dupes, and append any rows a newer version
// added so an old saved order still renders every widget.
function sanitizeOrder(raw) {
  const seen = new Set();
  const out = [];
  if (Array.isArray(raw)) for (const id of raw) if (ROWS.includes(id) && !seen.has(id)) { seen.add(id); out.push(id); }
  for (const id of ROWS) if (!seen.has(id)) out.push(id);
  return out;
}
// Merge a stored (possibly older) layout onto current defaults so widgets or
// options added in a later version get sane values on an old saved dock.
function mergeCfg(raw) {
  const base = defaultCfg();
  if (!raw || typeof raw !== 'object') return base;
  base.name = cleanName(raw.name);
  if (raw.preset) base.preset = raw.preset;
  if (raw.density === 'compact' || raw.density === 'comfy') base.density = raw.density;
  if (ACCENTS[raw.accent]) base.accent = raw.accent;
  base.order = sanitizeOrder(raw.order);
  if (raw.w && typeof raw.w === 'object') {
    for (const id of ALL) {
      if (raw.w[id] && typeof raw.w[id] === 'object') base.w[id] = { ...base.w[id], ...raw.w[id] };
    }
  }
  return base;
}
let cfg = defaultCfg();

// -------------------------------------------------------- capacity gate ----
// The disk ring holds a finite amount of video. A delay bigger than it can
// fill at the current bitrate would only run at what the buffer holds. The
// dock greys out (and refuses to arm) delays the buffer can't hold, from the
// server's own estimate (state.buffer_capacity_ms_est, held at the recent
// peak bitrate), so it greys out exactly what /arm would refuse.
const BUF_SLACK_MS = 2000;   // kept on top of the delay (BUFFER_SLACK_MS)
function currentCapMs() { return (s && s.buffer_capacity_ms_est) || 0; }
// The buffer size, in MB, that holds a delay of `ms` at the bitrate the
// server plans for, rounded up like the server's (buffer_mb_for).
function requiredMB(ms) {
  if (!ms || ms <= 0 || !s || !s.capacity_kbps) return 0;
  const bytes = Math.ceil((ms + BUF_SLACK_MS) * s.capacity_kbps / 8);
  return Math.ceil(Math.ceil(bytes * 16 / 15) / 1048576);
}
function overCap(ms) { const c = currentCapMs(); return c > 0 && ms > c; }
// Why a delay doesn't fit: firm once OBS's bitrate is measured, an
// estimate at a typical rate before that.
function capAdvice(ms) {
  return s && s.capacity_measured
    ? `Buffer too small for ${fmtDelay(ms)}. Raise the buffer size to at least ${requiredMB(ms)} MB in System, then restart.`
    : `Likely too long: at a typical rate the buffer holds about ${fmtDelay(currentCapMs())}. Your real bitrate decides once OBS connects.`;
}
function capToast(ms) {
  toast(capAdvice(ms), 'err');
}
// What the profile chips were last drawn for: which ones the buffer can't
// hold, the phase and the running delay. Chips redraw only when it moves.
let profileGate = '';
function profileGateKey() {
  const ds = s ? PHASE_TO_STATE[s.phase] || 'off' : 'off';
  return ds + ':' + (s?.armed_delay_ms || 0) + ':' + profileCache.map(p => (overCap(p.delay_ms || 0) ? 1 : 0)).join('');
}

// ---------------------------------------------------------- delay controls --

function phaseNow() { return s ? PHASE_TO_STATE[s.phase] || 'off' : 'off'; }
function isEngaged(ds) { return ds === 'armed' || ds === 'active'; }
// The delay in the field, in ms, held to what the server accepts so a label
// never offers a value the click would not send.
function inputMs() {
  const sec = Math.round(+$('d').value || 0);
  return Math.max(0, Math.min(DELAY_MAX_S, sec)) * 1000;
}

// While a delay is armed or running the number is a readout. A tap on it,
// a +/- or a profile opens it for a new value, and the main button then
// offers Adjust / Re-arm. Esc, "Keep" or leaving it unchanged closes it.
let editing = false;
function startEdit(focus) {
  const ds = phaseNow();
  if (!isEngaged(ds)) return;
  if (!editing) {
    editing = true;
    $('d').value = Math.round((s.armed_delay_ms || 0) / 1000);
    fitDelayInput();
  }
  setNumberMode(ds); renderCta();
  if (focus) { $('d').focus(); $('d').select(); }
}
function endEdit() {
  if (!editing) return;
  editing = false;
  if (!s) return;
  const ds = phaseNow();
  setNumberMode(ds); renderCta();
  $('d').blur();
}

function bump(n) {
  if (isEngaged(phaseNow())) startEdit(false);
  const step = cfg.w.number.step || 1;
  const i = $('d');
  setDelayField(Math.max(0, Math.min(DELAY_MAX_S, Math.round(+i.value || 0) + n * step)));
}
// Put a new number in the field, rolling its digits like the dashboard's
// (not while someone is typing in it).
function setDelayField(sec) {
  const i = $('d'), was = i.value || '0', now = String(sec);
  i.value = now;
  fitDelayInput(); renderCta();
  if (was === now || !motionOK() || document.activeElement === i) return;
  const cur = $('cur'), roll = $('d-roll');
  cur.classList.add('d-rolling');
  rollDigits(roll, was, now, () => cur.classList.remove('d-rolling'));
}
// Rolling digits, as on the dashboard: each changed digit rolls up (more)
// or down (less), the right-most first like a counter, and a digit that
// appears or goes away grows or shrinks its column instead of shoving the
// number sideways. Plays in `el`; `done` hands back to the plain text.
// Clicked again mid-roll, it runs at double speed so fast clicks keep up.
function rollDigits(el, from, to, done) {
  const quick = el.classList.contains('rolling');
  const duration = quick ? 170 : 340, stagger = quick ? 20 : 40;
  const n = Math.max(from.length, to.length);
  const A = from.padStart(n, ' '), B = to.padStart(n, ' ');
  const up = parseFloat(to) > parseFloat(from);
  const glyph = ch => { const c = document.createElement('span'); c.textContent = ch === ' ' ? '\u00a0' : ch; return c; };
  const cols = [], starts = [];
  let longest = 0;
  for (let i = 0; i < n; i++) {
    const col = document.createElement('span');
    cols.push(col);
    if (A[i] === B[i]) { col.textContent = B[i]; continue; }
    col.className = 'dg';
    const strip = document.createElement('span');
    strip.className = 'dg-strip';
    if (up) strip.append(glyph(A[i]), glyph(B[i])); else strip.append(glyph(B[i]), glyph(A[i]));
    col.append(strip);
    const delay = (n - 1 - i) * stagger;
    longest = Math.max(longest, delay + duration);
    starts.push(() => {
      strip.animate([{ transform: `translateY(${up ? '0%' : '-50%'})` }, { transform: `translateY(${up ? '-50%' : '0%'})` }],
        { duration, delay, easing: 'cubic-bezier(.3,1.3,.5,1)', fill: 'both' });
      if (A[i] === ' ') col.animate([{ width: '0px' }, { width: 'var(--dg-w)' }], { duration: duration * .7, delay, fill: 'both' });
      if (B[i] === ' ') col.animate([{ width: 'var(--dg-w)' }, { width: '0px' }], { duration: duration * .7, delay, fill: 'both' });
    });
  }
  el.replaceChildren(...cols);
  el.classList.add('rolling');
  starts.forEach(start => start());
  clearTimeout(el._roll);
  el._rollDone = done;
  el._roll = setTimeout(() => endRoll(el), longest + 20);
}
// Finish a roll now: typing, a focus or a fill tick takes over at once.
function endRoll(el) {
  clearTimeout(el._roll);
  if (!el.classList.contains('rolling')) return;
  el.classList.remove('rolling');
  const done = el._rollDone;
  el._rollDone = null;
  if (done) done();
}
// Size the idle setter to its digits so the unit sits right after them.
function fitDelayInput() {
  const i = $('d'), w = Math.max(1, i.value.length) + 'ch';
  if (i.style.width !== w) i.style.width = w;
}
// A typed value past the range settles on the limit shortly after typing
// stops, or at once on Enter or leaving the field (as on the dashboard).
let settleTimer = 0;
function settleDelayInput() {
  clearTimeout(settleTimer);
  const i = $('d');
  if (i.value === '') return;
  const settled = String(inputMs() / 1000);
  if (settled === i.value) return;
  i.value = settled;
  fitDelayInput(); renderCta();
  if (motionOK()) $('cur').animate([0, -5, 5, -3, 3, 0].map(x => ({ transform: `translateX(${x}px)` })), { duration: 320, easing: 'ease-out' });
}
// Show the requested state until the server confirms it. One timer, so a
// quick second click can't have the first click's timer cut it short.
function setPending(p) {
  pending = p; clearTimeout(pendingTimer);
  pendingTimer = setTimeout(() => { pending = null; renderCta(); }, 1500);
  tick();
}
function clearPending() { pending = null; clearTimeout(pendingTimer); }
function fmtRate(k) { if (!k || k <= 0) return ''; return k >= 1000 ? (k / 1000).toFixed(2) + ' Mbps' : Math.round(k) + ' kbps'; }
function fmt(ms) { if (ms < 100) return '0.0'; const x = Math.round(ms / 100) / 10; return x < 10 ? x.toFixed(1) : Math.round(x).toString(); }
// Crossfade helper so the sub-tip swaps smoothly instead of snapping.
function setTip(text) {
  const el = $('w-tip'); if (el.textContent === text) return;
  el.classList.add('fade'); clearTimeout(el._t);
  el._t = setTimeout(() => { el.textContent = text; el.classList.remove('fade'); }, 140);
}

function postArm(ms) { return fetchJ('/arm', { method: 'POST', headers: FORM, body: 'ms=' + ms }); }
async function arm(ms) {
  // Capacity guard, matched by the server's own /arm check. The toast names
  // the buffer size the delay actually needs.
  if (overCap(ms)) { capToast(ms); return; }
  setPending('arming');
  const r = await postArm(ms);
  if (!r.ok) { clearPending(); toast(failText(r, "Couldn't arm the delay"), 'err'); }
  else toast(`Arming ${fmtDelay(ms)}`, 'ok');
  tick();
}
// A new delay while one is engaged. /arm re-arms an armed delay, and moves
// a running one: shorter at once, longer once the buffer has the footage.
async function changeDelay(ms) {
  if (overCap(ms)) { capToast(ms); return; }
  const wasActive = phaseNow() === 'active';
  const r = await postArm(ms);
  if (!r.ok) { toast(failText(r, "Couldn't change the delay"), 'err'); tick(); return; }
  editing = false;
  toast(wasActive ? `Delay moving to ${fmtDelay(ms)}` : `Re-armed at ${fmtDelay(ms)}`, 'ok');
  if (r.j && r.j.phase) applyState(r.j); else tick();
}
async function activate() {
  setPending('active');
  const r = await fetchJ('/activate', { method: 'POST' });
  if (!r.ok) { clearPending(); toast(failText(r, "Can't activate yet"), 'err'); }
  else toast('Delay active', 'ok');
  tick();
}
// Cutting keeps the delay armed (the buffer keeps filling), so the dock
// goes to Activate, not back to Arm.
async function stop() {
  setPending('armed');
  const r = await fetchJ('/stop', { method: 'POST' });
  if (!r.ok) { clearPending(); toast(failText(r, "Couldn't cut the delay. It is still running."), 'err'); }
  else toast('Back to live', 'ok');
  tick();
}
async function disarm() {
  setPending('off');
  const r = await fetchJ('/disarm', { method: 'POST' });
  if (!r.ok) { clearPending(); toast(failText(r, "Couldn't disarm"), 'err'); }
  else toast('Disarmed', 'ok');
  tick();
}

function mainClick() {
  const m = $('main'); if (m._busy || m.disabled || !s) return;
  m._busy = true; setTimeout(() => m._busy = false, 300);
  settleDelayInput();
  const ds = PHASE_TO_STATE[s.phase] || 'off';
  if (editing && isEngaged(ds)) {
    const ms = inputMs();
    if (ms > 0 && ms !== (s.armed_delay_ms || 0)) changeDelay(ms); else endEdit();
    return;
  }
  if (ds === 'active') {
    // Cutting a live delay drops you back to real time instantly - guard it
    // behind a two-tap confirm when the option is on.
    if (cfg.w.activate.confirmcut && !m._cutArm) {
      m._cutArm = true; renderCta();
      clearTimeout(m._cutT); m._cutT = setTimeout(() => { m._cutArm = false; renderCta(); }, 3000);
      return;
    }
    m._cutArm = false; return stop();
  }
  if (ds === 'armed') return activate();
  if (ds === 'arming') return disarm();   // double-click safety: arming primary disarms too
  const ms = inputMs();
  if (ms > 0) arm(ms); else toast('Enter a delay first', 'err');
}
// Scheduled safe cut ("cut after this airs"): mark now, keep forwarding the
// buffered footage, auto-cut to live once the mark has aired everywhere.
// Same endpoints as the dashboard's Cut-after; state carries the countdown.
async function cutAfter() {
  const r = await fetchJ('/cut-after', { method: 'POST' });
  if (!r.ok) toast(failText(r, "Couldn't schedule the cut"), 'err');
  else toast('Mark set: cuts once this airs', 'ok');
  tick();
}
// Ending the hold ends the stream everywhere: the first click arms, a second
// within 3 s ends it (OBS docks can't show confirm dialogs).
async function endHold() {
  const b = $('hold-end');
  if (!b._armed) {
    b._armed = true; b.classList.add('armed'); b.textContent = 'Click again to end';
    b._reset = setTimeout(disarmEndHold, 3000);
    return;
  }
  disarmEndHold();
  const r = await fetchJ('/crash-protection/end', { method: 'POST' });
  if (r.status === 409) toast(r.j?.error || 'Nothing to end', 'info');
  else if (r.ok) toast('Stream ended on every destination', 'ok');
  else toast(failText(r, "Couldn't end the stream"), 'err');
  tick();
}
function disarmEndHold() {
  const b = $('hold-end');
  if (!b._armed) return;
  clearTimeout(b._reset); b._armed = false; b.classList.remove('armed'); b.textContent = 'End now';
}
async function cutAfterCancel() {
  const r = await fetchJ('/cut-after/cancel', { method: 'POST' });
  if (!r.ok) toast(failText(r, "Couldn't cancel the auto-cut. It will still cut."), 'err');
  else toast('Auto-cut cancelled', 'ok');
  tick();
}
// The secondary button next to the main CTA: cancels arming while the buffer
// fills, and schedules / cancels the safe cut while the delay is active.
function secondaryClick() {
  const b = $('cancel'); if (b._busy || !s) return;
  b._busy = true; setTimeout(() => b._busy = false, 300);
  const ds = PHASE_TO_STATE[s.phase] || 'off';
  if (ds === 'active') { if (s.safe_cut_pending) cutAfterCancel(); else cutAfter(); return; }
  disarm();
}

// The big number is an editable setter while idle and a readout once a delay
// is engaged, until a tap opens it for a new value (see startEdit).
function setNumberMode(ds) {
  if (editing && !isEngaged(ds)) editing = false;
  const readout = (ds === 'arming' || isEngaged(ds)) && !editing;
  const controls = cfg.w.number.controls && ds !== 'arming';
  setHidden($('d'), readout);
  setHidden($('v'), !readout);
  setHidden($('minus'), !controls);
  setHidden($('plus'), !controls);
  $('cur').classList.toggle('tap', readout && ds !== 'arming');
  $('w-screen').classList.toggle('editing', editing);
  const cap = $('edit-cap');
  setHidden(cap, !editing);
  // The label is set in capitals; the delay keeps its lowercase unit.
  if (editing && cap._ms !== s.armed_delay_ms) {
    cap._ms = s.armed_delay_ms;
    const now = document.createElement('span');
    now.className = 'edit-cap-num';
    now.textContent = fmtDelay(s.armed_delay_ms || 0);
    cap.replaceChildren('New delay · now ', now);
  }
}

// ------------------------------------------------------------- state paint --

// What the main button says and does in each state. `key` names the state;
// only a new key rebuilds the button (and animates it in), so a label that
// counts (Ready in 0:23, Arm 30s) just updates its text.
function mainLook(ds) {
  const m = $('main');
  const ms = inputMs(), armedMs = s.armed_delay_ms || 0;
  // Over the buffer's reach: the button says so instead of failing on click.
  const capLook = { key: 'cap', label: `Buffer holds only ${fmtDelay(currentCapMs())}`, disabled: true };
  if (editing && isEngaged(ds)) {
    if (ms <= 0 || ms === armedMs) return { key: 'keep', cls: 'ghost', label: `Keep ${fmtDelay(armedMs)}` };
    if (overCap(ms)) return capLook;
    return ds === 'active'
      ? { key: 'adjust', cls: 'primary', icon: '↻', label: `Adjust to ${fmtDelay(ms)}` }
      : { key: 'rearm', cls: 'primary', icon: '↻', label: `Re-arm at ${fmtDelay(ms)}` };
  }
  if (ds === 'active') return { key: 'cut', cls: 'danger' + (m._cutArm ? ' armed' : ''), label: m._cutArm ? 'Tap again to cut' : 'Cut delay' };
  if (ds === 'armed') {
    // Nothing arriving from OBS: /activate would only refuse, so wait.
    if (!s.ingest_alive) return { key: 'wait-armed', label: 'Waiting for OBS', disabled: true };
    const runs = s.capacity_short ? Math.min(armedMs, s.capacity_short.holds_ms) : armedMs;
    return { key: 'armed', cls: 'go', icon: '▶', label: `Activate ${fmtDelay(runs)}` };
  }
  if (ds === 'arming') {
    // The buffer fills in real time, so the wait is exactly what's missing.
    // Right after a click the server may not report the target yet.
    const target = armedMs || ms;
    const fill = Math.min(s.buffer_fill_ms || 0, target);
    const label = s.ingest_alive ? `Ready in ${fmtClock(target - fill)}` : 'Waiting for OBS';
    return { key: 'arming', cls: 'filling', label, disabled: true, p: target > 0 ? fill / target : 0 };
  }
  // Too long for the buffer is said now, not first when OBS connects.
  // Before a bitrate is measured the reach is a typical one: "about".
  if (!s.ingest_alive && ms > 0 && overCap(ms)) {
    return { key: 'wait-cap', label: `Buffer holds about ${fmtDelay(currentCapMs())}`, disabled: true };
  }
  if (!s.ingest_alive) return { key: 'wait', label: 'Waiting for OBS', disabled: true };
  if (ms <= 0) return { key: 'set', label: 'Set a delay first', disabled: true };
  if (overCap(ms)) return capLook;
  return { key: 'arm', cls: 'primary', icon: '⏱', label: `Arm ${fmtDelay(ms)}` };
}
function paintMain(look) {
  const m = $('main');
  setClass(m, 'btn full' + (look.cls ? ' ' + look.cls : ''));
  setDisabled(m, !!look.disabled);
  if (look.p !== undefined) {
    const p = look.p.toFixed(3);
    if (m._p !== p) { m._p = p; m.style.setProperty('--p', p); }
  }
  if (m._key === look.key) { setText(m._label, look.label); return; }
  const first = m._key === undefined;
  m._key = look.key;
  m._label = document.createElement('span');
  m._label.textContent = look.label;
  const parts = [m._label];
  if (look.icon) { const i = document.createElement('span'); i.textContent = look.icon; i.setAttribute('aria-hidden', 'true'); parts.unshift(i); }
  m.replaceChildren(...parts);
  if (first || !motionOK()) return;
  for (const p of parts) p.animate([{ opacity: 0, transform: 'translateY(5px)' }, { opacity: 1, transform: 'none' }], { duration: 220, easing: EASE });
}
// The button next to the main one: cancels arming, disarms, and schedules or
// cancels the safe cut while the delay is active. A pending cut always shows
// even with the option off - it may have been scheduled from the dashboard,
// and hiding it would lie.
function secondaryLook(ds) {
  if (ds === 'arming') return { cls: 'btn ghost disarm', text: '×', title: 'Cancel arming' };
  if (ds === 'armed') return { cls: 'btn ghost disarm', text: '×', title: 'Disarm' };
  if (ds !== 'active') return null;
  if (s.safe_cut_pending) {
    const cd = Math.max(0, Math.round((s.safe_cut_remaining_ms || 0) / 1000));
    return { cls: 'btn ghost cutafter pending', text: `⏱ ~${fmtDelay(cd * 1000)} ✕`, title: 'Auto-cut armed. Tap to cancel.' };
  }
  if (cfg.w.activate.safecut) return { cls: 'btn ghost cutafter', text: '⏱ Cut after', title: 'Cut to live after everything buffered has aired' };
  return null;
}

function renderCta() {
  if (!s) return;
  const m = $('main'), cancel = $('cancel');
  const ds = pending || PHASE_TO_STATE[s.phase] || 'off';
  // Drop a pending cut-confirm if we left the active state by any route, so a
  // stale arm can't skip the guard next time.
  if (ds !== 'active' && m._cutArm) { m._cutArm = false; clearTimeout(m._cutT); }
  // During a crash hold the hold card is the control: there is nothing to
  // arm or cut until OBS is back.
  setHidden($('w-cta'), !cfg.w.activate.on || !!s.hold);
  paintMain(mainLook(ds));
  const sec = secondaryLook(ds);
  setHidden(cancel, !sec);
  if (sec) {
    setClass(cancel, sec.cls); setText(cancel, sec.text); setTitle(cancel, sec.title);
    if (cancel.getAttribute('aria-label') !== sec.title) cancel.setAttribute('aria-label', sec.title);
  }
  if (ds !== 'arming') {
    // Dead-end nudges read as disabled: can't go below 0 or above the cap.
    const v = inputMs() / 1000;
    setDisabled($('minus'), (editing || ds === 'off') && v <= 0);
    setDisabled($('plus'), (editing || ds === 'off') && v >= DELAY_MAX_S);
  }
}

// The phase chip glides to its new width while the new word rises in.
function setPhase(cls, text) {
  const ph = $('ph'), asDot = cfg.w.phase.style === 'dot';
  setClass(ph, 'phase ' + cls + (asDot ? ' asdot' : ''));
  if (ph._text === text) return;
  const glide = ph._text !== undefined && !asDot && !ph.hidden && motionOK();
  const from = glide ? ph.offsetWidth : 0;
  ph._text = text; ph.title = text;
  const word = document.createElement('span');
  word.textContent = text;
  ph.replaceChildren(word);
  if (!glide) return;
  const to = ph.offsetWidth;
  if (from !== to) ph.animate([{ width: from + 'px' }, { width: to + 'px' }], { duration: 320, easing: EASE });
  word.animate([{ opacity: 0, transform: 'translateY(6px)', filter: 'blur(2px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }], { duration: 240, easing: EASE });
}
const PHASE_LOOK = { arming: ['arming', 'Buffering'], armed: ['armed', 'Armed'], active: ['active', 'Active'] };

function paintSource(alive) {
  setClass($('src'), 'pill ' + (alive ? 'on' : 'bad') + (cfg.w.source.style === 'dot' ? ' asdot' : ''));
  setText($('src-lbl'), alive ? 'Live' : 'Offline');
  setTitle($('src'), alive ? 'OBS is sending' : 'Nothing from OBS');
}

// How far the delay can reach: the armed one, or what the buffer holds of
// it when it can't hold all of it (the server measured it: capacity_short).
function reachMs() {
  const tgt = s.armed_delay_ms || 0, short = s.capacity_short;
  return short && short.armed_ms === tgt ? Math.min(tgt, short.holds_ms) : tgt;
}

// Readout: mirror the dashboard: show the armed target while engaged (or
// the fill while arming) and 0 in passthrough. current_delay_ms includes
// the pipeline's own ~1s transit, which would read as a phantom delay.
// Always in seconds, like the dashboard's big number: 900, never 15m.
function paintReadout(ds) {
  const tgt = s.armed_delay_ms || 0, fill = s.buffer_fill_ms || 0;
  // Running short (the buffer can't hold the armed delay): the readout
  // shows what really runs, in amber, like the dashboard.
  const short = isEngaged(ds) && s.capacity_short ? s.capacity_short : null;
  $('cur').classList.toggle('short', !!short);
  const show = ds === 'arming' ? Math.min(fill, reachMs())
    : short ? Math.min(tgt, short.holds_ms)
    : isEngaged(ds) ? tgt : 0;
  const nv = fmt(show);
  if (nv === lastVal) return;
  const was = lastVal;
  lastVal = nv;
  // The roll and the pop mark a change you made (armed, activated, cut),
  // not the buffer counting up while it fills.
  if (ds === 'arming' || !motionOK()) { endRoll($('v')); $('v').textContent = nv; return; }
  rollDigits($('v'), was, nv, () => { $('v').textContent = lastVal; });
  const cur = $('cur');
  cur.classList.add('changing'); clearTimeout(cur._t);
  cur._t = setTimeout(() => cur.classList.remove('changing'), 350);
}

function paintBar(ds) {
  // Toward what the buffer can reach: a bar stuck halfway reads as broken.
  const target = reachMs() || s.buffer_target_ms || 0;
  let pct = 0, cls = 'fill';
  if (ds === 'active') { pct = 100; cls += ' active'; }
  else if (target > 0) {
    pct = Math.min(100, (Math.min(s.buffer_fill_ms, target) / target) * 100);
    if (pct >= 99) cls += ' ready';
  }
  const fill = $('bar'), w = pct.toFixed(1) + '%';
  setClass(fill, cls);
  if (fill._w !== w) { fill._w = w; fill.style.width = w; }
  const pctEl = $('pct');
  setHidden(pctEl, !cfg.w.bar.pct);
  if (cfg.w.bar.pct) setText(pctEl, Math.round(pct) + '%');
}

// Egress glance, honouring its per-part options.
function paintEgress() {
  const eg = $('w-eg'), o = cfg.w.egress, parts = [];
  if (s.ingest_alive && o.on) {
    if (o.dests) parts.push(`<span>&#9650;</span><b>${s.destinations_alive || 0}/${s.destinations_total || 0}</b> live`);
    if (o.bitrate) { const rate = fmtRate(s.stats && s.stats.bitrate_kbps); if (rate) parts.push(`<b>${rate}</b> in`); }
    if (o.codec && s.video_codec) parts.push(`<b>${esc(s.video_codec)}</b>${s.multitrack_video ? ' &middot; MT' : ''}`);
  }
  setHidden(eg, !parts.length);
  if (parts.length) setHtml(eg, parts.join('<span class="sep">&middot;</span>'));
}

// Crash protection: the reconnect screen is holding the destinations.
function paintHold(hold) {
  const tail = !hold && s.tail_ms > 0;
  setHidden($('w-hold'), !hold && !tail);
  if (tail) {
    setText($('hold-why'), 'Delay airing');
    setText($('hold-clock'), fmtClock(s.tail_ms));
    setText($('hold-sub'), 'OBS stopped. The stream ends once your delay has aired.');
    return;
  }
  if (!hold) { disarmEndHold(); return; }
  setText($('hold-why'), hold.reason === 'freeze' ? 'OBS froze' : hold.reason === 'stopped' ? 'OBS stopped' : 'OBS dropped');
  setText($('hold-clock'), fmtClock(hold.remaining_ms));
  const held = (s.destinations || []).filter(d => d.on_hold).length;
  setText($('hold-sub'), held
    ? `${held} destination${held === 1 ? '' : 's'} on the reconnect screen`
    : 'Waiting for OBS');
}

// "4.2s" under a minute, whole seconds past it, for the live-behind hint.
function fmtBehind(ms) { return ms < 60000 ? (ms / 1000).toFixed(1) + 's' : fmtDelay(ms); }
// Hint text: full sentences, or errors-only (encoder offline) if opted.
function paintTip(ds, hold) {
  const tip = hold ? (hold.reason === 'crash' ? 'Reopen OBS in Normal Mode and start streaming to resume.' : 'Start streaming in OBS to resume.')
    : s.tail_ms > 0 ? 'End now cuts it short.'
    : !s.ingest_alive ? 'Start streaming in OBS to InstantClone.'
    : editing ? 'Pick a new delay, then confirm it below.'
    : ds === 'active' && s.safe_cut_pending ? `Auto-cut in ~${fmtDelay(s.safe_cut_remaining_ms || 0)}. Marked footage still airs.`
    : ds === 'active' ? `Live ${fmtBehind(s.current_delay_ms || 0)} behind real time.`
    : ds === 'armed' && s.capacity_short ? `The buffer holds only ${fmtDelay(s.capacity_short.holds_ms)}. Activate runs that much.`
    : ds === 'armed' ? 'Buffer full. Hit Activate to go live with delay.'
    : ds === 'arming' ? 'Filling buffer… you can cancel any time.'
    : 'Passthrough: no delay applied.';
  const show = cfg.w.tips.on && (!cfg.w.tips.errorsOnly || !s.ingest_alive);
  setHidden($('w-tip'), !show);
  if (show) setTip(tip);
}

// The notice line: a dock link the password turns away comes first, then
// an armed delay the buffer can't hold, then an encoder setting a
// destination won't take. The last two dismiss until they change.
let dismissedNote = '';
function currentNote() {
  return (s && s.capacity_short && s.capacity_short.message) || (s && s.compat_warning) || '';
}
// Open or close the notice with a short height-and-fade, from wherever it
// is on screen, so a dismiss folds it away instead of snapping.
const NOTE_CLOSED = { height: '0px', opacity: 0, marginTop: '0px', marginBottom: '0px', paddingTop: '0px', paddingBottom: '0px' };
function foldNote(box, hide) {
  const goal = hide ? 'closed' : 'open';
  if ((box._anim ? box._goal : (box.hidden ? 'closed' : 'open')) === goal) return;
  const snap = () => { const cs = getComputedStyle(box); return Object.fromEntries(Object.keys(NOTE_CLOSED).map(p => [p, cs[p]])); };
  const midway = box._anim ? snap() : null;
  if (box._anim) box._anim.cancel();
  box._anim = null;
  if (!motionOK()) { box.hidden = hide; return; }
  box.hidden = false;
  const open = snap();
  // The gap below it in the dock's column closes with it, so nothing snaps.
  const ps = getComputedStyle(box.parentElement);
  const gap = ps.display.includes('flex') || ps.display.includes('grid') ? parseFloat(ps.rowGap) || 0 : 0;
  const closed = { ...NOTE_CLOSED, marginBottom: -gap + 'px' };
  box.style.overflow = 'hidden';
  box._goal = goal;
  // Fade first, then close (open is the reverse).
  const frames = midway ? [midway, hide ? closed : open]
    : hide ? [open, { opacity: 0, offset: 0.45 }, closed]
    : [closed, { opacity: 0, offset: 0.5 }, open];
  const anim = box.animate(frames, { duration: hide ? 280 : 340, easing: 'cubic-bezier(.4,0,.2,1)' });
  box._anim = anim;
  anim.onfinish = () => {
    if (box._anim !== anim) return;
    box._anim = null;
    box.style.overflow = '';
    box.hidden = hide;
  };
}
function paintNote() {
  const box = $('w-note');
  const note = currentNote();
  const text = authBlocked ? AUTH_MSG : note !== dismissedNote ? note : '';
  foldNote(box, !text);
  if (!text) return;
  setClass(box, 'note' + (authBlocked ? ' bad' : ''));
  setText($('note-text'), text);
  setHidden($('note-x'), authBlocked);
}
$('note-x').onclick = () => { dismissedNote = currentNote(); paintNote(); };

function applyState(j) {
  s = j;
  if (profileGateKey() !== profileGate) renderProfiles();   // phase, delay or buffer reach moved
  const ds = PHASE_TO_STATE[s.phase] || 'off';
  if (pending && pending === ds) clearPending();   // the server caught up
  setNumberMode(ds);
  const [cls, text] = s.hold ? ['holding', 'Holding']
    : s.tail_ms > 0 ? ['holding', 'Ending']
    : PHASE_LOOK[ds] || (s.ingest_alive ? ['live', 'Live'] : ['off', 'Idle']);
  setPhase(cls, text);
  paintSource(!!s.ingest_alive);
  paintNote();
  paintReadout(ds);
  paintBar(ds);
  paintEgress();
  paintHold(s.hold);
  paintTip(ds, s.hold);
  renderDests();
  renderStats();
  renderCta();
  // The events list changed: fetch it.
  if (cfg.w.events.on && s.feed !== eventsSeen) { eventsSeen = s.feed; fetchEvents(); }
}

async function tick() {
  const r = await fetchJ('/state');
  if (!r.ok || !r.j) { paintSource(false); return; }
  applyState(r.j);
}

// ---------------------------------------------------------- destination strip --

// The strip is drawn from the state stream, which carries every destination's
// enabled / alive / bitrate / hold, so it follows toggles made anywhere with
// no polling. Only the platform (for colours and letters) comes from
// /destinations, fetched when the set of destinations changes. Rows are kept
// and patched in place, so an open "Turn off?" survives every update.
let destPlatforms = {};   // id -> platform
let destIdsSeen = '';     // destination ids the platforms were fetched for

// Brand hues so a glance says WHICH platform is live (user feedback: "color
// coded by platform"). Reds/greens tuned for the dark panel; custom/sink
// stay neutral. Applied via a --pc custom property + .pc class.
const PLATFORM_COLORS = {
  twitch: '#9146ff', youtube: '#ff4d4d', kick: '#53fc18', trovo: '#21c26d', restream: '#f27a3f',
};
// Platform marks: the dashboard's own (index.html, kept the same by a
// test). Twitch, YouTube and Kick are their glyphs; relays and local
// targets get a plain symbol; any other platform shows its letter.
const PLATFORM_MARK = {
  twitch: '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M4.3 3 3 6.4V19h4.4v2.4h2.4L12.2 19h3.6l4.9-4.9V3zm14.8 10.4-2.8 2.8h-4.4l-2.4 2.4v-2.4H5.8V4.6h13.3zM15.2 7.3h1.6v4.8h-1.6zm-4.4 0h1.6v4.8h-1.6z"/></svg>',
  youtube: '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path fill-rule="evenodd" d="M21.6 7.2a2.5 2.5 0 0 0-1.8-1.8C18.2 5 12 5 12 5s-6.2 0-7.8.4A2.5 2.5 0 0 0 2.4 7.2 26 26 0 0 0 2 12c0 1.7.1 3.3.4 4.8a2.5 2.5 0 0 0 1.8 1.8c1.6.4 7.8.4 7.8.4s6.2 0 7.8-.4a2.5 2.5 0 0 0 1.8-1.8c.3-1.5.4-3.1.4-4.8s-.1-3.3-.4-4.8zM10 15.1V8.9l5.3 3.1z"/></svg>',
  kick: '<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M4 3h5v5h2V6h2V4h2V3h5v6h-2v2h-2v2h2v2h2v6h-5v-1h-2v-2h-2v-2H9v5H4z"/></svg>',
  restream: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="5" cy="12" r="2"/><path d="M7 12h12M11 12l5-5h3M11 12l5 5h3"/></svg>',
  custom: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="4" y="4" width="16" height="6" rx="1.5"/><rect x="4" y="14" width="16" height="6" rx="1.5"/><path d="M8 7h.01M8 17h.01"/></svg>',
  sink: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></svg>',
};
// Put `slug`'s mark (or `fallback`'s first letter) in `el`, once per change.
function setMark(el, slug, fallback) {
  const key = slug + '|' + fallback;
  if (el._mark === key) return;
  el._mark = key;
  if (PLATFORM_MARK[slug]) el.innerHTML = PLATFORM_MARK[slug];
  else el.textContent = (slug || fallback || '?').slice(0, 1).toUpperCase();
}
async function fetchDestPlatforms() {
  const r = await fetchJ('/destinations');
  if (!r.ok || !Array.isArray(r.j)) return;
  destPlatforms = {};
  for (const d of r.j) destPlatforms[d.id] = d.platform || '';
  renderDests();
}
// off, streaming, on but not streaming yet (connecting, or waiting for the
// delay to fill), on the reconnect screen, or on and waiting for OBS.
function destState(d) {
  if (!d.enabled) return 'off';
  if (d.on_hold) return 'held';
  if (d.alive && (d.bitrate_kbps || 0) > 0) return 'alive';
  return s.ingest_alive ? 'wait' : 'idle';
}
function destColor(el, d) {
  const pc = (cfg.w.dest.brand && PLATFORM_COLORS[destPlatforms[d.id]]) || '';
  if (el._pc !== pc) { el._pc = pc; if (pc) el.style.setProperty('--pc', pc); else el.style.removeProperty('--pc'); }
  return pc ? ' pc' : '';
}
function renderDests() {
  const box = $('w-dests'); if (box.hidden || !s) return;
  const all = s.destinations || [];
  const ids = all.map(d => d.id).join();
  if (ids !== destIdsSeen) { destIdsSeen = ids; fetchDestPlatforms(); }
  const icons = cfg.w.dest.view === 'icons';
  // Show every configured destination by default so a disabled one can be
  // toggled back on. "Active only" is the opt-in filter for a clean look.
  const show = cfg.w.dest.active ? all.filter(d => d.enabled || d.alive) : all;
  const layout = (icons ? 'icons:' : 'rows:') + show.map(d => d.id).join() + ':' + all.length;
  if (box._layout !== layout) {
    box._layout = layout;
    box.className = 'dests' + (icons ? ' icons' : '');
    box._items = new Map();
    if (!show.length) {
      box.innerHTML = `<div class="empty">${all.length ? 'No active destinations right now.' : 'No destinations yet. Add them in the dashboard.'}</div>`;
      return;
    }
    box.replaceChildren(...show.map(d => {
      const el = icons ? destIcon() : destRow();
      box._items.set(d.id, el);
      return el;
    }));
  }
  for (const d of show) {
    const el = box._items.get(d.id);
    if (icons) patchDestIcon(el, d); else patchDestRow(el, d);
  }
}
function destRow() {
  const row = document.createElement('div');
  row.innerHTML = '<span class="d-mark"><span class="d-glyph"></span><span class="d-dot"></span></span><span class="d-name"></span><span class="d-plat"></span>';
  row._glyph = row.children[0].firstChild; row._name = row.children[1]; row._meta = row.children[2];
  row._sw = document.createElement('button');
  row._sw.setAttribute('role', 'switch');
  row._sw.onclick = () => onDestToggle(row);
  row.appendChild(row._sw);
  return row;
}
function patchDestRow(row, d) {
  row._d = d;
  const st = destState(d), platform = destPlatforms[d.id] || '';
  setClass(row, 'dest ' + st + destColor(row, d));
  setMark(row._glyph, platform, d.name || '');
  setText(row._name, d.name || 'Destination');
  const rate = cfg.w.dest.stats && st === 'alive' ? fmtRate(d.bitrate_kbps) : '';
  setClass(row._meta, rate ? 'd-rate' : 'd-plat');
  // A failing destination says so; why, and the next try, on hover. The
  // last reason is kept while it connects again, never once it is live.
  const fail = st === 'wait' ? d.failure : null;
  setText(row._meta, st === 'held' ? 'Holding' : st === 'wait' ? (fail ? 'Can\'t connect' : 'Connecting…') : rate || platform);
  setTitle(row, fail ? fail.advice + (fail.retry_in_ms != null ? ` Next try in ${Math.ceil(fail.retry_in_ms / 1000)}s.` : '') : '');
  setClass(row._sw, 'sw' + (d.enabled ? ' on' : ''));
  setTitle(row._sw, d.enabled ? 'Turn off' : 'Turn on');
  const checked = String(!!d.enabled);
  if (row._sw.getAttribute('aria-checked') !== checked) row._sw.setAttribute('aria-checked', checked);
  if (row._sw.getAttribute('aria-label') !== (d.name || 'Destination')) row._sw.setAttribute('aria-label', d.name || 'Destination');
}
// Compact circular badge: the platform's mark, live-glow ring, hover shows
// the name (native title). Tap arms it (confirm), tap again commits.
function destIcon() {
  const b = document.createElement('button');
  b.onclick = () => onDestIcon(b);
  return b;
}
function patchDestIcon(b, d) {
  b._d = d;
  if (b._armed) return;   // keep "tap again" on screen until it resolves
  const st = destState(d);
  const look = st === 'alive' ? ' alive' : st === 'wait' || st === 'held' ? ' wait' : '';
  setClass(b, 'dico ' + (d.enabled ? 'on' : 'off') + look + destColor(b, d));
  setMark(b, destPlatforms[d.id] || '', d.name || '');
  setTitle(b, `${d.name || 'Destination'} · ${d.enabled ? 'on' : 'off'}`);
  if (b.getAttribute('aria-label') !== b.title) b.setAttribute('aria-label', b.title);
}
function onDestIcon(b) {
  const d = b._d;
  if (!cfg.w.dest.confirm) { commitDest(d, !d.enabled); return; }
  if (b._armed) { disarmDestIcon(b); commitDest(d, !d.enabled); return; }
  // Arm: pulse an amber ring and swap the badge to a check so the "tap again
  // to confirm" intent is unmistakable, not just a colour change.
  b._armed = true;
  b.classList.add('arm'); b.textContent = '✓'; b._mark = '';
  b.title = d.enabled ? 'Tap again to turn OFF' : 'Tap again to go LIVE';
  b._t = setTimeout(() => disarmDestIcon(b), 3000);
}
function disarmDestIcon(b) {
  clearTimeout(b._t); b._armed = false;
  patchDestIcon(b, b._d);
}
// Misclick guard: the row swaps its switch for a Sure? / cancel pair that
// auto-dismisses after 3s. Toggling a live platform either way matters, so
// both directions are guarded.
function onDestToggle(row) {
  const d = row._d;
  if (!cfg.w.dest.confirm) { commitDest(d, !d.enabled); return; }
  if (row._confirm) return;
  const next = !d.enabled;
  const box = document.createElement('span'); box.className = 'confirm';
  box.innerHTML = `<span class="q">${next ? 'Turn on?' : 'Turn off?'}</span>`;
  const yes = document.createElement('button'); yes.className = 'cbtn yes'; yes.innerHTML = '&#10003;'; yes.setAttribute('aria-label', 'Confirm');
  const no = document.createElement('button'); no.className = 'cbtn no'; no.innerHTML = '&times;'; no.setAttribute('aria-label', 'Cancel');
  const cancel = () => { clearTimeout(row._ct); row._confirm = false; box.replaceWith(row._sw); };
  yes.onclick = () => { cancel(); commitDest(d, next); };
  no.onclick = cancel;
  box.append(yes, no);
  row._sw.replaceWith(box); row._confirm = true; row._ct = setTimeout(cancel, 3000);
  yes.focus();
}
async function commitDest(d, enabled) {
  const r = await fetchJ('/destinations/toggle', {
    method: 'POST', headers: FORM,
    body: `id=${encodeURIComponent(d.id)}&enabled=${enabled ? 'on' : 'off'}`,
  });
  if (!r.ok) toast(failText(r, `Couldn't turn ${d.name || 'it'} ${enabled ? 'on' : 'off'}`), 'err');
  else toast(`${d.name || 'Destination'} ${enabled ? 'on' : 'off'}`, 'ok');
}

// --------------------------------------------------------- extra widgets ----

// Delay profiles: one-tap chips from the user's saved profiles (same list as
// the dashboard). Idle, a tap picks the delay (and arms it with "Apply on
// tap"); armed or running, it moves to that delay the same way.
let profileCache = [];
async function fetchProfiles() {
  const r = await fetchJ('/profiles');
  if (r.ok && Array.isArray(r.j)) { profileCache = r.j; renderProfiles(); }
}
function profileTitle(name, ms, ds, isCur) {
  if (isCur) return `${name}: running now`;
  if (ds === 'arming') return 'Wait for the buffer to fill, or cancel arming';
  const apply = cfg.w.profiles.arm;
  if (ds === 'active') return apply ? `Adjust to ${fmtDelay(ms)}` : `Pick ${fmtDelay(ms)}, then Adjust`;
  if (ds === 'armed') return apply ? `Re-arm at ${fmtDelay(ms)}` : `Pick ${fmtDelay(ms)}, then Re-arm`;
  return apply ? `Arm ${fmtDelay(ms)}` : `Set ${fmtDelay(ms)}`;
}
function renderProfiles() {
  profileGate = profileGateKey();
  const box = $('w-profiles'); if (box.hidden) return;
  const list = cfg.w.profiles.view === 'list';
  box.className = 'profiles' + (list ? ' plist' : '');
  if (!profileCache.length) { box.innerHTML = '<div class="empty">No profiles yet. Add them in the dashboard.</div>'; return; }
  const ds = phaseNow(), target = s?.armed_delay_ms || 0;
  box.innerHTML = '';
  for (const p of profileCache) {
    const ms = p.delay_ms || 0, name = p.name || fmtDelay(ms);
    // Too big for the buffer at the current bitrate: greyed + disabled, with
    // a tooltip naming the size it needs. Same wall the arm path enforces.
    const tooBig = overCap(ms);
    const isCur = ds !== 'off' && ms === target;
    const b = document.createElement('button');
    b.className = (list ? 'prow' : 'pchip') + (tooBig ? ' toobig' : '') + (isCur ? ' on' : '');
    b.disabled = tooBig || isCur || ds === 'arming';
    b.title = tooBig ? capAdvice(ms) : profileTitle(name, ms, ds, isCur);
    if (isCur) b.setAttribute('aria-current', 'true');
    b.innerHTML = list
      ? `<span>${esc(name)}</span><span class="pv">${fmtDelay(ms)}</span>`
      : esc(name) + (cfg.w.profiles.value ? `<span class="pv">${fmtDelay(ms)}</span>` : '');
    b.onclick = () => profileSet(ms);
    box.appendChild(b);
  }
}
function profileSet(ms) {
  if (overCap(ms)) { capToast(ms); return; }
  const ds = phaseNow(), sec = Math.round(ms / 1000);
  if (ds === 'arming') return;
  if (isEngaged(ds)) {
    if (ms === (s.armed_delay_ms || 0)) return;
    if (cfg.w.profiles.arm) { changeDelay(ms); return; }
    startEdit(false);
  }
  setDelayField(sec);
  if (ds === 'off' && cfg.w.profiles.arm && sec > 0) arm(ms);
}

// Health stats: CPU / RAM / reconnects, plus a backpressure warning. All read
// from the state stream, so no extra polling.
function renderStats() {
  const box = $('w-stats'); if (box.hidden || !s) return;
  const o = cfg.w.stats, st = s.stats || {}, tiles = o.view === 'tiles';
  setClass(box, 'stats' + (tiles ? ' tiles' : ''));
  const items = [];
  if (o.cpu) items.push([(s.cpu_pct || 0).toFixed(0) + '%', 'cpu']);
  if (o.mem) items.push([Math.round((s.rss_bytes || 0) / 1048576), 'MB']);
  if (o.bitrate) { const rate = fmtRate(st.bitrate_kbps); if (rate) items.push([rate, 'in']); }
  if (o.recon) items.push([(st.egress_reconnects || 0) + (st.ingest_disconnects || 0), 'recon']);
  if (o.cuts) items.push([st.cuts || 0, 'cuts']);
  let html = items.map(([v, l]) => tiles
    ? `<span class="tile"><b>${v}</b><span>${l}</span></span>`
    : `<span class="st"><b>${v}</b> ${l}</span>`).join('');
  if (s.backpressure) html += '<span class="st bad">&#9888; buffer lag</span>';
  setHtml(box, html || '<span class="st muted">no metrics selected</span>');
}

// The dock's own switches (Auto behavior + Settings widgets), read and
// written through /dock/settings: a fixed set of on/off settings the dock
// token may change even with a dashboard password on. auth_enabled tells
// the dock whether the links it copies need the token.
let srvCfg = { auto_arm_on_connect: false, auto_activate_when_ready: false, tracing_enabled: false, 'crash_protection.enabled': false, auth_enabled: false };
let srvLoaded = false;
async function fetchServerCfg() {
  const r = await fetchJ('/dock/settings');
  if (!r.ok || !r.j) return;
  srvCfg = { ...srvCfg, ...r.j }; srvLoaded = true;
  renderBehavior(); renderSettings();
}
function paintSwitch(sw, on) {
  sw.className = 'sw sm' + (on ? ' on' : '');
  sw.setAttribute('aria-checked', String(on));
}
// A label + switch row bound to one dock switch.
function toggleRow(label, key, hint) {
  const row = document.createElement('div'); row.className = 'brow';
  if (hint) row.title = hint;
  const lb = document.createElement('span'); lb.className = 'blbl'; lb.textContent = label;
  const sw = document.createElement('button'); sw.type = 'button'; sw.setAttribute('role', 'switch'); sw.setAttribute('aria-label', label);
  paintSwitch(sw, !!srvCfg[key]);
  sw.disabled = !srvLoaded;   // never flip a switch whose real state is unknown
  sw.onclick = () => setServer(key, !srvCfg[key], sw, label);
  row.append(lb, sw);
  return row;
}
async function setServer(key, val, sw, label) {
  srvCfg[key] = val; paintSwitch(sw, val); sw.disabled = true;
  const r = await fetchJ('/dock/settings', { method: 'POST', headers: FORM, body: `${encodeURIComponent(key)}=${val}` });
  sw.disabled = false;
  if (!r.ok) {
    srvCfg[key] = !val; paintSwitch(sw, !val);
    toast(failText(r, `Couldn't change ${label}`), 'err');
    return;
  }
  if (r.j) srvCfg = { ...srvCfg, ...r.j };
  toast(`${label} ${val ? 'on' : 'off'}`, 'ok');
}
function renderBehavior() {
  const box = $('w-behavior'); if (box.hidden) return;
  box.replaceChildren(
    toggleRow('Pre-arm on stream start', 'auto_arm_on_connect', 'Arms your Default delay (System › General) when OBS starts streaming'),
    toggleRow('Auto-activate when ready', 'auto_activate_when_ready', 'Every arm goes on air once the buffer is full'));
}

// Settings: the live-safe knobs (things that don't need a restart). Buffer /
// ports stay in the dashboard because changing them restarts the pipeline.
function renderSettings() {
  const box = $('w-settings'); if (box.hidden) return;
  box.replaceChildren(
    toggleRow('Crash protection', 'crash_protection.enabled', 'If OBS drops, viewers see a reconnect screen instead of the stream ending'),
    toggleRow('Trace log', 'tracing_enabled', 'A detailed log for bug reports. Leave it off otherwise'));
  // Discord, chat and phone alerts are integrations, set up in the
  // dashboard: they take secret links, awkward to paste into a dock.
  const ig = document.createElement('div'); ig.className = 'brow';
  const lb = document.createElement('span'); lb.className = 'blbl'; lb.textContent = 'Discord, chat and phone alerts';
  const btns = document.createElement('span'); btns.className = 'srow-btns';
  const open = document.createElement('button'); open.className = 'minibtn'; open.textContent = 'Set up'; open.onclick = () => confirmOpenDashboard(btns);
  btns.append(open);
  ig.append(lb, btns); box.appendChild(ig);
}
function confirmOpenDashboard(btns) {
  btns.innerHTML = '';
  const q = document.createElement('span'); q.className = 'cq'; q.textContent = 'Open dashboard?';
  q.title = 'Opens the Integrations tab of the InstantClone dashboard in a new tab. With a password on, sign in there first.';
  const yes = document.createElement('button'); yes.className = 'minibtn'; yes.textContent = 'Yes';
  yes.onclick = () => { window.open(location.origin + '/#integrations', '_blank'); toast('Opening dashboard…', 'ok'); renderSettings(); };
  const no = document.createElement('button'); no.className = 'minibtn'; no.textContent = 'No'; no.onclick = renderSettings;
  btns.append(q, yes, no);
  yes.focus();
}

// Overlays: list the OBS browser-source overlays and copy their URLs, so a
// dock can double as an overlay picker.
let overlayCache = [];
let overlayQuery = '';
async function fetchOverlays() {
  const r = await fetchJ('/overlays');
  if (r.ok && Array.isArray(r.j)) { overlayCache = r.j; renderOverlays(); }
}
function renderOverlays() {
  const box = $('w-overlays'); if (box.hidden) return;
  box.innerHTML = '';
  if (!overlayCache.length) { box.innerHTML = '<div class="empty">No overlays yet. Open the dashboard once to seed the presets.</div>'; return; }
  // The search field is built once and left in place; only the list below it
  // re-renders on keystroke, so the input keeps focus + caret.
  if (cfg.w.overlays.search) {
    const inp = document.createElement('input'); inp.className = 'osearch'; inp.placeholder = 'Search overlays…'; inp.value = overlayQuery;
    inp.setAttribute('aria-label', 'Search overlays');
    inp.oninput = () => { overlayQuery = inp.value; renderOverlayList(); };
    box.appendChild(inp);
  }
  const list = document.createElement('div'); list.className = 'olist'; list.id = 'olist';
  box.appendChild(list);
  renderOverlayList();
}
function renderOverlayList() {
  const list = $('olist'); if (!list) return;
  list.innerHTML = '';
  let items = overlayCache;
  if (cfg.w.overlays.search && overlayQuery) {
    const q = overlayQuery.toLowerCase();
    items = items.filter(o => (o.name || o.slug).toLowerCase().includes(q));
  }
  if (!items.length) { list.innerHTML = '<div class="empty">No overlays match.</div>'; return; }
  if (cfg.w.overlays.group) {
    const studio = items.filter(o => o.studio), presets = items.filter(o => !o.studio);
    if (studio.length) { list.appendChild(overlayGroup('Studio')); for (const o of studio) list.appendChild(overlayRow(o)); }
    if (presets.length) { list.appendChild(overlayGroup('Presets')); for (const o of presets) list.appendChild(overlayRow(o)); }
  } else {
    for (const o of items) list.appendChild(overlayRow(o));
  }
}
function overlayGroup(text) { const h = document.createElement('div'); h.className = 'ogroup'; h.textContent = text; return h; }
function overlayRow(o) {
  const row = document.createElement('div'); row.className = 'orow';
  const nm = document.createElement('span'); nm.className = 'o-name'; nm.textContent = o.name || o.slug;
  if (o.studio && !cfg.w.overlays.group) { const tag = document.createElement('span'); tag.className = 'o-tag'; tag.textContent = 'studio'; nm.appendChild(tag); }
  const copy = document.createElement('button'); copy.className = 'minibtn'; copy.textContent = 'Copy URL';
  copy.title = 'Copy the browser-source URL'; copy.onclick = () => copyOverlay(o.slug);
  row.append(nm, copy);
  return row;
}
function overlayUrl(slug) {
  // The overlay files are served at /overlay/<slug>.html (matches the
  // dashboard). ?autohide=off keeps the overlay up even while live (some
  // presets otherwise fade out); opt-in via the widget option.
  const q = cfg.w.overlays.autohide ? '?autohide=off' : '';
  return `${location.origin}/overlay/${encodeURIComponent(slug)}.html${q}`;
}
async function copyOverlay(slug) {
  // Overlay URLs carry no secret, so a failed copy can show the URL itself.
  const url = overlayUrl(slug);
  if (await copyText(url)) toast('Overlay URL copied', 'ok'); else toast(url, 'info');
}

// Events: what integrations list here with a "Dock event" step (the Twitch
// alerts module lists each follow, sub, cheer and raid). Show one group,
// play an event's alert again, or skip the alert on stream now.
let eventCache = [], eventsSeen = -1;
// Not reached (integrations starting, a blip): the widget still shows, and
// asks again a moment later.
const EVENTS_RETRY_MS = 5000;
let eventsRetry = 0;
async function fetchEvents() {
  clearTimeout(eventsRetry);
  const r = await fetchJ('/dock/events');
  if (r.ok && Array.isArray(r.j)) eventCache = r.j;
  else eventsRetry = setTimeout(() => { if (cfg.w.events.on) fetchEvents(); }, EVENTS_RETRY_MS);
  renderEvents();
}
// The row at the top of what's scrolled to, and how far past it: kept
// where it was when the list is drawn again (newest first, so new events
// would push it down). At the very top, the newest stay in view.
function eventsAnchor(list) {
  if (!list || !list.scrollTop) return null;
  const row = [...list.children].find(r => r.offsetTop - list.offsetTop + r.offsetHeight > list.scrollTop);
  return row && { id: row.dataset.id, past: list.scrollTop - (row.offsetTop - list.offsetTop) };
}
function renderEvents() {
  const box = $('w-events'); if (box.hidden) return;
  const o = cfg.w.events;
  const groups = [...new Set(eventCache.map(e => e.group).filter(Boolean))];
  const only = groups.includes(o.only) ? o.only : '';
  const anchor = eventsAnchor(box.querySelector('.elist'));
  box.innerHTML = '';
  const head = document.createElement('div'); head.className = 'ehead';
  if (groups.length > 1) {
    const chips = document.createElement('div'); chips.className = 'echips';
    chips.setAttribute('role', 'group'); chips.setAttribute('aria-label', 'Show');
    for (const g of ['', ...groups]) {
      const b = document.createElement('button'); b.type = 'button';
      b.className = 'echip' + (g === only ? ' on' : ''); b.textContent = g || 'All';
      b.setAttribute('aria-pressed', String(g === only));
      b.onclick = () => { cfg.w.events.only = g; saveAndApply(); };
      chips.appendChild(b);
    }
    head.appendChild(chips);
  }
  if (o.skip) head.appendChild(miniBtn('Skip alert', skipAlert, 'End the alert on stream now: the next one comes on'));
  if (head.children.length) box.appendChild(head);
  // Newest first; the list scrolls past a few.
  const shown = eventCache.filter(e => !only || e.group === only).slice(-50).reverse();
  if (!shown.length) {
    box.insertAdjacentHTML('beforeend', '<div class="empty">No events yet. The Twitch alerts module lists each one here; a "Dock event" step adds your own.</div>');
    return;
  }
  const list = document.createElement('div'); list.className = 'elist';
  for (const e of shown) list.appendChild(eventRow(e, !only));
  box.appendChild(list);
  const kept = anchor && [...list.children].find(r => r.dataset.id === anchor.id);
  if (kept) list.scrollTop = kept.offsetTop - list.offsetTop + anchor.past;
}
function eventRow(e, tagged) {
  const row = document.createElement('div'); row.className = 'erow'; row.dataset.id = String(e.id);
  if (cfg.w.events.times) {
    const t = document.createElement('span'); t.className = 'e-time';
    t.textContent = new Date(e.at_ms).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    row.appendChild(t);
  }
  const text = document.createElement('span'); text.className = 'e-text'; text.textContent = e.text; text.title = e.text;
  row.appendChild(text);
  if (tagged && e.group) { const tag = document.createElement('span'); tag.className = 'o-tag'; tag.textContent = e.group; row.appendChild(tag); }
  if (e.replay) row.appendChild(miniBtn('Replay', () => replayEvent(e.id), 'Play its alert on stream again'));
  return row;
}
async function replayEvent(id) {
  const r = await fetchJ('/dock/events/replay', { method: 'POST', headers: FORM, body: 'id=' + id });
  if (r.ok) toast('Playing it again', 'ok'); else toast(failText(r, "Couldn't play it again"), 'err');
}
async function skipAlert() {
  const r = await fetchJ('/dock/alerts/skip', { method: 'POST', headers: FORM, body: '' });
  if (r.ok) toast('Alert skipped', 'ok'); else toast(failText(r, "Couldn't skip it"), 'err');
}

// -------------------------------------------------------------- layout apply --

function applyCfg(c) {
  cfg = c;
  const on = id => c.w[id].on;
  document.body.classList.toggle('compact', c.density === 'compact');
  // Accent hue (CSS custom properties drive every accent-colored surface).
  const [accent, ink] = ACCENTS[c.accent] || ACCENTS.cyan;
  document.documentElement.style.setProperty('--accent', accent);
  document.documentElement.style.setProperty('--accent-ink', ink);
  // Status bar: hidden if switched off, or if both of its parts are off (no
  // empty bar left taking space).
  $('w-top').hidden = !(on('status') && (on('phase') || on('source')));
  $('ph').hidden = !on('phase');       // phase chip within it
  $('src').hidden = !on('source');     // live pill within it
  // The number screen wraps the readout and the bar. The bar is a child of
  // the number (its toggle lives under the Number row), so turning the number
  // off hides the whole screen, bar included.
  $('w-screen').hidden = !on('number');
  $('w-screen').className = 'screen sz-' + (c.w.number.size || 'md') + (editing ? ' editing' : '');
  $('cur').classList.toggle('tint', !!c.w.number.tint);
  $('w-bar').hidden = !on('bar');
  $('w-bar').classList.toggle('thick', !!c.w.bar.thick);
  $('w-bar').classList.toggle('mono', !!c.w.bar.mono);
  for (const id of ['egress', 'tips', 'dest', 'profiles', 'stats', 'behavior', 'settings', 'overlays', 'events', 'activate']) $(WEL[id]).hidden = !on(id);
  document.querySelectorAll('.cur .u').forEach(u => u.hidden = !c.w.number.units);
  // Apply the saved drag order by re-appending containers in sequence.
  const wrap = document.querySelector('.wrap');
  for (const id of c.order) { const el = $(WEL[id]); if (el) wrap.appendChild(el); }
  // The crash hold card takes the place of the delay controls it stands in
  // for. A dock without them (destinations only, overlays) shows it on top,
  // so every dock can end the stream without losing its own purpose. The
  // notice line sits right below it, above the controls it is about.
  const holdAnchor = on('number') ? $('w-screen') : on('activate') ? $('w-cta') : wrap.firstElementChild;
  if (holdAnchor !== $('w-hold')) wrap.insertBefore($('w-hold'), holdAnchor);
  wrap.insertBefore($('w-note'), $('w-hold').nextSibling);
  if (on('profiles')) fetchProfiles();
  if (on('behavior') || on('settings')) { renderBehavior(); renderSettings(); fetchServerCfg(); }
  if (on('overlays')) fetchOverlays();
  if (on('events')) { eventsSeen = s ? s.feed : -1; fetchEvents(); }
  if (s) applyState(s); else renderCta();   // re-render with the new options
}

function dockKey(id) { return 'ic.dock.' + id; }
function saveLayout(id, json) {
  return fetchJ('/docks/' + encodeURIComponent(id), { method: 'POST', headers: { 'content-type': 'application/json' }, body: json });
}
// Mirror a layout into localStorage too: every OBS dock shares one browser
// profile, so a dock showing that slot adopts it live (the storage event).
function mirrorLayout(id, json) {
  try { if (json) localStorage.setItem(dockKey(id), json); else localStorage.removeItem(dockKey(id)); } catch (_) {}
}
async function persist() {
  const json = JSON.stringify(cfg);
  mirrorLayout(SLOT, json);
  const r = await saveLayout(SLOT, json);
  if (!r.ok) toast(failText(r, "Couldn't save this layout"), 'err');
}
function saveAndApply() { applyCfg(cfg); persist(); }

async function loadCfg() {
  // 1) A ?cfg= blob (shared / imported URL) wins and is saved to this slot.
  const p = params.get('cfg');
  if (p) {
    try {
      cfg = mergeCfg(JSON.parse(decodeURIComponent(escape(atob(p)))));
      applyCfg(cfg); persist();
      const u = new URL(location.href); u.searchParams.delete('cfg'); history.replaceState(null, '', u);
      return;
    } catch (_) {}
  }
  // 2) localStorage mirror: instant paint before the server answers.
  try { const ls = localStorage.getItem(dockKey(SLOT)); if (ls) { cfg = mergeCfg(JSON.parse(ls)); applyCfg(cfg); } } catch (_) {}
  // 3) Server copy is the durable source of truth; reconcile when it lands.
  const r = await fetchJ('/docks/' + encodeURIComponent(SLOT));
  if (r.ok && r.j && typeof r.j === 'object') {
    cfg = mergeCfg(r.j); applyCfg(cfg);
    mirrorLayout(SLOT, JSON.stringify(cfg));
  }
}

// -------------------------------------------------------------- gear editor --

let dragId = null;
// Which widget rows are expanded. Kept across the frequent buildEditor()
// rebuilds so flipping an option doesn't snap its panel shut.
const openRows = new Set();

function ensureEditorRoot() {
  let el = $('editor');
  if (!el) {
    el = document.createElement('div'); el.id = 'editor'; el.className = 'editor'; el.hidden = true;
    el.setAttribute('role', 'dialog'); el.setAttribute('aria-modal', 'true'); el.setAttribute('aria-label', 'Customize dock');
    el.innerHTML = '<div class="sheet"><div class="grabber"></div><div class="sheet-body" id="sheet-body"></div></div>';
    el.onclick = e => { if (e.target === el) closeEditor(); };
    document.body.appendChild(el);
  }
  return el;
}
function openEditor() {
  const root = ensureEditorRoot(); buildEditor(); root.hidden = false;
  requestAnimationFrame(() => root.classList.add('show'));
  fetchSlots();   // populate the Docks list
  root.querySelector('.eclose').focus();
}
function closeEditor() {
  const root = ensureEditorRoot(); root.classList.remove('show');
  dockUi = { open: '', form: null, confirm: '' };
  setTimeout(() => root.hidden = true, 220);   // let the slide-out finish
  $('gear').focus();
}

// A row of segmented buttons (the "mode" control). `current` is the selected
// value; `items` is [value, label] pairs; `onPick(val)` applies the choice.
function segControl(current, items, onPick) {
  const seg = document.createElement('div'); seg.className = 'seg';
  for (const [val, lab] of items) {
    const b = document.createElement('button'); b.type = 'button';
    const sel = String(current) === String(val);
    b.className = 'segb' + (sel ? ' sel' : ''); b.textContent = lab;
    b.setAttribute('aria-pressed', String(sel));
    b.onclick = () => onPick(val);
    seg.appendChild(b);
  }
  return seg;
}
function optionEls(id) {
  const specs = OPTS[id]; if (!specs) return null;
  const box = document.createElement('div'); box.className = 'opts';
  for (const sp of specs) {
    const t = sp.w || id;
    const row = document.createElement('div'); row.className = 'optrow';
    const lb = document.createElement('span'); lb.className = 'optlbl'; lb.textContent = sp.label; row.appendChild(lb);
    if (sp.modes || sp.sel) {
      const items = sp.modes || sp.sel.map(v => [v, String(v)]);
      row.appendChild(segControl(cfg.w[t][sp.k], items, val => { cfg.w[t][sp.k] = val; cfg.preset = 'custom'; saveAndApply(); buildEditor(); }));
    } else {
      const sw = document.createElement('button'); sw.type = 'button'; sw.setAttribute('role', 'switch'); sw.setAttribute('aria-label', sp.label);
      paintSwitch(sw, !!cfg.w[t][sp.k]);
      sw.onclick = () => { cfg.w[t][sp.k] = !cfg.w[t][sp.k]; cfg.preset = 'custom'; saveAndApply(); buildEditor(); };
      row.appendChild(sw);
    }
    box.appendChild(row);
  }
  return box;
}
function widgetRow(id) {
  const w = cfg.w[id];
  const row = document.createElement('div'); row.className = 'wrow' + (w.on ? '' : ' off'); row.dataset.id = id;
  const head = document.createElement('div'); head.className = 'head';
  // Drag starts only from the grip, so tapping the row still expands options.
  const grip = document.createElement('span'); grip.className = 'grip'; grip.innerHTML = '&#8942;&#8942;'; grip.title = 'Drag to reorder';
  grip.onmousedown = () => { row.draggable = true; };
  row.ondragstart = e => { dragId = id; row.classList.add('dragging'); e.dataTransfer.effectAllowed = 'move'; try { e.dataTransfer.setData('text/plain', id); } catch (_) {} };
  row.ondragend = () => { row.draggable = false; row.classList.remove('dragging'); document.querySelectorAll('.wrow.drop').forEach(x => x.classList.remove('drop')); };
  row.ondragover = e => { if (dragId && dragId !== id) { e.preventDefault(); row.classList.add('drop'); } };
  row.ondragleave = () => row.classList.remove('drop');
  row.ondrop = e => { e.preventDefault(); row.classList.remove('drop'); if (dragId) reorder(dragId, id); };
  const nm = document.createElement('span'); nm.className = 'nm'; nm.textContent = WIDGETS[id].label;
  const sw = document.createElement('button'); sw.type = 'button'; sw.className = 'sw' + (w.on ? ' on' : '');
  sw.setAttribute('role', 'switch'); sw.setAttribute('aria-checked', String(!!w.on)); sw.setAttribute('aria-label', WIDGETS[id].label);
  sw.onclick = e => { e.stopPropagation(); w.on = !w.on; cfg.preset = 'custom'; saveAndApply(); buildEditor(); };
  const opts = optionEls(id);
  head.append(grip, nm);
  if (opts) {
    row.classList.add('has-opts');
    const cv = document.createElement('span'); cv.className = 'cv'; cv.innerHTML = '&#8250;'; cv.setAttribute('aria-hidden', 'true'); head.appendChild(cv);
    if (openRows.has(id)) row.classList.add('open');
    head.onclick = e => {
      if (e.target === sw || e.target === grip) return;
      if (openRows.has(id)) openRows.delete(id); else openRows.add(id);
      row.classList.toggle('open');
    };
  }
  head.appendChild(sw);
  row.appendChild(head); if (opts) row.appendChild(opts);
  return row;
}
function sectionLabel(text, inline) {
  const el = document.createElement('div'); el.className = 'lbl' + (inline ? ' inline' : ''); el.textContent = text; return el;
}
function buildEditor() {
  const body = $('sheet-body'); if (!body) return;
  body.innerHTML = '';
  const head = document.createElement('div'); head.className = 'ehead';
  head.innerHTML = `<h3>Customize dock</h3><span class="slot" title="?dock=${esc(SLOT)}">${esc(slotName(SLOT))}</span>`;
  const x = document.createElement('button'); x.className = 'eclose'; x.innerHTML = '&times;'; x.title = 'Done'; x.setAttribute('aria-label', 'Close'); x.onclick = closeEditor;
  head.appendChild(x); body.appendChild(head);

  body.appendChild(sectionLabel('Preset'));
  const chips = document.createElement('div'); chips.className = 'chips';
  for (const [k, p] of Object.entries(PRESETS)) {
    const c = document.createElement('button'); c.type = 'button'; c.className = 'chip' + (cfg.preset === k ? ' sel' : ''); c.textContent = p.label;
    c.setAttribute('aria-pressed', String(cfg.preset === k));
    c.onclick = () => applyPreset(k); chips.appendChild(c);
  }
  body.appendChild(chips);

  body.appendChild(sectionLabel('Appearance'));
  const densRow = document.createElement('div'); densRow.className = 'densrow';
  densRow.appendChild(sectionLabel('Density', true));
  // Density + accent are orthogonal to the preset, so they don't mark it custom.
  densRow.appendChild(segControl(cfg.density, [['comfy', 'Comfy'], ['compact', 'Compact']], val => { cfg.density = val; saveAndApply(); buildEditor(); }));
  body.appendChild(densRow);
  const accRow = document.createElement('div'); accRow.className = 'densrow';
  accRow.appendChild(sectionLabel('Accent', true));
  const sw = document.createElement('div'); sw.className = 'swatches';
  for (const key of Object.keys(ACCENTS)) {
    const b = document.createElement('button'); b.type = 'button'; b.className = 'swatch' + (cfg.accent === key ? ' sel' : '');
    b.style.setProperty('--sw', ACCENTS[key][0]); b.title = key; b.setAttribute('aria-label', key + ' accent');
    b.setAttribute('aria-pressed', String(cfg.accent === key));
    b.onclick = () => { cfg.accent = key; saveAndApply(); buildEditor(); };
    sw.appendChild(b);
  }
  accRow.appendChild(sw); body.appendChild(accRow);

  body.appendChild(sectionLabel('Widgets · drag to reorder'));
  const list = document.createElement('div'); list.className = 'wlist';
  for (const id of cfg.order) list.appendChild(widgetRow(id));
  body.appendChild(list);

  body.appendChild(sectionLabel('Docks'));
  const docks = document.createElement('div'); docks.id = 'docks-box';
  body.appendChild(docks);
  renderDocks();

  const act = document.createElement('div'); act.className = 'ed-actions';
  // Escape hatch for a messed-up layout: two-tap reset back to defaults. The
  // dock keeps its name, which is not part of the look.
  const reset = document.createElement('button'); reset.className = 'btn ghost reset'; reset.textContent = 'Reset';
  reset.onclick = () => {
    if (!reset._arm) {
      reset._arm = true; reset.textContent = 'Sure?'; reset.classList.add('warn');
      setTimeout(() => { reset._arm = false; reset.textContent = 'Reset'; reset.classList.remove('warn'); }, 3000);
      return;
    }
    openRows.clear(); cfg = { ...defaultCfg(), name: cfg.name }; saveAndApply(); buildEditor(); toast('Dock reset to defaults', 'ok');
  };
  const copy = document.createElement('button'); copy.className = 'btn ghost'; copy.innerHTML = '<span aria-hidden="true">&#128279;</span><span>Copy link</span>';
  copy.onclick = () => copyDockLink(SLOT);
  const done = document.createElement('button'); done.className = 'btn primary'; done.textContent = 'Done'; done.onclick = closeEditor;
  act.append(reset, copy, done); body.appendChild(act);
}
function applyPreset(k) {
  const p = PRESETS[k]; if (!p) return;
  for (const id of ALL) cfg.w[id].on = p.on.includes(id);
  cfg.preset = k; saveAndApply(); buildEditor();
}
// Move `from` to just before `to` in the row order.
function reorder(from, to) {
  if (from === to) return;
  const o = cfg.order.filter(x => x !== from);
  o.splice(o.indexOf(to), 0, from);
  cfg.order = o; cfg.preset = 'custom'; saveAndApply(); buildEditor();
}

// ------------------------------------------------------------------ docks ----
// Every saved layout is a dock: one OBS Custom Browser Dock loads one by its
// link (?dock=<id>). The id in a link never changes once OBS has it, so a
// rename only changes the display name stored in the layout.

let knownSlots = [];            // saved slot ids, from the server
let slotLayouts = new Map();    // other slot id -> its saved layout
// What the Docks list shows: which row is open, the New / Rename form, and a
// pending two-step confirm ("delete:<id>" or "replace:<id>").
let dockUi = { open: '', form: null, confirm: '' };

async function fetchSlots() {
  const r = await fetchJ('/docks');
  if (!r.ok || !Array.isArray(r.j)) return;
  knownSlots = r.j;
  const others = knownSlots.filter(id => id !== SLOT);
  const got = await Promise.all(others.map(id => fetchJ('/docks/' + encodeURIComponent(id))));
  slotLayouts = new Map(others.map((id, i) => [id, got[i].ok && got[i].j && typeof got[i].j === 'object' ? got[i].j : null]));
  renderDocks();
}
function slotName(id) {
  const name = id === SLOT ? cfg.name : cleanName(slotLayouts.get(id)?.name);
  return name || (id === 'default' ? 'Main dock' : id);
}
function slotCfg(id) { return id === SLOT ? cfg : mergeCfg(slotLayouts.get(id)); }
function dockUrl(id) {
  return `${location.origin}/dock?dock=${encodeURIComponent(id)}${TOKEN ? '&token=' + TOKEN : ''}`;
}
// A dock's own link. It carries the access token when this dock has it; a
// dock let in by its cookie alone hands out links that work in this OBS.
async function copyDockLink(id, done) {
  if (!(await copyText(dockUrl(id)))) { toast("Couldn't copy the link. Click into the dock and try again.", 'err'); return; }
  const tail = srvCfg.auth_enabled && !TOKEN ? ' It works in this OBS.' : '';
  toast((done || `Link to "${slotName(id)}" copied.`) + tail, 'ok');
}
// A fresh slot id from a name: readable in the link, unique among saved docks.
function slotIdFor(name) {
  const base = name.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9_-]/g, '').replace(/-+/g, '-')
    .replace(/^-|-$/g, '').slice(0, 32) || 'dock';
  const taken = new Set([SLOT, ...knownSlots]);
  let id = base, n = 2;
  while (taken.has(id)) id = `${base}-${n++}`;
  return id;
}
async function createDock(name, layout) {
  name = cleanName(name);
  if (!name) { toast('Give the dock a name', 'err'); return false; }
  const id = slotIdFor(name);
  const copy = { ...JSON.parse(JSON.stringify(layout)), name };
  const json = JSON.stringify(copy);
  const r = await saveLayout(id, json);
  if (!r.ok) { toast(failText(r, "Couldn't create the dock"), 'err'); return false; }
  mirrorLayout(id, json);
  knownSlots = [...knownSlots, id]; slotLayouts.set(id, copy);
  dockUi = { open: id, form: null, confirm: '' };
  renderDocks();
  await copyDockLink(id, `"${name}" created, link copied. Add it in OBS: Docks > Custom Browser Docks.`);
  return true;
}
async function renameDock(id, name) {
  name = cleanName(name);
  dockUi.form = null;
  if (name === cleanName(slotCfg(id).name)) { renderDocks(); return; }
  if (id === SLOT) { cfg.name = name; saveAndApply(); buildEditor(); toast(name ? `Renamed to "${name}"` : 'Name cleared', 'ok'); return; }
  const next = { ...slotCfg(id), name }, json = JSON.stringify(next);
  const r = await saveLayout(id, json);
  if (!r.ok) { toast(failText(r, "Couldn't rename the dock"), 'err'); renderDocks(); return; }
  mirrorLayout(id, json); slotLayouts.set(id, next);
  renderDocks(); toast(name ? `Renamed to "${name}"` : 'Name cleared', 'ok');
}
// Give another dock this dock's look, keeping its own name.
async function copyLayoutTo(id) {
  const next = { ...JSON.parse(JSON.stringify(cfg)), name: cleanName(slotCfg(id).name) }, json = JSON.stringify(next);
  const r = await saveLayout(id, json);
  if (!r.ok) { toast(failText(r, "Couldn't copy the layout"), 'err'); return; }
  mirrorLayout(id, json); slotLayouts.set(id, next);
  renderDocks(); toast(`"${slotName(id)}" now uses this layout`, 'ok');
}
async function deleteDock(id) {
  const name = slotName(id);
  const r = await saveLayout(id, '');
  if (!r.ok) { toast(failText(r, "Couldn't delete the dock"), 'err'); return; }
  mirrorLayout(id, '');
  knownSlots = knownSlots.filter(x => x !== id); slotLayouts.delete(id);
  dockUi = { open: '', form: null, confirm: '' };
  renderDocks(); toast(`Deleted "${name}". Remove its dock in OBS too.`, 'ok');
}
// Two-step for anything that overwrites or deletes (OBS docks can't show a
// confirm dialog): the first tap turns the button red for 3 seconds.
function confirmStep(btn, key, label, run) {
  if (dockUi.confirm === key) { dockUi.confirm = ''; clearTimeout(btn._t); run(); return; }
  dockUi.confirm = key;
  btn.textContent = label; btn.classList.add('warn');
  btn._t = setTimeout(() => { if (dockUi.confirm === key) { dockUi.confirm = ''; renderDocks(); } }, 3000);
}
function miniBtn(text, onClick, title) {
  const b = document.createElement('button'); b.type = 'button'; b.className = 'minibtn'; b.textContent = text;
  if (title) b.title = title;
  b.onclick = onClick;
  return b;
}
// An inline text field (prompt() is unreliable inside OBS's browser dock).
function nameField(value, placeholder, onEnter, onEscape) {
  const inp = document.createElement('input'); inp.className = 'dfield'; inp.maxLength = 40;
  inp.value = value; inp.placeholder = placeholder; inp.setAttribute('aria-label', placeholder);
  inp.onkeydown = e => {
    if (e.key === 'Enter') { e.preventDefault(); onEnter(inp.value); }
    else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); onEscape(); }
  };
  return inp;
}
function dockRow(id) {
  const isThis = id === SLOT, open = dockUi.open === id;
  const row = document.createElement('div'); row.className = 'drow' + (isThis ? ' this' : '');
  const head = document.createElement('div'); head.className = 'dhead';
  const renaming = dockUi.form?.kind === 'rename' && dockUi.form.id === id;
  if (renaming) {
    const cancel = () => { dockUi.form = null; renderDocks(); };
    const inp = nameField(cleanName(slotCfg(id).name), slotName(id), v => renameDock(id, v), cancel);
    head.append(inp, miniBtn('Save', () => renameDock(id, inp.value)), miniBtn('Cancel', cancel));
    row.appendChild(head);
    requestAnimationFrame(() => { inp.focus(); inp.select(); });
    return row;
  }
  const nm = document.createElement('span'); nm.className = 'dname'; nm.title = `?dock=${id}`;
  nm.innerHTML = `<span class="dn">${esc(slotName(id))}</span>${isThis ? '<span class="dtag">This dock</span>' : ''}`;
  const more = document.createElement('button'); more.type = 'button'; more.className = 'dmore'; more.innerHTML = '&#8943;';
  more.title = 'More'; more.setAttribute('aria-label', `More for ${slotName(id)}`); more.setAttribute('aria-expanded', String(open));
  more.onclick = () => { dockUi = { open: open ? '' : id, form: null, confirm: '' }; renderDocks(); };
  head.append(nm, miniBtn('Copy link', () => copyDockLink(id), 'Copy the link for an OBS Custom Browser Dock'), more);
  row.appendChild(head);
  if (!open) return row;
  const acts = document.createElement('div'); acts.className = 'dacts';
  const full = knownSlots.length >= DOCKS_MAX;
  acts.appendChild(miniBtn('Rename', () => { dockUi.form = { kind: 'rename', id }; renderDocks(); }));
  const dup = miniBtn('Duplicate', () => createDock(`${slotName(id)} copy`, slotCfg(id)), 'A new dock with the same layout');
  dup.disabled = full;
  acts.appendChild(dup);
  if (!isThis) {
    const use = miniBtn('Use this layout', null, `Give "${slotName(id)}" the layout of this dock`);
    use.onclick = () => confirmStep(use, 'replace:' + id, 'Replace its layout?', () => copyLayoutTo(id));
    const del = miniBtn('Delete', null);
    del.onclick = () => confirmStep(del, 'delete:' + id, 'Tap again to delete', () => deleteDock(id));
    acts.append(use, del);
  }
  row.appendChild(acts);
  return row;
}
function newDockForm() {
  const f = dockUi.form;
  const box = document.createElement('div'); box.className = 'dform';
  const cancel = () => { dockUi.form = null; renderDocks(); };
  const create = async () => {
    const btn = box.querySelector('.minibtn.ok'); btn.disabled = true;
    const ok = await createDock(inp.value, f.from === 'this' ? cfg : defaultCfg());
    if (!ok) btn.disabled = false;
  };
  const inp = nameField(f.name, 'Dock name, like Scenes', create, cancel);
  inp.oninput = () => { f.name = inp.value; };
  const r1 = document.createElement('div'); r1.className = 'frow'; r1.appendChild(inp);
  const r2 = document.createElement('div'); r2.className = 'frow';
  r2.append(segControl(f.from, [['this', 'This layout'], ['defaults', 'Defaults']], v => { f.from = v; f.name = inp.value; renderDocks(); }));
  const btns = document.createElement('span'); btns.className = 'srow-btns';
  const go = miniBtn('Create', create); go.classList.add('ok');
  btns.append(miniBtn('Cancel', cancel), go);
  r2.appendChild(btns);
  box.append(r1, r2);
  requestAnimationFrame(() => inp.focus());
  return box;
}
function renderDocks() {
  const box = $('docks-box'); if (!box) return;
  const list = document.createElement('div'); list.className = 'dlist';
  for (const id of [SLOT, ...knownSlots.filter(x => x !== SLOT)]) list.appendChild(dockRow(id));
  const parts = [list];
  if (dockUi.form?.kind === 'new') parts.push(newDockForm());
  else {
    const add = document.createElement('button'); add.type = 'button'; add.className = 'dnew'; add.textContent = '+ New dock';
    add.disabled = knownSlots.length >= DOCKS_MAX;
    if (add.disabled) add.title = `${DOCKS_MAX} docks is the most InstantClone keeps. Delete one first.`;
    add.onclick = () => { dockUi = { open: '', form: { kind: 'new', name: '', from: 'this' }, confirm: '' }; renderDocks(); };
    parts.push(add);
  }
  const hint = document.createElement('div'); hint.className = 'hint';
  hint.textContent = 'Each OBS browser dock shows one layout. Make a new dock here, then paste its link into OBS under Docks > Custom Browser Docks.';
  parts.push(hint);
  box.replaceChildren(...parts);
}

// -------------------------------------------------------------------- boot --

// Typing or focusing the field ends a roll at once, so it never hides
// what is being typed.
$('d').addEventListener('focus', () => endRoll($('d-roll')));
$('d').addEventListener('input', () => {
  endRoll($('d-roll'));
  fitDelayInput(); renderCta();
  clearTimeout(settleTimer); settleTimer = setTimeout(settleDelayInput, 700);
});
// Enter in the delay field = press the main button. Esc puts back the
// running delay. Keyboard-first flow.
$('d').addEventListener('keydown', e => {
  if (e.key === 'Enter') { settleDelayInput(); mainClick(); }
  else if (e.key === 'Escape' && editing) { e.preventDefault(); endEdit(); }
});
// Leaving the field unchanged while picking a new delay closes the picker,
// unless focus went to the controls that finish the job.
$('d').addEventListener('blur', () => {
  settleDelayInput();
  setTimeout(() => {
    if (!editing || !s || inputMs() !== (s.armed_delay_ms || 0)) return;
    const a = document.activeElement;
    if (a && (a.id === 'minus' || a.id === 'plus' || a.id === 'main' || a.id === 'd')) return;
    endEdit();
  }, 150);
});
$('v').addEventListener('click', () => startEdit(true));
$('v').addEventListener('keydown', e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); startEdit(true); } });
document.addEventListener('keydown', e => {
  if (e.key !== 'Escape') return;
  const ed = $('editor');
  if (ed && !ed.hidden) closeEditor(); else endEdit();
});
// Staggered first paint (see body.boot CSS); the class comes off after one
// pass so later saves/reorders never replay the entrance.
document.body.classList.add('boot');
setTimeout(() => document.body.classList.remove('boot'), 800);
fitDelayInput();
applyCfg(cfg);   // paint defaults immediately
loadCfg();       // then hydrate from url / localStorage / server
fetchServerCfg();   // the switches, and whether a password is on

// Prefer SSE for state; fall back to 500ms polling if it is missing or drops.
let _pollTimer = null;
function startPolling() { if (_pollTimer) return; tick(); _pollTimer = setInterval(tick, 500); }
function stopPolling() { if (_pollTimer) { clearInterval(_pollTimer); _pollTimer = null; } }
let _es = null;
function startSSE() {
  if (!window.EventSource) { startPolling(); return; }
  try {
    _es = new EventSource('/events');
    _es.onmessage = ev => { try { applyState(JSON.parse(ev.data)); stopPolling(); } catch (_) {} };
    _es.onerror = () => { try { _es.close(); } catch (_) {} _es = null; startPolling(); };
  } catch (_) { startPolling(); }
}
startSSE();

// Same-slot layout sync: OBS docks share one browser profile, so when
// another instance of this slot saves (persist writes localStorage), the
// storage event fires here and we adopt the new layout live - no reload.
window.addEventListener('storage', e => {
  if (e.key !== dockKey(SLOT) || !e.newValue) return;
  try {
    const next = mergeCfg(JSON.parse(e.newValue));
    if (JSON.stringify(next) !== JSON.stringify(cfg)) applyCfg(next);
  } catch (_) {}
});

// Aux data (profiles, overlays, server switches, destinations) has no push
// channel, so refresh it whenever the dock regains focus or visibility -
// e.g. after editing profiles in the dashboard and clicking back into OBS.
function refreshAux() {
  if (cfg.w.profiles.on) fetchProfiles();
  if (cfg.w.overlays.on) fetchOverlays();
  fetchServerCfg();
  if (cfg.w.dest.on) fetchDestPlatforms();
}
window.addEventListener('focus', () => { if (!_es) tick(); refreshAux(); });
document.addEventListener('visibilitychange', () => { if (!document.hidden) refreshAux(); });
