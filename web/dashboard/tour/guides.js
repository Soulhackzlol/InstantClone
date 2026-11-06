// - Quick guides -------------------------------------------------------------
// Short walks to one thing at a time, picked from the ? button: each step
// opens the right tab, rings the real control and says one line about it,
// and the trail (flyTrail) flies from each step to the next, so the eye is
// led instead of told. No dark overlay: the page stays usable, so a step
// can be done right there. The full tour is still the first entry.
const GUIDES = [
  {id:'delay', title:'Go live with a delay', steps:[
    {target:'.cp-input-row', text:'Pick how many seconds viewers are behind you.'},
    {target:'#cp-cta', text:'Arm saves video for it while you stay live; once it\'s ready, Activate puts the delay on air.'},
    {target:'#cp-profiles', text:'Delays you use a lot are one click away here.'},
  ]},
  {id:'obs', title:'Connect OBS', steps:[
    {tab:'obs', target:'#obs-reg-card', text:'Close OBS, tick it and press Register with OBS: it gets set to stream here, key included.'},
    {tab:'obs', target:'.obs-dock-card', text:'Optional: add this dock to OBS to run the delay without leaving it.'},
  ]},
  {id:'platform', title:'Stream to your platforms', steps:[
    {tab:'destinations', target:'#dest-add-btn', text:'Add each platform you stream to, with its stream key.'},
    {tab:'destinations', target:'#dest-grid .dcard', text:'Each one has its switch: on goes live with you, off stays out.'},
  ]},
  {id:'alerts', title:'Alerts on stream', steps:[
    {tab:'integrations', target:'.ig-conns [data-tab="twitch"]', text:'Connect Twitch, so InstantClone hears follows, subs and bits.'},
    {tab:'integrations', target:'#ig-root [data-act="catalog"]', text:'Add "Twitch alerts" from the catalog.'},
    {tab:'overlay', sub:'alerts', target:'#ov-alerts .ig-alerts-status', text:'Put the alerts page in OBS with one click. Viewers see them from then on.'},
  ]},
  {id:'vod', title:'Keep music off the VOD', steps:[
    {tab:'system', sub:'twitch', target:'[data-subpane="twitch"] [data-vod-guide]', text:'Three steps, one at a time. Each one ticks as you go.'},
  ]},
  {id:'profiles', title:'Your own delay profiles', steps:[
    {tab:'profiles', target:'.profile-new', text:'Name a delay and save it.'},
    {target:'#cp-profiles', text:'It shows up here: one click arms it.'},
  ]},
];

let guideNow = null;      // {guide, at}
let guideEls = null;      // {ring, note}

function openHelpMenu(btn){
  const old = $('help-menu');
  if (old){ old.remove(); return; }
  const menu = document.createElement('div');
  menu.className = 'help-menu';
  menu.id = 'help-menu';
  menu.setAttribute('role', 'menu');
  menu.innerHTML = `<button role="menuitem" data-guide="tour"><b>Take the tour</b><span>Everything, in a minute</span></button>
    <div class="help-menu-h">Quick guides</div>`
    + GUIDES.map(g => `<button role="menuitem" data-guide="${g.id}">${g.title}<span>${g.steps.length} ${g.steps.length === 1 ? 'step' : 'steps'}</span></button>`).join('');
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
    const b = e.target.closest('[data-guide]');
    if (!b) return;
    shut({type:'keydown', key:'Escape'});
    if (b.dataset.guide === 'tour') openTour(0);
    else startGuide(b.dataset.guide);
  });
}

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
    if (!a) return;
    if (a.dataset.go === 'next') guideStep(guideNow.at + 1);
    else if (a.dataset.go === 'back') guideStep(guideNow.at - 1);
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
  guideEls.ring.remove();
  guideEls.note.remove();
  guideEls = null;
  guideNow = null;
  removeEventListener('resize', placeGuide);
  document.removeEventListener('scroll', placeGuide, true);
  document.removeEventListener('keydown', guideKey);
}

// The step's control, on its tab and sub-tab. Some are drawn a moment after
// their tab opens, so it is looked for a little while; a step whose control
// never shows (nothing added yet) is passed over.
const GUIDE_LOOK_MS = 1200;
async function guideTarget(step){
  if (step.tab) showTab(step.tab);
  if (step.sub) showSubTab(step.tab, step.sub);
  for (let waited = 0; waited <= GUIDE_LOOK_MS; waited += 100){
    const el = document.querySelector(step.target);
    if (el && el.getClientRects().length) return el;
    await new Promise(done => setTimeout(done, 100));
  }
  return null;
}

const GUIDE_SETTLE_MS = 380;
async function guideStep(n){
  const {guide} = guideNow;
  if (n >= guide.steps.length){ stopGuide(); return; }
  if (n < 0) return;
  // The trail starts where the eye is: the note whose button was pressed.
  const was = guideEls.note.classList.contains('in') ? guideEls.note.getBoundingClientRect() : null;
  const dir = n >= guideNow.at ? 1 : -1;
  guideNow.at = n;
  const step = guide.steps[n];
  const target = await guideTarget(step);
  if (!guideNow || guideNow.at !== n) return;   // closed or moved on meanwhile
  if (!target){ guideStep(n + dir); return; }
  guideNow.target = target;
  target.scrollIntoView({block:'center', behavior:motionOK() ? 'smooth' : 'auto'});
  const {note, ring} = guideEls;
  const last = n === guide.steps.length - 1;
  note.innerHTML = `<div class="guide-head"><span class="guide-count">${n + 1} / ${guide.steps.length}</span><b>${guide.title}</b>
      <button type="button" class="ic-btn-tiny" data-go="close" aria-label="Close">✕</button></div>
    <p></p>
    <div class="guide-foot">${n ? '<button type="button" class="ic-btn ic-btn-ghost ig-small" data-go="back">Back</button>' : ''}
      <button type="button" class="ic-btn ic-btn-primary ig-small" data-go="next">${last ? 'Done' : 'Next'}</button></div>`;
  note.querySelector('p').textContent = step.text;
  note.classList.remove('in');
  ring.classList.remove('in');
  // Once the tab has opened and the scroll settled: the trail flies from the
  // last control to this one, then the ring and the note appear on it.
  setTimeout(() => {
    if (!guideNow || guideNow.target !== target) return;
    placeGuide();
    const show = () => { if (guideEls){ ring.classList.add('in'); note.classList.add('in'); note.querySelector('[data-go="next"]').focus({preventScroll:true}); } };
    if (was) flyTrail(was, target, show);
    else show();
  }, motionOK() ? GUIDE_SETTLE_MS : 0);
}

const GUIDE_GAP = 12;
function placeGuide(){
  if (!guideNow || !guideNow.target) return;
  const r = guideNow.target.getBoundingClientRect();
  const {ring, note} = guideEls;
  Object.assign(ring.style, {left:`${r.left - 6}px`, top:`${r.top - 6}px`, width:`${r.width + 12}px`, height:`${r.height + 12}px`});
  const w = note.offsetWidth, h = note.offsetHeight;
  const below = r.bottom + GUIDE_GAP + h < innerHeight;
  const top = below ? r.bottom + GUIDE_GAP : Math.max(GUIDE_GAP, r.top - GUIDE_GAP - h);
  const left = Math.min(Math.max(GUIDE_GAP, r.left), innerWidth - w - GUIDE_GAP);
  Object.assign(note.style, {left:`${left}px`, top:`${top}px`});
}
