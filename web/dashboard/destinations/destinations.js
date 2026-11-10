// - Destinations ---------------------------─
// Platforms offered on the empty Destinations screen: [platform, note].
const DEST_STARTERS = [
  ['twitch', 'Enhanced Broadcasting and Dual Format'],
  ['youtube', 'Live and Shorts'],
  ['kick', 'Streams on Kick'],
  ['restream', 'TikTok, Instagram and more'],
  ['custom', 'Any RTMP server'],
  ['sink', 'Try it without going live'],
];
// Open the new-destination form with `platform` already picked.
function openNewDest(platform){
  openDestForm(null);
  $('dest-form-platform').value = platform;
  $('dest-form-platform').dispatchEvent(new Event('change'));
}
function renderDestStarters(){
  const grid = $('dest-empty-grid');
  if (grid.childElementCount) return;
  grid.replaceChildren(...DEST_STARTERS.map(([platform, note]) => {
    const tile = document.createElement('button');
    tile.type = 'button';
    tile.className = 'dest-starter platform-' + platform;
    const icon = document.createElement('span');
    icon.className = 'dest-starter-icon';
    icon.innerHTML = platformMark(platform);
    const name = document.createElement('span');
    name.className = 'dest-starter-name';
    name.textContent = PLATFORM_LABEL[platform];
    const sub = document.createElement('span');
    sub.className = 'dest-starter-note';
    sub.textContent = note;
    tile.append(icon, name, sub);
    tile.onclick = () => openNewDest(platform);
    return tile;
  }));
}
function rebuildDests(){
  const c = $('dest-grid');
  const empty = lastDests.length === 0;
  $('dest-empty').hidden = !empty;
  c.hidden = empty;
  if (empty) renderDestStarters();
  const seen = new Set();
  lastDests.forEach(d => {
    seen.add(d.id);
    let entry = destCardCache[d.id];
    if (!entry){ entry = buildDestCard(d); c.appendChild(entry.el); destCardCache[d.id] = entry; }
    updateDestCard(entry, d);
  });
  Object.keys(destCardCache).forEach(id => {
    if (!seen.has(id)){ destCardCache[id].el.remove(); delete destCardCache[id]; }
  });
  // The "+ Add" tile is built once and kept last. Moving it only when a
  // new card landed after it keeps keyboard focus and hover intact.
  let add = c.querySelector('.dest-add');
  if (!add){
    add = document.createElement('button');
    add.className = 'dest-add';
    add.innerHTML = `<span class="dest-add-icon"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" aria-hidden="true"><path d="M12 5v14M5 12h14"/></svg></span><span>Add destination</span>`;
    add.onclick = () => openDestForm(null);
  }
  if (c.lastElementChild !== add) c.appendChild(add);
  const waiting = lastDests.filter(d => isVerticalDest(d) && !verticalDiag(d).ready).length;
  // "1 added" has its own key: other languages say one differently.
  const added = lastDests.length === 1 ? '1 added' : `${lastDests.length} added`;
  $('dest-sub').textContent = `${added} · ${lastDests.filter(d => d.alive).length} live`
    + (waiting ? ` · ${waiting} waiting for a 9:16 canvas` : '');
}

// Outline glyph for the card's format indicator. 'horizontal' = a wide
// rect, 'vertical' = a tall rect, 'both' = a wide + a tall side by side
// (Twitch Dual Format). Stroke uses currentColor so the CSS classes tint it.
function formatIconSvg(kind){
  const open = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round">';
  if (kind === 'vertical') return open + '<rect x="4" y="1.5" width="8" height="13" rx="1.6"/></svg>';
  if (kind === 'both')     return open + '<rect x="0.8" y="4.5" width="8.2" height="6.6" rx="1.2"/><rect x="10" y="2.4" width="4.4" height="11.2" rx="1.2"/></svg>';
  return open + '<rect x="1.5" y="4" width="13" height="8" rx="1.6"/></svg>'; // horizontal
}

