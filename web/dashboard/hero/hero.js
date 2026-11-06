// - Animated number helper ----------------------
const numState = new Map(); // id → {to, last, gen}
// Fixed-duration ease-out tween for the stat readouts. NOTE the fix: the
// map is now SEEDED on first sight of an id. The old version only stored
// state on the animate path, which a fresh id could never reach (its
// throwaway object always had `to === target`), so the map stayed empty
// and every number SNAPPED - animateNumber never actually animated.
function animateNumber(id, target, dur, fmt){
  const el = $(id); if(!el) return;
  const fmtv = v => fmt ? fmt(v) : Math.round(v).toLocaleString();
  let cur = numState.get(id);
  if (!cur){                       // first sighting: show + seed, no anim
    numState.set(id, { to: target, last: target, gen: 0 });
    el.textContent = fmtv(target);
    return;
  }
  if (cur.to === target){          // already heading here
    if (cur.last === target) el.textContent = fmtv(target); // settled: exact
    return;                        // in-flight: let the running loop finish
  }
  cur.from = cur.last; cur.to = target; cur.start = performance.now();
  const gen = (cur.gen = (cur.gen || 0) + 1);
  const tick = (now) => {
    if (cur.gen !== gen) return;   // a newer retarget owns the element now
    const t = Math.min(1, (now - cur.start) / (dur || 600));
    const v = cur.from + (cur.to - cur.from) * (1 - Math.pow(1 - t, 3));
    cur.last = v; el.textContent = fmtv(v);
    if (t < 1) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

// - Hero delay number: continuous critically-damped motion ------
// The big delay figure is special: it COUNTS UP as the buffer fills
// (target moves ~4x/s) and ROLLS DOWN on cut/disarm (one big jump). A
// fixed-duration tween handles the jump but makes the count-up jerk -
// each state update retargets a varying gap over a fixed time, so the
// velocity keeps changing. Instead we run ONE persistent rAF that chases
// the latest target with a critically-damped spring (Unity SmoothDamp):
// it tracks a moving target at smooth, near-constant velocity (buttery
// count-up) and eases a big jump to rest with no overshoot (clean
// roll-down) - immune to update cadence, with no snap and no freeze.
const heroNum = { cur: 0, vel: 0, target: 0, smooth: 0.3, prev: 0, raf: 0, seen: false };
function setHeroDelayNumber(sec, smoothTime){
  // The first reading is where the delay already is: counting up to it on
  // opening the page would read as a delay arming.
  if (!heroNum.seen){ heroNum.seen = true; heroNum.cur = sec; }
  heroNum.target = sec;
  heroNum.smooth = smoothTime || 0.3;
  if (!heroNum.raf){
    heroNum.prev = performance.now();
    heroNum.raf = requestAnimationFrame(heroNumTick);
  }
}
function heroNumTick(now){
  const el = $('delay-big');
  if (!el){ heroNum.raf = 0; return; }
  const dt = Math.min(0.05, Math.max(0.001, (now - heroNum.prev) / 1000));
  heroNum.prev = now;
  const smoothTime = Math.max(0.02, heroNum.smooth);
  const omega = 2 / smoothTime;
  const x = omega * dt;
  const exp = 1 / (1 + x + 0.48*x*x + 0.235*x*x*x);
  const to = heroNum.target;
  const change = heroNum.cur - to;
  const temp = (heroNum.vel + omega * change) * dt;
  heroNum.vel = (heroNum.vel - omega * temp) * exp;
  let out = to + (change + temp) * exp;
  // Overshoot clamp (SmoothDamp): if we crossed the target, land on it.
  if ((to - heroNum.cur > 0) === (out > to)){ out = to; heroNum.vel = (out - to) / dt; }
  heroNum.cur = out;
  writeHeroNumber(el, heroNum.cur.toFixed(1));
  if (Math.abs(heroNum.cur - to) < 0.02 && Math.abs(heroNum.vel) < 0.1){
    heroNum.cur = to; writeHeroNumber(el, to.toFixed(1)); heroNum.raf = 0; return;
  }
  heroNum.raf = requestAnimationFrame(heroNumTick);
}
function writeHeroNumber(el, text){
  const refit = text.length !== el.textContent.length;
  el.textContent = text;
  if (refit) fitHeroNumber();
}
// An hour's delay is "3599.9 s / 3600.0s", wider than the card on a laptop
// window, so the figure shrinks just enough for the row to fit and grows
// back when it does. Measured when the text gains or loses a character,
// on a state update and on resize, never per animation frame.
function fitHeroNumber(){
  const big = $('delay-big');
  if (!big) return;
  const row = big.parentElement;
  big.style.removeProperty('--hero-fit');
  const overflow = row.scrollWidth - row.clientWidth;
  if (overflow <= 0) return;
  const width = big.getBoundingClientRect().width;
  const full = parseFloat(getComputedStyle(big).fontSize);
  big.style.setProperty('--hero-fit', Math.max(40, Math.floor(full * (width - overflow) / width)) + 'px');
}
window.addEventListener('resize', fitHeroNumber);

// - Compact delay panel ------------------------------------------------
// One row instead of the full panel, for when the tabs below need the room
// (see #hero.compact). Remembered across visits. The change is one motion:
// every part of the panel travels from where it was to where it lands, on
// one curve with the panel's height, and really resizes on the way (the
// field, the main button, the delay figure's text), so nothing stretches,
// blurs or snaps. Text that wraps differently crosses over; a part that
// goes fades out where it was, one that comes fades in where it lands. The
// tab card below follows frame by frame.
const HERO_COMPACT_KEY = 'ic-hero-compact';
const HERO_FOLD_MS = 500;
// A gentle start and a long, soft landing.
const HERO_EASE = 'cubic-bezier(.2,0,0,1)';
// The panel's parts and how each travels: 'box' moves and resizes, 'type'
// moves while its text changes size, 'swap' crosses over, and 'inner' (in
// a part that moves) only fades, in its place in that part.
const HERO_PARTS = [['.state-pill', 'box'], ['.hero-delay-row', 'type'], ['.hero-delay-sub', 'swap'], ['.arm-bar', 'box'],
  ['.cp-head', 'box'], ['.cp-input-row', 'box'], ['.cp-unit', 'inner'], ['.cp-hint', 'box'], ['.cp-profiles', 'box'], ['.cp-actions', 'box']];
function heroCompact(){ const hero = $('hero'); return !!hero && hero.classList.contains('compact'); }
function setHeroCompact(on, animate){
  const hero = $('hero'), btn = $('hdr-compact');
  // One change at a time: a second click mid-change is ignored, and a
  // change that isn't animated (the tour's) ends the one under way first.
  if (!hero || on === heroCompact()) return;
  if (hero.icFoldEnd){ if (animate) return; hero.icFoldEnd(); }
  try { localStorage.setItem(HERO_COMPACT_KEY, on ? '1' : ''); } catch(_){}
  if (btn){
    btn.setAttribute('aria-pressed', String(on));
    btn.title = on ? 'Show the full delay panel' : 'Compact the delay panel: more room for the tabs';
  }
  const apply = () => { hero.classList.toggle('compact', on); fitHeroNumber(); };
  if (!animate || hero.hidden || !motionOK()){ apply(); fitPaneCard(); return; }
  foldHero(hero, apply);
}
// Each part measured before and after `change`, then held where it lands,
// out of the flow, and sent there from where it was.
function foldHero(hero, change){
  const grid = hero.querySelector('.hero'), cta = $('cp-cta');
  const card = hero.getBoundingClientRect(), from = hero.offsetHeight, ctaWas = cta.offsetWidth;
  const columns = [...grid.children].filter(col => col.matches('.hero-readout,.hero-control'));
  const rowsWas = getComputedStyle(grid).gridTemplateRows, colsWas = columns.map(col => col.offsetHeight);
  const parts = HERO_PARTS.map(([sel, how]) => ({el:hero.querySelector(sel), how})).filter(part => part.el);
  parts.forEach(part => { part.was = heroPartState(part.el); });
  change();
  const to = hero.offsetHeight, ctaNow = cta.offsetWidth, gs = getComputedStyle(grid);
  const colsNow = columns.map(col => col.offsetHeight);
  // Every part is placed in the grid's own box, which stays still: the
  // columns between (positioned for their layering) step aside meanwhile.
  const base = grid.getBoundingClientRect();
  parts.forEach(part => {
    part.now = heroPartState(part.el);
    if (!part.now || part.how === 'inner') return;
    part.left = part.now.rect.left - base.left - grid.clientLeft;
    part.top = part.now.rect.top - base.top - grid.clientTop;
  });
  const held = [], runs = [];
  const hold = (el, style) => {
    held.push([el, Object.fromEntries(Object.keys(style).map(k => [k, el.style[k]]))]);
    Object.assign(el.style, style);
  };
  const move = (el, frames, opts) => { runs.push(el.animate(frames, Object.assign({duration:HERO_FOLD_MS, easing:HERO_EASE}, opts))); };
  // The columns keep the shape they land in, so every part can leave the
  // flow, and ease there from the one they had (their lines with them).
  hold(grid, {gridTemplateColumns:gs.gridTemplateColumns, gridTemplateRows:gs.gridTemplateRows});
  move(grid, [{gridTemplateRows:rowsWas}, {gridTemplateRows:gs.gridTemplateRows}]);
  columns.forEach((col, n) => {
    hold(col, {height:colsNow[n] + 'px', position:'static'});
    move(col, [{height:colsWas[n] + 'px'}, {height:colsNow[n] + 'px'}]);
    [...col.children].forEach(child => hold(child, {position:'static'}));
  });
  hero.classList.add('sizing');
  hold(hero, {height:to + 'px'});
  move(hero, [{height:from + 'px'}, {height:to + 'px'}]);
  parts.forEach(part => {
    if (!part.now){ if (part.was) runs.push(heroPartLeaves(hero, part.el, part.was, card)); return; }
    // It makes its room as it fades in, so the part it is in never squeezes.
    if (part.how === 'inner'){
      if (part.was) return;
      hold(part.el, {overflow:'hidden', whiteSpace:'nowrap'});
      move(part.el, [{maxWidth:'0px', opacity:0}, {maxWidth:part.now.rect.width + 'px', opacity:0, offset:.4}, {maxWidth:part.now.rect.width + 'px', opacity:1}]);
      return;
    }
    const sized = part.how === 'box', b = part.now.rect;
    hold(part.el, {position:'absolute', zIndex:'1', margin:'0', left:part.left + 'px', top:part.top + 'px',
      width:sized ? b.width + 'px' : '', height:sized ? b.height + 'px' : ''});
    if (!part.was){
      move(part.el, [{opacity:0, transform:'translateY(6px)'}, {opacity:1, transform:'none'}],
        {duration:HERO_FOLD_MS * .6, delay:HERO_FOLD_MS * .4, fill:'backwards'});
      return;
    }
    const a = part.was.rect, shift = `translate(${a.left - b.left}px,${a.top - b.top}px)`;
    if (part.how === 'swap'){
      // The old words fade where they were and the new ones where they
      // land, once the rest is nearly there: they never cross anything.
      runs.push(heroPartLeaves(hero, part.el, part.was, card));
      move(part.el, [{opacity:0, transform:'translateY(4px)'}, {opacity:1, transform:'none'}],
        {duration:HERO_FOLD_MS * .45, delay:HERO_FOLD_MS * .55, easing:'ease-out', fill:'backwards'});
    } else if (part.how === 'type'){
      // Around what it would cross: going up it moves aside first, going
      // down it drops first.
      const up = a.top > b.top, lead = {duration:HERO_FOLD_MS * .6}, trail = {easing:'cubic-bezier(.6,0,.2,1)'};
      move(part.el, [{translate:`${a.left - b.left}px 0px`}, {translate:'0px 0px'}], up ? lead : trail);
      move(part.el, [{transform:`translateY(${a.top - b.top}px)`}, {transform:'none'}], up ? trail : lead);
      [...part.el.children].forEach((child, n) => move(child, [{fontSize:part.was.fonts[n]}, {fontSize:part.now.fonts[n]}]));
    } else {
      move(part.el, [{transform:shift, width:a.width + 'px', height:a.height + 'px'}, {transform:'none', width:b.width + 'px', height:b.height + 'px'}]);
    }
  });
  // The main button really changes width (its words stay as they are).
  if (ctaWas !== ctaNow) move(cta, [{flexGrow:0, flexShrink:0, flexBasis:ctaWas + 'px', width:ctaWas + 'px'},
    {flexGrow:0, flexShrink:0, flexBasis:ctaNow + 'px', width:ctaNow + 'px'}]);
  const until = performance.now() + HERO_FOLD_MS + 30;
  const end = () => {
    clearTimeout(timer);
    runs.forEach(run => run.cancel());
    held.reverse().forEach(([el, was]) => Object.assign(el.style, was));
    hero.icFoldEnd = null;
    hero.classList.remove('sizing');
    fitHeroNumber();
    fitPaneCard();
  };
  const timer = setTimeout(end, HERO_FOLD_MS + 30);
  hero.icFoldEnd = end;
  (function follow(){ fitPaneCard(); if (hero.icFoldEnd === end && performance.now() < until) requestAnimationFrame(follow); })();
}
// Where a part is, how it looks (what a copy of it keeps once the panel's
// look has changed) and its text's sizes, or null when it isn't shown.
const HERO_GHOST_LOOK = ['display', 'fontSize', 'lineHeight', 'whiteSpace', 'textOverflow', 'overflow', 'padding', 'gap'];
function heroPartState(el){
  if (!el.getClientRects().length) return null;
  const cs = getComputedStyle(el);
  return {rect:el.getBoundingClientRect(), look:Object.fromEntries(HERO_GHOST_LOOK.map(k => [k, cs[k]])),
    fonts:[...el.children].map(child => getComputedStyle(child).fontSize)};
}
// A part that goes: a copy of it stays where it was and fades out there.
function heroPartLeaves(hero, el, was, card){
  const ghost = el.cloneNode(true), r = was.rect;
  ghost.removeAttribute('id');
  ghost.querySelectorAll('[id]').forEach(n => n.removeAttribute('id'));
  ghost.setAttribute('aria-hidden', 'true');
  Object.assign(ghost.style, was.look, {position:'absolute', margin:'0', boxSizing:'border-box', pointerEvents:'none',
    left:(r.left - card.left - hero.clientLeft) + 'px', top:(r.top - card.top - hero.clientTop) + 'px', width:r.width + 'px', height:r.height + 'px'});
  hero.appendChild(ghost);
  const fade = ghost.animate([{opacity:1}, {opacity:0, transform:'scale(.97)'}], {duration:160, easing:'ease-out', fill:'forwards'});
  const gone = () => ghost.remove();
  fade.finished.then(gone, gone);
  return fade;
}
(() => {
  let saved = false;
  try { saved = localStorage.getItem(HERO_COMPACT_KEY) === '1'; } catch(_){}
  if (saved) setHeroCompact(true, false);
})();

// - Calm mode --------------------------------
// A dashboard often shares the GPU with OBS and a game. Every looping
// animation (a pulsing dot, a sweep, the hero's light) asks for a new frame
// at each screen refresh, and when the browser can't draw them that fast
// the whole page stutters: clicks, scrolling, tabs. So the page counts its
// own frames now and then, and while it gets fewer than CALM.slowFps the
// loops rest (`html.calm` in the styles). They come back once frames have
// been fast for a few counts in a row, so it doesn't flicker on and off.
const CALM = {countMs:1000, everyMs:8000, firstMs:2500, slowFps:30, fineFps:50, fineCounts:3};
let fineCounts = 0;
function countFrames(){
  // A hidden tab draws no frames at all: that says nothing about the GPU.
  if (document.hidden) return;
  const start = performance.now();
  let frames = 0;
  requestAnimationFrame(function count(now){
    frames++;
    if (now - start < CALM.countMs){ requestAnimationFrame(count); return; }
    const fps = frames * 1000 / (now - start);
    const page = document.documentElement.classList;
    if (fps < CALM.slowFps){ fineCounts = 0; page.add('calm'); }
    else if (fps >= CALM.fineFps && ++fineCounts >= CALM.fineCounts) page.remove('calm');
  });
}
setTimeout(countFrames, CALM.firstMs);
setInterval(countFrames, CALM.everyMs);

// - Sparkline ----------------------------─
// Catmull-Rom -> cubic-bezier smoothing so the trace reads as a smooth
// curve instead of a jagged polyline.
function smoothLinePath(pts){
  if (pts.length < 2) return '';
  if (pts.length === 2) return `M ${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)} L ${pts[1][0].toFixed(1)} ${pts[1][1].toFixed(1)}`;
  let d = `M ${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
  for (let i = 0; i < pts.length - 1; i++){
    const p0 = pts[i-1] || pts[i], p1 = pts[i], p2 = pts[i+1], p3 = pts[i+2] || p2;
    const cp1x = p1[0] + (p2[0]-p0[0])/6, cp2x = p2[0] - (p3[0]-p1[0])/6;
    let cp1y = p1[1] + (p2[1]-p0[1])/6, cp2y = p2[1] - (p3[1]-p1[1])/6;
    // Clamp the control points' Y to the segment's own range so the curve
    // can't overshoot past the data (spiky CPU was bulging into weird humps
    // and dipping below the baseline). Keeps horizontal easing, kills the
    // vertical overshoot.
    const lo = Math.min(p1[1], p2[1]), hi = Math.max(p1[1], p2[1]);
    cp1y = Math.max(lo, Math.min(hi, cp1y));
    cp2y = Math.max(lo, Math.min(hi, cp2y));
    d += ` C ${cp1x.toFixed(1)} ${cp1y.toFixed(1)}, ${cp2x.toFixed(1)} ${cp2y.toFixed(1)}, ${p2[0].toFixed(1)} ${p2[1].toFixed(1)}`;
  }
  return d;
}
const sparkMaxState = {};
// Shared sparkline renderer (stats cards + destination cards). Floor
// anchored at 0 so height reflects the TRUE magnitude; ceiling grows
// instantly to a new peak but decays slowly (`key`-scoped) so a spike
// leaving the 60s window doesn't snap the axis - it tracks reality.
function drawSpark(lineEl, fillEl, data, width, height, key){
  if (!lineEl || !fillEl) return;
  if (data.length < 2){ lineEl.setAttribute('d',''); fillEl.setAttribute('d',''); return; }
  const target = Math.max(...data, 1);
  const prev = sparkMaxState[key] || target;
  const max = target >= prev ? target : prev + (target - prev) * 0.1;
  sparkMaxState[key] = max;
  const pad = 2, h = height - pad * 2, step = width / (data.length - 1);
  const pts = data.map((v, i) => [i * step, height - pad - Math.max(0, Math.min(1, v / max)) * h]);
  const line = smoothLinePath(pts);
  lineEl.setAttribute('d', line);
  fillEl.setAttribute('d', line + ` L ${width} ${height} L 0 ${height} Z`);
}
function updateSpark(lineId, fillId, data, width, height){
  drawSpark($(lineId), $(fillId), data, width, height, lineId);
}

// - Action endpoints -------------------------
function setPending(p){
  pendingPhase = p;
  clearTimeout(pendingClearT);
  pendingClearT = setTimeout(() => { pendingPhase = null; tick(); }, 2500);
  if (lastState) renderHero(lastState);
}
function clearPending(){ pendingPhase = null; clearTimeout(pendingClearT); }

async function arm(ms){
  // Capacity guard - defence in depth so a profile chip / scripted
  // call hits the same wall as the manual delay input. ms === 0 is
  // "disarm" and skips the check.
  if (ms > 0){
    const cap = currentBufferCapMs();
    if (cap > 0 && ms > cap){
      const needMB = requiredMBForMs(ms);
      toast(`Too long for the buffer: ${fmtDelay(ms)} at ${capacityRate()} needs at least ${needMB} MB (it has ${runningBufferMb()} MB, so ${fmtDelay(cap)}).`, 'err');
      return;
    }
  }
  // The server enforces the same capacity wall (defence in depth); surface
  // its message if bitrate rose between our check above and the request.
  const r = await postAction('/arm', new URLSearchParams({ms: String(ms)}));
  if (!r.ok){
    toast(r.error, 'err');
    tick();
    return;
  }
  toast(ms > 0 ? `Arming ${fmtDelay(ms)}…` : 'Disarmed', 'ok');
  tick();
}
// The delay actions report what the server did, not what was asked: a
// refusal or an unreachable app clears the optimistic state and says why.
async function delayAction(url, phase, done, failed){
  setPending(phase);
  const r = await postAction(url);
  if (r.ok) toast(done[0], done[1]);
  else { clearPending(); toast(`${failed}: ${r.error}`, 'err'); }
  tick();
}
function activate(){ return delayAction('/activate', 'active', ['Delay active', 'ok'], 'Could not activate the delay'); }
function stop(){ return delayAction('/stop', 'ready', ['Back to live', 'info'], 'Could not cut to live'); }
function disarm(){ return delayAction('/disarm', 'idle', ['Disarmed', 'info'], 'Could not disarm'); }

// "Cut after this airs" - mark the current live moment; the backend
// auto-cuts once every destination has aired past it. Clicking while a
// mark is pending cancels it. No setPending here: the phase doesn't
// change until the mark actually fires.
async function cutAfter(){
  if (lastState && lastState.safe_cut_pending){
    const r = await postAction('/cut-after/cancel');
    if (r.ok) toast('Scheduled cut cancelled', 'info');
    else toast(`Could not cancel the scheduled cut: ${r.error}`, 'err');
    tick();
    return;
  }
  const r = await postAction('/cut-after');
  if (r.ok) toast('Marked. Cutting to live once this moment airs.', 'ok');
  else toast(r.error, 'err');
  tick();
}
function cutAfterClick(ev){
  const btn = ev.currentTarget;
  if (btn._busy) return;
  btn._busy = true; setTimeout(() => { btn._busy = false; }, 300);
  cutAfter();
}

// - Custom delay field -----------------------------------------------
// The longest delay anything can arm, config::MAX_DELAY_MS in the backend.
// The buffer size is the real limit below it (see currentBufferCapMs).
const DELAY_MAX_S = 3600;
const reducedMotionQuery = window.matchMedia('(prefers-reduced-motion: reduce)');
// Motion plays only when the viewer allows it and the page is on screen: a
// background tab parks animations, and their leftovers would pile up.
const motionOK = () => !document.hidden && !reducedMotionQuery.matches;
// Replay a one-shot class. Unlike restartAnimation this also runs under
// reduced motion, for colour-only cues.
function replayClass(el, cls){
  if (!el) return;
  el.classList.remove(cls);
  void el.offsetWidth;
  el.classList.add(cls);
}
function rawDelayInput(){ return parseInt($('delay-input').value || '0', 10) || 0; }
// Seconds in the delay field, held to the range the backend accepts. A
// typed 4000 means 3600 here even before the field settles on it.
function delayInputSeconds(){ return Math.max(0, Math.min(DELAY_MAX_S, rawDelayInput())); }

function bumpDelay(d){
  const cur = rawDelayInput();
  const next = Math.max(0, Math.min(DELAY_MAX_S, cur + d));
  if (next === cur){ refuseDelayStep(d); return; }
  setDelayInput(next);
}
// Write the field from a stepper or a profile, rolling the digits over.
function setDelayInput(sec){
  const inp = $('delay-input');
  const wasEmpty = inp.value === '';
  const from = rawDelayInput();
  clearTimeout(delayClampTimer);
  inp.value = String(sec);
  if (from !== sec) rollDelayInput(from, sec, wasEmpty);
  renderCta();
}
// A delay armed from elsewhere (the dock, a hotkey, an integration, another
// tab) shows in the field too, or Activate would read as a change back to
// what the field last held. Someone typing there keeps what they typed.
let delayFieldArmedMs = 0;
function followArmedDelay(armedMs){
  if (armedMs === delayFieldArmedMs) return;
  delayFieldArmedMs = armedMs;
  if (!armedMs || document.activeElement === $('delay-input')) return;
  if (armedMs % 1000) $('delay-input').value = '';
  else setDelayInput(armedMs / 1000);
}
// A stepper pushed past 0 or the top: the range lights up, the stepper
// flushes, and the number nudges against the wall.
function refuseDelayStep(d){
  replayClass($('cp-cap'), 'flash');
  if (!motionOK()) return;
  replayClass(d < 0 ? $('cp-minus') : $('cp-plus'), 'deny');
  $('delay-input').animate([{ transform: 'none' }, { transform: `translateX(${6 * d}px)` }, { transform: 'none' }],
    { duration: 280, easing: 'cubic-bezier(.2,.7,.2,1)' });
}
// The range above the field: what the buffer holds at this bitrate, since
// that is the real limit, and never past the top. Whole minutes past two,
// so a wobbling bitrate doesn't make it tick. Blank until the config loads.
function renderDelayRange(){
  const cap = currentBufferCapMs();
  if (cap <= 0) return;
  const top = Math.min(cap, DELAY_MAX_S * 1000);
  setText($('cp-cap'), `up to ${fmtDelay(top >= 120000 ? Math.floor(top / 60000) * 60000 : top)}`);
}
// A typed value past the range settles on the limit shortly after typing
// stops, or at once on Enter or leaving the field.
let delayClampTimer = 0;
function settleDelayInput(){
  clearTimeout(delayClampTimer);
  const inp = $('delay-input');
  if (inp.value === '') return;
  const raw = rawDelayInput();
  const settled = Math.max(0, Math.min(DELAY_MAX_S, raw));
  if (settled === raw) return;
  inp.value = String(settled);
  renderCta();
  replayClass($('cp-cap'), 'flash');
  if (motionOK()) inp.closest('.cp-input-row').animate(
    [0, -6, 6, -4, 4, -2, 0].map(x => ({ transform: `translateX(${x}px)` })), { duration: 380, easing: 'ease-out' });
}

// Rolling digits: each changed digit rolls up (more) or down (less), the
// right-most first like a counter, and a digit that appears or goes away
// grows or shrinks its column instead of shoving the number sideways. It
// plays in an overlay with the field's metrics while the field's own text
// is transparent, then hands back. Focusing or typing ends it at once.
let delayRollTimer = 0;
function rollDelayInput(from, to, fromPlaceholder){
  const wrap = $('cp-input-wrap'), roll = $('cp-roll'), inp = $('delay-input');
  if (!wrap || !roll || !motionOK() || document.activeElement === inp){ endDelayRoll(); return; }
  // Clicked again mid-roll: hand over at double speed so fast clicking keeps up.
  const quick = wrap.classList.contains('rolling');
  const duration = quick ? 170 : 340, stagger = quick ? 20 : 40;
  const a = String(from), b = String(to);
  const n = Math.max(a.length, b.length);
  const A = a.padStart(n, ' '), B = b.padStart(n, ' ');
  const up = to > from;
  const colWidth = 'calc(1ch - .01em)';
  const cell = (ch, placeholder) => {
    const s = document.createElement('span');
    s.textContent = ch === ' ' ? ' ' : ch;
    if (placeholder) s.className = 'dg-ph';
    return s;
  };
  const cols = [], starts = [];
  let longest = 0;
  for (let i = 0; i < n; i++){
    const col = document.createElement('span');
    col.className = 'dg';
    cols.push(col);
    if (A[i] === B[i]){ col.textContent = B[i] === ' ' ? ' ' : B[i]; continue; }
    const strip = document.createElement('span');
    strip.className = 'dg-strip';
    const was = cell(A[i], fromPlaceholder), now = cell(B[i], false);
    if (up) strip.append(was, now); else strip.append(now, was);
    col.appendChild(strip);
    const delay = (n - 1 - i) * stagger;
    longest = Math.max(longest, delay + duration);
    starts.push(() => {
      strip.animate([{ transform: `translateY(${up ? '0%' : '-50%'})` }, { transform: `translateY(${up ? '-50%' : '0%'})` }],
        { duration, delay, easing: 'cubic-bezier(.3,1.3,.5,1)', fill: 'both' });
      if (A[i] === ' ') col.animate([{ width: '0px' }, { width: colWidth }], { duration: duration * .7, delay, fill: 'both' });
      if (B[i] === ' ') col.animate([{ width: colWidth }, { width: '0px' }], { duration: duration * .7, delay, fill: 'both' });
    });
  }
  roll.replaceChildren(...cols);
  roll.hidden = false;
  wrap.classList.add('rolling');
  starts.forEach(start => start());
  clearTimeout(delayRollTimer);
  delayRollTimer = setTimeout(endDelayRoll, longest + 20);
}
function endDelayRoll(){
  clearTimeout(delayRollTimer);
  const wrap = $('cp-input-wrap'), roll = $('cp-roll');
  if (!wrap || !wrap.classList.contains('rolling')) return;
  wrap.classList.remove('rolling');
  roll.hidden = true;
  roll.replaceChildren();
}

const delayInputEl = $('delay-input');
if (delayInputEl){
  delayInputEl.addEventListener('input', () => {
    endDelayRoll();
    clearTimeout(delayClampTimer);
    const raw = rawDelayInput();
    if (raw < 0 || raw > DELAY_MAX_S) delayClampTimer = setTimeout(settleDelayInput, 700);
    renderCta();
  });
  delayInputEl.addEventListener('focus', endDelayRoll);
  delayInputEl.addEventListener('blur', settleDelayInput);
  delayInputEl.addEventListener('keydown', e => { if (e.key === 'Enter') settleDelayInput(); });
}
$('s-bmb') && $('s-bmb').addEventListener('input', renderBufferHint);

function ctaClick(ev){
  const btn = ev.currentTarget;
  if (btn._busy) return;
  btn._busy = true; setTimeout(() => { btn._busy = false; }, 300);
  if (!lastState) return;
  const ds = PHASE_TO_STATE[lastState.phase] || 'off';
  const inputMs = delayInputSeconds() * 1000;
  const armedMs = lastState.armed_delay_ms || 0;
  const wantsChange = inputMs > 0 && inputMs !== armedMs;
  // Re-arm / adjust takes priority over the state's default action: if
  // the user has typed a different value in the input, that's their
  // intent. Empty input or input matching what's already armed falls
  // through to activate / stop / disarm.
  if ((ds === 'armed' || ds === 'active') && wantsChange){
    arm(inputMs);  // arm() repeats the capacity check; the input keeps the value
    return;
  }
  if (ds === 'active') { stop(); return; }
  if (ds === 'armed')  { activate(); return; }
  if (ds === 'arming') { disarm(); return; }
  if (inputMs > 0) arm(inputMs);
}

// Secondary disarm button - visible during `arming` or `armed` so the user
// can always back out without having to first activate then cut. Without
// this, the `armed` state had no exit path other than activating the
// delay (then cutting it). Felt broken.
function disarmClick(ev){
  const btn = ev.currentTarget;
  if (btn._busy) return;
  btn._busy = true; setTimeout(() => { btn._busy = false; }, 300);
  disarm();
}

// - Hero render ---------------------------─
// Swap the hero subtext with a directional micro-slide (old line drops
// out, new line drops in) - but ONLY when the message KIND changes.
// `key` names the kind; several messages embed live numbers (bitrate,
// buffer fill, the auto-cut countdown) that tick up to 4x/s, and
// animating those swaps made the line pulse constantly. Same kind =
// silent in-place text update; changed kind = animated swap.
function setSubText(el, text, key){
  if (!el) return;
  const kind = key || text;
  const sameKind = el._subKey === kind;
  el._subKey = kind;
  if (el.textContent === tr(text, el)) return;
  if (sameKind && !el._subT){ el.textContent = text; return; }
  clearTimeout(el._subT);
  el.classList.add('subfade');
  el._subT = setTimeout(() => {
    el._subT = null;
    el.textContent = text;
    // Pre-swap frame: park the new text above, invisible, transitions
    // off. Committing that frame (offsetWidth) then releasing the class
    // lets the normal transition land it - a drop-in, not a reappear.
    el.classList.add('subenter');
    el.classList.remove('subfade');
    void el.offsetWidth;
    el.classList.remove('subenter');
  }, 220);
}
// The state pill, written only when it changes. New words glide in: the old
// pair lifts out over the new one while the pill eases to its new width,
// in step with the colour sweep (CSS). First paint, hidden tabs and reduced
// motion swap the words in place.
function setStatePill(p){
  const pill = $('state-pill');
  const key = p.label + '|' + p.desc;
  if (pill.style.getPropertyValue('--sc') !== p.c) pill.style.setProperty('--sc', p.c);
  setClass($('state-dot'), 'pulse', !!p.pulse);
  if (pill._key === key) return;
  const first = pill._key === undefined;
  pill._key = key;
  const words = document.createElement('span');
  words.className = 'state-words';
  const label = document.createElement('span');
  label.className = 'state-label';
  label.textContent = p.label;
  const desc = document.createElement('span');
  desc.className = 'state-desc';
  desc.textContent = p.desc;
  words.append(label, desc);
  const current = pill.querySelector('.state-words:not(.leaving)');
  if (first || !current || !motionOK()){
    pill.querySelectorAll('.state-words').forEach(n => n.remove());
    pill.appendChild(words);
    return;
  }
  const from = pill.offsetWidth;
  // At most one pair is ever on its way out, whatever the change rate.
  pill.querySelectorAll('.state-words.leaving').forEach(n => n.remove());
  current.style.left = current.offsetLeft + 'px';
  current.classList.add('leaving');
  current.setAttribute('aria-hidden', 'true');
  pill.appendChild(words);
  const to = pill.offsetWidth;
  if (from !== to) pill.animate([{ width: from + 'px' }, { width: to + 'px' }], { duration: 380, easing: 'cubic-bezier(.2,.7,.2,1)' });
  current.animate(
    [{ opacity: 1, transform: 'none', filter: 'blur(0)' }, { opacity: 0, transform: 'translateY(-8px)', filter: 'blur(3px)' }],
    { duration: 160, easing: 'ease-in', fill: 'forwards' }).finished.finally(() => current.remove()).catch(() => {});
  words.animate(
    [{ opacity: 0, transform: 'translateY(8px)', filter: 'blur(3px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }],
    { duration: 260, delay: 60, easing: 'cubic-bezier(.2,.7,.2,1)', fill: 'backwards' });
}
function renderHero(s){
  const ds = PHASE_TO_STATE[s.phase] || 'off';
  // Aurora behind the readout: when OBS is forwarding without delay
  // ("passthrough") we still want the green "live" glow, not the gray
  // "idle" one - otherwise the hero looks dead while bytes are flowing.
  const auroraState = (!s.ingest_alive) ? 'off'
                     : (ds === 'off') ? 'active'   // passthrough → green
                     : ds;
  const readout = $('hero-readout');
  if (readout.dataset.state !== auroraState) readout.dataset.state = auroraState;
  const root = document.documentElement;
  if (root.dataset.streamState !== auroraState){
    if (root.dataset.streamState) restartAnimation($('aurora-bloom'), 'bloom');
    root.dataset.streamState = auroraState;
  }
  setStatePill(computeStatePill(s));
  // Running short: the buffer can't hold the armed delay, so what is on
  // air is the buffer's reach. The number shows that, in amber, and never
  // claims more delay than viewers are really behind; the line under it
  // names the delay that was asked for.
  const shortNow = s.capacity_short && s.capacity_short.armed_ms === s.armed_delay_ms ? s.capacity_short : null;
  const runsShort = (ds === 'armed' || ds === 'active') ? shortNow : null;
  const big = $('delay-big');
  const bigClass = 'hero-delay-num mono d-' + ds + (runsShort ? ' d-short' : '');
  if (big.className !== bigClass) big.className = bigClass;
  // big number: arming shows fill, otherwise target/current
  const targetSec = (s.armed_delay_ms || 0) / 1000;
  const fillSec = (s.buffer_fill_ms || 0) / 1000;
  // How far the fill can get: the whole delay, or what the buffer holds of
  // it. Counting toward 120 s that can never come reads as stuck.
  const reachSec = shortNow ? Math.min(targetSec, shortNow.holds_ms / 1000) : targetSec;
  const showSec = ds === 'arming' ? Math.min(fillSec, reachSec)
                 : runsShort ? Math.min(targetSec, runsShort.holds_ms / 1000)
                 : ds === 'armed' || ds === 'active' ? targetSec
                 : 0;
  // Continuous smooth motion (see setHeroDelayNumber). Tighter spring
  // while arming so the count-up tracks the buffer fill closely; a touch
  // softer for the big jumps (arm / activate / cut / disarm roll-down).
  setHeroDelayNumber(showSec, ds === 'arming' ? 0.22 : 0.34);
  if (ds === 'arming'){
    setHidden($('delay-of'), false);
    setText($('delay-of'), `/ ${reachSec.toFixed(1)}s`);
    setHidden($('arm-bar'), false);
    const pct = (reachSec > 0 ? Math.min(100, (fillSec / reachSec) * 100) : 0) + '%';
    const fill = $('arm-bar-fill');
    if (fill.style.width !== pct) fill.style.width = pct;
    // A bar that cannot move looks broken; dim it while there is no feed.
    setClass($('arm-bar'), 'stalled', !s.ingest_alive);
  } else {
    setHidden($('delay-of'), true);
    setHidden($('arm-bar'), true);
  }
  fitHeroNumber();
  // The aurora grows with the buffer while arming, and is at full strength
  // otherwise. Two decimals, so the tick writes only when the fill moves.
  const auroraFill = ds === 'arming' && reachSec > 0 ? Math.min(1, fillSec / reachSec).toFixed(2) : '1';
  const aurora = $('hero-aurora');
  if (aurora && aurora.style.getPropertyValue('--fill') !== auroraFill) aurora.style.setProperty('--fill', auroraFill);
  // Adjusting detector: during a live delay change the controller
  // updates `armed_delay_ms` immediately but `current_delay_ms` only
  // catches up once the cut machinery seeks to (or rewinds toward) the
  // new target. For decreases (10→5s) the cut is effectively instant
  // - armed and current converge within one or two pump ticks. For
  // increases (10→15s) it depends on buffer history: if we already
  // have ≥15s buffered, also instant; otherwise the controller waits
  // for more bytes to land and the gap stays visible. 1.5 s tolerance
  // skips the natural ~1s jitter from the per-IDR cut quantum.
  const curMs = s.current_delay_ms || 0;
  const armedMs = s.armed_delay_ms || 0;
  // Gated on egress_alive: current_delay_ms is derived from the slowest
  // LIVE destination and reads 0 when none is connected - without the
  // gate, an active delay with no destination showed a permanent (and
  // fake) "adjusting to Xs (currently 0s)" line.
  // Also gated on OBS: with OBS gone the buffer only drains, so the gap
  // grows without anything adjusting.
  const adjusting = ds === 'active' && armedMs > 0 && s.egress_alive && s.ingest_alive
    && Math.abs(armedMs - curMs) > 1500;
  // Sub copy. When OBS hasn't connected there's no signal to forward, so
  // don't claim we're forwarding anything - say we're waiting for it.
  // `subKey` names the message KIND: setSubText only animates the swap
  // when the kind changes, so the live numbers embedded in these lines
  // (buffer fill, bitrate, countdown) tick in place without pulsing.
  let sub = '', subKey = ds;
  if (s.tail_ms > 0){
    sub = 'OBS stopped · your viewers are still watching the delay';
    subKey = 'tail';
  }
  else if (ds === 'off'){
    sub = s.ingest_alive
      ? `Live with no delay · ${fmtKbps(s.stats.bitrate_kbps)} in`
      : `Waiting for ${encoderApp() || 'your stream'} to connect…`;
    subKey = s.ingest_alive ? 'off-live' : 'off-wait';
  }
  else if (ds === 'arming') sub = `Saving video: ${fillSec.toFixed(1)}s of ${targetSec.toFixed(1)}s · viewers see you live until you activate.`;
  else if (ds === 'armed') sub = `Ready. Press Activate to put viewers on the delay.`;
  else if (ds === 'active' && adjusting){
    // Honest copy: the delay never "rewinds" gradually - the cut
    // machinery waits until the ring holds enough history, then does
    // ONE IDR-aligned jump. While waiting, the server sets
    // buffer_building; the brief pre-jump window (< ~1 s, the cut
    // check's throttle) is the only other way to land here.
    if (s.buffer_building){
      sub = `Saving more video: moves to ${fmtDelay(armedMs)} once ${fillSec.toFixed(1)}s saved reaches it.`;
      subKey = 'adjust-building';
    } else {
      const dir = armedMs > curMs ? 'back' : 'forward';
      sub = dir === 'back' ? `Jumping back to ${fmtDelay(armedMs)} of delay…` : `Jumping forward to ${fmtDelay(armedMs)} of delay…`;
      subKey = 'adjust-' + dir;
    }
  }
  else if (ds === 'active' && s.safe_cut_pending){
    const cd = Math.max(0, Math.round((s.safe_cut_remaining_ms || 0) / 1000));
    sub = `Auto-cut armed: going live in ~${cd}s, everything up to your mark still airs.`;
    subKey = 'safecut';
  }
  else if (ds === 'active') sub = `Viewers see you ${fmtDelay(s.armed_delay_ms)} behind${s.ingest_alive ? ` · `+fmtKbps(s.stats.bitrate_kbps)+' in' : '.'}`;
  // The armed delay is longer than the buffer holds at this bitrate, so it
  // runs short (the server measured it: capacity_short). Say so where the
  // delay is, unless a scheduled cut is the more urgent news.
  const short = s.capacity_short;
  if (short && ds !== 'off' && subKey !== 'safecut' && !(s.tail_ms > 0)){
    sub = `The buffer holds only ${fmtDelay(short.holds_ms)} of this ${fmtDelay(short.armed_ms)} delay at this bitrate. Make the buffer bigger in System.`;
    subKey = 'short';
  }
  setSubText($('delay-sub'), sub, subKey);

  renderCta();
}

function renderCta(){
  renderDelayRange();
  const btn = $('cp-cta');
  const cancel = $('cp-cancel');
  const warn = $('cp-cap-warn');
  const inputEl = $('delay-input');
  const clearWarn = () => {
    setClass(inputEl, 'field-error', false);
    animateHidden(warn, true);
  };
  if (pendingPhase){
    setCta(btn, { key: 'applying', cls: 'ic-btn-disabled', pre: 'Applying', disabled: true, busy: true });
    slideHidden(cancel, true);
    renderCutAfter('off');
    clearWarn();
    return;
  }
  if (!lastState){
    setCta(btn, { key: 'boot', cls: 'ic-btn-disabled', pre: 'Arm', disabled: true });
    setHidden(cancel, true);
    renderCutAfter('off');
    clearWarn();
    return;
  }
  const ds = PHASE_TO_STATE[lastState.phase] || 'off';
  const inputMs = delayInputSeconds() * 1000;
  const armedMs = lastState.armed_delay_ms || 0;
  // "User wants to change to something different from what's currently
  // armed/active" - the trigger for re-arm / adjust UX. Treat empty
  // input as "no change requested" so default actions (Activate / Cut)
  // stay accessible without the user clearing the field.
  const wantsChange = inputMs > 0 && inputMs !== armedMs;
  const cap = currentBufferCapMs();
  const tooBig = wantsChange && cap > 0 && inputMs > cap;
  // The secondary Disarm shows only while armed, so the user always has an
  // exit path (without it, "armed" state forced you to activate+cut just to
  // get back to passthrough). While arming, the main button itself reads
  // "Cancel". Decided once here, so a tick never shows and re-hides it.
  slideHidden(cancel, ds !== 'armed');

  // Capacity refusal is the same message regardless of which state we
  // were in when the user typed an oversize value - pull it out once.
  // Rewritten only when the text changes, so the tick doesn't churn it.
  const showCapWarn = () => {
    setClass(inputEl, 'field-error', true);
    if (warn){
      const needMB = requiredMBForMs(inputMs);
      // Before OBS sends anything the bitrate is a typical one, so the
      // limit is an estimate; once measured it is what /arm enforces.
      const html = (lastState.capacity_measured
        ? `<b>Can't go to ${fmtDelay(inputMs)} at ${capacityRate()}.</b> The ${runningBufferMb()} MB buffer holds up to ${fmtDelay(cap)}; this needs at least ${needMB} MB.`
        : `<b>Likely too long for the buffer.</b> At a typical ${capacityRate()}, the ${runningBufferMb()} MB buffer holds about ${fmtDelay(cap)}; ${fmtDelay(inputMs)} needs about ${needMB} MB. Your real bitrate decides once OBS connects.`)
        + bufferAdviceHtml(inputMs);
      if (warn._html !== html){ warn._html = html; warn.innerHTML = html; }
      animateHidden(warn, false);
    }
  };
  const capLabel = () =>
    ({ key: 'cap', cls: 'ic-btn-disabled', pre: 'Buffer holds only ', num: fmtDelay(cap), n: cap, post: ': make it bigger first', disabled: true });

  if (ds === 'arming'){
    // Primary CTA already does "cancel" semantics here; the secondary
    // button is a clearer alias for users who don't read the label.
    setCta(btn, { key: 'cancel-arming', cls: 'ic-btn-ghost', pre: '✕ Cancel' });
    clearWarn();
  } else if (ds === 'armed'){
    if (wantsChange && tooBig){
      setCta(btn, capLabel());
      showCapWarn();
    } else if (wantsChange){
      // User typed a new delay while armed → re-arm at the new value.
      // arm() handles "smaller is instant, bigger waits for buffer".
      setCta(btn, { key: 'rearm', cls: 'ic-btn-primary', pre: '↻ Re-arm' });
      clearWarn();
    } else if (!lastState.ingest_alive){
      // Nothing is arriving, so there is no buffer to switch on and the
      // backend refuses this anyway. Say what it is waiting for instead of
      // offering a button that can only fail.
      setCta(btn, { key: 'wait-armed', cls: 'ic-btn-disabled cp-cta-waiting',
                    pre: `Waiting for ${encoderApp() || 'your stream'}. The delay starts when it connects`, disabled: true, waiting: true });
      setText(cancel, 'Disarm');
      clearWarn();
    } else {
      // One word: the number above already says how long the delay is.
      setCta(btn, { key: 'activate', cls: 'ic-btn-primary cp-cta-pulse', pre: '▶ Activate' });
      setText(cancel, 'Disarm');
      clearWarn();
    }
  } else if (ds === 'active'){
    if (wantsChange && tooBig){
      setCta(btn, capLabel());
      showCapWarn();
    } else if (wantsChange){
      // Live delay adjustment - the backend's arm_delay already routes
      // this to a live-update path: smaller is an instant forward cut,
      // larger triggers a rewind once the buffer has the history.
      setCta(btn, { key: 'adjust', cls: 'ic-btn-primary', pre: '↻ Adjust' });
      clearWarn();
    } else {
      setCta(btn, { key: 'cut', cls: 'ic-btn-danger', pre: '✕ Cut' });
      clearWarn();
    }
  } else {
    // off / idle (== passthrough or genuinely no source)
    if (!lastState.ingest_alive){
      // Nothing to delay yet, and the backend refuses it: Arm waits, and
      // the line under the number says what for.
      setCta(btn, { key: 'wait', cls: 'ic-btn-disabled', pre: 'Arm', disabled: true });
      // A typed delay past the range is flagged now, not first when OBS
      // connects: the field must never sit at 712 s under "0 - 48s".
      if (inputMs > 0 && tooBig) showCapWarn(); else clearWarn();
    } else if (inputMs > 0 && tooBig){
      setCta(btn, capLabel());
      showCapWarn();
    } else if (inputMs > 0){
      setCta(btn, { key: 'arm', cls: 'ic-btn-primary', pre: '⚡ Arm' });
      clearWarn();
    } else {
      setCta(btn, { key: 'idle', cls: 'ic-btn-disabled', pre: 'Arm', disabled: true });
      clearWarn();
    }
  }
  renderCutAfter(ds);
}

