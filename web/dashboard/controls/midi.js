// ── MIDI controller mapping (Windows only) ─────────────────────────────
// Same actions as the hotkeys, but "learn" is server-side: the MIDI
// message arrives at the backend, so we POST /midi/learn then poll
// /midi/poll until it is captured (or cancelled). Reuses the hk-* styles.
let midiBindings = emptyBindings();
let midiAvailable = false;
let midiDevices = [];      // every input the system reports
let midiListening = [];    // the ones we actually hold open
let midiSelected = '';     // '' = every device
let midiLearnAction = null;
let midiPollTimer = null;

// "note:1:36@Launchpad" -> "Note 36 · Launchpad"; "cc:1:20" -> "CC 20 · ch 1".
// The device is what tells two decks apart, so it earns the space: channel
// only shows when there is no device to show instead.
function midiLabel(sig, full){
  const at = (sig || '').indexOf('@');
  const head = at < 0 ? (sig || '') : sig.slice(0, at);
  const device = at < 0 ? '' : sig.slice(at + 1);
  const p = head.split(':');
  if (p.length !== 3) return sig || '';
  const [kind, ch, n] = p;
  const control = kind === 'note' ? 'Note ' + n : kind === 'cc' ? 'CC ' + n : head;
  if (!device) return control + ' · ch ' + ch;
  const shown = (!full && device.length > 18) ? device.slice(0, 17) + '…' : device;
  return control + ' · ' + shown;
}

function midiSetWarn(msg){ const w = $('midi-warn'); if (w) w.textContent = msg || ''; }

function midiRenderCapture(el, action){
  const sig = midiBindings[action] || '';
  el.classList.remove('recording','invalid','empty');
  if (midiLearnAction === action){
    el.classList.add('recording');
    el.innerHTML = '<span class="hk-cue">Press a pad or knob&hellip;</span>';
    el.setAttribute('aria-label', hkTitleOf(action) + ': learning, press a control or click to cancel');
    return;
  }
  if (!sig){
    el.classList.add('empty');
    el.innerHTML = '<span class="hk-cue">Click to learn</span>';
    el.setAttribute('aria-label', hkTitleOf(action) + ': no MIDI control mapped, click to learn');
    return;
  }
  el.innerHTML = '<span class="hk-chip">' + escAttr(midiLabel(sig)) + '</span>';
  el.title = midiLabel(sig, true);
  el.setAttribute('aria-label', hkTitleOf(action) + ': ' + midiLabel(sig, true) + ', click to remap');
}

function midiRenderList(){
  const list = $('midi-list'); if (!list) return;
  list.innerHTML = '';
  for (const [action, title, sub] of HOTKEY_ACTIONS){
    if (!isOffered(action, midiBindings)) continue;
    const row = document.createElement('div');
    row.className = 'hk-row';
    const main = document.createElement('div');
    main.className = 'hk-row-main';
    main.innerHTML = `<div class="hk-row-title">${escAttr(title)}</div>`
      + `<div class="tab-sub hk-row-sub">${escAttr(sub)}</div>`;
    const cap = document.createElement('div');
    cap.className = 'hk-capture';
    cap.dataset.action = action;
    cap.setAttribute('role','button');
    cap.setAttribute('tabindex','0');
    cap.addEventListener('click', () => midiToggleLearn(action));
    cap.addEventListener('keydown', ev => {
      if (ev.key === 'Enter' || ev.key === ' '){ ev.preventDefault(); midiToggleLearn(action); }
      if (ev.key === 'Escape' && midiLearnAction === action) midiCancelLearn();
    });
    const clear = document.createElement('button');
    clear.className = 'hk-clear';
    clear.type = 'button';
    clear.innerHTML = '&times;';
    clear.title = 'Clear ' + title + ' mapping';
    clear.setAttribute('aria-label', 'Clear ' + title + ' MIDI mapping');
    clear.classList.toggle('idle', !midiBindings[action]);
    clear.addEventListener('click', e => { e.stopPropagation(); midiClear(action); });
    row.append(main, cap, clear);
    list.appendChild(row);
    midiRenderCapture(cap, action);
  }
  midiRenderDevices();
}

