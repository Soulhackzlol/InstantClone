// - Motion helpers ----------------------------------------
// Content eases in card by card when a tab or sub-tab opens, so the eye
// lands on the first thing and follows down. The first STAGGER_MAX cards
// stagger; the rest come in with the last so long lists never make you wait.
const STAGGER_SELECTOR = '.dcard, .dest-starter, .stat-card, .metric-mini, .obs-section, .overlay-section, .profile-row, '
  + '.sub-pane > .sys-section, .sub-pane > .sys-reset, .sub-pane > .sys-about-grid';
const STAGGER_STEP_MS = 40;
const STAGGER_MAX = 8;
const prefersReducedMotion = () => window.matchMedia('(prefers-reduced-motion: reduce)').matches;
// Something that just opened glides into view once it has grown, when it
// would end below what its scroll box shows: whole when it fits, else its
// top. Never scrolls its top out of sight.
const REVEAL_MARGIN = 16;
const REVEAL_WAIT_MAX = 450;
function revealOpened(el){
  if (!el) return;
  const growing = el.getAnimations({subtree:true})
    .filter(a => a.effect && isFinite(a.effect.getComputedTiming().endTime))
    .map(a => a.finished.catch(() => {}));
  const settled = new Promise(done => setTimeout(done, REVEAL_WAIT_MAX));
  Promise.race([Promise.all(growing), settled]).then(() => {
    if (!el.isConnected) return;
    let box = el.parentElement;
    while (box && !(box.scrollHeight > box.clientHeight && /auto|scroll/.test(getComputedStyle(box).overflowY))) box = box.parentElement;
    const view = box ? box.getBoundingClientRect() : {top:0, bottom:window.innerHeight};
    const r = el.getBoundingClientRect();
    const below = r.bottom + REVEAL_MARGIN - view.bottom;
    const room = r.top - view.top - REVEAL_MARGIN;
    const by = Math.min(below, room);
    if (by > 0) (box || window).scrollBy({top:by, behavior:prefersReducedMotion() ? 'auto' : 'smooth'});
  });
}
// A <details> the user opens shows what it opened. The spring-driven
// .ic-disclose ones are pinned open and reveal from their own toggle.
document.addEventListener('click', (e) => {
  const summary = e.target.closest && e.target.closest('summary');
  const det = summary && summary.parentElement;
  if (!(det instanceof HTMLDetailsElement) || det.open || det.classList.contains('disc-ready')) return;
  setTimeout(() => { if (det.open) revealOpened(det); });
});
// Entrances replayed on demand, and one-shot classes taken off when they end.
function staggerIn(root){
  if (!root || prefersReducedMotion()) return;
  const items = [...root.querySelectorAll(STAGGER_SELECTOR)].filter(el => el.offsetParent !== null);
  items.forEach(el => el.classList.remove('stagger-in'));
  void root.offsetWidth; // one reflow restarts every item's animation
  items.forEach((el, i) => {
    el.style.setProperty('--stagger', Math.min(i, STAGGER_MAX) * STAGGER_STEP_MS + 'ms');
    el.classList.add('stagger-in');
  });
}
// Play a one-shot animation class again even if it's still on.
function restartAnimation(el, cls){
  if (prefersReducedMotion()) return;
  el.classList.remove(cls);
  void el.offsetWidth;
  el.classList.add(cls);
}
// One-shot classes come off when they finish: their end state must not pin
// transforms that hover effects use.
document.addEventListener('animationend', e => {
  if (e.animationName === 'staggerIn') e.target.classList.remove('stagger-in');
  else if (e.animationName === 'wentLive') e.target.classList.remove('went-live');
  else if (e.animationName === 'aurora-bloom') e.target.classList.remove('bloom');
  else if (e.animationName === 'cta-bloom') e.target.classList.remove('cp-cta-bloom');
  else if (e.animationName === 'cap-flash') e.target.classList.remove('flash');
  else if (e.animationName === 'step-deny') e.target.classList.remove('deny');
});