// - Main delay button ----------------------------------------------------
// renderCta runs on every state tick (~4x/s) and almost always lands on
// the same label, so setCta diffs against the last one and touches the
// DOM only on a real change. A new action (Arm → Activate) hands over:
// the old label lifts out as the new one settles in, and the fill blends
// (CSS transitions). Same action, new number (Arm 30s → Arm 31s): only
// the number flips. A profile launch can hold the button for the spark's
// flight so the change lands with it (holdCtaFor).
let ctaHoldUntil = 0, ctaHoldTimer = 0, ctaQueued = null;
function holdCtaFor(ms){ ctaHoldUntil = performance.now() + ms; }
function setCta(btn, next){
  if (!btn) return;
  const wait = ctaHoldUntil - performance.now();
  if (wait > 0){
    ctaQueued = next;
    if (!ctaHoldTimer) ctaHoldTimer = setTimeout(() => {
      ctaHoldTimer = 0;
      const queued = ctaQueued;
      ctaQueued = null;
      if (queued) setCta(btn, queued);
    }, wait);
    return;
  }
  const prev = btn._cta;
  btn._cta = next;
  if (!prev || prev.cls !== next.cls) btn.className = 'ic-btn cp-cta ' + next.cls;
  const disabled = !!next.disabled;
  if (btn.disabled !== disabled) btn.disabled = disabled;
  if (prev && prev.key === next.key && prev.num === next.num && prev.pre === next.pre) return;

  const animate = !!prev && prev.key !== 'boot' && motionOK();
  const current = btn.querySelector('.cta-label:not(.leaving)');
  if (prev && current && prev.key === next.key && prev.pre === next.pre){
    const numEl = current.querySelector('.cta-num');
    if (animate) flipCtaNumber(numEl, next.num || '', (next.n || 0) >= (prev.n || 0) ? 1 : -1);
    else numEl.textContent = next.num || '';
    return;
  }
  const label = ctaLabel(next);
  if (!animate || !current){
    if (prev) btn.querySelectorAll('.cta-label').forEach(n => n.remove());
    else btn.replaceChildren();
    btn.appendChild(label);
    return;
  }
  // At most one label is ever on its way out, whatever the click rate or
  // however long a background tab held its fade.
  btn.querySelectorAll('.cta-label.leaving').forEach(n => n.remove());
  current.classList.add('leaving');
  current.setAttribute('aria-hidden', 'true');
  current.animate(
    [{ opacity: 1, transform: 'none', filter: 'blur(0)' }, { opacity: 0, transform: 'translateY(-9px)', filter: 'blur(4px)' }],
    { duration: 150, easing: 'ease-in', fill: 'forwards' }).finished.finally(() => current.remove()).catch(() => {});
  btn.appendChild(label);
  label.animate(
    [{ opacity: 0, transform: 'translateY(9px)', filter: 'blur(4px)' }, { opacity: 1, transform: 'none', filter: 'blur(0)' }],
    { duration: 240, delay: 60, easing: 'cubic-bezier(.2,.7,.2,1)', fill: 'backwards' });
  // OBS just connected and the button became usable: mark it once.
  if (prev.waiting && !disabled) restartAnimation(btn, 'cp-cta-bloom');
}
function ctaLabel(d){
  const label = document.createElement('span');
  label.className = 'cta-label';
  const num = document.createElement('span');
  num.className = 'cta-num';
  num.textContent = d.num || '';
  label.append(d.pre || '', num, d.post || '');
  if (d.waiting || d.busy){
    const dots = document.createElement('span');
    dots.className = 'cta-dots';
    dots.setAttribute('aria-hidden', 'true');
    dots.innerHTML = '<i>.</i><i>.</i><i>.</i>';
    label.appendChild(dots);
  }
  return label;
}
// The new number slides in from the side it came from (below for more).
// The text is written first and never waits on an animation event, so a
// paused or cancelled animation can't leave a stale number behind; a
// faster click just restarts the slide.
function flipCtaNumber(el, text, dir){
  el.textContent = text;
  el.getAnimations().forEach(a => a.cancel());
  el.animate([{ transform: `translateY(${45 * dir}%)`, opacity: 0 }, { transform: 'none', opacity: 1 }],
    { duration: 180, easing: 'cubic-bezier(.2,.7,.2,1)' });
}

