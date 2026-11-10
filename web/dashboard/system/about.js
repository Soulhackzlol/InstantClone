// - About sub-tab: version + update check --------------─
let aboutAutoCheckedOnce = false;
function aboutMaybeAutoCheck(){
  if (aboutAutoCheckedOnce) return;
  aboutAutoCheckedOnce = true;
  aboutCheckUpdate(true);
}
// Compare two SemVer-ish strings. Tolerates a leading 'v' and trailing
// pre-release / build suffixes (anything after '-' or '+'). Missing
// segments default to 0 so '0.1.4' vs '0.1.4.0' compares equal.
function semverCmp(a, b){
  const parse = s => String(s || '').replace(/^v/i,'').split(/[-+]/,1)[0].split('.').map(n => parseInt(n,10) || 0);
  const pa = parse(a), pb = parse(b);
  const len = Math.max(pa.length, pb.length);
  for (let i = 0; i < len; i++){
    const x = pa[i] || 0, y = pb[i] || 0;
    if (x < y) return -1;
    if (x > y) return 1;
  }
  return 0;
}

let latestReleaseUrl = 'https://github.com/Soulhackzlol/InstantClone/releases/latest';
async function aboutCheckUpdate(silent){
  const ver = $('sys-about-version');
  const pill = $('sys-about-pill');
  const msg = $('sys-about-msg');
  const btn = $('sys-about-check-btn');
  const upd = $('sys-about-update-btn');
  if (upd) upd.hidden = true; // only the "update available" branch reveals it
  if (btn) btn.disabled = true;
  if (msg && !silent) msg.textContent = 'Checking github.com/Soulhackzlol/InstantClone…';
  try {
    const r = await fetch('/update-check');
    const j = await r.json();
    renderUpdateChanges(j);
    if (ver && j.current) ver.textContent = 'v' + j.current;
    if (pill) {
      pill.hidden = false;
      pill.classList.remove('ok','new','ahead','err');
      if (j.error) {
        pill.classList.add('err'); pill.textContent = 'check failed';
        if (msg){
          // The error is text from the network: never markup.
          const link = document.createElement('a');
          link.href = j.release_url || 'https://github.com/Soulhackzlol/InstantClone/releases';
          link.target = '_blank'; link.rel = 'noreferrer'; link.textContent = 'View releases';
          msg.replaceChildren('Could not reach GitHub: ' + (j.error || 'unknown') + '. ', link);
        }
      } else if (j.update_available && j.latest) {
        pill.classList.add('new'); pill.textContent = updateLabel(j);
        latestReleaseUrl = j.release_url || latestReleaseUrl;
        // Updating is optional. The button does it for you (download +
        // verify + swap + restart); the link is the manual route. Either
        // way the config survives, so say so.
        if (upd){ upd.hidden = false; upd.textContent = j.rebuild ? 'Install the new build' : 'Update to v' + j.latest; }
        const what = j.rebuild ? 'A newer build of v' + escAttr(j.latest) + ' is out, with fixes published under the same version number. ' : '';
        if (msg) msg.innerHTML = what + 'Optional: your settings, destinations, and overlays are kept either way. Use the button, or <a href="' + escAttr(j.release_url) + '" target="_blank" rel="noreferrer">download v' + escAttr(j.latest) + '</a> and replace the exe yourself.';
      } else if (j.latest && semverCmp(j.current, j.latest) > 0) {
        // Build is ahead of the latest published tag - either a pre-release
        // dev build, a build off main between tags, or a local checkout.
        // Either way, the user should know they are not on a shipped artifact.
        pill.classList.add('ahead'); pill.textContent = 'dev build · ahead of v' + j.latest;
        if (msg) msg.innerHTML = 'This build is newer than the latest tagged release (<a href="' + escAttr(j.release_url) + '" target="_blank" rel="noreferrer">v' + escAttr(j.latest) + '</a>). Pre-release or local build.';
      } else {
        pill.classList.add('ok'); pill.textContent = 'up to date';
        if (msg) msg.textContent = silent ? '' : 'You are on the latest release.';
      }
    }
  } catch(e) {
    renderUpdateChanges(null);
    if (pill) { pill.hidden = false; pill.classList.remove('ok','new','ahead'); pill.classList.add('err'); pill.textContent = 'check failed'; }
    if (msg) msg.textContent = 'Check failed: ' + e;
  } finally {
    if (btn) btn.disabled = false;
  }
}

