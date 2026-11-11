// ── Crash protection card ────────────────────────────────────────────────
// Every picture comes from /crash-protection/preview, drawn by the Rust
// renderer that also encodes the stream. Unsaved edits ride along in the
// query, so the preview and the style thumbnails follow every change
// before Save. The style list and default text come from GET /config.
const CP_ACCENTS = [['Cyan','#5ac8fa'],['Violet','#9d7bff'],['Lime','#7ee06b'],['Coral','#ff6b6b'],['Amber','#ffb73a']];
// Frames in the preview strip; the CSS animation steps through the same 24.
// Every style moves in 6, 8 or 12 steps per loop, and 24 is a multiple of
// each, so the preview shows every step at the moment the stream does.
const CP_PREVIEW_FRAMES = 24;
let cpState = {theme:'whisper', accent:'#5ac8fa', background:'', shape:'horizontal'};
let cpThemes = [];
let cpDefaults = {headline:'', subline:''};
let cpRefreshTimer = null;
// What the thumbnails were last drawn with. Picking a style or a shape
// doesn't change them, so only colour and text edits reload them.
let cpThumbKey = '';
// Bumped per refresh, so a strip that loads after a newer one is dropped.
let cpGeneration = 0;
let cpStripUrl = '';
let cpVideoUrl = '';
// Whether the preview is on screen: the video only plays while it is.
let cpPreviewSeen = true;
const cpStill = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;

