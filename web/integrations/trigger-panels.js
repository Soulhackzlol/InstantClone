// ---------------------------------------------------------------- trigger panels

// Panels both editors share: a chat activity trigger's rules with their
// live meter, an OBS scene picker, and an event trigger's thresholds.
// `bind` prefixes the field bindings so each editor routes them.

// The live meter: chat right now against the rules on screen, every
// second, while a rules panel is open. Only the meter redraws.
const Meter = {
  timer:null, reading:null, busy:false,
  watch(){
    if (this.timer) return;
    this.timer = setInterval(() => this.tick(), 1000);
    this.tick();
  },
  async tick(){
    const owner = OWNERS[Modal.kind];
    const t = owner && owner.handler && owner.handler().trigger;
    if (!q('.ig-rules') || !t || t.type !== 'chat_activity'){ this.stop(); return; }
    if (this.busy) return;
    this.busy = true;
    const r = await api('/integrations/meter', {trigger:t});
    this.busy = false;
    if (r && r.ok !== false) this.reading = r;
    if (!Modal.form) return;
    const box = q('.ig-rules .ig-meter');
    if (box) morph(box, meterHtml(t, this.reading));
    Modal.form.querySelectorAll('.ig-rule-bar').forEach(bar => {
      const n = +bar.dataset.n, rule = (t.rules || [])[n];
      if (!rule) return;
      morph(bar, ruleBarHtml(rule, this.reading, n));
      const now = ruleNow(rule, this.reading, n), want = +rule.value || 1;
      bar.setAttribute('aria-valuenow', String(Math.min(now || 0, want)));
      bar.setAttribute('aria-valuetext', now == null ? 'no chat yet' : `${now} of ${want}`);
    });
  },
  stop(){ clearInterval(this.timer); this.timer = null; },
};
// How far chat is toward one rule's threshold, as a bar. A words rule
// reads its own words' share.
function ruleNow(rule, r, n){
  if (!r) return null;
  if (rule.kind === 'words') return (r.shares || [])[n] ?? r.word_share;
  return {busier:r.busier, messages:r.messages, chatters:r.chatters}[rule.kind];
}
function meterAria(rule, now){
  const want = +rule.value || 1;
  return `aria-valuemin="0" aria-valuemax="${want}" aria-valuenow="${Math.min(now || 0, want)}" aria-valuetext="${now == null ? 'no chat yet' : esc(`${now} of ${want}`)}"`;
}
function ruleBarHtml(rule, r, n){
  const live = S.data.twitch.main.chat === 'connected';
  const now = live ? ruleNow(rule, r, n) : null;
  const want = +rule.value || 1;
  const unit = rule.kind === 'words' ? '%' : rule.kind === 'busier' ? '×' : '';
  if (now == null) return '<i style="width:0"></i><span>no chat yet</span>';
  const pct = now > 0 ? Math.max(2, Math.min(100, now / want * 100)) : 0;
  return `<i class="${now >= want ? 'on' : ''}" style="width:${pct.toFixed(1)}%"></i><span>${esc(String(now))}${unit} now</span>`;
}
function meterHtml(t, r){
  if (S.data.twitch.main.chat !== 'connected') return '<span class="ig-meter-dot off"></span><span>Connect Twitch to see chat here.</span>';
  if (!r) return '<span class="ig-meter-dot wait"></span><span>Listening to chat…</span>';
  const held = {hold:'but not during the reconnect screen', offline:'but only while you stream', warmup:'but not in the first 30 s of a stream'}[r.held];
  const state = r.fires ? ['fire', held ? `Would fire, ${held}` : 'Would fire right now']
    : !r.ready && (t.rules || []).some(x => x.kind === 'busier') ? ['learn', 'Learning how fast your chat normally goes…']
    : ['', 'Quiet: nothing would fire'];
  return `<span class="ig-meter-dot ${state[0]}"></span><b aria-live="polite">${state[1]}</b>
    <span class="muted">${plural(r.messages, 'message')} · normal ${r.ready ? r.normal : '?'} · ${plural(r.chatters, 'person').replace('persons', 'people')}${r.top_word ? ` · top word "${esc(r.top_word)}"` : ''}</span>`;
}
function rulesHtml(t, bind){
  const win = Math.round((t.window_ms || 10000) / 1000);
  const wins = [5, 10, 15, 20, 30, 45, 60, 90, 120];
  if (!wins.includes(win)) wins.push(win);
  const rows = (t.rules || []).map((r, n) => {
    const info = RULES[r.kind] || RULES.messages;
    const now = ruleNow(r, Meter.reading, n);
    return `<div class="ig-rule" data-key="rule-${n}">
      <select class="ic-input" data-bind="${bind}r-kind" data-n="${n}" aria-label="Rule">${Object.entries(RULES).map(([id, x]) => `<option value="${id}" ${id === r.kind ? 'selected' : ''}>${esc(x[0])}</option>`).join('')}</select>
      <span class="ig-rule-at">at least</span>
      <input class="ic-input mono ig-rule-num" data-bind="${bind}r-value" data-n="${n}" inputmode="decimal" value="${esc(r.value)}" aria-label="Threshold">
      <span class="ig-rule-unit">${esc(info[1])}</span>
      <button class="ic-btn-tiny" data-act="r-del" data-n="${n}" aria-label="Remove this rule">${svg('x')}</button>
      ${r.kind === 'words' ? `<input class="ic-input ig-rule-words" data-bind="${bind}r-words" data-n="${n}" value="${esc(r.words || '')}" placeholder="clip, pog, lul, let's go" aria-label="Words">` : ''}
      <div class="ig-rule-bar" data-n="${n}" title="${esc(info[2])}" role="meter" aria-label="${esc(info[0])} now"
        ${meterAria(r, now)}>${ruleBarHtml(r, Meter.reading, n)}</div>
    </div>`;
  }).join('');
  return `<div class="ig-rules">
    <div class="ig-rules-head"><span>Fires when</span>${segHtml([['all', 'all'], ['any', 'any']], t.match === 'any' ? 'any' : 'all', 'r-match')}<span>of these are true, over the last</span>
      <select class="ic-input ig-rule-win" data-bind="${bind}r-window" aria-label="Window">${wins.sort((x, y) => x - y).map(s => `<option value="${s}" ${s === win ? 'selected' : ''}>${s} s</option>`).join('')}</select></div>
    <div class="ig-rule-list" data-flip>${rows || '<div class="muted">No rules yet: add one.</div>'}</div>
    <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="r-add">${svg('plus')}Add a rule</button>
    <div class="ig-meter">${meterHtml(t, Meter.reading)}</div>
    <div class="dff"><label>Whose messages count</label>${rolesHtml(t, bind + 'role')}</div>
    <label class="crash-check"><input type="checkbox" data-bind="${bind}r-live" ${t.only_live !== false ? 'checked' : ''}><span><span class="crash-check-title">Only while streaming</span>
      <span class="muted">And not in the first 30 s, when everyone says hi. Never during the reconnect screen.</span></span></label>
    <div class="muted">Fires once per burst: then it waits for chat to calm down before it can fire again. "Busier than normal" compares with your own chat over the last 10 minutes, so the same rule fits a small and a huge channel.</div>
  </div>`;
}
// Edit a chat activity trigger from a rules panel field. True if handled.
function rulesInput(t, el, bind){
  const b = el.dataset.bind, n = +el.dataset.n, rule = (t.rules || [])[n];
  if (b === bind + 'r-kind' && rule){
    rule.kind = el.value;
    rule.value = {busier:3, messages:20, chatters:5, words:30}[el.value];
  }
  else if (b === bind + 'r-value' && rule) rule.value = Math.max(0, parseFloat(el.value) || 0);
  else if (b === bind + 'r-words' && rule) rule.words = el.value;
  else if (b === bind + 'r-window') t.window_ms = (+el.value || 10) * 1000;
  else if (b === bind + 'r-live') t.only_live = el.checked;
  else if (b === bind + 'role'){ t.roles = t.roles || {}; t.roles[el.dataset.name] = el.checked; }
  else return false;
  return true;
}
// A rules panel button. True if handled.
function rulesAct(t, a, el){
  if (!t || t.type !== 'chat_activity') return false;
  if (a === 'r-add'){
    const used = new Set((t.rules || []).map(r => r.kind));
    const kind = ['busier', 'chatters', 'words', 'messages'].find(k => !used.has(k)) || 'messages';
    (t.rules = t.rules || []).push({kind, value:{busier:3, messages:20, chatters:5, words:30}[kind], words:kind === 'words' ? 'clip, pog, lul' : ''});
  }
  else if (a === 'r-del') t.rules.splice(+el.dataset.n, 1);
  else if (a === 'r-match') t.match = el.dataset.v;
  else return false;
  return true;
}

