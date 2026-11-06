// ── Global hotkeys editor (Windows only) ───────────────────────────────
// Click a capture field, press a modifier+key combo, and it commits and
// saves instantly. Chips render the current binding; recording and invalid
// states carry the whole affordance. Order here is display-only (most-used
// first) and independent of the server-side action ids.
const HOTKEY_ACTIONS = [
  ['toggle',    'Toggle delay',        'Go delayed when live, cut to live when delayed. One key, on and off.'],
  ['activate',  'Activate delay',      'Switch the armed delay on.'],
  ['cut',       'Cut to live',         'Back to live, keeping the delay armed so the next Activate is instant.'],
  ['arm',       'Arm / disarm',        'Press once to arm at your default delay; press again to disarm and free the memory.'],
  ['cut_after', 'Cut after this airs', 'Press once to schedule a cut the instant the live edge reaches every viewer; press again to cancel it.'],
  ['end_hold',  'End crash protection', 'While the reconnect screen is on air after an OBS crash, end the stream on every destination now.'],
];
const emptyBindings = () => ({toggle:'',arm:'',activate:'',cut:'',cut_after:'',end_hold:''});
let hkBindings = emptyBindings();
let hkRec = null; // {action, el} while recording, else null
let hkWarnTimer = null;

const hkTitleOf = a => (HOTKEY_ACTIONS.find(x => x[0] === a) || [,''])[1];
// What an integration's delay step did, for the toast (its "arm" sets the
// delay rather than toggling it).
const FIRED_BY_INTEGRATION = {arm:'Set the delay', disarm:'Disarm the delay'};

// Named keys beyond letters/digits/F-keys, keyed by KeyboardEvent.code and
// mapped to the exact tokens the server's NAMED_KEYS table expects.
const HK_CODE_MAP = {
  Space:'Space', Tab:'Tab', Escape:'Esc', Enter:'Enter', NumpadEnter:'Enter', Backspace:'Backspace',
  ArrowLeft:'Left', ArrowRight:'Right', ArrowUp:'Up', ArrowDown:'Down',
  PageUp:'PageUp', PageDown:'PageDown', End:'End', Home:'Home', Insert:'Insert', Delete:'Delete',
  NumpadMultiply:'NumMultiply', NumpadAdd:'NumAdd', NumpadSubtract:'NumSubtract',
  NumpadDecimal:'NumDecimal', NumpadDivide:'NumDivide',
};
// Physical-key name from a KeyboardEvent.code, mirroring the server's
// key_to_vk: letters, digits, F1-F24, numpad and the nav cluster.
// Modifiers and everything else -> null.
function hkMainKey(code){
  let m;
  if ((m = /^Key([A-Z])$/.exec(code))) return m[1];
  if ((m = /^Digit([0-9])$/.exec(code))) return m[1];
  if ((m = /^Numpad([0-9])$/.exec(code))) return 'Numpad' + m[1];
  if ((m = /^F([1-9]|1[0-9]|2[0-4])$/.exec(code))) return 'F' + m[1];
  return HK_CODE_MAP[code] || null;
}
// Fixed modifier order matches the server's canonical form.
function hkMods(e){
  const m = [];
  if (e.ctrlKey) m.push('Ctrl');
  if (e.altKey) m.push('Alt');
  if (e.shiftKey) m.push('Shift');
  if (e.metaKey) m.push('Win');
  return m;
}

function hkSetWarn(msg){
  const w = $('hk-warn'); if (!w) return;
  w.textContent = msg || '';
  clearTimeout(hkWarnTimer);
  if (msg) hkWarnTimer = setTimeout(() => { if ($('hk-warn')) $('hk-warn').textContent = ''; }, 4000);
}

// Paint a capture field: chips for a bound combo, a cue when unbound.
function hkRenderCapture(el, action){
  const combo = hkBindings[action] || '';
  el.classList.remove('recording','invalid');
  if (!combo){
    el.classList.add('empty');
    el.innerHTML = '<span class="hk-cue">Click to set</span>';
    el.setAttribute('aria-label', hkTitleOf(action) + ': no hotkey set, click to set one');
    return;
  }
  el.classList.remove('empty');
  const parts = combo.split('+');
  el.innerHTML = parts.map(p =>
    `<span class="hk-chip">${escAttr(p)}</span>`).join('<span class="hk-sep">+</span>');
  el.setAttribute('aria-label', hkTitleOf(action) + ': ' + parts.join(' ') + ', click to change');
}

