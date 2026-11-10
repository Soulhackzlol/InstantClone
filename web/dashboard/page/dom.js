'use strict';
const $ = id => document.getElementById(id);
// Guarded writes for the per-tick renders: an unchanged value writes
// nothing, so the ~4x/s state refresh costs no style or layout work.
const setHidden = (el, hidden) => { if (el && el.hidden !== hidden) el.hidden = hidden; };
// Open or close a block with a short height-and-fade instead of a snap.
// It starts from what is on screen, so a toggle mid-way turns around
// rather than jumping, and it ends with `hidden` really set, so layout and
// screen readers agree with what shows. Calling it again for the state it
// is already in (or heading to) does nothing, so a 4x/s render can call it
// freely. `done` runs once it has settled.
const BOX_PROPS = ['height', 'opacity', 'marginTop', 'marginBottom', 'paddingTop', 'paddingBottom'];
const BOX_CLOSED = {height:'0px', opacity:0, marginTop:'0px', marginBottom:'0px', paddingTop:'0px', paddingBottom:'0px'};
function animateHidden(el, hide, done){
  morphHidden(el, hide, done, {props: BOX_PROPS, closed: closedBox, clip: {overflow: 'hidden'}, fadeAt: .45, ms: [280, 340]});
}
// What animateHidden and slideHidden share: from the box on screen to the
// open one or `how.closed(el)`, tweening `how.props` with `how.clip` styles
// on meanwhile, in `how.ms` [closing, opening].
function morphHidden(el, hide, done, how){
  if (!el) return;
  const goal = hide ? 'closed' : 'open';
  if ((el._anim ? el._goal : (el.hidden ? 'closed' : 'open')) === goal) return;
  const snapshot = () => {
    const cs = getComputedStyle(el), box = Object.fromEntries(how.props.map(p => [p, cs[p]]));
    // A keyword minimum (max-content) can't tween to 0: its width can.
    if (box.minWidth && !box.minWidth.endsWith('px')) box.minWidth = box.width;
    return box;
  };
  const midway = el._anim ? snapshot() : null;
  if (el._anim){ el._anim.cancel(); for (const k in how.clip) el.style[k] = ''; }
  el._anim = null;
  if (!motionOK()){ el.hidden = hide; if (done) done(); return; }
  el.hidden = false;
  const open = snapshot(), closed = how.closed(el);
  Object.assign(el.style, how.clip);
  el._goal = goal;
  // Closing fades the content before the space shrinks, and opening makes
  // the space before the content fades in, so nothing is seen squeezed.
  const frames = midway ? [midway, hide ? closed : open]
    : hide ? [open, {opacity: 0, offset: how.fadeAt}, closed]
    : [closed, {opacity: 0, offset: .5}, open];
  // It holds its last frame until `hidden` takes over, so no frame shows
  // it back open in between.
  const anim = el.animate(frames, {duration: how.ms[hide ? 0 : 1], easing: 'cubic-bezier(.4,0,.2,1)', fill: 'forwards'});
  el._anim = anim;
  anim.onfinish = () => {
    if (el._anim !== anim) return;
    el._anim = null;
    for (const k in how.clip) el.style[k] = '';
    el.hidden = hide;
    anim.cancel();
    if (done) done();
  };
}
// `animateHidden` for something that keeps its place (an overlay, a pill,
// one of two views sharing a box): a fade alone, no height. Same rules: it
// starts from what is on screen, ends with `hidden` set, and a repeat for
// the state it is in or heading to does nothing (`done` is then dropped).
function fadeHidden(el, hide, done){
  if (!el) return;
  const goal = hide ? 'closed' : 'open';
  if ((el._fade ? el._fadeGoal : (el.hidden ? 'closed' : 'open')) === goal) return;
  const from = el._fade ? getComputedStyle(el).opacity : (hide ? 1 : 0);
  if (el._fade) el._fade.cancel();
  el._fade = null;
  if (!motionOK()){ el.hidden = hide; if (done) done(); return; }
  el.hidden = false;
  el._fadeGoal = goal;
  const anim = el.animate([{opacity: from}, {opacity: hide ? 0 : 1}],
    {duration: hide ? 140 : 220, easing: 'cubic-bezier(.2,.7,.2,1)', fill: 'forwards'});
  el._fade = anim;
  anim.onfinish = () => {
    if (el._fade !== anim) return;
    el._fade = null;
    el.hidden = hide;
    anim.cancel();          // drop the held opacity: `hidden` says it now
    if (done) done();
  };
}
// The closed box. In a flex column or grid, the gap beside the item
// closes with it (a negative margin), or it would snap shut at the end.
function closedBox(el){
  const box = {...BOX_CLOSED};
  const parent = el.parentElement;
  if (!parent) return box;
  const ps = getComputedStyle(parent);
  const stacked = ps.display.includes('grid') || (ps.display.includes('flex') && ps.flexDirection.startsWith('column'));
  const gap = stacked ? parseFloat(ps.rowGap) || 0 : 0;
  if (!gap) return box;
  const shown = n => n && !n.hidden && getComputedStyle(n).display !== 'none';
  let prev = el.previousElementSibling;
  while (prev && !shown(prev)) prev = prev.previousElementSibling;
  let next = el.nextElementSibling;
  while (next && !shown(next)) next = next.nextElementSibling;
  if (prev) box.marginTop = -gap + 'px';
  else if (next) box.marginBottom = -gap + 'px';
  return box;
}
// `animateHidden` across, for a button in a row (Disarm, Cut after): its
// width opens or closes while it fades, and the gap before it with it, so
// the buttons beside it slide over instead of jumping. Same rules: from
// what is on screen, ends with `hidden` set, a repeat does nothing.
const ROW_PROPS = ['width', 'minWidth', 'flexGrow', 'opacity', 'marginLeft', 'paddingLeft', 'paddingRight'];
function rowClosed(el){
  const gap = parseFloat(getComputedStyle(el.parentElement).columnGap) || 0;
  return {width:'0px', minWidth:'0px', flexGrow:0, opacity:0, marginLeft:-gap + 'px', paddingLeft:'0px', paddingRight:'0px'};
}
function slideHidden(el, hide){
  morphHidden(el, hide, null, {props: ROW_PROPS, closed: rowClosed, clip: {overflow: 'hidden', whiteSpace: 'nowrap'}, fadeAt: .4, ms: [240, 300]});
}
// A slider's track filled up to its thumb (--p, which the range styles read).
function fillRange(el){
  const min = +el.min || 0, max = +el.max || 100;
  el.style.setProperty('--p', ((+el.value - min) / (max - min || 1)) * 100 + '%');
}
// Fade and shrink a small inline item (a header pill) out, then hide it.
function fadeOutHidden(el){
  if (!el || el.hidden) return;
  if (!motionOK()){ el.hidden = true; return; }
  el.animate([{opacity:1, transform:'none'}, {opacity:0, transform:'scale(.92)'}],
    {duration: 180, easing: 'ease-in'}).onfinish = () => { el.hidden = true; };
}
const setText = (el, text) => { if (el && el.textContent !== tr(text, el)) el.textContent = text; };
const setClass = (el, cls, on) => { if (el && el.classList.contains(cls) !== on) el.classList.toggle(cls, on); };
const setClassName = (el, name) => { if (el && el.className !== name) el.className = name; };
const setHtml = (el, html) => { if (el && el._html !== html){ el._html = html; el.innerHTML = html; } };
const $$ = sel => Array.from(document.querySelectorAll(sel));
// Escape text bound for innerHTML. Used by the hotkeys editor; values there
// are already constrained, so this is belt-and-braces against future callers.
const escAttr = s => String(s == null ? '' : s).replace(/[&<>"']/g,
  c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));

