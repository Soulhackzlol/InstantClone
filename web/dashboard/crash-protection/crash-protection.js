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
// The preview plays only while it can be seen: scrolled away it rests, so
// an open System pane costs nothing for a film strip nobody is watching.
if ('IntersectionObserver' in window){
  new IntersectionObserver(entries => entries.forEach(e => e.target.classList.toggle('resting', !e.isIntersecting)))
    .observe($('s-cp-preview'));
}


