// Integrations tab: the module list, the catalog, the module editor, the
// step builder, connections (Discord, Twitch, phone) and recipes.
//
// Loaded after the dashboard script, so it reuses its helpers ($, toast,
// staggerIn, fitPaneCard, flashCopied, prefersReducedMotion) and its
// components: a module is a .dcard, every editor is the destination modal
// (.ic-modal + .dest-form). All user text goes through esc() before it
// touches the DOM, and every redraw goes through morph() (see Rendering).
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
  megaphone:'M3 11v2a1 1 0 0 0 1 1h3l5 4V6L7 10H4a1 1 0 0 0-1 1zM16 8.5a5 5 0 0 1 0 7M19 5.5a9 9 0 0 1 0 13',
  hash:'M4 9h16M4 15h16M10 3L8 21M16 3l-2 18',
  bell:'M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9M13.7 21a2 2 0 0 1-3.4 0',
  pencil:'M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z',
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
// The icon a catalog module wears: what it is about, not how it sends.
const PRESET_ICON = {
  crash_alert:'shield', destination_down:'signal', going_live:'megaphone', phone_crash:'phone',
  tell_chat:'bubble', delay_command:'hash', delay_notice:'clock', mod_controls:'cut', socials:'hash',
  vod_markers:'bookmark', webhook:'link', crash_counter:'file', stream_alerts:'bell',
};
// Which step a card previews: the first one that says something.
const PREVIEW_ORDER = ['discord','chat','phone','marker','http','file','clip','delay_action','program'];
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
// "The hold runs out" reads "When the hold runs out"; "OBS crashed" stays.
function lowerFirst(s){
  return /^[A-Z][a-z]/.test(s || '') ? s[0].toLowerCase() + s.slice(1) : s;
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
function fmtMs(ms){
  ms = +ms || 0;
  if (!ms) return '';
  if (ms < 60000) return Math.round(ms / 1000) + ' s';
  return Math.round(ms / 60000) + ' min';
}
function plural(n, word){ return `${n} ${word}${n === 1 ? '' : 's'}`; }
function randomToken(){
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  return Array.from(b, x => x.toString(16).padStart(2, '0')).join('');
}
// Stable ids for objects with none (steps, handlers), so a redraw knows a
// moved step is the same step and slides it instead of rebuilding it.
const UIDS = new WeakMap();
let lastUid = 0;
function uidOf(o){
  if (!UIDS.has(o)) UIDS.set(o, ++lastUid);
  return UIDS.get(o);
}

// ---------------------------------------------------------------- rendering

// Redraws never replace innerHTML: morph() patches the live DOM to match
// the new HTML, so focus, carets, scroll and CSS transitions survive. A
// child with data-key is matched by key. Inside a [data-flip] parent,
// keyed children slide to their new place, fade in when new and fade out
// when gone; a [data-autoh] element animates to its new height.
const EASE = 'cubic-bezier(.2,.7,.2,1)';
const reduced = () => typeof prefersReducedMotion === 'function' && prefersReducedMotion();
// Buttons waiting on the app keep their spinner through redraws.
const busyEls = new WeakSet();

function morph(root, html){
  if (!root) return;
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  const motion = !reduced() && root.isConnected && root.getClientRects().length > 0;
  const ctx = {before: motion ? measure(root) : null, entered:[], left:[]};
  const focused = document.activeElement;
  morphChildren(root, tpl.content, ctx);
  // Moving a node drops its focus; give it back.
  if (focused && focused !== document.activeElement && focused.isConnected && root.contains(focused)
    && !focused.closest('[data-leaving]')) focused.focus({preventScroll:true});
  if (motion) animateMorph(ctx);
  placeIndicators(root);
}

function measure(root){
  const rects = new Map(), heights = new Map();
  root.querySelectorAll('[data-flip]').forEach(p => {
    rects.set(p, p.getBoundingClientRect());
    for (const c of p.children){
      if (c.hasAttribute('data-key') && !c.hasAttribute('data-leaving')) rects.set(c, c.getBoundingClientRect());
    }
  });
  root.querySelectorAll('[data-autoh]').forEach(el => heights.set(el, el.getBoundingClientRect().height));
  return {rects, heights};
}

const keyOf = n => n.nodeType === 1 ? n.getAttribute('data-key') : null;
// Leaving nodes are fading out; persistent ones (the unsaved-changes bar)
// were added outside the template. Morph leaves both alone.
const isLeaving = n => n.nodeType === 1 && (n.hasAttribute('data-leaving') || n.hasAttribute('data-persist'));
function nextLive(n){
  while (n && isLeaving(n)) n = n.nextSibling;
  return n;
}

function morphChildren(parent, src, ctx){
  const olds = [...parent.childNodes].filter(n => !isLeaving(n));
  const byKey = new Map();
  olds.forEach(n => { const k = keyOf(n); if (k) byKey.set(k, n); });
  const used = new Set();
  let oi = 0;
  let ref = nextLive(parent.firstChild);
  for (const next of [...src.childNodes]){
    const k = keyOf(next);
    let match = null;
    if (k){
      const c = byKey.get(k);
      if (c && c.nodeName === next.nodeName && !used.has(c)) match = c;
    } else {
      while (oi < olds.length && (used.has(olds[oi]) || keyOf(olds[oi]))) oi++;
      const c = olds[oi];
      if (c && c.nodeName === next.nodeName){ match = c; oi++; }
    }
    let node = next;
    if (match){ used.add(match); patch(match, next, ctx); node = match; }
    else if (k) ctx.entered.push(next);
    if (node === ref) ref = nextLive(ref.nextSibling);
    else parent.insertBefore(node, ref);
  }
  olds.forEach(n => { if (!used.has(n)) removeNode(n, parent, ctx); });
}

function patch(a, b, ctx){
  if (a.nodeType !== 1){
    if (a.nodeValue !== b.nodeValue) a.nodeValue = b.nodeValue;
    return;
  }
  const tag = a.tagName;
  // Form fields: only push a value the model changed, so whatever the user
  // typed into a field the model doesn't track yet survives a redraw.
  const oldValue = a.getAttribute('value'), oldChecked = a.hasAttribute('checked');
  const oldSelected = tag === 'SELECT' ? selectedOf(a) : null;
  syncAttrs(a, b);
  if (busyEls.has(a)){ a.classList.add('is-busy'); a.disabled = true; a.setAttribute('aria-busy', 'true'); }
  if (tag === 'INPUT'){
    if (a.type === 'checkbox' || a.type === 'radio'){
      if (b.hasAttribute('checked') !== oldChecked) a.checked = b.hasAttribute('checked');
    } else {
      const v = b.getAttribute('value') || '';
      if (v !== (oldValue || '') && a.value !== v && a !== document.activeElement) a.value = v;
    }
    return;
  }
  if (tag === 'TEXTAREA'){
    const v = b.textContent;
    if (v !== a.defaultValue){
      a.defaultValue = v;
      if (a.value !== v && a !== document.activeElement) a.value = v;
    }
    return;
  }
  morphChildren(a, b, ctx);
  if (tag === 'SELECT'){
    const now = selectedOf(a);
    if (now !== oldSelected && a.value !== now) a.value = now;
  }
}

function selectedOf(select){
  const o = select.querySelector('option[selected]') || select.querySelector('option');
  return o ? o.value : '';
}

function syncAttrs(a, b){
  // A <details> belongs to the user once drawn; textareas keep the height
  // autosize gave them.
  const keep = name => (name === 'open' && a.tagName === 'DETAILS')
    || (name === 'style' && a.hasAttribute('data-keep-style'));
  for (const {name} of [...a.attributes]){
    if (!b.hasAttribute(name) && !keep(name)) a.removeAttribute(name);
  }
  for (const {name, value} of b.attributes){
    if (a.getAttribute(name) !== value && !keep(name)) a.setAttribute(name, value);
  }
}

function removeNode(n, parent, ctx){
  const r = ctx.before && ctx.before.rects.get(n);
  const pr = ctx.before && ctx.before.rects.get(parent);
  if (!r || !pr || !r.width){ n.remove(); return; }
  // Out of the flow at once, so the rest slides into place while it fades.
  n.setAttribute('data-leaving', '');
  n.inert = true;
  Object.assign(n.style, {
    position:'absolute', margin:'0', pointerEvents:'none',
    left:(r.left - pr.left - parent.clientLeft + parent.scrollLeft) + 'px',
    top:(r.top - pr.top - parent.clientTop + parent.scrollTop) + 'px',
    width:r.width + 'px', height:r.height + 'px',
  });
  ctx.left.push(n);
}

function animateMorph(ctx){
  const {rects, heights} = ctx.before;
  // Slide what moved, measured against its parent so nested lists don't
  // move twice.
  rects.forEach((old, el) => {
    if (!el.hasAttribute('data-key') || !el.isConnected || el.hasAttribute('data-leaving') || !old.width) return;
    const parent = el.parentElement, pOld = rects.get(parent);
    if (!pOld) return;
    // A slide still running restarts from where it is; entrance animations
    // are the same in both measurements, so they are left alone.
    el.getAnimations().forEach(x => { if (x.id === 'flip') x.cancel(); });
    const now = el.getBoundingClientRect(), pNow = parent.getBoundingClientRect();
    const dx = (old.left - pOld.left) - (now.left - pNow.left);
    const dy = (old.top - pOld.top) - (now.top - pNow.top);
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
    el.animate([{transform:`translate(${dx}px,${dy}px)`}, {transform:'none'}], {id:'flip', duration:300, easing:EASE});
  });
  let n = 0;
  ctx.entered.forEach(el => {
    const parent = el.parentElement;
    if (!parent || !rects.has(parent) || !parent.hasAttribute('data-flip')) return;
    el.animate([
      {opacity:0, transform:'translateY(6px) scale(.98)', filter:'blur(3px)'},
      {opacity:1, transform:'none', filter:'blur(0)'},
    ], {duration:280, easing:EASE, delay:Math.min(n++, 8) * 28, fill:'backwards'});
  });
  ctx.left.forEach(el => {
    const a = el.animate([{opacity:1, transform:'none'}, {opacity:0, transform:'translateY(-4px) scale(.98)'}],
      {duration:170, easing:EASE, fill:'forwards'});
    a.onfinish = () => el.remove();
    a.oncancel = () => el.remove();
  });
  heights.forEach((h, el) => {
    if (!el.isConnected) return;
    const now = el.getBoundingClientRect().height;
    if (Math.abs(now - h) < 1) return;
    el.animate([{height:h + 'px'}, {height:now + 'px'}], {duration:260, easing:EASE});
  });
}

// Cards ease in one after another when the tab opens. Animations, not a
// class, so the redraw that lands mid-way can't cut them short.
function enterStagger(root){
  if (!root || reduced()) return;
  [...root.querySelectorAll('.ig-grid > [data-key], .ig-list > [data-key], .ig-pack')].slice(0, 24).forEach((el, n) => {
    el.animate([{opacity:0, transform:'translateY(8px)', filter:'blur(3px)'}, {opacity:1, transform:'none', filter:'blur(0)'}],
      {duration:340, easing:EASE, delay:Math.min(n, 12) * 30, fill:'backwards'});
  });
}

// Segmented controls carry the dashboard's sliding pill; it is placed
// under the active tab after every redraw, and glides when that changes.
const SEG_IND = '<span class="sub-tab-ind" data-keep-style aria-hidden="true"></span>';
function placeIndicators(root){
  root.querySelectorAll('.ig-seg').forEach(seg => {
    const ind = seg.querySelector(':scope > .sub-tab-ind');
    const on = seg.querySelector(':scope > .sub-tab.on');
    if (!ind) return;
    if (!on || !on.offsetWidth){ ind.style.opacity = '0'; return; }
    ind.style.left = on.offsetLeft + 'px';
    ind.style.width = on.offsetWidth + 'px';
    ind.style.opacity = '1';
  });
}

// Spinner on a button while `fn` runs; a second click is ignored.
async function busy(btn, fn){
  if (!btn || busyEls.has(btn)) return undefined;
  busyEls.add(btn);
  btn.classList.add('is-busy');
  btn.disabled = true;
  btn.setAttribute('aria-busy', 'true');
  try {
    return await fn();
  } finally {
    busyEls.delete(btn);
    btn.classList.remove('is-busy');
    btn.removeAttribute('aria-busy');
    btn.disabled = false;
  }
}

// Destructive buttons ask on the button itself: the first click arms it,
// a second within three seconds confirms. True when confirmed.
function armConfirm(btn, label){
  if (btn.hasAttribute('data-armed')) return true;
  const before = btn.innerHTML;
  btn.setAttribute('data-armed', '');
  btn.classList.add('ig-armed');
  btn.innerHTML = esc(label);
  setTimeout(() => {
    if (!btn.isConnected || !btn.hasAttribute('data-armed')) return;
    btn.removeAttribute('data-armed');
    btn.classList.remove('ig-armed');
    btn.innerHTML = before;
  }, 3000);
  return false;
}

// The live element for a selector inside the open modal, never one that is
// fading out.
function q(sel){
  if (!Modal.el) return null;
  return [...Modal.el.querySelectorAll(sel)].find(el => !el.closest('[data-leaving]')) || null;
}

function copyText(btn){
  const text = btn.dataset.text || '';
  navigator.clipboard.writeText(text).then(
    () => { if (typeof flashCopied === 'function' && btn.classList.contains('ic-btn')) flashCopied(btn); else toast('Copied', 'ok'); },
    () => toast('Could not copy: select it and copy it by hand', 'err'));
}

// ---------------------------------------------------------------- previews

// HTML for what a step sends, styled like where it lands.
function previewHtml(step, handler, big){
  const cls = 'ig-pv' + (big ? ' big' : '');
  if (!step){
    const n = handler ? (handler.steps || []).length : 0;
    return `<div class="${cls} ig-pv-action"><span>${svg('steps')}</span>${plural(n, 'step')}</div>`;
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
      return `<div class="${cls} ig-pv-discord"><span class="ig-av">${svg('shield')}</span><span class="ig-pv-col">
        <b>InstantClone</b><span class="ig-app">APP</span><div class="ig-txt">${esc(ping + text)}</div></span></div>`;
    }
    case 'chat':
      return `<div class="${cls} ig-pv-chat">${ask ? `<div><span class="u1">viewer</span>: ${esc(ask)}</div>` : ''}
        <div><span class="u2">${esc(chatAuthor(step))}</span>: ${esc(text)}</div></div>`;
    case 'phone':
      return `<div class="${cls} ig-pv-phone"><span class="ig-av"></span><span class="ig-pv-col">
        <small>INSTANTCLONE · now</small><b>${esc(renderSample(step.params.title, vars) || 'InstantClone')}</b><div>${esc(text)}</div></span></div>`;
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
function chatAuthor(step){
  const t = S.data.twitch;
  return (step.params.as === 'bot' && t.bot.login) || t.main.login || 'you';
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
    const why = (d && d.error) || 'Integrations are not available right now.';
    // A failed refresh keeps what is on screen; only a first load says so.
    if (S.data) toast(why, 'err', 5000);
    else morph($('ig-root'), `<div class="lan-warn"><span class="lw-ic">⚠</span><span>${esc(why)}</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="retry">Try again</button></div>`);
    return false;
  }
  S.data = d;
  render();
  return true;
}

async function poll(){
  if (document.hidden || !S.data || (!S.visible && !modalOpen())) return;
  const a = await api('/integrations/activity');
  if (!S.data || !a || !a.activity) return;
  const fastBefore = !!S.data.twitch.login;
  const twitchChanged = JSON.stringify(a.twitch) !== JSON.stringify(S.data.twitch);
  S.data.activity = a.activity;
  S.data.stats = a.stats;
  S.data.twitch = a.twitch;
  if (fastBefore !== !!a.twitch.login) schedulePoll();
  if (S.visible) render();
  // Only the Twitch page shows live state.
  if (Modal.kind === 'connections' && Conn.tab === 'twitch' && twitchChanged) Conn.render();
}

function schedulePoll(){
  clearInterval(S.pollTimer);
  const fast = S.data && S.data.twitch && S.data.twitch.login;
  S.pollTimer = setInterval(() => { poll(); }, fast ? 2000 : 3500);
}

async function refreshData(){
  const d = await api('/integrations');
  if (d && d.integrations){ S.data = d; if (S.visible) render(); }
}

// ---------------------------------------------------------------- pane

function stat(i){ return (S.data.stats || {})[i.id] || null; }
function issuesOf(i){ return ((S.data && S.data.issues) || {})[i.id] || []; }
function find(id){ return S.data.integrations.find(i => i.id === id); }

// On, but nothing to talk to: it uses chat, markers or clips while no
// Twitch account is connected.
function needsTwitch(i){
  if (S.data.twitch.main.login) return false;
  return (i.handlers || []).some(h => h.trigger && h.trigger.type.startsWith('chat'))
    || allSteps(i).some(s => s.type === 'chat' || s.type === 'marker' || s.type === 'clip');
}

function cardInfo(i){
  const h = (i.handlers || []).find(x => x.enabled) || (i.handlers || [])[0];
  const step = primaryStep(h);
  const k = step ? KINDS[step.type] : null;
  const st = stat(i);
  let where = 'Your own · ' + plural(allSteps(i).length, 'step');
  if (step && step.type === 'discord') where = channelName(step.params.connection);
  else if (step && step.type === 'chat') where = 'Twitch chat';
  else if (step && k) where = k.label;
  const failing = !!(i.enabled && st && st.failing > 0);
  const issues = issuesOf(i);
  let flag = null;
  if (failing) flag = {cls:'bad', label:'Failing', tip:'The last run failed. Open it to see why.'};
  else if (issues.length) flag = {cls:'warn', label:i.enabled ? 'Needs a fix' : 'Finish setup', tip:issues.join('; ')};
  else if (i.enabled && needsTwitch(i)) flag = {cls:'warn', label:'Needs Twitch', tip:'Connect Twitch in Connections'};
  const text = step && k && k.text ? step.params[k.text] : '';
  return {
    h, step, flag, failing, where,
    dc: k ? k.c : 'var(--accent)',
    icon: PRESET_ICON[i.preset] || (k ? k.icon : 'steps'),
    last: st && st.last_ms ? fmtAgo(st.last_ms) : 'never ran',
    summary: text ? renderSample(text, sampleMap(h)) : (h ? triggerSentence(h.trigger) : ''),
  };
}

function visibleIntegrations(){
  const q = S.search.trim().toLowerCase();
  return S.data.integrations.filter(i =>
    (S.filter === 'all' || categoryOf(i) === S.filter)
    && (!q || i.name.toLowerCase().includes(q) || JSON.stringify(i.handlers).toLowerCase().includes(q)));
}

function skeletonHtml(){
  return `<div class="ig-pane"><div class="tab-head"><div><div class="tab-title">Integrations</div>
    <div class="tab-sub">Loading…</div></div></div>
    <div class="ig-grid" aria-hidden="true">${'<div class="ig-skel"><i></i><b></b><u></u></div>'.repeat(6)}</div></div>`;
}

function render(){
  const root = $('ig-root');
  if (!root || !S.data) return;
  const d = S.data;
  const on = d.integrations.filter(i => i.enabled).length;
  const failing = d.integrations.filter(i => cardInfo(i).failing);
  const empty = d.integrations.length === 0;
  const sub = empty ? 'Alerts, chat commands and automations. Each card shows exactly what it sends.'
    : `<span class="ig-num">${on}</span> on${failing.length ? ` · <span class="ig-num">${failing.length}</span> need${failing.length === 1 ? 's' : ''} attention` : ''}. Each card shows exactly what it sends.`;
  morph(root, `<div class="ig-pane" data-flip>
      <div class="tab-head" data-key="head">
        <div><div class="tab-title">Integrations</div><div class="tab-sub ig-sub">${sub}</div></div>
        <div class="ig-head-actions">
          ${empty ? '' : '<button class="ic-btn ic-btn-ghost" data-act="share">Share</button>'}
          <button class="ic-btn ic-btn-ghost" data-act="import">Import recipe</button>
          <button class="ic-btn dest-add-btn" data-act="catalog">${svg('plus')}<span>Add</span></button>
        </div>
      </div>
      <div class="ig-notices" data-key="notices" data-flip>${noticesHtml(failing)}</div>
      ${connectionsHtml()}
      ${empty ? emptyHtml() : toolbarHtml() + `<div class="ig-items" data-key="items-${S.view}">${itemsHtml()}</div>`}
      ${activityHtml()}
    </div>`);
  if (typeof fitPaneCard === 'function') fitPaneCard();
}

function noticesHtml(failing){
  const d = S.data;
  const out = [];
  if (d.twitch.notice){
    out.push(`<div class="lan-warn ig-notice" data-key="n-twitch"><span class="lw-ic">⚠</span>
      <span class="ig-notice-text">${esc(d.twitch.notice)}</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="conn" data-tab="twitch">Reconnect</button></div>`);
  }
  failing.slice(0, 2).forEach(i => {
    const rec = (d.activity || []).find(r => r.integration_id === i.id && r.status === 'failed');
    const step = rec && rec.steps.find(s => s.status === 'failed');
    out.push(`<div class="lan-warn ig-notice" data-key="n-${esc(i.id)}"><span class="lw-ic">⚠</span>
      <span class="ig-notice-text"><b>${esc(i.name)}</b> ${step ? 'failed: ' + esc(step.detail) : 'is failing'}. Nothing else is affected.</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="edit" data-id="${esc(i.id)}">Fix</button></div>`);
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
  return `<div class="ig-conns" data-key="conns"><span class="ic-label">Connections</span>
    ${pill('discord', discord ? 'ok' : '', 'Discord', discord ? plural(discord, 'channel') : 'Add')}
    ${pill('twitch', twitchOn ? (twitchLive ? 'ok' : 'warn') : '', 'Twitch', twitchOn ? (twitchLive ? t.main.login : t.main.login + ' · ' + t.main.chat) : (t.available ? 'Connect' : 'Unavailable'))}
    ${pill('phone', phoneOn ? 'ok' : '', 'Phone', phoneOn ? c.phone.topic : 'Connect')}
  </div>`;
}

function toolbarHtml(){
  const counts = {all:S.data.integrations.length};
  S.data.integrations.forEach(i => { const c = categoryOf(i); counts[c] = (counts[c] || 0) + 1; });
  const tab = (id, label) => `<button class="sub-tab${S.filter === id ? ' on' : ''}" role="tab" aria-selected="${S.filter === id}"
    data-act="filter" data-f="${id}">${label} <span class="ig-count">${counts[id] || 0}</span></button>`;
  const view = (id, icon, label) => `<button class="sub-tab${S.view === id ? ' on' : ''}" data-act="view" data-v="${id}" aria-label="${label}" aria-pressed="${S.view === id}">
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="${icon}"/></svg></button>`;
  return `<div class="ig-toolbar" data-key="toolbar">
    <div class="sub-tabs ig-seg" role="tablist">
      ${tab('all', 'All')}${tab('alerts', 'Alerts')}${tab('chat', 'Chat')}${tab('auto', 'Automation')}${tab('own', 'Your own')}${SEG_IND}
    </div>
    <input class="ic-input ig-search" id="ig-search" type="search" placeholder="Search" aria-label="Search integrations" value="${esc(S.search)}" autocomplete="off">
    <div class="sub-tabs ig-seg ig-views">
      ${view('grid', 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z', 'Grid')}${view('list', 'M4 6h16M4 12h16M4 18h16', 'List')}${SEG_IND}
    </div>
  </div>`;
}

function itemsHtml(){
  const items = visibleIntegrations();
  const none = items.length ? '' : `<div class="ig-none" data-key="none">Nothing here${S.search ? ' matches "' + esc(S.search) + '"' : ' yet'}.</div>`;
  return S.view === 'grid' ? gridHtml(items, none) : listHtml(items, none);
}

function switchHtml(i){
  return `<button class="dcard-switch${i.enabled ? ' on' : ''}" role="switch" aria-checked="${i.enabled}"
    aria-label="${esc(i.name)}" data-act="toggle" data-id="${esc(i.id)}"><span></span></button>`;
}
function flagHtml(flag){
  if (!flag) return '';
  return `<span class="dcard-status ig-flag ${flag.cls}" title="${esc(flag.tip)}"><span class="d-dot"></span><span class="dcard-status-l">${flag.label}</span></span>`;
}

function gridHtml(items, none){
  return `<div class="ig-grid" data-flip>${none}${items.map(i => {
    const c = cardInfo(i);
    return `<div class="dcard ig-card ${i.enabled ? 'alive' : 'off'}" style="--dc:${c.dc}" data-key="c-${esc(i.id)}">
      <div class="dcard-screen" data-act="edit" data-id="${esc(i.id)}" role="button" tabindex="0" aria-label="Edit ${esc(i.name)}">
        ${previewHtml(c.step, c.h, false)}${flagHtml(c.flag)}
      </div>
      <div class="ig-card-foot">
        <span class="dcard-icon">${svg(c.icon)}</span>
        <div class="dcard-id"><div class="dcard-name">${esc(i.name)}</div>
          <div class="dcard-host">${esc(c.where)} · ${esc(c.last)}</div></div>
        ${switchHtml(i)}
      </div>
    </div>`;
  }).join('')}
  <button class="dest-add ig-add-tile" data-act="catalog" data-key="add"><span class="dest-add-icon">${svg('plus')}</span>Add or build your own</button></div>`;
}

function listHtml(items, none){
  return `<div class="ig-list" data-flip>${none}${items.map(i => {
    const c = cardInfo(i);
    const open = S.peek === i.id;
    return `<div class="dcard ig-row ${i.enabled ? 'alive' : 'off'}${open ? ' open' : ''}" style="--dc:${c.dc}" data-key="r-${esc(i.id)}" data-autoh>
      <div class="ig-row-main">
        <span class="dcard-icon">${svg(c.icon)}</span>
        <button class="ig-row-name" data-act="peek" data-id="${esc(i.id)}" aria-expanded="${open}">
          <span class="dcard-name">${esc(i.name)}</span><span class="dcard-host">${esc(c.where)}</span></button>
        <span class="ig-row-sum" title="${esc(c.summary)}">${esc(c.summary)}</span>
        ${c.flag ? flagHtml(c.flag) : `<span class="ig-row-last">${esc(c.last)}</span>`}
        ${switchHtml(i)}
        <button class="ic-btn-tiny" data-act="edit" data-id="${esc(i.id)}" aria-label="Edit ${esc(i.name)}">${svg('pencil')}</button>
      </div>
      ${open ? `<div class="ig-row-peek"><div class="dcard-screen">${previewHtml(c.step, c.h, true)}</div>
        <div class="ig-row-peek-actions">
          <button class="ic-btn ic-btn-primary" data-act="edit" data-id="${esc(i.id)}">Edit</button>
          <button class="ic-btn ic-btn-ghost" data-act="duplicate" data-id="${esc(i.id)}">Duplicate</button>
        </div></div>` : ''}
    </div>`;
  }).join('')}
  <button class="dest-add ig-add-row" data-act="catalog" data-key="add"><span class="dest-add-icon">${svg('plus')}</span>Add an integration, or build your own</button></div>`;
}

function emptyHtml(){
  const packs = S.data.catalog.packs;
  const preset = id => S.data.catalog.presets.find(p => p.id === id);
  return `<div class="dest-empty" data-key="empty">
    <div class="dest-empty-title">What should InstantClone do for you?</div>
    <div class="dest-empty-sub">Pick a pack to switch on a few integrations at once. Each shows what it sends, and you can change or remove anything later.</div>
    <div class="dest-empty-grid ig-packs">
      ${packs.map((p, n) => `<button class="dest-starter ig-pack${n === 0 ? ' ig-pack-rec' : ''}" data-act="pack" data-pack="${esc(p.id)}">
        <span class="dest-starter-name">${esc(p.name)}${n === 0 ? ' <span class="dcard-status s-ready ig-rec">Recommended</span>' : ''}</span>
        <span class="dest-starter-note">${esc(p.description)}</span>
        <span class="ig-pack-list">${p.presets.map(id => `<span>${svg(PRESET_ICON[id] || 'steps')}${esc((preset(id) || {}).name || id)}</span>`).join('')}</span>
      </button>`).join('')}
    </div>
    <div class="muted ig-empty-more">or <a href="#" data-act="catalog">browse everything</a> · <a href="#" data-act="build">build your own</a> · <a href="#" data-act="import">import a recipe</a></div>
  </div>`;
}

function activityHtml(){
  const rows = (S.data.activity || []).slice(0, 40);
  const seen = new Map();
  const body = rows.length ? rows.map(r => {
    let key = `${r.at_ms}-${r.integration_id}`;
    const dup = seen.get(key) || 0;
    seen.set(key, dup + 1);
    if (dup) key += '-' + dup;
    const failed = r.steps.find(s => s.status === 'failed');
    const detail = failed ? failed.label + ': ' + failed.detail
      : r.status === 'busy' ? 'skipped: already running'
      : r.steps.filter(s => s.status === 'ok').map(s => s.label).join(' · ') || r.trigger;
    return `<div class="ig-act-row" data-key="a-${esc(key)}"><span class="t">${esc(fmtClock(r.at_ms))}</span>
      <span class="n">${esc(r.name)}${r.test ? ' <span class="muted">(test)</span>' : ''}</span>
      <span class="d" title="${esc(detail)}">${esc(r.trigger)} → ${esc(detail)}</span><span class="s ${esc(r.status)}">${esc(r.status)}</span></div>`;
  }).join('') : '<div class="ig-act-empty" data-key="a-none">Runs show up here, with every step and what it answered.</div>';
  return `<details class="sys-section ig-activity-box" id="ig-activity-box" data-key="activity" ${S.activityOpen ? 'open' : ''}>
    <summary><span class="ig-act-title">Recent activity</span><span class="muted">${rows.length ? 'last run ' + esc(fmtAgo(rows[0].at_ms)) : 'nothing has run yet'}</span>${svg('chev')}</summary>
    <div class="ig-activity" data-flip>${body}</div>
  </details>`;
}

// ---------------------------------------------------------------- actions

async function toggle(id){
  const i = find(id);
  if (!i) return;
  const want = !i.enabled;
  const issues = issuesOf(i);
  if (want && issues.length){
    toast(`${i.name}: ${issues[0]} first`, 'info', 4500);
    openEditor(i, true);
    return;
  }
  // Flip at once; put it back if the app says no.
  i.enabled = want;
  render();
  const r = await api('/integrations/toggle', {id, enabled:want});
  if (r.ok) return;
  i.enabled = !want;
  render();
  toast(r.error || 'Could not switch it', 'err', 5000);
  if (want) openEditor(i, true);
}

async function addFrom(body, openAfter){
  const r = await api('/integrations/add', body);
  if (!r.ok){ toast(r.error || 'Could not add it', 'err', 5000); return; }
  await load();
  const added = (r.ids || []).map(find).filter(Boolean);
  const unfinished = added.find(i => !i.enabled);
  if (openAfter && unfinished){
    openEditor(unfinished, true);
    return;
  }
  toast(added.length > 1 ? `Added ${added.length} integrations` : `Added ${added[0] ? added[0].name : ''}`, 'ok');
  if (unfinished) toast(`${unfinished.name} needs one more detail before it can switch on`, 'info', 4500);
  if (Modal.kind === 'catalog' && body.preset) Catalog.flashAdded(body.preset);
}

async function duplicate(id){
  const r = await api('/integrations/duplicate', {id});
  if (!r.ok){ toast(r.error || 'Could not duplicate it', 'err'); return; }
  await load();
  toast('Duplicated', 'ok');
}

async function remove(id){
  const kind = Modal.kind;
  const r = await api('/integrations/delete', {id});
  if (!r.ok){ toast(r.error || 'Could not delete it', 'err'); return; }
  // Only close the editor that asked; the user may have moved on.
  if (Modal.kind === kind) Modal.close(true);
  await load();
  toast('Deleted', 'ok');
}

// `switchOn`: it should end up on, and waits for a detail first.
function openEditor(i, switchOn){
  if (i.preset) Editor.open(i, switchOn);
  else Builder.open(i, {switchOn});
}

// ---------------------------------------------------------------- modal shell

// One modal shell at a time, built on the destination editor's. Opening
// another while one is up swaps the content in place (no second backdrop).
// Escape, the backdrop and ✕ close it; `onClose` lets an editor hold the
// close to ask about unsaved changes, `after` runs once it really closes.
const Modal = {
  el:null, form:null, kind:null, onClose:null, after:null, opener:null, swapFrom:null,
  open(kind, width, height){
    if (this.el){
      this.swapFrom = this.form.getBoundingClientRect();
      this.hideGuard();
    } else {
      this.opener = document.activeElement;
      const wrap = document.createElement('div');
      wrap.className = 'ic-modal ig-modal';
      wrap.setAttribute('role', 'dialog');
      wrap.setAttribute('aria-modal', 'true');
      wrap.innerHTML = '<div class="ic-modal-backdrop" data-act="modal-close"></div><div class="dest-form" tabindex="-1"></div>';
      document.body.appendChild(wrap);
      wrap.addEventListener('click', onClick);
      wrap.addEventListener('input', onInput);
      wrap.addEventListener('keydown', onActivateKey);
      wrap.addEventListener('focusout', onFieldLeave);
      document.addEventListener('keydown', onModalKey);
      this.el = wrap;
      this.form = wrap.querySelector('.dest-form');
      this.form.focus({preventScroll:true});
    }
    this.kind = kind;
    this.onClose = null;
    this.after = null;
    this.form.style.width = `min(${width || 820}px,100%)`;
    this.form.style.height = height || '';
    return this.el;
  },
  body(html){
    if (!this.form) return;
    if (!this.swapFrom){ morph(this.form, html); return; }
    // A different modal in the same shell: resize to it and fade it in.
    const from = this.swapFrom;
    this.swapFrom = null;
    this.form.innerHTML = html;
    this.form.focus({preventScroll:true});
    placeIndicators(this.form);
    if (reduced()) return;
    const to = this.form.getBoundingClientRect();
    this.form.animate([{width:from.width + 'px', height:from.height + 'px'}, {width:to.width + 'px', height:to.height + 'px'}],
      {duration:280, easing:EASE});
    for (const c of this.form.children){
      c.animate([{opacity:0, transform:'translateY(4px)'}, {opacity:1, transform:'none'}],
        {duration:240, easing:EASE, delay:60, fill:'backwards'});
    }
  },
  close(force){
    if (!this.el) return;
    if (!force && this.onClose && this.onClose() === false) return;
    const after = this.after;
    this.after = null;
    this.onClose = null;
    this.kind = null;
    if (after){
      after();
      if (this.kind) return;   // it reopened something in this shell
    }
    const el = this.el, opener = this.opener;
    this.el = this.form = this.opener = this.swapFrom = null;
    document.removeEventListener('keydown', onModalKey);
    el.classList.add('closing');
    el.inert = true;
    setTimeout(() => el.remove(), reduced() ? 0 : 180);
    if (opener && opener.isConnected && typeof opener.focus === 'function') opener.focus({preventScroll:true});
    if (!S.visible) clearInterval(S.pollTimer);
  },
  // The "unsaved changes" bar, over the footer.
  guard(){
    const shown = this.form.querySelector('.ig-guard');
    if (shown){ shown.querySelector('[data-act="guard-keep"]').focus(); return; }
    const bar = document.createElement('div');
    bar.className = 'ig-guard';
    bar.setAttribute('data-persist', '');
    bar.setAttribute('role', 'alert');
    bar.innerHTML = `<span class="ig-guard-text">${svg('warn')}You have unsaved changes</span>
      <button class="ic-btn ic-btn-ghost" data-act="guard-discard">Discard</button>
      <button class="ic-btn ic-btn-ghost" data-act="guard-keep">Keep editing</button>
      <button class="ic-btn ic-btn-primary" data-act="guard-save">Save</button>`;
    this.form.appendChild(bar);
    bar.querySelector('[data-act="guard-keep"]').focus();
  },
  hideGuard(){
    const g = this.form && this.form.querySelector('.ig-guard');
    if (g) g.remove();
  },
};
function modalOpen(){ return !!Modal.el; }

function onModalKey(e){
  if (!Modal.el) return;
  if (e.key === 'Escape' && !e.defaultPrevented){
    e.preventDefault();
    if (Modal.form.querySelector('.ig-guard')) Modal.hideGuard(); else Modal.close();
    return;
  }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's' && (Modal.kind === 'editor' || Modal.kind === 'builder')){
    e.preventDefault();
    const save = q('.dest-form-foot [data-act="save"]');
    if (save && !save.disabled) save.click();
    return;
  }
  if (e.key === 'Tab') trapFocus(e);
}

// Tab cycles inside the modal instead of wandering into the page behind.
function trapFocus(e){
  const list = [...Modal.form.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')]
    .filter(x => !x.disabled && x.type !== 'hidden' && x.getClientRects().length && !x.closest('[data-leaving]'));
  if (!list.length) return;
  const first = list[0], last = list[list.length - 1], at = document.activeElement;
  if (e.shiftKey && (at === first || !Modal.form.contains(at))){ e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && (at === last || !Modal.form.contains(at))){ e.preventDefault(); first.focus(); }
}

function modalHead(icon, color, title, hint, extra){
  return `<div class="dest-form-head">
    <span class="dest-form-mark" style="--dc:${color}">${svg(icon)}</span>
    <div class="dest-form-titles"><h4>${title}</h4><div class="dest-form-hint">${hint}</div></div>
    ${extra || ''}
    <button type="button" class="dest-form-x" data-act="modal-close" aria-label="Close">${svg('x')}</button>
  </div>`;
}
function titleInput(value){
  return `<input class="ig-title-input" data-bind="name" value="${esc(value)}" aria-label="Name" maxlength="80" spellcheck="false" autocomplete="off">`;
}
function enableSwitch(bind, on, label){
  return `<label class="dest-form-enable"><input type="checkbox" data-bind="${bind}" ${on ? 'checked' : ''}><span class="dfe-track"><span></span></span><span>${label || 'Enabled'}</span></label>`;
}
function lastRunHtml(d){
  const st = stat(d);
  if (!st || !st.last_ms) return '<span class="ig-last">Hasn\'t run yet</span>';
  return `<span class="ig-last ${st.last_status === 'failed' ? 'bad' : 'ok'}"><i></i>Last ran ${esc(fmtAgo(st.last_ms))} · ${esc(st.last_status)}</span>`;
}
// The editors' footer: Delete away from Save, Save lit only with changes.
function footHtml(d, dirty, extra){
  return `<div class="dest-form-foot">
    ${d.id ? '<button class="ic-btn ic-btn-ghost ig-del" data-act="delete">Delete</button>' : ''}
    <div class="dest-form-msg muted ig-foot-msg">${d.id ? lastRunHtml(d) : '<span class="ig-last">New integration</span>'}${dirty ? '<span class="ig-unsaved">Unsaved</span>' : ''}</div>
    ${extra || ''}
    <button class="ic-btn ic-btn-primary" data-act="save" ${dirty || !d.id ? '' : 'disabled'} title="Save (Ctrl+S)">Save</button>
  </div>`;
}
function runLogHtml(steps){
  if (!steps || !steps.length) return '<div class="muted">No steps ran.</div>';
  return `<div class="ig-run">${steps.map(s => `<div><span class="t">${(s.at_ms / 1000).toFixed(2)} s</span>
    <span class="${esc(s.status)}">${s.status === 'ok' ? '✓' : s.status === 'failed' ? '✕' : '–'} ${esc(s.label)}</span>
    <span class="d" title="${esc(s.detail)}">${esc(s.detail)}</span></div>`).join('')}</div>`;
}
function warnHtml(text){
  return `<div class="lan-warn ig-inline-warn"><span class="lw-ic">⚠</span><span>${text}</span></div>`;
}

async function saveIntegration(d, onSaved){
  const kind = Modal.kind;
  const r = await api('/integrations/save', d);
  if (!r.ok){ toast(r.error || 'Could not save', 'err', 6000); return false; }
  if (onSaved) onSaved();
  if (r.missing && r.missing.length){
    toast(`Saved as a draft. Still to do: ${r.missing.join('; ')}`, 'info', 7000);
  } else if (r.warnings && r.warnings.length){
    toast(`Saved. Check these names, nothing fills them: ${r.warnings.map(w => '{' + w + '}').join(', ')}`, 'info', 7000);
  } else toast('Saved', 'ok');
  if (Modal.kind === kind) Modal.close(true);
  await load();
  return true;
}

// ---------------------------------------------------------------- catalog

const Catalog = {
  cat:'rec', added:null,
  open(){
    this.cat = 'rec';
    this.added = null;
    Modal.open('catalog', 1040, 'min(86vh,820px)');
    this.render();
  },
  render(){
    const cat = S.data.catalog;
    const counts = id => id === 'packs' ? cat.packs.length
      : cat.presets.filter(p => id === 'rec' ? p.recommended : p.category === id).length;
    const nav = [['rec', 'Recommended'], ['alerts', 'Alerts'], ['chat', 'Chat'], ['auto', 'Automation'], ['packs', 'Packs']]
      .map(([id, label]) => `<button class="${this.cat === id ? 'on' : ''}" data-act="cat" data-cat="${id}" aria-current="${this.cat === id}">${label}<small>${counts(id)}</small></button>`).join('');
    const tiles = this.cat === 'packs' ? this.packsHtml() : this.presetsHtml();
    Modal.body(`${modalHead('plus', 'var(--accent)', 'Add an integration', 'Every card shows exactly what it sends. Add it, then tweak anything.')}
      <div class="ig-cat"><nav class="ig-cat-nav" aria-label="Categories">${nav}
        <div class="ig-cat-import">Got a recipe from a friend? <a href="#" data-act="import">Import it</a></div></nav>
      <div class="ig-cat-grid">${tiles}</div></div>`);
  },
  packsHtml(){
    const cat = S.data.catalog;
    return cat.packs.map((p, n) => `<div class="dcard ig-cat-pack" data-key="pk-${esc(p.id)}" style="--i:${n}">
      <div class="dcard-name">${esc(p.name)}</div>
      <div class="muted ig-cat-desc">${esc(p.description)}</div>
      <div class="ig-pack-list">${p.presets.map(id => `<span>${svg(PRESET_ICON[id] || 'steps')}${esc((cat.presets.find(x => x.id === id) || {}).name || id)}</span>`).join('')}</div>
      <button class="ic-btn ic-btn-primary ig-small" data-act="pack" data-pack="${esc(p.id)}">Add pack</button>
    </div>`).join('');
  },
  presetsHtml(){
    const list = S.data.catalog.presets.filter(p => this.cat === 'rec' ? p.recommended : p.category === this.cat);
    const have = id => S.data.integrations.filter(i => i.preset === id).length;
    return `<button class="dest-add ig-cat-build" data-act="build" data-key="build" style="--i:0">
        <span class="dest-add-icon">${svg('plus')}</span>
        <span class="ig-cat-build-t">Build your own</span>
        <span class="ig-cat-build-d">Any trigger, any steps: chat, Discord, web requests, programs, the delay itself.</span>
      </button>` + list.map((p, n) => {
      const fake = fakeStep(p);
      const needs = needsNote(p.needs);
      const added = this.added === p.id;
      const button = added
        ? `<button class="ic-btn ic-btn-ghost ig-small ig-added" data-act="add-preset" data-preset="${esc(p.id)}">${svg('check')}Added</button>`
        : `<button class="ic-btn ${have(p.id) ? 'ic-btn-ghost' : 'ic-btn-primary'} ig-small" data-act="add-preset" data-preset="${esc(p.id)}">${have(p.id) ? 'Add another' : 'Add'}</button>`;
      return `<div class="dcard ig-cat-tile" style="--dc:${(KINDS[fake.step.type] || {}).c || 'var(--accent)'};--i:${n + 1}" data-key="t-${esc(p.id)}">
        <div class="dcard-screen">${previewHtml(fake.step, fake.handler, false)}</div>
        <div class="ig-cat-id"><span class="dcard-icon">${svg(PRESET_ICON[p.id] || 'steps')}</span>
          <div class="dcard-id"><div class="dcard-name">${esc(p.name)}</div><div class="ig-cat-desc">${esc(p.description)}</div>
          ${needs ? `<div class="ig-cat-needs">${esc(needs)}</div>` : ''}</div></div>
        ${button}
      </div>`;
    }).join('');
  },
  flashAdded(id){
    this.added = id;
    this.render();
    setTimeout(() => {
      if (this.added !== id) return;
      this.added = null;
      if (Modal.kind === 'catalog') this.render();
    }, 1600);
  },
  act(a, el){
    switch (a){
      case 'cat': {
        this.cat = el.dataset.cat;
        this.render();
        const grid = q('.ig-cat-grid');
        if (grid) grid.scrollTop = 0;
        return true;
      }
      case 'add-preset': busy(el, () => addFrom({preset:el.dataset.preset}, true)); return true;
    }
    return false;
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
  open(i, switchOn){
    this.d = withHandler(clone(i));
    // Freshly added and waiting for one detail: shown switched on, so
    // saving once it is complete also turns it on.
    this.dirty = !!switchOn && !this.d.enabled;
    if (switchOn) this.d.enabled = true;
    this.sel = Math.max(0, this.d.handlers.findIndex(h => h.enabled));
    // Missing a channel: open straight on the chip that fixes it.
    const known = id => S.data.connections.discord.some(c => c.id === id);
    this.chip = allSteps(this.d).some(s => s.type === 'discord' && !known(s.params.connection)) ? 'where' : null;
    this.run = null;
    this.moreVars = false;
    this.show();
  },
  resume(){
    if (pickNewChannel(this.d)){
      this.dirty = true;
      if (this.chip === 'where') this.chip = null;
    }
    this.show();
  },
  show(){
    Modal.open('editor', 920);
    Modal.onClose = () => !this.dirty || (Modal.guard(), false);
    this.render();
  },
  handler(){ return this.d.handlers[this.sel]; },
  render(){
    const d = this.d, h = this.handler();
    const step = primaryStep(h);
    const k = step ? KINDS[step.type] : null;
    const p = presetOf(d);
    const head = modalHead(PRESET_ICON[d.preset] || (k ? k.icon : 'steps'), k ? k.c : 'var(--accent)', titleInput(d.name),
      esc(p ? p.description : 'Pick a moment, then write on the message itself. Everything else is one click away.'),
      enableSwitch('enabled', d.enabled));
    const chips = this.chips(step, h).map(c => `<button class="ig-chip${this.chip === c.id ? ' on' : ''}${c.warn ? ' warn' : ''}" data-act="chip" data-chip="${c.id}" data-key="chip-${c.id}" aria-expanded="${this.chip === c.id}">
      <span>${esc(c.k)}</span><b>${esc(c.v)}</b>${svg('chev')}</button>`).join('');
    Modal.body(`${head}
      <div class="dest-form-body ig-ed-body">
        ${this.momentsHtml()}
        <div class="ig-ed-preview" data-flip data-autoh><div class="ig-ed-pv" data-key="pv-${this.sel}">${this.previewBoxHtml(step, h)}</div></div>
        <div class="ig-ed-options">
          <div class="ig-chips" data-flip>${chips}</div>
          <div class="ig-pop${this.chip ? '' : ' closed'}" data-flip data-autoh>${this.chip
            ? `<div class="ig-pop-in" data-key="pop-${this.chip}-${this.sel}">${this.popHtml(step, h)}</div>` : ''}</div>
        </div>
      </div>
      ${footHtml(d, this.dirty, `<button class="ic-btn ic-btn-ghost" data-act="ed-builder">Open in builder</button>
        <button class="ic-btn ic-btn-ghost" data-act="ed-test">${svg('play', ' class="ig-btn-ic"')}Send a test</button>`)}`);
    Modal.form.querySelectorAll('.ig-msg').forEach(autosize);
  },
  momentsHtml(){
    const hs = this.d.handlers;
    if (hs.length === 1) return '';
    if (hs.length > 5){
      return `<div class="sub-tabs ig-seg" role="tablist">${hs.map((h, n) => `<button class="sub-tab${n === this.sel ? ' on' : ''}${h.enabled ? '' : ' ig-dim'}" role="tab" aria-selected="${n === this.sel}"
        data-act="moment" data-n="${n}">${esc(triggerLabel(h.trigger))}</button>`).join('')}${SEG_IND}</div>`;
    }
    return `<div class="ig-moments" role="tablist" style="--n:${hs.length}">${hs.map((h, n) => `<div class="ig-moment">
      <button class="ig-dot${h.enabled ? ' on' : ''}" role="switch" aria-checked="${h.enabled}" aria-label="Send on: ${esc(triggerLabel(h.trigger))}" data-act="moment-toggle" data-n="${n}">${svg('check', ' stroke-width="3.2"')}</button>
      <button class="ig-moment-name${n === this.sel ? ' on' : ''}" role="tab" aria-selected="${n === this.sel}" data-act="moment" data-n="${n}">${esc(triggerLabel(h.trigger))}</button>
    </div>`).join('')}</div>`;
  },
  previewBoxHtml(step, h){
    if (!step || !(KINDS[step.type] || {}).text){
      return `<div class="dcard-screen ig-live-plain">${previewHtml(step, h, true)}
        <div class="muted">This ${h.trigger.type === 'event' ? 'moment' : 'trigger'} has nothing to write. <a href="#" data-act="ed-builder">Open it in the builder</a> to see every step.</div></div>`;
    }
    const k = KINDS[step.type];
    const value = step.params[k.text] || '';
    // The trigger's own values first; the always-there ones behind "more".
    const all = varsFor(h);
    const common = new Set(['delay', 'delay_state', 'hold_left', 'time']);
    const globals = new Set(S.data.vars.global.map(v => v.name));
    const shown = this.moreVars ? all : all.filter(v => !globals.has(v.name) || common.has(v.name));
    const tokens = shown.map(v => `<button class="ig-token" data-act="token" data-token="${esc(v.name)}" title="e.g. ${esc(v.sample)}">+ ${esc(v.name)}</button>`).join('')
      + (shown.length < all.length ? `<button class="ig-token more" data-act="more-vars">${all.length - shown.length} more…</button>` : '');
    const sample = sampleLine(value, h);
    const editor = `<textarea class="ig-msg" data-bind="text" rows="1" aria-label="Message" spellcheck="true" data-keep-style>${esc(value)}</textarea>
      <div class="ig-sample${sample ? '' : ' empty'}">${sample}</div>`;
    let box;
    if (step.type === 'discord'){
      const ping = step.params.ping === 'here' ? '@here' : step.params.ping === 'everyone' ? '@everyone' : '';
      box = `<div class="ig-live ig-live-discord">
        <div class="ig-live-head"><span class="hash">#</span>${esc(channelName(step.params.connection).replace(/^#/, ''))}<span class="ig-live-tag">Live preview</span></div>
        <div class="ig-live-msg"><span class="ig-live-av">${svg('shield')}</span>
          <div class="ig-live-col"><div class="ig-live-meta"><b>InstantClone</b><span class="ig-app">APP</span>${ping ? `<span class="ig-ping">${esc(ping)}</span>` : ''}</div>
          ${editor}</div></div></div>`;
    } else if (step.type === 'chat'){
      const t = h.trigger;
      const ask = t.type === 'chat_command' ? (t.command || '!command') : t.type === 'chat_message' ? (t.pattern || 'hey') : '';
      box = `<div class="ig-live ig-live-chat">
        <div class="ig-live-tag">STREAM CHAT · LIVE PREVIEW</div>
        ${ask ? `<div><b class="u1">viewer</b>: ${esc(ask)}</div>` : ''}
        <div><b class="u2">${esc(chatAuthor(step))}</b>:</div>${editor}</div>`;
    } else if (step.type === 'phone'){
      box = `<div class="ig-live ig-live-phone">
        <div class="ig-live-tag">INSTANTCLONE · now</div>
        <input class="ic-input" data-bind="title" value="${esc(step.params.title || '')}" placeholder="Title" aria-label="Title">
        ${editor}</div>`;
    } else {
      box = `<div class="dcard-screen ig-live-plain">
        <div class="ic-label">${esc(k.label)}${step.type === 'http' ? ' · body' : ''}</div>${editor}</div>`;
    }
    return `${box}<div class="ig-tokens"><span>Insert</span>${tokens}</div>`;
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
    if (step && step.type === 'http') out.push({id:'url', k:'Send to', v:step.params.url || 'set the address', warn:!step.params.url});
    if (step && step.type === 'file') out.push({id:'file', k:'File', v:step.params.path || 'pick a file', warn:!step.params.path});
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
      `<button class="sub-tab${id === cur ? ' on' : ''}" data-act="${act}" data-v="${esc(id)}" aria-pressed="${id === cur}">${esc(label)}</button>`).join('')}${SEG_IND}</div>`;
    switch (this.chip){
      case 'where': {
        const cur = (allSteps(d).find(s => s.type === 'discord') || {params:{}}).params.connection;
        return `<div class="ic-label">Post in</div><div class="ig-pick">
          ${S.data.connections.discord.map(c => `<button class="dest-starter${c.id === cur ? ' on' : ''}" data-act="set-where" data-v="${esc(c.id)}">
            <span class="dest-starter-name">#${esc(c.name)}</span><span class="dest-starter-note mono">${esc(c.hint)}</span></button>`).join('')}
          <button class="dest-add ig-pick-add" data-act="conn" data-tab="discord">${svg('plus')}Add a Discord channel</button></div>`;
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
        return `<div class="dfg"><div class="dff"><label>Command</label><input class="ic-input mono" data-bind="command" value="${esc(t.command)}" spellcheck="false"></div>
          <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="aliases" value="${esc((t.aliases || []).join(' '))}" placeholder="!retraso !d" spellcheck="false"></div></div>
          <div class="dff"><label>Per viewer, at most once every</label>${seg([['0', 'no limit'], ['10000', '10 s'], ['30000', '30 s'], ['60000', '1 min']], String(t.user_cooldown_ms || 0), 'set-ucd')}</div>`;
      case 'who':
        return `<div class="ic-label">Who can use it</div>${rolesHtml(t, 'role')}<div class="muted">You can always use your own commands.</div>`;
      case 'reply':
        return `<div class="ic-label">Reply as</div>${seg([['', 'Your account'], ['bot', 'Bot account']], step.params.as || '', 'set-as')}
          ${t.type.startsWith('chat') ? `<label class="crash-check"><input type="checkbox" data-bind="reply" ${step.params.reply === 'yes' ? 'checked' : ''}><span><span class="crash-check-title">Reply in the viewer's thread</span><span class="muted">Twitch shows it as an answer to their message</span></span></label>` : ''}
          ${S.data.twitch.bot.login ? '' : '<div class="muted">No bot account yet: <a href="#" data-act="conn" data-tab="twitch">connect one</a>, or messages go out as you.</div>'}`;
      case 'priority':
        return `<div class="ic-label">Priority</div>${seg([['', 'Normal'], ['high', 'High'], ['urgent', 'Urgent']], step.params.priority || '', 'set-priority')}`;
      case 'url':
        return `<div class="dfg"><div class="dff"><label>Address</label><input class="ic-input mono" data-bind="url" value="${esc(step.params.url || '')}" placeholder="https://…" spellcheck="false"></div>
          <div class="dff"><label>Method</label><select class="ic-input" data-bind="method">${['POST', 'GET', 'PUT', 'PATCH', 'DELETE'].map(m => `<option ${m === (step.params.method || 'POST') ? 'selected' : ''}>${m}</option>`).join('')}</select></div></div>`;
      case 'file':
        return `<div class="dfg"><div class="dff"><label>File</label><input class="ic-input mono" data-bind="path" value="${esc(step.params.path || '')}" placeholder="C:\\Stream\\crashes.txt" spellcheck="false"></div>
          <div class="dff"><label>Each time</label><select class="ic-input" data-bind="mode"><option value="" ${step.params.mode !== 'append' ? 'selected' : ''}>Replace the text</option><option value="append" ${step.params.mode === 'append' ? 'selected' : ''}>Add a line</option></select></div></div>
          <div class="muted">Point an OBS text source at this file to show it on stream.</div>`;
      case 'cooldown':
        return `<div class="ic-label">Wait between two runs</div>${seg([['0', 'No limit'], ['10000', '10 s'], ['30000', '30 s'], ['60000', '1 min'], ['300000', '5 min'], ['600000', '10 min']], String(d.cooldown_ms || 0), 'set-cooldown')}`;
    }
    return '';
  },
  runHtml(){
    if (!this.run) return '<div class="ig-running"><span class="ig-spin"></span>Running the test…</div>';
    if (this.run.error) return warnHtml(esc(this.run.error));
    return `<div class="ic-label">Test run: ${esc(this.run.status)}</div>${runLogHtml(this.run.steps)}
      <div class="muted">Messages and web requests went out for real, marked [TEST] and never pinging anyone. Waits were skipped; the stream, VOD, programs and files were left alone.</div>`;
  },
  input(el){
    const h = this.handler(), step = primaryStep(h), t = h.trigger;
    const b = el.dataset.bind;
    if (b === 'name') this.d.name = el.value;
    else if (b === 'enabled') this.d.enabled = el.checked;
    else if (b === 'text' && step){ step.params[KINDS[step.type].text] = el.value; autosize(el); }
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
    this.changed();
  },
  changed(){
    this.dirty = true;
    this.render();
  },
  act(a, el){
    const h = this.handler(), step = primaryStep(h), v = el.dataset.v;
    switch (a){
      case 'moment': this.sel = +el.dataset.n; if (this.chip === '__test') this.chip = null; this.render(); return true;
      case 'moment-toggle': { const x = this.d.handlers[+el.dataset.n]; x.enabled = !x.enabled; this.changed(); return true; }
      case 'chip': this.chip = this.chip === el.dataset.chip ? null : el.dataset.chip; this.render(); return true;
      case 'token': insertAtCursor(q('.ig-msg'), '{' + el.dataset.token + '}'); return true;
      case 'more-vars': this.moreVars = true; this.render(); return true;
      case 'set-where': allSteps(this.d).filter(s => s.type === 'discord').forEach(s => { s.params.connection = v; }); this.changed(); return true;
      case 'set-ping': if (step) step.params.ping = v; this.changed(); return true;
      case 'set-send': setSend(h, v); this.changed(); return true;
      case 'set-ucd': h.trigger.user_cooldown_ms = +v; this.changed(); return true;
      case 'set-as': if (step) step.params.as = v; this.changed(); return true;
      case 'set-priority': if (step) step.params.priority = v; this.changed(); return true;
      case 'set-cooldown': this.d.cooldown_ms = +v; this.changed(); return true;
      case 'ed-builder': Builder.open(this.d, {dirty:this.dirty}); return true;
      case 'ed-test': busy(el, () => this.test()); return true;
      case 'delete': if (armConfirm(el, 'Delete for good?')) busy(el, () => remove(this.d.id)); return true;
      case 'save': busy(el, () => saveIntegration(this.d, () => { this.dirty = false; })); return true;
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
function rolesHtml(t, bind){
  const label = r => r === 'vips' ? 'VIPs' : r[0].toUpperCase() + r.slice(1);
  return `<div class="ig-roles">${['everyone', 'subs', 'vips', 'mods'].map(r =>
    `<label class="ig-role"><input type="checkbox" data-bind="${bind}" data-name="${r}" ${t.roles && t.roles[r] ? 'checked' : ''}><span>${svg('check', ' stroke-width="3"')}${label(r)}</span></label>`).join('')}</div>`;
}
function autosize(t){
  t.style.height = 'auto';
  t.style.height = (t.scrollHeight + 2) + 'px';
}
function insertAtCursor(field, text){
  if (!field || !field.isConnected || field.closest('[data-leaving]')) return;
  const start = field.selectionStart == null ? field.value.length : field.selectionStart;
  const end = field.selectionEnd == null ? start : field.selectionEnd;
  field.value = field.value.slice(0, start) + text + field.value.slice(end);
  field.focus();
  field.setSelectionRange(start + text.length, start + text.length);
  field.dispatchEvent(new Event('input', {bubbles:true}));
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
const pathKey = p => p.join('.');

const Builder = {
  d:null, h:0, sel:'trigger', target:[], run:null, dirty:false, lastField:null,
  // opts: {dirty} carries the editor's unsaved state over, {switchOn} as in Editor.open.
  open(i, opts){
    opts = opts || {};
    this.d = i ? withHandler(clone(i)) : {id:'', name:'My integration', enabled:true, preset:'', cooldown_ms:0,
      handlers:[{enabled:true, trigger:{type:'event', event:'hold_opened', filters:{}}, steps:[]}]};
    this.dirty = !!opts.dirty || (!!opts.switchOn && !this.d.enabled);
    if (opts.switchOn) this.d.enabled = true;
    this.h = 0;
    this.sel = 'trigger';
    this.target = [];
    this.run = null;
    this.lastField = null;
    this.show();
  },
  resume(){
    if (pickNewChannel(this.d)) this.dirty = true;
    this.show();
  },
  show(){
    Modal.open('builder', 1240, 'min(92vh,900px)');
    Modal.onClose = () => !this.dirty || (Modal.guard(), false);
    this.render();
  },
  handler(){ return this.d.handlers[this.h]; },
  render(){
    const d = this.d, h = this.handler();
    const n = allSteps(d).length;
    Modal.body(`${modalHead('steps', 'var(--accent)', titleInput(d.name),
      `Your own · runs on this PC · ${plural(n, 'step')}`,
      `<button class="ic-btn ig-head-btn" data-act="b-test">${svg('play', ' class="ig-btn-ic"')}Test run</button>${enableSwitch('enabled', d.enabled)}`)}
      <div class="ig-bld">
        <aside class="ig-palette" aria-label="Blocks">
          <div class="muted ig-palette-hint">Click a block to add it where the dashed box is lit.</div>
          ${PALETTE.map(([group, items]) => `<div class="ic-label">${group}</div>${items.map(([k, label]) =>
            `<button data-act="b-add" data-kind="${k}" style="--c:${KINDS[k].c}"><i></i>${esc(label)}</button>`).join('')}`).join('')}
        </aside>
        <section class="ig-canvas" aria-label="Steps">
          <div class="ig-triggers" data-flip>${d.handlers.map((x, i) => `<button class="ig-trig${i === this.h ? ' on' : ''}${x.enabled ? '' : ' ig-dim'}" data-act="b-handler" data-n="${i}" data-key="h-${uidOf(x)}">
            When ${esc(lowerFirst(triggerLabel(x.trigger)))}</button>`).join('')}
            <button class="ig-trig add" data-act="b-add-handler" data-key="h-add">${svg('plus')}Another trigger</button></div>
          <div class="ig-steps" data-flip>
            <button class="ig-step ig-when${this.sel === 'trigger' ? ' on' : ''}" data-act="b-sel" data-path="trigger" data-key="when">
              <span class="ig-step-kind">WHEN</span><span class="ig-step-text">${esc(triggerSentence(h.trigger))}</span></button>
            ${this.stepsHtml(h.steps, [], 'root')}
          </div>
          ${this.run ? `<div class="sys-section ig-run-box"><div class="ic-label">Test run: ${esc(this.run.status || 'error')}</div>
            ${this.run.error ? warnHtml(esc(this.run.error)) : this.run.status === 'running'
              ? '<div class="ig-running"><span class="ig-spin"></span>Running the test…</div>' : runLogHtml(this.run.steps)}</div>` : ''}
        </section>
        <aside class="ig-inspector" aria-label="Settings" data-flip><div class="ig-insp" data-key="${this.inspectorKey()}">${this.inspectorHtml()}</div></aside>
      </div>
      ${footHtml(d, this.dirty)}`);
  },
  // A different selection is a different panel (fresh fields, crossfade).
  inspectorKey(){
    const s = this.sel === 'trigger' ? null : this.stepAt(this.sel);
    return `insp-${uidOf(this.handler())}-${s ? uidOf(s) : 'trigger'}`;
  },
  stepsHtml(steps, base, owner){
    const here = pathKey(base);
    const lit = pathKey(this.target) === here;
    const selKey = Array.isArray(this.sel) ? pathKey(this.sel) : null;
    const tool = (act, path, icon, label, extra) =>
      `<button class="ic-btn-tiny" data-act="${act}" data-path="${path}" aria-label="${label}"${extra || ''}>${svg(icon)}</button>`;
    const out = (steps || []).map((s, n) => {
      const path = base.concat(n), pk = pathKey(path), id = uidOf(s);
      const k = KINDS[s.type] || {label:s.type, tag:'?', c:'#888'};
      let html = `<div class="ig-step${selKey === pk ? ' on' : ''}" data-act="b-sel" data-path="${pk}" style="--c:${k.c}" data-key="s-${id}" role="button" tabindex="0">
        <span class="ig-step-kind">${k.tag}</span><span class="ig-step-text">${stepSentence(s)}</span>
        <span class="ig-step-tools">
          ${tool('b-move', pk, 'up', 'Move up', ` data-dir="-1"${n === 0 ? ' disabled' : ''}`)}
          ${tool('b-move', pk, 'down', 'Move down', ` data-dir="1"${n === steps.length - 1 ? ' disabled' : ''}`)}
          ${tool('b-dup', pk, 'copy', 'Duplicate')}
          ${tool('b-del', pk, 'x', 'Remove')}
        </span></div>`;
      if (s.type === 'if'){
        html += `<div class="ig-branch" data-key="br-${id}">
          <div class="ig-branch-label">Then</div><div class="ig-branch-list" data-flip>${this.stepsHtml(s.then, path.concat('then'), id + 't')}</div>
          <div class="ig-branch-label else">Otherwise</div><div class="ig-branch-list" data-flip>${this.stepsHtml(s.else, path.concat('else'), id + 'e')}</div>
        </div>`;
      }
      return html;
    }).join('');
    return out + `<button class="ig-drop${lit ? ' on' : ''}" data-act="b-target" data-path="${here}" data-key="drop-${owner}">${lit ? 'New blocks land here' : '+ Add a step here'}</button>`;
  },
  resolve(pathStr){
    return pathStr === '' ? [] : pathStr.split('.').map(p => /^\d+$/.test(p) ? +p : p);
  },
  // The list a path's last index lives in, and that index.
  locate(path){
    let list = this.handler().steps;
    for (let n = 0; n < path.length - 1; n += 2) list = list[path[n]][path[n + 1]];
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
    const varsBox = `<div class="ig-insp-vars"><div class="ic-label">Variables here</div>
      <div class="ig-tokens">${tokens}</div><div class="muted">Click one to insert it in the field you last used. <span class="mono">{name|text}</span> uses the text when the value is empty or 0.</div></div>`;
    if (this.sel === 'trigger') return this.triggerInspector(h) + varsBox;
    const s = this.stepAt(this.sel);
    if (!s) return varsBox;
    const k = KINDS[s.type];
    return `<div class="ig-insp-title" style="--c:${k.c}"><small>${k.tag}</small><b>${esc(k.label)}</b></div>${this.stepFields(s)}${varsBox}`;
  },
  stepFields(s){
    const field = (name, label, opts) => {
      opts = opts || {};
      const v = s.params[name] || '';
      if (opts.select) return `<div class="dff"><label>${label}</label><select class="ic-input" data-bind="p" data-name="${name}">${opts.select.map(([id, l]) => `<option value="${esc(id)}" ${id === v ? 'selected' : ''}>${esc(l)}</option>`).join('')}</select></div>`;
      if (opts.area) return `<div class="dff"><label>${label}</label><textarea class="ic-input" data-bind="p" data-name="${name}" placeholder="${esc(opts.ph || '')}">${esc(v)}</textarea></div>`;
      return `<div class="dff"><label>${label}</label><input class="ic-input${opts.mono ? ' mono' : ''}" data-bind="p" data-name="${name}" value="${esc(v)}" placeholder="${esc(opts.ph || '')}"${opts.mono ? ' spellcheck="false"' : ''}></div>`;
    };
    const note = text => `<div class="muted">${text}</div>`;
    const channels = S.data.connections.discord.map(c => [c.id, '#' + c.name]);
    switch (s.type){
      case 'discord': return (channels.length ? field('connection', 'Channel', {select:[['', 'Pick a channel']].concat(channels)}) : note('No Discord channel yet. <a href="#" data-act="conn" data-tab="discord">Add one</a>.'))
        + field('text', 'Message', {area:true}) + field('ping', 'Ping', {select:[['', 'Nobody'], ['here', '@here'], ['everyone', '@everyone']]});
      case 'chat': return field('text', 'Message', {area:true}) + field('as', 'Send as', {select:[['', 'Your account'], ['bot', 'Bot account']]})
        + field('reply', 'Reply to the viewer', {select:[['', 'No'], ['yes', 'Yes, in their thread']]});
      case 'phone': return field('title', 'Title') + field('text', 'Message', {area:true}) + field('priority', 'Priority', {select:[['', 'Normal'], ['high', 'High'], ['urgent', 'Urgent']]});
      case 'http': return field('method', 'Method', {select:['POST', 'GET', 'PUT', 'PATCH', 'DELETE'].map(m => [m, m])}) + field('url', 'Address', {mono:true, ph:'https://…'})
        + field('headers', 'Headers (one per line, Name: value)', {area:true}) + field('body', 'Body', {area:true, ph:'{"event":"{delay}"}'})
        + field('save_as', 'Save the answer as', {mono:true, ph:'response'})
        + note(`Later steps can use <span class="mono">{${esc(s.params.save_as || 'response')}.status}</span>, <span class="mono">.ok</span>, <span class="mono">.body</span> and <span class="mono">.json.field</span>.`);
      case 'wait': return field('ms', 'Wait (milliseconds)', {mono:true, ph:'20000'}) + note(`${esc(fmtMs(s.params.ms) || '0 s')}. Up to an hour.`);
      case 'wait_delay': return '<p class="ig-insp-p">Waits until the moment this started has reached your viewers. Only InstantClone can do this: it knows your delay.</p>'
        + field('extra_ms', 'Then wait a bit more (ms)', {mono:true, ph:'0'}) + field('follow', 'If the delay changes meanwhile', {select:[['', 'Follow it'], ['no', 'Keep the delay from the start']]});
      case 'if': return field('left', 'Check', {mono:true, ph:'{user_role}'}) + field('op', 'Is', {select:Object.entries(OPS)}) + field('right', 'Value', {mono:true, ph:'mod'})
        + note('Text compares ignoring case; "is more than" compares numbers.');
      case 'stop': return note('Ends this run here.');
      case 'delay_action': return field('action', 'Action', {select:Object.entries(DELAY_ACTIONS)})
        + (s.params.action === 'arm' ? field('seconds', 'Delay (seconds)', {mono:true, ph:'{arg1}'})
          + note('Arms this delay, or changes it live when a delay is already on air. Never disarms.') : '')
        + warnHtml('This changes your stream. Tests never run it.');
      case 'marker': return field('description', 'Label', {ph:'Crash'}) + note('Only works while you are live on Twitch.');
      case 'clip': return note('Clips the last moments of your stream. Later steps can use <span class="mono">{clip.url}</span> and <span class="mono">{clip.ok}</span>.');
      case 'program': return field('path', 'Program (a fixed path, no variables)', {mono:true, ph:'C:\\Tools\\thing.exe'}) + field('args', 'Arguments', {mono:true, ph:'--scene "Replay"'})
        + warnHtml('Runs on your PC. Recipes you import can never add this without asking you first.');
      case 'file': return field('path', 'File (a fixed path, no variables)', {mono:true, ph:'C:\\Stream\\status.txt'}) + field('text', 'Text', {area:true}) + field('mode', 'Each time', {select:[['', 'Replace the text'], ['append', 'Add a line']]});
      case 'set_var': return field('name', 'Name', {mono:true, ph:'winner'}) + field('value', 'Value', {ph:'{user}'}) + note('Later steps use it as <span class="mono">{name}</span>. Lasts for this run.');
      case 'counter': return field('name', 'Name', {mono:true, ph:'crashes'}) + field('op', 'Do', {select:[['', 'Add'], ['subtract', 'Subtract'], ['set', 'Set to'], ['reset', 'Reset to 0']]})
        + field('by', 'By', {mono:true, ph:'1'}) + note(`Kept between runs. Use it anywhere as <span class="mono">{counter.${esc(s.params.name || 'name')}}</span>.`);
    }
    return '';
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
      body = `<div class="dff"><label>Command</label><input class="ic-input mono" data-bind="t-command" value="${esc(t.command || '')}" placeholder="!clip" spellcheck="false"></div>
        <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="t-aliases" value="${esc((t.aliases || []).join(' '))}" spellcheck="false"></div>
        <div class="dff"><label>Who can use it</label>${rolesHtml(t, 't-role')}</div>
        <div class="dff"><label>Per viewer, once every (seconds)</label><input class="ic-input mono" data-bind="t-ucd" inputmode="numeric" value="${Math.round((t.user_cooldown_ms || 0) / 1000)}"></div>`;
    } else if (t.type === 'chat_message'){
      body = `<div class="dff"><label>Matches</label><input class="ic-input" data-bind="t-pattern" value="${esc(t.pattern || '')}" placeholder="gg*"></div>
        <div class="dff"><label>How</label><select class="ic-input" data-bind="t-mode">${[['contains', 'Contains'], ['starts_with', 'Starts with'], ['exact', 'Is exactly'], ['wildcard', 'Wildcard (* and ?)']].map(([id, l]) => `<option value="${id}" ${id === (t.mode || 'contains') ? 'selected' : ''}>${l}</option>`).join('')}</select></div>
        <div class="dff"><label>Who</label>${rolesHtml(t, 't-role')}</div>`;
    } else if (t.type === 'timer'){
      body = `<div class="dff"><label>Every (minutes)</label><input class="ic-input mono" data-bind="t-every" inputmode="numeric" value="${Math.max(1, Math.round((t.every_ms || 600000) / 60000))}"></div>
        <label class="crash-check"><input type="checkbox" data-bind="t-live" ${t.only_live !== false ? 'checked' : ''}><span><span class="crash-check-title">Only while streaming</span></span></label>`;
    } else if (t.type === 'webhook'){
      const url = `${location.origin}/hooks/${t.token || ''}`;
      body = `<div class="dff"><label>Call this address</label><div class="ig-copy-row"><input class="ic-input mono" readonly value="${esc(url)}">
        <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(url)}">Copy</button></div></div>
        <div class="muted">GET or POST from a Stream Deck, a script or any app on this network. The body is <span class="mono">{body}</span>; JSON fields are <span class="mono">{body.field}</span>.</div>
        <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="b-new-token">Make a new secret address</button>`;
    }
    const more = this.d.handlers.length > 1
      ? `<div class="ig-insp-row">${enableSwitch('h-enabled', h.enabled, 'This trigger is on')}
         <button class="ic-btn ic-btn-ghost ig-small" data-act="b-del-handler">Remove trigger</button></div>` : '';
    return `<div class="ig-insp-title" style="--c:#5ac8fa"><small>WHEN</small><b>${esc(triggerLabel(t))}</b></div>
      ${typeSel}${body}
      <div class="dff"><label>Wait between two runs (seconds)</label><input class="ic-input mono" data-bind="b-cooldown" inputmode="numeric" value="${Math.round((this.d.cooldown_ms || 0) / 1000)}"></div>${more}`;
  },
  input(el){
    const b = el.dataset.bind, h = this.handler(), t = h.trigger;
    if (b === 'name') this.d.name = el.value;
    else if (b === 'enabled') this.d.enabled = el.checked;
    else if (b === 'b-cooldown') this.d.cooldown_ms = Math.max(0, (parseFloat(el.value) || 0) * 1000);
    else if (b === 'h-enabled') h.enabled = el.checked;
    else if (b === 't-type') h.trigger = newTrigger(el.value);
    else if (b === 't-event'){ t.event = el.value; t.filters = {}; }
    else if (b === 't-filter'){ t.filters = t.filters || {}; t.filters[el.dataset.name] = el.value.trim(); }
    else if (b === 't-command') t.command = el.value.trim().toLowerCase();
    else if (b === 't-aliases') t.aliases = el.value.split(/[\s,]+/).map(x => x.trim().toLowerCase()).filter(Boolean);
    else if (b === 't-role'){ t.roles = t.roles || {}; t.roles[el.dataset.name] = el.checked; }
    else if (b === 't-ucd') t.user_cooldown_ms = Math.max(0, (parseFloat(el.value) || 0) * 1000);
    else if (b === 't-pattern') t.pattern = el.value;
    else if (b === 't-mode') t.mode = el.value;
    else if (b === 't-every') t.every_ms = Math.max(1, parseFloat(el.value) || 1) * 60000;
    else if (b === 't-live') t.only_live = el.checked;
    else if (b === 'p') this.stepAt(this.sel).params[el.dataset.name] = el.value;
    else return;
    this.dirty = true;
    this.render();
  },
  edited(){
    this.dirty = true;
    this.render();
  },
  act(a, el){
    const d = this.d;
    switch (a){
      case 'b-sel': this.sel = el.dataset.path === 'trigger' ? 'trigger' : this.resolve(el.dataset.path); this.render(); return true;
      case 'b-target': this.target = this.resolve(el.dataset.path); this.render(); return true;
      case 'b-add': {
        let list = null;
        try {
          const owner = this.target.length ? this.stepAt(this.target.slice(0, -1)) : null;
          if (!this.target.length || (owner && owner.type === 'if')) list = this.listAt(this.target);
        } catch(_){ list = null; }
        if (!list){ this.target = []; list = this.handler().steps; }
        list.push(newStep(el.dataset.kind));
        this.sel = this.target.concat(list.length - 1);
        if (el.dataset.kind === 'if') this.target = this.sel.concat('then');
        this.edited();
        const added = q('.ig-step.on');
        if (added) added.scrollIntoView({block:'nearest', behavior:reduced() ? 'auto' : 'smooth'});
        return true;
      }
      case 'b-move': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        const to = index + (+el.dataset.dir);
        if (to < 0 || to >= list.length) return true;
        [list[index], list[to]] = [list[to], list[index]];
        this.sel = path.slice(0, -1).concat(to);
        this.target = [];
        this.edited();
        return true;
      }
      case 'b-dup': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        list.splice(index + 1, 0, clone(list[index]));
        this.sel = path.slice(0, -1).concat(index + 1);
        this.target = [];
        this.edited();
        return true;
      }
      case 'b-del': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        list.splice(index, 1);
        this.sel = 'trigger';
        this.target = [];
        this.edited();
        return true;
      }
      case 'b-handler': this.h = +el.dataset.n; this.sel = 'trigger'; this.target = []; this.run = null; this.render(); return true;
      case 'b-add-handler':
        d.handlers.push({enabled:true, trigger:newTrigger('event'), steps:[]});
        this.h = d.handlers.length - 1; this.sel = 'trigger'; this.target = []; this.run = null;
        this.edited();
        return true;
      case 'b-del-handler':
        if (d.handlers.length < 2 || !armConfirm(el, 'Remove it and its steps?')) return true;
        d.handlers.splice(this.h, 1);
        this.h = 0; this.sel = 'trigger'; this.target = []; this.run = null;
        this.edited();
        return true;
      case 'b-new-token': this.handler().trigger.token = randomToken(); this.edited(); return true;
      case 'b-token': insertAtCursor(this.lastField, '{' + el.dataset.token + '}'); return true;
      case 'b-test': busy(el, () => this.test()); return true;
      case 'delete': if (armConfirm(el, 'Delete for good?')) busy(el, () => remove(d.id)); return true;
      case 'save': busy(el, () => saveIntegration(d, () => { this.dirty = false; })); return true;
    }
    return false;
  },
  async test(){
    this.run = {status:'running', steps:[]};
    this.render();
    const r = await api('/integrations/test', {integration:this.d, handler:this.h});
    this.run = r.ok ? r : {error:r.error || 'The test failed'};
    if (Modal.kind === 'builder') this.render();
  },
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
    Modal.open('connections', 760);
    Modal.onClose = () => { if (S.data.twitch.login) api('/twitch/cancel', {}); return true; };
    if (from) Modal.after = () => from.resume();
    this.render();
  },
  render(){
    // A slow reply can land after the user left this page.
    if (Modal.kind !== 'connections') return;
    const tabs = [['discord', 'Discord'], ['twitch', 'Twitch'], ['phone', 'Phone']].map(([id, l]) =>
      `<button class="sub-tab${this.tab === id ? ' on' : ''}" role="tab" aria-selected="${this.tab === id}" data-act="conn-tab" data-tab="${id}">${l}</button>`).join('');
    const pane = this.tab === 'discord' ? this.discordHtml() : this.tab === 'twitch' ? this.twitchHtml() : this.phoneHtml();
    Modal.body(`${modalHead('link', 'var(--accent)', 'Connections', 'Where integrations post. Set each one up once.')}
      <div class="dest-form-body ig-conn-body">
        <div class="sub-tabs ig-seg" role="tablist">${tabs}${SEG_IND}</div>
        <div class="ig-conn-pane" data-flip data-autoh><div class="ig-conn-in" data-key="tab-${this.tab}">${pane}</div></div>
      </div>`);
  },
  discordHtml(){
    const e = this.editing;
    const rows = S.data.connections.discord.map(c => `<div class="ig-acct" data-key="d-${esc(c.id)}"><span class="dcard-icon ig-ic-discord">${svg('bubble')}</span>
      <div class="dcard-id"><div class="dcard-name">#${esc(c.name)}</div><div class="dcard-host">${esc(c.hint)}</div></div>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="d-test" data-id="${esc(c.id)}">Test</button>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="d-edit" data-id="${esc(c.id)}">Edit</button>
      <button class="ic-btn-tiny" data-act="d-del" data-id="${esc(c.id)}" aria-label="Remove #${esc(c.name)}">${svg('x')}</button></div>`).join('');
    const form = e ? `<div class="sys-section ig-form" data-key="d-form-${esc(e.id || 'new')}">
        <div class="ic-label">${e.id ? 'Edit channel' : 'New channel'}</div>
        <div class="dfg"><div class="dff"><label>Name</label><input class="ic-input" data-bind="d-name" value="${esc(e.name)}" placeholder="Mods" autofocus></div>
        <div class="dff"><label>Webhook link</label><input class="ic-input mono" data-bind="d-url" value="" placeholder="${e.id ? 'leave blank to keep it' : 'https://discord.com/api/webhooks/…'}" spellcheck="false"></div></div>
        <div class="muted">In Discord: Server settings › Integrations › Webhooks › New webhook › Copy webhook URL.</div>
        <div class="ig-form-actions"><button class="ic-btn ic-btn-ghost" data-act="d-cancel">Cancel</button><button class="ic-btn ic-btn-primary" data-act="d-save">Save channel</button></div>
      </div>` : `<button class="dest-add ig-add-row" data-act="d-new" data-key="d-add"><span class="dest-add-icon">${svg('plus')}</span>Add a Discord channel</button>`;
    return `<div class="ig-accts" data-flip>${rows || '<div class="muted ig-none" data-key="d-none">No Discord channel yet.</div>'}${form}</div>`;
  },
  twitchHtml(){
    const t = S.data.twitch;
    if (!t.available) return warnHtml('This build has no Twitch app id, so it can\'t log in to Twitch. Official releases include one.');
    return (t.notice ? warnHtml(esc(t.notice)) : '')
      + this.accountHtml('main', t.main, 'Your Twitch account', 'Chat, VOD markers and clips. Log in once: InstantClone keeps it fresh.')
      + this.accountHtml('bot', t.bot, 'Bot account (optional)', 'A second account that posts in chat instead of you.')
      + '<div class="muted">No password ever goes through InstantClone: you approve it on twitch.tv, and can revoke it there any time.</div>';
  },
  accountHtml(which, a, title, blurb){
    const flow = S.data.twitch.login;
    const active = flow && flow.which === which;
    let right;
    if (active){
      right = flow.error
        ? `<button class="ic-btn" data-act="t-login" data-which="${which}">Try again</button>`
        : '<span class="ig-running"><span class="ig-spin"></span>Waiting for you on Twitch…</span>';
    } else if (a.login){
      right = `<span class="dcard-status ${a.chat === 'connected' ? 's-live' : 'ig-flag warn'}"><span class="d-dot"></span><span class="dcard-status-l">chat ${esc(a.chat)}</span></span>
        <button class="ic-btn ic-btn-ghost ig-small" data-act="t-logout" data-which="${which}">Disconnect</button>`;
    } else {
      right = `<button class="ic-btn ${which === 'main' ? 'ic-btn-primary' : ''}" data-act="t-login" data-which="${which}">Connect</button>`;
    }
    let code = '';
    if (active && flow.error) code = warnHtml(esc(flow.error));
    else if (active && flow.user_code){
      const uri = /^https:\/\//.test(flow.uri || '') ? flow.uri : 'https://www.twitch.tv/activate';
      code = `<div class="ig-device">
        <div class="ig-code">${esc(flow.user_code)}</div>
        <div class="ig-device-actions">
          <a class="ic-btn ic-btn-primary" href="${esc(uri)}" target="_blank" rel="noopener">Open twitch.tv/activate</a>
          <button class="ic-btn ic-btn-ghost" data-act="copy" data-text="${esc(flow.user_code)}">Copy code</button>
          <button class="ic-btn ic-btn-ghost" data-act="t-cancel">Cancel</button></div>
        <div class="muted">Enter the code there and approve. This page updates on its own. The code works for ${plural(Math.max(1, Math.round(flow.expires_in_s / 60)), 'more minute')}.</div></div>`;
    } else if (active) code = '<div class="ig-running"><span class="ig-spin"></span>Asking Twitch for a code…</div>';
    return `<div class="sys-section ig-account">
      <div class="ig-acct bare"><span class="dcard-icon ig-ic-twitch">${svg('bubble')}</span>
        <div class="dcard-id"><div class="dcard-name">${a.login ? esc(a.login) : title}</div><div class="dcard-host ig-blurb">${blurb}</div></div>${right}</div>${code}</div>`;
  },
  phoneHtml(){
    const p = S.data.connections.phone;
    return `<div class="sys-section ig-form">
      <div class="ig-insp-p">Phone pushes use <b>ntfy</b>, a free app for Android and iPhone. Install it, subscribe to a topic, and put the same topic here. Anyone who knows the topic can read it, so make it hard to guess.</div>
      <div class="dfg"><div class="dff"><label>Topic</label><div class="ig-copy-row"><input class="ic-input mono" data-bind="p-topic" value="${esc(p.topic)}" placeholder="instantclone-…" spellcheck="false">
        <button class="ic-btn ic-btn-ghost ig-small" data-act="p-random" title="Make a hard-to-guess topic">Random</button></div></div>
        <div class="dff"><label>Server <span class="muted">(optional)</span></label><input class="ic-input mono" data-bind="p-server" value="${esc(p.server)}" placeholder="https://ntfy.sh" spellcheck="false"></div></div>
      <div class="ig-form-actions"><button class="ic-btn ic-btn-ghost" data-act="p-test" ${p.topic ? '' : 'disabled'}>Send a test push</button><button class="ic-btn ic-btn-primary" data-act="p-save">Save</button></div>
    </div>`;
  },
  value(bind){
    const el = q(`[data-bind="${bind}"]`);
    return el ? el.value : '';
  },
  async act(a, el){
    switch (a){
      case 'conn-tab': this.tab = el.dataset.tab; this.editing = null; this.render(); return true;
      case 'd-new': this.editing = {id:'', name:''}; this.render(); focusField('d-name'); return true;
      case 'd-edit': {
        const c = S.data.connections.discord.find(x => x.id === el.dataset.id);
        if (c){ this.editing = {id:c.id, name:c.name}; this.render(); focusField('d-name'); }
        return true;
      }
      case 'd-cancel': this.editing = null; this.render(); return true;
      case 'd-save':
        await busy(el, async () => {
          const r = await api('/connections/discord', {id:this.editing.id, name:this.value('d-name'), url:this.value('d-url')});
          if (!r.ok){ toast(r.error, 'err', 6000); return; }
          this.editing = null;
          await refreshData();
          this.render();
          toast('Channel saved', 'ok');
        });
        return true;
      case 'd-del':
        if (!armConfirm(el, 'Remove?')) return true;
        await busy(el, async () => {
          await api('/connections/discord/delete', {id:el.dataset.id});
          await refreshData();
          this.render();
          toast('Channel removed. Integrations posting there are off until you pick another.', 'info', 5000);
        });
        return true;
      case 'd-test':
        await busy(el, async () => {
          const r = await api('/connections/discord/test', {id:el.dataset.id});
          toast(r.ok ? 'Sent a test message, check Discord' : r.error, r.ok ? 'ok' : 'err', 5000);
        });
        return true;
      case 't-login':
        await busy(el, async () => { await api('/twitch/login', {which:el.dataset.which}); await refreshData(); schedulePoll(); this.render(); });
        return true;
      case 't-cancel':
        await busy(el, async () => { await api('/twitch/cancel', {}); await refreshData(); this.render(); });
        return true;
      case 't-logout':
        if (!armConfirm(el, 'Chat stops. Sure?')) return true;
        await busy(el, async () => {
          await api('/twitch/logout', {which:el.dataset.which});
          await new Promise(r => setTimeout(r, 400));
          await refreshData();
          this.render();
        });
        return true;
      case 'p-random': {
        const f = q('[data-bind="p-topic"]');
        if (f){ f.value = 'instantclone-' + randomToken().slice(0, 12); f.focus(); }
        return true;
      }
      case 'p-save':
        await busy(el, async () => {
          const r = await api('/connections/phone', {topic:this.value('p-topic'), server:this.value('p-server')});
          if (!r.ok){ toast(r.error, 'err', 6000); return; }
          await refreshData();
          this.render();
          toast('Phone saved', 'ok');
        });
        return true;
      case 'p-test':
        await busy(el, async () => {
          const r = await api('/connections/phone/test', {});
          toast(r.ok ? 'Sent, check your phone' : r.error, r.ok ? 'ok' : 'err', 5000);
        });
        return true;
    }
    return false;
  },
};

function focusField(bind){
  const f = q(`[data-bind="${bind}"]`);
  if (f) f.focus({preventScroll:true});
}

// ---------------------------------------------------------------- recipes

const Recipes = {
  preview:null, text:'',
  resume(){
    Modal.open('import', 700);
    this.render();
  },
  openImport(){
    this.preview = null;
    this.text = '';
    Modal.open('import', 700);
    this.render();
    focusField('r-text');
  },
  render(){
    if (Modal.kind !== 'import') return;
    const p = this.preview;
    const channels = S.data.connections.discord;
    Modal.body(`${modalHead('download', 'var(--accent)', 'Import a recipe', 'Paste a recipe someone shared. You see everything it adds before anything changes.')}
      <div class="dest-form-body ig-conn-body">
        <textarea class="ic-input ig-textarea-code" data-bind="r-text" placeholder="ic-recipe:1:…" spellcheck="false">${esc(this.text)}</textarea>
        ${p ? `<div class="ig-recipe" data-key="r-${esc(p.name)}">
          <div class="sys-section ig-recipe-list">
            <div class="ig-recipe-head"><span class="ig-ok">${svg('check')}</span><b>${esc(p.name)}</b><span class="muted">${plural(p.items.length, 'integration')}</span></div>
            ${p.items.map(it => `<div class="ig-recipe-item"><b>${esc(it.name)}</b> <span class="muted">${esc(it.summary)}</span></div>`).join('')}
          </div>
          ${p.uses_discord ? (channels.length ? `<div class="dff"><label>Post its Discord messages in</label><select class="ic-input" data-bind="r-discord">${channels.map(c => `<option value="${esc(c.id)}">#${esc(c.name)}</option>`).join('')}</select></div>`
            : warnHtml('It posts to Discord: <a href="#" data-act="conn" data-tab="discord">add a Discord channel</a> first.')) : ''}
          ${p.local_effects ? `<label class="crash-check lan-warn"><input type="checkbox" data-bind="r-local"><span><span class="crash-check-title">I trust this recipe to run programs or write files on this PC</span><span class="muted">Only tick this if you know who made it.</span></span></label>` : ''}
          <div class="muted ig-recipe-safe">${svg('shield')}<span>Recipes are settings, not code. They can't see your keys, tokens or webhook links, and they arrive switched off.</span></div>
        </div>` : ''}
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Cancel</button>
        ${p ? `<button class="ic-btn ic-btn-primary" data-act="r-apply">Add ${p.items.length}</button>` : '<button class="ic-btn ic-btn-primary" data-act="r-preview">Preview</button>'}
      </div>`);
  },
  async act(a, el){
    if (a === 'r-preview'){
      await busy(el, async () => {
        this.text = Conn.value('r-text');
        const r = await api('/integrations/import', {recipe:this.text});
        if (!r.ok){ toast(r.error, 'err', 6000); return; }
        this.preview = r;
        this.render();
      });
      return true;
    }
    if (a === 'r-apply'){
      await busy(el, async () => {
        const sel = q('[data-bind="r-discord"]'), local = q('[data-bind="r-local"]');
        const r = await api('/integrations/import', {recipe:this.text, apply:true, discord:sel ? sel.value : '', allow_local:!!(local && local.checked)});
        if (!r.ok){ toast(r.error, 'err', 6000); return; }
        if (Modal.kind === 'import') Modal.close(true);
        await load();
        toast(`Added ${r.added}. They're off until you switch them on.`, 'ok', 4500);
      });
      return true;
    }
    if (a === 's-make'){ await busy(el, () => this.share()); return true; }
    return false;
  },
  openShare(){
    Modal.open('share', 700);
    Modal.body(`${modalHead('copy', 'var(--accent)', 'Share as a recipe', 'Pick what to share. Your connections, keys and secret links are never included.')}
      <div class="dest-form-body ig-conn-body">
        <div class="dff"><label>Recipe name</label><input class="ic-input" data-bind="s-name" placeholder="My crash kit"></div>
        <div class="ig-share-list">${S.data.integrations.map(i =>
          `<label class="crash-check"><input type="checkbox" data-bind="s-pick" value="${esc(i.id)}"><span><span class="crash-check-title">${esc(i.name)}</span></span></label>`).join('')}</div>
        <textarea class="ic-input ig-textarea-code" data-bind="s-out" readonly hidden></textarea>
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Close</button><button class="ic-btn ic-btn-primary" data-act="s-make">Make recipe</button></div>`);
    focusField('s-name');
  },
  async share(){
    const ids = [...Modal.el.querySelectorAll('[data-bind="s-pick"]:checked')].map(x => x.value);
    if (!ids.length){ toast('Tick at least one integration to share', 'info'); return; }
    const r = await api('/integrations/export', {ids, name:Conn.value('s-name')});
    if (!r.ok){ toast(r.error, 'err'); return; }
    const out = q('[data-bind="s-out"]');
    out.hidden = false;
    out.value = r.recipe;
    out.select();
    navigator.clipboard.writeText(r.recipe).then(() => toast('Recipe copied: paste it anywhere', 'ok', 3500), () => {});
  },
};

