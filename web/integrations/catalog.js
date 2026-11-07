// ---------------------------------------------------------------- catalog

const CAT_NAV = [['rec', 'Recommended', 'star'], ['alerts', 'Alerts', 'bell'], ['chat', 'Chat', 'bubble'], ['auto', 'Automation', 'bolt'], ['packs', 'Packs', 'layers']];
const CAT_LABEL = {alerts:'Alerts', chat:'Chat', auto:'Automation', packs:'Packs'};
// Recommended takes turns between categories, so it never opens on a row
// of one kind.
function takeTurns(list){
  const queues = [...new Set(list.map(p => p.category))].map(id => list.filter(p => p.category === id));
  const out = [];
  while (queues.some(x => x.length)) queues.forEach(x => { if (x.length) out.push(x.shift()); });
  return out;
}
const Catalog = {
  cat:'rec', added:null, query:'',
  open(){
    this.cat = 'rec';
    this.added = null;
    this.query = '';
    this.reopen();
  },
  // Back from something opened here: same category and search as before.
  reopen(){
    Modal.open('catalog', 1040, 'min(86vh,820px)');
    this.render();
  },
  words(){ return this.query.toLowerCase().split(/\s+/).filter(Boolean); },
  // Every word typed is in its name, what it does, its category or what it
  // needs, as the page shows them or in English.
  matches(item, category){
    const words = this.words();
    const shown = [item.name, item.description, category];
    const hay = shown.concat(shown.map(text => tr(text)), item.needs || []).join(' ').toLowerCase();
    return words.every(w => hay.includes(w));
  },
  // Without a Twitch app in this copy, what needs Twitch isn't a good
  // first pick: it stays in its own category, out of Recommended.
  usable(p){
    return S.data.twitch.available || !(p.needs || []).includes('twitch');
  },
  // The presets a category shows, so its count always matches its tiles.
  // Searching, the first entry is every match.
  presetsIn(id){
    const all = S.data.catalog.presets;
    if (this.query) return all.filter(p => (id === 'rec' || p.category === id) && this.matches(p, CAT_LABEL[p.category]));
    if (id !== 'rec') return all.filter(p => p.category === id);
    return takeTurns(all.filter(p => p.recommended && this.usable(p)));
  },
  packsIn(){ return S.data.catalog.packs.filter(p => this.matches(p, CAT_LABEL.packs)); },
  count(id){
    if (id === 'packs') return this.packsIn().length;
    return this.presetsIn(id).length + (id === 'rec' && this.query ? this.packsIn().length : 0);
  },
  render(){
    const cat = S.data.catalog, searching = !!this.query;
    const nav = CAT_NAV.map(([id, label, icon]) => {
      const n = this.count(id);
      return `<button class="${this.cat === id ? 'on' : ''}${n ? '' : ' none'}" data-act="cat" data-cat="${id}" aria-current="${this.cat === id}">
        ${svg(icon)}<span>${id === 'rec' && searching ? 'All results' : label}</span><small data-tick>${n}</small></button>`;
    }).join('');
    const acts = `<div class="ig-cat-acts"><button class="ic-btn ic-btn-ghost ig-small" data-act="import">Import a recipe</button>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="build">${svg('plus')}Build your own</button></div>`;
    Modal.body(`${modalHead('plus', 'var(--accent)', 'Add an integration', 'Pick one: it opens ready to tweak.', acts)}
      <div class="ig-cat-find">${svg('search')}<input id="ig-cat-search" type="text" value="${esc(this.query)}" aria-label="Search integrations"
        placeholder="Search ${cat.presets.length} integrations and ${cat.packs.length} packs: alerts, Discord, chat, clips…" spellcheck="false" autocomplete="off">
        ${searching ? '<button class="ig-cat-clear" data-act="cat-clear">Clear</button>' : '<kbd title="Press / to search">/</kbd>'}</div>
      <div class="ig-cat"><nav class="ig-cat-nav" aria-label="Categories">${nav}</nav>
      <div class="ig-cat-grid">${this.gridHtml()}</div></div>`);
  },
  // A category's tiles, or while searching, every match grouped by category.
  gridHtml(){
    if (!this.query && this.cat === 'packs') return this.packsHtml(this.packsIn());
    if (!this.query) return (this.cat === 'rec' ? startStripHtml() : '') + this.presetsHtml(this.presetsIn(this.cat));
    const ids = this.cat === 'rec' ? ['alerts', 'chat', 'auto', 'packs'] : [this.cat];
    const groups = ids.map(id => [id, id === 'packs' ? this.packsIn() : this.presetsIn(id)]).filter(([, list]) => list.length);
    const n = groups.reduce((sum, [, list]) => sum + list.length, 0);
    if (!n){
      return `<div class="ig-cat-none" data-key="none"><b>Nothing matches "${esc(this.query)}"</b>
        <span>Try another word, or build it yourself from any trigger and any steps.</span>
        <button class="ic-btn ic-btn-ghost ig-small" data-act="build">${svg('plus')}Build your own</button></div>`;
    }
    const query = esc(this.query);
    const lead = n === 1 ? `1 result for "${query}"` : `${n} results for "${query}"`;
    return `<div class="ig-cat-lead" data-key="lead">${lead}</div>`
      + groups.map(([id, list]) => `<div class="ig-cat-group" data-key="g-${id}">${CAT_LABEL[id]}<small>${list.length}</small></div>`
        + (id === 'packs' ? this.packsHtml(list) : this.presetsHtml(list))).join('');
  },
  // Starter packs first, big, then the add-on packs for one job each.
  packsHtml(list){
    const starters = list.filter(p => p.who), addons = list.filter(p => !p.who);
    const head = (key, title, sub) => `<div class="ig-pk-head" data-key="${key}"><b>${title}</b><span>${sub}</span></div>`;
    return (starters.length ? head('pk-h-start', 'Starter packs', 'A whole setup for one kind of stream. Pick the closest: everything stays editable, and Ctrl+Z takes the pack back out.')
        + `<div class="ig-starters" data-key="pk-starters">${starters.map((p, n) => starterCardHtml(p, n, this.words())).join('')}</div>` : '')
      + (addons.length ? head('pk-h-addon', 'Add-on packs', 'A few integrations for one job.') + this.addonsHtml(addons) : '');
  },
  addonsHtml(list){
    const cat = S.data.catalog, words = this.words();
    return list.map((p, n) => `<div class="dcard ig-cat-pack" data-key="pk-${esc(p.id)}" style="--i:${n}">
      <div class="dcard-name">${markWords(tr(p.name), words)}</div>
      <div class="ig-cat-desc">${markWords(tr(p.description), words)}</div>
      <div class="ig-pack-list">${p.presets.map(id => `<span>${svg(PRESET_ICON[id] || 'steps')}${esc((cat.presets.find(x => x.id === id) || {}).name || id)}</span>`).join('')}</div>
      <div class="ig-cat-foot">${needsHtml(p.needs)}
      <button class="ic-btn ic-btn-primary ig-small" data-act="pack" data-pack="${esc(p.id)}" aria-label="Add the ${esc(p.name)} pack">Add pack</button></div>
    </div>`).join('');
  },
  presetsHtml(list){
    const have = id => S.data.integrations.filter(i => i.preset === id).length;
    const words = this.words();
    return list.map((p, n) => {
      const fake = fakeStep(p);
      const added = this.added === p.id;
      const blocked = !this.usable(p);
      const button = added
        ? `<button class="ic-btn ic-btn-ghost ig-small ig-added" data-act="add-preset" data-preset="${esc(p.id)}">${svg('check')}Added</button>`
        : `<button class="ic-btn ${have(p.id) || blocked ? 'ic-btn-ghost' : 'ig-cat-add'} ig-small" data-act="add-preset" data-preset="${esc(p.id)}"
            aria-label="${have(p.id) ? `Add another ${esc(p.name)}` : `Add ${esc(p.name)}`}">${have(p.id) ? 'Add another' : 'Add'}</button>`;
      return `<div class="dcard ig-cat-tile" style="--dc:${(KINDS[fake.step.type] || {}).c || 'var(--accent)'};--i:${n}" data-key="t-${esc(p.id)}">
        <div class="dcard-screen" aria-hidden="true">${previewHtml(fake.step, fake.handler, false)}</div>
        <div class="ig-cat-id"><span class="dcard-icon">${svg(PRESET_ICON[p.id] || 'steps')}</span>
          <div class="dcard-id"><div class="dcard-name">${markWords(tr(p.name), words)}</div>${this.query ? `<div class="ig-cat-kind">${CAT_LABEL[p.category] || ''}</div>` : ''}</div></div>
        <div class="ig-cat-desc">${markWords(tr(p.description), words)}</div>
        <div class="ig-cat-foot">${needsHtml(p.needs)}${button}</div>
      </div>`;
    }).join('');
  },
  search(text){
    if (text === this.query) return;
    this.query = text;
    this.render();
    const grid = q('.ig-cat-grid');
    if (grid) grid.scrollTop = 0;
  },
  flashAdded(id){
    this.added = id;
    this.render();
    setTimeout(() => {
      if (this.added !== id) return;
      this.added = null;
      if (Modal.kind === 'catalog') this.render();
    }, 1600);
  },
  act(a, el){
    switch (a){
      case 'cat': {
        this.cat = el.dataset.cat;
        this.render();
        const grid = q('.ig-cat-grid');
        if (grid) grid.scrollTop = 0;
        return true;
      }
      case 'cat-clear': {
        this.search('');
        const input = $('ig-cat-search');
        if (input){ input.value = ''; input.focus(); }
        return true;
      }
      case 'add-preset': busy(el, () => addFrom({preset:el.dataset.preset}, true)); return true;
    }
    return false;
  },
};

