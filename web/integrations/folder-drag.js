// ---- Dragging an integration to a folder: onto a folder's heading or its
// group, onto "Not in a folder", or onto the new folder zone that shows
// while dragging.
function onDragStart(e){
  const card = e.target.closest && e.target.closest('[data-drag-id]');
  if (!card) return;
  S.dragging = card.dataset.dragId;
  e.dataTransfer.effectAllowed = 'move';
  e.dataTransfer.setData('text/plain', S.dragging);
  card.classList.add('ig-dragged');
  // After the browser took its picture of the card.
  requestAnimationFrame(() => { const root = $('ig-root'); if (root && S.dragging) root.classList.add('ig-dragging'); });
}
// The folder under the pointer, lit, when the card can go there.
function dropTarget(e){
  const at = e.target.closest && e.target.closest('[data-drop-folder]');
  const i = S.dragging && find(S.dragging);
  return at && i && (at.hasAttribute('data-drop-new') || at.dataset.dropFolder !== (i.folder || '')) ? at : null;
}
function onDragOver(e){
  const at = dropTarget(e);
  document.querySelectorAll('#ig-root .drop-on').forEach(x => { if (x !== at) x.classList.remove('drop-on'); });
  if (!at) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  at.classList.add('drop-on');
}
async function onDrop(e){
  const at = dropTarget(e), id = S.dragging;
  if (!at) return;
  e.preventDefault();
  onDragEnd();
  const i = find(id), was = (i && i.folder) || '';
  const undo = () => moveToFolder([id], was);
  if (!at.hasAttribute('data-drop-new')){
    const to = at.dataset.dropFolder;
    if (await moveToFolder([id], to)) undoToast(to ? `Moved to ${to}` : 'Out of its folder', undo);
    return;
  }
  const name = newFolderName();
  if (await moveToFolder([id], name)){
    renameNow(name);
    undoToast('Moved to a new folder', undo);
  }
}
function onDragEnd(){
  S.dragging = null;
  const root = $('ig-root');
  if (root) root.classList.remove('ig-dragging');
  document.querySelectorAll('#ig-root .drop-on, #ig-root .ig-dragged').forEach(x => x.classList.remove('drop-on', 'ig-dragged'));
  if (S.redraw){ S.redraw = false; render(); }
}

