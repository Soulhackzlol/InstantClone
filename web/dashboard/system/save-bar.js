// - Save bar -----------------------------------------
// The System form is dirty when what Save would send differs from what was
// last loaded or saved. Comparing the whole form body keeps that exact
// without tracking fields one by one.
let settingsSnapshot = null;
function settingsBody(){
  const body = new URLSearchParams({
    ingest_port:$('s-iport').value, web_port:$('s-wport').value,
    buffer_mb:$('s-bmb').value, buffer_path:$('s-bpath').value,
    ingest_bind_all:$('s-ibind').checked?'on':'off',
    web_bind_all:$('s-wbind').checked?'on':'off',
    tracing_enabled:$('s-trace').checked?'true':'false',
    twitch_client_id:$('s-twitch-client').value.trim(),
    start_with_windows:$('s-startup').checked?'on':'off',
    open_dashboard_on_launch:$('s-open-dash').checked?'true':'false',
    ui_language:$('s-language').value,
    update_check_enabled:$('s-update-check').checked?'true':'false',
    update_tray_notify:$('s-update-tray').checked?'true':'false',
    tray_color:(document.querySelector('input[name="s-tray-color"]:checked') || {value:'0'}).value,
    auto_arm_on_connect:$('s-auto-arm').checked?'true':'false',
    auto_activate_when_ready:$('s-auto-act').checked?'true':'false',
    auto_arm_delay_ms:String(Math.max(1, Math.min(DELAY_MAX_S, parseInt($('s-auto-arm-secs').value,10)||15)) * 1000),
  });
  Object.entries(cpFields()).forEach(([k, v]) => body.append(k, v));
  return body;
}
// The tray icon's color, Windows only (there is no tray elsewhere). The dots
// are the app's own list, so the page holds no copy of the colors.
function renderTrayColors(c){
  const colors = c.tray_colors || [];
  $('s-tray-color-row').hidden = c.os !== 'windows' || !colors.length;
  $('s-tray-colors').innerHTML = colors.map((color, i) =>
    `<label class="tray-color" style="--sw:${escAttr(color.hex)}" title="${escAttr(color.name)}">` +
    `<input type="radio" name="s-tray-color" value="${i}" aria-label="${escAttr(color.name)}"${i === c.tray_color_choice ? ' checked' : ''}><span></span></label>`).join('');
  // Two icons side by side never match: while another running copy shows
  // the color chosen here, this one shows the next free one.
  const chosen = colors[c.tray_color_choice];
  const shown = colors.find(color => color.hex === c.tray_color);
  $('s-tray-color-sub').textContent = chosen && shown && shown !== chosen
    ? `Another running InstantClone shows ${chosen.name.toLowerCase()}, so this one is ${shown.name.toLowerCase()} for now.`
    : 'Also colors this tab\'s icon. Give each copy its own to tell two InstantClones apart.';
}
// Which Twitch app Integrations use, from what was last saved. With no app
// at all the section opens by itself: it is the one way to get Twitch.
function renderTwitchApp(c){
  const own = !!c.twitch_client_id, builtIn = !!c.twitch_app_built_in;
  const status = $('sys-twitch-app-status'), note = $('sys-twitch-app-note');
  if (!status) return;
  status.textContent = own ? 'Using your own Twitch app.'
    : builtIn ? 'Using InstantClone\'s Twitch app.'
    : 'No Twitch app in this copy: Twitch login is off until you add one below.';
  status.classList.toggle('warn', !own && !builtIn);
  note.textContent = own ? 'your own app' : builtIn ? 'built in' : 'needs an app';
  if (!own && !builtIn) $('sys-twitch-app').open = true;
}
function settingsMarkClean(){
  settingsSnapshot = settingsBody().toString();
  updateSaveBar();
}
function settingsDirty(){
  return settingsSnapshot !== null && settingsBody().toString() !== settingsSnapshot;
}
// Only there while something is unsaved (or a save just failed and says
// why), so it never covers settings for nothing.
function updateSaveBar(){
  const foot = $('sys-foot');
  if (!foot) return;
  foot.hidden = !settingsDirty() && !$('s-msg').textContent;
}
// Throw the edits away: reload what the app has saved.
function discardSettings(){ $('s-msg').textContent = ''; flashFieldErrors(null); loadConfig({resetSystem: true}); }
// 6. The server's error can echo user text (a destination name): text only.
function showSettingsError(text){
  const span = document.createElement('span');
  span.className = 'err';
  span.textContent = text;
  $('s-msg').replaceChildren(span);
}
(() => {
  const pane = document.querySelector('.tab-pane[data-pane="system"]');
  let queued = false;
  // Clicks count too: style cards and colour swatches change the form
  // without an input event.
  const queue = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => { queued = false; updateSaveBar(); });
  };
  ['input', 'change', 'click'].forEach(type => pane.addEventListener(type, queue));
  window.addEventListener('beforeunload', e => {
    if (!settingsDirty()) return;
    e.preventDefault();
    e.returnValue = '';
  });
})();

// Hotkeys and MIDI are Windows-only; with neither, Controls has nothing in it.
function syncControlsTab(){
  const tab = document.querySelector('#sys-sub-tabs [data-subtab="controls"]');
  if (!tab) return;
  tab.hidden = $('s-hotkeys-section').hidden && $('s-midi-section').hidden;
  if (tab.hidden && tab.classList.contains('on')) showSubTab('system', 'general');
  moveSubTabInd(tab.closest('.tab-pane'));
}

