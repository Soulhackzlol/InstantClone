// ── Crash protection card ────────────────────────────────────────────────
// The reconnect screen's editor. Its pictures come from screen-preview.js;
// the style list and default text come from GET /config.
const CP_ACCENTS = [['Cyan','#5ac8fa'],['Violet','#9d7bff'],['Lime','#7ee06b'],['Coral','#ff6b6b'],['Amber','#ffb73a']];
let cpState = {theme:'whisper', accent:'#5ac8fa', background:'', shape:'horizontal'};
let cpThemes = [];
let cpDefaults = {headline:'', subline:''};
let cpRefreshTimer = null;
const cpPreview = screenPreview({frame:'s-cp-preview-frame', strip:'s-cp-preview', video:'s-cp-preview-video'});

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
  $('s-cp-full').onclick = () => screenOpenFull(cpLook(), cpState.shape);
}
function cpSetAccent(hex){ cpState.accent = hex.toLowerCase(); cpRender(); }
function cpSyncVisibility(){
  $('s-cp-details').hidden = !$('s-cp-enabled').checked;
  cpScheduleRefresh();
}
// Reflect cpState in the controls, then refresh the pictures.
function cpRender(){
  screenMarkTheme($('s-cp-themes'), cpState.theme);
  document.querySelectorAll('#s-cp-swatches .crash-swatch').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.hex === cpState.accent)));
  document.querySelectorAll('[data-cp-shape]').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.cpShape === cpState.shape)));
  $('s-cp-accent').value = cpState.accent;
  const theme = cpThemes.find(t => t.id === cpState.theme);
  $('s-cp-bg').value = cpState.background || (theme ? theme.background : '#000000');
  $('s-cp-bg-note').hidden = !!cpState.background;
  $('s-cp-bg-reset').hidden = !cpState.background;
  cpScheduleRefresh();
  // The transition screen shares these colours.
  dtSchedulePreview();
}
function cpRenderText(){
  screenRenderText('s-cp', cpDefaults);
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
// What the screen looks like: the fields its pictures are drawn from.
function cpLook(){
  return {
    'crash_protection.theme': cpState.theme,
    'crash_protection.accent': cpState.accent,
    'crash_protection.background': cpState.background,
    'crash_protection.headline': $('s-cp-headline').value,
    'crash_protection.subline': $('s-cp-subline').value,
  };
}
function cpScheduleRefresh(){
  clearTimeout(cpRefreshTimer);
  cpRefreshTimer = setTimeout(cpRefresh, 250);
}
function cpRefresh(){
  if ($('s-cp-details').hidden || !screensShown('crash')) { cpPreview.pause(); return; }
  const look = cpLook();
  screenDrawThumbs($('s-cp-themes'), look);
  const theme = cpThemes.find(t => t.id === cpState.theme);
  cpPreview.refresh(look, cpState.shape, !!(theme && theme.soft));
}
function cpApplyConfig(cp){
  if (!cp) return;
  if (Array.isArray(cp.themes) && cp.themes.length) {
    cpThemes = cp.themes;
    screenBuildThemes($('s-cp-themes'), cpThemes, () => cpState.theme, id => { cpState.theme = id; cpRender(); });
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
