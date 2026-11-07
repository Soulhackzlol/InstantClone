// - Quick guides -------------------------------------------------------------
// Walks to one thing at a time, picked from the ? button and kept in folders.
// Each step opens the right tab, rings the real control and says one line
// about it, and the trail (flyTrail) flies from each step to the next, so the
// eye is led instead of told. No dark overlay: the page stays usable, so a
// step can be done right there.
//
// A step: {tab, sub, target, text, act, wait, skip, modal}
//   target  a selector, or a function that finds the element
//   text    a line, or a function that picks one for how things stand now
//   act     {label, run}: a button that does the step for you
//   wait    a check: the step waits for you to do it, then moves on alone
//   skip    a check: the step is passed over when it's already done
//   modal   the target is in a dialog (otherwise an open dialog closes first)
// A guide's title can be a function too, and `suggest` says when the menu
// should point it out: what you haven't set up yet.

// What the Integrations guides look at, through Integrations.guide. That
// script loads after this one, so each check asks for it when it runs.
const igGuide = () => (window.Integrations && Integrations.guide) || null;
const igTwitchOn = () => !!(igGuide() && igGuide().twitchOn());
const igDiscordOn = () => !!(igGuide() && igGuide().discordOn());
const igAlerts = () => igGuide() && igGuide().alerts();
const igPhoneOn = () => !!(igGuide() && igGuide().phoneOn());
const igCount = () => (igGuide() ? igGuide().count() : 0);
const igAmountMoment = () => igGuide() && igGuide().amountMoment();
const guideShown = sel => { const el = document.querySelector(sel); return !!(el && el.getClientRects().length); };
const guideClick = sel => { const el = document.querySelector(sel); if (el) el.click(); };
// The catalog, opened on a category or a search.
function guideCatalog({cat, query} = {}){
  if (!guideShown('.ig-cat')) guideClick('#ig-root [data-act="catalog"]');
  setTimeout(() => {
    if (cat) guideClick(`.ig-cat-nav [data-cat="${cat}"]`);
    const find = $('ig-cat-search');
    if (query && find){ find.value = query; find.dispatchEvent(new Event('input', {bubbles:true})); }
  }, 120);
}

