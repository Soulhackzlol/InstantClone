// - Multi-step wizard state machine -----------------
// Steps are computed live so the Twitch step joins/leaves the sequence
// the instant the platform changes. We track the CURRENT step by id, not
// index, so a platform switch that removes the Twitch step can never
// strand the wizard on a panel that no longer exists.
let wizCurrent = 'welcome';
// Travel direction for the slide transition: 'fwd' on Next/initial,
// 'back' on Back. Read by CSS via #wizard[data-dir].
let wizDir = 'fwd';

// Every step that can ever appear, in order. The progress bar renders one
// permanent segment per entry; `wizStepIds()` is the ACTIVE subset for the
// current platform. Twitch is the only optional one today.
const WEL_STEPS = ['welcome', 'startup', 'delay', 'crash'];
const WIZ_ALL_STEPS = [...WEL_STEPS, 'obs', 'platform', 'key', 'twitch'];
const WIZ_STEP_NAMES = {welcome: 'Language', startup: 'Startup', delay: 'Delay ready', crash: 'If OBS crashes', obs: 'Your app',
  platform: 'Platform', key: 'Stream key', twitch: 'Twitch extras'};
let wizShownStep = '';
const wizVisited = new Set();

function wizStepIds(){
  const ids = [...WEL_STEPS, 'obs', 'platform', 'key'];
  if ($('w-platform').value === 'twitch') ids.push('twitch');
  return ids;
}

