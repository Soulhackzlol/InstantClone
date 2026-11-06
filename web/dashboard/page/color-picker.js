// - Color picker -------------------------
// Every <input type="color"> opens the app's own picker instead of the
// browser's, which looks foreign and differs between Chrome, Firefox and
// OBS's docks. It writes the input's value and fires `input` while you drag
// and `change` when it closes, so whatever listened to the native picker
// still works. A redraw that replaces the input is followed: the picker
// finds the new one by the same attributes.
const CP_RECENT_KEY = 'ic.recentColors';
const CP_RECENT_MAX = 8;
const CP_APP_COLORS = ['#5ac8fa', '#3ee0c5', '#54e38e', '#ffd166', '#ff9f43', '#ff6b6b', '#ff7eb6', '#b28dff'];
const CP_GAP = 8;
const CP_SHEET_MAX = 520;

function hexToHsv(hex){
  const m = /^#?([0-9a-f]{6})$/i.exec(hex || '');
  const n = m ? parseInt(m[1], 16) : 0x5ac8fa;
  const r = (n >> 16 & 255) / 255, g = (n >> 8 & 255) / 255, b = (n & 255) / 255;
  const max = Math.max(r, g, b), d = max - Math.min(r, g, b);
  let h = 0;
  if (d) h = max === r ? ((g - b) / d + 6) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
  return {h: h * 60, s: max ? d / max : 0, v: max};
}
function hsvToHex({h, s, v}){
  const f = k => {
    const x = (k + h / 60) % 6;
    return Math.round((v - v * s * Math.max(0, Math.min(x, 4 - x, 1))) * 255);
  };
  return '#' + [f(5), f(3), f(1)].map(c => c.toString(16).padStart(2, '0')).join('');
}