// The device picker plus the one line that says what is actually being
// heard. Those are different things: a device can be present but held by
// another app, or picked and then unplugged, and both used to read the same
// as "listening".
let midiDeviceSig = null;
function midiRenderDevices(){
  const sel = $('midi-device');
  const sig = JSON.stringify([midiDevices, midiSelected]);
  // Rebuilt only when the choices change: the learn poll runs three times a
  // second, and replacing the options under an open dropdown would close it.
  if (sel && sig !== midiDeviceSig && document.activeElement !== sel){
    midiDeviceSig = sig;
    const opts = ['<option value="">Every connected device</option>'];
    const known = midiDevices.slice();
    if (midiSelected && !known.includes(midiSelected)) known.push(midiSelected);
    known.forEach(d => {
      const gone = !midiDevices.includes(d);
      opts.push(`<option value="${escAttr(d)}"${d === midiSelected ? ' selected' : ''}>`
        + escAttr(d) + (gone ? ' (not connected)' : '') + '</option>');
    });
    sel.innerHTML = opts.join('');
    sel.value = midiSelected || '';
  }
  const note = $('midi-note');
  if (!note) return;
  note.hidden = false;
  if (!midiDevices.length){
    note.textContent = 'No MIDI controller detected. Connect one and it shows up here.';
  } else if (midiListening.length){
    note.textContent = 'Listening to: ' + midiListening.join(', ');
  } else if (midiSelected && !midiDevices.includes(midiSelected)){
    note.textContent = `"${midiSelected}" isn't connected, so nothing is listening.`;
  } else if (!anyMidiBound()){
    // Nothing is held open until something is bound - that is deliberate, so
    // a controller stays usable by your DAW until you actually map a pad.
    note.textContent = 'Detected ' + midiDevices.join(', ') + '. Press Learn on an action to map it.';
  } else {
    note.textContent = 'Detected ' + midiDevices.join(', ') + ', but another app has it open (your other InstantClone, if you run two). Windows gives a MIDI device to one app at a time.';
  }
}

// Whether any action currently has a MIDI control mapped. Drives the idle
// note: with nothing bound we hold no device open on purpose, which is not
// the same thing as another app having taken it.
function anyMidiBound(){
  return Object.values(midiBindings || {}).some(v => v);
}

async function midiSetDevice(name){
  midiSelected = name;
  midiRenderDevices();
  try {
    const body = new URLSearchParams(); body.set('midi_device', name);
    const j = await (await fetch('/config', {method:'POST', body})).json();
    if (j.ok) toast(name ? `Listening to ${name}` : 'Listening to every MIDI device', 'ok');
    else { toast(`Couldn't save the device: ${j.error}`, 'err'); await reloadBindings('midi'); }
  } catch(e){ toast('Save failed: the app may have stopped responding', 'err'); await reloadBindings('midi'); }
  // The listener picks the change up on its next sweep (~2 s); refresh so
  // the note stops claiming the old device.
  setTimeout(midiPollOnce, 2400);
}

async function midiToggleLearn(action){
  if (midiLearnAction === action){ await midiCancelLearn(); return; }
  if (midiLearnAction) await midiCancelLearn();
  // Re-check before refusing. midiAvailable is whatever the last poll saw,
  // and the poll only runs during a learn - so a controller plugged in after
  // this page was opened would otherwise be invisible until a reload, which
  // is exactly the order a first-time user does things in.
  if (!midiAvailable){
    await midiPollOnce();
    if (!midiAvailable){ midiSetWarn('Connect a MIDI controller first.'); return; }
  }
  try {
    const j = await (await fetch('/midi/learn', {method:'POST', body:new URLSearchParams({action})})).json();
    if (!j.ok){ midiSetWarn(j.error || 'Could not start learn'); return; }
    midiLearnAction = action;
    midiSetWarn('');
    midiRenderList();
    midiStartPoll();
  } catch(e){ midiSetWarn('Learn failed: the app may have stopped responding'); }
}

async function midiCancelLearn(){
  const was = midiLearnAction;
  midiLearnAction = null;
  midiStopPoll();
  try { await fetch('/midi/learn/cancel', {method:'POST'}); } catch(e){}
  if (was) midiRenderList();
}

