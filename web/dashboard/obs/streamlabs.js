// - Streamlabs Desktop (experimental) ------------------------------
// Once connected, its Twitch streams come through InstantClone: the server
// points Streamlabs' Enhanced Broadcasting setup at us and follows its Go
// Live flow (src/streamlabs.rs). The card and the wizard only show where
// Streamlabs is installed, so OBS users never see it.
let slInfo = null;
// The app sending the stream, named only when it's certain: OBS while
// Streamlabs isn't connected, Streamlabs while its stream is the one
// arriving. With Streamlabs connected either app can stream here, so
// otherwise '' and the words stay neutral ("your stream").
function encoderApp(){
  if (lastState && lastState.publisher_app === 'streamlabs') return 'Streamlabs';
  return slInfo && slInfo.enabled ? '' : 'OBS';
}
// "OBS is streaming through InstantClone right now.", for the confirms.
function liveNowText(){
  const app = encoderApp();
  return app ? `${app} is streaming through InstantClone right now.` : 'Your stream is going through InstantClone right now.';
}
async function slStatus(){
  try { slInfo = await (await fetch('/streamlabs/status')).json(); }
  catch(_){ slInfo = null; }
  return slInfo;
}
const slReady = r => !!(r && r.enabled && r.state === 'connected' && r.platform === 'twitch');
// [pill tone, pill label, status line] for one status answer.
// Streamlabs is one per PC, so it streams through one InstantClone: said
// before Connect is pressed when another copy (another channel) has it.
const slOtherCopy = r => r && !r.enabled && r.other_copy ? r.other_copy : null;
function slOtherCopyText(other){
  return other.running
    ? `Streamlabs streams through the other InstantClone (dashboard port ${other.port}). Disconnect it there to use Streamlabs here.`
    : `Streamlabs is set to another InstantClone (dashboard port ${other.port}), which is closed. Connect moves it to this one.`;
}
function slView(r){
  const other = slOtherCopy(r);
  if (other) return [other.running ? 'warn' : '', other.running ? 'In use' : 'Off', slOtherCopyText(other)];
  if (!r.enabled) return ['', 'Off', 'Found Streamlabs on this PC. Connect it to stream through InstantClone from its Go Live button.'];
  if (r.state === 'not_installed') return ['bad', 'Not found', "Streamlabs Desktop isn't installed here, or hasn't been opened yet. Open it once."];
  if (r.state === 'error') return ['bad', 'Problem', r.problem];
  if (r.state !== 'connected') return ['warn', 'Waiting', 'Connected. Open Streamlabs and InstantClone picks it up.'];
  if (r.problem) return ['warn', 'Check Streamlabs', r.problem];
  if (r.live && r.route === 'around') return ['bad', 'Not through InstantClone', "Streamlabs is live, but its stream isn't reaching InstantClone, so the delay doesn't apply. Go live to Twitch from Streamlabs' Go Live window, without Streamlabs' own Multistream."];
  if (r.live) return ['ok', 'Live', r.route === 'single'
    ? 'Live through InstantClone as one video track (Enhanced Broadcasting was off).'
    : 'Live through InstantClone with Enhanced Broadcasting.'];
  return ['ok', 'Connected', 'Ready. Go live from Streamlabs as usual: its Twitch stream comes through InstantClone.'];
}
function renderStreamlabs(r){
  renderObsTabApp(r);
  const card = $('sl-card');
  if (card){
    card.hidden = !r || !r.supported || !(r.installed || r.enabled);
    if (!card.hidden){
      const [tone, label, text] = slView(r);
      setClassName($('sl-pill'), 'ic-pill obs-reg-pill' + (tone ? ' ' + tone : ''));
      setText($('sl-pill-l'), label);
      setText($('sl-status'), text);
      const other = slOtherCopy(r);
      $('sl-on-btn').hidden = r.enabled;
      $('sl-on-btn').disabled = !!(other && other.running);
      $('sl-off-btn').hidden = !r.enabled;
      $('sl-import-btn').hidden = !slReady(r);
    }
  }
  renderWizStreamlabs(r);
  syncWizStreamlabsImport();
}
// Wizard step 1: the app pick shows where Streamlabs can run (Windows); the
// Streamlabs panel says what's in the way before Connect is pressed.
function renderWizStreamlabs(r){
  const tiles = $('wiz-app-tiles');
  if (!tiles) return;
  tiles.hidden = !r || !r.supported;
  if (tiles.hidden) return;
  const other = slOtherCopy(r);
  // Kept current by Live: opening Streamlabs while this step shows
  // brings Connect up at once.
  const state = r.enabled ? 'connected' : !r.installed && !other ? 'missing' : 'connect';
  $('wiz-sl-card').dataset.state = state;
  $('wiz-sl-btn').disabled = !!(other && other.running);
  setText($('wiz-sl-status'), other ? slOtherCopyText(other) : '');
  const [title, lead] = WIZ_SL_COPY[state](r);
  setText($('wiz-sl-title'), title);
  if (lead !== null) setText($('wiz-sl-lead'), lead);
  else if ($('wiz-sl-lead').innerHTML !== wizSlConnectLead) $('wiz-sl-lead').innerHTML = wizSlConnectLead;
  syncWizNext();
}
// The connect pitch is in the page already; the other states say their own.
const wizSlConnectLead = $('wiz-sl-lead').innerHTML;
const WIZ_SL_COPY = {
  connect: () => ['Connect Streamlabs', null],
  missing: () => ["We can't find Streamlabs on this PC", 'Already have it? Open it once and it shows up here by itself.'],
  connected: r => ['Streamlabs is connected', r.problem || (r.state === 'connected'
    ? 'Keep Streamlabs signed in with your Twitch account.'
    : 'Open Streamlabs and InstantClone picks it up.')],
};
let wizApp = 'obs';
function wizPickApp(app){
  wizApp = app;
  $('wiz-app-tiles').dataset.app = app;
  $$('#wiz-app-tiles .w-app').forEach(b => {
    const on = b.dataset.app === app;
    b.classList.toggle('on', on);
    b.setAttribute('aria-checked', on ? 'true' : 'false');
  });
  $$('.w-app-pane').forEach(p => p.classList.toggle('on', p.dataset.paneApp === app));
  // Later steps explain things in the picked app's own words.
  $$('#wizard [data-app-only]').forEach(el => { el.hidden = el.dataset.appOnly !== app; });
  clearWizMsg();
  syncWizNext();
}
async function refreshStreamlabs(){ renderStreamlabs(await slStatus()); }
// The OBS tab shows one app's setup: the one picked here, else Streamlabs
// while it's connected, else OBS. Both apps can still stream to this copy,
// which the note under the switch says. The switch is the tab's sub-tabs,
// so a pick goes through showSubTab like any other.
let obsTabApp = null;
function obsTabPickApp(app){ obsTabApp = app; renderObsTabApp(slInfo); }
function renderObsTabApp(r){
  const tabs = $('obs-app-tabs');
  if (!tabs) return;
  const wasHidden = tabs.hidden;
  tabs.hidden = !r || !r.supported || !(r.installed || r.enabled);
  const app = tabs.hidden ? 'obs' : (obsTabApp || (r.enabled ? 'streamlabs' : 'obs'));
  const pane = tabs.closest('.tab-pane');
  const current = tabs.querySelector('.sub-tab.on');
  tabs.querySelectorAll('.sub-tab').forEach(t => t.classList.toggle('on', t.dataset.subtab === app));
  // Measured once it shows, or when the pick changed under it.
  if (!tabs.hidden && (wasHidden || !current || current.dataset.subtab !== app)) moveSubTabInd(pane);
  pane.querySelector('.obs-grid').dataset.app = app;
  const streamlabs = app === 'streamlabs';
  $('obs-tab-title').textContent = streamlabs ? 'Streamlabs' : 'OBS';
  $('obs-tab-sub').textContent = streamlabs
    ? 'Connect Streamlabs. Your real platform stream keys stay in Destinations.'
    : 'Point OBS at InstantClone. Your real platform stream keys stay in Destinations.';
  const obsRegistered = obsInstallList().some(i => i.state === 'current');
  const note = !streamlabs && r && r.enabled
    ? 'Streamlabs stays connected too, so either app can stream here. Switch to Streamlabs above to disconnect it.'
    : streamlabs && obsRegistered
    ? 'OBS stays registered too, so either app can stream here.'
    : '';
  const noteEl = $('obs-app-note');
  noteEl.hidden = !note;
  noteEl.textContent = note;
}
async function slConnect(on){
  const btn = $(on ? 'sl-on-btn' : 'sl-off-btn');
  btn.disabled = true;
  const r = await postAction(on ? '/streamlabs/connect' : '/streamlabs/disconnect');
  btn.disabled = false;
  $('sl-msg').textContent = r.ok ? '' : r.error;
  if (r.ok) toast(on ? 'Streamlabs connected' : 'Streamlabs disconnected: its Twitch setup is back as it was', 'ok');
  await refreshStreamlabs();
  // The server picks Streamlabs up a moment later; show that without waiting
  // for the next poll.
  if (on) setTimeout(refreshStreamlabs, 1200);
}
// What importing Streamlabs' Twitch key did, in one line.
function slImportText(d){
  const name = `"${d.destination}"`;
  const done = d.outcome === 'added' ? `Added the Twitch destination ${name} with Streamlabs' key.`
    : d.outcome === 'updated' ? `Put Streamlabs' Twitch key in ${name}.`
    : `${name} already has Streamlabs' Twitch key. Nothing to change.`;
  return d.enabled ? done : `${done} It's switched off: turn it on in Destinations to stream there.`;
}
async function slImportTwitch(){
  const btn = $('sl-import-btn');
  btn.disabled = true;
  const r = await postAction('/streamlabs/import-twitch');
  btn.disabled = false;
  const text = r.ok ? slImportText(r.data) : r.error;
  // Kept on the card, not only in a toast: the key itself never shows, so
  // this line is what says it worked.
  $('sl-msg').textContent = text;
  toast(text, r.ok ? 'ok' : 'err');
  if (r.ok && r.data.outcome !== 'unchanged'){ loadDestinations(); loadConfig(); }
}

