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
    return {ok:false, error:'The app did not answer. Is InstantClone still running?'};
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
  search:'M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3',
  rewind:'M3 12a9 9 0 1 0 3-6.7L3 8M3 3v5h5',
  chev:'M6 9l6 6 6-6',
  code:'M8 7l-5 5 5 5M16 7l5 5-5 5',
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
  mic:'M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3zM19 10v2a7 7 0 0 1-14 0v-2M12 19v3',
  rec:'M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 16a4 4 0 1 0 0-8 4 4 0 0 0 0 8z',
  heart:'M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21.2l8.8-8.8a5.5 5.5 0 0 0 0-7.8z',
  gift:'M20 12v10H4V12M2 7h20v5H2zM12 22V7M12 7H7.5a2.5 2.5 0 0 1 0-5C11 2 12 7 12 7zM12 7h4.5a2.5 2.5 0 0 0 0-5C13 2 12 7 12 7z',
  gem:'M6 3h12l4 6-10 12L2 9zM2 9h20M12 21L8 9l4-6 4 6z',
  users:'M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM23 21v-2a4 4 0 0 0-3-3.9M16 3.1a4 4 0 0 1 0 7.8',
  tv:'M2 7h20v13H2zM17 2l-5 5-5-5',
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
  light:        {label:'Smart light',   tag:'LIGHT',  c:'#fde68a', icon:'bulb', hint:'Turns a light or plug on your network on or off, or colors it. Found by a scan: no hub or account.'},
  viewer:       {label:'Viewer value',  tag:'VALUE',  c:'#fbbf24', icon:'gem', hint:'Gives, takes or spends what a viewer holds: coins, points. Kept between streams.'},
  math:         {label:'Maths',         tag:'VALUE',  c:'#93c5fd', icon:'hash', hint:'Works out a number: a sum, a share, a random one.'},
  wait_chat:    {label:'Wait for chat', tag:'WAIT',   c:'#c3cad1', icon:'bubble', hint:'Waits for someone to type something, up to a time limit.'},
  call:         {label:'Run a block',   tag:'FLOW',   c:'#f0a53a', icon:'layers', hint:'Runs a block of steps this integration keeps under a name.'},
  reward:       {label:'Channel points reward', tag:'TWITCH', c:'#a78bfa', icon:'gift', hint:'Switches a channel points reward on or off, so viewers can only redeem it when it makes sense.'},
  obs:          {label:'OBS',           tag:'OBS',    c:'#86efac', icon:'layers', hint:'Switches a scene, shows or hides a source, records, saves a replay, mutes, switches a filter, plays media or presses an OBS hotkey.'},
};
// The icon a catalog module wears: what it is about, not how it sends.
const PRESET_ICON = {
  crash_alert:'shield', destination_down:'signal', going_live:'megaphone', phone_crash:'phone',
  tell_chat:'bubble', delay_command:'hash', delay_notice:'clock', mod_controls:'cut', socials:'hash',
  vod_markers:'bookmark', webhook:'link', crash_counter:'file', stream_alerts:'bell', api_command:'globe',
  clip_button:'film', delay_status:'clock', smart_alerts:'pulse', stream_timeline:'list', chapters:'bookmark',
  highlight:'star', hype_clip:'flame', on_air_light:'bulb', scene_delay:'layers', twitch_alerts:'heart',
};
// Which step a card previews: the first one that says something.
const PREVIEW_ORDER = ['discord','chat','phone','overlay','marker','http','light','file','timeline','clip','delay_action','program'];
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
  arm:'Set the delay', cut_after:'Back to live, viewers miss nothing', cut:'Back to live now, viewers skip ahead',
  activate:'Turn on the delay that\'s ready', toggle:'Turn the delay on or off', disarm:'Turn the delay off',
  end_hold:'End the reconnect screen',
};
// What an OBS step can do: [label, what it needs picked, group]. Mirrors
// model::OBS_ACTIONS.
const OBS_ACTIONS = {
  scene:['Switch to a scene', 'scene', 'Scenes and sources'], show:['Show a source', 'source', 'Scenes and sources'],
  hide:['Hide a source', 'source', 'Scenes and sources'], toggle:['Show or hide a source (flip)', 'source', 'Scenes and sources'],
  text:['Change a text source', 'source', 'Scenes and sources'],
  mute:['Mute a source', 'source', 'Audio'], unmute:['Unmute a source', 'source', 'Audio'],
  mute_toggle:['Mute or unmute a source (flip)', 'source', 'Audio'],
  filter_on:['Turn a filter on', 'filter', 'Filters'], filter_off:['Turn a filter off', 'filter', 'Filters'],
  filter_toggle:['Turn a filter on or off (flip)', 'filter', 'Filters'],
  media_play:['Play a media source from the start', 'source', 'Media'], media_stop:['Stop a media source', 'source', 'Media'],
  record_start:['Start recording', '', 'Recording'], record_stop:['Stop recording', '', 'Recording'],
  replay_save:['Save the replay buffer', '', 'Recording'], screenshot:['Take a screenshot', '', 'Recording'],
  hotkey:['Press an OBS hotkey', 'hotkey', 'Hotkeys'],
  check:['Check OBS: keep what it\'s doing', '', 'Ask OBS'],
};
// What each OBS action does, said under the picker.
const OBS_NOTES = {
  replay_save:'The replay buffer must be running in OBS (Start Replay Buffer). The file lands in your recording folder.',
  screenshot:'OBS saves a picture of what\'s on air to your recording folder, like its own Screenshot hotkey.',
  record_start:'Saves to OBS\'s recording folder, with OBS\'s recording settings.',
  hotkey:'Any OBS hotkey, set to keys or not: start the virtual camera, a transition, a plugin\'s action.',
  media_play:'Restarts a video or sound you added as a Media Source.',
};
// An OBS hotkey's name, readable: OBSBasic.StartVirtualCam is "Start Virtual Cam".
const hotkeyLabel = name => name.replace(/^OBSBasic\./, '').replace(/[._]/g, ' ').replace(/([a-z])([A-Z])/g, '$1 $2');
const OPS = {
  is:'is', is_not:'is not', contains:'contains', not_contains:'does not contain', starts_with:'starts with',
  greater:'is more than', less:'is less than', empty:'is empty', not_empty:'is not empty',
  whole_number:'is a whole number', chance:'% chance (0.1 is one in a thousand)',
};
// Comparisons that need no value to compare with.
const OPS_ALONE = ['empty', 'not_empty', 'whole_number', 'chance'];
// An If holds up to this many checks: left, op, right, then left2…
// Mirrors model::MAX_CONDITIONS.
const MAX_CONDITIONS = 4;
const condSuffix = n => n ? String(n + 1) : '';
// An If's checks in order: the first always, the others once their op is set.
function conditionRows(s){
  const p = s.params || {}, rows = [];
  for (let n = 0; n < MAX_CONDITIONS; n++){
    const x = condSuffix(n);
    if (n && !p['op' + x]) continue;
    rows.push({sfx:x, left:p['left' + x] || '', op:p['op' + x] || '', right:p['right' + x] || ''});
  }
  return rows;
}
// Write `rows` back numbered 1, 2, 3 with no gaps.
function setConditions(s, rows){
  for (let n = 0; n < MAX_CONDITIONS; n++){
    const x = condSuffix(n), r = rows[n];
    ['left', 'op', 'right'].forEach(k => { if (r) s.params[k + x] = r[k]; else delete s.params[k + x]; });
  }
  if (rows.length < 2) delete s.params.match;
}
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
    kind:['note', 'chapter', 'highlight'], muted:['yes', 'no'], gift:['yes', 'no'], automatic:['yes', 'no'],
    tier:['1', '2', '3']}[name] || null;
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
  // A step that's switched on says more than one waiting to be (the raid
  // alert, not its optional welcome in chat).
  const on = found.filter(x => x.enabled !== false);
  for (const list of [on, found]){
    for (const kind of PREVIEW_ORDER){
      const s = list.find(x => x.type === kind);
      if (s) return s;
    }
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
    case 'block': return `Block: ${t.name || 'not named'}`;
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
  delay_ms:'delay (ms)', delay_max:'longest delay now', phase:'delay phase', hold_active:'reconnect screen on?', hold_left:'time left on hold',
  destinations_live:'platforms live', destinations_total:'platforms on', obs_live:'OBS streaming?',
  bitrate:'bitrate', channel:'your channel', time:'time now', date:'date', user:'viewer\'s name',
  user_login:'viewer\'s login', user_role:'viewer\'s role', message:'their message', args:'text after the command',
  target:'who it\'s about', arg1:'1st word', arg2:'2nd word', arg3:'3rd word', body:'what was sent',
  query:'address query', source:'pressed from', 'obs.scene':'OBS scene', 'obs.streaming':'streaming now?',
  'obs.recording':'recording now?', 'obs.muted':'muted?', 'obs.visible':'showing?', 'clip.url':'clip link', 'clip.ok':'clip made?', 'clip.error':'why no clip',
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
function myValues(){
  return (S.data.values || []).map(v => ({name:'my.' + v.name, sample:v.value}));
}
function sampleMap(handler){
  const m = {};
  varsFor(handler).forEach(v => { m[v.name] = v.sample; });
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
  const prefixes = ['counter.', 'viewer.', 'target.', 'top.', 'saved.'].concat(names.filter(n => n.endsWith('.status')).map(n => n.slice(0, -6)));
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
  const re = new RegExp(`(?<!\\{)\\{${from.replace(/\./g, '\\.')}([|#?][^}]*)?\\}`, 'g');
  walk(handler.steps, s => Object.keys(s.params || {}).forEach(k => {
    if (typeof s.params[k] === 'string') s.params[k] = s.params[k].replace(re, (all, fallback) => `{${to}${fallback || ''}}`);
  }));
}
// Mirrors src/integrations/template.rs, for previews.
// `escape`, when given, is applied to inserted values only, like the
// server does for web requests (see template::render_escaped).
function renderSample(text, vars, escape){
  return String(text || '').replace(/\{\{([A-Za-z0-9_.]+)\}\}|\{([A-Za-z0-9_.]{1,64})([|#?][^}]*)?\}/g,
    (all, escaped, name, tail) => {
      if (escaped) return '{' + escaped + '}';
      const v = vars[name] == null ? '' : String(vars[name]);
      // {name#one#many}: the count, then the word that fits it.
      if (tail && tail[0] === '#'){
        const forms = tail.slice(1).split('#');
        if (forms.length !== 2) return all;
        const count = v.trim() || '0';
        // A word written $name is that variable's value: {n#$my.coin#$my.coins}.
        const word = count === '1' ? forms[0] : forms[1];
        const filled = word[0] === '$' ? (vars[word.slice(1)] == null ? '' : String(vars[word.slice(1)])) : word;
        return (escape ? escape(count) : count) + ' ' + (word[0] === '$' && escape ? escape(filled) : filled);
      }
      // {name?yes:no}: one text or the other, by whether the value is yes.
      if (tail && tail[0] === '?'){
        const cut = tail.indexOf(':');
        if (cut < 0) return all;
        return /^(yes|true|on|1)$/i.test(v.trim()) ? tail.slice(1, cut) : tail.slice(cut + 1);
      }
      if (tail && (v === '' || v === '0')) return tail.slice(1);
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
// How a chat message trigger can match, each with what it means in a line.
const MATCH_MODES = {
  word:{label:'Has the word', means:'Fires when the message has it as its own word or phrase. Not when it is only part of a longer word.'},
  contains:{label:'Contains these letters', means:'Fires whenever these letters appear, in the middle of another word too.'},
  starts_with:{label:'Starts with', means:'Fires when the message begins with it.'},
  exact:{label:'Is exactly', means:'Fires when the whole message is just this.'},
  wildcard:{label:'Wildcard (* and ?)', means:'* stands for anything, ? for one character, and the whole message has to fit: gg* is any message starting with gg.'},
};
// Whether a chat message fires a trigger. Mirrors engine::text_matches.
function textMatches(text, pattern, mode){
  const said = String(text).toLowerCase(), want = String(pattern).trim().toLowerCase();
  if (!want) return false;
  const literal = want.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  switch (mode){
    // No letter or digit right before or after it: an apostrophe ends a word.
    case 'word': return new RegExp(`(?<![\\p{Alphabetic}\\p{N}])${literal}(?![\\p{Alphabetic}\\p{N}])`, 'u').test(said);
    case 'starts_with': return said.startsWith(want);
    case 'exact': return said.trim() === want;
    case 'wildcard': return new RegExp('^' + want.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.') + '$', 's').test(said.trim());
  }
  return said.includes(want);
}
// A few messages made from the pattern itself, each marked by whether it
// would fire: the difference between the ways to match, shown, not told.
function matchExamplesHtml(pattern, mode){
  const p = pattern.trim().replace(/[*?]/g, '');
  if (!p) return '';
  const tries = [p, p.toUpperCase() + '!', `oh ${p} yes`, `${p}ing`, `re${p}`, `${p} and more`];
  return `<div class="ig-fires-row">${tries.map(said => {
    const fires = textMatches(said, pattern, mode);
    return `<span class="ig-fires ${fires ? 'yes' : 'no'}" title="${fires ? 'Fires it' : 'Does not fire it'}">${fires ? '✓' : '✗'} ${esc(said)}</span>`;
  }).join('')}</div>`;
}
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
    // A preview can't roll: it shows the branch that runs most often.
    case 'chance': return parseFloat(l) >= 50;
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
  // A login in the address (user:pass@) never shows, like the rest.
  // The login runs to the last @ before the host (user:p@ss@host).
  const m = /^(https?:\/\/)(?:[^/?#\s]*@)?([^/?#\s@]+)(\S*)/i.exec(String(url || '').trim());
  if (!m) return String(url || '').trim() ? 'an address' : '';
  return m[1] + m[2] + (m[3] && m[3] !== '/' ? '/…' : '');
}
// Whether a field holds something that reads differently once sent: a
// variable, or (in Discord) Markdown, a link or a countdown.
function needsRender(raw, discord){
  const text = String(raw || '');
  return /\{[A-Za-z0-9_.]{1,64}(?:[|#?][^}]*)?\}/.test(text)
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
// A yes or no reads better as words in a message, so its chip writes
// both, ready to change. An If step compares the plain yes, so `plain`.
// By the value's own name, or by the field of a step's answer ({cam.muted}).
const YES_NO_WORDS = {
  muted:['muted', 'unmuted'], visible:['showing', 'hidden'], streaming:['live', 'offline'],
  recording:['recording', 'not recording'], ok:['worked', 'failed'], obs_live:['live', 'offline'],
};
function tokenText(name, plain){
  const parts = name.split('.');
  const words = YES_NO_WORDS[parts[parts.length - 1]];
  return plain || !words ? `{${name}}` : `{${name}?${words[0]}:${words[1]}}`;
}
// Every way to write a value, each with what it gives.
const FORMS_HELP = `<details class="ig-more-fields ig-forms"><summary>Ways to write a value</summary><dl>
  <dt>{viewers}</dt><dd>the value<b>42</b></dd>
  <dt>{game|nothing yet}</dt><dd>this text when it's empty or 0<b>nothing yet</b></dd>
  <dt>{viewers#viewer#viewers}</dt><dd>the number and the word that fits it<b>1 viewer, 42 viewers</b></dd>
  <dt>{obs.muted?muted:unmuted}</dt><dd>the first text for yes, the second for no<b>muted</b></dd>
  <dt>{{user}}</dt><dd>written as it is<b>{user}</b></dd></dl></details>`;
// The fields a step's answer has, said under that step's name.
const FIELD_LABEL = {
  scene:'scene on air', streaming:'streaming?', recording:'recording?', muted:'muted?', visible:'showing?',
  status:'status', ok:'worked?', body:'whole answer', output:'what it printed', code:'exit code',
};
// Insert chips for a field, grouped by where each value comes from: what
// started the run, each earlier step (under its answer's name), and the
// values that are always there, folded away when `foldLive`. `act` is the
// editor's insert action; `extra` goes after the last chip.
function tokensHtml(vars, act, extra, foldLive){
  const groups = [];
  vars.forEach(v => {
    const key = v.step || v.group;
    let g = groups.find(x => x.key === key);
    if (!g) groups.push(g = {key, v, list:[]});
    g.list.push(v);
  });
  const label = v => {
    const field = v.step && answerName(v.step) && v.name.split('.').length === 2 ? FIELD_LABEL[v.name.split('.')[1]] : '';
    return v.label || field || varLabel(v.name);
  };
  const chip = v => `<button class="ig-token" data-act="${act}" data-token="${esc(v.name)}" data-sample="${esc(v.sample)}" aria-label="Insert ${esc(label(v))}">+ ${esc(label(v))}</button>`;
  const head = v => {
    if (!v.step) return {trigger:'From what happened', mine:'Your values', live:'Always there'}[v.group] || '';
    const name = answerName(v.step);
    return `${esc(stepSource(v.step))}${name ? `<code>${esc(name)}</code>` : ''}`;
  };
  return groups.map((g, n) => {
    const chips = g.list.map(chip).join('') + (n === groups.length - 1 ? extra || '' : '');
    if (g.key === 'live' && foldLive){
      return `<details class="ig-vgroup ig-vfold"><summary>${svg('chev')}${head(g.v)}<span class="ig-vcount">${g.list.length}</span></summary><div class="ig-tokens">${chips}</div></details>`;
    }
    return `<div class="ig-vgroup"><div class="ig-vgroup-h">${head(g.v)}</div><div class="ig-tokens">${chips}</div></div>`;
  }).join('') || (extra || '');
}
// The name later steps reach a step's answer by, for the steps whose
// answer has several fields; '' for any other step.
function answerName(s){
  const base = s.type === 'http' ? 'response' : s.type === 'program' && s.params.wait === 'yes' ? 'program'
    : s.type === 'obs' && s.params.action === 'check' ? 'obs' : '';
  return base && ((s.params.save_as || '').trim() || base);
}
// Who made a group of values, in words.
function stepSource(s){
  if (s.type === 'obs' && s.params.action === 'check'){
    const names = listedNames(s.params.source);
    return 'Check OBS' + (!names.length ? '' : ' · ' + (names.length > 2 ? `${names[0]} +${names.length - 1}` : names.join(', ')));
  }
  return (KINDS[s.type] || {label:s.type}).label;
}
// The names an OBS step lists, one per line: one mute or show can take
// several sources. Mirrors obsws::listed_names.
function listedNames(text){
  const names = [];
  String(text || '').split('\n').map(x => x.trim()).forEach(x => { if (x && !names.includes(x)) names.push(x); });
  return names;
}
// What a check that reads several sources names each one's values by:
// {obs.mic_aux.muted}. Mirrors obsws::source_keys.
function sourceKeys(names){
  const keys = [];
  names.forEach(name => {
    const base = name.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+/, '').slice(0, 24).replace(/_+$/, '') || 'source';
    let key = base;
    for (let n = 2; keys.includes(key); n++) key = base + n;
    keys.push(key);
  });
  return keys;
}
// Whether another step in `handler` keeps its answer under `step`'s name.
function answerClash(handler, step){
  const name = answerName(step);
  let clash = false;
  if (name) walk(handler.steps, s => { clash = clash || (s !== step && answerName(s) === name); });
  return clash;
}
// Give `step` an answer name no other step uses: obs, obs2, obs3.
function nameAnswer(handler, step){
  if (!answerClash(handler, step)) return;
  const base = answerName(step).replace(/\d+$/, '');
  for (let n = 2; ; n++){
    step.params.save_as = base + n;
    if (!answerClash(handler, step)) return;
  }
}
function highlightVars(text){
  return esc(text).replace(/\{[A-Za-z0-9_.]{1,64}(?:[|#?][^}]*)?\}/g, m => `<span class="v">${m}</span>`);
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
  fillRanges(root);
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
  if (tag === 'BUTTON') a._wantDisabled = b.hasAttribute('disabled');
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
// Its row too: a narrow window wraps a control onto two lines.
const SEG_IND = '<span class="sub-tab-ind" data-keep-style aria-hidden="true"></span>';

// A slider's track fills up to its thumb (--p, read by index.html's CSS).
function fillRange(el){
  const min = +el.min || 0, max = +el.max || 100;
  el.style.setProperty('--p', ((+el.value - min) / (max - min || 1)) * 100 + '%');
}
function fillRanges(root){ root.querySelectorAll('input[type=range]').forEach(fillRange); }
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
function undoToast(text, undo){ actionToast(text, 'Undo', undo); }
// A toast with one button: Undo, or the obvious next step ("Edit copy").
function actionToast(text, label, act){
  const rail = $('toast-rail');
  if (!rail){ toast(text, 'ok'); return; }
  const el = document.createElement('div');
  el.className = 'ic-toast ok ig-undo';
  el.innerHTML = `<span class="ic-toast-icon">✓</span><span class="ig-undo-text"></span><button type="button" class="ig-undo-btn"></button>
    <i class="ig-undo-bar" style="animation-duration:${UNDO_MS}ms"></i>`;
  el.querySelector('.ig-undo-text').textContent = text;
  el.querySelector('.ig-undo-btn').textContent = label;
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
    await act();
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
  // The label names the field: clicking it focuses it. Without `for`, the
  // Show button inside it would be its control, and a click on the words
  // would show the secret, maybe on stream.
  const fid = 'sf-' + String(id).replace(/[^A-Za-z0-9_-]/g, '_');
  control = control.replace(/<(input|textarea)\b/, `<$1 id="${fid}"`);
  return `<div class="dff ig-secret-field${shown ? ' shown' : ''}" data-field="${esc(id)}"><label for="${fid}">${label}
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
    [...String(s.params[f] || '').matchAll(/\{([A-Za-z0-9_.]{1,64})(?:[|#?][^}]*)?\}/g)].map(m => m[1])))];
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
  // The issue form's fields for one integration: its name, and its last
  // run when that failed.
  reportFields(i){
    const r = lastRunOf(i.id);
    return {integration:i.name, run:r && r.status === 'failed' ? reportLog(r) : ''};
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
      <a role="menuitem" href="${esc(reportUrl(this.reportFields(i)))}" target="_blank" rel="noreferrer" style="--i:3">${svg('warn')}Report a problem</a>
      <div class="ig-menu-sep" role="separator"></div>
      <button role="menuitem" class="danger" data-act="menu-delete" data-id="${esc(id)}" style="--i:4">${svg('x')}Delete</button>`;
    document.body.appendChild(el);
    el.addEventListener('click', onClick);
    // The report opens in a new tab; the menu has done its job.
    el.addEventListener('click', e => { if (e.target.closest('a[role="menuitem"]')) this.close(); });
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

// The clipboard API only exists on https or this PC; a phone on the LAN
// reaches the dashboard over plain http, where the old copy command still
// works. True when it was copied.
async function writeClipboard(text){
  if (navigator.clipboard && window.isSecureContext){
    try { await navigator.clipboard.writeText(text); return true; } catch(_){}
  }
  const area = document.createElement('textarea');
  area.value = text;
  area.setAttribute('readonly', '');
  area.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0';
  // Inside the modal when one is open: its focus trap would pull focus
  // straight back otherwise.
  (Modal.form || document.body).appendChild(area);
  const had = document.activeElement;
  area.select();
  let done = false;
  try { done = document.execCommand('copy'); } catch(_){}
  area.remove();
  if (had && had.isConnected && typeof had.focus === 'function') had.focus({preventScroll:true});
  return done;
}
const COPY_FAILED = 'Could not copy: select it and copy it by hand';
async function copyText(btn){
  if (!await writeClipboard(btn.dataset.text || '')){ toast(COPY_FAILED, 'err'); return; }
  if (typeof flashCopied === 'function' && btn.classList.contains('ic-btn')) flashCopied(btn); else toast('Copied', 'ok');
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
    case 'light': {
      const rgb = lightRgb(step.params.color);
      const off = step.params.action === 'off';
      return `<div class="${cls} ig-pv-action"><span class="ig-bulb${off ? ' off' : ''}${step.params.action === 'flash' ? ' flash' : ''}" style="--bulb:${off ? 'transparent' : rgb}">${svg('bulb')}</span>${esc(lightSummary(step.params))}</div>`;
    }
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
  // The Recent activity row showing its steps (its data-key), and whether
  // an editor shows its last run's steps.
  actOpen:null, lastRunOpen:false,
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
  // The answer redraws the tab: the card around it eases to its new height.
  if (typeof easeCardHeight === 'function') easeCardHeight(render); else render();
  // Opening the dashboard connects OBS: show it soon, not at the next poll.
  if ((d.obs || {}).state === 'connecting') setTimeout(poll, 1200);
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
  S.data.obs = a.obs;
  const alerts = Object.assign({}, a.alerts, {look:lookOf()});
  const alertsChanged = JSON.stringify(alerts) !== JSON.stringify(S.data.alerts);
  S.data.alerts = alerts;
  if (fastBefore !== !!a.twitch.login) schedulePoll();
  if (S.visible) render();
  // Pages that show live state.
  if (Modal.kind === 'connections' && ((Conn.tab === 'twitch' && twitchChanged) || (Conn.tab === 'stream' && alertsChanged))) Conn.render();
  if (Modal.kind === 'editor' && alertsChanged && (primaryStep(Editor.handler()) || {}).type === 'overlay') Editor.render();
  if (Modal.kind === 'alerts' && alertsChanged){
    const status = q('.ig-look-body [data-key="alerts-status"]');
    if (status) status.outerHTML = alertsStatusHtml(false);
  }
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
const LOCAL_LOCK_TEXT = 'Running programs and writing files needs a dashboard password while the dashboard is open to your network (System › Security). Anyone on your network could use them otherwise.';
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
  else if (localLocked() && runsLocally(i)) flag = {cls:'warn', label:'Needs a password', tip:LOCAL_LOCK_TEXT};
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
          <div class="ig-exp-note">Still experimental: they work, but how they look and how you set them up may still change. Found a bug or have an idea? <a href="${esc(reportUrl())}" target="_blank" rel="noreferrer">Report it</a></div></div>
        <div class="ig-head-actions">
          ${empty ? '' : '<button class="ic-btn ic-btn-ghost" data-act="share">Share</button>'}
          <button class="ic-btn ic-btn-ghost" data-act="values">Your values</button>
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
  const consent = twitchEventsIn('needs_consent');
  if (consent.length && d.twitch.main.login){
    out.push(`<div class="lan-warn ig-notice" data-key="n-twitch-consent"><span class="lw-ic">⚠</span>
      <span class="ig-notice-text">Twitch needs your OK before InstantClone hears <b>${esc(twitchNouns(consent))}</b>: your login is from before these events.</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="conn" data-tab="twitch">Allow</button></div>`);
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
  const pills = CONNECTIONS.map(([id, , name]) => {
    const [cls, words] = connState(id);
    return `<button class="ic-pill ${cls}" data-act="conn" data-tab="${id}"><span class="dot"></span><span class="ic-pill-k">${name}</span><span class="ic-pill-v">${esc(words)}</span></button>`;
  }).join('');
  return `<div class="ig-conns" data-key="conns"><span class="ic-label">Connections</span>${pills}</div>`;
}
// Whether an integration talks to OBS: a scene trigger or an OBS step,
// switched on. Mirrors Engine::update_obs_want.
function usesObs(i){
  return (i.handlers || []).some(h => h.enabled && (h.trigger.type === 'scene' || isObsEvent(h.trigger) || hasStepOn(h, 'obs')));
}
// A trigger on something OBS reports itself (recording, a replay, mute).
function isObsEvent(t){
  return t.type === 'event' && (eventOf(t.event) || {}).group === 'OBS';
}
function hasStepOn(h, type){
  let found = false;
  walk(h.steps, s => { found = found || (s.type === type && s.enabled !== false); });
  return found;
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
    // A row opens onto its steps, to read, copy or report.
    const open = S.actOpen === key;
    return `<div class="ig-act-item${open ? ' open' : ''}" data-key="a-${esc(key)}" data-autoh>
      <div class="ig-act-row link" data-act="act-row" data-k="${esc(key)}" role="button" tabindex="0" aria-expanded="${open}"><span class="t">${esc(fmtClock(r.at_ms))}</span>
      <span class="n">${esc(r.name)}${r.test ? ' <span class="muted">(test)</span>' : ''}</span>
      <span class="d">${esc(r.trigger)} → ${esc(detail)}</span><span class="s ${esc(r.status)}">${esc(r.status === 'cooldown' ? 'skipped' : r.status)}</span>${svg('chev', ' class="c"')}</div>
      ${open ? `<div class="ig-act-more">${runPanelHtml(r, true)}</div>` : ''}</div>`;
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
  if (want && localLocked() && runsLocally(i)){ toast(LOCAL_LOCK_TEXT, 'info', 7000); return; }
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
    // Kept as a draft by the app (something is still missing) is not on.
    if (r.ok && !r.drafted) on++;
    else if (!r.ok) await api('/integrations/save', draft);
  }
  await load();
  if (on) toast(`${plural(on, 'integration')} from the pack switched on`, 'ok', 4500);
}

// The copy lands right after the original, switched off; it glows so the
// eye finds it, and the toast opens it.
async function duplicate(id){
  const r = await api('/integrations/duplicate', {id});
  if (!r.ok){ toast(r.error || 'Could not duplicate it', 'err'); return; }
  await load();
  const copy = find(r.id);
  if (!copy){ toast('Duplicated', 'ok'); return; }
  reveal(copy.id);
  actionToast(`Duplicated as ${copy.name}, switched off`, 'Edit copy', () => {
    const now = find(copy.id);
    if (now) openEditor(now);
  });
}
// Bring a card or row into view and glow it. A search or a category
// filter that hides it steps aside first.
function reveal(id){
  const i = find(id);
  if (!i) return;
  if (!visibleIntegrations().includes(i)){ S.search = ''; S.filter = 'all'; render(); }
  // After the morph has placed it.
  requestAnimationFrame(() => {
    const card = document.querySelector(`[data-key="c-${CSS.escape(id)}"], [data-key="r-${CSS.escape(id)}"]`);
    if (!card) return;
    card.scrollIntoView({block:'nearest', behavior:reduced() ? 'auto' : 'smooth'});
    glow(id);
  });
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
  // A failing card says "open it to see why": opened, it shows why.
  const st = stat(i);
  if (i.enabled && st && st.failing > 0 && lastRunOf(i.id)) setLastRun(true);
}
// The last run's steps over an editor's footer: shown or hidden.
function setLastRun(open){
  if (S.lastRunOpen === open) return;
  S.lastRunOpen = open;
  const o = OWNERS[Modal.kind];
  if (o && o.render && (Modal.kind === 'editor' || Modal.kind === 'builder')) o.render();
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
      // The last run's steps fold away at a click anywhere else.
      wrap.addEventListener('pointerdown', e => {
        if (S.lastRunOpen && !e.target.closest('.ig-lastrun, [data-act="last-run"]')) setLastRun(false);
      });
      wrap.addEventListener('toggle', e => {
        const key = e.target.dataset && e.target.dataset.fold;
        if (key) e.target.open ? Recipes.folds.add(key) : Recipes.folds.delete(key);
      }, true);
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
    S.lastRunOpen = false;
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
    fillRanges(this.form);
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
    const save = q('.dest-form-foot [data-act="save"]');
    bar.innerHTML = `<span class="ig-guard-text">${svg('warn')}You have unsaved changes</span>
      <button class="ic-btn ic-btn-ghost" data-act="guard-discard">Discard</button>
      <button class="ic-btn ic-btn-ghost" data-act="guard-keep">Keep editing</button>
      ${save && save.disabled ? '' : '<button class="ic-btn ic-btn-primary" data-act="guard-save">Save</button>'}`;
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
    else if (S.lastRunOpen) setLastRun(false);
    else if (Modal.kind === 'editor' && Editor.closeChip()) return;
    else Modal.close();
    return;
  }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's' && (Modal.kind === 'editor' || Modal.kind === 'builder')){
    e.preventDefault();
    const save = q('.dest-form-foot [data-act="save"]');
    if (save && !save.disabled) save.click();
    else if (save && localLocked()) toast(LOCAL_LOCK_TEXT, 'info', 7000);
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
// "Last ran 3 min ago · failed". With that run still in Recent activity,
// a button that opens its steps (see lastRunPanelHtml).
function lastRunHtml(d){
  const st = stat(d);
  if (!st || !st.last_ms) return '<span class="ig-last">Hasn\'t run yet</span>';
  const how = {ok:'worked', failed:'failed', stopped:'stopped early'}[st.last_status] || st.last_status;
  const cls = `ig-last ${st.last_status === 'failed' ? 'bad' : 'ok'}`;
  const text = `<i></i>Last ran ${esc(fmtAgo(st.last_ms))} · ${esc(how)}`;
  if (!lastRunOf(d.id)) return `<span class="${cls}">${text}</span>`;
  return `<button type="button" class="${cls} ig-last-btn" data-act="last-run" aria-expanded="${S.lastRunOpen}"
    title="${S.lastRunOpen ? 'Hide' : 'See'} what each step did">${text}${svg('chev')}</button>`;
}
// The newest real run of an integration in Recent activity: tests and
// skipped presses aren't runs, and the footer counts neither.
function lastRunOf(id){
  return (S.data.activity || []).find(r => r.integration_id === id && !r.test && r.status !== 'busy' && r.status !== 'cooldown') || null;
}
// The last run's steps, over the footer of the editor it belongs to.
function lastRunPanelHtml(d){
  const r = S.lastRunOpen && d.id ? lastRunOf(d.id) : null;
  if (!r) return '';
  return `<div class="ig-lastrun" role="region" aria-label="Last run">
    <div class="ig-lastrun-head"><b>Last run</b><span class="muted">${esc(fmtClock(r.at_ms))} · ${esc(r.trigger)}</span>
      <button type="button" class="ic-btn-tiny" data-act="last-run" aria-label="Close">${svg('x')}</button></div>
    ${runPanelHtml(r, false)}</div>`;
}
// A run as text, for "Copy run log" and the issue form's run log.
function runLogText(r){
  const lines = r.steps.map(s => `${((s.at_ms || 0) / 1000).toFixed(2).padStart(7)} s  ${s.status.padEnd(7)} ${s.label}${s.detail ? ': ' + s.detail : ''}`);
  return [`${r.name} · ${new Date(r.at_ms).toLocaleString()} · ${r.status}${r.test ? ' (test)' : ''}`, `When: ${r.trigger}`]
    .concat(lines.length ? lines : ['No steps ran.']).join('\n');
}
// A run log for the issue form, cut to what a link can carry.
function reportLog(r){
  const log = runLogText(r);
  return log.length > 1500 ? log.slice(0, 1500) + '\n…' : log;
}
// A run's steps with Copy, Report it and, from Recent activity, a way to
// the integration.
function runPanelHtml(r, withOpen){
  const log = runLogText(r);
  const report = reportUrl({integration:r.name, run:reportLog(r)});
  return `${runLogHtml(r.steps)}<div class="ig-run-actions">
    <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(log)}">${svg('copy', ' class="ig-btn-ic"')}Copy run log</button>
    ${withOpen && find(r.integration_id) ? `<button class="ic-btn ic-btn-ghost ig-small" data-act="edit" data-id="${esc(r.integration_id)}">${svg('pencil', ' class="ig-btn-ic"')}Open integration</button>` : ''}
    <a class="ig-run-report" href="${esc(report)}" target="_blank" rel="noreferrer">Report it</a></div>`;
}
// The integrations issue form with what is known filled in, by the form's
// own field ids (.github/ISSUE_TEMPLATE/integrations.yml).
function reportUrl(fields){
  const p = new URLSearchParams({template:'integrations.yml'});
  if (S.data && S.data.version) p.set('version', 'v' + S.data.version);
  Object.entries(fields || {}).forEach(([k, v]) => { if (v) p.set(k, v); });
  return 'https://github.com/Soulhackzlol/InstantClone/issues/new?' + p;
}
// The editors' footer: Delete away from Save, Save lit only with changes.
function footHtml(d, dirty, extra, canSave){
  const locked = localLocked() && runsLocally(d);
  return `<div class="dest-form-foot">${lastRunPanelHtml(d)}
    ${d.id ? '<button class="ic-btn ic-btn-ghost ig-del" data-act="delete">Delete</button>' : ''}
    <div class="dest-form-msg muted ig-foot-msg">${d.id ? lastRunHtml(d) : '<span class="ig-last">New integration</span>'}${locked ? `<button type="button" class="ig-foot-need ig-foot-why" data-act="lock-why">Needs a dashboard password: why?</button>`
      : dirty ? '<span class="ig-unsaved">Unsaved</span>'
      : d.id && issuesOf(d).length ? `<span class="ig-foot-need">${d.enabled ? 'Before it can run' : 'To switch it on'}: ${esc(issuesOf(d)[0])}</span>` : ''}</div>
    ${extra || ''}
    ${dirty ? `<button class="ic-btn ic-btn-ghost ig-discard" data-act="discard" title="${d.id ? 'Back to the last saved version' : 'Throw this new one away'}">Discard</button>` : ''}
    <button class="ic-btn ic-btn-primary" data-act="save" ${!locked && (dirty || canSave || !d.id) ? '' : 'disabled'} title="${locked ? esc(LOCAL_LOCK_TEXT) : 'Save (Ctrl+S)'}">Save</button>
  </div>`;
}
// Discard: back to the last saved version, staying in the editor (`ed`,
// the simple one or the builder), with Undo for the edits. A new one has
// no saved version, so it closes.
function discardEdits(ed){
  const saved = ed.d.id && find(ed.d.id);
  if (!saved){ Modal.close(true); return; }
  if (document.querySelector('.ig-rewind-badge')) return;
  const kind = Modal.kind, edits = ed.d;
  let fresh = null;
  rewind(true, () => {
    ed.d = fresh = withHandler(clone(saved));
    ed.dirty = false;
    ed.reset();
    ed.render();
  });
  undoToast('Changes discarded', () => {
    if (Modal.kind !== kind || !fresh || ed.d !== fresh) return;
    rewind(false, () => {
      ed.d = edits;
      ed.dirty = true;
      ed.reset();
      ed.render();
    });
  });
}
// Discard puts the blocks back one by one: the ones the edits added slide
// out, the ones that stay travel to where the saved version has them, and
// the ones the edits took away slide back in. Undo (`back` false) plays
// the same moves the other way. `swap` changes what is shown. A block is
// known in both versions by its kind and what it says.
function rewind(back, swap){
  const host = q('.ig-canvas');
  if (reduced() || !host){ swap(); return; }
  const sig = el => el.classList.contains('ig-when') ? 'when'
    : `${(el.querySelector('.ig-step-kind') || {}).textContent}|${(el.querySelector('.ig-step-text') || {}).textContent}`;
  const was = new Map();
  host.querySelectorAll('.ig-step').forEach(el => {
    const list = was.get(sig(el)) || [];
    list.push({rect:el.getBoundingClientRect(), copy:el.cloneNode(true)});
    was.set(sig(el), list);
  });
  const badge = document.createElement('div');
  badge.className = 'ig-rewind-badge' + (back ? '' : ' fwd');
  badge.setAttribute('aria-hidden', 'true');
  badge.innerHTML = `${svg('rewind')}<span>${back ? 'Back to saved' : 'Edits back'}</span>`;
  const box = host.getBoundingClientRect();
  Object.assign(badge.style, {left:(box.left + box.width / 2) + 'px', top:(box.top + 22) + 'px'});
  document.body.appendChild(badge);
  swap();
  host.querySelectorAll('[data-leaving]').forEach(n => n.remove());
  const plan = [...host.querySelectorAll('.ig-step')].map(el => {
    const list = was.get(sig(el));
    return {el, from:list && list.shift()};
  });
  let turn = 0;
  const next = () => Math.min(turn++, 16) * 75;
  // First out: what the other version doesn't have.
  was.forEach(list => list.forEach(({rect, copy}) => {
    copy.querySelectorAll('[data-act],[data-key],[id]').forEach(n => ['data-act', 'data-key', 'id'].forEach(a => n.removeAttribute(a)));
    ['data-act', 'data-key', 'data-path'].forEach(a => copy.removeAttribute(a));
    copy.setAttribute('aria-hidden', 'true');
    Object.assign(copy.style, {position:'fixed', left:rect.left + 'px', top:rect.top + 'px', width:rect.width + 'px', height:rect.height + 'px',
      margin:'0', zIndex:'1500', pointerEvents:'none'});
    document.body.appendChild(copy);
    copy.animate([{transform:'none', opacity:1},
      {transform:`translate(${back ? 70 : -70}px,-6px) rotate(${back ? 5 : -5}deg) scale(.9)`, opacity:0}],
      {duration:340, delay:next(), easing:'cubic-bezier(.4,0,1,1)', fill:'both'}).onfinish = () => copy.remove();
  }));
  // Then, top first, each block that moved is picked up and set down.
  plan.forEach(({el, from}) => {
    el.getAnimations().forEach(a => a.cancel());
    const to = el.getBoundingClientRect();
    if (!from){
      el.animate([{transform:`translateX(${back ? -50 : 50}px) scale(.92)`, opacity:0}, {transform:'none', opacity:1}],
        {duration:380, delay:next(), easing:'cubic-bezier(.2,.9,.3,1.2)', fill:'backwards'});
      return;
    }
    const dx = from.rect.left - to.left, dy = from.rect.top - to.top;
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
    el.animate([
      {transform:`translate(${dx}px,${dy}px)`, zIndex:5},
      {transform:`translate(${dx * 0.85}px,${dy * 0.85 - 8}px) scale(1.03)`, zIndex:5, offset:.18},
      {transform:'translateY(-4px) scale(1.02)', zIndex:5, offset:.8},
      {transform:'none', zIndex:5},
    ], {duration:520, delay:next(), easing:'cubic-bezier(.3,.7,.2,1)', fill:'backwards'});
  });
  // Lanes fade back in under their steps.
  host.querySelectorAll('.ig-branch').forEach(b => {
    b.getAnimations().forEach(a => a.cancel());
    b.animate([{opacity:.25}, {opacity:1}], {duration:420, delay:120, easing:'ease-out', fill:'backwards'});
  });
  setTimeout(() => {
    badge.animate([{opacity:1}, {opacity:0, transform:'translate(-50%,-6px)'}], {duration:220, fill:'forwards'}).onfinish = () => badge.remove();
  }, 650 + turn * 75);
}
function runLogHtml(steps){
  if (!steps || !steps.length) return '<div class="muted">No steps ran.</div>';
  return `<div class="ig-run">${steps.map((s, i) => `<div style="--i:${i}"><span class="t">${(s.at_ms / 1000).toFixed(2)} s</span>
    <span class="${esc(s.status)}">${s.status === 'ok' ? '✓' : s.status === 'failed' ? '✕' : '○'} ${esc(s.label)}</span>
    <span class="d">${esc(s.detail)}</span></div>`).join('')}</div>`;
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
  // Unfinished work is kept as a draft, switched off: the switch says so.
  if (r.drafted) d.enabled = false;
  if (onSaved) onSaved();
  const missing = r.missing || [], unknown = r.warnings || [];
  const names = unknown.length ? `Nothing fills ${unknown.map(w => '{' + w + '}').join(', ')}: check the spelling.` : '';
  if (missing.length){
    toast(`Saved as a draft${r.drafted ? ', switched off' : ''}. Still to do: ${missing.join('; ')}.${names ? ' ' + names : ''}`, 'info', 8000);
  } else if (names) toast(`Saved. ${names}`, 'info', 7000);
  else toast('Saved', 'ok');
  if (Modal.kind === kind){
    await savedBeat();
    // Something typed while it saved stays on screen, unsaved, instead of
    // closing with the editor.
    const owner = OWNERS[kind];
    if (Modal.kind === kind && !(opts && opts.stay)){
      if (owner && owner.dirty) owner.render();
      else Modal.close(true);
    }
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
    const counts = id => id === 'packs' ? cat.packs.length : this.presetsIn(id).length;
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
  // Without a Twitch app in this copy, what needs Twitch isn't a good
  // first pick: it stays in its own category, out of Recommended.
  usable(p){
    return S.data.twitch.available || !(p.needs || []).includes('twitch');
  },
  // The presets a category shows, so its count always matches its tiles.
  presetsIn(id){
    return S.data.catalog.presets.filter(p => id === 'rec' ? p.recommended && this.usable(p) : p.category === id);
  },
  presetsHtml(){
    const list = this.presetsIn(this.cat);
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
      const blocked = !this.usable(p);
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
      case 'light': return ['', 'A light on your network', 'You pick it after adding: WiZ, LIFX, Govee, Yeelight, Kasa, Shelly or WLED'];
      case 'file': return ['', 'A file', 'You pick the file it writes'];
      case 'obs': {
        const o = S.data.obs || {};
        return o.state === 'connected' ? ['ok', 'OBS WebSocket']
          : o.state === 'error' ? ['miss', 'OBS WebSocket', o.problem || 'Switch it on in OBS: Tools › WebSocket Server Settings']
          : ['', 'OBS WebSocket', 'Switch it on in OBS: Tools › WebSocket Server Settings'];
      }
    }
    return null;
  }).filter(Boolean);
  if (!pills.length) return '';
  return `<div class="ig-cat-needs"><span class="muted">Needs</span><span class="ig-cat-needs-list">${pills.map(([cls, label, tip]) =>
    `<span class="ig-need ${cls}"${tip ? ` title="${esc(tip)}"` : ''}>${cls === 'ok' ? svg('check', ' stroke-width="3"') : cls === 'miss' ? svg('warn') : ''}<span class="sr-only">${cls === 'ok' ? 'connected: ' : cls === 'miss' ? 'not connected yet: ' : ''}</span>${esc(label)}</span>`).join('')}</span></div>`;
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
    // Tests start as a dry run: sending for real is a choice, every time.
    this.testDry = true;
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
      : sel.type === 'scene' && !sel.scene ? 'scene' : commandClash(this.d) ? 'cmd' : null;
    this.lastField = null;
    this.run = null;
    this.moreVars = false;
    this.converted = null;
    TRIES.clear();
    this.show();
  },
  // After `d` is swapped for another version of it (Discard, Undo).
  reset(){
    this.d.handlers.forEach(h => sampleArgsFrom(h, presetOf(this.d)));
    this.sel = Math.min(this.sel, this.d.handlers.length - 1);
    this.chip = null;
    this.lastField = null;
    this.run = null;
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
        ${twitchEventStatusHtml(h.trigger)}
        <div class="ig-ed-preview" data-flip data-autoh><div class="ig-ed-pv" data-key="pv-${this.sel}">${this.previewBoxHtml(step, h)}</div></div>
        <div class="ig-ed-options">
          <div class="ig-chips" data-flip>${chips}</div>
          <div class="ig-pop${this.chip ? '' : ' closed'}" data-flip data-autoh>${this.chip
            ? `<div class="ig-pop-in" data-key="pop-${this.chip}-${this.sel}">${this.popHtml(step, h)}</div>` : ''}</div>
        </div>
      </div>
      ${footHtml(d, this.dirty, `<button class="ic-btn ic-btn-ghost" data-act="ed-builder">Open in builder</button>
        ${step && step.type === 'overlay'
          ? `<button class="ic-btn ic-btn-ghost" data-act="alert-show">${svg('play', ' class="ig-btn-ic"')}Show it on stream</button>`
          : `<button class="ic-btn ic-btn-ghost" data-act="ed-test">${svg('play', ' class="ig-btn-ic"')}Send a test</button>`}`, this.justAdded)}`);
    Modal.form.querySelectorAll('.ig-msg').forEach(autosize);
    if (this.chip === 'detect') Meter.watch();
    if (this.chip === 'scene') ObsLive.watch();
    if (step && step.type === 'overlay') this.syncStage(step, h, false);
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
    const alert = (primaryStep(hs[this.sel]) || {}).type === 'overlay';
    const hint = `<div class="ig-mhint">Each one has its own ${alert ? 'alert' : 'settings'}: click a name to edit it below. The tick turns it on or off.</div>`;
    // Too many for a timeline: chips that wrap, each keeping its switch.
    if (hs.length > 5){
      return `<div class="ig-mchips" role="group" aria-label="Moments">${hs.map((h, n) => `<div class="ig-mchip${n === this.sel ? ' on' : ''}">
        ${dot(h, n)}${name(n, 'ig-mchip-name')}</div>`).join('')}</div>${hint}`;
    }
    return `<div class="ig-moments" role="group" aria-label="Moments" style="--n:${hs.length}">${hs.map((h, n) => `<div class="ig-moment">
      ${dot(h, n)}${name(n, 'ig-moment-name')}</div>`).join('')}</div>${hint}`;
  },
  previewBoxHtml(step, h){
    if (step && step.type === 'light'){
      const state = this.d.preset === 'on_air_light' && (h.trigger.filters || {}).state;
      return `<div class="dcard-screen ig-live-plain">
        <div class="ic-label">${state ? 'When it turns ' + esc({live:'live', delay:'delayed', crash:'crashed', off:'off'}[state] || state) : 'The light'}</div>
        ${previewHtml(step, h, true)}${lightControlsHtml(step.params, name => `data-bind="l-${name}"`)}</div>`;
    }
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
    } else if (step.type === 'overlay'){
      const p = step.params;
      box = `<div class="ig-live ig-live-alert">
        <div class="ig-live-row"><span class="ig-live-tag">ON STREAM · LIVE PREVIEW</span>
          <span class="ig-live-theme"><button class="ig-link" data-act="alert-replay">${svg('play', ' class="ig-btn-ic"')}Play it</button>
          <span>${esc(THEMES[themeOf(lookOf())].name)} theme</span><button class="ig-link" data-act="alerts-open">Change the look</button></span></div>
        <div class="ig-ed-stage"><iframe class="ig-stage-frame" src="/alerts?demo" title="Alert preview" tabindex="-1" aria-hidden="true"></iframe></div>
        <div class="ig-alert-fields">
          <div class="dff"><label>Small title</label>${tplHtml(`<input class="ic-input ig-tpl-in" data-bind="title" value="${esc(p.title || '')}" placeholder="Optional, like New follower" aria-label="Small title" spellcheck="false">`, p.title, vars, 'line')}</div>
          <div class="dff"><label>Text</label>${editor}</div>
        </div></div>${alertsStatusHtml(true)}`;
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
    // Every step counts: the simple editor edits more than the message.
    const unknown = [];
    walk(h.steps, s => unknownVars(h, s).forEach(u => { if (!unknown.some(x => x.name === u.name)) unknown.push(u); }));
    return `${box}${unknownVarsHtml(unknown)}<div class="ig-vals"><div class="ig-vals-h">Insert a value</div>${tokens}</div>${FORMS_HELP}`;
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
    if (step && step.type === 'overlay'){
      const p = step.params;
      out.push({id:'acolor', k:'Accent', v:alertColorName(p.color)});
      out.push({id:'asound', k:'Sound', v:soundLabel(p.sound)});
      out.push({id:'apic', k:'Picture', v:p.image ? (MEDIA_REF.test(p.image) ? p.image.slice(6) : 'a link') : 'none',
        warn:!!p.image && !MEDIA_REF.test(p.image) && !WEB_LINK.test(p.image)});
      out.push({id:'atime', k:'On screen for', v:(p.seconds || '6') + ' s'});
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
      out.push({id:'cmd', k:'Command', v:[t.command].concat(t.aliases || []).join(' '), warn:!!commandClash(d)});
      out.push({id:'who', k:'Who can use it', v:rolesLabel(t.roles)});
    }
    if (step && step.type === 'chat') out.push({id:'reply', k:'Reply as', v:step.params.as === 'bot' ? 'bot account' : 'your account'});
    if (step && step.type === 'phone') out.push({id:'priority', k:'Priority', v:step.params.priority || 'normal'});
    const light = firstStep(h, 'light');
    if (light) out.push({id:'light', k:'Light', v:light.params.name || 'pick your light', warn:!light.params.device});
    const http = firstStep(h, 'http');
    if (http && d.preset === 'on_air_light') out.push({id:'url', k:'Light', v:lightOf(http, t) || maskUrl(http.params.url) || 'pick your light', warn:!http.params.url});
    else if (http) out.push({id:'url', k:t.type.startsWith('chat') ? 'Answer from' : 'Send to', v:maskUrl(http.params.url) || 'set the address', warn:!http.params.url});
    if (step && step.type === 'file') out.push({id:'file', k:'File', v:step.params.path || 'pick a file', warn:!step.params.path});
    out.push({id:'cooldown', k:'Repeat at most', v:d.cooldown_ms ? 'once every ' + fmtDur(d.cooldown_ms) : 'anytime'});
    out.push({id:'quiet', k:'Quiet hours', v:d.quiet ? `${d.quiet.from} to ${d.quiet.to}` : 'off'});
    return out;
  },
  // The alert on the real page, in the page's look but in the middle of
  // the box, so the short stage shows it whole. `replay` 'full' plays it
  // coming in and going out (alerts.html showDemo).
  syncStage(step, h, replay){
    const frame = q('.ig-ed-stage .ig-stage-frame');
    if (!frame) return;
    const look = Object.assign({}, lookOf(), {position:'middle', align:'center'});
    stageShow(frame, frame.parentElement.clientWidth / 0.72, {look, card:alertCardOf(step, h), replay, sound:!!replay});
  },
  // The other alerts in this integration: one click gives them all this
  // option, as each moment has its own.
  everyAlertHtml(names){
    const others = this.d.handlers.filter(h => h !== this.handler() && (primaryStep(h) || {}).type === 'overlay');
    if (!others.length) return '';
    return `<button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="alert-all" data-names="${names.join(' ')}">Use on every alert here (${others.length + 1})</button>`;
  },
  // Values worth narrowing on ("only YouTube", "only crashes"); counts,
  // durations and reports change every time.
  filterVars(t){
    const e = eventOf(t.event);
    const skip = new Set(['hold', 'hold_ends_at', 'down_for', 'down_s', 'down_total', 'previous', 'drops', 'within',
      'duration', 'crashes', 'highlights', 'report', 'chapters', 'line', 'file',
      // Twitch: amounts and free text are for If steps, not filters.
      'months', 'streak', 'message', 'count', 'total', 'bits', 'viewers', 'cost', 'input', 'seconds']);
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
      case 'acolor': {
        const mine = /^#[0-9a-f]{6}$/i.test(step.params.color || '') ? step.params.color.toLowerCase() : '';
        const shown = alertColor(mine, lookOf());
        const known = !mine || CARD_COLORS.some(([c]) => c === mine);
        const swatch = (c, label, style) => `<button class="ig-swatch${c === mine ? ' on' : ''}" style="--sw:${style}" data-act="set-color" data-v="${c}"
          role="radio" aria-checked="${c === mine}" aria-label="${label}" title="${label}"></button>`;
        const swatches = [swatch('', 'The theme\'s', alertColor('', lookOf()))].concat(CARD_COLORS.map(([c, label]) => swatch(c, label, c))).join('');
        return `<div class="ic-label">Accent</div>
          <div class="ig-swatches"><span class="ig-swatch-set" role="radiogroup" aria-label="Alert accent">${swatches}</span>
            <label class="ig-swatch custom${known ? '' : ' on'}" title="Any color"><input type="color" data-bind="color" value="${esc(shown)}" aria-label="Any color (now ${esc(shown)})"></label></div>
          <div class="muted">The first one is the theme's. Everything else about the look is the theme's: <button class="ig-link" data-act="alerts-open">change the look</button>.</div>${this.everyAlertHtml(['color'])}`;
      }
      case 'asound': {
        const sound = step.params.sound || '';
        const volume = step.params.volume === undefined || step.params.volume === '' ? 70 : +step.params.volume;
        return `<div class="ic-label">Sound</div>${soundPickerHtml(sound, 'pick-sound', true)}
          <div class="ig-volume"><label for="ig-vol">Volume</label><input id="ig-vol" type="range" min="0" max="100" step="5" data-bind="volume" value="${volume}">
            <span class="mono">${volume}%</span>
            <button class="ic-btn ic-btn-ghost ig-small" data-act="sound-play" ${sound ? '' : 'disabled'}>${svg('play', ' class="ig-btn-ic"')}Play</button></div>
          <div class="muted">Plays with the alert, through OBS, so viewers hear it too.</div>${this.everyAlertHtml(['sound', 'volume'])}`;
      }
      case 'apic':
        return `<div class="ic-label">Picture</div>${picturePickerHtml(step.params.image || '', 'pick-picture')}
          <div class="muted">A PNG, JPG or GIF (it plays). Where it goes is the theme's.</div>${this.everyAlertHtml(['image'])}`;
      case 'atime':
        return `<div class="ic-label">On screen for</div>${seg(ALERT_SECONDS.map(n => [n, n + ' s']), step.params.seconds || '6', 'set-seconds')}
          <div class="muted">Alerts that come in together wait their turn, so none cuts another short.</div>${this.everyAlertHtml(['seconds'])}`;
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
            arm:'Arms this delay, or changes it while one is armed or on air. It goes on air once the buffer is full if Auto-activate is on; otherwise press Activate.',
            cut_after:'Goes back to live once viewers have caught up with you, so they miss nothing. It takes as long as the delay. If a cut is already waiting, it keeps that one.',
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
        return `${commandClashHtml(d)}<div class="dfg"><div class="dff"><label>Command</label><input class="ic-input mono" data-bind="command" value="${esc(t.command)}" spellcheck="false"></div>
          <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="aliases" value="${esc((t.aliases || []).join(' '))}" placeholder="!retraso !d" spellcheck="false"></div></div>
          <div class="dff"><label>Each viewer, at most once every</label>${seg([['0', 'no limit'], ['10000', '10 s'], ['30000', '30 s'], ['60000', '1 min']], String(t.user_cooldown_ms || 0), 'set-ucd')}</div>`;
      case 'who':
        return `<div class="ic-label">Who can use it</div>${rolesHtml(t, 'role')}<div class="muted">You can always use your own commands.</div>`;
      case 'reply':
        return `<div class="ic-label">Reply as</div>${seg([['', 'Your account'], ['bot', 'Bot account']], step.params.as || '', 'set-as')}
          ${t.type.startsWith('chat') ? `<label class="crash-check"><input type="checkbox" data-bind="reply" ${step.params.reply === 'yes' ? 'checked' : ''}><span><span class="crash-check-title">Reply in the viewer's thread</span><span class="muted">Twitch shows it as an answer to their message</span></span></label>` : ''}
          ${S.data.twitch.bot.login ? '' : '<div class="muted">No bot account yet: <a href="#" data-act="conn" data-tab="twitch">connect one</a>, or messages go out as you.</div>'}`;
      case 'light': {
        const light = firstStep(h, 'light');
        const web = d.preset === 'on_air_light' ? `<details class="ig-more-fields"><summary>Home Assistant or Philips Hue instead</summary>
          <div class="muted">They take a web request rather than a scan.</div>
          <div class="ig-starts">${(S.data.catalog.light_starts || []).filter(x => x.id !== 'wled').map(x => `<button class="ig-start" data-act="light-to-web" data-v="${esc(x.id)}">${esc(x.label)}</button>`).join('')}</div></details>` : '';
        return lightPickerHtml(light ? light.params : {}) + web;
      }
      case 'priority':
        return `<div class="ic-label">Priority</div>${seg([['', 'Normal'], ['high', 'High'], ['urgent', 'Urgent']], step.params.priority || '', 'set-priority')}`;
      case 'url': {
        const http = firstStep(h, 'http'), chatty = t.type.startsWith('chat');
        if (d.preset === 'on_air_light') return `<button class="ic-btn ic-btn-ghost ig-small" data-act="light-to-net">${svg('bulb', ' class="ig-btn-ic"')}Use a light on my network instead (no hub)</button>` + this.lightHtml(http, t);
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
      <div class="muted">${device ? help[device.id] || '' : 'Pick your light: InstantClone fills in what each state sends it.'} Then <b>Send a test</b>, set to <b>Send it for real</b>, lights it up for the moment picked above.</div>
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
    else if (/^l-(action|color|brightness)$/.test(b) && step) step.params[b.slice(2)] = el.value.trim();
    else if ((b === 'above' || b === 'footer' || b === 'key' || b === 'color') && step) step.params[b] = el.value;
    else if (b === 'key-split' && step) splitKey(step, el.checked);
    else if (b === 'volume' && step) step.params.volume = el.value.trim();
    else if (b === 'image-link' && step) step.params.image = el.value.trim();
    else if (b === 'sound-link' && step) step.params.sound = el.value.trim();
    else if ((b === 'f-name' || b === 'f-value') && step){
      this.fields(step, list => {
        const f = list[+el.dataset.n];
        if (f) f[b === 'f-name' ? 'name' : 'value'] = el.value;
      });
    }
    else if (b === 'param'){ if (!paramInput(t, el)) return; }
    else if (b === 'seconds' && firstStep(h, 'delay_action')) firstStep(h, 'delay_action').params.seconds = el.value.trim();
    else if (b === 'seconds-dur' && firstStep(h, 'delay_action')){
      // Up to an hour, the backend's config::MAX_DELAY_MS.
      const ms = durValue(el, 1000, 3600000);
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
      case 'pick-sound':
        if (!step) return true;
        step.params.sound = v;
        this.changed();
        if (v){ playAlertSound(alertCardOf(step, h)); showPlaying('pick-sound', v); }
        return true;
      case 'pick-picture': if (step) step.params.image = v; this.changed(); return true;
      case 'media-upload':
        uploadMedia(el.dataset.kind).then(ref => {
          if (!ref || Modal.kind !== 'editor' || primaryStep(this.handler()) !== step) return;
          step.params[el.dataset.kind === 'sound' ? 'sound' : 'image'] = ref;
          this.changed();
          if (el.dataset.kind === 'sound') playAlertSound(alertCardOf(step, h));
        });
        return true;
      case 'alert-replay': if (step) this.syncStage(step, h, 'full'); return true;
      case 'set-seconds': if (step) step.params.seconds = v; this.changed(); return true;
      case 'alert-all': {
        if (!step) return true;
        const names = el.dataset.names.split(' ');
        let n = 0;
        this.d.handlers.forEach(x => {
          const s = primaryStep(x);
          if (!s || s === step || s.type !== 'overlay') return;
          names.forEach(name => { s.params[name] = step.params[name] || ''; });
          n++;
        });
        this.changed();
        toast(`Done for ${plural(n, 'other alert')}. Save to keep it.`, 'ok');
        return true;
      }
      case 'sound-play': if (step) playAlertSound(alertCardOf(step, h)); return true;
      case 'alert-show': if (step) showAlertNow(step, h, el); return true;
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
      // One light for every state: the on-air light is one device.
      case 'light-use': allSteps(this.d).filter(s => s.type === 'light').forEach(s => useDevice(s, el)); this.changed(); return true;
      case 'l-swatch': if (step) step.params.color = v; this.changed(); return true;
      case 'light-to-net': onAirToNetwork(this.d); this.chip = 'light'; this.changed(); return true;
      case 'light-to-web': onAirToWeb(this.d, v); this.chip = 'url'; this.changed(); return true;
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
      case 'chip':
        this.chip = this.chip === el.dataset.chip ? null : el.dataset.chip;
        this.render();
        if (this.chip) revealOpened(q('.ig-ed-options'));
        return true;
      case 'token': {
        // Into the card field last written in, or the message itself.
        const field = this.lastField && this.lastField.isConnected ? this.lastField : q('.ig-msg');
        insertAtCursor(field, tokenText(el.dataset.token));
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
      // Undo brings it back, so no "are you sure" first.
      case 'delete': busy(el, () => remove(this.d.id)); return true;
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
  const mode = `<div class="sub-tabs ig-seg">${[['dry', 'Dry run'], ['', 'Send it for real']].map(([id, label]) => {
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

// Another switched-on integration answering one of this one's chat
// commands, as {word, name}, or null: both would reply. Mirrors
// api::command_taken.
function commandClash(d){
  const words = i => (i.handlers || []).filter(h => h.enabled && h.trigger.type === 'chat_command')
    .flatMap(h => [h.trigger.command].concat(h.trigger.aliases || []));
  const mine = words(d);
  if (!mine.length) return null;
  for (const other of S.data.integrations){
    if (!other.enabled || other.id === d.id) continue;
    const word = mine.find(w => words(other).includes(w));
    if (word) return {word, name:other.name};
  }
  return null;
}
function commandClashHtml(d){
  const c = commandClash(d);
  return c ? warnHtml(`<b>${esc(c.name)}</b> already answers ${esc(c.word)}. This one stays off until you pick another command or switch that one off.`) : '';
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
  if (s.type === 'if' && conditionRows(s).some(r => !r.left.trim())) return 'say what it checks';
  if ((s.type === 'set_var' || s.type === 'counter') && !(p.name || '').trim()) return 'name the value';
  if (s.type === 'viewer' && !/^[A-Za-z0-9_]{1,32}$/.test((p.value || '').trim())) return 'name the value: letters, digits and _';
  if (s.type === 'call' && !(p.block || '').trim()) return 'pick the block';
  if (s.type === 'reward' && !(p.title || '').trim()) return 'pick the reward';
  if (s.type === 'wait' && !/\{|^\d+$/.test((p.ms || '').trim())) return 'set how long to wait';
  if ([p.scene, p.source, p.filter, p.hotkey, s.type === 'light' ? p.device : '', s.type === 'program' || s.type === 'file' ? p.path : ''].some(v => /\{/.test(v || ''))) return 'names here can\'t use values';
  if (s.type === 'light'){
    if (!(p.device || '').trim()) return 'pick the light or plug';
    if (['color', 'flash'].includes(p.action) && !(p.color || '').trim()) return 'pick a color';
  }
  if ((s.type === 'program' || s.type === 'file') && !(p.path || '').trim()) return s.type === 'file' ? 'pick the file' : 'pick the program';
  if (s.type === 'obs'){
    const action = OBS_ACTIONS[p.action];
    if (!action) return 'pick what to do in OBS';
    const need = action[1];
    if (need === 'filter' && !(p.source || '').trim()) return 'pick the source or scene';
    if (need && !(p[need] || '').trim()) return {scene:'pick the scene', source:'pick the source', filter:'pick the filter', hotkey:'pick the hotkey'}[need];
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
function lightSummary(p){
  const color = (p.color || '').trim(), bright = (p.brightness || '').trim();
  const at = bright ? ` at ${bright}${/\{/.test(bright) ? '' : '%'}` : '';
  if (p.action === 'off') return 'Turn off';
  if (p.action === 'on') return 'Turn on' + at;
  return (p.action === 'flash' ? 'Flash ' : 'Turn on, ') + (color || 'pick a color') + at;
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
        <span class="ig-light-id"><b>${esc(x.name)}</b><small>${esc(x.brand)}${x.plug ? ' plug' : ''} · ${esc(x.address)}</small></span>
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

// Opens a value's name in a note, as the notes write them.
const MONO = '<span class="mono">';
// What a Maths step can do with its two numbers. Mirrors runner::sum.
const MATH_OPS = {'':'Plus', subtract:'Minus', multiply:'Times', divide:'Divided by', percent:'Percent of',
  min:'Or, if smaller,', max:'Or, if bigger,', random:'To (a random number between them)'};
// The channel points rewards InstantClone made on the channel:
// [{id, title, cost, enabled, prompt, wants_input}], null until read.
// `error` is why Twitch said no; `editing` is the reward whose form is
// open (its id, or 'new').
const Rewards = {list:null, error:'', loading:false, editing:null};
// Redeem triggers set to "only the ones I pick" with nothing picked yet:
// an empty filter means any reward, so the choice is remembered here.
const REWARD_ONLY = new WeakSet();
// A reward's title as a filter pattern. A comma would split the list of
// patterns, so ? (any one character) stands in for it.
const rewardPattern = title => title.replace(/,/g, '?');
const samePattern = (a, b) => a.trim().toLowerCase() === b.trim().toLowerCase();
// A redeem trigger's reward filter: which of the rewards made here it
// names, and whatever else was typed (rewards made on Twitch itself).
function rewardChoice(t){
  const parts = ((t.filters || {}).reward || '').split(',').map(x => x.trim()).filter(Boolean);
  const made = Rewards.list || [];
  return {
    any:!parts.length && !REWARD_ONLY.has(t),
    picked:made.filter(r => parts.some(x => samePattern(x, rewardPattern(r.title)))).map(r => r.id),
    other:parts.filter(x => !made.some(r => samePattern(x, rewardPattern(r.title)))),
  };
}
function setRewardChoice(t, ids, other){
  const titles = (Rewards.list || []).filter(r => ids.includes(r.id)).map(r => rewardPattern(r.title));
  t.filters = t.filters || {};
  t.filters.reward = titles.concat(other).join(', ');
}
// Which rewards start a redeem trigger: any, or the ones ticked.
function rewardPickHtml(t){
  const c = rewardChoice(t), made = Rewards.list || [];
  const scope = `<div class="sub-tabs ig-seg">${[['any', 'Any reward'], ['only', 'Only the ones I pick']].map(([id, label]) =>
    `<button class="sub-tab${(id === 'any') === c.any ? ' on' : ''}" data-act="b-reward-scope" data-v="${id}" aria-pressed="${(id === 'any') === c.any}">${label}</button>`).join('')}${SEG_IND}</div>`;
  if (c.any) return `<div class="dff"><label>Which rewards start this</label>${scope}<div class="muted">Every redemption on your channel runs these steps.</div></div>`;
  const rows = made.map(r => {
    const on = c.picked.includes(r.id);
    return `<button type="button" class="ig-pick${on ? ' on' : ''}" role="checkbox" aria-checked="${on}" data-act="b-reward-pick" data-id="${esc(r.id)}">
      <span class="ig-pick-box">${on ? svg('check') : ''}</span><span class="ig-pick-t">${esc(r.title)}</span><span class="muted">${esc(String(r.cost))} pts${r.enabled ? '' : ' · off'}</span></button>`;
  }).join('');
  const nothing = !c.picked.length && !c.other.length;
  return `<div class="dff"><label>Which rewards start this</label>${scope}
    <div class="ig-picks">${rows || (Rewards.list ? '<div class="muted">You have made no rewards here yet. Make one below, or name one you made on Twitch.</div>' : '')}</div>
    <input class="ic-input" data-bind="t-reward-other" value="${esc(c.other.join(', '))}" placeholder="Rewards made on Twitch, by name: Hydrate, Stretch*" aria-label="Other rewards, by name">
    <div class="muted">${nothing ? '<b>Nothing picked yet:</b> until you pick one, every reward starts this.' : 'Names you type match ignoring case. A * stands for anything.'}</div></div>`;
}
// One reward's form, to make it or change it.
function rewardFormHtml(r){
  return `<div class="ig-reward-form" data-id="${esc(r.id || '')}" data-was="${esc(r.title || '')}">
    <div class="ig-reward-form-row"><input class="ic-input" data-r="title" value="${esc(r.title || '')}" placeholder="Name viewers see" maxlength="45" aria-label="Name">
      <span class="ig-reward-cost"><input class="ic-input mono" data-r="cost" value="${esc(r.cost ? String(r.cost) : '')}" placeholder="500" inputmode="numeric" aria-label="Cost in points"><span class="muted">pts</span></span></div>
    <input class="ic-input" data-r="prompt" value="${esc(r.prompt || '')}" placeholder="Description (optional)" maxlength="200" aria-label="Description">
    <label class="crash-check"><input type="checkbox" data-r="wants_input" ${r.wants_input ? 'checked' : ''}><span><span class="crash-check-title">The viewer types a message with it</span>
      <span class="muted">Steps read it as <span class="mono">{input}</span>.</span></span></label>
    <div class="ig-reward-form-foot"><button type="button" class="ic-btn ic-btn-ghost ig-small" data-act="rewards-cancel">Cancel</button>
      <button type="button" class="ic-btn ic-btn-primary ig-small" data-act="rewards-save">${r.id ? 'Save' : 'Make it'}</button></div></div>`;
}
// The rewards made here: a switch, an edit and a delete each, and a way to
// make one. Read from Twitch the first time it is shown.
function rewardsHtml(){
  if (!Rewards.list && !Rewards.error && !Rewards.loading){
    Rewards.loading = true;
    queueMicrotask(() => loadRewards());
  }
  const row = r => Rewards.editing === r.id ? rewardFormHtml(r) : `<div class="ig-reward${r.enabled ? '' : ' off'}" data-id="${esc(r.id)}">
      <button type="button" class="ig-switch${r.enabled ? ' on' : ''}" role="switch" aria-checked="${r.enabled}" data-act="rewards-switch" data-on="${r.enabled ? '' : '1'}"
        aria-label="${r.enabled ? 'On: viewers can redeem it' : 'Off: viewers do not see it'}" title="${r.enabled ? 'On: viewers can redeem it' : 'Off: viewers do not see it'}"><span></span></button>
      <span class="ig-reward-t"><b>${esc(r.title)}</b><span class="muted">${esc(String(r.cost))} pts${r.wants_input ? ' · asks for a message' : ''}${r.prompt ? ' · ' + esc(r.prompt) : ''}</span></span>
      <button type="button" class="ic-btn-tiny" data-act="rewards-edit" aria-label="Edit ${esc(r.title)}" title="Edit">${svg('pencil')}</button>
      <button type="button" class="ic-btn-tiny" data-act="rewards-delete" aria-label="Delete ${esc(r.title)}" title="Delete">${svg('x')}</button></div>`;
  const relogin = /log out of Twitch/.test(Rewards.error);
  const body = Rewards.error ? `<div class="ig-unknown" role="status"><div>${svg('warn')}<span>${esc(Rewards.error)}</span></div></div>
      <div class="ig-holders-row">${relogin ? '<button type="button" class="ic-btn ic-btn-ghost ig-small" data-act="conn" data-tab="twitch">Open Connections</button>' : ''}
        <button type="button" class="ic-btn ic-btn-ghost ig-small" data-act="rewards-load">Try again</button></div>`
    : !Rewards.list ? '<div class="ig-reward ig-reward-wait"><span class="muted">Asking Twitch for your rewards…</span></div>'
    : `${Rewards.list.map(row).join('') || '<div class="muted">None yet. Make one and it is on your channel right away.</div>'}
      ${Rewards.editing === 'new' ? rewardFormHtml({}) : `<button type="button" class="ig-cond-add" data-act="rewards-new">${svg('plus')}New reward</button>`}`;
  return `<div class="dff ig-rewards"><label>Your rewards</label>${body}
    <div class="muted">Made and changed here, live on your channel at once. Twitch only lets an app change rewards it made, so ones you made on Twitch itself are not listed.</div></div>`;
}
// Ask the app for the rewards (with `change`: after making that change),
// then redraw. True when Twitch took it.
async function loadRewards(change){
  const r = await api('/integrations/rewards', change);
  Rewards.loading = false;
  const refused = !r || r.ok === false;
  // A change Twitch refused leaves the list as it was, with a word why.
  if (refused && change && Rewards.list) toast((r && r.error) || 'Twitch did not answer', 'err', 7000);
  else Rewards.error = refused ? ((r && r.error) || 'Twitch did not answer') : '';
  if (!refused) Rewards.list = r.rewards || [];
  const o = OWNERS[Modal.kind];
  if (o && o.render) o.render();
  return !refused;
}
// A reward's new name, wherever the integration being built names the old
// one: its redeem triggers and its reward steps.
function renameReward(d, was, now){
  const follow = steps => (steps || []).forEach(s => {
    if (s.type === 'reward' && s.params.title === was) s.params.title = now;
    follow(s.then); follow(s.else);
  });
  d.handlers.forEach(h => {
    follow(h.steps);
    const f = h.trigger.filters;
    if (h.trigger.event === 'twitch_redeem' && f && f.reward){
      f.reward = f.reward.split(',').map(x => samePattern(x, rewardPattern(was)) ? rewardPattern(now) : x.trim()).join(', ');
    }
  });
}
// The names of an integration's blocks, for "Run a block" steps.
const blockNames = d => (d.handlers || []).filter(h => h.trigger.type === 'block' && h.trigger.name).map(h => h.trigger.name);
// Who holds each viewer value, once asked for: [{name, holders, top:[{login, amount}]}].
const Holders = {values:null};
// A viewer value's holders in its step's editor: the ones with the most,
// and a way to fix one viewer by hand or start everyone over.
function holdersHtml(value){
  if (!/^[A-Za-z0-9_]{1,32}$/.test(value)) return '';
  const mine = (Holders.values || []).find(v => v.name === value);
  const list = !Holders.values ? '' : !mine ? '<div class="muted">Nobody holds any yet.</div>'
    : `<div class="muted">${plural(mine.holders, 'viewer')} hold some. The most:</div>
      <ol class="ig-holders-list">${mine.top.slice(0, 10).map(h => `<li><span class="mono">${esc(h.login)}</span><b>${esc(String(h.amount))}</b></li>`).join('')}</ol>`;
  return `<div class="dff ig-holders" data-value="${esc(value)}"><label>Who holds ${esc(value)}</label>${list}
    <div class="ig-holders-row"><input class="ic-input mono" data-h="login" placeholder="viewer" spellcheck="false" aria-label="Viewer">
      <input class="ic-input mono" data-h="amount" placeholder="0" inputmode="numeric" aria-label="Amount">
      <button type="button" class="ic-btn ic-btn-ghost" data-act="holders-set">Set</button></div>
    <div class="ig-holders-row"><button type="button" class="ic-btn ic-btn-ghost" data-act="holders-load">${Holders.values ? 'Refresh' : 'Show who holds it'}</button>
      ${mine ? '<button type="button" class="ic-btn ic-btn-ghost" data-act="holders-clear">Reset everyone to 0</button>' : ''}</div></div>`;
}
// Ask the app for the holders (with `change`: after making that change), then redraw.
async function loadHolders(change){
  const r = await api('/integrations/viewers', change);
  if (!r || r.ok === false){ toast((r && r.error) || 'That didn\'t work', 'err'); return; }
  Holders.values = r.values || [];
  const o = OWNERS[Modal.kind];
  if (o && o.render) o.render();
}
// The step editor: any trigger, any steps, checks with then/otherwise.
// Selection and insertion points are paths like [2,'then',0].
const PALETTE = [
  ['Send', [['discord', 'Discord'], ['chat', 'Twitch chat'], ['phone', 'Phone'], ['http', 'Web request']]],
  ['Flow', [['if', 'If / otherwise'], ['wait', 'Wait'], ['wait_delay', 'Wait for the delay'], ['wait_chat', 'Wait for chat'], ['call', 'Run a block'], ['stop', 'Stop']]],
  ['Viewers', [['viewer', 'Viewer value'], ['math', 'Maths']]],
  ['Stream', [['overlay', 'Show on stream'], ['reward', 'Channel points reward'], ['delay_action', 'Delay action'], ['obs', 'OBS'], ['marker', 'VOD marker'], ['clip', 'Clip'], ['timeline', 'Timeline']]],
  ['Home', [['light', 'Smart light']]],
  ['Anything', [['edit_text', 'Edit text'], ['set_var', 'Remember a value'], ['counter', 'Counter'], ['program', 'Run a program'], ['file', 'Write a file']]],
];
// The kind of start a trigger is, for its icon.
function triggerKind(t){
  return TRIGGERS.find(k => k.id === (t && t.type)) || TRIGGERS[0];
}
// What can start an integration, shown as tiles so every kind is in sight.
const TRIGGERS = [
  {id:'event', icon:'bolt', label:'Something happens', hint:'OBS crashes, a platform drops, a raid comes in, recording stops…',
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
  {id:'block', icon:'layers', label:'A block of steps', hint:'Steps your other triggers run by name',
    eg:'Check the bet once, use it in every game'},
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
  obs_recording_started:{icon:'rec', desc:'Recording starts in OBS, by you or a step.'},
  obs_recording_stopped:{icon:'rec', desc:'Recording stops, with where OBS saved the file.'},
  obs_replay_saved:{icon:'film', desc:'The replay buffer saves a clip, with where the file is.'},
  obs_mute_changed:{icon:'mic', desc:'A mic or any audio source is muted or unmuted.'},
  twitch_follow:{icon:'heart', desc:'Someone new follows your channel.'},
  twitch_sub:{icon:'star', desc:'A new sub, bought or gifted (gift: yes or no).'},
  twitch_resub:{icon:'star', desc:'A sub shares their resub, with their months and message.'},
  twitch_gift:{icon:'gift', desc:'Someone gifts subs to your community.'},
  twitch_cheer:{icon:'gem', desc:'Bits in chat, with the message.'},
  twitch_raid:{icon:'users', desc:'Another channel raids you, with its viewers.'},
  twitch_redeem:{icon:'bolt', desc:'A viewer redeems a channel points reward: any, or the ones you pick.'},
  twitch_ad:{icon:'tv', desc:'An ad break starts, run by you or by Twitch.'},
};
// Whether an event comes from Twitch's event feed (follows, subs, raids...).
const isTwitchEvent = id => (eventOf(id) || {}).group === 'Twitch';
// The state of the trigger's Twitch event: what to show and do about it.
function twitchEventStatusHtml(t){
  if (t.type !== 'event' || !isTwitchEvent(t.event)) return '';
  const tw = S.data.twitch, ev = tw.events || {}, sub = (ev.subs || {})[t.event] || '';
  const line = (dot, text, button) => `<div class="ig-meter"><span class="ig-meter-dot ${dot}"></span><span>${text}</span>${button || ''}</div>`;
  const toConn = label => `<button class="ic-btn ic-btn-ghost ig-small" data-act="conn" data-tab="twitch">${label}</button>`;
  if (!tw.main.login) return line('off', 'Connect your Twitch account first.', toConn('Connect'));
  if (sub === 'listening') return line('ok', '<b>Listening on Twitch</b>');
  if (sub === 'needs_consent') return line('off', 'Twitch needs your OK for this one: your login is from before it existed.', toConn('Allow'));
  if (/^error/.test(sub)) return line('off', esc(sub.replace(/^error: /, '')));
  if (ev.state === 'error') return line('off', esc(ev.problem || 'Can\'t reach Twitch right now.'));
  if (sub === 'pending' || ev.state === 'connecting') return line('wait', 'Connecting to Twitch…');
  return line('wait', 'Listens once this integration is on and saved.');
}
// A Twitch event in a list: "follows, subs and raids".
const TWITCH_NOUNS = {twitch_follow:'follows', twitch_sub:'subs', twitch_resub:'resubs', twitch_gift:'gifted subs',
  twitch_cheer:'bits', twitch_raid:'raids', twitch_redeem:'channel point rewards', twitch_ad:'ad breaks'};
function twitchNouns(ids){
  const words = ids.map(id => TWITCH_NOUNS[id] || id);
  return words.length > 1 ? words.slice(0, -1).join(', ') + ' and ' + words[words.length - 1] : words.join('');
}
// The Twitch events, by id, in this state.
function twitchEventsIn(state){
  const subs = ((S.data.twitch.events || {}).subs) || {};
  return Object.keys(subs).filter(id => subs[id] === state);
}
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
// Checks nest 4 deep at most, the top level counting as the first.
// Mirrors model::MAX_DEPTH: an If needs room for its own lanes too.
const MAX_NEST = 4;
const branchDepth = path => path.filter(x => x === 'then' || x === 'else').length;
// How many levels of lanes a step opens below itself.
function nestLevels(s){
  return s && s.type === 'if' ? 1 + Math.max(0, ...(s.then || []).concat(s.else || []).map(nestLevels)) : 0;
}
// Whether a step opening `levels` levels fits in the list at `listPath`.
const nestFits = (listPath, levels) => branchDepth(listPath) + levels <= MAX_NEST - 1;

const Builder = {
  d:null, h:0, sel:'trigger', target:[], after:null, run:null, dirty:false, lastField:null, back:null,
  testValues:{}, testDry:true, testOpen:false,
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
    this.testDry = true;
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
  // After `d` is swapped for another version of it (Discard, Undo).
  reset(){
    this.h = Math.min(this.h, this.d.handlers.length - 1);
    this.sel = 'trigger';
    this.target = [];
    this.after = null;
    this.choosing = null;
    this.unpicked = new WeakSet();
    this.lastField = null;
    this.resetTest();
    TRIES.clear();
  },
  show(){
    Modal.open('builder', 1680, 'min(94vh,1120px)');
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
          ${PALETTE.map(([group, items]) => `<div class="ic-label">${group}</div>${items.map(([k, label]) =>
            `<button data-act="b-add" data-kind="${k}" style="--c:${KINDS[k].c}"${LOCAL_KINDS.includes(k) && localLocked()
              ? ` class="ig-pal-locked" aria-disabled="true" title="${esc(LOCAL_LOCK_TEXT)}"` : ` title="${esc(KINDS[k].hint)}"`}><span class="ig-pal-ic">${svg(KINDS[k].icon)}</span>${esc(label)}</button>`).join('')}`).join('')}
        </aside>
        <section class="ig-canvas" aria-label="Steps">
          <div class="ig-triggers" data-flip>${this.triggerTabs().map((label, i) => `<button class="ig-trig${i === this.h ? ' on' : ''}${d.handlers[i].enabled ? '' : ' ig-dim'}" data-act="b-handler" data-n="${i}" data-key="h-${uidOf(d.handlers[i])}"${d.handlers[i].enabled ? '' : ' title="Switched off"'}${i === this.h ? ' aria-current="true"' : ''}>
            ${esc(label)}</button>`).join('')}
            <button class="ig-trig add" data-act="b-add-handler" data-key="h-add">${svg('plus')}Another trigger</button></div>
          ${this.choosing ? this.chooserHtml(h) : `<div class="ig-steps" data-flip data-list="">
            <button class="ig-step ig-when${this.sel === 'trigger' ? ' on' : ''}" data-act="b-sel" data-path="trigger" data-key="when">
              <span class="ig-step-ic">${svg(triggerKind(h.trigger).icon)}</span><span class="ig-step-body"><span class="ig-step-kind">When</span><span class="ig-step-text">${esc(triggerSentence(h.trigger))}</span></span></button>
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
      // A mistyped name is quieter than what's missing, and waits for it.
      const unknown = missing || off ? [] : unknownVars(this.handler(), s);
      let html = `<div class="ig-step${selKey === pk ? ' on' : ''}${off ? ' off' : ''}${missing ? ' warn' : ''}" data-path="${pk}" style="--c:${k.c}" data-key="s-${id}">
        <button type="button" class="ig-step-main" data-act="b-sel" data-path="${pk}"${selKey === pk ? ' aria-current="true"' : ''}
          title="Alt+Up or Alt+Down moves it, Ctrl+D duplicates it, Delete removes it">
          <span class="ig-step-ic">${svg(k.icon || 'steps')}</span><span class="ig-step-body"><span class="ig-step-kind">${esc(k.label)}</span><span class="ig-step-text">${cardSentence(s)}${off ? '<span class="sr-only">, switched off</span>' : ''}</span></span></button>
        ${missing ? `<span class="ig-step-warn">${esc(missing)}</span>`
          : unknown.length ? `<span class="ig-step-warn soft">${unknown.map(u => `{${esc(u.name)}}`).join(' ')}: nothing fills ${unknown.length > 1 ? 'these' : 'it'}</span>` : ''}
        <span class="ig-step-tools">
          ${tool('b-move', pk, 'up', 'Move up', ` data-dir="-1"${this.moveTarget(path, -1) ? '' : ' disabled'}`)}
          ${tool('b-move', pk, 'down', 'Move down', ` data-dir="1"${this.moveTarget(path, 1) ? '' : ' disabled'}`)}
          ${tool('b-toggle-step', pk, off ? 'eyeOff' : 'eye', off ? 'Switch this step on' : 'Switch this step off')}
          ${tool('b-dup', pk, 'copy', 'Duplicate')}
          ${tool('b-del', pk, 'x', 'Remove')}
        </span></div>`;
      if (s.type === 'if'){
        html += `<div class="ig-branch" data-key="br-${id}"><div class="ig-lanes">
          <div class="ig-lane yes"><div class="ig-branch-label">${svg('check')}Then</div><div class="ig-branch-list" data-flip data-list="${pathKey(path.concat('then'))}">${this.stepsHtml(s.then, path.concat('then'), id + 't')}</div></div>
          <div class="ig-lane no"><div class="ig-branch-label else">${svg('x')}Otherwise</div><div class="ig-branch-list" data-flip data-list="${pathKey(path.concat('else'))}">${this.stepsHtml(s.else, path.concat('else'), id + 'e')}</div></div>
        </div></div>`;
      }
      if (afterKey === pk){
        html += `<button class="ig-drop after on" data-act="b-target" data-path="${here}" data-key="after-${id}" title="Click to add at the end instead">Next block goes here</button>`;
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
  // Whether `path` leads to a step of this trigger.
  has(path){
    let list = this.handler().steps;
    for (let n = 0; list && n < path.length - 1; n += 2) list = (list[path[n]] || {})[path[n + 1]];
    return !!(list && list[path[path.length - 1]]);
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
  // Whether `count` more steps, opening `levels` levels of lanes, fit in
  // the list at `base`.
  roomFor(base, count, levels){
    let total = 0;
    this.d.handlers.forEach(h => walk(h.steps, () => { total++; }));
    if (total + count > 64){ toast('That\'s the most steps one integration can have (64)', 'info'); return false; }
    if (!nestFits(base, levels || 0)){ toast('Checks go 4 deep at most: add it a level up', 'info'); return false; }
    return true;
  },
  // Put a dragged block, or a dragged step, at place `at` of the list at
  // path `listKey` (counted without the step), and say whether it went.
  // The list is held before the step leaves its old place, which can
  // renumber the path to it.
  dropAt(drag, listKey, at){
    const listPath = this.resolve(listKey), list = this.listAt(listPath);
    if (drag.kind){
      const step = newStep(drag.kind, this.handler().trigger);
      if (!this.roomFor(listPath, 1, nestLevels(step))) return false;
      list.splice(at, 0, step);
      nameAnswer(this.handler(), step);
      this.sel = listPath.concat(at);
      // Like a click: a new check's blocks go inside it.
      if (step.type === 'if'){ this.target = this.sel.concat('then'); this.after = null; }
      else { this.after = this.sel; this.target = []; }
    } else {
      const {list:from, index} = this.locate(this.resolve(drag.path));
      const step = from[index];
      from.splice(index, 1);
      list.splice(at, 0, step);
      walk(this.handler().steps, (s, here) => { if (s === step) this.sel = here; });
      this.after = null;
      this.target = [];
    }
    this.changed();
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
    return nestFits(target.path.slice(0, -1), nestLevels(list[index])) ? target : null;
  },
  inspectorHtml(){
    const h = this.handler();
    const selected = Array.isArray(this.sel) ? this.stepAt(this.sel) : null;
    // Only what this step can use: the trigger's values, what steps
    // before it made, and the live ones.
    const upto = selected || (h.steps || [])[0];
    const varsBox = `<div class="ig-insp-vars"><div class="ic-label">Values it can use</div>
      <div class="muted">Click a text field, then a value to put it there.</div>
      <div class="ig-vals">${tokensHtml(varsFor(h, upto), 'b-token', '', true)}</div>${FORMS_HELP}</div>`;
    if (this.choosing){
      return `<div class="ig-insp-title" style="--c:#5ac8fa"><span class="ig-insp-ic">${svg('bolt')}</span><span><small>WHEN</small><b>${this.choosing === 'event' ? 'What happens?' : 'How does it start?'}</b></span></div>
        <p class="ig-insp-p">Pick it in the middle. Then add steps from the list on the left: each one runs in order.</p>`;
    }
    // Only shown stacked, where the steps are a long way up.
    const back = `<button type="button" class="ig-insp-back" data-act="b-back-steps">${svg('up')}Back to steps</button>`;
    if (this.sel === 'trigger') return back + this.triggerInspector(h) + varsBox;
    if (!selected) return varsBox;
    const k = KINDS[selected.type];
    const off = selected.enabled === false;
    return `${back}<div class="ig-insp-title" style="--c:${k.c}"><span class="ig-insp-ic">${svg(k.icon || 'steps')}</span><span>${k.tag.toLowerCase() === k.label.toLowerCase() ? '' : `<small>${k.tag}</small>`}<b>${esc(k.label)}</b></span></div>
      <div class="muted ig-insp-hint">${esc(k.hint)}</div>
      ${off ? warnHtml('This step is switched off: runs skip it. <a href="#" data-act="b-toggle-step" data-path="' + pathKey(this.sel) + '">Switch it on</a>') : ''}
      ${answerClash(h, selected) ? warnHtml(`Another step also keeps its answer as <b>${esc(answerName(selected))}</b>, so later steps get whichever ran last. <a href="#" data-act="b-name-answer">Give this one its own name</a>`) : ''}
      ${unknownVarsHtml(unknownVars(h, selected))}${this.stepFields(selected)}${varsBox}`;
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
      case 'if': return this.ifFields(s, note);
      case 'stop': return field('quiet', 'In the history', {select:[['', 'This run shows'], ['yes', 'This run is left out']]})
        + note('Ends this run here. Leave it out of the history when it stops most of the time, like a timer that rolls for a rare drop.');
      case 'viewer': {
        const everyone = s.params.who === 'active';
        return field('value', 'Value (letters, digits and _)', {mono:true, fixed:true, ph:'coins'})
          + field('who', 'Whose', {select:[['', 'The viewer who started this'], ['target', 'Who they named (@name)'], ['named', 'A viewer this step names'], ['active', 'Everyone who chatted lately']]})
          + (s.params.who === 'named' ? field('login', 'Viewer', {mono:true, ph:'{reply.user_login}'}) : '')
          + (everyone ? field('minutes', 'Chatted in the last (minutes)', {mono:true, ph:'10'})
            : field('op', 'Do', {select:[['', 'Give'], ['spend', 'Spend: only if they have enough'], ['subtract', 'Take: never below 0'], ['set', 'Set to'], ['reset', 'Reset to 0']]}))
          + (s.params.op === 'reset' && !everyone ? '' : field('by', 'Amount', {mono:true, ph:'1'}))
          + note(s.params.op === 'spend' && !everyone ? `Follow it with an If on ${MONO}{paid}</span>: yes when it was taken, no when they hold too little (or the amount isn't a whole number from 1 up).`
            : `One number per viewer, kept between streams. Read it as ${MONO}{viewer.${esc(s.params.value || 'coins')}}</span> or ${MONO}{target.${esc(s.params.value || 'coins')}}</span>, add ${MONO}.rank</span> for their place, and ${MONO}{top.${esc(s.params.value || 'coins')}}</span> for the five who hold the most.`)
          + holdersHtml((s.params.value || '').trim());
      }
      case 'math': return field('a', 'Number', {mono:true, ph:'{arg1}'})
        + field('op', 'Do', {select:Object.entries(MATH_OPS)})
        + field('b', 'And', {mono:true, ph:'2'})
        + field('save_as', 'Save it as', {mono:true, fixed:true, ph:'result'})
        + note(`Whole numbers only. Later steps use it as ${MONO}{${esc((s.params.save_as || '').trim() || 'result')}}</span>.`);
      case 'wait_chat': {
        const n = esc((s.params.save_as || '').trim() || 'reply');
        return field('text', 'A message that starts with (empty: any message)', {mono:true, ph:'!accept'})
          + field('from', 'From', {select:[['', 'Anyone'], ['viewer', 'The viewer who started this'], ['target', 'Who they named (@name)']]})
          + field('seconds', 'Wait up to (seconds, 300 at most)', {mono:true, ph:'30'})
          + field('save_as', 'Save the answer as', {mono:true, fixed:true, ph:'reply'})
          + note(`${MONO}{${n}.ok}</span> is yes when someone answered in time, no otherwise. Then ${MONO}{${n}.user}</span>, ${MONO}{${n}.user_login}</span>, ${MONO}{${n}.message}</span> and ${MONO}{${n}.arg1}</span> (its second word) say who and what.`);
      }
      case 'reward': {
        const made = (Rewards.list || []).map(r => r.title);
        // A reward typed before the list was read, or made elsewhere since, still shows.
        const titles = made.includes(s.params.title) || !s.params.title ? made : [s.params.title].concat(made);
        return (titles.length ? field('title', 'Reward', {select:[['', 'Pick a reward']].concat(titles.map(t => [t, t]))})
            : field('title', 'Reward (its exact name)', {fixed:true, ph:'Hydrate'}))
          + field('action', 'Switch it', {select:[['off', 'Off: viewers no longer see it'], ['', 'On: viewers can redeem it']]})
          + note('For a reward that should only be there some of the time: off while a round is running, on again when it is over.')
          + rewardsHtml();
      }
      case 'call': {
        const blocks = blockNames(this.d);
        return (blocks.length ? field('block', 'Block', {select:[['', 'Pick a block']].concat(blocks.map(b => [b, b]))})
          : note('No block yet. Add a trigger of the kind "A block of steps", name it and give it steps.'))
          + note('Its steps run here with this run\'s values, and what they save stays for the next steps. A Stop inside the block stops this run too.');
      }
      case 'delay_action': return field('action', 'Does', {select:Object.entries(DELAY_ACTIONS)})
        + (s.params.action === 'arm' ? field('seconds', 'Delay in whole seconds (0 turns it off, blank uses your Default delay), or a value like {arg1}', {mono:true, ph:'30'})
          + note('Starts or changes the delay. A delay the buffer can\'t hold fails this step.') : '')
        + warnHtml('This changes your stream. Tests never run it.');
      case 'marker': return field('description', 'Label', {ph:'Crash'}) + note('Only works while you are live on Twitch.');
      case 'clip': return note('Clips the last moments of your stream. Later steps can use <span class="mono">{clip.url}</span>, <span class="mono">{clip.ok}</span> and, when it failed, <span class="mono">{clip.error}</span>.');
      case 'program': return (localLocked() ? warnHtml(esc(LOCAL_LOCK_TEXT)) : '')
        + field('path', 'Program (a fixed path, no variables)', {mono:true, fixed:true, ph:'C:\\Tools\\thing.exe'}) + field('args', 'Arguments', {mono:true, ph:'--scene "Replay"'})
        + note('Values like <span class="mono">{arg1}</span> keep only plain letters, numbers, spaces and <span class="mono">_ . , + = / -</span>, so chat can\'t sneak in a command or an option of its own.')
        + field('wait', 'Its answer', {select:[['', 'Don\'t wait for it'], ['yes', 'Wait for it and keep what it prints (10 s at most)']]})
        + (s.params.wait === 'yes' ? field('save_as', 'Save the answer as', {mono:true, fixed:true, ph:'program'})
          + note(`Later steps can use <span class="mono">{${esc((s.params.save_as || '').trim() || 'program')}.output}</span> (what it printed), <span class="mono">.code</span> (how it ended) and <span class="mono">.ok</span> (yes when the code is 0). It runs with no window; one still running after 10 s is stopped.`) : '')
        + warnHtml('Runs on your PC. Recipes you import can never add this without asking you first.');
      case 'file': return (localLocked() ? warnHtml(esc(LOCAL_LOCK_TEXT)) : '') + field('path', 'File (a fixed path, no variables)', {mono:true, fixed:true, ph:'C:\\Stream\\status.txt'}) + field('text', 'Text', {area:true}) + field('mode', 'Each time', {select:[['', 'Replace the text'], ['append', 'Add a line']]});
      case 'set_var': return field('name', 'Name', {mono:true, fixed:true, ph:'winner'}) + field('value', 'Value', {ph:'{user}'})
        + field('keep', 'Keep it', {select:[['', 'For this run'], ['yes', 'Between runs and streams']]})
        + note(s.params.keep === 'yes' ? `Later steps use it as ${MONO}{${esc(s.params.name || 'name')}}</span>; any other run reads it as ${MONO}{saved.${esc(s.params.name || 'name')}}</span>.`
          : 'Later steps use it as <span class="mono">{name}</span>. Lasts for this run.');
      case 'overlay': return field('title', 'Small title', {ph:'Clip'}) + field('text', 'Text', {area:true, ph:'{user} just clipped that'})
        + field('seconds', 'On screen for', {select:[['3', '3 s'], ['6', '6 s'], ['10', '10 s'], ['15', '15 s'], ['30', '30 s']]
          .concat(['', '3', '6', '10', '15', '30'].includes(s.params.seconds || '') ? [] : [[s.params.seconds, s.params.seconds + ' s']])})
        + field('color', 'Accent, like #ff7eb6 (empty: the theme\'s)', {mono:true, fixed:true, ph:'the theme\'s'})
        + field('sound', 'Sound: theme, chime, pop, bell, rise, coin, sparkle, whoosh, fanfare, zap, blip, an upload (media:name) or a link', {mono:true, fixed:true, ph:'none'})
        + field('volume', 'Volume, 0 to 100', {mono:true, fixed:true, ph:'70'})
        + field('image', 'Picture: an upload (media:name) or a link to a PNG, JPG or GIF', {mono:true, fixed:true, ph:'https://…/party.gif'})
        + alertsStatusHtml(true)
        + `<button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="alert-show">${svg('play', ' class="ig-btn-ic"')}Show it on stream</button>`
        + note('Tests never put anything on stream: "Show it on stream" does, with sample values.');
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
        + field('by', 'By', {mono:true, ph:'1'}) + field('save_as', 'Also save it as (optional)', {mono:true, fixed:true, ph:'coins'})
        + note(`Kept between runs. Use it anywhere as <span class="mono">{counter.${esc(s.params.name || 'name')}}</span>. When the name holds a value like <span class="mono">{user_login}</span>, save it under a fixed name to use it in later steps.`);
      case 'obs': return this.obsFields(s, field, note);
      case 'light': return lightPickerHtml(s.params) + lightControlsHtml(s.params, name => `data-bind="p" data-name="${name}"`)
        + note('The device is fixed here, so chat can never pick it. Color and brightness may use values: {arg1} turns "!light blue" blue. A dry test never touches your lights.');
    }
    return '';
  },
  // An If's checks: what each looks at, how it compares, and with what.
  // A yes or no value gets yes and no to click. Several checks are joined
  // by "and" or "or", set once for all of them.
  ifFields(s, note){
    const rows = conditionRows(s), any = s.params.match === 'any';
    const samples = {};
    varsFor(this.handler(), s).forEach(v => { samples[v.name] = v.sample; });
    const isYesNo = left => {
      const m = /^\{([A-Za-z0-9_.]{1,64})\}$/.exec(left.trim());
      return !!m && /^(yes|no)$/.test(samples[m[1]] || '');
    };
    const row = (r, n) => {
      const alone = OPS_ALONE.includes(r.op);
      const quick = !alone && isYesNo(r.left) ? `<div class="ig-cond-quick">${['yes', 'no'].map(v =>
        `<button type="button" class="ig-start${r.right.trim() === v ? ' on' : ''}" data-act="b-cond-val" data-name="right${r.sfx}" data-v="${v}" aria-pressed="${r.right.trim() === v}">${v}</button>`).join('')}</div>` : '';
      return `${n ? `<div class="ig-cond-join">${any ? 'or' : 'and'}</div>` : ''}<div class="ig-cond">
        <div class="ig-cond-top"><input class="ic-input mono" data-bind="p" data-name="left${r.sfx}" value="${esc(r.left)}" placeholder="{user_role}" spellcheck="false" aria-label="What it checks">
        ${rows.length > 1 ? `<button type="button" class="ic-btn-tiny" data-act="b-cond-del" data-n="${n}" aria-label="Remove this check" title="Remove this check">${svg('x')}</button>` : ''}</div>
        <div class="ig-cond-cmp${alone ? ' alone' : ''}"><select class="ic-input" data-bind="p" data-name="op${r.sfx}" aria-label="How it compares">${Object.entries(OPS).map(([id, label]) =>
          `<option value="${id}" ${id === r.op ? 'selected' : ''}>${esc(label)}</option>`).join('')}</select>
        ${alone ? '' : `<input class="ic-input mono" data-bind="p" data-name="right${r.sfx}" value="${esc(r.right)}" placeholder="mod" spellcheck="false" aria-label="Value">`}</div>${quick}</div>`;
    };
    const match = rows.length < 2 ? '' : `<div class="dff"><label>Then runs when</label><div class="sub-tabs ig-seg">${[['all', 'all are true'], ['any', 'any one is true']].map(([id, label]) =>
      `<button class="sub-tab${(id === 'any') === any ? ' on' : ''}" data-act="b-cond-match" data-v="${id}" aria-pressed="${(id === 'any') === any}">${label}</button>`).join('')}${SEG_IND}</div></div>`;
    return `<div class="dff"><label>${rows.length > 1 ? 'Checks' : 'Check'}</label><div class="ig-conds">${rows.map(row).join('')}</div>
      ${rows.length < MAX_CONDITIONS ? `<button type="button" class="ig-cond-add" data-act="b-cond-add">${svg('plus')}Add a check</button>` : ''}</div>${match}`
      + note('Text compares ignoring case; "is more than" compares numbers. Click a value below to put it in the field you last clicked.');
  },
  // Pick names from OBS's own list: a find box over it, one line each.
  // A typed name is kept too, as OBS may be closed or the source not made
  // yet. `multi`: several names, one per line in the field. `items` are
  // names, or [name, label].
  pickHtml(s, name, label, items, opts){
    const multi = !!opts.multi;
    const picked = multi ? listedNames(s.params[name]) : [(s.params[name] || '').trim()].filter(Boolean);
    const key = uidOf(s) + ':' + name, find = (this.pickFind || {})[key] || '', typed = find.trim();
    const all = items.map(x => Array.isArray(x) ? x : [x, x]);
    picked.filter(v => !all.some(([x]) => x === v)).forEach(v => all.unshift([v, v]));
    const shown = typed ? all.filter(([v, l]) => (v + ' ' + l).toLowerCase().includes(typed.toLowerCase())) : all;
    const row = ([v, l], isNew) => {
      const on = picked.includes(v);
      return `<button type="button" class="ig-pick-row${on ? ' on' : ''}${isNew ? ' new' : ''}" role="option" aria-selected="${on}" data-act="b-obs-pick" data-name="${name}" data-v="${esc(v)}"${multi ? ' data-multi="1"' : ''}${isNew ? ' data-new="1"' : ''} title="${esc(v)}"><span class="ig-pick-mark${multi ? ' box' : ''}">${svg('check', ' stroke-width="3"')}</span><span class="ig-pick-t">${isNew ? `Use “${esc(l)}”` : esc(l)}</span></button>`;
    };
    const rows = shown.map(x => row(x)).join('') + (typed && !all.some(([v]) => v === typed) ? row([typed, typed], true) : '');
    const empty = all.length ? 'Nothing matches.' : opts.offline;
    const head = picked.length ? `<span class="ig-pick-sum" title="${esc(picked.join(', '))}">${multi ? `${picked.length} picked` : esc(picked[0])}</span><button type="button" class="ig-pick-clear" data-act="b-obs-clear" data-name="${name}">Clear</button>` : '';
    return `<div class="dff ig-pick"><div class="ig-pick-h"><span class="ig-pick-l">${label}</span>${head}</div>
      <div class="ig-pick-box"><div class="ig-pick-find">${svg('search')}<input data-bind="pick-find" data-name="${esc(key)}" value="${esc(find)}" placeholder="${esc(opts.ph)}" spellcheck="false" aria-label="Find or type a name"></div>
      <div class="ig-pick-list" role="listbox"${multi ? ' aria-multiselectable="true"' : ''} aria-label="${esc(label)}">${rows || `<div class="ig-pick-none">${esc(empty)}</div>`}</div></div></div>`;
  },
  // An OBS step: what it does, and the scenes or sources, picked from
  // OBS's own lists while it's connected.
  obsFields(s, field, note){
    const action = s.params.action || 'scene';
    const o = ObsLive.status;
    const offline = 'OBS isn\'t connected: type the name above.';
    const named = (name, label, ph, items, multi) => this.pickHtml(s, name, label, items, {ph, multi, offline});
    const scenes = (o && o.scenes) || [], sources = (o && o.sources) || [];
    const filters = (o && o.filters) || {}, hotkeys = (o && o.hotkeys) || [];
    // The actions in groups, as OBS's own menus split them.
    const groups = {};
    Object.entries(OBS_ACTIONS).forEach(([id, [label, , group]]) => { (groups[group] = groups[group] || []).push([id, label]); });
    let body = `<div class="dff"><label>Does</label><select class="ic-input" data-bind="p" data-name="action">${Object.entries(groups).map(([group, list]) =>
      `<optgroup label="${esc(group)}">${list.map(([id, label]) => `<option value="${id}" ${id === action ? 'selected' : ''}>${esc(label)}</option>`).join('')}</optgroup>`).join('')}</select></div>`;
    const need = (OBS_ACTIONS[action] || [])[1];
    if (action === 'scene') body += named('scene', 'Scene', 'Find or type a scene', scenes);
    else if (action === 'check'){
      const picked = listedNames(s.params.source);
      body += named('source', 'Sources to check too (optional)', 'Find or type a source', sources, true);
      if (picked.length) body += named('scene', 'Showing in the scene (none: the one on air)', 'Find or type a scene', scenes);
      const n = esc(answerName(s)), v = f => `<span class="mono">{${n}.${f}}</span>`;
      body += field('save_as', 'Keep the answer as', {mono:true, fixed:true, ph:'obs'})
        + note(`Later steps find it under "Values it can use" as ${v('scene')}, ${v('streaming')}, ${v('recording')}${!picked.length ? ''
          : `, ${v('muted')} and ${v('visible')}${picked.length > 1 ? ' (yes when all of them are), and each source\'s own' : ''}`}. Each check keeps its own name.`);
    }
    else if (need === 'filter'){
      // Only sources and scenes that have a filter, then that one's filters.
      const owners = Object.keys(filters).filter(name => filters[name].length);
      body += named('source', 'Source or scene', 'Find or type a source or scene', owners);
      body += named('filter', 'Filter', 'Find or type a filter', filters[s.params.source] || []);
      if (o && o.state === 'connected' && !owners.length) body += note('None of your sources or scenes has a filter yet. In OBS: right-click a source, Filters.');
    } else if (need === 'hotkey'){
      body += named('hotkey', 'Hotkey', 'Find a hotkey, or type one like OBSBasic.StartVirtualCam', hotkeys.map(x => [x, hotkeyLabel(x)]));
    } else if (need === 'source'){
      // A text source takes one text; anything else can take several.
      const one = action === 'text';
      body += named('source', one ? 'Text source' : 'Sources', 'Find or type a source', sources, !one);
      if (one) body += field('text', 'Text', {area:true, ph:'Crashes today: {counter.crashes}'});
      else if (['show', 'hide', 'toggle'].includes(action)) body += named('scene', 'In the scene (none: the one on air)', 'Find or type a scene', scenes);
    }
    if (OBS_NOTES[action]) body += note(OBS_NOTES[action]);
    return `<div data-obs-live>${obsStatusHtml()}</div>${body}`
      + note(action === 'check' ? 'Changes nothing in OBS, so tests run it too. Names are fixed here, so chat can never pick them.'
        : need ? 'Names are fixed here, so chat can never pick them. Tests never touch OBS.' : 'Tests never touch OBS.');
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
      body += twitchEventStatusHtml(t);
      // Channel points: the rewards made here are managed where they are used.
      if (t.event === 'twitch_redeem') body += rewardPickHtml(t) + rewardsHtml();
      if (isObsEvent(t)) body += '<div class="muted">Heard from OBS through its WebSocket server: switch it on once in OBS, Tools › WebSocket Server Settings.</div>';
      body += paramsHtml(t, 't-');
      // Which reward is picked above, from a list: not typed here.
      const filters = Editor.filterVars(t).filter(v => !(t.event === 'twitch_redeem' && v.name === 'reward'));
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
      body = `${commandClashHtml(this.d)}<div class="dff"><label>Command</label><input class="ic-input mono" data-bind="t-command" value="${esc(t.command || '')}" placeholder="!clip" spellcheck="false"></div>
        <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="t-aliases" value="${esc((t.aliases || []).join(' '))}" spellcheck="false"></div>
        <div class="dff"><label>Who can use it</label>${rolesHtml(t, 't-role')}</div>
        <div class="dff"><label>Each viewer, at most once every</label>${durInput('t-ucd-dur', t.user_cooldown_ms || 0, 's')}</div>
        ${t.user_cooldown_ms ? `<div class="dff"><label>Tell a viewer who is too early (optional)</label><input class="ic-input" data-bind="t-cd-reply" value="${esc(t.cooldown_reply || '')}" placeholder="Come back in {cooldown_left}."></div>
        <div class="muted">Each viewer hears it once per wait. Waits of 10 minutes or more are remembered when the app restarts.</div>` : ''}`;
    } else if (t.type === 'block'){
      body = `<div class="dff"><label>Name</label><input class="ic-input" data-bind="t-block-name" value="${esc(t.name || '')}" placeholder="check the bet" maxlength="40" spellcheck="false"></div>
        <div class="muted">It never starts on its own: a "Run a block" step in another trigger runs it, with that run's values. A Stop inside it stops that run.</div>`;
    } else if (t.type === 'chat_message'){
      const mode = t.mode || 'contains';
      body = `<div class="dff"><label>Matches</label><input class="ic-input" data-bind="t-pattern" value="${esc(t.pattern || '')}" placeholder="gg"></div>
        <div class="dff"><label>How</label><select class="ic-input" data-bind="t-mode">${Object.entries(MATCH_MODES).map(([id, m]) => `<option value="${id}" ${id === mode ? 'selected' : ''}>${m.label}</option>`).join('')}</select>
          <div class="muted">${MATCH_MODES[mode].means}</div>${matchExamplesHtml(t.pattern || '', mode)}</div>
        <div class="dff"><label>Try a message</label><input class="ic-input" data-bind="t-try" value="${esc(this.tryText || '')}" placeholder="Type what a viewer might say">
          ${(this.tryText || '').trim() && (t.pattern || '').trim() ? `<div class="ig-fires-row"><span class="ig-fires ${textMatches(this.tryText, t.pattern, mode) ? 'yes' : 'no'}">${textMatches(this.tryText, t.pattern, mode) ? 'This message fires it' : 'This message does not fire it'}</span></div>` : ''}</div>
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
    return `<div class="ig-insp-title" style="--c:#5ac8fa"><span class="ig-insp-ic">${svg(triggerKind(t).icon)}</span><span><small>WHEN</small><b>${esc(triggerLabel(t))}</b></span></div>
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
    else if (b === 't-reward-other'){
      setRewardChoice(t, rewardChoice(t).picked, el.value.split(',').map(x => x.trim()).filter(Boolean));
      // Typing a name must not redraw the field under the caret.
      this.dirty = true;
      return;
    }
    else if (b === 't-command') t.command = el.value.trim().toLowerCase();
    else if (b === 't-aliases') t.aliases = el.value.split(/[\s,]+/).map(x => x.trim().toLowerCase()).filter(Boolean);
    else if (b === 't-role'){ t.roles = t.roles || {}; t.roles[el.dataset.name] = el.checked; }
    else if (b === 't-ucd-dur'){ const ms = durationOf(0, 86400000); if (ms == null) return; t.user_cooldown_ms = ms; }
    else if (b === 't-pattern') t.pattern = el.value;
    else if (b === 't-cd-reply') t.cooldown_reply = el.value.trim();
    else if (b === 't-block-name') this.renameBlock(t, el.value.trim());
    else if (b === 't-mode') t.mode = el.value;
    // Trying a message changes nothing that is saved.
    else if (b === 't-try'){ this.tryText = el.value; this.render(); return; }
    else if (b === 't-every-dur'){ const ms = durationOf(60000, 7 * 86400000); if (ms == null) return; t.every_ms = ms; }
    else if (b === 't-live') t.only_live = el.checked;
    else if (b === 't-param'){ if (!paramInput(t, el)) return; }
    else if (rulesInput(t, el, 't-') || sceneInput(t, el, 't-') || onlyLiveInput(t, el, 't-') || onlyLiveInput(t, el, '')){ /* changed below */ }
    else if (b === 'p'){
      const s = this.stepAt(this.sel);
      s.params[el.dataset.name] = el.value;
      if (el.dataset.name === 'action' || el.dataset.name === 'wait') nameAnswer(h, s);
    }
    else if (b === 'p-dur'){ const ms = durationOf(0, 3600000); if (ms == null) return; this.stepAt(this.sel).params[el.dataset.name] = String(ms); }
    else if (b === 'bf-name' || b === 'bf-value'){
      const s = this.stepAt(this.sel);
      Editor.fields(s, list => { const f = list[+el.dataset.n]; if (f) f[b === 'bf-name' ? 'name' : 'value'] = el.value; });
    }
    else if (b === 'test-value'){ this.testValues[el.dataset.name] = el.value; return; }
    // Finding in a list narrows it; nothing in the integration changes.
    else if (b === 'pick-find'){ this.pickFind = Object.assign({}, this.pickFind, {[el.dataset.name]:el.value}); this.render(); return; }
    else if (b === 'try'){
      // Values to try the request with; not part of the integration.
      tryEntry(this.stepAt(this.sel)).inputs[el.dataset.name] = el.value;
      return;
    }
    else if (!quietInput(this.d, el)) return;
    this.changed();
  },
  // A block's new name, in the block and in every step that runs it.
  renameBlock(t, name){
    const was = t.name;
    const follow = steps => (steps || []).forEach(s => {
      if (s.type === 'call' && was && s.params.block === was) s.params.block = name;
      follow(s.then); follow(s.else);
    });
    this.d.handlers.forEach(h => follow(h.steps));
    t.name = name;
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
      case 'alert-show': { const s = selected(); if (s) showAlertNow(s, this.handler(), el); return true; }
      case 'b-obs-pick': {
        const s = selected();
        if (!s) return true;
        const name = el.dataset.name, v = el.dataset.v;
        if (el.dataset.multi){
          const list = listedNames(s.params[name]), at = list.indexOf(v);
          if (at < 0) list.push(v); else list.splice(at, 1);
          s.params[name] = list.join('\n');
        } else s.params[name] = (s.params[name] || '').trim() === v ? '' : v;
        // A typed name, now picked, leaves the find box for the next one.
        if (el.dataset.new && this.pickFind) delete this.pickFind[uidOf(s) + ':' + name];
        this.changed();
        return true;
      }
      case 'b-reward-scope': {
        const t = this.handler().trigger;
        if (el.dataset.v === 'any'){ REWARD_ONLY.delete(t); setRewardChoice(t, [], []); }
        else REWARD_ONLY.add(t);
        this.changed();
        return true;
      }
      case 'b-reward-pick': {
        const t = this.handler().trigger, c = rewardChoice(t), id = el.dataset.id;
        setRewardChoice(t, c.picked.includes(id) ? c.picked.filter(x => x !== id) : c.picked.concat(id), c.other);
        this.changed();
        return true;
      }
      case 'b-cond-add': {
        const s = selected();
        if (s){ const rows = conditionRows(s); if (rows.length < MAX_CONDITIONS) setConditions(s, rows.concat({left:'', op:'is', right:''})); }
        this.changed();
        return true;
      }
      case 'b-cond-del': {
        const s = selected();
        if (s){ const rows = conditionRows(s); rows.splice(+el.dataset.n, 1); setConditions(s, rows); }
        this.changed();
        return true;
      }
      case 'b-cond-val': { const s = selected(); if (s) s.params[el.dataset.name] = el.dataset.v; this.changed(); return true; }
      case 'b-cond-match': {
        const s = selected();
        if (s){ if (el.dataset.v === 'any') s.params.match = 'any'; else delete s.params.match; }
        this.changed();
        return true;
      }
      case 'b-obs-clear': { const s = selected(); if (s) s.params[el.dataset.name] = ''; this.changed(); return true; }
      case 'light-use': { const s = selected(); if (s) useDevice(s, el); this.changed(); return true; }
      case 'l-swatch': { const s = selected(); if (s) s.params.color = el.dataset.v; this.changed(); return true; }
      case 'bf-add': { const s = selected(); if (s) Editor.fields(s, list => list.push({name:'', value:'', inline:true})); this.changed(); return true; }
      case 'bf-del': { const s = selected(); if (s) Editor.fields(s, list => list.splice(+el.dataset.n, 1)); this.changed(); return true; }
      case 'bf-inline': {
        const s = selected();
        if (s) Editor.fields(s, list => { const f = list[+el.dataset.n]; if (f) f.inline = !f.inline; });
        this.changed();
        return true;
      }
      case 'b-sel': {
        const picked = el.dataset.path === 'trigger' ? 'trigger' : this.resolve(el.dataset.path);
        // A step still fading out after a switch of trigger isn't this trigger's.
        if (Array.isArray(picked) && !this.has(picked)) return true;
        this.sel = picked;
        // New blocks land right after the step picked.
        this.after = Array.isArray(this.sel) ? this.sel : null;
        this.render();
        this.showInspector();
        return true;
      }
      case 'b-back-steps': {
        const step = q('.ig-step.on') || q('.ig-when.on') || q('.ig-canvas');
        if (step) step.scrollIntoView({block:'center', behavior:reduced() ? 'auto' : 'smooth'});
        const main = step && (step.matches('button') ? step : step.querySelector('.ig-step-main'));
        if (main) main.focus({preventScroll:true});
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
        if (!this.roomFor(base, 1, nestLevels(step))) return true;
        list.splice(at, 0, step);
        nameAnswer(this.handler(), step);
        this.sel = base.concat(at);
        // A check's blocks go inside it; anything else, right after it.
        if (kind === 'if'){ this.target = this.sel.concat('then'); this.after = null; }
        else this.after = this.sel;
        this.changed();
        if (this.narrow()) this.showInspector();
        else this.reveal('.ig-step.on');
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
        walk([list[index + 1]], s => nameAnswer(this.handler(), s));
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
      case 'b-del-handler': {
        if (d.handlers.length < 2 || !armConfirm(el, 'Click again: removes it and its steps')) return true;
        const at = this.h, label = this.triggerTabs()[at];
        const [gone] = d.handlers.splice(at, 1);
        this.h = 0; this.sel = 'trigger'; this.target = []; this.after = null; this.resetTest();
        this.choosing = this.unpicked.has(this.handler()) ? 'kind' : null;
        this.changed();
        undoToast(`Removed the trigger "${label}"`, () => {
          if (Modal.kind !== 'builder' || this.d !== d) return;
          d.handlers.splice(Math.min(at, d.handlers.length), 0, gone);
          this.h = d.handlers.indexOf(gone);
          this.sel = 'trigger'; this.target = []; this.after = null; this.resetTest();
          this.choosing = this.unpicked.has(gone) ? 'kind' : null;
          this.changed();
        });
        return true;
      }
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
      case 'b-name-answer': { const s = selected(); if (s) nameAnswer(this.handler(), s); this.changed(); return true; }
      case 'b-token': {
        // Into the field last written in, or the selected step's text.
        const s = selected(), main = s && (KINDS[s.type] || {}).text;
        let field = this.lastField && this.lastField.isConnected ? this.lastField : null;
        if (!field && main){
          // Never written in: the value goes at the end, not before it all.
          field = q(`.ig-inspector [data-bind="p"][data-name="${main}"]`);
          if (field) field.setSelectionRange(field.value.length, field.value.length);
        }
        if (!field){ toast('Click a text field first, then the value to put in it', 'info'); return true; }
        insertAtCursor(field, tokenText(el.dataset.token, s && s.type === 'if'));
        return true;
      }
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
      case 'delete': busy(el, () => remove(d.id)); return true;
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
  // Stacked (a phone, a narrow window), the blocks, the steps and the
  // settings are one page; see index.html's max-width:760px.
  narrow(){ return matchMedia('(max-width:760px)').matches; },
  // There, the settings sit under every step: bring them up when a step
  // is picked or added. "Back to steps" goes the other way.
  showInspector(){
    if (!this.narrow()) return;
    const insp = q('.ig-inspector');
    if (insp) insp.scrollIntoView({block:'start', behavior:reduced() ? 'auto' : 'smooth'});
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
    case 'chat_command': return {type, command:'!command', aliases:[], roles:{everyone:true, subs:true, vips:true, mods:true}, user_cooldown_ms:30000, cooldown_reply:''};
    case 'block': return {type, name:''};
    case 'chat_message': return {type, pattern:'', mode:'word', roles:{everyone:true, subs:true, vips:true, mods:true}};
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
    obs:{action:'scene', scene:'', source:'', text:'', filter:'', hotkey:''},
    light:{device:'', name:'', plug:'', action:'color', color:'red', brightness:''},
    viewer:{value:'coins', who:'', op:'', by:'10'}, math:{a:'', op:'multiply', b:'2', save_as:'result'},
    wait_chat:{text:'', from:'', seconds:'30', save_as:'reply'}, call:{block:''}, reward:{title:'', action:'off'},
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
    case 'chat_message': return `A chat message ${{exact:'is', starts_with:'starts with', word:'has the word', contains:'contains the letters'}[t.mode] || 'matches'} "${t.pattern || ''}"`;
    case 'block': return t.name ? `A "Run a block" step runs ${t.name}` : 'A block of steps (not named yet)';
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
// Dragging: a block from the list on the left into the steps, or a step
// to another place, in or out of an If's lanes. Mouse and pen only: on a
// touch screen the canvas scrolls, and a tap adds or picks as before.
const StepDrag = {
  start:null,   // where a press began, until it moves far enough to drag
  live:null,    // the drag under way
  down(e){
    if (Modal.kind !== 'builder' || e.button !== 0 || e.pointerType === 'touch' || this.live) return;
    const main = e.target.closest('.ig-step-main');
    const block = !main && e.target.closest('.ig-palette button[data-act="b-add"]:not(.ig-pal-locked)');
    if (!main && !block) return;
    const card = main ? main.closest('.ig-step') : block;
    if (main && !card.dataset.path) return;
    this.start = {x:e.clientX, y:e.clientY, id:e.pointerId, card,
      kind:block ? block.dataset.kind : '', path:main ? card.dataset.path : null};
  },
  move(e){
    if (this.live){ this.live.x = e.clientX; this.live.y = e.clientY; return; }
    const s = this.start;
    if (!s || e.pointerId !== s.id) return;
    if (Math.hypot(e.clientX - s.x, e.clientY - s.y) >= 6) this.begin(e);
  },
  // A small tilted pill under the pointer: the block's icon, its kind and
  // what it does, and how many steps an If carries along.
  ghostHtml(s){
    if (s.kind){
      const k = KINDS[s.kind];
      return {c:k.c, html:`<span class="ig-pal-ic">${svg(k.icon)}</span><span class="ig-ghost-t"><small>New step</small><b>${esc(k.label)}</b></span>`};
    }
    const step = Builder.stepAt(Builder.resolve(s.path)), k = KINDS[step.type] || {label:step.type, c:'#888', icon:'steps'};
    const text = (s.card.querySelector('.ig-step-text') || {}).textContent || '';
    let inside = -1;
    walk([step], () => { inside++; });
    return {c:k.c, html:`<span class="ig-pal-ic">${svg(k.icon || 'steps')}</span><span class="ig-ghost-t"><small>${esc(k.label)}</small><b>${esc(text)}</b></span>${inside > 0 ? `<span class="ig-ghost-n">+${inside}</span>` : ''}`};
  },
  // The block leaves its place: a copy of it shrinks into the pointer
  // while the pill grows there.
  lift(s, x, y){
    if (reduced()) return;
    const src = s.kind ? s.card.querySelector('.ig-pal-ic') : s.card;
    if (!src) return;
    const r = src.getBoundingClientRect(), copy = src.cloneNode(true);
    ['data-act', 'data-key', 'data-path', 'id'].forEach(a => copy.removeAttribute(a));
    copy.querySelectorAll('[data-act],[data-key],[id]').forEach(n => ['data-act', 'data-key', 'id'].forEach(a => n.removeAttribute(a)));
    copy.classList.remove('on', 'ig-in-hand');
    copy.setAttribute('aria-hidden', 'true');
    if (s.kind) copy.style.setProperty('--c', KINDS[s.kind].c);
    Object.assign(copy.style, {position:'fixed', left:r.left + 'px', top:r.top + 'px', width:r.width + 'px', height:r.height + 'px',
      margin:'0', zIndex:'2998', pointerEvents:'none', transformOrigin:'0 0'});
    document.body.appendChild(copy);
    const k = s.kind ? 0.6 : 0.22;
    copy.animate([{transform:'none', opacity:1},
      {transform:`translate(${x - r.left - r.width * k / 2}px,${y - r.top - r.height * k / 2}px) scale(${k})`, opacity:0}],
      {duration:260, easing:'cubic-bezier(.4,0,.2,1)', fill:'forwards'}).onfinish = () => copy.remove();
  },
  begin(e){
    const s = this.start, look = this.ghostHtml(s);
    const ghost = document.createElement('div');
    ghost.className = 'ig-ghost';
    ghost.style.setProperty('--c', look.c);
    ghost.setAttribute('aria-hidden', 'true');
    ghost.innerHTML = `<div class="ig-ghost-in">${look.html}</div>`;
    document.body.append(ghost);
    const getSel = window.getSelection && window.getSelection();
    if (getSel) getSel.removeAllRanges();
    document.documentElement.classList.add('ig-dragging-on');
    // The socket is as tall as the block in hand.
    const h = s.kind ? 56 : Math.round(s.card.getBoundingClientRect().height);
    // The pointer holds the pill by its icon.
    this.live = {ghost, x:e.clientX, y:e.clientY, dx:18, dy:20, drop:null, raf:0, slot:null, slotAt:'', h};
    this.lift(s, e.clientX, e.clientY);
    // The step is in hand: out of the stack, its socket where it was.
    this.shift(() => {
      this.held().forEach(n => n.classList.add('ig-in-hand'));
      this.live.drop = this.place(e.clientX, e.clientY);
      this.setSlot(this.live.drop, true);
    });
    this.frame();
  },
  // The step being carried, with its lanes and its "next block" marker.
  held(){
    const s = this.start;
    const card = s && s.path != null && q(`.ig-step[data-path="${s.path}"]`);
    if (!card) return [];
    const out = [card];
    for (let n = card.nextElementSibling; n && !n.classList.contains('ig-step') && !n.matches('.ig-drop:not(.after)'); n = n.nextElementSibling){
      if (n.matches('.ig-branch, .ig-drop.after')) out.push(n);
    }
    return out;
  },
  // Each frame: follow the pointer, scroll near an edge, find the drop.
  frame(){
    const L = this.live;
    if (!L) return;
    L.ghost.style.transform = `translate(${L.x - L.dx}px,${L.y - L.dy}px)`;
    const canvas = q('.ig-canvas');
    if (canvas){
      const r = canvas.getBoundingClientRect(), edge = 56;
      const over = L.x >= r.left && L.x <= r.right;
      const push = L.y < r.top + edge ? L.y - (r.top + edge) : L.y > r.bottom - edge ? L.y - (r.bottom - edge) : 0;
      if (over && push) canvas.scrollTop += Math.max(-16, Math.min(16, push / 3));
    }
    // A redraw may have put the step in hand back: take it out again.
    this.held().forEach(n => n.classList.add('ig-in-hand'));
    L.drop = this.place(L.x, L.y);
    L.ghost.classList.toggle('no', !!L.drop && L.drop.ok === false);
    const key = L.drop && L.drop.ok ? `${L.drop.list}:${L.drop.at}` : '';
    if (key !== L.slotAt || (L.slot && !L.slot.isConnected)) this.shift(() => this.setSlot(L.drop, false));
    L.raf = requestAnimationFrame(() => this.frame());
  },
  // Where an element sits in the layout, ignoring the slides under way,
  // so a sliding step never moves the drop it slides for.
  layoutTop(el, root){
    let y = 0;
    for (let n = el; n && n !== root; n = n.offsetParent) y += n.offsetTop;
    return root.getBoundingClientRect().top + y;
  },
  // The list under the pointer (the deepest lane) and the place in it,
  // counted without the step in hand.
  place(x, y){
    const canvas = q('.ig-canvas'), root = q('.ig-steps');
    if (!canvas || !root) return null;
    const box = canvas.getBoundingClientRect();
    if (x < box.left || x > box.right || y < box.top || y > box.bottom) return null;
    const hit = document.elementFromPoint(x, y);
    const holder = hit && canvas.contains(hit) ? hit.closest('.ig-lane, .ig-steps') : null;
    const listEl = holder ? (holder.matches('.ig-steps') ? holder : holder.querySelector(':scope > [data-list]')) : root;
    if (!listEl) return null;
    const cards = [...listEl.querySelectorAll(':scope > .ig-step:not(.ig-when):not(.ig-in-hand)')];
    // A step and its lanes are one block: past its middle is after it.
    const mids = cards.map(c => {
      const next = c.nextElementSibling, top = this.layoutTop(c, root);
      const bottom = next && next.classList.contains('ig-branch') ? this.layoutTop(next, root) + next.offsetHeight : top + c.offsetHeight;
      return (top + bottom) / 2;
    });
    let at = mids.findIndex(mid => y < mid);
    if (at < 0) at = cards.length;
    return {list:listEl.dataset.list, at, el:listEl, cards, ok:this.allowed(listEl.dataset.list, at)};
  },
  // true, false (not here), or 'same' (back where it came from).
  allowed(listKey, at){
    const s = this.start;
    const levels = s.kind ? (s.kind === 'if' ? 1 : 0) : nestLevels(Builder.stepAt(Builder.resolve(s.path)));
    if (!nestFits(Builder.resolve(listKey), levels)) return false;
    if (s.path == null) return true;
    // Never into its own lanes.
    if (listKey === s.path || listKey.startsWith(s.path + '.')) return false;
    const from = Builder.resolve(s.path);
    return pathKey(from.slice(0, -1)) === listKey && at === from[from.length - 1] ? 'same' : true;
  },
  // The socket: a gap the size of the block, where it would land.
  setSlot(drop, still){
    const L = this.live;
    if (L.slot) L.slot.remove();
    L.slot = null;
    L.slotAt = '';
    if (!drop || !drop.ok) return;
    const slot = document.createElement('div');
    slot.className = 'ig-slot' + (still ? ' still' : '');
    slot.setAttribute('aria-hidden', 'true');
    slot.style.setProperty('--h', L.h + 'px');
    const end = [...drop.el.children].reverse().find(c => c.matches('.ig-drop:not(.after)')) || null;
    drop.el.insertBefore(slot, drop.at < drop.cards.length ? drop.cards[drop.at] : end);
    L.slot = slot;
    L.slotAt = `${drop.list}:${drop.at}`;
  },
  // Runs `change` (the socket moves, the step leaves or comes back), then
  // slides what it pushed aside from where it was seen to where it is.
  // Steps inside lanes move with their lanes, so only their own part.
  shift(change){
    const canvas = q('.ig-canvas');
    if (!canvas || reduced()){ change(); return; }
    const els = [...canvas.querySelectorAll('.ig-step, .ig-branch, .ig-drop')];
    const seen = new Map(els.map(el => [el, el.getBoundingClientRect().top]));
    change();
    els.forEach(el => el.getAnimations().forEach(a => { if (a.id === 'shift') a.cancel(); }));
    const moved = new Map();
    els.forEach(el => { if (el.isConnected && !el.classList.contains('ig-in-hand')) moved.set(el, seen.get(el) - el.getBoundingClientRect().top); });
    moved.forEach((dy, el) => {
      const lane = el.parentElement && el.parentElement.closest('.ig-branch');
      const own = dy - (lane && moved.has(lane) ? moved.get(lane) : 0);
      if (Math.abs(own) >= 1) el.animate([{transform:`translateY(${own}px)`}, {transform:'none'}], {id:'shift', duration:220, easing:'cubic-bezier(.2,.9,.3,1)'});
    });
  },
  up(e){
    const s = this.start, L = this.live;
    this.start = null;
    if (!L || (s && e.pointerId !== s.id)) return;
    const d = L.drop, pill = L.ghost.firstElementChild.getBoundingClientRect();
    const ghost = this.stop();
    // The click a drag ends with is not a click.
    const eat = ev => { ev.stopPropagation(); ev.preventDefault(); };
    window.addEventListener('click', eat, true);
    setTimeout(() => window.removeEventListener('click', eat, true), 0);
    // The redraw puts the step where its socket is: nothing jumps.
    if (d && d.ok === true && Builder.dropAt(s, d.list, d.at)){
      ghost.remove();
      this.clear(L, false);
      this.land(pill, Array.isArray(Builder.sel) && q(`.ig-step[data-path="${pathKey(Builder.sel)}"]`));
      return;
    }
    this.clear(L, true);
    if (d && d.ok === 'same'){
      ghost.remove();
      this.land(pill, q(`.ig-step[data-path="${s.path}"]`));
    } else this.home(ghost, s);
  },
  cancel(){
    const s = this.start, L = this.live;
    this.start = null;
    if (!L) return;
    const ghost = this.stop();
    this.clear(L, true);
    this.home(ghost, s);
  },
  // Ends the drag; the pill is handed back, still on screen.
  stop(){
    const L = this.live;
    this.live = null;
    cancelAnimationFrame(L.raf);
    document.documentElement.classList.remove('ig-dragging-on');
    return L.ghost;
  },
  // Takes the socket away and puts the step in hand back, sliding the rest
  // into place when `slide`.
  clear(L, slide){
    const undo = () => {
      if (L.slot) L.slot.remove();
      document.querySelectorAll('.ig-in-hand').forEach(x => x.classList.remove('ig-in-hand'));
    };
    if (slide) this.shift(undo); else undo();
  },
  // Dropped: the step falls from where the pill was into its place and
  // settles; what an If carries follows it in, one by one.
  land(from, card){
    if (reduced() || !card) return;
    const to = card.getBoundingClientRect(), k = Math.min(1, from.width / to.width);
    card.getAnimations().forEach(a => a.cancel());
    card.animate([
      {transformOrigin:'0 0', transform:`translate(${from.left - to.left}px,${from.top - to.top}px) scale(${k}) rotate(-2.5deg)`, opacity:.7},
      {transformOrigin:'0 0', transform:'translateY(2px) scale(1.01)', opacity:1, offset:.7},
      {transformOrigin:'0 0', transform:'none', opacity:1},
    ], {duration:440, easing:'cubic-bezier(.2,.8,.2,1)'});
    card.animate([{boxShadow:'0 0 0 0 color-mix(in oklch,var(--accent) 50%,transparent)'}, {boxShadow:'0 0 0 12px transparent'}],
      {duration:560, delay:280, easing:'ease-out'});
    const branch = card.nextElementSibling;
    if (!branch || !branch.classList.contains('ig-branch')) return;
    branch.getAnimations().forEach(a => a.cancel());
    branch.animate([{opacity:0, transform:'translateY(-8px)'}, {opacity:1, transform:'none'}],
      {duration:300, delay:200, easing:'cubic-bezier(.2,.8,.2,1)', fill:'backwards'});
    [...branch.querySelectorAll('.ig-step')].slice(0, 16).forEach((c, i) => {
      c.getAnimations().forEach(a => a.cancel());
      c.animate([{opacity:0, transform:'translateY(-12px) scale(.96)'}, {opacity:1, transform:'none'}],
        {duration:300, delay:260 + i * 55, easing:'cubic-bezier(.2,.9,.3,1.15)', fill:'backwards'});
    });
  },
  // Not dropped: the pill flies back to where the block came from.
  home(ghost, s){
    const card = s && (s.path != null ? q(`.ig-step[data-path="${s.path}"]`) : s.card);
    if (reduced() || !card || !card.isConnected){ ghost.remove(); return; }
    const p = ghost.firstElementChild.getBoundingClientRect(), r = card.getBoundingClientRect();
    ghost.animate([{opacity:1}, {transform:`translate(${r.left + 12}px,${r.top + (r.height - p.height) / 2}px)`, opacity:0}],
      {duration:240, easing:'cubic-bezier(.4,0,.2,1)', fill:'forwards'}).onfinish = () => ghost.remove();
  },
};
document.addEventListener('pointerdown', e => StepDrag.down(e));
document.addEventListener('pointermove', e => StepDrag.move(e));
document.addEventListener('pointerup', e => StepDrag.up(e));
document.addEventListener('pointercancel', () => StepDrag.cancel());
window.addEventListener('blur', () => StepDrag.cancel());
// Escape drops the drag, not the builder.
window.addEventListener('keydown', e => {
  if (e.key !== 'Escape' || !StepDrag.live) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  StepDrag.cancel();
}, true);

// A step's sentence on its card, under its kind: "OBS" already says what
// "OBS: mute…" would.
function cardSentence(s){
  const text = stepSentence(s);
  return s.type === 'obs' ? text.replace(/^OBS: ([a-z])/, (all, c) => c.toUpperCase()) : text;
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
    case 'if': {
      const one = r => `${highlightVars(r.left || '…')} ${esc(OPS[r.op] || r.op || '')}${OPS_ALONE.includes(r.op) ? '' : ' ' + highlightVars(r.right || '')}`;
      return 'If ' + conditionRows(s).map(one).join(p.match === 'any' ? ' or ' : ' and ');
    }
    case 'stop': return p.quiet === 'yes' ? 'Stop here, out of the history' : 'Stop here';
    case 'delay_action': return p.action === 'arm' && p.seconds && Number(p.seconds) <= 0 ? 'Turn the delay off'
      : p.action === 'arm' && p.seconds ? `Set a ${highlightVars(p.seconds)} s delay` : esc(DELAY_ACTIONS[p.action] || 'Delay action');
    case 'marker': return `VOD marker ${q(p.description)}`;
    case 'clip': return 'Create a clip';
    case 'program': return `Run ${p.path ? esc(p.path.split(/[\\/]/).pop()) : 'a program you pick'} ${highlightVars(p.args || '')}${p.wait === 'yes' ? ', and keep its answer' : ''}`;
    case 'file': return `${p.mode === 'append' ? 'Add to' : 'Write'} ${p.path ? esc(p.path.split(/[\\/]/).pop()) : 'a file you pick'} ${q(p.text)}`;
    case 'set_var': return `${p.keep === 'yes' ? 'Keep' : 'Remember'} ${esc(p.name || '…')} = ${highlightVars(p.value || '')}`;
    case 'viewer': {
      const whose = {target:'who they named', active:'everyone who chatted', named:highlightVars(p.login || '…')}[p.who] || 'the viewer';
      const amount = highlightVars(p.by || '1'), value = esc(p.value || '…');
      if (p.who === 'active') return `Give ${amount} ${value} to ${whose}`;
      return {spend:`Spend ${amount} ${value} of ${whose}, if they have it`, subtract:`Take ${amount} ${value} from ${whose}`,
        set:`Set ${value} of ${whose} to ${amount}`, reset:`Reset ${value} of ${whose}`}[p.op] || `Give ${amount} ${value} to ${whose}`;
    }
    case 'math': return `${highlightVars(p.a || '0')} ${esc((MATH_OPS[p.op || ''] || '').toLowerCase())} ${highlightVars(p.b || '0')} → <span class="v">{${esc((p.save_as || '').trim() || 'result')}}</span>`;
    case 'wait_chat': return `Wait up to ${highlightVars(p.seconds || '30')} s for ${p.text ? q(p.text) : 'a message'} from ${{viewer:'the viewer', target:'who they named'}[p.from] || 'anyone'}`;
    case 'call': return `Run the block ${esc(p.block || '…')}`;
    case 'reward': return `Switch the reward ${q(p.title || '…')} ${p.action === 'off' ? 'off' : 'on'}`;
    case 'overlay': return `On stream ${q(p.text)}`;
    case 'edit_text': return `${esc((EDIT_OPS[p.op] || EDIT_OPS.first_line)[0])}: ${highlightVars(p.input || '…')} → <span class="v">{${esc((p.save_as || '').trim() || 'text')}}</span>`;
    case 'counter': return `Counter ${esc(p.name || '…')} ${p.op === 'reset' ? 'reset' : p.op === 'set' ? '= ' + esc(p.by || '0') : (p.op === 'subtract' ? '-' : '+') + esc(p.by || '1')}`;
    case 'light': return `${esc(p.name || 'Light')}: ${esc(lightSummary(p).toLowerCase())}`;
    case 'obs': {
      const what = (OBS_ACTIONS[p.action] || ['OBS'])[0];
      if (p.action === 'scene') return `OBS: switch to ${q(p.scene)}`;
      if (p.action === 'text') return `OBS: ${esc(p.source || '(source)')} says ${q(p.text)}`;
      const sources = listedNames(p.source).map(x => q(x)).join(', ');
      if (p.action === 'check') return `OBS: check ${sources || "what it's doing"} → ${esc(answerName(s))}`;
      if (p.action === 'hotkey') return `OBS: press ${esc(p.hotkey ? hotkeyLabel(p.hotkey) : '(hotkey)')}`;
      if (/^filter_/.test(p.action)) return `OBS: ${esc(what.replace(/ a filter/, '').toLowerCase())} ${q(p.filter)} on ${esc(p.source || '(source)')}`;
      const need = (OBS_ACTIONS[p.action] || [])[1];
      if (!need) return `OBS: ${esc(what.toLowerCase())}`;
      return `OBS: ${esc(what.replace(/ a (media )?source.*$/, '').toLowerCase())} ${sources || q('')}${p.scene && ['show', 'hide', 'toggle'].includes(p.action) ? ' in ' + esc(p.scene) : ''}`;
    }
  }
  return esc(s.type);
}

// ---------------------------------------------------------------- connections

// Where integrations post and listen: one card each, with how it stands.
// [id, icon, name, what it's for].
const CONNECTIONS = [
  ['discord', 'bubble', 'Discord', 'Messages and live-updating cards in your server.'],
  ['twitch', 'tv', 'Twitch', 'Chat, VOD markers, clips, and follows, subs and raids.'],
  ['phone', 'phone', 'Phone', 'Pushes to your phone, through the free ntfy app.'],
  ['obs', 'rec', 'OBS', 'Scenes, sources and recording, through OBS\'s WebSocket.'],
  ['stream', 'screen', 'Alerts page', 'Where alerts show up on stream, and how they look.'],
];

// How a connection stands: [pill class, words]. The strip and the cards
// say the same.
function connState(id){
  const c = S.data.connections, t = S.data.twitch;
  switch (id){
    case 'discord': return c.discord.length ? ['ok', plural(c.discord.length, 'channel')] : ['', 'Add'];
    case 'twitch':
      if (!t.main.login) return ['', t.available ? 'Connect' : 'Set up'];
      return t.main.chat === 'connected' ? ['ok', 'connected'] : ['warn', 'chat ' + t.main.chat];
    case 'phone': return c.phone.topic ? ['ok', 'connected'] : ['', 'Connect'];
    // The open dashboard keeps OBS connected. Out of reach only warns when
    // an integration needs it: otherwise it just has to be set up.
    case 'obs': {
      const o = S.data.obs || {};
      if (o.state === 'connected') return ['ok', 'connected'];
      if (o.state === 'error') return o.used ? ['warn', 'can\'t reach'] : ['', 'Set up'];
      return ['', 'connecting…'];
    }
    case 'stream': return alertsPill();
  }
  return ['', ''];
}

const Conn = {
  tab:'discord', editing:null,
  // `tab`: the card to open (none: all closed).
  open(tab){
    // Opened from an editor or an import: go back to it, work intact.
    const from = {editor:Editor, builder:Builder, import:Recipes}[Modal.kind] || null;
    this.tab = tab || null;
    this.editing = null;
    Modal.open('connections', 760);
    Modal.onClose = () => { if (S.data.twitch.login) api('/twitch/cancel', {}); return true; };
    if (from) Modal.after = () => from.resume();
    this.render();
  },
  resume(){ this.open(this.tab); },
  render(){
    // A slow reply can land after the user left this page.
    if (Modal.kind !== 'connections') return;
    const panes = {discord:() => this.discordHtml(), twitch:() => this.twitchHtml(), phone:() => this.phoneHtml(),
      obs:() => this.obsHtml(), stream:() => this.alertsHtml()};
    const cards = CONNECTIONS.map(([id, icon, name, blurb]) => {
      const open = this.tab === id, [cls, words] = connState(id);
      return `<section class="ig-cx${open ? ' open' : ''}" data-key="cx-${id}">
        <button class="ig-cx-head" data-act="conn-tab" data-tab="${id}" aria-expanded="${open}">
          <span class="dcard-icon ig-ic-${id}">${svg(icon)}</span>
          <span class="ig-cx-id"><b>${name}</b><span>${blurb}</span></span>
          <span class="ic-pill ${cls}"><span class="dot"></span><span class="ic-pill-v">${esc(words)}</span></span>
          <span class="ig-cx-chev">${svg('chev')}</span>
        </button>
        <div class="ig-cx-body" data-flip data-autoh>${open ? `<div class="ig-cx-in" data-key="cxb-${id}">${panes[id]()}</div>` : ''}</div>
      </section>`;
    }).join('');
    Modal.body(`${modalHead('link', 'var(--accent)', 'Connections', 'Where integrations post and what they listen to. Set each one up once.')}
      <div class="dest-form-body ig-conn-body"><div class="ig-cxs">${cards}</div></div>`);
    if (this.tab === 'obs') ObsLive.watch();
  },
  // The alerts page: in OBS or not, its look in a line, and the designer.
  alertsHtml(){
    const a = S.data.alerts || {}, look = lookOf(), own = THEMES[themeOf(look)];
    const url = a.url || `${location.origin}/alerts`;
    const font = look.font ? (look.font === 'system' ? 'this PC\'s font' : look.font) : own.font;
    return `<div class="sys-section ig-form ig-setup">${alertsStatusHtml(false)}
      <div class="ig-look-sum"><span class="ig-look-dot" style="--sw:${alertColor('', look)}"></span>
        <span class="ig-look-sum-t"><b>${esc(own.name)}</b> · ${esc(font)} · ${esc(soundLabel(look.sound || own.sound))}${look.css ? ' · custom CSS' : ''}</span>
        <button class="ic-btn ic-btn-primary ig-small" data-act="alerts-open">Change the look</button></div>
      <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="alert-test">${svg('play', ' class="ig-btn-ic"')}Show a test alert</button>
      <details class="ig-more-fields"><summary>Add it to OBS by hand</summary>
        <div class="ig-insp-p">In OBS: <b>+</b> under Sources › <b>Browser</b>, paste this address, and make it as big as your canvas. Tick <b>Control audio via OBS</b> so alert sounds reach the stream.</div>
        <div class="ig-copy-row"><input class="ic-input mono" readonly value="${esc(url)}" aria-label="Alerts address">
          <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(url)}">Copy</button></div></details>
    </div>`;
  },
  // OBS's WebSocket: how it is connected, with Try again, and what it's
  // for. Looking here connects it for a minute, like picking a scene.
  obsHtml(){
    const users = S.data.integrations.filter(i => i.enabled && usesObs(i)).map(i => i.name);
    return `<div class="sys-section ig-form" data-obs-live>${obsStatusHtml()}
      <div class="ig-insp-p">Scene triggers and OBS steps talk to OBS through its WebSocket server, built into OBS 28 and later. Switch it on once in OBS: <b>Tools › WebSocket Server Settings › Enable WebSocket server</b>, then OK. InstantClone reads the port and password from OBS itself.</div>
      <div class="muted">${users.length ? `Kept connected for ${esc(users.slice(0, 3).join(', '))}${users.length > 3 ? ` and ${users.length - 3} more` : ''}.`
        : 'Nothing uses OBS yet, so InstantClone only stays connected while the dashboard is open.'}</div></div>`;
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
      + this.accountHtml('main', t.main, 'Your Twitch account', 'Log in once: InstantClone keeps it fresh.')
      + this.twitchEventsHtml(t)
      + this.accountHtml('bot', t.bot, 'Bot account (optional)', 'A second account that posts in chat instead of you.')
      + `<label class="crash-check"><input type="checkbox" data-act="t-shared" ${S.data.twitch_shared_chat ? 'checked' : ''}>
        <span><span class="crash-check-title">Shared Chat: listen to partner channels too</span>
        <span class="muted">When you share chat with other streamers, their viewers can use your commands and count toward "chat gets busy". Off: only your own chat does.</span></span></label>`
      + '<div class="muted">No password ever goes through InstantClone: you approve it on twitch.tv, and can revoke it there any time.'
      + (t.app === 'custom' ? ' Logging in through your own Twitch app (<a href="#" data-act="twitch-app">change</a>).' : '') + '</div>';
  },
  // What Twitch events integrations hear, and an Allow when the login
  // predates them. Logging in again asks Twitch for the new permissions.
  twitchEventsHtml(t){
    const ev = t.events || {}, subs = ev.subs || {};
    if (!t.main.login || !Object.keys(subs).length) return '';
    const hearing = twitchEventsIn('listening'), consent = twitchEventsIn('needs_consent');
    const flow = S.data.twitch.login, logging = flow && flow.which === 'main';
    return `<div class="sys-section ig-form" data-key="t-events">
      <div class="ic-label">Twitch events</div>
      ${hearing.length ? `<div class="ig-meter"><span class="ig-meter-dot ok"></span><span>Listening for ${esc(twitchNouns(hearing))}</span></div>` : ''}
      ${ev.state === 'error' ? `<div class="ig-meter"><span class="ig-meter-dot off"></span><span>${esc(ev.problem)}</span></div>` : ''}
      ${consent.length ? `<div class="lan-warn ig-notice"><span class="lw-ic">⚠</span>
        <span class="ig-notice-text">Twitch needs your OK for <b>${esc(twitchNouns(consent))}</b>. Your login is from before these events: Allow logs you in again with the same account, asking Twitch only to read them.</span>
        ${logging ? '' : '<button class="ic-btn ic-btn-primary ig-small" data-act="t-login" data-which="main">Allow on Twitch</button>'}</div>` : ''}
    </div>`;
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
      case 'conn-tab':
        this.tab = this.tab === el.dataset.tab ? null : el.dataset.tab;
        this.editing = null;
        this.render();
        if (this.tab) revealOpened(q(`[data-key="cx-${this.tab}"]`));
        return true;
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

// ---------------------------------------------------------------- alerts page

// The alerts browser source: where "Show on stream" cards appear. How
// every card looks is the page's theme (S.data.alerts.look, edited in
// AlertsLook); each alert can bring its own accent, sound and picture.
// Every preview here is the real page (/alerts?demo) drawing a sample, so
// what you see is what OBS shows.

// Each theme as web/alerts.html draws it: its name, colors, opacity (%),
// corners (px), font, sound, its own entrance and where a picture goes.
const THEMES = {
  island:{name:'Island', fg:'#ffffff', bg:'#000000', accent:'#5ac8fa', op:96, r:40, font:'Inter', sound:'chime', motion:'Grows from a dot', pic:'left'},
  signal:{name:'Signal', fg:'#eef4ff', bg:'#0b0f14', accent:'#3ee0c5', op:86, r:0, font:'Chakra Petch', sound:'zap', motion:'Decodes', pic:'left'},
  banner:{name:'Banner', fg:'#0b0d12', bg:'#fcd34d', accent:'#fcd34d', op:100, r:0, font:'Bebas Neue', sound:'fanfare', motion:'Wipes on', pic:'left'},
  bubble:{name:'Bubble', fg:'#1b1d24', bg:'#ffffff', accent:'#a78bfa', op:100, r:30, font:'Fredoka', sound:'pop', motion:'Boings out', pic:'left'},
  minimal:{name:'Minimal', fg:'#ffffff', bg:'#000000', accent:'#5ac8fa', op:0, r:0, font:'Inter', sound:'blip', motion:'Inks in', pic:'left'},
  paper:{name:'Paper', fg:'#1d1a16', bg:'#f4efe4', accent:'#e2553d', op:100, r:3, font:'Permanent Marker', sound:'whoosh', motion:'Slaps on', pic:'left'},
  cinema:{name:'Cinema', fg:'#f4ede1', bg:'#0a0908', accent:'#d4b06a', op:90, r:2, font:'Playfair Display', sound:'bell', motion:'Opens like a shutter', pic:'top'},
};
// alerts::FONTS, ANIMATIONS and SOUNDS.
const ALERT_FONTS = ['Inter', 'Poppins', 'Montserrat', 'Fredoka', 'Rubik', 'Bebas Neue', 'Oswald', 'Bangers', 'Orbitron',
  'Press Start 2P', 'Permanent Marker', 'Roboto Mono', 'Chakra Petch', 'Playfair Display', 'Special Elite'];
const ALERT_ANIMS = [['slide', 'Slide'], ['pop', 'Pop'], ['fade', 'Fade'], ['zoom', 'Zoom'], ['bounce', 'Bounce'], ['drop', 'Drop']];
const ALERT_SOUNDS = [['chime', 'Chime'], ['pop', 'Pop'], ['bell', 'Bell'], ['rise', 'Rise'], ['coin', 'Coin'],
  ['sparkle', 'Sparkle'], ['whoosh', 'Whoosh'], ['fanfare', 'Fanfare'], ['zap', 'Zap'], ['blip', 'Blip']];
const ALERT_SECONDS = ['3', '6', '10', '15', '30'];
// What the theme designer's "Back to ... as it comes" clears.
const LOOK_STYLE = ['font', 'text', 'background', 'accent', 'opacity', 'radius', 'animation', 'picture'];
// A link the alerts page can load as it is (alerts::is_web_link), and an
// upload (alert_media's `media:<name>`).
const WEB_LINK = /^https?:\/\/[^\s"'<>\\`{}]+$/;
const MEDIA_REF = /^media:[a-z0-9._-]+$/;

function lookOf(){ return (S.data.alerts || {}).look || {}; }
function themeOf(look){ return THEMES[look.theme] ? look.theme : 'island'; }
function mediaSrc(ref){ return MEDIA_REF.test(ref || '') ? '/alerts/media/' + ref.slice(6) : WEB_LINK.test(ref || '') ? ref : ''; }
function mediaOf(kind){ return ((S.data.alerts || {}).media || []).filter(m => m.kind === kind); }

function soundLabel(sound){
  if (!sound) return 'none';
  if (sound === 'theme') return 'theme sound';
  const found = ALERT_SOUNDS.find(([id]) => id === sound);
  if (found) return found[1].toLowerCase();
  return MEDIA_REF.test(sound) ? sound.slice(6) : 'a link';
}
// The accent an alert shows: its own, the page's, or the theme's.
function alertColor(value, look){
  if (/^#[0-9a-f]{6}$/i.test(value || '')) return value.toLowerCase();
  return /^#[0-9a-f]{6}$/i.test(look.accent || '') ? look.accent : THEMES[themeOf(look)].accent;
}
function alertColorName(value){
  if (!value) return 'theme\'s';
  const found = CARD_COLORS.find(([c]) => c === value.toLowerCase());
  return found ? found[1].toLowerCase() : value.toLowerCase();
}

// The Connections pill: [class, words].
function alertsPill(){
  const a = S.data.alerts || {};
  if (a.in_obs) return ['ok', 'in OBS'];
  if (a.elsewhere) return ['warn', 'not in OBS'];
  return ['', 'Add to OBS'];
}

// Is the alerts page in OBS, with one click to put it there. `compact`
// is the editor's line under an alert's preview.
function alertsStatusHtml(compact){
  const a = S.data.alerts || {};
  const inObs = a.in_obs > 0;
  const line = inObs ? 'Your alerts page is open in OBS.'
    : a.elsewhere ? 'Your alerts page is open in a browser, but not in OBS: viewers won\'t see alerts.'
    : 'Your alerts page isn\'t in OBS yet: viewers won\'t see alerts.';
  const action = !inObs
    ? `<button class="ic-btn ic-btn-primary ig-small" data-act="alerts-add">${svg('plus', ' class="ig-btn-ic"')}Add to OBS</button>`
    : compact ? '' : '<button class="ic-btn ic-btn-ghost ig-small" data-act="alerts-add">Add to the scene on air</button>';
  return `<div class="ig-alerts-status${inObs ? ' ok' : ''}" data-key="alerts-status">
    <span class="ig-meter-dot ${inObs ? 'ok' : 'off'}"></span><span class="ig-alerts-line">${line}</span>${action}</div>`;
}

// The first alert set up, with sample values: what previews show.
function sampleAlert(){
  for (const i of S.data.integrations){
    for (const h of i.handlers || []){
      const step = (h.steps || []).find(s => s.type === 'overlay' && s.enabled !== false);
      if (step) return alertCardOf(step, h);
    }
  }
  return {title:'New follower', text:'Nat', sound:'theme', image:''};
}

// An alert step as the page shows it, filled in with sample values.
function alertCardOf(step, h){
  const p = step.params, vars = sampleMap(h);
  return {title:renderSample(p.title || '', vars), text:renderSample(p.text || '', vars), seconds:p.seconds || '6',
    color:p.color || '', sound:p.sound || '', volume:p.volume || '', image:p.image || ''};
}

// Fit a real alerts page (`/alerts?demo`, laid out `canvas` px wide) into
// its box and show it `payload` ({look, card, replay, sound}); a frame
// still loading gets it once it has. Kept as properties, not attributes:
// a redraw resets attributes, and calls this again.
function stageShow(frame, canvas, payload){
  if (!frame) return;
  frame.icCanvas = canvas;
  frame.icPayload = payload;
  stageFit(frame);
  if (!frame.icWired){
    frame.icWired = true;
    frame.addEventListener('load', () => { frame.icReady = true; stagePost(frame, frame.icPayload); });
    // A box that grows or settles (the modal opening) is fitted again.
    new ResizeObserver(() => stageFit(frame)).observe(frame.parentElement);
  }
  if (frame.icReady) stagePost(frame, payload);
}
function stageFit(frame){
  const box = frame.parentElement, s = box && box.clientWidth / frame.icCanvas;
  if (!s) return;
  frame.style.width = frame.icCanvas + 'px';
  frame.style.height = Math.ceil(box.clientHeight / s) + 'px';
  frame.style.transform = `scale(${s})`;
}
function stagePost(frame, payload){
  if (frame.contentWindow && payload) frame.contentWindow.postMessage({instantcloneAlert:payload}, location.origin);
}

// The sound button just pressed shows it playing for a moment.
function showPlaying(act, v){
  const b = Modal.form && [...Modal.form.querySelectorAll(`[data-act="${act}"]`)].find(x => x.dataset.v === v);
  if (!b) return;
  b.classList.remove('ig-playing');
  void b.offsetWidth;
  b.classList.add('ig-playing');
  clearTimeout(b.icPlaying);
  b.icPlaying = setTimeout(() => b.classList.remove('ig-playing'), 1200);
}

// The font picker's samples: just "Aa" in each font, one small request.
function loadFontSamples(){
  if (document.getElementById('ig-font-samples')) return;
  const link = document.createElement('link');
  link.id = 'ig-font-samples';
  link.rel = 'stylesheet';
  const families = ALERT_FONTS.map(f => 'family=' + encodeURIComponent(f).replace(/%20/g, '+')).join('&');
  link.href = `https://fonts.googleapis.com/css2?${families}&text=Aa&display=swap`;
  document.head.appendChild(link);
}

// Play a sound as the alerts page would, from a hidden page of its own.
function playAlertSound(card, look){
  let frame = document.getElementById('ig-sound-frame');
  if (!frame){
    frame = document.createElement('iframe');
    frame.id = 'ig-sound-frame';
    frame.src = '/alerts?demo';
    frame.allow = 'autoplay';
    frame.hidden = true;
    document.body.appendChild(frame);
  }
  if (!frame.icWired){
    frame.icWired = true;
    frame.addEventListener('load', () => { frame.icReady = true; stagePost(frame, frame.icPayload); }, {once:true});
  }
  frame.icPayload = {look:look || lookOf(), card, sound:true};
  if (frame.icReady) stagePost(frame, frame.icPayload);
}

// A sound to pick: for an alert, none or the theme's; for the theme, its
// own. Then the built-in ones, the uploads (each with its delete), one
// more upload, and a link. Picking one plays it.
function soundPickerHtml(current, act, forAlert){
  const look = lookOf(), own = THEMES[themeOf(look)];
  const opt = (id, label) => `<button class="ig-sound${id === current ? ' on' : ''}" data-act="${act}" data-v="${esc(id)}" aria-pressed="${id === current}">
    ${id ? svg('play', ' class="ig-btn-ic"') : ''}${esc(label)}</button>`;
  const items = forAlert
    ? [opt('', 'None'), opt('theme', `Theme sound (${soundLabel(look.sound || own.sound)})`)]
    : [opt('', `${own.name}'s own (${soundLabel(own.sound)})`)];
  ALERT_SOUNDS.forEach(([id, label]) => items.push(opt(id, label)));
  mediaOf('sound').forEach(m => items.push(`<span class="ig-media-item">${opt('media:' + m.name, m.name)}${mediaDelHtml(m.name)}</span>`));
  const link = WEB_LINK.test(current) ? current : '';
  return `<div class="ig-sounds">${items.join('')}
      <button class="ig-sound ig-upload" data-act="media-upload" data-kind="sound">${svg('plus', ' class="ig-btn-ic"')}Upload a sound</button></div>
    <details class="ig-more-fields"${link ? ' open' : ''}><summary>Or a link to a sound file</summary>
      <input class="ic-input mono" data-bind="sound-link" value="${esc(link)}" placeholder="https://…/ding.mp3" spellcheck="false" aria-label="Link to a sound file"></details>`;
}

// A picture to pick: none, the uploads, one more upload, or a link.
function picturePickerHtml(current, act){
  const tile = (ref, inner, label) => `<button class="ig-pic${ref === current ? ' on' : ''}" data-act="${act}" data-v="${esc(ref)}"
    aria-pressed="${ref === current}" aria-label="${esc(label)}" title="${esc(label)}">${inner}</button>`;
  const items = [tile('', '<span>None</span>', 'No picture')];
  mediaOf('picture').forEach(m => items.push(`<span class="ig-media-item">${tile('media:' + m.name,
    `<img src="${esc(mediaSrc('media:' + m.name))}" alt="" loading="lazy">`, m.name)}${mediaDelHtml(m.name)}</span>`));
  const link = WEB_LINK.test(current) ? current : '';
  return `<div class="ig-pics">${items.join('')}
      <button class="ig-pic ig-upload" data-act="media-upload" data-kind="picture">${svg('plus')}<span>Upload</span></button></div>
    <details class="ig-more-fields"${link || (current && !MEDIA_REF.test(current)) ? ' open' : ''}><summary>Or a link to a picture or GIF</summary>
      <input class="ic-input mono" data-bind="image-link" value="${esc(MEDIA_REF.test(current) ? '' : current)}" placeholder="https://…/party.gif" spellcheck="false" aria-label="Link to a picture"></details>
    ${current && !MEDIA_REF.test(current) && !WEB_LINK.test(current) ? warnHtml('This isn\'t a web link: it has to start with https://') : ''}`;
}

function mediaDelHtml(name){
  return `<button class="ig-media-x" data-act="media-del" data-name="${esc(name)}" aria-label="Delete ${esc(name)}" title="Delete ${esc(name)}">${svg('x')}</button>`;
}

// The newest uploads and where the page is open, without touching the
// look being edited.
async function refreshAlerts(){
  const a = await api('/integrations/activity');
  if (a && a.alerts) S.data.alerts = Object.assign({}, a.alerts, {look:lookOf()});
}

// Pick a file and upload it. Resolves to its `media:` name, or null. A
// closed file dialog never resolves, which is fine: nothing waits on it.
function uploadMedia(kind){
  return new Promise(resolve => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = kind === 'sound' ? '.mp3,.ogg,.wav,.m4a,.webm' : '.png,.jpg,.jpeg,.gif,.webp';
    input.onchange = async () => {
      const file = input.files[0];
      if (!file) return resolve(null);
      if (file.size > 10 * 1024 * 1024){ toast('That file is over 10 MB', 'err'); return resolve(null); }
      const data = await new Promise(done => {
        const reader = new FileReader();
        reader.onload = () => done(String(reader.result).split(',')[1] || '');
        reader.onerror = () => done('');
        reader.readAsDataURL(file);
      });
      // base64url: the alphabet recipes use too.
      const r = await api('/integrations/alerts/media/upload', {name:file.name, data:data.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')});
      if (!r.ok){ toast(r.error || 'The upload didn\'t work', 'err', 6000); return resolve(null); }
      await refreshAlerts();
      toast(`Uploaded ${r.name}`, 'ok');
      resolve('media:' + r.name);
    };
    input.click();
  });
}

async function deleteMedia(el){
  if (!armConfirm(el, 'Delete?')) return;
  const name = el.dataset.name;
  const r = await api('/integrations/alerts/media/delete', {name});
  if (!r.ok){ toast(r.error || 'Couldn\'t delete it', 'err'); return; }
  await refreshAlerts();
  toast(`Deleted ${name}. Alerts that used it go without until you pick another.`, 'info', 5000);
  const o = OWNERS[Modal.kind];
  if (o && o.render) o.render();
}

// Put this alert on the alerts page for real, with sample values.
async function showAlertNow(step, h, btn){
  await busy(btn, async () => {
    const r = await api('/integrations/alerts/test', alertCardOf(step, h));
    if (!r.ok){ toast(r.error || 'Could not show it', 'err'); return; }
    const a = S.data.alerts || {};
    toast(a.in_obs ? 'It\'s on your alerts page in OBS now' : 'Sent, but your alerts page isn\'t in OBS: add it to OBS to see it',
      a.in_obs ? 'ok' : 'info', 5000);
  });
}

async function addAlertsToObs(btn){
  await busy(btn, async () => {
    const r = await api('/integrations/alerts/add-to-obs', {});
    if (!r.ok){ toast(r.error || 'OBS didn\'t add it', 'err', 7000); return; }
    toast('Added to the scene on air in OBS', 'ok');
    // OBS opens the page a moment later: the pill catches up then.
    setTimeout(() => poll(), 1500);
  });
}

// ---------------------------------------------------------------- alerts look

// The theme designer: the real page on a stage, the themes to start from,
// and every part of the look. Changes save on their own a moment later
// and OBS follows at once.
const AlertsLook = {
  saveTimer:null, saving:false,
  open(){
    const from = {editor:Editor, builder:Builder, connections:Conn}[Modal.kind] || null;
    Modal.open('alerts', 1060);
    Modal.onClose = () => { this.flush(); return true; };
    if (from) Modal.after = () => from.resume();
    this.render();
  },
  resume(){ this.open(); },
  look(){ return S.data.alerts.look; },
  render(){
    if (Modal.kind !== 'alerts') return;
    const look = this.look(), theme = themeOf(look), own = THEMES[theme];
    const styled = LOOK_STYLE.some(k => look[k] !== '' && look[k] != null) || look.sound || look.css;
    const section = (title, body) => `<section class="ig-look-sec"><div class="ic-label">${title}</div>${body}</section>`;
    Modal.body(`${modalHead('screen', 'var(--accent)', 'Alerts look', 'How every alert looks and sounds on stream. OBS follows as you change it.')}
      <div class="dest-form-body ig-look-body">
        <div class="ig-look-stagecol">
          <div class="ig-stage" data-key="stage"><iframe class="ig-stage-frame" src="/alerts?demo" title="Alerts preview" tabindex="-1" aria-hidden="true"></iframe></div>
          <div class="ig-look-acts">
            <button class="ic-btn ic-btn-ghost ig-small" data-act="look-replay">${svg('play', ' class="ig-btn-ic"')}Play it again</button>
            <button class="ic-btn ic-btn-ghost ig-small" data-act="alert-test">Show a test alert in OBS</button>
          </div>
          ${alertsStatusHtml(false)}
        </div>
        <div class="ig-look-controls">
          ${section('Theme', this.themesHtml(theme))}
          ${section('Place', this.placeHtml(look))}
          ${section('Font', this.fontsHtml(look, own))}
          ${section('Colors', this.colorsHtml(look, own))}
          ${section('Motion and picture', this.motionHtml(look, own))}
          ${section('Theme sound', `<div class="muted">Alerts set to "Theme sound" play this.</div>${soundPickerHtml(look.sound || '', 'look-sound', false)}`)}
          <details class="ig-more-fields"${look.css ? ' open' : ''}><summary>Custom CSS (advanced)</summary>
            <textarea class="ic-input ig-textarea-code ig-look-css" data-bind="look-css" spellcheck="false" placeholder=".card { letter-spacing: 1px }" aria-label="Custom CSS">${esc(look.css || '')}</textarea>
            <div class="muted">Comes after the theme, so it wins. Parts: <span class="mono">.card</span> <span class="mono">.pic</span> <span class="mono">.title</span> <span class="mono">.text</span> (its words <span class="mono">.w</span>, letters <span class="mono">.c</span>) <span class="mono">.time</span> <span class="mono">.fx</span>; this theme is <span class="mono">body.theme-${theme}</span>.</div></details>
          <button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="look-reset" ${styled ? '' : 'disabled'}>Back to ${esc(own.name)} as it comes</button>
        </div>
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg muted ig-saved${this.saving ? ' busy' : ''}" data-look-saved>
        <span class="ig-saved-dot" aria-hidden="true"></span><span data-look-msg>${this.saving ? 'Saving…' : 'Saved: OBS shows it now.'}</span></div>
        <button class="ic-btn ic-btn-primary" data-act="modal-close">Done</button></div>`);
    this.sync(true);
  },
  themesHtml(theme){
    return `<div class="ig-themes" role="radiogroup" aria-label="Theme">${Object.entries(THEMES).map(([id, t]) =>
      `<button class="ig-theme${id === theme ? ' on' : ''}" data-act="look-theme" data-v="${id}" role="radio" aria-checked="${id === theme}">
        <span class="ig-theme-stage"><iframe class="ig-theme-frame" src="/alerts?demo" tabindex="-1" aria-hidden="true" data-theme="${id}"></iframe></span>
        <span class="ig-theme-name">${t.name}</span><span class="ig-theme-sub">${esc(t.motion)} · ${soundLabel(t.sound)}</span></button>`).join('')}</div>`;
  },
  seg(name, items, cur){
    return `<div class="sub-tabs ig-seg">${items.map(([id, label]) =>
      `<button class="sub-tab${id === cur ? ' on' : ''}" data-act="look-set" data-name="${name}" data-v="${id}" aria-pressed="${id === cur}">${esc(label)}</button>`).join('')}${SEG_IND}</div>`;
  },
  range(name, label, value, min, max, unit, set){
    return `<div class="dff"><label>${label}${set ? ` <button class="ig-reset" data-act="look-clear" data-name="${name}">theme's</button>` : ''}</label>
      <div class="ig-range"><input type="range" min="${min}" max="${max}" step="1" data-bind="look-${name}" value="${value}" aria-label="${label}">
      <span class="mono" data-out="${name}">${value}${unit}</span></div></div>`;
  },
  // Where alerts sit, picked on a little screen: nine spots, and the card
  // slides to the one picked (index.html places it from data-x/data-y).
  placeHtml(look){
    const y = look.position || 'bottom', x = look.align || 'center';
    const name = (py, px) => py === 'middle' && px === 'center' ? 'Middle' : `${py[0].toUpperCase()}${py.slice(1)} ${px}`;
    const spots = ['top', 'middle', 'bottom'].flatMap(py => ['left', 'center', 'right'].map(px => {
      const on = py === y && px === x;
      return `<button class="ig-spot${on ? ' on' : ''}" data-act="look-place" data-y="${py}" data-x="${px}" role="radio" aria-checked="${on}" aria-label="${name(py, px)}" title="${name(py, px)}"></button>`;
    })).join('');
    return `<div class="ig-place-row">
      <div class="ig-place" data-y="${y}" data-x="${x}" role="radiogroup" aria-label="Where alerts show">${spots}
        <span class="ig-place-card" style="--sw:${alertColor('', look)}" aria-hidden="true"><i></i><i></i></span></div>
      <div class="ig-place-side"><div class="ig-place-now"><b>${name(y, x)}</b><span class="muted">of the screen</span></div>
        ${this.range('scale', 'Size', look.scale || 100, 50, 200, '%', false)}</div></div>`;
  },
  // Each font as a tile written in itself (only "Aa", fetched by
  // loadFontSamples), the theme's own first.
  fontsHtml(look, own){
    loadFontSamples();
    const cur = look.font || '';
    const tile = (id, family, name, sub) => `<button class="ig-font${id === cur ? ' on' : ''}" data-act="look-set" data-name="font" data-v="${esc(id)}" aria-pressed="${id === cur}" title="${esc(sub || name)}">
      <span class="ig-font-aa" style="font-family:${family}">Aa</span><span class="ig-font-n">${esc(name)}</span></button>`;
    const family = f => `'${f}',system-ui,sans-serif`;
    return `<div class="ig-fonts">${tile('', family(own.font), 'Theme\'s', `${own.name}'s own: ${own.font}`)}
      ${ALERT_FONTS.map(f => tile(f, family(f), f)).join('')}${tile('system', 'system-ui,sans-serif', 'This PC\'s', 'The font this PC uses')}</div>`;
  },
  colorsHtml(look, own){
    const color = (name, label, fallback) => {
      const value = look[name] || fallback;
      return `<div class="ig-colorf${look[name] ? ' set' : ''}" style="--sw:${esc(value)}">
        <label class="ig-swatch"><input type="color" data-bind="look-color" data-name="${name}" value="${esc(value)}" aria-label="${label} color"></label>
        <span class="ig-colorf-t"><b>${label}</b><span class="mono" data-out="c-${name}">${look[name] ? esc(value) : 'theme\'s'}</span></span>
        ${look[name] ? `<button class="ig-reset" data-act="look-clear" data-name="${name}" title="Back to the theme's" aria-label="Back to the theme's ${label.toLowerCase()} color">${svg('rewind')}</button>` : ''}</div>`;
    };
    return `<div class="ig-colors">${color('text', 'Text', own.fg)}${color('background', 'Background', own.bg)}${color('accent', 'Accent', own.accent)}</div>
      <div class="ig-look-grid">${this.range('opacity', 'Background opacity', look.opacity == null ? own.op : look.opacity, 0, 100, '%', look.opacity != null)}
        ${this.range('radius', 'Corners', look.radius == null ? own.r : look.radius, 0, 48, 'px', look.radius != null)}</div>`;
  },
  motionHtml(look, own){
    const cur = look.animation || '';
    const anims = [['', `Theme's (${own.motion.toLowerCase()})`]].concat(ALERT_ANIMS).map(([id, label]) =>
      `<button class="ig-sound${id === cur ? ' on' : ''}" data-act="look-set" data-name="animation" data-v="${id}" aria-pressed="${id === cur}">${esc(label)}</button>`).join('');
    return `<div class="dff"><label>Comes in with <span class="muted">· point at one to see it</span></label><div class="ig-sounds">${anims}</div></div>
      <div class="dff"><label>Picture</label>${this.seg('picture', [['', `Theme's (${own.pic === 'top' ? 'above' : 'left'})`], ['left', 'Left'], ['top', 'Above']], look.picture || '')}</div>`;
  },
  // Show the look on the stage, and each theme on its tile (`tiles`: only
  // when redrawn; a slider moving changes none of them). Pointing at a
  // tile plays that theme's entrance.
  sync(tiles){
    const card = sampleAlert(), stage = q('.ig-look-body .ig-stage-frame');
    stageShow(stage, 1280, {look:this.look(), card});
    if (!tiles) return;
    Modal.form.querySelectorAll('.ig-theme-frame').forEach(f => {
      stageShow(f, 760, {look:{theme:f.dataset.theme, position:'middle', align:'center', scale:170}, card});
      f.closest('.ig-theme').onpointerenter = () => stagePost(f, {...f.icPayload, replay:true});
    });
    // Pointing at an entrance plays it on the stage, nothing saved.
    Modal.form.querySelectorAll('[data-act="look-set"][data-name="animation"]').forEach(b => {
      b.onpointerenter = () => stagePost(stage, {look:{...this.look(), animation:b.dataset.v}, card, replay:true});
    });
  },
  input(el){
    const look = this.look(), b = el.dataset.bind;
    const out = (name, text) => { const o = q(`[data-out="${name}"]`); if (o) o.textContent = text; };
    if (b === 'look-scale'){ look.scale = +el.value; out('scale', el.value + '%'); }
    else if (b === 'look-opacity'){ look.opacity = +el.value; out('opacity', el.value + '%'); }
    else if (b === 'look-radius'){ look.radius = +el.value; out('radius', el.value + 'px'); }
    else if (b === 'look-color'){
      const name = el.dataset.name, first = !look[name];
      look[name] = el.value;
      el.closest('.ig-colorf').style.setProperty('--sw', el.value);
      out('c-' + name, el.value);
      // The first change brings the chip its way back to the theme's.
      if (first){ this.changed(true); return; }
    }
    else if (b === 'look-css') look.css = el.value;
    else if (b === 'sound-link'){
      const link = el.value.trim();
      if (link && !WEB_LINK.test(link)) return;
      look.sound = link;
    }
    else return;
    this.changed(false);
  },
  changed(redraw){
    this.saving = true;
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => this.flush(), 400);
    this.savedLine();
    if (redraw) this.render(); else this.sync(false);
  },
  savedLine(){
    const box = q('[data-look-saved]');
    if (!box) return;
    box.classList.toggle('busy', this.saving);
    box.querySelector('[data-look-msg]').textContent = this.saving ? 'Saving…' : 'Saved: OBS shows it now.';
  },
  async flush(){
    if (!this.saveTimer) return;
    clearTimeout(this.saveTimer);
    this.saveTimer = null;
    const r = await api('/integrations/alerts/look', this.look());
    this.saving = false;
    if (!r.ok){ toast(r.error || 'Couldn\'t save the look', 'err'); return; }
    this.savedLine();
  },
  playSound(sound){ playAlertSound({sound:sound || 'theme', volume:70}, this.look()); },
  async act(a, el){
    const look = this.look(), v = el.dataset.v;
    switch (a){
      case 'look-theme':
        // A theme as it comes: the last one's colors and font would clash.
        look.theme = v;
        LOOK_STYLE.forEach(k => { look[k] = k === 'opacity' || k === 'radius' ? null : ''; });
        this.changed(true);
        stagePost(q('.ig-look-body .ig-stage-frame'), {look, card:sampleAlert(), replay:'full', sound:true});
        return true;
      case 'look-place':
        look.position = el.dataset.y;
        look.align = el.dataset.x;
        this.changed(true);
        return true;
      case 'look-set':
        look[el.dataset.name] = v;
        this.changed(true);
        // A new entrance is seen by playing it.
        if (el.dataset.name === 'animation') stagePost(q('.ig-look-body .ig-stage-frame'), {look, card:sampleAlert(), replay:true});
        return true;
      case 'look-clear': {
        const name = el.dataset.name;
        look[name] = name === 'opacity' || name === 'radius' ? null : '';
        this.changed(true);
        return true;
      }
      case 'look-sound': look.sound = v; this.changed(true); this.playSound(v); showPlaying('look-sound', v); return true;
      case 'look-replay':
        stagePost(q('.ig-look-body .ig-stage-frame'), {look, card:sampleAlert(), replay:'full', sound:true});
        return true;
      case 'look-reset':
        if (!armConfirm(el, 'Clears your changes, CSS too. Sure?')) return true;
        LOOK_STYLE.forEach(k => { look[k] = k === 'opacity' || k === 'radius' ? null : ''; });
        look.sound = '';
        look.css = '';
        this.changed(true);
        return true;
      case 'media-upload':
        uploadMedia('sound').then(ref => {
          if (!ref || Modal.kind !== 'alerts') return;
          look.sound = ref;
          this.changed(true);
          this.playSound(ref);
        });
        return true;
    }
    return false;
  },
};

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
function recipeItemHtml(it, n){
  // "OBS crashes, the hold runs out": names after the first read on.
  const who = (it.triggers || []).map((t, i) => esc(i && /^[A-Z][a-z]/.test(t.what) ? t.what[0].toLowerCase() + t.what.slice(1) : t.what)
    + (t.who ? ` <span class="ig-rwho">(${esc(t.who)})</span>` : '')).join(', ');
  const effects = it.effects || [];
  const level = effects.some(e => e.level === 'danger') ? 'danger' : effects.some(e => e.level === 'caution') ? 'caution' : 'safe';
  const flags = effects.map((e, i) =>
    `<span class="ig-rflag ${esc(e.level)}" style="--i:${i}">${e.level === 'info' ? '' : svg('warn')}${esc(e.text)}</span>`).join('');
  const handlers = (it.integration && it.integration.handlers) || [];
  let count = 0;
  handlers.forEach(h => walk(h.steps, () => { count++; }));
  const steps = handlers.map(h =>
    `<div class="ig-rhandler"><div class="ig-rtrig">${svg('bolt')}<span>${esc(triggerSentence(h.trigger))}${h.enabled === false ? ' <span class="muted">(switched off)</span>' : ''}</span></div>
      <ol class="ig-rsteps">${recipeStepsHtml(h.steps)}</ol></div>`).join('');
  const json = it.integration ? JSON.stringify(it.integration, null, 2) : '';
  const raw = `<div class="ig-rraw-wrap"><div class="ig-rraw-head"><span>What gets added, exactly</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(json)}">${svg('copy', ' class="ig-btn-ic"')}Copy</button></div>
    <pre class="ig-rraw">${jsonColored(json)}</pre></div>`;
  return `<div class="ig-ritem ${level}" style="--i:${n}">
    <b class="ig-rname">${esc(it.name)}</b>
    <div class="ig-rwhen">When ${who || 'nothing'}</div>
    ${flags ? `<div class="ig-rflags">${flags}</div>` : ''}
    ${handlers.length ? `<div class="ig-rfolds">${recipeFold(`steps-${n}`, 'steps', `See every step <span class="ig-rcount">${count}</span>`, steps)}
      ${json ? recipeFold(`raw-${n}`, 'code', 'Raw settings', raw) : ''}</div>` : ''}</div>`;
}
// A row that opens smoothly onto `body`. Which rows are open survives a
// redraw of the import window (see Recipes.folds).
function recipeFold(key, icon, label, body){
  return `<details class="ig-rfold" data-fold="${esc(key)}"${Recipes.folds.has(key) ? ' open' : ''}>
    <summary>${svg(icon, ' class="ig-rfold-ic"')}<span class="ig-rfold-label">${label}</span>${svg('chev', ' class="ig-rchev"')}</summary>
    <div class="ig-rfold-body">${body}</div></details>`;
}
// JSON text with keys, strings, numbers and literals colored, every piece
// escaped on its own.
function jsonColored(text){
  const token = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g;
  let out = '', last = 0, m;
  while ((m = token.exec(text))){
    out += esc(text.slice(last, m.index));
    const cls = m[1] ? (m[2] ? 'jk' : 'js') : m[3] ? (m[3] === 'null' ? 'jn' : 'jb') : 'jd';
    out += `<span class="${cls}">${esc(m[1] || m[0])}</span>${m[2] ? esc(m[2]) : ''}`;
    last = token.lastIndex;
  }
  return out + esc(text.slice(last));
}
// Recipes never carry where a step reaches: shared and imported, they lose
// the program and its arguments, the file, the web address and the OBS
// scene or source (recipe::strip_private). Say so where that step is, so
// nobody wonders which program it would run.
const RECIPE_FILLS = {program:[['path'], 'program and its arguments'], file:[['path'], 'file'],
  http:[['url'], 'address'], obs:[['scene', 'source'], 'scene or source'], light:[['device'], 'light']};
function recipeFillNote(s){
  const fill = RECIPE_FILLS[s.type], p = s.params || {};
  // Recording, the replay buffer, screenshots and hotkeys pick no name.
  if (s.type === 'obs' && ['', 'hotkey'].includes((OBS_ACTIONS[p.action] || [])[1] || '')) return '';
  if (!fill || fill[0].some(name => p[name])) return '';
  return ` <span class="ig-rnote">· a recipe can't choose the ${fill[1]}: you pick ${s.type === 'program' ? 'them' : 'it'} after adding</span>`;
}
// The steps of a recipe being imported, in the builder's words, checks
// with their branches nested. What runs on the PC is tinted red; what
// changes the delay, OBS or calls a web address, amber.
function recipeStepsHtml(steps){
  if (!steps || !steps.length) return '<li class="ig-rstep ig-rnone">Nothing</li>';
  return steps.map((s, i) => {
    const k = KINDS[s.type] || {};
    const risk = LOCAL_KINDS.includes(s.type) ? ' danger' : ['delay_action', 'obs', 'http'].includes(s.type) ? ' caution' : '';
    const branches = s.type !== 'if' ? '' : `<ol class="ig-rsteps">${recipeStepsHtml(s.then)}</ol>`
      + ((s.else || []).length ? `<div class="ig-relse">Otherwise</div><ol class="ig-rsteps">${recipeStepsHtml(s.else)}</ol>` : '');
    return `<li class="ig-rstep${risk}" style="--i:${i}"><div class="ig-rline"><span class="ig-rtag" style="--c:${k.c || 'var(--fg-3)'}">${esc(k.tag || s.type)}</span>
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

// Your values: things set once ({my.name}) that any message or step reads,
// so a coin's name or a game's odds change in one place; and, on its own
// tab, what viewers hold. Each row shows what the value is for and one
// field to change it; its name and label hide behind the pencil, since
// renaming is rare and changing the value is the point.
const Values = {
  list:[], tab:'values', find:'', more:null, saved:'',
  open(){
    this.list = clone(S.data.values || []);
    this.saved = JSON.stringify(this.clean());
    this.tab = 'values';
    this.find = '';
    this.more = null;
    Modal.open('values', 860);
    Modal.onClose = () => !this.dirty() || (Modal.guard(), false);
    this.render();
    // Who holds what, without being asked: this is where it is managed.
    loadHolders();
  },
  // What Save would send: rows left wholly empty were never meant to be one.
  clean(){
    return this.list.filter(v => v.name || v.label || v.value).map(v => ({name:v.name.trim(), label:v.label.trim(), value:v.value}));
  },
  dirty(){ return JSON.stringify(this.clean()) !== this.saved; },
  // How a value is best edited, read off what it holds and what it is for.
  kindOf(v){
    if (/\n/.test(v.value) || v.value.length > 60 || / \| /.test(v.value)) return 'list';
    if (!/^-?\d+(\.\d+)?$/.test(v.value.trim())) return 'text';
    return /chance|percent|%/i.test(v.name + ' ' + v.label) ? 'chance' : 'number';
  },
  // How many integrations read {my.<name>} somewhere.
  usedIn(name){
    if (!name) return 0;
    const reads = new RegExp(`[{$]my\\.${name}(?![A-Za-z0-9_])`);
    return S.data.integrations.filter(i => reads.test(JSON.stringify(i))).length;
  },
  editorHtml(v, n){
    const at = `data-n="${n}"`;
    switch (this.kindOf(v)){
      case 'chance': return `<span class="ig-val-chance"><input type="range" min="0" max="100" step="0.5" value="${esc(v.value)}" data-bind="v-value" ${at} aria-label="${esc(v.label || v.name)}, as a slider">
        <span class="ig-val-unit"><input class="ic-input mono" data-bind="v-value" ${at} value="${esc(v.value)}" inputmode="decimal" aria-label="${esc(v.label || v.name)}"><span class="muted">%</span></span></span>`;
      case 'number': return `<input class="ic-input mono ig-val-num" data-bind="v-value" ${at} value="${esc(v.value)}" inputmode="decimal" aria-label="${esc(v.label || v.name)}">`;
      case 'list': {
        const lines = v.value.split('\n').filter(x => x.trim()).length;
        return `<textarea class="ic-input" data-bind="v-value" ${at} rows="${Math.min(8, Math.max(3, v.value.split('\n').length))}" aria-label="${esc(v.label || v.name)}">${esc(v.value)}</textarea>
          <div class="muted">${/\n/.test(v.value) ? `${plural(lines, 'line')}: one is picked each time.` : 'One per line, or separated by |.'}</div>`;
      }
    }
    return `<input class="ic-input" data-bind="v-value" ${at} value="${esc(v.value)}" aria-label="${esc(v.label || v.name)}">`;
  },
  rowHtml(v, n){
    const kind = this.kindOf(v), open = this.more === n, used = this.usedIn(v.name.trim());
    const meta = !v.name ? 'Give it a name to use it' : `<code class="mono">{my.${esc(v.name)}}</code> · ${used ? 'used in ' + plural(used, 'integration') : 'not used yet'}`;
    return `<div class="ig-val${kind === 'list' ? ' wide' : ''}${open ? ' open' : ''}" data-key="val-${n}">
      <div class="ig-val-main"><div class="ig-val-what"><b>${esc(v.label || v.name || 'New value')}</b><span class="ig-val-meta">${meta}</span></div>
        <div class="ig-val-edit">${this.editorHtml(v, n)}</div>
        <button type="button" class="ic-btn-tiny${open ? ' on' : ''}" data-act="v-more" data-n="${n}" aria-expanded="${open}" aria-label="Name and label of ${esc(v.label || v.name || 'this value')}" title="Name and label">${svg('pencil')}</button></div>
      ${open ? `<div class="ig-val-more"><div class="dff"><label>What it is for</label><input class="ic-input" data-bind="v-label" data-n="${n}" value="${esc(v.label)}" placeholder="!daily gives"></div>
        <div class="dff"><label>Name in steps</label><span class="ig-val-name mono">{my.<input class="ic-input mono" data-bind="v-name" data-n="${n}" value="${esc(v.name)}" placeholder="daily" maxlength="32" spellcheck="false">}</span>
          ${used ? `<div class="muted">${plural(used, 'integration')} read${used === 1 ? 's' : ''} it by this name: renaming it here leaves them reading nothing.</div>` : ''}</div>
        <button type="button" class="ic-btn ic-btn-ghost ig-small" data-act="v-del" data-n="${n}">Delete this value</button></div>` : ''}</div>`;
  },
  valuesHtml(){
    const find = this.find.trim().toLowerCase();
    const shown = this.list.map((v, n) => ({v, n})).filter(({v, n}) => n === this.more || !find || [v.name, v.label, v.value].some(x => x.toLowerCase().includes(find)));
    const groups = [['text', 'Words'], ['number', 'Amounts'], ['chance', 'Chances'], ['list', 'Lists a step picks from']]
      .map(([kind, title]) => ({title, rows:shown.filter(({v}) => this.kindOf(v) === kind)})).filter(g => g.rows.length);
    return `${this.list.length > 6 ? `<input class="ic-input ig-val-find" data-bind="v-find" value="${esc(this.find)}" placeholder="Find a value" aria-label="Find a value">` : ''}
      ${groups.map(g => `<div class="ig-val-group" data-key="g-${g.title}"><div class="ic-label">${g.title}</div>${g.rows.map(({v, n}) => this.rowHtml(v, n)).join('')}</div>`).join('')}
      ${!this.list.length ? '<div class="muted ig-val-empty">Nothing yet. A recipe you import brings its own values; or add one and write it as <span class="mono">{my.name}</span> wherever a value fits.</div>'
        : !shown.length ? '<div class="muted ig-val-empty">Nothing matches.</div>' : ''}
      <button type="button" class="ig-cond-add" data-act="v-add">${svg('plus')}Add a value</button>
      <div class="muted">Use one in any message or step as <span class="mono">{my.name}</span>. In a counting word, write it with a $: <span class="mono">{viewer.coins#$my.coin#$my.coins}</span> says "1 coin" or "5 coins" in your words.</div>`;
  },
  heldHtml(){
    const held = (Holders.values || []).map(v => holdersHtml(v.name)).join('');
    return !Holders.values ? '<div class="muted ig-val-empty">Asking the app who holds what…</div>'
      : held || '<div class="muted ig-val-empty">Nothing yet. A "Viewer value" step gives viewers coins or points, and they show up here.</div>';
  },
  render(){
    if (Modal.kind !== 'values') return;
    const dirty = this.dirty(), holders = (Holders.values || []).reduce((sum, v) => sum + v.holders, 0);
    const tabs = [['values', `Values · ${this.clean().length}`], ['held', `What viewers hold${holders ? ' · ' + holders : ''}`]];
    Modal.body(`${modalHead('steps', 'var(--accent)', 'Your values', 'Set something once and use it in any message or step. Change it here and everything that uses it follows.')}
      <div class="dest-form-body ig-conn-body ig-val-body">
        <div class="sub-tabs ig-seg ig-val-tabs">${tabs.map(([id, label]) => `<button class="sub-tab${id === this.tab ? ' on' : ''}" data-act="v-tab" data-v="${id}" aria-pressed="${id === this.tab}">${label}</button>`).join('')}${SEG_IND}</div>
        ${this.tab === 'held' ? this.heldHtml() : this.valuesHtml()}
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg">${dirty ? 'Changes not saved yet' : ''}</div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Close</button><button class="ic-btn ic-btn-primary" data-act="v-save" ${dirty ? '' : 'disabled'}>Save</button></div>`);
  },
  input(el){
    const b = el.dataset.bind, v = this.list[+el.dataset.n];
    if (b === 'v-find') this.find = el.value;
    else if (!v) return;
    else if (b === 'v-value') v.value = el.value;
    else if (b === 'v-name'){ v.name = el.value.trim(); v.named = true; }
    else if (b === 'v-label'){
      v.label = el.value;
      // Until a name is typed, it follows the label: "!daily gives" is daily_gives.
      if (!v.named && !this.usedIn(v.name)) v.name = el.value.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '').slice(0, 32);
    }
    this.render();
  },
  // Why the list can't be saved as it is, with the row to look at; or null.
  problem(){
    const names = new Set();
    for (const [n, v] of this.list.entries()){
      if (!v.name && !v.label && !v.value) continue;
      if (!/^[A-Za-z0-9_]{1,32}$/.test(v.name.trim())) return {n, why:`"${v.label || v.name || 'A value'}" needs a name: letters, digits and _`};
      if (names.has(v.name.trim())) return {n, why:`Two values are named ${v.name.trim()}`};
      names.add(v.name.trim());
    }
    return null;
  },
  async act(a, el){
    const n = +el.dataset.n;
    if (a === 'v-tab'){ this.tab = el.dataset.v; this.render(); return true; }
    if (a === 'v-more'){ this.more = this.more === n ? null : n; this.render(); return true; }
    if (a === 'v-add'){
      this.list.push({name:'', label:'', value:''});
      this.more = this.list.length - 1;
      this.find = '';
      this.render();
      focusField('v-label');
      return true;
    }
    if (a === 'v-del'){
      if (this.usedIn(this.list[n].name) && !armConfirm(el, 'Still in use. Delete?')) return true;
      this.list.splice(n, 1);
      this.more = null;
      this.render();
      return true;
    }
    if (a === 'v-save'){
      const wrong = this.problem();
      if (wrong){ this.more = wrong.n; this.find = ''; this.render(); toast(wrong.why, 'err', 6000); return true; }
      await busy(el, async () => {
        const values = this.clean();
        const r = await api('/integrations/values', {values});
        if (!r || r.ok === false){ toast((r && r.error) || 'That didn\'t save', 'err', 6000); return; }
        this.saved = JSON.stringify(values);
        if (Modal.kind === 'values') Modal.close(true);
        await load();
        toast('Saved', 'ok');
      });
      return true;
    }
    return false;
  },
};
const Recipes = {
  preview:null, text:'', back:null,
  // Which "See every step" and "Raw settings" rows are open.
  folds:new Set(),
  resume(){
    Modal.open('import', 700);
    Modal.back = this.back;
    this.render();
  },
  openImport(){
    this.preview = null;
    this.folds.clear();
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
            ${p.items.map((it, n) => recipeItemHtml(it, n)).join('')}
          </div>
          ${recipeWarningHtml(p)}
          ${p.alerts ? this.alertsHtml(p) : ''}
          ${(p.values || []).length ? `<div class="sys-section" data-key="r-values"><div class="ic-label">Values it brings</div>
            <div class="muted">You change these later under <b>Your values</b>. One you already set stays as you set it.</div>
            <ul class="ig-value-list">${p.values.map(v => `<li><b>${esc(v.label || v.name)}</b> <span class="mono">${esc(v.mine == null ? v.value : v.mine)}</span>${v.mine == null ? '' : ' <span class="muted">(yours, kept)</span>'}</li>`).join('')}</ul></div>` : ''}
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
    this.syncAlerts();
  },
  // A recipe's alerts bring their theme (shown on the real page) and
  // their uploads: taking the theme is a choice, ticked to start only
  // while this alerts page still has the plain default one.
  alertsHtml(p){
    const al = p.alerts, own = THEMES[themeOf(al.look)];
    const names = list => list.map(m => `<b>${esc(m.name)}</b>`).join(', ');
    return `<div class="sys-section ig-recipe-alerts" data-key="r-alerts">
      <div class="ic-label">Their alerts look</div>
      <div class="ig-stage ig-stage-short"><iframe class="ig-stage-frame" src="/alerts?demo" title="Their alerts look" tabindex="-1" aria-hidden="true"></iframe></div>
      <label class="crash-check"><input type="checkbox" data-bind="r-theme" ${al.mine_is_default ? 'checked' : ''}>
        <span><span class="crash-check-title">Use their alerts look (${esc(own.name)}${al.look.css ? ', with custom CSS' : ''})</span>
        <span class="muted">Every alert on your alerts page takes it, and the theme sound. You can change it any time in Alerts page.</span></span></label>
      ${al.media.length ? `<div class="muted">Comes with ${names(al.media)}: saved with your uploads.</div>` : ''}
      ${al.left_out.length ? warnHtml(`Too big to come along: ${names(al.left_out)}. Those alerts play the theme sound, or show no picture, until you pick another.`) : ''}
    </div>`;
  },
  // Their look on the stage, with their first alert.
  syncAlerts(){
    const p = this.preview;
    if (!p || !p.alerts) return;
    let card = {title:'New follower', text:'Nat', sound:'theme'};
    for (const it of p.items){
      const h = (it.integration.handlers || []).find(x => (x.steps || []).some(s => s.type === 'overlay'));
      if (h){ card = alertCardOf(h.steps.find(s => s.type === 'overlay'), h); break; }
    }
    // Their uploads aren't saved here yet: the stage shows the look alone.
    card.image = MEDIA_REF.test(card.image) ? '' : card.image;
    stageShow(q('.ig-recipe-alerts .ig-stage-frame'), 1280, {look:Object.assign({}, p.alerts.look, {position:'middle', align:'center'}), card});
  },
  async act(a, el){
    if (a === 'r-preview'){
      await busy(el, async () => {
        this.text = Conn.value('r-text');
        const r = await api('/integrations/import', {recipe:this.text});
        if (!r.ok){ toast(r.error, 'err', 6000); return; }
        this.preview = r;
        this.folds.clear();
        this.render();
      });
      return true;
    }
    if (a === 'r-apply'){
      await busy(el, async () => {
        const sel = q('[data-bind="r-discord"]'), local = q('[data-bind="r-local"]'), theme = q('[data-bind="r-theme"]');
        const r = await api('/integrations/import', {recipe:this.text, apply:true, discord:sel ? sel.value : '', allow_local:!!(local && local.checked),
          use_theme:!!(theme && theme.checked)});
        if (!r.ok){ toast(r.error, 'err', 6000); return; }
        if (Modal.kind === 'import') Modal.close(true);
        await load();
        this.landed((r.ids || []).map(find).filter(Boolean), r.added);
      });
      return true;
    }
    if (a === 's-make'){ await busy(el, () => this.share()); return true; }
    return false;
  },
  // What an import added: the first that still needs a detail opens on it
  // (a recipe never carries addresses or channels); otherwise they glow
  // where they landed.
  landed(added, count){
    const n = plural(count || added.length, 'integration');
    const unfinished = added.find(i => issuesOf(i).length);
    if (unfinished){
      toast(`Added ${n}, switched off. ${unfinished.name} needs one more detail first.`, 'info', 5000);
      openEditor(unfinished);
      return;
    }
    added.forEach(i => reveal(i.id));
    toast(`Added ${n}. They're off until you switch them on.`, 'ok', 4500);
  },
  // A pasted recipe previews at once: there is nothing else to do with it.
  pasted(el){
    const text = el.value.trim();
    if (!text.startsWith('ic-recipe:1:') || text === this.text.trim()) return;
    this.act('r-preview', q('.dest-form-foot .ic-btn-primary'));
  },
  // `only`: shared from one integration's menu, the recipe is made (and
  // copied) at once; ticking more remakes it with "Make recipe".
  openShare(only){
    const one = only ? find(only) : null;
    Modal.open('share', 700);
    Modal.body(`${modalHead('copy', 'var(--accent)', 'Share as a recipe', 'Pick what to share. Your connections, keys and secret links are never included.')}
      <div class="dest-form-body ig-conn-body">
        <div class="dff"><label>Recipe name</label><input class="ic-input" data-bind="s-name" placeholder="My crash kit" value="${one ? esc(one.name) : ''}"></div>
        <div class="ig-share-list">${S.data.integrations.map(i =>
          `<label class="crash-check"><input type="checkbox" data-bind="s-pick" value="${esc(i.id)}" ${i.id === only ? 'checked' : ''}><span><span class="crash-check-title">${esc(i.name)}</span></span></label>`).join('')}</div>
        <label class="crash-check"><input type="checkbox" data-bind="s-theme" checked><span><span class="crash-check-title">Alerts bring my alerts look</span>
          <span class="muted">The theme, its changes and custom CSS, and the sounds and pictures they use (up to 4 MB in all). They choose whether to use it.</span></span></label>
        <textarea class="ic-input ig-textarea-code" data-bind="s-out" readonly hidden></textarea>
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Close</button><button class="ic-btn ic-btn-primary" data-act="s-make">Make recipe</button></div>`);
    if (one) busy(q('[data-act="s-make"]'), () => this.share());
    else focusField('s-name');
  },
  async share(){
    const ids = [...Modal.el.querySelectorAll('[data-bind="s-pick"]:checked')].map(x => x.value);
    if (!ids.length){ toast('Tick at least one integration to share', 'info'); return; }
    const theme = q('[data-bind="s-theme"]');
    const r = await api('/integrations/export', {ids, name:Conn.value('s-name'), with_theme:!theme || theme.checked});
    if (!r.ok){ toast(r.error, 'err'); return; }
    if (Modal.kind !== 'share') return;
    const out = q('[data-bind="s-out"]');
    out.hidden = false;
    out.value = r.recipe;
    // Selected too, so a copy that fails is one Ctrl+C away.
    if (await writeClipboard(r.recipe)) toast('Recipe copied: paste it anywhere', 'ok', 3500);
    else toast(COPY_FAILED, 'err');
    out.focus({preventScroll:true});
    out.select();
  },
};

// ---------------------------------------------------------------- events

const OWNERS = {editor:Editor, builder:Builder, connections:Conn, alerts:AlertsLook, import:Recipes, share:Recipes, catalog:Catalog, values:Values};

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
    case 'discard': discardEdits(Modal.kind === 'builder' ? Builder : Editor); break;
    case 'guard-discard':
      if (Modal.pending === 'back' && Modal.back) Modal.leaveBack(); else Modal.close(true);
      break;
    case 'menu': Menu.toggle(el, el.dataset.id); break;
    case 'menu-delete':
      Menu.close();
      remove(el.dataset.id);
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
    case 'lock-why': toast(LOCAL_LOCK_TEXT, 'info', 8000); break;
    case 'last-run': {
      setLastRun(!S.lastRunOpen);
      const back = q('.ig-last-btn');
      if (!S.lastRunOpen && back) back.focus({preventScroll:true});
      break;
    }
    case 'act-row': S.actOpen = S.actOpen === el.dataset.k ? null : el.dataset.k; render(); break;
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
    case 'fix-var': {
      const o = OWNERS[Modal.kind];
      if (!o || !o.handler) break;
      renameVar(o.handler(), el.dataset.from, el.dataset.to);
      o.changed();
      break;
    }
    case 'obs-retry': busy(el, () => ObsLive.tick()); break;
    case 'catalog': Catalog.open(); break;
    case 'build': Builder.open(null); break;
    case 'import': Recipes.openImport(); break;
    case 'values': Values.open(); break;
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
    case 'rewards-load': Rewards.error = ''; busy(el, () => loadRewards()); break;
    case 'rewards-new': case 'rewards-edit': case 'rewards-cancel': {
      const row = el.closest('.ig-reward');
      Rewards.editing = a === 'rewards-new' ? 'new' : a === 'rewards-edit' ? row.dataset.id : null;
      if (OWNERS[Modal.kind] && OWNERS[Modal.kind].render) OWNERS[Modal.kind].render();
      const first = q('.ig-reward-form [data-r="title"]');
      if (first && a !== 'rewards-cancel') first.focus();
      break;
    }
    case 'rewards-save': {
      const form = el.closest('.ig-reward-form'), typed = name => form.querySelector(`[data-r="${name}"]`);
      const title = typed('title').value.trim(), cost = typed('cost').value.trim();
      if (!title || !/^[1-9]\d*$/.test(cost)){ toast('Give it a name and a cost in points', 'err'); break; }
      const change = {title, cost:+cost, prompt:typed('prompt').value.trim(), wants_input:typed('wants_input').checked};
      if (form.dataset.id) change.id = form.dataset.id;
      const was = form.dataset.was;
      busy(el, async () => {
        if (!await loadRewards(change)) return;
        Rewards.editing = null;
        // What this integration said about the old name follows the new one.
        if (was && was !== title && Modal.kind === 'builder'){ renameReward(Builder.d, was, title); Builder.changed(); }
        else if (OWNERS[Modal.kind] && OWNERS[Modal.kind].render) OWNERS[Modal.kind].render();
      });
      break;
    }
    case 'rewards-switch': busy(el, () => loadRewards({id:el.closest('.ig-reward').dataset.id, enabled:!!el.dataset.on})); break;
    case 'rewards-delete':
      if (!armConfirm(el, 'Delete?')) break;
      busy(el, () => loadRewards({id:el.closest('.ig-reward').dataset.id, delete:true}));
      break;
    case 'holders-load': busy(el, () => loadHolders()); break;
    case 'holders-set': {
      const box = el.closest('.ig-holders'), field = name => box.querySelector(`[data-h="${name}"]`).value.trim();
      if (!field('login') || !/^-?\d+$/.test(field('amount'))){ toast('Type a viewer and a whole number', 'err'); break; }
      busy(el, () => loadHolders({value:box.dataset.value, login:field('login'), amount:+field('amount')}));
      break;
    }
    case 'holders-clear':
      if (!armConfirm(el, 'Everyone back to 0. Sure?')) break;
      busy(el, () => loadHolders({value:el.closest('.ig-holders').dataset.value, clear:true}));
      break;
    case 'light-scan':
      busy(el, async () => {
        const r = await api('/integrations/lights/scan', {});
        if (!r || r.ok === false){ toast((r && r.error) || 'The scan didn\'t work', 'err'); return; }
        LightScan.devices = r.devices || [];
        const o = OWNERS[Modal.kind];
        if (o && o.render) o.render();
      });
      break;
    case 'light-try':
      busy(el, async () => {
        const r = await api('/integrations/lights/try', {device:el.dataset.ref, color:el.dataset.color || ''});
        if (r && r.ok !== false) toast(r.note ? 'Flashed it: ' + r.note : 'Flashed it', 'ok');
        else toast((r && r.error) || 'It didn\'t answer', 'err');
      });
      break;
    case 'alerts-add': addAlertsToObs(el); break;
    case 'alerts-open': AlertsLook.open(); break;
    case 'media-del': deleteMedia(el); break;
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
  if (el.type === 'range') fillRange(el);
  if (el.id === 'ig-search'){
    S.search = el.value;
    render();
    return;
  }
  if (!el.dataset || !el.dataset.bind || !Modal.el || !Modal.el.contains(el)) return;
  if (Modal.kind === 'editor') Editor.input(el);
  else if (Modal.kind === 'builder') Builder.input(el);
  else if (Modal.kind === 'alerts') AlertsLook.input(el);
  else if (Modal.kind === 'values') Values.input(el);
  // Pasted or dropped, not typed: typing would preview half a recipe.
  else if (Modal.kind === 'import' && el.dataset.bind === 'r-text' && /^insertFrom(Paste|Drop)$/.test(e.inputType || '')) Recipes.pasted(el);
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
  // Enter in a list's find box picks the first match, or the typed name.
  if (e.key === 'Enter' && e.target.matches('[data-bind="pick-find"]')){
    e.preventDefault();
    const first = e.target.closest('.ig-pick').querySelector('.ig-pick-row');
    if (first) first.click();
    return;
  }
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
  // From the Overlay tab: where alerts go on stream.
  async openAlertsPage(){
    if (!S.data && !(await load())) return;
    Conn.open('stream');
  },
};

// A reload or a restored tab can land straight on Integrations.
const pane = document.querySelector('.tab-pane[data-pane="integrations"]');
if (pane && !pane.hidden) window.Integrations.show();
})();
