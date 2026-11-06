// ---------------------------------------------------------------- lights
// Lights and plugs a scan found, shared by every editor this session.
const LightScan = {devices:null};
const LIGHT_SWATCHES = [['red', '#ff2020'], ['amber', '#ffa000'], ['green', '#00dc46'], ['cyan', '#00d2ff'], ['blue', '#1e50ff'], ['purple', '#963cff'], ['pink', '#ff46aa'], ['white', '#ffffff']];
const LIGHT_ACTIONS = {color:'Turn on in a color', flash:'Flash a color, then stay on', on:'Turn on', off:'Turn off'};
// A light step's color as CSS, for previews: a name, a hex, or a value
// that only becomes a color when it runs.
function lightRgb(color){
  const c = (color || '').trim().toLowerCase();
  const named = LIGHT_SWATCHES.find(([n]) => n === c);
  if (named) return named[1];
  if (c === 'orange') return '#ff6e00';
  if (c === 'yellow') return '#ffdc00';
  if (/^warm( white)?$/.test(c)) return '#ffbe78';
  return /^#?([0-9a-f]{3}|[0-9a-f]{6})$/.test(c) ? (c[0] === '#' ? c : '#' + c) : '#9ca3af';
}
// One whole sentence per case, so each reads in the page's language.
function lightSummary(p){
  const color = (p.color || '').trim() || 'pick a color', bright = (p.brightness || '').trim();
  const pct = /\{/.test(bright) ? bright : bright + '%';
  if (p.action === 'off') return 'Turn off';
  if (p.action === 'on') return bright ? `Turn on at ${pct}` : 'Turn on';
  if (p.action === 'flash') return bright ? `Flash ${color} at ${pct}` : `Flash ${color}`;
  return bright ? `Turn on, ${color} at ${pct}` : `Turn on, ${color}`;
}
// The device a step drives, and the scan to pick another.
function lightPickerHtml(p){
  const ref = p.device || '';
  const where = ref ? (ref.split(':')[1] || '').split('#')[0] : '';
  const current = ref
    ? `<div class="ig-meter"><span class="ig-meter-dot ok"></span><b>${esc(p.name || 'Your light')}</b><span class="muted">${esc(where)}</span>
        <button class="ic-btn ic-btn-ghost ig-small" data-act="light-try" data-ref="${esc(ref)}" data-color="${esc(p.color || '')}">Flash it</button></div>`
    : '<div class="muted">No light picked yet.</div>';
  const list = LightScan.devices;
  const found = !list ? '' : list.length ? `<div class="ig-light-list">${list.map(x => `<div class="ig-light-row${x.ref === ref ? ' on' : ''}">
        <span class="ig-bulb" style="--bulb:${x.plug ? 'transparent' : '#fde68a'}">${svg(x.plug ? 'bolt' : 'bulb')}</span>
        <span class="ig-light-id"><b translate="no">${esc(x.name)}</b><small>${esc(x.brand)}${x.plug ? ' plug' : ''} · ${esc(x.address)}</small></span>
        <button class="ic-btn ic-btn-ghost ig-small" data-act="light-try" data-ref="${esc(x.ref)}" data-color="${esc(p.color || '')}" title="Flash it, to see which one it is">Flash</button>
        <button class="ic-btn ${x.ref === ref ? 'ic-btn-ghost' : 'ic-btn-primary'} ig-small" data-act="light-use" data-ref="${esc(x.ref)}" data-name="${esc(x.name)}" data-plug="${x.plug ? 'yes' : ''}" ${x.ref === ref ? 'disabled' : ''}>${x.ref === ref ? 'In use' : 'Use'}</button></div>`).join('')}</div>`
    : '<div class="muted">Nothing found on this network.</div>';
  return `<div class="dff ig-light-pick"><label>Light or plug</label>${current}
    <button class="ic-btn ic-btn-ghost ig-small" data-act="light-scan">${svg('refresh', ' class="ig-btn-ic"')}${list ? 'Look again' : 'Find lights on my network'}</button>
    ${found}
    <details class="ig-more-fields"><summary>Not found?</summary><div class="muted">It must be on the same network as this PC.
      <b>Govee</b>: turn on LAN Control for it in the Govee Home app, and let InstantClone through the Windows firewall if it asks.
      <b>Yeelight</b>: turn on LAN Control in the Yeelight app. <b>TP-Link</b>: older Kasa models only; Tapo and newer Kasa firmware need a login this can't do.
      <b>Shelly</b> and <b>WLED</b>: a password on the device keeps it from being used here.
      Brands that only work through their own cloud (Lepro's AI series, Tuya and Smart Life devices) aren't on the network this way.</div></details></div>`;
}
// What the light does: on, off, a color or a flash, and how bright.
// `bind(name)` gives each field its data-bind, per editor.
function lightControlsHtml(p, bind){
  const plug = p.plug === 'yes';
  const actions = Object.entries(LIGHT_ACTIONS).filter(([id]) => !plug || id === 'on' || id === 'off');
  const action = actions.some(([id]) => id === p.action) ? p.action : actions[0][0];
  let out = `<div class="dff"><label>Does</label><select class="ic-input" ${bind('action')}>${actions.map(([id, l]) =>
    `<option value="${id}" ${id === action ? 'selected' : ''}>${esc(l)}</option>`).join('')}</select></div>`;
  if (plug) return out + '<div class="muted">A plug only turns on or off.</div>';
  if (action === 'color' || action === 'flash'){
    const color = p.color || '';
    out += `<div class="dff"><label>Color</label><div class="ig-swatches">${LIGHT_SWATCHES.map(([n, hex]) =>
        `<button class="ig-swatch${color.toLowerCase() === n ? ' on' : ''}" style="--sw:${hex}" data-act="l-swatch" data-v="${n}" aria-label="${n}" aria-pressed="${color.toLowerCase() === n}" title="${n}"></button>`).join('')}
        <input type="color" class="ig-swatch-pick" value="${esc(/^#[0-9a-f]{6}$/i.test(lightRgb(color)) ? lightRgb(color) : '#ffffff')}" ${bind('color')} aria-label="Any color"></div>
      <input class="ic-input mono" ${bind('color')} value="${esc(color)}" placeholder="red, #ff2020, or {arg1}" spellcheck="false"></div>`;
  }
  if (action !== 'off') out += `<div class="dff ig-narrow"><label>Brightness, %</label><input class="ic-input" ${bind('brightness')} value="${esc(p.brightness || '')}" placeholder="as it is" inputmode="numeric"></div>`;
  return out;
}
function useDevice(step, el){
  step.params.device = el.dataset.ref;
  step.params.name = el.dataset.name || '';
  step.params.plug = el.dataset.plug || '';
  if (step.params.plug === 'yes' && !['on', 'off'].includes(step.params.action)) step.params.action = 'on';
}
// The on-air light, from a web request (Home Assistant, Hue...) to a
// light on the network, and back. Each state keeps its moment.
const ON_AIR_LIGHT = {live:['color', 'red'], delay:['color', 'amber'], crash:['flash', 'red'], off:['off', '']};
function onAirToNetwork(d){
  d.handlers.forEach(h => {
    const [action, color] = ON_AIR_LIGHT[(h.trigger.filters || {}).state] || ON_AIR_LIGHT.live;
    h.steps = h.steps.map(s => s.type === 'http' ? {type:'light', params:{device:'', name:'', plug:'', action, color, brightness:''}, then:[], else:[]} : s);
  });
}
function onAirToWeb(d, id){
  const start = (S.data.catalog.light_starts || []).find(x => x.id === id);
  if (!start) return;
  d.handlers.forEach(h => {
    const state = (h.trigger.filters || {}).state || 'live';
    h.steps = h.steps.map(s => s.type === 'light' ? {type:'http', params:{method:start.method, url:start.url, body:start.bodies[state] || ''}, then:[], else:[]} : s);
  });
}

// The light device a step is set up for, by what it sends.
function lightStart(http, t){
  if (!(http.params.url || '').trim()) return null;
  const state = ((t && t.filters) || {}).state || 'live';
  return (S.data.catalog.light_starts || []).find(x => x.bodies[state] === http.params.body && x.method === (http.params.method || 'POST'));
}
function lightOf(http, t){
  const device = lightStart(http, t);
  return device ? device.label + (http.params.url ? '' : ' (set its address)') : '';
}

// A filter's name in words.
function filterName(name){
  return {destination:'platform name', platform:'platform', reason:'why', state:'on-air state', kind:'line kind',
    stopped:'stopped on purpose', protected:'crash protection took over'}[name] || name.replace(/_/g, ' ');
}
// Whether a filter is really one of the trigger's settings (a threshold).
function isParam(t, name){
  const e = eventOf(t.event);
  return !!(e && (e.params || []).some(p => p.name === name));
}
// An event trigger's thresholds in words, for its chip: "Fires after: 60 s
// down". `bad` when one isn't a usable number.
function tuneSummary(t){
  const e = eventOf(t.event);
  const params = (e && e.params) || [];
  if (!params.length) return null;
  const f = t.filters || {};
  const val = name => { const p = params.find(x => x.name === name); return f[name] || (p ? p.default : ''); };
  const bad = params.some(p => f[p.name] !== undefined && f[p.name] !== '' && !(+f[p.name] > 0));
  // A Twitch amount's minimum: 1 is any amount.
  const least = params.find(p => p.name.startsWith('min_'));
  if (least){
    const n = +val(least.name);
    return {k:'Only from', v:n > 1 ? `${n} ${paramUnit(least)}` : 'any amount', bad};
  }
  switch (t.event){
    case 'destination_still_down': return {k:'Fires after', v:`${val('after_s')} s down`, bad};
    case 'destination_unstable': return {k:'Fires at', v:`${val('drops')} drops in ${val('within_min')} min`, bad};
    case 'destination_steady': return {k:'Fires after', v:`${val('for_min')} min steady`, bad};
  }
  return {k:'Settings', v:params.map(p => paramShort(p, val(p.name))).join(', '), bad};
}

// Each moment's name. A scene rule of a delay reads by its role ("Game:
// delay on", "Any other scene: back to live"); two that read the same
// otherwise are told apart by what they do.
function momentLabels(hs){
  const labels = hs.map(h => triggerLabel(h.trigger));
  return labels.map((label, n) => {
    const h = hs[n];
    const step = primaryStep(h) || (h.steps || [])[0];
    if (h.trigger.type === 'scene' && step && step.type === 'delay_action'){
      const scene = !h.trigger.scene && step.params.action === 'arm' ? 'Game scene (pick it)' : sceneLabel(h.trigger);
      return `${scene}: ${delayRole(step.params)}`;
    }
    if (labels.filter(x => x === label).length < 2) return label;
    const does = step && step.type === 'delay_action' ? delayActionLabel(step.params, {})
      : step && step.type === 'timeline' ? 'adds a line' : step ? (KINDS[step.type] || {}).label : '';
    return does ? `${label} · ${does}` : `${label} · ${labels.slice(0, n + 1).filter(x => x === label).length}`;
  });
}
// A scene trigger's scene in words.
function sceneLabel(t){
  if (!t.scene) return 'A scene (pick it)';
  if (t.scene === '*') return t.except ? 'Any other scene' : 'Any scene';
  return t.scene;
}
function delayRole(p){
  return {arm:'delay on', activate:'delay on', cut_after:'back to live', cut:'back to live', disarm:'no delay', toggle:'delay on or off'}[p.action] || 'the delay';
}
// "Start a 30 s delay", "Back to live, no skipping".
function delayActionLabel(p, vars){
  const seconds = renderSample(p.seconds || '', vars).trim();
  if (p.action === 'arm' && seconds && Number(seconds) <= 0) return 'Turn the delay off';
  return p.action === 'arm' && seconds ? `Set a ${seconds} s delay` : (DELAY_ACTIONS[p.action] || 'Delay action');
}

// How a message with variables will read, or nothing when it has none.
function sampleLine(text, h, discord){
  if (!needsRender(text, discord)) return '';
  const read = renderSample(text, sampleMap(h));
  return 'Reads like: <b>' + (discord ? discordMarkdown(read) : esc(read)) + '</b>';
}

// Quiet hours, shared by both editors.
function quietHtml(d){
  const q = d.quiet;
  return `<div class="ic-label">Quiet hours</div>
    <div class="sub-tabs ig-seg">${[['off', 'Off'], ['on', 'On']].map(([id, label]) =>
      `<button class="sub-tab${(id === 'on') === !!q ? ' on' : ''}" data-act="set-quiet" data-v="${id}" aria-pressed="${(id === 'on') === !!q}">${label}</button>`).join('')}${SEG_IND}</div>
    ${q ? `<div class="ig-quiet"><div class="dff"><label>From</label><input class="ic-input mono" type="time" data-bind="quiet-from" value="${esc(q.from)}"></div>
      <div class="dff"><label>To</label><input class="ic-input mono" type="time" data-bind="quiet-to" value="${esc(q.to)}"></div></div>` : ''}
    <div class="muted">${svg('moon', ' class="ig-inline-ic"')}For every moment here: nothing runs between these times, on this PC's clock. For alerts that would wake you up.</div>`;
}
function quietInput(d, el){
  const b = el.dataset.bind;
  if ((b !== 'quiet-from' && b !== 'quiet-to') || !d.quiet || !el.value) return false;
  d.quiet[b === 'quiet-from' ? 'from' : 'to'] = el.value;
  return true;
}

// Picking a hotkey or a MIDI pad for a "shortcut" trigger. The hotkey is
// recorded right here, with the Controls tab's own key reading (hkMods,
// hkMainKey) and its "stand down while recording" (hkCaptureMode); the pad
// is learned by the app, which hears the controller, and polled for.
const Keys = {
  recording:false, padTimer:null, padUntil:0,
  owner(){ return OWNERS[Modal.kind]; },
  trigger(){ const o = this.owner(); return o && o.handler ? o.handler().trigger : null; },
  record(){
    if (this.recording) return;
    this.recording = true;
    if (typeof hkCaptureMode === 'function') hkCaptureMode(true);
    document.addEventListener('keydown', this.onKey, true);
    document.addEventListener('pointerdown', this.onOutside, true);
    this.owner().render();
    focusAct('key-record');
  },
  stopRecording(){
    if (!this.recording) return;
    this.recording = false;
    document.removeEventListener('keydown', this.onKey, true);
    document.removeEventListener('pointerdown', this.onOutside, true);
    if (typeof hkCaptureMode === 'function') hkCaptureMode(false);
  },
  onOutside: e => {
    if (e.target.closest && e.target.closest('[data-act="key-record"]')) return;
    Keys.stopRecording();
    const o = Keys.owner();
    if (o) o.render();
  },
  onKey: e => {
    e.preventDefault();
    e.stopPropagation();
    const t = Keys.trigger();
    if (e.key === 'Escape' || !t){ Keys.stopRecording(); Keys.owner().render(); return; }
    if (e.key === 'Backspace' || e.key === 'Delete'){ Keys.stopRecording(); t.hotkey = ''; Keys.owner().changed(); return; }
    const mods = hkMods(e), key = hkMainKey(e.code);
    if (!key) return;
    if (!mods.length){ toast('Hold Ctrl, Alt, Shift or Win too, so a stray key never fires it', 'info'); return; }
    const combo = mods.concat(key).join('+');
    Keys.stopRecording();
    if ((S.data.shortcuts.taken || []).includes(combo)){
      toast(`${combo} already runs a delay action (System › Controls). Pick another.`, 'err', 5000);
      Keys.owner().render();
      return;
    }
    t.hotkey = combo;
    Keys.owner().changed();
  },
  async learnPad(btn){
    const r = await busy(btn, () => api('/integrations/midi/learn', {}));
    if (!r || !r.ok){ toast((r && r.error) || 'Could not listen for a pad', 'err', 5000); return; }
    this.padUntil = Date.now() + 30000;
    this.owner().render();
    clearInterval(this.padTimer);
    this.padTimer = setInterval(() => this.pollPad(), 400);
  },
  async pollPad(){
    const r = await api('/integrations/midi/poll', {});
    const t = this.trigger();
    if (!t){ this.stop(); return; }
    if (r.captured){
      this.stopPad();
      if ((S.data.shortcuts.taken || []).some(sig => sig === r.captured || sig === r.captured.split('@')[0])){
        toast('That pad already runs a delay action (System › Controls). Pick another.', 'err', 5000);
        this.owner().render();
        return;
      }
      t.midi = r.captured;
      this.owner().changed();
    } else if (!r.learning || Date.now() > this.padUntil){
      this.stopPad();
      this.owner().render();
    }
  },
  stopPad(){
    clearInterval(this.padTimer);
    this.padTimer = null;
    this.padUntil = 0;
  },
  // Leaving the editor ends both, and gives the hotkeys back.
  stop(){
    this.stopRecording();
    if (this.padTimer){ this.stopPad(); api('/integrations/midi/cancel', {}); }
  },
};

// The saved hotkey of an integration that Windows wouldn't register.
function refusedKey(i){
  const refused = S.data.shortcuts.refused || [];
  const h = (i.handlers || []).find(h => h.enabled && h.trigger.type === 'shortcut' && refused.includes(h.trigger.hotkey));
  return h ? h.trigger.hotkey : '';
}
// The button's own address: a Stream Deck, Bitfocus Companion or any
// program presses it by calling it.
function buttonAddressHtml(t){
  if (!t.token){
    return `<div class="dff"><label>Stream Deck &amp; apps</label><button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="btn-token-new">${svg('link')}Make an address</button></div>`;
  }
  const url = `${location.origin}/hooks/${t.token}`;
  return `<div class="dff">${secretField('btn-' + t.token.slice(0, 6), `<div class="ig-copy-row"><input class="ic-input mono ig-secret" readonly value="${esc(url)}">
      <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(url)}">Copy</button></div>`, 'Stream Deck &amp; apps (keep it secret)')}
    <div class="muted">Stream Deck: a <b>Website</b> action with this address and <b>GET request in background</b> on. Companion: an HTTP request. Any program: <span class="mono">curl ${esc(url.replace(/\/hooks\/.*/, '/hooks/…'))}</span></div>
    <div class="ig-btn-row"><button class="ic-btn ic-btn-ghost ig-small" data-act="btn-token-new">New address</button>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="btn-token-clear">Remove</button></div></div>`;
}
function shortcutHtml(t){
  if (!S.data.shortcuts.available){
    return warnHtml('Hotkeys and MIDI pads work on Windows only. Here, press it through its address instead.') + buttonAddressHtml(t)
      + onlyLiveHtml(t, '', 'Only while streaming', 'Presses while OBS isn\'t streaming do nothing.');
  }
  const refused = t.hotkey && (S.data.shortcuts.refused || []).includes(t.hotkey)
    ? warnHtml(`Windows won't let InstantClone use ${esc(t.hotkey)}: another app holds it (often Discord, a game overlay or a launcher). Pick another key.`) : '';
  const keys = t.hotkey ? t.hotkey.split('+').map(p => `<span class="hk-chip">${esc(p)}</span>`).join('<span class="hk-sep">+</span>') : '';
  const listening = !!Keys.padTimer;
  return `<div class="ig-keys">
    <div class="dff"><label>Hotkey</label><div class="ig-key-row">
      <button class="hk-capture ig-key${Keys.recording ? ' recording' : t.hotkey ? '' : ' empty'}" data-act="key-record">${Keys.recording
        ? '<span class="hk-cue">Press the keys… Esc cancels</span>' : keys || '<span class="hk-cue">Click, then press keys</span>'}</button>
      ${t.hotkey ? `<button class="ic-btn-tiny" data-act="key-clear" aria-label="Remove the hotkey">${svg('x')}</button>` : ''}</div></div>
    <div class="dff"><label>MIDI pad</label><div class="ig-key-row">
      <button class="hk-capture ig-key${listening ? ' recording' : t.midi ? '' : ' empty'}" data-act="pad-learn">${listening
        ? '<span class="hk-cue">Press a pad or button…</span>' : t.midi ? `<span class="hk-chip">${esc(padLabel(t.midi))}</span>` : '<span class="hk-cue">Click, then press a pad</span>'}</button>
      ${t.midi ? `<button class="ic-btn-tiny" data-act="pad-clear" aria-label="Remove the pad">${svg('x')}</button>` : ''}</div></div>
  </div>
  ${refused}<div class="muted">Works while you're in a game. Use any of them, or all. Keys and pads that run the delay (System › Controls) stay theirs.</div>
  ${buttonAddressHtml(t)}
  ${onlyLiveHtml(t, '', 'Only while streaming', 'Presses while OBS isn\'t streaming do nothing.')}`;
}
function focusAct(act){
  const el = q(`[data-act="${act}"]`);
  if (el) el.focus({preventScroll:true});
}