// Wizard: connecting Streamlabs is step 1 done; with Twitch picked, the key
// step can take the key from Streamlabs instead of a paste. The key goes
// straight into a destination on the server, never through this page.
let wizKeyImported = false;
async function wizConnectStreamlabs(){
  const btn = $('wiz-sl-btn');
  btn.disabled = true;
  const r = await postAction('/streamlabs/connect');
  btn.disabled = false;
  if (!r.ok){ $('wiz-sl-status').textContent = r.error; return; }
  wizObsAdvance('connect');
  await refreshStreamlabs();
  setTimeout(refreshStreamlabs, 1200);
}
function syncWizStreamlabsImport(){
  const row = $('w-sl-import');
  if (!row) return;
  const twitch = $('w-platform') && $('w-platform').value === 'twitch';
  row.hidden = !twitch || !(wizKeyImported || slReady(slInfo));
  $('w-sl-import-btn').hidden = wizKeyImported;
  // An import only covers Twitch: picking another platform wants a paste.
  const key = $('w-key');
  key.disabled = twitch && wizKeyImported;
  key.placeholder = key.disabled ? 'imported from Streamlabs' : 'paste your stream key';
}
async function wizImportStreamlabsKey(){
  const btn = $('w-sl-import-btn'), msg = $('w-sl-import-msg');
  btn.disabled = true;
  const r = await postAction('/streamlabs/import-twitch');
  btn.disabled = false;
  if (!r.ok){ msg.textContent = r.error; return; }
  wizKeyImported = true;
  $('w-key').value = '';
  msg.textContent = 'Imported. Your key went straight to the Twitch destination.';
  clearWizMsg();
  syncWizStreamlabsImport();
}

// The OBS tab asks once when it opens; what changes outside the dashboard
// after that (an OBS opened or closed, Streamlabs installed) arrives
// through Live, without polling.
function showObsTab(){
  refreshObsRegisterStatus();
  refreshStreamlabs();
  refreshSysVodScript();
}
// Everything shown about OBS on this PC, once it changed (see Live).
async function refreshObsSetup(){
  _obsInstalled = null;
  await Promise.all([refreshObsRegisterStatus(), obsRegInfo(true)]);
  refreshSysVodScript();
  refreshSysEbLaunch();
  refreshObsEbLaunchAvailability();
}
function openObsLegacy(){
  const d = $('sys-eb-launch');
  if (!d) return;
  d.open = true;
  d.scrollIntoView({behavior: motionOK() ? 'smooth' : 'auto', block: 'start'});
}