// The OBS connection as the scene picker and OBS steps show it, asked for
// every two seconds while one is open (that also keeps OBS connected
// meanwhile).
const ObsLive = {
  timer:null, status:null,
  watch(){
    if (this.timer) return;
    this.timer = setInterval(() => this.tick(), 2000);
    this.tick();
  },
  async tick(){
    if (!q('[data-obs-live]')){ this.stop(); return; }
    const r = await api('/integrations/obs');
    if (!r || r.ok === false) return;
    const changed = JSON.stringify(r) !== JSON.stringify(this.status);
    this.status = r;
    const owner = OWNERS[Modal.kind];
    if (changed && owner && owner.render) owner.render();
  },
  stop(){ clearInterval(this.timer); this.timer = null; },
};
// One line on how OBS is connected, with a Retry when it isn't.
function obsStatusHtml(){
  const o = ObsLive.status;
  if (!o || o.state === 'connecting' || o.state === 'off') return '<div class="ig-meter"><span class="ig-meter-dot wait"></span><span>Connecting to OBS…</span></div>';
  if (o.state === 'connected') return `<div class="ig-meter"><span class="ig-meter-dot ok"></span><b>OBS connected</b><span class="muted">${o.current ? 'on air: ' + esc(o.current) : ''}</span></div>`;
  return `<div class="ig-meter"><span class="ig-meter-dot off"></span><span>${esc(o.problem || 'Can\'t reach OBS.')}
    ${/WebSocket/.test(o.problem || '') ? '' : ' In OBS: Tools › WebSocket Server Settings › Enable WebSocket server, then OK.'}</span>
    <button class="ic-btn ic-btn-ghost ig-small" data-act="obs-retry">${svg('refresh', ' class="ig-btn-ic"')}Try again</button></div>`;
}
// Scene buttons from OBS's list. `act` sets the value, `cur` is picked.
function scenePickHtml(cur, act, withAny){
  const o = ObsLive.status, scenes = (o && o.scenes) || [];
  const pick = (value, label, live) => `<button class="ig-start${value === cur ? ' on' : ''}${live ? ' live' : ''}" data-act="${act}" data-v="${esc(value)}" aria-pressed="${value === cur}">${esc(label)}</button>`;
  const any = withAny ? pick('*', 'Any other scene') : '';
  if (!scenes.length && !any) return '<div class="muted">Your scenes show here once OBS is connected.</div>';
  return `<div class="ig-scene-list">${any}${scenes.map(x => pick(x, x, o && x === o.current)).join('')}</div>`;
}
function sceneHtml(t, bind, d){
  const settle = Math.round((t.settle_ms == null ? 3000 : t.settle_ms) / 1000);
  const settles = [0, 1, 3, 5, 10, 20, 30, 60];
  if (!settles.includes(settle)) settles.push(settle);
  // In a scene delay, the "back to live" rule can be any scene but the
  // game one.
  const isOther = d && d.preset === 'scene_delay' && (firstStep(d.handlers.find(h => h.trigger === t) || {}, 'delay_action') || {params:{}}).params.action !== 'arm';
  return `<div class="ig-scene" data-obs-live>
    ${obsStatusHtml()}
    <div class="ic-label">Scene</div>${scenePickHtml(t.scene, 'set-scene', isOther)}
    <div class="dfg"><div class="dff"><label>Or a name pattern (* matches anything)</label><input class="ic-input" data-bind="${bind}scene" value="${esc(t.scene || '')}" placeholder="Game*" spellcheck="false"></div>
      ${isOther && t.scene === '*' ? `<div class="dff"><label>But not</label><div class="muted">Never your game scene${t.except ? ` (${esc(t.except)})` : ''}. Change it in the first moment.</div></div>`
        : `<div class="dff"><label>But not (optional)</label><input class="ic-input" data-bind="${bind}scene-except" value="${esc(t.except || '')}" placeholder="Game over, BRB*" spellcheck="false"></div>`}
      <div class="dff"><label>Only when coming from (optional)</label><input class="ic-input" data-bind="${bind}scene-from" value="${esc(t.from || '')}" placeholder="any scene" spellcheck="false"></div></div>
    <div class="dff"><label>Wait on the scene this long before acting</label><div class="sub-tabs ig-seg">${settles.sort((x, y) => x - y).map(x =>
      `<button class="sub-tab${x === settle ? ' on' : ''}" data-act="set-settle" data-v="${x}" aria-pressed="${x === settle}">${x ? x + ' s' : 'no wait'}</button>`).join('')}${SEG_IND}</div></div>
    ${onlyLiveHtml(t, bind, 'Only while streaming', 'Switching scenes before you go live does nothing. A stream that starts on a matching scene counts.')}
    <div class="muted">Flicking through scenes does nothing: it only acts once the scene stayed. Never during the reconnect screen. Several scenes: Game, Game 2.${isOther ? ' "Any other scene" never counts the game scene.' : ''}</div>
  </div>`;
}
// The "only while streaming" switch of a scene or button trigger.
function onlyLiveHtml(t, bind, title, hint){
  return `<label class="crash-check"><input type="checkbox" data-bind="${bind}only-live" ${t.only_live ? 'checked' : ''}><span>
    <span class="crash-check-title">${title}</span><span class="muted">${hint}</span></span></label>`;
}
function onlyLiveInput(t, el, bind){
  if (el.dataset.bind !== bind + 'only-live') return false;
  t.only_live = el.checked;
  return true;
}
function sceneInput(t, el, bind){
  const b = el.dataset.bind;
  if (b === bind + 'scene') t.scene = el.value;
  else if (b === bind + 'scene-from') t.from = el.value;
  else if (b === bind + 'scene-except') t.except = el.value;
  else return false;
  return true;
}
function sceneAct(t, a, el){
  if (!t || t.type !== 'scene') return false;
  if (a === 'set-scene') t.scene = el.dataset.v;
  else if (a === 'set-settle') t.settle_ms = (+el.dataset.v) * 1000;
  else return false;
  return true;
}
// An event trigger's thresholds ("still down after 60 s"), kept among its
// filters. Empty when the event has none. A number that isn't above 0 is
// marked and not kept: the app would quietly use the default instead.
function paramsHtml(t, bind){
  const e = eventOf(t.event);
  const params = (e && e.params) || [];
  if (!params.length) return '';
  return `<div class="dfg ig-params">${params.map(p => {
    const unit = paramUnit(p);
    const label = p.label.replace(/\s*\(.*\)/, '');
    return `<div class="dff"><label>${esc(label)}${unit ? ` <span class="muted">(${unit})</span>` : ''}</label>
      <input class="ic-input mono ig-param" data-bind="${bind}param" data-name="${esc(p.name)}" inputmode="decimal" value="${esc((t.filters || {})[p.name] || p.default)}"></div>`;
  }).join('')}</div>`;
}
// What a setting counts in, from its label: "Still down after (seconds)"
// is in s, "At least (bits)" in bits.
function paramUnit(p){
  if (/second/i.test(p.label)) return 's';
  if (/minute/i.test(p.label)) return 'min';
  return (p.label.match(/\(([^)]*)\)/) || [])[1] || '';
}
// Keep a threshold typed in a params field. False when it isn't a number
// above 0 (the field says so).
function paramInput(t, el){
  const v = el.value.trim();
  const ok = +v > 0 && Number.isFinite(+v);
  el.setAttribute('aria-invalid', ok ? 'false' : 'true');
  el.title = ok ? '' : 'Use a number above 0';
  if (!ok) return false;
  t.filters = t.filters || {};
  t.filters[el.dataset.name] = v;
  return true;
}

