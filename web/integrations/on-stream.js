// ---------------------------------------------------------------- on stream

// The Overlay tab's Alerts and Goals pages. That tab is where everything
// viewers see is managed, but what these two show is integrations data, so
// they are drawn here and share this file's editors, modal and polling.

// What a goal counts by itself (goals.rs `Counts`): how it reads and looks.
const COUNTS = {
  subs:    {label:'Subs',     icon:'star',   c:'#a78bfa', what:'Counts new subs, resubs and gifted subs'},
  follows: {label:'Follows',  icon:'heart',  c:'#5ac8fa', what:'Counts follows'},
  bits:    {label:'Bits',     icon:'gem',    c:'#fcd34d', what:'Counts the bits viewers cheer'},
  own:     {label:'Your own', icon:'target', c:'#86efac', what:'Moves only when a step or you change it'},
};
const TIMER_COLOR = '#fda4af';
// The longest a timer holds (goals.rs `MAX_TIMER_S`), in ms.
const MAX_TIMER_MS = 30 * 24 * 3600 * 1000;
// How many of each there can be (goals.rs `MAX_GOALS`, `MAX_TIMERS`).
const TRACKER_MAX = {goal:16, timer:8};
// What a new goal or timer can start from: [kind, name, what it is, fields].
const TRACKER_STARTERS = [
  ['goal', 'Sub goal', 'A bar that fills with every sub.', {counts:'subs', target:50}],
  ['goal', 'Follower goal', 'Counts follows toward a number.', {counts:'follows', target:100}],
  ['goal', 'Bit goal', 'Fills with the bits viewers cheer.', {counts:'bits', target:5000}],
  ['goal', 'Your own goal', 'Anything else: wins, deaths, a prize pot.', {counts:'own', target:10}],
  ['timer', 'Subathon', 'A countdown that subs and bits make longer.', {start_s:3600, per_sub_s:300, per_100_bits_s:60}],
  ['timer', 'Countdown', 'Starting soon, a break, a challenge.', {start_s:300}],
];

function tracking(){ return (S.data && S.data.tracking) || {goals:[], timers:[]}; }
function trackersOf(what){ return what === 'goal' ? tracking().goals : tracking().timers; }
function timerLeft(t){ return t.running ? Math.max(0, t.ends_at_ms - Date.now()) : t.left_ms; }
// How full a goal is, 0 to 1.
function goalShare(g){ return Math.min(1, g.value / (g.target || 1)); }
// A countdown as a clock: 4:05, or 1:02:09 from an hour up. Rounded up, so
// it reads 0:00 only once it is over (goals.rs `clock_text`).
function clockText(ms){
  const total = Math.ceil(ms / 1000), h = Math.floor(total / 3600), m = Math.floor(total / 60) % 60, sec = total % 60;
  return `${h ? h + ':' + String(m).padStart(2, '0') : m}:${String(sec).padStart(2, '0')}`;
}
function countText(n){ return Number(n || 0).toLocaleString('en-US'); }
function showsAlerts(i){ return allSteps(i).some(s => s.type === 'overlay'); }
// The overlay a goal or timer gets when it is first put on stream.
function trackerSlug(what, id){ return `${what}-${id}`; }
// Goals and timers as variables, each with where it stands now.
function trackerValues(){
  const t = tracking();
  return t.goals.flatMap(g => [{name:`goal.${g.id}`, sample:String(g.value)}, {name:`goal.${g.id}.target`, sample:String(g.target)}])
    .concat(t.timers.map(x => ({name:`timer.${x.id}`, sample:clockText(timerLeft(x))})));
}