// The welcome's questions: what each one reads from /config, and what a pick
// posts back. Each saves as it is picked, so Back shows what was chosen.
const WEL_PICKS = {
  language: {current: c => c.ui_language || 'auto', fields: value => ({ui_language: value})},
  startup: {current: c => (c.start_with_windows ? 'on' : 'off'), fields: value => ({start_with_windows: value})},
  delay: {current: c => (c.auto_arm_on_connect ? String(c.auto_arm_delay_ms) : '0'),
    fields: value => (value === '0' ? {auto_arm_on_connect: 'false'} : {auto_arm_on_connect: 'true', auto_arm_delay_ms: value})},
  crash: {current: c => (c.crash_protection && c.crash_protection.enabled ? 'true' : 'false'),
    fields: value => ({'crash_protection.enabled': value})},
};
const welChosen = {};
const welValue = group => welChosen[group] ?? (lastConfig ? WEL_PICKS[group].current(lastConfig) : '');
function renderWelChoices(){
  $$('#wizard [data-wel]').forEach(choices => {
    const value = welValue(choices.dataset.wel);
    choices.querySelectorAll('.wel-choice').forEach(choice => {
      const on = choice.dataset.value === value;
      choice.classList.toggle('on', on);
      choice.setAttribute('aria-checked', on ? 'true' : 'false');
    });
  });
}
// One span per word, so the words can rise in turn.
function renderWelTitle(){
  const title = $('wel-title');
  if (title.children.length) return;
  const words = tr('Welcome to InstantClone').split(' ');
  title.setAttribute('aria-label', words.join(' '));
  title.replaceChildren(...words.flatMap((word, i) => {
    const span = document.createElement('span');
    span.className = 'wel-word';
    span.style.setProperty('--i', i);
    span.setAttribute('aria-hidden', 'true');
    span.textContent = word;
    return [span, ' '];
  }));
}
// Saves a pick that changed; false (with the old answer back) if it couldn't.
async function welSave(group, value, was){
  welChosen[group] = value;
  renderWelChoices();
  let j;
  try { j = await (await fetch('/config', {method:'POST', body:new URLSearchParams(WEL_PICKS[group].fields(value))})).json(); }
  catch (_){ j = {ok:false, error:"Couldn't reach InstantClone. Check it is running, then try again."}; }
  if (j.ok) return true;
  welChosen[group] = was;
  renderWelChoices();
  wizMsg(j.error, 'err');
  return false;
}
// The language a setting shows the page in, the way the server picks it
// (page_language in web.rs). Auto can already be what's on screen.
function languageOf(setting){
  if (setting !== 'auto') return setting;
  for (const tag of navigator.languages || []){
    const base = tag.split('-')[0].toLowerCase();
    if (base === 'es' || base === 'en') return base;
  }
  return 'en';
}
const LANG_FADE_MS = 140;
// A new language comes with the page's strings, so it loads the page again:
// what's on screen dips out, and it comes back in place without its entrance
// (data-lang-return on <html>, set before the first paint at the top of the page).
function reloadForLanguage(){
  const inWizard = !$('wizard').hidden;
  const tab = document.querySelector('.tab.on'), sub = document.querySelector('.tab-pane:not([hidden]) .sub-tab.on');
  try {
    sessionStorage.setItem('instantclone_lang_return', JSON.stringify({view: inWizard ? 'wizard' : 'dashboard', height: $('wizard').offsetHeight,
      tab: tab && tab.dataset.tab, sub: sub && sub.dataset.subtab}));
  } catch (_){}
  if (!motionOK()){ location.reload(); return; }
  document.documentElement.classList.add('lang-leaving');
  setTimeout(() => location.reload(), LANG_FADE_MS);
}
// The page is drawn again in its new language: show it. The wizard's step keeps
// the mark until it moves on (renderWizStep), or its entrance would start now.
// A draft nobody read (setup finished in another window meanwhile) goes too:
// it can hold a stream key.
function langReturnReady(){
  const back = window.IC_LANG_BACK;
  if (back && back.view === 'dashboard' && back.tab && !$('pane-card').hidden){
    showTab(back.tab);
    if (back.sub) showSubTab(back.tab, back.sub);
  }
  document.documentElement.classList.add('lang-ready');
  if ($('wizard').hidden) setTimeout(langReturnEnd, 320);
  try { sessionStorage.removeItem(WIZ_DRAFT); } catch (_){}
}
function langReturnEnd(){
  const root = document.documentElement;
  delete root.dataset.langReturn;
  root.classList.remove('lang-ready');
}
async function welPick(choice){
  const group = choice.parentElement.dataset.wel, value = choice.dataset.value, was = welValue(group);
  if (value === was || !(await welSave(group, value, was)) || group !== 'language') return;
  if (languageOf(value) === document.documentElement.lang) return;
  saveWizDraft();
  reloadForLanguage();
}
// What the steps hold, kept across the reload a language change needs. It
// is read straight back and removed, so the stream key rests here only for
// the moment the page takes to load, in this tab alone.
const WIZ_DRAFT = 'instantclone_wizard_draft';
function saveWizDraft(){
  try {
    sessionStorage.setItem(WIZ_DRAFT, JSON.stringify({app: wizApp, platform: $('w-platform').value, key: $('w-key').value,
      custom: $('w-custom').value, confirmed: wizObsConfirmed, visited: [...wizVisited], keyboard: wizOnKeyboard}));
  } catch (_){}
}
function restoreWizDraft(){
  let draft = null;
  try {
    draft = JSON.parse(sessionStorage.getItem(WIZ_DRAFT));
    sessionStorage.removeItem(WIZ_DRAFT);
  } catch (_){}
  if (!draft) return;
  wizPickApp(draft.app);
  $('w-platform').value = draft.platform;
  $('w-platform').dispatchEvent(new Event('change'));
  $('w-key').value = draft.key;
  $('w-custom').value = draft.custom;
  draft.visited.forEach(step => wizVisited.add(step));
  wizOnKeyboard = draft.keyboard;
  if (draft.confirmed) wizObsAdvance('connect');
}
// Arrow keys walk a question's answers, as in any radio group, and Enter on
// the marked answer goes on, so the keyboard needs no trip to Next.
const WEL_ARROWS = {ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1};
function welChoiceKey(event){
  const choice = event.target;
  if (!choice.classList.contains('wel-choice')) return;
  if (event.key === 'Enter' && choice.classList.contains('on')){
    event.preventDefault();
    wizNext();
    return;
  }
  const step = WEL_ARROWS[event.key];
  if (!step) return;
  event.preventDefault();
  const choices = [...choice.parentElement.children];
  choices[(choices.indexOf(choice) + step + choices.length) % choices.length].focus();
}
$('wizard').addEventListener('keydown', welChoiceKey);
// The card eases to the new step's height instead of jumping to it.
function easeWizHeight(from){
  const card = $('wizard'), to = card.offsetHeight;
  if (!from || from === to || !motionOK()) return;
  card.classList.add('wiz-easing');
  const ease = card.animate([{height: from + 'px'}, {height: to + 'px'}], {duration:320, easing:'cubic-bezier(.2,.7,.2,1)'});
  ease.onfinish = ease.oncancel = () => card.classList.remove('wiz-easing');
}

