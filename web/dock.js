// InstantClone OBS dock: a configurable stack of widgets driven by a saved
// layout. The layout is per-slot (?dock=<id>), persisted server-side so it
// survives OBS clearing its browser cache, with a localStorage mirror for
// instant paint. dock.html holds the markup+CSS; this file is all behaviour.

const $ = id => document.getElementById(id);
const PHASE_TO_STATE = { idle: 'off', preparing: 'arming', ready: 'armed', active: 'active' };
const EASE = 'cubic-bezier(.32,.72,0,1)';
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

async function fetchJ(u, o) {
  try {
    const r = await fetch(u, o);
    const j = r.headers.get('content-type')?.includes('json') ? await r.json() : null;
    return { ok: r.ok, status: r.status, j };
  } catch (_) { return { ok: false, status: 0, j: null }; }
}
function toast(msg, kind) {
  const t = $('toast'); t.className = 'show ' + (kind || ''); t.textContent = msg;
  clearTimeout(t._h); t._h = setTimeout(() => t.className = '', 1800);
}
function esc(v) {
  return String(v).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}

// ---------------------------------------------------------------- config ----

// The dock slot this page is (?dock=<id>). Sanitised to match the server's
// allowed key charset so a hand-typed slot can round-trip to storage.
const SLOT = ((new URLSearchParams(location.search).get('dock') || 'default')
  .replace(/[^a-zA-Z0-9_-]/g, '').slice(0, 40)) || 'default';

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
  activate: { label: 'Action button', def: { on: true, confirmcut: false, safecut: true } },
};
// Every widget that has stored options. phase + source are sub-widgets of the
// status bar; bar rides along with the number screen it lives in.
const ALL = ['status', 'phase', 'source', 'number', 'bar', 'egress', 'tips', 'dest', 'profiles', 'stats', 'behavior', 'settings', 'overlays', 'activate'];
// Reorderable rows = the top-level containers the user can drag around.
const ROWS = ['status', 'number', 'profiles', 'egress', 'tips', 'dest', 'stats', 'behavior', 'settings', 'overlays', 'activate'];
// DOM container per row.
const WEL = {
  status: 'w-top', number: 'w-screen', profiles: 'w-profiles', egress: 'w-eg', tips: 'w-tip', dest: 'w-dests',
  stats: 'w-stats', behavior: 'w-behavior', settings: 'w-settings', overlays: 'w-overlays', activate: 'w-cta',
};
// Presets = which widgets are on. Options keep their defaults / last tweak.
const ALL_ON = ['status', 'phase', 'source', 'number', 'bar', 'egress', 'tips', 'dest', 'profiles', 'stats', 'behavior', 'settings', 'overlays', 'activate'];
const PRESETS = {
  full:      { label: 'Full',         on: ['status', 'phase', 'source', 'number', 'bar', 'egress', 'tips', 'dest', 'activate'] },
  minimal:   { label: 'Minimal',      on: ['status', 'phase', 'source', 'number', 'activate'] },
  delay:     { label: 'Delay only',   on: ['number', 'activate'] },
  dest:      { label: 'Destinations', on: ['status', 'phase', 'source', 'dest'] },
  control:   { label: 'Control',      on: ['status', 'source', 'behavior', 'settings', 'overlays'] },
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
    { k: 'arm', label: 'Arm on tap' }, { k: 'value', label: 'Show seconds' },
  ],
  stats:  [
    { k: 'view', label: 'Layout', modes: [['row', 'Row'], ['tiles', 'Tiles']] },
    { k: 'cpu', label: 'CPU' }, { k: 'mem', label: 'RAM' }, { k: 'recon', label: 'Reconnects' },
    { k: 'bitrate', label: 'Bitrate' }, { k: 'cuts', label: 'Cuts' },
  ],
  overlays: [
    { k: 'search', label: 'Search bar' }, { k: 'group', label: 'Folders' }, { k: 'autohide', label: 'Copy with autohide off' },
  ],
  activate: [{ k: 'safecut', label: 'Cut after airs' }, { k: 'confirmcut', label: 'Confirm cut' }],
};