function cpInit(){
  const custom = $('s-cp-accent').parentElement;
  CP_ACCENTS.forEach(([name, hex]) => {
    const swatch = document.createElement('button');
    swatch.type = 'button';
    swatch.className = 'crash-swatch';
    swatch.style.background = hex;
    swatch.dataset.hex = hex;
    swatch.title = name;
    swatch.setAttribute('aria-label', name);
    swatch.onclick = () => cpSetAccent(hex);
    $('s-cp-swatches').insertBefore(swatch, custom);
  });
  $('s-cp-accent').oninput = e => cpSetAccent(e.target.value);
  $('s-cp-bg').oninput = e => { cpState.background = e.target.value.toLowerCase(); cpRender(); };
  $('s-cp-bg-reset').onclick = () => { cpState.background = ''; cpRender(); };
  $('s-cp-text-reset').onclick = () => {
    $('s-cp-headline').value = cpDefaults.headline;
    $('s-cp-subline').value = cpDefaults.subline;
    cpRenderText();
  };
  $('s-cp-headline').oninput = cpRenderText;
  $('s-cp-subline').oninput = cpRenderText;
  $('s-cp-hold').oninput = cpRenderHold;
  document.querySelectorAll('[data-cp-shape]').forEach(button => {
    button.onclick = () => { cpState.shape = button.dataset.cpShape; cpRender(); };
  });
}
// One card per style, a radio group: arrow keys move the choice.
function cpBuildThemes(){
  const box = $('s-cp-themes');
  // Fresh cards have empty thumbnails, whatever the last refresh drew.
  cpThumbKey = '';
  box.replaceChildren(...cpThemes.map(theme => {
    const card = document.createElement('button');
    card.type = 'button';
    card.className = 'crash-theme';
    card.setAttribute('role', 'radio');
    card.dataset.cpTheme = theme.id;
    const thumb = document.createElement('img');
    thumb.alt = '';
    const name = document.createElement('span');
    name.textContent = theme.name;
    card.append(thumb, name);
    card.onclick = () => { cpState.theme = theme.id; cpRender(); };
    return card;
  }));
  box.onkeydown = e => {
    const step = {ArrowRight:1, ArrowDown:1, ArrowLeft:-1, ArrowUp:-1}[e.key];
    if (!step) return;
    e.preventDefault();
    const index = cpThemes.findIndex(t => t.id === cpState.theme);
    cpState.theme = cpThemes[(index + step + cpThemes.length) % cpThemes.length].id;
    cpRender();
    updateSaveBar();
    box.querySelector('[data-cp-theme="' + cpState.theme + '"]').focus();
  };
}
function cpSetAccent(hex){ cpState.accent = hex.toLowerCase(); cpRender(); }
function cpSyncVisibility(){
  $('s-cp-details').hidden = !$('s-cp-enabled').checked;
  cpScheduleRefresh();
}
// Reflect cpState in the controls, then refresh the pictures.
function cpRender(){
  document.querySelectorAll('#s-cp-themes .crash-theme').forEach(card => {
    const on = card.dataset.cpTheme === cpState.theme;
    card.setAttribute('aria-checked', String(on));
    card.tabIndex = on ? 0 : -1;
  });
  document.querySelectorAll('#s-cp-swatches .crash-swatch').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.hex === cpState.accent)));
  document.querySelectorAll('[data-cp-shape]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.cpShape === cpState.shape)));
  $('s-cp-preview-frame').classList.toggle('vertical', cpState.shape === 'vertical');
  $('s-cp-accent').value = cpState.accent;
  const theme = cpThemes.find(t => t.id === cpState.theme);
  $('s-cp-bg').value = cpState.background || (theme ? theme.background : '#000000');
  $('s-cp-bg-note').hidden = !!cpState.background;
  $('s-cp-bg-reset').hidden = !cpState.background;
  cpScheduleRefresh();
  // The transition screen shares these colours.
  dtSchedulePreview();
}
// Character counters, and "Reset text" once either line is edited.
function cpRenderText(){
  ['s-cp-headline', 's-cp-subline'].forEach(id => {
    const input = $(id);
    const counter = $(id + '-count');
    counter.textContent = input.value.length + '/' + input.maxLength;
    counter.classList.toggle('full', input.value.length >= input.maxLength);
  });
  $('s-cp-text-reset').hidden = $('s-cp-headline').value === cpDefaults.headline
    && $('s-cp-subline').value === cpDefaults.subline;
  cpScheduleRefresh();
}
function cpRenderHold(){
  const hold = $('s-cp-hold');
  const secs = parseInt(hold.value, 10);
  fillRange(hold);
  const minutes = Math.floor(secs / 60), rest = secs % 60;
  $('s-cp-hold-value').textContent = !minutes ? rest + ' s' : rest ? minutes + ' min ' + rest + ' s' : minutes + ' min';
}
// Form fields for POST /config and the preview query. The server clamps
// and validates every one, so these are passed through as typed.
function cpFields(){
  return {
    'crash_protection.enabled': $('s-cp-enabled').checked ? 'true' : 'false',
    'crash_protection.hold_secs': $('s-cp-hold').value,
    'crash_protection.theme': cpState.theme,
    'crash_protection.accent': cpState.accent,
    'crash_protection.background': cpState.background,
    'crash_protection.headline': $('s-cp-headline').value,
    'crash_protection.subline': $('s-cp-subline').value,
    'crash_protection.every_disconnect': $('s-cp-every').checked ? 'true' : 'false',
  };
}
function cpScheduleRefresh(){
  clearTimeout(cpRefreshTimer);
  cpRefreshTimer = setTimeout(cpRefresh, 250);
}
function cpRefresh(){
  if ($('s-cp-details').hidden) return;
  const generation = ++cpGeneration;
  const fields = cpFields();
  const previewUrl = extra => '/crash-protection/preview?' + new URLSearchParams({...fields, ...extra});
  const thumbKey = [cpState.accent, cpState.background, fields['crash_protection.headline'], fields['crash_protection.subline']].join('|');
  if (thumbKey !== cpThumbKey) {
    cpThumbKey = thumbKey;
    document.querySelectorAll('#s-cp-themes .crash-theme').forEach(card => {
      card.querySelector('img').src = previewUrl({'crash_protection.theme': card.dataset.cpTheme, size: 'thumb', phase: '0.3'});
    });
  }
  const theme = cpThemes.find(t => t.id === cpState.theme);
  if (theme && theme.soft) {
    cpRefreshVideo(previewUrl({orientation: cpState.shape, format: 'mp4'}), generation);
    return;
  }
  cpShowVideo(false);
  // Fetch the strip whole and swap it in once it has arrived, so the
  // preview never flashes empty or half-drawn while it loads.
  fetch(previewUrl({orientation: cpState.shape, frames: String(CP_PREVIEW_FRAMES)}))
    .then(response => response.ok ? response.blob() : Promise.reject(response.status))
    .then(blob => {
      if (generation !== cpGeneration) return;
      URL.revokeObjectURL(cpStripUrl);
      cpStripUrl = URL.createObjectURL(blob);
      $('s-cp-preview').style.backgroundImage = 'url("' + cpStripUrl + '")';
    })
    .catch(() => {});
}
// A soft style's preview: the loop encoded as the stream carries it,
// fetched whole and swapped in once it has arrived, like the strip.
function cpRefreshVideo(url, generation){
  fetch(url)
    .then(response => response.ok ? response.blob() : Promise.reject(response.status))
    .then(blob => {
      if (generation !== cpGeneration) return;
      URL.revokeObjectURL(cpVideoUrl);
      cpVideoUrl = URL.createObjectURL(blob);
      $('s-cp-preview-video').src = cpVideoUrl;
      cpShowVideo(true);
    })
    .catch(() => {});
}
function cpShowVideo(show){
  const video = $('s-cp-preview-video');
  video.hidden = !show;
  $('s-cp-preview').hidden = show;
  cpPlayVideo();
}
// Plays only while shown and on screen; with reduced motion it rests on
// its first frame.
function cpPlayVideo(){
  const video = $('s-cp-preview-video');
  if (!video.hidden && cpPreviewSeen && !cpStill) video.play().catch(() => {});
  else video.pause();
}
function cpApplyConfig(cp){
  if (!cp) return;
  if (Array.isArray(cp.themes) && cp.themes.length) {
    cpThemes = cp.themes;
    cpBuildThemes();
  }
  cpDefaults = cp.defaults || cpDefaults;
  $('s-cp-enabled').checked = !!cp.enabled;
  $('s-cp-hold').value = cp.hold_secs || 120;
  $('s-cp-headline').value = cp.headline || '';
  $('s-cp-subline').value = cp.subline || '';
  $('s-cp-every').checked = !!cp.every_disconnect;
  cpState = {...cpState, theme: cp.theme || 'whisper', accent: cp.accent || '#5ac8fa', background: cp.background || ''};
  cpRenderHold();
  cpRenderText();
  cpSyncVisibility();
  cpRender();
}
cpInit();