function midiStartPoll(){ midiStopPoll(); midiPollTimer = setInterval(midiPollOnce, 300); }
function midiStopPoll(){ if (midiPollTimer){ clearInterval(midiPollTimer); midiPollTimer = null; } }

// One poll: refresh availability/devices/bindings and, while learning,
// detect when the server has captured (learning back to null).
async function midiPollOnce(){
  let j;
  try { j = await (await fetch('/midi/poll', {method:'POST'})).json(); }
  catch(e){ return; }
  // An answer that isn't the MIDI status (logged out, an error) says nothing
  // about a learn: read as one, it ended it with "Learn timed out".
  if (!j || !('connected' in j)) return;
  midiAvailable = !!j.connected;
  midiDevices = j.devices || [];
  midiListening = j.listening || [];
  midiSelected = j.device || '';
  const before = Object.assign({}, midiBindings);
  if (j.bindings) midiBindings = Object.assign(midiBindings, j.bindings);
  if (j.learning == null){
    const was = midiLearnAction;
    const got = was && midiBindings[was];
    midiLearnAction = null;
    midiStopPoll();
    midiRenderList();
    // One more refresh a beat later. The listener needs a sweep to reopen the
    // device now that a binding exists, and the response that told us the
    // learn finished was taken before that - so without this the panel can
    // settle on "another app has it open" at the end of a mapping that
    // actually worked.
    if (got) setTimeout(midiPollOnce, 700);
    if (was && !got){
      // The listener drops a learn nobody finished, and the app refuses a
      // pad an integration uses: either way the field going back to normal
      // on its own needs a reason attached to it.
      midiSetWarn(j.refused || 'Learn timed out. Click the field and press a control within 30 seconds.');
    }
    if (was && got){
      // One control drives one action, so learning a pad that was already
      // mapped takes it off the other action. Say which, rather than
      // leaving the user to spot the row that went empty.
      const stolen = Object.keys(before).find(a => a !== was && before[a] && !midiBindings[a]);
      toast(`${hkTitleOf(was)}: ${midiLabel(midiBindings[was])}`
        + (stolen ? ` (moved off ${hkTitleOf(stolen)})` : ''), 'ok');
    }
  } else {
    midiRenderList();
  }
}

async function midiClear(action){
  midiBindings[action] = '';
  midiRenderList();
  try {
    const body = new URLSearchParams(); body.set('midi.' + action, '');
    const j = await (await fetch('/config', {method:'POST', body})).json();
    if (j.ok) toast(`${hkTitleOf(action)} mapping cleared`, 'ok');
    else { toast(`Couldn't clear mapping: ${j.error}`, 'err'); await reloadBindings('midi'); }
  } catch(e){ toast('Save failed: the app may have stopped responding', 'err'); await reloadBindings('midi'); }
}

document.addEventListener('change', e => {
  if (e.target && e.target.id === 'midi-device') midiSetDevice(e.target.value);
});

// Same reasoning as the tab switch, one level up: a learn request must not
// outlive the person who made it. Hiding the tab cancels normally; an unload
// gets one sendBeacon, which still lands when a fetch would be cancelled.
// The listener also expires a learn on its own after 30 s, for the browser
// that is killed outright and never runs either handler.
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState !== 'hidden') return;
  if (hkRec) hkCancelRecording();
  if (midiLearnAction) midiCancelLearn();
});
window.addEventListener('pagehide', () => {
  if (hkRec){
    hkRec = null;
    try { navigator.sendBeacon('/hotkeys/capture', new URLSearchParams({on:'0'})); } catch(e){}
  }
  if (!midiLearnAction) return;
  midiLearnAction = null;
  midiStopPoll();
  try { navigator.sendBeacon('/midi/learn/cancel'); } catch(e){}
});

// One-off poll on load to learn availability + device names.
async function midiInit(){
  try {
    const j = await (await fetch('/midi/poll', {method:'POST'})).json();
    midiAvailable = !!j.connected;
    midiDevices = j.devices || [];
    midiListening = j.listening || [];
    midiSelected = j.device || '';
    if (j.bindings) midiBindings = Object.assign(midiBindings, j.bindings);
  } catch(e){}
  midiRenderList();
}