// Diagnose why a non-Twitch vertical destination is (not) ready from the
// live /state signals. `label` is the pill (a state, like the other
// pills), `hint` is the visible one-line fix on the card, `title` the
// full explanation. The 9:16 canvas is OBS's Enhanced Broadcasting
// Additional canvas; with an enabled Twitch destination, Twitch's config
// decides, so it only comes when Dual Format is on for that channel.
//   - OBS not streaming, or a canvas is live  -> nothing to diagnose
//   - EB live but no 9:16 canvas              -> "No 9:16 canvas"
//   - OBS streaming without EB                -> "EB off"
// An enabled non-Twitch destination set to Vertical (Twitch always takes
// both canvases natively).
function isVerticalDest(d){
  return d.enabled && d.platform !== 'twitch' && d.stream_format === 'vertical';
}
function verticalDiag(d){
  const s = lastState || {};
  // d.vertical_ready updates once per supervisor tick; vertical_present is
  // live, so trust it too and never flash a warning while one is on the wire.
  if (d.vertical_ready !== false || s.vertical_present) return {ready:true};
  if (!s.ingest_alive) return {ready:true};
  // Matches the backend: only an enabled Twitch destination with a key
  // makes InstantClone proxy Twitch's config.
  const viaTwitch = Array.isArray(lastDests) && lastDests.some(x => x.platform === 'twitch' && x.enabled && x.stream_key_set);
  if (s.multitrack_video && viaTwitch)
    return {ready:false, label:'No 9:16 canvas', hint:'Twitch sends no vertical track (Dual Format is off)',
      title:'With a Twitch destination, Twitch\'s config decides the tracks, and it has no vertical one for this channel. Turn on Twitch Dual Format for the channel, or disable the Twitch destination so InstantClone builds the config and uses OBS\'s Additional canvas.'};
  if (s.multitrack_video)
    return {ready:false, label:'No 9:16 canvas', hint:'In OBS, pick an Additional canvas',
      title:'OBS is streaming with Enhanced Broadcasting but sends no 9:16 canvas. In OBS: Settings \u2192 Stream \u2192 Additional canvas, pick your vertical canvas (for example Aitum Vertical).'};
  return {ready:false, label:'Needs OBS setup', hint:'Turn on Enhanced Broadcasting in OBS',
    title:'OBS is streaming without Enhanced Broadcasting, so there is no 9:16 canvas. In OBS: Settings \u2192 Stream, turn on Enhanced Broadcasting (Multi-track via InstantClone on the InstantClone service) and pick your vertical canvas under Additional canvas.'};
}
// Set the card's format icon + hover tooltip from the destination's
// *actual* live state, not an assumed config. Twitch reads horizontal by
// default and only flips to "both" when a vertical canvas is really on the
// wire (Dual Format live). Non-Twitch shows horizontal, or vertical with an
// amber state while it waits for OBS's 9:16 canvas.
function updateFormatIcon(r, d){
  if (!r.format) return;
  let kind, title, cls = '';
  if (d.platform === 'twitch'){
    if (d.vertical_canvas_present){
      kind = 'both'; cls = 'fmt-vertical';
      title = 'Dual Format active: Twitch is getting both a horizontal (16:9) and a vertical (9:16) canvas. InstantClone sends both.';
    } else {
      kind = 'horizontal';
      title = 'Horizontal (16:9): a normal Twitch stream (Enhanced Broadcasting is horizontal too). Turn on Twitch Dual Format in OBS to also send a vertical 9:16 canvas.';
    }
  } else if (d.stream_format === 'vertical'){
    kind = 'vertical';
    const vd = verticalDiag(d);
    if (!vd.ready){
      cls = 'fmt-wait';
      title = vd.title;
    } else {
      cls = 'fmt-vertical';
      title = 'Vertical (9:16): gets the vertical canvas OBS sends with Enhanced Broadcasting (TikTok / YouTube Shorts / Kick mobile).';
    }
  } else {
    kind = 'horizontal';
    title = 'Horizontal (16:9): the standard landscape feed.';
  }
  if (r.fmtKind !== kind){ r.format.innerHTML = formatIconSvg(kind); r.fmtKind = kind; }
  const wantClass = 'dcard-format' + (cls ? ' ' + cls : '');
  if (r.format.className !== wantClass) r.format.className = wantClass;
  if (r.format.title !== tr(title, r.format)){ r.format.title = title; r.format.setAttribute('aria-label', title); }
}