// A draft saved with no trigger still opens: it gets a starting one.
function withHandler(d){
  if (!d.handlers || !d.handlers.length) d.handlers = [{enabled:true, trigger:newTrigger('event'), steps:[]}];
  return d;
}

// Discord steps with no channel yet get the first one, e.g. right after
// the user added their first channel from inside an editor. True if any
// step changed.
function pickNewChannel(d){
  const first = S.data.connections.discord[0];
  if (!first) return false;
  let changed = false;
  allSteps(d).forEach(s => {
    const known = S.data.connections.discord.some(c => c.id === s.params.connection);
    if (s.type === 'discord' && !known){ s.params.connection = first.id; changed = true; }
  });
  return changed;
}

function setSend(h, v){
  const first = (h.steps || [])[0];
  if (first && (first.type === 'wait' || first.type === 'wait_delay')) h.steps.shift();
  if (v === 'aired') h.steps.unshift({type:'wait_delay', params:{}, then:[], else:[]});
  else if (v.startsWith('w')) h.steps.unshift({type:'wait', params:{ms:v.slice(1)}, then:[], else:[]});
}
function rolesLabel(r){
  if (!r || r.everyone) return 'everyone';
  const who = ['subs', 'vips', 'mods'].filter(x => r[x]).map(x => x === 'vips' ? 'VIPs' : x);
  return who.length ? who.join(', ') : 'only you';
}
function rolesHtml(t, bind){
  const label = r => r === 'vips' ? 'VIPs' : r[0].toUpperCase() + r.slice(1);
  return `<div class="ig-roles">${['everyone', 'subs', 'vips', 'mods'].map(r =>
    `<label class="ig-role"><input type="checkbox" data-bind="${bind}" data-name="${r}" ${t.roles && t.roles[r] ? 'checked' : ''}><span>${svg('check', ' stroke-width="3"')}${label(r)}</span></label>`).join('')}</div>`;
}
// A field showing its sample text instead is hidden: measuring it then
// would give the shown text's height. It measures when it's shown again.
function autosize(t){
  if (t.closest('.ig-tpl.has-vars') && t !== document.activeElement){ t.style.height = ''; return; }
  t.style.height = 'auto';
  t.style.height = (t.scrollHeight + 2) + 'px';
}
// A narrower window wraps messages and segmented controls onto more lines:
// grow with it, and keep each sliding pill under its tab.
window.addEventListener('resize', () => {
  const root = $('ig-root');
  if (root) placeIndicators(root);
  if (!Modal.form) return;
  Modal.form.querySelectorAll('.ig-msg').forEach(autosize);
  placeIndicators(Modal.form);
});
function insertAtCursor(field, text){
  if (!field || !field.isConnected || field.closest('[data-leaving]')) return;
  const start = field.selectionStart == null ? field.value.length : field.selectionStart;
  const end = field.selectionEnd == null ? start : field.selectionEnd;
  field.value = field.value.slice(0, start) + text + field.value.slice(end);
  field.focus();
  field.setSelectionRange(start + text.length, start + text.length);
  field.dispatchEvent(new Event('input', {bubbles:true}));
}