// - "Cut after this airs" secondary action --------------
// Only offered while a delay is active - there's nothing to schedule
// otherwise (the backend refuses too, this just keeps the button from
// teasing a dead action). Shares the CTA row at 1/4 width, so labels
// stay compact and the full story lives in the hovercard + title.
// While a mark is pending the button becomes a live countdown and
// clicking it cancels. The hovercard copy swaps with the mode; guarded
// by data-mode so the ~1 Hz state ticks don't rebuild identical DOM
// under the user's cursor.
function renderCutAfter(ds){
  const wrap = $('cp-cutafter-wrap');
  if (!wrap) return;
  const show = !pendingPhase && !!lastState && ds === 'active';
  slideHidden(wrap, !show);
  if (!show) return;
  const btn = $('cp-cutafter');
  const card = $('cp-cutafter-card');
  const pending = !!lastState.safe_cut_pending;
  setClass(btn, 'pending', pending);
  if (pending){
    const s = Math.max(0, Math.round((lastState.safe_cut_remaining_ms || 0) / 1000));
    setText(btn, `⏱ ~${s}s ✕`);
    if (card && card.dataset.mode !== 'pending'){
      card.dataset.mode = 'pending';
      card.innerHTML = `<b>Mark set.</b>
        <span>Everything up to your press is still going out to viewers. The instant it has aired on every destination, the delay cuts itself to live.</span>
        <span class="cutafter-mini mono">tap the button again to cancel</span>`;
    }
  } else {
    setText(btn, '⏱ Cut after');
    if (card && card.dataset.mode !== 'idle'){
      card.dataset.mode = 'idle';
      card.innerHTML = `<b>Stop counting your delay.</b>
        <span>Match over? Press this the moment your reaction ends. Everything up to that press still reaches your viewers, then the delay cuts itself to live.</span>
        <span class="cutafter-mini mono">press now&ensp;▸&ensp;reaction airs&ensp;▸&ensp;auto-cut&ensp;▸&ensp;live</span>`;
    }
  }
}