// One-click update: POST /update/apply downloads and verifies the new exe,
// then the server restarts like the Restart button does, installing it at
// the end of its clean shutdown. We show progress, then wait for the fresh
// process and reload. Manual download stays the fallback in every error.
let _updating = false;
async function applyUpdate(){
  if (_updating) return;
  // Applying an update restarts the exe, which tears down every egress -
  // same live-stream drop the tray Quit guards against. Confirm only while
  // OBS is actually publishing; when nothing is live this stays one click.
  if (lastState && lastState.ingest_alive &&
      !confirm(`${liveNowText()} Updating restarts the app and drops every destination.`
        + (lastState.multitrack_video ? ` With Enhanced Broadcasting, Twitch only comes back once you stop and start streaming in ${encoderApp() || 'your app'}.` : '')
        + '\n\nUpdate anyway?')){
    return;
  }
  const upd = $('sys-about-update-btn');
  const chk = $('sys-about-check-btn');
  const msg = $('sys-about-msg');
  const label = upd ? upd.textContent : '';
  _updating = true;
  if (upd){ upd.disabled = true; upd.textContent = 'Downloading…'; }
  if (chk) chk.disabled = true;
  if (msg) msg.textContent = 'Downloading and verifying the update. Keep InstantClone open.';
  let j = null;
  try { j = await (await fetch('/update/apply', {method:'POST'})).json(); } catch(_){ /* answered below */ }
  if (!j || !j.ok){
    _updating = false;
    if (upd){ upd.disabled = false; upd.textContent = label; }
    if (chk) chk.disabled = false;
    if (msg){
      // The error can quote GitHub or the OS: text, never markup.
      const err = document.createElement('span');
      err.className = 'err';
      err.textContent = (j && j.error) || 'The update could not start: InstantClone did not answer.';
      const link = document.createElement('a');
      link.href = latestReleaseUrl; link.target = '_blank'; link.rel = 'noreferrer';
      link.textContent = 'update manually';
      msg.replaceChildren(err, ' You can still ', link, '.');
    }
    return;
  }
  // Remembered for the reloaded page, which says so if the old version
  // had to be kept (see checkUpdateOutcome).
  try { sessionStorage.setItem('ic-update-to', JSON.stringify({version: j.version, rebuild: !!j.rebuild})); } catch(_){}
  const what = j.rebuild ? 'the new build of v' + j.version : 'v' + j.version;
  if (upd) upd.textContent = 'Restarting…';
  if (msg) msg.textContent = 'Verified ' + what + '. InstantClone is restarting to install it.';
  waitForRestart({text: 'Installing ' + what + '. This page reconnects by itself.'});
}