function defaultCfg() {
  const w = {};
  for (const id of ALL) w[id] = { ...WIDGETS[id].def };
  return { v: 1, preset: 'full', density: 'comfy', accent: 'cyan', order: [...ROWS], w };
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
// fill at the current bitrate silently stalls in "arming" forever, which
// reads as a bug. Mirror the dashboard's estimate so the dock greys out (and
// refuses to arm) delays the buffer can't hold. Soft estimate: ~160 kbps
// audio, ~5% RTMP framing, a little headroom, and a peak-hold on bitrate so
// the gate doesn't flicker a chip in/out on a momentary dip. The server
// enforces the same wall on /arm, so this is UX, not the last line of defence.
const BUF_AUDIO_KBPS = 160, BUF_OVERHEAD_PCT = 5, BUF_HEADROOM_PCT = 3, BUF_REF_KBPS = 10000;
let bufferMB = 0;          // persisted buffer size, from /config
let bitrateHoldKbps = 0;   // worst-case recent bitrate (peak-hold, slow decay)
function updateBitrateHold() {
  const m = s && s.stats && s.stats.bitrate_kbps;
  if (!m || m <= 1000) return;                         // no signal - keep last hold
  bitrateHoldKbps = Math.max(m, (bitrateHoldKbps || m) * 0.985);
}
function planningKbps() {
  if (bitrateHoldKbps > 1000) return bitrateHoldKbps;
  const m = s && s.stats && s.stats.bitrate_kbps;
  return (m && m > 1000) ? m : BUF_REF_KBPS;           // plan for 10 Mbps before we know
}
function capMsFor(mb, kbps) {
  if (!mb || mb <= 0) return 0;
  const total = (Math.max(1, kbps) + BUF_AUDIO_KBPS) * (1 + BUF_OVERHEAD_PCT / 100);
  return Math.floor((mb * 8192) / total * (1 - BUF_HEADROOM_PCT / 100)) * 1000;
}
function currentCapMs() { return bufferMB > 0 ? capMsFor(bufferMB, planningKbps()) : 0; }
function requiredMB(ms) {
  if (!ms || ms <= 0) return 0;
  const total = (planningKbps() + BUF_AUDIO_KBPS) * (1 + BUF_OVERHEAD_PCT / 100);
  return Math.ceil((ms / 1000) * total / (1 - BUF_HEADROOM_PCT / 100) / 8192);
}
function overCap(ms) { const c = currentCapMs(); return c > 0 && ms > c; }
async function fetchBufferMB() {
  const r = await fetchJ('/config');
  if (r.ok && r.j && typeof r.j.buffer_mb === 'number') bufferMB = r.j.buffer_mb;
  renderProfiles();   // re-gate chips now that capacity is known
}

// ---------------------------------------------------------- delay controls --

function bump(n) {
  const step = cfg.w.number.step || 1;
  const i = $('d');
  i.value = Math.max(0, Math.min(600, (+i.value || 0) + n * step));
  fitDelayInput(); renderCta();
}
// Size the idle setter to its digits so the unit sits right after them.
function fitDelayInput() {
  const i = $('d'), w = Math.max(1, i.value.length) + 'ch';
  if (i.style.width !== w) i.style.width = w;
}
// Show the requested state until the server confirms it. One timer, so a
// quick second click can't have the first click's timer cut it short.
function setPending(p) {
  pending = p; clearTimeout(pendingTimer);
  pendingTimer = setTimeout(() => { pending = null; renderCta(); }, 1500);
  tick();
}
function fmtRate(k) { if (!k || k <= 0) return ''; return k >= 1000 ? (k / 1000).toFixed(2) + ' Mbps' : Math.round(k) + ' kbps'; }
function fmt(ms) { if (ms < 100) return '0.0'; const x = Math.round(ms / 100) / 10; return x < 10 ? x.toFixed(1) : Math.round(x).toString(); }
// Crossfade helper so the sub-tip swaps smoothly instead of snapping.
function setTip(text) {
  const el = $('w-tip'); if (el.textContent === text) return;
  el.classList.add('fade'); clearTimeout(el._t);
  el._t = setTimeout(() => { el.textContent = text; el.classList.remove('fade'); }, 140);
}

async function arm(ms) {
  // Capacity guard, matched by the server's own /arm check. Skip for ms===0
  // (disarm). The toast names the buffer size the delay actually needs.
  if (ms > 0 && overCap(ms)) {
    toast(`Buffer too small - ${(ms / 1000).toFixed(0)}s needs about ${requiredMB(ms)} MB. Raise it in the dashboard.`, 'err');
    return;
  }
  setPending('arming');
  const r = await fetchJ('/arm', { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: 'ms=' + ms });
  if (!r.ok) { toast(r.j?.error || 'Failed to arm', 'err'); pending = null; }
  else toast(`Arming ${(ms / 1000).toFixed(0)}s`, 'ok');
  tick();
}
async function activate() {
  setPending('active');
  const r = await fetchJ('/activate', { method: 'POST' });
  if (!r.ok) { toast(r.j?.error || 'Cannot activate yet', 'err'); pending = null; }
  else toast('Delay active', 'ok');
  tick();
}
// Cutting keeps the delay armed (the buffer keeps filling), so the dock
// goes to Activate, not back to Arm.
async function stop() { setPending('armed'); await fetchJ('/stop', { method: 'POST' }); toast('Back to live', 'ok'); tick(); }
async function disarm() { setPending('off'); await fetchJ('/disarm', { method: 'POST' }); toast('Disarmed', 'ok'); tick(); }

function mainClick() {
  const m = $('main'); if (m._busy || m.disabled) return;
  m._busy = true; setTimeout(() => m._busy = false, 300);
  if (!s) return;
  const ds = PHASE_TO_STATE[s.phase] || 'off';
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
  const ms = (+$('d').value || 0) * 1000;
  if (ms > 0) arm(ms); else toast('Enter a delay', 'err');
}
// Scheduled safe cut ("cut after this airs"): mark now, keep forwarding the
// buffered footage, auto-cut to live once the mark has aired everywhere.
// Same endpoints as the dashboard's Cut-after; state carries the countdown.
async function cutAfter() {
  const r = await fetchJ('/cut-after', { method: 'POST' });
  if (!r.ok) toast(r.j?.error || 'Cannot schedule cut', 'err');
  else toast('Mark set - cuts once this airs', 'ok');
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
  else toast(r.ok ? 'Stream ended on every destination' : 'Could not end the stream', r.ok ? 'ok' : 'err');
  tick();
}
function disarmEndHold() {
  const b = $('hold-end');
  if (!b._armed) return;
  clearTimeout(b._reset); b._armed = false; b.classList.remove('armed'); b.textContent = 'End now';
}
async function cutAfterCancel() {
  await fetchJ('/cut-after/cancel', { method: 'POST' });
  toast('Auto-cut cancelled', 'ok');
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
// is engaged - the +/- retire so the screen reads clean (the feedback ask).
function setNumberMode(ds) {
  const engaged = ds === 'arming' || ds === 'armed' || ds === 'active';
  const controls = cfg.w.number.controls && !engaged;
  setHidden($('d'), engaged);
  setHidden($('v'), !engaged);
  setHidden($('minus'), !controls);
  setHidden($('plus'), !controls);
}

// ------------------------------------------------------------- state paint --

// What the main button says and does in each state. `key` names the state;
// only a new key rebuilds the button (and animates it in), so a label that
// counts (Ready in 0:23, Arm 30s) just updates its text.
function mainLook(ds) {
  const m = $('main');
  if (ds === 'active') return { key: 'cut', cls: 'danger', label: m._cutArm ? 'Tap again to cut' : 'Cut delay' };
  if (ds === 'armed') return { key: 'armed', cls: 'go', icon: '▶', label: 'Activate' };
  if (ds === 'arming') {
    // The buffer fills in real time, so the wait is exactly what's missing.
    // Right after a click the server may not report the target yet.
    const target = s.armed_delay_ms || (+$('d').value || 0) * 1000;
    const fill = Math.min(s.buffer_fill_ms || 0, target);
    const label = s.ingest_alive ? `Ready in ${fmtClock(target - fill)}` : 'Waiting for OBS';
    return { key: 'arming', cls: 'filling', label, disabled: true, p: target > 0 ? fill / target : 0 };
  }
  const sec = +$('d').value || 0;
  if (!s.ingest_alive) return { key: 'wait', label: 'Waiting for OBS', disabled: true };
  if (sec <= 0) return { key: 'set', label: 'Set a delay first', disabled: true };
  return { key: 'arm', cls: 'primary', icon: '⏱', label: `Arm ${sec.toFixed(0)}s` };
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
  if (look.icon) { const i = document.createElement('span'); i.textContent = look.icon; parts.unshift(i); }
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
    return { cls: 'btn ghost cutafter pending', text: `⏱ ~${cd}s ✕`, title: 'Auto-cut armed - tap to cancel' };
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
  if (sec) { setClass(cancel, sec.cls); setText(cancel, sec.text); setTitle(cancel, sec.title); }
  if (ds === 'off') {
    // Dead-end nudges read as disabled: can't go below 0 or above the cap.
    const v = +$('d').value || 0;
    setDisabled($('minus'), v <= 0);
    setDisabled($('plus'), v >= 600);
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
  setTitle($('src'), alive ? 'Live' : 'Offline');
}

// Readout: mirror the dashboard - show the armed target while engaged (or
// the fill while arming) and 0 in passthrough. current_delay_ms includes
// the pipeline's own ~1s transit, which would read as a phantom delay.
function paintReadout(ds) {
  const tgtSec = (s.armed_delay_ms || 0) / 1000, fillSec = (s.buffer_fill_ms || 0) / 1000;
  const showSec = ds === 'arming' ? Math.min(fillSec, tgtSec) : (ds === 'armed' || ds === 'active') ? tgtSec : 0;
  const nv = fmt(showSec * 1000);
  if (nv === lastVal) return;
  lastVal = nv; setText($('v'), nv);
  // The pop marks a change you made (armed, activated, cut), not the
  // buffer counting up while it fills.
  if (ds === 'arming' || !motionOK()) return;
  const cur = $('cur');
  cur.classList.add('changing'); clearTimeout(cur._t);
  cur._t = setTimeout(() => cur.classList.remove('changing'), 350);
}

function paintBar(ds) {
  const target = s.armed_delay_ms || s.buffer_target_ms || 0;
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
    setText($('hold-sub'), 'OBS stopped. Ends when viewers reach the end');
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

// Hint text: full sentences, or errors-only (encoder offline) if opted.
function paintTip(ds, hold) {
  const tip = hold ? (hold.reason === 'crash' ? 'Reopen OBS (Run in Normal Mode) and start streaming to resume.' : 'Start streaming in OBS to resume.')
    : s.tail_ms > 0 ? 'End now cuts it short.'
    : !s.ingest_alive ? 'Point your encoder at rtmp://…/live'
    : ds === 'active' && s.safe_cut_pending ? `Auto-cut in ~${Math.max(0, Math.round((s.safe_cut_remaining_ms || 0) / 1000))}s - marked footage still airs.`
    : ds === 'active' ? `Live ${(s.current_delay_ms / 1000).toFixed(1)}s behind real time.`
    : ds === 'armed' ? 'Buffer full. Hit Activate to go live with delay.'
    : ds === 'arming' ? 'Filling buffer… you can cancel any time.'
    : 'Passthrough - no delay applied.';
  const show = cfg.w.tips.on && (!cfg.w.tips.errorsOnly || !s.ingest_alive);
  setHidden($('w-tip'), !show);
  if (show) setTip(tip);
}

function applyState(j) {
  s = j;
  updateBitrateHold();   // feed the capacity peak-hold before anything reads it
  const ds = PHASE_TO_STATE[s.phase] || 'off';
  if (pending && pending === ds) { pending = null; clearTimeout(pendingTimer); }   // the server caught up
  setNumberMode(ds);
  const [cls, text] = s.hold ? ['holding', 'Holding']
    : s.tail_ms > 0 ? ['holding', 'Ending']
    : PHASE_LOOK[ds] || (s.ingest_alive ? ['live', 'Live'] : ['off', 'Idle']);
  setPhase(cls, text);
  paintSource(!!s.ingest_alive);
  paintReadout(ds);
  paintBar(ds);
  paintEgress();
  paintHold(s.hold);
  paintTip(ds, s.hold);
  renderDests();
  renderStats();
  renderCta();
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
  row.innerHTML = '<span class="d-dot"></span><span class="d-name"></span><span class="d-plat"></span>';
  row._name = row.children[1]; row._meta = row.children[2];
  row._sw = document.createElement('button');
  row._sw.onclick = () => onDestToggle(row);
  row.appendChild(row._sw);
  return row;
}
function patchDestRow(row, d) {
  row._d = d;
  const st = destState(d), platform = destPlatforms[d.id] || '';
  setClass(row, 'dest ' + st + destColor(row, d));
  setText(row._name, d.name || 'Destination');
  const rate = cfg.w.dest.stats && st === 'alive' ? fmtRate(d.bitrate_kbps) : '';
  setClass(row._meta, rate ? 'd-rate' : 'd-plat');
  setText(row._meta, st === 'held' ? 'Holding' : st === 'wait' ? 'Connecting…' : rate || platform);
  setClass(row._sw, 'sw' + (d.enabled ? ' on' : ''));
  setTitle(row._sw, d.enabled ? 'Turn off' : 'Turn on');
}
// Compact circular badge: platform initial, live-glow ring, hover shows the
// name (native title). Tap arms it (confirm), tap again commits.
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
  setText(b, (destPlatforms[d.id] || d.name || '?').slice(0, 1).toUpperCase());
  setTitle(b, `${d.name || 'Destination'} · ${d.enabled ? 'on' : 'off'}`);
}
function onDestIcon(b) {
  const d = b._d;
  if (!cfg.w.dest.confirm) { commitDest(d, !d.enabled); return; }
  if (b._armed) { disarmDestIcon(b); commitDest(d, !d.enabled); return; }
  // Arm: pulse an amber ring and swap the badge to a check so the "tap again
  // to confirm" intent is unmistakable, not just a colour change.
  b._armed = true;
  b.classList.add('arm'); b.textContent = '✓';
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
  const yes = document.createElement('button'); yes.className = 'cbtn yes'; yes.innerHTML = '&#10003;';
  const no = document.createElement('button'); no.className = 'cbtn no'; no.innerHTML = '&times;';
  const cancel = () => { clearTimeout(row._ct); row._confirm = false; box.replaceWith(row._sw); };
  yes.onclick = () => { cancel(); commitDest(d, next); };
  no.onclick = cancel;
  box.append(yes, no);
  row._sw.replaceWith(box); row._confirm = true; row._ct = setTimeout(cancel, 3000);
}
async function commitDest(d, enabled) {
  const r = await fetchJ('/destinations/toggle', {
    method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: `id=${encodeURIComponent(d.id)}&enabled=${enabled ? 'on' : 'off'}`,
  });
  if (!r.ok) toast(r.j?.error || 'Toggle failed', 'err');
  else toast(`${d.name || 'Destination'} ${enabled ? 'on' : 'off'}`, 'ok');
}

// --------------------------------------------------------- extra widgets ----

// Delay profiles: one-tap chips from the user's saved profiles (same list as
// the dashboard). Tapping fills the delay and optionally arms it.
let profileCache = [];
async function fetchProfiles() {
  const r = await fetchJ('/profiles');
  if (r.ok && Array.isArray(r.j)) { profileCache = r.j; renderProfiles(); }
}
function renderProfiles() {
  const box = $('w-profiles'); if (box.hidden) return;
  const list = cfg.w.profiles.view === 'list';
  box.className = 'profiles' + (list ? ' plist' : '');
  if (!profileCache.length) { box.innerHTML = '<div class="empty">No profiles yet. Add them in the dashboard.</div>'; return; }
  box.innerHTML = '';
  for (const p of profileCache) {
    const ms = p.delay_ms || 0, sec = Math.round(ms / 1000);
    // Too big for the buffer at the current bitrate: greyed + disabled, with
    // a tooltip naming the size it needs. Same wall the arm path enforces.
    const tooBig = overCap(ms);
    const b = document.createElement('button');
    b.className = (list ? 'prow' : 'pchip') + (tooBig ? ' toobig' : '');
    b.disabled = tooBig;
    b.title = tooBig ? `Buffer too small - needs about ${requiredMB(ms)} MB. Raise it in the dashboard.` : `Set ${sec}s delay`;
    b.innerHTML = list
      ? `<span>${esc(p.name || sec + 's')}</span><span class="pv">${sec}s</span>`
      : esc(p.name || sec + 's') + (cfg.w.profiles.value ? `<span class="pv">${sec}s</span>` : '');
    b.onclick = () => profileSet(sec);
    box.appendChild(b);
  }
}
function profileSet(sec) {
  if (overCap(sec * 1000)) {
    toast(`Buffer too small for ${sec}s - raise it in the dashboard.`, 'err');
    return;
  }
  $('d').value = sec; fitDelayInput(); renderCta();
  const ds = s ? (PHASE_TO_STATE[s.phase] || 'off') : 'off';
  if (cfg.w.profiles.arm && ds === 'off' && sec > 0) arm(sec * 1000);
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

// Server settings mirrored on the dock (Auto behavior + Settings widgets).
// Read from /config; each boolean flip is a safe partial POST /config.
let srvCfg = { auto_arm_on_connect: false, auto_activate_when_ready: false, tracing_enabled: true };
async function fetchServerCfg() {
  const r = await fetchJ('/config');
  if (!r.ok || !r.j) return;
  srvCfg.auto_arm_on_connect = !!r.j.auto_arm_on_connect;
  srvCfg.auto_activate_when_ready = !!r.j.auto_activate_when_ready;
  srvCfg.tracing_enabled = !!r.j.tracing_enabled;
  if (typeof r.j.buffer_mb === 'number') { bufferMB = r.j.buffer_mb; renderProfiles(); }
  renderBehavior(); renderSettings();
}
// A label + switch row bound to a boolean server-config key.
function toggleRow(label, key) {
  const row = document.createElement('div'); row.className = 'brow';
  const lb = document.createElement('span'); lb.className = 'blbl'; lb.textContent = label;
  const sw = document.createElement('button'); sw.type = 'button'; sw.className = 'sw sm' + (srvCfg[key] ? ' on' : '');
  sw.onclick = () => setServer(key, !srvCfg[key], sw);
  row.append(lb, sw);
  return row;
}
async function setServer(key, val, sw) {
  srvCfg[key] = val; if (sw) sw.className = 'sw sm' + (val ? ' on' : '');
  const r = await fetchJ('/config', {
    method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: `${key}=${val ? 'true' : 'false'}`,
  });
  if (!r.ok) { toast('Save failed', 'err'); fetchServerCfg(); }
  else toast(val ? 'On' : 'Off', 'ok');
}
function renderBehavior() {
  const box = $('w-behavior'); if (box.hidden) return;
  box.innerHTML = '';
  box.appendChild(toggleRow('Auto-arm on connect', 'auto_arm_on_connect'));
  box.appendChild(toggleRow('Auto-activate when full', 'auto_activate_when_ready'));
}

// Settings: the live-safe knobs (things that don't need a restart). Buffer /
// ports stay in the dashboard because changing them restarts the pipeline.
function renderSettings() {
  const box = $('w-settings'); if (box.hidden) return;
  box.innerHTML = '';
  box.appendChild(toggleRow('Wire trace log', 'tracing_enabled'));
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
  q.title = 'Opens the Integrations tab of the InstantClone dashboard in a new tab.';
  const yes = document.createElement('button'); yes.className = 'minibtn'; yes.textContent = 'Yes';
  yes.onclick = () => { window.open(location.origin + '/#integrations', '_blank'); toast('Opening dashboard…', 'ok'); renderSettings(); };
  const no = document.createElement('button'); no.className = 'minibtn'; no.textContent = 'No'; no.onclick = renderSettings;
  btns.append(q, yes, no);
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
  const copy = document.createElement('button'); copy.className = 'minibtn'; copy.innerHTML = '&#128279;'; copy.title = 'Copy browser-source URL'; copy.onclick = () => copyOverlay(o.slug);
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
  const url = overlayUrl(slug);
  try { await navigator.clipboard.writeText(url); toast('Overlay URL copied', 'ok'); }
  catch (_) { toast(url, 'ok'); }
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
  $('w-screen').className = 'screen sz-' + (c.w.number.size || 'md');
  $('cur').classList.toggle('tint', !!c.w.number.tint);
  $('w-bar').hidden = !on('bar');
  $('w-bar').classList.toggle('thick', !!c.w.bar.thick);
  $('w-bar').classList.toggle('mono', !!c.w.bar.mono);
  for (const id of ['egress', 'tips', 'dest', 'profiles', 'stats', 'behavior', 'settings', 'overlays', 'activate']) $(WEL[id]).hidden = !on(id);
  document.querySelectorAll('.cur .u').forEach(u => u.hidden = !c.w.number.units);
  // Apply the saved drag order by re-appending containers in sequence.
  const wrap = document.querySelector('.wrap');
  for (const id of c.order) { const el = $(WEL[id]); if (el) wrap.appendChild(el); }
  // The crash hold card takes the place of the delay controls it stands in
  // for. A dock without them (destinations only, overlays) shows it on top,
  // so every dock can end the stream without losing its own purpose.
  const holdAnchor = on('number') ? $('w-screen') : on('activate') ? $('w-cta') : wrap.firstElementChild;
  if (holdAnchor !== $('w-hold')) wrap.insertBefore($('w-hold'), holdAnchor);
  if (on('profiles')) { fetchProfiles(); if (!bufferMB) fetchBufferMB(); }   // capacity gate needs buffer size
  if (on('behavior') || on('settings')) { renderBehavior(); renderSettings(); fetchServerCfg(); }
  if (on('overlays')) fetchOverlays();
  if (s) applyState(s); else renderCta();   // re-render with the new options
}

function persist() {
  const json = JSON.stringify(cfg);
  try { localStorage.setItem('ic.dock.' + SLOT, json); } catch (_) {}
  fetchJ('/docks/' + encodeURIComponent(SLOT), { method: 'POST', headers: { 'content-type': 'application/json' }, body: json });
}
function saveAndApply() { applyCfg(cfg); persist(); }

async function loadCfg() {
  // 1) A ?cfg= blob (shared / imported URL) wins and is saved to this slot.
  const p = new URLSearchParams(location.search).get('cfg');
  if (p) {
    try {
      cfg = mergeCfg(JSON.parse(decodeURIComponent(escape(atob(p)))));
      applyCfg(cfg); persist();
      const u = new URL(location.href); u.searchParams.delete('cfg'); history.replaceState(null, '', u);
      return;
    } catch (_) {}
  }
  // 2) localStorage mirror: instant paint before the server answers.
  try { const ls = localStorage.getItem('ic.dock.' + SLOT); if (ls) { cfg = mergeCfg(JSON.parse(ls)); applyCfg(cfg); } } catch (_) {}
  // 3) Server copy is the durable source of truth; reconcile when it lands.
  const r = await fetchJ('/docks/' + encodeURIComponent(SLOT));
  if (r.ok && r.j && typeof r.j === 'object') {
    cfg = mergeCfg(r.j); applyCfg(cfg);
    try { localStorage.setItem('ic.dock.' + SLOT, JSON.stringify(cfg)); } catch (_) {}
  }
}

// -------------------------------------------------------------- gear editor --

let dragId = null;
// Which widget rows are expanded. Kept across the frequent buildEditor()
// rebuilds so flipping an option doesn't snap its panel shut.
const openRows = new Set();
// Saved dock slots the server knows about (for the Docks switcher).
let knownSlots = [];

async function fetchSlots() {
  const r = await fetchJ('/docks');
  if (r.ok && Array.isArray(r.j)) { knownSlots = r.j; if (!$('editor').hidden) buildEditor(); }
}

function ensureEditorRoot() {
  let el = $('editor');
  if (!el) {
    el = document.createElement('div'); el.id = 'editor'; el.className = 'editor'; el.hidden = true;
    el.innerHTML = '<div class="sheet"><div class="grabber"></div><div class="sheet-body" id="sheet-body"></div></div>';
    el.onclick = e => { if (e.target === el) closeEditor(); };
    document.body.appendChild(el);
  }
  return el;
}
function openEditor() {
  const root = ensureEditorRoot(); buildEditor(); root.hidden = false;
  requestAnimationFrame(() => root.classList.add('show'));
  fetchSlots();   // populate the Docks switcher
}
function closeEditor() {
  const root = ensureEditorRoot(); root.classList.remove('show');
  setTimeout(() => root.hidden = true, 220);   // let the slide-out finish
}

// A row of segmented buttons (the "mode" control). `current` is the selected
// value; `items` is [value, label] pairs; `onPick(val)` applies the choice.
function segControl(current, items, onPick) {
  const seg = document.createElement('div'); seg.className = 'seg';
  for (const [val, lab] of items) {
    const b = document.createElement('button'); b.type = 'button';
    b.className = 'segb' + (String(current) === String(val) ? ' sel' : ''); b.textContent = lab;
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
      const sw = document.createElement('button'); sw.type = 'button'; sw.className = 'sw sm' + (cfg.w[t][sp.k] ? ' on' : '');
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
  sw.onclick = e => { e.stopPropagation(); w.on = !w.on; cfg.preset = 'custom'; saveAndApply(); buildEditor(); };
  const opts = optionEls(id);
  head.append(grip, nm);
  if (opts) {
    const cv = document.createElement('span'); cv.className = 'cv'; cv.innerHTML = '&#8250;'; head.appendChild(cv);
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
  const body = $('sheet-body'); body.innerHTML = '';
  const head = document.createElement('div'); head.className = 'ehead';
  head.innerHTML = `<h3>Customize dock</h3><span class="slot">${esc(SLOT)}</span>`;
  const x = document.createElement('button'); x.className = 'eclose'; x.innerHTML = '&times;'; x.title = 'Done'; x.onclick = closeEditor;
  head.appendChild(x); body.appendChild(head);

  body.appendChild(sectionLabel('Preset'));
  const chips = document.createElement('div'); chips.className = 'chips';
  for (const [k, p] of Object.entries(PRESETS)) {
    const c = document.createElement('button'); c.type = 'button'; c.className = 'chip' + (cfg.preset === k ? ' sel' : ''); c.textContent = p.label;
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
    b.style.setProperty('--sw', ACCENTS[key][0]); b.title = key;
    b.onclick = () => { cfg.accent = key; saveAndApply(); buildEditor(); };
    sw.appendChild(b);
  }
  accRow.appendChild(sw); body.appendChild(accRow);

  body.appendChild(sectionLabel('Widgets · drag to reorder'));
  const list = document.createElement('div'); list.className = 'wlist';
  for (const id of cfg.order) list.appendChild(widgetRow(id));
  body.appendChild(list);

  // Docks switcher: surface that multiple docks exist (?dock=<id>) and let
  // the user copy a URL to paste into another OBS browser dock.
  body.appendChild(sectionLabel('Docks'));
  const slots = document.createElement('div'); slots.className = 'chips';
  for (const id of Array.from(new Set([SLOT, ...knownSlots]))) {
    const c = document.createElement('button'); c.type = 'button';
    c.className = 'chip' + (id === SLOT ? ' sel' : ''); c.textContent = id;
    c.title = 'Copy this dock’s URL';
    c.onclick = () => copySlotUrl(id);
    slots.appendChild(c);
  }
  const add = document.createElement('button'); add.type = 'button'; add.className = 'chip add'; add.innerHTML = '&#43; New';
  add.onclick = () => newDock(add);
  slots.appendChild(add);
  body.appendChild(slots);
  const hint = document.createElement('div'); hint.className = 'hint';
  hint.textContent = 'Each OBS browser dock loads a layout by URL. Copy one, then add a new browser dock in OBS to run a second.';
  body.appendChild(hint);

  const act = document.createElement('div'); act.className = 'ed-actions';
  // Escape hatch for a messed-up layout: two-tap reset back to defaults.
  const reset = document.createElement('button'); reset.className = 'btn ghost reset'; reset.textContent = 'Reset';
  reset.onclick = () => {
    if (!reset._arm) {
      reset._arm = true; reset.textContent = 'Sure?'; reset.classList.add('warn');
      setTimeout(() => { reset._arm = false; reset.textContent = 'Reset'; reset.classList.remove('warn'); }, 3000);
      return;
    }
    openRows.clear(); cfg = defaultCfg(); saveAndApply(); buildEditor(); toast('Dock reset to defaults', 'ok');
  };
  const copy = document.createElement('button'); copy.className = 'btn ghost'; copy.innerHTML = '<span>&#128279;</span> Copy dock URL'; copy.onclick = copyDockUrl;
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
async function copyDockUrl() {
  const b64 = btoa(unescape(encodeURIComponent(JSON.stringify(cfg))));
  const url = `${location.origin}/dock?dock=${encodeURIComponent(SLOT)}&cfg=${b64}`;
  try { await navigator.clipboard.writeText(url); toast('Dock URL copied', 'ok'); }
  catch (_) { toast(url, 'ok'); }
}
// A slot's plain URL - it loads that slot's own saved layout on open.
async function copySlotUrl(id) {
  const url = `${location.origin}/dock?dock=${encodeURIComponent(id)}`;
  try { await navigator.clipboard.writeText(url); toast(id === SLOT ? 'This dock URL copied' : `Dock "${id}" URL copied`, 'ok'); }
  catch (_) { toast(url, 'ok'); }
}
// Inline name entry (prompt() is unreliable inside OBS's browser dock).
function newDock(addChip) {
  const inp = document.createElement('input'); inp.className = 'newdock'; inp.placeholder = 'name…'; inp.maxLength = 40;
  const commit = () => { const id = inp.value.replace(/[^a-zA-Z0-9_-]/g, '').slice(0, 40); if (id) copySlotUrl(id); buildEditor(); };
  inp.onkeydown = e => { if (e.key === 'Enter') commit(); else if (e.key === 'Escape') buildEditor(); };
  inp.onblur = () => setTimeout(() => { if (!$('editor').hidden) buildEditor(); }, 150);
  addChip.replaceWith(inp); inp.focus();
}

// -------------------------------------------------------------------- boot --

$('d').addEventListener('input', () => { fitDelayInput(); renderCta(); });
// Enter in the delay field = press the main button (arm). Keyboard-first flow.
$('d').addEventListener('keydown', e => { if (e.key === 'Enter') mainClick(); });
// Staggered first paint (see body.boot CSS); the class comes off after one
// pass so later saves/reorders never replay the entrance.
document.body.classList.add('boot');
setTimeout(() => document.body.classList.remove('boot'), 800);
fitDelayInput();
applyCfg(cfg);   // paint defaults immediately
loadCfg();       // then hydrate from url / localStorage / server

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
  if (e.key !== 'ic.dock.' + SLOT || !e.newValue) return;
  try {
    const next = mergeCfg(JSON.parse(e.newValue));
    if (JSON.stringify(next) !== JSON.stringify(cfg)) applyCfg(next);
  } catch (_) {}
});

// Aux data (profiles, overlays, server config, destinations) has no push
// channel, so refresh it whenever the dock regains focus or visibility -
// e.g. after editing profiles in the dashboard and clicking back into OBS.
function refreshAux() {
  if (cfg.w.profiles.on) { fetchProfiles(); fetchBufferMB(); }   // buffer size may have changed in the dashboard
  if (cfg.w.overlays.on) fetchOverlays();
  if (cfg.w.behavior.on || cfg.w.settings.on) fetchServerCfg();
  if (cfg.w.dest.on) fetchDestPlatforms();
}
window.addEventListener('focus', () => { if (!_es) tick(); refreshAux(); });
document.addEventListener('visibilitychange', () => { if (!document.hidden) refreshAux(); });