// Segmented progress. One PERMANENT segment per possible step (segments are
// never added/removed) so the optional Twitch segment animates its
// collapse/expand instead of popping in and out. Done means seen; a segment
// past what's done can't be pressed. Inactive steps (Twitch when not chosen)
// collapse.
function syncWizSegments(){
  const prog = $('wiz-progress');
  if (prog.children.length !== WIZ_ALL_STEPS.length){
    prog.replaceChildren(...WIZ_ALL_STEPS.map(sid => {
      const seg = document.createElement('button');
      seg.type = 'button';
      seg.onclick = () => wizGoTo(sid);
      const tip = document.createElement('span');
      tip.className = 'wiz-seg-tip';
      tip.textContent = WIZ_STEP_NAMES[sid];
      seg.appendChild(tip);
      return seg;
    }));
  }
  const ids = wizStepIds(), furthest = wizFurthestStep(ids);
  WIZ_ALL_STEPS.forEach((sid, idx) => {
    let cls = 'wiz-seg';
    if (!ids.includes(sid)) cls += ' collapsed';
    else if (sid === wizCurrent) cls += ' on';
    else if (wizVisited.has(sid)) cls += ' done';
    const seg = prog.children[idx];
    seg.className = cls;
    seg.disabled = ids.indexOf(sid) > furthest;
    if (sid === wizCurrent) seg.setAttribute('aria-current', 'step');
    else seg.removeAttribute('aria-current');
  });
}

function renderWizStep(){
  const fromHeight = $('wizard').offsetHeight;
  const ids = wizStepIds();
  if (!ids.includes(wizCurrent)) wizCurrent = ids[ids.length - 1];
  const i = ids.indexOf(wizCurrent);
  $('wizard').dataset.dir = wizDir;
  // The welcome's questions are their own headings: no "Step 3 of 8" above.
  $('wizard').toggleAttribute('data-intro', WEL_STEPS.includes(wizCurrent));
  const moved = wizShownStep !== wizCurrent;
  if (moved && wizShownStep) langReturnEnd();
  wizShownStep = wizCurrent;
  wizVisited.add(wizCurrent);
  // Show the active panel only.
  document.querySelectorAll('#wizard .wiz-step').forEach(el => {
    el.hidden = el.dataset.wstep !== wizCurrent;
  });
  // Stop the optional OBS-connection poll when leaving its step, and clear a
  // stuck spinner so returning shows the idle button (a confirmed ✓ stays).
  if (wizCurrent !== 'obs'){
    wizStopObsCheckTimer();
    const wov = $('wiz-obs-verify');
    if (wov && wov.dataset.state === 'waiting') wov.dataset.state = 'idle';
  }
  const last = (i === ids.length - 1);
  $('wiz-back').hidden = (i === 0);
  $('wiz-next').innerHTML = last ? 'Save &amp; Start' : 'Next &rarr;';
  syncWizNext();
  // The welcome step is its own heading (CSS hides these); the rest show a
  // plain position counter (the progress bar shows the rest visually).
  $('wiz-title').textContent = 'Set up your stream';
  $('wiz-lead').textContent = `Step ${i + 1} of ${ids.length}`;
  if (wizCurrent === 'welcome') renderWelTitle();
  renderWelChoices();
  // Name the platform in the key-step heading.
  const pn = $('w-key-platform-name');
  if (pn){
    const opt = $('w-platform').selectedOptions[0];
    pn.textContent = opt ? opt.textContent : 'platform';
  }
  clearWizMsg();
  wireAllDisclosures($('wizard'));
  reserveCopyButtons($('wizard'));
  if (wizCurrent === 'key'){
    syncWizStreamlabsImport();
    syncWizTwitchKey();
    refreshStreamlabs();
    // With "Get my key from Twitch" on offer, that's the first thing to do.
    setTimeout(() => {
      const twitchBtn = $('w-tw-key-btn');
      const k = twitchBtn && twitchBtn.offsetParent ? twitchBtn : $('w-key');
      if (k && !k.disabled) k.focus();
    }, 60);
  }
  // A step that just opened starts at the top, and someone on the keyboard
  // lands on its answer. A mouse doesn't need the focus ring.
  if (moved){
    window.scrollTo(0, 0);
    const answer = document.querySelector(`[data-wstep="${wizCurrent}"] .wel-choice.on`);
    if (answer && wizOnKeyboard) answer.focus({preventScroll: true});
  }
  easeWizHeight(fromHeight);
}
let wizOnKeyboard = false;
$('wizard').addEventListener('keydown', () => { wizOnKeyboard = true; });
$('wizard').addEventListener('pointerdown', () => { wizOnKeyboard = false; });
// A key typed or wiped changes how far the bar can go, and answers the last complaint.
['w-key', 'w-custom'].forEach(id => $(id).addEventListener('input', () => { clearWizMsg(); syncWizSegments(); }));

function wizNext(){
  if (!validateWizStep(wizCurrent)) return;
  const ids = wizStepIds();
  const i = ids.indexOf(wizCurrent);
  if (i >= ids.length - 1){ saveWizard(); return; }
  wizDir = 'fwd';
  wizCurrent = ids[i + 1];
  renderWizStep();
}

