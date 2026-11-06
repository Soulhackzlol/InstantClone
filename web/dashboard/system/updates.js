// - Updates: the check, the header pill, What's new -----------------------------
// Header pill: is there a newer release? Asked on load, every few hours for
// a dashboard left open for days, and when the tab comes back into view.
// Failures are silent - we don't want offline / rate-limited users to see a
// scary error pill for a passive nicety. The server caches the answer for
// 10 min, so a refresh spree doesn't hammer GitHub.
const UPDATE_RECHECK_MS = 3 * 60 * 60 * 1000;
const UPDATE_FOCUS_RECHECK_MS = 10 * 60 * 1000;
let lastUpdateCheck = 0;
async function checkForUpdate(){
  const pill = $('hdr-update');
  const label = $('hdr-update-v');
  if (!pill || !label) return;
  // Respect the opt-out (a missing field defaults to on). The manual
  // "Check for updates" button bypasses this and always works.
  if (lastConfig && lastConfig.update_check_enabled === false){ pill.hidden = true; return; }
  lastUpdateCheck = Date.now();
  try {
    const r = await (await fetch('/update-check')).json();
    if (!r || r.error) return;
    if (!r.update_available){ pill.hidden = true; return; }
    label.textContent = updateLabel(r);
    // Hovering it says what the update brings; System > About lists it all.
    const brings = (r.changes || []).slice(0, 3).map(c => `${c.kind}: ${c.text}`);
    pill.title = brings.length ? brings.join('\n') : 'Open the updater';
    pill.hidden = false;
  } catch(_){}
}
setInterval(checkForUpdate, UPDATE_RECHECK_MS);
document.addEventListener('visibilitychange', () => {
  if (!document.hidden && lastUpdateCheck && Date.now() - lastUpdateCheck > UPDATE_FOCUS_RECHECK_MS) checkForUpdate();
});
// "Tell me in the tray" only means something while checks run.
// "Tell me in the tray" only means something while checks run, so it shows
// only then: it folds away when they're switched off and back when they're
// on. `animate` is false when the settings load, so a page opens settled.
let hasTray = true;
function syncUpdateTray(animate = true){
  const row = $('s-update-tray-row');
  const hide = !hasTray || !$('s-update-check').checked;
  if (animate) animateHidden(row, hide); else setHidden(row, hide);
}

// After an update (in-app or by hand) the server says what changed since the
// build the user last saw: a card in System > About until "Got it", and a
// quiet header pill on the first load only.
let whatsNew = null;
async function loadWhatsNew(){
  try { whatsNew = await (await fetch('/whats-new')).json(); } catch(_){ return; }
  if (!whatsNew || !whatsNew.show) return;
  const w = whatsNew;
  const title = w.rebuild ? 'Updated to a new build of v' + w.to : 'Updated to v' + w.to;
  $('whats-new-title').textContent = title;
  $('whats-new-sub').textContent = w.from && !w.rebuild
    ? `From v${w.from}. Your settings, destinations and overlays came along.`
    : 'Your settings, destinations and overlays came along.';
  const changes = w.changes || [];
  $('whats-new-list').replaceChildren(...changes.slice(0, UPDATE_CHANGES_SHOWN).map(changeRow));
  $('whats-new-changes').hidden = !changes.length;
  const link = $('whats-new-link');
  const more = changes.length > UPDATE_CHANGES_SHOWN;
  link.href = more && w.changes_url ? w.changes_url : w.release_url;
  link.textContent = more ? 'All ' + changes.length + ' changes on GitHub' : 'Release notes';
  animateHidden($('whats-new'), false);
  const key = [w.from, w.to, w.rebuild, w.changes_url].join('|');
  let shown = true;
  try { shown = localStorage.getItem('ic-whats-new-pill') === key; localStorage.setItem('ic-whats-new-pill', key); } catch(_){}
  if (!shown){
    $('hdr-whats-new-v').textContent = w.rebuild ? 'Updated build' : 'Updated to v' + w.to;
    $('hdr-whats-new').hidden = false;
  }
}
function openWhatsNew(){
  $('hdr-whats-new').hidden = true;
  showTab('system');
  showSubTab('system', 'about');
}
async function dismissWhatsNew(){
  animateHidden($('whats-new'), true);
  fadeOutHidden($('hdr-whats-new'));
  try { await fetch('/whats-new/dismiss', {method:'POST'}); }
  catch(_){ toast('Couldn\'t save that. It may show again next time.', 'err'); }
}

// What an available update brings, from its commits since this build (only
// release builds know their commit). Written as text: a commit subject is
// never markup. Past a handful, the rest are one link away.
const UPDATE_CHANGES_SHOWN = 8;
function renderUpdateChanges(j){
  const box = $('sys-about-changes'), list = $('sys-about-changes-list');
  if (!box || !list) return;
  const changes = (j && j.update_available && j.changes) || [];
  const rows = changes.slice(0, UPDATE_CHANGES_SHOWN).map(changeRow);
  const more = changes.length - UPDATE_CHANGES_SHOWN;
  if (more > 0 && j.changes_url){
    const li = document.createElement('li');
    const a = document.createElement('a');
    a.href = j.changes_url; a.target = '_blank'; a.rel = 'noreferrer';
    a.textContent = `and ${more} more on GitHub`;
    li.appendChild(a);
    rows.push(li);
  }
  list.replaceChildren(...rows);
  if (changes.length) $('sys-about-changes-title').textContent =
    j.rebuild ? 'What the new build brings' : `What v${j.latest} brings`;
  animateHidden(box, !changes.length);
}
// One change: its kind as a badge, then the line. Text only: a commit
// subject is never markup.
function changeRow(c){
  const li = document.createElement('li');
  const kind = document.createElement('b');
  kind.textContent = c.kind;
  kind.dataset.kind = c.kind;
  li.append(kind, c.text);
  return li;
}
// What an available update is called: a newer version, or a newer build of
// this same version (`rebuild`, spotted by the exe's SHA-256).
function updateLabel(r){
  return r.rebuild ? 'New v' + r.latest + ' build' : 'v' + r.latest + ' available';
}
// The header "update available" pill opens the in-app updater (System > About)
// instead of GitHub, so one click leads to the one-click Update button.
function openUpdateInAbout(){
  showTab('system');
  showSubTab('system','about');
  try { aboutCheckUpdate(); } catch(_){}
}