// ---------------------------------------------------------------- events

const OWNERS = {editor:Editor, builder:Builder, connections:Conn, import:Recipes, share:Recipes, catalog:Catalog};

async function onClick(e){
  const el = e.target.closest('[data-act]');
  if (!el || el.disabled) return;
  const a = el.dataset.act;
  if (el.tagName === 'A') e.preventDefault();
  const owner = el.closest('.ig-modal') && OWNERS[Modal.kind];
  if (owner && await owner.act(a, el)) return;
  switch (a){
    case 'modal-close': Modal.close(); break;
    case 'guard-keep': Modal.hideGuard(); break;
    case 'guard-discard': Modal.close(true); break;
    case 'guard-save': {
      const save = q('.dest-form-foot [data-act="save"]');
      Modal.hideGuard();
      if (save) save.click();
      break;
    }
    case 'catalog': Catalog.open(); break;
    case 'build': Builder.open(null); break;
    case 'import': Recipes.openImport(); break;
    case 'share': Recipes.openShare(); break;
    case 'conn': Conn.open(el.dataset.tab); break;
    case 'retry': busy(el, load); break;
    case 'edit': { const i = find(el.dataset.id); if (i) openEditor(i); break; }
    case 'toggle': toggle(el.dataset.id); break;
    case 'duplicate': busy(el, () => duplicate(el.dataset.id)); break;
    case 'peek': S.peek = S.peek === el.dataset.id ? null : el.dataset.id; render(); break;
    case 'filter': S.filter = el.dataset.f; render(); break;
    case 'view': S.view = el.dataset.v; store.set('ig-view', S.view); render(); break;
    case 'copy': copyText(el); break;
    case 'pack':
      busy(el, async () => {
        if (Modal.kind === 'catalog') Modal.close(true);
        await addFrom({pack:el.dataset.pack}, false);
      });
      break;
  }
}