function wizBack(){
  const ids = wizStepIds();
  const i = ids.indexOf(wizCurrent);
  if (i > 0){ wizDir = 'back'; wizCurrent = ids[i - 1]; renderWizStep(); }
}

// A progress segment: back to any step, and forward over the ones already done.
function wizGoTo(step){
  const ids = wizStepIds(), target = ids.indexOf(step);
  if (step === wizCurrent || target < 0 || target > wizFurthestStep(ids)) return;
  wizDir = target > ids.indexOf(wizCurrent) ? 'fwd' : 'back';
  wizCurrent = step;
  renderWizStep();
}

// Optional, local-only "did OBS reach us?" check. Polls /state for ingest_alive
// (OBS publishing into our RTMP server) - a passive read that sends nothing
// anywhere. No destination is saved during the wizard, and OBS points only at
// 127.0.0.1, so nothing can egress to a platform while this runs.
let _obsCheckTimer = null;
function wizStopObsCheckTimer(){ if (_obsCheckTimer){ clearInterval(_obsCheckTimer); _obsCheckTimer = null; } }
function wizStartObsCheck(){
  const panel = $('wiz-obs-verify'); if (!panel) return;
  panel.dataset.state = 'waiting';
  wizStopObsCheckTimer();
  _obsCheckTimer = setInterval(async () => {
    try {
      const s = await (await fetch('/state')).json();
      if (s && s.ingest_alive){
        wizStopObsCheckTimer();
        panel.dataset.state = 'ok';
        const codec = (s.video_codec && s.video_codec !== 'unknown') ? ' (' + s.video_codec + ')' : '';
        const ok = panel.querySelector('.wov-status-ok');
        if (ok) ok.textContent = 'OBS is reaching InstantClone' + codec + '. Nothing left your PC. You can stop streaming now.';
        const node = document.querySelector('.obs-node[data-node="verify"]');
        if (node) node.dataset.verified = '1';   // turn the step's dot green ✓
      }
    } catch(_){ /* transient fetch error - keep polling */ }
  }, 700);
}
function wizStopObsCheck(){
  wizStopObsCheckTimer();
  const panel = $('wiz-obs-verify');
  if (panel && panel.dataset.state !== 'ok') panel.dataset.state = 'idle';
}

// - Guided OBS flow (the 2-node stepper) ---------------
const OBS_FLOW = ['connect','verify'];
// True once the user clicks "I've set this up in OBS" (advances past connect).
let wizObsConfirmed = false;
function wizObsNode(node){ return document.querySelector(`.obs-node[data-node="${node}"]`); }
// Mark a node done and open the next one.
function wizObsAdvance(node){
  const el = wizObsNode(node); if (!el) return;
  el.dataset.state = 'done';
  const next = OBS_FLOW[OBS_FLOW.indexOf(node) + 1];
  if (next){ const nx = wizObsNode(next); if (nx && nx.dataset.state !== 'done') nx.dataset.state = 'active'; }
  if (node === 'connect'){ wizObsConfirmed = true; clearWizMsg(); syncWizNext(); }
}
// One bright button at a time: on the OBS step, Next stays quiet until the
// app is connected, so the eye goes to what connects it first.
function syncWizNext(){
  const ready = wizCurrent !== 'obs' || !wizStepProblem('obs');
  $('wiz-next').classList.toggle('ic-btn-primary', ready);
  $('wiz-next').classList.toggle('ic-btn-ghost', !ready);
  syncWizSegments();
}
// Re-open a completed node to review or redo it; collapses whatever was open.
function wizObsReopen(node){
  const el = wizObsNode(node);
  if (!el || el.dataset.state === 'locked' || el.dataset.state === 'active') return;
  OBS_FLOW.forEach(n => { const e = wizObsNode(n); if (e && e.dataset.state === 'active') e.dataset.state = 'done'; });
  el.dataset.state = 'active';
}

