// InstantClone OBS dock: a configurable stack of widgets driven by a saved
// layout. The layout is per-slot (?dock=<id>), persisted server-side so it
// survives OBS clearing its browser cache, with a localStorage mirror for
// instant paint. dock.html holds the markup+CSS; this file is all behaviour.

const $ = id => document.getElementById(id);
const PHASE_TO_STATE = { idle: 'off', preparing: 'arming', ready: 'armed', active: 'active' };
let s = null, lastVal = '0.0', pending = null;

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
  number:   { label: 'Delay number',  def: { on: true, units: true, step: 5, controls: true, size: 'md' } },
  bar:      { label: 'Buffer bar',    def: { on: true, pct: false, thick: false } },
  egress:   { label: 'Egress glance', def: { on: true, dests: true, bitrate: true, codec: false } },
  tips:     { label: 'Hint text',     def: { on: true, errorsOnly: false } },
  dest:     { label: 'Destinations',  def: { on: false, confirm: true, view: 'rows', active: false, stats: false } },
  profiles: { label: 'Delay profiles',def: { on: false, arm: false, value: true } },
  stats:    { label: 'Health stats',  def: { on: false, cpu: true, mem: true, recon: true, bitrate: false, cuts: false } },
  behavior: { label: 'Auto behavior', def: { on: false } },
  settings: { label: 'Settings',      def: { on: false } },
  overlays: { label: 'Overlays',      def: { on: false, autohide: false, search: true, group: true } },
  activate: { label: 'Action button', def: { on: true, confirmcut: false } },
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
    { k: 'controls', label: '+/- buttons' }, { k: 'units', label: 'Unit' },
    { k: 'on', label: 'Buffer bar', w: 'bar' }, { k: 'pct', label: 'Show %', w: 'bar' }, { k: 'thick', label: 'Thick bar', w: 'bar' },
  ],
  egress: [{ k: 'dests', label: 'Dest count' }, { k: 'bitrate', label: 'Bitrate' }, { k: 'codec', label: 'Codec' }],
  tips:   [{ k: 'errorsOnly', label: 'Errors only' }],
  dest:   [
    { k: 'view', label: 'Layout', modes: [['rows', 'Rows'], ['icons', 'Icons']] },
    { k: 'confirm', label: 'Confirm tap' }, { k: 'stats', label: 'Show bitrate' }, { k: 'active', label: 'Active only' },
  ],
  profiles: [{ k: 'arm', label: 'Arm on tap' }, { k: 'value', label: 'Show seconds' }],
  stats:  [
    { k: 'cpu', label: 'CPU' }, { k: 'mem', label: 'RAM' }, { k: 'recon', label: 'Reconnects' },
    { k: 'bitrate', label: 'Bitrate' }, { k: 'cuts', label: 'Cuts' },
  ],
  overlays: [
    { k: 'search', label: 'Search bar' }, { k: 'group', label: 'Folders' }, { k: 'autohide', label: 'Copy with autohide off' },
  ],
  activate: [{ k: 'confirmcut', label: 'Confirm cut' }],
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

// ---------------------------------------------------------- delay controls --

function bump(n) {
  const step = cfg.w.number.step || 1;
  const i = $('d');
  i.value = Math.max(0, Math.min(600, (+i.value || 0) + n * step));
  renderCta();
}
function setPending(p) { pending = p; setTimeout(() => pending = null, 1500); tick(); }
function fmtRate(k) { if (!k || k <= 0) return ''; return k >= 1000 ? (k / 1000).toFixed(2) + ' Mbps' : Math.round(k) + ' kbps'; }
function fmt(ms) { if (ms < 100) return '0.0'; const x = Math.round(ms / 100) / 10; return x < 10 ? x.toFixed(1) : Math.round(x).toString(); }
// Crossfade helper so the sub-tip swaps smoothly instead of snapping.
function setTip(text) {
  const el = $('w-tip'); if (el.textContent === text) return;
  el.classList.add('fade'); clearTimeout(el._t);
  el._t = setTimeout(() => { el.textContent = text; el.classList.remove('fade'); }, 140);
}