const Stage = {
  // The saved overlays' slugs, to know which goals have their own.
  slugs:null,
  clockT:0,
  // The goal or timer being edited: {what, isNew, d, was, value}.
  form:null,

  // The Studio's list, which it fetches: one list for both.
  async loadSlugs(){
    const list = typeof Studio !== 'undefined' ? await Studio.loadSaved() : [];
    this.slugs = new Set(list.map(o => o.slug));
  },
  render(){
    const root = $(S.stage === 'alerts' ? 'ov-alerts' : 'ov-goals');
    if (!root || !S.data) return;
    if (S.stage === 'alerts') morph(root, this.sourceHtml());
    else {
      morph(root, this.goalsHtml());
      const frame = root.querySelector('.og-tframe');
      if (frame) stageShow(frame, OVERLAY_CANVAS, null);
      this.tick();
    }
    if (typeof fitPaneCard === 'function') fitPaneCard();
  },

  // ---- the pages' shared parts
  // The details panel's head: an icon in its colour, a name, what it is,
  // and anything after (a button).
  detailHeadHtml(icon, color, title, kind, after = ''){
    return `<div class="ovg-detail-head"><span class="ovg-ic" style="--tc:${color}">${svg(icon)}</span>
      <div class="ovg-detail-id"><div class="ovg-title">${esc(title)}</div><div class="ovg-kind">${esc(kind)}</div></div>${after}</div>`;
  },
  // Where OBS finds it: the path (it fits), and Copy URL for the address.
  urlBoxHtml(url, hint = ''){
    const path = url.replace(/^https?:\/\/[^/]+/, '');
    return `<div class="ovg-urlbox"><div class="ic-label">Browser source</div>
      <div class="copy-block"><code class="mono" title="${esc(url)}">${esc(path).replace('/overlay/', '/overlay/<wbr>')}</code>
        <button class="ic-btn ic-btn-ghost" data-act="copy" data-text="${esc(url)}">Copy URL</button></div>${hint}</div>`;
  },

  // ---- alerts
  // The Overlay tab's Alerts page is the Studio's gallery of alert designs
  // (index.html); this draws the bar above it: the one browser source in
  // OBS that plays every alert. Which design an alert plays is its step's
  // (the Design chip). The designs are kept here for both.
  designs:[], saved:[],
  async loadDesigns(){
    const list = typeof Studio !== 'undefined' ? await Studio.loadSaved() : [];
    this.saved = list;
    this.designs = list.filter(o => o.kind === 'alerts');
    await this.ensureDefault();
  },
  // Every design an alert can take: the ones saved, then the built-in ones
  // (the themes' cards) not saved yet, which are made when first taken.
  // A built-in one keeps its theme once saved, to be shown as ready-made.
  allDesigns(){
    const themes = window.ICOverlay ? window.ICOverlay.ALERT_THEMES : [];
    const saved = this.designs.map(d => ({slug:d.slug, name:d.name || d.slug, theme:themes.find(t => d.slug === 'alert-' + t) || ''}));
    const built = themes.map(t => ({slug:'alert-' + t, name:THEMES[t].name, theme:t, builtin:true}));
    return saved.concat(built.filter(b => !saved.some(d => d.slug === b.slug)));
  },
  designNamed(slug){ return this.allDesigns().find(d => d.slug === slug) || null; },
  // A built-in design is written to the overlays folder the first time it
  // is taken: from then on it is a file like any other, to edit.
  async ensureDesign(slug){
    // The folder as it is now: one saved since (and changed) is never
    // written over.
    await this.loadDesigns();
    const d = this.designNamed(slug);
    if (!d || !d.builtin) return true;
    const doc = window.ICOverlay.alertPreset(d.theme);
    return this.writeDesign(slug, doc);
  },
  async writeDesign(slug, doc){
    let r = null;
    try {
      const sent = await fetch('/overlays/' + encodeURIComponent(slug), {method:'POST', headers:{'Content-Type':'text/html'}, body:window.ICOverlay.bake(doc)});
      r = await sent.json();
    } catch(_){}
    if (!r || !r.ok){ toast((r && r.error) || 'Could not make that design', 'err', 6000); return false; }
    await this.loadDesigns();
    return true;
  },
  async setDefault(slug){
    const look = Object.assign({}, lookOf(), {design:slug});
    const r = await api('/integrations/alerts/look', look);
    if (!r || r.ok === false){ toast((r && r.error) || 'That didn\'t save', 'err', 6000); return false; }
    S.data.alerts = Object.assign({}, S.data.alerts, {look});
    return true;
  },
  // Alerts from before designs had the alerts page's look: the first visit
  // makes that look a design ("My alerts"), the default that every alert
  // without one of its own plays. A look never changed is the built-in
  // Island's.
  async ensureDefault(){
    if (!S.data || this.defaulting || !window.ICOverlay) return;
    const look = lookOf();
    if (look.design) return;
    this.defaulting = true;
    try {
      const kept = ['font', 'text', 'background', 'accent', 'opacity', 'radius', 'animation', 'picture', 'css', 'scale', 'position', 'align'];
      const changed = themeOf(look) !== 'island' || !!look.sound || kept.some(k => look[k] != null && look[k] !== ''
        && !(k === 'scale' && +look[k] === 100) && !(k === 'position' && look[k] === 'bottom') && !(k === 'align' && look[k] === 'center'));
      // Anyone with alerts keeps the look they had, in its theme's layers;
      // a new install starts from the ready-made Island.
      const mine = changed || this.alerts().length > 0;
      let slug = 'alert-island';
      if (mine){
        slug = 'my-alerts';
        for (let n = 2; this.saved.some(o => o.slug === slug); n++) slug = 'my-alerts-' + n;
        const was = {theme:themeOf(look), sound:look.sound};
        kept.forEach(k => { if (look[k] != null && look[k] !== '') was[k] = look[k]; });
        const doc = window.ICOverlay.alertLookDoc(was) || this.cardDoc(was);
        doc.name = 'My alerts';
        if (!(await this.writeDesign(slug, doc))) return;
      } else if (!(await this.ensureDesign(slug))) return;
      if (await this.setDefault(slug) && mine) toast('Your alerts now use a design called "My alerts". Edit it on the Overlay tab.', 'ok', 5000);
    } finally { this.defaulting = false; }
  },
  // A look with custom CSS, written for the old card's markup: that card,
  // as one Alert card layer where the page put it.
  cardDoc(look){
    const doc = window.ICOverlay.alertCardDoc(look.theme), card = doc.widgets[0];
    if (look.sound) doc.sound = look.sound;
    card.card = Object.assign({}, look);
    delete card.card.sound;
    const v = look.position === 'top' ? 'top' : look.position === 'middle' ? 'center' : 'bottom';
    const side = look.align === 'left' || look.align === 'right' ? look.align : '';
    card.anchor = v === 'center' ? side || 'center' : side ? v + '-' + side : v;
    card.offset = {x:side ? 36 : -560, y:v === 'center' ? -180 : 36};
    return doc;
  },
  alerts(){
    const out = [];
    S.data.integrations.filter(showsAlerts).forEach(i => (i.handlers || []).forEach((h, n) => {
      let step = null;
      walk(h.steps, s => { if (!step && s.type === 'overlay') step = s; });
      if (step) out.push({i, h, step, key:`${i.id}:${n}`});
    }));
    return out;
  },
  // The design an alert plays: its own, or the default when it has none or
  // its own is gone (as the alerts page does).
  designOf(x){ return this.designNamed(x.step.params.design || '') || this.designNamed(lookOf().design || ''); },
  // The alerts that play a design (`names`, what starts each), of how many;
  // whether it is the default (the one alerts added later get), every
  // alert's too; and the default's name.
  designUse(slug){
    if (!S.data) return null;
    const all = this.alerts(), mine = all.filter(x => (this.designOf(x) || {}).slug === slug);
    const isDefault = lookOf().design === slug;
    // `own`: alerts given another design of their own in Integrations.
    const own = all.filter(x => x.step.params.design && x.step.params.design !== slug).length;
    return {names:mine.map(x => triggerSentence(x.h.trigger)), total:all.length, isDefault, own,
      everywhere:isDefault && mine.length === all.length, defaultName:designLabel('')};
  },
  // A deleted design: the default, gone, is the ready-made Island again (its
  // alerts play the default already).
  async designGone(slug){
    await this.loadDesigns();
    if (lookOf().design === slug && await this.ensureDesign('alert-island')) await this.setDefault('alert-island');
  },
  // The one browser source that plays every alert: in OBS or not, and its
  // address to copy.
  sourceHtml(){
    const a = S.data.alerts || {}, url = a.url || `${location.origin}/alerts`;
    return alertsStatusHtml(true, `<span class="og-asource-url"><button class="ic-btn ic-btn-ghost ig-small" data-act="alert-test" title="A test alert on your alerts page, in OBS too">${svg('play', ' class="ig-btn-ic"')}Send a test</button>
      <code class="mono" title="${esc(url)}">${esc(url.replace(/^https?:\/\/[^/]+/, ''))}</code>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(url)}"
        title="One browser source plays every alert. Added by hand: the size of your canvas, with Control audio via OBS ticked so their sounds reach the stream">Copy URL</button></span>`);
  },
  // One design for every alert: the default (what alerts added later play)
  // becomes it, and the alerts' own designs are let go.
  async useEverywhere(slug){
    if (!(await this.ensureDesign(slug)) || !(await this.setDefault(slug))) return false;
    // Each integration's own designs go, kept if its save fails.
    const own = new Map();
    this.alerts().forEach(a => { if (a.step.params.design) own.set(a.i, (own.get(a.i) || []).concat(a.step)); });
    for (const [i, steps] of own){
      const was = steps.map(s => s.params.design);
      steps.forEach(s => { delete s.params.design; });
      const r = await api('/integrations/save', i);
      if (!r || r.ok === false){
        steps.forEach((s, n) => { s.params.design = was[n]; });
        toast((r && r.error) || 'That didn\'t save', 'err', 6000);
        return false;
      }
    }
    toast('Every alert plays it now, and alerts you add later too', 'ok');
    return true;
  },

  // ---- goals and timers
  // Laid out as the other pages are: every goal and timer on the left, each
  // still saying where it stands; the chosen one's own overlay, live, in the
  // middle; and on the right, what moves it and what puts it on stream.
  sel:null,
  // [what, item] for each one, goals first.
  trackers(){
    const t = tracking();
    return t.goals.map(g => ['goal', g]).concat(t.timers.map(x => ['timer', x]));
  },
  // What it is: its icon, colour, and what it counts or gains time from.
  trackerLook(what, x){
    if (what === 'goal'){
      const c = COUNTS[x.counts] || COUNTS.own;
      return {icon:c.icon, color:c.c, line:c.what + (x.each_stream ? ', from 0 each stream' : '')};
    }
    const adds = [[x.per_sub_s, 'a sub'], [x.per_100_bits_s, '100 bits'], [x.per_follow_s, 'a follow']].filter(([s]) => s > 0)
      .map(([s, from]) => `${from} +${fmtDur(s * 1000)}`);
    return {icon:'clock', color:TIMER_COLOR, line:adds.length ? capFirst(adds.join(' · ')) : 'From ' + fmtDur(x.start_s * 1000)};
  },
  // A timer's state: [status class, word].
  timerWord(x){
    const left = timerLeft(x);
    return left <= 0 ? ['s-wait', 'Over'] : x.running ? ['s-live', 'Running'] : ['', 'Paused'];
  },
  tileHtml(what, x, on){
    const look = this.trackerLook(what, x);
    const meta = what === 'goal'
      ? `<span class="ovg-tile-path ig-num">${countText(x.value)} of ${countText(x.target)}</span>
         <span class="og-tbar"><i style="transform:translateX(${(goalShare(x) * 100 - 100).toFixed(2)}%)"></i></span>`
      : `<span class="ovg-tile-path"><b class="og-clock ig-num" data-id="${esc(x.id)}">${clockText(timerLeft(x))}</b> · ${this.timerWord(x)[1]}</span>`;
    return `<button class="ovg-tile${on ? ' sel' : ''}" data-key="tile-${what}-${esc(x.id)}" data-act="tr-pick" data-what="${what}" data-id="${esc(x.id)}" style="--tc:${look.color}">
      <span class="ovg-tile-ic">${svg(look.icon)}</span><span class="ovg-tile-txt"><span class="ovg-tile-name" translate="no">${esc(x.name)}</span>${meta}</span></button>`;
  },
  // Its own overlay as viewers see it (it reads the real numbers), or what
  // putting it on stream will make.
  viewHtml(what, x){
    const slug = trackerSlug(what, x.id), live = this.onStream(what, x);
    const stage = live
      ? `<div class="ovg-stage" data-key="tv-${esc(slug)}"><iframe class="ig-stage-frame og-tframe" src="/overlay/${encodeURIComponent(slug)}.html" title="${esc(x.name)} on stream" tabindex="-1" aria-hidden="true"></iframe></div>`
      : `<div class="ovg-stage og-tnone" data-key="tv-none"><b>Not on stream yet</b>
          <span>Put it on stream to make its overlay: ${what === 'goal' ? 'its bar, name and numbers' : 'its name and clock'}, as layers you can restyle. It shows here as viewers will see it.</span></div>`;
    return `<section class="overlay-section ovg-view" data-key="tr-view">
      <div class="ovg-viewbar"><span class="ic-label">Preview</span>${live ? '<span class="ph-cap"><span class="ph-dot" style="background:var(--ok)"></span>Live: real numbers</span>' : ''}</div>
      <div class="ovg-fit" data-key="fit">${stage}</div></section>`;
  },
  onStream(what, x){ return !!this.slugs && this.slugs.has(trackerSlug(what, x.id)); },
  // A button that adds `n` (or takes it away), saying `label`; and Reset.
  nudgeHtml(n, label, aria, at){
    return `<button class="ic-btn ic-btn-ghost ig-small og-nudge" data-act="tr-change" data-do="add" data-n="${n}" ${at}
      aria-label="${aria}">${n < 0 ? '−' : '+'}${label}</button>`;
  },
  resetHtml(at){ return `<button class="ic-btn ic-btn-ghost ig-small og-push" data-act="tr-change" data-do="reset" ${at}>Reset</button>`; },
  goalMeterHtml(g, at){
    const done = g.value >= g.target, share = goalShare(g);
    const step = n => this.nudgeHtml(n, Math.abs(n), `${n < 0 ? 'Take' : 'Add'} ${Math.abs(n)} ${n < 0 ? 'from' : 'to'} ${esc(g.name)}`, at);
    return `<div class="og-meter${done ? ' done' : ''}">
      <div class="og-figure"><b class="og-value ig-num" data-tick>${countText(g.value)}</b><span class="og-of ig-num">of ${countText(g.target)}</span>
        <span class="og-pct ig-num">${done ? 'Reached' : Math.floor(share * 100) + '%'}</span></div>
      <div class="og-bar" role="progressbar" aria-label="${esc(g.name)}" aria-valuemin="0" aria-valuemax="${g.target}" aria-valuenow="${Math.min(g.value, g.target)}">
        <i style="transform:translateX(${(share * 100 - 100).toFixed(2)}%)"></i></div>
      <div class="og-row">${step(-1)}${step(1)}${g.counts === 'bits' ? step(100) : ''}${this.resetHtml(at)}</div></div>`;
  },
  timerMeterHtml(x, at){
    const over = timerLeft(x) <= 0, running = x.running && !over, word = this.timerWord(x);
    const minute = n => this.nudgeHtml(n * 60, '1 min', `${n < 0 ? 'Take a minute from' : 'Add a minute to'} ${esc(x.name)}`, at);
    return `<div class="og-meter">
      <div class="og-figure"><b class="og-value og-clock ig-num" data-id="${esc(x.id)}">${clockText(timerLeft(x))}</b>
        <span class="dcard-status ${word[0]} og-state"><span class="d-dot"></span><span class="dcard-status-l">${word[1]}</span></span></div>
      <div class="og-row">
        <button class="ic-btn ${running ? 'ic-btn-ghost' : 'ic-btn-primary'} ig-small" data-act="tr-change" data-do="${running ? 'pause' : 'start'}" ${at}>${svg(running ? 'pause' : 'play', ' class="ig-btn-ic"')}${running ? 'Pause' : over ? 'Start again' : 'Start'}</button>
        ${minute(-1)}${minute(1)}${this.resetHtml(at)}</div></div>`;
  },
  detailHtml(what, x){
    const look = this.trackerLook(what, x), at = `data-what="${what}" data-id="${esc(x.id)}"`;
    const slug = trackerSlug(what, x.id), url = `${location.origin}/overlay/${encodeURIComponent(slug)}.html`;
    const stream = this.onStream(what, x)
      ? `<div class="ovg-primary">
          <button class="ic-btn ic-btn-primary" data-act="tr-obs" data-slug="${esc(slug)}" data-name="${esc(x.name)}">${svg('plus', ' class="ig-btn-ic"')}Add to OBS</button>
          <button class="ic-btn ic-btn-ghost" data-act="tr-look" data-slug="${esc(slug)}">${svg('pencil', ' class="ig-btn-ic"')}Edit its look</button></div>
        ${this.urlBoxHtml(url)}`
      : `<div class="ovg-primary"><button class="ic-btn ic-btn-primary" data-act="tr-show" ${at}>${svg('screen', ' class="ig-btn-ic"')}Put it on stream</button></div>`;
    const hears = what === 'goal' ? x.counts !== 'own' : !!(x.per_sub_s || x.per_100_bits_s || x.per_follow_s);
    const deaf = hears && !S.data.twitch.main.login
      ? `<div class="ig-alerts-status" data-key="tr-twitch"><span class="ig-meter-dot off"></span>
          <span class="ig-alerts-line">Follows, subs and bits come from Twitch, and no Twitch account is connected: it won't move by itself yet.</span>
          <button class="ic-btn ic-btn-ghost ig-small" data-act="conn" data-tab="twitch">Connect Twitch</button></div>` : '';
    return `<aside class="overlay-section ovg-detail og-detail" data-key="tr-detail-${what}-${esc(x.id)}" style="--dc:${look.color}">
      ${this.detailHeadHtml(look.icon, look.color, x.name, look.line,
        `<button class="ic-btn-tiny og-push" data-act="tr-edit" ${at} aria-label="Edit ${esc(x.name)}" title="Edit">${svg('pencil')}</button>`)}
      ${deaf}
      ${what === 'goal' ? this.goalMeterHtml(x, at) : this.timerMeterHtml(x, at)}
      ${stream}</aside>`;
  },
  startersHtml(){
    return TRACKER_STARTERS.map(([what, name, note, fields], n) => {
      const look = this.trackerLook(what, fields);
      return `<button class="dest-starter" data-act="tr-new" data-n="${n}" style="--dc:${look.color}">
        <span class="dest-starter-icon">${svg(look.icon)}</span><span class="dest-starter-name">${name}</span><span class="dest-starter-note">${note}</span></button>`;
    }).join('');
  },
  goalsHtml(){
    const t = tracking();
    if (!t.goals.length && !t.timers.length){
      return `<div class="dest-empty" data-key="empty">
        <div class="dest-empty-title">What is the stream working toward?</div>
        <div class="dest-empty-sub">A goal fills up as viewers follow, sub or cheer. A timer counts down, and can gain time with every sub. Pick one to start from: you can change everything about it.</div>
        <div class="dest-empty-grid">${this.startersHtml()}</div></div>`;
    }
    const all = this.trackers();
    const [what, cur] = all.find(([w, x]) => `${w}:${x.id}` === this.sel) || all[0];
    this.sel = `${what}:${cur.id}`;
    const group = (kind, label) => {
      const list = all.filter(([w]) => w === kind);
      return list.length ? `<div class="ovg-group" data-key="g-${kind}">${label}</div>` + list.map(([w, x]) => this.tileHtml(w, x, x === cur)).join('') : '';
    };
    return `<div class="ovg-page" data-flip>
      <div class="ovg-top" data-key="tr-top">
        <section class="overlay-section ovg-shelf" data-key="tr-list">
          <div class="ovg-shelf-head"><span class="ic-label">Goals and timers</span><span class="ovg-count">${all.length}</span></div>
          <div class="ovg-list">${group('goal', 'Goals')}${group('timer', 'Timers')}</div></section>
        ${this.viewHtml(what, cur)}
        ${this.detailHtml(what, cur)}
      </div>
      <div class="muted og-note" data-key="note">Each one gets an overlay of its own when you put it on stream, made of layers you can restyle in the Studio. Any overlay can show them too (a <b>Goal bar</b>, a <b>Timer</b>, or <span class="mono">{{goal.name}}</span>), and integrations move them with a <b>Goal or timer</b> step.</div></div>`;
  },
  // Running clocks count down by themselves: one wake-up a second, just
  // after the second shown ends, and none while no clock runs.
  tick(){
    clearTimeout(this.clockT);
    if (S.stage !== 'goals' || document.hidden) return;
    let next = 0;
    document.querySelectorAll('#ov-goals .og-clock').forEach(el => {
      const t = tracking().timers.find(x => x.id === el.dataset.id);
      if (!t) return;
      const left = timerLeft(t), text = clockText(left);
      if (el.textContent !== text) el.textContent = text;
      if (t.running && left > 0) next = Math.min(next || Infinity, left % 1000 + 25);
    });
    if (next) this.clockT = setTimeout(() => this.tick(), next);
  },

  // ---- the editor
  // A short name for `name` that no other goal (or timer) has.
  freeId(what, name, mine){
    const base = snakeName(name, 28) || what;
    const taken = id => id !== mine && trackersOf(what).some(x => x.id === id);
    if (!taken(base)) return base;
    for (let n = 2; ; n++) if (!taken(`${base}_${n}`)) return `${base}_${n}`;
  },
  edit(what, id, starter){
    const found = trackersOf(what).find(x => x.id === id);
    const fresh = what === 'goal' ? {id:'', name:'', counts:'subs', target:50, each_stream:false}
      : {id:'', name:'', start_s:3600, per_sub_s:0, per_100_bits_s:0, per_follow_s:0, tiers:true, max_s:0};
    const d = found ? clone(found) : Object.assign(fresh, starter || {});
    if (!found) d.id = this.freeId(what, d.name);
    this.form = {what, isNew:!found, d, was:JSON.stringify(d), value:found && what === 'goal' ? found.value : 0};
    this.form.valueWas = this.form.value;
    Modal.open('tracker', 580);
    Modal.onClose = () => !this.dirty() || (Modal.guard(), false);
    this.renderForm();
    focusField('tr-name');
  },
  dirty(){
    const f = this.form;
    return !!f && (JSON.stringify(f.d) !== f.was || f.value !== f.valueWas);
  },
  goalFormHtml(d){
    const c = COUNTS[d.counts] || COUNTS.own;
    return `<div class="dff"><label>It counts</label>
        <div class="sub-tabs ig-seg og-seg" role="radiogroup" aria-label="What it counts">${Object.entries(COUNTS).map(([id, k]) =>
          `<button type="button" class="sub-tab${id === d.counts ? ' on' : ''}" role="radio" aria-checked="${id === d.counts}" data-act="tr-counts" data-v="${id}">${svg(k.icon)}<span>${k.label}</span></button>`).join('')}${SEG_IND}</div>
        <div class="muted">${c.what}.${d.counts === 'own' ? ' Add a "Goal or timer" step to any integration to move it.' : ''}</div></div>
      <div class="dfg">
        <div class="dff"><label>Target</label><input class="ic-input mono" data-bind="tr-target" value="${esc(d.target)}" inputmode="numeric" maxlength="10" autocomplete="off"></div>
        <div class="dff"><label>It stands at</label><input class="ic-input mono" data-bind="tr-value" value="${esc(this.form.value)}" inputmode="numeric" maxlength="10" autocomplete="off"></div>
      </div>
      <label class="crash-check"><input type="checkbox" data-bind="tr-each" ${d.each_stream ? 'checked' : ''}>
        <span><span class="crash-check-title">Start from 0 with every stream</span>
        <span class="muted">Off: it keeps counting from stream to stream until you reset it.</span></span></label>`;
  },
  timerFormHtml(d){
    const dur = (name, label, unit) => `<div class="dff"><label>${label}</label>${durInput('tr-dur', d[name] * 1000, unit, ` data-name="${name}"`)}</div>`;
    return `<div class="dfg">${dur('start_s', 'Starts at', 'm')}${dur('max_s', 'Never holds more than', 'm')}</div>
      <div class="muted">Write a length like 5m, 1h or 1h 30m. For no limit, leave the second one at 0s.</div>
      <div class="ic-label og-form-label">Viewers add time</div>
      <div class="dfg og-dfg3">${dur('per_sub_s', 'Each sub adds', 's')}${dur('per_100_bits_s', 'Every 100 bits add', 's')}${dur('per_follow_s', 'Each follow adds', 's')}</div>
      <div class="muted">0s adds nothing. A timer that ran out stays over until you start it again.</div>
      <label class="crash-check"><input type="checkbox" data-bind="tr-tiers" ${d.tiers ? 'checked' : ''}>
        <span><span class="crash-check-title">Bigger subs add more</span>
        <span class="muted">A tier 2 sub adds twice as much, a tier 3 five times.</span></span></label>`;
  },
  renderForm(){
    if (Modal.kind !== 'tracker') return;
    const f = this.form, d = f.d, goal = f.what === 'goal';
    const look = this.trackerLook(f.what, d);
    const title = f.isNew ? (goal ? 'New goal' : 'New timer') : (goal ? 'Goal' : 'Timer');
    const hint = goal ? 'A number your stream works toward. A Goal bar widget draws it.' : 'A clock that counts down. A Timer widget draws it.';
    Modal.body(`${modalHead(look.icon, look.color, title, hint)}
      <div class="dest-form-body ig-conn-body og-form">
        <div class="dff"><label>Name</label><input class="ic-input" data-bind="tr-name" value="${esc(d.name)}" placeholder="${goal ? 'Sub goal' : 'Subathon'}" maxlength="40" autocomplete="off">
          <div class="muted">Shown on stream. In messages and steps it is <code class="mono og-id">{${f.what}.${esc(d.id)}}</code>${f.isNew ? '' : ', which stays as it is'}.</div></div>
        ${goal ? this.goalFormHtml(d) : this.timerFormHtml(d)}
      </div>
      <div class="dest-form-foot">${f.isNew ? '' : '<button class="ic-btn ic-btn-ghost og-del" data-act="tr-del">Delete</button>'}<div class="dest-form-msg"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Cancel</button>
        <button class="ic-btn ic-btn-primary" data-act="save">${f.isNew ? (goal ? 'Add the goal' : 'Add the timer') : 'Save'}</button></div>`);
  },
  // Typing never redraws the form: a field keeps its caret, and marks
  // itself when what it holds can't be used.
  input(el){
    const f = this.form, b = el.dataset.bind;
    if (!f) return;
    const d = f.d, mark = ok => { el.setAttribute('aria-invalid', ok ? 'false' : 'true'); return ok; };
    if (b === 'tr-name'){
      d.name = el.value;
      // Until it is saved once, its short name follows what it is called.
      if (f.isNew) d.id = this.freeId(f.what, d.name);
      const id = q('.og-id');
      if (id) id.textContent = `{${f.what}.${d.id}}`;
    } else if (b === 'tr-target'){
      if (mark(/^[1-9]\d{0,8}$/.test(el.value.trim()))) d.target = +el.value;
    } else if (b === 'tr-value'){
      if (mark(/^\d{1,9}$/.test(el.value.trim()))) f.value = +el.value;
    } else if (b === 'tr-each') d.each_stream = el.checked;
    else if (b === 'tr-tiers') d.tiers = el.checked;
    else if (b === 'tr-dur'){
      const ms = durValue(el, el.dataset.name === 'start_s' ? 1000 : 0, MAX_TIMER_MS);
      if (ms != null) d[el.dataset.name] = Math.round(ms / 1000);
    }
  },
  async save(remove){
    const f = this.form, key = f.what === 'goal' ? 'goals' : 'timers';
    // A redraw keeps what a number field holds, not its mark: each is read again.
    if (!remove) Modal.form.querySelectorAll('[data-bind="tr-target"],[data-bind="tr-value"],[data-bind="tr-dur"]').forEach(el => this.input(el));
    const d = Object.assign({}, f.d, {name:f.d.name.trim()});
    if (!remove){
      if (!d.name){ toast('Give it a name', 'err'); focusField('tr-name'); return; }
      if (Modal.form.querySelector('[aria-invalid="true"]')){ toast('One of the numbers isn\'t right yet', 'err'); return; }
    }
    const next = clone(tracking()), at = next[key].findIndex(x => x.id === d.id);
    if (remove){ if (at >= 0) next[key].splice(at, 1); }
    else if (at >= 0) next[key][at] = d;
    else next[key].push(d);
    let r = await api('/integrations/goals', {goals:next.goals, timers:next.timers});
    if (!r || r.ok === false){ toast((r && r.error) || 'That didn\'t save', 'err', 6000); return; }
    if (!remove && f.what === 'goal' && f.value !== f.valueWas){
      r = await api('/integrations/goals/change', {what:'goal', id:d.id, action:'set', amount:f.value});
      if (!r || r.ok === false){ toast((r && r.error) || 'Saved, but its count didn\'t change', 'err', 6000); return; }
    }
    S.data.tracking = r.tracking;
    if (!remove) this.sel = `${f.what}:${d.id}`;
    // Its own overlay goes with it: there is nothing left for it to show.
    const slug = trackerSlug(f.what, d.id);
    if (remove && this.slugs && this.slugs.has(slug)){
      await api(`/overlays/${encodeURIComponent(slug)}/delete`, {});
      await this.loadSlugs();
    }
    this.form = null;
    Modal.close(true);
    this.render();
    toast(remove ? `Deleted ${d.name}` : f.isNew ? `${d.name} is ready: put it on stream when you like` : 'Saved', 'ok', 4500);
  },
  async change(body){
    const r = await api('/integrations/goals/change', body);
    if (!r || r.ok === false){ toast((r && r.error) || 'That didn\'t work', 'err'); return; }
    S.data.tracking = r.tracking;
    this.render();
  },
  // Make the overlay that shows this one goal or timer: a file like any
  // other in the gallery, with a link for OBS, restyled in the Studio.
  async putOnStream(what, id){
    const IC = window.ICOverlay, item = trackersOf(what).find(x => x.id === id);
    if (!IC || !item) return;
    // Made already (the list wasn't in yet): it keeps the look it was given.
    const slug = trackerSlug(what, id);
    await this.loadSlugs();
    if (this.slugs.has(slug)){ this.render(); return; }
    const page = IC.bake(IC.trackerDoc(what === 'goal' ? 'Goal' : 'Timer', id, item.name));
    let r = null;
    try {
      const sent = await fetch('/overlays/' + encodeURIComponent(slug), {method:'POST', headers:{'Content-Type':'text/html'}, body:page});
      r = await sent.json();
    } catch(_){}
    if (!r || !r.ok){ toast((r && r.error) || 'Could not make its overlay', 'err', 6000); return; }
    await this.loadSlugs();
    this.render();
    toast(`${item.name} has its overlay: add it to OBS, or copy its link`, 'ok', 5000);
  },
  async act(a, el){
    const what = el.dataset.what, id = el.dataset.id;
    switch (a){
      case 'tr-new': {
        const [kind, name, , fields] = TRACKER_STARTERS[+el.dataset.n];
        // Said before the form, not after it was filled in.
        if (trackersOf(kind).length >= TRACKER_MAX[kind]){
          toast(kind === 'timer' ? `That's the most timers there can be (${TRACKER_MAX[kind]}): delete one first`
            : `That's the most goals there can be (${TRACKER_MAX[kind]}): delete one first`, 'info', 5000);
          return true;
        }
        this.edit(kind, '', Object.assign({name}, fields));
        return true;
      }
      case 'tr-edit': this.edit(what, id); return true;
      case 'tr-pick': this.sel = `${what}:${id}`; this.render(); return true;
      case 'tr-counts': this.form.d.counts = el.dataset.v; this.renderForm(); return true;
      case 'tr-change':
        if (el.dataset.do === 'reset' && !armConfirm(el, what === 'goal' ? 'Back to 0?' : 'Start over?')) return true;
        await busy(el, () => this.change({what, id, action:el.dataset.do, amount:+el.dataset.n || 0}));
        return true;
      case 'tr-show': await busy(el, () => this.putOnStream(what, id)); return true;
      case 'tr-obs': await addOverlayToObs(el, el.dataset.slug, el.dataset.name); return true;
      // The Studio opens over this page, and goes back to it.
      case 'tr-look':
        if (typeof Studio !== 'undefined') Studio.openStudio(el.dataset.slug, 'goals');
        return true;
      case 'tr-del': {
        const has = this.slugs && this.slugs.has(trackerSlug(this.form.what, this.form.d.id));
        if (!armConfirm(el, has ? 'Delete it and its overlay?' : 'Delete it?')) return true;
        await busy(el, () => this.save(true));
        return true;
      }
      case 'save': await busy(el, () => this.save(false)); return true;
    }
    return false;
  },
};

