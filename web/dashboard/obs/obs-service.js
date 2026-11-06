// - OBS service registration --------------------─
// Patches OBS's services.json so InstantClone shows up in the Service
// dropdown like Twitch/YouTube. A PC can hold an installed OBS and portable
// ones, each with its own list, so every OBS is a row the streamer ticks.
// OBS reads its list when it starts and writes it back from memory when it
// quits, so the server refuses a change to an OBS that is open, and that row
// says to close it. The wizard and the OBS tab show the same list, with the
// same ticks. Refreshed when the tab opens and whenever an OBS changes
// (opened, closed, registered elsewhere): see Live.
let obsInstalls = null;
const obsPicked = new Map();   // install id -> ticked, kept across refreshes
const OBS_INSTALL_PILL = {
  missing: ['bad', 'Open it once'],
  current: ['ok', 'Registered'],
  stale: ['warn', 'Out of date'],
  other: ['warn', 'Other InstantClone'],
  none: ['', 'Not registered'],
};
async function loadObsInstalls(){
  try { obsInstalls = await (await fetch('/obs/installs')).json(); }
  catch(_){ obsInstalls = null; }
  return obsInstalls;
}
const obsInstallList = () => (obsInstalls && obsInstalls.installs) || [];
const obsTicked = () => obsInstallList().filter(i => obsPicked.get(i.id) && i.state !== 'missing');
function obsInstallPill(i){
  if (i.running && i.state !== 'current') return ['warn', 'Close it first'];
  if (i.state === 'other' && i.other_port) return ['warn', 'Other InstantClone :' + i.other_port];
  return OBS_INSTALL_PILL[i.state] || ['', i.state];
}
// One frame per host, built once; rows are then kept by id so a refresh
// updates them in place and only a new OBS animates in.
function obsInstallsFrame(host){
  host.innerHTML = `<ul class="obs-inst-list"></ul>
    <div class="obs-none" hidden>
      <span class="obs-none-mark" aria-hidden="true">${APP_MARK.obs}<i></i><i></i></span>
      <div class="obs-none-t">We can't find OBS on this PC</div>
      <div class="obs-none-s">Already have it? Open it once and it shows up here by itself.</div>
      <a class="ic-btn ic-btn-primary" href="https://obsproject.com/download" target="_blank" rel="noopener">Download OBS</a>
    </div>
    <div class="obs-inst-foot">
      <button type="button" class="obs-add-toggle" hidden>Using a portable OBS?</button>
      <div class="obs-inst-btns">
        <button type="button" class="ic-btn ic-btn-ghost obs-unreg obs-inst-unreg" hidden>Unregister</button>
        <button type="button" class="ic-btn ic-btn-primary obs-inst-reg">Register with OBS</button>
      </div>
    </div>`;
  host.querySelector('.obs-add-toggle').onclick = e => obsPickPortable(e.currentTarget);
  host.querySelector('.obs-inst-reg').onclick = e => obsChangeTicked(true, e.currentTarget);
  host.querySelector('.obs-inst-unreg').onclick = e => obsChangeTicked(false, e.currentTarget);
}
function obsInstallRow(inst){
  const row = document.createElement('li');
  row.className = 'obs-inst';
  row.dataset.id = inst.id;
  row.innerHTML = `<label class="obs-inst-pick">
      <input type="checkbox" class="obs-inst-cb">
      <span class="obs-inst-box" aria-hidden="true"><svg viewBox="0 0 16 16"><path d="M3.5 8.5l3 3 6-7"/></svg></span>
      <span class="obs-inst-mark">${APP_MARK.obs}</span>
      <span class="obs-inst-text"><span class="obs-inst-name"></span><span class="obs-inst-path mono"></span></span>
    </label>
    <span class="ic-pill obs-reg-pill"><span class="dot"></span><span class="obs-inst-pill-l"></span></span>
    <span class="obs-inst-acts">
      <button type="button" class="obs-inst-act" data-act="change" aria-label="Change where this OBS is">
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M10.5 2.5l3 3L6 13H3v-3z"/></svg>
      </button>
      <button type="button" class="obs-inst-act danger" data-act="remove" aria-label="Remove this OBS">
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.6 8.5h5.8l.6-8.5"/></svg>
      </button>
    </span>`;
  row.querySelector('.obs-inst-cb').onchange = e => {
    obsPicked.set(inst.id, e.target.checked);
    renderObsInstalls();
  };
  row.querySelector('[data-act="change"]').onclick = e => obsPickPortable(e.currentTarget, inst.id);
  row.querySelector('[data-act="remove"]').onclick = e => obsForgetPortable(e.currentTarget, inst.id);
  return row;
}
// A row that leaves folds shut before it goes, so the list closes the gap.
function obsInstallRowOut(row){
  if (row.classList.contains('leaving')) return;
  row.style.height = row.offsetHeight + 'px';
  void row.offsetHeight;
  row.classList.add('leaving');
  row.style.height = '0px';
  const gone = () => row.remove();
  row.addEventListener('transitionend', e => { if (e.propertyName === 'height') gone(); });
  setTimeout(gone, 450);
}
function renderObsInstallsInto(host){
  if (!host) return;
  if (!host.querySelector('.obs-inst-list')) obsInstallsFrame(host);
  const list = host.querySelector('.obs-inst-list');
  const installs = obsInstallList();
  const ids = new Set(installs.map(i => i.id));
  [...list.children].forEach(row => { if (!ids.has(row.dataset.id)) obsInstallRowOut(row); });
  installs.forEach((inst, idx) => {
    let row = [...list.children].find(r => r.dataset.id === inst.id && !r.classList.contains('leaving'));
    if (!row){
      row = obsInstallRow(inst);
      row.style.animationDelay = (idx * 45) + 'ms';
      list.appendChild(row);
    }
    const ticked = !!obsPicked.get(inst.id) && inst.state !== 'missing';
    const cb = row.querySelector('.obs-inst-cb');
    cb.checked = ticked;
    cb.disabled = inst.state === 'missing';
    row.classList.toggle('picked', ticked);
    const name = inst.portable ? 'OBS Studio (portable)' : 'OBS Studio';
    const [tone, label] = obsInstallPill(inst);
    const nameEl = row.querySelector('.obs-inst-name'), pathEl = row.querySelector('.obs-inst-path');
    if (nameEl.textContent !== tr(name, nameEl)) nameEl.textContent = name;
    if (pathEl.textContent !== inst.path){ pathEl.textContent = inst.path; pathEl.title = inst.path; }
    setClassName(row.querySelector('.obs-reg-pill'), 'ic-pill obs-reg-pill' + (tone ? ' ' + tone : ''));
    setText(row.querySelector('.obs-inst-pill-l'), label);
    // Only a portable OBS was added by hand; the installed one is found on its own.
    const acts = row.querySelector('.obs-inst-acts');
    acts.hidden = !inst.portable;
    acts.querySelectorAll('button').forEach(b => {
      b.disabled = inst.running;
      b.title = inst.running ? 'Close this OBS first'
        : b.dataset.act === 'change' ? 'Moved it? Pick its obs64.exe again' : 'Remove from the list';
    });
  });
  // Nothing to register: say what to do about it instead of offering a button.
  const noObs = !!obsInstalls && !installs.length;
  host.querySelector('.obs-none').hidden = !noObs;
  host.querySelector('.obs-inst-foot').classList.toggle('none', noObs);
  host.querySelector('.obs-add-toggle').hidden = !(obsInstalls && obsInstalls.portable_pick);
  const ticked = obsTicked();
  const reg = host.querySelector('.obs-inst-reg'), unreg = host.querySelector('.obs-inst-unreg');
  reg.disabled = !ticked.length;
  reg.hidden = noObs;
  setText(reg, ticked.length > 1 ? `Register in ${ticked.length} OBS` : 'Register with OBS');
  unreg.hidden = !ticked.some(i => i.state !== 'none');
  // One bright button at a time in the wizard: Register until an OBS is
  // registered, then the step's confirm. With no OBS the bright one is
  // Download, and "I've set this up" only joins for a Custom RTMP setup.
  if (host.closest('#wizard')){
    const registerFirst = ticked.length > 0 && !installs.some(i => i.state === 'current');
    const confirmFirst = !registerFirst && !noObs;
    reg.classList.toggle('ic-btn-primary', registerFirst);
    reg.classList.toggle('ic-btn-ghost', !registerFirst);
    $('wiz-obs-confirm').classList.toggle('ic-btn-primary', confirmFirst);
    $('wiz-obs-confirm').classList.toggle('ic-btn-ghost', !confirmFirst);
    $('wiz-obs-service').classList.toggle('no-obs', noObs);
    $('obs-flow').classList.toggle('no-obs', noObs);
    document.querySelector('[data-node="connect"] .obs-node-hint').hidden = noObs;
    syncWizObsConfirm();
    syncWizNext();
  }
}
// With no OBS, confirming only means something for a Custom RTMP setup.
function syncWizObsConfirm(){
  const noObs = $('obs-flow').classList.contains('no-obs');
  $('wiz-obs-confirm').hidden = noObs && !$('wiz-obs-custom').classList.contains('is-open');
}
// The OBS tab card's own pill and line sum the rows up.
function renderObsRegSummary(){
  const card = $('obs-reg-card');
  if (!card) return;
  const installs = obsInstallList();
  const any = s => installs.some(i => i.state === s);
  const view = !obsInstalls ? ['bad', 'Unknown', 'Could not ask InstantClone about OBS. Is it still running?']
    : !installs.length ? ['bad', 'OBS not found', '']
    : any('stale') ? ['warn', 'Out of date', obsInstalls.update_pending
        ? 'An entry points at old ports. It updates by itself when that OBS closes.'
        : "An entry points at old ports, so that OBS can't reach InstantClone. Tick it and register again."]
    : any('current') ? ['ok', 'Registered', 'OBS lists InstantClone as a service. To stream through it:']
    : ['', 'Not registered', 'Tick the OBS you stream with and register: InstantClone joins its Service list.'];
  card.dataset.state = any('current') && !any('stale') ? 'current' : 'none';
  setClassName($('obs-reg-pill'), 'ic-pill obs-reg-pill' + (view[0] ? ' ' + view[0] : ''));
  setText($('obs-reg-pill-l'), view[1]);
  setText($('obs-reg-status'), view[2]);
  $('obs-reg-steps').hidden = !any('current');
}
function renderObsInstalls(){
  // A new OBS starts ticked when it needs a click; one registered already doesn't.
  obsInstallList().forEach(i => {
    if (!obsPicked.has(i.id)) obsPicked.set(i.id, i.state === 'none' || i.state === 'stale');
  });
  renderObsInstallsInto($('obs-installs'));
  renderObsInstallsInto($('wiz-obs-installs'));
  renderObsRegSummary();
}
async function refreshObsRegisterStatus(){
  await loadObsInstalls();
  renderObsInstalls();
}
async function obsChangeTicked(register, btn){
  const ids = obsTicked().filter(i => register || i.state !== 'none').map(i => i.id);
  if (!ids.length) return;
  btn.disabled = true;
  const r = await postAction(register ? '/obs/register' : '/obs/unregister', new URLSearchParams({ids: ids.join('\n')}));
  btn.disabled = false;
  if (r.ok){
    toast((r.data && r.data.message) || 'Done', 'ok');
    // Done with those: a fresh tick is needed to change them again.
    ids.forEach(id => obsPicked.set(id, false));
  } else {
    toast(r.error, 'err');
  }
  const msg = $('obs-reg-msg');
  if (msg) msg.textContent = r.ok ? '' : r.error;
  await refreshObsRegisterStatus();
}
// Windows' own Open dialog, opened by InstantClone on this PC: the browser
// can't hand over a real path. The link waits while the dialog is open.
// `replace`: the row whose OBS moved, so the pick takes its place.
async function obsPickPortable(btn, replace){
  const label = btn.textContent;
  btn.disabled = true;
  btn.classList.add('waiting');
  if (!replace) btn.textContent = 'Pick its obs64.exe in the window that opened…';
  const r = await postAction('/obs/portable/pick', new URLSearchParams({replace: replace || ''}));
  btn.disabled = false;
  btn.classList.remove('waiting');
  if (!replace) btn.textContent = label;
  if (r.ok && r.data && r.data.cancelled) return;
  toast(r.ok ? r.data.message : r.error, r.ok ? 'ok' : 'err');
  if (r.ok) await refreshObsRegisterStatus();
}
async function obsForgetPortable(btn, id){
  btn.disabled = true;
  const r = await postAction('/obs/portable/forget', new URLSearchParams({id}));
  if (!r.ok){ btn.disabled = false; toast(r.error, 'err'); return; }
  obsPicked.delete(id);
  toast(r.data.message, 'ok');
  await refreshObsRegisterStatus();
}