const IG = {tab:'integrations'};
const IN_MODAL = {modal:true};
const GUIDE_FOLDERS = [
  {id:'start', title:'Getting started', guides:[
    {id:'delay', title:'Go live with a delay', steps:[
      {target:'.cp-input-row', text:'Pick how many seconds viewers are behind you.'},
      {target:'#cp-cta', text:'Press Arm: InstantClone starts saving video while you stay live. Once enough is saved, press Activate and viewers see you that many seconds late.'},
      {target:'#cp-profiles', text:'Delays you use a lot are one click away here.'},
    ]},
    {id:'obs', title:'Connect OBS', steps:[
      {tab:'obs', target:'#obs-reg-card', text:'Close OBS, tick your OBS in the list and press Register with OBS. OBS is then set to stream here, key included.'},
      {tab:'obs', target:'.obs-dock-card', text:'Optional: add this dock to OBS to run the delay without leaving it.'},
    ]},
    {id:'platform', title:'Stream to your platforms', steps:[
      {tab:'destinations', target:'#dest-add-btn', text:'Add each platform you stream to, with its stream key.'},
      {tab:'destinations', target:'#dest-grid .dcard', text:'Each platform has a switch: on, it goes live when you do; off, it is left out.'},
    ]},
    {id:'profiles', title:'Your own delay profiles', steps:[
      {tab:'profiles', target:'.profile-new', text:'Name a delay and save it.'},
      {target:'#cp-profiles', text:'It shows up here: one click arms it.'},
    ]},
    {id:'vod', title:'Keep music off the VOD', steps:[
      {tab:'system', sub:'twitch', target:'[data-subpane="twitch"] [data-vod-guide]', text:'Do these three steps in order. Each one gets a tick when it is done.'},
    ]},
  ]},
  {id:'integrations', title:'Integrations', guides:[
    {id:'ig-first', title:() => igCount() ? 'Add another integration' : 'Your first integration', suggest:() => !igCount(), steps:[
      {...IG, target:'.ig-conns', skip:() => igCount() > 0,
        text:'These are the accounts integrations use. Green is connected; a pill that says Connect still needs you.'},
      {...IG, target:'#ig-root [data-act="catalog"]',
        text:() => igCount() ? 'New ones come from the catalog, like the ones you have.' : 'Every integration starts in the catalog.',
        act:{label:'Open it', run:() => guideCatalog()}, wait:() => guideShown('.ig-cat')},
      {...IN_MODAL, target:'.ig-cat-nav', text:'Browse by what they do: alerts, chat, automation, or ready-made packs.'},
      {...IN_MODAL, target:'.ig-cat-find', text:'Or search for what you want, like "clip", "raid" or "Discord".'},
      {...IN_MODAL, target:'.ig-cat-grid .ig-cat-tile', text:'Press Add on any of them. It opens already filled in, so you only change what you want.',
        wait:() => guideShown('.ig-ed-body')},
      {...IN_MODAL, target:'.ig-ed-body .ig-ed-sect', text:'Section 1 says when it runs. If it reacts to several things, each one has its own on/off switch.'},
      {...IN_MODAL, target:'.ig-ed-body .ig-opt', text:'Options are rows: click one to change it. A row in orange still needs something from you.'},
      {...IN_MODAL, target:'.dest-form-foot [data-act="ed-test"], .dest-form-foot [data-act="alert-show"]',
        text:'Try it before you save: it runs once, for real.'},
      {...IN_MODAL, target:'.dest-form-foot [data-act="save"]', text:'Save turns it on. Changed something by mistake? Ctrl+Z undoes it.'},
    ]},
    {id:'ig-accounts', title:'Connect your accounts', suggest:() => !igTwitchOn(), steps:[
      {...IG, target:'.ig-conns [data-tab="twitch"]',
        text:() => igTwitchOn() ? 'Twitch is connected: chat commands, clips, VOD markers and alerts can use it.'
          : 'Connect Twitch first: most integrations hear your chat, follows, subs and bits through it.'},
      {...IG, target:'.ig-conns [data-tab="discord"]',
        text:() => igDiscordOn() ? 'Discord is ready: integrations post in your channels. Add more channels here.'
          : 'Discord: add a channel and integrations can post there, even cards that update live.'},
      {...IG, target:'.ig-conns [data-tab="phone"]',
        text:() => igPhoneOn() ? 'Your phone is connected: pushes reach it through the ntfy app.'
          : 'Phone: pushes through the free ntfy app, for when OBS crashes and you\'re away from the PC.'},
      {...IG, target:'.ig-conns [data-tab="obs"]', text:'OBS: lets integrations switch scenes, show sources and react to recording. The dashboard connects it for you.'},
      {...IG, target:'.ig-conns [data-tab="stream"]', text:'Alerts page: where on-stream alerts show up. Add it to OBS once.'},
    ]},
    {id:'ig-discord', title:() => igDiscordOn() ? 'More Discord posts' : 'Post to Discord', steps:[
      {...IG, target:'.ig-conns [data-tab="discord"]',
        text:() => igDiscordOn() ? 'You have a Discord channel. Add more to post in other rooms, like one for mods or clips.'
          : 'First, add the Discord channel to post in.',
        act:{label:'Open it', run:() => guideClick('.ig-conns [data-tab="discord"]')}, wait:() => guideShown('[data-act="d-new"]')},
      {...IN_MODAL, target:'[data-act="d-new"]', skip:() => !guideShown('[data-act="d-new"]'),
        text:'In Discord: Edit Channel, Integrations, Webhooks, New Webhook. Copy its URL and paste it here.'},
      {...IG, target:'#ig-root [data-act="catalog"]', text:'Then pick what to post: go-live notices, clips, a stream timeline that writes itself.',
        act:{label:'Show them', run:() => guideCatalog({query:'Discord'})}, wait:() => guideShown('.ig-ed-body')},
      {...IN_MODAL, target:'.ig-ed-body [data-chip="where"]', skip:() => !guideShown('.ig-ed-body [data-chip="where"]'),
        text:'Post in picks the channel. Each integration can use a different one.'},
    ]},
    {id:'ig-chat', title:'Chat commands', steps:[
      {...IG, target:'.ig-toolbar [data-f="chat"]', text:'Chat integrations answer commands like !delay or !discord, and react to channel point rewards.'},
      {...IG, target:'#ig-root [data-act="catalog"]', text:'The catalog has ready ones.',
        act:{label:'Show them', run:() => guideCatalog({cat:'chat'})}, wait:() => guideShown('.ig-cat')},
      {...IN_MODAL, target:'.ig-cat-grid .ig-cat-tile', text:'Each one shows what a viewer types and what comes back. Add one and change its words.',
        wait:() => guideShown('.ig-ed-body')},
      {...IN_MODAL, target:'.ig-ed-body .ig-opt', text:'Here you choose who can use it and how often, for example mods only or once a minute.'},
    ]},
    {id:'ig-levels', title:'Bigger alerts for big amounts', suggest:() => !!igAlerts(), steps:[
      {...IG, target:() => { const a = igAlerts(); return a ? document.querySelector(`[data-drag-id="${CSS.escape(a.id)}"]`) : $('ig-root').querySelector('[data-act="catalog"]'); },
        text:() => igAlerts() ? 'Open your Twitch alerts.' : 'Add "Twitch alerts" from the catalog first.',
        act:{label:'Open', run:() => { const a = igAlerts(); if (a) igGuide().open(a); else guideCatalog({query:'Twitch alerts'}); }},
        wait:() => guideShown('.ig-ed-body')},
      {...IN_MODAL, target:igAmountMoment, text:'Levels work on alerts with an amount: bits, resub months, gifted subs and raids. Pick one.',
        act:{label:'Pick it', run:() => { const m = igAmountMoment(); if (m) m.click(); }}, wait:() => guideShown('.ig-ed-body [data-chip="tiers"]')},
      {...IN_MODAL, target:'.ig-ed-body [data-chip="tiers"]', text:'Open the Levels row.',
        act:{label:'Open it', run:() => guideClick('.ig-ed-body [data-chip="tiers"]')}, wait:() => guideShown('.ig-tiers')},
      {...IN_MODAL, target:'.ig-tiers [data-act="tier-add"]', skip:() => !guideShown('.ig-tiers [data-act="tier-add"]'),
        text:'Add a level, then type from how many it plays, like 1000 bits.', wait:() => guideShown('.ig-tier-from')},
      {...IN_MODAL, target:'.ig-tier.on', text:'Edit chooses which level you are changing. Each level has its own design, sound and text. Send a test to see it.'},
    ]},
    {id:'ig-build', title:'Build your own', steps:[
      {...IG, target:'#ig-root [data-act="catalog"]', text:'Nothing in the catalog does it? Build your own.',
        act:{label:'Open the catalog', run:() => guideCatalog()}, wait:() => guideShown('.ig-cat')},
      {...IN_MODAL, target:'.ig-cat-acts [data-act="build"]', text:'Build your own starts from scratch. Import a recipe loads one someone shared with you.',
        wait:() => guideShown('.ig-modal .dest-form-foot [data-act="save"]') && !guideShown('.ig-cat')},
      {...IN_MODAL, target:'.ig-modal .dest-form-body', text:'Pick what starts it (a chat command, an event, a key), then add the steps it runs, in order.'},
      {...IN_MODAL, target:'.ig-modal [data-act="b-test"]', skip:() => !guideShown('.ig-modal [data-act="b-test"]'),
        text:'Test runs it once, so you see each step work before you save.'},
    ]},
    {id:'ig-organize', title:'Folders, search and undo', suggest:() => igCount() >= 8, steps:[
      {...IG, target:'#ig-search', text:'Search your integrations by name.'},
      {...IG, target:'.ig-toolbar .ig-seg', text:'Or filter by kind: alerts, chat, automation, or the ones you built.'},
      {...IG, target:'.ig-toolbar [data-act="folder-new"]', text:'Make a folder, then drag cards into it. While you drag, a drop zone floats at the top for a new one.'},
      {...IG, target:'#ig-root .ig-card', skip:() => !guideShown('#ig-root .ig-card'),
        text:'Each card has its switch. Moved, removed or switched one by mistake? Ctrl+Z undoes it.'},
    ]},
  ]},
  {id:'stream', title:'On stream', guides:[
    {id:'alerts', title:() => igAlerts() ? 'Your alerts on stream' : 'Alerts on stream', suggest:() => !igAlerts(), steps:[
      {...IG, target:'.ig-conns [data-tab="twitch"]', text:'Connect Twitch, so InstantClone hears follows, subs and bits.', skip:igTwitchOn},
      {...IG, target:'#ig-root [data-act="catalog"]', text:'Add "Twitch alerts" from the catalog.', skip:() => !!igAlerts(),
        act:{label:'Find it', run:() => guideCatalog({query:'Twitch alerts'})}},
      {tab:'overlay', sub:'alerts', target:'#ov-alerts .ig-alerts-status', text:'Put the alerts page in OBS with one click. Viewers see them from then on.'},
    ]},
    {id:'goals', title:'Goals and subathon timers', steps:[
      {tab:'overlay', sub:'goals', target:'#ov-goals', text:'Goals and timers count by themselves: follows, subs and bits move them.'},
      {tab:'overlay', sub:'goals', target:'#ov-goals .ovg-primary', skip:() => !guideShown('#ov-goals .ovg-primary'),
        text:'Press Put it on stream and it becomes an overlay you can restyle in the Studio.'},
    ]},
  ]},
];
const GUIDES = GUIDE_FOLDERS.flatMap(f => f.guides);
const guideTitle = g => typeof g.title === 'function' ? g.title() : g.title;
// Suggested: not done yet, and worth it for how things stand now.
const guideSuggested = (g, done) => !done.includes(g.id) && !!(g.suggest && g.suggest());

