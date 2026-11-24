// ── Delay transition (System > Extras) ───────────────────────────────────
// A screen for a moment ("Going back 30 s") over the jump back when the
// delay goes up (on or raised), and one ("Going back to the present") over
// the jump forward when it goes down (off or lowered). Each is turned on by
// its own tile and has its own words; they share a style and colours. Their
// pictures come from screen-preview.js.
// `loaded` once the saved settings are in: no pictures of empty words before.
let dtState = {theme:'halo', accent:'#5ac8fa', background:'', shape:'horizontal', screen:'into', loaded:false};
let dtDefaults = {headline:'', subline:'', catch_up_headline:'', catch_up_subline:''};
let dtRefreshTimer = null;
const dtPreview = screenPreview({frame:'s-dt-preview-frame', video:'s-dt-preview-video'});

// Form fields for POST /config.
function dtFields(){
  return {
    'delay_transition.enabled': $('s-dt-enabled').checked ? 'true' : 'false',
    'delay_transition.catch_up': $('s-dt-catch-up').checked ? 'true' : 'false',
    'delay_transition.theme': dtState.theme,
    'delay_transition.accent': dtState.accent,
    'delay_transition.background': dtState.background,
    'delay_transition.headline': $('s-dt-headline').value,
    'delay_transition.subline': $('s-dt-subline').value,
    'delay_transition.catch_up_headline': $('s-dt-cu-headline').value,
    'delay_transition.catch_up_subline': $('s-dt-cu-subline').value,
  };
}
// The delay the screens show: the armed one, turned on next.
function dtDelayMs(){
  return (lastState && lastState.armed_delay_ms) || DT_EXAMPLE_DELAY_MS;
}
// What `screen` (the one previewed, by default) looks like, in the
// preview's crash_protection.* fields. {delay} shows as the armed delay
// going up, and as no delay going down.
function dtLook(screen = dtState.screen){
  const catchUp = screen === 'catch-up';
  const prefix = catchUp ? 's-dt-cu' : 's-dt';
  return {
    'crash_protection.theme': dtState.theme,
    'crash_protection.accent': dtState.accent,
    'crash_protection.background': dtState.background,
    'crash_protection.headline': $(prefix + '-headline').value,
    'crash_protection.subline': $(prefix + '-subline').value,
    delay: String(catchUp ? 0 : dtDelayMs()),
  };
}
// The delay {delay} shows as before any is armed.
const DT_EXAMPLE_DELAY_MS = 30000;
function dtRender(){
  const into = $('s-dt-enabled').checked, catchUp = $('s-dt-catch-up').checked;
  $('s-dt-details').hidden = !into && !catchUp;
  // The preview shows a screen that is on; the switch only when both are.
  if (!into && catchUp) dtState.screen = 'catch-up';
  if (into && !catchUp) dtState.screen = 'into';
  $('s-dt-screens').hidden = !(into && catchUp);
  $('s-dt-stage-label').hidden = into && catchUp;
  document.querySelectorAll('[data-dt-screen]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.dtScreen === dtState.screen)));
  $('s-dt-text').hidden = !into;
  $('s-dt-cu-text').hidden = !catchUp;
  screenMarkTheme($('s-dt-themes'), dtState.theme);
  const theme = cpThemes.find(t => t.id === dtState.theme);
  screenRenderColours('s-dt', dtState, theme && theme.background);
  document.querySelectorAll('[data-dt-shape]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.dtShape === dtState.shape)));
  screenRenderText('s-dt', dtDefaults);
  screenRenderText('s-dt-cu', {headline:dtDefaults.catch_up_headline, subline:dtDefaults.catch_up_subline});
  dtRenderFlow();
  dtSchedulePreview();
}
// Under the preview, what viewers see in order.
function dtRenderFlow(){
  const behind = fmtDelay(dtDelayMs()) + ' behind';
  const goesUp = dtState.screen === 'into';
  setText($('s-dt-flow-before'), goesUp ? 'Live' : behind);
  setText($('s-dt-flow-after'), goesUp ? behind : 'Live');
}
function dtSchedulePreview(){
  clearTimeout(dtRefreshTimer);
  dtRefreshTimer = setTimeout(dtRefresh, 250);
}
// The pictures, and the order viewers see, which follows the armed delay
// (it can change while this card is away).
function dtRefresh(){
  if (!dtState.loaded || !screensShown('transition')) { dtPreview.pause(); return; }
  dtRenderFlow();
  dtDrawArt('s-dt-art-into', 'into');
  dtDrawArt('s-dt-art-catch-up', 'catch-up');
  if ($('s-dt-details').hidden) { dtPreview.pause(); return; }
  const look = dtLook();
  screenDrawThumbs($('s-dt-themes'), look);
  dtPreview.refresh(look, dtState.shape);
}
// A tile's still of its screen, reloaded only when the screen changes.
function dtDrawArt(id, screen){
  const img = $(id);
  const url = screenPreviewUrl({...dtLook(screen), size: 'thumb', phase: '0.3'});
  if (img.dataset.url !== url) { img.dataset.url = url; img.src = url; }
}
function dtApplyConfig(dt){
  if (!dt) return;
  dtDefaults = dt.defaults || dtDefaults;
  $('s-dt-enabled').checked = !!dt.enabled;
  $('s-dt-catch-up').checked = !!dt.catch_up;
  dtState = {...dtState, theme: dt.theme || 'halo', accent: dt.accent || '#5ac8fa', background: dt.background || ''};
  dtState.loaded = true;
  // The same styles the crash card offers.
  screenBuildThemes($('s-dt-themes'), cpThemes, () => dtState.theme, id => { dtState.theme = id; dtRender(); });
  $('s-dt-headline').value = dt.headline || '';
  $('s-dt-subline').value = dt.subline || '';
  $('s-dt-cu-headline').value = dt.catch_up_headline || '';
  $('s-dt-cu-subline').value = dt.catch_up_subline || '';
  dtRender();
}
// Editing a screen's words shows that screen.
function dtShowWhileEditing(id, screen){
  $(id).onfocus = () => { if (dtState.screen !== screen) { dtState.screen = screen; dtRender(); } };
  $(id).oninput = dtRender;
}
$('s-dt-enabled').onchange = dtRender;
$('s-dt-catch-up').onchange = dtRender;
dtShowWhileEditing('s-dt-headline', 'into');
dtShowWhileEditing('s-dt-subline', 'into');
dtShowWhileEditing('s-dt-cu-headline', 'catch-up');
dtShowWhileEditing('s-dt-cu-subline', 'catch-up');
$('s-dt-text-reset').onclick = () => {
  $('s-dt-headline').value = dtDefaults.headline;
  $('s-dt-subline').value = dtDefaults.subline;
  dtState.screen = 'into';
  dtRender();
};
$('s-dt-cu-text-reset').onclick = () => {
  $('s-dt-cu-headline').value = dtDefaults.catch_up_headline;
  $('s-dt-cu-subline').value = dtDefaults.catch_up_subline;
  dtState.screen = 'catch-up';
  dtRender();
};
document.querySelectorAll('[data-dt-screen]').forEach(button => {
  button.onclick = () => { dtState.screen = button.dataset.dtScreen; dtRender(); };
});
document.querySelectorAll('[data-dt-shape]').forEach(button => {
  button.onclick = () => { dtState.shape = button.dataset.dtShape; dtRender(); };
});
$('s-dt-full').onclick = () => screenOpenFull(dtLook(), dtState.shape);
screenBuildColours('s-dt', change => { dtState = {...dtState, ...change}; dtRender(); });

// Whether the saved screens are encoded for the stream yet (from /state).
// A style takes a few seconds to encode for each quality the stream sends;
// a delay changed before then plays a plain style instead. Edits count once
// saved, so this card's unsaved ones say so rather than "ready".
const DT_READY = {
  dirty: 'Save to use your changes',
  ready: 'Ready on stream',
  building: 'Getting ready, a few seconds',
  waiting: 'Gets ready once OBS is live',
};
function dtRenderReady(s){
  const line = $('s-dt-ready');
  const on = $('s-dt-enabled').checked || $('s-dt-catch-up').checked;
  let state = '';
  if (on && dtUnsaved()) state = 'dirty';
  else if (on && s.transition_screens_ready === true) state = 'ready';
  else if (on && s.transition_screens_ready === false) state = 'building';
  else if (on && !s.ingest_alive) state = 'waiting';
  line.dataset.state = state;
  const hint = state === 'building' ? 'A delay changed before then shows a plain style.' : '';
  if (line.title !== tr(hint, line)) line.title = hint;
  setText(line, DT_READY[state] || '');
  line.hidden = !state;
}
// Whether this card's settings differ from the saved ones (not some other
// setting on the tab). The snapshot is read once per save, not per /state.
let dtSaved = {from: null, fields: null};
function dtUnsaved(){
  if (settingsSnapshot === null) return false;
  if (dtSaved.from !== settingsSnapshot) dtSaved = {from: settingsSnapshot, fields: new URLSearchParams(settingsSnapshot)};
  return Object.entries(dtFields()).some(([key, value]) => dtSaved.fields.get(key) !== value);
}
