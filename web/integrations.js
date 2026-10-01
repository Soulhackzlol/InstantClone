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
  globe:'M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM2 12h20M12 2a15 15 0 0 1 0 20M12 2a15 15 0 0 0 0 20',
  pencil:'M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z',
  back:'M15 18l-6-6 6-6',
  dots:'M5 12h.01M12 12h.01M19 12h.01',
  eye:'M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z',
  eyeOff:'M3 3l18 18M10.6 10.6a2 2 0 0 0 2.8 2.8M9.9 5.1A10 10 0 0 1 12 5c6.4 0 10 7 10 7a17 17 0 0 1-3 3.9M6.6 6.6C3.8 8.4 2 12 2 12s3.6 7 10 7c1.9 0 3.6-.6 5-1.5',
  screen:'M3 4h18v12H3zM8 20h8M12 16v4',
  type:'M4 7V4h16v3M9 20h6M12 4v16',
  keyboard:'M3 6h18v12H3zM7 10h.01M11 10h.01M15 10h.01M7 14h10',
  moon:'M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z',
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
  overlay:      {label:'Show on stream', tag:'STREAM', c:'#5ac8fa', icon:'screen',  text:'text'},
  edit_text:    {label:'Edit text',     tag:'VALUE',  c:'#93c5fd', icon:'type'},
};
// The icon a catalog module wears: what it is about, not how it sends.
const PRESET_ICON = {
  crash_alert:'shield', destination_down:'signal', going_live:'megaphone', phone_crash:'phone',
  tell_chat:'bubble', delay_command:'hash', delay_notice:'clock', mod_controls:'cut', socials:'hash',
  vod_markers:'bookmark', webhook:'link', crash_counter:'file', stream_alerts:'bell', api_command:'globe',
  clip_button:'film',
};
// Which step a card previews: the first one that says something.
const PREVIEW_ORDER = ['discord','chat','phone','overlay','marker','http','file','clip','delay_action','program'];
const DELAY_ACTIONS = {
  cut:'Cut the delay', arm:'Set the delay to…', activate:'Turn the delay on', toggle:'Toggle the delay',
  cut_after:'Cut after this airs', end_hold:'End the reconnect screen',
};
const OPS = {
  is:'is', is_not:'is not', contains:'contains', not_contains:'does not contain', starts_with:'starts with',
  greater:'is more than', less:'is less than', empty:'is empty', not_empty:'is not empty',
};
// What an "Edit text" step can do: [label, what `a` is, what `b` is].
// Mirrors template::edit_text.
const EDIT_OPS = {
  first_line:['Keep the first line', '', ''],
  between:['Keep the part between two texts', 'After', 'Before'],
  replace:['Find and replace', 'Find', 'Replace with'],
  cut:['Cut it to a length', 'Characters', ''],
  round:['Round a number', 'Decimals', ''],
  digits:['Group the digits (12,571,578)', '', ''],
  upper:['UPPERCASE', '', ''],
  lower:['lowercase', '', ''],
  random:['Pick one line at random', '', ''],
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
    case 'shortcut': return shortcutLabel(t) || 'A hotkey or pad';
  }
  return t.type;
}
// `Ctrl+Alt+K or pad 36`, or '' when nothing is set yet.
function shortcutLabel(t){
  return [t.hotkey, t.midi && padLabel(t.midi)].filter(Boolean).join(' or ');
}
// `note:1:36@Launchpad` reads "pad 36 (Launchpad)".
function padLabel(sig){
  const m = /^(note|cc):(\d+):(\d+)(?:@(.+))?$/.exec(sig || '');
  if (!m) return sig || '';
  return `${m[1] === 'note' ? 'pad' : 'knob'} ${m[3]}${m[2] !== '1' ? ' ch ' + m[2] : ''}${m[4] ? ' (' + m[4] + ')' : ''}`;
}
// "The hold runs out" reads "When the hold runs out", "A web call" "When
// a web call"; "OBS crashed" stays.
function lowerFirst(s){
  return /^[A-Z][a-z ]/.test(s || '') ? s[0].toLowerCase() + s.slice(1) : s;
}

// What each variable holds, for the card shown over a variable chip.
const VAR_HELP = {
  reason:'Why it happened. For OBS: crash or freeze. For a destination: what went wrong.',
  hold:'How long the reconnect screen can stay on air.',
  hold_ends_at:'When the reconnect screen runs out, as a Unix time. In Discord, <t:{hold_ends_at}:R> is a live countdown.',
  uses:'How many times this integration has run, this time included. Kept across restarts.',
  text:'What the Edit text step made.',
  down_for:'How long OBS was gone.',
  destination:'The destination\'s name.',
  platform:'The destination\'s platform, like youtube or twitch.',
  previous:'The delay before this change.',
  stopped:'yes when you stopped the stream yourself, no when OBS dropped.',
  protected:'yes when crash protection kept your destinations up.',
  delay:'The delay, written for people. On delay events, the new one.',
  delay_ms:'The delay in milliseconds, for maths and comparisons.',
  delay_state:'on or off.',
  phase:'What the delay is doing: idle, preparing, ready or active.',
  hold_active:'yes while the reconnect screen is on air.',
  hold_left:'Time left on the reconnect screen.',
  destinations_live:'How many destinations are live right now.',
  destinations_total:'How many destinations are switched on.',
  obs_live:'yes while OBS is sending to InstantClone.',
  bitrate:'What OBS is sending, in kbps.',
  channel:'Your Twitch channel.',
  time:'The time now, on this PC.',
  date:'Today\'s date.',
  user:'The viewer\'s display name.',
  user_login:'The viewer\'s login name, in lowercase.',
  user_role:'The viewer\'s role: broadcaster, mod, vip, sub or viewer.',
  message:'The whole chat message.',
  args:'Everything typed after the command.',
  target:'Who the command is about: the name after it (without @), or whoever typed it.',
  arg1:'The first word after the command.',
  arg2:'The second word after the command.',
  arg3:'The third word after the command.',
  body:'What the caller sent. If it is JSON, use {body.field} for one field.',
  query:'The part of the address after the ?.',
  'clip.url':'The link to the clip the Clip step made.',
  'clip.ok':'yes when the Clip step made a clip.',
};
// A variable's description, including the ones named by the user's own
// steps (web answers, counters, remembered values).
function varHelp(name){
  if (VAR_HELP[name]) return VAR_HELP[name];
  let m;
  if ((m = /^(.+)\.json\.(.+)$/.exec(name))) return `The value at ${m[2].split('.').join(' › ')} in the "${m[1]}" web answer.`;
  if ((m = /^(.+)\.status$/.exec(name))) return `The status the "${m[1]}" web request got back, like 200.`;
  if ((m = /^(.+)\.ok$/.exec(name))) return `yes when the "${m[1]}" web request worked, otherwise no.`;
  if ((m = /^(.+)\.body$/.exec(name))) return `Everything the "${m[1]}" web request got back.`;
  if ((m = /^counter\.(.+)$/.exec(name))) return `The "${m[1]}" counter, kept between runs.`;
  if ((m = /^arg([4-9])$/.exec(name))) return `Word number ${m[1]} after the command.`;
  if ((m = /^body\.(.+)$/.exec(name))) return `The ${m[1].split('.').join(' › ')} field of what the caller sent.`;
  return 'A value an earlier step in this run set.';
}

// The card over a hovered or focused variable chip: what it holds and an
// example. One element, reused; a short delay so sweeping across chips
// doesn't flash cards.
const VarTip = {
  el:null, target:null, timer:null,
  want(target){
    if (target === this.target) return;
    clearTimeout(this.timer);
    this.target = target;
    if (!target){ this.hide(); return; }
    const showing = this.el && this.el.classList.contains('on');
    this.timer = setTimeout(() => this.show(target), showing ? 0 : 260);
  },
  show(target){
    if (!target.isConnected) return;
    if (!this.el){
      this.el = document.createElement('div');
      this.el.className = 'ig-vtip';
      this.el.setAttribute('role', 'tooltip');
      this.el.innerHTML = '<code></code><p></p><div class="ig-vtip-eg"><span>e.g.</span><b></b></div><small>Click to insert</small>';
      document.body.appendChild(this.el);
    }
    const name = target.dataset.token, sample = target.dataset.sample || '';
    this.el.querySelector('code').textContent = '{' + name + '}';
    this.el.querySelector('p').textContent = varHelp(name);
    this.el.querySelector('.ig-vtip-eg b').textContent = sample.length > 120 ? sample.slice(0, 120) + '…' : (sample || '(empty)');
    const r = target.getBoundingClientRect(), w = this.el.offsetWidth, h = this.el.offsetHeight;
    const above = r.top - h - 8 >= 8;
    this.el.style.left = Math.max(8, Math.min(r.left + r.width / 2 - w / 2, innerWidth - w - 8)) + 'px';
    this.el.style.top = (above ? r.top - h - 8 : r.bottom + 8) + 'px';
    this.el.classList.toggle('below', !above);
    this.el.classList.add('on');
  },
  hide(){
    clearTimeout(this.timer);
    this.target = null;
    if (this.el) this.el.classList.remove('on');
  },
};
const tokenAt = e => (e.target && e.target.closest) ? e.target.closest('.ig-modal .ig-token[data-token]') : null;
document.addEventListener('pointerover', e => VarTip.want(tokenAt(e)));
document.addEventListener('focusin', e => VarTip.want(tokenAt(e)));
document.addEventListener('pointerdown', () => VarTip.hide(), true);
window.addEventListener('scroll', () => VarTip.hide(), true);

