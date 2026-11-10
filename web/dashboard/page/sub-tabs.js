// - Sub-tabs (the System pane and the Overlay pane) -------
// The Overlay tab's pages: the gallery is this file's (Studio), Alerts and
// Goals are drawn by integrations.js, which only reads their data while
// one of them shows.
function overlaySub(){
  const on = document.querySelector('.tab-pane[data-pane="overlay"] .sub-tab.on');
  return on ? on.dataset.subtab : 'delay';
}
function overlaySubShown(sub){
  document.querySelectorAll('[data-ovact]').forEach(el => { el.hidden = el.dataset.ovact !== sub; });
  if ((sub === 'delay' || sub === 'custom' || sub === 'alerts') && window.ICOverlay) Studio.show(sub);
  if (window.Integrations) Integrations.stage(sub);
}
// Jump straight to System -> Access (used by the "set an ingest key" pointer
// on the OBS tab; the key control lives under Access).
function goNetworkSettings(){ showTab('system'); showSubTab('system', 'access'); }
// Sub-tabs that were renamed or merged, so a remembered tab or an old link
// still lands in the right place.
const SUBTAB_MOVED = {behavior:'general', buffer:'general', network:'access', notifications:'alerts', diagnostics:'alerts', overlays:'delay'};

function showSubTab(paneName, subName){
  const pane = document.querySelector(`.tab-pane[data-pane="${paneName}"]`);
  if (!pane) return;
  subName = SUBTAB_MOVED[subName] || subName;
  pane.querySelectorAll('.sub-tab').forEach(t => t.classList.toggle('on', t.dataset.subtab === subName));
  pane.querySelectorAll('.sub-pane').forEach(p => p.hidden = p.dataset.subpane !== subName);
  moveSubTabInd(pane);
  staggerIn(pane.querySelector(`.sub-pane[data-subpane="${subName}"]`));
  try { localStorage.setItem('ic-subtab-' + paneName, subName); } catch(e){}
  if (paneName === 'overlay') overlaySubShown(subName);
  if (paneName === 'obs') obsTabPickApp(subName);
  if (paneName !== 'system') return;
  updateSaveBar();
  if (subName === 'about') aboutMaybeAutoCheck();
  if (subName === 'twitch') refreshSysVodScript();
}
function moveSubTabInd(pane){
  const cur = pane.querySelector('.sub-tab.on'); if (!cur) return;
  const ind = pane.querySelector('.sub-tab-ind'); if (!ind) return;
  ind.style.left = cur.offsetLeft + 'px';
  ind.style.width = cur.offsetWidth + 'px';
  centerInStrip(cur);
}
$$('.sub-tab').forEach(t => {
  t.addEventListener('click', e => {
    const pane = e.currentTarget.closest('.tab-pane');
    if (pane && !e.currentTarget.classList.contains('on')) showSubTab(pane.dataset.pane, e.currentTarget.dataset.subtab);
  });
});
window.addEventListener('resize', () => {
  document.querySelectorAll('.tab-pane:not([hidden])').forEach(moveSubTabInd);
});
// Restore last-active sub-tab per pane on load.
(() => {
  document.querySelectorAll('.tab-pane').forEach(pane => {
    if (!pane.querySelector('.sub-tabs')) return;
    let saved = null;
    try { saved = localStorage.getItem('ic-subtab-' + pane.dataset.pane); } catch(e){}
    saved = SUBTAB_MOVED[saved] || saved;
    if (!saved || !pane.querySelector(`.sub-tab[data-subtab="${saved}"]`)) return;
    // Classes only: this runs while the page script is still loading, so
    // the per-tab hooks in showSubTab wait for the first real switch.
    pane.querySelectorAll('.sub-tab').forEach(t => t.classList.toggle('on', t.dataset.subtab === saved));
    pane.querySelectorAll('.sub-pane').forEach(p => p.hidden = p.dataset.subpane !== saved);
    pane.querySelectorAll('[data-ovact]').forEach(el => { el.hidden = el.dataset.ovact !== saved; });
  });
})();