// Wait for InstantClone to come back after a restart, an update or a Web UI
// port change, then reload. The old process keeps answering for about a
// second while it shuts down, so an answer alone isn't proof: a fresh one
// shows as a 401 (logins live in memory, so a password-protected dashboard
// lands on its login page) or as an uptime no older than this wait. On a new
// port the old address is gone for good, so any answer there will do.
const RESTART_PATIENCE_MS = 60000;
function waitForRestart(opts){
  opts = opts || {};
  const started = Date.now();
  const origin = opts.port ? location.protocol + '//' + location.hostname + ':' + opts.port : '';
  const target = origin ? origin + '/' : '';
  restartNotice(opts.text || 'Restarting InstantClone. This page reconnects by itself.', 'info');
  let late = false, busy = false;
  const iv = setInterval(async () => {
    if (busy) return;
    busy = true;
    try {
      if (await isBackUp(origin, started)){
        clearInterval(iv);
        // Same address: reload, as assigning an unchanged URL with a #hash
        // would only scroll.
        if (origin) location.href = target; else location.reload();
        return;
      }
    } catch(_){ /* still down - keep waiting */ }
    finally { busy = false; }
    if (!late && Date.now() - started > RESTART_PATIENCE_MS){
      // Keep polling: a "port in use" prompt on the PC can hold the
      // relaunch for as long as the user takes to answer it.
      late = true;
      restartNotice('Restart is taking longer than expected. If the dashboard doesn\'t come back, start InstantClone again from where you keep the exe.', 'err', target);
      const msg = $('sys-about-msg');
      if (msg && _updating) msg.textContent = 'The update is taking longer than expected.';
    }
  }, 1500);
}
async function isBackUp(origin, since){
  if (origin){
    // Another port is another origin: an opaque answer still proves life.
    await fetch(origin + '/state', {mode:'no-cors', cache:'no-store'});
    return true;
  }
  const r = await fetch('/state', {cache:'no-store'});
  if (r.status === 401) return true;
  if (!r.ok) return false;
  const s = await r.json();
  return typeof s.uptime_secs === 'number' && s.uptime_secs <= Math.floor((Date.now() - since) / 1000);
}
// The restart's progress in the toast rail, so it is seen on any tab. Stays
// until the page reloads; `href` adds the address to open by hand.
function restartNotice(text, kind, href){
  let el = $('restart-notice');
  if (!el){
    el = document.createElement('div');
    el.id = 'restart-notice';
    el.setAttribute('role', 'status');
    el.setAttribute('aria-live', 'polite');
    el.innerHTML = '<span class="ic-toast-icon"></span><span></span>';
    $('toast-rail').appendChild(el);
  }
  el.className = 'ic-toast ' + kind;
  el.firstChild.textContent = kind === 'err' ? '!' : 'i';
  const body = el.lastChild;
  body.textContent = text;
  if (href){
    const a = document.createElement('a');
    a.href = href; a.textContent = href;
    body.append(' ', a);
  }
}
// After a one-click update the page reloads on the new process. If that same
// update is still on offer, the install didn't happen and the old version was
// kept (the staged file failed its last check, or antivirus took it): say so.
async function checkUpdateOutcome(){
  let wanted = null;
  try {
    wanted = JSON.parse(sessionStorage.getItem('ic-update-to') || 'null');
    sessionStorage.removeItem('ic-update-to');
  } catch(_){}
  if (!wanted) return;
  try {
    const j = await (await fetch('/update-check')).json();
    if (j.update_available && j.latest === wanted.version && !!j.rebuild === !!wanted.rebuild){
      toast('The update didn\'t install, so InstantClone kept the version you had. Try again in System > About.', 'err', 9000);
    }
  } catch(_){}
}

// Restart the app (e.g. to apply a buffer/port change). The server tears
// down every egress cleanly (deleteStream upstream) and THEN relaunches, so
// this is safe to run mid-stream - but it still drops the live edge for a few
// seconds, so we confirm first when OBS is actually publishing.
let _restarting = false, _quitting = false;
async function restartApp(btn){
  if (_restarting || _quitting) return;
  if (lastState && lastState.ingest_alive &&
      !confirm(`${liveNowText()} Restarting relaunches the app and drops every destination for a few seconds.`
        + (lastState.multitrack_video ? ` With Enhanced Broadcasting, Twitch only comes back once you stop and start streaming in ${encoderApp() || 'your app'}.` : '')
        + '\n\nRestart anyway?')){
    return;
  }
  _restarting = true;
  if (btn){ btn.disabled = true; btn.textContent = 'Restarting…'; }
  // waitForRestart's notice says so, wherever the user is.
  try { await fetch('/app/restart', {method:'POST'}); } catch(_){ /* server going down */ }
  waitForRestart();
}