async function arm(ms) {
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
async function stop() { setPending('off'); await fetchJ('/stop', { method: 'POST' }); toast('Back to live', 'ok'); tick(); }
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
function disarmClick() {
  const b = $('cancel'); if (b._busy) return;
  b._busy = true; setTimeout(() => b._busy = false, 300);
  disarm();
}

// The big number is an editable setter while idle and a readout once a delay
// is engaged - the +/- retire so the screen reads clean (the feedback ask).
function setNumberMode(ds) {
  const engaged = ds === 'arming' || ds === 'armed' || ds === 'active';
  const controls = cfg.w.number.controls && !engaged;
  $('d').hidden = engaged;
  $('v').hidden = !engaged;
  $('minus').hidden = !controls;
  $('plus').hidden = !controls;
}

// ------------------------------------------------------------- state paint --

function renderCta() {
  if (!s) return;
  const m = $('main'), cancel = $('cancel');
  const ds = pending || PHASE_TO_STATE[s.phase] || 'off';
  const ingest = s.ingest_alive;
  // Drop a pending cut-confirm if we left the active state by any route, so a
  // stale arm can't skip the guard next time.
  if (ds !== 'active' && m._cutArm) { m._cutArm = false; clearTimeout(m._cutT); }
  m.className = 'btn full'; m.disabled = false; cancel.hidden = true;
  if (ds === 'active') {
    m.textContent = m._cutArm ? 'Tap again to cut' : 'Cut delay'; m.classList.add('danger');
  } else if (ds === 'armed') {
    m.innerHTML = '<span>&#9654;</span> Activate'; m.classList.add('go'); cancel.hidden = false;
  } else if (ds === 'arming') {
    const tgt = s.armed_delay_ms || 1;
    const pct = Math.min(99, Math.round((s.buffer_fill_ms / tgt) * 100));
    m.innerHTML = `<span class="spin"></span> Buffering ${pct}%`; m.disabled = true; cancel.hidden = false;
  } else {
    const ms = (+$('d').value || 0) * 1000;
    if (!ingest) { m.textContent = 'Waiting for encoder…'; m.disabled = true; }
    else if (ms <= 0) { m.textContent = 'Set a delay above'; m.disabled = true; }
    else { m.innerHTML = `<span>&#9201;</span> Arm ${(ms / 1000).toFixed(0)}s`; m.classList.add('primary'); }
  }
}

function applyState(j) {
  s = j;
  const ds = PHASE_TO_STATE[s.phase] || 'off';
  setNumberMode(ds);
  // Phase chip: idle vs passthrough vs arming/armed/active.
  const phMap = { off: s.ingest_alive ? ['live', 'Live'] : ['off', 'Idle'], arming: ['arming', 'Buffering'], armed: ['armed', 'Armed'], active: ['active', 'Active'] };
  const [cls, txt] = phMap[ds];
  $('ph').className = 'phase ' + cls + (cfg.w.phase.style === 'dot' ? ' asdot' : '');
  $('ph').textContent = txt; $('ph').title = txt;
  $('src').className = 'pill ' + (s.ingest_alive ? 'on' : 'bad') + (cfg.w.source.style === 'dot' ? ' asdot' : '');
  $('src-lbl').textContent = s.ingest_alive ? 'Live' : 'Offline';
  $('src').title = s.ingest_alive ? 'Live' : 'Offline';
  // Readout: mirror the dashboard - show the armed target while engaged (or
  // the fill while arming) and 0 in passthrough. current_delay_ms includes
  // the pipeline's own ~1s transit, which would read as a phantom delay.
  const tgtSec = (s.armed_delay_ms || 0) / 1000, fillSec = (s.buffer_fill_ms || 0) / 1000;
  const showSec = ds === 'arming' ? Math.min(fillSec, tgtSec) : (ds === 'armed' || ds === 'active') ? tgtSec : 0;
  const nv = fmt(showSec * 1000);
  if (nv !== lastVal) {
    $('v').textContent = nv; lastVal = nv;
    const cur = $('v').parentElement; cur.classList.add('changing'); setTimeout(() => cur.classList.remove('changing'), 350);
  }
  // Progress bar (+ optional %).
  const target = s.armed_delay_ms || s.buffer_target_ms || 0;
  const fill = $('bar'); fill.classList.remove('ready', 'active');
  let pct = 0;
  if (ds === 'active') { fill.style.width = '100%'; fill.classList.add('active'); pct = 100; }
  else if (target > 0) {
    pct = Math.min(100, (Math.min(s.buffer_fill_ms, target) / target) * 100);
    fill.style.width = pct + '%'; if (pct >= 99) fill.classList.add('ready');
  } else fill.style.width = '0%';
  const pctEl = $('pct'); pctEl.hidden = !cfg.w.bar.pct; if (cfg.w.bar.pct) pctEl.textContent = Math.round(pct) + '%';
  // Egress glance, honouring its per-part options.
  const eg = $('w-eg');
  if (s.ingest_alive && cfg.w.egress.on) {
    const parts = [];
    if (cfg.w.egress.dests) parts.push(`<span>&#9650;</span><b>${s.destinations_alive || 0}/${s.destinations_total || 0}</b> live`);
    if (cfg.w.egress.bitrate) { const rate = fmtRate(s.stats && s.stats.bitrate_kbps); if (rate) parts.push(`<b>${rate}</b> in`); }
    if (cfg.w.egress.codec && s.video_codec) parts.push(`<b>${esc(s.video_codec)}</b>${s.multitrack_video ? ' &middot; MT' : ''}`);
    if (parts.length) { eg.innerHTML = parts.join('<span class="sep">&middot;</span>'); eg.hidden = false; } else eg.hidden = true;
  } else eg.hidden = true;
  // Hint text: full sentences, or errors-only (encoder offline) if opted.
  const tip = !s.ingest_alive ? 'Point your encoder at rtmp://…/live'
    : ds === 'active' ? `Live ${(s.current_delay_ms / 1000).toFixed(1)}s behind real time.`
    : ds === 'armed' ? 'Buffer full. Hit Activate to go live with delay.'
    : ds === 'arming' ? 'Filling buffer… you can cancel any time.'
    : 'Passthrough - no delay applied.';
  const showTip = cfg.w.tips.on && (!cfg.w.tips.errorsOnly || !s.ingest_alive);
  $('w-tip').hidden = !showTip; if (showTip) setTip(tip);
  renderStats();
  renderCta();
}

async function tick() {
  const r = await fetchJ('/state');
  if (!r.ok || !r.j) { $('src').className = 'pill bad'; $('src-lbl').textContent = 'Offline'; return; }
  applyState(r.j);
}

// ---------------------------------------------------------- destination strip --

let _destTimer = null;
let destCache = [];   // last /destinations payload, for instant re-render on option change
function startDests() { if (_destTimer) return; fetchDests(); _destTimer = setInterval(fetchDests, 4000); }
function stopDests() { if (_destTimer) { clearInterval(_destTimer); _destTimer = null; } }
async function fetchDests() {
  if ($('w-dests').hidden) return;
  const r = await fetchJ('/destinations');
  if (r.ok && Array.isArray(r.j)) { destCache = r.j; renderDests(r.j); }
}
function renderDests(list) {
  const box = $('w-dests'); if (box.hidden) return;
  const icons = cfg.w.dest.view === 'icons';
  // Show every configured destination by default so a disabled one can be
  // toggled back on. "Active only" is the opt-in filter for a clean look.
  const show = cfg.w.dest.active ? list.filter(d => d.enabled || d.alive) : list;
  box.className = 'dests' + (icons ? ' icons' : '');
  if (!show.length) { box.innerHTML = '<div class="empty">No destinations yet. Add them in the dashboard.</div>'; return; }
  box.innerHTML = '';
  for (const d of show) box.appendChild(icons ? destIcon(d) : destRow(d));
}
function destRow(d) {
  const row = document.createElement('div');
  row.className = 'dest' + (d.alive ? ' alive' : '');
  const rate = cfg.w.dest.stats && d.alive ? fmtRate(d.bitrate_kbps) : '';
  const meta = rate ? `<span class="d-rate">${rate}</span>` : `<span class="d-plat">${esc(d.platform || '')}</span>`;
  row.innerHTML = `<span class="d-dot"></span><span class="d-name">${esc(d.name || 'Destination')}</span>${meta}`;
  const sw = document.createElement('button');
  sw.className = 'sw' + (d.enabled ? ' on' : '');
  sw.title = d.enabled ? 'Turn off' : 'Turn on';
  sw.onclick = () => onDestToggle(d, row, sw);
  row.appendChild(sw);
  return row;
}
// Compact circular badge: platform initial, live-glow ring, hover shows the
// name (native title). Tap arms it (confirm), tap again commits.
function destIcon(d) {
  const b = document.createElement('button');
  b.className = 'dico' + (d.enabled ? ' on' : ' off') + (d.alive ? ' alive' : '');
  b.textContent = (d.platform || d.name || '?').slice(0, 1).toUpperCase();
  b.title = `${d.name || 'Destination'} · ${d.enabled ? 'on' : 'off'}`;
  b.onclick = () => onDestIcon(d, b);
  return b;
}
function onDestIcon(d, b) {
  if (!cfg.w.dest.confirm) { commitDest(d, !d.enabled); return; }
  if (b._armed) { clearTimeout(b._t); commitDest(d, !d.enabled); return; }   // 2nd tap commits; the re-render clears the armed look
  // Arm: pulse an amber ring and swap the badge to a check so the "tap again
  // to confirm" intent is unmistakable, not just a colour change.
  b._armed = true; b._letter = b.textContent; b.textContent = '✓';
  b.classList.add('arm');
  b.title = d.enabled ? 'Tap again to turn OFF' : 'Tap again to go LIVE';
  clearTimeout(b._t);
  b._t = setTimeout(() => {
    b._armed = false; b.classList.remove('arm'); b.textContent = b._letter;
    b.title = `${d.name || 'Destination'} · ${d.enabled ? 'on' : 'off'}`;
  }, 3000);
}
// Misclick guard: the row swaps its switch for a Sure? / cancel pair that
// auto-dismisses after 3s. Toggling a live platform either way matters, so
// both directions are guarded.
function onDestToggle(d, row, sw) {
  if (!cfg.w.dest.confirm) { commitDest(d, !d.enabled); return; }
  if (row._confirm) return;
  const next = !d.enabled;
  const box = document.createElement('span'); box.className = 'confirm';
  box.innerHTML = `<span class="q">${next ? 'Go live?' : 'Turn off?'}</span>`;
  const yes = document.createElement('button'); yes.className = 'cbtn yes'; yes.innerHTML = '&#10003;';
  const no = document.createElement('button'); no.className = 'cbtn no'; no.innerHTML = '&times;';
  const cancel = () => { clearTimeout(row._ct); row._confirm = false; box.replaceWith(sw); };
  yes.onclick = () => { cancel(); commitDest(d, next); };
  no.onclick = cancel;
  box.append(yes, no);
  sw.replaceWith(box); row._confirm = true; row._ct = setTimeout(cancel, 3000);
}
async function commitDest(d, enabled) {
  const r = await fetchJ('/destinations/toggle', {
    method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' },
    body: `id=${encodeURIComponent(d.id)}&enabled=${enabled ? 'on' : 'off'}`,
  });
  if (!r.ok) toast(r.j?.error || 'Toggle failed', 'err');
  else toast(`${d.name || 'Destination'} ${enabled ? 'on' : 'off'}`, 'ok');
  fetchDests();
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
  if (!profileCache.length) { box.innerHTML = '<div class="empty">No profiles yet. Add them in the dashboard.</div>'; return; }
  box.innerHTML = '';
  for (const p of profileCache) {
    const sec = Math.round((p.delay_ms || 0) / 1000);
    const b = document.createElement('button'); b.className = 'pchip';
    b.innerHTML = esc(p.name || sec + 's') + (cfg.w.profiles.value ? `<span class="pv">${sec}s</span>` : '');
    b.onclick = () => profileSet(sec);
    box.appendChild(b);
  }
}
function profileSet(sec) {
  $('d').value = sec; renderCta();
  const ds = s ? (PHASE_TO_STATE[s.phase] || 'off') : 'off';
  if (cfg.w.profiles.arm && ds === 'off' && sec > 0) arm(sec * 1000);
}

// Health stats: CPU / RAM / reconnects, plus a backpressure warning. All read
// from the state stream, so no extra polling.
function renderStats() {
  const box = $('w-stats'); if (box.hidden || !s) return;
  const o = cfg.w.stats, st = s.stats || {}, parts = [];
  if (o.cpu) parts.push(`<span class="st"><b>${(s.cpu_pct || 0).toFixed(0)}</b>% cpu</span>`);
  if (o.mem) parts.push(`<span class="st"><b>${Math.round((s.rss_bytes || 0) / 1048576)}</b> MB</span>`);
  if (o.bitrate) { const rate = fmtRate(st.bitrate_kbps); if (rate) parts.push(`<span class="st"><b>${rate}</b></span>`); }
  if (o.recon) parts.push(`<span class="st"><b>${(st.egress_reconnects || 0) + (st.ingest_disconnects || 0)}</b> recon</span>`);
  if (o.cuts) parts.push(`<span class="st"><b>${st.cuts || 0}</b> cuts</span>`);
  if (s.backpressure) parts.push('<span class="st bad">&#9888; buffer lag</span>');
  box.innerHTML = parts.join('') || '<span class="st muted">no metrics selected</span>';
}

// Server settings mirrored on the dock (Auto behavior + Settings widgets).
// Read from /config; each boolean flip is a safe partial POST /config.
let srvCfg = { auto_arm_on_connect: false, auto_activate_when_ready: false, tracing_enabled: true, webhook_set: false };
async function fetchServerCfg() {
  const r = await fetchJ('/config');
  if (!r.ok || !r.j) return;
  srvCfg.auto_arm_on_connect = !!r.j.auto_arm_on_connect;
  srvCfg.auto_activate_when_ready = !!r.j.auto_activate_when_ready;
  srvCfg.tracing_enabled = !!r.j.tracing_enabled;
  srvCfg.webhook_set = !!r.j.webhook_set;
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
  const wh = document.createElement('div'); wh.className = 'brow';
  const lb = document.createElement('span'); lb.className = 'blbl'; lb.textContent = srvCfg.webhook_set ? 'Discord webhook set' : 'No Discord webhook';
  const btns = document.createElement('span'); btns.className = 'srow-btns';
  if (srvCfg.webhook_set) {
    const test = document.createElement('button'); test.className = 'minibtn'; test.textContent = 'Test'; test.onclick = testWebhook;
    const clr = document.createElement('button'); clr.className = 'minibtn'; clr.textContent = 'Clear'; clr.onclick = clearWebhook;
    btns.append(test, clr);
  } else {
    // Setting a webhook means pasting a secret URL - awkward on a dock, so
    // "Set" bounces to the dashboard (behind a confirm).
    const set = document.createElement('button'); set.className = 'minibtn'; set.textContent = 'Set'; set.onclick = () => confirmOpenDashboard(btns);
    btns.append(set);
  }
  wh.append(lb, btns); box.appendChild(wh);
}
function confirmOpenDashboard(btns) {
  btns.innerHTML = '';
  const q = document.createElement('span'); q.className = 'cq'; q.textContent = 'Open dashboard?';
  q.title = 'This opens the InstantClone dashboard in a new tab so you can paste the webhook URL.';
  const yes = document.createElement('button'); yes.className = 'minibtn'; yes.textContent = 'Yes';
  yes.onclick = () => { window.open(location.origin + '/', '_blank'); toast('Opening dashboard…', 'ok'); renderSettings(); };
  const no = document.createElement('button'); no.className = 'minibtn'; no.textContent = 'No'; no.onclick = renderSettings;
  btns.append(q, yes, no);
}
async function testWebhook() {
  const r = await fetchJ('/test-webhook', { method: 'POST' });
  // The endpoint answers 200 even when Discord rejects, so trust the JSON ok.
  const ok = r.ok && (!r.j || r.j.ok !== false);
  toast(ok ? 'Test event sent' : (r.j?.error || 'Test failed'), ok ? 'ok' : 'err');
}
async function clearWebhook() {
  const r = await fetchJ('/config', { method: 'POST', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: 'webhook_clear=1' });
  if (r.ok) { srvCfg.webhook_set = false; renderSettings(); toast('Webhook cleared', 'ok'); }
  else toast('Failed', 'err');
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
  const open = document.createElement('button'); open.className = 'minibtn'; open.textContent = 'Open'; open.title = 'Preview in a new tab'; open.onclick = () => openOverlay(o.slug);
  const copy = document.createElement('button'); copy.className = 'minibtn'; copy.innerHTML = '&#128279;'; copy.title = 'Copy browser-source URL'; copy.onclick = () => copyOverlay(o.slug);
  row.append(nm, open, copy);
  return row;
}
function overlayUrl(slug) {
  // The overlay files are served at /overlay/<slug>.html (matches the
  // dashboard). ?autohide=off keeps the overlay up even while live (some
  // presets otherwise fade out); opt-in via the widget option.
  const q = cfg.w.overlays.autohide ? '?autohide=off' : '';
  return `${location.origin}/overlay/${encodeURIComponent(slug)}.html${q}`;
}
function openOverlay(slug) { window.open(overlayUrl(slug), '_blank'); }
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
  $('w-bar').hidden = !on('bar');
  $('w-bar').classList.toggle('thick', !!c.w.bar.thick);
  for (const id of ['egress', 'tips', 'dest', 'profiles', 'stats', 'behavior', 'settings', 'overlays', 'activate']) $(WEL[id]).hidden = !on(id);
  document.querySelectorAll('.cur .u').forEach(u => u.hidden = !c.w.number.units);
  // Apply the saved drag order by re-appending containers in sequence.
  const wrap = document.querySelector('.wrap');
  for (const id of c.order) { const el = $(WEL[id]); if (el) wrap.appendChild(el); }
  if (on('dest')) { startDests(); if (destCache.length) renderDests(destCache); } else stopDests();
  if (on('profiles')) fetchProfiles();
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
  const copy = document.createElement('button'); copy.className = 'btn ghost'; copy.innerHTML = '<span>&#128279;</span> Copy dock URL'; copy.onclick = copyDockUrl;
  const done = document.createElement('button'); done.className = 'btn primary'; done.textContent = 'Done'; done.onclick = closeEditor;
  act.append(copy, done); body.appendChild(act);
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

$('d').addEventListener('input', renderCta);
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
window.addEventListener('focus', () => { if (!_es) tick(); });
