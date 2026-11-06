// - Tabs: switching panes, the sliding indicator, strips that scroll ---------
let _tabIdx = -1;   // index of the current tab, for slide direction (see showTab)
// A link like /#integrations opens on that tab (the OBS dock uses it).
// /#about is System > About, where the tray's update link leads.
window.addEventListener('load', () => {
  const name = location.hash.slice(1);
  if (name === 'about') openUpdateInAbout();
  else if (name && document.querySelector(`.tab[data-tab="${CSS.escape(name)}"]`)) showTab(name);
  // Once only: a refresh later stays on whatever tab is open then.
  if (name) history.replaceState(null, '', location.pathname + location.search);
});
function showTab(name){
  // Guard unsaved Studio edits: switching away from the Overlay tab hides the
  // editor and abandons the in-progress doc, so confirm first.
  if (name !== 'overlay'){
    const m = $('ov-studio');
    if (m && !m.hidden && window.ICOverlay && Studio.isDirty && Studio.isDirty()){
      if (!confirm('You have unsaved overlay changes. Leave the Studio and discard them?')) return;
    }
  }
  // Direction of travel across the tab strip, so the pane slides in from the
  // side you moved toward. -1 (unseen) on first switch => no slide, just fade.
  const tabEls = $$('.tab');
  const newIdx = tabEls.findIndex(t => t.dataset.tab === name);
  const dir = (_tabIdx < 0 || newIdx < 0) ? 0 : Math.sign(newIdx - _tabIdx);
  if (newIdx >= 0) _tabIdx = newIdx;
  const incoming = document.querySelector(`.tab-pane[data-pane="${name}"]`);
  if (incoming) incoming.style.setProperty('--pane-dx', dir === 0 ? '0px' : (dir > 0 ? '20px' : '-20px'));

  // Capture the card height BEFORE the swap so we can tween it to the new
  // pane's height (panes vary; without this the card snaps taller/shorter).
  const card = document.querySelector('.pane-card');
  const reduceMo = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const cardH0 = (card && !reduceMo) ? card.offsetHeight : null;

  $$('.tab').forEach(t => t.classList.toggle('on', t.dataset.tab === name));
  $$('.tab-pane').forEach(p => p.hidden = p.dataset.pane !== name);
  moveTabInd();
  // A pending MIDI learn belongs to the System tab. Leaving it means the
  // user is not about to press a pad for us, and an armed learn swallows
  // the next press instead of running its action.
  if (name !== 'system'){
    if (hkRec) hkCancelRecording();
    if (midiLearnAction) midiCancelLearn();
  }
  if (name === 'obs') showObsTab();
  if (name === 'logs') pollLogs();
  // Opening Stats: where the counters are now, with no count up to them.
  if (name === 'stats' && lastState){ numState.clear(); paintStats(lastState); }
  if (window.Integrations){ if (name === 'integrations') Integrations.show(); else Integrations.hide(); }
  if (name === 'overlay') overlaySubShown(overlaySub());
  // The Studio modal lives on <body>, so close it when leaving the tab.
  else {
    const m = $('ov-studio'); if (m) m.hidden = true;
    if (window.Integrations) Integrations.stage(null);
  }
  // Sub-tab indicators only have a real width after the pane unhides,
  // so reposition any sub-tab indicator inside the freshly shown pane.
  const pane = document.querySelector(`.tab-pane[data-pane="${name}"]`);
  if (pane){ moveSubTabInd(pane); reserveCopyButtons(pane); }
  fitPaneCard();
  if (name !== 'system') staggerIn(pane);
  // Run the open sub-tab's hooks (VOD callout, update check), which a
  // sub-tab restored at load hasn't had yet.
  if (name === 'system'){
    const current = pane && pane.querySelector('.sub-tab.on');
    showSubTab('system', current ? current.dataset.subtab : 'general');
  }
  // Last, once the pane and its hooks have drawn what they can, so the
  // tween heads for the height the pane really has.
  if (card && cardH0 != null) morphCardHeight(card, cardH0);
}
function moveTabInd(){
  const cur = document.querySelector('.tab.on'); if (!cur) return;
  const ind = $('tab-ind');
  ind.style.left = cur.offsetLeft + 'px';
  ind.style.width = cur.offsetWidth + 'px';
  centerInStrip(cur);
}
// A strip of tabs too wide for a phone scrolls sideways: the picked tab
// slides to its middle, and an edge that hides more tabs fades out.
function centerInStrip(tab){
  const strip = tab.parentElement;
  if (strip.scrollWidth <= strip.clientWidth) return;
  const left = tab.offsetLeft + tab.offsetWidth / 2 - strip.clientWidth / 2;
  strip.scrollTo({left: Math.max(0, left), behavior: prefersReducedMotion() ? 'auto' : 'smooth'});
}
function fadeStripEdges(strip){
  const more = strip.scrollWidth - strip.clientWidth;
  strip.classList.toggle('more-l', more > 1 && strip.scrollLeft > 1);
  strip.classList.toggle('more-r', more > 1 && strip.scrollLeft < more - 1);
}
document.querySelectorAll('.tabs, .sub-tabs, .ic-header-right').forEach(strip => {
  strip.addEventListener('scroll', () => fadeStripEdges(strip), {passive:true});
  new ResizeObserver(() => fadeStripEdges(strip)).observe(strip);
});
// Clicking the tab that's already open does nothing: replaying its enter
// animation reads as a glitch, not feedback. (Code that re-shows the current
// tab on purpose calls showTab directly.)
$$('.tab').forEach(t => t.addEventListener('click', () => {
  if (!t.classList.contains('on')) showTab(t.dataset.tab);
}));
window.addEventListener('resize', moveTabInd);

// - Warm-up: each tab laid out once before its first click -----------------
// A pane never shown before lays its text out for the first time when its
// tab is clicked: a one-off cost (up to ~70 ms for OBS, four times that on a
// slow PC) that lands on the first frame of the slide-in. Once the page is
// idle, every hidden pane and sub-pane is laid out once, out of sight
// (.pane-warm), and hidden again in the same task, so nothing is ever drawn.
const WARM_START_MS = 2000;   // after the page's own entrance has played
const whenIdle = window.requestIdleCallback
  ? fn => requestIdleCallback(fn, {timeout: 3000})
  : fn => setTimeout(fn, 200);
function warmHiddenPanes(){
  const card = $('pane-card');
  // The wizard is up: the panes have no width yet. Ask again later.
  if (!card || card.hidden || !card.clientWidth){ setTimeout(warmHiddenPanes, 5000); return; }
  const panes = $$('.tab-pane');
  const warmNext = () => {
    const pane = panes.shift();
    if (!pane) return;
    warmPane(pane);
    whenIdle(warmNext);
  };
  whenIdle(warmNext);
}
function warmPane(pane){
  const width = $('pane-card').clientWidth;
  const hidden = [pane, ...pane.querySelectorAll('.sub-pane')].filter(el => el.hidden);
  if (!hidden.length || !width) return;
  for (const el of hidden){ el.classList.add('pane-warm'); el.hidden = false; }
  // Out of the flow, at the width it will have: only the outermost ones.
  const outer = hidden.filter(el => !el.parentElement.closest('.pane-warm'));
  for (const el of outer) el.style.width = width + 'px';
  void pane.offsetHeight;
  for (const el of hidden){ el.hidden = true; el.classList.remove('pane-warm'); }
  for (const el of outer) el.style.width = '';
}