// Quit the app for good - no relaunch. Same clean egress teardown as the
// Windows tray Quit. Unlike restart we do NOT poll for reconnect: the process
// is gone, so we replace the view with a terminal farewell instead of leaving
// a frozen dashboard that looks broken.
async function quitApp(btn){
  if (_quitting || _restarting) return;
  if (lastState && lastState.ingest_alive &&
      !confirm(`${liveNowText()} Quitting stops the delay and drops every destination.\n\nQuit anyway?`)){
    return;
  }
  if (!confirm('Quit InstantClone? The dashboard will go offline until you launch the app again.')) return;
  _quitting = true;
  if (btn){ btn.disabled = true; btn.textContent = 'Quitting…'; }
  toast('Stopping InstantClone…','info');
  try { await fetch('/app/quit', {method:'POST'}); } catch(_){ /* server going down */ }
  showQuitFarewell();
}

// Full-screen "stopped" overlay shown after Quit. Covers the dashboard so the
// dead poll loop (whose fetches now just fail silently) isn't visible behind a
// frozen UI. Uses theme tokens so it reads correctly in the current palette.
function showQuitFarewell(){
  const el = document.createElement('div');
  el.style.cssText = 'position:fixed;inset:0;z-index:99999;display:flex;align-items:center;'+
    'justify-content:center;text-align:center;padding:24px;background:var(--bg);color:var(--fg)';
  el.innerHTML =
    '<div style="max-width:440px">'+
      '<div style="font-size:34px;line-height:1;margin-bottom:14px;color:var(--fg-3)">■</div>'+
      '<h2 style="margin:0 0 10px;font-size:20px">InstantClone has stopped</h2>'+
      '<p style="margin:0;font-size:14px;line-height:1.55;color:var(--fg-2)">'+
        'Every destination was disconnected cleanly and the buffer freed. '+
        'You can close this tab. Launch InstantClone again to bring the dashboard back.'+
      '</p>'+
    '</div>';
  document.body.appendChild(el);
}

// Swap the handful of OS-specific strings so the dashboard reads native on
// whatever platform the server runs. `os` comes from /config ("windows",
// "linux", "macos"). Called on every config load. The file-manager name is
// cached for the reveal() error toast.
let _fileManager = 'Explorer';
function applyPlatformCopy(os){
  os = os || 'windows';
  _fileManager = os === 'linux' ? 'file manager' : os === 'macos' ? 'Finder' : 'Explorer';
  const onWindows = os === 'windows';
  // Only Windows has a tray to tell.
  hasTray = onWindows;
  const title = $('s-startup-title');
  if (title) title.textContent = onWindows ? 'Start with Windows' : 'Start at login';
  setText($('wel-startup-t'), onWindows ? 'Start InstantClone with Windows?' : 'Start InstantClone at login?');
  const sub = $('s-startup-sub');
  if (sub) sub.textContent = onWindows
    ? 'Launches at login and waits in the tray, so OBS always has somewhere to publish.'
    : 'Launches at login and keeps running in the background, so OBS always has somewhere to publish.';
  const odSub = $('s-open-dash-sub');
  if (odSub) odSub.textContent = onWindows
    ? 'Opens this page in your browser each time the app starts. Turn off to stay in the tray.'
    : 'Opens this page in your browser each time the app starts. Turn off to keep it running in the background.';
  document.querySelectorAll('.reveal-btn').forEach(b => {
    const which = /reveal\('trace'/.test(b.getAttribute('onclick') || '') ? 'log file' : 'buffer file';
    b.textContent = `Show ${which} in ${_fileManager}`;
  });
}

// Open one of our files in the OS file browser. `what` is a fixed keyword the
// server maps to a known path (buffer / trace / overlays).
async function reveal(what){
  try {
    const j = await (await fetch('/reveal/' + what, {method:'POST'})).json();
    if (!j || !j.ok) toast('Could not open ' + _fileManager,'err');
  } catch(_){ toast('Could not open ' + _fileManager,'err'); }
}