// - Menu: the tour, then a folder per subject, ticks on the guides done -----
const GUIDES_DONE_KEY = 'instantclone_guides_done';
const GUIDES_OPEN_KEY = 'instantclone_guides_open';
function guidesDone(){ try { return JSON.parse(localStorage.getItem(GUIDES_DONE_KEY)) || []; } catch { return []; } }
function markGuideDone(id){
  const done = guidesDone();
  if (done.includes(id)) return;
  try { localStorage.setItem(GUIDES_DONE_KEY, JSON.stringify(done.concat(id))); } catch {}
}
function openFolder(){ try { return localStorage.getItem(GUIDES_OPEN_KEY); } catch { return null; } }

function helpMenuHtml(){
  const done = guidesDone();
  // With nothing remembered, the folder with a suggestion opens first.
  const hinted = GUIDE_FOLDERS.find(f => f.guides.some(g => guideSuggested(g, done)));
  const open = openFolder() || (hinted ? hinted.id : 'start');
  const folder = f => {
    const n = f.guides.filter(g => done.includes(g.id)).length, on = f.id === open;
    const hint = f.guides.some(g => guideSuggested(g, done)) ? '<i class="help-hint-dot" aria-hidden="true"></i>' : '';
    return `<div class="help-folder${on ? ' open' : ''}">
      <button class="help-folder-h" data-folder="${f.id}" aria-expanded="${on}">${f.title}${hint}<span>${n} / ${f.guides.length}</span></button>
      <div class="help-folder-b"><div>${f.guides.map(g => `<button role="menuitem" data-guide="${g.id}"${on ? '' : ' tabindex="-1"'}>
        <i class="help-tick${done.includes(g.id) ? ' on' : ''}" aria-hidden="true"></i>${guideTitle(g)}${guideSuggested(g, done) ? '<em class="help-hint">Suggested</em>' : ''}
        <span>${g.steps.length} ${g.steps.length === 1 ? 'step' : 'steps'}</span></button>`).join('')}</div></div>
    </div>`;
  };
  return `<button role="menuitem" data-guide="tour"><b>Take the tour</b><span>Everything, in a minute</span></button>
    <div class="help-menu-h">Guides</div>${GUIDE_FOLDERS.map(folder).join('')}`;
}

