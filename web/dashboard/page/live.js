// - Tick / poll loop -------------------------
// applyState() is the pure render side - used by both the poll path
// and the SSE push path so we don't have two copies of the dashboard
// reconciliation logic.
function fmtUptime(sec){
  sec = Math.max(0, Math.floor(sec));
  const d = Math.floor(sec/86400), h = Math.floor(sec%86400/3600), m = Math.floor(sec%3600/60), r = sec%60;
  if (d) return d+'d '+h+'h';
  if (h) return h+'h '+m+'m';
  if (m) return m+'m '+r+'s';
  return r+'s';
}
// Crash protection: while a hold is on air, say why, how many destinations
// it keeps live and how long is left, and offer End now. The same banner
// covers the delay still airing after OBS left with no hold.
function renderHoldBanner(s){
  const banner = $('hold-banner');
  if (!banner) return;
  const hold = s.hold;
  const tail = !hold && s.tail_ms > 0;
  if (banner.hidden !== !(hold || tail)){
    banner.hidden = !(hold || tail);
    fitPaneCard();
  }
  if (tail){
    $('hold-title').textContent = 'OBS stopped. Your delay is still airing.';
    const live = s.destinations_alive || 0;
    $('hold-sub').textContent = `${live === 1 ? '1 destination live' : `${live} destinations live`} · ends in ${fmtClock(s.tail_ms)}, when viewers reach the end`;
    return;
  }
  if (!hold) return;
  $('hold-title').textContent = hold.reason === 'freeze'
    ? 'OBS stopped sending video. Keeping your destinations live.'
    : hold.reason === 'stopped' ? 'OBS stopped the stream. Keeping your destinations live.'
    : 'OBS dropped. Keeping your destinations live.';
  const held = (s.destinations || []).filter(d => d.on_hold).length;
  // After a crash OBS asks about Safe Mode, which leaves scripts and
  // plugins off (the VOD track script among them).
  const reopen = hold.reason === 'crash' ? 'reopen OBS in Normal Mode and start streaming' : 'waiting for OBS';
  const live = held ? `${held === 1 ? '1 destination live' : `${held} destinations live`} · ${reopen}` : reopen[0].toUpperCase() + reopen.slice(1);
  $('hold-sub').textContent = `${live} · ${fmtClock(hold.remaining_ms)} left`;
}
// "1:05" for a millisecond countdown.
function fmtClock(ms){
  const secs = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, '0')}`;
}
// Ending the hold ends the stream everywhere, so it takes a second click
// within 3 s (no dialog: it has to work the same in the OBS dock).
async function endHold(btn){
  if (!btn._armed){
    btn._armed = true;
    btn.textContent = 'Click again to end';
    btn._reset = setTimeout(() => { btn._armed = false; btn.textContent = 'End now'; }, 3000);
    return;
  }
  clearTimeout(btn._reset);
  btn._armed = false;
  btn.textContent = 'End now';
  try {
    const r = await fetch('/crash-protection/end', {method:'POST'});
    const j = await r.json().catch(() => ({}));
    if (r.status === 409){ toast(j.error || 'Nothing to end', 'info'); return; }
    if (!r.ok){ toast('Could not end the stream', 'err'); return; }
    applyState(j);
    toast('Stream ended on every destination', 'ok');
  } catch(_){ toast('Could not reach InstantClone', 'err'); }
}

// - Live data -----------------------------------------------------
// The state feed carries a change counter for each kind of data this page
// fetches on its own (`rev`: config, destinations, profiles, OBS,
// Streamlabs...; see src/revisions.rs). When one moves, whatever shows that
// data fetches it again, so a change made in another window, from the tray
// or outside the app (OBS opened, Streamlabs installed) shows without a
// reload, and nothing polls for it. The loaders redraw only what differs,
// so a refetch that brings nothing new draws nothing.
const Live = {
  revs: null,
  loaders: {},
  // `load` runs each time `topic` moves.
  on(topic, load){
    (this.loaders[topic] = this.loaders[topic] || []).push(coalesced(load));
  },
  apply(revs){
    if (!revs) return;
    const before = this.revs;
    this.revs = revs;
    // The first frame is a baseline: the page loaded everything itself.
    // After a restart every counter starts over, and differs: all reload.
    if (!before) return;
    Object.keys(revs).forEach(topic => {
      if (revs[topic] !== before[topic]) (this.loaders[topic] || []).forEach(load => load());
    });
  },
};
// `fn`, safe to call at any rate: one run at a time, and calls landing
// mid-run make a single rerun, so the last change is always fetched.
function coalesced(fn){
  let running = false, again = false;
  const run = async () => {
    if (running){ again = true; return; }
    running = true;
    try { await fn(); } catch(e){ console.warn(e); }
    running = false;
    if (again){ again = false; run(); }
  };
  return run;
}
// `fn` at most once every `ms`: calls inside that time make one run at its
// end, so a burst of changes is one fetch and the last change still shows.
function throttled(fn, ms){
  let last = 0, timer = 0;
  return () => {
    const wait = last + ms - Date.now();
    if (wait <= 0){ last = Date.now(); return fn(); }
    if (!timer) timer = setTimeout(() => { timer = 0; last = Date.now(); fn(); }, wait);
  };
}
// Whether `value` differs from the last one seen under `key`: a reload
// redraws a section only when its own data changed.
const seenData = new Map();
function dataChanged(key, value){
  const text = JSON.stringify(value === undefined ? null : value);
  if (seenData.get(key) === text) return false;
  seenData.set(key, text);
  return true;
}

function applyState(s){
  const prev = lastState;
  Live.apply(s.rev);
  if ($('stat-idle')) $('stat-idle').hidden = !!s.ingest_alive;
  if (prev){
    if (!prev.ingest_alive && s.ingest_alive) toast('OBS connected','ok');
    else if (prev.ingest_alive && !s.ingest_alive) toast('OBS disconnected','err');
    else if (prev.ingest_alive && s.ingest_alive && prev.publisher_token !== s.publisher_token)
      toast('OBS reconnected, timeline realigned','info');
  }
  lastState = s;
  followArmedDelay(s.armed_delay_ms || 0);
  if (window.ICOverlay) Studio.upgradeSaved();
  // The server compares the running buffer with the saved one, so the
  // notice is right after a reload too, and gone once a restart applied it.
  const banner = $('restart-banner');
  animateHidden(banner, !s.restart_pending, fitPaneCard);
  // System-info tiles (About tab): live bits, every tick, tab-independent.
  if ($('si-uptime')) $('si-uptime').textContent = fmtUptime(s.uptime_secs || 0);
  if ($('si-mem')) $('si-mem').textContent = Math.round((s.rss_bytes || 0) / 1048576) + ' MB';
  if ($('si-cpu')) $('si-cpu').textContent = Math.round(s.cpu_pct || 0) + '%';
  if ($('si-dests')){
    const t = s.destinations_total || 0, a = s.destinations_alive || 0;
    $('si-dests').textContent = t ? (a + ' of ' + t + ' live') : 'none';
  }
  // A hotkey or pad that just fired. This is the only feedback for a press
  // that landed while the user was in a game, so it runs before the
  // not-configured early return and regardless of which tab is open.
  showFiredAction(s.last_action);
  // Whether Windows actually took each hotkey. Applied before the
  // not-configured early return below, since Settings is reachable either
  // way and a dead binding is exactly what a first-time user would set.
  if (s.hotkey_conflicts) hkApplyConflicts(s.hotkey_conflicts);
  if ($('hero').hidden) return; // not configured
  // pendingPhase holds a raw phase ('ready', 'idle'), so compare it raw:
  // mapped through PHASE_TO_STATE it never matched after Cut or Disarm and
  // "Applying" sat out its full 2.5 s fallback.
  if (pendingPhase && pendingPhase === s.phase) clearPending();
  renderHero(s);
  renderHoldBanner(s);
  renderHeader(s);
  renderStats(s);
  renderCpProfiles();
  renderBufferHint();
  // The profiles pane uses the live bitrate to decide which entries are
  // unreachable at the current buffer size - rebuild on every tick so
  // the dimming follows the bitrate without the user re-clicking the tab.
  if (!$('pane-card').hidden) renderProfilesPane();
  // Live destination stats - /state already carries alive / bitrate_kbps /
  // tags_sent / bytes_sent / cuts / reconnects per destination, so merge
  // those into our cached list and rebuild the cards on every state tick.
  // Previously the cards only refreshed on explicit user actions (saving
  // a destination, switching tabs) so bitrate sparklines and status pills
  // froze until the user hit F5.
  if (Array.isArray(s.destinations) && lastDests.length > 0){
    s.destinations.forEach(sd => {
      const ld = lastDests.find(d => d.id === sd.id);
      // Copy whatever /state sends instead of a hand-maintained field list.
      // The old whitelist silently froze any live field it didn't know
      // about - both the Dual Format icon and the res/codec readout
      // shipped broken that way, stuck at whatever was true when the page
      // last fetched /destinations. Adding a field to /state now just
      // works. Config-only fields (platform, stream_format, stream_key_set,
      // url_redacted) aren't in /state, so they survive untouched.
      if (ld) Object.assign(ld, sd);
    });
    rebuildDests();
  }
  const ds = PHASE_TO_STATE[s.phase] || 'off';
  // Footer status: ingest down → "standby"; up & no delay → "live
  // (passthrough)"; otherwise the delay phase. Matches the new state
  // pill semantics so the dashboard tells a consistent story.
  $('foot-status').textContent = !s.ingest_alive ? 'standby'
                                : ds === 'off' ? 'live, no delay'
                                : ds;
}
async function tick(){
  try { applyState(await (await fetch('/state')).json()); } catch(_){}
}

// Prefer Server-Sent Events for state. One long-lived /events socket
// replaces the 2 Hz polling loop - server writes only when state
// actually changes, so idle CPU drops. Falls back to polling if
// EventSource isn't supported (very old browsers) or the stream
// errors out (e.g. proxy that doesn't grok text/event-stream).
let _statePollTimer = null;
function startStatePolling(){
  if (_statePollTimer) return;
  tick();
  _statePollTimer = setInterval(tick, 500);
}
function stopStatePolling(){
  if (_statePollTimer){ clearInterval(_statePollTimer); _statePollTimer = null; }
}
let _stateES = null;
function startStateSSE(){
  if (!window.EventSource){ startStatePolling(); return; }
  try {
    _stateES = new EventSource('/events');
    _stateES.onmessage = ev => {
      try { applyState(JSON.parse(ev.data)); stopStatePolling(); } catch(_){}
    };
    _stateES.onerror = () => {
      try { _stateES.close(); } catch(_){}
      _stateES = null;
      startStatePolling();
    };
  } catch(_){ startStatePolling(); }
}

// Header pill: is there a newer release? Asked on load, every few hours for
// a dashboard left open for days, and when the tab comes back into view.
// Failures are silent - we don't want offline / rate-limited users to see a
// scary error pill for a passive nicety. The server caches the answer for
// 10 min, so a refresh spree doesn't hammer GitHub.
const UPDATE_RECHECK_MS = 3 * 60 * 60 * 1000;
const UPDATE_FOCUS_RECHECK_MS = 10 * 60 * 1000;
let lastUpdateCheck = 0;
async function checkForUpdate(){
  const pill = $('hdr-update');
  const label = $('hdr-update-v');
  if (!pill || !label) return;
  // Respect the opt-out (a missing field defaults to on). The manual
  // "Check for updates" button bypasses this and always works.
  if (lastConfig && lastConfig.update_check_enabled === false){ pill.hidden = true; return; }
  lastUpdateCheck = Date.now();
  try {
    const r = await (await fetch('/update-check')).json();
    if (!r || r.error) return;
    if (!r.update_available){ pill.hidden = true; return; }
    label.textContent = updateLabel(r);
    // Hovering it says what the update brings; System > About lists it all.
    const brings = (r.changes || []).slice(0, 3).map(c => `${c.kind}: ${c.text}`);
    pill.title = brings.length ? brings.join('\n') : 'Open the updater';
    pill.hidden = false;
  } catch(_){}
}
setInterval(checkForUpdate, UPDATE_RECHECK_MS);
document.addEventListener('visibilitychange', () => {
  if (!document.hidden && lastUpdateCheck && Date.now() - lastUpdateCheck > UPDATE_FOCUS_RECHECK_MS) checkForUpdate();
});
// "Tell me in the tray" only means something while checks run.
// "Tell me in the tray" only means something while checks run, so it shows
// only then: it folds away when they're switched off and back when they're
// on. `animate` is false when the settings load, so a page opens settled.
let hasTray = true;
function syncUpdateTray(animate = true){
  const row = $('s-update-tray-row');
  const hide = !hasTray || !$('s-update-check').checked;
  if (animate) animateHidden(row, hide); else setHidden(row, hide);
}

// After an update (in-app or by hand) the server says what changed since the
// build the user last saw: a card in System > About until "Got it", and a
// quiet header pill on the first load only.
let whatsNew = null;
async function loadWhatsNew(){
  try { whatsNew = await (await fetch('/whats-new')).json(); } catch(_){ return; }
  if (!whatsNew || !whatsNew.show) return;
  const w = whatsNew;
  const title = w.rebuild ? 'Updated to a new build of v' + w.to : 'Updated to v' + w.to;
  $('whats-new-title').textContent = title;
  $('whats-new-sub').textContent = w.from && !w.rebuild
    ? `From v${w.from}. Your settings, destinations and overlays came along.`
    : 'Your settings, destinations and overlays came along.';
  const changes = w.changes || [];
  $('whats-new-list').replaceChildren(...changes.slice(0, UPDATE_CHANGES_SHOWN).map(changeRow));
  $('whats-new-changes').hidden = !changes.length;
  const link = $('whats-new-link');
  const more = changes.length > UPDATE_CHANGES_SHOWN;
  link.href = more && w.changes_url ? w.changes_url : w.release_url;
  link.textContent = more ? 'All ' + changes.length + ' changes on GitHub' : 'Release notes';
  animateHidden($('whats-new'), false);
  const key = [w.from, w.to, w.rebuild, w.changes_url].join('|');
  let shown = true;
  try { shown = localStorage.getItem('ic-whats-new-pill') === key; localStorage.setItem('ic-whats-new-pill', key); } catch(_){}
  if (!shown){
    $('hdr-whats-new-v').textContent = w.rebuild ? 'Updated build' : 'Updated to v' + w.to;
    $('hdr-whats-new').hidden = false;
  }
}
function openWhatsNew(){
  $('hdr-whats-new').hidden = true;
  showTab('system');
  showSubTab('system', 'about');
}
async function dismissWhatsNew(){
  animateHidden($('whats-new'), true);
  fadeOutHidden($('hdr-whats-new'));
  try { await fetch('/whats-new/dismiss', {method:'POST'}); }
  catch(_){ toast('Couldn\'t save that. It may show again next time.', 'err'); }
}

// What an available update brings, from its commits since this build (only
// release builds know their commit). Written as text: a commit subject is
// never markup. Past a handful, the rest are one link away.
const UPDATE_CHANGES_SHOWN = 8;
function renderUpdateChanges(j){
  const box = $('sys-about-changes'), list = $('sys-about-changes-list');
  if (!box || !list) return;
  const changes = (j && j.update_available && j.changes) || [];
  const rows = changes.slice(0, UPDATE_CHANGES_SHOWN).map(changeRow);
  const more = changes.length - UPDATE_CHANGES_SHOWN;
  if (more > 0 && j.changes_url){
    const li = document.createElement('li');
    const a = document.createElement('a');
    a.href = j.changes_url; a.target = '_blank'; a.rel = 'noreferrer';
    a.textContent = `and ${more} more on GitHub`;
    li.appendChild(a);
    rows.push(li);
  }
  list.replaceChildren(...rows);
  if (changes.length) $('sys-about-changes-title').textContent =
    j.rebuild ? 'What the new build brings' : `What v${j.latest} brings`;
  animateHidden(box, !changes.length);
}
// One change: its kind as a badge, then the line. Text only: a commit
// subject is never markup.
function changeRow(c){
  const li = document.createElement('li');
  const kind = document.createElement('b');
  kind.textContent = c.kind;
  kind.dataset.kind = c.kind;
  li.append(kind, c.text);
  return li;
}
// What an available update is called: a newer version, or a newer build of
// this same version (`rebuild`, spotted by the exe's SHA-256).
function updateLabel(r){
  return r.rebuild ? 'New v' + r.latest + ' build' : 'v' + r.latest + ' available';
}
// The header "update available" pill opens the in-app updater (System > About)
// instead of GitHub, so one click leads to the one-click Update button.
function openUpdateInAbout(){
  showTab('system');
  showSubTab('system','about');
  try { aboutCheckUpdate(); } catch(_){}
}

(async function init(){
  // The counters before anything is fetched: a change landing while the page
  // loads moves one past this, and the first state frame fetches it again.
  try { Live.revs = (await (await fetch('/state')).json()).rev || null; } catch(_){}
  try { platforms = await (await fetch('/platforms')).json(); } catch(_){ platforms = []; }
  fillWizardPlatforms();
  wireAllDisclosures();
  reserveCopyButtons();
  wireChatDock();
  fillAppMarks();
  await loadConfig();
  langReturnReady();
  refreshObsRegisterStatus();
  refreshStreamlabs();
  Live.on('config', () => loadConfig({live: true}));
  Live.on('destinations', loadDestinations);
  Live.on('profiles', loadProfiles);
  Live.on('overlays', refreshOverlays);
  Live.on('obs', refreshObsSetup);
  Live.on('streamlabs', refreshStreamlabs);
  checkForUpdate();
  loadWhatsNew();
  checkUpdateOutcome();
  startStateSSE();
  // A busy log moves its counter many times a second: read it once a second.
  Live.on('logs', throttled(pollLogs, 1000));
  pollLogs();
  document.addEventListener('visibilitychange', () => { if (!document.hidden) pollLogs(); });
  // Space drives the delay only from the bare page: a focused control
  // (button, switch, select, style card) or an open dialog keeps it, and
  // holding the key down doesn't repeat the action.
  document.addEventListener('keydown', e => {
    if (e.key !== ' ' || e.repeat) return;
    if (e.target.closest('input,textarea,select,button,a,[role],[tabindex],[contenteditable]')) return;
    if (document.querySelector('.ic-modal:not([hidden]),#ov-studio:not([hidden]),#tour:not([hidden])')) return;
    e.preventDefault();
    ctaClick({currentTarget: $('cp-cta')});
  });
  // First-run tour autostart lives inside loadConfig() (see the
  // `if (configured)` branch). That hook fires both on the
  // wizard-to-dashboard transition AND on initial load when the user is
  // already configured, with the localStorage flag preventing double-
  // fires. Doing it here would race the wizard (hdr-bar hidden → skip)
  // and leave first-run users without a tour after they save settings.
})();