// - Profile chips in hero -----------------------
// Chips shown under the delay field; more profiles get a "+N more" chip.
const CP_CHIPS_MAX = 5;
function renderCpProfiles(){
  const c = $('cp-profiles');
  const ds = lastState ? PHASE_TO_STATE[lastState.phase] : 'off';
  const target = lastState ? lastState.armed_delay_ms : 0;
  const cap = currentBufferCapMs();
  // applyState calls this every state tick (~4x/s). Skip the pass unless
  // something that affects the chips actually changed - the profile list,
  // the armed target, the phase-group, or which chips are over capacity
  // (stable thanks to the bitrate peak-hold).
  const list = (profiles || []).slice(0, CP_CHIPS_MAX);
  const more = Math.max(0, (profiles || []).length - CP_CHIPS_MAX);
  const sig = JSON.stringify([list.map(p => [p.name, p.delay_ms]), ds, target,
    list.map(p => (cap > 0 && p.delay_ms > cap) ? 1 : 0), more]);
  if (c._sig === sig) return;
  c._sig = sig;
  // The rest stay one click away in the Profiles tab.
  let moreChip = c.querySelector('.cp-more');
  if (moreChip) moreChip.remove();
  // Patch the chips in place instead of rebuilding them: a rebuilt chip
  // can't transition, so the armed fill would snap in, or replay on every
  // phase change. Every write is skipped when the value already matches.
  while (c.children.length > list.length) c.lastElementChild.remove();
  list.forEach((p, i) => {
    let b = c.children[i];
    if (!b){
      b = document.createElement('button');
      b.type = 'button';
      b.innerHTML = '<span class="bolt" style="color:var(--accent)">⚡</span><span class="chip-delay"></span>';
      c.appendChild(b);
    }
    const tooBig = cap > 0 && p.delay_ms > cap;
    const isCur = target === p.delay_ms && ds !== 'off';
    const cls = 'cp-profile-chip' + (isCur ? ' on' : '') + (tooBig ? ' too-big' : '');
    if (b.className !== cls) b.className = cls;
    // Stay clickable while armed/active so the user can pivot between
    // profiles without disarming first - arm() routes the request to
    // the live-adjust path. Only the "arming" mid-fill state and the
    // explicit current profile are disabled. A too-big chip stays
    // focusable and clickable (aria-disabled) so it can say why it can't
    // arm (explainProfile) - a tooltip alone never reaches touch or
    // keyboard users.
    const disabled = isCur || ds === 'arming';
    if (b.disabled !== disabled) b.disabled = disabled;
    if (tooBig) b.setAttribute('aria-disabled', 'true'); else b.removeAttribute('aria-disabled');
    const title = tooBig ? `${p.name}: too long for the buffer. Click for details.`
                : ds === 'active' && !isCur ? `Adjust to ${p.name}` : `Arm: ${p.name}`;
    if (b.title !== tr(title, b)) b.title = title;
    b.dataset.ms = p.delay_ms;
    b.dataset.name = p.name;
    const label = b.lastElementChild;
    const text = fmtDelay(p.delay_ms);
    if (label.textContent !== text) label.textContent = text;
  });
  if (more){
    if (!moreChip){
      moreChip = document.createElement('button');
      moreChip.type = 'button';
      moreChip.className = 'cp-profile-chip cp-more';
    }
    moreChip.textContent = `+${more} more`;
    moreChip.title = `See all ${profiles.length} profiles`;
    c.appendChild(moreChip);
  }
}
// One listener for every chip, so patching chips never rebinds handlers.
$('cp-profiles') && $('cp-profiles').addEventListener('click', e => {
  const chip = e.target.closest('.cp-profile-chip');
  if (!chip || chip.disabled) return;
  if (chip.classList.contains('cp-more')){ showTab('profiles'); return; }
  const ms = Number(chip.dataset.ms);
  if (chip.classList.contains('too-big')){ explainProfile(chip, ms); return; }
  hideProfileMessage();
  launchProfile(chip);
  setDelayInput(Math.round(ms / 1000));
  arm(ms);
});