function openHelpMenu(btn){
  const old = $('help-menu');
  if (old){ old.remove(); return; }
  const menu = document.createElement('div');
  menu.className = 'help-menu';
  menu.id = 'help-menu';
  menu.setAttribute('role', 'menu');
  menu.innerHTML = helpMenuHtml();
  document.body.appendChild(menu);
  const r = btn.getBoundingClientRect();
  menu.style.top = `${r.bottom + 8}px`;
  menu.style.right = `${Math.max(8, innerWidth - r.right)}px`;
  menu.querySelector('button').focus();
  const shut = e => {
    if (e.type === 'keydown' && e.key !== 'Escape') return;
    if (e.type === 'pointerdown' && (menu.contains(e.target) || btn.contains(e.target))) return;
    menu.remove();
    document.removeEventListener('pointerdown', shut, true);
    document.removeEventListener('keydown', shut, true);
  };
  document.addEventListener('pointerdown', shut, true);
  document.addEventListener('keydown', shut, true);
  menu.addEventListener('click', e => {
    const head = e.target.closest('[data-folder]');
    if (head){ toggleFolder(menu, head); return; }
    const b = e.target.closest('[data-guide]');
    if (!b) return;
    shut({type:'keydown', key:'Escape'});
    if (b.dataset.guide === 'tour') openTour(0);
    else startGuide(b.dataset.guide);
  });
}
// One folder open at a time; the menu remembers which.
function toggleFolder(menu, head){
  const id = head.dataset.folder;
  menu.querySelectorAll('.help-folder').forEach(f => {
    const h = f.querySelector('[data-folder]'), on = h === head && !f.classList.contains('open');
    f.classList.toggle('open', on);
    h.setAttribute('aria-expanded', on);
    f.querySelectorAll('[data-guide]').forEach(b => { if (on) b.removeAttribute('tabindex'); else b.tabIndex = -1; });
  });
  try { localStorage.setItem(GUIDES_OPEN_KEY, head.getAttribute('aria-expanded') === 'true' ? id : ''); } catch {}
}