// A catalog entry's preview: the app's own sample of what it sends, or,
// without one, its sample line.
// "!chapter Boss fight" in a catalog entry's example: its previews read
// "Boss fight", not the generic sample.
function sampleArgsFrom(handler, preset){
  const ask = (preset && preset.preview && preset.preview.ask) || '';
  const t = handler.trigger || {};
  if (t.type !== 'chat_command' || !ask.startsWith(t.command + ' ')) return;
  SAMPLE_ARGS.set(handler, ask.slice(t.command.length + 1));
}
function fakeStep(p){
  if (p.sample){
    const handler = clone(p.sample);
    sampleArgsFrom(handler, p);
    walk(handler.steps, sayStep);
    const step = primaryStep(handler);
    if (step) return {step, handler};
  }
  const pv = p.preview || {};
  const type = {discord:'discord', chat:'chat', phone:'phone', marker:'marker', json:'http', file:'file'}[pv.kind] || 'chat';
  const param = (KINDS[type] || {}).text || 'text';
  const step = {type, params:{[param]: sayExactly(pv.text || ''), path:'crashes.txt', title:tr('OBS crashed'),
    body: type === 'http' ? '{"event":"hold_opened","delay_ms":30000}' : ''}, then:[], else:[]};
  const handler = {trigger: pv.ask ? {type:'chat_command', command:pv.ask} : {type:'event', event:'hold_opened'}, steps:[step]};
  return {step, handler};
}
// What a catalog entry or pack needs, each as a pill: green when it's
// there, amber when it still has to be connected.
// A starter pack's card: who it suits, what's inside (ticked when you have
// it already), what it needs, and a button that adds only what's missing.
const STARTER_TINT = {starter_first:'oklch(0.75 0.15 350)', starter_competitive:'var(--accent)',
  starter_subathon:'var(--warn)', starter_community:'oklch(0.68 0.16 275)'};
