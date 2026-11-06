// ---------------------------------------------------------------- pane

function stat(i){ return (S.data.stats || {})[i.id] || null; }
function issuesOf(i){ return ((S.data && S.data.issues) || {})[i.id] || []; }
function find(id){ return S.data.integrations.find(i => i.id === id); }

// On, but nothing to talk to: it uses chat, markers or clips while no
// Twitch account is connected.
function needsTwitch(i){
  if (S.data.twitch.main.login) return false;
  return (i.handlers || []).some(h => h.trigger && h.trigger.type.startsWith('chat'))
    || allSteps(i).some(s => ['chat', 'marker', 'clip', 'channel'].includes(s.type));
}

// Run a program and Write a file. With no dashboard password, only the
// streaming PC itself may set them up: anyone on the network could reach
// the dashboard from another device and run anything from chat.
const LOCAL_KINDS = ['program', 'file'];
const LOCAL_LOCK_TEXT = 'Running programs and writing files needs a dashboard password while the dashboard is open to your network (System › Access). Anyone on your network could use them otherwise.';
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
    && (!q || i.name.toLowerCase().includes(q) || (i.folder || '').toLowerCase().includes(q)
      || JSON.stringify(i.handlers).toLowerCase().includes(q)));
}

function skeletonHtml(){
  return `<div class="ig-pane"><div class="tab-head"><div><div class="tab-title">Integrations</div>
    <div class="tab-sub">Loading…</div></div></div>
    <div class="ig-grid" aria-hidden="true">${'<div class="ig-skel"><i></i><b></b><u></u></div>'.repeat(6)}</div></div>`;
}

function render(){
  // The Overlay tab's page follows the same data. While it shows, this tab
  // is hidden and is drawn when it is opened instead.
  if (S.stage){
    Stage.render();
    if (!S.visible) return;
  }
  const root = $('ig-root');
  if (!root || !S.data) return;
  if (S.dragging){ S.redraw = true; return; }
  const d = S.data;
  const on = d.integrations.filter(i => i.enabled).length;
  const failing = d.integrations.filter(i => cardInfo(i).failing);
  const empty = d.integrations.length === 0;
  const sub = empty ? 'Alerts, chat commands and automations.'
    : `<span class="ig-num" data-tick>${on}</span> of ${d.integrations.length} running`
      + (failing.length ? ` · <span class="ig-num" data-tick>${failing.length}</span> ${failing.length === 1 ? 'needs attention.' : 'need attention.'}` : '.');
  morph(root, `<div class="ig-pane" data-flip>
      <div class="tab-head" data-key="head">
        <div><div class="tab-title">Integrations <span class="ig-exp">Experimental</span></div><div class="tab-sub ig-sub">${sub}</div>
          <div class="ig-exp-note">Still experimental: they work, but how they look and how you set them up may still change. Found a bug or have an idea? <a href="${esc(reportUrl())}" target="_blank" rel="noreferrer">Report it</a></div></div>
        <div class="ig-head-actions">
          ${empty ? '' : '<button class="ic-btn ic-btn-ghost" data-act="share">Share</button>'}
          <button class="ic-btn ic-btn-ghost" data-act="values">Your values</button>
          <button class="ic-btn dest-add-btn" data-act="catalog">${svg('plus')}<span>Add</span></button>
        </div>
      </div>
      <div class="ig-notices" data-key="notices" data-flip>${noticesHtml(failing)}</div>
      ${connectionsHtml()}
      ${empty ? emptyHtml() : toolbarHtml() + `<div class="ig-items" data-key="items-${S.view}" data-flip>${itemsHtml()}</div>`}
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
  if (twitchMissing()){
    out.push(`<div class="lan-warn ig-notice" data-key="n-twitch-missing"><span class="lw-ic">⚠</span>
      <span class="ig-notice-text">Log in to Twitch again once to allow <b>${esc(twitchMissing())}</b>: your login is from before InstantClone asked for that.</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="conn" data-tab="twitch">Log in again</button></div>`);
  } else if (consent.length && d.twitch.main.login){
    out.push(`<div class="lan-warn ig-notice" data-key="n-twitch-consent"><span class="lw-ic">⚠</span>
      <span class="ig-notice-text">Twitch needs your OK before InstantClone hears <b>${esc(twitchNouns(consent))}</b>: your login is from before these events.</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="conn" data-tab="twitch">Allow</button></div>`);
  }
  failing.slice(0, 2).forEach(i => {
    const rec = (d.activity || []).find(r => r.integration_id === i.id && r.status === 'failed');
    const step = rec && rec.steps.find(s => s.status === 'failed');
    out.push(`<div class="lan-warn ig-notice" data-key="n-${esc(i.id)}"><span class="lw-ic">⚠</span>
      <span class="ig-notice-text"><b translate="no">${esc(i.name)}</b> ${step ? 'failed: ' + esc(step.detail) : 'is failing'}. Nothing else is affected.</span>
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
    <button class="ic-btn ic-btn-ghost ig-small ig-new-folder" data-act="folder-new" aria-label="New folder" title="New folder, then drag integrations into it">${svg('folder', ' class="ig-btn-ic"')}<span>New folder</span></button>
    <div class="sub-tabs ig-seg ig-views">
      ${view('grid', 'M4 4h7v7H4zM13 4h7v7h-7zM4 13h7v7H4zM13 13h7v7h-7z', 'Grid')}${view('list', 'M4 6h16M4 12h16M4 18h16', 'List')}${SEG_IND}
    </div>
  </div>`;
}

function itemsHtml(){
  const items = visibleIntegrations();
  const none = items.length ? '' : `<div class="ig-none" data-key="none">Nothing here${S.search ? ' matches "' + esc(S.search) + '"' : ' yet'}.</div>`;
  const draw = S.view === 'grid' ? gridHtml : listHtml;
  const folders = folderNames();
  // Shown while a card is dragged: drop it here for a folder of its own.
  const fresh = `<div class="ig-drop-new" data-drop-folder="" data-drop-new data-key="drop-new">${svg('folder')}Drop here for a new folder</div>`;
  if (!folders.length || !items.length) return fresh + draw(items, none, true, '');
  // In folders, A to Z, then the rest. A search shows every match, and
  // only folders with one.
  const groups = folders.map(f => [f, items.filter(i => i.folder === f)]).concat([['', items.filter(i => !i.folder)]])
    .filter(([f, list]) => list.length || (f && !S.search && S.pending.includes(f)));
  const last = groups[groups.length - 1][0];
  return fresh + groups.map(([f, list], n) => {
    const folded = !S.search && S.folded.has(f);
    const body = !list.length ? `<div class="ig-folder-empty" data-drop-folder="${esc(f)}" data-key="ge:${esc(f)}">Drag integrations here</div>`
      : draw(list, '', n === groups.length - 1, f);
    return folderHeadHtml(f, list, folded) + (folded ? '' : body);
  }).join('') + (!S.search && (S.folded.has(last) || !groups[groups.length - 1][1].length) ? draw([], '', true, last) : '');
}