function hkRenderList(){
  const list = $('hk-list'); if (!list) return;
  list.innerHTML = '';
  for (const [action, title, sub] of HOTKEY_ACTIONS){
    const row = document.createElement('div');
    row.className = 'hk-row';
    const main = document.createElement('div');
    main.className = 'hk-row-main';
    main.innerHTML = `<div class="hk-row-title">${escAttr(title)}</div>`
      + `<div class="tab-sub hk-row-sub">${escAttr(sub)}</div>`
      + `<div class="hk-flag" data-flag="${escAttr(action)}">`
      + `<span aria-hidden="true">&#9888;</span> Another app owns this combo. Pick a different one`
      + `</div>`;
    const cap = document.createElement('div');
    cap.className = 'hk-capture';
    cap.dataset.action = action;
    cap.setAttribute('role','button');
    cap.setAttribute('tabindex','0');
    cap.addEventListener('click', () => hkStartRecording(action, cap));
    cap.addEventListener('keydown', ev => {
      // Enter/Space starts recording; once recording, the capture-phase
      // handler owns every key, so this only fires when idle.
      if (!hkRec && (ev.key === 'Enter' || ev.key === ' ')){ ev.preventDefault(); hkStartRecording(action, cap); }
    });
    const clear = document.createElement('button');
    clear.className = 'hk-clear';
    clear.type = 'button';
    clear.innerHTML = '&times;';
    clear.title = 'Clear ' + title + ' hotkey';
    clear.setAttribute('aria-label', 'Clear ' + title + ' hotkey');
    clear.classList.toggle('idle', !hkBindings[action]);
    clear.addEventListener('click', e => { e.stopPropagation(); hkSaveBinding(action, '', cap); });
    row.append(main, cap, clear);
    list.appendChild(row);
    hkRenderCapture(cap, action);
  }
  hkApplyConflicts(hkConflicts);
}

// The last hotkey / MIDI press the backend saw, by sequence number so a
// repeat of the same action still reads as a new event. The first sighting
// after a page load is only recorded, never announced - it may be minutes
// old, and a toast for it would be a lie about what just happened.
let lastFiredSeq = null;
function showFiredAction(fired){
  if (!fired || typeof fired.seq !== 'number') return;
  if (lastFiredSeq === null){ lastFiredSeq = fired.seq; return; }
  if (fired.seq === lastFiredSeq) return;
  // The counter restarts with the process, so a lower number means the app
  // was restarted under us - adopt it silently rather than announcing an
  // action from before the restart.
  if (fired.seq < lastFiredSeq){ lastFiredSeq = fired.seq; return; }
  lastFiredSeq = fired.seq;
  const where = fired.source === 'midi' ? '#midi-list' : '#hk-list';
  const via = {midi:'MIDI', integration:'Integration', tray:'Tray menu'}[fired.source] || 'Hotkey';
  const what = (fired.source === 'integration' && FIRED_BY_INTEGRATION[fired.action]) || hkTitleOf(fired.action) || fired.action;
  const label = `${via} · ${what}`;
  toast(fired.problem ? `${label}: ${fired.problem}` : label, fired.problem ? 'err' : 'ok');
  // A tray pick is no binding: nothing in the hotkey list to light up.
  if (fired.source === 'tray') return;
  const cap = document.querySelector(`${where} .hk-capture[data-action="${fired.action}"]`);
  if (!cap) return;
  const cls = fired.problem ? 'fired-bad' : 'fired';
  cap.classList.remove('fired','fired-bad');
  void cap.offsetWidth;  // restart the animation on a repeat press
  cap.classList.add(cls);
  setTimeout(() => cap.classList.remove(cls), 950);
}

// Bindings Windows refused to register, from the /state tick. Touches only
// the flag + the field's border, never the field's contents, so a conflict
// arriving mid-recording cannot interrupt what the user is typing.
let hkConflicts = [];
function hkApplyConflicts(list){
  const next = Array.isArray(list) ? list : [];
  // /state ticks once a second whether or not anyone is looking at this
  // section; nothing to do unless the set actually moved.
  if (next.join(',') === hkConflicts.join(',') && $$('#hk-list .hk-flag.on').length === next.length) return;
  hkConflicts = next;
  $$('#hk-list .hk-flag').forEach(f => f.classList.toggle('on', hkConflicts.includes(f.dataset.flag)));
  $$('#hk-list .hk-capture').forEach(c =>
    c.classList.toggle('conflict', hkConflicts.includes(c.dataset.action)));
}

// Global hotkeys have to stand down while we record: Windows hands a
// registered combo to us instead of to the browser, so the field would never
// see the keypress and the action would fire instead of being rebound.
function hkCaptureMode(on){
  try { fetch('/hotkeys/capture', {method:'POST', body:new URLSearchParams({on: on ? '1' : '0'})}); }
  catch(e){}
}