// - Walking a guide ------------------------------------------------------------
let guideNow = null;      // {guide, at, target, waiting}
let guideEls = null;      // {ring, note}

function startGuide(id){
  const guide = GUIDES.find(g => g.id === id);
  if (!guide) return;
  if (!$('tour').hidden) closeTour();
  stopGuide();
  const ring = document.createElement('div');
  ring.className = 'guide-ring';
  const note = document.createElement('div');
  note.className = 'guide-note';
  note.setAttribute('role', 'dialog');
  note.setAttribute('aria-live', 'polite');
  document.body.append(ring, note);
  guideEls = {ring, note};
  guideNow = {guide, at:-1};
  note.addEventListener('click', e => {
    const a = e.target.closest('[data-go]');
    if (!a || !guideNow) return;
    const step = guideNow.guide.steps[guideNow.at];
    if (a.dataset.go === 'next') guideStep(guideNow.at + 1);
    else if (a.dataset.go === 'back') guideStep(guideNow.at - 1);
    else if (a.dataset.go === 'act' && step.act) step.act.run();
    else stopGuide();
  });
  addEventListener('resize', placeGuide);
  document.addEventListener('scroll', placeGuide, true);
  document.addEventListener('keydown', guideKey);
  guideStep(0);
}

function guideKey(e){
  if (e.key === 'Escape' && guideNow) stopGuide();
}