// ── Delay transition (System > General) ──────────────────────────────────
// A screen instead of a jump back when a delay turns on. Its style and
// words are its own; its colours are the crash screen's.
let dtDefaults = {headline:'', subline:''};
let dtPreviewTimer = null;
let dtGeneration = 0;
let dtVideoUrl = '';
function dtFields(){
  return {
    'delay_transition.enabled': $('s-dt-enabled').checked ? 'true' : 'false',
    'delay_transition.theme': $('s-dt-theme').value,
    'delay_transition.headline': $('s-dt-headline').value,
    'delay_transition.subline': $('s-dt-subline').value,
  };
}
// The same styles the crash card offers, from GET /config.
function dtBuildThemes(picked){
  const select = $('s-dt-theme');
  select.replaceChildren(...cpThemes.map(theme => new Option(theme.name, theme.id)));
  select.value = picked;
}
function dtRender(){
  $('s-dt-details').hidden = !$('s-dt-enabled').checked;
  $('s-dt-text-reset').hidden = $('s-dt-headline').value === dtDefaults.headline
    && $('s-dt-subline').value === dtDefaults.subline;
  dtSchedulePreview();
  updateSaveBar();
}
function dtSchedulePreview(){
  clearTimeout(dtPreviewTimer);
  dtPreviewTimer = setTimeout(dtRefreshPreview, 250);
}
// The transition screen as viewers will see it: a still for the flat
// styles, the encoded loop for the soft ones. Unsaved edits ride along.
function dtRefreshPreview(){
  const video = $('s-dt-preview-video');
  if ($('s-dt-details').hidden) { video.pause(); return; }
  const generation = ++dtGeneration;
  const id = $('s-dt-theme').value;
  const theme = cpThemes.find(t => t.id === id);
  const params = new URLSearchParams({
    'crash_protection.theme': id,
    'crash_protection.accent': cpState.accent,
    'crash_protection.background': cpState.background,
    'crash_protection.headline': $('s-dt-headline').value,
    'crash_protection.subline': $('s-dt-subline').value,
    size: 'thumb',
  });
  const show = isVideo => { video.hidden = !isVideo; $('s-dt-preview').hidden = isVideo; };
  if (!theme || !theme.soft) {
    params.set('phase', '0.3');
    $('s-dt-preview').src = '/crash-protection/preview?' + params;
    video.pause();
    show(false);
    return;
  }
  params.set('format', 'mp4');
  fetch('/crash-protection/preview?' + params)
    .then(response => response.ok ? response.blob() : Promise.reject(response.status))
    .then(blob => {
      if (generation !== dtGeneration) return;
      URL.revokeObjectURL(dtVideoUrl);
      dtVideoUrl = URL.createObjectURL(blob);
      video.src = dtVideoUrl;
      show(true);
      if (!cpStill) video.play().catch(() => {});
    })
    .catch(() => {});
}
function dtApplyConfig(dt){
  if (!dt) return;
  dtDefaults = dt.defaults || dtDefaults;
  $('s-dt-enabled').checked = !!dt.enabled;
  dtBuildThemes(dt.theme || 'halo');
  $('s-dt-headline').value = dt.headline || '';
  $('s-dt-subline').value = dt.subline || '';
  dtRender();
}
$('s-dt-enabled').onchange = dtRender;
$('s-dt-theme').onchange = dtRender;
$('s-dt-headline').oninput = dtRender;
$('s-dt-subline').oninput = dtRender;
$('s-dt-text-reset').onclick = () => {
  $('s-dt-headline').value = dtDefaults.headline;
  $('s-dt-subline').value = dtDefaults.subline;
  dtRender();
};
// The colours live on the Crash protection card: take the streamer there.
$('s-dt-colours').onclick = e => {
  e.preventDefault();
  const tab = document.querySelector('#sys-sub-tabs [data-subtab="crash"]');
  if (tab) tab.click();
};
// The preview plays only while it can be seen: scrolled away it rests, so
// an open System pane costs nothing for a film strip nobody is watching.
if ('IntersectionObserver' in window){
  new IntersectionObserver(entries => entries.forEach(e => e.target.classList.toggle('resting', !e.isIntersecting)))
    .observe($('s-cp-preview'));
  new IntersectionObserver(entries => entries.forEach(e => { cpPreviewSeen = e.isIntersecting; cpPlayVideo(); }))
    .observe($('s-cp-preview-frame'));
}