// What a step still needs before moving on, or '' when nothing. Only two can
// hold it back: connecting the app, and the key. The others have an answer
// from the start.
function wizStepProblem(id){
  if (id === 'obs'){
    if (wizApp === 'streamlabs') return slInfo && slInfo.enabled ? '' : 'Connect Streamlabs above to continue.';
    if (wizObsConfirmed) return '';
    return obsInstalls && !obsInstallList().length
      ? 'Install OBS and open it once to continue.'
      : 'Set OBS up above, then click "I\'ve set this up in OBS" to continue.';
  }
  if (id !== 'key') return '';
  const platform = $('w-platform').value;
  const key = $('w-key').value.trim(), custom = $('w-custom').value.trim();
  // Kick needs BOTH a Server URL (from its dashboard) and a key, like a
  // custom endpoint that also has a separate key field.
  if (platform === 'kick' && !custom) return 'Paste your Kick Server URL (from the Kick dashboard) to continue.';
  if (platform === 'custom' && !custom) return 'Enter your custom RTMP URL to continue.';
  if (platform === 'twitch' && wizKeyImported) return '';
  if (platform !== 'custom' && !key) return 'Paste your stream key to continue.';
  return '';
}
function validateWizStep(id){
  const problem = wizStepProblem(id);
  if (!problem) return true;
  wizMsg(problem, 'err');
  if (id === 'obs'){
    const c = $('wiz-obs-confirm');   // draw the eye to the button that unblocks
    if (c){ c.classList.remove('nudge'); void c.offsetWidth; c.classList.add('nudge'); }
  }
  return false;
}
// How far the progress bar can go: every step already seen, up to the first
// that still needs something. Nothing typed is lost going back and forth.
function wizFurthestStep(ids){
  const stop = ids.findIndex(id => !wizVisited.has(id) || wizStepProblem(id));
  return stop < 0 ? ids.length - 1 : stop;
}

function wizMsg(text, kind){ $('w-msg').innerHTML = `<span class="${kind || ''}">${text}</span>`; }
function clearWizMsg(){ $('w-msg').innerHTML = ''; }

// A tiny critically-damped spring driving one scalar (px height). Springs are
// interruptible and velocity-aware by nature: re-targeting mid-flight keeps the
// live position AND velocity, so a rapid open/close reverses without the
// "brick wall" a re-started CSS ease produces (velocity snaps to 0 there).
// Damping ratio is fixed at 1.0 - no overshoot, because a click that just
// revealed a panel carried no momentum to justify a bounce (Apple, Designing
// Fluid Interfaces). `response` is the settle feel in seconds, not a duration.
function makeSpring(onFrame, response){
  const w = 2 * Math.PI / response;   // natural frequency; zeta = 1 (critical)
  let value = 0, velocity = 0, target = 0, raf = 0, last = 0;
  function frame(now){
    let dt = (now - last) / 1000; last = now;
    if (dt > 0.064) dt = 0.064;       // clamp after a tab stall / dropped frames
    const steps = Math.max(1, Math.ceil(dt / 0.008));  // substeps keep it stable
    const h = dt / steps;
    for (let i = 0; i < steps; i++){
      velocity += (-w * w * (value - target) - 2 * w * velocity) * h;
      value += velocity * h;
    }
    if (Math.abs(value - target) < 0.3 && Math.abs(velocity) < 2){
      value = target; velocity = 0; raf = 0; onFrame(value, true); return;
    }
    onFrame(value, false);
    raf = requestAnimationFrame(frame);
  }
  return {
    set(v){ value = v; },
    to(t){ target = t; if (!raf){ last = performance.now(); raf = requestAnimationFrame(frame); } },
    get active(){ return !!raf; }
  };
}

// Reusable disclosure reveal. Upgrades any <details class="ic-disclose"> into a
// spring-animated collapsible while keeping native keyboard + screen-reader
// semantics. The wrapping "sleeve" height is spring-driven (interruptible,
// velocity-carrying); the padded body rides a composited opacity + translateY.
// The native <details> is pinned open so content is always mounted and
// measurable; the `is-open` class drives the visual state and the arrow.
const DISC_RESPONSE = 0.34;   // spring settle feel, seconds (see makeSpring)

