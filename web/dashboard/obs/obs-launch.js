// - Launch OBS with --config-url ----------------------
// Spawns obs64.exe with the Enhanced Broadcasting CLI flag. The user
// has to use this every time they want EB on a Custom RTMP setup,
// since OBS only reads --config-url at launch and rtmp_custom's
// settings plugin discards file-injected URLs. Errors surface inline
// next to the button so the user can fix install paths or close a
// running OBS first.
// One-click VOD + EB: saves the destination (so the VOD flag is written),
// then runs the server-side setup and renders a red-to-green checklist so
// the user can see exactly which step needs attention.
// Render a red-to-green step checklist (shared by the destination-form
// and System-tab VOD+EB flows).
function renderEbChecklist(list, steps){
  if (!list || !steps) return;
  list.innerHTML = steps.map(s =>
    `<li style="display:flex;gap:7px;align-items:flex-start;margin:3px 0">
       <span style="color:${s.ok?'var(--ok,#3fb950)':'var(--err)'};font-weight:700">${s.ok?'✓':'✗'}</span>
       <span><b>${s.name}</b> - ${s.msg}</span>
     </li>`).join('');
}

async function setupVodEb(){
  const btn = $('obs-eb-setup-btn');
  const list = $('vod-eb-checklist');
  if (!btn || btn.disabled) return;
  btn.disabled = true;
  if (list){ list.hidden = false; list.innerHTML = '<li>Saving destination…</li>'; }
  if (!(await saveDestForm())){
    if (list) list.innerHTML = '<li class="bad">Fix the destination form, then try again.</li>';
    btn.disabled = false;
    return;
  }
  try {
    const r = await (await fetch('/obs/setup-vod-eb', {method:'POST'})).json();
    if (r.steps) renderEbChecklist(list, r.steps);
    toast(r.ok ? 'VOD + EB ready' : 'VOD + EB needs attention', r.ok ? 'ok' : 'err');
  } catch(e){
    if (list) list.innerHTML = '<li class="bad">Request failed: '+String(e)+'</li>';
    toast('Request failed','err');
  }
  btn.disabled = false;
}

// Create a Desktop shortcut that cold-starts the whole VOD + EB flow.
// Callable from the destination form (default ids) and the System tab.
async function createEbShortcut(btnId, msgId){
  const btn = $(btnId || 'eb-shortcut-btn');
  const msg = $(msgId || 'eb-shortcut-msg');
  if (!btn || btn.disabled) return;
  btn.disabled = true;
  if (msg) msg.textContent = 'Creating shortcut…';
  try {
    const r = await (await fetch('/shortcut/create-eb', {method:'POST'})).json();
    if (r.ok){
      if (msg) msg.textContent = 'Created '+r.kind+': '+r.path;
      toast('Desktop shortcut created','ok');
    } else {
      if (msg) msg.textContent = r.error || 'Failed';
      toast(r.error || 'Failed','err');
    }
  } catch(e){
    if (msg) msg.textContent = String(e);
    toast('Request failed','err');
  }
  btn.disabled = false;
}

// Said while OBS can't be found to launch, and taken back once it can.
const OBS_NOT_FOUND = 'OBS not found at the standard install path.';
function sayObsFound(msg, found){
  if (!msg) return;
  if (!found && !msg.textContent) msg.textContent = OBS_NOT_FOUND;
  else if (found && msg.textContent === tr(OBS_NOT_FOUND, msg)) msg.textContent = '';
}
async function refreshObsEbLaunchAvailability(){
  const btn = $('obs-eb-setup-btn');
  const msg = $('obs-eb-launch-msg');
  if (!btn) return;
  try {
    const r = await (await fetch('/obs/launch-status')).json();
    if (!r.installed){
      btn.disabled = true;
      setClassName(btn, 'ic-btn ic-btn-disabled');
    } else {
      btn.disabled = false;
      setClassName(btn, 'ic-btn ic-btn-primary');
    }
    sayObsFound(msg, r.installed);
  } catch(_){}
}

// - System tab: OBS quick launch -------------------
// EB / VOD+EB launchers that work without opening any destination's
// settings. They reuse the same backend endpoints as the destination
// form; the only extra logic is the VOD+EB pre-step that persists
// vod_audio on the Twitch destination - without it, the next
// destination save would flip OBS's VOD flag straight back off
// (reconcile_obs_vod_files derives the flag from the destinations).

