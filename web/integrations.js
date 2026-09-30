// Integrations tab: the module list, the catalog, the module editor, the
// step builder, connections (Discord, Twitch, phone) and recipes.
//
// Loaded after the dashboard script, so it reuses its helpers ($, toast,
// staggerIn, fitPaneCard) and its components: a module is a .dcard, every
// editor is the destination modal (.ic-modal + .dest-form). All user text
// goes through esc() before it touches innerHTML.
(function(){
'use strict';

// ---------------------------------------------------------------- helpers

const esc = v => String(v == null ? '' : v).replace(/[&<>"']/g,
  c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const clone = v => JSON.parse(JSON.stringify(v));
const store = {
  get(k){ try { return localStorage.getItem(k); } catch(_){ return null; } },
  set(k, v){ try { localStorage.setItem(k, v); } catch(_){} },
};

async function api(path, body){
  try {
    const opts = body === undefined ? {} : {
      method:'POST', headers:{'Content-Type':'application/json'}, body:JSON.stringify(body),
    };
    const r = await fetch(path, opts);
    return await r.json();
  } catch(_){
    return {ok:false, error:'The app did not answer - is InstantClone still running?'};
  }
}

const PATHS = {
  shield:'M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z',
  bubble:'M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z',
  phone:'M7 2h10a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2zM11 18h2',
  link:'M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7',
  bookmark:'M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z',
  clock:'M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 6v6l4 2',
  cut:'M6 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM20 4L8.1 15.9M14.5 14.5L20 20M8.1 8.1L12 12',
  signal:'M2 2l20 20M8.5 16.5a5 5 0 0 1 7 0M5 12.9a10 10 0 0 1 5.2-2.8M19 12.9a10 10 0 0 0-2-1.5M12 20h.01',
  file:'M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6',
  steps:'M4 6h16M4 12h10M4 18h7',
  plus:'M12 5v14M5 12h14',
  check:'M20 6L9 17l-5-5',
  chev:'M6 9l6 6 6-6',
  up:'M18 15l-6-6-6 6',
  down:'M6 9l6 6 6-6',
  x:'M18 6L6 18M6 6l12 12',
  copy:'M9 9h11v11H9zM5 15H4V4h11v1',
  play:'M7 4l13 8-13 8z',
  download:'M12 3v12M7 10l5 5 5-5M5 21h14',
  film:'M3 5h18v14H3zM7 5v14M17 5v14M3 9h4M17 9h4M3 15h4M17 15h4',
  terminal:'M4 17l6-5-6-5M12 19h8',
  warn:'M12 9v4M12 17h.01M10.3 3.9L1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z',
  bolt:'M13 2L3 14h9l-1 8 10-12h-9l1-8z',
};
const svg = (name, extra) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"${extra||''}><path d="${PATHS[name]}"/></svg>`;

// What each step kind is, how it looks, and which parameter holds its text.
const KINDS = {
  discord:      {label:'Discord',       tag:'SEND',   c:'#a5b4fc', icon:'bubble',   text:'text'},
  chat:         {label:'Twitch chat',   tag:'SEND',   c:'#a78bfa', icon:'bubble',   text:'text'},
  phone:        {label:'Phone push',    tag:'SEND',   c:'#f9a8d4', icon:'phone',    text:'text'},
  http:         {label:'Web request',   tag:'SEND',   c:'#fcd34d', icon:'link',     text:'body'},
  wait:         {label:'Wait',          tag:'WAIT',   c:'#c3cad1', icon:'clock'},
  wait_delay:   {label:'Wait for the delay', tag:'WAIT', c:'#c3cad1', icon:'clock'},
  if:           {label:'If / otherwise',tag:'IF',     c:'#f0a53a', icon:'bolt'},
  stop:         {label:'Stop',          tag:'STOP',   c:'#f2665a', icon:'x'},
  delay_action: {label:'Delay action',  tag:'STREAM', c:'#86efac', icon:'cut'},
  marker:       {label:'VOD marker',    tag:'TWITCH', c:'#a78bfa', icon:'bookmark', text:'description'},
  clip:         {label:'Clip',          tag:'TWITCH', c:'#a78bfa', icon:'film'},
  program:      {label:'Run a program', tag:'RUN',    c:'#fcd34d', icon:'terminal'},
  file:         {label:'Write a file',  tag:'FILE',   c:'#fcd34d', icon:'file',     text:'text'},
  set_var:      {label:'Remember a value', tag:'VALUE', c:'#93c5fd', icon:'steps', text:'value'},
  counter:      {label:'Counter',       tag:'VALUE',  c:'#93c5fd', icon:'steps'},
};
// Which step a card previews: the first one that says something.
const PREVIEW_ORDER = ['discord','chat','phone','marker','http','file','clip','delay_action','program'];
const CATEGORY = {alerts:'Alerts', chat:'Chat', auto:'Automation', own:'Your own'};
const DELAY_ACTIONS = {
  cut:'Cut the delay', arm:'Set the delay to…', activate:'Turn the delay on', toggle:'Toggle the delay',
  cut_after:'Cut after this airs', end_hold:'End the reconnect screen',
};
const OPS = {
  is:'is', is_not:'is not', contains:'contains', not_contains:'does not contain', starts_with:'starts with',
  greater:'is more than', less:'is less than', empty:'is empty', not_empty:'is not empty',
};
// Known values for event filters, so "Only if" offers choices.
const FILTER_CHOICES = {reason:['crash','freeze'], stopped:['yes','no'], protected:['yes','no']};

function walk(steps, fn, path){
  (steps || []).forEach((s, i) => {
    const here = (path || []).concat(i);
    fn(s, here);
    walk(s.then, fn, here.concat('then'));
    walk(s.else, fn, here.concat('else'));
  });
}
function allSteps(integration){
  const out = [];
  (integration.handlers || []).forEach(h => walk(h.steps, s => out.push(s)));
  return out;
}
function primaryStep(handler){
  const found = [];
  walk(handler && handler.steps, s => found.push(s));
  for (const kind of PREVIEW_ORDER){
    const s = found.find(x => x.type === kind);
    if (s) return s;
  }
  return null;
}
function presetOf(i){
  const cat = S.data && S.data.catalog;
  return cat ? cat.presets.find(p => p.id === i.preset) : null;
}
function categoryOf(i){
  const p = presetOf(i);
  return p ? p.category : (i.preset === 'stream_alerts' ? 'alerts' : 'own');
}
function eventOf(id){ return (S.data && S.data.events.find(e => e.id === id)) || null; }
function channelName(id){
  const c = S.data && S.data.connections.discord.find(x => x.id === id);
  return c ? '#' + c.name : 'no channel';
}
function triggerLabel(t){
  if (!t) return '';
  switch (t.type){
    case 'event': { const e = eventOf(t.event); return e ? e.label : t.event; }
    case 'chat_command': return t.command || '!command';
    case 'chat_message': return 'Chat message';
    case 'timer': return `Every ${Math.round((t.every_ms||0)/60000)} min`;
    case 'webhook': return 'Web call';
  }
  return t.type;
}

// Sample values for previews: the variables this handler can use.
function varsFor(handler){
  const d = S.data;
  const list = d.vars.global.slice();
  const t = handler && handler.trigger;
  if (t && t.type === 'event'){ const e = eventOf(t.event); if (e) list.unshift(...e.vars); }
  if (t && (t.type === 'chat_command' || t.type === 'chat_message')) list.unshift(...d.vars.chat);
  if (t && t.type === 'webhook') list.unshift(...d.vars.web);
  walk(handler && handler.steps, s => {
    if (s.type === 'http'){
      const n = (s.params.save_as || '').trim() || 'response';
      list.push({name:n + '.status', sample:'200'}, {name:n + '.ok', sample:'yes'}, {name:n + '.body', sample:'{...}'});
    }
    if (s.type === 'clip') list.push({name:'clip.url', sample:'https://clips.twitch.tv/BraveSnipe'});
    if (s.type === 'counter' && s.params.name) list.push({name:'counter.' + s.params.name, sample:'2'});
    if (s.type === 'set_var' && s.params.name) list.push({name:s.params.name, sample:s.params.value || ''});
  });
  const seen = new Set();
  return list.filter(v => !seen.has(v.name) && seen.add(v.name));
}
function sampleMap(handler){
  const m = {};
  varsFor(handler).forEach(v => { m[v.name] = v.sample; });
  return m;
}
// Mirrors src/integrations/template.rs, for previews.
function renderSample(text, vars){
  return String(text || '').replace(/\{\{([A-Za-z0-9_.]+)\}\}|\{([A-Za-z0-9_.]{1,64})(?:\|([^}]*))?\}/g,
    (all, escaped, name, fallback) => {
      if (escaped) return '{' + escaped + '}';
      const v = vars[name] == null ? '' : String(vars[name]);
      if (fallback !== undefined && (v === '' || v === '0')) return fallback;
      return v;
    });
}
function highlightVars(text){
  return esc(text).replace(/\{[A-Za-z0-9_.]{1,64}(?:\|[^}]*)?\}/g, m => `<span class="v">${m}</span>`);
}
function fmtAgo(ms){
  if (!ms) return 'never';
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return 'just now';
  if (s < 3600) return Math.round(s / 60) + ' min ago';
  if (s < 86400) return Math.round(s / 3600) + ' h ago';
  return Math.round(s / 86400) + ' days ago';
}
function fmtClock(ms){
  return new Date(ms).toLocaleTimeString([], {hour:'2-digit', minute:'2-digit', second:'2-digit'});
}
function randomToken(){
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  return Array.from(b, x => x.toString(16).padStart(2, '0')).join('');
}

// ---------------------------------------------------------------- previews

// HTML for what a step sends, styled like where it lands.
function previewHtml(step, handler, big){
  const cls = 'ig-pv' + (big ? ' big' : '');
  if (!step){
    const n = handler ? (handler.steps || []).length : 0;
    return `<div class="${cls} ig-pv-action"><span>${svg('steps')}</span>${n} step${n === 1 ? '' : 's'}</div>`;
  }
  const vars = sampleMap(handler);
  const k = KINDS[step.type] || {};
  const text = k.text ? renderSample(step.params[k.text], vars) : '';
  const ask = handler && handler.trigger && handler.trigger.type === 'chat_command'
    ? (handler.trigger.command || '!command')
    : handler && handler.trigger && handler.trigger.type === 'chat_message' ? (handler.trigger.pattern || 'hey') : '';
  switch (step.type){
    case 'discord': {
      const ping = step.params.ping === 'here' ? '@here ' : step.params.ping === 'everyone' ? '@everyone ' : '';
      return `<div class="${cls} ig-pv-discord"><span class="ig-av">${svg('shield')}</span><span style="min-width:0">
        <b>InstantClone</b><span class="ig-app">APP</span><div class="ig-txt">${esc(ping + text)}</div></span></div>`;
    }
    case 'chat':
      return `<div class="${cls} ig-pv-chat">${ask ? `<div><span class="u1">viewer</span>: ${esc(ask)}</div>` : ''}
        <div><span class="u2">${esc((S.data.twitch.bot.login && step.params.as === 'bot') ? S.data.twitch.bot.login : (S.data.twitch.main.login || 'you'))}</span>: ${esc(text)}</div></div>`;
    case 'phone':
      return `<div class="${cls} ig-pv-phone"><span class="ig-av"></span><span style="min-width:0">
        <small>INSTANTCLONE · now</small><b style="font-weight:600">${esc(renderSample(step.params.title, vars) || 'InstantClone')}</b><div>${esc(text)}</div></span></div>`;
    case 'marker':
      return `<div class="${cls} ig-pv-marker"><span class="tag">${esc(text || 'Marker')} · 1:02:14</span>
        <div class="bar"><i></i><b style="left:22%"></b><b style="left:48%"></b><b style="left:80%"></b></div></div>`;
    case 'http': {
      const body = text.trim();
      let shown = body ? body : `${step.params.method || 'POST'} ${step.params.url || '(address to set)'}`;
      try { shown = JSON.stringify(JSON.parse(body), null, 1).replace(/\n\s*/g, ' '); } catch(_){}
      return `<div class="${cls} ig-pv-json">${esc(shown).replace(/&quot;([^&]*)&quot;:/g, '<span class="k">"$1"</span>:')}</div>`;
    }
    case 'file':
      return `<div class="${cls} ig-pv-file"><code>${esc((step.params.path || 'file.txt').split(/[\\/]/).pop())}</code><span>${esc(text)}</span></div>`;
    case 'clip':
      return `<div class="${cls} ig-pv-action"><span>${svg('film')}</span>Clips the last moments, and hands you the link</div>`;
    case 'delay_action':
      return `<div class="${cls} ig-pv-action"><span>${svg('cut')}</span>${esc(DELAY_ACTIONS[step.params.action] || 'Delay action')}</div>`;
    case 'program':
      return `<div class="${cls} ig-pv-action"><span>${svg('terminal')}</span>${esc('Runs ' + ((step.params.path || '').split(/[\\/]/).pop() || 'a program'))}</div>`;
  }
  return `<div class="${cls} ig-pv-action"><span>${svg(k.icon || 'steps')}</span>${esc(k.label || step.type)}</div>`;
}

// ---------------------------------------------------------------- state

const S = {
  data:null, visible:false, filter:'all', search:'', peek:null,
  view: store.get('ig-view') === 'list' ? 'list' : 'grid',
  pollTimer:null, activityOpen:false,
};

async function load(){
  const d = await api('/integrations');
  if (!d || d.ok === false || !d.integrations){
    $('ig-root').innerHTML = `<div class="lan-warn"><span class="lw-ic">⚠</span><span>${esc((d && d.error) || 'Integrations are not available right now.')}</span></div>`;
    return;
  }
  S.data = d;
  render();
}

async function poll(){
  if (!S.visible && !modalOpen()) return;
  const a = await api('/integrations/activity');
  if (!S.data || !a || !a.activity) return;
  const loginWas = JSON.stringify(S.data.twitch.login);
  const flowWas = !!S.data.twitch.login;
  const changed = JSON.stringify(a.stats) !== JSON.stringify(S.data.stats)
    || JSON.stringify(a.twitch) !== JSON.stringify(S.data.twitch);
  S.data.activity = a.activity;
  S.data.stats = a.stats;
  S.data.twitch = a.twitch;
  if (flowWas !== !!a.twitch.login) schedulePoll();
  if (S.visible && !modalOpen()){
    // A full redraw would steal the caret from someone typing a search.
    const typing = document.activeElement && document.activeElement.id === 'ig-search';
    if (changed && !typing) render(); else renderActivity();
  }
  // Only the Twitch page shows live state; redrawing the others would wipe
  // what the user is typing into them.
  if (Modal.kind === 'connections' && Conn.tab === 'twitch' && (changed || loginWas !== JSON.stringify(a.twitch.login))) Conn.render();
}

function schedulePoll(){
  clearInterval(S.pollTimer);
  const fast = S.data && S.data.twitch && S.data.twitch.login;
  S.pollTimer = setInterval(() => { poll(); }, fast ? 2000 : 3500);
}

// ---------------------------------------------------------------- pane

function stat(i){ return (S.data.stats || {})[i.id] || null; }

function cardInfo(i){
  const h = (i.handlers || []).find(x => x.enabled) || (i.handlers || [])[0];
  const step = primaryStep(h);
  const k = step ? KINDS[step.type] : null;
  const st = stat(i);
  let where = 'Your own · ' + allSteps(i).length + ' steps';
  if (step && step.type === 'discord') where = channelName(step.params.connection);
  else if (step && step.type === 'chat') where = 'Twitch chat';
  else if (step && k) where = k.label;
  const failing = i.enabled && st && st.failing > 0;
  return {
    h, step,
    dc: k ? k.c : 'var(--accent)',
    icon: k ? k.icon : 'steps',
    where, failing,
    last: st && st.last_ms ? fmtAgo(st.last_ms) : 'never',
    summary: step && k && k.text ? step.params[k.text] : triggerLabel(h && h.trigger),
  };
}

function visibleIntegrations(){
  const q = S.search.trim().toLowerCase();
  return S.data.integrations.filter(i =>
    (S.filter === 'all' || categoryOf(i) === S.filter)
    && (!q || i.name.toLowerCase().includes(q) || JSON.stringify(i.handlers).toLowerCase().includes(q)));
}

function render(){
  const root = $('ig-root');
  if (!root || !S.data) return;
  const d = S.data;
  const on = d.integrations.filter(i => i.enabled).length;
  const failing = d.integrations.filter(i => cardInfo(i).failing);
  const empty = d.integrations.length === 0;
  root.innerHTML = `
    <div style="display:flex;flex-direction:column;gap:14px">
      <div class="tab-head">
        <div>
          <div class="tab-title">Integrations</div>
          <div class="tab-sub">${empty ? 'Alerts, chat commands and automations. Each card shows exactly what it sends.'
            : `${on} on${failing.length ? ` · ${failing.length} need${failing.length === 1 ? 's' : ''} attention` : ''}. Each card shows exactly what it sends.`}</div>
        </div>
        <div style="display:flex;gap:8px">
          <button class="ic-btn ic-btn-ghost" data-act="share" ${empty ? 'hidden' : ''}>Share</button>
          <button class="ic-btn ic-btn-ghost" data-act="import">Import recipe</button>
          <button class="ic-btn dest-add-btn" data-act="catalog">${svg('plus')}<span>Add</span></button>
        </div>
      </div>
      ${noticesHtml(failing)}
      ${connectionsHtml()}
      ${empty ? emptyHtml() : `${toolbarHtml()}<div id="ig-items">${itemsHtml()}</div>`}
      <details class="sys-section" id="ig-activity-box" ${S.activityOpen ? 'open' : ''} style="padding:12px 16px">
        <summary style="cursor:pointer;font-weight:600">Recent activity <span class="muted" id="ig-activity-count"></span></summary>
        <div class="ig-activity" id="ig-activity" style="margin-top:10px"></div>
      </details>
    </div>`;
  renderActivity();
  if (typeof fitPaneCard === 'function') fitPaneCard();
}

function noticesHtml(failing){
  const d = S.data;
  const out = [];
  if (d.twitch.notice){
    out.push(`<div class="lan-warn" style="margin-top:0;align-items:center"><span class="lw-ic">⚠</span>
      <span style="flex:1">${esc(d.twitch.notice)}</span>
      <button class="ic-btn ic-btn-ghost" style="padding:5px 11px;font-size:12px" data-act="conn" data-tab="twitch">Reconnect</button></div>`);
  }
  failing.slice(0, 2).forEach(i => {
    const rec = (d.activity || []).find(r => r.integration_id === i.id && r.status === 'failed');
    const step = rec && rec.steps.find(s => s.status === 'failed');
    out.push(`<div class="lan-warn" style="margin-top:0;align-items:center"><span class="lw-ic">⚠</span>
      <span style="flex:1"><b style="color:var(--fg)">${esc(i.name)}</b> ${step ? 'failed: ' + esc(step.detail) : 'is failing'}. Nothing else is affected.</span>
      <button class="ic-btn ic-btn-ghost" style="padding:5px 11px;font-size:12px" data-act="edit" data-id="${esc(i.id)}">Fix</button></div>`);
  });
  return out.join('');
}

function connectionsHtml(){
  const c = S.data.connections, t = S.data.twitch;
  const discord = c.discord.length;
  const twitchOn = !!t.main.login;
  const twitchLive = t.main.chat === 'connected';
  const phoneOn = !!c.phone.topic;
  const pill = (tab, cls, k, v) =>
    `<button class="ic-pill ${cls}" data-act="conn" data-tab="${tab}"><span class="dot"></span><span class="ic-pill-k">${k}</span><span class="ic-pill-v">${esc(v)}</span></button>`;
  return `<div class="ig-conns"><span class="ic-label" style="margin-right:4px">Connections</span>
    ${pill('discord', discord ? 'ok' : '', 'Discord', discord ? `${discord} channel${discord === 1 ? '' : 's'}` : 'Add')}
    ${pill('twitch', twitchOn ? (twitchLive ? 'ok' : 'warn') : '', 'Twitch', twitchOn ? (twitchLive ? t.main.login : t.main.login + ' · ' + t.main.chat) : (t.available ? 'Connect' : 'Unavailable'))}
    ${pill('phone', phoneOn ? 'ok' : '', 'Phone', phoneOn ? c.phone.topic : 'Connect')}
  </div>`;
}

function toolbarHtml(){
  const counts = {all:S.data.integrations.length};
  S.data.integrations.forEach(i => { const c = categoryOf(i); counts[c] = (counts[c] || 0) + 1; });
  const tab = (id, label) => `<button class="sub-tab${S.filter === id ? ' on' : ''}" data-act="filter" data-f="${id}"
    style="flex:0 0 auto;padding:6px 13px;${S.filter === id ? 'background:var(--surface-3)' : ''}">${label} <span style="color:var(--fg-4)">${counts[id] || 0}</span></button>`;
  const view = (id, icon, label) => `<button class="sub-tab${S.view === id ? ' on' : ''}" data-act="view" data-v="${id}" aria-label="${label}"
    style="flex:0 0 auto;padding:6px 9px;${S.view === id ? 'background:var(--surface-3)' : ''}"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="${icon}"/></svg></button>`;
  return `<div class="ig-toolbar">
    <div class="sub-tabs" role="tablist" style="flex:0 0 auto">
      ${tab('all', 'All')}${tab('alerts', 'Alerts')}${tab('chat', 'Chat')}${tab('auto', 'Automation')}${tab('own', 'Your own')}
    </div>
    <input class="ic-input ig-search" id="ig-search" placeholder="Search" aria-label="Search integrations" value="${esc(S.search)}">
    <div class="sub-tabs" role="tablist" style="flex:0 0 auto">
      ${view('grid', 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z', 'Grid')}${view('list', 'M4 6h16M4 12h16M4 18h16', 'List')}
    </div>
  </div>`;
}

function itemsHtml(){
  const items = visibleIntegrations();
  if (!items.length){
    return `<div class="muted" style="padding:18px 4px">Nothing here${S.search ? ' matches "' + esc(S.search) + '"' : ' yet'}.</div>`;
  }
  return S.view === 'grid' ? gridHtml(items) : listHtml(items);
}

function switchHtml(i){
  return `<button class="dcard-switch${i.enabled ? ' on' : ''}" role="switch" aria-checked="${i.enabled}"
    aria-label="${esc(i.name)}" data-act="toggle" data-id="${esc(i.id)}"><span></span></button>`;
}

function gridHtml(items){
  return `<div class="ig-grid">${items.map(i => {
    const c = cardInfo(i);
    return `<div class="dcard ig-card ${i.enabled ? 'alive' : 'off'}" style="--dc:${c.dc}">
      <div class="dcard-screen" data-act="edit" data-id="${esc(i.id)}" role="button" tabindex="0" aria-label="Edit ${esc(i.name)}">
        ${previewHtml(c.step, c.h, false)}
        ${c.failing ? '<span class="dcard-status s-wait ig-flag"><span class="d-dot"></span><span class="dcard-status-l">Failing</span></span>' : ''}
      </div>
      <div class="ig-card-foot">
        <span class="dcard-icon">${svg(c.icon)}</span>
        <div class="dcard-id"><div class="dcard-name" style="font-size:13px">${esc(i.name)}</div>
          <div class="dcard-host" style="font-size:10.5px">${esc(c.where)} · ${esc(c.last)}</div></div>
        ${switchHtml(i)}
      </div>
    </div>`;
  }).join('')}
  <button class="dest-add ig-add-tile" data-act="catalog"><span class="dest-add-icon">${svg('plus')}</span>Add or build your own</button></div>`;
}

function listHtml(items){
  return `<div class="ig-list">${items.map(i => {
    const c = cardInfo(i);
    const open = S.peek === i.id;
    const status = c.failing ? ['s-wait', 'Failing'] : i.enabled ? ['s-ready', 'On'] : ['', 'Off'];
    return `<div class="dcard ig-row ${i.enabled ? 'alive' : 'off'}" style="--dc:${c.dc}">
      <div class="ig-row-main">
        <span class="dcard-icon">${svg(c.icon)}</span>
        <div class="ig-row-name" data-act="peek" data-id="${esc(i.id)}" role="button" tabindex="0" aria-expanded="${open}"><div class="dcard-name">${esc(i.name)}</div>
          <div class="dcard-host mono">${esc(c.where)}</div></div>
        <span class="ig-row-sum mono">${esc(c.summary || '')}</span>
        <span class="ig-row-last">${esc(c.last)}</span>
        <div class="dcard-status ${status[0]}"><span class="d-dot"></span><span class="dcard-status-l">${status[1]}</span></div>
        ${switchHtml(i)}
        <button class="ic-btn-tiny" data-act="edit" data-id="${esc(i.id)}" aria-label="Edit ${esc(i.name)}">✎</button>
      </div>
      ${open ? `<div class="ig-row-peek"><div class="dcard-screen">${previewHtml(c.step, c.h, true)}</div>
        <div style="width:160px;display:flex;flex-direction:column;gap:6px">
          <button class="ic-btn" style="padding:8px 12px" data-act="edit" data-id="${esc(i.id)}">Edit</button>
          <button class="ic-btn ic-btn-ghost" style="padding:8px 12px" data-act="duplicate" data-id="${esc(i.id)}">Duplicate</button>
        </div></div>` : ''}
    </div>`;
  }).join('')}
  <button class="dest-add" style="min-height:52px;flex-direction:row" data-act="catalog"><span class="dest-add-icon" style="width:28px;height:28px">${svg('plus')}</span>Add an integration, or build your own</button></div>`;
}

function emptyHtml(){
  const packs = S.data.catalog.packs;
  const preset = id => S.data.catalog.presets.find(p => p.id === id);
  return `<div class="dest-empty">
    <div class="dest-empty-title">What should InstantClone do for you?</div>
    <div class="dest-empty-sub">Pick a pack to switch on a few integrations at once. Each shows what it sends, and you can change or remove anything later.</div>
    <div class="dest-empty-grid" style="width:min(900px,100%)">
      ${packs.map((p, n) => `<button class="dest-starter" data-act="pack" data-pack="${esc(p.id)}" style="gap:8px;padding:18px;${n === 0 ? 'border-color:color-mix(in oklch,var(--accent) 45%,var(--line))' : ''}">
        <span class="dest-starter-name" style="font-size:15px">${esc(p.name)}${n === 0 ? ' <span class="dcard-status s-ready" style="min-width:0;max-width:none;margin-left:6px;vertical-align:1px"><span class="dcard-status-l">Recommended</span></span>' : ''}</span>
        <span class="dest-starter-note" style="line-height:1.5">${esc(p.description)}</span>
        <span style="display:flex;flex-direction:column;gap:4px;margin-top:4px;font-size:12.5px;color:var(--fg-2)">
          ${p.presets.map(id => `<span>· ${esc((preset(id) || {}).name || id)}</span>`).join('')}</span>
      </button>`).join('')}
    </div>
    <div class="muted" style="margin-top:14px">or <a href="#" data-act="catalog">browse everything</a> · <a href="#" data-act="build">build your own</a> · <a href="#" data-act="import">import a recipe</a></div>
  </div>`;
}

function renderActivity(){
  const box = $('ig-activity');
  if (!box || !S.data) return;
  const rows = (S.data.activity || []).slice(0, 40);
  const count = $('ig-activity-count');
  if (count) count.textContent = rows.length ? `· last run ${fmtAgo(rows[0].at_ms)}` : '· nothing has run yet';
  box.innerHTML = rows.length ? rows.map(r => {
    const failed = r.steps.find(s => s.status === 'failed');
    const detail = failed ? failed.label + ': ' + failed.detail
      : r.status === 'busy' ? 'skipped: already running'
      : r.steps.filter(s => s.status === 'ok').map(s => s.label).join(' · ') || r.trigger;
    return `<div><span class="t">${esc(fmtClock(r.at_ms))}</span><span class="n">${esc(r.name)}${r.test ? ' <span class="muted">(test)</span>' : ''}</span>
      <span class="d" title="${esc(detail)}">${esc(r.trigger)} → ${esc(detail)}</span><span class="s ${esc(r.status)}">${esc(r.status)}</span></div>`;
  }).join('') : '<div class="muted" style="display:block">Runs show up here, with every step and what it answered.</div>';
}

// ---------------------------------------------------------------- actions

function find(id){ return S.data.integrations.find(i => i.id === id); }

async function toggle(id){
  const i = find(id);
  if (!i) return;
  const r = await api('/integrations/toggle', {id, enabled:!i.enabled});
  if (r.ok){ i.enabled = !i.enabled; render(); }
  else { toast(r.error || 'Could not switch it', 'err', 5000); if (!i.enabled) openEditor(i); }
}

async function addFrom(body, openAfter){
  const r = await api('/integrations/add', body);
  if (!r.ok){ toast(r.error || 'Could not add it', 'err', 5000); return; }
  await load();
  const added = (r.ids || []).map(find).filter(Boolean);
  toast(added.length > 1 ? `Added ${added.length} integrations` : `Added ${added[0] ? added[0].name : ''}`, 'ok');
  const unfinished = added.find(i => !i.enabled);
  if (openAfter && unfinished){
    Modal.close(true);
    if (unfinished.preset) Editor.open(unfinished, true); else Builder.open(unfinished);
    return;
  }
  if (unfinished) toast(`${unfinished.name} needs one more detail before it can switch on`, 'info', 4500);
  if (Modal.kind === 'catalog') Catalog.render();
}

async function duplicate(id){
  const r = await api('/integrations/duplicate', {id});
  if (!r.ok){ toast(r.error || 'Could not duplicate it', 'err'); return; }
  await load();
  toast('Duplicated', 'ok');
}

async function remove(id){
  const i = find(id);
  if (!i || !confirm(`Delete "${i.name}"? This can't be undone.`)) return;
  const r = await api('/integrations/delete', {id});
  if (!r.ok){ toast(r.error || 'Could not delete it', 'err'); return; }
  Modal.close();
  await load();
  toast('Deleted', 'ok');
}

function openEditor(i){
  if (i.preset) Editor.open(i); else Builder.open(i);
}

// ---------------------------------------------------------------- modal shell

// One modal at a time, built on the destination editor's shell. Escape and
// the backdrop close it; `onClose` lets an editor ask before losing edits.
const Modal = {
  el:null, kind:null, onClose:null, after:null,
  open(kind, html, width){
    this.close(true);
    const wrap = document.createElement('div');
    wrap.className = 'ic-modal';
    wrap.setAttribute('role', 'dialog');
    wrap.setAttribute('aria-modal', 'true');
    wrap.innerHTML = `<div class="ic-modal-backdrop" data-act="modal-close"></div>
      <div class="dest-form" style="width:min(${width || 820}px,100%)">${html}</div>`;
    document.body.appendChild(wrap);
    wrap.addEventListener('click', onClick);
    wrap.addEventListener('input', onInput);
    wrap.addEventListener('change', onInput);
    wrap.addEventListener('keydown', onActivateKey);
    this.el = wrap;
    this.kind = kind;
    this.onClose = null;
    this.after = null;
    document.addEventListener('keydown', onModalKey);
    return wrap;
  },
  body(html){
    if (this.el) this.el.querySelector('.dest-form').innerHTML = html;
  },
  close(force){
    if (!this.el) return;
    if (!force && this.onClose && this.onClose() === false) return;
    const after = this.after;
    this.el.remove();
    this.el = null;
    this.kind = null;
    this.after = null;
    document.removeEventListener('keydown', onModalKey);
    if (after) after();
  },
};
function modalOpen(){ return !!Modal.el; }
function onModalKey(e){
  if (e.key === 'Escape' && !e.defaultPrevented){ e.preventDefault(); Modal.close(); }
}
function modalHead(icon, color, title, hint, extra){
  return `<div class="dest-form-head">
    <span class="dest-form-mark" style="--dc:${color}">${svg(icon)}</span>
    <div class="dest-form-titles"><h4>${title}</h4><div class="dest-form-hint">${hint}</div></div>
    ${extra || ''}
    <button type="button" class="dest-form-x" data-act="modal-close" aria-label="Close">✕</button>
  </div>`;
}

// ---------------------------------------------------------------- catalog

const Catalog = {
  cat:'rec',
  open(){
    this.cat = 'rec';
    Modal.open('catalog', '', 1040);
    this.render();
  },
  render(){
    const cat = S.data.catalog;
    const counts = id => id === 'packs' ? cat.packs.length
      : cat.presets.filter(p => id === 'rec' ? p.recommended : p.category === id).length;
    const nav = [['rec', 'Recommended'], ['alerts', 'Alerts'], ['chat', 'Chat'], ['auto', 'Automation'], ['packs', 'Packs']]
      .map(([id, label]) => `<button class="${this.cat === id ? 'on' : ''}" data-act="cat" data-cat="${id}">${label}<small>${counts(id)}</small></button>`).join('');
    const have = id => S.data.integrations.filter(i => i.preset === id).length;
    let tiles;
    if (this.cat === 'packs'){
      tiles = cat.packs.map(p => `<div class="dcard" style="padding:14px;gap:10px">
        <div class="dcard-name">${esc(p.name)}</div>
        <div class="muted" style="line-height:1.5">${esc(p.description)}</div>
        <div style="font-size:12.5px;color:var(--fg-2)">${p.presets.map(id => esc((cat.presets.find(x => x.id === id) || {}).name || id)).join(' · ')}</div>
        <button class="ic-btn ic-btn-primary" style="align-self:flex-start;padding:7px 14px;font-size:12.5px" data-act="pack" data-pack="${esc(p.id)}">Add pack</button>
      </div>`).join('');
    } else {
      const list = cat.presets.filter(p => this.cat === 'rec' ? p.recommended : p.category === this.cat);
      tiles = `<button class="dest-add" data-act="build" style="min-height:214px;padding:16px;text-align:center">
          <span class="dest-add-icon">${svg('plus')}</span>
          <span style="font-size:14px;font-weight:600;color:var(--fg)">Build your own</span>
          <span style="font-size:12px;line-height:1.5;color:var(--fg-3);max-width:220px">Any trigger, any steps: chat, Discord, web requests, programs, the delay itself.</span>
        </button>` + list.map(p => {
        const fake = fakeStep(p);
        const n = have(p.id);
        const needs = needsNote(p.needs);
        return `<div class="dcard" style="--dc:${(KINDS[fake.step.type] || {}).c || 'var(--accent)'};padding:12px;gap:10px">
          <div class="dcard-screen" style="padding:10px;min-height:84px;display:flex;align-items:center">${previewHtml(fake.step, fake.handler, false)}</div>
          <div style="display:flex;align-items:center;gap:10px">
            <div class="dcard-id"><div class="dcard-name">${esc(p.name)}</div><div class="dcard-host" style="white-space:normal;font-family:inherit;line-height:1.45">${esc(p.description)}</div>
              ${needs ? `<div style="margin-top:4px;font-size:11px;color:var(--warn)">${esc(needs)}</div>` : ''}</div>
          </div>
          <button class="ic-btn ${n ? 'ic-btn-ghost' : 'ic-btn-primary'}" style="padding:7px 12px;font-size:12px" data-act="add-preset" data-preset="${esc(p.id)}">${n ? 'Add another' : 'Add'}</button>
        </div>`;
      }).join('');
    }
    Modal.body(`${modalHead('plus', 'var(--accent)', 'Add an integration', 'Every card shows exactly what it sends. Add it, then tweak anything.')}
      <div class="ig-cat"><nav class="ig-cat-nav" aria-label="Categories">${nav}
        <div style="margin-top:auto;padding:10px;border-radius:10px;border:1px dashed var(--line-2);font-size:12px;line-height:1.5;color:var(--fg-3)">
          Got a recipe from a friend? <a href="#" data-act="import" style="color:var(--accent);font-weight:600">Import it</a></div></nav>
      <div class="ig-cat-grid">${tiles}</div></div>`);
    const form = Modal.el.querySelector('.dest-form');
    form.style.height = 'min(86vh,820px)';
  },
};

// A catalog entry's preview without building it: its sample line.
function fakeStep(p){
  const pv = p.preview || {};
  const type = {discord:'discord', chat:'chat', phone:'phone', marker:'marker', json:'http', file:'file'}[pv.kind] || 'chat';
  const param = (KINDS[type] || {}).text || 'text';
  const step = {type, params:{[param]: pv.text || '', path:'crashes.txt', title:'OBS crashed',
    body: type === 'http' ? '{"event":"hold_opened","delay_ms":30000}' : ''}, then:[], else:[]};
  const handler = {trigger: pv.ask ? {type:'chat_command', command:pv.ask} : {type:'event', event:'hold_opened'}, steps:[step]};
  return {step, handler};
}
function needsNote(needs){
  const c = S.data.connections, t = S.data.twitch;
  if (needs === 'discord' && !c.discord.length) return 'Needs a Discord channel (you can add it next)';
  if (needs === 'twitch' && !t.available) return 'Needs Twitch, which this build can\'t log in to';
  if (needs === 'twitch' && !t.main.login) return 'Needs Twitch connected';
  if (needs === 'phone' && !c.phone.topic) return 'Needs your phone connected';
  if (needs === 'web') return 'You paste the address it sends to';
  if (needs === 'file') return 'You pick the file it writes';
  return '';
}

// ---------------------------------------------------------------- module editor

// The simple editor for catalog modules: pick a moment, write on the
// preview itself, and every option is a chip that opens its own panel.
const Editor = {
  d:null, sel:0, chip:null, run:null, dirty:false, moreVars:false,
  // `switchOn`: freshly added and waiting for one detail; shown switched on
  // so saving once it is complete also turns it on.
  open(i, switchOn){
    this.d = withHandler(clone(i));
    if (switchOn) this.d.enabled = true;
    this.sel = Math.max(0, this.d.handlers.findIndex(h => h.enabled));
    // Missing a channel: open straight on the chip that fixes it.
    const known = id => S.data.connections.discord.some(c => c.id === id);
    this.chip = allSteps(this.d).some(s => s.type === 'discord' && !known(s.params.connection)) ? 'where' : null;
    this.run = null;
    this.dirty = false;
    this.moreVars = false;
    Modal.open('editor', '', 920);
    Modal.onClose = () => !this.dirty || confirm('Close without saving your changes?');
    this.render();
  },
  handler(){ return this.d.handlers[this.sel]; },
  resume(){
    if (pickNewChannel(this.d)) this.dirty = true;
    Modal.open('editor', '', 920);
    Modal.onClose = () => !this.dirty || confirm('Close without saving your changes?');
    this.render();
  },
  render(){
    const d = this.d, h = this.handler();
    const step = primaryStep(h);
    const k = step ? KINDS[step.type] : null;
    const p = presetOf(d);
    const head = modalHead(k ? k.icon : 'steps', k ? k.c : 'var(--accent)',
      `<input class="ig-name" data-bind="name" value="${esc(d.name)}" aria-label="Name" style="width:100%;max-width:380px;padding:1px 0;border:0;border-bottom:1px dashed var(--line-2);background:transparent;color:var(--fg);font:inherit;font-weight:600;outline:none">`,
      esc(p ? p.description : 'Pick a moment, then write on the message itself. Everything else is one click away.'),
      `<label class="dest-form-enable"><input type="checkbox" data-bind="enabled" ${d.enabled ? 'checked' : ''}><span class="dfe-track"><span></span></span><span>Enabled</span></label>`);
    Modal.body(`${head}
      <div class="dest-form-body" style="padding:20px 24px 22px;gap:18px">
        ${this.momentsHtml()}
        ${this.previewBoxHtml(step, h)}
        <div style="display:flex;flex-direction:column;gap:10px">
          <div class="ig-chips">${this.chips(step, h).map(c => `<button class="ig-chip${this.chip === c.id ? ' on' : ''}${c.warn ? ' warn' : ''}" data-act="chip" data-chip="${c.id}">
            <span>${esc(c.k)}</span><b>${esc(c.v)}</b>${svg('chev')}</button>`).join('')}</div>
          <div class="ig-pop" id="ig-pop">${this.popHtml(step, h)}</div>
        </div>
      </div>
      <div class="dest-form-foot">
        <div class="dest-form-msg muted" style="flex:1">${this.lastRunHtml()}</div>
        <button class="ic-btn ic-btn-ghost" data-act="ed-builder">Open in builder</button>
        <button class="ic-btn ic-btn-ghost" data-act="ed-test">Send a test</button>
        <button class="ic-btn ic-btn-ghost" data-act="ed-delete">Delete</button>
        <button class="ic-btn ic-btn-primary" data-act="ed-save">Save</button>
      </div>`);
    const msg = Modal.el.querySelector('.ig-msg');
    if (msg) autosize(msg);
  },
  momentsHtml(){
    const hs = this.d.handlers;
    if (hs.length === 1) return '';
    if (hs.length > 5){
      return `<div class="sub-tabs ig-seg" role="tablist">${hs.map((h, n) => `<button class="sub-tab${n === this.sel ? ' on' : ''}" data-act="moment" data-n="${n}"
        style="${h.enabled ? '' : 'opacity:.5'}">${esc(triggerLabel(h.trigger))}</button>`).join('')}</div>`;
    }
    return `<div class="ig-moments" role="tablist" style="--n:${hs.length}">${hs.map((h, n) => `<div class="ig-moment">
      <button class="ig-dot${h.enabled ? ' on' : ''}" role="switch" aria-checked="${h.enabled}" aria-label="Send on: ${esc(triggerLabel(h.trigger))}" data-act="moment-toggle" data-n="${n}">${svg('check', ' stroke-width="3.2"')}</button>
      <button class="ig-moment-name${n === this.sel ? ' on' : ''}" role="tab" aria-selected="${n === this.sel}" data-act="moment" data-n="${n}">${esc(triggerLabel(h.trigger))}</button>
    </div>`).join('')}</div>`;
  },
  previewBoxHtml(step, h){
    if (!step || !(KINDS[step.type] || {}).text){
      return `<div class="dcard-screen" style="padding:16px">${previewHtml(step, h, true)}
        <div class="muted" style="margin-top:10px">This ${h.trigger.type === 'event' ? 'moment' : 'trigger'} has nothing to write. <a href="#" data-act="ed-builder">Open it in the builder</a> to see every step.</div></div>`;
    }
    const k = KINDS[step.type];
    const value = step.params[k.text] || '';
    // The trigger's own values first; the always-there ones behind "more".
    const all = varsFor(h);
    const common = new Set(['delay', 'delay_state', 'hold_left', 'time']);
    const globals = new Set(S.data.vars.global.map(v => v.name));
    const shown = this.moreVars ? all : all.filter(v => !globals.has(v.name) || common.has(v.name));
    const tokens = shown.map(v => `<button class="ig-token" data-act="token" data-token="${esc(v.name)}" title="e.g. ${esc(v.sample)}">+ ${esc(v.name)}</button>`).join('')
      + (shown.length < all.length ? `<button class="sub-tab" style="flex:0 0 auto;padding:2px 8px;font-size:11.5px" data-act="more-vars">${all.length - shown.length} more…</button>` : '');
    const editor = `<textarea class="ig-msg" data-bind="text" rows="1" aria-label="Message" spellcheck="true">${esc(value)}</textarea>
      <div class="ig-sample" id="ig-sample">${sampleLine(value, h)}</div>`;
    let box;
    if (step.type === 'discord'){
      const ping = step.params.ping === 'here' ? '@here ' : step.params.ping === 'everyone' ? '@everyone ' : '';
      box = `<div style="border-radius:14px;background:#313338;overflow:hidden">
        <div style="height:38px;display:flex;align-items:center;gap:8px;padding:0 16px;border-bottom:1px solid rgba(0,0,0,.3);color:#f2f3f5;font-size:14px;font-weight:600">
          <span style="color:#80848e;font-size:18px;font-weight:400">#</span>${esc(channelName(step.params.connection).replace(/^#/, ''))}
          <span style="margin-left:auto;font-size:11px;font-weight:500;color:#949ba4">Live preview</span></div>
        <div style="display:flex;gap:14px;padding:16px">
          <span style="width:40px;height:40px;flex:none;border-radius:50%;background:var(--accent);display:grid;place-items:center;color:#051018">${svg('shield', ' style="width:18px;height:18px"')}</span>
          <div style="flex:1;min-width:0;color:#dbdee1;font-size:14.5px">
            <div style="margin-bottom:6px"><b style="color:#f2f3f5">InstantClone</b><span class="ig-app" style="padding:0 4px;margin-left:6px;border-radius:3px;background:#5865f2;color:#fff;font-size:10px;font-weight:700">APP</span>
            ${ping ? `<span style="margin-left:8px;padding:0 4px;border-radius:3px;background:rgba(88,101,242,.3);color:#c9cdfb;font-size:12px">${esc(ping.trim())}</span>` : ''}</div>
            ${editor}</div></div></div>`;
    } else if (step.type === 'chat'){
      const t = h.trigger;
      const ask = t.type === 'chat_command' ? (t.command || '!command') : t.type === 'chat_message' ? (t.pattern || 'hey') : '';
      const who = (step.params.as === 'bot' && S.data.twitch.bot.login) || S.data.twitch.main.login || 'you';
      box = `<div style="border-radius:14px;background:#18181b;padding:14px 16px;color:#efeff1;font-size:14px;display:flex;flex-direction:column;gap:8px">
        <div style="font-size:10.5px;font-weight:700;letter-spacing:.06em;color:#adadb8">STREAM CHAT · LIVE PREVIEW</div>
        ${ask ? `<div><b style="color:#ff7eb6">viewer</b>: ${esc(ask)}</div>` : ''}
        <div><b style="color:#5ac8fa">${esc(who)}</b>:</div>${editor}</div>`;
    } else if (step.type === 'phone'){
      box = `<div style="border-radius:22px;background:#1d2530;padding:18px;display:flex;flex-direction:column;gap:10px;color:#fff">
        <div style="font-size:10.5px;color:rgba(255,255,255,.7)">INSTANTCLONE · now</div>
        <input class="ic-input" data-bind="title" value="${esc(step.params.title || '')}" placeholder="Title" aria-label="Title" style="background:rgba(255,255,255,.08)">
        ${editor}</div>`;
    } else {
      box = `<div class="dcard-screen" style="padding:14px 16px">
        <div class="ic-label" style="margin-bottom:8px">${esc(k.label)}${step.type === 'http' ? ' · body' : ''}</div>${editor}</div>`;
    }
    return `<div>${box}<div class="ig-tokens" style="margin-top:10px">Insert ${tokens}</div></div>`;
  },
  // The chips shown for this moment, with their current value.
  chips(step, h){
    const d = this.d, out = [];
    const discordSteps = allSteps(d).filter(s => s.type === 'discord');
    const t = h.trigger;
    if (discordSteps.length){
      const id = discordSteps[0].params.connection;
      const set = S.data.connections.discord.some(c => c.id === id);
      out.push({id:'where', k:'Post in', v:set ? channelName(id) : 'pick a channel', warn:!set});
      if (step && step.type === 'discord'){
        const ping = step.params.ping || '';
        out.push({id:'ping', k:'Ping', v:ping === 'here' ? '@here' : ping === 'everyone' ? '@everyone' : 'nobody'});
      }
    }
    if (t.type === 'event' || t.type === 'timer' || t.type === 'webhook') out.push({id:'send', k:'Send', v:this.sendValue(h).label});
    if (t.type === 'event' && this.filterVars(t).length){
      const set = Object.entries(t.filters || {}).filter(([, v]) => v);
      out.push({id:'only', k:'Only if', v:set.length ? set.map(([k, v]) => `${k} is ${v}`).join(', ') : 'always'});
    }
    if (t.type === 'chat_command'){
      out.push({id:'cmd', k:'Command', v:[t.command].concat(t.aliases || []).join(' ')});
      out.push({id:'who', k:'Who', v:rolesLabel(t.roles)});
    }
    if (step && step.type === 'chat') out.push({id:'reply', k:'Reply as', v:step.params.as === 'bot' ? 'bot account' : 'your account'});
    if (step && step.type === 'phone') out.push({id:'priority', k:'Priority', v:step.params.priority || 'normal'});
    if (step && step.type === 'http') out.push({id:'url', k:'Send to', v:step.params.url || 'set the address'});
    if (step && step.type === 'file') out.push({id:'file', k:'File', v:step.params.path || 'pick a file'});
    out.push({id:'cooldown', k:'Wait between', v:fmtMs(d.cooldown_ms) || 'no limit'});
    return out;
  },
  filterVars(t){
    const e = eventOf(t.event);
    return e ? e.vars.filter(v => v.name !== 'hold' && v.name !== 'down_for' && v.name !== 'previous') : [];
  },
  sendValue(h){
    const first = (h.steps || [])[0];
    if (first && first.type === 'wait_delay') return {id:'aired', label:'once the delay aired'};
    if (first && first.type === 'wait'){
      const ms = parseInt(first.params.ms, 10) || 0;
      return {id:'w' + ms, label:'after ' + fmtMs(ms)};
    }
    return {id:'now', label:'right away'};
  },
  popHtml(step, h){
    if (this.chip === '__test') return this.runHtml();
    const d = this.d, t = h.trigger;
    const seg = (items, cur, act) => `<div class="sub-tabs ig-seg">${items.map(([id, label]) =>
      `<button class="sub-tab${id === cur ? ' on' : ''}" data-act="${act}" data-v="${esc(id)}">${esc(label)}</button>`).join('')}</div>`;
    switch (this.chip){
      case 'where': {
        const cur = (allSteps(d).find(s => s.type === 'discord') || {params:{}}).params.connection;
        return `<div class="ic-label">Post in</div><div class="ig-pick">
          ${S.data.connections.discord.map(c => `<button class="dest-starter${c.id === cur ? ' on' : ''}" style="--dc:#a5b4fc" data-act="set-where" data-v="${esc(c.id)}">
            <span class="dest-starter-name" style="font-size:13px">#${esc(c.name)}</span><span class="dest-starter-note mono">${esc(c.hint)}</span></button>`).join('')}
          <button class="dest-add" style="min-height:0;padding:10px 16px" data-act="conn" data-tab="discord">+ Add a Discord channel</button></div>`;
      }
      case 'ping':
        return `<div class="ic-label">Ping</div>${seg([['', 'Nobody'], ['here', '@here'], ['everyone', '@everyone']], step.params.ping || '', 'set-ping')}
          <div class="muted">Text from chat or a web service can never ping anyone: only this option can.</div>`;
      case 'send':
        return `<div class="ic-label">When to send</div>${seg([['now', 'Right away'], ['w10000', '10 s'], ['w20000', '20 s'], ['w30000', '30 s'], ['w60000', '1 min'], ['aired', 'Once the delay aired']],
          this.sendValue(h).id, 'set-send')}
          <div class="muted">${t.type === 'event' && t.event === 'hold_opened' ? 'Waiting a bit means a 3-second blip never reaches anyone.' : '"Once the delay aired" waits until viewers have seen the moment.'}</div>`;
      case 'only':
        return `<div class="ic-label">Only if</div><div class="dfg">${this.filterVars(t).map(v => {
          const cur = (t.filters || {})[v.name] || '';
          const choices = FILTER_CHOICES[v.name];
          const input = choices
            ? `<select class="ic-input" data-bind="filter" data-name="${esc(v.name)}"><option value="">any</option>${choices.map(c => `<option ${c === cur ? 'selected' : ''}>${esc(c)}</option>`).join('')}</select>`
            : `<input class="ic-input" data-bind="filter" data-name="${esc(v.name)}" value="${esc(cur)}" placeholder="any (e.g. ${esc(v.sample)})">`;
          return `<div class="dff"><label>${esc(v.name.replace(/_/g, ' '))}</label>${input}</div>`;
        }).join('')}</div>`;
      case 'cmd':
        return `<div class="dfg"><div class="dff"><label>Command</label><input class="ic-input mono" data-bind="command" value="${esc(t.command)}"></div>
          <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="aliases" value="${esc((t.aliases || []).join(' '))}" placeholder="!retraso !d"></div></div>
          <div class="dff" style="max-width:360px"><label>Per viewer, at most once every</label>${seg([['0', 'no limit'], ['10000', '10 s'], ['30000', '30 s'], ['60000', '1 min']], String(t.user_cooldown_ms || 0), 'set-ucd')}</div>`;
      case 'who':
        return `<div class="ic-label">Who can use it</div><div class="checkbox-row" style="margin-top:0">${['everyone', 'subs', 'vips', 'mods'].map(r =>
          `<label><input type="checkbox" data-bind="role" data-name="${r}" ${t.roles && t.roles[r] ? 'checked' : ''}> ${r === 'everyone' ? 'Everyone' : r.toUpperCase() === 'VIPS' ? 'VIPs' : r[0].toUpperCase() + r.slice(1)}</label>`).join('')}</div>
          <div class="muted">You can always use your own commands.</div>`;
      case 'reply':
        return `<div class="ic-label">Reply as</div>${seg([['', 'Your account'], ['bot', 'Bot account']], step.params.as || '', 'set-as')}
          ${t.type.startsWith('chat') ? `<label class="crash-check" style="margin-top:4px"><input type="checkbox" data-bind="reply" ${step.params.reply === 'yes' ? 'checked' : ''}><span><span class="crash-check-title">Reply in the viewer's thread</span><span class="muted">Twitch shows it as an answer to their message</span></span></label>` : ''}
          ${S.data.twitch.bot.login ? '' : '<div class="muted">No bot account yet: <a href="#" data-act="conn" data-tab="twitch">connect one</a>, or messages go out as you.</div>'}`;
      case 'priority':
        return `<div class="ic-label">Priority</div>${seg([['', 'Normal'], ['high', 'High'], ['urgent', 'Urgent']], step.params.priority || '', 'set-priority')}`;
      case 'url':
        return `<div class="dfg"><div class="dff"><label>Address</label><input class="ic-input mono" data-bind="url" value="${esc(step.params.url || '')}" placeholder="https://…"></div>
          <div class="dff"><label>Method</label><select class="ic-input" data-bind="method">${['POST', 'GET', 'PUT', 'PATCH', 'DELETE'].map(m => `<option ${m === (step.params.method || 'POST') ? 'selected' : ''}>${m}</option>`).join('')}</select></div></div>`;
      case 'file':
        return `<div class="dfg"><div class="dff"><label>File</label><input class="ic-input mono" data-bind="path" value="${esc(step.params.path || '')}" placeholder="C:\\Stream\\crashes.txt"></div>
          <div class="dff"><label>Each time</label><select class="ic-input" data-bind="mode"><option value="" ${step.params.mode !== 'append' ? 'selected' : ''}>Replace the text</option><option value="append" ${step.params.mode === 'append' ? 'selected' : ''}>Add a line</option></select></div></div>
          <div class="muted">Point an OBS text source at this file to show it on stream.</div>`;
      case 'cooldown':
        return `<div class="ic-label">Wait between two runs</div>${seg([['0', 'No limit'], ['10000', '10 s'], ['30000', '30 s'], ['60000', '1 min'], ['300000', '5 min'], ['600000', '10 min']], String(d.cooldown_ms || 0), 'set-cooldown')}`;
    }
    return `<div class="muted" style="min-height:70px;display:grid;place-items:center">Every option lives in a chip above. Click one to change it.</div>`;
  },
  runHtml(){
    if (!this.run) return '<div class="muted">Running the test…</div>';
    if (this.run.error) return `<div class="lan-warn" style="margin-top:0"><span class="lw-ic">⚠</span><span>${esc(this.run.error)}</span></div>`;
    return `<div class="ic-label">Test run: ${esc(this.run.status)}</div>${runLogHtml(this.run.steps)}
      <div class="muted">Messages and web requests went out for real, messages marked [TEST] and never pinging anyone. Waits were skipped; the stream, VOD, programs and files were left alone.</div>`;
  },
  lastRunHtml(){
    const st = stat(this.d);
    if (!st || !st.last_ms) return 'Hasn\'t run yet';
    const color = st.last_status === 'failed' ? 'var(--danger)' : 'var(--live)';
    return `<span style="display:inline-flex;align-items:center;gap:6px"><span style="width:6px;height:6px;border-radius:50%;background:${color}"></span>Last ran ${esc(fmtAgo(st.last_ms))} · ${esc(st.last_status)}</span>`;
  },
  changed(rerender){
    this.dirty = true;
    if (rerender) this.render();
  },
  input(el){
    const h = this.handler(), step = primaryStep(h), t = h.trigger;
    const b = el.dataset.bind;
    if (b === 'name') this.d.name = el.value;
    else if (b === 'enabled') this.d.enabled = el.checked;
    else if (b === 'text' && step){
      step.params[KINDS[step.type].text] = el.value;
      autosize(el);
      const s = $('ig-sample');
      if (s) s.innerHTML = sampleLine(el.value, h);
    }
    else if (b === 'title' && step) step.params.title = el.value;
    else if (b === 'filter'){ t.filters = t.filters || {}; t.filters[el.dataset.name] = el.value.trim(); }
    else if (b === 'command') t.command = el.value.trim().toLowerCase();
    else if (b === 'aliases') t.aliases = el.value.split(/[\s,]+/).map(x => x.trim().toLowerCase()).filter(Boolean);
    else if (b === 'role'){ t.roles = t.roles || {}; t.roles[el.dataset.name] = el.checked; }
    else if (b === 'reply' && step) step.params.reply = el.checked ? 'yes' : '';
    else if ((b === 'url' || b === 'method') && step){
      // A catalog webhook sends every event to one address: set them all.
      allSteps(this.d).filter(s => s.type === 'http').forEach(s => { s.params[b] = el.value; });
    }
    else if ((b === 'path' || b === 'mode') && step) step.params[b] = el.value;
    else return;
    this.dirty = true;
    if (el.tagName === 'SELECT' || el.type === 'checkbox') this.refreshChips();
  },
  refreshChips(){
    const h = this.handler(), step = primaryStep(h);
    const chips = Modal.el && Modal.el.querySelector('.ig-chips');
    if (chips) chips.innerHTML = this.chips(step, h).map(c => `<button class="ig-chip${this.chip === c.id ? ' on' : ''}${c.warn ? ' warn' : ''}" data-act="chip" data-chip="${c.id}">
      <span>${esc(c.k)}</span><b>${esc(c.v)}</b>${svg('chev')}</button>`).join('');
  },
  act(a, el){
    const h = this.handler(), step = primaryStep(h), v = el.dataset.v;
    switch (a){
      case 'moment': this.sel = +el.dataset.n; if (this.chip === '__test') this.chip = null; this.render(); return true;
      case 'moment-toggle': { const x = this.d.handlers[+el.dataset.n]; x.enabled = !x.enabled; this.changed(true); return true; }
      case 'chip': this.chip = this.chip === el.dataset.chip ? null : el.dataset.chip; this.render(); return true;
      case 'token': insertAtCursor(Modal.el.querySelector('.ig-msg'), '{' + el.dataset.token + '}'); return true;
      case 'more-vars': this.moreVars = true; this.render(); return true;
      case 'set-where': allSteps(this.d).filter(s => s.type === 'discord').forEach(s => { s.params.connection = v; }); this.changed(true); return true;
      case 'set-ping': if (step) step.params.ping = v; this.changed(true); return true;
      case 'set-send': setSend(h, v); this.changed(true); return true;
      case 'set-ucd': h.trigger.user_cooldown_ms = +v; this.changed(true); return true;
      case 'set-as': if (step) step.params.as = v; this.changed(true); return true;
      case 'set-priority': if (step) step.params.priority = v; this.changed(true); return true;
      case 'set-cooldown': this.d.cooldown_ms = +v; this.changed(true); return true;
      case 'ed-builder': { const d = this.d; this.dirty = false; Modal.close(true); Builder.open(d, true); return true; }
      case 'ed-test': this.test(); return true;
      case 'ed-delete': remove(this.d.id); return true;
      case 'ed-save': saveIntegration(this.d, () => { this.dirty = false; }); return true;
    }
    return false;
  },
  async test(){
    this.chip = '__test';
    this.run = null;
    this.render();
    const r = await api('/integrations/test', {integration:this.d, handler:this.sel});
    this.run = r.ok ? r : {error:r.error || 'The test failed'};
    if (Modal.kind === 'editor') this.render();
  },
};

// How a message with variables will read, or nothing when it has none.
function sampleLine(text, h){
  if (!/\{[A-Za-z0-9_.]/.test(text || '')) return '';
  return 'Reads like: <b>' + esc(renderSample(text, sampleMap(h))) + '</b>';
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
function fmtMs(ms){
  ms = +ms || 0;
  if (!ms) return '';
  if (ms < 60000) return Math.round(ms / 1000) + ' s';
  return Math.round(ms / 60000) + ' min';
}
function autosize(t){ t.style.height = 'auto'; t.style.height = (t.scrollHeight + 2) + 'px'; }
function insertAtCursor(field, text){
  if (!field) return;
  const start = field.selectionStart == null ? field.value.length : field.selectionStart;
  const end = field.selectionEnd == null ? start : field.selectionEnd;
  field.value = field.value.slice(0, start) + text + field.value.slice(end);
  field.focus();
  field.setSelectionRange(start + text.length, start + text.length);
  field.dispatchEvent(new Event('input', {bubbles:true}));
}
function runLogHtml(steps){
  if (!steps || !steps.length) return '<div class="muted">No steps ran.</div>';
  return `<div class="ig-run">${steps.map(s => `<div><span class="t">${(s.at_ms / 1000).toFixed(2)} s</span>
    <span class="${esc(s.status)}">${s.status === 'ok' ? '✓' : s.status === 'failed' ? '✕' : '–'} ${esc(s.label)}</span>
    <span class="d" title="${esc(s.detail)}">${esc(s.detail)}</span></div>`).join('')}</div>`;
}

async function saveIntegration(d, onSaved){
  const r = await api('/integrations/save', d);
  if (!r.ok){ toast(r.error || 'Could not save', 'err', 6000); return false; }
  if (onSaved) onSaved();
  if (r.missing && r.missing.length){
    toast(`Saved as a draft. Still to do: ${r.missing.join('; ')}`, 'info', 7000);
  } else if (r.warnings && r.warnings.length){
    toast(`Saved. Check these names, nothing fills them: ${r.warnings.map(w => '{' + w + '}').join(', ')}`, 'info', 7000);
  } else toast('Saved', 'ok');
  Modal.close(true);
  await load();
  return true;
}

// ---------------------------------------------------------------- builder

// The step editor: any trigger, any steps, checks with then/otherwise.
// Selection and insertion points are paths like [2,'then',0].
const PALETTE = [
  ['Send', [['discord', 'Discord'], ['chat', 'Twitch chat'], ['phone', 'Phone'], ['http', 'Web request']]],
  ['Flow', [['if', 'If / otherwise'], ['wait', 'Wait'], ['wait_delay', 'Wait for the delay'], ['stop', 'Stop']]],
  ['Stream', [['delay_action', 'Delay action'], ['marker', 'VOD marker'], ['clip', 'Clip']]],
  ['Anything', [['program', 'Run a program'], ['file', 'Write a file'], ['set_var', 'Remember a value'], ['counter', 'Counter']]],
];
const TRIGGERS = [['event', 'An InstantClone event'], ['chat_command', 'A chat command'], ['chat_message', 'A chat message'], ['timer', 'Every few minutes'], ['webhook', 'A web call (URL)']];

const Builder = {
  d:null, h:0, sel:'trigger', target:[], run:null, dirty:false,
  open(i, fromEditor){
    this.d = i ? withHandler(clone(i)) : {id:'', name:'My integration', enabled:true, preset:'', cooldown_ms:0,
      handlers:[{enabled:true, trigger:{type:'event', event:'hold_opened', filters:{}}, steps:[]}]};
    this.h = 0;
    this.sel = 'trigger';
    this.target = [];
    this.run = null;
    this.dirty = !!fromEditor;
    Modal.open('builder', '', 1240);
    Modal.onClose = () => !this.dirty || confirm('Close without saving your changes?');
    this.render();
  },
  handler(){ return this.d.handlers[this.h]; },
  resume(){
    if (pickNewChannel(this.d)) this.dirty = true;
    Modal.open('builder', '', 1240);
    Modal.onClose = () => !this.dirty || confirm('Close without saving your changes?');
    this.renderKeepScroll();
  },
  render(){
    const d = this.d;
    Modal.body(`${modalHead('steps', 'var(--accent)',
      `<input data-bind="b-name" value="${esc(d.name)}" aria-label="Name" style="width:100%;max-width:420px;padding:1px 0;border:0;border-bottom:1px dashed var(--line-2);background:transparent;color:var(--fg);font:inherit;font-weight:600;outline:none">`,
      `Your own · runs on this PC · ${allSteps(d).length} steps`,
      `<button class="ic-btn" style="padding:8px 14px" data-act="b-test">${svg('play', ' style="width:13px;height:13px"')} Test run</button>
       <label class="dest-form-enable"><input type="checkbox" data-bind="b-enabled" ${d.enabled ? 'checked' : ''}><span class="dfe-track"><span></span></span><span>Enabled</span></label>`)}
      <div class="ig-bld">
        <aside class="ig-palette" aria-label="Blocks">
          <div class="muted" style="padding:2px 4px 4px;line-height:1.45">Click a block to add it where the dashed box is lit.</div>
          ${PALETTE.map(([group, items]) => `<div class="ic-label">${group}</div>${items.map(([k, label]) =>
            `<button data-act="b-add" data-kind="${k}" style="--c:${KINDS[k].c}"><i></i>${esc(label)}</button>`).join('')}`).join('')}
        </aside>
        <section class="ig-canvas" aria-label="Steps">
          <div class="ig-triggers">${d.handlers.map((h, n) => `<button class="sub-tab${n === this.h ? ' on' : ''}" data-act="b-handler" data-n="${n}"
            style="flex:0 0 auto;padding:6px 12px;border-radius:999px;border:1px solid var(--line);${n === this.h ? 'background:var(--surface-3)' : ''}${h.enabled ? '' : ';opacity:.5'}">When ${esc(triggerLabel(h.trigger))}</button>`).join('')}
            <button class="sub-tab" data-act="b-add-handler" style="flex:0 0 auto;padding:6px 12px;border-radius:999px;border:1px dashed var(--line-2)">+ Another trigger</button></div>
          <div class="ig-steps">
            <button class="ig-step${this.sel === 'trigger' ? ' on' : ''}" data-act="b-sel" data-path="trigger" style="--c:#5ac8fa;border-color:${this.sel === 'trigger' ? '' : 'color-mix(in oklch,var(--accent) 35%,var(--line))'}">
              <span class="ig-step-kind">WHEN</span><span class="ig-step-text">${esc(triggerSentence(this.handler().trigger))}</span></button>
            ${this.stepsHtml(this.handler().steps, [])}
          </div>
          ${this.run ? `<div class="sys-section" style="margin-top:16px"><div class="ic-label" style="margin-bottom:8px">Test run: ${esc(this.run.status || 'error')}</div>
            ${this.run.error ? `<div class="lan-warn" style="margin-top:0"><span class="lw-ic">⚠</span><span>${esc(this.run.error)}</span></div>` : runLogHtml(this.run.steps)}</div>` : ''}
        </section>
        <aside class="ig-inspector" aria-label="Settings">${this.inspectorHtml()}</aside>
      </div>
      <div class="dest-form-foot">
        <div class="dest-form-msg muted" style="flex:1">${d.id ? Editor.lastRunHtml.call({d}) : 'New integration'}</div>
        ${d.id ? '<button class="ic-btn ic-btn-ghost" data-act="b-delete">Delete</button>' : ''}
        <button class="ic-btn ic-btn-primary" data-act="b-save">Save</button>
      </div>`);
    const f = Modal.el.querySelector('.dest-form');
    f.style.height = 'min(92vh,900px)';
  },
  stepsHtml(steps, base){
    const key = p => p.join('.');
    const tgt = key(this.target);
    const out = (steps || []).map((s, n) => {
      const path = base.concat(n);
      const k = KINDS[s.type] || {label:s.type, tag:'?', c:'#888'};
      const selected = Array.isArray(this.sel) && key(this.sel) === key(path);
      let html = `<div class="ig-step${selected ? ' on' : ''}" data-act="b-sel" data-path="${key(path)}" style="--c:${k.c}">
        <span class="ig-step-kind">${k.tag}</span><span class="ig-step-text">${stepSentence(s)}</span>
        <span class="ig-step-tools">
          <button class="ic-btn-tiny" data-act="b-move" data-path="${key(path)}" data-dir="-1" aria-label="Move up">${svg('up', ' style="width:13px;height:13px"')}</button>
          <button class="ic-btn-tiny" data-act="b-move" data-path="${key(path)}" data-dir="1" aria-label="Move down">${svg('down', ' style="width:13px;height:13px"')}</button>
          <button class="ic-btn-tiny" data-act="b-dup" data-path="${key(path)}" aria-label="Duplicate">${svg('copy', ' style="width:13px;height:13px"')}</button>
          <button class="ic-btn-tiny" data-act="b-del" data-path="${key(path)}" aria-label="Remove">${svg('x', ' style="width:13px;height:13px"')}</button>
        </span></div>`;
      if (s.type === 'if'){
        html += `<div class="ig-branch"><div class="ig-branch-label">Then</div>${this.stepsHtml(s.then, path.concat('then'))}
          <div class="ig-branch-label" style="color:var(--warn)">Otherwise</div>${this.stepsHtml(s.else, path.concat('else'))}</div>`;
      }
      return html;
    }).join('');
    return out + `<button class="ig-drop${tgt === key(base) ? ' on' : ''}" data-act="b-target" data-path="${key(base)}">${tgt === key(base) ? 'New blocks land here' : '+ Add a step here'}</button>`;
  },
  resolve(pathStr){
    const parts = pathStr === '' ? [] : pathStr.split('.').map(p => /^\d+$/.test(p) ? +p : p);
    return parts;
  },
  // The list a path's last index lives in, and that index.
  locate(path){
    let list = this.handler().steps;
    for (let n = 0; n < path.length - 1; n++){
      const p = path[n];
      if (typeof p === 'number'){
        const s = list[p];
        const branch = path[n + 1];
        list = s[branch];
        n++;
      }
    }
    return {list, index:path[path.length - 1]};
  },
  listAt(path){
    if (!path.length) return this.handler().steps;
    const s = this.stepAt(path.slice(0, -1));
    return s[path[path.length - 1]];
  },
  stepAt(path){
    const {list, index} = this.locate(path);
    return list[index];
  },
  inspectorHtml(){
    const h = this.handler();
    const tokens = varsFor(h).map(v => `<button class="ig-token" data-act="b-token" data-token="${esc(v.name)}" title="e.g. ${esc(v.sample)}">${esc(v.name)}</button>`).join('');
    const varsBox = `<div style="margin-top:auto"><div class="ic-label" style="margin-bottom:8px">Variables here</div>
      <div class="ig-tokens">${tokens}</div><div class="muted" style="margin-top:6px;font-size:11.5px">Click one to insert it in the field you last used. <span class="mono">{name|text}</span> uses the text when the value is empty or 0.</div></div>`;
    if (this.sel === 'trigger') return this.triggerInspector(h) + varsBox;
    const s = this.stepAt(this.sel);
    if (!s) return varsBox;
    const k = KINDS[s.type];
    const field = (name, label, opts) => {
      opts = opts || {};
      const v = s.params[name] || '';
      if (opts.select) return `<div class="dff"><label>${label}</label><select class="ic-input" data-bind="p" data-name="${name}">${opts.select.map(([id, l]) => `<option value="${esc(id)}" ${id === v ? 'selected' : ''}>${esc(l)}</option>`).join('')}</select></div>`;
      if (opts.area) return `<div class="dff"><label>${label}</label><textarea class="ic-input" data-bind="p" data-name="${name}" placeholder="${esc(opts.ph || '')}">${esc(v)}</textarea></div>`;
      return `<div class="dff"><label>${label}</label><input class="ic-input${opts.mono ? ' mono' : ''}" data-bind="p" data-name="${name}" value="${esc(v)}" placeholder="${esc(opts.ph || '')}"></div>`;
    };
    const channels = S.data.connections.discord.map(c => [c.id, '#' + c.name]);
    let body = '';
    switch (s.type){
      case 'discord': body = (channels.length ? field('connection', 'Channel', {select:[['', 'Pick a channel']].concat(channels)}) : '<div class="muted">No Discord channel yet. <a href="#" data-act="conn" data-tab="discord">Add one</a>.</div>')
        + field('text', 'Message', {area:true}) + field('ping', 'Ping', {select:[['', 'Nobody'], ['here', '@here'], ['everyone', '@everyone']]}); break;
      case 'chat': body = field('text', 'Message', {area:true}) + field('as', 'Send as', {select:[['', 'Your account'], ['bot', 'Bot account']]})
        + field('reply', 'Reply to the viewer', {select:[['', 'No'], ['yes', 'Yes, in their thread']]}); break;
      case 'phone': body = field('title', 'Title') + field('text', 'Message', {area:true}) + field('priority', 'Priority', {select:[['', 'Normal'], ['high', 'High'], ['urgent', 'Urgent']]}); break;
      case 'http': body = field('method', 'Method', {select:['POST', 'GET', 'PUT', 'PATCH', 'DELETE'].map(m => [m, m])}) + field('url', 'Address', {mono:true, ph:'https://…'})
        + field('headers', 'Headers (one per line, Name: value)', {area:true}) + field('body', 'Body', {area:true, ph:'{"event":"{delay}"}'})
        + field('save_as', 'Save the answer as', {mono:true, ph:'response'}) + `<div class="muted">Later steps can use <span class="mono">{${esc(s.params.save_as || 'response')}.status}</span>, <span class="mono">.ok</span>, <span class="mono">.body</span> and <span class="mono">.json.field</span>.</div>`; break;
      case 'wait': body = field('ms', 'Wait (milliseconds)', {mono:true, ph:'20000'}) + `<div class="muted">${esc(fmtMs(s.params.ms) || '0 s')}. Up to an hour.</div>`; break;
      case 'wait_delay': body = `<p style="margin:0;font-size:13px;line-height:1.55;color:var(--fg-2)">Waits until the moment this started has reached your viewers. Only InstantClone can do this: it knows your delay.</p>`
        + field('extra_ms', 'Then wait a bit more (ms)', {mono:true, ph:'0'}) + field('follow', 'If the delay changes meanwhile', {select:[['', 'Follow it'], ['no', 'Keep the delay from the start']]}); break;
      case 'if': body = field('left', 'Check', {mono:true, ph:'{user_role}'}) + field('op', 'Is', {select:Object.entries(OPS)}) + field('right', 'Value', {mono:true, ph:'mod'})
        + '<div class="muted">Text compares ignoring case; "is more than" compares numbers.</div>'; break;
      case 'stop': body = '<div class="muted">Ends this run here.</div>'; break;
      case 'delay_action': body = field('action', 'Action', {select:Object.entries(DELAY_ACTIONS)})
        + (s.params.action === 'arm' ? field('seconds', 'Delay (seconds)', {mono:true, ph:'{arg1}'})
          + '<div class="muted">Arms this delay, or changes it live when a delay is already on air. Never disarms.</div>' : '')
        + '<div class="lan-warn" style="margin-top:0"><span class="lw-ic">⚠</span><span>This changes your stream. Tests never run it.</span></div>'; break;
      case 'marker': body = field('description', 'Label', {ph:'Crash'}) + '<div class="muted">Only works while you are live on Twitch.</div>'; break;
      case 'clip': body = '<div class="muted">Clips the last moments of your stream. Later steps can use <span class="mono">{clip.url}</span> and <span class="mono">{clip.ok}</span>.</div>'; break;
      case 'program': body = field('path', 'Program (a fixed path, no variables)', {mono:true, ph:'C:\\Tools\\thing.exe'}) + field('args', 'Arguments', {mono:true, ph:'--scene "Replay"'})
        + '<div class="lan-warn" style="margin-top:0"><span class="lw-ic">⚠</span><span>Runs on your PC. Recipes you import can never add this without asking you first.</span></div>'; break;
      case 'file': body = field('path', 'File (a fixed path, no variables)', {mono:true, ph:'C:\\Stream\\status.txt'}) + field('text', 'Text', {area:true}) + field('mode', 'Each time', {select:[['', 'Replace the text'], ['append', 'Add a line']]}); break;
      case 'set_var': body = field('name', 'Name', {mono:true, ph:'winner'}) + field('value', 'Value', {ph:'{user}'}) + '<div class="muted">Later steps use it as <span class="mono">{name}</span>. Lasts for this run.</div>'; break;
      case 'counter': body = field('name', 'Name', {mono:true, ph:'crashes'}) + field('op', 'Do', {select:[['', 'Add'], ['subtract', 'Subtract'], ['set', 'Set to'], ['reset', 'Reset to 0']]})
        + field('by', 'By', {mono:true, ph:'1'}) + `<div class="muted">Kept between runs. Use it anywhere as <span class="mono">{counter.${esc(s.params.name || 'name')}}</span>.</div>`; break;
    }
    return `<div><div style="font-size:9.5px;font-weight:700;letter-spacing:.1em;color:${k.c}">${k.tag}</div><div style="margin-top:3px;font-size:15px;font-weight:600">${esc(k.label)}</div></div>${body}${varsBox}`;
  },
  triggerInspector(h){
    const t = h.trigger;
    const typeSel = `<div class="dff"><label>When</label><select class="ic-input" data-bind="t-type">${TRIGGERS.map(([id, l]) => `<option value="${id}" ${id === t.type ? 'selected' : ''}>${l}</option>`).join('')}</select></div>`;
    let body = '';
    if (t.type === 'event'){
      const groups = {};
      S.data.events.forEach(e => { (groups[e.group] = groups[e.group] || []).push(e); });
      body = `<div class="dff"><label>Event</label><select class="ic-input" data-bind="t-event">${Object.entries(groups).map(([g, list]) =>
        `<optgroup label="${esc(g)}">${list.map(e => `<option value="${e.id}" ${e.id === t.event ? 'selected' : ''}>${esc(e.label)}</option>`).join('')}</optgroup>`).join('')}</select></div>`;
      const e = eventOf(t.event);
      (e ? e.vars : []).filter(v => FILTER_CHOICES[v.name] || v.name === 'destination').forEach(v => {
        const cur = (t.filters || {})[v.name] || '';
        const ch = FILTER_CHOICES[v.name];
        body += `<div class="dff"><label>Only if ${esc(v.name)} is</label>${ch
          ? `<select class="ic-input" data-bind="t-filter" data-name="${v.name}"><option value="">any</option>${ch.map(c => `<option ${c === cur ? 'selected' : ''}>${c}</option>`).join('')}</select>`
          : `<input class="ic-input" data-bind="t-filter" data-name="${v.name}" value="${esc(cur)}" placeholder="any">`}</div>`;
      });
    } else if (t.type === 'chat_command'){
      body = `<div class="dff"><label>Command</label><input class="ic-input mono" data-bind="t-command" value="${esc(t.command || '')}" placeholder="!clip"></div>
        <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="t-aliases" value="${esc((t.aliases || []).join(' '))}"></div>
        ${this.rolesHtml(t)}
        <div class="dff"><label>Per viewer, once every (seconds)</label><input class="ic-input mono" data-bind="t-ucd" value="${Math.round((t.user_cooldown_ms || 0) / 1000)}"></div>`;
    } else if (t.type === 'chat_message'){
      body = `<div class="dff"><label>Matches</label><input class="ic-input" data-bind="t-pattern" value="${esc(t.pattern || '')}" placeholder="gg*"></div>
        <div class="dff"><label>How</label><select class="ic-input" data-bind="t-mode">${[['contains', 'Contains'], ['starts_with', 'Starts with'], ['exact', 'Is exactly'], ['wildcard', 'Wildcard (* and ?)']].map(([id, l]) => `<option value="${id}" ${id === (t.mode || 'contains') ? 'selected' : ''}>${l}</option>`).join('')}</select></div>
        ${this.rolesHtml(t)}`;
    } else if (t.type === 'timer'){
      body = `<div class="dff"><label>Every (minutes)</label><input class="ic-input mono" data-bind="t-every" value="${Math.max(1, Math.round((t.every_ms || 600000) / 60000))}"></div>
        <label class="crash-check"><input type="checkbox" data-bind="t-live" ${t.only_live !== false ? 'checked' : ''}><span><span class="crash-check-title">Only while streaming</span></span></label>`;
    } else if (t.type === 'webhook'){
      const url = `${location.origin}/hooks/${t.token || ''}`;
      body = `<div class="dff"><label>Call this address</label><div style="display:flex;gap:6px"><input class="ic-input mono" readonly value="${esc(url)}" style="font-size:11.5px">
        <button class="ic-btn ic-btn-ghost" style="padding:8px 10px" data-act="b-copy" data-text="${esc(url)}">Copy</button></div></div>
        <div class="muted">GET or POST from a Stream Deck, a script or any app on this network. The body is <span class="mono">{body}</span>; JSON fields are <span class="mono">{body.field}</span>.</div>
        <button class="ic-btn ic-btn-ghost" style="align-self:flex-start;padding:7px 12px;font-size:12px" data-act="b-new-token">Make a new secret address</button>`;
    }
    const h2 = this.d.handlers.length > 1
      ? `<div style="display:flex;gap:8px"><label class="crash-check" style="flex:1"><input type="checkbox" data-bind="h-enabled" ${h.enabled ? 'checked' : ''}><span><span class="crash-check-title">This trigger is on</span></span></label>
         <button class="ic-btn ic-btn-ghost" style="padding:6px 10px;font-size:12px" data-act="b-del-handler">Remove trigger</button></div>` : '';
    return `<div><div style="font-size:9.5px;font-weight:700;letter-spacing:.1em;color:#5ac8fa">WHEN</div><div style="margin-top:3px;font-size:15px;font-weight:600">${esc(triggerLabel(t))}</div></div>
      ${typeSel}${body}
      <div class="dff"><label>Wait between two runs (seconds)</label><input class="ic-input mono" data-bind="b-cooldown" value="${Math.round((this.d.cooldown_ms || 0) / 1000)}"></div>${h2}`;
  },
  rolesHtml(t){
    return `<div class="dff"><label>Who can use it</label><div class="checkbox-row" style="margin-top:0">${['everyone', 'subs', 'vips', 'mods'].map(r =>
      `<label><input type="checkbox" data-bind="t-role" data-name="${r}" ${t.roles && t.roles[r] ? 'checked' : ''}> ${r === 'vips' ? 'VIPs' : r[0].toUpperCase() + r.slice(1)}</label>`).join('')}</div></div>`;
  },
  input(el){
    const b = el.dataset.bind, h = this.handler(), t = h.trigger;
    let rerender = false;
    if (b === 'b-name') this.d.name = el.value;
    else if (b === 'b-enabled') this.d.enabled = el.checked;
    else if (b === 'b-cooldown') this.d.cooldown_ms = Math.max(0, (parseFloat(el.value) || 0) * 1000);
    else if (b === 'h-enabled'){ h.enabled = el.checked; rerender = true; }
    else if (b === 't-type'){ h.trigger = newTrigger(el.value); rerender = true; }
    else if (b === 't-event'){ t.event = el.value; t.filters = {}; rerender = true; }
    else if (b === 't-filter'){ t.filters = t.filters || {}; t.filters[el.dataset.name] = el.value.trim(); rerender = true; }
    else if (b === 't-command'){ t.command = el.value.trim().toLowerCase(); rerender = true; }
    else if (b === 't-aliases') t.aliases = el.value.split(/[\s,]+/).map(x => x.trim().toLowerCase()).filter(Boolean);
    else if (b === 't-role'){ t.roles = t.roles || {}; t.roles[el.dataset.name] = el.checked; }
    else if (b === 't-ucd') t.user_cooldown_ms = Math.max(0, (parseFloat(el.value) || 0) * 1000);
    else if (b === 't-pattern') t.pattern = el.value;
    else if (b === 't-mode') t.mode = el.value;
    else if (b === 't-every') t.every_ms = Math.max(1, parseFloat(el.value) || 1) * 60000;
    else if (b === 't-live') t.only_live = el.checked;
    else if (b === 'p'){
      const s = this.stepAt(this.sel);
      s.params[el.dataset.name] = el.value;
      rerender = el.tagName === 'SELECT';
      const card = Modal.el.querySelector(`.ig-step[data-path="${this.sel.join('.')}"] .ig-step-text`);
      if (card) card.innerHTML = stepSentence(s);
    } else return;
    this.dirty = true;
    if (rerender) this.renderKeepScroll();
  },
  renderKeepScroll(){
    const c = Modal.el && Modal.el.querySelector('.ig-canvas');
    const top = c ? c.scrollTop : 0;
    this.render();
    const c2 = Modal.el && Modal.el.querySelector('.ig-canvas');
    if (c2) c2.scrollTop = top;
  },
  act(a, el){
    const d = this.d;
    switch (a){
      case 'b-sel': this.sel = el.dataset.path === 'trigger' ? 'trigger' : this.resolve(el.dataset.path); this.renderKeepScroll(); return true;
      case 'b-target': this.target = this.resolve(el.dataset.path); this.renderKeepScroll(); return true;
      case 'b-add': {
        let list;
        try { list = this.listAt(this.target); } catch(_){ list = null; }
        if (!list){ this.target = []; list = this.handler().steps; }
        list.push(newStep(el.dataset.kind));
        this.sel = this.target.concat(list.length - 1);
        if (el.dataset.kind === 'if') this.target = this.sel.concat('then');
        this.dirty = true;
        this.renderKeepScroll();
        return true;
      }
      case 'b-move': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        const to = index + (+el.dataset.dir);
        if (to < 0 || to >= list.length) return true;
        [list[index], list[to]] = [list[to], list[index]];
        this.sel = path.slice(0, -1).concat(to);
        this.dirty = true; this.renderKeepScroll(); return true;
      }
      case 'b-dup': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        list.splice(index + 1, 0, clone(list[index]));
        this.dirty = true; this.renderKeepScroll(); return true;
      }
      case 'b-del': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        list.splice(index, 1);
        this.sel = 'trigger';
        this.target = [];
        this.dirty = true; this.renderKeepScroll(); return true;
      }
      case 'b-handler': this.h = +el.dataset.n; this.sel = 'trigger'; this.target = []; this.run = null; this.renderKeepScroll(); return true;
      case 'b-add-handler':
        d.handlers.push({enabled:true, trigger:newTrigger('event'), steps:[]});
        this.h = d.handlers.length - 1; this.sel = 'trigger'; this.target = []; this.dirty = true; this.render(); return true;
      case 'b-del-handler':
        if (d.handlers.length > 1 && confirm('Remove this trigger and its steps?')){ d.handlers.splice(this.h, 1); this.h = 0; this.sel = 'trigger'; this.dirty = true; this.render(); }
        return true;
      case 'b-new-token': this.handler().trigger.token = randomToken(); this.dirty = true; this.render(); return true;
      case 'b-copy': navigator.clipboard.writeText(el.dataset.text); toast('Copied', 'ok'); return true;
      case 'b-token': insertAtCursor(Builder.lastField, '{' + el.dataset.token + '}'); return true;
      case 'b-test': this.test(); return true;
      case 'b-delete': remove(d.id); return true;
      case 'b-save': saveIntegration(d, () => { this.dirty = false; }); return true;
    }
    return false;
  },
  async test(){
    this.run = {status:'running', steps:[]};
    this.renderKeepScroll();
    const r = await api('/integrations/test', {integration:this.d, handler:this.h});
    this.run = r.ok ? r : {error:r.error || 'The test failed'};
    if (Modal.kind === 'builder') this.renderKeepScroll();
  },
  lastField:null,
};

function newTrigger(type){
  switch (type){
    case 'chat_command': return {type, command:'!command', aliases:[], roles:{everyone:true, subs:true, vips:true, mods:true}, user_cooldown_ms:30000};
    case 'chat_message': return {type, pattern:'', mode:'contains', roles:{everyone:true, subs:true, vips:true, mods:true}};
    case 'timer': return {type, every_ms:600000, only_live:true};
    case 'webhook': return {type, token:randomToken()};
  }
  return {type:'event', event:'hold_opened', filters:{}};
}
function newStep(kind){
  const p = {
    discord:{connection:(S.data.connections.discord[0] || {}).id || '', text:''},
    chat:{text:''}, phone:{title:'InstantClone', text:''}, http:{method:'POST', url:'', body:''},
    wait:{ms:'10000'}, wait_delay:{}, if:{left:'', op:'is', right:''}, stop:{},
    delay_action:{action:'cut'}, marker:{description:'Highlight'}, clip:{}, program:{path:'', args:''},
    file:{path:'', text:'', mode:''}, set_var:{name:'', value:''}, counter:{name:'count', op:'', by:'1'},
  }[kind] || {};
  return {type:kind, params:clone(p), then:[], else:[]};
}
function triggerSentence(t){
  switch (t.type){
    case 'event': { const f = Object.entries(t.filters || {}).filter(([, v]) => v).map(([k, v]) => `${k} is ${v}`);
      return triggerLabel(t) + (f.length ? ' · only if ' + f.join(', ') : ''); }
    case 'chat_command': return `Someone types ${t.command || '!command'} · ${rolesLabel(t.roles)}`;
    case 'chat_message': return `A chat message ${t.mode === 'exact' ? 'is' : t.mode === 'starts_with' ? 'starts with' : 'matches'} "${t.pattern || ''}"`;
    case 'timer': return `Every ${Math.round((t.every_ms || 0) / 60000)} min${t.only_live !== false ? ' while streaming' : ''}`;
    case 'webhook': return 'Something calls its secret address';
  }
  return t.type;
}
function stepSentence(s){
  const p = s.params || {};
  const q = v => `“${highlightVars(v || '')}”`;
  switch (s.type){
    case 'discord': return `Discord ${esc(channelName(p.connection))} ${q(p.text)}${p.ping ? ' · @' + esc(p.ping) : ''}`;
    case 'chat': return `Chat ${q(p.text)}${p.as === 'bot' ? ' · as bot' : ''}${p.reply === 'yes' ? ' · as a reply' : ''}`;
    case 'phone': return `Phone ${q(p.text)}`;
    case 'http': return `${esc(p.method || 'POST')} ${esc(p.url || '(address)')}${p.save_as ? ' → ' + esc(p.save_as) : ''}`;
    case 'wait': return `Wait ${esc(fmtMs(p.ms) || '0 s')}`;
    case 'wait_delay': return 'Wait until it has aired for viewers <span class="v">{delay}</span>';
    case 'if': return `If ${highlightVars(p.left || '…')} ${esc(OPS[p.op] || p.op || '')} ${highlightVars(p.right || '')}`;
    case 'stop': return 'Stop here';
    case 'delay_action': return esc(DELAY_ACTIONS[p.action] || 'Delay action') + (p.action === 'arm' && p.seconds ? ' to ' + highlightVars(p.seconds) + ' s' : '');
    case 'marker': return `VOD marker ${q(p.description)}`;
    case 'clip': return 'Create a clip';
    case 'program': return `Run ${esc((p.path || '(program)').split(/[\\/]/).pop())} ${highlightVars(p.args || '')}`;
    case 'file': return `${p.mode === 'append' ? 'Add to' : 'Write'} ${esc((p.path || '(file)').split(/[\\/]/).pop())} ${q(p.text)}`;
    case 'set_var': return `Remember ${esc(p.name || '…')} = ${highlightVars(p.value || '')}`;
    case 'counter': return `Counter ${esc(p.name || '…')} ${p.op === 'reset' ? 'reset' : p.op === 'set' ? '= ' + esc(p.by || '0') : (p.op === 'subtract' ? '-' : '+') + esc(p.by || '1')}`;
  }
  return esc(s.type);
}

// ---------------------------------------------------------------- connections

const Conn = {
  tab:'discord', editing:null,
  open(tab){
    // Opened from an editor or an import: go back to it, work intact.
    const from = {editor:Editor, builder:Builder, import:Recipes}[Modal.kind] || null;
    this.tab = tab || 'discord';
    this.editing = null;
    Modal.open('connections', '', 760);
    Modal.onClose = () => { if (S.data.twitch.login) api('/twitch/cancel', {}); return true; };
    if (from) Modal.after = () => from.resume();
    this.render();
  },
  render(){
    const tabs = [['discord', 'Discord'], ['twitch', 'Twitch'], ['phone', 'Phone']].map(([id, l]) =>
      `<button class="sub-tab${this.tab === id ? ' on' : ''}" data-act="conn-tab" data-tab="${id}" style="flex:0 0 auto;padding:6px 14px;${this.tab === id ? 'background:var(--surface-3)' : ''}">${l}</button>`).join('');
    const focused = document.activeElement && Modal.el && Modal.el.contains(document.activeElement) ? document.activeElement.dataset.bind : null;
    Modal.body(`${modalHead('link', 'var(--accent)', 'Connections', 'Where integrations post. Set each one up once.')}
      <div class="dest-form-body" style="padding:18px 22px 22px;gap:14px">
        <div class="sub-tabs" role="tablist" style="align-self:flex-start">${tabs}</div>
        ${this.tab === 'discord' ? this.discordHtml() : this.tab === 'twitch' ? this.twitchHtml() : this.phoneHtml()}
      </div>`);
    if (focused){ const f = Modal.el.querySelector(`[data-bind="${focused}"]`); if (f) f.focus(); }
  },
  discordHtml(){
    const list = S.data.connections.discord;
    const e = this.editing;
    const rows = list.map(c => `<div class="ig-acct"><span class="dcard-icon" style="--dc:#a5b4fc">${svg('bubble')}</span>
      <div class="dcard-id"><div class="dcard-name">#${esc(c.name)}</div><div class="dcard-host">${esc(c.hint)}</div></div>
      <button class="ic-btn ic-btn-ghost" style="padding:6px 10px;font-size:12px" data-act="d-test" data-id="${esc(c.id)}">Test</button>
      <button class="ic-btn ic-btn-ghost" style="padding:6px 10px;font-size:12px" data-act="d-edit" data-id="${esc(c.id)}">Edit</button>
      <button class="ic-btn-tiny" data-act="d-del" data-id="${esc(c.id)}" aria-label="Remove #${esc(c.name)}">${svg('x', ' style="width:13px;height:13px"')}</button></div>`).join('');
    const form = e ? `<div class="sys-section" style="display:flex;flex-direction:column;gap:12px">
        <div class="ic-label">${e.id ? 'Edit channel' : 'New channel'}</div>
        <div class="dfg"><div class="dff"><label>Name</label><input class="ic-input" data-bind="d-name" value="${esc(e.name)}" placeholder="Mods"></div>
        <div class="dff"><label>Webhook link</label><input class="ic-input mono" data-bind="d-url" value="" placeholder="${e.id ? 'leave blank to keep it' : 'https://discord.com/api/webhooks/…'}"></div></div>
        <div class="muted">In Discord: Server settings › Integrations › Webhooks › New webhook › Copy webhook URL.</div>
        <div style="display:flex;gap:8px;justify-content:flex-end"><button class="ic-btn ic-btn-ghost" data-act="d-cancel">Cancel</button><button class="ic-btn ic-btn-primary" data-act="d-save">Save channel</button></div>
      </div>` : `<button class="dest-add" style="min-height:52px;flex-direction:row" data-act="d-new"><span class="dest-add-icon" style="width:28px;height:28px">${svg('plus')}</span>Add a Discord channel</button>`;
    return (rows || '<div class="muted">No Discord channel yet.</div>') + form;
  },
  twitchHtml(){
    const t = S.data.twitch;
    if (!t.available) return `<div class="lan-warn" style="margin-top:0"><span class="lw-ic">⚠</span><span>This build has no Twitch app id, so it can't log in to Twitch. Official releases include one.</span></div>`;
    const flow = t.login;
    const acct = (which, a, title, blurb) => {
      const active = flow && flow.which === which;
      let right;
      if (active){
        right = flow.error
          ? `<button class="ic-btn" data-act="t-login" data-which="${which}">Try again</button>`
          : '<span class="muted">Waiting for you on Twitch…</span>';
      } else if (a.login){
        right = `<span class="dcard-status ${a.chat === 'connected' ? 's-live' : 's-wait'}" style="min-width:0"><span class="d-dot"></span><span class="dcard-status-l">chat ${esc(a.chat)}</span></span>
          <button class="ic-btn ic-btn-ghost" style="padding:6px 10px;font-size:12px" data-act="t-logout" data-which="${which}">Disconnect</button>`;
      } else {
        right = `<button class="ic-btn ${which === 'main' ? 'ic-btn-primary' : ''}" data-act="t-login" data-which="${which}">Connect</button>`;
      }
      const code = active && !flow.error ? `<div style="display:flex;flex-direction:column;gap:10px;margin-top:4px">
          ${flow.user_code ? `<div class="ig-code">${esc(flow.user_code)}</div>
            <div style="display:flex;gap:8px;justify-content:center">
              <a class="ic-btn ic-btn-primary" href="${esc(/^https:\/\//.test(flow.uri || '') ? flow.uri : 'https://www.twitch.tv/activate')}" target="_blank" rel="noopener" style="text-decoration:none">Open twitch.tv/activate</a>
              <button class="ic-btn ic-btn-ghost" data-act="b-copy" data-text="${esc(flow.user_code)}">Copy code</button>
              <button class="ic-btn ic-btn-ghost" data-act="t-cancel">Cancel</button></div>
            <div class="muted" style="text-align:center">Enter the code there and approve. This page updates on its own. The code works for ${Math.max(1, Math.round(flow.expires_in_s / 60))} more minutes.</div>`
          : '<div class="muted">Asking Twitch for a code…</div>'}</div>`
        : active && flow.error ? `<div class="lan-warn" style="margin-top:0"><span class="lw-ic">⚠</span><span>${esc(flow.error)}</span></div>` : '';
      return `<div class="sys-section" style="display:flex;flex-direction:column;gap:10px">
        <div class="ig-acct" style="padding:0;border:0;background:none"><span class="dcard-icon">${svg('bubble')}</span>
          <div class="dcard-id"><div class="dcard-name">${a.login ? esc(a.login) : title}</div><div class="dcard-host" style="font-family:inherit;white-space:normal">${blurb}</div></div>${right}</div>${code}</div>`;
    };
    return (t.notice ? `<div class="lan-warn" style="margin-top:0"><span class="lw-ic">⚠</span><span>${esc(t.notice)}</span></div>` : '')
      + acct('main', t.main, 'Your Twitch account', 'Chat, VOD markers and clips. Log in once: InstantClone keeps it fresh.')
      + acct('bot', t.bot, 'Bot account (optional)', 'A second account that posts in chat instead of you.')
      + '<div class="muted">No password ever goes through InstantClone: you approve it on twitch.tv, and can revoke it there any time.</div>';
  },
  phoneHtml(){
    const p = S.data.connections.phone;
    return `<div class="sys-section" style="display:flex;flex-direction:column;gap:12px">
      <div style="font-size:13px;line-height:1.6;color:var(--fg-2)">Phone pushes use <b>ntfy</b>, a free app for Android and iPhone. Install it, subscribe to a topic, and put the same topic here. Anyone who knows the topic can read it, so make it hard to guess.</div>
      <div class="dfg"><div class="dff"><label>Topic</label><div style="display:flex;gap:6px"><input class="ic-input mono" data-bind="p-topic" value="${esc(p.topic)}" placeholder="instantclone-…">
        <button class="ic-btn ic-btn-ghost" style="padding:8px 10px" data-act="p-random" title="Make a hard-to-guess topic">Random</button></div></div>
        <div class="dff"><label>Server <span class="muted">(optional)</span></label><input class="ic-input mono" data-bind="p-server" value="${esc(p.server)}" placeholder="https://ntfy.sh"></div></div>
      <div style="display:flex;gap:8px;justify-content:flex-end"><button class="ic-btn ic-btn-ghost" data-act="p-test" ${p.topic ? '' : 'disabled'}>Send a test push</button><button class="ic-btn ic-btn-primary" data-act="p-save">Save</button></div>
    </div>`;
  },
  async act(a, el){
    switch (a){
      case 'conn-tab': this.tab = el.dataset.tab; this.editing = null; this.render(); return true;
      case 'd-new': this.editing = {id:'', name:''}; this.render(); return true;
      case 'd-edit': { const c = S.data.connections.discord.find(x => x.id === el.dataset.id); this.editing = {id:c.id, name:c.name}; this.render(); return true; }
      case 'd-cancel': this.editing = null; this.render(); return true;
      case 'd-save': {
        const name = Modal.el.querySelector('[data-bind="d-name"]').value, url = Modal.el.querySelector('[data-bind="d-url"]').value;
        const r = await api('/connections/discord', {id:this.editing.id, name, url});
        if (!r.ok){ toast(r.error, 'err', 6000); return true; }
        this.editing = null; await refreshData(); this.render(); toast('Channel saved', 'ok'); return true;
      }
      case 'd-del':
        if (!confirm('Remove this channel? Integrations posting there switch off until you pick another.')) return true;
        await api('/connections/discord/delete', {id:el.dataset.id}); await refreshData(); this.render(); return true;
      case 'd-test': { const r = await api('/connections/discord/test', {id:el.dataset.id}); toast(r.ok ? 'Sent a test message, check Discord' : r.error, r.ok ? 'ok' : 'err', 5000); return true; }
      case 't-login': await api('/twitch/login', {which:el.dataset.which}); await refreshData(); schedulePoll(); this.render(); return true;
      case 't-cancel': await api('/twitch/cancel', {}); await refreshData(); this.render(); return true;
      case 't-logout':
        if (!confirm('Disconnect this Twitch account? Chat commands and chat messages stop until you connect again.')) return true;
        await api('/twitch/logout', {which:el.dataset.which}); setTimeout(async () => { await refreshData(); this.render(); }, 400); return true;
      case 'p-random': Modal.el.querySelector('[data-bind="p-topic"]').value = 'instantclone-' + randomToken().slice(0, 12); return true;
      case 'p-save': {
        const r = await api('/connections/phone', {topic:Modal.el.querySelector('[data-bind="p-topic"]').value, server:Modal.el.querySelector('[data-bind="p-server"]').value});
        if (!r.ok){ toast(r.error, 'err', 6000); return true; }
        await refreshData(); this.render(); toast('Phone saved', 'ok'); return true;
      }
      case 'p-test': { const r = await api('/connections/phone/test', {}); toast(r.ok ? 'Sent, check your phone' : r.error, r.ok ? 'ok' : 'err', 5000); return true; }
    }
    return false;
  },
};

async function refreshData(){
  const d = await api('/integrations');
  if (d && d.integrations){ S.data = d; if (S.visible) render(); }
}

// ---------------------------------------------------------------- recipes

const Recipes = {
  preview:null, text:'',
  resume(){
    Modal.open('import', '', 700);
    this.render();
  },
  openImport(){
    this.preview = null;
    this.text = '';
    Modal.open('import', '', 700);
    this.render();
  },
  render(){
    const p = this.preview;
    const channels = S.data.connections.discord;
    Modal.body(`${modalHead('download', 'var(--accent)', 'Import a recipe', "Paste a recipe someone shared. You see everything it adds before anything changes.")}
      <div class="dest-form-body" style="padding:18px 22px 22px;gap:14px">
        <textarea class="ic-input ig-textarea-code" data-bind="r-text" placeholder="ic-recipe:1:…">${esc(this.text)}</textarea>
        ${p ? `<div class="sys-section" style="display:flex;flex-direction:column;gap:8px">
          <div style="display:flex;align-items:center;gap:8px"><span style="color:var(--live)">✓</span><b>${esc(p.name)}</b><span class="muted">${p.items.length} integration${p.items.length === 1 ? '' : 's'}</span></div>
          ${p.items.map(it => `<div style="font-size:13px;color:var(--fg-2)">· <b style="color:var(--fg)">${esc(it.name)}</b> <span class="muted">${esc(it.summary)}</span></div>`).join('')}
        </div>
        ${p.uses_discord ? (channels.length ? `<div class="dff"><label>Post its Discord messages in</label><select class="ic-input" data-bind="r-discord">${channels.map(c => `<option value="${esc(c.id)}">#${esc(c.name)}</option>`).join('')}</select></div>`
          : '<div class="lan-warn" style="margin-top:0"><span class="lw-ic">⚠</span><span>It posts to Discord: <a href="#" data-act="conn" data-tab="discord">add a Discord channel</a> first.</span></div>') : ''}
        ${p.local_effects ? `<label class="crash-check lan-warn" style="margin-top:0"><input type="checkbox" data-bind="r-local"><span><span class="crash-check-title">I trust this recipe to run programs or write files on this PC</span><span class="muted">Only tick this if you know who made it.</span></span></label>` : ''}
        <div class="muted" style="display:flex;gap:8px"><span>🛡</span><span>Recipes are settings, not code. They can't see your keys, tokens or webhook links, and they arrive switched off.</span></div>` : ''}
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg" style="flex:1"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Cancel</button>
        ${p ? `<button class="ic-btn ic-btn-primary" data-act="r-apply">Add ${p.items.length}</button>` : '<button class="ic-btn ic-btn-primary" data-act="r-preview">Preview</button>'}
      </div>`);
  },
  async act(a){
    if (a === 'r-preview'){
      this.text = Modal.el.querySelector('[data-bind="r-text"]').value;
      const r = await api('/integrations/import', {recipe:this.text});
      if (!r.ok){ toast(r.error, 'err', 6000); return true; }
      this.preview = r; this.render(); return true;
    }
    if (a === 'r-apply'){
      const sel = Modal.el.querySelector('[data-bind="r-discord"]'), local = Modal.el.querySelector('[data-bind="r-local"]');
      const r = await api('/integrations/import', {recipe:this.text, apply:true, discord:sel ? sel.value : '', allow_local:!!(local && local.checked)});
      if (!r.ok){ toast(r.error, 'err', 6000); return true; }
      Modal.close(true); await load(); toast(`Added ${r.added}. They're off until you switch them on.`, 'ok', 4500); return true;
    }
    return false;
  },
  openShare(){
    Modal.open('share', '', 700);
    Modal.body(`${modalHead('copy', 'var(--accent)', 'Share as a recipe', 'Pick what to share. Your connections, keys and secret links are never included.')}
      <div class="dest-form-body" style="padding:18px 22px 22px;gap:12px">
        <div class="dff"><label>Recipe name</label><input class="ic-input" data-bind="s-name" placeholder="My crash kit"></div>
        <div style="display:flex;flex-direction:column;gap:6px;max-height:240px;overflow:auto">${S.data.integrations.map(i =>
          `<label class="crash-check"><input type="checkbox" data-bind="s-pick" value="${esc(i.id)}"><span><span class="crash-check-title">${esc(i.name)}</span></span></label>`).join('')}</div>
        <textarea class="ic-input ig-textarea-code" data-bind="s-out" readonly hidden></textarea>
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg" style="flex:1"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Close</button><button class="ic-btn ic-btn-primary" data-act="s-make">Make recipe</button></div>`);
  },
  async share(){
    const ids = Array.from(Modal.el.querySelectorAll('[data-bind="s-pick"]:checked')).map(x => x.value);
    const r = await api('/integrations/export', {ids, name:Modal.el.querySelector('[data-bind="s-name"]').value});
    if (!r.ok){ toast(r.error, 'err'); return; }
    const out = Modal.el.querySelector('[data-bind="s-out"]');
    out.hidden = false;
    out.value = r.recipe;
    out.select();
    navigator.clipboard.writeText(r.recipe).then(() => toast('Recipe copied: paste it anywhere', 'ok', 3500), () => {});
  },
};

// ---------------------------------------------------------------- events

async function onClick(e){
  const el = e.target.closest('[data-act]');
  if (!el) return;
  const a = el.dataset.act;
  if (el.tagName === 'A' || a === 'catalog' || a === 'build' || a === 'import') e.preventDefault();
  if (Modal.kind === 'editor' && Editor.act(a, el)) return;
  if (Modal.kind === 'builder' && Builder.act(a, el)) return;
  if (Modal.kind === 'connections' && await Conn.act(a, el)) return;
  if (Modal.kind === 'import' && await Recipes.act(a, el)) return;
  switch (a){
    case 'modal-close': Modal.close(); break;
    case 'catalog': Catalog.open(); break;
    case 'cat': Catalog.cat = el.dataset.cat; Catalog.render(); break;
    case 'add-preset': addFrom({preset:el.dataset.preset}, true); break;
    case 'pack': Modal.close(true); addFrom({pack:el.dataset.pack}, false); break;
    case 'build': Builder.open(null); break;
    case 'import': Recipes.openImport(); break;
    case 'share': Recipes.openShare(); break;
    case 's-make': Recipes.share(); break;
    case 'conn': Conn.open(el.dataset.tab); break;
    case 'edit': { const i = find(el.dataset.id); if (i) openEditor(i); break; }
    case 'toggle': toggle(el.dataset.id); break;
    case 'duplicate': duplicate(el.dataset.id); break;
    case 'peek': S.peek = S.peek === el.dataset.id ? null : el.dataset.id; render(); break;
    case 'filter': S.filter = el.dataset.f; render(); break;
    case 'view': S.view = el.dataset.v; store.set('ig-view', S.view); render(); break;
    case 'b-copy': navigator.clipboard.writeText(el.dataset.text); toast('Copied', 'ok'); break;
  }
}

function onInput(e){
  const el = e.target;
  if (el.id === 'ig-search'){
    S.search = el.value;
    const items = $('ig-items');
    if (items) items.innerHTML = itemsHtml();
    return;
  }
  if (!el.dataset || !el.dataset.bind) return;
  if (Modal.kind === 'editor') Editor.input(el);
  else if (Modal.kind === 'builder'){
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') Builder.lastField = el;
    Builder.input(el);
  }
}

// Enter or Space on a clickable card acts like a click.
function onActivateKey(e){
  if ((e.key === 'Enter' || e.key === ' ') && e.target.matches('[role="button"][data-act]')){
    e.preventDefault();
    e.target.click();
  }
}

// Remember the last text field the builder used, for variable inserts.
document.addEventListener('focusin', e => {
  if (Modal.kind === 'builder' && e.target.matches('.ig-inspector input:not([readonly]), .ig-inspector textarea')) Builder.lastField = e.target;
});

function wireRoot(){
  const root = $('ig-root');
  if (!root || root.dataset.wired) return;
  root.dataset.wired = '1';
  root.addEventListener('click', onClick);
  root.addEventListener('input', onInput);
  root.addEventListener('keydown', onActivateKey);
  root.addEventListener('toggle', e => { if (e.target.id === 'ig-activity-box') S.activityOpen = e.target.open; }, true);
}

window.Integrations = {
  async show(){
    S.visible = true;
    wireRoot();
    await load();
    schedulePoll();
  },
  hide(){
    S.visible = false;
    if (!modalOpen()) clearInterval(S.pollTimer);
  },
};

// A reload or a restored tab can land straight on Integrations.
const pane = document.querySelector('.tab-pane[data-pane="integrations"]');
if (pane && !pane.hidden) window.Integrations.show();
})();
