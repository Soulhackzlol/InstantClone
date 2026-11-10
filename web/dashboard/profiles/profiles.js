// - Profiles -----------------------------
async function loadProfiles(){
  try {
    const r = await fetch('/profiles');
    const j = await r.json();
    profiles = (Array.isArray(j) ? j : (j.profiles || [])).filter(p => !isDeleting('profile:' + p.name));
    renderCpProfiles(); renderProfilesPane();
  } catch(_){}
}
// The profile being edited in place: its saved name, and what was typed
// so far (kept, so a redraw mid-edit doesn't lose it).
let profileEdit = null;
// The starting profiles have Spanish; a name someone typed stays as typed.
const STARTING_PROFILES = new Set(['Quick', 'Standard', 'Tournament', 'Long']);
const profileShownName = name => (STARTING_PROFILES.has(name) ? tr(name) : name);
function renderProfilesPane(){
  const c = $('profile-list');
  const cap = currentBufferCapMs();
  const ds = lastState ? PHASE_TO_STATE[lastState.phase] : 'off';
  const armed = lastState ? lastState.armed_delay_ms : 0;
  // Same anti-flicker guard as the hero chips: rebuilt every tick, this
  // pane flickered too. Only rebuild when the rendered content changes.
  const sig = JSON.stringify([(profiles || []).map(p => [p.name, p.delay_ms]), ds, armed,
    (profiles || []).map(p => (cap > 0 && p.delay_ms > cap) ? requiredMBForMs(p.delay_ms) : 0), profileEdit && profileEdit.name]);
  if (c._sig === sig) return;
  c._sig = sig;
  // Deleted or renamed from another window: there's nothing left to edit.
  if (profileEdit && !(profiles || []).some(p => p.name === profileEdit.name)) profileEdit = null;
  // Typing in the row being edited goes on in the same field after a redraw.
  const typing = c.contains(document.activeElement) ? document.activeElement.dataset.f : null;
  c.innerHTML = '';
  if (!(profiles || []).length){
    c.innerHTML = '<div class="profile-empty"><b>No saved delays yet</b>'
      + '<span>Name a delay below, like "Tournament" at 300 seconds. It shows up as a one-click chip under the delay input, ready to arm.</span></div>';
    return;
  }
  profiles.forEach(p => {
    if (profileEdit && profileEdit.name === p.name){ c.appendChild(profileEditRow()); return; }
    const isCur = ds !== 'off' && lastState && lastState.armed_delay_ms === p.delay_ms;
    const tooBig = cap > 0 && p.delay_ms > cap;
    const b = document.createElement('button');
    b.className = 'profile-row' + (isCur ? ' current' : '') + (tooBig ? ' too-big' : '');
    b.innerHTML = `<span class="profile-bolt">⚡</span><span class="profile-name" translate="no"></span><span class="profile-secs mono"></span>`;
    b.querySelector('.profile-name').textContent = profileShownName(p.name);
    b.querySelector('.profile-secs').textContent = fmtDelay(p.delay_ms);
    if (isCur){
      const tag = document.createElement('span'); tag.className = 'profile-tag'; tag.textContent = ds;
      b.appendChild(tag);
    }
    const needMB = tooBig ? requiredMBForMs(p.delay_ms) : 0;
    if (tooBig){
      // Quiet: "needs X MB" with a small inline link to the buffer setting.
      // Nothing auto-changes; the row itself isn't armable.
      const warn = document.createElement('span'); warn.className = 'profile-warn';
      warn.append(`too long: needs ${needMB} MB`);
      const link = document.createElement('a');
      link.textContent = 'Bigger buffer →';
      link.onclick = (e) => { e.stopPropagation(); jumpToBufferSetting(needMB); };
      warn.appendChild(link);
      b.appendChild(warn);
      b.title = `${fmtDelay(p.delay_ms)} is too long for the buffer at the current bitrate: it needs at least ${needMB} MB (you have ${runningBufferMb()} MB, about ${fmtDelay(cap)}).`;
    } else if (ds === 'active' && !isCur){
      b.title = `Adjust live delay to ${p.name}`;
    } else if (ds === 'armed' && !isCur){
      b.title = `Re-arm at ${p.name}`;
    }
    const edit = document.createElement('span'); edit.className = 'ic-btn-tiny'; edit.textContent = '✎';
    edit.title = 'Rename or change the delay';
    edit.onclick = (e) => { e.stopPropagation(); startProfileEdit(p); };
    const x = document.createElement('span'); x.className = 'ic-btn-tiny'; x.textContent = '✕';
    x.title = 'Delete';
    x.onclick = (e) => { e.stopPropagation(); delProfile(p.name); };
    b.append(edit, x);
    // Usable rows arm the delay; too-big rows do nothing on click (the
    // inline link is the only action).
    b.onclick = tooBig ? null : () => arm(p.delay_ms);
    c.appendChild(b);
  });
  const field = typing && c.querySelector(`.editing [data-f="${typing}"]`);
  if (field) field.focus();
}
// Why a profile can't be saved as typed, or '' when it can.
function profileProblem(name, secs){
  if (!name) return 'Give the profile a name first';
  if (!secs || secs <= 0 || secs > DELAY_MAX_S) return 'Set a delay between 1 second and 1 hour';
  // Refuse to save a profile that the current buffer can't even hold -
  // the user wouldn't be able to arm it anyway, and a saved-but-unusable
  // entry is just clutter waiting to bite them. Capacity math uses
  // current bitrate if streaming, otherwise a 10 Mbps reference.
  const cap = currentBufferCapMs();
  const ms = secs * 1000;
  if (cap > 0 && ms > cap){
    const needMB = requiredMBForMs(ms);
    const rate = lastState && lastState.capacity_measured ? capacityRate() : `a typical ${capacityRate()}`;
    return `Can't save a ${fmtDelay(ms)} profile: at ${rate} the ${runningBufferMb()} MB buffer holds about ${fmtDelay(cap)}. Make it at least ${needMB} MB in System first.`;
  }
  return '';
}
// Saves a profile (`was`: the one it replaces when edited); false, said
// in a toast, when it couldn't.
async function saveProfile(name, secs, was){
  const problem = profileProblem(name, secs);
  if (problem){ toast(problem, 'err'); return false; }
  const body = new URLSearchParams({name, delay_ms: String(secs*1000)});
  if (was) body.set('was', was);
  const r = await fetch('/profiles',{method:'POST', body});
  if (!r.ok){
    // Surface server error (e.g. duplicate name) instead of a flat "Failed"
    let msg = `Couldn't save profile (HTTP ${r.status})`;
    try { const j = await r.json(); if (j.error) msg = `Couldn't save: ${j.error}`; } catch(_){}
    toast(msg, 'err');
    return false;
  }
  toast('Profile saved','ok');
  loadProfiles();
  return true;
}
async function addProfile(){
  const saved = await saveProfile($('prof-name').value.trim(), parseInt($('prof-secs').value || '0', 10));
  if (saved){ $('prof-name').value=''; $('prof-secs').value=''; }
}
function startProfileEdit(p){
  profileEdit = {name:p.name, draftName:p.name, draftSecs:String(Math.round(p.delay_ms / 1000))};
  renderProfilesPane();
  const field = document.querySelector('#profile-list .editing input');
  if (field){ field.focus(); field.select(); }
}
function stopProfileEdit(){
  profileEdit = null;
  renderProfilesPane();
}
// The row being edited: its name and seconds as fields, Save and Cancel.
// Enter saves, Esc cancels.
function profileEditRow(){
  const row = document.createElement('form');
  row.className = 'profile-row editing';
  row.innerHTML = `<span class="profile-bolt">⚡</span>
    <input class="ic-input" data-f="name" aria-label="Profile name" spellcheck="false">
    <input class="ic-input mono" data-f="secs" type="number" min="1" max="${DELAY_MAX_S}" step="1" aria-label="Seconds">
    <button type="submit" class="ic-btn ic-btn-primary">Save</button>
    <button type="button" class="ic-btn ic-btn-ghost" data-f="cancel">Cancel</button>`;
  const name = row.querySelector('[data-f="name"]'), secs = row.querySelector('[data-f="secs"]');
  name.value = profileEdit.draftName;
  secs.value = profileEdit.draftSecs;
  name.oninput = () => { profileEdit.draftName = name.value; };
  secs.oninput = () => { profileEdit.draftSecs = secs.value; };
  row.querySelector('[data-f="cancel"]').onclick = stopProfileEdit;
  row.onkeydown = e => { if (e.key === 'Escape'){ e.preventDefault(); stopProfileEdit(); } };
  row.onsubmit = async e => {
    e.preventDefault();
    const was = profileEdit.name;
    if (await saveProfile(name.value.trim(), parseInt(secs.value || '0', 10), was) && profileEdit && profileEdit.name === was) stopProfileEdit();
  };
  return row;
}
function delProfile(name){
  deleteWithUndo('profile:' + name, `${name} deleted`, async () => {
    const r = await postAction('/profiles/delete', new URLSearchParams({name}), true);
    if (!r.ok) toast(`Could not delete ${name}: ${r.error}`, 'err');
    loadProfiles();
  }, loadProfiles);
  profiles = profiles.filter(p => p.name !== name);
  renderCpProfiles(); renderProfilesPane();
}
// Jump to System -> General (buffer size) and light the field up. The size
// it needs is offered on the toast, never typed in behind the user's back;
// taking it only fills the field (the Save bar appears, Discard undoes it).
// Buffer changes need a restart (the DiskRing is immutable once mapped);
// the restart banner says so once it is saved. mb 0 = just show the place.
function jumpToBufferSetting(mb){
  showTab('system');
  showSubTab('system', 'general');
  const el = $('s-bmb');
  const offer = el && mb > (parseInt(el.value, 10) || 0);
  if (el){
    el.classList.remove('flash'); void el.offsetWidth; el.classList.add('flash');
    el.focus({preventScroll: true});
    el.scrollIntoView({behavior: motionOK() ? 'smooth' : 'auto', block:'center'});
  }
  const fill = () => {
    el.value = mb;
    el.dispatchEvent(new Event('input', {bubbles: true}));
    toast('Press Save, then restart InstantClone to use it.', 'info', 5000);
  };
  if (offer) toast(`It needs at least ${mb} MB.`, 'info', 9000, {label: `Use ${mb} MB`, run: fill});
  else toast(lastState && lastState.restart_pending ? 'Saved already: restart InstantClone to use the new buffer.'
       : 'Make the buffer bigger here, Save, then restart.', 'info', 5000);
}
// "Show me" in the buffer advice (hero warning, profile chip message).
['cp-cap-warn', 'cp-chip-msg'].forEach(id => {
  const el = $(id);
  if (el) el.addEventListener('click', e => {
    const b = e.target.closest('.cap-show');
    if (b) jumpToBufferSetting(Number(b.dataset.mb) || 0);
  });
});

