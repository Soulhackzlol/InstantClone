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
    return `<button type="button" class="ig-reward-pick${on ? ' on' : ''}" role="checkbox" aria-checked="${on}" data-act="b-reward-pick" data-id="${esc(r.id)}">
      <span class="ig-reward-pick-box">${on ? svg('check') : ''}</span><span class="ig-reward-pick-t">${esc(r.title)}</span><span class="muted">${esc(String(r.cost))} pts${r.enabled ? '' : ' · off'}</span></button>`;
  }).join('');
  const nothing = !c.picked.length && !c.other.length;
  return `<div class="dff"><label>Which rewards start this</label>${scope}
    <div class="ig-reward-picks">${rows || (Rewards.list ? '<div class="muted">You have made no rewards here yet. Make one below, or name one you made on Twitch.</div>' : '')}</div>
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
  ['Stream', [['overlay', 'Show on stream'], ['dock', 'Dock event'], ['reward', 'Channel points reward'], ['channel', 'Stream title and category'], ['goal', 'Goal or timer'], ['delay_action', 'Delay action'], ['obs', 'OBS'], ['marker', 'VOD marker'], ['clip', 'Clip'], ['timeline', 'Timeline']]],
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
  delay_armed:{icon:'clock', desc:'You arm one: it starts saving video for it.'},
  delay_ready:{icon:'check', desc:'There\'s enough saved: it can go on air.'},
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
  return andList(ids.map(id => TWITCH_NOUNS[id] || id));
}
// "a, b and c".
function andList(words){
  return words.length > 1 ? words.slice(0, -1).join(', ') + ' and ' + words[words.length - 1] : words.join('');
}
// What the Twitch login can't do until it logs in again (a login from
// before InstantClone asked for it), in words.
function twitchMissing(){
  return andList(S.data.twitch.main.missing || []);
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
  d:null, h:0, sel:'trigger', target:[], after:null, run:null, dirty:false, back:null,
  testValues:{}, testDry:true, testOpen:false,
  // opts: {dirty} carries the editor's unsaved state over, {switchOn} as in
  // Editor.open, {back} as in Modal.back (default: the catalog, if open).
  open(i, opts){
    opts = opts || {};
    dropAddUndos();
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
    if (selected && selected.type === 'overlay') alertStage(selected, h, false);
  },
  // Whether it still shows `step`: an upload or a design picked a moment
  // ago lands only there.
  editing(step){ return Modal.kind === 'builder' && Array.isArray(this.sel) && this.has(this.sel) && this.stepAt(this.sel) === step; },
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
    // A check on the alert's amount is one of its bigger ones (Bigger ones
    // in the editor): it reads as amounts, not as Then and Otherwise.
    const amount = tierVar(this.handler());
    const out = (steps || []).map((s, n) => {
      const path = base.concat(n), pk = pathKey(path), id = uidOf(s);
      const tier = s.type === 'if' && amount && isTierCheck(s, amount) ? `${s.params.right} ${TIER_UNITS[amount]}` : '';
      const k = tier ? Object.assign({}, KINDS.if, {label:'Bigger alerts'}) : KINDS[s.type] || {label:s.type, tag:'?', c:'#888'};
      const off = s.enabled === false;
      // Inside a switched-off check, nothing runs: nothing is missing.
      const missing = off || this.insideOff(path) ? '' : stepMissing(s);
      // A mistyped name is quieter than what's missing, and waits for it.
      const unknown = missing || off ? [] : unknownVars(this.handler(), s);
      let html = `<div class="ig-step${selKey === pk ? ' on' : ''}${off ? ' off' : ''}${missing ? ' warn' : ''}" data-path="${pk}" style="--c:${k.c}" data-key="s-${id}">
        <button type="button" class="ig-step-main" data-act="b-sel" data-path="${pk}"${selKey === pk ? ' aria-current="true"' : ''}
          title="Alt+Up or Alt+Down moves it, Ctrl+D duplicates it, Delete removes it">
          <span class="ig-step-ic">${svg(k.icon || 'steps')}</span><span class="ig-step-body"><span class="ig-step-kind">${esc(k.label)}</span><span class="ig-step-text">${tier ? `From ${esc(tier)} up, a bigger one plays` : cardSentence(s)}${off ? '<span class="sr-only">, switched off</span>' : ''}</span>${s.type === 'overlay' ? alertMetaHtml(s) : ''}</span></button>
        ${missing ? `<span class="ig-step-warn">${esc(missing)}</span>`
          : unknown.length ? `<span class="ig-step-warn soft"><span translate="no">${unknown.map(u => `{${esc(u.name)}}`).join(' ')}:</span> ${unknown.length > 1 ? 'nothing fills these' : 'nothing fills it'}</span>` : ''}
        <span class="ig-step-tools">
          ${tool('b-move', pk, 'up', 'Move up', ` data-dir="-1"${this.moveTarget(path, -1) ? '' : ' disabled'}`)}
          ${tool('b-move', pk, 'down', 'Move down', ` data-dir="1"${this.moveTarget(path, 1) ? '' : ' disabled'}`)}
          ${tool('b-toggle-step', pk, off ? 'eyeOff' : 'eye', off ? 'Switch this step on' : 'Switch this step off')}
          ${tool('b-dup', pk, 'copy', 'Duplicate')}
          ${tool('b-del', pk, 'x', 'Remove')}
        </span></div>`;
      if (s.type === 'if'){
        html += `<div class="ig-branch" data-key="br-${id}"><div class="ig-lanes">
          <div class="ig-lane yes"><div class="ig-branch-label">${svg('check')}${tier ? 'Below ' + esc(tier) : 'Then'}</div><div class="ig-branch-list" data-flip data-list="${pathKey(path.concat('then'))}">${this.stepsHtml(s.then, path.concat('then'), id + 't')}</div></div>
          <div class="ig-lane no"><div class="ig-branch-label else">${svg(tier ? 'up' : 'x')}${tier ? 'From ' + esc(tier) : 'Otherwise'}</div><div class="ig-branch-list" data-flip data-list="${pathKey(path.concat('else'))}">${this.stepsHtml(s.else, path.concat('else'), id + 'e')}</div></div>
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
  // Whether `count` more steps, opening `levels` levels of lanes, fit in
  // the list at `base`: MAX_STEPS in all, checks 4 deep at most. Says why
  // when they don't.
  roomFor(base, count, levels){
    if (!stepsFit(this.d, count)) return false;
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
    // The values themselves come to the field being written in (ValuePick).
    const varsBox = `<div class="ig-insp-vars">${FORMS_HELP}</div>`;
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
      case 'dock': return field('text', 'Line', {ph:'{user} cheered {bits} bits'})
        + field('group', "Group, for the dock's filter", {ph:'Bits'})
        + field('replay', 'Replay', {select:[['', 'Plays the alert this run showed before it'], ['no', 'None']]})
        + note("Shows in every OBS dock with the Events widget on, newest first. It lasts until InstantClone restarts.");
      case 'timeline': return field('text', 'Line', {area:true, ph:'⭐ Highlight · [clip]({clip.url})'})
        + field('kind', 'It is', {select:Object.entries(TIMELINE_KINDS)})
        + field('at', 'It marks', {select:[['live', 'What you do now (lands after the delay)'], ['aired', 'What viewers see now']]})
        + note('The timeline is this stream\'s, shared by every integration: <span class="mono">{timeline}</span> shows it, <span class="mono">{chapters}</span> lists the chapters for YouTube. A new stream starts a new one.')
        + note('A button or a scene is pressed live: "what you do now". A mod\'s chat command is about what they saw: "what viewers see now".');
      case 'chat': return field('text', 'Message', {area:true}) + field('as', 'Send as', {select:[['', 'Your account'], ['bot', 'Bot account']]})
        + field('reply', 'Reply to the viewer', {select:[['', 'No'], ['yes', 'Yes, in their thread']]})
        + note('Start it with a Twitch command to run one: <span class="mono">/timeout {user} 10m</span>, <span class="mono">/ban</span>, <span class="mono">/unban</span>, <span class="mono">/shoutout</span>, <span class="mono">/announce</span> or <span class="mono">/me</span>. Your account runs it. What a viewer types can fill it in, never start one.');
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
          + note('Starts or changes the delay. A delay too long for the buffer fails this step.') : '')
        + warnHtml('This changes your stream. Tests never run it.');
      case 'marker': return field('description', 'Label', {ph:'Crash'}) + note('Only works while you are live on Twitch.');
      case 'channel': return field('title', 'New title (blank: leave it as it is)', {ph:'{args}'})
        + field('category', 'New category (blank: leave it as it is)', {ph:'{args}, or VALORANT'})
        + note('A category can be typed loosely: "valo" finds VALORANT. Later steps can use <span class="mono">{stream.category}</span> (its name on Twitch), <span class="mono">{stream.ok}</span> and, when it failed, <span class="mono">{stream.error}</span>.');
      case 'goal': {
        const timer = s.params.what === 'timer', word = timer ? 'timer' : 'goal';
        const made = trackersOf(word).map(x => [x.id, x.name]);
        // One that was deleted since still shows, so the step says what it lost.
        const options = !s.params.id || made.some(([id]) => id === s.params.id) ? made : [[s.params.id, s.params.id + ' (deleted)']].concat(made);
        const does = timer ? [['start', 'Start it'], ['pause', 'Pause it'], ['add', 'Add time'], ['subtract', 'Take time off'], ['set', 'Set the time left'], ['reset', 'Back to its start, stopped']]
          : [['add', 'Add'], ['subtract', 'Subtract'], ['set', 'Set to'], ['reset', 'Reset to 0']];
        const takesAmount = ['add', 'subtract', 'set'].includes(s.params.action);
        return field('what', 'Change', {select:[['', 'A goal'], ['timer', 'A timer']]})
          + (options.length ? field('id', timer ? 'Timer' : 'Goal', {select:[['', timer ? 'Pick a timer' : 'Pick a goal']].concat(options)})
            : note(`${timer ? 'No timer yet.' : 'No goal yet.'} Make one in <b>Overlay › Goals and timers</b>.`))
          + field('action', 'Do', {select:does})
          + (takesAmount ? field('amount', timer ? 'Seconds, or a value like {arg1}' : 'How much, or a value like {arg1}', {mono:true, ph:timer ? '60' : '1'}) : '')
          + note(`Any message can say where it stands: <span class="mono">{${word}.${esc(s.params.id || 'name')}}</span>${timer ? '' : `, and <span class="mono">{goal.${esc(s.params.id || 'name')}.target}</span>`}. Tests never change your goals and timers.`);
      }
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
      case 'overlay': {
        const p = s.params;
        const seconds = ALERT_SECONDS.includes(p.seconds || '6') ? ALERT_SECONDS : ALERT_SECONDS.concat(p.seconds);
        return `<div class="ig-live ig-live-alert ig-bld-alert">
            <div class="ig-live-row"><span class="ig-live-tag">ON STREAM · LIVE PREVIEW</span>
              <button class="ig-link" data-act="alert-replay">${svg('play', ' class="ig-btn-ic"')}Play it</button></div>
            <div class="ig-ed-stage"><iframe class="ig-stage-frame" src="/alerts?demo" title="Alert preview" tabindex="-1" aria-hidden="true"></iframe></div></div>`
          + field('title', 'Small title', {ph:'Clip'}) + field('text', 'Text', {area:true, ph:'{user} just clipped that'})
          + field('message', 'Viewer\'s message (optional)', {ph:'{message}'})
          + `<div class="ic-label">Design</div>${designTilesHtml(p.design)}
            <div class="ic-label">Accent</div>${accentPickerHtml(s)}
            <div class="ic-label">Sound</div>${soundControlsHtml(s)}
            <div class="ic-label">Picture</div>${picturePickerHtml(p.image || '', 'pick-picture')}
            <div class="ic-label">On screen for</div>${segHtml(seconds.map(n => [n, n + ' s']), p.seconds || '6', 'set-seconds')}`
          + alertsStatusHtml(true)
          + `<button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="alert-show">${svg('play', ' class="ig-btn-ic"')}Show it on stream</button>`
          + note('Tests never put anything on stream: "Show it on stream" does, with sample values.');
      }
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
      + note('Text compares ignoring case; "is more than" compares numbers.');
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
        + note(`Later steps can use it as ${v('scene')}, ${v('streaming')}, ${v('recording')}${!picked.length ? ''
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
    // An alert's volume slider and any-colour swatch.
    else if ((b === 'volume' || b === 'color') && Array.isArray(this.sel)) this.stepAt(this.sel).params[b] = el.value.trim();
    else if (b === 'p'){
      const s = this.stepAt(this.sel);
      s.params[el.dataset.name] = el.value;
      if (el.dataset.name === 'action' || el.dataset.name === 'wait') nameAnswer(h, s);
      // A goal and a timer don't share names or what can be done to them.
      if (s.type === 'goal' && el.dataset.name === 'what') Object.assign(s.params, {id:'', action:'add', amount:el.value === 'timer' ? '60' : '1'});
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
    const alert = selected();
    if (alert && alert.type === 'overlay' && alertAct(a, el, alert, this.handler(), this)) return true;
    switch (a){
      case 'b-color': { const s = selected(); if (s) s.params.color = el.dataset.v; this.changed(); return true; }
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
    overlay:{title:'', text:'', seconds:'6'}, dock:{text:'', group:''}, edit_text:{input:'', op:'first_line', a:'', b:'', save_as:'text'},
    obs:{action:'scene', scene:'', source:'', text:'', filter:'', hotkey:''},
    light:{device:'', name:'', plug:'', action:'color', color:'red', brightness:''},
    viewer:{value:'coins', who:'', op:'', by:'10'}, math:{a:'', op:'multiply', b:'2', save_as:'result'},
    wait_chat:{text:'', from:'', seconds:'30', save_as:'reply'}, call:{block:''}, reward:{title:'', action:'off'}, channel:{title:'', category:''},
    goal:{what:'', id:(tracking().goals[0] || {}).id || '', action:'add', amount:'1'},
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
    case 'chat_message': {
      const said = t.pattern || '';
      return {exact:`A chat message is "${said}"`, starts_with:`A chat message starts with "${said}"`, word:`A chat message has the word "${said}"`,
        contains:`A chat message contains the letters "${said}"`}[t.mode] || `A chat message matches "${said}"`;
    }
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
    case 'dock': return `Dock ${p.group ? esc(p.group) + ' ' : ''}${q(p.text)}`;
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
    case 'goal': {
      const timer = p.what === 'timer', found = trackersOf(timer ? 'timer' : 'goal').find(x => x.id === p.id);
      const name = q(found ? found.name : p.id || '…'), n = q(p.amount || '1');
      if (timer) return {start:`Start the timer ${name}`, pause:`Pause the timer ${name}`, reset:`Reset the timer ${name}`,
        subtract:`Take ${n} s off the timer ${name}`, set:`Set the timer ${name} to ${n} s`}[p.action] || `Add ${n} s to the timer ${name}`;
      return {subtract:`Take ${n} from the goal ${name}`, set:`Set the goal ${name} to ${n}`, reset:`Reset the goal ${name}`}[p.action] || `Add ${n} to the goal ${name}`;
    }
    case 'reward': return p.action === 'off' ? `Switch off the reward ${q(p.title || '…')}` : `Switch on the reward ${q(p.title || '…')}`;
    case 'channel': return [p.title && `Set the title to ${q(p.title)}`, p.category && `${p.title ? 'the' : 'Set the'} category to ${q(p.category)}`].filter(Boolean).join(', ') || 'Set the title or category';
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

