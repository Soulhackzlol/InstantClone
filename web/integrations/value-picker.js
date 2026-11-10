// ------------------------------------------------------------ value picker
// Values come to the field being written in, in both the simple editor and
// the builder: a few suggestions right under it, and every value in a
// searchable menu, opened by "More values…" or by typing {. Which values a
// field can use comes from valueContext. The strip is a data-persist node,
// so the form's redraws (morph) leave it where it is.
const SUGGESTED = 4;
const COMMON_LIVE = ['time', 'uptime', 'delay'];
const ValuePick = {
  field:null, ctx:null, strip:null, menu:null, mode:'', braceAt:-1, range:null, query:'', active:0, items:[],
  attach(field, ctx){
    if (field === this.field) return;
    this.detach();
    if (!ctx().vars.length) return;
    this.field = field;
    this.ctx = ctx;
    const strip = this.strip = document.createElement('div');
    strip.className = 'ig-vstrip';
    strip.setAttribute('data-persist', '');
    // A click here keeps the caret where it was in the field.
    strip.addEventListener('pointerdown', e => e.preventDefault());
    strip.addEventListener('click', e => {
      const b = e.target.closest('button');
      if (b && b.hasAttribute('data-more')) this.openMenu('search');
      else if (b) this.choose(b.dataset.pick);
    });
    // Under the whole row a field sits in, and under its sample line.
    let anchor = field.closest('.ig-tpl, .ig-bfield, .ig-cond-top') || field;
    if (anchor.nextElementSibling && anchor.nextElementSibling.classList.contains('ig-sample')) anchor = anchor.nextElementSibling;
    anchor.after(strip);
    this.renderStrip();
  },
  detach(){
    this.closeMenu();
    if (this.strip) this.strip.remove();
    this.field = this.ctx = this.strip = null;
  },
  values(){ return this.ctx ? this.ctx().vars : []; },
  // The latest earlier steps' answers first, then what started the run,
  // then the few always-there values most messages use. One already in the
  // field isn't offered again.
  suggested(){
    const text = this.field.value;
    const usable = this.values().filter(v => !v.name.includes('.json.') && !text.includes('{' + v.name));
    const steps = [];
    usable.filter(v => v.group === 'steps').forEach(v => { if (!steps.some(x => x.step === v.step)) steps.push(v); });
    return steps.reverse().slice(0, 2)
      .concat(usable.filter(v => v.group === 'trigger'), usable.filter(v => v.group === 'live' && COMMON_LIVE.includes(v.name)))
      .slice(0, SUGGESTED);
  },
  renderStrip(){
    if (!this.strip) return;
    const chips = this.suggested().map(v => `<button type="button" class="ig-vchip" data-pick="${esc(v.name)}" data-token="${esc(v.name)}" data-sample="${esc(v.sample)}">
      <b>${esc(valueLabel(v))}</b><span>${esc(shortSample(v.sample, 22))}</span></button>`).join('');
    const html = `<span class="ig-vstrip-h">Add</span>${chips}<button type="button" class="ig-vchip more" data-more>More values…</button>
      <span class="ig-vstrip-k">or type <code>{</code></span>`;
    if (this.strip._html !== html){ this.strip._html = html; this.strip.innerHTML = html; }
  },
  openMenu(mode){
    const f = this.field;
    this.mode = mode;
    this.range = [f.selectionStart, f.selectionEnd];
    this.query = '';
    this.active = 0;
    const menu = this.menu || this.buildMenu();
    const input = menu.querySelector('input');
    input.value = '';
    // Typing {: the field itself is the search.
    menu.classList.toggle('typed', mode === 'brace');
    this.renderMenu();
    menu.classList.add('on');
    this.place();
    if (mode === 'search') input.focus({preventScroll:true});
  },
  buildMenu(){
    const menu = this.menu = document.createElement('div');
    menu.className = 'ig-vmenu';
    menu.innerHTML = `<div class="ig-vmenu-find">${svg('search')}<input type="text" placeholder="Find a value" aria-label="Find a value" spellcheck="false" autocomplete="off"><span class="ig-vmenu-n"></span></div>
      <div class="ig-vmenu-list" role="listbox" aria-label="Values"></div>
      <div class="ig-vmenu-foot">↑↓ to pick · Enter to add · Esc to close</div>`;
    menu.addEventListener('pointerdown', e => { if (!e.target.closest('input')) e.preventDefault(); });
    menu.addEventListener('click', e => { const i = e.target.closest('[data-pick]'); if (i) this.choose(i.dataset.pick); });
    const input = menu.querySelector('input');
    input.addEventListener('input', () => { this.query = input.value; this.active = 0; this.renderMenu(); });
    input.addEventListener('keydown', e => this.menuKey(e));
    document.body.appendChild(menu);
    return menu;
  },
  closeMenu(){
    if (!this.mode) return;
    this.mode = '';
    this.menu.classList.remove('on');
  },
  words(){ return this.query.toLowerCase().split(/\s+/).filter(Boolean); },
  renderMenu(){
    const all = this.values(), words = this.words();
    const list = all.filter(v => {
      const hay = `${v.name} ${valueLabel(v)} ${valueHelp(v)}`.toLowerCase();
      return words.every(w => hay.includes(w));
    });
    this.items = list.map(v => v.name);
    this.active = Math.min(this.active, Math.max(0, list.length - 1));
    let html = '', group;
    list.forEach((v, n) => {
      const key = v.step || v.group;
      if (key !== group){ group = key; html += `<div class="ig-vmenu-g">${valueSource(v)}</div>`; }
      const help = valueHelp(v);
      html += `<button type="button" class="ig-vmenu-i${n === this.active ? ' on' : ''}" role="option" aria-selected="${n === this.active}" data-pick="${esc(v.name)}"${help ? ` title="${esc(help)}"` : ''}>
        <span><b>${markWords(valueLabel(v), words)}</b><small><code>{${markWords(v.name, words)}}</code>${help ? ' ' + esc(help) : ''}</small></span>
        <em>${esc(shortSample(v.sample, 26))}</em></button>`;
    });
    const menu = this.menu;
    menu.querySelector('.ig-vmenu-list').innerHTML = html || `<div class="ig-vmenu-none">No value matches "${esc(this.query)}".</div>`;
    menu.querySelector('.ig-vmenu-n').textContent = words.length ? `${list.length} of ${all.length}` : `${all.length}`;
    const on = menu.querySelector('.ig-vmenu-i.on');
    if (on) on.scrollIntoView({block:'nearest'});
  },
  // Under the strip, or above the field when there's no room below.
  place(){
    if (!this.mode) return;
    const menu = this.menu, f = this.field.getBoundingClientRect();
    const under = (this.strip && this.strip.isConnected ? this.strip : this.field).getBoundingClientRect();
    const w = menu.offsetWidth, h = menu.offsetHeight;
    menu.style.left = Math.max(8, Math.min(f.left, innerWidth - w - 8)) + 'px';
    menu.style.top = (under.bottom + 6 + h <= innerHeight - 8 ? under.bottom + 6 : Math.max(8, f.top - h - 6)) + 'px';
  },
  // Arrows, Enter and Esc, from the menu's search or the field typed into.
  menuKey(e){
    const n = this.items.length;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp'){
      e.preventDefault();
      if (n){ this.active = (this.active + (e.key === 'ArrowDown' ? 1 : -1) + n) % n; this.renderMenu(); }
    } else if ((e.key === 'Enter' || e.key === 'Tab') && n){
      e.preventDefault();
      this.choose(this.items[this.active]);
    } else if (e.key === 'Escape'){
      e.preventDefault();
      const back = this.mode === 'search';
      this.closeMenu();
      if (back) this.restoreCaret();
    }
  },
  restoreCaret(){
    this.field.focus({preventScroll:true});
    this.field.setSelectionRange(this.range[0], this.range[1]);
  },
  // Into the field: over the typed { and what follows it, or where the
  // caret was, a space apart from the word before it.
  choose(name){
    const f = this.field;
    if (!f || !f.isConnected){ this.detach(); return; }
    let text = tokenText(name, this.ctx().plain);
    if (this.mode === 'brace') f.setSelectionRange(this.braceAt, f.selectionStart);
    else {
      if (this.mode === 'search') this.restoreCaret();
      if (/[\p{L}\p{N}.,!?:;)]$/u.test(f.value.slice(0, f.selectionStart))) text = ' ' + text;
    }
    this.closeMenu();
    insertAtCursor(f, text);
  },
  // The field changed (`e`, its input event) or its caret moved.
  followField(e){
    const f = this.field;
    if (this.mode !== 'brace' && e && e.data === '{' && f.value[f.selectionStart - 1] === '{'){
      this.braceAt = f.selectionStart - 1;
      this.openMenu('brace');
    } else if (this.mode === 'brace'){
      const at = f.selectionStart, typed = f.value.slice(this.braceAt + 1, at);
      if (f.value[this.braceAt] !== '{' || at <= this.braceAt || /[\s{}]/.test(typed)) this.closeMenu();
      else if (typed !== this.query){ this.query = typed; this.active = 0; this.renderMenu(); this.place(); }
    }
    if (e) this.renderStrip();
  },
};
// The values a text field can take, as a function read at use, or null for
// a field that takes none: a step's own names and picks never do.
const TEXT_FIELD = ':not([readonly]):not([inputmode]):not([data-fixed]):not(.ig-dur):not([type="color"]):not([type="checkbox"])';
const NOT_TEXT = new Set(['save_as', 'name', 'block', 'id', 'device', 'path', 'method']);
function valueContext(el){
  if (!el.matches || !el.matches(`input${TEXT_FIELD}, textarea${TEXT_FIELD}`)) return null;
  if (Modal.kind === 'editor' && el.closest('.ig-live, .ig-live-plain')){
    return () => ({vars:varsFor(Editor.handler()), plain:false});
  }
  if (Modal.kind === 'builder' && el.closest('.ig-inspector') && Array.isArray(Builder.sel) && !NOT_TEXT.has(el.dataset.name)){
    return () => {
      const s = Builder.stepAt(Builder.sel);
      return {vars:varsFor(Builder.handler(), s), plain:!!s && s.type === 'if'};
    };
  }
  return null;
}
document.addEventListener('focusin', e => {
  if (ValuePick.menu && ValuePick.menu.contains(e.target)) return;
  const ctx = valueContext(e.target);
  if (ctx) ValuePick.attach(e.target, ctx);
});
// Gone once focus leaves the field and its menu (a redraw that moves the
// field gives focus straight back, so this waits a moment to look).
document.addEventListener('focusout', () => setTimeout(() => {
  const at = document.activeElement, p = ValuePick;
  if (p.field && at !== p.field && !(p.menu && p.menu.contains(at))) p.detach();
}));
document.addEventListener('input', e => { if (e.target === ValuePick.field) ValuePick.followField(e); });
document.addEventListener('selectionchange', () => {
  if (ValuePick.mode === 'brace' && document.activeElement === ValuePick.field) ValuePick.followField(null);
});
// Before the editor's own keys (and the modal's Esc) while typing {.
document.addEventListener('keydown', e => {
  if (ValuePick.mode === 'brace' && e.target === ValuePick.field) ValuePick.menuKey(e);
}, true);
window.addEventListener('resize', () => ValuePick.place());
window.addEventListener('scroll', () => ValuePick.place(), true);
// The name later steps reach a step's answer by, for the steps whose
// answer has several fields; '' for any other step.
function answerName(s){
  const base = s.type === 'http' ? 'response' : s.type === 'program' && s.params.wait === 'yes' ? 'program'
    : s.type === 'obs' && s.params.action === 'check' ? 'obs' : '';
  return base && ((s.params.save_as || '').trim() || base);
}
// Who made a group of values, in words.
function stepSource(s){
  if (s.type === 'obs' && s.params.action === 'check'){
    const names = listedNames(s.params.source);
    return 'Check OBS' + (!names.length ? '' : ' · ' + (names.length > 2 ? `${names[0]} +${names.length - 1}` : names.join(', ')));
  }
  return (KINDS[s.type] || {label:s.type}).label;
}
// The names an OBS step lists, one per line: one mute or show can take
// several sources. Mirrors obsws::listed_names.
function listedNames(text){
  const names = [];
  String(text || '').split('\n').map(x => x.trim()).forEach(x => { if (x && !names.includes(x)) names.push(x); });
  return names;
}
// What a check that reads several sources names each one's values by:
// {obs.mic_aux.muted}. Mirrors obsws::source_keys.
// "Daily gives!" as a name a template can say: daily_gives, at most `max`.
function snakeName(text, max){
  return text.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+/, '').slice(0, max).replace(/_+$/, '');
}
function sourceKeys(names){
  const keys = [];
  names.forEach(name => {
    const base = snakeName(name, 24) || 'source';
    let key = base;
    for (let n = 2; keys.includes(key); n++) key = base + n;
    keys.push(key);
  });
  return keys;
}
// Whether another step in `handler` keeps its answer under `step`'s name.
function answerClash(handler, step){
  const name = answerName(step);
  let clash = false;
  if (name) walk(handler.steps, s => { clash = clash || (s !== step && answerName(s) === name); });
  return clash;
}
// Give `step` an answer name no other step uses: obs, obs2, obs3.
function nameAnswer(handler, step){
  if (!answerClash(handler, step)) return;
  const base = answerName(step).replace(/\d+$/, '');
  for (let n = 2; ; n++){
    step.params.save_as = base + n;
    if (!answerClash(handler, step)) return;
  }
}
function highlightVars(text){
  return esc(text).replace(/\{[A-Za-z0-9_.]{1,64}(?:[|#?][^}]*)?\}/g, m => `<span class="v">${m}</span>`);
}
function fmtAgo(ms){
  if (!ms) return 'never';
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} days ago`;
}
function fmtClock(ms){
  return new Date(ms).toLocaleTimeString([], {hour:'2-digit', minute:'2-digit', second:'2-digit'});
}
function fmtMs(ms){
  ms = +ms || 0;
  if (!ms) return '';
  if (ms < 60000) return Math.round(ms / 1000) + ' s';
  return Math.round(ms / 60000) + ' min';
}
function plural(n, word){ return `${n} ${word}${n === 1 ? '' : 's'}`; }
// "10s", "2m", "1m 30s", "1h": how a duration field shows a length.
function fmtDur(ms){
  ms = Math.max(0, Math.round(+ms || 0));
  if (!ms) return '0s';
  if (ms % 1000) return (ms / 1000).toFixed(1).replace(/\.0$/, '') + 's';
  const total = ms / 1000, h = Math.floor(total / 3600), m = Math.floor(total / 60) % 60, sec = total % 60;
  return [h ? h + 'h' : '', m ? m + 'm' : '', sec ? sec + 's' : ''].filter(Boolean).join(' ');
}
// What a duration field holds, in ms: "10s", "2m", "1m 30s", "1.5m", "1h",
// or a bare number in `unit` ("s" or "m"). NaN when it isn't one.
function parseDur(text, unit){
  const t = String(text || '').trim().toLowerCase().replace(/,/g, '.');
  if (!t) return NaN;
  const per = {ms:1, s:1000, sec:1000, m:60000, min:60000, h:3600000};
  if (/^\d+(\.\d+)?$/.test(t)) return Math.round(+t * per[unit || 's']);
  let total = 0, rest = t;
  const part = /^(\d+(?:\.\d+)?)\s*(ms|sec|min|s|m|h)\s*/;
  while (rest){
    const m = part.exec(rest);
    if (!m) return NaN;
    total += +m[1] * per[m[2]];
    rest = rest.slice(m[0].length);
  }
  return Math.round(total);
}
// A text field for a length of time. `bind` names it for the editor;
// `unit` is what a bare number means. Shows the value as "2m".
function durInput(bind, ms, unit, extra){
  return `<input class="ic-input mono ig-dur" data-bind="${bind}" data-unit="${unit || 's'}" value="${esc(fmtDur(ms))}"
    spellcheck="false" autocomplete="off" placeholder="10s, 2m"${extra || ''}>`;
}
// Read a duration field: the ms, or null (marked invalid) when it isn't one.
function durValue(el, min, max){
  const ms = parseDur(el.value, el.dataset.unit);
  const ok = Number.isFinite(ms) && ms >= (min || 0) && ms <= (max == null ? Infinity : max);
  el.setAttribute('aria-invalid', ok ? 'false' : 'true');
  el.title = ok ? '' : `Like 10s, 2m or 1m 30s${min ? ', at least ' + fmtDur(min) : ''}${max != null ? ', at most ' + fmtDur(max) : ''}`;
  return ok ? ms : null;
}
function randomToken(){
  const b = new Uint8Array(16);
  crypto.getRandomValues(b);
  return Array.from(b, x => x.toString(16).padStart(2, '0')).join('');
}
// Stable ids for objects with none (steps, handlers), so a redraw knows a
// moved step is the same step and slides it instead of rebuilding it.
const UIDS = new WeakMap();
let lastUid = 0;
function uidOf(o){
  if (!UIDS.has(o)) UIDS.set(o, ++lastUid);
  return UIDS.get(o);
}

