// - The pane card: its height follows the open tab ---------------------------
// Tween the pane card from its previous height (h0) to the new pane's own
// height (its natural one, or the full room the Logs pane is given), so
// switching between panes of different length morphs instead of snapping. Height is a layout property, but this is one short click-driven
// transition (not per-frame). .pane-card clips overflow during it so no
// scrollbar flashes, and its reserved gutter (scrollbar-gutter:stable) keeps
// the content from shifting when the bar comes back. Interruptible: a rapid re-switch re-targets from the
// live height. Releases back to the stylesheet's height on finish so dynamic
// content (logs, stats) can still grow the card afterwards.
function morphCardHeight(card, h0){
  // A tween still running is dropped first: measured mid-tween, the card
  // reads the tween's start, and an early return below would leave it
  // clipped. Measured with the stylesheet's height, not `auto`: Logs has a
  // fixed one, and `auto` tweened to its short natural height, then snapped.
  if (card._morphDone){
    card.removeEventListener('transitionend', card._morphDone);
    clearTimeout(card._morphTimer);
    card._morphDone = null;
  }
  card.style.transition = ''; card.style.height = ''; card.style.overflow = '';
  const h1 = card.offsetHeight;              // new pane's height, clamped
  if (h1 === h0) return;
  card.style.overflow = 'hidden';
  card.style.height = h0 + 'px';
  void card.offsetHeight;                     // lock the start before transitioning
  card.style.transition = 'height .28s var(--ease-out)';
  card.style.height = h1 + 'px';
  card._morphDone = (e) => {
    if (e && (e.propertyName !== 'height' || e.target !== card)) return;
    card.style.transition = ''; card.style.height = ''; card.style.overflow = '';
    card.removeEventListener('transitionend', card._morphDone);
    clearTimeout(card._morphTimer);
    card._morphDone = null;
  };
  card.addEventListener('transitionend', card._morphDone);
  // No transitionend comes if the tween never runs (a hidden card, a tab in
  // the background): release the card anyway, or it stays clipped.
  card._morphTimer = setTimeout(card._morphDone, 400);
}
// Content that comes in after its tab opened (the app's answer to a fetch)
// grows or shrinks the card the way a tab switch does, from wherever it is,
// instead of popping it to the new height. `draw` changes the pane.
function easeCardHeight(draw){
  const card = $('pane-card');
  const h0 = (card && !card.hidden && !prefersReducedMotion()) ? card.offsetHeight : null;
  draw();
  if (h0 != null) morphCardHeight(card, h0);
}
// The pane card scrolls on its own. Cap it at the room left below its top
// edge, less what follows it (the footer and the page's bottom padding), so
// the card, the System save bar pinned to it and the footer all sit inside
// the window and the page has nothing to scroll. The shell is as tall as its
// content, so its bottom less the card's is that space, whatever the card's
// height. 360 px is the card's own floor.
function fitPaneCard(){
  const card = $('pane-card');
  if (!card || card.hidden || !card.parentElement) return;
  const box = card.getBoundingClientRect();
  const below = card.parentElement.getBoundingClientRect().bottom - box.bottom;
  const top = box.top + window.scrollY;
  card.style.setProperty('--pane-max', Math.max(360, innerHeight - top - below) + 'px');
}
window.addEventListener('resize', fitPaneCard);
