// - Reset settings / Reset everything ----------------
// Two-step destructive flow. The "settings" scope is recoverable in
// practice (the user can just re-enter their port/buffer/webhook
// values) so a single click-through is fine. The "all" scope wipes
// destinations + delay profiles and bounces the app into the wizard,
// so we additionally require typing the literal word RESET before
// the confirm button enables.
let resetScope = null;
function openResetModal(scope){
  resetScope = scope;
  const m = $('reset-modal');
  const title = $('reset-modal-title');
  const sub = $('reset-modal-sub');
  const list = $('reset-modal-list');
  const keeps = $('reset-modal-keeps');
  const typebox = $('reset-modal-typebox');
  const confirm = $('reset-confirm-btn');
  const input = $('reset-confirm-input');
  if (scope === 'all'){
    title.textContent = 'Reset everything?';
    sub.textContent = "This wipes every setting plus all destinations and profiles, then reloads this page into the setup wizard.";
    list.innerHTML = [
      '<li><b>All destinations</b> (Twitch / YouTube / custom keys): gone</li>',
      '<li><b>All saved delay profiles</b>: gone</li>',
      '<li><b>All your overlays</b> (presets + custom): deleted, the built-in set restored</li>',
      '<li><b>All integrations</b>, their Discord and phone connections, and the Twitch login: gone</li>',
      '<li>Network ports, buffer settings, overlays folder, password and stream key, hotkeys, MIDI, crash protection: reset to defaults</li>',
      '<li>Diagnostic trace toggle: turned off</li>',
    ].join('');
    keeps.innerHTML = '<b>Kept:</b> the buffer file on disk (it\'s reset on every clean shutdown anyway) and the OBS service registration (use the OBS tab to unregister if you want).';
    typebox.hidden = false;
    confirm.disabled = true;
    confirm.textContent = 'Reset everything';
    input.value = '';
    setTimeout(() => input.focus(), 50);
  } else {
    title.textContent = 'Reset settings?';
    sub.textContent = "Restores app-level settings to defaults. Your destinations, profiles and integrations stay.";
    list.innerHTML = [
      '<li>Network ports back to <code class="mono">:1935</code> / <code class="mono">:7799</code></li>',
      '<li>Buffer size and file back to defaults</li>',
      '<li>Overlays folder reset to <code class="mono">overlays/</code></li>',
      '<li>Diagnostic trace toggle turned off</li>',
      '<li>Dashboard password and stream key removed, so the dashboard and InstantClone are open again</li>',
      '<li>Hotkeys, MIDI, crash protection and the General options (arm when OBS connects, activate when ready, default delay, updates) back to defaults</li>',
    ].join('');
    keeps.innerHTML = '<b>Kept:</b> destinations, delay profiles, integrations, OBS service registration, your buffer file on disk.';
    typebox.hidden = true;
    confirm.disabled = false;
    confirm.textContent = 'Reset settings';
    setTimeout(() => $('reset-cancel-btn').focus(), 50);
  }
  m.hidden = false;
  document.addEventListener('keydown', onResetModalKey);
}
function closeResetModal(){
  const m = $('reset-modal');
  if (!m || m.hidden) return;
  m.hidden = true;
  resetScope = null;
  document.removeEventListener('keydown', onResetModalKey);
}
function onResetModalKey(e){
  if (e.key === 'Escape'){ e.preventDefault(); closeResetModal(); }
}
function onResetConfirmInput(){
  if (resetScope !== 'all') return;
  const input = $('reset-confirm-input');
  const confirm = $('reset-confirm-btn');
  confirm.disabled = (input.value.trim() !== 'RESET');
}
async function confirmResetModal(){
  if (!resetScope) return;
  const scope = resetScope;
  const confirm = $('reset-confirm-btn');
  confirm.disabled = true;
  confirm.textContent = scope === 'all' ? 'Resetting everything…' : 'Resetting…';
  const oldWebPort = lastConfig ? +lastConfig.web_port : 0;
  try {
    const r = await fetch('/config/reset?scope=' + scope, {method:'POST'});
    if (!r.ok){
      const j = await r.json().catch(() => ({error: r.statusText}));
      toast('Reset failed: ' + (j.error || r.statusText), 'err');
      confirm.disabled = false;
      confirm.textContent = scope === 'all' ? 'Reset everything' : 'Reset settings';
      return;
    }
    closeResetModal();
    const j = await r.json().catch(() => ({}));
    // The default Web UI port is live at once: follow it there, as saving a
    // port change does. Behind a proxy the proxy is the user's to repoint.
    const newWebPort = +j.web_port || 0;
    if (oldWebPort && newWebPort && newWebPort !== oldWebPort && +location.port === oldWebPort){
      waitForRestart({port: newWebPort, text: 'Settings reset. The dashboard moved to port ' + newWebPort + '. Taking you there.'});
      return;
    }
    toast(scope === 'all'
      ? 'Everything reset. Reloading into the wizard…'
      : 'Settings reset to defaults. Reloading…', 'ok');
    // Give the server a moment to flush the new config + persist, then
    // reload. The page reload re-reads /config + /state from the now
    // pristine state and the wizard / settings reflect defaults.
    setTimeout(() => location.reload(), 700);
  } catch (e){
    toast('Reset request failed: ' + e, 'err');
    confirm.disabled = false;
    confirm.textContent = scope === 'all' ? 'Reset everything' : 'Reset settings';
  }
}

