// ---------------------------------------------------------------- rendering

// Redraws never replace innerHTML: morph() patches the live DOM to match
// the new HTML, so focus, carets, scroll and CSS transitions survive. A
// child with data-key is matched by key. Inside a [data-flip] parent,
// keyed children slide to their new place, fade in when new and fade out
// when gone; a [data-autoh] element animates to its new height.
const EASE = 'cubic-bezier(.2,.7,.2,1)';
const reduced = () => typeof prefersReducedMotion === 'function' && prefersReducedMotion();
// Buttons waiting on the app keep their spinner through redraws.
const busyEls = new WeakSet();

function morph(root, html){
  if (!root) return;
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  const motion = !reduced() && root.isConnected && root.getClientRects().length > 0;
  const ctx = {before: motion ? measure(root) : null, entered:[], left:[]};
  const focused = document.activeElement;
  morphChildren(root, tpl.content, ctx);
  // Moving a node drops its focus; give it back.
  if (focused && focused !== document.activeElement && focused.isConnected && root.contains(focused)
    && !focused.closest('[data-leaving]')) focused.focus({preventScroll:true});
  if (motion) animateMorph(ctx);
  placeIndicators(root);
  fillRanges(root);
}

function measure(root){
  const rects = new Map(), heights = new Map();
  root.querySelectorAll('[data-flip]').forEach(p => {
    rects.set(p, p.getBoundingClientRect());
    for (const c of p.children){
      if (c.hasAttribute('data-key') && !c.hasAttribute('data-leaving')) rects.set(c, c.getBoundingClientRect());
    }
  });
  root.querySelectorAll('[data-autoh]').forEach(el => heights.set(el, el.getBoundingClientRect().height));
  return {rects, heights};
}

const keyOf = n => n.nodeType === 1 ? n.getAttribute('data-key') : null;
// Leaving nodes are fading out; persistent ones (the unsaved-changes bar)
// were added outside the template. Morph leaves both alone.
const isLeaving = n => n.nodeType === 1 && (n.hasAttribute('data-leaving') || n.hasAttribute('data-persist'));
function nextLive(n){
  while (n && isLeaving(n)) n = n.nextSibling;
  return n;
}

function morphChildren(parent, src, ctx){
  const olds = [...parent.childNodes].filter(n => !isLeaving(n));
  const byKey = new Map();
  olds.forEach(n => { const k = keyOf(n); if (k) byKey.set(k, n); });
  const used = new Set();
  let oi = 0;
  let ref = nextLive(parent.firstChild);
  for (const next of [...src.childNodes]){
    const k = keyOf(next);
    let match = null;
    if (k){
      const c = byKey.get(k);
      if (c && c.nodeName === next.nodeName && !used.has(c)) match = c;
    } else {
      while (oi < olds.length && (used.has(olds[oi]) || keyOf(olds[oi]))) oi++;
      const c = olds[oi];
      if (c && c.nodeName === next.nodeName){ match = c; oi++; }
    }
    let node = next;
    if (match){ used.add(match); patch(match, next, ctx); node = match; }
    else if (k) ctx.entered.push(next);
    if (node === ref) ref = nextLive(ref.nextSibling);
    else parent.insertBefore(node, ref);
  }
  olds.forEach(n => { if (!used.has(n)) removeNode(n, parent, ctx); });
}

function patch(a, b, ctx){
  if (a.nodeType !== 1){
    // What is on screen is in the page's language (see web/i18n.js).
    const said = tr(b.nodeValue, a.parentElement);
    if (a.nodeValue !== said){
      const was = a.nodeValue;
      a.nodeValue = said;
      if (ctx.before) tick(a.parentElement, was, said);
    }
    return;
  }
  const tag = a.tagName;
  // Form fields: only push a value the model changed, so whatever the user
  // typed into a field the model doesn't track yet survives a redraw.
  const oldValue = a.getAttribute('value'), oldChecked = a.hasAttribute('checked');
  const oldSelected = tag === 'SELECT' ? selectedOf(a) : null;
  syncAttrs(a, b);
  // The new markup has no height for a message box: measure it again, or a
  // redraw (focusing it shows "Reads like") shrinks it and cuts its text.
  if (tag === 'TEXTAREA' && a.classList.contains('ig-msg')) autosize(a);
  // A busy button redrawn as disabled (Preview turning into a locked Add)
  // stays disabled once the work is done: see `busy`.
  if (tag === 'BUTTON') a._wantDisabled = b.hasAttribute('disabled');
  if (busyEls.has(a)){ a.classList.add('is-busy'); a.disabled = true; a.setAttribute('aria-busy', 'true'); }
  if (decoders.has(a)) a.classList.add('ig-decoding');
  if (tag === 'INPUT'){
    if (a.type === 'checkbox' || a.type === 'radio'){
      if (b.hasAttribute('checked') !== oldChecked) a.checked = b.hasAttribute('checked');
    } else {
      const v = b.getAttribute('value') || '';
      if (v !== (oldValue || '') && a.value !== v && a !== document.activeElement) a.value = v;
    }
    return;
  }
  if (tag === 'TEXTAREA'){
    const v = b.textContent;
    if (v !== a.defaultValue){
      a.defaultValue = v;
      if (a.value !== v && a !== document.activeElement) a.value = v;
    }
    return;
  }
  morphChildren(a, b, ctx);
  if (tag === 'SELECT'){
    const now = selectedOf(a);
    if (now !== oldSelected && a.value !== now) a.value = now;
  }
}

function selectedOf(select){
  const o = select.querySelector('option[selected]') || select.querySelector('option');
  return o ? o.value : '';
}