function starterCardHtml(p, n, words){
  const cat = S.data.catalog, have = id => S.data.integrations.some(i => i.preset === id);
  const missing = p.presets.filter(id => !have(id)).length;
  const rows = p.presets.map(id => {
    const preset = cat.presets.find(x => x.id === id) || {}, got = have(id);
    return `<li class="${got ? 'got' : ''}"><span class="ig-st-ic">${svg(PRESET_ICON[id] || 'steps')}</span>
      <span class="ig-st-name">${esc(preset.name || id)}</span>${got ? `<span class="ig-st-got">${svg('check', ' stroke-width="3"')}Added</span>` : ''}</li>`;
  }).join('');
  const label = !missing ? 'All added' : missing === p.presets.length ? `Add all ${missing}` : missing === 1 ? 'Add the 1 missing' : `Add the ${missing} missing`;
  return `<article class="ig-starter" data-key="st-${esc(p.id)}" style="--i:${n};--pk:${STARTER_TINT[p.id] || 'var(--accent)'}">
    <div class="ig-st-top"><span class="ig-st-badge">${svg(p.icon || 'layers')}</span>
      <div class="ig-st-id"><div class="ig-st-title">${markWords(tr(p.name), words || [])}</div><div class="ig-st-who">${esc(p.who)}</div></div></div>
    <p class="ig-st-desc">${markWords(tr(p.description), words || [])}</p>
    <ul class="ig-st-list" aria-label="What it adds">${rows}</ul>
    <div class="ig-st-foot">${needsHtml(p.needs)}
      <button class="ic-btn ${missing ? 'ic-btn-primary' : 'ic-btn-ghost'} ig-small ig-st-add" data-act="pack" data-pack="${esc(p.id)}"${missing ? '' : ' disabled'}
        aria-label="${esc(label)}: ${esc(p.name)}">${missing ? svg('plus', ' class="ig-btn-ic"') : ''}${label}</button></div>
  </article>`;
}
// While there are only a few integrations, Recommended opens on the
// starter packs: one click to a setup made for your kind of stream.
const START_STRIP_UNDER = 3;
function startStripHtml(){
  if (S.data.integrations.length >= START_STRIP_UNDER) return '';
  const starters = S.data.catalog.packs.filter(p => p.who);
  if (!starters.length) return '';
  return `<div class="ig-start-strip" data-key="start-strip"><div class="ig-start-txt"><b>New here? Start from a pack</b>
      <span>A few integrations at once, made for your kind of stream.</span></div>
    <div class="ig-start-chips">${starters.map(p => `<button class="ig-start-chip" style="--pk:${STARTER_TINT[p.id] || 'var(--accent)'}" data-act="cat" data-cat="packs">
      ${svg(p.icon || 'layers')}<span>${esc(p.name)}</span></button>`).join('')}</div></div>`;
}

