// - Spotlight onboarding tour --------------------─
// Each step optionally points at a real DOM element. The SVG mask
// punches a hole over it, the tooltip floats next to it. Steps with
// `tab` switch tabs first. Steps with `target: null` show a centered
// modal-style tooltip (used for welcome + wrap-up).
const TOUR = [
  {
    target: null, side: 'center',
    title: 'Welcome to InstantClone',
    get body(){ const app = encoderApp() || 'OBS or Streamlabs'; return `
      <p>InstantClone sits between ${app} and your streaming platforms (Twitch, YouTube, Kick…). It lets you <b>add/remove/change a stream delay live</b> without disconnecting ${app}.</p>
      <p>This 30-second tour points to the bits you'll actually use. Hit <b>Esc</b> or <b>Skip</b> at any time.</p>
      <div class="tour-tip"><strong>The big picture:</strong> ${app} → InstantClone → your platforms. You stay in ${app}, you change the delay here.</div>
    `; },
  },
  {
    target: '#hero-readout', side: 'right',
    title: 'The delay readout',
    body: `
      <p>Big number = your <b>current delivered delay</b>. The pill above + aurora glow tell you the state:</p>
      <ul>
        <li><b>OFFLINE</b>: OBS isn't streaming to InstantClone yet (where you probably are right now)</li>
        <li><b>LIVE · no delay</b>: your stream goes out as it is</li>
        <li><b>ARMING</b>: saving video until there's enough for your delay</li>
        <li><b>ARMED</b>: ready, press Activate</li>
        <li><b>ACTIVE</b>: delay is live</li>
      </ul>
    `,
  },
  {
    target: '.cp-input-row', side: 'bottom',
    title: 'Set a custom delay',
    body: `
      <p>Type how many seconds of delay you want. Use the <b>−</b> / <b>+</b> buttons to nudge.</p>
      <p>The range above it is the longest delay the buffer holds at your bitrate, up to 1 hour. Need more? Make the buffer bigger in <b>System</b>. You can also save commonly-used delays as <b>Profiles</b> for one-click insta-arm.</p>
    `,
  },
  {
    target: '#cp-cta', side: 'top',
    title: 'Arm → Activate (two-phase)',
    body: `
      <p>The big button changes meaning by state:</p>
      <ul>
        <li><b>Arm</b> → saves video for the delay while viewers still see you live (you can still cancel)</li>
        <li><b>Activate</b> → once it's ready, viewers switch to the delayed stream</li>
        <li><b>Cut</b> → viewers go back to live. The saved video stays, so the next Activate is instant</li>
        <li><b>⏱ Cut after</b> → appears while the delay is active. Press it the second your match reaction ends: everything up to that press still airs, then the delay cuts itself to live. No more counting 30 seconds in your head.</li>
      </ul>
      <div class="tour-tip"><strong>Shortcut:</strong> press <b>Space</b> to fire this button when nothing else is selected (click an empty part of the page first). Fields and buttons keep Space for themselves.</div>
    `,
  },
  {
    target: '#tabs', side: 'bottom',
    title: 'Tabs',
    body: `
      <p>Everything else lives in the tabs below. Destinations is the most important: that's where Twitch, YouTube, Kick etc. live.</p>
    `,
  },
  {
    // Dynamic: highlight a real destination card once one exists (so the
    // spotlight isn't a viewport-filling grid), otherwise the Add button.
    target: () => document.querySelector('#dest-grid .dcard')
      || document.querySelector('#dest-add-btn')
      || document.querySelector('#dest-grid'),
    side: 'top', tab: 'destinations',
    title: 'Destinations',
    body: `
      <p>Every platform you add gets a colour-coded card with a live bitrate sparkline and a status pill. Hit <b>+ Add destination</b> to create one.</p>
      <p>Stream keys are <b>per-destination</b> and live here. OBS only needs a placeholder identifier.</p>
      <details class="ic-disclose">
        <summary>Vertical (9:16) for Shorts / mobile</summary>
        <div class="ic-disclose-body">
          <p>Set any non-Twitch destination to <b>Vertical</b> and it gets the 9:16 canvas OBS sends with Enhanced Broadcasting. Set it up once in OBS: <b>Settings → Stream</b>, turn on <b>Enhanced Broadcasting</b> (<b>Multi-track via InstantClone</b> on the InstantClone service), and pick your vertical canvas (for example Aitum Vertical) under <b>Additional canvas</b>. It works with or without Twitch. While OBS streams without it, the card says what's missing.</p>
        </div>
      </details>
      <div class="tour-tip"><strong>Streaming vs Connected:</strong> Streaming = bytes flowing. Connected = TCP up, but OBS hasn't sent a frame yet.</div>
    `,
  },
  {
    target: '.tab[data-tab="obs"]', side: 'bottom', tab: 'obs',
    title: 'Your streaming apps',
    get body(){ return `
      <p>This tab manages the streaming apps on this PC. Every OBS and Streamlabs Desktop it finds shows up here: set each one up once and it streams to InstantClone, key included.</p>
      <p>Here you can also add the <b>OBS dock</b> to run the delay from inside OBS, show your chat in OBS and check OBS's recommended settings. Another app or a custom RTMP server? Its manual setup is at the bottom.</p>
    ` + (slInfo && slInfo.enabled ? `
      <p>Streamlabs is connected on this tab: go live from Streamlabs' <b>Go Live</b> button as usual, signed in with Twitch, and the stream comes through InstantClone.</p>
      <p style="font-size:12px;color:var(--fg-3);margin-top:8px">Until you go live, the dashboard reads <b>OFFLINE</b>. That's expected, not a mistake.</p>
      <div class="tour-tip"><strong>Twitch only inside Streamlabs:</strong> it's just the way in. Your real platform keys live in <b>Destinations</b>, with or without Twitch.</div>
    ` : `
      <p>Then open OBS and press <b>Start Streaming</b>.</p>
      <p style="font-size:12px;color:var(--fg-3);margin-top:8px">Until you Start Streaming, the dashboard reads <b>OFFLINE</b>. That's expected, not a mistake.</p>
    `); },
  },
  {
    target: '.tab[data-tab="overlay"]', side: 'bottom', tab: 'overlay',
    title: 'Everything viewers see',
    body: `
      <p>Four pages, by what they show: <b>Delay</b> (telling viewers you're on a delay), <b>Alerts</b> (follows, subs, raids and more), <b>Goals and timers</b> (a sub goal, a subathon) and <b>Custom</b> (anything else, like a starting-soon screen).</p>
      <p>Each one is a browser source: <b>Add to OBS</b> puts it in your scene in one click. <b>Overlay Studio</b> changes how any of them look, layer by layer, and previews it in every delay state.</p>
    `,
  },
  {
    target: '#hdr-compact', side: 'bottom',
    title: 'Need more room?',
    body: `
      <p>This button folds the delay panel into one slim bar, so the tabs get the screen. The state, the delay and the main button stay in it.</p>
      <p>Press it again to bring the big panel back.</p>
    `,
  },
  {
    target: '#hdr-help', side: 'bottom', tab: 'destinations',
    title: 'You\'re ready',
    body: `
      <p>That's everything. Profiles, System, Logs are self-explanatory once you've used the rest.</p>
      <p>This <b>?</b> button brings the tour back any time, and <b>quick guides</b> that walk you to one thing: connecting OBS, alerts, the VOD audio and more.</p>
      <div class="tour-tip"><strong>Need help?</strong> Press <b>Copy diagnostics</b> in the Logs tab and paste it into a bug report. Secrets are left out.</div>
    `,
  },
];