function syncAttrs(a, b){
  // A <details> belongs to the user once drawn; textareas keep the height
  // autosize gave them.
  const keep = name => (name === 'open' && a.tagName === 'DETAILS')
    || (name === 'style' && a.hasAttribute('data-keep-style'));
  for (const {name} of [...a.attributes]){
    if (!b.hasAttribute(name) && !keep(name)) a.removeAttribute(name);
  }
  for (const {name, value} of b.attributes){
    const said = tr.ATTRS.includes(name) ? tr(value, a) : value;
    if (a.getAttribute(name) !== said && !keep(name)) a.setAttribute(name, said);
  }
}

function removeNode(n, parent, ctx){
  const r = ctx.before && ctx.before.rects.get(n);
  const pr = ctx.before && ctx.before.rects.get(parent);
  if (!r || !pr || !r.width){ n.remove(); return; }
  // Out of the flow at once, so the rest slides into place while it fades.
  n.setAttribute('data-leaving', '');
  n.inert = true;
  Object.assign(n.style, {
    position:'absolute', margin:'0', pointerEvents:'none',
    left:(r.left - pr.left - parent.clientLeft + parent.scrollLeft) + 'px',
    top:(r.top - pr.top - parent.clientTop + parent.scrollTop) + 'px',
    width:r.width + 'px', height:r.height + 'px',
  });
  ctx.left.push(n);
}

function animateMorph(ctx){
  const {rects, heights} = ctx.before;
  // Slide what moved, measured against its parent so nested lists don't
  // move twice.
  rects.forEach((old, el) => {
    if (!el.hasAttribute('data-key') || !el.isConnected || el.hasAttribute('data-leaving') || !old.width) return;
    const parent = el.parentElement, pOld = rects.get(parent);
    if (!pOld) return;
    // A slide still running restarts from where it is; entrance animations
    // are the same in both measurements, so they are left alone.
    el.getAnimations().forEach(x => { if (x.id === 'flip') x.cancel(); });
    const now = el.getBoundingClientRect(), pNow = parent.getBoundingClientRect();
    const dx = (old.left - pOld.left) - (now.left - pNow.left);
    const dy = (old.top - pOld.top) - (now.top - pNow.top);
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
    el.animate([{transform:`translate(${dx}px,${dy}px)`}, {transform:'none'}], {id:'flip', duration:300, easing:EASE});
  });
  let n = 0;
  ctx.entered.forEach(el => {
    const parent = el.parentElement;
    if (!parent || !rects.has(parent) || !parent.hasAttribute('data-flip')) return;
    el.animate([
      {opacity:0, transform:'translateY(6px) scale(.98)', filter:'blur(3px)'},
      {opacity:getComputedStyle(el).opacity, transform:'none', filter:'blur(0)'},
    ], {duration:280, easing:EASE, delay:Math.min(n++, 8) * 28, fill:'backwards'});
  });
  ctx.left.forEach(el => {
    const a = el.animate([{opacity:1, transform:'none'}, {opacity:0, transform:'translateY(-4px) scale(.98)'}],
      {duration:170, easing:EASE, fill:'forwards'});
    a.onfinish = () => el.remove();
    a.oncancel = () => el.remove();
  });
  heights.forEach((h, el) => {
    if (!el.isConnected) return;
    const now = el.getBoundingClientRect().height;
    if (Math.abs(now - h) < 1) return;
    el.animate([{height:h + 'px'}, {height:now + 'px'}], {duration:260, easing:EASE});
  });
}

// Cards ease in one after another when the tab opens. Animations, not a
// class, so the redraw that lands mid-way can't cut them short.
function enterStagger(root){
  if (!root || reduced()) return;
  [...root.querySelectorAll('.ig-grid > [data-key], .ig-list > [data-key], .ig-starter')].slice(0, 24).forEach((el, n) => {
    // Up to its own opacity: a card that's off is dimmer than the others.
    el.animate([{opacity:0, transform:'translateY(8px)', filter:'blur(3px)'}, {opacity:getComputedStyle(el).opacity, transform:'none', filter:'blur(0)'}],
      {duration:340, easing:EASE, delay:Math.min(n, 12) * 30, fill:'backwards'});
  });
}

// Segmented controls carry the dashboard's sliding pill; it is placed
// under the active tab after every redraw, and glides when that changes.
// Its row too: a narrow window wraps a control onto two lines.
const SEG_IND = '<span class="sub-tab-ind" data-keep-style aria-hidden="true"></span>';

// A slider's track fills up to its thumb (--p, read by index.html's CSS).
function fillRanges(root){ root.querySelectorAll('input[type=range]').forEach(fillRange); }
function placeIndicator(seg){
  const ind = seg.querySelector(':scope > .sub-tab-ind');
  const on = seg.querySelector(':scope > .sub-tab.on');
  if (!ind) return;
  if (!on || !on.offsetWidth){ ind.style.opacity = '0'; return; }
  ind.style.left = on.offsetLeft + 'px';
  ind.style.width = on.offsetWidth + 'px';
  ind.style.top = on.offsetTop + 'px';
  ind.style.height = on.offsetHeight + 'px';
  ind.style.opacity = '1';
}
// A tab changes size after the redraw that placed the pill: the translator
// swaps its words, a folded panel opens around it.
const segTabSizes = new ResizeObserver(entries => {
  for (const seg of new Set(entries.map(e => e.target.closest('.ig-seg')))) if (seg) placeIndicator(seg);
});
function placeIndicators(root){
  root.querySelectorAll('.ig-seg').forEach(seg => {
    placeIndicator(seg);
    seg.querySelectorAll(':scope > .sub-tab').forEach(tab => segTabSizes.observe(tab));
  });
}