function buildDestCard(d){
  const el = document.createElement('div');
  el.className = 'dcard';
  el.innerHTML = `
    <div class="dcard-head">
      <span class="dcard-icon"></span>
      <div class="dcard-id"><div class="dcard-name" translate="no"></div><div class="dcard-host mono"></div></div>
      <span class="dcard-format" tabindex="0" role="img"></span>
      <button class="dcard-switch dcard-toggle" type="button" role="switch" title="Enable / disable"><span></span></button>
      <div class="dcard-status"><span class="d-dot"></span><span class="dcard-status-l"></span></div>
    </div>
    <div class="dcard-screen">
      <div class="dcard-body">
        <div class="dcard-rate-row"><span class="dcard-rate mono">-</span><span class="dcard-rate-u">kbps</span></div>
        <span class="dcard-foot-meta" title="Frames sent · delay cuts · reconnects" hidden></span>
        <a class="dcard-player" target="_blank" rel="noopener" hidden title="Open the test destination's player to watch exactly what a platform would get, delay included">▶ Watch output</a>
      </div>
      <div class="dcard-res mono"></div>
      <div class="dcard-why">
        <div class="dcard-why-in">
          <div class="dcard-why-advice" aria-live="polite"></div>
          <div class="dcard-why-row">
            <span class="dcard-why-next"></span>
            <button type="button" class="dcard-why-btn dcard-why-retry">Retry now</button>
            <button type="button" class="dcard-why-btn dcard-why-more" aria-expanded="false">Details</button>
          </div>
          <div class="dcard-why-detail mono" hidden></div>
        </div>
      </div>
      <i class="dcard-why-bar" aria-hidden="true"></i>
      <svg class="dcard-spark" viewBox="0 0 400 40" preserveAspectRatio="none">
        <path class="dcard-spark-fill" style="fill:color-mix(in oklch,var(--dc,var(--accent)) 16%,transparent)" d=""/>
        <path class="dcard-spark-line" style="fill:none;stroke:var(--dc,var(--accent));stroke-width:2;stroke-linecap:round;stroke-linejoin:round" d=""/>
      </svg>
    </div>
    <div class="dcard-actions">
      <button class="dcard-act dcard-edit" type="button" title="Edit">✎</button>
      <button class="dcard-act dcard-del" type="button" title="Remove">✕</button>
    </div>`;
  const refs = {
    el, last: d, history: [],
    icon: el.querySelector('.dcard-icon'),
    name: el.querySelector('.dcard-name'),
    host: el.querySelector('.dcard-host'),
    format: el.querySelector('.dcard-format'),
    fmtKind: '',
    statL: el.querySelector('.dcard-status-l'),
    statE: el.querySelector('.dcard-status'),
    rate: el.querySelector('.dcard-rate'),
    res: el.querySelector('.dcard-res'),
    sparkLine: el.querySelector('.dcard-spark-line'),
    sparkFill: el.querySelector('.dcard-spark-fill'),
    meta: el.querySelector('.dcard-foot-meta'),
    toggle: el.querySelector('.dcard-toggle'),
    edit: el.querySelector('.dcard-edit'),
    del: el.querySelector('.dcard-del'),
    player: el.querySelector('.dcard-player'),
    why: el.querySelector('.dcard-why'),
    whyAdvice: el.querySelector('.dcard-why-advice'),
    whyNext: el.querySelector('.dcard-why-next'),
    whyRetry: el.querySelector('.dcard-why-retry'),
    whyMore: el.querySelector('.dcard-why-more'),
    whyDetail: el.querySelector('.dcard-why-detail'),
    whyBar: el.querySelector('.dcard-why-bar'),
    retryDeadline: 0,
    lastPlatform: '',
    lastStat: '',
  };
  refs.edit.onclick = () => openDestForm(refs.last);
  refs.toggle.onclick = () => {
    const d = refs.last;
    // Switching off a destination that is on air ends the stream there, and
    // switching it back on can't resume it: ask first, Keep focused.
    if (d.enabled && d.alive){
      cardConfirm(refs, `Stop streaming to ${d.name}? Viewers there lose the stream.`, 'Stop streaming',
        () => toggleDest(refs.last));
      return;
    }
    toggleDest(d);
  };
  // One on air asks first: Undo can't give its viewers the stream back.
  refs.del.onclick = () => {
    const d = refs.last;
    if (d.alive) cardConfirm(refs, `Remove ${d.name}? Viewers there lose the stream, and its stream key goes with it.`, 'Remove',
      () => deleteDest(d));
    else removeDestWithUndo(d);
  };
  refs.whyRetry.onclick = () => retryDest(refs);
  refs.whyMore.onclick = () => {
    const open = refs.whyDetail.hidden || refs.whyDetail._goal === 'closed';
    animateHidden(refs.whyDetail, !open);
    refs.whyMore.setAttribute('aria-expanded', String(open));
    refs.whyMore.textContent = open ? 'Hide details' : 'Details';
  };
  return refs;
}

// A question asked inside the card instead of a browser dialog: the safe
// answer is focused, Esc or a click elsewhere keeps things as they are.
function cardConfirm(r, question, yesLabel, onYes){
  r.el.querySelector('.dcard-ask')?.remove();
  const ask = document.createElement('div');
  ask.className = 'dcard-ask';
  ask.setAttribute('role', 'alertdialog');
  ask.setAttribute('aria-label', question);
  const text = document.createElement('div');
  text.className = 'dcard-ask-q';
  text.textContent = question;
  const keep = document.createElement('button');
  keep.type = 'button';
  keep.className = 'ic-btn ic-btn-ghost';
  keep.textContent = 'Keep';
  const yes = document.createElement('button');
  yes.type = 'button';
  yes.className = 'ic-btn ic-btn-warn';
  yes.textContent = yesLabel;
  const row = document.createElement('div');
  row.className = 'dcard-ask-row';
  row.append(keep, yes);
  ask.append(text, row);
  const opener = document.activeElement;
  const close = () => {
    ask.remove();
    document.removeEventListener('pointerdown', outside, true);
    if (opener && opener.isConnected) opener.focus();
  };
  const outside = e => { if (!ask.contains(e.target)) close(); };
  keep.onclick = close;
  yes.onclick = () => { close(); onYes(); };
  ask.addEventListener('keydown', e => { if (e.key === 'Escape'){ e.stopPropagation(); close(); } });
  r.el.appendChild(ask);
  document.addEventListener('pointerdown', outside, true);
  keep.focus();
}

async function retryDest(r){
  r.whyRetry.disabled = true;
  const res = await postAction('/destinations/retry', new URLSearchParams({id: r.last.id}));
  if (!res.ok) toast(res.error, 'info');
  tick();
}