let tourIdx = 0;
let tourResizeHandler = null;
let tourScrollHandler = null;

let tourFoldedHero = false;
function openTour(idx){
  tourIdx = idx || 0;
  // The tour points at the delay panel's parts: it shows them all, and
  // folds it back up after if it was folded.
  if (heroCompact()){ tourFoldedHero = true; setHeroCompact(false, false); }
  $('tour').hidden = false;
  $('hdr-help-hint').hidden = true;
  try { localStorage.setItem('instantclone_onboarded','1'); } catch(_){}
  // Reposition on resize and scroll so the spotlight tracks the target.
  if (!tourResizeHandler){
    tourResizeHandler = () => positionTour();
    tourScrollHandler = () => positionTour();
    window.addEventListener('resize', tourResizeHandler);
    window.addEventListener('scroll', tourScrollHandler, true);
  }
  renderTour();
}

function closeTour(){
  $('tour').hidden = true;
  if (tourFoldedHero){ tourFoldedHero = false; setHeroCompact(true, true); }
  if (tourResizeHandler){
    window.removeEventListener('resize', tourResizeHandler);
    window.removeEventListener('scroll', tourScrollHandler, true);
    tourResizeHandler = null; tourScrollHandler = null;
  }
}

function tourNext(){
  if (tourIdx < TOUR.length - 1) { tourIdx++; renderTour(); }
  else closeTour();
}
function tourPrev(){ if (tourIdx > 0){ tourIdx--; renderTour(); } }