function wireStage(){
  ['ov-alerts', 'ov-goals'].forEach(id => {
    const root = $(id);
    if (!root || root.dataset.wired) return;
    root.dataset.wired = '1';
    root.addEventListener('click', onClick);
    root.addEventListener('keydown', onActivateKey);
    root.addEventListener('input', e => { if (e.target.type === 'range') fillRange(e.target); });
  });
}

const OWNERS = {editor:Editor, builder:Builder, connections:Conn, import:Recipes, share:Recipes, catalog:Catalog, values:Values, tracker:Stage};

async function onClick(e){
  const el = e.target.closest('[data-act]');
  if (!el || el.disabled) return;
  const a = el.dataset.act;
  if (el.tagName === 'A') e.preventDefault();
  if (el.closest('.ig-menu') && a !== 'menu-delete' && a !== 'menu-folder') Menu.close();
  const owner = el.closest('.ig-modal') && OWNERS[Modal.kind];
  if (owner && await owner.act(a, el)) return;
  if (el.closest('.ov-stage') && await Stage.act(a, el)) return;
  switch (a){
    case 'modal-close': Modal.close(); break;
    case 'modal-back': Modal.goBack(); break;
    case 'guard-keep': Modal.hideGuard(); break;
    case 'discard': discardEdits(Modal.kind === 'builder' ? Builder : Editor); break;
    case 'guard-discard':
      if (Modal.pending === 'back' && Modal.back) Modal.leaveBack(); else Modal.close(true);
      break;
    case 'menu': Menu.toggle(el, el.dataset.id); break;
    case 'menu-delete':
      Menu.close();
      remove(el.dataset.id);
      break;
    case 'share-one': Recipes.openShare(el.dataset.id); break;
    case 'menu-folder': Menu.folders(el.dataset.id); break;
    case 'set-folder': moveToFolder([el.dataset.id], el.dataset.v); break;
    case 'fold': {
      const f = el.dataset.folder;
      if (!S.folded.delete(f)) S.folded.add(f);
      store.set('ig-folded', JSON.stringify([...S.folded]));
      render();
      break;
    }
    case 'folder-rename': renameNow(el.dataset.folder); break;
    case 'folder-new': {
      const name = newFolderName();
      S.pending.push(name);
      renameNow(name);
      break;
    }
    // Its integrations stay, out of any folder.
    case 'folder-remove': {
      const f = el.dataset.folder;
      const ids = S.data.integrations.filter(i => i.folder === f).map(i => i.id);
      S.pending = S.pending.filter(x => x !== f);
      if (S.folded.delete(f)) store.set('ig-folded', JSON.stringify([...S.folded]));
      if (!ids.length || await moveToFolder(ids, '')) toast(ids.length ? `Removed ${f}: what was in it stays` : `Removed ${f}`, 'ok');
      render();
      break;
    }
    case 'reveal': {
      const id = el.dataset.field, show = !S.revealed.has(id);
      if (show) S.revealed.add(id); else S.revealed.delete(id);
      if (OWNERS[Modal.kind] && OWNERS[Modal.kind].render) OWNERS[Modal.kind].render();
      const field = q(`.ig-secret-field[data-field="${CSS.escape(id)}"] .ig-secret`);
      decodeField(field, show);
      break;
    }
    case 'lock-why': toast(LOCAL_LOCK_TEXT, 'info', 8000); break;
    case 'last-run': {
      setLastRun(!S.lastRunOpen);
      const back = q('.ig-last-btn');
      if (!S.lastRunOpen && back) back.focus({preventScroll:true});
      break;
    }
    case 'act-row': S.actOpen = S.actOpen === el.dataset.k ? null : el.dataset.k; render(); break;
    case 'guard-save': {
      const save = q('.dest-form-foot [data-act="save"]');
      Modal.hideGuard();
      if (save) save.click();
      break;
    }
    case 'tpl-edit': {
      // A field showing what it will say: back to what was written.
      const field = el.parentElement && el.parentElement.querySelector('.ig-tpl-in');
      if (field){ field.focus(); const end = field.value.length; field.setSelectionRange(end, end); }
      break;
    }
    case 'fix-var': {
      const o = OWNERS[Modal.kind];
      if (!o || !o.handler) break;
      renameVar(o.handler(), el.dataset.from, el.dataset.to);
      o.changed();
      break;
    }
    case 'obs-retry': busy(el, () => ObsLive.tick()); break;
    case 'catalog': Catalog.open(); break;
    case 'build': Builder.open(null); break;
    case 'import': Recipes.openImport(); break;
    case 'values': Values.open(); break;
    case 'share': Recipes.openShare(); break;
    case 'conn': Conn.open(el.dataset.tab); break;
    case 'retry': busy(el, load); break;
    case 'edit': { const i = find(el.dataset.id); if (i) openEditor(i); break; }
    case 'toggle': toggle(el.dataset.id); break;
    case 'duplicate': busy(el, () => duplicate(el.dataset.id)); break;
    case 'peek': S.peek = S.peek === el.dataset.id ? null : el.dataset.id; render(); break;
    case 'filter': S.filter = el.dataset.f; render(); break;
    case 'view': S.view = el.dataset.v; store.set('ig-view', S.view); render(); break;
    case 'copy': copyText(el); break;
    case 'twitch-app': openTwitchAppSettings(); break;
    case 'rewards-load': Rewards.error = ''; busy(el, () => loadRewards()); break;
    case 'rewards-new': case 'rewards-edit': case 'rewards-cancel': {
      const row = el.closest('.ig-reward');
      Rewards.editing = a === 'rewards-new' ? 'new' : a === 'rewards-edit' ? row.dataset.id : null;
      if (OWNERS[Modal.kind] && OWNERS[Modal.kind].render) OWNERS[Modal.kind].render();
      const first = q('.ig-reward-form [data-r="title"]');
      if (first && a !== 'rewards-cancel') first.focus();
      break;
    }
    case 'rewards-save': {
      const form = el.closest('.ig-reward-form'), typed = name => form.querySelector(`[data-r="${name}"]`);
      const title = typed('title').value.trim(), cost = typed('cost').value.trim();
      if (!title || !/^[1-9]\d*$/.test(cost)){ toast('Give it a name and a cost in points', 'err'); break; }
      const change = {title, cost:+cost, prompt:typed('prompt').value.trim(), wants_input:typed('wants_input').checked};
      if (form.dataset.id) change.id = form.dataset.id;
      const was = form.dataset.was;
      busy(el, async () => {
        if (!await loadRewards(change)) return;
        Rewards.editing = null;
        // What this integration said about the old name follows the new one.
        if (was && was !== title && Modal.kind === 'builder'){ renameReward(Builder.d, was, title); Builder.changed(); }
        else if (OWNERS[Modal.kind] && OWNERS[Modal.kind].render) OWNERS[Modal.kind].render();
      });
      break;
    }
    case 'rewards-switch': busy(el, () => loadRewards({id:el.closest('.ig-reward').dataset.id, enabled:!!el.dataset.on})); break;
    case 'rewards-delete':
      if (!armConfirm(el, 'Delete?')) break;
      busy(el, () => loadRewards({id:el.closest('.ig-reward').dataset.id, delete:true}));
      break;
    case 'holders-load': busy(el, () => loadHolders()); break;
    case 'holders-set': {
      const box = el.closest('.ig-holders'), field = name => box.querySelector(`[data-h="${name}"]`).value.trim();
      if (!field('login') || !/^-?\d+$/.test(field('amount'))){ toast('Type a viewer and a whole number', 'err'); break; }
      busy(el, () => loadHolders({value:box.dataset.value, login:field('login'), amount:+field('amount')}));
      break;
    }
    case 'holders-clear':
      if (!armConfirm(el, 'Everyone back to 0. Sure?')) break;
      busy(el, () => loadHolders({value:el.closest('.ig-holders').dataset.value, clear:true}));
      break;
    case 'light-scan':
      busy(el, async () => {
        const r = await api('/integrations/lights/scan', {});
        if (!r || r.ok === false){ toast((r && r.error) || 'The scan didn\'t work', 'err'); return; }
        LightScan.devices = r.devices || [];
        const o = OWNERS[Modal.kind];
        if (o && o.render) o.render();
      });
      break;
    case 'light-try':
      busy(el, async () => {
        const r = await api('/integrations/lights/try', {device:el.dataset.ref, color:el.dataset.color || ''});
        if (r && r.ok !== false) toast(r.note ? 'Flashed it: ' + r.note : 'Flashed it', 'ok');
        else toast((r && r.error) || 'It didn\'t answer', 'err');
      });
      break;
    case 'alerts-add': addAlertsToObs(el); break;
    case 'alerts-open': openAlertsPage(); break;
    case 'media-del': deleteMedia(el); break;
    case 'alert-test':
      busy(el, async () => {
        const r = await api('/integrations/alerts/test', {});
        toast(r.ok ? 'A test card is on the alerts source now' : r.error || 'Could not show it', r.ok ? 'ok' : 'err');
      });
      break;
    // Shortcut and quiet hours: the same in both editors.
    case 'key-record': Keys.record(); break;
    case 'key-clear': Keys.trigger().hotkey = ''; OWNERS[Modal.kind].changed(); break;
    case 'pad-learn': if (!Keys.padTimer) Keys.learnPad(el); break;
    case 'pad-clear': Keys.trigger().midi = ''; OWNERS[Modal.kind].changed(); break;
    case 'btn-token-new': Keys.trigger().token = randomToken(); OWNERS[Modal.kind].changed(); break;
    case 'btn-token-clear': Keys.trigger().token = ''; OWNERS[Modal.kind].changed(); break;
    case 'set-quiet': {
      const o = OWNERS[Modal.kind];
      o.d.quiet = el.dataset.v === 'on' ? (o.d.quiet || {from:'23:00', to:'08:00'}) : null;
      o.changed();
      break;
    }
    case 'pack':
      busy(el, async () => {
        if (Modal.kind === 'catalog') Modal.close(true);
        const added = await addFrom({pack:el.dataset.pack}, false);
        // Nothing to post to yet: connect Discord now, and the pack's
        // drafts get it as soon as it's saved.
        const waiting = added.filter(i => !i.enabled && allSteps(i).some(st => st.type === 'discord')).map(i => i.id);
        if (waiting.length && !S.data.connections.discord.length){
          Conn.open('discord');
          Conn.editing = {id:'', name:''};
          Conn.render();
          Modal.after = () => finishDrafts(waiting);
        }
      });
      break;
  }
}

function onInput(e){
  const el = e.target;
  if (el.type === 'range') fillRange(el);
  if (el.id === 'ig-search'){
    S.search = el.value;
    render();
    return;
  }
  if (el.id === 'ig-cat-search'){ Catalog.search(el.value); return; }
  if (!el.dataset || !el.dataset.bind || !Modal.el || !Modal.el.contains(el)) return;
  if (Modal.kind === 'editor') Editor.input(el);
  else if (Modal.kind === 'builder') Builder.input(el);
  else if (Modal.kind === 'values') Values.input(el);
  else if (Modal.kind === 'tracker') Stage.input(el);
  // Pasted or dropped, not typed: typing would preview half a recipe.
  else if (Modal.kind === 'import' && el.dataset.bind === 'r-text' && /^insertFrom(Paste|Drop)$/.test(e.inputType || '')) Recipes.pasted(el);
}