// Why a destination isn't streaming, under its numbers: plain advice, the
// next attempt and a slim bar running down to it. Shown only while the
// destination is on and not streaming; the last reason stays readable
// while it connects again.
function renderDestFailure(r, d, streaming){
  const f = d.enabled && !streaming ? d.failure : null;
  r.el.classList.toggle('failing', !!f);
  if (!f){
    r.retryDeadline = 0;
    r.whyBar.classList.remove('run');
    return;
  }
  if (r.whyAdvice.textContent !== tr(f.advice, r.whyAdvice)) r.whyAdvice.textContent = f.advice;
  if (r.whyDetail.textContent !== tr(f.detail, r.whyDetail)) r.whyDetail.textContent = f.detail;
  const waiting = f.retry_in_ms != null;
  const secs = waiting ? Math.max(1, Math.ceil(f.retry_in_ms / 1000)) : 0;
  const next = !f.retries ? 'Fix it in Edit, then it connects.'
             : waiting ? `Next try in ${secs} s`
             : 'Trying again now…';
  if (r.whyNext.textContent !== tr(next, r.whyNext)) r.whyNext.textContent = next;
  r.whyRetry.hidden = !waiting;
  if (waiting) r.whyRetry.disabled = false;
  // The bar restarts only for a new wait, not for each tick of the same one.
  const deadline = waiting ? Date.now() + f.retry_in_ms : 0;
  if (!waiting){
    r.retryDeadline = 0;
    r.whyBar.classList.remove('run');
  } else if (Math.abs(deadline - r.retryDeadline) > 900){
    r.retryDeadline = deadline;
    r.whyBar.style.animationDuration = f.retry_in_ms + 'ms';
    r.whyBar.classList.remove('run');
    void r.whyBar.offsetWidth;
    r.whyBar.classList.add('run');
  }
}

function updateDestCard(r, d){
  r.last = d;
  r.el.classList.toggle('alive', !!d.alive);
  r.el.classList.toggle('off', !d.enabled);
  if (r.lastPlatform !== d.platform){
    if (r.lastPlatform) r.el.classList.remove('platform-'+r.lastPlatform);
    r.el.classList.add('platform-'+d.platform);
    r.lastPlatform = d.platform;
    r.icon.innerHTML = platformMark(d.platform);
  }
  if (r.name.textContent !== d.name) r.name.textContent = d.name;
  // Host stays clean (just the platform). The format is conveyed by the
  // icon + hover tooltip set below, not by appending text here.
  const platLabel = PLATFORM_LABEL[d.platform] || d.platform;
  if (r.host.textContent !== tr(platLabel, r.host)) r.host.textContent = platLabel;
  updateFormatIcon(r, d);
  // alive = TCP connected. bitrate>0 = actually forwarding frames. The
  // sink/Twitch session can be CONNECTED but receiving nothing (OBS hasn't
  // sent the first IDR yet) - UI used to call that "Streaming" which is
  // misleading. Split into three states: Streaming (green pulse), Connected
  // (accent, just sitting on a healthy TCP connection), Idle (gray).
  // A vertical destination with no 9:16 canvas on the wire while OBS is
  // live is a fourth state: it deliberately sends no video, so the pill
  // names what's missing and the card shows the fix.
  const vdiag = isVerticalDest(d) ? verticalDiag(d) : null;
  const verticalWaiting = !!vdiag && !vdiag.ready;
  const isStreaming = d.alive && (d.bitrate_kbps || 0) > 0 && !verticalWaiting;
  const onHold = !!d.on_hold;
  const failing = !!d.failure && d.enabled && !isStreaming && !onHold;
  const stat = !d.enabled ? 'off' : onHold || verticalWaiting || failing ? 'wait' : isStreaming ? 'live' : d.alive ? 'ready' : 'off';
  r.statE.className = 'dcard-status s-' + stat;
  // Going live gets one pulse in the platform's colour, so the change is
  // seen from across the room. Not on first render: the card was already live.
  if (stat === 'live' && r.lastStat && r.lastStat !== 'live') restartAnimation(r.el, 'went-live');
  r.lastStat = stat;
  r.statL.textContent = !d.enabled ? 'Disabled'
                       : onHold ? 'Reconnect screen'
                       : verticalWaiting ? vdiag.label
                       : isStreaming ? 'Streaming'
                       : d.alive ? 'Connected'
                       : failing ? (d.failure.retry_in_ms != null ? 'Retrying' : d.failure.retries ? 'Connecting' : 'Not connected')
                       : 'Idle';
  r.statE.title = onHold ? `Crash protection: live on the reconnect screen until ${encoderApp() || 'your stream'} is back.`
                : verticalWaiting ? vdiag.title : '';
  // rate + history (auto kbps -> Mbps, matching the header + stats).
  const kbps = d.alive ? (d.bitrate_kbps || 0) : 0;
  r.history.push(kbps); if (r.history.length > HISTORY_LEN) r.history.shift();
  const rateU = r.rateU || (r.rateU = r.el.querySelector('.dcard-rate-u'));
  // Until video flows, the card says what it's doing instead of a bare dash.
  const idleMsg = !d.enabled ? 'Off. Switch it on to stream here.'
                : !d.alive && failing && (d.failure.retry_in_ms != null || !d.failure.retries) ? 'Not connected'
                : !d.alive ? ((lastState || {}).ingest_alive ? 'Connecting…' : (encoderApp() ? `Goes live when ${encoderApp()} does` : 'Goes live when you do'))
                : !isStreaming ? 'Connected, waiting for video'
                : '';
  r.rate.classList.toggle('is-msg', !!idleMsg);
  if (idleMsg){ r.rate.textContent = idleMsg; if (rateU) rateU.textContent = ''; }
  else if (kbps >= 1000){ r.rate.textContent = (kbps/1000).toFixed(2); if (rateU) rateU.textContent = 'Mbps'; }
  else { r.rate.textContent = Math.round(kbps); if (rateU) rateU.textContent = 'kbps'; }
  // Detected resolution + codec of the forwarded track (confirms the 9:16
  // canvas is really flowing). Hidden until known, and while nothing is
  // forwarded: under "Not connected" it read as if video were going out.
  if (r.res){
    const parts = [];
    if (d.alive && d.video_res) parts.push(d.video_res.replace('x','×'));
    if (d.alive && d.video_codec) parts.push(d.video_codec);
    const txt = verticalWaiting ? vdiag.hint : parts.join(' · ');
    r.res.classList.toggle('is-hint', verticalWaiting);
    if (r.res.textContent !== tr(txt, r.res)) r.res.textContent = txt;
  }
  renderDestFailure(r, d, isStreaming || onHold);
  // Smooth, 0-anchored, per-card stable sparkline (same renderer as stats).
  drawSpark(r.sparkLine, r.sparkFill, r.history, 400, 40, 'dc:' + d.id);
  // Counters only once there is something to count.
  const frames = d.tags_sent || 0;
  r.meta.hidden = !frames && !d.cuts && !d.reconnects;
  const framesText = frames >= 1e6 ? (frames / 1e6).toFixed(1) + 'M'
                   : frames >= 1e4 ? (frames / 1e3).toFixed(1) + 'k'
                   : frames.toLocaleString();
  const cuts = d.cuts || 0, reconnects = d.reconnects || 0;
  const metaText = [
    frames === 1 ? '1 frame' : `${framesText} frames`,
    cuts === 1 ? '1 cut' : `${cuts} cuts`,
    reconnects === 1 ? '1 reconnect' : `${reconnects} reconnects`,
  ].join(' · ');
  if (r.meta.textContent !== tr(metaText, r.meta)) r.meta.textContent = metaText;
  r.toggle.title = d.enabled ? 'Disable this destination' : 'Enable this destination';
  r.toggle.classList.toggle('on', d.enabled);
  r.toggle.setAttribute('aria-checked', d.enabled ? 'true' : 'false');
  if (r.ariaName !== d.name){
    r.ariaName = d.name;
    r.toggle.setAttribute('aria-label', `Stream to ${d.name}`);
    r.edit.setAttribute('aria-label', `Edit ${d.name}`);
    r.del.setAttribute('aria-label', `Remove ${d.name}`);
    r.whyRetry.setAttribute('aria-label', `Retry ${d.name} now`);
  }
  // Test-sink card: link to the sink's built-in live player. The
  // managed child runs whenever the destination is enabled, so gate on
  // enabled (not alive - the player page is up even between publishes).
  if (r.player){
    const showPlayer = d.platform === 'sink' && d.enabled;
    r.player.hidden = !showPlayer;
    if (showPlayer && r.player.href !== SINK_PLAYER_URL) r.player.href = SINK_PLAYER_URL;
  }
}