function renderTour(){
  const t = TOUR[tourIdx];
  $('tour-step').textContent = `${tourIdx + 1} / ${TOUR.length}`;
  $('tour-title').textContent = t.title;
  $('tour-body').innerHTML = t.body;
  $('tour-prev').disabled = (tourIdx === 0);
  $('tour-next').textContent = (tourIdx === TOUR.length - 1) ? 'Got it' : 'Next →';
  // If this step belongs to a different tab, switch first so the
  // target element exists before we try to position the spotlight.
  if (t.tab) showTab(t.tab);
  // Slight delay so tab-switch animation lays out, then position.
  requestAnimationFrame(() => requestAnimationFrame(positionTour));
}

function positionTour(){
  const t = TOUR[tourIdx]; if (!t) return;
  const hl = $('tour-highlight');
  const tip = $('tour-tooltip');
  const mask = document.getElementById('tour-mask-rect-hole');

  // Centered mode: no target, just centre the tooltip and hide the spotlight.
  if (!t.target){
    hl.classList.add('hidden');
    if (mask){ mask.setAttribute('width', '0'); mask.setAttribute('height', '0'); }
    tip.dataset.side = 'center';
    tip.style.left = '50%';
    tip.style.top  = '50%';
    return;
  }

  // Target may be a CSS selector or a function returning the best element
  // for the current state (e.g. a real destination card once some exist).
  const el = typeof t.target === 'function' ? t.target() : document.querySelector(t.target);
  if (!el){
    // Target missing (maybe hidden) - fall back to centered.
    hl.classList.add('hidden');
    if (mask){ mask.setAttribute('width', '0'); mask.setAttribute('height', '0'); }
    tip.dataset.side = 'center';
    tip.style.left = '50%'; tip.style.top = '50%';
    return;
  }

  // Scroll into view first, then read rect.
  try { el.scrollIntoView({block:'center', behavior:'smooth'}); } catch(_){}
  const r = el.getBoundingClientRect();
  const pad = 6; // extra padding around the highlight
  const hx = Math.max(0, r.left   - pad);
  const hy = Math.max(0, r.top    - pad);
  const hw = r.width  + pad * 2;
  const hh = r.height + pad * 2;

  hl.classList.remove('hidden');
  hl.style.left   = hx + 'px';
  hl.style.top    = hy + 'px';
  hl.style.width  = hw + 'px';
  hl.style.height = hh + 'px';
  if (mask){
    mask.setAttribute('x', hx);
    mask.setAttribute('y', hy);
    mask.setAttribute('width', hw);
    mask.setAttribute('height', hh);
  }

  // Smart placement: pick the side with the most room. Override with
  // step's preferred `side` if it fits.
  const vw = window.innerWidth, vh = window.innerHeight;
  const tw = 380, th = tip.offsetHeight || 280;
  const preferred = t.side || 'bottom';
  const sideFits = (s) => {
    if (s === 'top')    return r.top > th + 30;
    if (s === 'bottom') return vh - r.bottom > th + 30;
    if (s === 'left')   return r.left > tw + 30;
    if (s === 'right')  return vw - r.right > tw + 30;
    return true;
  };
  const order = [preferred, 'bottom', 'top', 'right', 'left'];
  const side = order.find(sideFits) || 'bottom';
  tip.dataset.side = side;

  let tx, ty;
  if (side === 'bottom'){
    tx = Math.min(vw - tw - 12, Math.max(12, hx));
    ty = hy + hh;
  } else if (side === 'top'){
    tx = Math.min(vw - tw - 12, Math.max(12, hx));
    ty = hy - th;
  } else if (side === 'right'){
    tx = hx + hw;
    ty = Math.min(vh - th - 12, Math.max(12, hy));
  } else { // left
    tx = hx - tw;
    ty = Math.min(vh - th - 12, Math.max(12, hy));
  }
  // Final clamp: never let the tooltip land off-screen, even if the target
  // is larger than the viewport (e.g. a full grid of destination cards).
  tx = Math.min(vw - tw - 12, Math.max(12, tx));
  ty = Math.min(vh - th - 12, Math.max(12, ty));
  tip.style.left = tx + 'px';
  tip.style.top  = ty + 'px';
}

