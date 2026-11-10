// What an insert chip says: the value in words. The variable itself shows
// on hover, with an example.
const VAR_LABEL = {
  reason:'why', hold:'hold length', hold_ends_at:'hold ends at', down_for:'how long it was down',
  destination:'platform name', platform:'platform', previous:'before', delay:'delay', delay_state:'delay on/off',
  delay_ms:'delay (ms)', delay_max:'longest delay now', phase:'delay phase', hold_active:'reconnect screen on?', hold_left:'time left on hold',
  destinations_live:'platforms live', destinations_total:'platforms on', obs_live:'OBS streaming?',
  bitrate:'bitrate', channel:'your channel', time:'time now', date:'date', user:'viewer\'s name',
  user_login:'viewer\'s login', user_role:'viewer\'s role', message:'their message', words:'their words (no cheers)', args:'text after the command',
  target:'who it\'s about', arg1:'1st word', arg2:'2nd word', arg3:'3rd word', body:'what was sent',
  query:'address query', source:'pressed from', 'obs.scene':'OBS scene', 'obs.streaming':'streaming now?',
  'obs.recording':'recording now?', 'obs.muted':'muted?', 'obs.visible':'showing?', 'clip.url':'clip link', 'clip.ok':'clip made?', 'clip.error':'why no clip',
  'stream.ok':'title or category set?', 'stream.category':'category on Twitch', 'stream.error':'why it wasn\'t set',
  uptime:'stream time', vod_time:'VOD time', onair:'on-air state', timeline:'timeline', chapters:'chapter list',
  previous_chapter:'chapter before', duration:'stream length', crashes:'crashes', drops:'drops',
  highlights:'highlights', report:'health report', state:'on-air state', line:'timeline line', kind:'line kind',
  down_s:'seconds down', within:'counted over', down_total:'total time down', messages:'messages',
  normal:'normal pace', busier:'times busier', chatters:'people talking', word_share:'% saying the words',
  top_word:'top word', scene:'scene', previous_scene:'scene before', uses:'times run', text:'edited text',
  obs_source:'OBS source', muted:'muted?', file:'saved file', tier:'sub tier', gift:'gifted?', months:'months subbed',
  streak:'month streak', count:'subs gifted', total:'gifted in total', bits:'bits', viewers:'raiders', reward:'reward',
  cost:'reward cost', input:'what they typed', seconds:'ad length (s)', automatic:'automatic?',
  stopped:'stopped on purpose?', protected:'crash protection took over?',
};
function varLabel(name){
  if (VAR_LABEL[name]) return VAR_LABEL[name];
  let m;
  if ((m = /^counter\.(.+)$/.exec(name))) return `${m[1]} count`;
  if ((m = /^(goal|timer)\.([a-z0-9_]+)(?:\.(\w+))?$/.exec(name))) return `${m[2]} ${m[1]}${m[3] ? ' ' + m[3] : ''}`;
  if ((m = /^(.+)\.json\.(.+)$/.exec(name))) return m[2].split('.').pop().replace(/_/g, ' ');
  if ((m = /^(.+)\.(status|ok|body|output|code)$/.exec(name))) return `${m[1]} ${{status:'status', ok:'worked?', body:'answer', output:'answer', code:'exit code'}[m[2]]}`;
  return name.replace(/[._]/g, ' ');
}

