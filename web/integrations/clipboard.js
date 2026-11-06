// ---------------------------------------------------------------- copying

// The clipboard API only exists on https or this PC; a phone on the LAN
// reaches the dashboard over plain http, where the old copy command still
// works. True when it was copied.
async function writeClipboard(text){
  if (navigator.clipboard && window.isSecureContext){
    try { await navigator.clipboard.writeText(text); return true; } catch(_){}
  }
  const area = document.createElement('textarea');
  area.value = text;
  area.setAttribute('readonly', '');
  area.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0';
  // Inside the modal when one is open: its focus trap would pull focus
  // straight back otherwise.
  (Modal.form || document.body).appendChild(area);
  const had = document.activeElement;
  area.select();
  let done = false;
  try { done = document.execCommand('copy'); } catch(_){}
  area.remove();
  if (had && had.isConnected && typeof had.focus === 'function') had.focus({preventScroll:true});
  return done;
}
const COPY_FAILED = 'Could not copy: select it and copy it by hand';
async function copyText(btn){
  if (!await writeClipboard(btn.dataset.text || '')){ toast(COPY_FAILED, 'err'); return; }
  if (typeof flashCopied === 'function' && btn.classList.contains('ic-btn')) flashCopied(btn); else toast('Copied', 'ok');
}