// Browser close / refresh while the Studio has unsaved edits: trigger the
// native "leave site?" prompt. (Browsers ignore custom text here.)
window.addEventListener('beforeunload', e => {
  try {
    const m = $('ov-studio');
    if (m && !m.hidden && window.ICOverlay && Studio.isDirty && Studio.isDirty()){
      e.preventDefault(); e.returnValue = '';
    }
  } catch(_){}
});

document.addEventListener('keydown', e => {
  if (!$('tour').hidden){
    if (e.key === 'Escape')     { e.preventDefault(); closeTour(); }
    else if (e.key === 'ArrowRight'){ e.preventDefault(); tourNext(); }
    else if (e.key === 'ArrowLeft') { e.preventDefault(); tourPrev(); }
  }
});

async function downloadLogs(){
  try {
    const j = await (await fetch('/logs')).json();
    const lines = j.lines || [];
    const ts = new Date().toISOString().replace(/[:.]/g,'-');
    const blob = new Blob([lines.join('\n')+'\n'], {type:'text/plain'});
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url; a.download = 'instantclone-log-'+ts+'.txt';
    document.body.appendChild(a); a.click(); document.body.removeChild(a);
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    toast('Saved '+lines.length+' lines','ok');
  } catch(e){ toast('Download failed: '+e,'err'); }
}

// The bug-report bundle from /diagnostics, secrets already taken out by the
// server. navigator.clipboard only exists on https or localhost, and the
// dashboard is often opened from another PC on the LAN over plain http, so
// fall back to the old selection copy there.
async function copyDiagnostics(btn){
  if (btn.dataset.busy) return;
  btn.dataset.busy = '1';
  try {
    const r = await fetch('/diagnostics');
    if (!r.ok) throw new Error('HTTP '+r.status);
    const text = await r.text();
    let copied = false;
    if (navigator.clipboard && window.isSecureContext){
      try { await navigator.clipboard.writeText(text); copied = true; } catch(_){}
    }
    if (!copied) copied = copyViaSelection(text);
    if (!copied) throw new Error('the browser blocked the clipboard');
    flashCopied(btn);
  } catch(e){
    toast('Could not copy diagnostics: '+e.message,'err');
  } finally {
    delete btn.dataset.busy;
  }
}
function copyViaSelection(text){
  const prev = document.activeElement;
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.setAttribute('readonly','');
  ta.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0;pointer-events:none';
  document.body.appendChild(ta);
  ta.select();
  let ok = false;
  try { ok = document.execCommand('copy'); } catch(_){}
  ta.remove();
  if (prev && prev.focus) prev.focus({preventScroll:true});
  return ok;
}