function wireDisclosure(det){
  if (det._wired) return;
  const summary = det.querySelector(':scope > summary');
  const body = summary && summary.nextElementSibling;
  if (!summary || !body) return;
  det._wired = true;
  det.classList.add('disc-ready');   // marks it as spring-managed (see CSS fallback)

  const sleeve = document.createElement('div');
  sleeve.className = 'disc-sleeve';
  body.parentNode.insertBefore(sleeve, body);
  sleeve.appendChild(body);

  const startOpen = det.hasAttribute('open');
  det.open = true;                                    // keep content mounted
  det.classList.toggle('is-open', startOpen);
  summary.setAttribute('aria-expanded', String(startOpen));
  sleeve.style.height = startOpen ? 'auto' : '0px';

  const reduced = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // Natural height without a visible flash: swap to auto, read, restore.
  // Layout read only, no paint between, so nothing flickers.
  function naturalHeight(){
    const prev = sleeve.style.height;
    sleeve.style.height = 'auto';
    const h = sleeve.getBoundingClientRect().height;
    sleeve.style.height = prev;
    return h;
  }

  const spring = makeSpring((v, done) => {
    // When fully settled open, release to `auto` so later reflow (font swap,
    // rewrap, resize) is never clipped to a stale pixel height.
    if (done && det.classList.contains('is-open')) sleeve.style.height = 'auto';
    else sleeve.style.height = Math.max(0, v) + 'px';
  }, DISC_RESPONSE);

  function toggle(){
    const open = !det.classList.contains('is-open');
    det.classList.toggle('is-open', open);
    summary.setAttribute('aria-expanded', String(open));
    if (reduced()){ sleeve.style.height = open ? 'auto' : '0px'; return; }
    // Only re-seed the spring's value from the DOM when it's at rest (e.g.
    // height was 'auto'); mid-flight we leave value/velocity intact so the
    // reversal carries momentum instead of jumping.
    if (!spring.active) spring.set(sleeve.getBoundingClientRect().height);
    spring.to(open ? naturalHeight() : 0);
    if (open) revealOpened(det);
  }
  det._toggle = toggle;

  summary.addEventListener('click', (e) => { e.preventDefault(); toggle(); });
}

function wireAllDisclosures(root){
  (root || document).querySelectorAll('details.ic-disclose').forEach(wireDisclosure);
}

// Programmatic open/close (e.g. an inline "details" link). Wires on demand.
function toggleDisclosure(id){
  const det = document.getElementById(id);
  if (!det) return;
  wireDisclosure(det);
  det._toggle();
}

function fillWizardPlatforms(){
  const sel = $('w-platform'); sel.innerHTML = '';
  const tiles = $('w-platform-tiles'); if (tiles) tiles.innerHTML = '';
  // The test sink is excluded here on purpose: the wizard writes the
  // legacy single-destination config (which requires a key), and its
  // job is wiring your REAL platform. The sink lives in Destinations.
  platforms.filter(p => p.slug !== 'sink').forEach((p, idx) => {
    const o = document.createElement('option');
    o.value = p.slug; o.textContent = p.label; sel.appendChild(o);
    if (!tiles) return;
    // A tile drives the hidden select: set value + dispatch 'change' so
    // every existing listener (custom reveal, key help) fires unchanged.
    const b = document.createElement('button');
    b.type = 'button'; b.className = 'w-ptile'; b.dataset.slug = p.slug;
    b.setAttribute('role', 'radio');
    b.style.setProperty('--dc', PLATFORM_COLOR[p.slug] || 'var(--line-2)');
    // Staggered entrance when the platform step appears.
    b.style.animationDelay = (idx * 40) + 'ms';
    b.innerHTML = `<span class="w-ptile-check" aria-hidden="true">✓</span>`
      + `<span class="w-ptile-badge">${platformMark(p.slug)}</span>`
      + `<span class="w-ptile-label"></span>`;
    b.querySelector('.w-ptile-label').textContent = p.label;
    b.addEventListener('click', () => {
      sel.value = p.slug; sel.dispatchEvent(new Event('change'));
    });
    tiles.appendChild(b);
  });
  if (!sel._wired){
    sel._wired = true;
    sel.addEventListener('change', () => {
      const isCustom = sel.value === 'custom';
      const isKick = sel.value === 'kick';
      // Custom and Kick both need a Server URL field. Kick's Server is
      // shown per-streamer in its dashboard, so it can't be hardcoded.
      $('w-custom-wrap').hidden = !isCustom && !isKick;
      const wclabel = $('w-custom-label'), wcinput = $('w-custom');
      if (isKick){
        if (wclabel) wclabel.textContent = 'Kick Server URL (from your Kick dashboard)';
        if (wcinput) wcinput.placeholder = 'rtmps://…global-contribute.live-video.net:443/app';
      } else {
        if (wclabel) wclabel.textContent = 'Custom RTMP URL (without the stream key)';
        if (wcinput) wcinput.placeholder = 'rtmp://127.0.0.1:1936/live';
      }
      // Only Custom destinations can embed the stream key in the URL
      // and skip the key field. Twitch/YouTube/Kick keys are always
      // required, so the "(optional…)" hint would mislead - hide it
      // unless we're on Custom.
      const optHint = $('w-key-optional-hint');
      if (optHint) optHint.hidden = !isCustom;
      renderKeyHelp(sel.value, 'w-key-help');
      syncWizPlatformTiles();
      // Re-render the step chrome: changing to/from Twitch adds or drops
      // the Twitch step, so the progress bar and Next label must update.
      if (!$('wizard').hidden) renderWizStep();
    });
    // VOD-audio opt-in reveals its OBS how-to inline.
    // Enter in the key field advances, like a real form.
    const keyInput = $('w-key');
    if (keyInput) keyInput.addEventListener('keydown', e => {
      if (e.key === 'Enter'){ e.preventDefault(); wizNext(); }
    });
  }
  const optHint = $('w-key-optional-hint');
  if (optHint) optHint.hidden = sel.value !== 'custom';
  renderKeyHelp(sel.value, 'w-key-help');
  syncWizPlatformTiles();
}