async function loadDestinations(){
  try {
    const all = await (await fetch('/destinations')).json();
    lastDests = all.filter(d => !isDeleting('dest:' + d.id));
    rebuildDests();
  } catch(_){}
  // Keep the System tab's quick-launch warning/buttons in sync with the
  // current destination set (covers startup and every dest change).
  refreshSysEbLaunch();
}

// - Destination form -------------------------
// Where focus goes back to when the form closes (the card's Edit button,
// the Add tile...).
let destFormOpener = null;
// A name the form filled in from the platform, so picking another platform
// replaces it, while a name the user typed is never touched.
let destFormAutoName = '';
function uniqueDestName(base, ownId){
  const taken = new Set((lastDests || []).filter(x => x.id !== ownId).map(x => x.name));
  if (!taken.has(base)) return base;
  let n = 2;
  while (taken.has(`${base} ${n}`)) n++;
  return `${base} ${n}`;
}
function autoNameDest(){
  const name = $('dest-form-name');
  if (name.value.trim() !== '' && name.value !== destFormAutoName) return;
  const sel = $('dest-form-platform');
  const label = (PLATFORM_LABEL[sel.value] || (sel.selectedOptions[0] || {}).textContent || '').trim();
  destFormAutoName = label ? uniqueDestName(label, $('dest-form-id').value) : '';
  name.value = destFormAutoName;
}
function openDestForm(d){
  const active = document.activeElement;
  destFormOpener = active && active !== document.body ? active : null;
  destFormAutoName = '';
  $('dest-modal').hidden = false;
  $('dest-form').scrollTop = 0;
  $('dest-form-title').textContent = d ? 'Edit destination' : 'New destination';
  $('dest-form-id').value = d ? d.id : '';
  $('dest-form-name').value = d ? d.name : '';
  fillDestPlatforms();
  fillTwitchIngests((d && d.twitch_ingest) || '');
  $('dest-form-platform').value = d ? d.platform : 'twitch';
  $('dest-form-platform').dispatchEvent(new Event('change'));
  $('dest-form-custom').value = d ? d.custom_egress_url : '';
  $('dest-form-youtube-ingest').value = (d && d.youtube_ingest) || '';
  $('dest-form-format').value = (d && d.stream_format === 'vertical') ? 'vertical' : 'horizontal';
  syncFormatHelp();
  $('dest-form-key').value = '';
  $('dest-form-key').placeholder = d && d.stream_key_set ? 'leave blank to keep current' : 'paste stream key';
  document.getElementById('dest-form-key-hint').textContent = d && d.stream_key_set ? '(currently set)' : '';
  $('dest-form-enabled').checked = d ? d.enabled : true;
  $('dest-form-vod-audio').checked = !!(d && d.vod_audio);
  $('dest-form-audio').value = (d && d.audio_track) || 'auto';
  updateVodVisibility();
  refreshObsEbLaunchAvailability();
  $('dest-form-msg').innerHTML = '';
  // Focus the name field for immediate typing (a filled-in name is
  // selected, so typing replaces it), and wire Esc-to-close.
  setTimeout(() => { const n = $('dest-form-name'); if (n){ n.focus(); if (n.value === destFormAutoName) n.select(); } }, 60);
  document.addEventListener('keydown', onDestModalKey);
}
function cancelDestForm(){
  $('dest-modal').hidden = true;
  document.removeEventListener('keydown', onDestModalKey);
  const back = destFormOpener;
  destFormOpener = null;
  if (back && back.isConnected) back.focus();
  else if ($('dest-add-btn')) $('dest-add-btn').focus();
}
function onDestModalKey(e){
  if (e.key === 'Escape'){ e.preventDefault(); cancelDestForm(); return; }
  // Enter in a field saves, like any form. Buttons, links, selects and
  // disclosures keep their own Enter.
  if (e.key === 'Enter' && !e.isComposing && !e.repeat && e.target.matches('#dest-form input')){
    e.preventDefault();
    saveDestForm();
  }
}
// Portal the destination modal to <body>. It lives inside the destinations
// tab-pane in markup, but .tab-pane keeps a `transform` (animation
// fill-mode: both), which makes it the containing block for position:fixed
// - so the modal would anchor to the pane, not the viewport (top cut off).
// Reparenting to <body> restores true viewport-centred overlay behaviour.
(function portalDestModal(){
  const m = document.getElementById('dest-modal');
  if (m && m.parentElement !== document.body) document.body.appendChild(m);
})();

