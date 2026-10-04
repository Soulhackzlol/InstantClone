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
  star:'M12 2l3.1 6.3 6.9 1-5 4.9 1.2 6.8L12 17.8 5.8 21l1.2-6.8-5-4.9 6.9-1z',
  flame:'M8.5 14.5A2.5 2.5 0 0 0 11 12c0-1.4-.5-2-1-3-1.1-2.1-.2-4 2-6 .5 2.5 2 4.9 4 6.5 2 1.6 3 3.5 3 5.5a7 7 0 1 1-14 0c0-1.2.4-2.3 1-3.2a2.5 2.5 0 0 0 2.5 2.7z',
  bulb:'M9 18h6M10 22h4M12 2a7 7 0 0 0-4 12.7c.6.5 1 1.3 1 2.1V17h6v-.2c0-.8.4-1.6 1-2.1A7 7 0 0 0 12 2z',
  layers:'M12 2l10 5-10 5L2 7zM2 17l10 5 10-5M2 12l10 5 10-5',
  pulse:'M22 12h-4l-3 9L9 3l-3 9H2',
  list:'M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01',
  columns:'M4 5h6v14H4zM14 5h6v14h-6z',
  rows:'M4 5h16v14H4z',
  refresh:'M21 12a9 9 0 1 1-2.6-6.4M21 4v5h-5',
};
const svg = (name, extra) =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"${extra||''}><path d="${PATHS[name]}"/></svg>`;

// What each step kind is, how it looks, and which parameter holds its text.
const KINDS = {
  discord:      {label:'Discord',       tag:'SEND',   c:'#a5b4fc', icon:'bubble',   text:'text', hint:'Posts a message in one of your Discord channels.'},
  chat:         {label:'Twitch chat',   tag:'SEND',   c:'#a78bfa', icon:'bubble',   text:'text', hint:'Says something in your Twitch chat.'},
  phone:        {label:'Phone push',    tag:'SEND',   c:'#f9a8d4', icon:'phone',    text:'text', hint:'A notification on your phone.'},
  http:         {label:'Web request',   tag:'SEND',   c:'#fcd34d', icon:'link',     text:'body', hint:'Calls a website or an API, and keeps its answer for later steps.'},
  wait:         {label:'Wait',          tag:'WAIT',   c:'#c3cad1', icon:'clock', hint:'Pauses before the next step.'},
  wait_delay:   {label:'Wait for the delay', tag:'WAIT', c:'#c3cad1', icon:'clock', hint:'Waits until your viewers see this moment.'},
  if:           {label:'If / otherwise',tag:'IF',     c:'#f0a53a', icon:'bolt', hint:'Runs some steps only when a check is true, others when it isn\'t.'},
  stop:         {label:'Stop',          tag:'STOP',   c:'#f2665a', icon:'x', hint:'Ends the run here.'},
  delay_action: {label:'Delay action',  tag:'STREAM', c:'#86efac', icon:'cut', hint:'Sets, switches on, cuts or disarms your delay.'},
  marker:       {label:'VOD marker',    tag:'TWITCH', c:'#a78bfa', icon:'bookmark', text:'description', hint:'Marks this moment in your Twitch VOD.'},
  clip:         {label:'Clip',          tag:'TWITCH', c:'#a78bfa', icon:'film', hint:'Clips the last moments of your Twitch stream.'},
  program:      {label:'Run a program', tag:'RUN',    c:'#fcd34d', icon:'terminal', hint:'Starts a program on this PC.'},
  file:         {label:'Write a file',  tag:'FILE',   c:'#fcd34d', icon:'file',     text:'text', hint:'Writes text to a file, for an OBS text source for example.'},
  set_var:      {label:'Remember a value', tag:'VALUE', c:'#93c5fd', icon:'steps', text:'value', hint:'Keeps a value for the next steps of this run.'},
  counter:      {label:'Counter',       tag:'VALUE',  c:'#93c5fd', icon:'steps', hint:'Counts up or down, and keeps the count between runs.'},
  overlay:      {label:'Show on stream', tag:'STREAM', c:'#5ac8fa', icon:'screen',  text:'text', hint:'Shows a card on your stream, through the alerts browser source.'},
  edit_text:    {label:'Edit text',     tag:'VALUE',  c:'#93c5fd', icon:'type', hint:'Reshapes text: a line, a part, a rounded number, a random pick.'},
  timeline:     {label:'Timeline',      tag:'LOG',    c:'#5ac8fa', icon:'list', text:'text', hint:'Adds a line, a chapter or a highlight to this stream\'s timeline, at its place in the VOD.'},
  obs:          {label:'OBS',           tag:'OBS',    c:'#86efac', icon:'layers', hint:'Switches a scene, shows or hides a source, or changes what a text source says.'},
};
// The icon a catalog module wears: what it is about, not how it sends.
const PRESET_ICON = {
  crash_alert:'shield', destination_down:'signal', going_live:'megaphone', phone_crash:'phone',
  tell_chat:'bubble', delay_command:'hash', delay_notice:'clock', mod_controls:'cut', socials:'hash',
  vod_markers:'bookmark', webhook:'link', crash_counter:'file', stream_alerts:'bell', api_command:'globe',
  clip_button:'film', delay_status:'clock', smart_alerts:'pulse', stream_timeline:'list', chapters:'bookmark',
  highlight:'star', hype_clip:'flame', on_air_light:'bulb', scene_delay:'layers',
};
// Which step a card previews: the first one that says something.
const PREVIEW_ORDER = ['discord','chat','phone','overlay','marker','http','file','timeline','clip','delay_action','program'];
// A Discord card's colors: the dashboard's palette, readable on Discord.
const CARD_COLORS = [['#5ac8fa', 'Blue'], ['#3fcf8e', 'Green'], ['#f0a53a', 'Amber'], ['#f2665a', 'Red'],
  ['#fcd34d', 'Gold'], ['#a78bfa', 'Purple'], ['#ff7eb6', 'Pink'], ['#8b949e', 'Grey']];
// What a chat activity rule measures: [label, unit, what the number is].
const RULES = {
  busier:['Busier than normal', '× normal', 'How many times faster than your chat usually goes'],
  messages:['Messages', 'messages', 'Lines of chat in the window'],
  chatters:['People talking', 'people', 'Different people, so one spammer can\'t set it off'],
  words:['Saying these words', '% of messages', 'Share of messages with one of the words'],
};
const TIMELINE_KINDS = {note:'A line', chapter:'A chapter', highlight:'A highlight'};
const DELAY_ACTIONS = {
  arm:'Turn the delay on', cut_after:'Back to live, viewers miss nothing', cut:'Back to live now, viewers skip ahead',
  activate:'Turn on the delay that\'s ready', toggle:'Turn the delay on or off', disarm:'Turn the delay off',
  end_hold:'End the reconnect screen',
};
// What an OBS step can do, and what each needs.
const OBS_ACTIONS = {
  scene:['Switch to a scene', 'scene'], show:['Show a source', 'source'], hide:['Hide a source', 'source'],
  toggle:['Show or hide a source (flip)', 'source'], text:['Change a text source', 'source'],
};
const OPS = {
  is:'is', is_not:'is not', contains:'contains', not_contains:'does not contain', starts_with:'starts with',
  greater:'is more than', less:'is less than', empty:'is empty', not_empty:'is not empty',
  whole_number:'is a whole number',
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
// Known values for event filters, so "Only if" offers choices. A
// destination's reason is free text ("connection timed out"), so only
// OBS's own reasons are a list.
function filterChoices(event, name){
  if (name === 'reason') return event === 'hold_opened' ? ['crash', 'freeze', 'stopped'] : null;
  return {stopped:['yes', 'no'], protected:['yes', 'no'], state:['live', 'delay', 'crash', 'off'],
    kind:['note', 'chapter', 'highlight']}[name] || null;
}

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
  return c ? '#' + c.name : 'no channel yet';
}
function triggerLabel(t){
  if (!t) return '';
  switch (t.type){
    case 'event': {
      const e = eventOf(t.event), f = t.filters || {};
      if (!e) return t.event;
      // Several triggers on one event read apart: "On air: delay",
      // "Timeline: chapter".
      if (f.state || f.kind) return `${{onair_changed:'On air', timeline_updated:'Timeline'}[t.event] || e.label}: ${f.state || f.kind}`;
      return e.label;
    }
    case 'chat_command': return t.command || '!command';
    case 'chat_message': return 'Chat message';
    case 'timer': return `Every ${Math.round((t.every_ms||0)/60000)} min`;
    case 'webhook': return 'Web call';
    case 'shortcut': return shortcutLabel(t) || 'A button';
    case 'chat_activity': return 'Chat gets busy';
    case 'scene': return t.scene ? `Scene: ${sceneLabel(t)}` : 'An OBS scene';
  }
  return t.type;
}
// A threshold with its unit, from its label: "180 s", "3 drops", "10 min".
function paramShort(p, v){
  if (/second/i.test(p.label)) return `${v} s`;
  if (/minute/i.test(p.label)) return `${v} min`;
  return `${v} ${p.label.toLowerCase()}`;
}
// `Ctrl+Alt+K, pad 36 or Stream Deck`, or '' when nothing is set yet.
function shortcutLabel(t){
  const parts = [t.hotkey, t.midi && padLabel(t.midi), t.token && 'Stream Deck'].filter(Boolean);
  return parts.length > 1 ? parts.slice(0, -1).join(', ') + ' or ' + parts[parts.length - 1] : parts.join('');
}
// "3× busier, 5 people and 25% saying clip": a chat activity trigger in words.
function activitySummary(t){
  const parts = (t.rules || []).map(r => {
    const v = +r.value || 0;
    switch (r.kind){
      case 'busier': return `${v}× busier`;
      case 'messages': return `${v}+ messages`;
      case 'chatters': return `${v}+ people`;
      case 'words': return `${v}% saying ${(r.words || '').split(',')[0].trim() || '…'}`;
    }
    return r.kind;
  });
  if (!parts.length) return 'no rules yet';
  const joint = t.match === 'any' ? ' or ' : ' and ';
  return parts.length > 1 ? parts.slice(0, -1).join(', ') + joint + parts[parts.length - 1] : parts[0];
}
// `note:1:36@Launchpad` reads "pad 36 (Launchpad)".
function padLabel(sig){
  const m = /^(note|cc):(\d+):(\d+)(?:@(.+))?$/.exec(sig || '');
  if (!m) return sig || '';
  // A CC control works as a button: it presses at half way and up.
  return `${m[1] === 'note' ? 'pad' : 'button (CC)'} ${m[3]}${m[2] !== '1' ? ' ch ' + m[2] : ''}${m[4] ? ' (' + m[4] + ')' : ''}`;
}
// "The hold runs out" reads "When the hold runs out", "A web call" "When
// a web call"; "OBS crashed" stays.
function lowerFirst(s){
  return /^[A-Z][a-z ]/.test(s || '') ? s[0].toLowerCase() + s.slice(1) : s;
}

// What an insert chip says: the value in words. The variable itself shows
// on hover, with an example.
const VAR_LABEL = {
  reason:'why', hold:'hold length', hold_ends_at:'hold ends at', down_for:'how long it was down',
  destination:'platform name', platform:'platform', previous:'before', delay:'delay', delay_state:'delay on/off',
  delay_ms:'delay (ms)', phase:'delay phase', hold_active:'reconnect screen on?', hold_left:'time left on hold',
  destinations_live:'platforms live', destinations_total:'platforms on', obs_live:'OBS streaming?',
  bitrate:'bitrate', channel:'your channel', time:'time now', date:'date', user:'viewer\'s name',
  user_login:'viewer\'s login', user_role:'viewer\'s role', message:'their message', args:'text after the command',
  target:'who it\'s about', arg1:'1st word', arg2:'2nd word', arg3:'3rd word', body:'what was sent',
  query:'address query', source:'pressed from', 'clip.url':'clip link', 'clip.ok':'clip made?', 'clip.error':'why no clip',
  uptime:'stream time', vod_time:'VOD time', onair:'on-air state', timeline:'timeline', chapters:'chapter list',
  previous_chapter:'chapter before', duration:'stream length', crashes:'crashes', drops:'drops',
  highlights:'highlights', report:'health report', state:'on-air state', line:'timeline line', kind:'line kind',
  down_s:'seconds down', within:'counted over', down_total:'total time down', messages:'messages',
  normal:'normal pace', busier:'times busier', chatters:'people talking', word_share:'% saying the words',
  top_word:'top word', scene:'scene', previous_scene:'scene before', uses:'times run', text:'edited text',
  stopped:'stopped on purpose?', protected:'crash protection took over?',
};
function varLabel(name){
  if (VAR_LABEL[name]) return VAR_LABEL[name];
  let m;
  if ((m = /^counter\.(.+)$/.exec(name))) return `${m[1]} count`;
  if ((m = /^(.+)\.json\.(.+)$/.exec(name))) return m[2].split('.').pop().replace(/_/g, ' ');
  if ((m = /^(.+)\.(status|ok|body)$/.exec(name))) return `${m[1]} ${{status:'status', ok:'worked?', body:'answer'}[m[2]]}`;
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
  if ((m = /^(.+)\.ok$/.exec(name))) return `yes when the "${m[1]}" web request worked, otherwise no.`;
  if ((m = /^(.+)\.body$/.exec(name))) return `Everything the "${m[1]}" web request got back.`;
  if ((m = /^counter\.(.+)$/.exec(name))) return `The "${m[1]}" counter, kept between runs.`;
  if ((m = /^arg([4-9])$/.exec(name))) return `Word number ${m[1]} after the command.`;
  if ((m = /^body\.(.+)$/.exec(name))) return `The ${m[1].split('.').join(' › ')} field of what the caller sent.`;
  if ((m = /^query\.(.+)$/.exec(name))) return `The "${m[1]}" value in the address, like ?${m[1]}=red.`;
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

// What a chat command's previews use as the words after it: a catalog
// entry's own example ("!chapter Boss fight"), set when it opens.
const SAMPLE_ARGS = new WeakMap();

// Sample values for previews: the variables this handler can use, each
// with its `group` (trigger, steps, live). With `upto`, only what a step
// before that one made: a step can't use a clip link made after it.
function varsFor(handler, upto){
  const d = S.data;
  const own = [];
  const t = handler && handler.trigger;
  if (t && t.type === 'event'){
    const e = eventOf(t.event);
    // A countdown sample should count down from now.
    if (e) own.push(...e.vars.map(v => v.name === 'hold_ends_at' ? {name:v.name, sample:String(Math.floor(Date.now() / 1000) + 120)} : v));
  }
  if (t && (t.type === 'chat_command' || t.type === 'chat_message')){
    const args = SAMPLE_ARGS.get(handler);
    own.push(...d.vars.chat.map(v => args == null ? v : {name:v.name,
      sample:{args, arg1:args.split(' ')[0], target:args.split(' ')[0], message:`${t.command || ''} ${args}`.trim()}[v.name] ?? v.sample}));
  }
  if (t && (t.type === 'webhook' || t.type === 'shortcut')) own.push(...d.vars.web);
  if (t && t.type === 'chat_activity') own.push(...d.vars.activity);
  if (t && t.type === 'scene') own.push(...d.vars.scene);
  const made = [];
  const known = () => { const m = {}; own.concat(made, d.vars.global).forEach(v => { if (!(v.name in m)) m[v.name] = v.sample; }); return m; };
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
    if (s.type === 'clip') made.push({name:'clip.url', sample:'https://clips.twitch.tv/BraveSnipe'},
      {name:'clip.id', sample:'BraveSnipe'}, {name:'clip.ok', sample:'yes'}, {name:'clip.error', sample:''});
    if (s.type === 'counter' && fixed(s.params.name)) made.push({name:'counter.' + s.params.name, sample:'2'});
    if (s.type === 'set_var' && fixed(s.params.name)) made.push({name:s.params.name, sample:s.params.value || ''});
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
      outputs(s);
    }
    return false;
  };
  visit(handler && handler.steps);
  const seen = new Set();
  return own.map(v => Object.assign({group:'trigger'}, v))
    .concat(made.map(v => Object.assign({group:'steps'}, v)), d.vars.global.map(v => Object.assign({group:'live'}, v)))
    .filter(v => !seen.has(v.name) && seen.add(v.name));
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
    case 'whole_number': return /^[0-9]+$/.test(l);
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
// The bits of Discord's Markdown previews need: code blocks, `code`,
// **bold**, *italic*, [links](url) and countdowns. `text` is raw; every
// piece is escaped before it is styled.
function discordMarkdown(text){
  const blocks = [];
  let html = esc(text).replace(/```(?:\w*\n)?([\s\S]*?)```/g, (all, code) => {
    blocks.push(`<pre>${code.replace(/^\n|\n$/g, '')}</pre>`);
    return `\u0000${blocks.length - 1}\u0000`;
  });
  html = html.replace(/`([^`\n]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<i>$2</i>')
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)/g, '<span class="ig-link">$1</span>')
    .replace(/(^|[\s(])(https?:\/\/[^\s<]+)/g, '$1<span class="ig-link">$2</span>');
  html = discordTimes(html).replace(/\n/g, '<br>');
  return html.replace(/\u0000(\d+)\u0000/g, (all, n) => blocks[+n]);
}
// `Name | value` lines as fields; `||` takes the full width. Mirrors
// runner::parse_fields. `keepEmpty` keeps half-written rows, for editing.
function parseFields(text, keepEmpty){
  return String(text || '').split('\n').map(line => {
    const at = fieldBar(line);
    if (at < 0) return null;
    const full = line.startsWith('||', at);
    return {name:line.slice(0, at).trim(), value:line.slice(at + (full ? 2 : 1)).trim(), inline:!full};
  }).filter(f => f && (keepEmpty || (f.name && f.value)));
}
// The first `|` outside {…}: a name can hold {reason|unknown}.
function fieldBar(line){
  let depth = 0;
  for (let i = 0; i < line.length; i++){
    const c = line[i];
    if (c === '{') depth++;
    else if (c === '}') depth = Math.max(0, depth - 1);
    else if (c === '|' && !depth) return i;
  }
  return -1;
}
// A bar outside braces in a name would split it in the wrong place.
function fieldsText(fields){
  const safe = name => { let out = '', depth = 0;
    for (const c of name){ depth += c === '{' ? 1 : c === '}' && depth ? -1 : 0; out += c === '|' && !depth ? '/' : c; }
    return out; };
  return fields.map(f => `${safe(f.name)} ${f.inline ? '|' : '||'} ${f.value}`).join('\n');
}
// How wide each field sits, in twelfths of the card: inline fields share a
// row two or three at a time, like Discord; the rest take it all.
function fieldSpans(fields){
  const spans = [];
  let run = [];
  const flush = () => {
    for (let at = 0; at < run.length; at += 3){
      const row = run.slice(at, at + 3);
      row.forEach(() => spans.push(12 / row.length));
    }
    run = [];
  };
  fields.forEach(f => { if (f.inline) run.push(f); else { flush(); spans.push(12); } });
  flush();
  return spans;
}
// A card's color, falling back to the InstantClone blue.
function cardColor(value, vars){
  const v = renderSample(value || '', vars || {}).trim();
  return /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.test(v) ? (v[0] === '#' ? v : '#' + v) : '#5ac8fa';
}
// "Today at 21:04", the way Discord labels a card's timestamp.
function discordNow(){
  return 'Today at ' + new Date().toLocaleTimeString([], {hour:'2-digit', minute:'2-digit'});
}
// A Discord card (embed) as it lands, filled with sample values.
function cardHtml(step, vars, big){
  const p = step.params;
  const fill = v => renderSample(v || '', vars);
  const title = fill(p.title), desc = fill(p.text), footer = fill(p.footer);
  const fields = parseFields(p.fields).map(f => ({name:fill(f.name), value:fill(f.value), inline:f.inline}))
    .filter(f => f.name.trim() && f.value.trim());
  const foot = [footer, p.timestamp === 'yes' ? discordNow() : ''].filter(Boolean).join(' • ');
  return `<div class="ig-embed${big ? ' big' : ''}" style="--ec:${esc(cardColor(p.color, vars))}">
    ${title ? `<div class="ig-embed-title${/^https?:\/\//.test(fill(p.url)) ? ' link' : ''}">${discordMarkdown(title)}</div>` : ''}
    ${desc ? `<div class="ig-embed-desc">${discordMarkdown(desc)}</div>` : ''}
    ${fields.length ? `<div class="ig-embed-fields">${fields.map((f, n) => `<div class="${f.inline ? 'in' : ''}" style="--span:${fieldSpans(fields)[n]}"><b>${esc(f.name)}</b><span>${discordMarkdown(f.value)}</span></div>`).join('')}</div>` : ''}
    ${foot ? `<div class="ig-embed-foot"><i>${svg('shield')}</i>${esc(foot)}</div>` : ''}
  </div>`;
}