// Reflect the hidden select's value onto the tiles. Driven by the
// select's 'change' event, so tile clicks, the initial config load, and
// any programmatic set all keep the highlight correct.
function syncWizPlatformTiles(){
  const sel = $('w-platform'); const tiles = $('w-platform-tiles');
  if (!tiles) return;
  tiles.querySelectorAll('.w-ptile').forEach(b => {
    const on = b.dataset.slug === sel.value;
    b.classList.toggle('on', on);
    b.setAttribute('aria-checked', on ? 'true' : 'false');
  });
}

// The wizard's OBS card is the same list as the OBS tab's.
function refreshWizObsStatus(){ return refreshObsRegisterStatus(); }
// Re-check when the window regains focus - e.g. the user alt-tabbed to
// OBS and closed it - so its "close it first" clears itself.
window.addEventListener('focus', () => {
  const wiz = $('wizard');
  if (wiz && !wiz.hidden && wizCurrent === 'obs') refreshWizObsStatus();
});

// Per-platform "where do I get my stream key + what gotchas to know"
// hint. Rendered under the key input in both the wizard and the
// destination form. Source of truth is /platforms (key_url, key_help,
// tip). For Kick we mark the tip as a warning because its no-B-frames
// rule is the kind of thing that drops the stream silently - worth a
// visual nudge, not just plain grey text.
// Break a long single-string platform tip into breathable paragraphs at
// sentence boundaries (~130+ chars each), so a 700-char wall like Twitch's
// transcoding note reads as a few short paragraphs instead of one slab.
// Short tips pass through untouched.
function formatTip(tip){
  // Translated whole, before it splits: the dictionary has the whole tip.
  const t = tr((tip || '').trim());
  if (t.length <= 220) return `<p>${t}</p>`;
  const sentences = t.split(/(?<=\.)\s+(?=[A-Z])/);
  const paras = []; let buf = '';
  for (const s of sentences){
    buf = buf ? buf + ' ' + s : s;
    if (buf.length >= 130){ paras.push(buf); buf = ''; }
  }
  if (buf) paras.push(buf);
  return paras.map(s => `<p>${s}</p>`).join('');
}

function renderKeyHelp(slug, hostId){
  const host = document.getElementById(hostId);
  if (!host) return;
  const p = platforms.find(x => x.slug === slug);
  if (!p || (!p.key_url && !p.tip)){ host.hidden = true; host.innerHTML = ''; return; }
  const parts = [];
  if (p.key_url){
    const where = p.key_help || 'Where to find your stream key';
    parts.push(`<a class="w-key-link" href="${p.key_url}" target="_blank" rel="noopener">${where} →</a>`);
  }
  if (p.tip){
    // Long platform-specific gotcha goes behind a disclosure. The
    // summary is intentionally short ("Things to know about Twitch")
    // so it doesn't crowd the key input; the user only sees the full
    // paragraph if they care enough to click. Kick's tip is the
    // exception - its B-frames rule silently drops the stream, so
    // we expand it by default for that one platform.
    const isKick = slug === 'kick';
    // The sink's tip IS the feature explanation (what it does, where
    // the player lives) - open it by default like Kick's warning.
    const isSink = slug === 'sink';
    const label = isKick ? '⚠ Things to know about Kick'
                : isSink ? 'What is the test destination?'
                : `Things to know about ${p.label || 'this platform'}`;
    const cls = isKick ? 'w-key-tip warn' : 'w-key-tip';
    parts.push(`
      <details class="ic-disclose w-key-disclose"${isKick || isSink ? ' open' : ''}>
        <summary class="${cls}">${label}</summary>
        <div class="ic-disclose-body">${formatTip(p.tip)}</div>
      </details>
    `);
  }
  host.innerHTML = parts.join('');
  host.hidden = false;
  wireAllDisclosures(host);   // freshly injected <details> need the spring wiring
}