// VOD-audio mode is Twitch-only (the gate is hardcoded in OBS). The
// "Launch OBS for EB + VOD" experimental button below only matters
// when VOD audio is actually enabled - showing it otherwise just
// invites the user to misuse the CLI flag. Visibility cascades:
//   platform != twitch        -> hide both
//   platform == twitch + !vod -> show VOD toggle, hide EB launcher
//   platform == twitch + vod  -> show both
function updateVodVisibility(){
  const plat = $('dest-form-platform').value;
  const vodWrap = $('dest-form-vod-wrap');
  const vodEbWrap = $('dest-form-vod-eb-wrap');
  if (!vodWrap) return;
  vodWrap.hidden = plat !== 'twitch';
  if (plat !== 'twitch'){
    $('dest-form-vod-audio').checked = false;
    if (vodEbWrap) vodEbWrap.hidden = true;
    return;
  }
  if (vodEbWrap){
    vodEbWrap.hidden = !$('dest-form-vod-audio').checked;
  }
  // Steer script vs legacy method by the detected OBS version (async).
  refreshVodMethods();
}

// Cached /obs/register-status (5s TTL) - shared by the VOD-method steering
// and any other caller that just needs the OBS version / script state.
let _obsRegCache = null, _obsRegAt = 0;
async function obsRegInfo(force){
  const now = Date.now();
  if (!force && _obsRegCache && now - _obsRegAt < 5000) return _obsRegCache;
  try { _obsRegCache = await (await fetch('/obs/register-status')).json(); }
  catch(_){ _obsRegCache = {}; }
  _obsRegAt = now;
  return _obsRegCache;
}
// Compare a "maj.min.patch" string against a target. Returns true/false, or
// null when the version is unknown (OBS never run / logs unreadable).
function obsVerAtLeast(ver, a, b, c){
  if (!ver) return null;
  const m = String(ver).split('.').map(n => parseInt(n, 10) || 0);
  const t = [a, b, c];
  for (let i = 0; i < 3; i++){
    if ((m[i]||0) > t[i]) return true;
    if ((m[i]||0) < t[i]) return false;
  }
  return true;
}
// OBS 32.2+ locks the built-in VOD Track method, so the script is the only
// path there; older/unknown OBS can also use the legacy checkbox.
async function refreshVodMethods(){
  const badge = $('dest-vod-script-badge');
  if (!badge) return;
  // Streaming from Streamlabs: its own VOD Track works, OBS's steps don't apply.
  const sl = slInfo || await slStatus();
  const viaStreamlabs = !!(sl && sl.enabled);
  $('dest-vod-streamlabs').hidden = !viaStreamlabs;
  ['dest-vod-obs-desc', 'dest-vod-script'].forEach(id => { $(id).hidden = viaStreamlabs; });
  if (viaStreamlabs){
    $('dest-vod-legacy').hidden = true;
    return;
  }
  const info = await obsRegInfo();
  const atLeast = obsVerAtLeast(info.obs_version, 32, 2, 0);
  const verLabel = info.obs_version ? ('OBS ' + info.obs_version) : 'your OBS version';
  const legacy = $('dest-vod-legacy');
  if (atLeast === true){
    badge.textContent = '(required on ' + verLabel + ')';
    if (legacy) legacy.hidden = true;
  } else {
    badge.textContent = '(recommended)';
    if (legacy) legacy.hidden = false;
  }
}
// Save the VOD-unlocker script wherever the user wants it (portable OBS
// installs vary; OBS loads scripts by absolute path). The bytes are embedded
// in the app and served at /obs/vod-script/download - no network, no fixed
// server-side path. We prompt for a location with the File System Access API
// when the browser has it (a real native Save dialog), and fall back to a
// plain download otherwise. The VOD guide's first step (system/vod-guide.js)
// calls it with its button and message line; true once the file is out.
async function installVodScript(btn, msg){
  const FILENAME = 'optional-vod-unlocker.lua';
  const done = (t) => { if (msg) msg.textContent = t; };

  // Preferred path: showSaveFilePicker lets the user choose the folder.
  if (window.showSaveFilePicker){
    let handle;
    try {
      handle = await window.showSaveFilePicker({
        suggestedName: FILENAME,
        types: [{ description: 'OBS Lua script', accept: { 'text/x-lua': ['.lua'] } }],
      });
    } catch (e){
      if (e && e.name === 'AbortError') return false; // user cancelled - no noise
      handle = null; // any other failure: fall through to plain download
    }
    if (handle){
      if (btn) btn.disabled = true;
      done('Saving…');
      try {
        const bytes = await (await fetch('/obs/vod-script/download')).arrayBuffer();
        const w = await handle.createWritable();
        await w.write(bytes);
        await w.close();
        done('Saved: ' + FILENAME);
        if (btn) btn.disabled = false;
        return true;
      } catch (e){
        done('Could not save: ' + (e && e.message ? e.message : e));
        toast('Save failed','err');
      }
      if (btn) btn.disabled = false;
      return false;
    }
  }

  // Fallback: browsers without the picker (e.g. Firefox) get a normal
  // download; where it lands is up to their download settings.
  const a = document.createElement('a');
  a.href = '/obs/vod-script/download';
  a.download = FILENAME;
  document.body.appendChild(a);
  a.click();
  a.remove();
  done('Downloaded ' + FILENAME + ' to your downloads folder.');
  return true;
}

