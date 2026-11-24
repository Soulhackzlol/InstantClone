// ── Screens (System > Extras) ────────────────────────────────────────────
// Both screen editors share one sub-tab, one at a time: the strip on top
// (the same as the OBS tab's app strip) picks Crash protection or the
// Delay transition. The other one is hidden by a class, not `hidden`, so
// Find a setting still reaches it (it skips hidden sections) and opens it
// through screensShow.
const SCREENS = ['crash', 'transition'];

function screensCurrent(){
  const on = document.querySelector('.screens-switch .sub-tab.on');
  return on ? on.dataset.screenTab : SCREENS[0];
}
function screensShow(which){
  if (!SCREENS.includes(which)) which = SCREENS[0];
  document.querySelectorAll('[data-screen]').forEach(section =>
    section.classList.toggle('screen-off', section.dataset.screen !== which));
  document.querySelectorAll('.screens-switch .sub-tab').forEach(tab => {
    const on = tab.dataset.screenTab === which;
    tab.classList.toggle('on', on);
    tab.setAttribute('aria-selected', String(on));
    tab.tabIndex = on ? 0 : -1;
  });
  screensMoveInd();
  try { localStorage.setItem('ic-screen', which); } catch(e){}
  // The one shown draws its pictures; the other rests until it is.
  if (which === 'crash') cpScheduleRefresh(); else dtSchedulePreview();
}
function screensShown(which){
  return !$(which === 'crash' ? 's-cp-section' : 's-dt-section').classList.contains('screen-off');
}
// The indicator slides under the chosen tab. Measured only while shown:
// a hidden strip has no size.
function screensMoveInd(){
  const cur = document.querySelector('.screens-switch .sub-tab.on');
  if (!cur || !cur.offsetWidth) return;
  const ind = cur.parentElement.querySelector('.sub-tab-ind');
  ind.style.left = cur.offsetLeft + 'px';
  ind.style.width = cur.offsetWidth + 'px';
}
// A tab list: Tab reaches the chosen one, the arrow keys (and Home, End)
// move between them.
document.querySelectorAll('.screens-switch .sub-tab').forEach(tab => {
  tab.onclick = () => screensShow(tab.dataset.screenTab);
  tab.onkeydown = e => {
    const index = SCREENS.indexOf(tab.dataset.screenTab);
    const to = {ArrowLeft: index - 1, ArrowRight: index + 1, Home: 0, End: SCREENS.length - 1}[e.key];
    if (to === undefined) return;
    e.preventDefault();
    const which = SCREENS[(to + SCREENS.length) % SCREENS.length];
    screensShow(which);
    document.querySelector('.screens-switch [data-screen-tab="' + which + '"]').focus();
  };
});
window.addEventListener('resize', screensMoveInd);
(() => {
  let saved = null;
  try { saved = localStorage.getItem('ic-screen'); } catch(e){}
  screensShow(saved);
})();