// Wizard escape hatch. Reveals the dashboard without writing any
// destination to the server, so `configured` stays false on disk and
// the wizard returns next launch (gentle nudge, not a permanent skip).
// Triggers the same first-run tour as the configured path so the user
// still gets walked through the UI before being left alone.
let wizardSkipped = false;
function skipWizard(){
  wizardSkipped = true;
  $('wizard').hidden = true;
  $('hdr-bar').hidden = false;
  $('hero').hidden = false;
  $('tabs').hidden = false;
  $('pane-card').hidden = false;
  $('ic-foot').hidden = false; fitPaneCard();
  loadDestinations(); loadProfiles(); loadOverlays();
  const cur = document.querySelector('.tab.on') || document.querySelector('.tab[data-tab="destinations"]');
  if (cur) showTab(cur.dataset.tab);
  setTimeout(moveTabInd, 50);
  try {
    if (!localStorage.getItem('instantclone_onboarded')){
      setTimeout(() => openTour(0), 900);
    }
  } catch(_){}
}

async function saveWizard(){
  const platform = $('w-platform').value, key = $('w-key').value.trim(), custom = $('w-custom').value.trim();
  // The Streamlabs import already saved the Twitch destination.
  if (platform === 'twitch' && wizKeyImported){
    wizMsg('Saved. Starting up…', 'ok');
    setTimeout(loadConfig, 500);
    return;
  }
  // Defensive: the key step validates before you can reach Save, but if we
  // ever land here empty, bounce back to that step rather than erroring.
  // Kick needs both a Server URL and a key; custom needs the URL; everyone
  // else needs a key.
  const kickMissing = platform === 'kick' && (!custom || !key);
  if (kickMissing || (platform !== 'custom' && platform !== 'kick' && !key) || (platform === 'custom' && !custom)){
    wizCurrent = 'key'; renderWizStep();
    const m = platform === 'kick' ? 'Paste your Kick Server URL and stream key to continue.'
      : platform === 'custom' ? 'Enter your custom RTMP URL to continue.'
      : 'Paste your stream key to continue.';
    wizMsg(m, 'err');
    return;
  }
  const params = {platform, stream_key:key, custom_egress_url:custom};
  // The wizard's VOD-audio toggle is informational only: on OBS 32.2+ the
  // VOD track comes from the unlocker script (set up after the wizard, needs
  // no InstantClone flag), so we deliberately do NOT set the legacy
  // vod_audio / IVS-override flag here. The user can still enable the old
  // method later per-destination for OBS before 32.2.
  const btn = $('wiz-next'); if (btn){ btn.disabled = true; }
  // A request that fails outright (the app restarting, a lost connection)
  // reads as an error too, so the button comes back instead of staying off.
  let j;
  try { j = await(await fetch('/config',{method:'POST',body:new URLSearchParams(params)})).json(); }
  catch (_){ j = {ok:false, error:"Couldn't reach InstantClone. Check it is running, then try again."}; }
  if (j.ok){ wizMsg('Saved. Starting up…', 'ok'); nudgeObsWiringIfNeeded(); setTimeout(loadConfig, 500); }
  else { if (btn) btn.disabled = false; wizMsg(j.error, 'err'); }
}

// Soft catch for the beginner who breezed past the OBS step: if OBS still isn't
// wired to InstantClone when the wizard finishes, remind them how - never a
// blocker. A registered user already saw "restart OBS + pick InstantClone", so
// only the not-registered case (the actual silent dead-end onto OFFLINE) nudges.
async function nudgeObsWiringIfNeeded(){
  try {
    // Streaming from Streamlabs: OBS needs no wiring.
    const sl = await slStatus();
    if (sl && sl.enabled) return;
    await loadObsInstalls();
    const found = obsInstallList();
    if (found.some(i => i.state === 'current')) return;
    const srv = ($('obs-srv') && $('obs-srv').textContent) || 'rtmp://localhost:1935/live';
    const how = !found.length
      ? `In OBS → Settings → Stream: Service = Custom, Server = ${srv}.`
      : `In OBS → Settings → Stream: Service = Custom, Server = ${srv}. Or close OBS and register InstantClone on the OBS tab.`;
    toast('One more step to go live: point OBS at InstantClone. ' + how, 'info', 11000);
  } catch(_){ /* status endpoint unreachable; the first-run tour still covers OBS wiring */ }
}
function flashFieldErrors(message){
  $$('input.field-error').forEach(el => el.classList.remove('field-error'));
  if (!message) return;
  const lower = message.toLowerCase();
  Object.entries(SETTINGS_FIELDS).forEach(([k, id]) => {
    if (lower.includes(k.toLowerCase())){
      const el = $(id); if (!el) return;
      el.classList.add('field-error');
      el.addEventListener('input', () => el.classList.remove('field-error'), {once:true});
    }
  });
}

