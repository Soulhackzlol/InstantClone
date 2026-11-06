// Editors normalise some fields (commands lowercased, numbers clamped)
// without touching the field being typed in; once it is left, show the
// value that was kept. The last redraw rendered it into the attribute.
function onFieldLeave(e){
  const el = e.target;
  if (!el.dataset || !el.dataset.bind || (Modal.kind !== 'editor' && Modal.kind !== 'builder')) return;
  if (el.tagName === 'INPUT' && el.type !== 'checkbox' && el.type !== 'radio' && el.hasAttribute('value')){
    const kept = el.getAttribute('value');
    if (el.value !== kept) el.value = kept;
  }
}

// Enter or Space on a clickable card acts like a click; arrow keys move
// through a group of radio buttons (the card colors) and pick.
function onActivateKey(e){
  if (e.target.matches('.ig-folder-in') && (e.key === 'Enter' || e.key === 'Escape')){
    e.preventDefault();
    if (e.key === 'Enter') renameFolder(e.target.dataset.folder, e.target.value);
    else { S.renaming = null; render(); }
    return;
  }
  // Enter in a list's find box picks the first match, or the typed name.
  if (e.key === 'Enter' && e.target.matches('[data-bind="pick-find"]')){
    e.preventDefault();
    const first = e.target.closest('.ig-picker').querySelector('.ig-pick-row');
    if (first) first.click();
    return;
  }
  if ((e.key === 'Enter' || e.key === ' ') && e.target.matches('[role="button"][data-act]')){
    e.preventDefault();
    e.target.click();
    return;
  }
  if (Modal.kind === 'builder' && e.target.matches('.ig-step-main')){
    const row = e.target.closest('.ig-step');
    const tools = row.querySelector('.ig-step-tools');
    const pick = e.altKey && e.key === 'ArrowUp' ? tools.querySelector('[data-dir="-1"]')
      : e.altKey && e.key === 'ArrowDown' ? tools.querySelector('[data-dir="1"]')
      : (e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'd' ? tools.querySelector('[data-act="b-dup"]')
      : e.key === 'Delete' ? tools.querySelector('[data-act="b-del"]') : null;
    if (!pick) return;
    e.preventDefault();
    if (pick.disabled) return;
    Builder.act(pick.dataset.act, pick);
    // The keyboard stays on the step it moved or copied.
    if (Array.isArray(Builder.sel)){
      const at = q(`.ig-step-main[data-path="${pathKey(Builder.sel)}"]`);
      if (at) at.focus({preventScroll:true});
    }
    return;
  }
  const dir = {ArrowRight:1, ArrowDown:1, ArrowLeft:-1, ArrowUp:-1}[e.key];
  if (dir && e.target.matches('[role="radio"][data-act]')){
    const group = [...e.target.closest('[role="radiogroup"]').querySelectorAll('[role="radio"][data-act]')];
    const next = group[(group.indexOf(e.target) + dir + group.length) % group.length];
    e.preventDefault();
    next.click();
    const again = q(`[role="radio"][data-act="${next.dataset.act}"][data-v="${CSS.escape(next.dataset.v)}"]`);
    if (again) again.focus();
  }
}

// A message field measures itself again once it shows (see autosize).
document.addEventListener('focusin', e => {
  if (e.target.matches && e.target.matches('.ig-msg')) autosize(e.target);
});
// A hidden page has nobody to show activity to; catch up when it's back.
document.addEventListener('visibilitychange', () => {
  if (!document.hidden && (S.visible || S.stage || modalOpen())) poll();
  if (!document.hidden) Stage.tick();
});

function wireRoot(){
  const root = $('ig-root');
  if (!root || root.dataset.wired) return;
  root.dataset.wired = '1';
  root.addEventListener('click', onClick);
  root.addEventListener('input', onInput);
  root.addEventListener('keydown', onActivateKey);
  root.addEventListener('dragstart', onDragStart);
  root.addEventListener('dragover', onDragOver);
  root.addEventListener('drop', onDrop);
  root.addEventListener('dragend', onDragEnd);
  // A folder's new name is kept once its field is left, as with Enter.
  root.addEventListener('focusout', e => {
    if (e.target.matches('.ig-folder-in') && S.renaming === e.target.dataset.folder) renameFolder(S.renaming, e.target.value);
  });
  root.addEventListener('toggle', e => { if (e.target.id === 'ig-activity-box') S.activityOpen = e.target.open; }, true);
}

