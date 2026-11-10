// - LAN exposure warning -----------------------─
// The two toggles carry different risks, so say the one that applies:
// the web UI is an unauthenticated control surface, while the ingest port
// lets anyone on the network publish into the buffer. Both default off,
// and this stays hidden until one is actually ticked.
// --- Optional dashboard auth (Security section) --------------------------
let _authEnabled = false;
function applyAuthState(enabled, dockToken){
  _authEnabled = !!enabled;
  if ($('sec-off')) $('sec-off').hidden = _authEnabled;
  if ($('sec-on')) $('sec-on').hidden = !_authEnabled;
  const st = $('pw-status');
  if (st){
    st.textContent = _authEnabled ? 'On' : 'Off';
    st.className = 'sys-version-pill ' + (_authEnabled ? 'ok' : 'off');
  }
  const stn = $('pw-status-note');
  if (stn) stn.textContent = _authEnabled ? 'Sign-in required' : 'Local-only use needs none';
  const url = $('sec-dock-url');
  if (url){
    url.textContent = dockToken ? (location.origin + '/dock?token=' + dockToken) : '';
    url.classList.remove('revealed'); // always start hidden
  }
  if ($('sec-dock-toggle')) $('sec-dock-toggle').textContent = 'Show';
  showDockUrl(dockToken);
  renderLanWarning();
}
// The OBS tab hands out the dock link too. With a password on, a link
// without the token lands the dock on the login page, so it carries the
// token and is masked like the Security copy.
function showDockUrl(dockToken){
  const el = $('dock-url');
  if (!el) return;
  el.textContent = location.origin + '/dock' + (dockToken ? '?token=' + dockToken : '');
  el.classList.toggle('blur-secret', !!dockToken);
  el.classList.remove('revealed');
  el.title = dockToken ? 'Hidden to prevent leaks on stream. Use Show to reveal.' : '';
  const toggle = $('dock-url-toggle');
  if (toggle){ toggle.hidden = !dockToken; toggle.textContent = 'Show'; }
}

// Reveal/hide a blurred secret without ever changing what Copy grabs.
function toggleSecret(id, btn){
  const el = $(id);
  if (!el) return;
  const showing = el.classList.toggle('revealed');
  if (btn) btn.textContent = showing ? 'Hide' : 'Show';
}

async function enableAuth(btn, inputId){
  const inp = $(inputId);
  const pw = inp ? inp.value : '';
  if (!pw || pw.length < 8){ toast('Use at least 8 characters', 'err'); return; }
  const label = btn && btn.textContent;
  if (btn){ btn.disabled = true; btn.textContent = 'Saving…'; }
  try {
    const r = await fetch('/auth/set-password', {method:'POST', headers:{'Content-Type':'application/x-www-form-urlencoded'}, body:'password=' + encodeURIComponent(pw)});
    if (r.ok){ if (inp) inp.value=''; toast('Password set', 'ok'); await loadConfig(); }
    else { const j = await r.json().catch(()=>({})); toast((j&&j.error)?j.error:'Could not set password', 'err'); }
  } catch(_){ toast('Network error', 'err'); }
  finally { if (btn){ btn.disabled = false; btn.textContent = label || 'Enable password'; } }
}

async function disableAuth(btn){
  if (!confirm('Turn off the password? The dashboard becomes open to anyone who can reach it.')) return;
  if (btn){ btn.disabled = true; }
  try {
    const r = await fetch('/auth/disable', {method:'POST'});
    if (r.ok){ toast('Password disabled', 'ok'); await loadConfig(); } else toast('Could not disable', 'err');
  } catch(_){ toast('Network error', 'err'); }
  finally { if (btn){ btn.disabled = false; } }
}

async function regenDock(btn){
  if (!confirm('Regenerate the dock URL? The current one stops working and you must repaste the new URL into OBS.')) return;
  if (btn){ btn.disabled = true; }
  try {
    const r = await fetch('/auth/regen-dock', {method:'POST'});
    if (r.ok){
      const j = await r.json().catch(()=>({}));
      if ($('sec-dock-url') && j && j.dock_token){
        $('sec-dock-url').textContent = location.origin + '/dock?token=' + j.dock_token;
        $('sec-dock-url').classList.remove('revealed');
        if ($('sec-dock-toggle')) $('sec-dock-toggle').textContent = 'Show';
        showDockUrl(j.dock_token);
      }
      toast('New dock URL generated', 'ok');
    } else toast('Could not regenerate', 'err');
  } catch(_){ toast('Network error', 'err'); }
  finally { if (btn){ btn.disabled = false; } }
}

async function logoutNow(){
  try { await fetch('/logout', {method:'POST'}); } catch(_){}
  location.href = '/login';
}

function renderLanWarning(){
  const web = $('s-wbind').checked;
  const ingest = $('s-ibind').checked;
  const hasKey = !!($('s-ingest-key') && $('s-ingest-key').value.trim());
  const parts = [];
  if (web && !_authEnabled) parts.push('The web UI has no password, so anyone on your network can change the delay, cut and edit destinations. Integrations can’t be set up to run programs or write files until you set a Dashboard password above.');
  if (ingest && !hasKey) parts.push('InstantClone accepts any stream key, so anyone on your network can stream into it. Set a stream key for InstantClone above to lock it.');
  const on = parts.length > 0;
  $('lan-warn').hidden = !on;
  if (on) $('lan-warn-text').textContent = parts.join(' ') + ' Only enable on a network you trust.';
}