function onInput(e){
  const el = e.target;
  if (el.id === 'ig-search'){
    S.search = el.value;
    render();
    return;
  }
  if (!el.dataset || !el.dataset.bind || !Modal.el || !Modal.el.contains(el)) return;
  if (Modal.kind === 'editor') Editor.input(el);
  else if (Modal.kind === 'builder') Builder.input(el);
}

// Editors normalise some fields (commands lowercased, numbers clamped)
// without touching the field being typed in; once it is left, show the
// value that was kept. The last redraw rendered it into the attribute.
function onFieldLeave(e){
  const el = e.target;
  if (!el.dataset || !el.dataset.bind || (Modal.kind !== 'editor' && Modal.kind !== 'builder')) return;
  if (el.tagName === 'INPUT' && el.type !== 'checkbox' && el.type !== 'radio' && el.hasAttribute('value')){
    const kept = el.getAttribute('value');
    if (el.value !== kept) el.value = kept;
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
// A hidden page has nobody to show activity to; catch up when it's back.
document.addEventListener('visibilitychange', () => {
  if (!document.hidden && (S.visible || modalOpen())) poll();
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
    const root = $('ig-root');
    // Known data paints at once and refreshes quietly; a first visit shows
    // the skeleton until the app answers.
    if (S.data){
      render();
      enterStagger(root);
      load();
    } else if (root){
      root.innerHTML = skeletonHtml();
      if (await load() && S.visible) enterStagger(root);
    }
    if (S.visible) schedulePoll();
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