// Sample values for previews: the variables this handler can use.
function varsFor(handler){
  const d = S.data;
  const list = d.vars.global.slice();
  const t = handler && handler.trigger;
  if (t && t.type === 'event'){
    const e = eventOf(t.event);
    // A countdown sample should count down from now.
    if (e) list.unshift(...e.vars.map(v => v.name === 'hold_ends_at' ? {name:v.name, sample:String(Math.floor(Date.now() / 1000) + 120)} : v));
  }
  if (t && (t.type === 'chat_command' || t.type === 'chat_message')) list.unshift(...d.vars.chat);
  if (t && t.type === 'webhook') list.unshift(...d.vars.web);
  walk(handler && handler.steps, s => {
    if (s.type === 'http'){
      const n = (s.params.save_as || '').trim() || 'response';
      // Once "Send request" ran, the real answer is the sample.
      const tried = TRIES.get(uidOf(s));
      const got = tried && tried.status ? tried : null;
      list.push({name:n + '.status', sample:got ? String(got.status) : '200'},
        {name:n + '.ok', sample:got ? (got.status >= 200 && got.status < 300 ? 'yes' : 'no') : 'yes'},
        {name:n + '.body', sample:got ? got.body.slice(0, 200) : '{...}'});
      if (got && got.json !== undefined){
        jsonLeaves(got.json).filter(l => l.usable && (n + '.json.' + l.path).length <= 64).slice(0, 60)
          .forEach(l => list.push({name:`${n}.json.${l.path}`, sample:l.text}));
      }
    }
    if (s.type === 'clip') list.push({name:'clip.url', sample:'https://clips.twitch.tv/BraveSnipe'});
    if (s.type === 'counter' && s.params.name) list.push({name:'counter.' + s.params.name, sample:'2'});
    if (s.type === 'set_var' && s.params.name) list.push({name:s.params.name, sample:s.params.value || ''});
    if (s.type === 'edit_text'){
      const m = {};
      list.forEach(v => { m[v.name] = v.sample; });
      list.push({name:(s.params.save_as || '').trim() || 'text', sample:editTextSample(s, m)});
    }
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
// `escape`, when given, is applied to inserted values only, like the
// server does for web requests (see template::render_escaped).
function renderSample(text, vars, escape){
  return String(text || '').replace(/\{\{([A-Za-z0-9_.]+)\}\}|\{([A-Za-z0-9_.]{1,64})(?:\|([^}]*))?\}/g,
    (all, escaped, name, fallback) => {
      if (escaped) return '{' + escaped + '}';
      const v = vars[name] == null ? '' : String(vars[name]);
      if (fallback !== undefined && (v === '' || v === '0')) return fallback;
      return escape ? escape(v) : v;
    });
}
// Mirrors template::edit_text, for previews. "Pick one" previews its
// first choice.
function editText(op, input, a, b){
  switch (op){
    case 'first_line': return (input.split('\n').map(l => l.trim()).find(Boolean)) || '';
    case 'between': {
      const from = a ? input.indexOf(a) : 0;
      if (from < 0) return '';
      const rest = input.slice(from + a.length), to = b ? rest.indexOf(b) : rest.length;
      return to < 0 ? '' : rest.slice(0, to).trim();
    }
    case 'replace': return a ? input.split(a).join(b) : input;
    case 'cut': {
      const max = Math.max(1, parseInt(a, 10) || 100), chars = [...input];
      return chars.length <= max ? input : chars.slice(0, max).join('').trimEnd() + '…';
    }
    case 'round': {
      const n = Number(input.trim());
      return input.trim() && Number.isFinite(n) ? n.toFixed(Math.min(6, parseInt(a, 10) || 0)) : input;
    }
    case 'digits': return /^-?\d+$/.test(input.trim()) ? input.trim().replace(/\B(?=(\d{3})+(?!\d))/g, ',') : input;
    case 'upper': return input.toUpperCase();
    case 'lower': return input.toLowerCase();
    case 'random': return (input.split(input.includes('\n') ? '\n' : '|').map(p => p.trim()).find(Boolean)) || '';
  }
  return input;
}
// Mirrors runner::compare, so previews follow checks like a run does.
function compareSample(left, op, right){
  const l = String(left).trim().toLowerCase(), r = String(right).trim().toLowerCase();
  const blank = l === '' || l === '0' || l === 'no';
  const nums = l !== '' && r !== '' && Number.isFinite(+l) && Number.isFinite(+r);
  switch (op){
    case 'is': return l === r;
    case 'is_not': return l !== r;
    case 'contains': return l.includes(r);
    case 'not_contains': return !l.includes(r);
    case 'starts_with': return l.startsWith(r);
    case 'empty': return blank;
    case 'not_empty': return !blank;
    case 'greater': return nums && +l > +r;
    case 'less': return nums && +l < +r;
  }
  return false;
}
// The first step of `type` a run would reach with these values: undefined
// when none, null when a Stop comes first.
function reachedStep(steps, vars, type){
  for (const s of steps || []){
    if (s.type === 'stop') return null;
    if (s.type === type) return s;
    if (s.type !== 'if') continue;
    const pass = compareSample(renderSample(s.params.left, vars), s.params.op, renderSample(s.params.right, vars));
    const found = reachedStep(pass ? s.then : s.else, vars, type);
    if (found !== undefined) return found;
  }
  return undefined;
}
function editTextSample(s, vars){
  const p = s.params;
  return editText(p.op, renderSample(p.input, vars), renderSample(p.a, vars), renderSample(p.b, vars));
}
// Discord writes `<t:1790000000:R>` as a time that keeps itself current
// ("in 2 minutes"). Previews show it the same way; `html` is escaped text.
function discordTimes(html){
  return html.replace(/&lt;t:(\d{1,12})(?::([tTdDfFR]))?&gt;/g, (all, secs, style) => {
    const at = +secs * 1000;
    let shown;
    if (style === 'R'){
      const diff = Math.round((at - Date.now()) / 60000);
      shown = diff === 0 ? 'now' : diff > 0 ? `in ${plural(diff, 'minute')}` : `${plural(-diff, 'minute')} ago`;
    } else shown = new Date(at).toLocaleString([], style === 't' || style === 'T' ? {timeStyle:'short'} : {dateStyle:'medium', timeStyle:'short'});
    return `<span class="ig-ts">${esc(shown)}</span>`;
  });
}
// Mirrors template::url_component: only unreserved characters stay.
// A pasted command from a chat bot rather than a plain address.
const BOT_SYNTAX = /\$\(|\$\{|\{readapi\.|\$readapi\(/i;
const urlComponent = v => encodeURIComponent(v).replace(/[!'()*]/g, c => '%' + c.charCodeAt(0).toString(16).toUpperCase());
const jsonStringContent = v => JSON.stringify(v).slice(1, -1);
const oneLine = v => v.replace(/[\u0000-\u001f\u007f]/g, ' ');
// An address as safe to show on stream: scheme and host only, since the
// path and query often carry an API key or a webhook secret.
function maskUrl(url){
  const m = /^(https?:\/\/[^/?#\s]+)(\S*)/i.exec(String(url || '').trim());
  if (!m) return String(url || '').trim() ? 'an address' : '';
  return m[1] + (m[2] && m[2] !== '/' ? '/…' : '');
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
    if (a.nodeValue !== b.nodeValue){
      const was = a.nodeValue;
      a.nodeValue = b.nodeValue;
      if (ctx.before) tick(a.parentElement, was, b.nodeValue);
    }
    return;
  }
  const tag = a.tagName;
  // Form fields: only push a value the model changed, so whatever the user
  // typed into a field the model doesn't track yet survives a redraw.
  const oldValue = a.getAttribute('value'), oldChecked = a.hasAttribute('checked');
  const oldSelected = tag === 'SELECT' ? selectedOf(a) : null;
  syncAttrs(a, b);
  if (busyEls.has(a)){ a.classList.add('is-busy'); a.disabled = true; a.setAttribute('aria-busy', 'true'); }
  if (decoders.has(a)) a.classList.add('ig-decoding');
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

// ---------------------------------------------------------------- motion details

// A changed number or short value rolls in instead of snapping: up when a
// number grew, down when it shrank. Only for elements marked data-tick.
function tick(el, was, now){
  if (!el || !el.hasAttribute('data-tick')) return;
  const a = parseFloat(was), b = parseFloat(now);
  const from = !isNaN(a) && !isNaN(b) && b < a ? '-45%' : '45%';
  el.animate([
    {transform:`translateY(${from})`, opacity:0, filter:'blur(2px)'},
    {transform:'none', opacity:1, filter:'blur(0)'},
  ], {duration:260, easing:EASE});
}

// A secret field decodes letter by letter when shown and folds back into
// dots, from the end, when hidden. Each letter scrambles for a moment, then
// settles. It is drawn on a layer over the field (which morph owns and
// leaves untouched); one animation frame loop, writing only what changed,
// and typing into the field ends it at once.
const SCRAMBLE = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789#%&*+=?';
const DECODE_MAX = 300;          // longer values just switch
const decoders = new WeakMap();  // field -> stop()

function decodeField(control, reveal){
  if (!control || reduced()) return;
  const host = control.closest('.ig-secret-field');
  const chars = [...(control.value || '')];
  if (!host || !chars.length || chars.length > DECODE_MAX) return;
  if (decoders.has(control)) decoders.get(control)();
  const layer = decodeLayer(control, host);
  const cells = [];
  for (const ch of chars){
    if (ch === '\n'){ layer.appendChild(document.createElement('br')); continue; }
    const span = layer.appendChild(document.createElement('span'));
    cells.push({span, ch, state:-1});
  }
  layer.scrollTop = control.scrollTop;
  layer.scrollLeft = control.scrollLeft;
  const n = cells.length;
  const step = Math.min(24, 480 / n), settle = 130;
  const total = (n - 1) * step + settle;
  const started = performance.now();
  let frame = 0;
  const stop = () => {
    cancelAnimationFrame(frame);
    control.removeEventListener('input', stop);
    control.classList.remove('ig-decoding');
    decoders.delete(control);
    layer.remove();
  };
  const draw = now => {
    const t = now - started;
    // Scrambled letters change 20 times a second, not every frame.
    const beat = Math.floor(t / 50);
    cells.forEach((c, i) => {
      const at = (reveal ? i : n - 1 - i) * step;
      const state = t < at ? (reveal ? 0 : 2) : t < at + settle ? 1 : (reveal ? 2 : 0);
      const text = state === 0 ? '•' : state === 2 ? c.ch : SCRAMBLE[(i * 7 + beat * 13) % SCRAMBLE.length];
      if (c.span.textContent !== text) c.span.textContent = text;
      if (c.state !== state){ c.state = state; c.span.className = 's' + state; }
    });
    if (t < total) frame = requestAnimationFrame(draw); else stop();
  };
  control.classList.add('ig-decoding');
  control.addEventListener('input', stop);
  decoders.set(control, stop);
  frame = requestAnimationFrame(draw);
}

// The layer the decode draws on: the field's exact box and type metrics,
// so letters land where the field's own letters are.
function decodeLayer(control, host){
  const cs = getComputedStyle(control);
  const box = control.getBoundingClientRect(), at = host.getBoundingClientRect();
  const layer = document.createElement('div');
  layer.className = 'ig-decode' + (control.tagName === 'TEXTAREA' ? ' area' : '');
  layer.setAttribute('aria-hidden', 'true');
  layer.setAttribute('data-persist', '');
  Object.assign(layer.style, {
    left:(box.left - at.left) + 'px', top:(box.top - at.top) + 'px', width:box.width + 'px', height:box.height + 'px',
  });
  for (const prop of ['fontFamily', 'fontSize', 'fontWeight', 'letterSpacing', 'lineHeight', 'paddingTop', 'paddingRight',
    'paddingBottom', 'paddingLeft', 'borderTopWidth', 'borderRightWidth', 'borderBottomWidth', 'borderLeftWidth']){
    layer.style[prop] = cs[prop];
  }
  return host.appendChild(layer);
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

// A toast with Undo, for actions that are easy to regret.
function undoToast(text, undo){
  const rail = $('toast-rail');
  if (!rail){ toast(text, 'ok'); return; }
  const el = document.createElement('div');
  el.className = 'ic-toast ok ig-undo';
  el.innerHTML = `<span class="ic-toast-icon">✓</span><span class="ig-undo-text"></span><button type="button" class="ig-undo-btn">Undo</button>
    <i class="ig-undo-bar" style="animation-duration:${UNDO_MS}ms"></i>`;
  el.querySelector('.ig-undo-text').textContent = text;
  let used = false;
  const leave = () => {
    if (!el.isConnected || el.classList.contains('leaving')) return;
    el.classList.add('leaving');
    setTimeout(() => el.remove(), 350);
  };
  el.querySelector('button').addEventListener('click', async () => {
    if (used) return;
    used = true;
    leave();
    await undo();
  });
  rail.appendChild(el);
  // Pointing at it holds the countdown (the bar pauses in CSS too).
  let left = UNDO_MS, since = Date.now(), timer = setTimeout(leave, left);
  el.addEventListener('pointerenter', () => { clearTimeout(timer); left -= Date.now() - since; });
  el.addEventListener('pointerleave', () => { since = Date.now(); timer = setTimeout(leave, Math.max(left, 600)); });
}
const UNDO_MS = 8000;

// A secret field (topic, webhook link, headers, secret address): hidden
// like a password until the user asks to see it.
function secretField(id, control, label){
  const shown = S.revealed.has(id);
  return `<div class="dff ig-secret-field${shown ? ' shown' : ''}" data-field="${esc(id)}"><label>${label}
    <button type="button" class="ig-reveal" data-act="reveal" data-field="${esc(id)}" aria-pressed="${shown}">
      <span class="ig-eye">${svg('eye')}${svg('eyeOff')}</span><span data-tick>${shown ? 'Hide' : 'Show'}</span></button></label>${control}</div>`;
}

function parseJson(text){
  try {
    const v = JSON.parse(text);
    return v !== null && typeof v === 'object' ? v : undefined;
  } catch(_){ return undefined; }
}
// Every value in a JSON answer, with the path a variable uses to reach it.
// `usable`: the path fits a variable name (letters, digits, _).
function jsonLeaves(root){
  const out = [];
  const visit = (v, path) => {
    if (out.length >= 200) return;
    if (v !== null && typeof v === 'object'){
      (Array.isArray(v) ? v.slice(0, 25).map((x, i) => [i, x]) : Object.entries(v)).forEach(([k, x]) => visit(x, path.concat(k)));
      return;
    }
    out.push({path:path.join('.'), text:v === null ? '' : String(v), usable:path.length > 0 && path.every(k => /^[A-Za-z0-9_]+$/.test(String(k)))});
  };
  visit(root, []);
  return out;
}
// The answer as a tree; every value is a button that picks its variable.
function jsonTreeHtml(root, saveAs){
  let budget = 400;
  const node = (v, path, key, depth, index) => {
    if (budget-- <= 0) return '';
    const label = key === null ? '' : `<span class="k">${esc(key)}</span>`;
    // The first level eases in one row after another.
    const order = depth === 1 ? ` style="--i:${index}"` : '';
    if (v !== null && typeof v === 'object'){
      const all = Array.isArray(v) ? v.length : Object.keys(v).length;
      const entries = Array.isArray(v) ? v.slice(0, 25).map((x, i) => [i, x]) : Object.entries(v).slice(0, 60);
      const kids = entries.map(([k, x], i) => node(x, path.concat(k), String(k), depth + 1, i)).join('')
        + (all > entries.length ? `<div class="ig-jmore">… ${all - entries.length} more</div>` : '');
      if (key === null) return kids || '<div class="muted">(empty)</div>';
      return `<details class="ig-jnode"${order} ${depth < 2 ? 'open' : ''}><summary>${svg('chev')}${label}<span class="ig-jcount">${Array.isArray(v) ? `[${all}]` : `{${all}}`}</span></summary><div class="ig-jkids">${kids}</div></details>`;
    }
    const name = `${saveAs}.json.${path.join('.')}`;
    const usable = path.every(k => /^[A-Za-z0-9_]+$/.test(String(k))) && name.length <= 64;
    const text = v === null ? 'null' : String(v);
    const shown = typeof v === 'string' ? `"${text.length > 80 ? text.slice(0, 80) + '…' : text}"` : text;
    const kind = v === null ? 'null' : typeof v;
    return usable
      ? `<button class="ig-jleaf"${order} data-act="b-pick" data-token="${esc(name)}" data-sample="${esc(text.slice(0, 200))}" title="{${esc(name)}}">${label}<span class="v ${kind}">${esc(shown)}</span><span class="ig-juse">Use</span></button>`
      : `<div class="ig-jleaf off"${order} title="This name has characters a variable can't reach">${label}<span class="v ${kind}">${esc(shown)}</span></div>`;
  };
  return node(root, [], null, 0, 0);
}

// ---------------------------------------------------------------- try a web request

// What "Send request" got, per web request step (by uidOf). Cleared
// whenever an editor opens: the steps are fresh objects each time.
const TRIES = new Map();

function tryEntry(s){
  let e = TRIES.get(uidOf(s));
  if (!e){ e = {inputs:{}}; TRIES.set(uidOf(s), e); }
  return e;
}

// "Try it": values for the variables the request uses, the button, and
// what came back, where every value can be picked. `useLabel` names what
// the picked value's main button does in this editor ('' for none).
function tryHtml(s, handler, useLabel){
  const t = tryEntry(s);
  const used = [...new Set(['url', 'headers', 'body'].flatMap(f =>
    [...String(s.params[f] || '').matchAll(/\{([A-Za-z0-9_.]{1,64})(?:\|[^}]*)?\}/g)].map(m => m[1])))];
  const samples = sampleMap(handler);
  const inputs = used.map(n => `<div class="dff"><label class="mono">${esc(n)}</label><input class="ic-input" data-bind="try" data-name="${esc(n)}"
    value="${esc(n in t.inputs ? t.inputs[n] : (samples[n] || ''))}" placeholder="a value to try"></div>`).join('');
  return `<div class="ig-try">
    <div class="ig-try-head"><span class="ic-label">Try it</span>
      <button class="ic-btn ic-btn-primary ig-small" data-act="b-try" ${s.params.url ? '' : 'disabled'}>${svg('play', ' class="ig-btn-ic"')}Send request</button></div>
    ${used.length ? `<div class="muted">Try it with:</div><div class="ig-try-inputs">${inputs}</div>` : ''}
    ${tryResultHtml(s, t, useLabel)}
  </div>`;
}

function tryResultHtml(s, t, useLabel){
  if (t.error) return warnHtml(esc(t.error));
  if (!t.status) return '<div class="muted">Sends the request for real and shows the answer. Then pick what to use from it.</div>';
  const n = (s.params.save_as || '').trim() || 'response';
  const ok = t.status >= 200 && t.status < 300;
  const head = `<div class="ig-try-meta"><span class="ig-status-pill ${ok ? 'ok' : 'bad'}">${t.status}</span>
    <span class="muted">${t.ms} ms${t.truncated ? ' · long answer, cut for display' : ''}</span></div>`;
  const picked = t.picked ? `<div class="ig-picked" data-key="picked-${esc(t.picked.token)}">
    <div class="ig-picked-what"><code>{${esc(t.picked.token)}}</code><span title="${esc(t.picked.sample)}">${esc(t.picked.sample) || '<i>empty</i>'}</span></div>
    <div class="ig-picked-actions">${useLabel ? `<button class="ic-btn ic-btn-primary ig-small" data-act="b-use-reply">${useLabel}</button>` : ''}
    <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="{${esc(t.picked.token)}}">Copy</button></div></div>` : '';
  if (t.json === undefined){
    return `${head}${picked}<div class="ig-try-text">${t.body ? esc(t.body.slice(0, 3000)) : '<span class="muted">The answer was empty.</span>'}</div>
      ${t.body ? `<button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="b-pick" data-token="${esc(n)}.body" data-sample="${esc(t.body.slice(0, 200))}">Use the whole answer</button>` : ''}`;
  }
  return `${head}${picked}<div class="muted">Click the value you want to use.</div><div class="ig-jtree" data-key="answer-${t.answer}">${jsonTreeHtml(t.json, n)}</div>`;
}

// Send the request with the typed values, escaped exactly as a run would.
async function tryRequest(s, handler){
  if (!s) return;
  const t = tryEntry(s);
  const vars = Object.assign(sampleMap(handler), t.inputs);
  const jsonBody = /^\s*[{[]/.test(s.params.body || '');
  const r = await api('/integrations/fetch', {
    method: s.params.method || 'POST',
    url: renderSample(s.params.url, vars, urlComponent).trim(),
    headers: renderSample(s.params.headers, vars, oneLine),
    body: renderSample(s.params.body, vars, jsonBody ? jsonStringContent : null),
  });
  t.picked = null;
  if (!r.ok){ t.error = r.error || 'The request failed'; t.status = 0; return; }
  Object.assign(t, {error:'', status:r.status, ms:Math.round(r.ms), body:r.body, truncated:r.truncated,
    json:parseJson(r.body), answer:(t.answer || 0) + 1});
}

function firstStep(handler, type){
  let found = null;
  walk(handler && handler.steps, s => { if (!found && s.type === type) found = s; });
  return found;
}

// The "⋯" menu of a card or row: everything you can do to it, Delete
// included, without opening it. Lives on <body> so no card clips it.
const Menu = {
  el:null, id:null, btn:null,
  toggle(btn, id){
    if (this.id === id){ this.close(); return; }
    this.open(btn, id);
  },
  open(btn, id){
    this.close();
    const i = find(id);
    if (!i) return;
    const el = document.createElement('div');
    el.className = 'ig-menu';
    el.setAttribute('role', 'menu');
    el.innerHTML = `<button role="menuitem" data-act="edit" data-id="${esc(id)}" style="--i:0">${svg('pencil')}Edit</button>
      <button role="menuitem" data-act="duplicate" data-id="${esc(id)}" style="--i:1">${svg('copy')}Duplicate</button>
      <button role="menuitem" data-act="share-one" data-id="${esc(id)}" style="--i:2">${svg('link')}Share as a recipe</button>
      <div class="ig-menu-sep" role="separator"></div>
      <button role="menuitem" class="danger" data-act="menu-delete" data-id="${esc(id)}" style="--i:3">${svg('x')}Delete</button>`;
    document.body.appendChild(el);
    el.addEventListener('click', onClick);
    el.addEventListener('keydown', e => this.keys(e));
    const r = btn.getBoundingClientRect(), w = el.offsetWidth, h = el.offsetHeight;
    el.style.left = Math.max(8, Math.min(r.right - w, innerWidth - w - 8)) + 'px';
    el.style.top = (r.bottom + 6 + h > innerHeight - 8 ? r.top - h - 6 : r.bottom + 6) + 'px';
    this.el = el;
    this.id = id;
    this.btn = btn;
    btn.setAttribute('aria-expanded', 'true');
    el.querySelector('button').focus({preventScroll:true});
    document.addEventListener('pointerdown', this.outside, true);
    window.addEventListener('scroll', this.dismiss, true);
    window.addEventListener('resize', this.dismiss);
  },
  close(){
    if (!this.el) return;
    const el = this.el, btn = this.btn, hadFocus = el.contains(document.activeElement);
    this.el = this.id = this.btn = null;
    document.removeEventListener('pointerdown', this.outside, true);
    window.removeEventListener('scroll', this.dismiss, true);
    window.removeEventListener('resize', this.dismiss);
    el.classList.add('closing');
    setTimeout(() => el.remove(), reduced() ? 0 : 120);
    if (btn && btn.isConnected){
      btn.setAttribute('aria-expanded', 'false');
      if (hadFocus) btn.focus({preventScroll:true});
    }
  },
  keys(e){
    const items = [...this.el.querySelectorAll('[role="menuitem"]')];
    const at = items.indexOf(document.activeElement);
    if (e.key === 'Escape'){ e.preventDefault(); this.close(); }
    else if (e.key === 'ArrowDown'){ e.preventDefault(); items[(at + 1) % items.length].focus(); }
    else if (e.key === 'ArrowUp'){ e.preventDefault(); items[(at - 1 + items.length) % items.length].focus(); }
    else if (e.key === 'Tab') this.close();
  },
  outside: e => { if (Menu.el && !Menu.el.contains(e.target) && !e.target.closest('[data-act="menu"]')) Menu.close(); },
  dismiss: e => { if (Menu.el && !(e && e.target && Menu.el.contains(e.target))) Menu.close(); },
};

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
        <b>InstantClone</b><span class="ig-app">APP</span><div class="ig-txt">${discordTimes(esc(ping + text))}</div></span></div>`;
    }
    case 'chat':
      return `<div class="${cls} ig-pv-chat">${ask ? `<div><span class="u1">viewer</span>: ${esc(ask)}</div>` : ''}
        <div><span class="u2">${esc(chatAuthor(step))}</span>: ${esc(text)}</div></div>`;
    case 'phone':
      return `<div class="${cls} ig-pv-phone"><span class="ig-av"></span><span class="ig-pv-col">
        <small>INSTANTCLONE · now</small><b>${esc(renderSample(step.params.title, vars) || 'InstantClone')}</b><div>${esc(text)}</div></span></div>`;
    case 'overlay':
      return `<div class="${cls} ig-pv-overlay"><div class="ig-pv-alert">
        ${step.params.title ? `<small>${esc(renderSample(step.params.title, vars))}</small>` : ''}<b>${esc(text || 'Something on stream')}</b><i></i></div></div>`;
    case 'marker':
      return `<div class="${cls} ig-pv-marker"><span class="tag">${esc(text || 'Marker')} · 1:02:14</span>
        <div class="bar"><i></i><b style="left:22%"></b><b style="left:48%"></b><b style="left:80%"></b></div></div>`;
    case 'http': {
      const body = text.trim();
      let shown = body ? body : `${step.params.method || 'POST'} ${maskUrl(step.params.url) || '(address to set)'}`;
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
  // Secret fields the user chose to show (by field id). Hidden by default:
  // a dashboard is often on a stream.
  revealed:new Set(),
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
  const quiet = i.enabled && quietNow(i.quiet);
  return {
    h, step, flag, failing, where,
    dc: k ? k.c : 'var(--accent)',
    icon: PRESET_ICON[i.preset] || (k ? k.icon : 'steps'),
    last: quiet ? 'quiet until ' + i.quiet.to : st && st.last_ms ? fmtAgo(st.last_ms) : 'never ran',
    hist: histHtml(st),
    summary: text ? renderSample(text, sampleMap(h)) : (h ? triggerSentence(h.trigger) : ''),
  };
}

// Whether quiet hours (`{from:'23:00', to:'08:00'}`) cover this minute.
// Mirrors model::QuietHours::contains.
function quietNow(q){
  if (!q || !q.from || !q.to) return false;
  const min = t => { const [h, m] = t.split(':').map(Number); return h * 60 + m; };
  const d = new Date(), now = d.getHours() * 60 + d.getMinutes(), from = min(q.from), to = min(q.to);
  return from <= to ? now >= from && now < to : now >= from || now < to;
}

// The last runs as a row of dots, oldest first: a card that fails now and
// then shows it before it is "Failing".
function histHtml(st){
  const recent = (st && st.recent) || [];
  if (!recent.length) return '';
  const bad = recent.filter(r => r === 'failed').length;
  const label = `Last ${plural(recent.length, 'run')}${bad ? `, ${bad} failed` : ', all fine'}`;
  return `<span class="ig-hist" title="${label}" aria-label="${label}">${recent.map(r => `<i class="${esc(r)}"></i>`).join('')}</span>`;
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
    : `<span class="ig-num" data-tick>${on}</span> on${failing.length ? ` · <span class="ig-num" data-tick>${failing.length}</span> need${failing.length === 1 ? 's' : ''} attention` : ''}. Each card shows exactly what it sends.`;
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
    ${pill('twitch', twitchOn ? (twitchLive ? 'ok' : 'warn') : '', 'Twitch', twitchOn ? (twitchLive ? 'connected' : 'chat ' + t.main.chat) : (t.available ? 'Connect' : 'Unavailable'))}
    ${pill('phone', phoneOn ? 'ok' : '', 'Phone', phoneOn ? 'connected' : 'Connect')}
  </div>`;
}

function toolbarHtml(){
  const counts = {all:S.data.integrations.length};
  S.data.integrations.forEach(i => { const c = categoryOf(i); counts[c] = (counts[c] || 0) + 1; });
  const tab = (id, label) => `<button class="sub-tab${S.filter === id ? ' on' : ''}" role="tab" aria-selected="${S.filter === id}"
    data-act="filter" data-f="${id}">${label} <span class="ig-count" data-tick>${counts[id] || 0}</span></button>`;
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
function moreHtml(i){
  return `<button class="ic-btn-tiny ig-more" data-act="menu" data-id="${esc(i.id)}" aria-label="More for ${esc(i.name)}"
    aria-haspopup="menu" aria-expanded="${Menu.id === i.id}">${svg('dots', ' stroke-width="3"')}</button>`;
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
        ${previewHtml(c.step, c.h, false)}${flagHtml(c.flag)}${c.hist}
      </div>
      <div class="ig-card-foot">
        <span class="dcard-icon">${svg(c.icon)}</span>
        <div class="dcard-id"><div class="dcard-name">${esc(i.name)}</div>
          <div class="dcard-host">${esc(c.where)} · ${esc(c.last)}</div></div>
        ${moreHtml(i)}${switchHtml(i)}
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
        ${c.hist}${c.flag ? flagHtml(c.flag) : `<span class="ig-row-last">${esc(c.last)}</span>`}
        ${switchHtml(i)}
        <button class="ic-btn-tiny" data-act="edit" data-id="${esc(i.id)}" aria-label="Edit ${esc(i.name)}">${svg('pencil')}</button>
        ${moreHtml(i)}
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
    const open = find(r.integration_id)
      ? ` data-act="edit" data-id="${esc(r.integration_id)}" role="button" tabindex="0" title="Open ${esc(r.name)}"` : '';
    return `<div class="ig-act-row${open ? ' link' : ''}" data-key="a-${esc(key)}"${open}><span class="t">${esc(fmtClock(r.at_ms))}</span>
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
  if (want) glow(id);
  const r = await api('/integrations/toggle', {id, enabled:want});
  if (r.ok) return;
  i.enabled = !want;
  render();
  toast(r.error || 'Could not switch it', 'err', 5000);
  if (want) openEditor(i, true);
}

// A card that just switched on gives one soft pulse of its colour.
function glow(id){
  const card = document.querySelector(`[data-key="c-${CSS.escape(id)}"], [data-key="r-${CSS.escape(id)}"]`);
  if (!card || reduced()) return;
  const color = getComputedStyle(card).getPropertyValue('--dc').trim() || 'currentColor';
  card.animate([
    {boxShadow:`0 0 0 0 color-mix(in oklch, ${color} 55%, transparent)`},
    {boxShadow:`0 0 0 10px color-mix(in oklch, ${color} 0%, transparent)`},
  ], {duration:620, easing:'cubic-bezier(.2,.7,.2,1)'});
}

async function addFrom(body, openAfter){
  const fromCatalog = Modal.kind === 'catalog';
  const r = await api('/integrations/add', body);
  if (!r.ok){ toast(r.error || 'Could not add it', 'err', 5000); return; }
  await load();
  const added = (r.ids || []).map(find).filter(Boolean);
  const ids = added.map(i => i.id);
  const unfinished = added.find(i => !i.enabled);
  if (openAfter && unfinished){
    // Going back from here means "not this one": the draft goes again.
    const back = fromCatalog ? {title:'Back to the catalog, without adding it', go:async () => {
      await deleteIds([unfinished.id]);
      Catalog.reopen();
    }} : null;
    openEditor(unfinished, true, {justAdded:true, back});
    return;
  }
  undoToast(added.length > 1 ? `Added ${added.length} integrations` : `Added ${added[0] ? added[0].name : ''}`, async () => {
    await deleteIds(ids);
    if (Modal.kind === 'catalog') Catalog.render();
  });
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
  const i = find(id);
  // Kept so Undo can put it back exactly, in the same place.
  const snapshot = i ? clone(i) : null;
  const at = i ? S.data.integrations.indexOf(i) : -1;
  const r = await api('/integrations/delete', {id});
  if (!r.ok){ toast(r.error || 'Could not delete it', 'err'); return; }
  // Only close the editor that asked; the user may have moved on.
  if (kind && Modal.kind === kind) Modal.close(true);
  await load();
  if (!snapshot){ toast('Deleted', 'ok'); return; }
  undoToast(`Deleted ${snapshot.name}`, async () => {
    const back = await api('/integrations/save', Object.assign(snapshot, {at}));
    if (!back.ok) toast(back.error || 'Could not bring it back', 'err', 5000);
    await load();
  });
}

async function deleteIds(ids){
  for (const id of ids) await api('/integrations/delete', {id});
  await load();
}

// `switchOn`: it should end up on, and waits for a detail first.
function openEditor(i, switchOn, opts){
  if (i.preset) Editor.open(i, switchOn, opts);
  else Builder.open(i, Object.assign({switchOn}, opts));
}

// ---------------------------------------------------------------- modal shell

// One modal shell at a time, built on the destination editor's. Opening
// another while one is up swaps the content in place (no second backdrop).
// Escape, the backdrop and ✕ close it; `onClose` lets an editor hold the
// close to ask about unsaved changes, `after` runs once it really closes.
const Modal = {
  el:null, form:null, kind:null, onClose:null, after:null, opener:null, swapFrom:null,
  // `back`: {go, title, keepsEdits} for a view opened from another one.
  // `pending`: what the unsaved-changes bar's Discard should finish.
  back:null, pending:null,
  open(kind, width, height){
    Menu.close();
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
    this.back = null;
    this.pending = null;
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
    if (!force){
      this.pending = 'close';
      if (this.onClose && this.onClose() === false) return;
    }
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
    // Shown secrets hide again: the next time could be on stream.
    S.revealed.clear();
    VarTip.hide();
    Keys.stop();
    document.removeEventListener('keydown', onModalKey);
    el.classList.add('closing');
    el.inert = true;
    setTimeout(() => el.remove(), reduced() ? 0 : 180);
    if (opener && opener.isConnected && typeof opener.focus === 'function') opener.focus({preventScroll:true});
    if (!S.visible) clearInterval(S.pollTimer);
  },
  // Back to the view this one was opened from. Edits that travel back
  // with it skip the unsaved-changes question; with nowhere to go back to,
  // it closes (and `after` returns to the opener).
  goBack(){
    if (!this.back){ this.close(); return; }
    if (!this.back.keepsEdits){
      this.pending = 'back';
      if (this.onClose && this.onClose() === false) return;
    }
    this.leaveBack();
  },
  leaveBack(){
    const go = this.back.go;
    this.back = null;
    this.pending = null;
    go();
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
  const back = Modal.back || Modal.after
    ? `<button type="button" class="dest-form-x ig-back" data-act="modal-back" aria-label="Back" title="${esc((Modal.back && Modal.back.title) || 'Back')}">${svg('back')}</button>` : '';
  return `<div class="dest-form-head">${back}
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
function footHtml(d, dirty, extra, canSave){
  return `<div class="dest-form-foot">
    ${d.id ? '<button class="ic-btn ic-btn-ghost ig-del" data-act="delete">Delete</button>' : ''}
    <div class="dest-form-msg muted ig-foot-msg">${d.id ? lastRunHtml(d) : '<span class="ig-last">New integration</span>'}${dirty ? '<span class="ig-unsaved">Unsaved</span>' : ''}</div>
    ${extra || ''}
    <button class="ic-btn ic-btn-primary" data-act="save" ${dirty || canSave || !d.id ? '' : 'disabled'} title="Save (Ctrl+S)">Save</button>
  </div>`;
}
function runLogHtml(steps){
  if (!steps || !steps.length) return '<div class="muted">No steps ran.</div>';
  return `<div class="ig-run">${steps.map((s, i) => `<div style="--i:${i}"><span class="t">${(s.at_ms / 1000).toFixed(2)} s</span>
    <span class="${esc(s.status)}">${s.status === 'ok' ? '✓' : s.status === 'failed' ? '✕' : '–'} ${esc(s.label)}</span>
    <span class="d" title="${esc(s.detail)}">${esc(s.detail)}</span></div>`).join('')}</div>`;
}
function warnHtml(text){
  return `<div class="lan-warn ig-inline-warn"><span class="lw-ic">⚠</span><span>${text}</span></div>`;
}

// The Save button turns into "Saved" with its check popping in, for a
// beat, before the editor closes.
function savedBeat(){
  const btn = q('.dest-form-foot [data-act="save"]');
  if (!btn || reduced()) return Promise.resolve();
  busyEls.delete(btn);
  btn.classList.remove('is-busy');
  btn.classList.add('ig-saved');
  btn.innerHTML = `${svg('check', ' stroke-width="3"')}Saved`;
  return new Promise(done => setTimeout(done, 380));
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
  if (Modal.kind === kind){
    await savedBeat();
    if (Modal.kind === kind) Modal.close(true);
  }
  await load();
  return true;
}

// ---------------------------------------------------------------- catalog

const Catalog = {
  cat:'rec', added:null,
  open(){
    this.cat = 'rec';
    this.added = null;
    this.reopen();
  },
  // Back from something opened here: same category as before.
  reopen(){
    Modal.open('catalog', 1040, 'min(86vh,820px)');
    this.render();
  },
  render(){
    const cat = S.data.catalog;
    const counts = id => id === 'packs' ? cat.packs.length
      : cat.presets.filter(p => id === 'rec' ? p.recommended : p.category === id).length;
    const nav = [['rec', 'Recommended'], ['alerts', 'Alerts'], ['chat', 'Chat'], ['auto', 'Automation'], ['packs', 'Packs']]
      .map(([id, label]) => `<button class="${this.cat === id ? 'on' : ''}" data-act="cat" data-cat="${id}" aria-current="${this.cat === id}">${label}<small data-tick>${counts(id)}</small></button>`).join('');
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
  d:null, sel:0, chip:null, run:null, dirty:false, moreVars:false, back:null, justAdded:false,
  // opts: {back} (see Modal.back); {justAdded} when the catalog just made
  // it as a draft, which closing keeps and Back removes again.
  open(i, switchOn, opts){
    opts = opts || {};
    this.d = withHandler(clone(i));
    this.back = opts.back || null;
    this.justAdded = !!opts.justAdded;
    // Waiting for one detail: shown switched on, so saving once it is
    // complete also turns it on.
    this.dirty = !!switchOn && !this.d.enabled && !this.justAdded;
    if (switchOn) this.d.enabled = true;
    this.sel = Math.max(0, this.d.handlers.findIndex(h => h.enabled));
    // Missing a detail: open straight on the chip that fixes it.
    const known = id => S.data.connections.discord.some(c => c.id === id);
    const steps = allSteps(this.d);
    const keyless = this.d.handlers.some(h => h.trigger.type === 'shortcut' && !h.trigger.hotkey && !h.trigger.midi);
    this.chip = steps.some(s => s.type === 'discord' && !known(s.params.connection)) ? 'where'
      : steps.some(s => s.type === 'http' && !s.params.url) ? 'url' : keyless ? 'key' : null;
    this.run = null;
    this.moreVars = false;
    this.converted = null;
    TRIES.clear();
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
    Modal.back = this.back;
    Modal.onClose = () => {
      if (this.dirty){ Modal.guard(); return false; }
      if (this.justAdded && Modal.pending === 'close') toast(`${this.d.name} is kept as a draft, switched off`, 'info', 4000);
      return true;
    };
    this.render();
  },
  // The picked value takes the whole answer's place in the reply, or
  // joins the end of it.
  useInReply(){
    const h = this.handler(), http = firstStep(h, 'http'), step = primaryStep(h);
    const t = http && tryEntry(http), field = step && (KINDS[step.type] || {}).text;
    if (!t || !t.picked || !field) return;
    const token = '{' + t.picked.token + '}';
    const whole = '{' + ((http.params.save_as || '').trim() || 'response') + '.body}';
    const text = step.params[field] || '';
    step.params[field] = text.includes(whole) ? text.replace(whole, token) : (text.trimEnd() + ' ' + token).trim();
    this.changed();
  },
  convertSoon(text){
    clearTimeout(this.convertTimer);
    this.convertTimer = setTimeout(() => this.convert(text), 300);
  },
  // Fill the address, the reply and the command from a bot command.
  async convert(text){
    const r = await api('/integrations/convert', {text});
    if (Modal.kind !== 'editor') return;
    if (!r.ok){
      this.converted = {error:r.error};
      this.render();
      return;
    }
    const h = this.handler(), step = primaryStep(h), field = step && (KINDS[step.type] || {}).text;
    allSteps(this.d).filter(s => s.type === 'http').forEach(s => { s.params.url = r.url; });
    if (r.reply && field) step.params[field] = r.reply;
    if (r.command && h.trigger.type === 'chat_command') h.trigger.command = r.command;
    this.converted = {from:r.from, reply:!!r.reply, command:r.command, unknown:r.unknown || []};
    this.changed();
    // The field still shows the pasted text while it has focus.
    const f = q('[data-bind="url"]');
    if (f) f.value = r.url;
  },
  convertedHtml(){
    const c = this.converted;
    if (!c) return '';
    if (c.error) return warnHtml(esc(c.error));
    if (c.start){
      return `<div class="ig-converted">${svg('check', ' stroke-width="3"')}<span>Ready to go: ${esc(c.start)} asks decapi.me, a free service many streamers use. Change the reply on the preview above.</span></div>`;
    }
    const filled = ['the address'].concat(c.reply ? ['the reply'] : [], c.command ? [`the command (${esc(c.command)})`] : []);
    const unknown = c.unknown.length
      ? ` These have no InstantClone match and stay as written: ${c.unknown.map(u => `<code>${esc(u)}</code>`).join(' ')}` : '';
    return `<div class="ig-converted">${svg('check', ' stroke-width="3"')}<span>Converted from ${esc(c.from)}: ${filled.join(', ')} filled in.${unknown}</span></div>`;
  },
  // A ready-made command takes this trigger's place, and answers right
  // away so the preview reads like the real thing.
  async useStart(id){
    const start = (S.data.catalog.api_starts || []).find(x => x.id === id);
    if (!start) return;
    const was = this.handler();
    const fresh = clone(start.handler);
    fresh.enabled = was.enabled;
    this.d.handlers[this.sel] = fresh;
    const p = presetOf(this.d);
    const named = (p && this.d.name === p.name) || S.data.catalog.api_starts.some(x => x.command === this.d.name);
    if (named) this.d.name = start.command;
    this.converted = {start:start.command};
    TRIES.clear();
    this.changed();
    // The request button spins while the real answer comes in.
    await busy(q('[data-act="b-try"]'), () => tryRequest(firstStep(fresh, 'http'), fresh));
    if (Modal.kind === 'editor' && this.handler() === fresh) this.render();
  },
  startsHtml(http){
    const starts = S.data.catalog.api_starts || [];
    return `<div class="ig-starts"><span class="ic-label">Ready-made</span>${starts.map(x =>
      `<button class="ig-start${x.handler.steps[0].params.url === http.params.url ? ' on' : ''}" data-act="ed-start" data-v="${esc(x.id)}">${esc(x.command)}</button>`).join('')}</div>`;
  },
  // Back from "Open in builder", with whatever was changed there.
  fromBuilder(d, dirty){
    this.d = withHandler(d);
    this.dirty = dirty;
    this.sel = Math.min(this.sel, this.d.handlers.length - 1);
    this.chip = null;
    this.run = null;
    this.show();
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
      <span>${esc(c.k)}</span><b data-tick>${esc(c.v)}</b>${svg('chev')}</button>`).join('');
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
        <button class="ic-btn ic-btn-ghost" data-act="ed-test">${svg('play', ' class="ig-btn-ic"')}Send a test</button>`, this.justAdded)}`);
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
    // Fields of a tried answer are picked from its tree, so they wait here.
    const shown = this.moreVars ? all
      : all.filter(v => (!globals.has(v.name) || common.has(v.name)) && !v.name.includes('.json.'));
    const tokens = shown.map(v => `<button class="ig-token" data-act="token" data-token="${esc(v.name)}" data-sample="${esc(v.sample)}">+ ${esc(v.name)}</button>`).join('')
      + (shown.length < all.length ? `<button class="ig-token more" data-act="more-vars">${all.length - shown.length} more…</button>` : '');
    const sample = sampleLine(value, h, step.type === 'discord');
    // A tried answer can send the run down another branch ("offline").
    const vars = sampleMap(h), reached = reachedStep(h.steps, vars, step.type);
    const instead = reached && reached !== step
      ? `<div class="ig-sample ig-instead">Right now it would say: <b>${esc(renderSample(reached.params[k.text], vars))}</b></div>` : '';
    const editor = `<textarea class="ig-msg" data-bind="text" rows="1" aria-label="Message" spellcheck="true" data-keep-style>${esc(value)}</textarea>
      <div class="ig-sample${sample ? '' : ' empty'}">${sample}</div>${instead}`;
    let box;
    if (step.type === 'discord'){
      const ping = step.params.ping === 'here' ? '@here' : step.params.ping === 'everyone' ? '@everyone' : '';
      const updates = step.params.edit === 'last';
      box = `<div class="ig-live ig-live-discord">
        <div class="ig-live-head"><span class="hash">#</span>${esc(channelName(step.params.connection).replace(/^#/, ''))}<span class="ig-live-tag">Live preview</span></div>
        <div class="ig-live-msg"><span class="ig-live-av">${svg('shield')}</span>
          <div class="ig-live-col"><div class="ig-live-meta"><b>InstantClone</b><span class="ig-app">APP</span>${ping && !updates ? `<span class="ig-ping">${esc(ping)}</span>` : ''}
            ${updates ? `<span class="ig-live-upd">${svg('pencil')}edits the last message</span>` : ''}</div>
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
        out.push({id:'msg', k:'Message', v:step.params.edit === 'last' ? 'edits the last one' : 'a new one'});
        if (step.params.edit !== 'last') out.push({id:'ping', k:'Ping', v:ping === 'here' ? '@here' : ping === 'everyone' ? '@everyone' : 'nobody'});
      }
    }
    if (t.type === 'shortcut') out.push({id:'key', k:'Starts with', v:shortcutLabel(t) || 'set a key or pad', warn:!t.hotkey && !t.midi});
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
    const http = firstStep(h, 'http');
    if (http) out.push({id:'url', k:t.type.startsWith('chat') ? 'Answer from' : 'Send to', v:maskUrl(http.params.url) || 'set the address', warn:!http.params.url});
    if (step && step.type === 'file') out.push({id:'file', k:'File', v:step.params.path || 'pick a file', warn:!step.params.path});
    out.push({id:'cooldown', k:'Wait between', v:fmtMs(d.cooldown_ms) || 'no limit'});
    out.push({id:'quiet', k:'Quiet hours', v:d.quiet ? `${d.quiet.from} to ${d.quiet.to}` : 'off'});
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
      case 'msg':
        return `<div class="ic-label">Message</div>${seg([['', 'Send a new one'], ['last', 'Edit the last one']], step.params.edit || '', 'set-edit')}
          <div class="muted">"Edit the last one" changes the message this integration last posted in that channel, so a drop stays one message that goes from down to back. If someone deleted it, a new one goes out.</div>`;
      case 'key': return shortcutHtml(t);
      case 'quiet': return quietHtml(d);
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
      case 'url': {
        const http = firstStep(h, 'http'), chatty = t.type.startsWith('chat');
        return `${chatty && d.preset === 'api_command' ? this.startsHtml(http) : ''}<div class="dfg"><div class="dff"><label>${chatty ? 'Address, or a bot command to convert' : 'Address'}</label>
            <input class="ic-input mono" data-bind="url" value="${esc(http.params.url || '')}" placeholder="${chatty ? 'https://… or $(urlfetch https://…)' : 'https://…'}" spellcheck="false" autocomplete="off"></div>
          <div class="dff"><label>Method</label><select class="ic-input" data-bind="method">${['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map(m => `<option ${m === (http.params.method || 'POST') ? 'selected' : ''}>${m}</option>`).join('')}</select></div></div>
          ${chatty ? '<div class="muted">Paste a command from Nightbot, StreamElements, Fossabot or Streamlabs: the address, the reply and the command name fill in for you.</div>' : ''}
          ${this.convertedHtml()}
          ${tryHtml(http, h, chatty ? 'Use in the reply' : '')}`;
      }
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
    else if (b === 'url' && t.type.startsWith('chat') && BOT_SYNTAX.test(el.value)){
      // A pasted bot command: converted once typing pauses, not saved raw.
      this.convertSoon(el.value);
      return;
    }
    else if (b === 'url' || b === 'method'){
      // A catalog webhook sends every event to one address: set them all.
      allSteps(this.d).filter(s => s.type === 'http').forEach(s => { s.params[b] = el.value; });
      if (b === 'url') this.converted = null;
    }
    else if (b === 'try'){
      // Values to try the request with; not part of the integration.
      tryEntry(firstStep(h, 'http')).inputs[el.dataset.name] = el.value;
      return;
    }
    else if ((b === 'path' || b === 'mode') && step) step.params[b] = el.value;
    else if (!quietInput(this.d, el)) return;
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
      case 'set-edit': if (step) step.params.edit = v; this.changed(); return true;
      case 'ed-start': this.useStart(v); return true;
      case 'set-send': setSend(h, v); this.changed(); return true;
      case 'set-ucd': h.trigger.user_cooldown_ms = +v; this.changed(); return true;
      case 'set-as': if (step) step.params.as = v; this.changed(); return true;
      case 'set-priority': if (step) step.params.priority = v; this.changed(); return true;
      case 'set-cooldown': this.d.cooldown_ms = +v; this.changed(); return true;
      case 'ed-builder':
        Builder.open(this.d, {dirty:this.dirty, back:{title:'Back to the simple editor', keepsEdits:true,
          go:() => Editor.fromBuilder(Builder.d, Builder.dirty)}});
        return true;
      case 'ed-test': busy(el, () => this.test()); return true;
      case 'delete': if (armConfirm(el, 'Delete for good?')) busy(el, () => remove(this.d.id)); return true;
      case 'save': busy(el, () => saveIntegration(this.d, () => { this.dirty = false; })); return true;
      case 'b-try':
        busy(el, async () => {
          await tryRequest(firstStep(h, 'http'), h);
          if (Modal.kind === 'editor') this.render();
        });
        return true;
      case 'b-pick':
        tryEntry(firstStep(h, 'http')).picked = {token:el.dataset.token, sample:el.dataset.sample || ''};
        this.render();
        return true;
      case 'b-use-reply': this.useInReply(); return true;
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
function sampleLine(text, h, discord){
  if (!/\{[A-Za-z0-9_.]/.test(text || '')) return '';
  const read = esc(renderSample(text, sampleMap(h)));
  return 'Reads like: <b>' + (discord ? discordTimes(read) : read) + '</b>';
}

// Quiet hours, shared by both editors.
function quietHtml(d){
  const q = d.quiet;
  return `<div class="ic-label">Quiet hours</div>
    <div class="sub-tabs ig-seg">${[['off', 'Off'], ['on', 'On']].map(([id, label]) =>
      `<button class="sub-tab${(id === 'on') === !!q ? ' on' : ''}" data-act="set-quiet" data-v="${id}" aria-pressed="${(id === 'on') === !!q}">${label}</button>`).join('')}${SEG_IND}</div>
    ${q ? `<div class="ig-quiet"><div class="dff"><label>From</label><input class="ic-input mono" type="time" data-bind="quiet-from" value="${esc(q.from)}"></div>
      <div class="dff"><label>To</label><input class="ic-input mono" type="time" data-bind="quiet-to" value="${esc(q.to)}"></div></div>` : ''}
    <div class="muted">${svg('moon', ' class="ig-inline-ic"')}Nothing runs between these times, on this PC's clock. For alerts that would wake you up.</div>`;
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
      toast(`${combo} already runs a delay action (Controls tab). Pick another.`, 'err', 5000);
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
        toast('That pad already runs a delay action (Controls tab). Pick another.', 'err', 5000);
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

function shortcutHtml(t){
  if (!S.data.shortcuts.available){
    return warnHtml('Hotkeys and MIDI pads work on Windows only. On this system, start it with a web call instead.');
  }
  const keys = t.hotkey ? t.hotkey.split('+').map(p => `<span class="hk-chip">${esc(p)}</span>`).join('<span class="hk-sep">+</span>') : '';
  const listening = !!Keys.padTimer;
  return `<div class="ig-keys">
    <div class="dff"><label>Hotkey</label><div class="ig-key-row">
      <button class="hk-capture ig-key${Keys.recording ? ' recording' : t.hotkey ? '' : ' empty'}" data-act="key-record">${Keys.recording
        ? '<span class="hk-cue">Press the keys… Esc cancels</span>' : keys || '<span class="hk-cue">Click, then press keys</span>'}</button>
      ${t.hotkey ? `<button class="ic-btn-tiny" data-act="key-clear" aria-label="Remove the hotkey">${svg('x')}</button>` : ''}</div></div>
    <div class="dff"><label>MIDI pad</label><div class="ig-key-row">
      <button class="hk-capture ig-key${listening ? ' recording' : t.midi ? '' : ' empty'}" data-act="pad-learn">${listening
        ? '<span class="hk-cue">Press a pad or knob…</span>' : t.midi ? `<span class="hk-chip">${esc(padLabel(t.midi))}</span>` : '<span class="hk-cue">Click, then press a pad</span>'}</button>
      ${t.midi ? `<button class="ic-btn-tiny" data-act="pad-clear" aria-label="Remove the pad">${svg('x')}</button>` : ''}</div></div>
  </div>
  <div class="muted">Works while you're in a game. Use either one, or both. Keys and pads that run the delay (Controls tab) stay theirs.</div>`;
}
function focusAct(act){
  const el = q(`[data-act="${act}"]`);
  if (el) el.focus({preventScroll:true});
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
  ['Stream', [['overlay', 'Show on stream'], ['delay_action', 'Delay action'], ['marker', 'VOD marker'], ['clip', 'Clip']]],
  ['Anything', [['edit_text', 'Edit text'], ['set_var', 'Remember a value'], ['counter', 'Counter'], ['program', 'Run a program'], ['file', 'Write a file']]],
];
// What can start an integration, shown as tiles so every kind is in sight.
const TRIGGERS = [
  {id:'event', icon:'bolt', label:'Something happens', hint:'OBS crashes, a destination drops, the delay changes…',
    eg:'OBS crashes: tell the mods on Discord'},
  {id:'chat_command', icon:'hash', label:'A chat command', hint:'!rank, !delay, anything', chat:true,
    eg:'!rank answers with your rank'},
  {id:'chat_message', icon:'bubble', label:'A chat message', hint:'When chat says something', chat:true,
    eg:'Someone says "gg": thank them'},
  {id:'timer', icon:'clock', label:'On a timer', hint:'Every few minutes', eg:'Every 15 min: remind chat to follow'},
  {id:'webhook', icon:'link', label:'A web call', hint:'Stream Deck, scripts, other apps', eg:'A Stream Deck button cuts the delay'},
  {id:'shortcut', icon:'keyboard', label:'A hotkey or MIDI pad', hint:'A key combo or a pad, even in game', windows:true,
    eg:'Ctrl+Alt+C clips and posts the link'},
];
// Why a trigger tile can't be picked right now, or ''.
function triggerBlocked(k){
  if (k.windows && !S.data.shortcuts.available) return 'Windows only';
  return k.chat && !S.data.twitch.main.login ? 'Needs Twitch connected' : '';
}
// How each event looks when picking one.
const EVENT_INFO = {
  obs_connected:{icon:'play', desc:'OBS starts sending to InstantClone.'},
  obs_disconnected:{icon:'signal', desc:'OBS stops sending, on purpose or not.'},
  eb_detected:{icon:'film', desc:'OBS starts Twitch Enhanced Broadcasting.'},
  hold_opened:{icon:'shield', desc:'OBS dropped; the reconnect screen holds your stream.'},
  obs_back:{icon:'check', desc:'OBS reconnects and the stream carries on.'},
  hold_expired:{icon:'clock', desc:'OBS didn\'t come back in time.'},
  hold_ended:{icon:'cut', desc:'You end the reconnect screen yourself.'},
  destination_live:{icon:'megaphone', desc:'A destination starts showing your stream.'},
  destination_dropped:{icon:'warn', desc:'A destination loses your stream.'},
  all_destinations_down:{icon:'signal', desc:'No destination is live any more.'},
  delay_on:{icon:'clock', desc:'The delay starts.'},
  delay_off:{icon:'clock', desc:'The delay stops: viewers see you live.'},
  delay_changed:{icon:'clock', desc:'The delay gets longer or shorter.'},
};
// Every event as a tile, grouped; `big` for the canvas chooser.
function eventTilesHtml(selected, big){
  const groups = {};
  S.data.events.forEach(e => { (groups[e.group] = groups[e.group] || []).push(e); });
  let n = 0;
  return Object.entries(groups).map(([g, list]) => `<div class="ig-evgroup"><div class="ic-label">${esc(g)}</div>
    <div class="ig-evs${big ? ' big' : ''}" role="radiogroup" aria-label="${esc(g)}">${list.map(e => {
      const info = EVENT_INFO[e.id] || {icon:'bolt', desc:''};
      const on = e.id === selected;
      return `<button type="button" class="ig-ev${on ? ' on' : ''}" role="radio" aria-checked="${on}" data-act="b-event" data-v="${esc(e.id)}" style="--i:${n++}">
        <span class="ig-ev-ic">${svg(info.icon)}</span><span class="ig-ev-t"><b>${esc(e.label)}</b><small>${esc(info.desc)}</small></span>
        ${on ? `<span class="ig-ev-on">${svg('check', ' stroke-width="3"')}</span>` : ''}</button>`;
    }).join('')}</div></div>`).join('');
}
const pathKey = p => p.join('.');

const Builder = {
  d:null, h:0, sel:'trigger', target:[], run:null, dirty:false, lastField:null, back:null,
  // "Send request" results per web request step (by uidOf), this session.
  tries:new Map(),
  // opts: {dirty} carries the editor's unsaved state over, {switchOn} as in
  // Editor.open, {back} as in Modal.back (default: the catalog, if open).
  open(i, opts){
    opts = opts || {};
    this.back = opts.back !== undefined ? opts.back
      : Modal.kind === 'catalog' ? {title:'Back to the catalog', go:() => Catalog.reopen()} : null;
    TRIES.clear();
    this.d = i ? withHandler(clone(i)) : {id:'', name:'My integration', enabled:true, preset:'', cooldown_ms:0,
      handlers:[{enabled:true, trigger:{type:'event', event:'hold_opened', filters:{}}, steps:[]}]};
    // A trigger nobody picked yet: the canvas asks how it starts instead of
    // quietly assuming "OBS crashes".
    this.unpicked = new WeakSet();
    if (!i) this.unpicked.add(this.d.handlers[0]);
    this.choosing = i ? null : 'kind';
    this.cancelTrigger = null;
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
    Modal.back = this.back;
    Modal.onClose = () => !this.dirty || (Modal.guard(), false);
    this.render();
  },
  handler(){ return this.d.handlers[this.h]; },
  render(){
    const d = this.d, h = this.handler();
    const n = allSteps(d).length;
    Modal.body(`${modalHead('steps', 'var(--accent)', titleInput(d.name),
      `Your own · runs on this PC · <span data-tick>${plural(n, 'step')}</span>`,
      `<button class="ic-btn ig-head-btn" data-act="b-test">${svg('play', ' class="ig-btn-ic"')}Test run</button>${enableSwitch('enabled', d.enabled)}`)}
      <div class="ig-bld${this.choosing ? ' picking' : ''}">
        <aside class="ig-palette" aria-label="Blocks">
          <div class="muted ig-palette-hint">Click a block to add it where the dashed box is lit.</div>
          ${PALETTE.map(([group, items]) => `<div class="ic-label">${group}</div>${items.map(([k, label]) =>
            `<button data-act="b-add" data-kind="${k}" style="--c:${KINDS[k].c}"><i></i>${esc(label)}</button>`).join('')}`).join('')}
        </aside>
        <section class="ig-canvas" aria-label="Steps">
          <div class="ig-triggers" data-flip>${d.handlers.map((x, i) => `<button class="ig-trig${i === this.h ? ' on' : ''}${x.enabled ? '' : ' ig-dim'}" data-act="b-handler" data-n="${i}" data-key="h-${uidOf(x)}">
            ${this.unpicked.has(x) ? 'New trigger' : 'When ' + esc(lowerFirst(triggerLabel(x.trigger)))}</button>`).join('')}
            <button class="ig-trig add" data-act="b-add-handler" data-key="h-add">${svg('plus')}Another trigger</button></div>
          ${this.choosing ? this.chooserHtml(h) : `<div class="ig-steps" data-flip>
            <button class="ig-step ig-when${this.sel === 'trigger' ? ' on' : ''}" data-act="b-sel" data-path="trigger" data-key="when">
              <span class="ig-step-kind">WHEN</span><span class="ig-step-text">${esc(triggerSentence(h.trigger))}</span></button>
            ${this.stepsHtml(h.steps, [], 'root')}
          </div>
          ${this.selectedHttp() ? `<div class="ig-try-wrap" data-key="try-${uidOf(this.selectedHttp())}">${tryHtml(this.selectedHttp(), h, 'Reply in chat with it')}</div>` : ''}
          ${this.run ? `<div class="sys-section ig-run-box"><div class="ic-label">Test run: ${esc(this.run.status || 'error')}</div>
            ${this.run.error ? warnHtml(esc(this.run.error)) : this.run.status === 'running'
              ? '<div class="ig-running"><span class="ig-spin"></span>Running the test…</div>' : runLogHtml(this.run.steps)}</div>` : ''}`}
        </section>
        <aside class="ig-inspector" aria-label="Settings" data-flip><div class="ig-insp" data-key="${this.inspectorKey()}">${this.inspectorHtml()}</div></aside>
      </div>
      ${footHtml(d, this.dirty)}`);
  },
  // "How does it start?" then, for events, "What happens?": big tiles in
  // the canvas, so the first decision is visible and can't be skipped.
  chooserHtml(h){
    const picked = !this.unpicked.has(h);
    if (this.choosing === 'event'){
      return `<div class="ig-chooser" data-key="chooser-event">
        <div class="ig-chooser-head">
          <button type="button" class="ig-chooser-back" data-act="b-pick-back">${svg('back')}${picked ? 'Keep it as it was' : 'Other ways to start'}</button>
          <h3>What happens?</h3><p>Pick the moment it reacts to.</p></div>
        ${eventTilesHtml(picked ? h.trigger.event : '', true)}</div>`;
    }
    return `<div class="ig-chooser" data-key="chooser-kind">
      <div class="ig-chooser-head"><h3>How does it start?</h3><p>Pick what sets it off. You can change it any time.</p></div>
      <div class="ig-kinds">${TRIGGERS.map((k, n) => {
        const blocked = triggerBlocked(k);
        return `<button type="button" class="ig-kind" data-act="b-trigger-type" data-v="${k.id}" style="--i:${n}"${blocked && k.windows ? ' disabled' : ''}>
        <span class="ig-kind-ic">${svg(k.icon)}</span><b>${k.label}</b><small>${k.hint}</small>
        <span class="ig-kind-eg">${blocked || 'e.g. ' + esc(k.eg)}</span></button>`;
      }).join('')}</div></div>`;
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
    const tokens = varsFor(h).map(v => `<button class="ig-token" data-act="b-token" data-token="${esc(v.name)}" data-sample="${esc(v.sample)}">${esc(v.name)}</button>`).join('');
    const varsBox = `<div class="ig-insp-vars"><div class="ic-label">Variables here</div>
      <div class="ig-tokens">${tokens}</div><div class="muted">Click one to insert it in the field you last used. <span class="mono">{name|text}</span> uses the text when the value is empty or 0.</div></div>`;
    if (this.choosing){
      return `<div class="ig-insp-title" style="--c:#5ac8fa"><small>WHEN</small><b>${this.choosing === 'event' ? 'What happens?' : 'How does it start?'}</b></div>
        <p class="ig-insp-p">Pick it in the middle. Then add steps from the list on the left: each one runs in order.</p>`;
    }
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
        + field('text', 'Message', {area:true})
        + field('edit', 'Message', {select:[['', 'Send a new one'], ['last', 'Edit the last one it sent there']]})
        + (s.params.edit === 'last' ? note('A drop stays one message: "down", then "back". If the last one was deleted, a new one goes out.')
          : field('ping', 'Ping', {select:[['', 'Nobody'], ['here', '@here'], ['everyone', '@everyone']]}))
        + note('<span class="mono">&lt;t:{hold_ends_at}:R&gt;</span> shows a live countdown in Discord.');
      case 'chat': return field('text', 'Message', {area:true}) + field('as', 'Send as', {select:[['', 'Your account'], ['bot', 'Bot account']]})
        + field('reply', 'Reply to the viewer', {select:[['', 'No'], ['yes', 'Yes, in their thread']]});
      case 'phone': return field('title', 'Title') + field('text', 'Message', {area:true}) + field('priority', 'Priority', {select:[['', 'Normal'], ['high', 'High'], ['urgent', 'Urgent']]});
      case 'http': return field('method', 'Method', {select:['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map(m => [m, m])})
        + field('url', 'Address', {mono:true, ph:'https://api.example/rank/{arg1}'})
        + note('Values from chat are encoded for you, so a viewer can only fill in their part. Keep API keys in Headers: they stay hidden here and never go into recipes.')
        + secretField('headers-' + uidOf(s), `<textarea class="ic-input ig-secret" data-bind="p" data-name="headers" placeholder="Authorization: Bearer …">${esc(s.params.headers || '')}</textarea>`, 'Headers (one per line, Name: value)')
        + field('body', 'Body', {area:true, ph:'{"event":"{delay}"}'})
        + field('save_as', 'Save the answer as', {mono:true, ph:'response'})
        + note(`Later steps can use <span class="mono">{${esc(s.params.save_as || 'response')}.status}</span>, <span class="mono">.ok</span>, <span class="mono">.body</span> and <span class="mono">.json.field</span>.`)
        + note('Try it under the steps: send the request for real and pick what to use from the answer.');
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
      case 'overlay': return field('title', 'Small title', {ph:'Clip'}) + field('text', 'Text', {area:true, ph:'{user} just clipped that'})
        + field('seconds', 'On screen for', {select:[['3', '3 s'], ['6', '6 s'], ['10', '10 s'], ['15', '15 s'], ['30', '30 s']]})
        + alertsSetupHtml() + note('Tests never put anything on stream: use "Show a test alert" above.');
      case 'edit_text': {
        const op = EDIT_OPS[s.params.op] || EDIT_OPS.first_line;
        const got = editTextSample(s, sampleMap(this.handler()));
        return field('input', 'Text', {mono:true, ph:'{api.body}'})
          + field('op', 'Do', {select:Object.entries(EDIT_OPS).map(([id, o]) => [id, o[0]])})
          + (op[1] ? field('a', op[1], {mono:true}) : '') + (op[2] ? field('b', op[2], {mono:true}) : '')
          + field('save_as', 'Save it as', {mono:true, ph:'text'})
          + note(`Gives <b>${esc(got) || '(nothing)'}</b> with the sample values.${s.params.op === 'random' ? ' Each run picks one of the lines, or one of the parts between |.' : ''}`)
          + note(`Later steps use it as <span class="mono">{${esc((s.params.save_as || '').trim() || 'text')}}</span>.`);
      }
      case 'counter': return field('name', 'Name', {mono:true, ph:'crashes'}) + field('op', 'Do', {select:[['', 'Add'], ['subtract', 'Subtract'], ['set', 'Set to'], ['reset', 'Reset to 0']]})
        + field('by', 'By', {mono:true, ph:'1'}) + note(`Kept between runs. Use it anywhere as <span class="mono">{counter.${esc(s.params.name || 'name')}}</span>.`);
    }
    return '';
  },
  triggerInspector(h){
    const t = h.trigger;
    const typeSel = `<div class="dff"><label>Starts when</label><div class="ig-ttypes" role="radiogroup" aria-label="Starts when">${TRIGGERS.map(k => {
      const blocked = triggerBlocked(k);
      return `<button type="button" class="ig-ttype${k.id === t.type ? ' on' : ''}" role="radio" aria-checked="${k.id === t.type}" data-act="b-trigger-type" data-v="${k.id}"${blocked && k.windows && k.id !== t.type ? ' disabled' : ''}>
        ${svg(k.icon)}<b>${k.label}</b><small>${blocked || k.hint}</small></button>`;
    }).join('')}</div></div>`;
    let body = '';
    if (t.type === 'event'){
      const current = eventOf(t.event), info = EVENT_INFO[t.event] || {icon:'bolt', desc:''};
      body = `<div class="dff"><label>What happens</label><button type="button" class="ig-evcurrent" data-act="b-change-event">
        <span class="ig-ev-ic">${svg(info.icon)}</span><span class="ig-ev-t"><b>${esc(current ? current.label : t.event)}</b><small>${esc(info.desc)}</small></span>
        <span class="ig-evchange">Change</span></button></div>`;
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
      body = secretField('hook', `<div class="ig-copy-row"><input class="ic-input mono ig-secret" readonly value="${esc(url)}">
        <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(url)}">Copy</button></div>`, 'Call this address (keep it secret)') + `
        <div class="muted">GET or POST from a Stream Deck, a script or any app on this network. The body is <span class="mono">{body}</span>; JSON fields are <span class="mono">{body.field}</span>.</div>
        <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="b-new-token">Make a new secret address</button>`;
    } else if (t.type === 'shortcut'){
      body = shortcutHtml(t);
    }
    const more = this.d.handlers.length > 1
      ? `<div class="ig-insp-row">${enableSwitch('h-enabled', h.enabled, 'This trigger is on')}
         <button class="ic-btn ic-btn-ghost ig-small" data-act="b-del-handler">Remove trigger</button></div>` : '';
    return `<div class="ig-insp-title" style="--c:#5ac8fa"><small>WHEN</small><b>${esc(triggerLabel(t))}</b></div>
      ${typeSel}${body}
      <div class="dff"><label>Wait between two runs (seconds)</label><input class="ic-input mono" data-bind="b-cooldown" inputmode="numeric" value="${Math.round((this.d.cooldown_ms || 0) / 1000)}"></div>
      <div class="ig-insp-quiet">${quietHtml(this.d)}</div>${more}`;
  },
  input(el){
    const b = el.dataset.bind, h = this.handler(), t = h.trigger;
    if (b === 'name') this.d.name = el.value;
    else if (b === 'enabled') this.d.enabled = el.checked;
    else if (b === 'b-cooldown') this.d.cooldown_ms = Math.max(0, (parseFloat(el.value) || 0) * 1000);
    else if (b === 'h-enabled') h.enabled = el.checked;
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
    else if (b === 'try'){
      // Values to try the request with; not part of the integration.
      tryEntry(this.stepAt(this.sel)).inputs[el.dataset.name] = el.value;
      return;
    }
    else if (!quietInput(this.d, el)) return;
    this.changed();
  },
  changed(){
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
        this.changed();
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
        this.changed();
        return true;
      }
      case 'b-dup': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        list.splice(index + 1, 0, clone(list[index]));
        this.sel = path.slice(0, -1).concat(index + 1);
        this.target = [];
        this.changed();
        return true;
      }
      case 'b-del': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        list.splice(index, 1);
        this.sel = 'trigger';
        this.target = [];
        this.changed();
        return true;
      }
      case 'b-handler':
        this.h = +el.dataset.n; this.sel = 'trigger'; this.target = []; this.run = null;
        this.choosing = this.unpicked.has(this.handler()) ? 'kind' : null;
        this.render();
        return true;
      case 'b-add-handler': {
        const added = {enabled:true, trigger:newTrigger('event'), steps:[]};
        d.handlers.push(added);
        this.unpicked.add(added);
        this.choosing = 'kind';
        this.h = d.handlers.length - 1; this.sel = 'trigger'; this.target = []; this.run = null;
        this.changed();
        return true;
      }
      case 'b-del-handler':
        if (d.handlers.length < 2 || !armConfirm(el, 'Remove it and its steps?')) return true;
        d.handlers.splice(this.h, 1);
        this.h = 0; this.sel = 'trigger'; this.target = []; this.run = null;
        this.changed();
        return true;
      case 'b-trigger-type': {
        const h = this.handler(), v = el.dataset.v;
        if (v === 'event'){
          // Which event is the next question, asked in the canvas.
          this.cancelTrigger = this.unpicked.has(h) ? null : clone(h.trigger);
          if (h.trigger.type !== 'event') h.trigger = newTrigger('event');
          this.choosing = 'event';
          this.render();
          return true;
        }
        if (h.trigger.type !== v || this.unpicked.has(h)) h.trigger = newTrigger(v);
        this.unpicked.delete(h);
        this.choosing = null;
        this.sel = 'trigger';
        this.changed();
        if (v === 'shortcut') focusAct('key-record');
        const first = {chat_command:'t-command', chat_message:'t-pattern', timer:'t-every'}[v];
        if (first){
          // The starting value is a placeholder: typing replaces it.
          focusField(first);
          const f = q(`[data-bind="${first}"]`);
          if (f) f.select();
        }
        return true;
      }
      case 'b-change-event':
        this.cancelTrigger = clone(this.handler().trigger);
        this.choosing = 'event';
        this.render();
        return true;
      case 'b-event': {
        const h = this.handler();
        h.trigger = {type:'event', event:el.dataset.v, filters:{}};
        this.unpicked.delete(h);
        this.choosing = null;
        this.cancelTrigger = null;
        this.sel = 'trigger';
        this.changed();
        return true;
      }
      case 'b-pick-back': {
        const h = this.handler();
        if (this.unpicked.has(h)) this.choosing = 'kind';
        else {
          if (this.cancelTrigger) h.trigger = this.cancelTrigger;
          this.cancelTrigger = null;
          this.choosing = null;
        }
        this.render();
        return true;
      }
      case 'b-new-token': this.handler().trigger.token = randomToken(); this.changed(); return true;
      case 'b-token': insertAtCursor(this.lastField, '{' + el.dataset.token + '}'); return true;
      case 'b-test': if (this.askToPick()) busy(el, () => this.test()); return true;
      case 'b-try':
        busy(el, async () => {
          await tryRequest(this.selectedHttp(), this.handler());
          if (Modal.kind !== 'builder') return;
          this.render();
          this.reveal('.ig-try-wrap');
        });
        return true;
      case 'b-pick':
        tryEntry(this.selectedHttp()).picked = {token:el.dataset.token, sample:el.dataset.sample || ''};
        this.render();
        this.reveal('.ig-picked');
        return true;
      case 'b-use-reply': this.replyWithPick(); return true;
      case 'delete': if (armConfirm(el, 'Delete for good?')) busy(el, () => remove(d.id)); return true;
      case 'save': if (this.askToPick()) busy(el, () => saveIntegration(d, () => { this.dirty = false; })); return true;
    }
    return false;
  },
  // The picked value goes into the chat reply right after the request: the
  // one already there, or a new one.
  replyWithPick(){
    const s = this.stepAt(this.sel), t = tryEntry(s);
    if (!t.picked) return;
    const token = '{' + t.picked.token + '}';
    const {list, index} = this.locate(this.sel);
    const next = list[index + 1];
    if (next && next.type === 'chat'){
      next.params.text = ((next.params.text || '').trimEnd() + ' ' + token).trim();
    } else {
      const chat = newStep('chat');
      const trig = this.handler().trigger;
      chat.params.text = trig.type.startsWith('chat') ? `{user}: ${token}` : token;
      list.splice(index + 1, 0, chat);
    }
    this.sel = this.sel.slice(0, -1).concat(index + 1);
    this.target = [];
    this.changed();
  },
  // A trigger still waiting for "How does it start?" can't be saved or run.
  // Shows it and says so; true when every trigger is picked.
  askToPick(){
    const at = this.d.handlers.findIndex(x => this.unpicked.has(x));
    if (at < 0) return true;
    this.h = at;
    this.choosing = 'kind';
    this.sel = 'trigger';
    this.render();
    toast('Pick how it starts first', 'info');
    return false;
  },
  // The selected step, when it is a web request (it gets "Try it").
  selectedHttp(){
    const s = Array.isArray(this.sel) ? this.stepAt(this.sel) : null;
    return s && s.type === 'http' ? s : null;
  },
  // Bring a just-drawn part of the canvas into view.
  reveal(selector){
    const el = q(selector);
    if (el) el.scrollIntoView({block:'nearest', behavior:reduced() ? 'auto' : 'smooth'});
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
    case 'shortcut': return {type, hotkey:'', midi:''};
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
    overlay:{title:'', text:'', seconds:'6'}, edit_text:{input:'', op:'first_line', a:'', b:'', save_as:'text'},
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
    case 'shortcut': return shortcutLabel(t) ? `You press ${shortcutLabel(t)}` : 'You press a hotkey or pad (not set yet)';
  }
  return t.type;
}
function stepSentence(s){
  const p = s.params || {};
  const q = v => `“${highlightVars(v || '')}”`;
  switch (s.type){
    case 'discord': return `Discord ${esc(channelName(p.connection))} ${q(p.text)}${p.edit === 'last' ? ' · edits the last one' : p.ping ? ' · @' + esc(p.ping) : ''}`;
    case 'chat': return `Chat ${q(p.text)}${p.as === 'bot' ? ' · as bot' : ''}${p.reply === 'yes' ? ' · as a reply' : ''}`;
    case 'phone': return `Phone ${q(p.text)}`;
    case 'http': return `${esc(p.method || 'POST')} ${esc(maskUrl(p.url) || '(address)')}${p.save_as ? ' → ' + esc(p.save_as) : ''}`;
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
    case 'overlay': return `On stream ${q(p.text)}`;
    case 'edit_text': return `${esc((EDIT_OPS[p.op] || EDIT_OPS.first_line)[0])}: ${highlightVars(p.input || '…')} → <span class="v">{${esc((p.save_as || '').trim() || 'text')}}</span>`;
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
    const tabs = [['discord', 'Discord'], ['twitch', 'Twitch'], ['phone', 'Phone'], ['stream', 'On stream']].map(([id, l]) =>
      `<button class="sub-tab${this.tab === id ? ' on' : ''}" role="tab" aria-selected="${this.tab === id}" data-act="conn-tab" data-tab="${id}">${l}</button>`).join('');
    const pane = {discord:() => this.discordHtml(), twitch:() => this.twitchHtml(), phone:() => this.phoneHtml(),
      stream:() => `<div class="sys-section ig-form">${alertsSetupHtml()}</div>`}[this.tab]();
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
        ${secretField('d-url', `<input class="ic-input mono ig-secret" data-bind="d-url" value="" placeholder="${e.id ? 'leave blank to keep it' : 'https://discord.com/api/webhooks/…'}" spellcheck="false" autocomplete="off">`, 'Webhook link')}</div>
        <div class="muted">In Discord: Server settings › Integrations › Webhooks › New webhook › Copy webhook URL.</div>
        <div class="ig-form-actions"><button class="ic-btn ic-btn-ghost" data-act="d-cancel">Cancel</button><button class="ic-btn ic-btn-primary" data-act="d-save">Save channel</button></div>
      </div>` : `<button class="dest-add ig-add-row" data-act="d-new" data-key="d-add"><span class="dest-add-icon">${svg('plus')}</span>Add a Discord channel</button>`;
    return `<div class="ig-accts" data-flip>${rows || '<div class="muted ig-none" data-key="d-none">No Discord channel yet.</div>'}${form}</div>`;
  },
  twitchHtml(){
    const t = S.data.twitch;
    if (!t.available){
      return warnHtml('This copy of InstantClone has no Twitch app, so it can\'t log in to Twitch yet. '
        + 'You can add your own in a couple of minutes: <a href="#" data-act="twitch-app">System › Twitch login app</a>.');
    }
    return (t.notice ? warnHtml(esc(t.notice)) : '')
      + this.accountHtml('main', t.main, 'Your Twitch account', 'Chat, VOD markers and clips. Log in once: InstantClone keeps it fresh.')
      + this.accountHtml('bot', t.bot, 'Bot account (optional)', 'A second account that posts in chat instead of you.')
      + '<div class="muted">No password ever goes through InstantClone: you approve it on twitch.tv, and can revoke it there any time.'
      + (t.app === 'custom' ? ' Logging in through your own Twitch app (<a href="#" data-act="twitch-app">change</a>).' : '') + '</div>';
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
      <div class="dfg">${secretField('p-topic', `<div class="ig-copy-row"><input class="ic-input mono ig-secret" data-bind="p-topic" value="${esc(p.topic)}" placeholder="instantclone-…" spellcheck="false" autocomplete="off">
        <button class="ic-btn ic-btn-ghost ig-small" data-act="p-random" title="Make a hard-to-guess topic">Random</button></div>`, 'Topic')}
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
        if (f){
          // A new topic has to be typed into the phone app, so show it.
          S.revealed.add('p-topic');
          this.render();
          const fresh = q('[data-bind="p-topic"]');
          fresh.value = 'instantclone-' + randomToken().slice(0, 12);
          decodeField(fresh, true);
        }
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

// The alerts browser source: where "Show on stream" cards appear.
function alertsSetupHtml(){
  const url = `${location.origin}/alerts`;
  return `<div class="ig-setup">
    <div class="ig-insp-p"><b>Once, in OBS:</b> add a <b>Browser source</b> with this address, as big as your canvas. Cards appear at the bottom; add <span class="mono">?at=top</span> to put them at the top.</div>
    <div class="ig-copy-row"><input class="ic-input mono" readonly value="${esc(url)}" aria-label="Alerts address">
      <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(url)}">Copy</button></div>
    <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="alert-test">${svg('play', ' class="ig-btn-ic"')}Show a test alert</button>
  </div>`;
}

// System > Twitch & OBS, with the Twitch login app section open.
function openTwitchAppSettings(){
  Modal.close(true);
  if (typeof showTab === 'function') showTab('system');
  if (typeof showSubTab === 'function') showSubTab('system', 'twitch');
  const box = document.getElementById('sys-twitch-app');
  if (!box) return;
  box.open = true;
  // After the sub-tab has laid out and started its entrance.
  setTimeout(() => {
    box.scrollIntoView({block:'start', behavior:reduced() ? 'auto' : 'smooth'});
    const field = document.getElementById('s-twitch-client');
    if (field) setTimeout(() => field.focus({preventScroll:true}), 300);
  }, 80);
}

function focusField(bind){
  const f = q(`[data-bind="${bind}"]`);
  if (f) f.focus({preventScroll:true});
}

// ---------------------------------------------------------------- recipes

const Recipes = {
  preview:null, text:'', back:null,
  resume(){
    Modal.open('import', 700);
    Modal.back = this.back;
    this.render();
  },
  openImport(){
    this.preview = null;
    this.text = '';
    this.back = Modal.kind === 'catalog' ? {title:'Back to the catalog', go:() => Catalog.reopen()} : null;
    this.resume();
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
  openShare(only){
    Modal.open('share', 700);
    Modal.body(`${modalHead('copy', 'var(--accent)', 'Share as a recipe', 'Pick what to share. Your connections, keys and secret links are never included.')}
      <div class="dest-form-body ig-conn-body">
        <div class="dff"><label>Recipe name</label><input class="ic-input" data-bind="s-name" placeholder="My crash kit"></div>
        <div class="ig-share-list">${S.data.integrations.map(i =>
          `<label class="crash-check"><input type="checkbox" data-bind="s-pick" value="${esc(i.id)}" ${i.id === only ? 'checked' : ''}><span><span class="crash-check-title">${esc(i.name)}</span></span></label>`).join('')}</div>
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
  if (el.closest('.ig-menu') && a !== 'menu-delete') Menu.close();
  const owner = el.closest('.ig-modal') && OWNERS[Modal.kind];
  if (owner && await owner.act(a, el)) return;
  switch (a){
    case 'modal-close': Modal.close(); break;
    case 'modal-back': Modal.goBack(); break;
    case 'guard-keep': Modal.hideGuard(); break;
    case 'guard-discard':
      if (Modal.pending === 'back' && Modal.back) Modal.leaveBack(); else Modal.close(true);
      break;
    case 'menu': Menu.toggle(el, el.dataset.id); break;
    case 'menu-delete':
      if (armConfirm(el, 'Click again to delete')){ Menu.close(); remove(el.dataset.id); }
      break;
    case 'share-one': Recipes.openShare(el.dataset.id); break;
    case 'reveal': {
      const id = el.dataset.field, show = !S.revealed.has(id);
      if (show) S.revealed.add(id); else S.revealed.delete(id);
      if (OWNERS[Modal.kind] && OWNERS[Modal.kind].render) OWNERS[Modal.kind].render();
      const field = q(`.ig-secret-field[data-field="${CSS.escape(id)}"] .ig-secret`);
      decodeField(field, show);
      break;
    }
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
    case 'twitch-app': openTwitchAppSettings(); break;
    case 'alert-test':
      busy(el, async () => {
        const r = await api('/integrations/alerts/test', {});
        toast(r.ok ? 'A test card is on the alerts source now' : r.error || 'Could not show it', r.ok ? 'ok' : 'err');
      });
      break;
    // Shortcut and quiet hours: the same in both editors.
    case 'key-record': Keys.record(); break;
    case 'key-clear': Keys.trigger().hotkey = ''; OWNERS[Modal.kind].changed(); break;
    case 'pad-learn': if (!Keys.padTimer) Keys.learnPad(el); break;
    case 'pad-clear': Keys.trigger().midi = ''; OWNERS[Modal.kind].changed(); break;
    case 'set-quiet': {
      const o = OWNERS[Modal.kind];
      o.d.quiet = el.dataset.v === 'on' ? (o.d.quiet || {from:'23:00', to:'08:00'}) : null;
      o.changed();
      break;
    }
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