function firstTwitchDest(){
  return (lastDests || []).find(d => d.enabled && d.platform === 'twitch' && d.stream_key_set);
}

// OBS-installed probe, cached until an OBS changes (refreshObsSetup
// clears it): loadDestinations() calls this on every refresh.
let _obsInstalled = null;
async function refreshSysEbLaunch(){
  const warn = $('sys-eb-twitch-warn');
  if (warn) warn.hidden = !!firstTwitchDest();
  if (_obsInstalled === null){
    try { _obsInstalled = !!(await (await fetch('/obs/launch-status')).json()).installed; } catch(_){}
  }
  const dis = _obsInstalled === false;
  ['sys-eb-btn','sys-vodeb-btn'].forEach(id => {
    const b = $(id);
    // Legacy launchers: plain buttons, not the page's main call to action.
    if (b){ b.disabled = dis; setClassName(b, dis ? 'ic-btn ic-btn-disabled' : 'ic-btn ic-btn-ghost'); }
  });
  if (_obsInstalled !== null) sayObsFound($('sys-eb-msg'), _obsInstalled);
}

async function sysLaunchEb(){
  const btn = $('sys-eb-btn');
  if (!btn || btn.disabled) return;
  btn.disabled = true;
  const msg = $('sys-eb-msg');
  const list = $('sys-vodeb-checklist');
  if (list) list.hidden = true;
  try {
    const r = await (await fetch('/obs/launch-with-eb', {method:'POST'})).json();
    if (msg) msg.textContent = r.ok ? r.message : (r.error || 'Launch failed');
    toast(r.ok ? 'OBS launched with Enhanced Broadcasting' : 'Launch failed', r.ok ? 'ok' : 'err');
  } catch(e){
    if (msg) msg.textContent = String(e);
    toast('Request failed','err');
  }
  btn.disabled = false;
}

async function sysLaunchVodEb(){
  const btn = $('sys-vodeb-btn');
  if (!btn || btn.disabled) return;
  btn.disabled = true;
  const list = $('sys-vodeb-checklist');
  const msg = $('sys-eb-msg');
  if (msg) msg.textContent = '';
  if (list){ list.hidden = false; list.innerHTML = '<li>Preparing…</li>'; }
  const steps = [];
  try {
    const d = firstTwitchDest();
    if (!d){
      renderEbChecklist(list, [{name:'Twitch destination', ok:false,
        msg:'none enabled with a stream key. Add one in the Destinations tab first'}]);
      toast('Add a Twitch destination first','err');
      btn.disabled = false;
      return;
    }
    if (d.vod_audio){
      steps.push({name:'VOD audio mode', ok:true, msg:`already on for "${d.name}"`});
    } else {
      // Same full-field resend discipline as toggleDest: anything not
      // passed through resets to its default on the server.
      const body = new URLSearchParams({
        id:d.id, name:d.name, platform:d.platform,
        custom_egress_url:d.custom_egress_url||'', twitch_ingest:d.twitch_ingest||'',
        youtube_ingest:d.youtube_ingest||'',
        vod_audio:'on',
        vod_audio_inject_eb: d.vod_audio_inject_eb ? 'on' : 'off',
        stream_format:d.stream_format||'horizontal', audio_track:d.audio_track||'auto', enabled:'on',
      });
      const rr = await (await fetch('/destinations', {method:'POST', body})).json();
      steps.push({name:'VOD audio mode', ok:!!rr.ok,
        msg: rr.ok ? `enabled on "${d.name}"` : (rr.error || 'destination update failed')});
      if (!rr.ok){
        renderEbChecklist(list, steps);
        toast('Could not update the Twitch destination','err');
        btn.disabled = false;
        return;
      }
      loadDestinations();
    }
    const r = await (await fetch('/obs/setup-vod-eb', {method:'POST'})).json();
    renderEbChecklist(list, steps.concat(r.steps || []));
    toast(r.ok ? 'VOD + EB ready' : 'VOD + EB needs attention', r.ok ? 'ok' : 'err');
  } catch(e){
    if (list) list.innerHTML = '<li class="bad">Request failed: '+String(e)+'</li>';
    toast('Request failed','err');
  }
  btn.disabled = false;
}

