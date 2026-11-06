// ---------------------------------------------------------------- helpers

const esc = v => String(v == null ? '' : v).replace(/[&<>"']/g,
  c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const clone = v => JSON.parse(JSON.stringify(v));
const store = {
  get(k){ try { return localStorage.getItem(k); } catch(_){ return null; } },
  set(k, v){ try { localStorage.setItem(k, v); } catch(_){} },
};

// Stored JSON, or `fallback` when there is none or it doesn't read.
function readJson(text, fallback){
  try { return text ? JSON.parse(text) : fallback; } catch(_){ return fallback; }
}

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
  folder:'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z',
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
  target:'M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 18a6 6 0 1 0 0-12 6 6 0 0 0 0 12zM12 14a2 2 0 1 0 0-4 2 2 0 0 0 0 4z',
  pause:'M8 5v14M16 5v14',
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
  dock:         {label:'Dock event',    tag:'DOCK',   c:'#5ac8fa', icon:'list', text:'text', hint:"Lists it in your OBS dock's Events, to filter, and to replay the alert this run showed."},
  timeline:     {label:'Timeline',      tag:'LOG',    c:'#5ac8fa', icon:'list', text:'text', hint:'Adds a line, a chapter or a highlight to this stream\'s timeline, at its place in the VOD.'},
  light:        {label:'Smart light',   tag:'LIGHT',  c:'#fde68a', icon:'bulb', hint:'Turns a light or plug on your network on or off, or colors it. Found by a scan: no hub or account.'},
  viewer:       {label:'Viewer value',  tag:'VALUE',  c:'#fbbf24', icon:'gem', hint:'Gives, takes or spends what a viewer holds: coins, points. Kept between streams.'},
  math:         {label:'Maths',         tag:'VALUE',  c:'#93c5fd', icon:'hash', hint:'Works out a number: a sum, a share, a random one.'},
  wait_chat:    {label:'Wait for chat', tag:'WAIT',   c:'#c3cad1', icon:'bubble', hint:'Waits for someone to type something, up to a time limit.'},
  call:         {label:'Run a block',   tag:'FLOW',   c:'#f0a53a', icon:'layers', hint:'Runs a block of steps this integration keeps under a name.'},
  reward:       {label:'Channel points reward', tag:'TWITCH', c:'#a78bfa', icon:'gift', hint:'Switches a channel points reward on or off, so viewers can only redeem it when it makes sense.'},
  channel:      {label:'Stream title and category', tag:'TWITCH', c:'#a78bfa', icon:'tv', hint:'Sets your stream\'s title, its category, or both, on Twitch.'},
  goal:         {label:'Goal or timer', tag:'STREAM', c:'#86efac', icon:'target', hint:'Adds to a goal, or starts, pauses or adds time to a timer.'},
  obs:          {label:'OBS',           tag:'OBS',    c:'#86efac', icon:'layers', hint:'Switches a scene, shows or hides a source, records, saves a replay, mutes, switches a filter, plays media or presses an OBS hotkey.'},
};
// The icon a catalog module wears: what it is about, not how it sends.
const PRESET_ICON = {
  crash_alert:'shield', destination_down:'signal', going_live:'megaphone', phone_crash:'phone',
  tell_chat:'bubble', delay_command:'hash', delay_notice:'clock', mod_controls:'cut', socials:'hash',
  vod_markers:'bookmark', webhook:'link', crash_counter:'file', stream_alerts:'bell', api_command:'globe',
  clip_button:'film', delay_status:'clock', smart_alerts:'pulse', stream_timeline:'list', chapters:'bookmark',
  highlight:'star', hype_clip:'flame', on_air_light:'bulb', scene_delay:'layers', twitch_alerts:'heart',
  hype_alerts:'flame', stream_log:'list', ad_break_notice:'tv', mute_reward:'mic',
};
// Which step a card previews: the first one that says something.
const PREVIEW_ORDER = ['discord','chat','phone','overlay','marker','http','light','file','timeline','dock','clip','delay_action','program'];
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