function needsHtml(needs){
  const c = S.data.connections, t = S.data.twitch;
  const pills = (needs || []).map(n => {
    switch (n){
      case 'discord': return c.discord.length ? ['ok', 'Discord'] : ['miss', 'Discord channel', 'You add it right after adding this'];
      case 'twitch': return t.main.login ? ['ok', 'Twitch'] : t.available ? ['miss', 'Twitch', 'Connect it in Connections']
        : ['miss', 'Twitch (set up first)', 'This copy has no Twitch login app yet: System › Twitch login app'];
      case 'phone': return c.phone.topic ? ['ok', 'Phone'] : ['miss', 'Phone', 'Connect it in Connections'];
      case 'web': return ['', 'An address', 'You paste the address it sends to'];
      case 'light': return ['', 'A light on your network', 'You pick it after adding: WiZ, LIFX, Govee, Yeelight, Kasa, Shelly or WLED'];
      case 'file': return ['', 'A file', 'You pick the file it writes'];
      case 'dock': return ['', 'The OBS dock', "Switch on Events in your dock's layout to see them"];
      case 'obs': {
        const o = S.data.obs || {};
        return o.state === 'connected' ? ['ok', 'OBS WebSocket']
          : o.state === 'error' ? ['miss', 'OBS WebSocket', o.problem || 'Switch it on in OBS: Tools › WebSocket Server Settings']
          : ['', 'OBS WebSocket', 'Switch it on in OBS: Tools › WebSocket Server Settings'];
      }
    }
    return null;
  }).filter(Boolean);
  if (!pills.length) return '';
  return `<div class="ig-cat-needs"><span class="muted">Needs</span><span class="ig-cat-needs-list">${pills.map(([cls, label, tip]) =>
    `<span class="ig-need ${cls}"${tip ? ` title="${esc(tip)}"` : ''}>${cls === 'ok' ? svg('check', ' stroke-width="3"') : cls === 'miss' ? svg('warn') : ''}<span class="sr-only">${cls === 'ok' ? 'connected: ' : cls === 'miss' ? 'not connected yet: ' : ''}</span>${esc(label)}</span>`).join('')}</span></div>`;
}