// The OBS setup steps only matter once Vertical is picked; the YouTube
// key note only for YouTube.
function syncFormatHelp(){
  const help = $('dest-form-format-help');
  if (!help) return;
  help.hidden = $('dest-form-format').value !== 'vertical';
  $('dest-form-format-youtube').hidden = $('dest-form-platform').value !== 'youtube';
}
$('dest-form-format').addEventListener('change', syncFormatHelp);

function fillDestPlatforms(){
  const sel = $('dest-form-platform'); sel.innerHTML = '';
  platforms.forEach(p => { const o = document.createElement('option'); o.value = p.slug; o.textContent = p.label; sel.appendChild(o); });
  if (!sel._wired){
    sel._wired = true;
    sel.addEventListener('change', () => {
      // Custom AND Kick both take a user-supplied Server URL. Kick shows
      // each streamer a distinct Server value in its dashboard (Settings
      // -> Stream), so there is no single host we can hardcode - the
      // streamer pastes their own, exactly like a custom RTMP endpoint.
      const isKick = sel.value === 'kick';
      $('dest-form-custom-wrap').hidden = sel.value !== 'custom' && !isKick;
      const clabel = $('dest-form-custom-label');
      const chelp = $('dest-form-custom-help');
      const cinput = $('dest-form-custom');
      if (isKick){
        if (clabel) clabel.textContent = 'Kick Server URL';
        if (cinput) cinput.placeholder = 'rtmps://…global-contribute.live-video.net:443/app';
        if (chelp){ chelp.hidden = false; chelp.innerHTML = 'Copy the <b>Server</b> field from your <a href="https://kick.com/dashboard/settings/stream" target="_blank" rel="noopener">Kick dashboard</a> (Settings &rarr; Stream), and the <b>Stream key</b> below it. Kick uses <b>rtmps</b> (TLS), and InstantClone connects over it automatically.'; }
      } else {
        if (clabel) clabel.textContent = 'Custom RTMP URL';
        if (cinput) cinput.placeholder = 'rtmp://host/app  (rtmps:// also supported)';
        if (chelp){ chelp.hidden = true; chelp.innerHTML = ''; }
      }
      $('dest-form-twitch-wrap').hidden = sel.value !== 'twitch';
      $('dest-form-youtube-wrap').hidden = sel.value !== 'youtube';
      // Format choice is meaningless for Twitch (native dual canvas) -
      // hide it there and show the "Twitch does both" note instead.
      const isTwitch = sel.value === 'twitch';
      $('dest-form-format-wrap').hidden = isTwitch;
      $('dest-form-twitch-native-note').hidden = !isTwitch;
      // The local test sink takes any key, so asking for one would just
      // be a fake required field. The key-help block stays visible - it
      // carries the "what is this / where's the player" explainer.
      const isSink = sel.value === 'sink';
      $('dest-form-key-label').hidden = isSink;
      $('dest-form-key').hidden = isSink;
      renderKeyHelp(sel.value, 'dest-form-key-help');
      updateVodVisibility();
      syncFormatHelp();
    });
    const vodToggle = $('dest-form-vod-audio');
    if (vodToggle) vodToggle.addEventListener('change', updateVodVisibility);
  }
}
// The editor's header shows the chosen platform's mark in its colour.
function syncDestFormMark(){
  const slug = $('dest-form-platform').value;
  const mark = $('dest-form-mark');
  mark.style.setProperty('--dc', PLATFORM_COLOR[slug] || 'var(--fg-3)');
  mark.innerHTML = platformMark(slug);
}
$('dest-form-platform').addEventListener('change', syncDestFormMark);
$('dest-form-platform').addEventListener('change', autoNameDest);
// Fill the Twitch ingest picker, then select `value`. The first call per
// page load awaits a fetch, so the value must be applied here, after the
// options exist; set earlier it would silently fall back to Auto.
async function fillTwitchIngests(value){
  const sel = $('dest-form-twitch-ingest');
  if (!twitchIngests){
    try { twitchIngests = await (await fetch('/twitch_ingests')).json(); } catch(_){ twitchIngests = []; }
  }
  sel.innerHTML = '';
  twitchIngests.forEach(p => { const o = document.createElement('option'); o.value = p.slug; o.textContent = p.label; sel.appendChild(o); });
  sel.value = value || '';
}