// Existing init paths call loadOverlays() before the `const Studio`
// statement runs; the try/catch swallows the temporal-dead-zone error so
// early calls no-op. The Studio loads its own saved list in init().
function loadOverlays(){ try { if(window.ICOverlay) Studio.loadSaved(); } catch(_){} }
// The overlays changed elsewhere (see Live).
function refreshOverlays(){ if (window.ICOverlay) return Studio.refreshSaved(); }

// Turn the clicked button into its own confirmation: "Copy" -> "✓ Copied"
// for ~1.5s. Immediate, on-the-thing-you-touched feedback beats a detached
// toast. The button is passed in explicitly (callers forward `this` from the
// inline handler) rather than read from the deprecated global `window.event`.
const COPIED_HTML = '<span class="cp-ico" aria-hidden="true">✓</span><span>Copied</span>';

// Pin a copy button's width to the wider of its "Copy" / "Copied" states so the
// label swap can never resize it or knock it onto a new line. Measured once,
// lazily, with a hidden probe while the button is on-screen (tab-hidden buttons
// would measure 0). Runs before the first swap, so even that first click is
// shift-free.
function reserveCopyWidth(btn){
  if (btn._copyW != null) return;
  const cur = btn.getBoundingClientRect().width;
  if (!cur) return;                     // hidden (inactive tab) - defer to a later click
  const probe = btn.cloneNode(false);
  probe.removeAttribute('id');
  probe.innerHTML = COPIED_HTML;
  probe.style.cssText += ';position:absolute;left:-9999px;top:0;visibility:hidden;min-width:0;pointer-events:none';
  (btn.parentNode || document.body).appendChild(probe);
  btn._copyW = Math.max(cur, probe.getBoundingClientRect().width);
  probe.remove();
  btn.style.minWidth = btn._copyW + 'px';
}

// Reserve every copy button that's currently on-screen, so its width is locked
// before the first click - no one-time widen on the "Copy" -> "Copied" swap.
// Idempotent and skips off-screen (0-width) buttons, so it's safe to call on
// every tab switch / wizard render; hidden tabs get reserved when shown.
function reserveCopyButtons(root){
  (root || document).querySelectorAll('.ic-btn[onclick*="copy"]').forEach(reserveCopyWidth);
}

function flashCopied(btn){
  // Tolerate an element or an event (defensive: an inline handler that passed
  // `event` still resolves to its button) and no-op if neither yields one.
  if (btn && !(btn instanceof Element) && btn.currentTarget) btn = btn.currentTarget;
  if (!(btn instanceof Element)) return;
  btn = btn.closest('.ic-btn');
  if (!btn) return;
  if (btn._copyT) clearTimeout(btn._copyT);
  if (btn._copyHTML == null) btn._copyHTML = btn.innerHTML;
  reserveCopyWidth(btn);                // lock footprint before the swap
  btn.classList.add('is-copied');
  btn.innerHTML = COPIED_HTML;
  btn._copyT = setTimeout(() => {
    if (btn._copyHTML != null){ btn.innerHTML = btn._copyHTML; btn._copyHTML = null; }
    btn.classList.remove('is-copied');
    btn._copyT = null;
  }, 1500);
}
// The Clipboard API only exists on https and localhost, so a dashboard opened
// over plain http from another machine on the LAN falls back to the older
// copy command, and as a last resort says how to copy by hand.
async function writeClipboard(text){
  try {
    if (navigator.clipboard && window.isSecureContext){ await navigator.clipboard.writeText(text); return true; }
  } catch(_){}
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.setAttribute('readonly', '');
  ta.style.cssText = 'position:fixed;top:0;left:0;opacity:0;pointer-events:none';
  const back = document.activeElement;
  document.body.appendChild(ta);
  ta.select();
  let ok = false;
  try { ok = document.execCommand('copy'); } catch(_){}
  ta.remove();
  if (back && back.focus) back.focus();
  return ok;
}
async function copyText(btn, text, label){
  if (await writeClipboard(text)){ flashCopied(btn); return; }
  toast(`Couldn't copy ${label || 'that'} here. Select it and press Ctrl+C.`, 'err');
}
function copyEl(btn, id, label){ return copyText(btn, $(id).textContent, label); }