function stopGuide(){
  if (!guideEls) return;
  clearInterval(guideNow && guideNow.waiting);
  guideEls.ring.remove();
  guideEls.note.remove();
  guideEls = null;
  guideNow = null;
  removeEventListener('resize', placeGuide);
  document.removeEventListener('scroll', placeGuide, true);
  document.removeEventListener('keydown', guideKey);
}

const guideFind = target => typeof target === 'function' ? target() : document.querySelector(target);
const guideText = text => typeof text === 'function' ? text() : text;

// The step's control, on its tab and sub-tab. Some are drawn a moment after
// their tab or dialog opens, so it is looked for a little while; a step whose
// control never shows (nothing added yet) is passed over.
const GUIDE_LOOK_MS = 1200;
async function guideTarget(step){
  if (!step.modal && document.querySelector('.ic-modal') && igGuide()) igGuide().closeDialog();
  if (step.tab) showTab(step.tab);
  if (step.sub) showSubTab(step.tab, step.sub);
  for (let waited = 0; waited <= GUIDE_LOOK_MS; waited += 100){
    const el = guideFind(step.target);
    if (el && el.getClientRects().length) return el;
    await new Promise(done => setTimeout(done, 100));
  }
  return null;
}

// The trail waits for its target to stop moving (a tab opening, a smooth
// scroll, a dialog growing), so it lands where the control ends up.
const GUIDE_STILL_FRAMES = 4;
const GUIDE_STILL_MAX_MS = 1100;
function untilStill(el){
  return new Promise(done => {
    if (!motionOK()){ done(); return; }
    const start = performance.now();
    let last = '', still = 0;
    const look = () => {
      const r = el.getBoundingClientRect(), now = `${Math.round(r.left)},${Math.round(r.top)},${Math.round(r.width)}`;
      still = now === last ? still + 1 : 0;
      last = now;
      if (still >= GUIDE_STILL_FRAMES || performance.now() - start > GUIDE_STILL_MAX_MS) done();
      else requestAnimationFrame(look);
    };
    requestAnimationFrame(look);
  });
}
const GUIDE_POLL_MS = 300;
const GUIDE_DONE_PAUSE_MS = 450;
async function guideStep(n){
  const {guide} = guideNow;
  clearInterval(guideNow.waiting);
  if (n >= guide.steps.length){ markGuideDone(guide.id); stopGuide(); return; }
  if (n < 0) return;
  // The trail starts where the eye is: the note whose button was pressed,
  // or for the first step, the ? button the guide was picked from.
  const was = guideEls.note.classList.contains('in') ? guideEls.note.getBoundingClientRect()
    : n === 0 && $('hdr-help') ? $('hdr-help').getBoundingClientRect() : null;
  const dir = n >= guideNow.at ? 1 : -1;
  guideNow.at = n;
  const step = guide.steps[n];
  if (step.skip && step.skip()){ guideStep(n + dir); return; }
  const target = await guideTarget(step);
  if (!guideNow || guideNow.at !== n) return;   // closed or moved on meanwhile
  if (!target){ guideStep(n + dir); return; }
  guideNow.target = target;
  target.scrollIntoView({block:'center', behavior:motionOK() ? 'smooth' : 'auto'});
  const {note, ring} = guideEls;
  note.innerHTML = guideNoteHtml(guide, n);
  note.querySelector('p').textContent = guideText(step.text);
  note.classList.remove('in');
  ring.classList.remove('in', 'done');
  if (step.wait) guideWait(step, n);
  // Once the target has settled: the trail flies from the last place the eye
  // was to it, and the ring and the note appear as it lands.
  await untilStill(target);
  if (!guideNow || guideNow.target !== target) return;
  placeGuide();
  const show = () => {
    if (!guideEls) return;
    ring.classList.add('in');
    note.classList.add('in');
    const first = note.querySelector('.guide-foot .ic-btn-primary') || note.querySelector('.guide-foot button');
    if (first) first.focus({preventScroll:true});
  };
  if (was) flyTrail(was, target, show);
  else show();
}

