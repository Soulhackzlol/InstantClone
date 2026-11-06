// - Toast ------------------------------─
// Errors stay long enough to read and are announced at once (role=alert);
// the rest are polite. Hovering or focusing a toast holds it, and every
// toast has a close button. `action` ({label, run, expire, undo}) adds one
// button, used for Undo; `expire` runs when the toast goes without it.
function toast(msg, kind, ms, action){
  const k = kind || 'info';
  const icon = { ok:'✓', err:'!', info:'i' }[k] || 'i';
  const el = document.createElement('div');
  el.className = 'ic-toast '+k;
  if (k === 'err') el.setAttribute('role', 'alert');
  el.innerHTML = `<span class="ic-toast-icon" aria-hidden="true">${icon}</span><span class="ic-toast-text"></span>`;
  el.querySelector('.ic-toast-text').textContent = msg;
  let left = ms || (k === 'err' ? 6000 : 2400);
  let timer = 0, since = 0;
  let acted = false;
  const close = () => {
    clearTimeout(timer);
    if (el.classList.contains('leaving')) return;
    el.classList.add('leaving');
    setTimeout(() => el.remove(), 350);
    if (action && !acted && action.expire) action.expire();
  };
  const run = () => { since = Date.now(); timer = setTimeout(close, left); };
  const hold = () => { if (!timer) return; clearTimeout(timer); timer = 0; left = Math.max(1200, left - (Date.now() - since)); };
  if (action){
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'ic-toast-act';
    b.textContent = action.label;
    if (action.undo) b.dataset.undo = '';
    b.onclick = () => { acted = true; close(); action.run(); };
    el.appendChild(b);
  }
  const x = document.createElement('button');
  x.type = 'button';
  x.className = 'ic-toast-x';
  x.setAttribute('aria-label', 'Dismiss');
  x.textContent = '✕';
  x.onclick = close;
  el.appendChild(x);
  el.addEventListener('mouseenter', hold);
  el.addEventListener('focusin', hold);
  el.addEventListener('mouseleave', () => { if (!timer && !el.contains(document.activeElement)) run(); });
  el.addEventListener('focusout', e => { if (!timer && !el.contains(e.relatedTarget)) run(); });
  $('toast-rail').appendChild(el);
  run();
  return el;
}
// POST and read the answer. A network failure, a non-2xx status or
// {ok:false} is never reported as success: {ok, data, error}.
// `keepalive`: the request still goes out if the page is closing.
async function postAction(url, body, keepalive){
  try {
    const r = await fetch(url, {method:'POST', body, keepalive:!!keepalive});
    let data = null;
    try { data = await r.json(); } catch(_){}
    if (!r.ok || (data && data.ok === false)){
      return {ok:false, data, error:(data && data.error) || `InstantClone answered ${r.status}`};
    }
    return {ok:true, data};
  } catch(_){
    return {ok:false, data:null, error:'Could not reach InstantClone. Is it still running?'};
  }
}

// Deleting something Undo can bring back: it leaves the page at once, and
// the delete is sent when its Undo toast goes (it waits while pointed at),
// or when the page closes, since Undo can't be pressed any more then. Undo
// sends nothing.
const DELETE_UNDO_MS = 8000;
const pendingDeletes = new Map();   // key -> sends that delete now
function deleteWithUndo(key, label, send, restore){
  let settled = false;
  const settle = () => { settled = true; pendingDeletes.delete(key); };
  const commit = () => { if (settled) return; settle(); send(); };
  pendingDeletes.set(key, commit);
  toast(label, 'info', DELETE_UNDO_MS, {label:'Undo', undo:true, expire:commit, run(){ if (settled) return; settle(); restore(); }});
}
const isDeleting = key => pendingDeletes.has(key);
// Ctrl+Z on the page (not in a text box or an editor, which have their
// own) presses the newest Undo still showing.
document.addEventListener('keydown', e => {
  if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.key.toLowerCase() !== 'z' || e.defaultPrevented) return;
  if (e.target.closest && e.target.closest('input, textarea, select, [contenteditable="true"]')) return;
  if (document.querySelector('.ig-modal')) return;
  const undos = document.querySelectorAll('#toast-rail .ic-toast:not(.leaving) [data-undo]');
  if (!undos.length) return;
  e.preventDefault();
  undos[undos.length - 1].click();
});
window.addEventListener('pagehide', () => [...pendingDeletes.values()].forEach(commit => commit()));