function hkStartRecording(action, el){
  if (hkRec) hkCancelRecording();
  hkCaptureMode(true);
  hkRec = {action, el};
  el.classList.remove('empty');
  el.classList.add('recording');
  el.innerHTML = '<span class="hk-cue">Press keys&hellip;</span>';
  el.focus();
  hkSetWarn('');
  document.addEventListener('keydown', hkOnKeydown, true);
  el.addEventListener('blur', hkOnBlur);
}

function hkStopRecording(){
  document.removeEventListener('keydown', hkOnKeydown, true);
  if (hkRec) hkRec.el.removeEventListener('blur', hkOnBlur);
  hkRec = null;
  hkCaptureMode(false);
}

function hkCancelRecording(){
  if (!hkRec) return;
  const {action, el} = hkRec;
  hkStopRecording();
  hkRenderCapture(el, action);
}

function hkOnBlur(){ if (hkRec) hkCancelRecording(); }

function hkFlashInvalid(msg){
  if (!hkRec) return;
  const el = hkRec.el;
  el.classList.add('invalid');
  setTimeout(() => el && el.classList.remove('invalid'), 340);
  hkSetWarn(msg);
}

// Combos that almost certainly mean something else somewhere. A global
// hotkey is taken from every other app on the machine, so binding Ctrl+V
// means paste stops working everywhere until InstantClone quits - the same
// surprise as recording a bound combo, one step further out. We still bind
// it (it is their machine) but we say so.
function hkCommonShortcut(mods, key){
  return mods.length === 1 && mods[0] === 'Ctrl' && /^[A-Z0-9]$/.test(key);
}

function hkOnKeydown(e){
  if (!hkRec) return;
  e.preventDefault(); e.stopPropagation();
  if (e.key === 'Escape'){ hkCancelRecording(); return; }
  if (e.key === 'Backspace' || e.key === 'Delete'){
    const {action, el} = hkRec; hkStopRecording(); hkSaveBinding(action, '', el); return;
  }
  const mods = hkMods(e);
  const key = hkMainKey(e.code);
  if (!key){
    // Modifiers only so far: live preview of what's held.
    const cue = mods.length ? mods.join(' + ') + ' + &hellip;' : 'Press keys&hellip;';
    hkRec.el.innerHTML = `<span class="hk-cue">${cue}</span>`;
    return;
  }
  if (mods.length === 0){ hkFlashInvalid('Add Ctrl, Alt, Shift or Win: a bare key would fire by accident'); return; }
  const combo = mods.concat(key).join('+');
  // One combo, one action - the same rule the server enforces and the MIDI
  // side follows. Taking it from the other action is what the user just
  // asked for; refusing would leave them to go and clear it by hand first.
  const dup = Object.keys(hkBindings).find(a => a !== hkRec.action && hkBindings[a] === combo);
  if (hkCommonShortcut(mods, key)){
    hkSetWarn(combo + ' is a common shortcut. While InstantClone runs, other apps stop receiving it.');
  }
  const {action, el} = hkRec;
  hkStopRecording();
  if (dup){
    hkBindings[dup] = '';
    hkRenderList();
    const moved = document.querySelector(`#hk-list .hk-capture[data-action="${action}"]`);
    hkSaveBinding(action, combo, moved || el, hkTitleOf(dup));
    return;
  }
  hkSaveBinding(action, combo, el);
}

// Persist one binding. Optimistic: paint it, then POST. On failure, resync
// from the server so the UI never lies about what is actually bound.
// A save that failed has painted what the app doesn't hold, and what the app
// sends back is what it sent last: repaint it even so.
function reloadBindings(key){
  seenData.delete(key);
  return loadConfig();
}
async function hkSaveBinding(action, combo, el, movedOff){
  hkBindings[action] = combo;
  hkRenderCapture(el, action);
  const clearBtn = el.parentElement && el.parentElement.querySelector('.hk-clear');
  if (clearBtn) clearBtn.classList.toggle('idle', !combo);
  // The new combo has not been through RegisterHotKey yet; the next /state
  // tick says whether it took. Until then, do not keep flagging the old one.
  hkApplyConflicts(hkConflicts.filter(a => a !== action));
  try {
    const body = new URLSearchParams(); body.set('hotkey.' + action, combo);
    const j = await (await fetch('/config', {method:'POST', body})).json();
    if (j.ok){
      toast(combo
        ? `${hkTitleOf(action)}: ${combo}` + (movedOff ? ` (moved off ${movedOff})` : '')
        : `${hkTitleOf(action)} hotkey cleared`, 'ok');
    } else {
      toast(`Couldn't save hotkey: ${j.error}`, 'err');
      await reloadBindings('hotkeys');
    }
  } catch(e){
    toast('Save failed: the app may have stopped responding', 'err');
    await reloadBindings('hotkeys');
  }
}