function guideNoteHtml(guide, n){
  const step = guide.steps[n], last = n === guide.steps.length - 1;
  const act = step.act ? `<button type="button" class="ic-btn ic-btn-primary ig-small" data-go="act">${step.act.label}</button>` : '';
  const next = step.wait && !last
    ? `<button type="button" class="ic-btn ic-btn-ghost ig-small" data-go="next">Skip</button>`
    : `<button type="button" class="ic-btn ${act ? 'ic-btn-ghost' : 'ic-btn-primary'} ig-small" data-go="next">${last ? 'Done' : 'Next'}</button>`;
  const dots = guide.steps.map((_, i) => `<i class="${i < n ? 'was' : i === n ? 'now' : ''}"></i>`).join('');
  return `<div class="guide-head"><span class="guide-count">${n + 1} / ${guide.steps.length}</span><b>${guideTitle(guide)}</b>
      <button type="button" class="ic-btn-tiny" data-go="close" aria-label="Close">✕</button></div>
    <div class="guide-dots" aria-hidden="true">${dots}</div>
    <p></p>${step.wait ? '<div class="guide-wait"><span class="guide-wait-dot"></span>Waiting for you to do it</div>' : ''}
    <div class="guide-foot">${n ? '<button type="button" class="ic-btn ic-btn-ghost ig-small" data-go="back">Back</button>' : ''}${act}${next}</div>`;
}

// A step you do yourself: once it's done, the ring says so and the guide
// moves on by itself.
function guideWait(step, n){
  guideNow.waiting = setInterval(() => {
    if (!guideNow || guideNow.at !== n){ return; }
    if (!step.wait()) return;
    clearInterval(guideNow.waiting);
    guideEls.ring.classList.add('done');
    const line = guideEls.note.querySelector('.guide-wait');
    if (line){ line.classList.add('ok'); line.lastChild.textContent = 'Done'; }
    setTimeout(() => { if (guideNow && guideNow.at === n) guideStep(n + 1); }, GUIDE_DONE_PAUSE_MS);
  }, GUIDE_POLL_MS);
}

const GUIDE_GAP = 12;
function placeGuide(){
  if (!guideNow || !guideNow.target) return;
  // A target redrawn under the guide (a dialog re-rendered) is found again.
  if (!guideNow.target.isConnected){
    const step = guideNow.guide.steps[guideNow.at], again = guideFind(step.target);
    if (!again) return;
    guideNow.target = again;
  }
  const r = guideNow.target.getBoundingClientRect();
  const {ring, note} = guideEls;
  Object.assign(ring.style, {left:`${r.left - 6}px`, top:`${r.top - 6}px`, width:`${r.width + 12}px`, height:`${r.height + 12}px`});
  const w = note.offsetWidth, h = note.offsetHeight;
  const below = r.bottom + GUIDE_GAP + h < innerHeight;
  const top = below ? r.bottom + GUIDE_GAP : Math.max(GUIDE_GAP, r.top - GUIDE_GAP - h);
  const left = Math.min(Math.max(GUIDE_GAP, r.left), innerWidth - w - GUIDE_GAP);
  Object.assign(note.style, {left:`${left}px`, top:`${top}px`});
}
// Dialogs redraw as you work in them: the ring follows its control.
setInterval(() => { if (guideNow && guideEls && guideEls.ring.classList.contains('in')) placeGuide(); }, 250);