// Mirrors template::url_component: only unreserved characters stay.
// A pasted command from a chat bot rather than a plain address.
const BOT_SYNTAX = /\$\(|\$\{|\{readapi\.|\$readapi\(/i;
// Mirrors template::url_component, `.` and `..` included.
const urlComponent = v => v === '.' || v === '..' ? v.replace(/\./g, '%2E')
  : encodeURIComponent(v).replace(/[!'()*]/g, c => '%' + c.charCodeAt(0).toString(16).toUpperCase());
const jsonStringContent = v => JSON.stringify(v).slice(1, -1);
const oneLine = v => v.replace(/[\u0000-\u001f\u007f]/g, ' ');
// An address as safe to show on stream: scheme and host only, since the
// path and query often carry an API key or a webhook secret.
function maskUrl(url){
  const m = /^(https?:\/\/[^/?#\s]+)(\S*)/i.exec(String(url || '').trim());
  if (!m) return String(url || '').trim() ? 'an address' : '';
  return m[1] + (m[2] && m[2] !== '/' ? '/…' : '');
}
// Whether a field holds something that reads differently once sent: a
// variable, or (in Discord) Markdown, a link or a countdown.
function needsRender(raw, discord){
  const text = String(raw || '');
  return /\{[A-Za-z0-9_.]{1,64}(?:\|[^}]*)?\}/.test(text)
    || (!!discord && /\[[^\]\n]+\]\(|\*\*|`|<t:\d|https?:\/\//.test(text));
}
// A field that shows what it will say until it's being edited, then what
// was written (variables and all). `control` carries class ig-tpl-in;
// `cls` lays the shown text out like the field: msg, line, title or above.
function tplHtml(control, raw, vars, cls, discord){
  if (!needsRender(raw, discord)) return `<div class="ig-tpl">${control}</div>`;
  const text = renderSample(raw, vars);
  const shown = text.trim() ? (discord ? discordMarkdown(text) : esc(text)) : '<span class="ig-tpl-none">empty with these sample values</span>';
  const tag = cls === 'line' ? '' : '<span class="ig-tpl-hint">sample</span>';
  return `<div class="ig-tpl has-vars">${control}<div class="ig-tpl-out ${cls}" data-act="tpl-edit" aria-hidden="true" title="Click to edit. Shown with sample values.">${tag}${shown}</div></div>`;
}
// Insert chips for a field: the values in words, grouped. `act` is the
// editor's insert action.
function tokensHtml(vars, act, extra){
  const group = {trigger:'From what happened', steps:'From earlier steps', live:'Always there'};
  let last = '';
  return vars.map(v => {
    const head = v.group && v.group !== last ? `<span class="ig-tokens-group">${group[v.group]}</span>` : '';
    last = v.group || last;
    return `${head}<button class="ig-token" data-act="${act}" data-token="${esc(v.name)}" data-sample="${esc(v.sample)}" aria-label="Insert ${esc(varLabel(v.name))}">+ ${esc(varLabel(v.name))}</button>`;
  }).join('') + (extra || '');
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
// "10s", "2m", "1m 30s", "1h": how a duration field shows a length.
function fmtDur(ms){
  ms = Math.max(0, Math.round(+ms || 0));
  if (!ms) return '0s';
  if (ms % 1000) return (ms / 1000).toFixed(1).replace(/\.0$/, '') + 's';
  const total = ms / 1000, h = Math.floor(total / 3600), m = Math.floor(total / 60) % 60, sec = total % 60;
  return [h ? h + 'h' : '', m ? m + 'm' : '', sec ? sec + 's' : ''].filter(Boolean).join(' ');
}
// What a duration field holds, in ms: "10s", "2m", "1m 30s", "1.5m", "1h",
// or a bare number in `unit` ("s" or "m"). NaN when it isn't one.
function parseDur(text, unit){
  const t = String(text || '').trim().toLowerCase().replace(/,/g, '.');
  if (!t) return NaN;
  const per = {ms:1, s:1000, sec:1000, m:60000, min:60000, h:3600000};
  if (/^\d+(\.\d+)?$/.test(t)) return Math.round(+t * per[unit || 's']);
  let total = 0, rest = t;
  const part = /^(\d+(?:\.\d+)?)\s*(ms|sec|min|s|m|h)\s*/;
  while (rest){
    const m = part.exec(rest);
    if (!m) return NaN;
    total += +m[1] * per[m[2]];
    rest = rest.slice(m[0].length);
  }
  return Math.round(total);
}
// A text field for a length of time. `bind` names it for the editor;
// `unit` is what a bare number means. Shows the value as "2m".
function durInput(bind, ms, unit, extra){
  return `<input class="ic-input mono ig-dur" data-bind="${bind}" data-unit="${unit || 's'}" value="${esc(fmtDur(ms))}"
    spellcheck="false" autocomplete="off" placeholder="10s, 2m"${extra || ''}>`;
}
// Read a duration field: the ms, or null (marked invalid) when it isn't one.
function durValue(el, min, max){
  const ms = parseDur(el.value, el.dataset.unit);
  const ok = Number.isFinite(ms) && ms >= (min || 0) && ms <= (max == null ? Infinity : max);
  el.setAttribute('aria-invalid', ok ? 'false' : 'true');
  el.title = ok ? '' : `Like 10s, 2m or 1m 30s${min ? ', at least ' + fmtDur(min) : ''}${max != null ? ', at most ' + fmtDur(max) : ''}`;
  return ok ? ms : null;
}
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
  // The new markup has no height for a message box: measure it again, or a
  // redraw (focusing it shows "Reads like") shrinks it and cuts its text.
  if (tag === 'TEXTAREA' && a.classList.contains('ig-msg')) autosize(a);
  // A busy button redrawn as disabled (Preview turning into a locked Add)
  // stays disabled once the work is done: see `busy`.
  if (busyEls.has(a)){ a._wantDisabled = b.hasAttribute('disabled'); a.classList.add('is-busy'); a.disabled = true; a.setAttribute('aria-busy', 'true'); }
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
// Its row too: a narrow window wraps a control onto two lines.
const SEG_IND = '<span class="sub-tab-ind" data-keep-style aria-hidden="true"></span>';
function placeIndicators(root){
  root.querySelectorAll('.ig-seg').forEach(seg => {
    const ind = seg.querySelector(':scope > .sub-tab-ind');
    const on = seg.querySelector(':scope > .sub-tab.on');
    if (!ind) return;
    if (!on || !on.offsetWidth){ ind.style.opacity = '0'; return; }
    ind.style.left = on.offsetLeft + 'px';
    ind.style.width = on.offsetWidth + 'px';
    ind.style.top = on.offsetTop + 'px';
    ind.style.height = on.offsetHeight + 'px';
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
    btn.disabled = !!btn._wantDisabled;
    delete btn._wantDisabled;
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
  // The same choice as the runner: `{"a":…}` or `[…]` is JSON, `{user}` a variable.
  const jsonBody = /^\s*(\[|\{\s*["}])/.test(s.params.body || '');
  const formBody = /application\/x-www-form-urlencoded/i.test(s.params.headers || '');
  const r = await api('/integrations/fetch', {
    method: s.params.method || 'POST',
    url: renderSample(s.params.url, vars, urlComponent).trim(),
    headers: renderSample(s.params.headers, vars, oneLine),
    body: renderSample(s.params.body, vars, jsonBody ? jsonStringContent : formBody ? urlComponent : null),
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
  // Resize events target the window, which isn't a Node.
  dismiss: e => { if (Menu.el && !(e && e.target instanceof Node && Menu.el.contains(e.target))) Menu.close(); },
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
      if (step.params.style === 'card'){
        const above = renderSample(step.params.above, vars).trim();
        return `<div class="${cls} ig-pv-discord"><span class="ig-av">${svg('shield')}</span><span class="ig-pv-col">
          <b>InstantClone</b><span class="ig-app">APP</span>
          ${ping || above ? `<div class="ig-txt">${discordMarkdown(ping + above)}</div>` : ''}${cardHtml(step, vars, big)}</span></div>`;
      }
      return `<div class="${cls} ig-pv-discord"><span class="ig-av">${svg('shield')}</span><span class="ig-pv-col">
        <b>InstantClone</b><span class="ig-app">APP</span><div class="ig-txt">${discordTimes(esc(ping + text))}</div></span></div>`;
    }
    case 'timeline': {
      const live = step.params.at !== 'aired';
      const at = vars[live ? 'vod_time' : 'uptime'] || '1:02:14';
      return `<div class="${cls} ig-pv-timeline"><span class="ig-pv-tl-kind ${esc(step.params.kind || 'note')}">${esc(TIMELINE_KINDS[step.params.kind] || TIMELINE_KINDS.note)}</span>
        <div><code>${esc(at)}</code>${discordMarkdown(text || 'A moment')}</div></div>`;
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
      return `<div class="${cls} ig-pv-action"><span>${svg('cut')}</span>${esc(delayActionLabel(step.params, vars))}</div>`;
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

// Run a program and Write a file. With no dashboard password, only the
// streaming PC itself may set them up: anyone on the network could reach
// the dashboard from another device and run anything from chat.
const LOCAL_KINDS = ['program', 'file'];
const LOCAL_LOCK_TEXT = 'Running programs and writing files can only be set up on the streaming PC, or here once the dashboard has a password (System › Security). Anyone on your network could use them otherwise.';
function localLocked(){ return !!S.data && S.data.local_steps === false; }
function runsLocally(i){ return allSteps(i).some(s => LOCAL_KINDS.includes(s.type)); }

// On, but it pushes to a phone that isn't connected.
function needsPhone(i){
  return !S.data.connections.phone.topic && allSteps(i).some(s => s.type === 'phone' && s.enabled !== false);
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
  else if (i.enabled && needsTwitch(i)) flag = {cls:'warn', label:'Needs Twitch', tip:S.data.twitch.available
    ? 'Connect Twitch in Connections' : 'This copy has no Twitch login app yet: System › Twitch login app'};
  else if (i.enabled && S.data.twitch_other_channel && allSteps(i).some(s => s.type === 'marker' || s.type === 'clip'))
    flag = {cls:'warn', label:'Other channel', tip:`Markers and clips go to ${S.data.twitch.main.login}'s channel, not the one you stream to. See Connections › Twitch.`};
  else if (i.enabled && needsPhone(i)) flag = {cls:'warn', label:'Needs your phone', tip:'Connect your phone in Connections'};
  else if (i.enabled && refusedKey(i)) flag = {cls:'warn', label:'Key taken', tip:`Another app holds ${refusedKey(i)}. Open it to pick another key.`};
  else if (localLocked() && runsLocally(i)) flag = {cls:'warn', label:'Streaming PC only', tip:LOCAL_LOCK_TEXT};
  const text = step && step.type === 'discord' && step.params.style === 'card'
    ? step.params.title || step.params.text : step && k && k.text ? step.params[k.text] : '';
  const quiet = i.enabled && quietNow(i.quiet);
  return {
    h, step, flag, failing, where,
    // What still blocks it, said under its name instead of in a tooltip.
    issue: !failing && issues.length ? capFirst(issues[0]) : '',
    dc: k ? k.c : 'var(--accent)',
    icon: PRESET_ICON[i.preset] || (k ? k.icon : 'steps'),
    last: quiet ? 'quiet until ' + i.quiet.to : st && st.last_ms ? fmtAgo(st.last_ms) : 'never ran',
    hist: histHtml(st),
    summary: text ? renderSample(text, sampleMap(h)) : (h ? triggerSentence(h.trigger) : ''),
  };
}

// Whether quiet hours (`{from:'23:00', to:'08:00'}`) cover this minute.
// Mirrors model::QuietHours::contains.
function capFirst(text){ return text ? text[0].toUpperCase() + text.slice(1) : text; }

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
  const sub = empty ? 'Alerts, chat commands and automations.'
    : `<span class="ig-num" data-tick>${on}</span> of ${d.integrations.length} running${failing.length ? ` · <span class="ig-num" data-tick>${failing.length}</span> need${failing.length === 1 ? 's' : ''} attention` : ''}.`;
  morph(root, `<div class="ig-pane" data-flip>
      <div class="tab-head" data-key="head">
        <div><div class="tab-title">Integrations <span class="ig-exp">Experimental</span></div><div class="tab-sub ig-sub">${sub}</div>
          <div class="ig-exp-note">New in 0.1.15 and experimental: they work, but how they look and how you set them up may still change. Found a bug or have an idea? <a href="https://github.com/Soulhackzlol/InstantClone/issues/new?template=integrations.yml" target="_blank" rel="noreferrer">Report it</a></div></div>
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
    ${pill('twitch', twitchOn ? (twitchLive ? 'ok' : 'warn') : '', 'Twitch', twitchOn ? (twitchLive ? 'connected' : 'chat ' + t.main.chat) : (t.available ? 'Connect' : 'Set up'))}
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
          ${c.issue ? `<div class="dcard-host ig-host-warn" title="${esc(c.issue)}">${esc(c.issue)}</div>`
            : `<div class="dcard-host">${esc(c.where)} · ${esc(c.last)}</div>`}</div>
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
          <span class="dcard-name">${esc(i.name)}</span><span class="dcard-host${c.issue ? ' ig-host-warn' : ''}">${esc(c.issue || c.where)}</span></button>
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
  const works = p => S.data.twitch.available || !(p.needs || []).includes('twitch');
  const best = Math.max(0, packs.findIndex(works));
  return `<div class="dest-empty" data-key="empty">
    <div class="dest-empty-title">What should InstantClone do for you?</div>
    <div class="dest-empty-sub">Pick a pack to add a few integrations at once. Anything that needs a connection waits, switched off, until it's connected. You can change or remove anything later.</div>
    <div class="dest-empty-grid ig-packs">
      ${packs.map((p, n) => `<button class="dest-starter ig-pack${n === best ? ' ig-pack-rec' : ''}" data-act="pack" data-pack="${esc(p.id)}">
        <span class="dest-starter-name">${esc(p.name)}${n === best ? ' <span class="dcard-status s-ready ig-rec">Recommended</span>' : ''}</span>
        <span class="dest-starter-note">${esc(p.description)}</span>
        <span class="ig-pack-list">${p.presets.map(id => `<span>${svg(PRESET_ICON[id] || 'steps')}${esc((preset(id) || {}).name || id)}</span>`).join('')}</span>
        ${needsHtml(p.needs)}
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
      : r.status === 'busy' ? 'skipped: too many runs at once'
      : r.status === 'cooldown' ? 'skipped: pressed again too soon (Repeat at most)'
      : r.steps.filter(s => s.status === 'ok').map(s => s.label).join(' · ') || r.trigger;
    const open = find(r.integration_id)
      ? ` data-act="edit" data-id="${esc(r.integration_id)}" role="button" tabindex="0" title="Open ${esc(r.name)}"` : '';
    return `<div class="ig-act-row${open ? ' link' : ''}" data-key="a-${esc(key)}"${open}><span class="t">${esc(fmtClock(r.at_ms))}</span>
      <span class="n">${esc(r.name)}${r.test ? ' <span class="muted">(test)</span>' : ''}</span>
      <span class="d" title="${esc(detail)}">${esc(r.trigger)} → ${esc(detail)}</span><span class="s ${esc(r.status)}">${esc(r.status === 'cooldown' ? 'skipped' : r.status)}</span></div>`;
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

// Add from the catalog. The integrations added, or none.
async function addFrom(body, openAfter){
  const fromCatalog = Modal.kind === 'catalog';
  const r = await api('/integrations/add', body);
  if (!r.ok){ toast(r.error || 'Could not add it', 'err', 5000); return []; }
  await load();
  const added = (r.ids || []).map(find).filter(Boolean);
  if (!added.length){ toast('You already have everything in this pack', 'info'); return []; }
  const ids = added.map(i => i.id);
  const unfinished = added.find(i => !i.enabled);
  if (openAfter && unfinished){
    // Going back from here means "not this one": the draft goes again.
    const back = fromCatalog ? {title:'Back to the catalog, without adding it', go:async () => {
      await deleteIds([unfinished.id]);
      Catalog.reopen();
    }} : null;
    openEditor(unfinished, true, {justAdded:true, back});
    return added;
  }
  undoToast(added.length > 1 ? `Added ${added.length} integrations` : `Added ${added[0] ? added[0].name : ''}`, async () => {
    await deleteIds(ids);
    if (Modal.kind === 'catalog') Catalog.render();
  });
  if (unfinished) toast(`${unfinished.name} needs one more detail before it can switch on`, 'info', 4500);
  if (Modal.kind === 'catalog' && body.preset) Catalog.flashAdded(body.preset);
  return added;
}

// A pack's drafts that only waited for a Discord channel get the first
// one, and switch on when nothing else is missing.
async function finishDrafts(ids){
  if (!S.data.connections.discord.length) return;
  let on = 0;
  for (const id of ids){
    const i = find(id);
    if (!i || !pickNewChannel(i)) continue;
    const draft = clone(i);
    const r = await api('/integrations/save', Object.assign(clone(draft), {enabled:true}));
    if (r.ok) on++;
    else await api('/integrations/save', draft);
  }
  await load();
  if (on) toast(`${plural(on, 'integration')} from the pack switched on`, 'ok', 4500);
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
    // As variables, not inline sizes, so the phone layout can override them.
    this.form.style.setProperty('--ig-w', `min(${width || 820}px,100%)`);
    this.form.style.setProperty('--ig-h', height || 'auto');
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
    Meter.stop();
    ObsLive.stop();
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
    // One layer at a time: the guard, then an open chip, then the modal.
    if (Modal.form.querySelector('.ig-guard')) Modal.hideGuard();
    else if (Modal.kind === 'editor' && Editor.closeChip()) return;
    else Modal.close();
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
  return `<label class="dest-form-enable"><input type="checkbox" data-bind="${bind}" ${on ? 'checked' : ''}><span class="dfe-track"><span></span></span><span data-tick>${label || (on ? 'On' : 'Off (draft)')}</span></label>`;
}
function lastRunHtml(d){
  const st = stat(d);
  if (!st || !st.last_ms) return '<span class="ig-last">Hasn\'t run yet</span>';
  const how = {ok:'worked', failed:'failed', stopped:'stopped early'}[st.last_status] || st.last_status;
  return `<span class="ig-last ${st.last_status === 'failed' ? 'bad' : 'ok'}"><i></i>Last ran ${esc(fmtAgo(st.last_ms))} · ${esc(how)}</span>`;
}
// The editors' footer: Delete away from Save, Save lit only with changes.
function footHtml(d, dirty, extra, canSave){
  const locked = localLocked() && runsLocally(d);
  return `<div class="dest-form-foot">
    ${d.id ? '<button class="ic-btn ic-btn-ghost ig-del" data-act="delete">Delete</button>' : ''}
    <div class="dest-form-msg muted ig-foot-msg">${d.id ? lastRunHtml(d) : '<span class="ig-last">New integration</span>'}${locked ? `<span class="ig-foot-need" title="${esc(LOCAL_LOCK_TEXT)}">Change it on the streaming PC</span>`
      : dirty ? '<span class="ig-unsaved">Unsaved</span>'
      : d.id && issuesOf(d).length ? `<span class="ig-foot-need">To switch it on: ${esc(issuesOf(d)[0])}</span>` : ''}</div>
    ${extra || ''}
    <button class="ic-btn ic-btn-primary" data-act="save" ${!locked && (dirty || canSave || !d.id) ? '' : 'disabled'} title="${locked ? esc(LOCAL_LOCK_TEXT) : 'Save (Ctrl+S)'}">Save</button>
  </div>`;
}
function runLogHtml(steps){
  if (!steps || !steps.length) return '<div class="muted">No steps ran.</div>';
  return `<div class="ig-run">${steps.map((s, i) => `<div style="--i:${i}"><span class="t">${(s.at_ms / 1000).toFixed(2)} s</span>
    <span class="${esc(s.status)}">${s.status === 'ok' ? '✓' : s.status === 'failed' ? '✕' : '○'} ${esc(s.label)}</span>
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

// `opts.stay`: the editor stays open after saving (the builder).
async function saveIntegration(d, onSaved, opts){
  const kind = Modal.kind;
  const r = await api('/integrations/save', d);
  if (!r.ok){ toast(r.error || 'Could not save', 'err', 6000); return false; }
  if (r.id) d.id = r.id;
  if (onSaved) onSaved();
  if (r.missing && r.missing.length){
    toast(`Saved as a draft. Still to do: ${r.missing.join('; ')}`, 'info', 7000);
  } else if (r.warnings && r.warnings.length){
    toast(`Saved. Check these names, nothing fills them: ${r.warnings.map(w => '{' + w + '}').join(', ')}`, 'info', 7000);
  } else toast('Saved', 'ok');
  if (Modal.kind === kind){
    await savedBeat();
    if (Modal.kind === kind && !(opts && opts.stay)) Modal.close(true);
  }
  await load();
  if (opts && opts.stay && Modal.kind === kind && OWNERS[kind]) OWNERS[kind].render();
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
    Modal.body(`${modalHead('plus', 'var(--accent)', 'Add an integration', 'Add it, then tweak anything.')}
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
      ${needsHtml(p.needs)}
      <button class="ic-btn ic-btn-primary ig-small" data-act="pack" data-pack="${esc(p.id)}" aria-label="Add the ${esc(p.name)} pack">Add pack</button>
    </div>`).join('');
  },
  presetsHtml(){
    // Without a Twitch app in this copy, what needs Twitch isn't a good
    // first pick: it stays in its own category, out of Recommended.
    const usable = p => S.data.twitch.available || !(p.needs || []).includes('twitch');
    const list = S.data.catalog.presets.filter(p => this.cat === 'rec' ? p.recommended && usable(p) : p.category === this.cat);
    const have = id => S.data.integrations.filter(i => i.preset === id).length;
    // Last: a beginner meets the ready-made ones first.
    const build = `<button class="dest-add ig-cat-build" data-act="build" data-key="build" style="--i:${list.length + 1}">
        <span class="dest-add-icon">${svg('plus')}</span>
        <span class="ig-cat-build-t">Build your own</span>
        <span class="ig-cat-build-d">Start from a blank page: any trigger, any steps.</span>
      </button>`;
    return list.map((p, n) => {
      const fake = fakeStep(p);
      const added = this.added === p.id;
      const blocked = !usable(p);
      const button = added
        ? `<button class="ic-btn ic-btn-ghost ig-small ig-added" data-act="add-preset" data-preset="${esc(p.id)}">${svg('check')}Added</button>`
        : `<button class="ic-btn ${have(p.id) || blocked ? 'ic-btn-ghost' : 'ig-cat-add'} ig-small" data-act="add-preset" data-preset="${esc(p.id)}"
            aria-label="${have(p.id) ? 'Add another' : 'Add'} ${esc(p.name)}">${have(p.id) ? 'Add another' : 'Add'}</button>`;
      return `<div class="dcard ig-cat-tile" style="--dc:${(KINDS[fake.step.type] || {}).c || 'var(--accent)'};--i:${n}" data-key="t-${esc(p.id)}">
        <div class="dcard-screen" aria-hidden="true">${previewHtml(fake.step, fake.handler, false)}</div>
        <div class="ig-cat-id"><span class="dcard-icon">${svg(PRESET_ICON[p.id] || 'steps')}</span>
          <div class="dcard-id"><div class="dcard-name">${esc(p.name)}</div><div class="ig-cat-desc" title="${esc(p.description)}">${esc(p.description)}</div>
          ${needsHtml(p.needs)}</div></div>
        ${button}
      </div>`;
    }).join('') + build;
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

// A catalog entry's preview: the app's own sample of what it sends, or,
// without one, its sample line.
// "!chapter Boss fight" in a catalog entry's example: its previews read
// "Boss fight", not the generic sample.
function sampleArgsFrom(handler, preset){
  const ask = (preset && preset.preview && preset.preview.ask) || '';
  const t = handler.trigger || {};
  if (t.type !== 'chat_command' || !ask.startsWith(t.command + ' ')) return;
  SAMPLE_ARGS.set(handler, ask.slice(t.command.length + 1));
}
function fakeStep(p){
  if (p.sample){
    const handler = clone(p.sample);
    sampleArgsFrom(handler, p);
    const step = primaryStep(handler);
    if (step) return {step, handler};
  }
  const pv = p.preview || {};
  const type = {discord:'discord', chat:'chat', phone:'phone', marker:'marker', json:'http', file:'file'}[pv.kind] || 'chat';
  const param = (KINDS[type] || {}).text || 'text';
  const step = {type, params:{[param]: pv.text || '', path:'crashes.txt', title:'OBS crashed',
    body: type === 'http' ? '{"event":"hold_opened","delay_ms":30000}' : ''}, then:[], else:[]};
  const handler = {trigger: pv.ask ? {type:'chat_command', command:pv.ask} : {type:'event', event:'hold_opened'}, steps:[step]};
  return {step, handler};
}
// What a catalog entry or pack needs, each as a pill: green when it's
// there, amber when it still has to be connected.
function needsHtml(needs){
  const c = S.data.connections, t = S.data.twitch;
  const pills = (needs || []).map(n => {
    switch (n){
      case 'discord': return c.discord.length ? ['ok', 'Discord'] : ['miss', 'Discord channel', 'You add it right after adding this'];
      case 'twitch': return t.main.login ? ['ok', 'Twitch'] : t.available ? ['miss', 'Twitch', 'Connect it in Connections']
        : ['miss', 'Twitch (set up first)', 'This copy has no Twitch login app yet: System › Twitch login app'];
      case 'phone': return c.phone.topic ? ['ok', 'Phone'] : ['miss', 'Phone', 'Connect it in Connections'];
      case 'web': return ['', 'An address', 'You paste the address it sends to'];
      case 'file': return ['', 'A file', 'You pick the file it writes'];
      case 'obs': return ['', 'OBS WebSocket', 'Switch it on in OBS: Tools › WebSocket Server Settings'];
    }
    return null;
  }).filter(Boolean);
  if (!pills.length) return '';
  return `<div class="ig-cat-needs"><span class="muted">Needs</span>${pills.map(([cls, label, tip]) =>
    `<span class="ig-need ${cls}"${tip ? ` title="${esc(tip)}"` : ''}>${cls === 'ok' ? svg('check', ' stroke-width="3"') : cls === 'miss' ? svg('warn') : ''}<span class="sr-only">${cls === 'ok' ? 'connected: ' : cls === 'miss' ? 'not connected yet: ' : ''}</span>${esc(label)}</span>`).join('')}</div>`;
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
    this.d.handlers.forEach(h => sampleArgsFrom(h, presetOf(this.d)));
    this.back = opts.back || null;
    this.justAdded = !!opts.justAdded;
    this.testValues = {};
    this.testDry = false;
    // Waiting for one detail: shown switched on, so saving once it is
    // complete also turns it on.
    this.dirty = !!switchOn && !this.d.enabled && !this.justAdded;
    if (switchOn) this.d.enabled = true;
    this.sel = Math.max(0, this.d.handlers.findIndex(h => h.enabled));
    // Missing a detail: open straight on the chip that fixes it.
    const known = id => S.data.connections.discord.some(c => c.id === id);
    const steps = allSteps(this.d);
    const sel = this.d.handlers[this.sel].trigger;
    const keyless = this.d.handlers.some(h => h.trigger.type === 'shortcut' && !h.trigger.hotkey && !h.trigger.midi && !h.trigger.token);
    this.chip = steps.some(s => s.type === 'discord' && !known(s.params.connection)) ? 'where'
      : steps.some(s => s.type === 'http' && !s.params.url) ? 'url' : keyless ? 'key'
      : sel.type === 'scene' && !sel.scene ? 'scene' : null;
    this.lastField = null;
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
    this.d.handlers.forEach(h => sampleArgsFrom(h, presetOf(this.d)));
    this.dirty = dirty;
    this.sel = Math.min(this.sel, this.d.handlers.length - 1);
    this.chip = null;
    this.run = null;
    this.show();
  },
  handler(){ return this.d.handlers[this.sel]; },
  // Esc on an open chip closes just the chip, back on its button.
  closeChip(){
    const open = this.chip;
    if (!open) return false;
    this.chip = null;
    this.render();
    const btn = q(`[data-act="chip"][data-chip="${open}"]`);
    if (btn) btn.focus({preventScroll:true});
    return true;
  },
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
    if (this.chip === 'detect') Meter.watch();
    if (this.chip === 'scene') ObsLive.watch();
  },
  // The moments it acts on, each with its switch. A moment that still
  // misses something says so in its colour and its tooltip.
  momentsHtml(){
    const hs = this.d.handlers;
    if (hs.length === 1) return '';
    const labels = momentLabels(hs);
    const dot = (h, n) => `<button class="ig-dot${h.enabled ? ' on' : ''}" role="switch" aria-checked="${h.enabled}" aria-label="Runs on: ${esc(labels[n])}"
      title="${h.enabled ? 'On: this moment runs. Click to skip it.' : 'Off: this moment is skipped. Click to turn it on.'}" data-act="moment-toggle" data-n="${n}">${svg('check', ' stroke-width="3.2"')}</button>`;
    const name = (n, cls) => {
      const missing = h => h.enabled && handlerMissing(h);
      const warn = missing(hs[n]);
      return `<button class="${cls}${n === this.sel ? ' on' : ''}${warn ? ' warn' : ''}"${n === this.sel ? ' aria-current="true"' : ''}
        data-act="moment" data-n="${n}"${warn ? ` title="${esc(capFirst(warn))}"` : ''}>${esc(labels[n])}</button>`;
    };
    // Too many for a timeline: chips that wrap, each keeping its switch.
    if (hs.length > 5){
      return `<div class="ig-mchips" role="group" aria-label="Moments">${hs.map((h, n) => `<div class="ig-mchip${n === this.sel ? ' on' : ''}">
        ${dot(h, n)}${name(n, 'ig-mchip-name')}</div>`).join('')}</div>`;
    }
    return `<div class="ig-moments" role="group" aria-label="Moments" style="--n:${hs.length}">${hs.map((h, n) => `<div class="ig-moment">
      ${dot(h, n)}${name(n, 'ig-moment-name')}</div>`).join('')}</div>`;
  },
  previewBoxHtml(step, h){
    if (!step || !(KINDS[step.type] || {}).text){
      const hint = step && step.type === 'delay_action' ? 'Pick what it does with <b>Does</b> below, or' : 'Nothing to write here.';
      return `<div class="dcard-screen ig-live-plain">${previewHtml(step, h, true)}
        <div class="muted">${hint} <a href="#" data-act="ed-builder">Open it in the builder</a> to see every step.</div></div>`;
    }
    const k = KINDS[step.type];
    const value = step.params[k.text] || '';
    const vars = sampleMap(h);
    // The trigger's own values first; the always-there ones behind "more".
    const all = varsFor(h);
    const common = new Set(['delay', 'delay_state', 'hold_left', 'time', 'uptime']);
    // Fields of a tried answer are picked from its tree, so they wait here.
    const shown = this.moreVars ? all : all.filter(v => (v.group !== 'live' || common.has(v.name)) && !v.name.includes('.json.'));
    const tokens = tokensHtml(shown, 'token',
      shown.length < all.length ? `<button class="ig-token more" data-act="more-vars">${all.length - shown.length} more…</button>` : '');
    const discord = step.type === 'discord';
    const sample = sampleLine(value, h, discord);
    // A tried answer can send the run down another branch ("offline").
    const reached = reachedStep(h.steps, vars, step.type);
    const instead = reached && reached !== step
      ? `<div class="ig-sample ig-instead">Right now it would say: <b>${esc(renderSample(reached.params[k.text], vars))}</b></div>` : '';
    const card = discord && step.params.style === 'card';
    const area = `<textarea class="ig-msg ig-tpl-in" data-bind="text" rows="1" aria-label="${card ? 'Card text' : 'Message'}" spellcheck="true" data-keep-style${card ? ' placeholder="Card text (optional)"' : ''}>${esc(value)}</textarea>`;
    const editor = tplHtml(area, value, vars, 'msg', discord)
      + `<div class="ig-sample${sample ? '' : ' empty'}">${sample}</div>${instead}`;
    let box;
    if (discord){
      const set = S.data.connections.discord.some(c => c.id === step.params.connection);
      const ping = step.params.ping === 'here' ? '@here' : step.params.ping === 'everyone' ? '@everyone' : '';
      const edit = step.params.edit || '';
      const note = {last:'updates its last message', close:'updates its last message, then starts fresh'}[edit];
      const body = card ? this.cardEditorHtml(step, h, editor) : editor;
      const channel = set ? esc(channelName(step.params.connection).replace(/^#/, ''))
        : '<button class="none" data-act="chip" data-chip="where">pick a channel</button>';
      box = `<div class="ig-live ig-live-discord">
        <div class="ig-live-head"><span class="hash${set ? '' : ' none'}">#</span>${channel}<span class="ig-live-tag">Live preview</span></div>
        <div class="ig-live-msg"><span class="ig-live-av">${svg('shield')}</span>
          <div class="ig-live-col"><div class="ig-live-meta"><b>InstantClone</b><span class="ig-app">APP</span><span class="ig-when">${esc(discordNow())}</span>
            ${ping && edit !== 'close' ? `<span class="ig-ping">${esc(ping)}</span>` : ''}
            ${note ? `<span class="ig-live-upd">${svg('pencil')}${note}</span>` : ''}</div>
          ${body}</div></div></div>`;
    } else if (step.type === 'chat'){
      const t = h.trigger;
      const ask = t.type === 'chat_command' ? `${t.command || '!command'}${SAMPLE_ARGS.has(h) ? ' ' + SAMPLE_ARGS.get(h) : ''}` : t.type === 'chat_message' ? (t.pattern || 'hey') : '';
      box = `<div class="ig-live ig-live-chat">
        <div class="ig-live-tag">STREAM CHAT · LIVE PREVIEW</div>
        ${ask ? `<div><b class="u1">viewer</b>: ${esc(ask)}</div>` : ''}
        <div><b class="u2">${esc(chatAuthor(step))}</b>:</div>${editor}</div>`;
    } else if (step.type === 'phone'){
      box = `<div class="ig-live ig-live-phone">
        <div class="ig-live-tag">INSTANTCLONE · now</div>
        ${tplHtml(`<input class="ic-input ig-tpl-in" data-bind="title" value="${esc(step.params.title || '')}" placeholder="Title" aria-label="Title">`, step.params.title, vars, 'line')}
        ${editor}</div>`;
    } else if (step.type === 'http' && this.d.preset === 'on_air_light'){
      const state = (h.trigger.filters || {}).state || 'live';
      box = `<div class="dcard-screen ig-live-plain">
        <div class="ic-label">When it turns ${esc({live:'live', delay:'delayed', crash:'crashed', off:'off'}[state] || state)}</div>
        <div>InstantClone tells your light, at ${esc(maskUrl(step.params.url) || 'the address you set in Light')}.</div>
        <details class="ig-more-fields"><summary>What it sends (advanced)</summary>${editor}</details></div>`;
      return box;
    } else {
      box = `<div class="dcard-screen ig-live-plain">
        <div class="ic-label">${esc(k.label)}${step.type === 'http' ? ' · what it sends' : ''}</div>${editor}</div>`;
    }
    return `${box}<div class="ig-tokens"><span>Insert</span>${tokens}</div>`;
  },
  // A Discord card you write on: the text above it, then the card itself
  // (title, body, fields, footer) in its color, then the color swatches.
  // Every part shows what it will say until it's being edited. `editor`
  // is the body's textarea with its "reads like" line.
  cardEditorHtml(step, h, editor){
    const p = step.params, vars = sampleMap(h);
    const color = cardColor(p.color, vars);
    const fields = parseFields(p.fields, true);
    const ts = p.timestamp === 'yes';
    const input = (bind, value, attrs, cls) => `<input class="ig-tpl-in${cls ? ' ' + cls : ''}" data-bind="${bind}" value="${esc(value || '')}" spellcheck="false" ${attrs}>`;
    const known = CARD_COLORS.some(([c]) => c === color);
    const swatches = CARD_COLORS.map(([c, label]) => {
      const on = c === color;
      return `<button class="ig-swatch${on ? ' on' : ''}" style="--sw:${c}" data-act="set-color" data-v="${c}" role="radio" aria-checked="${on}"
        tabindex="${on || (!known && c === CARD_COLORS[0][0]) ? 0 : -1}" aria-label="${label}" title="${label}"></button>`;
    }).join('');
    return `${tplHtml(`<input class="ig-above ig-tpl-in" data-bind="above" value="${esc(p.above || '')}" placeholder="Optional line above the card. A clip link here shows a video player." aria-label="Line above the card" spellcheck="false">`, p.above, vars, 'above', true)}
      <div class="ig-embed big edit" style="--ec:${esc(color)}">
        ${tplHtml(input('title', p.title, 'placeholder="Title" aria-label="Card title"', 'ig-embed-title-in'), p.title, vars, 'title', true)}
        ${editor}
        <div class="ig-fields-edit" data-flip>${fields.map((f, n) => `<div class="ig-field-edit${f.inline ? '' : ' full'}" style="--span:${fieldSpans(fields)[n]}" data-key="f-${n}">
          ${tplHtml(input('f-name', f.name, `data-n="${n}" placeholder="Name" aria-label="Field name"`), f.name, vars, 'line', false)}
          ${tplHtml(input('f-value', f.value, `data-n="${n}" placeholder="Value, like {reason}" aria-label="Field value"`), f.value, vars, 'line', true)}
          <button class="ig-field-w" data-act="f-inline" data-n="${n}" aria-pressed="${f.inline}" aria-label="Side by side"
            title="${f.inline ? 'Side by side with the next field. Click for full width.' : 'Full width. Click to sit side by side.'}">${svg(f.inline ? 'columns' : 'rows')}</button>
          <button class="ic-btn-tiny" data-act="f-del" data-n="${n}" aria-label="Remove this field">${svg('x')}</button></div>`).join('')}
          <button class="ig-field-add" data-act="f-add" data-key="f-add">${svg('plus')}Add a field</button></div>
        <div class="ig-embed-foot edit"><i>${svg('shield')}</i>
          ${tplHtml(input('footer', p.footer, 'placeholder="Footer" aria-label="Footer"'), p.footer, vars, 'line', false)}
          ${ts && p.footer ? '<span class="ig-foot-dot" aria-hidden="true">•</span>' : ''}
          <button class="ig-ts${ts ? ' on' : ''}" data-act="set-ts" aria-pressed="${ts}">${svg('clock')}${ts ? esc(discordNow()) : 'Add the time'}</button></div>
      </div>
      <div class="ig-swatches"><span class="ig-swatch-set" role="radiogroup" aria-label="Card color">${swatches}</span>
        <label class="ig-swatch custom${known ? '' : ' on'}" title="Any color"><input type="color" data-bind="color" value="${esc(color)}" aria-label="Any color (now ${esc(color)})"></label></div>`;
  },
  // Edit the card's fields: `change` gets the list, the text is rebuilt.
  fields(step, change){
    const list = parseFields(step.params.fields, true);
    change(list);
    step.params.fields = fieldsText(list);
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
        const ping = step.params.ping || '', edit = step.params.edit || '';
        out.push({id:'look', k:'Look', v:step.params.style === 'card' ? 'card' : 'plain message'});
        out.push({id:'msg', k:'Each time', v:{last:'updates the same message', close:'updates it, then starts fresh'}[edit] || 'posts a new message'});
        // Finishing a message never pings; the others can post a new one.
        if (edit !== 'close') out.push({id:'ping', k:'Ping', v:ping === 'here' ? '@here' : ping === 'everyone' ? '@everyone' : 'nobody'});
      }
    }
    if (t.type === 'shortcut') out.push({id:'key', k:'Starts with', v:shortcutLabel(t) || 'set a key, pad or Stream Deck', warn:!t.hotkey && !t.midi && !t.token});
    const action = firstStep(h, 'delay_action');
    if (action) out.push({id:'action', k:'Does', v:lowerFirst(delayActionLabel(action.params, {}))});
    if (t.type === 'chat_activity') out.push({id:'detect', k:'Fires when', v:activitySummary(t), warn:!(t.rules || []).length});
    if (t.type === 'scene') out.push({id:'scene', k:'Scene', v:t.scene ? sceneLabel(t) : 'pick a scene', warn:!t.scene});
    const tune = t.type === 'event' && tuneSummary(t);
    if (tune) out.push({id:'tune', k:tune.k, v:tune.v, warn:tune.bad});
    if (t.type === 'event' || t.type === 'timer' || t.type === 'webhook') out.push({id:'send', k:'Send', v:this.sendValue(h).label});
    if (t.type === 'event' && this.filterVars(t).length && d.preset !== 'on_air_light'){
      const set = Object.entries(t.filters || {}).filter(([k, v]) => v && !isParam(t, k));
      out.push({id:'only', k:'Only if', v:set.length ? set.map(([k, v]) => `${filterName(k)}: ${v}`).join(', ') : 'no conditions'});
    }
    if (t.type === 'chat_command'){
      out.push({id:'cmd', k:'Command', v:[t.command].concat(t.aliases || []).join(' ')});
      out.push({id:'who', k:'Who can use it', v:rolesLabel(t.roles)});
    }
    if (step && step.type === 'chat') out.push({id:'reply', k:'Reply as', v:step.params.as === 'bot' ? 'bot account' : 'your account'});
    if (step && step.type === 'phone') out.push({id:'priority', k:'Priority', v:step.params.priority || 'normal'});
    const http = firstStep(h, 'http');
    if (http && d.preset === 'on_air_light') out.push({id:'url', k:'Light', v:lightOf(http, t) || maskUrl(http.params.url) || 'pick your light', warn:!http.params.url});
    else if (http) out.push({id:'url', k:t.type.startsWith('chat') ? 'Answer from' : 'Send to', v:maskUrl(http.params.url) || 'set the address', warn:!http.params.url});
    if (step && step.type === 'file') out.push({id:'file', k:'File', v:step.params.path || 'pick a file', warn:!step.params.path});
    out.push({id:'cooldown', k:'Repeat at most', v:d.cooldown_ms ? 'once every ' + fmtDur(d.cooldown_ms) : 'anytime'});
    out.push({id:'quiet', k:'Quiet hours', v:d.quiet ? `${d.quiet.from} to ${d.quiet.to}` : 'off'});
    return out;
  },
  // Values worth narrowing on ("only YouTube", "only crashes"); counts,
  // durations and reports change every time.
  filterVars(t){
    const e = eventOf(t.event);
    const skip = new Set(['hold', 'hold_ends_at', 'down_for', 'down_s', 'down_total', 'previous', 'drops', 'within',
      'duration', 'crashes', 'highlights', 'report', 'chapters', 'line']);
    return e ? e.vars.filter(v => !skip.has(v.name)) : [];
  },
  sendValue(h){
    const first = (h.steps || [])[0];
    if (first && first.type === 'wait_delay') return {id:'aired', label:'once viewers see it'};
    if (first && first.type === 'wait'){
      const ms = parseInt(first.params.ms, 10) || 0;
      return {id:'w' + ms, label:'after ' + fmtMs(ms)};
    }
    return {id:'now', label:'right away'};
  },
  popHtml(step, h){
    if (this.chip === '__test') return testPanelHtml(this);
    const d = this.d, t = h.trigger;
    const seg = (items, cur, act) => `<div class="sub-tabs ig-seg">${items.map(([id, label]) =>
      `<button class="sub-tab${id === cur ? ' on' : ''}" data-act="${act}" data-v="${esc(id)}" aria-pressed="${id === cur}">${esc(label)}</button>`).join('')}${SEG_IND}</div>`;
    switch (this.chip){
      case 'where': {
        const cur = (allSteps(d).find(s => s.type === 'discord') || {params:{}}).params.connection;
        return `<div class="ic-label">Post in</div><div class="ig-pick">
          ${S.data.connections.discord.map(c => `<button class="dest-starter${c.id === cur ? ' on' : ''}" data-act="set-where" data-v="${esc(c.id)}" aria-pressed="${c.id === cur}">
            <span class="dest-starter-name">#${esc(c.name)}</span><span class="dest-starter-note mono">${esc(c.hint)}</span></button>`).join('')}
          <button class="dest-add ig-pick-add" data-act="conn" data-tab="discord">${svg('plus')}Add a Discord channel</button></div>`;
      }
      case 'look':
        return `<div class="ic-label">Look</div>${seg([['', 'Plain message'], ['card', 'Card']], step.params.style || '', 'set-style')}
          <div class="muted">A card has a colored edge, a title, fields side by side and a timestamp. Write on it right in the preview.</div>`;
      case 'msg': {
        const edit = step.params.edit || '';
        const key = step.params.key || '';
        const perPlatform = /\{destination\}/.test(key);
        const canSplit = varsFor(h).some(v => v.name === 'destination');
        return `<div class="ic-label">Each time it runs</div>${seg([['', 'Post a new message'], ['last', 'Update the same message'], ['close', 'Update it, then start fresh']], edit, 'set-edit')}
          <div class="muted">${{
            last:'Edits the message it posted there before, so one alert goes from "down" to "back" instead of piling up. If someone deleted it, a new one goes out.',
            close:'Edits the message it posted before into its final state, then lets it go: the next alert starts a new message. With nothing to finish (a short blip never got a message), it stays quiet.',
          }[edit] || 'Every run posts its own message.'}</div>
          ${edit && canSplit ? `<label class="crash-check"><input type="checkbox" data-bind="key-split" ${perPlatform ? 'checked' : ''}><span>
            <span class="crash-check-title">A separate message for each platform</span>
            <span class="muted">YouTube and Kick each get their own message instead of sharing one.</span></span></label>` : ''}
          ${edit ? `<details class="ig-more-fields"${key && !perPlatform && key !== '{destination}' ? ' open' : ''}><summary>Message group (advanced)</summary>
            <div class="dff ig-narrow"><input class="ic-input mono" data-bind="key" value="${esc(key)}" placeholder="none" spellcheck="false" aria-label="Message group"></div>
            <div class="muted">Moments with the same group update the same message: in the ready-made alerts, "is down" and "is back" share one. Change it on both, or they stop finding each other.</div></details>` : ''}`;
      }
      case 'action': {
        const a = firstStep(h, 'delay_action');
        const secs = a.params.seconds || '';
        const field = /\{/.test(secs)
          ? `<input class="ic-input mono" data-bind="seconds" value="${esc(secs)}" placeholder="30">`
          : durInput('seconds-dur', (parseFloat(secs) || 30) * 1000, 's');
        return `<div class="ic-label">Does</div>${seg(['arm', 'cut_after', 'cut', 'activate', 'disarm'].map(x => [x, DELAY_ACTIONS[x]]), a.params.action, 'set-action')}
          ${a.params.action === 'arm' ? `<div class="dff ig-narrow"><label>Delay</label>${field}</div>` : ''}
          <div class="muted">${{
            arm:'Starts this delay, or changes it while one is on air.',
            cut_after:'Goes back to live once viewers have caught up with you, so they miss nothing. It takes as long as the delay.',
            cut:'Goes back to live right away. Viewers skip what was still in the delay.',
            activate:'Puts on air the delay that finished getting ready.',
            disarm:'No delay at all, and none getting ready.',
          }[a.params.action] || ''} Tests never touch the stream.</div>`;
      }
      case 'detect': return rulesHtml(t, '');
      case 'scene': return sceneHtml(t, '', this.d);
      case 'tune': {
        const tune = tuneSummary(t);
        return `<div class="ic-label">${esc(tune.k)}</div>${paramsHtml(t, '')}<div class="muted">Each moment keeps its own numbers: a phone push can wait longer than the Discord card.</div>`;
      }
      case 'key': return shortcutHtml(t);
      case 'quiet': return quietHtml(d);
      case 'ping':
        return `<div class="ic-label">Ping</div>${seg([['', 'Nobody'], ['here', '@here'], ['everyone', '@everyone']], step.params.ping || '', 'set-ping')}
          <div class="muted">${step.params.edit === 'last' ? 'Only when it posts a new message: an update never pings. ' : ''}Text from chat or a web service can never ping anyone: only this option can.</div>`;
      case 'send':
        return `<div class="ic-label">When to send</div>${seg([['now', 'Right away'], ['w10000', '10 s'], ['w20000', '20 s'], ['w30000', '30 s'], ['w60000', '1 min'], ['aired', 'Once viewers see it']],
          this.sendValue(h).id, 'set-send')}
          <div class="muted">${t.type === 'event' && t.event === 'hold_opened' ? 'Waiting a bit means a 3-second blip never reaches anyone.' : '"Once viewers see it" waits out your delay, so the moment has reached them.'}</div>`;
      case 'only':
        return `<div class="ic-label">Only if</div><div class="dfg">${this.filterVars(t).map(v => {
          const cur = (t.filters || {})[v.name] || '';
          const listed = filterChoices(t.event, v.name);
          const choices = listed && (!cur || listed.includes(cur)) ? listed : null;
          const input = choices
            ? `<select class="ic-input" data-bind="filter" data-name="${esc(v.name)}"><option value="">any</option>${choices.map(c => `<option ${c === cur ? 'selected' : ''}>${esc(c)}</option>`).join('')}</select>`
            : `<input class="ic-input" data-bind="filter" data-name="${esc(v.name)}" value="${esc(cur)}" placeholder="any, like ${esc(v.sample)}">`;
          return `<div class="dff"><label>${esc(capFirst(filterName(v.name)))}</label>${input}</div>`;
        }).join('')}</div>
        <div class="muted">Several at once: YouTube, Kick. A * matches anything: *timed out*. Capitals don't matter.</div>`;
      case 'cmd':
        return `<div class="dfg"><div class="dff"><label>Command</label><input class="ic-input mono" data-bind="command" value="${esc(t.command)}" spellcheck="false"></div>
          <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="aliases" value="${esc((t.aliases || []).join(' '))}" placeholder="!retraso !d" spellcheck="false"></div></div>
          <div class="dff"><label>Each viewer, at most once every</label>${seg([['0', 'no limit'], ['10000', '10 s'], ['30000', '30 s'], ['60000', '1 min']], String(t.user_cooldown_ms || 0), 'set-ucd')}</div>`;
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
        if (d.preset === 'on_air_light') return this.lightHtml(http, t);
        return `${chatty && d.preset === 'api_command' ? this.startsHtml(http) : ''}<div class="dfg">${secretField('e-url', `<input class="ic-input mono ig-secret" data-bind="url" value="${esc(http.params.url || '')}" placeholder="${chatty ? 'https://… or $(urlfetch https://…)' : 'https://…'}" spellcheck="false" autocomplete="off">`,
            chatty ? 'Address, or a bot command to convert' : 'Address')}
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
        return `<div class="ic-label">Repeat at most</div>${seg([['0', 'Anytime'], ['10000', 'Every 10 s'], ['30000', '30 s'], ['60000', '1 min'], ['300000', '5 min'], ['600000', '10 min']], String(d.cooldown_ms || 0), 'set-cooldown')}
          <div class="dff ig-narrow"><label>Or once every</label>${durInput('cooldown-dur', d.cooldown_ms, 's')}</div>
          <div class="muted">For every moment here, each counting on its own. A button pressed too soon shows as skipped in Recent activity.</div>`;
    }
    return '';
  },
  // The on-air light's device and address. Picking a device fills in
  // what each state sends it; the address is the user's own.
  lightHtml(http, t){
    const starts = S.data.catalog.light_starts || [];
    const device = lightStart(http, t);
    const help = {
      home_assistant:'In Home Assistant: Settings › Automations › Create, add a Webhook trigger with the id instantclone-onair. InstantClone sends live, delay, crash or off; use {{ trigger.json.state }} there to choose the color.',
      wled:'Your WLED light\'s address. The WLED app shows it (Config › WiFi Setup).',
      hue:'Your Hue bridge\'s address, a username from the bridge, and the light\'s number. The Hue app shows the bridge\'s IP.',
    };
    const example = {home_assistant:'http://homeassistant.local:8123/api/webhook/instantclone-onair', wled:'http://192.168.1.50/json/state',
      hue:'http://192.168.1.20/api/USERNAME/lights/1/state'};
    return `<div class="ic-label">Your light</div><div class="ig-starts">${starts.map(x =>
        `<button class="ig-start${device && device.id === x.id ? ' on' : ''}" data-act="light-start" data-v="${esc(x.id)}" aria-pressed="${!!device && device.id === x.id}">${esc(x.label)}</button>`).join('')}</div>
      ${secretField('e-light-url', `<input class="ic-input mono ig-secret" data-bind="url" value="${esc(http.params.url || '')}"
        placeholder="${esc(device ? example[device.id] || '' : 'Pick your light first')}" spellcheck="false" autocomplete="off">`, 'Its address')}
      <div class="muted">${device ? help[device.id] || '' : 'Pick your light: InstantClone fills in what each state sends it.'} Then <b>Send a test</b> lights it up for the moment picked above.</div>
      <details class="ig-more-fields"><summary>Advanced: method</summary><div class="dff ig-narrow"><select class="ic-input" data-bind="method" aria-label="Method">
        ${['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map(m => `<option ${m === (http.params.method || 'POST') ? 'selected' : ''}>${m}</option>`).join('')}</select></div>
        <div class="muted">What each state sends is in the preview: pick a moment (live, delayed…) and open "What it sends".</div></details>`;
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
      // A catalog webhook or light sends to one address: set them all.
      allSteps(this.d).filter(s => s.type === 'http').forEach(s => { s.params[b] = el.value; });
      if (b === 'url') this.converted = null;
    }
    else if (b === 'try'){
      // Values to try the request with; not part of the integration.
      tryEntry(firstStep(h, 'http')).inputs[el.dataset.name] = el.value;
      return;
    }
    else if (b === 'test-value'){ (this.testValues = this.testValues || {})[el.dataset.name] = el.value; return; }
    else if ((b === 'path' || b === 'mode') && step) step.params[b] = el.value;
    else if ((b === 'above' || b === 'footer' || b === 'key' || b === 'color') && step) step.params[b] = el.value;
    else if (b === 'key-split' && step) splitKey(step, el.checked);
    else if ((b === 'f-name' || b === 'f-value') && step){
      this.fields(step, list => {
        const f = list[+el.dataset.n];
        if (f) f[b === 'f-name' ? 'name' : 'value'] = el.value;
      });
    }
    else if (b === 'param'){ if (!paramInput(t, el)) return; }
    else if (b === 'seconds' && firstStep(h, 'delay_action')) firstStep(h, 'delay_action').params.seconds = el.value.trim();
    else if (b === 'seconds-dur' && firstStep(h, 'delay_action')){
      const ms = durValue(el, 1000, 600000);
      if (ms == null) return;
      firstStep(h, 'delay_action').params.seconds = String(ms / 1000);
    }
    else if (b === 'cooldown-dur'){
      const ms = durValue(el, 0, 86400000);
      if (ms == null) return;
      this.d.cooldown_ms = ms;
    }
    else if (!rulesInput(t, el, '') && !sceneInput(t, el, '') && !onlyLiveInput(t, el, '') && !quietInput(this.d, el)) return;
    this.changed();
  },
  changed(){
    this.dirty = true;
    this.edits = (this.edits || 0) + 1;
    syncSceneRules(this.d);
    this.render();
  },
  // Point every state of the on-air light at one device, with what that
  // device needs to hear for each state. An address the user typed stays.
  useLight(id){
    const starts = S.data.catalog.light_starts || [];
    const start = starts.find(x => x.id === id);
    if (!start) return;
    this.d.handlers.forEach(h => {
      const state = (h.trigger.filters || {}).state, http = firstStep(h, 'http');
      if (!http) return;
      http.params.method = start.method;
      const url = (http.params.url || '').trim();
      if (!url || starts.some(x => x.url === url)) http.params.url = start.url;
      if (start.bodies[state]) http.params.body = start.bodies[state];
    });
    this.changed();
  },
  act(a, el){
    const h = this.handler(), step = primaryStep(h), v = el.dataset.v;
    if (rulesAct(h.trigger, a, el) || sceneAct(h.trigger, a, el)){ this.changed(); return true; }
    switch (a){
      case 'set-style': if (step) step.params.style = v; this.changed(); return true;
      case 'set-color': if (step) step.params.color = v; this.changed(); return true;
      case 'set-ts': if (step) step.params.timestamp = step.params.timestamp === 'yes' ? '' : 'yes'; this.changed(); return true;
      case 'f-add': {
        if (!step) return true;
        this.fields(step, list => list.push({name:'', value:'', inline:true}));
        this.changed();
        const names = Modal.form.querySelectorAll('[data-bind="f-name"]');
        if (names.length) names[names.length - 1].focus();
        return true;
      }
      case 'f-del': if (step) this.fields(step, list => list.splice(+el.dataset.n, 1)); this.changed(); return true;
      case 'f-inline':
        if (step) this.fields(step, list => { const f = list[+el.dataset.n]; if (f) f.inline = !f.inline; });
        this.changed();
        return true;
      case 'light-start': this.useLight(v); return true;
      case 'set-action': {
        const a = firstStep(h, 'delay_action');
        if (a){
          a.params.action = v;
          if (v === 'arm' && !a.params.seconds) a.params.seconds = '30';
        }
        this.changed();
        return true;
      }
      case 'moment':
        this.sel = +el.dataset.n;
        this.testValues = {};
        this.run = null;
        if (this.chip === '__test') this.chip = null;
        this.render();
        return true;
      case 'moment-toggle': { const x = this.d.handlers[+el.dataset.n]; x.enabled = !x.enabled; this.changed(); return true; }
      case 'chip': this.chip = this.chip === el.dataset.chip ? null : el.dataset.chip; this.render(); return true;
      case 'token': {
        // Into the card field last written in, or the message itself.
        const field = this.lastField && this.lastField.isConnected ? this.lastField : q('.ig-msg');
        insertAtCursor(field, '{' + el.dataset.token + '}');
        return true;
      }
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
      case 'ed-test': this.chip = '__test'; this.render(); focusAct('test-run'); return true;
      case 'test-run': busy(el, () => this.test()); return true;
      case 'test-mode': this.testDry = v === 'dry'; this.render(); return true;
      case 'delete': if (armConfirm(el, 'Delete for good?')) busy(el, () => remove(this.d.id)); return true;
      case 'save': {
        const at = this.edits;
        busy(el, () => saveIntegration(this.d, () => { if (this.edits === at) this.dirty = false; }));
        return true;
      }
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
    this.run = {running:true};
    this.render();
    const r = await api('/integrations/test', {integration:this.d, handler:this.sel, values:testValues(this), dry:!!this.testDry});
    this.run = r.ok ? r : {error:r.error || 'The test failed'};
    if (Modal.kind === 'editor') this.render();
  },
};

// A test's panel, the same in both editors: how it runs (for real or
// dry), the values it runs with, and what each step did.
function testPanelHtml(o){
  const h = o.handler();
  const own = varsFor(h).filter(v => v.group === 'trigger');
  const values = o.testValues || {};
  const mode = `<div class="sub-tabs ig-seg">${[['', 'Send it for real'], ['dry', 'Dry run']].map(([id, label]) => {
    const on = (id === 'dry') === !!o.testDry;
    return `<button class="sub-tab${on ? ' on' : ''}" data-act="test-mode" data-v="${id}" aria-pressed="${on}">${label}</button>`;
  }).join('')}${SEG_IND}</div>`;
  const inputs = own.length ? `<details class="ig-more-fields"${Object.keys(values).length ? ' open' : ''}><summary>Test with other values</summary>
    <div class="ig-test-values">${own.map(v => `<div class="dff"><label title="{${esc(v.name)}}">${esc(capFirst(varLabel(v.name)))}</label>
      <input class="ic-input" data-bind="test-value" data-name="${esc(v.name)}" value="${esc(v.name in values ? values[v.name] : v.sample)}"></div>`).join('')}</div></details>` : '';
  const run = o.run;
  const sent = run && (run.steps || []).some(st => st.status !== 'skipped');
  const verdict = run && (run.status === 'ok' ? (sent ? 'everything worked' : 'nothing was sent, see why below')
    : {failed:'a step failed', stopped:'stopped early'}[run.status] || run.status);
  const result = !run ? '' : run.running ? '<div class="ig-running"><span class="ig-spin"></span>Running the test…</div>'
    : run.error ? warnHtml(esc(run.error))
    : `<div class="ic-label">Result: ${esc(verdict)}</div>${runLogHtml(run.steps)}`;
  const note = o.testDry ? 'A dry run sends nothing at all: each step says what it would have sent.'
    : 'Messages go out for real, marked [TEST], and never ping anyone. Web requests go out too: a light changes and stays that way. Waits are skipped; the stream, the VOD, OBS, programs and files are left alone.';
  return `<div class="ig-testbox"><div class="ig-test-head">${mode}
    <button class="ic-btn ic-btn-primary ig-small" data-act="test-run">${svg('play', ' class="ig-btn-ic"')}${run && !run.running ? 'Run again' : 'Run the test'}</button></div>
    ${inputs}<div class="ig-test-result" aria-live="polite">${result}</div><div class="muted">${note}</div></div>`;
}
// What a test runs with: every value the panel shows, edited or not.
function testValues(o){
  const values = {};
  varsFor(o.handler()).filter(v => v.group === 'trigger').forEach(v => { values[v.name] = v.sample; });
  return Object.assign(values, o.testValues || {});
}

// What a moment still misses before it can run, or ''.
function handlerMissing(h){
  const t = h.trigger;
  if (t.type === 'scene' && !t.scene) return 'pick the scene';
  if (t.type === 'shortcut' && !t.hotkey && !t.midi && !t.token) return 'set a key, pad or Stream Deck';
  let missing = '';
  const look = steps => (steps || []).forEach(s => {
    if (missing || s.enabled === false) return;
    missing = stepMissing(s);
    look(s.then);
    look(s.else);
  });
  look(h.steps);
  return missing;
}
// What a step still misses, or ''.
function stepMissing(s){
  const p = s.params || {};
  if (s.type === 'discord' && !S.data.connections.discord.some(c => c.id === p.connection)) return 'pick a Discord channel';
  if (s.type === 'http' && !(p.url || '').trim()) return 'set the address';
  if (s.type === 'http' && /BRIDGE-IP|USERNAME/.test(p.url || '')) return 'replace BRIDGE-IP and USERNAME with your bridge\'s';
  if (s.type === 'phone' && !S.data.connections.phone.topic) return 'connect your phone';
  if (s.type === 'discord' && p.style === 'card' && !['title', 'text', 'fields', 'image', 'above'].some(k => (p[k] || '').trim())) return 'give the card a title or some text';
  if (s.type === 'if' && !(p.left || '').trim()) return 'say what it checks';
  if ((s.type === 'set_var' || s.type === 'counter') && !(p.name || '').trim()) return 'name the value';
  if (s.type === 'wait' && !/\{|^\d+$/.test((p.ms || '').trim())) return 'set how long to wait';
  if ([p.scene, p.source, s.type === 'program' || s.type === 'file' ? p.path : ''].some(v => /\{/.test(v || ''))) return 'names here can\'t use values';
  if ((s.type === 'program' || s.type === 'file') && !(p.path || '').trim()) return s.type === 'file' ? 'pick the file' : 'pick the program';
  if (s.type === 'obs'){
    const need = (OBS_ACTIONS[p.action] || [])[1];
    if (!need) return 'pick what to do in OBS';
    if (!(p[need] || '').trim()) return need === 'scene' ? 'pick the scene' : 'pick the source';
  }
  if ((s.type === 'chat' || s.type === 'phone' || s.type === 'overlay' || s.type === 'timeline') && !(p.text || '').trim()) return 'write what it says';
  if (s.type === 'discord' && p.style !== 'card' && !(p.text || '').trim()) return 'write the message';
  return '';
}

// "A separate message for each platform": the message group follows the
// platform, or (unticked) goes. What it was is kept, so ticking it again
// brings back "flap {destination}" rather than a plain "{destination}"
// that would share the "is down" card's message.
const KEYS_WERE = new Map();
function splitKey(step, on){
  const id = uidOf(step);
  if (on) step.params.key = KEYS_WERE.get(id) || '{destination}';
  else { KEYS_WERE.set(id, step.params.key || ''); step.params.key = ''; }
}

// Scene delay: the "any other scene" rule never counts the game scene,
// whatever the game scene is called.
function syncSceneRules(d){
  if (d.preset !== 'scene_delay') return;
  const arm = d.handlers.find(h => h.trigger.type === 'scene' && (firstStep(h, 'delay_action') || {params:{}}).params.action === 'arm');
  if (!arm) return;
  d.handlers.forEach(h => {
    if (h !== arm && h.trigger.type === 'scene' && h.trigger.scene === '*') h.trigger.except = arm.trigger.scene || '';
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
  return p.action === 'arm' && seconds ? `Turn on a ${seconds} s delay` : (DELAY_ACTIONS[p.action] || 'Delay action');
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
  ${refused}<div class="muted">Works while you're in a game. Use any of them, or all. Keys and pads that run the delay (Controls tab) stay theirs.</div>
  ${buttonAddressHtml(t)}
  ${onlyLiveHtml(t, '', 'Only while streaming', 'Presses while OBS isn\'t streaming do nothing.')}`;
}
function focusAct(act){
  const el = q(`[data-act="${act}"]`);
  if (el) el.focus({preventScroll:true});
}

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
  const seg = (items, cur, act) => `<div class="sub-tabs ig-seg">${items.map(([id, label]) =>
    `<button class="sub-tab${id === cur ? ' on' : ''}" data-act="${act}" data-v="${esc(id)}" aria-pressed="${id === cur}">${esc(label)}</button>`).join('')}${SEG_IND}</div>`;
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
    <div class="ig-rules-head"><span>Fires when</span>${seg([['all', 'all'], ['any', 'any']], t.match === 'any' ? 'any' : 'all', 'r-match')}<span>of these are true, over the last</span>
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
    const unit = /second/i.test(p.label) ? 's' : /minute/i.test(p.label) ? 'min' : '';
    const label = p.label.replace(/\s*\(.*\)/, '');
    return `<div class="dff"><label>${esc(label)}${unit ? ` <span class="muted">(${unit})</span>` : ''}</label>
      <input class="ic-input mono ig-param" data-bind="${bind}param" data-name="${esc(p.name)}" inputmode="decimal" value="${esc((t.filters || {})[p.name] || p.default)}"></div>`;
  }).join('')}</div>`;
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

// ---------------------------------------------------------------- builder

// The step editor: any trigger, any steps, checks with then/otherwise.
// Selection and insertion points are paths like [2,'then',0].
const PALETTE = [
  ['Send', [['discord', 'Discord'], ['chat', 'Twitch chat'], ['phone', 'Phone'], ['http', 'Web request']]],
  ['Flow', [['if', 'If / otherwise'], ['wait', 'Wait'], ['wait_delay', 'Wait for the delay'], ['stop', 'Stop']]],
  ['Stream', [['overlay', 'Show on stream'], ['delay_action', 'Delay action'], ['obs', 'OBS'], ['marker', 'VOD marker'], ['clip', 'Clip'], ['timeline', 'Timeline']]],
  ['Anything', [['edit_text', 'Edit text'], ['set_var', 'Remember a value'], ['counter', 'Counter'], ['program', 'Run a program'], ['file', 'Write a file']]],
];
// What can start an integration, shown as tiles so every kind is in sight.
const TRIGGERS = [
  {id:'event', icon:'bolt', label:'Something happens', hint:'OBS crashes, a platform drops, the delay changes…',
    eg:'OBS crashes: tell the mods on Discord'},
  {id:'chat_command', icon:'hash', label:'A chat command', hint:'!rank, !delay, anything', chat:true,
    eg:'!rank answers with your rank'},
  {id:'chat_message', icon:'bubble', label:'A chat message', hint:'When chat says something', chat:true,
    eg:'Someone says "gg": thank them'},
  {id:'chat_activity', icon:'flame', label:'Chat gets busy', hint:'Rules you stack: speed, people, words', chat:true,
    eg:'Chat goes 3× faster: clip it'},
  {id:'timer', icon:'clock', label:'On a timer', hint:'Every few minutes', eg:'Every 15 min: remind chat to follow'},
  {id:'shortcut', icon:'keyboard', label:'A button', hint:'Hotkey, MIDI pad or Stream Deck, even in game',
    eg:'Ctrl+Alt+C clips and posts the link'},
  {id:'scene', icon:'layers', label:'An OBS scene', hint:'When OBS switches scene', eg:'The game scene turns the delay on'},
  {id:'webhook', icon:'link', label:'A web call', hint:'Scripts and apps that send data', eg:'n8n sends a message to post'},
];
// Why a trigger tile can't be picked right now, or ''.
function triggerBlocked(k){
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
  destination_live:{icon:'megaphone', desc:'YouTube, Kick or another platform starts showing your stream.'},
  destination_dropped:{icon:'warn', desc:'A platform loses your stream, even for a moment.'},
  all_destinations_down:{icon:'signal', desc:'No platform is live any more.'},
  delay_on:{icon:'clock', desc:'The delay starts.'},
  delay_off:{icon:'clock', desc:'The delay stops: viewers see you live.'},
  delay_changed:{icon:'clock', desc:'The delay gets longer or shorter.'},
  delay_armed:{icon:'clock', desc:'You arm one: the buffer starts filling.'},
  delay_ready:{icon:'check', desc:'The buffer holds it: it can go on air.'},
  delay_disarmed:{icon:'x', desc:'You cancel it before it goes on air.'},
  stream_started:{icon:'play', desc:'A new stream begins. A crash OBS comes back from isn\'t one.'},
  stream_ended:{icon:'film', desc:'The stream is over, with a report: length, crashes, drops.'},
  onair_changed:{icon:'bulb', desc:'Live, delayed, crashed or off: for lights and signs.'},
  timeline_updated:{icon:'list', desc:'Any integration adds a line to the stream timeline.'},
  destination_recovered:{icon:'check', desc:'A platform that dropped is live again.'},
  destination_still_down:{icon:'warn', desc:'Still down after a while: not a blip.'},
  destination_unstable:{icon:'pulse', desc:'It drops again and again.'},
  destination_steady:{icon:'check', desc:'After dropping, it has held for a while.'},
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
  d:null, h:0, sel:'trigger', target:[], after:null, run:null, dirty:false, lastField:null, back:null,
  testValues:{}, testDry:false, testOpen:false,
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
    this.after = null;
    this.lastField = null;
    this.resetTest();
    this.show();
  },
  resetTest(){
    this.run = null;
    this.testValues = {};
    this.testOpen = false;
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
    let count = 0;
    walk(h.steps, () => { count++; });
    const hint = `${d.preset ? 'From the catalog' : 'Your own'} · <span data-tick>${plural(count, 'step')}</span>`
      + (d.handlers.length > 1 ? ` in this trigger, ${d.handlers.length} triggers` : '');
    const selected = Array.isArray(this.sel) ? this.stepAt(this.sel) : null;
    Modal.body(`${modalHead('steps', 'var(--accent)', titleInput(d.name), hint,
      `<button class="ic-btn ig-head-btn" data-act="b-test">${svg('play', ' class="ig-btn-ic"')}Send a test</button>${enableSwitch('enabled', d.enabled)}`)}
      <div class="ig-bld${this.choosing ? ' picking' : ''}">
        <aside class="ig-palette" aria-label="Blocks">
          <div class="muted ig-palette-hint">Click a block to add it where the dashed box is lit.</div>
          ${PALETTE.map(([group, items]) => `<div class="ic-label">${group}</div>${items.map(([k, label]) =>
            `<button data-act="b-add" data-kind="${k}" style="--c:${KINDS[k].c}"${LOCAL_KINDS.includes(k) && localLocked()
              ? ` class="ig-pal-locked" aria-disabled="true" title="${esc(LOCAL_LOCK_TEXT)}"` : ` title="${esc(KINDS[k].hint)}"`}><i></i>${esc(label)}</button>`).join('')}`).join('')}
        </aside>
        <section class="ig-canvas" aria-label="Steps">
          <div class="ig-triggers" data-flip>${this.triggerTabs().map((label, i) => `<button class="ig-trig${i === this.h ? ' on' : ''}${d.handlers[i].enabled ? '' : ' ig-dim'}" data-act="b-handler" data-n="${i}" data-key="h-${uidOf(d.handlers[i])}"${d.handlers[i].enabled ? '' : ' title="Switched off"'}${i === this.h ? ' aria-current="true"' : ''}>
            ${esc(label)}</button>`).join('')}
            <button class="ig-trig add" data-act="b-add-handler" data-key="h-add">${svg('plus')}Another trigger</button></div>
          ${this.choosing ? this.chooserHtml(h) : `<div class="ig-steps" data-flip>
            <button class="ig-step ig-when${this.sel === 'trigger' ? ' on' : ''}" data-act="b-sel" data-path="trigger" data-key="when">
              <span class="ig-step-kind">WHEN</span><span class="ig-step-text">${esc(triggerSentence(h.trigger))}</span></button>
            ${this.stepsHtml(h.steps, [], 'root')}
          </div>
          ${this.selectedHttp() ? `<div class="ig-try-wrap" data-key="try-${uidOf(this.selectedHttp())}">${tryHtml(this.selectedHttp(), h, 'Reply in chat with it')}</div>` : ''}
          ${this.testOpen ? `<div class="sys-section ig-run-box" data-key="testbox">${testPanelHtml(this)}</div>` : ''}`}
        </section>
        <aside class="ig-inspector" aria-label="Settings" data-flip><div class="ig-insp" data-key="${this.inspectorKey()}">${this.inspectorHtml()}</div></aside>
      </div>
      ${footHtml(d, this.dirty)}`);
    if (this.sel === 'trigger' && !this.choosing){
      if (h.trigger.type === 'chat_activity') Meter.watch();
      if (h.trigger.type === 'scene') ObsLive.watch();
    }
    if (selected && selected.type === 'obs') ObsLive.watch();
  },
  // Each trigger's tab label. Two alike get a number, so no two tabs read
  // the same and the one being edited is never in doubt.
  triggerTabs(){
    const labels = this.d.handlers.map(x => this.unpicked.has(x) ? 'New trigger' : 'When ' + lowerFirst(triggerLabel(x.trigger)));
    return labels.map((l, i) => labels.filter(x => x === l).length > 1
      ? `${l} · ${labels.slice(0, i + 1).filter(x => x === l).length}` : l);
  },
  // "How does it start?" then, for events, "What happens?": big tiles in
  // the canvas, so the first decision is visible and can't be skipped.
  chooserHtml(h){
    const picked = !this.unpicked.has(h);
    const back = `<button type="button" class="ig-chooser-back" data-act="b-pick-back">${svg('back')}${picked ? 'Keep it as it was' : 'Other ways to start'}</button>`;
    if (this.choosing === 'event'){
      return `<div class="ig-chooser" data-key="chooser-event">
        <div class="ig-chooser-head">${back}<h3>What happens?</h3><p>Pick the moment it reacts to.</p></div>
        ${eventTilesHtml(picked && h.trigger.type === 'event' ? h.trigger.event : '', true)}</div>`;
    }
    return `<div class="ig-chooser" data-key="chooser-kind">
      <div class="ig-chooser-head">${picked ? back : ''}<h3>How does it start?</h3><p>Pick what sets it off. You can change it any time.</p></div>
      <div class="ig-kinds">${TRIGGERS.map((k, n) => {
        const blocked = triggerBlocked(k);
        return `<button type="button" class="ig-kind${k.id === h.trigger.type && picked ? ' on' : ''}" data-act="b-trigger-type" data-v="${k.id}" style="--i:${n}">
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
    const lit = !this.after && pathKey(this.target) === here;
    const selKey = Array.isArray(this.sel) ? pathKey(this.sel) : null;
    const afterKey = this.after ? pathKey(this.after) : null;
    const tool = (act, path, icon, label, extra) =>
      `<button class="ic-btn-tiny" data-act="${act}" data-path="${path}" aria-label="${label}" title="${label}"${extra || ''}>${svg(icon)}</button>`;
    const out = (steps || []).map((s, n) => {
      const path = base.concat(n), pk = pathKey(path), id = uidOf(s);
      const k = KINDS[s.type] || {label:s.type, tag:'?', c:'#888'};
      const off = s.enabled === false;
      // Inside a switched-off check, nothing runs: nothing is missing.
      const missing = off || this.insideOff(path) ? '' : stepMissing(s);
      let html = `<div class="ig-step${selKey === pk ? ' on' : ''}${off ? ' off' : ''}${missing ? ' warn' : ''}" data-path="${pk}" style="--c:${k.c}" data-key="s-${id}">
        <button type="button" class="ig-step-main" data-act="b-sel" data-path="${pk}"${selKey === pk ? ' aria-current="true"' : ''}
          title="Alt+Up or Alt+Down moves it, Ctrl+D duplicates it, Delete removes it">
          <span class="ig-step-kind">${k.tag}</span><span class="ig-step-text">${stepSentence(s)}${off ? '<span class="sr-only">, switched off</span>' : ''}</span></button>
        ${missing ? `<span class="ig-step-warn">${esc(missing)}</span>` : ''}
        <span class="ig-step-tools">
          ${tool('b-move', pk, 'up', 'Move up', ` data-dir="-1"${this.moveTarget(path, -1) ? '' : ' disabled'}`)}
          ${tool('b-move', pk, 'down', 'Move down', ` data-dir="1"${this.moveTarget(path, 1) ? '' : ' disabled'}`)}
          ${tool('b-toggle-step', pk, off ? 'eyeOff' : 'eye', off ? 'Switch this step on' : 'Switch this step off')}
          ${tool('b-dup', pk, 'copy', 'Duplicate')}
          ${tool('b-del', pk, 'x', 'Remove')}
        </span></div>`;
      if (s.type === 'if'){
        html += `<div class="ig-branch" data-key="br-${id}">
          <div class="ig-branch-label">Then</div><div class="ig-branch-list" data-flip>${this.stepsHtml(s.then, path.concat('then'), id + 't')}</div>
          <div class="ig-branch-label else">Otherwise</div><div class="ig-branch-list" data-flip>${this.stepsHtml(s.else, path.concat('else'), id + 'e')}</div>
        </div>`;
      }
      if (afterKey === pk){
        html += `<button class="ig-drop after on" data-act="b-target" data-path="${here}" data-key="after-${id}">New blocks land here · click to add at the end instead</button>`;
      }
      return html;
    }).join('');
    const empty = !base.length && !(steps || []).length
      ? `<div class="ig-empty-steps" data-key="empty-steps">${svg('steps')}<span><b>Nothing happens yet.</b> Pick what it does from the blocks on the left: post in Discord, answer in chat, show it on stream… Steps run from top to bottom.</span></div>` : '';
    return out + empty + `<button class="ig-drop${lit ? ' on' : ''}" data-act="b-target" data-path="${here}" data-key="drop-${owner}">${lit ? 'New blocks land here' : '+ Add a step here'}</button>`;
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
  // Whether `count` more steps fit (64 in all) at a list `base` (checks
  // nest 4 deep at most). Says why when they don't.
  roomFor(base, count){
    let total = 0;
    this.d.handlers.forEach(h => walk(h.steps, () => { total++; }));
    if (total + count > 64){ toast('That\'s the most steps one integration can have (64)', 'info'); return false; }
    if (base.filter(x => x === 'then' || x === 'else').length >= 4){ toast('Checks go 4 deep at most: add it a level up', 'info'); return false; }
    return true;
  },
  // Whether a step sits inside a switched-off check.
  insideOff(path){
    for (let n = 1; n < path.length; n += 2){
      if (this.stepAt(path.slice(0, n)).enabled === false) return true;
    }
    return false;
  },
  // Where one step up or down takes the step at `path`, in reading order:
  // past its neighbour, into a check next to it (its Then from above, its
  // Otherwise from below), between a check's Then and Otherwise, or out of
  // a check. {list, at, path} as it will be once the step is taken out;
  // null when it can't move that way (or it would nest too deep).
  moveTarget(path, dir){
    const {list, index} = this.locate(path);
    const base = path.slice(0, -1);
    const to = index + dir;
    let target = null;
    if (to >= 0 && to < list.length){
      const next = list[to];
      if (next.type === 'if' && next.enabled !== false){
        target = dir > 0 ? {list:next.then, at:0, path:base.concat(index, 'then', 0)}
          : {list:next.else, at:next.else.length, path:base.concat(to, 'else', next.else.length)};
      } else target = {list, at:to, path:base.concat(to)};
    } else if (path.length > 1){
      const branch = path[path.length - 2], parentPath = path.slice(0, -2);
      const parent = this.stepAt(parentPath), {list:outer, index:at} = this.locate(parentPath);
      if (branch === 'then' && dir > 0) target = {list:parent.else, at:0, path:parentPath.concat('else', 0)};
      else if (branch === 'else' && dir < 0) target = {list:parent.then, at:parent.then.length, path:parentPath.concat('then', parent.then.length)};
      else if (dir < 0) target = {list:outer, at, path:parentPath.slice(0, -1).concat(at)};
      else target = {list:outer, at:at + 1, path:parentPath.slice(0, -1).concat(at + 1)};
    }
    if (!target) return null;
    // Checks nest at most 4 deep, counting the step's own checks.
    const height = s => 1 + Math.max(0, ...(s.then || []).concat(s.else || []).map(height));
    const depth = target.path.filter(x => x === 'then' || x === 'else').length + height(list[index]);
    return depth <= 4 ? target : null;
  },
  inspectorHtml(){
    const h = this.handler();
    const selected = Array.isArray(this.sel) ? this.stepAt(this.sel) : null;
    // Only what this step can use: the trigger's values, what steps
    // before it made, and the live ones.
    const upto = selected || (h.steps || [])[0];
    const varsBox = `<div class="ig-insp-vars"><div class="ic-label">Values it can use</div>
      <div class="ig-tokens">${tokensHtml(varsFor(h, upto), 'b-token')}</div><div class="muted">Click a text field, then a value to put it there. <span class="mono">{name|text}</span> uses the text when the value is empty or 0.</div></div>`;
    if (this.choosing){
      return `<div class="ig-insp-title" style="--c:#5ac8fa"><small>WHEN</small><b>${this.choosing === 'event' ? 'What happens?' : 'How does it start?'}</b></div>
        <p class="ig-insp-p">Pick it in the middle. Then add steps from the list on the left: each one runs in order.</p>`;
    }
    if (this.sel === 'trigger') return this.triggerInspector(h) + varsBox;
    if (!selected) return varsBox;
    const k = KINDS[selected.type];
    const off = selected.enabled === false;
    return `<div class="ig-insp-title" style="--c:${k.c}"><small>${k.tag}</small><b>${esc(k.label)}</b></div>
      <div class="muted ig-insp-hint">${esc(k.hint)}</div>
      ${off ? warnHtml('This step is switched off: runs skip it. <a href="#" data-act="b-toggle-step" data-path="' + pathKey(this.sel) + '">Switch it on</a>') : ''}
      ${this.stepFields(selected)}${varsBox}`;
  },
  // A card's fields as rows: name, value, side by side or full width.
  fieldsHtml(s){
    const fields = parseFields(s.params.fields, true);
    return `<div class="ig-bfields">${fields.map((f, n) => `<div class="ig-bfield">
      <input class="ic-input" data-bind="bf-name" data-n="${n}" value="${esc(f.name)}" placeholder="Name" aria-label="Field name">
      <input class="ic-input" data-bind="bf-value" data-n="${n}" value="${esc(f.value)}" placeholder="Value, like {reason}" aria-label="Field value">
      <button class="ic-btn-tiny" data-act="bf-inline" data-n="${n}" aria-pressed="${f.inline}" aria-label="Side by side"
        title="${f.inline ? 'Side by side with the next field. Click for full width.' : 'Full width. Click to sit side by side.'}">${svg(f.inline ? 'columns' : 'rows')}</button>
      <button class="ic-btn-tiny" data-act="bf-del" data-n="${n}" aria-label="Remove this field">${svg('x')}</button></div>`).join('')}
      <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="bf-add">${svg('plus')}Add a field</button></div>`;
  },
  stepFields(s){
    const field = (name, label, opts) => {
      opts = opts || {};
      const v = s.params[name] || '';
      if (opts.select) return `<div class="dff"><label>${label}</label><select class="ic-input" data-bind="p" data-name="${name}">${opts.select.map(([id, l]) => `<option value="${esc(id)}" ${id === v ? 'selected' : ''}>${esc(l)}</option>`).join('')}</select></div>`;
      if (opts.area) return `<div class="dff"><label>${label}</label><textarea class="ic-input" data-bind="p" data-name="${name}" placeholder="${esc(opts.ph || '')}">${esc(v)}</textarea></div>`;
      const input = `<input class="ic-input${opts.mono ? ' mono' : ''}${opts.secret ? ' ig-secret' : ''}" data-bind="p" data-name="${name}" value="${esc(v)}" placeholder="${esc(opts.ph || '')}"${opts.mono ? ' spellcheck="false"' : ''}${opts.secret ? ' autocomplete="off"' : ''}${opts.fixed ? ' data-fixed' : ''}>`;
      // Addresses can carry keys (?key=, a Hue username): hidden until Show.
      if (opts.secret) return secretField(`p-${name}-${uidOf(s)}`, input, label);
      return `<div class="dff"><label>${label}</label>${input}</div>`;
    };
    // A length of time, or a value like {arg1} kept as written.
    const duration = (name, label, unit, fallback) => {
      const v = s.params[name] || '';
      if (/\{/.test(v)) return field(name, label + ' (ms)', {mono:true});
      return `<div class="dff"><label>${label}</label>${durInput('p-dur', v === '' ? fallback : +v, unit, ` data-name="${name}"`)}</div>`;
    };
    const note = text => `<div class="muted">${text}</div>`;
    const channels = S.data.connections.discord.map(c => [c.id, '#' + c.name]);
    switch (s.type){
      case 'discord': {
        const card = s.params.style === 'card';
        const edit = s.params.edit || '';
        const color = cardColor(s.params.color);
        const look = card ? field('title', 'Title', {ph:'🔴 {destination} is down'})
          + `<div class="dff"><label>Color</label><div class="ig-swatches" role="radiogroup" aria-label="Card color">${CARD_COLORS.map(([c, label]) =>
            `<button class="ig-swatch${c === color ? ' on' : ''}" style="--sw:${c}" data-act="b-color" data-v="${c}" role="radio" aria-checked="${c === color}"
              tabindex="${c === color || (!CARD_COLORS.some(([x]) => x === color) && c === CARD_COLORS[0][0]) ? 0 : -1}" aria-label="${label}" title="${label}"></button>`).join('')}
            <input class="ic-input mono ig-color-in" data-bind="p" data-name="color" value="${esc(s.params.color || '')}" placeholder="#5ac8fa or {variable}" spellcheck="false" aria-label="Any color"></div></div>`
          + field('text', 'Card text', {area:true, ph:'Since {down_for}. **Bold**, `code` and [links](https://…) work.'})
          + `<div class="dff"><label>Fields</label>${this.fieldsHtml(s)}</div>`
          + field('footer', 'Footer', {ph:'InstantClone'})
          + field('timestamp', 'Time in the footer', {select:[['yes', 'Yes'], ['', 'No']]})
          + field('above', 'Line above the card', {ph:'{clip.url} shows a video player'})
          + `<details class="ig-more-fields"><summary>Links and images</summary>${field('url', 'Title links to', {mono:true, ph:'https://twitch.tv/{channel}'})
            + field('image', 'Big image', {mono:true, ph:'https://…/image.png'}) + field('thumbnail', 'Small image (top right)', {mono:true, ph:'https://…'})}</details>`
          : field('text', 'Message', {area:true});
        return (channels.length ? field('connection', 'Channel', {select:[['', 'Pick a channel']].concat(channels)}) : note('No Discord channel yet. <a href="#" data-act="conn" data-tab="discord">Add one</a>.'))
          + `<div class="dcard-screen ig-insp-pv">${previewHtml(s, this.handler(), true)}</div>`
          + field('style', 'Look', {select:[['card', 'Card'], ['', 'Plain message']]})
          + look
          + field('edit', 'Each time it runs', {select:[['', 'Post a new message'], ['last', 'Update the same message'], ['close', 'Update it, then start fresh']]})
          + (edit ? note(edit === 'close' ? 'With nothing to finish (a short blip never got a message), it stays quiet.'
              : 'If someone deleted that message, a new one goes out.')
            + field('key', 'Message group (optional)', {mono:true, ph:'{destination}'})
            + note('Steps with the same group update the same message. <span class="mono">{destination}</span> gives each platform its own.') : '')
          + (edit !== 'close' ? field('ping', 'Ping', {select:[['', 'Nobody'], ['here', '@here'], ['everyone', '@everyone']]})
            + (edit === 'last' && s.params.ping ? note('Only when it posts a new message: an update never pings.') : '') : '')
          + note('<span class="mono">&lt;t:{hold_ends_at}:R&gt;</span> shows a live countdown in Discord.');
      }
      case 'timeline': return field('text', 'Line', {area:true, ph:'⭐ Highlight · [clip]({clip.url})'})
        + field('kind', 'It is', {select:Object.entries(TIMELINE_KINDS)})
        + field('at', 'It marks', {select:[['live', 'What you do now (lands after the delay)'], ['aired', 'What viewers see now']]})
        + note('The timeline is this stream\'s, shared by every integration: <span class="mono">{timeline}</span> shows it, <span class="mono">{chapters}</span> lists the chapters for YouTube. A new stream starts a new one.')
        + note('A button or a scene is pressed live: "what you do now". A mod\'s chat command is about what they saw: "what viewers see now".');
      case 'chat': return field('text', 'Message', {area:true}) + field('as', 'Send as', {select:[['', 'Your account'], ['bot', 'Bot account']]})
        + field('reply', 'Reply to the viewer', {select:[['', 'No'], ['yes', 'Yes, in their thread']]});
      case 'phone': return field('title', 'Title') + field('text', 'Message', {area:true}) + field('priority', 'Priority', {select:[['', 'Normal'], ['high', 'High'], ['urgent', 'Urgent']]});
      case 'http': return field('method', 'Method', {select:['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map(m => [m, m])})
        + field('url', 'Address', {mono:true, secret:true, ph:'https://api.example/rank/{arg1}'})
        + note('Values from chat are encoded for you, so a viewer can only fill in their part. Keep API keys in Headers: they stay hidden here and never go into recipes.')
        + secretField('headers-' + uidOf(s), `<textarea class="ic-input ig-secret" data-bind="p" data-name="headers" placeholder="Authorization: Bearer …">${esc(s.params.headers || '')}</textarea>`, 'Headers (one per line, Name: value)')
        + field('body', 'What it sends', {area:true, ph:'{"event":"{delay}"}'})
        + field('save_as', 'Save the answer as', {mono:true, ph:'response'})
        + note(`Later steps can use <span class="mono">{${esc(s.params.save_as || 'response')}.status}</span>, <span class="mono">.ok</span>, <span class="mono">.body</span> and <span class="mono">.json.field</span>.`)
        + note('Try it under the steps: send the request for real and pick what to use from the answer.');
      case 'wait': return duration('ms', 'Wait', 's', 10000) + note('Like 20s or 2m. Up to an hour.');
      case 'wait_delay': return '<p class="ig-insp-p">Waits until the moment this started has reached your viewers. Only InstantClone can do this: it knows your delay.</p>'
        + duration('extra_ms', 'Then wait a bit more', 's', 0) + field('follow', 'If the delay changes meanwhile', {select:[['', 'Follow it'], ['no', 'Keep the delay from the start']]});
      case 'if': return field('left', 'Check', {mono:true, ph:'{user_role}'}) + field('op', 'Is', {select:Object.entries(OPS)}) + field('right', 'Value', {mono:true, ph:'mod'})
        + note('Text compares ignoring case; "is more than" compares numbers.');
      case 'stop': return note('Ends this run here.');
      case 'delay_action': return field('action', 'Does', {select:Object.entries(DELAY_ACTIONS)})
        + (s.params.action === 'arm' ? field('seconds', 'Delay in whole seconds (0 turns it off), or a value like {arg1}', {mono:true, ph:'30'})
          + note('Starts this delay, or changes it while one is on air. Never cancels it.') : '')
        + warnHtml('This changes your stream. Tests never run it.');
      case 'marker': return field('description', 'Label', {ph:'Crash'}) + note('Only works while you are live on Twitch.');
      case 'clip': return note('Clips the last moments of your stream. Later steps can use <span class="mono">{clip.url}</span>, <span class="mono">{clip.ok}</span> and, when it failed, <span class="mono">{clip.error}</span>.');
      case 'program': return (localLocked() ? warnHtml(esc(LOCAL_LOCK_TEXT)) : '')
        + field('path', 'Program (a fixed path, no variables)', {mono:true, fixed:true, ph:'C:\\Tools\\thing.exe'}) + field('args', 'Arguments', {mono:true, ph:'--scene "Replay"'})
        + note('Values like <span class="mono">{arg1}</span> keep only letters, numbers, spaces and <span class="mono">_ . , : # + = / -</span>, so chat can\'t sneak in a command of its own.')
        + warnHtml('Runs on your PC. Recipes you import can never add this without asking you first.');
      case 'file': return (localLocked() ? warnHtml(esc(LOCAL_LOCK_TEXT)) : '') + field('path', 'File (a fixed path, no variables)', {mono:true, fixed:true, ph:'C:\\Stream\\status.txt'}) + field('text', 'Text', {area:true}) + field('mode', 'Each time', {select:[['', 'Replace the text'], ['append', 'Add a line']]});
      case 'set_var': return field('name', 'Name', {mono:true, fixed:true, ph:'winner'}) + field('value', 'Value', {ph:'{user}'}) + note('Later steps use it as <span class="mono">{name}</span>. Lasts for this run.');
      case 'overlay': return field('title', 'Small title', {ph:'Clip'}) + field('text', 'Text', {area:true, ph:'{user} just clipped that'})
        + field('seconds', 'On screen for', {select:[['3', '3 s'], ['6', '6 s'], ['10', '10 s'], ['15', '15 s'], ['30', '30 s']]
          .concat(['', '3', '6', '10', '15', '30'].includes(s.params.seconds || '') ? [] : [[s.params.seconds, s.params.seconds + ' s']])})
        + alertsSetupHtml() + note('Tests never put anything on stream: use "Show a test alert" above.');
      case 'edit_text': {
        const op = EDIT_OPS[s.params.op] || EDIT_OPS.first_line;
        const got = editTextSample(s, sampleMap(this.handler()));
        return field('input', 'Text', {mono:true, ph:'{api.body}'})
          + field('op', 'Do', {select:Object.entries(EDIT_OPS).map(([id, o]) => [id, o[0]])})
          + (op[1] ? field('a', op[1], {mono:true}) : '') + (op[2] ? field('b', op[2], {mono:true}) : '')
          + field('save_as', 'Save it as', {mono:true, fixed:true, ph:'text'})
          + note(`Gives <b>${esc(got) || '(nothing)'}</b> with the sample values.${s.params.op === 'random' ? ' Each run picks one of the lines, or one of the parts between |.' : ''}`)
          + note(`Later steps use it as <span class="mono">{${esc((s.params.save_as || '').trim() || 'text')}}</span>.`);
      }
      case 'counter': return field('name', 'Name', {mono:true, ph:'crashes'}) + field('op', 'Do', {select:[['', 'Add'], ['subtract', 'Subtract'], ['set', 'Set to'], ['reset', 'Reset to 0']]})
        + field('by', 'By', {mono:true, ph:'1'}) + note(`Kept between runs. Use it anywhere as <span class="mono">{counter.${esc(s.params.name || 'name')}}</span>.`);
      case 'obs': return this.obsFields(s, field, note);
    }
    return '';
  },
  // An OBS step: what it does, and the scene or source, picked from OBS's
  // own lists while it's connected.
  obsFields(s, field, note){
    const action = s.params.action || 'scene';
    const o = ObsLive.status;
    const pick = (items, name) => items.length ? `<div class="ig-obs-pick">${items.map(x =>
      `<button class="ig-start${x === s.params[name] ? ' on' : ''}" data-act="b-obs-pick" data-name="${name}" data-v="${esc(x)}" aria-pressed="${x === s.params[name]}">${esc(x)}</button>`).join('')}</div>` : '';
    const named = (name, label, ph, items) => `<div class="dff"><label>${label}</label>${pick(items, name)}
      <input class="ic-input" data-bind="p" data-name="${name}" data-fixed value="${esc(s.params[name] || '')}" placeholder="${esc(ph)}" spellcheck="false"></div>`;
    const scenes = (o && o.scenes) || [], sources = (o && o.sources) || [];
    let body = field('action', 'Does', {select:Object.entries(OBS_ACTIONS).map(([id, [label]]) => [id, label])});
    if (action === 'scene') body += named('scene', 'Scene', 'Scene name', scenes);
    else {
      body += named('source', action === 'text' ? 'Text source' : 'Source', 'Source name', sources);
      body += action === 'text' ? field('text', 'Text', {area:true, ph:'Crashes today: {counter.crashes}'})
        : named('scene', 'In the scene (blank: the one on air)', 'the scene on air', scenes);
    }
    return `<div data-obs-live>${obsStatusHtml()}</div>${body}`
      + note('Scene and source names are fixed here, so chat can never pick them. Tests never touch OBS.');
  },
  triggerInspector(h){
    const t = h.trigger;
    const kind = TRIGGERS.find(k => k.id === t.type) || TRIGGERS[0];
    const typeSel = `<div class="dff"><label>Starts when</label><button type="button" class="ig-evcurrent" data-act="b-change-kind">
      <span class="ig-ev-ic">${svg(kind.icon)}</span><span class="ig-ev-t"><b>${esc(kind.label)}</b><small>${esc(triggerBlocked(kind) || kind.hint)}</small></span>
      <span class="ig-evchange">Change</span></button></div>`;
    let body = '';
    if (t.type === 'event'){
      const current = eventOf(t.event), info = EVENT_INFO[t.event] || {icon:'bolt', desc:''};
      body = `<div class="dff"><label>What happens</label><button type="button" class="ig-evcurrent" data-act="b-change-event">
        <span class="ig-ev-ic">${svg(info.icon)}</span><span class="ig-ev-t"><b>${esc(current ? current.label : t.event)}</b><small>${esc(info.desc)}</small></span>
        <span class="ig-evchange">Change</span></button></div>`;
      body += paramsHtml(t, 't-');
      const filters = Editor.filterVars(t);
      if (filters.length){
        body += `<div class="ig-insp-group"><div class="ic-label">Only if</div>${filters.map(v => {
          const cur = (t.filters || {})[v.name] || '';
          const listed = filterChoices(t.event, v.name);
          const ch = listed && (!cur || listed.includes(cur)) ? listed : null;
          return `<div class="dff"><label>${esc(capFirst(filterName(v.name)))}</label>${ch
            ? `<select class="ic-input" data-bind="t-filter" data-name="${v.name}"><option value="">any</option>${ch.map(c => `<option ${c === cur ? 'selected' : ''}>${c}</option>`).join('')}</select>`
            : `<input class="ic-input" data-bind="t-filter" data-name="${v.name}" value="${esc(cur)}" placeholder="any, like ${esc(v.sample)}">`}</div>`;
        }).join('')}<div class="muted">Several at once: YouTube, Kick. A * matches anything: *timed out*.</div></div>`;
      }
    } else if (t.type === 'chat_activity'){
      body = rulesHtml(t, 't-');
    } else if (t.type === 'scene'){
      body = sceneHtml(t, 't-', this.d);
    } else if (t.type === 'chat_command'){
      body = `<div class="dff"><label>Command</label><input class="ic-input mono" data-bind="t-command" value="${esc(t.command || '')}" placeholder="!clip" spellcheck="false"></div>
        <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="t-aliases" value="${esc((t.aliases || []).join(' '))}" spellcheck="false"></div>
        <div class="dff"><label>Who can use it</label>${rolesHtml(t, 't-role')}</div>
        <div class="dff"><label>Each viewer, at most once every</label>${durInput('t-ucd-dur', t.user_cooldown_ms || 0, 's')}</div>`;
    } else if (t.type === 'chat_message'){
      body = `<div class="dff"><label>Matches</label><input class="ic-input" data-bind="t-pattern" value="${esc(t.pattern || '')}" placeholder="gg*"></div>
        <div class="dff"><label>How</label><select class="ic-input" data-bind="t-mode">${[['contains', 'Contains'], ['starts_with', 'Starts with'], ['exact', 'Is exactly'], ['wildcard', 'Wildcard (* and ?)']].map(([id, l]) => `<option value="${id}" ${id === (t.mode || 'contains') ? 'selected' : ''}>${l}</option>`).join('')}</select></div>
        <div class="dff"><label>Who</label>${rolesHtml(t, 't-role')}</div>`;
    } else if (t.type === 'timer'){
      body = `<div class="dff"><label>Every</label>${durInput('t-every-dur', t.every_ms || 600000, 'm')}</div>
        <div class="muted">Like 15m or 1h. Once a minute at most.</div>
        <label class="crash-check"><input type="checkbox" data-bind="t-live" ${t.only_live !== false ? 'checked' : ''}><span><span class="crash-check-title">Only while streaming</span></span></label>`;
    } else if (t.type === 'webhook'){
      const url = `${location.origin}/hooks/${t.token || ''}`;
      body = secretField('hook', `<div class="ig-copy-row"><input class="ic-input mono ig-secret" readonly value="${esc(url)}">
        <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(url)}">Copy</button></div>`, 'Call this address (keep it secret)') + `
        <div class="muted">GET or POST from a Stream Deck, a script or any app on this network. The body is <span class="mono">{body}</span>; JSON fields are <span class="mono">{body.field}</span>; <span class="mono">?team=red</span> is <span class="mono">{query.team}</span>.</div>
        <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="b-new-token">Make a new secret address</button>`;
    } else if (t.type === 'shortcut'){
      body = shortcutHtml(t);
    }
    // With more than one trigger, each can be switched off or removed,
    // right under its name rather than at the end of a long panel.
    const manage = this.d.handlers.length > 1
      ? `<div class="ig-trig-manage">${enableSwitch('h-enabled', h.enabled, h.enabled ? 'This trigger is on' : 'This trigger is off')}
         <button class="ic-btn ic-btn-ghost ig-small ig-trig-del" data-act="b-del-handler" aria-label="Remove this trigger">${svg('x')}Remove</button></div>` : '';
    return `<div class="ig-insp-title" style="--c:#5ac8fa"><small>WHEN</small><b>${esc(triggerLabel(t))}</b></div>
      ${manage}${typeSel}${body}
      <div class="ig-insp-group"><div class="ic-label">Every trigger here</div>
        <div class="dff"><label>Repeat at most once every</label>${durInput('b-cooldown-dur', this.d.cooldown_ms || 0, 's')}</div>
        <div class="muted">0s: no limit. Each trigger counts its own time. A button pressed too soon shows as skipped in Recent activity.</div>
        <div class="ig-insp-quiet">${quietHtml(this.d)}</div></div>`;
  },
  input(el){
    const b = el.dataset.bind, h = this.handler(), t = h.trigger;
    const durationOf = (min, max) => durValue(el, min, max);
    if (b === 'name') this.d.name = el.value;
    else if (b === 'enabled') this.d.enabled = el.checked;
    else if (b === 'b-cooldown-dur'){ const ms = durationOf(0, 86400000); if (ms == null) return; this.d.cooldown_ms = ms; }
    else if (b === 'h-enabled') h.enabled = el.checked;
    else if (b === 't-filter'){ t.filters = t.filters || {}; t.filters[el.dataset.name] = el.value.trim(); }
    else if (b === 't-command') t.command = el.value.trim().toLowerCase();
    else if (b === 't-aliases') t.aliases = el.value.split(/[\s,]+/).map(x => x.trim().toLowerCase()).filter(Boolean);
    else if (b === 't-role'){ t.roles = t.roles || {}; t.roles[el.dataset.name] = el.checked; }
    else if (b === 't-ucd-dur'){ const ms = durationOf(0, 86400000); if (ms == null) return; t.user_cooldown_ms = ms; }
    else if (b === 't-pattern') t.pattern = el.value;
    else if (b === 't-mode') t.mode = el.value;
    else if (b === 't-every-dur'){ const ms = durationOf(60000, 7 * 86400000); if (ms == null) return; t.every_ms = ms; }
    else if (b === 't-live') t.only_live = el.checked;
    else if (b === 't-param'){ if (!paramInput(t, el)) return; }
    else if (rulesInput(t, el, 't-') || sceneInput(t, el, 't-') || onlyLiveInput(t, el, 't-') || onlyLiveInput(t, el, '')){ /* changed below */ }
    else if (b === 'p') this.stepAt(this.sel).params[el.dataset.name] = el.value;
    else if (b === 'p-dur'){ const ms = durationOf(0, 3600000); if (ms == null) return; this.stepAt(this.sel).params[el.dataset.name] = String(ms); }
    else if (b === 'bf-name' || b === 'bf-value'){
      const s = this.stepAt(this.sel);
      Editor.fields(s, list => { const f = list[+el.dataset.n]; if (f) f[b === 'bf-name' ? 'name' : 'value'] = el.value; });
    }
    else if (b === 'test-value'){ this.testValues[el.dataset.name] = el.value; return; }
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
    this.edits = (this.edits || 0) + 1;
    syncSceneRules(this.d);
    this.render();
  },
  act(a, el){
    const d = this.d;
    if (rulesAct(this.handler().trigger, a, el) || sceneAct(this.handler().trigger, a, el)){ this.changed(); return true; }
    const selected = () => Array.isArray(this.sel) ? this.stepAt(this.sel) : null;
    switch (a){
      case 'b-color': { const s = selected(); if (s) s.params.color = el.dataset.v; this.changed(); return true; }
      case 'b-obs-pick': { const s = selected(); if (s) s.params[el.dataset.name] = el.dataset.v; this.changed(); return true; }
      case 'bf-add': { const s = selected(); if (s) Editor.fields(s, list => list.push({name:'', value:'', inline:true})); this.changed(); return true; }
      case 'bf-del': { const s = selected(); if (s) Editor.fields(s, list => list.splice(+el.dataset.n, 1)); this.changed(); return true; }
      case 'bf-inline': {
        const s = selected();
        if (s) Editor.fields(s, list => { const f = list[+el.dataset.n]; if (f) f.inline = !f.inline; });
        this.changed();
        return true;
      }
      case 'b-sel': {
        this.sel = el.dataset.path === 'trigger' ? 'trigger' : this.resolve(el.dataset.path);
        // New blocks land right after the step picked.
        this.after = Array.isArray(this.sel) ? this.sel : null;
        this.render();
        return true;
      }
      case 'b-target': this.target = this.resolve(el.dataset.path); this.after = null; this.render(); return true;
      case 'b-add': {
        if (LOCAL_KINDS.includes(el.dataset.kind) && localLocked()){ toast(LOCAL_LOCK_TEXT, 'info', 7000); return true; }
        const kind = el.dataset.kind, step = newStep(kind, this.handler().trigger);
        let list = null, at = 0, base = [];
        if (this.after){
          try { const found = this.locate(this.after); list = found.list; at = found.index + 1; base = this.after.slice(0, -1); } catch(_){ list = null; }
        }
        if (!list){
          try {
            const owner = this.target.length ? this.stepAt(this.target.slice(0, -1)) : null;
            if (!this.target.length || (owner && owner.type === 'if')) list = this.listAt(this.target);
          } catch(_){ list = null; }
          if (!list){ this.target = []; list = this.handler().steps; }
          at = list.length;
          base = this.target;
        }
        if (!this.roomFor(base, 1)) return true;
        list.splice(at, 0, step);
        this.sel = base.concat(at);
        // A check's blocks go inside it; anything else, right after it.
        if (kind === 'if'){ this.target = this.sel.concat('then'); this.after = null; }
        else this.after = this.sel;
        this.changed();
        const added = q('.ig-step.on');
        if (added) added.scrollIntoView({block:'nearest', behavior:reduced() ? 'auto' : 'smooth'});
        return true;
      }
      case 'b-move': {
        const path = this.resolve(el.dataset.path);
        const target = this.moveTarget(path, +el.dataset.dir);
        if (!target) return true;
        const {list, index} = this.locate(path);
        const [step] = list.splice(index, 1);
        target.list.splice(target.at, 0, step);
        this.sel = target.path;
        this.after = null;
        this.target = [];
        this.changed();
        // Keep the keyboard on the step that moved.
        const moved = q(`.ig-step[data-path="${pathKey(target.path)}"] [data-act="b-move"][data-dir="${el.dataset.dir}"]`);
        if (moved && !moved.disabled) moved.focus({preventScroll:true});
        return true;
      }
      case 'b-toggle-step': {
        const s = this.stepAt(this.resolve(el.dataset.path));
        if (s.enabled === false) delete s.enabled; else s.enabled = false;
        this.changed();
        return true;
      }
      case 'b-dup': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        let size = 0;
        walk([list[index]], () => { size++; });
        if (!this.roomFor([], size)) return true;
        list.splice(index + 1, 0, clone(list[index]));
        this.sel = path.slice(0, -1).concat(index + 1);
        this.after = this.sel;
        this.target = [];
        this.changed();
        return true;
      }
      case 'b-del': {
        const path = this.resolve(el.dataset.path), {list, index} = this.locate(path);
        const [gone] = list.splice(index, 1);
        const inside = gone.type === 'if' && (gone.then.length || gone.else.length);
        this.sel = 'trigger';
        this.after = null;
        this.target = [];
        this.changed();
        const handler = this.handler();
        undoToast(`Removed ${(KINDS[gone.type] || {label:'a step'}).label}${inside ? ' and the steps inside it' : ''}`, () => {
          if (Modal.kind !== 'builder' || this.d !== d) return;
          // Its list must still be part of the integration.
          let attached = d.handlers.includes(handler) && list === handler.steps;
          walk(handler.steps, x => { attached = attached || x.then === list || x.else === list; });
          if (!d.handlers.includes(handler) || !attached){ toast('Couldn\'t put it back: what held it was removed too', 'info'); return; }
          list.splice(Math.min(index, list.length), 0, gone);
          this.h = d.handlers.indexOf(handler);
          this.sel = 'trigger';
          this.after = null;
          this.changed();
        });
        return true;
      }
      case 'b-handler':
        this.leaveChooser();
        this.h = +el.dataset.n; this.sel = 'trigger'; this.target = []; this.after = null; this.resetTest();
        this.choosing = this.unpicked.has(this.handler()) ? 'kind' : null;
        this.render();
        return true;
      case 'b-add-handler': {
        this.leaveChooser();
        const added = {enabled:true, trigger:newTrigger('event'), steps:[]};
        d.handlers.push(added);
        this.unpicked.add(added);
        this.choosing = 'kind';
        this.h = d.handlers.length - 1; this.sel = 'trigger'; this.target = []; this.after = null; this.resetTest();
        this.changed();
        return true;
      }
      case 'b-del-handler':
        if (d.handlers.length < 2 || !armConfirm(el, 'Click again: removes it and its steps')) return true;
        d.handlers.splice(this.h, 1);
        this.h = 0; this.sel = 'trigger'; this.target = []; this.after = null; this.resetTest();
        this.changed();
        return true;
      case 'b-change-kind':
        this.cancelTrigger = clone(this.handler().trigger);
        this.choosing = 'kind';
        this.render();
        return true;
      case 'b-trigger-type': {
        const h = this.handler(), v = el.dataset.v;
        if (v === 'event'){
          // Which event is the next question, asked in the canvas. The
          // trigger only changes once one is picked.
          this.choosing = 'event';
          this.render();
          return true;
        }
        if (h.trigger.type !== v || this.unpicked.has(h)) h.trigger = newTrigger(v);
        this.unpicked.delete(h);
        this.choosing = null;
        this.cancelTrigger = null;
        this.sel = 'trigger';
        this.changed();
        if (v === 'shortcut') focusAct('key-record');
        const first = {chat_command:'t-command', chat_message:'t-pattern'}[v];
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
      case 'b-token':
        if (!this.lastField || !this.lastField.isConnected){ toast('Click a text field first, then the value to put in it', 'info'); return true; }
        insertAtCursor(this.lastField, '{' + el.dataset.token + '}');
        return true;
      case 'b-test': this.leaveChooser(); this.testOpen = true; this.render(); this.reveal('.ig-run-box'); focusAct('test-run'); return true;
      case 'test-run': if (this.askToPick()) busy(el, () => this.test()); return true;
      case 'test-mode': this.testDry = el.dataset.v === 'dry'; this.render(); return true;
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
      // The builder stays open after saving: building is trying things.
      case 'save': {
        this.leaveChooser();
        if (!this.askToPick()) return true;
        const at = this.edits;
        busy(el, () => saveIntegration(d, () => { if (this.edits === at) this.dirty = false; }, {stay:true}));
        return true;
      }
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
      const chat = newStep('chat', this.handler().trigger);
      const trig = this.handler().trigger;
      chat.params.text = trig.type.startsWith('chat') ? `{user}: ${token}` : token;
      list.splice(index + 1, 0, chat);
    }
    this.sel = this.sel.slice(0, -1).concat(index + 1);
    this.after = this.sel;
    this.target = [];
    this.changed();
  },
  // Leaving the chooser any other way than picking keeps the trigger as
  // it was.
  leaveChooser(){
    if (!this.choosing || this.unpicked.has(this.handler())) return;
    if (this.cancelTrigger) this.handler().trigger = this.cancelTrigger;
    this.cancelTrigger = null;
    this.choosing = null;
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
    this.testOpen = true;
    this.run = {running:true};
    this.render();
    this.reveal('.ig-run-box');
    const r = await api('/integrations/test', {integration:this.d, handler:this.h, values:testValues(this), dry:this.testDry});
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
    case 'shortcut': return {type, hotkey:'', midi:'', token:'', only_live:false};
    case 'chat_activity': return {type, window_ms:15000, match:'all', only_live:true,
      roles:{everyone:true, subs:true, vips:true, mods:true},
      rules:[{kind:'busier', value:3, words:''}, {kind:'chatters', value:5, words:''}]};
    case 'scene': return {type, scene:'', from:'', except:'', settle_ms:3000, only_live:false};
  }
  return {type:'event', event:'hold_opened', filters:{}};
}
// A new step of `kind`, set up for `trigger`: a timeline line from a
// button or a scene marks what you do now, one from chat what viewers saw.
function newStep(kind, trigger){
  const live = trigger && (trigger.type === 'shortcut' || trigger.type === 'scene'
    || (trigger.type === 'event' && ['hold_opened', 'obs_back'].includes(trigger.event)));
  const p = {
    discord:{connection:(S.data.connections.discord[0] || {}).id || '', style:'card', color:'#5ac8fa', title:'', text:'',
      footer:'InstantClone', timestamp:'yes'},
    timeline:{text:'', kind:'note', at:live ? 'live' : 'aired'},
    chat:{text:''}, phone:{title:'InstantClone', text:''}, http:{method:'POST', url:'', body:''},
    wait:{ms:'10000'}, wait_delay:{}, if:{left:'', op:'is', right:''}, stop:{},
    delay_action:{action:'cut'}, marker:{description:'Highlight'}, clip:{}, program:{path:'', args:''},
    file:{path:'', text:'', mode:''}, set_var:{name:'', value:''}, counter:{name:'count', op:'', by:'1'},
    overlay:{title:'', text:'', seconds:'6'}, edit_text:{input:'', op:'first_line', a:'', b:'', save_as:'text'},
    obs:{action:'scene', scene:'', source:'', text:''},
  }[kind] || {};
  return {type:kind, params:clone(p), then:[], else:[]};
}
function triggerSentence(t){
  switch (t.type){
    case 'event': {
      const f = Object.entries(t.filters || {}).filter(([k, v]) => v && !isParam(t, k)).map(([k, v]) => `${filterName(k)}: ${v}`);
      const tune = tuneSummary(t);
      return triggerLabel(t) + (tune ? ` · ${tune.k.toLowerCase()} ${tune.v}` : '') + (f.length ? ' · only if ' + f.join(', ') : '');
    }
    case 'chat_command': return `Someone types ${t.command || '!command'} · ${rolesLabel(t.roles)}`;
    case 'chat_message': return `A chat message ${t.mode === 'exact' ? 'is' : t.mode === 'starts_with' ? 'starts with' : 'matches'} "${t.pattern || ''}"`;
    case 'timer': return `Every ${fmtDur(t.every_ms || 0)}${t.only_live !== false ? ' while streaming' : ''}`;
    case 'webhook': return 'Something calls its secret address';
    case 'shortcut': return (shortcutLabel(t) ? `You press ${shortcutLabel(t)}` : 'You press a button (not set yet)') + (t.only_live ? ' while streaming' : '');
    case 'chat_activity': return `Chat gets busy: ${activitySummary(t)} in ${Math.round((t.window_ms || 10000) / 1000)} s`;
    case 'scene': return t.scene
      ? `OBS switches to ${sceneLabel(t)}${t.except && t.scene !== '*' ? ' (not ' + t.except + ')' : ''}${t.from ? ' from ' + t.from : ''}${t.settle_ms ? ' and stays ' + Math.round(t.settle_ms / 1000) + ' s' : ''}${t.only_live ? ' while streaming' : ''}`
      : 'OBS switches to a scene (not picked yet)';
  }
  return t.type;
}
function stepSentence(s){
  const p = s.params || {};
  const q = v => `“${highlightVars(v || '')}”`;
  switch (s.type){
    case 'discord': return `Discord ${p.style === 'card' ? 'card ' : ''}${esc(channelName(p.connection))} ${q(p.style === 'card' ? p.title || p.text : p.text)}${p.edit === 'last' ? ' · updates the same message' : p.edit === 'close' ? ' · updates it, then starts fresh' : p.ping ? ' · @' + esc(p.ping) : ''}`;
    case 'timeline': return `Timeline ${esc((TIMELINE_KINDS[p.kind] || TIMELINE_KINDS.note).toLowerCase())} ${q(p.text)}`;
    case 'chat': return `Chat ${q(p.text)}${p.as === 'bot' ? ' · as bot' : ''}${p.reply === 'yes' ? ' · as a reply' : ''}`;
    case 'phone': return `Phone ${q(p.text)}`;
    case 'http': return `${esc(p.method || 'POST')} ${esc(maskUrl(p.url) || '(address)')}${p.save_as ? ' → ' + esc(p.save_as) : ''}`;
    case 'wait': return `Wait ${/\{/.test(p.ms || '') ? highlightVars(p.ms) + ' ms' : esc(fmtDur(p.ms))}`;
    case 'wait_delay': return 'Wait until it has aired for viewers <span class="v">{delay}</span>' + (+p.extra_ms ? ' + ' + esc(fmtDur(p.extra_ms)) : '');
    case 'if': return `If ${highlightVars(p.left || '…')} ${esc(OPS[p.op] || p.op || '')} ${highlightVars(p.right || '')}`;
    case 'stop': return 'Stop here';
    case 'delay_action': return p.action === 'arm' && p.seconds && Number(p.seconds) <= 0 ? 'Turn the delay off'
      : p.action === 'arm' && p.seconds ? `Turn on a ${highlightVars(p.seconds)} s delay` : esc(DELAY_ACTIONS[p.action] || 'Delay action');
    case 'marker': return `VOD marker ${q(p.description)}`;
    case 'clip': return 'Create a clip';
    case 'program': return `Run ${p.path ? esc(p.path.split(/[\\/]/).pop()) : 'a program you pick'} ${highlightVars(p.args || '')}`;
    case 'file': return `${p.mode === 'append' ? 'Add to' : 'Write'} ${p.path ? esc(p.path.split(/[\\/]/).pop()) : 'a file you pick'} ${q(p.text)}`;
    case 'set_var': return `Remember ${esc(p.name || '…')} = ${highlightVars(p.value || '')}`;
    case 'overlay': return `On stream ${q(p.text)}`;
    case 'edit_text': return `${esc((EDIT_OPS[p.op] || EDIT_OPS.first_line)[0])}: ${highlightVars(p.input || '…')} → <span class="v">{${esc((p.save_as || '').trim() || 'text')}}</span>`;
    case 'counter': return `Counter ${esc(p.name || '…')} ${p.op === 'reset' ? 'reset' : p.op === 'set' ? '= ' + esc(p.by || '0') : (p.op === 'subtract' ? '-' : '+') + esc(p.by || '1')}`;
    case 'obs': {
      const what = (OBS_ACTIONS[p.action] || ['OBS'])[0];
      if (p.action === 'scene') return `OBS: switch to ${q(p.scene)}`;
      if (p.action === 'text') return `OBS: ${esc(p.source || '(source)')} says ${q(p.text)}`;
      return `OBS: ${esc(what.replace(/ a source.*$/, '').toLowerCase())} ${q(p.source)}${p.scene ? ' in ' + esc(p.scene) : ''}`;
    }
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
    const other = S.data.twitch_other_channel && t.main.login
      ? warnHtml(`You're logged in as <b>${esc(t.main.login)}</b>, but your Twitch destination streams to another channel. `
        + `VOD markers and clips go to ${esc(t.main.login)}'s channel, and fail while it's offline. `
        + 'To use them on the channel you stream to, disconnect and log in with that account.')
      : '';
    return (t.notice ? warnHtml(esc(t.notice)) : '') + other
      + this.accountHtml('main', t.main, 'Your Twitch account', 'Chat, VOD markers and clips. Log in once: InstantClone keeps it fresh.')
      + this.accountHtml('bot', t.bot, 'Bot account (optional)', 'A second account that posts in chat instead of you.')
      + `<label class="crash-check"><input type="checkbox" data-act="t-shared" ${S.data.twitch_shared_chat ? 'checked' : ''}>
        <span><span class="crash-check-title">Shared Chat: listen to partner channels too</span>
        <span class="muted">When you share chat with other streamers, their viewers can use your commands and count toward "chat gets busy". Off: only your own chat does.</span></span></label>`
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
          const r = await api('/connections/discord/delete', {id:el.dataset.id});
          if (!r.ok){ toast(r.error || 'Could not remove it', 'err', 6000); return; }
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
      case 't-shared': {
        const r = await api('/twitch/shared-chat', {on:el.checked});
        if (!r.ok){ el.checked = !el.checked; toast(r.error || 'Could not save it', 'err'); return true; }
        await refreshData();
        toast(el.checked ? 'Partner channels\' viewers can use your commands' : 'Only your own chat counts', 'ok');
        return true;
      }
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

// One integration of a recipe being imported: its triggers and who can set
// them off, then everything it reaches, the riskiest first.
function recipeItemHtml(it){
  // "OBS crashes, the hold runs out": names after the first read on.
  const who = (it.triggers || []).map((t, n) => esc(n && /^[A-Z][a-z]/.test(t.what) ? t.what[0].toLowerCase() + t.what.slice(1) : t.what)
    + (t.who ? ` <span class="ig-rwho">(${esc(t.who)})</span>` : '')).join(', ');
  const effects = (it.effects || []).map(e =>
    `<span class="ig-rflag ${esc(e.level)}">${e.level === 'info' ? '' : '⚠ '}${esc(e.text)}</span>`).join('');
  const handlers = (it.integration && it.integration.handlers) || [];
  return `<div class="ig-recipe-item"><b>${esc(it.name)}</b>
    <div class="ig-rwhen">When ${who || 'nothing'}</div>
    ${effects ? `<div class="ig-rflags">${effects}</div>` : ''}
    ${handlers.length ? `<details class="ig-rdetails"><summary>See every step</summary>${handlers.map(h =>
      `<div class="ig-rhandler"><div class="ig-rtrig">${esc(triggerSentence(h.trigger))}${h.enabled === false ? ' <span class="muted">(switched off)</span>' : ''}</div>
        <ol class="ig-rsteps">${recipeStepsHtml(h.steps)}</ol></div>`).join('')}</details>` : ''}
    ${it.integration ? `<details class="ig-rdetails"><summary>Show the raw settings</summary>
      <pre class="ig-rraw">${esc(JSON.stringify(it.integration, null, 2))}</pre></details>` : ''}</div>`;
}
// Recipes never carry where a step reaches: shared and imported, they lose
// the program and its arguments, the file, the web address and the OBS
// scene or source (recipe::strip_private). Say so where that step is, so
// nobody wonders which program it would run.
const RECIPE_FILLS = {program:[['path'], 'program and its arguments'], file:[['path'], 'file'],
  http:[['url'], 'address'], obs:[['scene', 'source'], 'scene or source']};
function recipeFillNote(s){
  const fill = RECIPE_FILLS[s.type], p = s.params || {};
  if (!fill || fill[0].some(name => p[name])) return '';
  return ` <span class="ig-rnote">· a recipe can't choose the ${fill[1]}: you pick it after adding</span>`;
}
// The steps of a recipe being imported, in the builder's words, checks
// with their branches nested. What runs on the PC is tinted red; what
// changes the delay, OBS or calls a web address, amber.
function recipeStepsHtml(steps){
  if (!steps || !steps.length) return '<li class="ig-rstep muted">Nothing</li>';
  return steps.map(s => {
    const k = KINDS[s.type] || {};
    const risk = LOCAL_KINDS.includes(s.type) ? ' danger' : ['delay_action', 'obs', 'http'].includes(s.type) ? ' caution' : '';
    const branches = s.type !== 'if' ? '' : `<ol class="ig-rsteps">${recipeStepsHtml(s.then)}</ol>`
      + ((s.else || []).length ? `<div class="ig-relse">Otherwise</div><ol class="ig-rsteps">${recipeStepsHtml(s.else)}</ol>` : '');
    return `<li class="ig-rstep${risk}"><div class="ig-rline"><span class="ig-rtag" style="--c:${k.c || 'var(--fg-3)'}">${esc(k.tag || s.type)}</span>
      <span class="ig-step-text">${stepSentence(s)}${recipeFillNote(s)}${s.enabled === false ? ' <span class="muted">(switched off)</span>' : ''}</span></div>${branches}</li>`;
  }).join('');
}
// Said once above the Add button when anything in the recipe is risky.
function recipeWarningHtml(p){
  const danger = p.items.some(it => (it.effects || []).some(e => e.level === 'danger'));
  if (p.local_effects && localLocked()) return warnHtml(esc(LOCAL_LOCK_TEXT));
  if (!danger) return '';
  return warnHtml('Read the red lines before adding it. Recipes arrive switched off: nothing runs until you open one and switch it on.');
}

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
            ${p.items.map(it => recipeItemHtml(it)).join('')}
          </div>
          ${recipeWarningHtml(p)}
          ${p.uses_discord ? (channels.length ? `<div class="dff"><label>Post its Discord messages in</label><select class="ic-input" data-bind="r-discord">${channels.map(c => `<option value="${esc(c.id)}">#${esc(c.name)}</option>`).join('')}</select></div>`
            : warnHtml('It posts to Discord: <a href="#" data-act="conn" data-tab="discord">add a Discord channel</a> first.')) : ''}
          ${p.local_effects && !localLocked() ? `<label class="crash-check lan-warn"><input type="checkbox" data-bind="r-local"><span><span class="crash-check-title">I trust this recipe to run programs or write files on this PC</span><span class="muted">Only tick this if you know who made it.</span></span></label>` : ''}
          <div class="muted ig-recipe-safe">${svg('shield')}<span>Recipes are settings, not code. They can't see your keys, tokens or webhook links, and they arrive switched off.</span></div>
        </div>` : ''}
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Cancel</button>
        ${p ? `<button class="ic-btn ic-btn-primary" data-act="r-apply" ${p.local_effects && localLocked() ? 'disabled' : ''}>Add ${p.items.length}</button>` : '<button class="ic-btn ic-btn-primary" data-act="r-preview">Preview</button>'}
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
    case 'tpl-edit': {
      // A field showing what it will say: back to what was written.
      const field = el.parentElement && el.parentElement.querySelector('.ig-tpl-in');
      if (field){ field.focus(); const end = field.value.length; field.setSelectionRange(end, end); }
      break;
    }
    case 'obs-retry': busy(el, () => ObsLive.tick()); break;
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
    case 'btn-token-new': Keys.trigger().token = randomToken(); OWNERS[Modal.kind].changed(); break;
    case 'btn-token-clear': Keys.trigger().token = ''; OWNERS[Modal.kind].changed(); break;
    case 'set-quiet': {
      const o = OWNERS[Modal.kind];
      o.d.quiet = el.dataset.v === 'on' ? (o.d.quiet || {from:'23:00', to:'08:00'}) : null;
      o.changed();
      break;
    }
    case 'pack':
      busy(el, async () => {
        if (Modal.kind === 'catalog') Modal.close(true);
        const added = await addFrom({pack:el.dataset.pack}, false);
        // Nothing to post to yet: connect Discord now, and the pack's
        // drafts get it as soon as it's saved.
        const waiting = added.filter(i => !i.enabled && allSteps(i).some(st => st.type === 'discord')).map(i => i.id);
        if (waiting.length && !S.data.connections.discord.length){
          Conn.open('discord');
          Conn.editing = {id:'', name:''};
          Conn.render();
          Modal.after = () => finishDrafts(waiting);
        }
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

// Enter or Space on a clickable card acts like a click; arrow keys move
// through a group of radio buttons (the card colors) and pick.
function onActivateKey(e){
  if ((e.key === 'Enter' || e.key === ' ') && e.target.matches('[role="button"][data-act]')){
    e.preventDefault();
    e.target.click();
    return;
  }
  if (Modal.kind === 'builder' && e.target.matches('.ig-step-main')){
    const row = e.target.closest('.ig-step');
    const tools = row.querySelector('.ig-step-tools');
    const pick = e.altKey && e.key === 'ArrowUp' ? tools.querySelector('[data-dir="-1"]')
      : e.altKey && e.key === 'ArrowDown' ? tools.querySelector('[data-dir="1"]')
      : (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'd' ? tools.querySelector('[data-act="b-dup"]')
      : e.key === 'Delete' ? tools.querySelector('[data-act="b-del"]') : null;
    if (!pick) return;
    e.preventDefault();
    if (pick.disabled) return;
    Builder.act(pick.dataset.act, pick);
    // The keyboard stays on the step it moved or copied.
    if (Array.isArray(Builder.sel)){
      const at = q(`.ig-step-main[data-path="${pathKey(Builder.sel)}"]`);
      if (at) at.focus({preventScroll:true});
    }
    return;
  }
  const dir = {ArrowRight:1, ArrowDown:1, ArrowLeft:-1, ArrowUp:-1}[e.key];
  if (dir && e.target.matches('[role="radio"][data-act]')){
    const group = [...e.target.closest('[role="radiogroup"]').querySelectorAll('[role="radio"][data-act]')];
    const next = group[(group.indexOf(e.target) + dir + group.length) % group.length];
    e.preventDefault();
    next.click();
    const again = q(`[role="radio"][data-act="${next.dataset.act}"][data-v="${CSS.escape(next.dataset.v)}"]`);
    if (again) again.focus();
  }
}

// Remember the last text field each editor used, for variable inserts.
document.addEventListener('focusin', e => {
  const text = ':not([readonly]):not([inputmode]):not([data-fixed]):not(.ig-dur):not([type="color"]):not([type="checkbox"])';
  // A message field measures itself again once it shows (see autosize).
  if (e.target.matches && e.target.matches('.ig-msg')) autosize(e.target);
  if (Modal.kind === 'builder' && e.target.matches(`.ig-inspector input${text}, .ig-inspector textarea`)) Builder.lastField = e.target;
  if (Modal.kind === 'editor' && e.target.matches(`.ig-live input${text}, .ig-live textarea`)) Editor.lastField = e.target;
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