// One save at a time: a double Enter or a double click on Save would
// otherwise add a new destination twice, since no id is sent until the
// first save answers.
let destSaving = false;
async function saveDestForm(){
  if (destSaving) return false;
  destSaving = true;
  try { return await postDestForm(); }
  finally { destSaving = false; }
}
async function postDestForm(){
  const id = $('dest-form-id').value;
  const name = $('dest-form-name').value.trim();
  const platform = $('dest-form-platform').value;
  const custom = $('dest-form-custom').value.trim();
  const ingest = $('dest-form-twitch-ingest').value;
  const yingest = $('dest-form-youtube-ingest').value;
  const key = $('dest-form-key').value.trim();
  const enabled = $('dest-form-enabled').checked;
  // VOD-audio is Twitch-only. Defensive gating in case the user
  // enabled it then switched platform without the change handler
  // running.
  const vodAudio = platform === 'twitch' && $('dest-form-vod-audio').checked;
  // Vertical is a non-Twitch concept; Twitch always gets both canvases.
  const format = platform !== 'twitch' ? $('dest-form-format').value : 'horizontal';
  if (!name){ $('dest-form-msg').innerHTML = '<span class="err">Name required</span>'; return false; }
  const body = new URLSearchParams({
    name, platform, custom_egress_url:custom, enabled:enabled?'on':'off',
    twitch_ingest:ingest, youtube_ingest:yingest,
    vod_audio: vodAudio ? 'on' : 'off',
    stream_format: format,
    audio_track: $('dest-form-audio').value || 'auto',
  });
  if (id) body.set('id', id);
  if (key) body.set('stream_key', key);
  let j;
  try { j = await (await fetch('/destinations', {method:'POST', body})).json(); }
  catch(_){ j = {ok:false, error:'The app did not answer. Is InstantClone still running?'}; }
  if (j.ok){ toast('Saved','ok'); cancelDestForm(); loadDestinations(); loadConfig(); return true; }
  const msg = document.createElement('span');
  msg.className = 'err';
  msg.textContent = j.error || 'Could not save this destination.';
  $('dest-form-msg').replaceChildren(msg);
  return false;
}
// The enabled-only route: it changes nothing else, so no field has to be
// sent back to survive the toggle.
async function toggleDest(d){
  const r = await postAction('/destinations/toggle',
    new URLSearchParams({id: d.id, enabled: d.enabled ? 'off' : 'on'}));
  // A refused toggle (say, switching on a Kick destination with no
  // Server URL yet) says why instead of the switch snapping back.
  if (!r.ok) toast(r.error, 'err');
  loadDestinations();
}
async function deleteDest(d){
  const r = await postAction('/destinations/delete', new URLSearchParams({id: d.id}), true);
  if (!r.ok) toast(`Could not remove ${d.name}: ${r.error}`, 'err');
  loadDestinations(); loadConfig();
}
function removeDestWithUndo(d){
  const key = 'dest:' + d.id;
  deleteWithUndo(key, `${d.name} removed`, () => deleteDest(d), loadDestinations);
  lastDests = lastDests.filter(x => x.id !== d.id);
  rebuildDests();
}

// Per-platform accent, mirroring the .dcard.platform-* CSS so a wizard
// tile matches the destination card the same choice produces.
const PLATFORM_COLOR = { twitch:'#a78bfa', youtube:'#fda4af', kick:'#86efac',
  trovo:'#67e8f9', restream:'#5eead4', custom:'#fcd34d', sink:'#93c5fd' };