// What each variable holds, for the card shown over a variable chip.
const VAR_HELP = {
  reason:'Why it happened. For OBS: crash, freeze or stopped (you stopped, and crash protection held anyway). For a platform: what went wrong.',
  hold:'How long the reconnect screen can stay on air.',
  hold_ends_at:'When the reconnect screen runs out, as a Unix time. In Discord, <t:{hold_ends_at}:R> is a live countdown.',
  uses:'How many times this integration has run, this time included. Kept across restarts.',
  text:'What the Edit text step made.',
  down_for:'How long it was down.',
  destination:'The platform\'s name, as you named it in Destinations.',
  platform:'Which platform it is, like youtube or twitch.',
  previous:'The delay before this change.',
  stopped:'yes when you stopped the stream yourself, no when OBS dropped.',
  protected:'yes when crash protection kept your destinations up.',
  delay:'The delay, written for people. On delay events, the new one.',
  delay_ms:'The delay in milliseconds, for maths and comparisons.',
  delay_max:'The longest delay the buffer holds right now, in seconds (an hour at most). !setdelay checks against it.',
  delay_state:'on or off.',
  phase:'What the delay is doing: idle, preparing, ready or active.',
  hold_active:'yes while the reconnect screen is on air.',
  hold_left:'Time left on the reconnect screen.',
  destinations_live:'How many platforms are live right now.',
  destinations_total:'How many platforms are switched on.',
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
  query:'The part of the address after the ?. Use {query.name} for one value of it.',
  'clip.url':'The link to the clip the Clip step made.',
  'clip.ok':'yes when the Clip step made a clip.',
  'clip.error':'Why the Clip step made no clip, like "connect Twitch first". Empty when it worked.',
  'stream.ok':'yes when the Stream title and category step set what it was given.',
  'stream.category':'The category as Twitch names it: a mod types "valo", this says VALORANT.',
  'stream.error':'Why the title or category was not set, like no category by that name. Empty when it worked.',
  source:'What pressed it: hotkey, midi, stream_deck (its address) or web.',
  previous_chapter:'The chapter before the latest one: what the stream goes back to after a "Technical difficulties" chapter.',
  uptime:'How long this stream has been on air, like 1:02:14. Empty between streams.',
  vod_time:'Where a moment happening now lands in the VOD: the uptime plus your delay.',
  onair:'crash, delay, live or off: the most important one that is true right now.',
  timeline:'This stream\'s timeline so far, one line per moment, with its place in the VOD.',
  chapters:'This stream\'s chapters, written the way YouTube reads them from a description.',
  duration:'How long the stream ran.',
  crashes:'How many times OBS dropped during the stream.',
  drops:'How many times platforms dropped.',
  highlights:'How many highlights the timeline got.',
  report:'One line per platform: how many drops, and how long it was down.',
  state:'The on-air state now: live, delay, crash or off.',
  line:'The line just added to the timeline, with its time.',
  kind:'note, chapter or highlight.',
  down_s:'How long it was down, in seconds, for comparisons.',
  within:'The window the drops were counted in.',
  down_total:'How long it was down in total this stream.',
  messages:'Chat messages in the window.',
  normal:'How many messages a window usually has in your chat.',
  busier:'How many times busier than normal chat is, like 4.2.',
  chatters:'Different people talking in the window.',
  word_share:'Percent of messages with one of the words.',
  top_word:'The word chat used most, out of yours.',
  scene:'The OBS scene now on air.',
  previous_scene:'The scene before it.',
};
// A variable's description, including the ones named by the user's own
// steps (web answers, counters, remembered values).
function varHelp(name){
  if (VAR_HELP[name]) return VAR_HELP[name];
  let m;
  if ((m = /^(.+)\.json\.(.+)$/.exec(name))) return `The value at ${m[2].split('.').join(' › ')} in the "${m[1]}" web answer.`;
  if ((m = /^(.+)\.status$/.exec(name))) return `The status the "${m[1]}" web request got back, like 200.`;
  if ((m = /^(.+)\.ok$/.exec(name))) return `yes when the "${m[1]}" step worked, otherwise no.`;
  if ((m = /^(.+)\.output$/.exec(name))) return `What the "${m[1]}" program printed.`;
  if ((m = /^(.+)\.code$/.exec(name))) return `How the "${m[1]}" program ended: 0 when it went well.`;
  if ((m = /^([^.]+)\.[^.]+\.(muted|visible)$/.exec(name))) return `yes when this source was ${m[2] === 'muted' ? 'muted' : 'showing'} as "${m[1]}" checked.`;
  if ((m = /^(.+)\.(scene|streaming|recording|muted|visible)$/.exec(name))) return {
    scene:`The scene on air when "${m[1]}" checked OBS.`, streaming:`yes when OBS was streaming as "${m[1]}" checked.`,
    recording:`yes when OBS was recording as "${m[1]}" checked.`, muted:`yes when the source "${m[1]}" checked was muted.`,
    visible:`yes when the source "${m[1]}" checked was showing.`}[m[2]];
  if ((m = /^(.+)\.body$/.exec(name))) return `Everything the "${m[1]}" web request got back.`;
  if ((m = /^counter\.(.+)$/.exec(name))) return `The "${m[1]}" counter, kept between runs.`;
  if ((m = /^goal\.([a-z0-9_]+)$/.exec(name))) return `Where your "${m[1]}" goal stands. Add .target, .left, .percent or .name for the rest.`;
  if ((m = /^goal\.([a-z0-9_]+)\.target$/.exec(name))) return `What your "${m[1]}" goal counts to.`;
  if ((m = /^timer\.([a-z0-9_]+)$/.exec(name))) return `The time left on your "${m[1]}" timer, as a clock. Add .seconds, .running or .name for the rest.`;
  if ((m = /^arg([4-9])$/.exec(name))) return `Word number ${m[1]} after the command.`;
  if ((m = /^body\.(.+)$/.exec(name))) return `The ${m[1].split('.').join(' › ')} field of what the caller sent.`;
  if ((m = /^query\.(.+)$/.exec(name))) return `The "${m[1]}" value in the address, like ?${m[1]}=red.`;
  return '';
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
    this.el.querySelector('p').textContent = varHelp(name) || SET_EARLIER;
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
const tokenAt = e => (e.target && e.target.closest) ? e.target.closest('.ig-modal .ig-vchip[data-token]') : null;
document.addEventListener('pointerover', e => VarTip.want(tokenAt(e)));
document.addEventListener('focusin', e => VarTip.want(tokenAt(e)));
document.addEventListener('pointerdown', () => VarTip.hide(), true);
window.addEventListener('scroll', () => VarTip.hide(), true);

// What a chat command's previews use as the words after it: a catalog
// entry's own example ("!chapter Boss fight"), set when it opens.
const SAMPLE_ARGS = new WeakMap();
// The amount a moment's samples show while one of its bigger alerts is
// edited ({name, value}): the least that plays it.
const TIER_SAMPLE = new WeakMap();

// Sample values for previews: the variables this handler can use, each
// with its `group` (trigger, steps, live). With `upto`, only what a step
// before that one made: a step can't use a clip link made after it.
function varsFor(handler, upto){
  const d = S.data;
  const own = [];
  const t = handler && handler.trigger;
  if (t && t.type === 'event'){
    const e = eventOf(t.event);
    // A countdown sample should count down from now; an amount is at least
    // the trigger's minimum (from 1000 bits up previews 1000).
    const tier = TIER_SAMPLE.get(handler);
    if (e) own.push(...e.vars.map(v => {
      if (v.name === 'hold_ends_at') return {name:v.name, sample:String(Math.floor(Date.now() / 1000) + 120)};
      if (tier && tier.name === v.name) return {name:v.name, sample:tier.value};
      const least = +(t.filters || {})['min_' + v.name] || 0;
      return least > +v.sample ? {name:v.name, sample:String(least)} : v;
    }));
  }
  // A block runs inside whatever started it: most often a command.
  if (t && (t.type === 'chat_command' || t.type === 'chat_message' || t.type === 'block')){
    const args = SAMPLE_ARGS.get(handler);
    own.push(...d.vars.chat.map(v => args == null ? v : {name:v.name,
      sample:{args, arg1:args.split(' ')[0], target:args.split(' ')[0], message:`${t.command || ''} ${args}`.trim()}[v.name] ?? v.sample}));
  }
  if (t && (t.type === 'webhook' || t.type === 'shortcut')) own.push(...d.vars.web);
  if (t && t.type === 'chat_activity') own.push(...d.vars.activity);
  if (t && t.type === 'scene') own.push(...d.vars.scene);
  const made = [];
  const known = () => { const m = {}; own.concat(made, d.vars.global, myValues()).forEach(v => { if (!(v.name in m)) m[v.name] = v.sample; }); return m; };
  const outputs = s => {
    if (s.type === 'http'){
      const n = (s.params.save_as || '').trim() || 'response';
      // Once "Send request" ran, the real answer is the sample.
      const tried = TRIES.get(uidOf(s));
      const got = tried && tried.status ? tried : null;
      made.push({name:n + '.status', sample:got ? String(got.status) : '200'},
        {name:n + '.ok', sample:got ? (got.status >= 200 && got.status < 300 ? 'yes' : 'no') : 'yes'},
        {name:n + '.body', sample:got ? got.body.slice(0, 200) : '{...}'});
      if (got && got.json !== undefined){
        jsonLeaves(got.json).filter(l => l.usable && (n + '.json.' + l.path).length <= 64).slice(0, 60)
          .forEach(l => made.push({name:`${n}.json.${l.path}`, sample:l.text}));
      }
    }
    // A name made from a value ({user}) is no variable anyone can write.
    const fixed = name => name && !name.includes('{');
    if (s.type === 'obs' && s.params.action === 'check' && fixed(answerName(s))){
      const n = answerName(s);
      made.push({name:n + '.scene', sample:(S.data.obs && S.data.obs.current) || 'Gaming'}, {name:n + '.streaming', sample:'yes'}, {name:n + '.recording', sample:'no'});
      const names = listedNames(s.params.source), several = names.length > 1;
      if (names.length) made.push({name:n + '.muted', sample:'no', label:several ? 'all muted?' : ''}, {name:n + '.visible', sample:'yes', label:several ? 'all showing?' : ''});
      if (several) sourceKeys(names).forEach((key, i) => made.push({name:`${n}.${key}.muted`, sample:'no', label:`${names[i]} muted?`},
        {name:`${n}.${key}.visible`, sample:'yes', label:`${names[i]} showing?`}));
    }
    if (s.type === 'program' && s.params.wait === 'yes' && fixed((s.params.save_as || '').trim() || 'program')){
      const n = (s.params.save_as || '').trim() || 'program';
      made.push({name:n + '.output', sample:'Diamond II'}, {name:n + '.code', sample:'0'}, {name:n + '.ok', sample:'yes'});
    }
    if (s.type === 'clip') made.push({name:'clip.url', sample:'https://clips.twitch.tv/BraveSnipe'},
      {name:'clip.id', sample:'BraveSnipe'}, {name:'clip.ok', sample:'yes'}, {name:'clip.error', sample:''});
    if (s.type === 'channel') made.push({name:'stream.ok', sample:'yes'}, {name:'stream.category', sample:'VALORANT'}, {name:'stream.error', sample:''});
    if (s.type === 'counter' && fixed(s.params.name)) made.push({name:'counter.' + s.params.name, sample:'2'});
    if (s.type === 'counter' && (s.params.save_as || '').trim() && fixed(s.params.save_as.trim())) made.push({name:s.params.save_as.trim(), sample:'2'});
    if (s.type === 'set_var' && fixed(s.params.name)) made.push({name:s.params.name, sample:s.params.value || ''});
    if (s.type === 'viewer' && s.params.op === 'spend') made.push({name:'paid', sample:'yes'});
    if (s.type === 'viewer' && s.params.who === 'active') made.push({name:'active.count', sample:'12'});
    if (s.type === 'math' && fixed((s.params.save_as || '').trim() || 'result')) made.push({name:(s.params.save_as || '').trim() || 'result', sample:'100'});
    if (s.type === 'wait_chat' && fixed((s.params.save_as || '').trim() || 'reply')){
      const n = (s.params.save_as || '').trim() || 'reply';
      made.push({name:n + '.ok', sample:'yes'}, {name:n + '.user', sample:'Nat'}, {name:n + '.user_login', sample:'nat'},
        {name:n + '.message', sample:s.params.text || '!accept'}, {name:n + '.arg1', sample:''});
    }
    if (s.type === 'edit_text' && fixed((s.params.save_as || '').trim() || 'text')) made.push({name:(s.params.save_as || '').trim() || 'text', sample:editTextSample(s, known())});
  };
  // Steps in run order up to `upto`: switched-off ones never run, and a
  // step inside one branch of a check never sees the other branch's.
  const holds = (steps, x) => { let found = false; walk(steps, y => { found = found || y === x; }); return found; };
  const visit = steps => {
    for (const s of steps || []){
      if (upto && s === upto) return true;
      if (s.enabled === false){ if (upto && holds([s], upto)) return true; continue; }
      if (s.type === 'if'){
        if (upto && holds(s.then, upto)) return visit(s.then);
        if (upto && holds(s.else, upto)) return visit(s.else);
        visit(s.then);
        visit(s.else);
        continue;
      }
      const before = made.length;
      outputs(s);
      made.slice(before).forEach(v => { v.step = s; });
    }
    return false;
  };
  visit(handler && handler.steps);
  const seen = new Set();
  return own.map(v => Object.assign({group:'trigger'}, v))
    .concat(made.map(v => Object.assign({group:'steps'}, v)), myValues().map(v => Object.assign({group:'mine'}, v)),
      d.vars.global.map(v => Object.assign({group:'live'}, v)))
    .filter(v => !seen.has(v.name) && seen.add(v.name));
}
// The streamer's own values as variables: {my.<name>}, each with what it holds now.
// Goals and timers ride along: {goal.<id>} and {timer.<id>} are the streamer's too.
function myValues(){
  return (S.data.values || []).map(v => ({name:'my.' + v.name, sample:v.value})).concat(trackerValues());
}
// The sample timeline's lines end in a word ("`0:12` 🟢 Live"): that word
// in the page's language, so the preview reads like the rest of the page.
function saidTimeline(text){
  return String(text || '').split('\n').map(line => line.replace(/[A-Za-z][A-Za-z ]*$/, word => tr(word))).join('\n');
}
function sampleMap(handler){
  const m = {};
  varsFor(handler).forEach(v => { m[v.name] = v.name === 'timeline' ? saidTimeline(v.sample) : v.sample; });
  return m;
}
// Variables a step's fields use that nothing fills there: most likely a
// typo ({usr}), which would send as nothing. Like api::unknown_vars, but in
// run order like the insert chips, since a value a later step makes is
// still empty here. Each with the closest real name, when one is close.
function unknownVars(handler, step){
  const used = new Set();
  Object.values(step.params || {}).forEach(v => {
    for (const m of String(v || '').matchAll(/\{\{[A-Za-z0-9_.]+\}\}|\{([A-Za-z0-9_.]{1,64})(?:[|#?][^}]*)?\}/g)) if (m[1]) used.add(m[1]);
  });
  if (!used.size) return [];
  const names = varsFor(handler, step).map(v => v.name);
  const t = (handler && handler.trigger) || {};
  // Families no list can hold: any counter (counters outlive a run), any
  // field of a web answer or of what a web call sent, any word of a command.
  // Viewer values, the leaderboard and kept texts are read by name too.
  const prefixes = ['counter.', 'viewer.', 'target.', 'top.', 'saved.', 'goal.', 'timer.'].concat(names.filter(n => n.endsWith('.status')).map(n => n.slice(0, -6)));
  if (t.type === 'webhook' || t.type === 'shortcut') prefixes.push('body.', 'query.');
  const chat = t.type === 'chat_command' || t.type === 'chat_message' || t.type === 'block';
  return [...used].filter(n => !names.includes(n) && !prefixes.some(p => n.startsWith(p)) && !(chat && /^arg[1-9]$/.test(n)))
    .map(name => ({name, hint:closestName(name, names)}));
}
// The name `typed` was most likely meant to be: two edits away at most.
function closestName(typed, names){
  let best = '', score = 3;
  for (const n of names){
    const d = editDistance(typed.toLowerCase(), n.toLowerCase());
    if (d < score){ score = d; best = n; }
  }
  return best;
}
function editDistance(a, b){
  if (Math.abs(a.length - b.length) > 2) return 3;
  let prev = Array.from({length:b.length + 1}, (_, j) => j);
  for (let i = 1; i <= a.length; i++){
    const row = [i];
    for (let j = 1; j <= b.length; j++) row[j] = Math.min(prev[j] + 1, row[j - 1] + 1, prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
    prev = row;
  }
  return prev[b.length];
}
// A calm note under the fields for each unknown name, with a one-click
// fix when a real name is close.
function unknownVarsHtml(list){
  if (!list.length) return '';
  return `<div class="ig-unknown" role="status">${list.slice(0, 3).map(u => `<div>${svg('warn')}<span>Nothing fills <code>{${esc(u.name)}}</code>, so it sends as nothing.${u.hint
    ? ` Did you mean <button type="button" class="ig-fixvar" data-act="fix-var" data-from="${esc(u.name)}" data-to="${esc(u.hint)}">{${esc(u.hint)}}</button>?` : ''}</span></div>`).join('')}</div>`;
}
// Rename a variable in every field of every step of `handler`.
function renameVar(handler, from, to){
  // Not inside {{…}}: that is text written out as is.
  const name = from.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(`(?<!\\{)\\{${name}([|#?][^}]*)?\\}`, 'g');
  walk(handler.steps, s => Object.keys(s.params || {}).forEach(k => {
    if (typeof s.params[k] === 'string') s.params[k] = s.params[k].replace(re, (all, fallback) => `{${to}${fallback || ''}}`);
  }));
}