// A too-big profile wiggles and says what it needs, in a message that
// stays up a few seconds (renderCta's per-tick reset doesn't own it).
let profileMessageTimer = 0;
function explainProfile(chip, ms){
  const msg = $('cp-chip-msg');
  if (!msg) return;
  // One sentence per element, so each one translates on its own.
  msg.innerHTML = `<b>This profile is too long for your buffer.</b> <span>It needs at least ${requiredMBForMs(ms)} MB.</span>`
    + bufferAdviceHtml(ms);
  animateHidden(msg, false);
  if (motionOK()) chip.animate([0, -5, 5, -3, 2, 0].map(r => ({ transform: `rotate(${r}deg)` })),
    { duration: 400, easing: 'ease-out' });
  clearTimeout(profileMessageTimer);
  profileMessageTimer = setTimeout(hideProfileMessage, 6000);
}
function hideProfileMessage(){
  clearTimeout(profileMessageTimer);
  animateHidden($('cp-chip-msg'), true);
}

// Profile launch: the bolt flashes and a spark arcs from the chip into the
// main button, which ripples as it takes the new state. The button holds
// its label for the flight so the change lands with the spark. Click-only:
// nothing here runs from a state tick.
const PROFILE_FLIGHT_MS = 420;
function launchProfile(chip){
  const cta = $('cp-cta');
  if (!cta || !motionOK()) return;
  const bolt = chip.querySelector('.bolt') || chip;
  if (!cta.getBoundingClientRect().width) return;
  const ease = 'cubic-bezier(.2,.7,.2,1)';
  chip.animate([{ transform: 'none' }, { transform: 'scale(.92)' }, { transform: 'none' }], { duration: 220, easing: ease });
  const glow = px => `drop-shadow(0 0 ${px}px oklch(0.80 0.17 65 / .9))`;
  bolt.animate([{ transform: 'none', filter: glow(0) }, { transform: 'scale(1.5) rotate(-14deg)', filter: glow(6) },
                { transform: 'none', filter: glow(0) }], { duration: 420, easing: ease });
  holdCtaFor(PROFILE_FLIGHT_MS);
  flyTrail(bolt, cta, () => rippleCta(cta), {ms: PROFILE_FLIGHT_MS, warm: true});
}
function rippleCta(cta){
  const ripple = document.createElement('span');
  ripple.className = 'cp-ripple';
  cta.appendChild(ripple);
  ripple.animate([{ transform: 'scale(0)', opacity: .9 }, { transform: `scale(${cta.offsetWidth / 16})`, opacity: 0 }],
    { duration: 520, easing: 'cubic-bezier(.2,.8,.3,1)' }).finished.finally(() => ripple.remove()).catch(() => {});
}