// Reflect the current ingest key across the OBS tab: the copyable Stream key,
// the helper note, the input, and the LAN warning (a set key removes the
// "anyone can publish" line).
function applyIngestKey(key){
  const codeEl = $('obs-key');
  if (codeEl) codeEl.textContent = key || 'main';
  const inp = $('s-ingest-key');
  if (inp && document.activeElement !== inp) inp.value = key || '';
  const note = $('obs-key-note');
  if (note) note.innerHTML = key
    ? 'InstantClone is locked. OBS must use <b>this exact key</b> to stream in.'
    : 'Any string works. The real platform key lives in <b>Destinations</b>.';
  const st = $('ingest-status');
  if (st){
    st.textContent = key ? 'Locked' : 'Unlocked';
    st.className = 'sys-version-pill ' + (key ? 'ok' : 'off');
  }
  const stn = $('ingest-status-note');
  if (stn) stn.textContent = key ? 'Only an OBS with your key can stream in' : 'Any stream key is accepted';
  renderLanWarning();
}

function genIngestKey(){
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  const inp = $('s-ingest-key');
  if (inp){
    inp.value = Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
    // Reveal the freshly generated key so it can be read and copied; it
    // re-blurs on its own once focus leaves the field.
    inp.classList.add('revealed');
    const t = $('ingest-key-toggle'); if (t) t.textContent = 'Hide';
  }
}

async function saveIngestKey(btn){
  const inp = $('s-ingest-key');
  const key = inp ? inp.value.trim() : '';
  const msg = $('ingest-key-msg');
  if (btn){ btn.disabled = true; btn.textContent = 'Saving…'; }
  try {
    const resp = await fetch('/config', {
      method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: 'ingest_key=' + encodeURIComponent(key),
    });
    if (resp.ok){
      applyIngestKey(key);
      if (msg) msg.textContent = key
        ? 'Locked. Set this as your OBS stream key.'
        : 'Stream key cleared. Any key is accepted again.';
      toast(key ? 'Stream key saved' : 'Stream key cleared', 'ok');
    } else {
      if (msg) msg.textContent = 'Could not save (server returned ' + resp.status + ').';
      toast('Could not save the stream key', 'err');
    }
  } catch(_){
    if (msg) msg.textContent = 'Could not save (network error).';
    toast('Could not save the stream key', 'err');
  } finally {
    if (btn){ btn.disabled = false; btn.textContent = 'Save key'; }
  }
}

let _savingSettings = false;
// Overlays folder lives on the Overlay tab now, so it saves itself rather
// than riding the System tab's Save button. POSTs just overlays_dir and
// refreshes the gallery from the new location.
async function saveOverlaysDir(btn){
  const dir = $('s-ovdir').value.trim();
  const label = btn && btn.textContent;
  if (btn){ btn.disabled = true; btn.textContent = 'Saving…'; }
  try {
    const j = await (await fetch('/config', {method:'POST', body:new URLSearchParams({overlays_dir:dir})})).json();
    if (j.ok){ toast('Overlays folder saved', 'ok'); await loadConfig(); if (typeof loadOverlays === 'function') loadOverlays(); }
    else toast(`Couldn't save: ${j.error}`, 'err');
  } catch(e){ toast('Save failed: the app may have stopped responding', 'err'); }
  finally { if (btn){ btn.disabled = false; btn.textContent = label || 'Save'; } }
}

async function saveSettings(){
  if (_savingSettings) return; // guard against a double-click double-POST
  const btn = document.querySelector('#sys-foot .ic-btn-primary');
  const body = settingsBody();
  const oldWebPort = lastConfig ? +lastConfig.web_port : 0;
  _savingSettings = true;
  if (btn){ btn.disabled = true; btn.textContent = 'Saving…'; }
  try {
    const j = await(await fetch('/config',{method:'POST',body})).json();
    if (j.ok){
      flashFieldErrors(null);
      $('s-msg').textContent = '';
      toast('Settings saved','ok');
      // The dashboard moves to a new Web UI port at once and this address
      // stops answering: follow it there. Behind a proxy (another port in
      // the address bar) the proxy is the user's to repoint.
      const newWebPort = +$('s-wport').value;
      if (oldWebPort && newWebPort !== oldWebPort && +location.port === oldWebPort){
        settingsMarkClean();
        waitForRestart({port: newWebPort, text: 'The dashboard moved to port ' + newWebPort + '. Taking you there.'});
        return;
      }
      if (languageOf(body.get('ui_language')) !== document.documentElement.lang){
        settingsMarkClean();
        reloadForLanguage();
        return;
      }
      // The restart banner follows the server's restart_pending (applyState).
      loadConfig({resetSystem: true});
    } else { flashFieldErrors(j.error); toast(`Couldn't save settings: ${j.error}`, 'err'); showSettingsError(j.error); }
  } catch(e){
    toast('Couldn\'t save settings: the app may have stopped responding','err');
    showSettingsError('Request failed: ' + e);
  } finally {
    _savingSettings = false;
    if (btn){ btn.disabled = false; btn.textContent = 'Save settings'; }
    updateSaveBar();
  }
}