const ColorPick = {
  el:null, input:null, finder:'', was:'', hsv:{h:0, s:0, v:0}, frame:0, emitting:0,

  open(input){
    if (this.input === input) return this.close();
    if (this.el) this.close();
    this.input = input;
    this.finder = this.finderOf(input);
    // Several inputs can match the finder (no id, same class): which one
    // this was, so a redraw finds the same field again and not the first.
    this.finderAt = Math.max(0, [...document.querySelectorAll(this.finder)].indexOf(input));
    this.was = input.value || '#000000';
    this.hsv = hexToHsv(this.was);
    this.el = this.build();
    document.body.appendChild(this.el);
    this.paint(false);
    this.follow();
    this.el.querySelector('.cp-sv').focus({preventScroll:true});
  },

  // Closing keeps the color, like the browser's picker: putting the old
  // one back would set it outright, not undo the change (a theme's color
  // would become a fixed one).
  close(){
    const el = this.el, input = this.input;
    if (!el) return;
    cancelAnimationFrame(this.frame);
    this.el = null;
    this.input = null;
    if (input && input.isConnected){
      if (input.value !== this.was){
        input.dispatchEvent(new Event('change', {bubbles:true}));
        this.remember(input.value);
      }
      input.focus({preventScroll:true});
    }
    el.classList.add('closing');
    el.addEventListener('animationend', () => el.remove(), {once:true});
    setTimeout(() => el.remove(), 300);
  },

  // A selector that finds this input again after a redraw replaced it.
  finderOf(input){
    if (input.id) return '#' + CSS.escape(input.id);
    const attrs = ['data-bind', 'data-name', 'aria-label', 'oninput', 'class']
      .filter(name => input.hasAttribute(name))
      .map(name => `[${name}=${JSON.stringify(input.getAttribute(name))}]`);
    return 'input[type=color]' + attrs.join('');
  },

  build(){
    const el = document.createElement('div');
    el.className = 'cp';
    el.setAttribute('role', 'dialog');
    el.setAttribute('aria-label', 'Pick a color');
    const eye = 'EyeDropper' in window
      ? `<button type="button" class="cp-icon" data-cp="eye" title="Pick a color from the screen" aria-label="Pick a color from the screen"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m2 22 1-1h3l9-9"/><path d="M3 21v-3l9-9"/><path d="m15 6 3.4-3.4a2.1 2.1 0 1 1 3 3L18 9l.4.4a2.1 2.1 0 1 1-3 3l-3.8-3.8a2.1 2.1 0 1 1 3-3l.4.4Z"/></svg></button>`
      : '';
    const recent = this.recent();
    const swatches = list => list.map(c => `<button type="button" class="cp-sw" data-cp="sw" data-c="${c}" style="--sw:${c}" title="${c}" aria-label="${c}"></button>`).join('');
    el.innerHTML = `<div class="cp-sv" tabindex="0" role="slider" aria-label="Shade: drag, or use the arrow keys"><span class="cp-thumb"></span></div>
      <div class="cp-row"><span class="cp-now" title="Before and now"></span>
        <div class="cp-hue" tabindex="0" role="slider" aria-label="Hue" aria-valuemin="0" aria-valuemax="360"><span class="cp-thumb"></span></div></div>
      <div class="cp-row">${eye}<input class="ic-input mono cp-hex" maxlength="7" spellcheck="false" autocomplete="off" aria-label="Hex value"></div>
      ${recent.length ? `<div class="cp-label">Recent</div><div class="cp-swatches">${swatches(recent)}</div>` : ''}
      <div class="cp-label">App colors</div><div class="cp-swatches">${swatches(CP_APP_COLORS)}</div>`;
    this.drag(el.querySelector('.cp-sv'), (x, y) => { this.hsv.s = x; this.hsv.v = 1 - y; });
    this.drag(el.querySelector('.cp-hue'), x => { this.hsv.h = x * 360; });
    el.addEventListener('keydown', e => this.onKey(e));
    el.addEventListener('click', e => this.onClick(e));
    el.querySelector('.cp-hex').addEventListener('input', e => {
      const value = e.target.value.trim().replace(/^#?/, '#');
      if (!/^#[0-9a-f]{6}$/i.test(value)) return;
      this.hsv = hexToHsv(value);
      this.paint(true, true);
    });
    return el;
  },

  // Pointer drags track 1:1, even outside the box, from where they pressed.
  drag(box, set){
    const at = e => {
      const r = box.getBoundingClientRect();
      set(Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)));
      this.paint(true);
    };
    box.addEventListener('pointerdown', e => {
      if (e.button) return;
      e.preventDefault();
      box.focus({preventScroll:true});
      box.setPointerCapture(e.pointerId);
      box.classList.add('dragging');
      at(e);
    });
    box.addEventListener('pointermove', e => { if (box.hasPointerCapture(e.pointerId)) at(e); });
    const end = () => box.classList.remove('dragging');
    box.addEventListener('pointerup', end);
    box.addEventListener('pointercancel', end);
  },

  onKey(e){
    // The picker's keys stay here: the modal behind must not close or
    // take the focus back.
    e.stopPropagation();
    // Enter on one of the picker's buttons (the eyedropper, a swatch)
    // presses it; anywhere else it closes, like Escape.
    if (e.key === 'Enter' && e.target.closest('button')) return;
    if (e.key === 'Escape' || e.key === 'Enter'){ e.preventDefault(); this.close(); return; }
    if (e.key === 'Tab') return this.trap(e);
    const step = e.shiftKey ? 0.1 : 0.01;
    const dx = {ArrowLeft:-1, ArrowRight:1}[e.key] || 0, dy = {ArrowUp:-1, ArrowDown:1}[e.key] || 0;
    if (!dx && !dy) return;
    const target = e.target.closest('.cp-sv, .cp-hue');
    if (!target) return;
    e.preventDefault();
    if (target.classList.contains('cp-hue')) this.hsv.h = (this.hsv.h + (dx || -dy) * step * 360 + 360) % 360;
    else {
      this.hsv.s = Math.min(1, Math.max(0, this.hsv.s + dx * step));
      this.hsv.v = Math.min(1, Math.max(0, this.hsv.v - dy * step));
    }
    this.paint(true);
  },

  trap(e){
    const stops = [...this.el.querySelectorAll('[tabindex="0"], input, button')];
    const i = stops.indexOf(document.activeElement);
    const next = stops[(i + (e.shiftKey ? -1 : 1) + stops.length) % stops.length];
    e.preventDefault();
    next.focus();
  },

  async onClick(e){
    const b = e.target.closest('[data-cp]');
    if (!b) return;
    if (b.dataset.cp === 'sw'){
      this.hsv = hexToHsv(b.dataset.c);
      this.paint(true);
      return;
    }
    if (b.dataset.cp === 'eye'){
      try {
        const picked = await new EyeDropper().open();
        if (!this.el) return;
        this.hsv = hexToHsv(picked.sRGBHex);
        this.paint(true);
      } catch(_){ /* closed with Esc: nothing picked */ }
    }
  },

  // Show the color, and when `emit`, hand it to the input. `typed`: the hex
  // field is being written, so it keeps what is typed.
  paint(emit, typed){
    const el = this.el;
    if (!el) return;
    const hex = hsvToHex(this.hsv);
    el.style.setProperty('--cp-h', Math.round(this.hsv.h));
    el.style.setProperty('--cp-c', hex);
    el.style.setProperty('--cp-was', this.was);
    const sv = el.querySelector('.cp-sv .cp-thumb'), hue = el.querySelector('.cp-hue .cp-thumb');
    sv.style.left = this.hsv.s * 100 + '%';
    sv.style.top = (1 - this.hsv.v) * 100 + '%';
    hue.style.left = this.hsv.h / 3.6 + '%';
    el.querySelector('.cp-hue').setAttribute('aria-valuenow', Math.round(this.hsv.h));
    el.querySelector('.cp-sv').setAttribute('aria-valuetext', hex);
    if (!typed) el.querySelector('.cp-hex').value = hex;
    if (emit) this.write(this.input, hex);
  },

  write(input, hex){
    if (!input || input.value === hex) return;
    input.value = hex;
    input.dispatchEvent(new Event('input', {bubbles:true}));
  },

  // Each frame: keep beside the input, which may move, or be replaced by
  // a redraw (then find the new one), or be gone (then close).
  follow(){
    if (!this.el) return;
    if (!this.input.isConnected){
      const again = document.querySelectorAll(this.finder)[this.finderAt];
      if (!again) return this.close();
      this.input = again;
    }
    // Measured without the opening animation's scale, so it doesn't drift.
    const r = this.input.getBoundingClientRect();
    const box = {width:this.el.offsetWidth, height:this.el.offsetHeight};
    // A phone shows it as a sheet along the bottom (see the CSS).
    if (innerWidth <= CP_SHEET_MAX){
      this.frame = requestAnimationFrame(() => this.follow());
      return;
    }
    const below = r.bottom + CP_GAP + box.height <= innerHeight - CP_GAP;
    const top = below ? r.bottom + CP_GAP : Math.max(CP_GAP, r.top - CP_GAP - box.height);
    const left = Math.min(innerWidth - box.width - CP_GAP, Math.max(CP_GAP, r.left + r.width / 2 - box.width / 2));
    this.el.style.top = top + 'px';
    this.el.style.left = left + 'px';
    this.el.style.setProperty('--cp-origin', `${r.left + r.width / 2 - left}px ${below ? 0 : box.height}px`);
    this.el.style.setProperty('--cp-dy', below ? '-4px' : '4px');
    this.frame = requestAnimationFrame(() => this.follow());
  },

  recent(){
    try { return JSON.parse(localStorage.getItem(CP_RECENT_KEY) || '[]').filter(c => /^#[0-9a-f]{6}$/i.test(c)); }
    catch(_){ return []; }
  },
  remember(hex){
    const list = [hex.toLowerCase(), ...this.recent().filter(c => c.toLowerCase() !== hex.toLowerCase())].slice(0, CP_RECENT_MAX);
    try { localStorage.setItem(CP_RECENT_KEY, JSON.stringify(list)); } catch(_){ /* private window: no recents */ }
  },
};
// A click (or Space/Enter) on a color input opens this picker instead.
document.addEventListener('click', e => {
  const input = e.target instanceof HTMLInputElement && e.target.type === 'color' ? e.target : null;
  if (!input || input.disabled) return;
  e.preventDefault();
  ColorPick.open(input);
}, true);
// A press anywhere else keeps the color and closes it.
document.addEventListener('pointerdown', e => {
  if (!ColorPick.el || ColorPick.el.contains(e.target)) return;
  if (e.target === ColorPick.input || (e.target.closest && e.target.closest('label') && e.target.closest('label').contains(ColorPick.input))) return;
  ColorPick.close();
}, true);
function staggerIn(root){
  if (!root || prefersReducedMotion()) return;
  const items = [...root.querySelectorAll(STAGGER_SELECTOR)].filter(el => el.offsetParent !== null);
  items.forEach(el => el.classList.remove('stagger-in'));
  void root.offsetWidth; // one reflow restarts every item's animation
  items.forEach((el, i) => {
    el.style.setProperty('--stagger', Math.min(i, STAGGER_MAX) * STAGGER_STEP_MS + 'ms');
    el.classList.add('stagger-in');
  });
}
// Play a one-shot animation class again even if it's still on.
function restartAnimation(el, cls){
  if (prefersReducedMotion()) return;
  el.classList.remove(cls);
  void el.offsetWidth;
  el.classList.add(cls);
}
// One-shot classes come off when they finish: their end state must not pin
// transforms that hover effects use.
document.addEventListener('animationend', e => {
  if (e.animationName === 'staggerIn') e.target.classList.remove('stagger-in');
  else if (e.animationName === 'wentLive') e.target.classList.remove('went-live');
  else if (e.animationName === 'aurora-bloom') e.target.classList.remove('bloom');
  else if (e.animationName === 'cta-bloom') e.target.classList.remove('cp-cta-bloom');
  else if (e.animationName === 'cap-flash') e.target.classList.remove('flash');
  else if (e.animationName === 'step-deny') e.target.classList.remove('deny');
});

// A link like /#integrations opens on that tab (the OBS dock uses it).
// /#about is System > About, where the tray's update link leads.
window.addEventListener('load', () => {
  const name = location.hash.slice(1);
  if (name === 'about') openUpdateInAbout();
  else if (name && document.querySelector(`.tab[data-tab="${CSS.escape(name)}"]`)) showTab(name);
  // Once only: a refresh later stays on whatever tab is open then.
  if (name) history.replaceState(null, '', location.pathname + location.search);
});
function showTab(name){
  // Guard unsaved Studio edits: switching away from the Overlay tab hides the
  // editor and abandons the in-progress doc, so confirm first.
  if (name !== 'overlay'){
    const m = $('ov-studio');
    if (m && !m.hidden && window.ICOverlay && Studio.isDirty && Studio.isDirty()){
      if (!confirm('You have unsaved overlay changes. Leave the Studio and discard them?')) return;
    }
  }
  // Direction of travel across the tab strip, so the pane slides in from the
  // side you moved toward. -1 (unseen) on first switch => no slide, just fade.
  const tabEls = $$('.tab');
  const newIdx = tabEls.findIndex(t => t.dataset.tab === name);
  const dir = (_tabIdx < 0 || newIdx < 0) ? 0 : Math.sign(newIdx - _tabIdx);
  if (newIdx >= 0) _tabIdx = newIdx;
  const incoming = document.querySelector(`.tab-pane[data-pane="${name}"]`);
  if (incoming) incoming.style.setProperty('--pane-dx', dir === 0 ? '0px' : (dir > 0 ? '20px' : '-20px'));

  // Capture the card height BEFORE the swap so we can tween it to the new
  // pane's height (panes vary; without this the card snaps taller/shorter).
  const card = document.querySelector('.pane-card');
  const reduceMo = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const cardH0 = (card && !reduceMo) ? card.offsetHeight : null;

  $$('.tab').forEach(t => t.classList.toggle('on', t.dataset.tab === name));
  $$('.tab-pane').forEach(p => p.hidden = p.dataset.pane !== name);
  moveTabInd();
  // A pending MIDI learn belongs to the System tab. Leaving it means the
  // user is not about to press a pad for us, and an armed learn swallows
  // the next press instead of running its action.
  if (name !== 'system'){
    if (hkRec) hkCancelRecording();
    if (midiLearnAction) midiCancelLearn();
  }
  if (name === 'obs') showObsTab();
  if (name === 'logs') pollLogs();
  // Opening Stats: where the counters are now, with no count up to them.
  if (name === 'stats' && lastState){ numState.clear(); paintStats(lastState); }
  if (window.Integrations){ if (name === 'integrations') Integrations.show(); else Integrations.hide(); }
  if (name === 'overlay') overlaySubShown(overlaySub());
  // The Studio modal lives on <body>, so close it when leaving the tab.
  else {
    const m = $('ov-studio'); if (m) m.hidden = true;
    if (window.Integrations) Integrations.stage(null);
  }
  // Sub-tab indicators only have a real width after the pane unhides,
  // so reposition any sub-tab indicator inside the freshly shown pane.
  const pane = document.querySelector(`.tab-pane[data-pane="${name}"]`);
  if (pane){ moveSubTabInd(pane); reserveCopyButtons(pane); }
  fitPaneCard();
  if (name !== 'system') staggerIn(pane);
  // Run the open sub-tab's hooks (VOD callout, update check), which a
  // sub-tab restored at load hasn't had yet.
  if (name === 'system'){
    const current = pane && pane.querySelector('.sub-tab.on');
    showSubTab('system', current ? current.dataset.subtab : 'general');
  }
  // Last, once the pane and its hooks have drawn what they can, so the
  // tween heads for the height the pane really has.
  if (card && cardH0 != null) morphCardHeight(card, cardH0);
}
function moveTabInd(){
  const cur = document.querySelector('.tab.on'); if (!cur) return;
  const ind = $('tab-ind');
  ind.style.left = cur.offsetLeft + 'px';
  ind.style.width = cur.offsetWidth + 'px';
  centerInStrip(cur);
}
// A strip of tabs too wide for a phone scrolls sideways: the picked tab
// slides to its middle, and an edge that hides more tabs fades out.
function centerInStrip(tab){
  const strip = tab.parentElement;
  if (strip.scrollWidth <= strip.clientWidth) return;
  const left = tab.offsetLeft + tab.offsetWidth / 2 - strip.clientWidth / 2;
  strip.scrollTo({left: Math.max(0, left), behavior: prefersReducedMotion() ? 'auto' : 'smooth'});
}
function fadeStripEdges(strip){
  const more = strip.scrollWidth - strip.clientWidth;
  strip.classList.toggle('more-l', more > 1 && strip.scrollLeft > 1);
  strip.classList.toggle('more-r', more > 1 && strip.scrollLeft < more - 1);
}
document.querySelectorAll('.tabs, .sub-tabs, .ic-header-right').forEach(strip => {
  strip.addEventListener('scroll', () => fadeStripEdges(strip), {passive:true});
  new ResizeObserver(() => fadeStripEdges(strip)).observe(strip);
});

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
// Clicking the tab that's already open does nothing: replaying its enter
// animation reads as a glitch, not feedback. (Code that re-shows the current
// tab on purpose calls showTab directly.)
$$('.tab').forEach(t => t.addEventListener('click', () => {
  if (!t.classList.contains('on')) showTab(t.dataset.tab);
}));
window.addEventListener('resize', moveTabInd);
// The pane card scrolls on its own. Cap it at the room left below its top
// edge, less what follows it (the footer and the page's bottom padding), so
// the card, the System save bar pinned to it and the footer all sit inside
// the window and the page has nothing to scroll. The shell is as tall as its
// content, so its bottom less the card's is that space, whatever the card's
// height. 360 px is the card's own floor.
function fitPaneCard(){
  const card = $('pane-card');
  if (!card || card.hidden) return;
  const box = card.getBoundingClientRect();
  const below = card.parentElement.getBoundingClientRect().bottom - box.bottom;
  const top = box.top + window.scrollY;
  card.style.setProperty('--pane-max', Math.max(360, innerHeight - top - below) + 'px');
}
window.addEventListener('resize', fitPaneCard);

