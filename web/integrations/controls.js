// ---------------------------------------------------------------- small controls: busy buttons, confirm on the button, undo toasts, secret fields

// Spinner on a button while `fn` runs; a second click is ignored.
async function busy(btn, fn){
  if (!btn || busyEls.has(btn)) return undefined;
  busyEls.add(btn);
  btn.classList.add('is-busy');
  btn.disabled = true;
  btn.setAttribute('aria-busy', 'true');
  try {
    return await fn();
  } finally {
    busyEls.delete(btn);
    btn.classList.remove('is-busy');
    btn.removeAttribute('aria-busy');
    btn.disabled = !!btn._wantDisabled;
    delete btn._wantDisabled;
  }
}

// Destructive buttons ask on the button itself: the first click arms it,
// a second within three seconds confirms. True when confirmed.
function armConfirm(btn, label){
  if (btn.hasAttribute('data-armed')) return true;
  const before = btn.innerHTML;
  btn.setAttribute('data-armed', '');
  btn.classList.add('ig-armed');
  btn.innerHTML = esc(label);
  setTimeout(() => {
    if (!btn.isConnected || !btn.hasAttribute('data-armed')) return;
    btn.removeAttribute('data-armed');
    btn.classList.remove('ig-armed');
    btn.innerHTML = before;
  }, 3000);
  return false;
}

// The live element for a selector inside the open modal, never one that is
// fading out.
function q(sel){
  if (!Modal.el) return null;
  return [...Modal.el.querySelectorAll(sel)].find(el => !el.closest('[data-leaving]')) || null;
}

// A toast with Undo, for actions that are easy to regret.
// `adds`: the Undo takes away integrations just added (see dropAddUndos).
function undoToast(text, undo, adds){ actionToast(text, 'Undo', undo, adds); }
// A toast with one button: Undo, or the obvious next step ("Edit copy").
function actionToast(text, label, act, adds){
  const rail = $('toast-rail');
  if (!rail){ toast(text, 'ok'); return; }
  const el = document.createElement('div');
  el.className = 'ic-toast ok ig-undo' + (adds ? ' ig-undo-adds' : '');
  el.innerHTML = `<span class="ic-toast-icon">✓</span><span class="ig-undo-text"></span><button type="button" class="ig-undo-btn"></button>
    <i class="ig-undo-bar" style="animation-duration:${UNDO_MS}ms"></i>`;
  el.querySelector('.ig-undo-text').textContent = text;
  el.querySelector('.ig-undo-btn').textContent = label;
  let used = false;
  const leave = () => {
    if (!el.isConnected || el.classList.contains('leaving')) return;
    el.classList.add('leaving');
    setTimeout(() => el.remove(), 350);
  };
  el.icLeave = leave;
  el.querySelector('button').addEventListener('click', async () => {
    if (used) return;
    used = true;
    leave();
    await act();
  });
  rail.appendChild(el);
  // Pointing at it holds the countdown (the bar pauses in CSS too).
  let left = UNDO_MS, since = Date.now(), timer = setTimeout(leave, left);
  el.addEventListener('pointerenter', () => { clearTimeout(timer); left -= Date.now() - since; });
  el.addEventListener('pointerleave', () => { since = Date.now(); timer = setTimeout(leave, Math.max(left, 600)); });
}
const UNDO_MS = 8000;
// An integration's editor opening: an Undo of an add still up could take
// away what it edits, so it goes.
function dropAddUndos(){
  document.querySelectorAll('#toast-rail .ig-undo-adds').forEach(el => { if (el.icLeave) el.icLeave(); });
}

// A secret field (topic, webhook link, headers, secret address): hidden
// like a password until the user asks to see it.
function secretField(id, control, label){
  const shown = S.revealed.has(id);
  // The label names the field: clicking it focuses it. Without `for`, the
  // Show button inside it would be its control, and a click on the words
  // would show the secret, maybe on stream.
  const fid = 'sf-' + String(id).replace(/[^A-Za-z0-9_-]/g, '_');
  control = control.replace(/<(input|textarea)\b/, `<$1 id="${fid}"`);
  return `<div class="dff ig-secret-field${shown ? ' shown' : ''}" data-field="${esc(id)}"><label for="${fid}">${label}
    <button type="button" class="ig-reveal" data-act="reveal" data-field="${esc(id)}" aria-pressed="${shown}">
      <span class="ig-eye">${svg('eye')}${svg('eyeOff')}</span><span data-tick>${shown ? 'Hide' : 'Show'}</span></button></label>${control}</div>`;
}
