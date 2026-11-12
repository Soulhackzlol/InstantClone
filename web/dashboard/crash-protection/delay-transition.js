// ── Delay transition (System > Extras) ───────────────────────────────────
// A screen instead of a jump back when a delay turns on or goes up. Its
// style and words are its own; its colours are the crash screen's, so the
// two screens match. Its pictures come from screen-preview.js.
let dtState = {theme:'halo', shape:'horizontal'};
let dtDefaults = {headline:'', subline:''};
let dtRefreshTimer = null;
const dtPreview = screenPreview({frame:'s-dt-preview-frame', strip:'s-dt-preview', video:'s-dt-preview-video'});

// Form fields for POST /config.
function dtFields(){
  return {
    'delay_transition.enabled': $('s-dt-enabled').checked ? 'true' : 'false',
    'delay_transition.theme': dtState.theme,
    'delay_transition.headline': $('s-dt-headline').value,
    'delay_transition.subline': $('s-dt-subline').value,
  };
}
// What the screen looks like, in the preview's crash_protection.* fields.
function dtLook(){
  return {
    'crash_protection.theme': dtState.theme,
    'crash_protection.accent': cpState.accent,
    'crash_protection.background': cpState.background,
    'crash_protection.headline': $('s-dt-headline').value,
    'crash_protection.subline': $('s-dt-subline').value,
  };
}
function dtRender(){
  $('s-dt-details').hidden = !$('s-dt-enabled').checked;
  screenMarkTheme($('s-dt-themes'), dtState.theme);
  document.querySelectorAll('[data-dt-shape]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.dtShape === dtState.shape)));
  screenRenderText('s-dt', dtDefaults);
  dtSchedulePreview();
}
function dtSchedulePreview(){
  clearTimeout(dtRefreshTimer);
  dtRefreshTimer = setTimeout(dtRefresh, 250);
}
function dtRefresh(){
  if ($('s-dt-details').hidden || !screensShown('transition')) { dtPreview.pause(); return; }
  const look = dtLook();
  screenDrawThumbs($('s-dt-themes'), look);
  const theme = cpThemes.find(t => t.id === dtState.theme);
  dtPreview.refresh(look, dtState.shape, !!(theme && theme.soft));
}
function dtApplyConfig(dt){
  if (!dt) return;
  dtDefaults = dt.defaults || dtDefaults;
  $('s-dt-enabled').checked = !!dt.enabled;
  dtState.theme = dt.theme || 'halo';
  // The same styles the crash card offers.
  screenBuildThemes($('s-dt-themes'), cpThemes, () => dtState.theme, id => { dtState.theme = id; dtRender(); });
  $('s-dt-headline').value = dt.headline || '';
  $('s-dt-subline').value = dt.subline || '';
  dtRender();
}
$('s-dt-enabled').onchange = dtRender;
$('s-dt-headline').oninput = dtRender;
$('s-dt-subline').oninput = dtRender;
$('s-dt-text-reset').onclick = () => {
  $('s-dt-headline').value = dtDefaults.headline;
  $('s-dt-subline').value = dtDefaults.subline;
  dtRender();
};
document.querySelectorAll('[data-dt-shape]').forEach(button => {
  button.onclick = () => { dtState.shape = button.dataset.dtShape; dtRender(); };
});
$('s-dt-full').onclick = () => screenOpenFull(dtLook(), dtState.shape);
// The colours live on the Crash protection card: take the streamer there.
$('s-dt-colours').onclick = () => screensShow('crash');
