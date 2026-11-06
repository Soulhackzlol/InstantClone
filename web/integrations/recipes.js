// ---------------------------------------------------------------- recipes

// One integration of a recipe being imported: its triggers and who can set
// them off, then everything it reaches, the riskiest first.
function recipeItemHtml(it, n){
  // "OBS crashes, the hold runs out": names after the first read on.
  const who = (it.triggers || []).map((t, i) => esc(i && /^[A-Z][a-z]/.test(t.what) ? t.what[0].toLowerCase() + t.what.slice(1) : t.what)
    + (t.who ? ` <span class="ig-rwho">(${esc(t.who)})</span>` : '')).join(', ');
  const effects = it.effects || [];
  const level = effects.some(e => e.level === 'danger') ? 'danger' : effects.some(e => e.level === 'caution') ? 'caution' : 'safe';
  const flags = effects.map((e, i) =>
    `<span class="ig-rflag ${esc(e.level)}" style="--i:${i}">${e.level === 'info' ? '' : svg('warn')}${esc(e.text)}</span>`).join('');
  const handlers = (it.integration && it.integration.handlers) || [];
  let count = 0;
  handlers.forEach(h => walk(h.steps, () => { count++; }));
  const steps = handlers.map(h =>
    `<div class="ig-rhandler"><div class="ig-rtrig">${svg('bolt')}<span>${esc(triggerSentence(h.trigger))}${h.enabled === false ? ' <span class="muted">(switched off)</span>' : ''}</span></div>
      <ol class="ig-rsteps">${recipeStepsHtml(h.steps)}</ol></div>`).join('');
  const json = it.integration ? JSON.stringify(it.integration, null, 2) : '';
  const raw = `<div class="ig-rraw-wrap"><div class="ig-rraw-head"><span>What gets added, exactly</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(json)}">${svg('copy', ' class="ig-btn-ic"')}Copy</button></div>
    <pre class="ig-rraw">${jsonColored(json)}</pre></div>`;
  return `<div class="ig-ritem ${level}" style="--i:${n}">
    <b class="ig-rname" translate="no">${esc(it.name)}</b>
    <div class="ig-rwhen">When ${who || 'nothing'}</div>
    ${flags ? `<div class="ig-rflags">${flags}</div>` : ''}
    ${handlers.length ? `<div class="ig-rfolds">${recipeFold(`steps-${n}`, 'steps', `See every step <span class="ig-rcount">${count}</span>`, steps)}
      ${json ? recipeFold(`raw-${n}`, 'code', 'Raw settings', raw) : ''}</div>` : ''}</div>`;
}
// A row that opens smoothly onto `body`. Which rows are open survives a
// redraw of the import window (see Recipes.folds).
function recipeFold(key, icon, label, body){
  return `<details class="ig-rfold" data-fold="${esc(key)}"${Recipes.folds.has(key) ? ' open' : ''}>
    <summary>${svg(icon, ' class="ig-rfold-ic"')}<span class="ig-rfold-label">${label}</span>${svg('chev', ' class="ig-rchev"')}</summary>
    <div class="ig-rfold-body">${body}</div></details>`;
}
// JSON text with keys, strings, numbers and literals colored, every piece
// escaped on its own.
function jsonColored(text){
  const token = /("(?:\\.|[^"\\])*")(\s*:)?|\b(true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/g;
  let out = '', last = 0, m;
  while ((m = token.exec(text))){
    out += esc(text.slice(last, m.index));
    const cls = m[1] ? (m[2] ? 'jk' : 'js') : m[3] ? (m[3] === 'null' ? 'jn' : 'jb') : 'jd';
    out += `<span class="${cls}">${esc(m[1] || m[0])}</span>${m[2] ? esc(m[2]) : ''}`;
    last = token.lastIndex;
  }
  return out + esc(text.slice(last));
}
// Recipes never carry where a step reaches: shared and imported, they lose
// the program and its arguments, the file, the web address and the OBS
// scene or source (recipe::strip_private). Say so where that step is, so
// nobody wonders which program it would run.
const RECIPE_FILLS = {program:[['path'], 'the program and its arguments'], file:[['path'], 'the file'],
  http:[['url'], 'the address'], obs:[['scene', 'source'], 'the scene or source'], light:[['device'], 'the light']};
function recipeFillNote(s){
  const fill = RECIPE_FILLS[s.type], p = s.params || {};
  // Recording, the replay buffer, screenshots and hotkeys pick no name.
  if (s.type === 'obs' && ['', 'hotkey'].includes((OBS_ACTIONS[p.action] || [])[1] || '')) return '';
  if (!fill || fill[0].some(name => p[name])) return '';
  return ` <span class="ig-rnote">· a recipe can't choose ${fill[1]}: ${s.type === 'program' ? 'you pick them after adding' : 'you pick it after adding'}</span>`;
}
// The steps of a recipe being imported, in the builder's words, checks
// with their branches nested. What runs on the PC is tinted red; what
// changes the delay, OBS or calls a web address, amber.
function recipeStepsHtml(steps){
  if (!steps || !steps.length) return '<li class="ig-rstep ig-rnone">Nothing</li>';
  return steps.map((s, i) => {
    const k = KINDS[s.type] || {};
    const risk = LOCAL_KINDS.includes(s.type) ? ' danger' : ['delay_action', 'obs', 'http'].includes(s.type) ? ' caution' : '';
    const branches = s.type !== 'if' ? '' : `<ol class="ig-rsteps">${recipeStepsHtml(s.then)}</ol>`
      + ((s.else || []).length ? `<div class="ig-relse">Otherwise</div><ol class="ig-rsteps">${recipeStepsHtml(s.else)}</ol>` : '');
    return `<li class="ig-rstep${risk}" style="--i:${i}"><div class="ig-rline"><span class="ig-rtag" style="--c:${k.c || 'var(--fg-3)'}">${esc(k.tag || s.type)}</span>
      <span class="ig-step-text">${stepSentence(s)}${recipeFillNote(s)}${s.enabled === false ? ' <span class="muted">(switched off)</span>' : ''}</span></div>${branches}</li>`;
  }).join('');
}
// Said once above the Add button when anything in the recipe is risky.
function recipeWarningHtml(p){
  const danger = p.items.some(it => (it.effects || []).some(e => e.level === 'danger'));
  if (p.local_effects && localLocked()) return warnHtml(esc(LOCAL_LOCK_TEXT));
  if (!danger) return '';
  return warnHtml('Read the red lines before adding it. Recipes arrive switched off: nothing runs until you open one and switch it on.');
}

// Your values: things set once ({my.name}) that any message or step reads,
// so a coin's name or a game's odds change in one place; and, on its own
// tab, what viewers hold. Values are listed under the integration that
// reads them. Each row shows what the value is for and one field to change
// it; its name and label hide behind the pencil, since renaming is rare and
// changing the value is the point.
// Where a value added in this window sits until it is saved. "+" can't be
// an integration's id.
const NEW_VALUE_HOME = {id:'+', title:'New', others:0};
const VALUES_SHOWN_AT_ONCE = 8;
const Values = {
  list:[], tab:'values', find:'', more:null, saved:'', unfolded:new Set(),
  open(){
    // A row's kind and home are read once, here: decided again on every
    // key, a row would jump to another group, or swap its field, under the
    // typing.
    this.list = clone(S.data.values || []).map(v => ({...v, kind:this.kindOf(v), home:this.homeOf(v.name)}));
    this.saved = JSON.stringify(this.clean());
    // A few values show at once. Many open as a list of their integrations,
    // each one click away: thirty rows in a row is a wall to scroll.
    this.unfolded = new Set(this.list.length <= VALUES_SHOWN_AT_ONCE ? this.list.map(v => v.home.id) : []);
    this.tab = 'values';
    this.find = '';
    this.more = null;
    Modal.open('values', 860);
    Modal.onClose = () => !this.dirty() || (Modal.guard(), false);
    this.render();
    // Who holds what, without being asked: this is where it is managed.
    loadHolders();
  },
  // What Save would send: rows left wholly empty were never meant to be one.
  clean(){
    return this.list.filter(v => v.name || v.label || v.value).map(v => ({name:v.name.trim(), label:v.label.trim(), value:v.value}));
  },
  dirty(){ return JSON.stringify(this.clean()) !== this.saved; },
  // How a value is best edited, read off what it holds and what it is for.
  kindOf(v){
    if (/\n/.test(v.value) || / \| /.test(v.value)) return 'list';
    // A sentence, like what a command answers: room to read it, but no list.
    if (v.value.length > 60) return 'long';
    if (!/^-?\d+(\.\d+)?$/.test(v.value.trim())) return 'text';
    return /chance|percent|%/i.test(v.name + ' ' + v.label) ? 'chance' : 'number';
  },
  // The integrations that read {my.<name>} somewhere.
  readers(name){
    if (!name) return [];
    const reads = new RegExp(`[{$]my\\.${name}(?![A-Za-z0-9_])`);
    return S.data.integrations.filter(i => reads.test(JSON.stringify(i)));
  },
  usedIn(name){ return this.readers(name).length; },
  // Where a value is listed: under the first integration that reads it,
  // which is what a streamer looks for ("the coins game"), not under what
  // sort of text it holds. `others`: how many more read it too.
  homeOf(name){
    const [first, ...rest] = this.readers(name);
    return first ? {id:first.id, title:first.name, others:rest.length} : {id:'', title:'Not used yet', others:0};
  },
  editorHtml(v, n){
    const at = `data-n="${n}"`;
    switch (v.kind){
      case 'long': return `<textarea class="ic-input" data-bind="v-value" ${at} rows="3" aria-label="${esc(v.label || v.name)}">${esc(v.value)}</textarea>`;
      case 'chance': return `<span class="ig-val-chance"><input type="range" min="0" max="100" step="0.5" value="${esc(v.value)}" data-bind="v-value" ${at} aria-label="${esc(v.label || v.name)}, as a slider">
        <span class="ig-val-unit"><input class="ic-input mono" data-bind="v-value" ${at} value="${esc(v.value)}" inputmode="decimal" aria-label="${esc(v.label || v.name)}"><span class="muted">%</span></span></span>`;
      case 'number': return `<input class="ic-input mono ig-val-num" data-bind="v-value" ${at} value="${esc(v.value)}" inputmode="decimal" aria-label="${esc(v.label || v.name)}">`;
      case 'list': {
        const lines = v.value.split('\n').filter(x => x.trim()).length;
        return `<textarea class="ic-input" data-bind="v-value" ${at} rows="${Math.min(8, Math.max(3, v.value.split('\n').length))}" aria-label="${esc(v.label || v.name)}">${esc(v.value)}</textarea>
          <div class="muted">${/\n/.test(v.value) ? `${plural(lines, 'line')}: one is picked each time.` : 'One per line, or separated by |.'}</div>`;
      }
    }
    return `<input class="ic-input" data-bind="v-value" ${at} value="${esc(v.value)}" placeholder="Its value" aria-label="${esc(v.label || v.name)}">`;
  },
  rowHtml(v, n){
    const wide = ['list', 'long'].includes(v.kind), open = this.more === n, used = this.usedIn(v.name.trim());
    const meta = !v.name ? 'Give it a name to use it' : `<code class="mono">{my.${esc(v.name)}}</code>${v.home.others ? ' · also in ' + plural(v.home.others, 'other integration') : ''}`;
    return `<div class="ig-val${wide ? ' wide' : ''}${open ? ' open' : ''}" data-key="val-${n}">
      <div class="ig-val-main"><div class="ig-val-what"><b>${esc(v.label || v.name || 'New value')}</b><span class="ig-val-meta">${meta}</span></div>
        <div class="ig-val-edit">${this.editorHtml(v, n)}</div>
        <button type="button" class="ic-btn-tiny${open ? ' on' : ''}" data-act="v-more" data-n="${n}" aria-expanded="${open}" aria-label="Name and label of ${esc(v.label || v.name || 'this value')}" title="Name and label">${svg('pencil')}</button></div>
      ${open ? `<div class="ig-val-more"><div class="dff"><label>What it is for</label><input class="ic-input" data-bind="v-label" data-n="${n}" value="${esc(v.label)}" placeholder="!daily gives"></div>
        <div class="dff"><label>Name in steps</label><span class="ig-val-name mono">{my.<input class="ic-input mono" data-bind="v-name" data-n="${n}" value="${esc(v.name)}" placeholder="daily" maxlength="32" spellcheck="false">}</span>
          ${used ? `<div class="muted">${used === 1 ? '1 integration reads it by this name: renaming it here leaves it reading nothing.' : `${used} integrations read it by this name: renaming it here leaves them reading nothing.`}</div>` : ''}</div>
        <button type="button" class="ic-btn ic-btn-ghost ig-small" data-act="v-del" data-n="${n}">Delete this value</button></div>` : ''}</div>`;
  },
  valuesHtml(){
    const find = this.find.trim().toLowerCase();
    const shown = this.list.map((v, n) => ({v, n})).filter(({v, n}) => n === this.more || !find || [v.name, v.label, v.value].some(x => x.toLowerCase().includes(find)));
    // One group per integration, in the order of the list behind this
    // window; then what nothing reads yet, then what was just added. Inside
    // a group short fields come first and lists last.
    const kinds = ['text', 'long', 'number', 'chance', 'list'];
    const groups = S.data.integrations.map(i => i.id).concat('', NEW_VALUE_HOME.id)
      .map(id => shown.filter(({v}) => v.home.id === id)).filter(rows => rows.length)
      .map(rows => ({home:rows[0].v.home, rows:rows.sort((a, b) => kinds.indexOf(a.v.kind) - kinds.indexOf(b.v.kind) || a.n - b.n)}));
    return `${this.list.length > 6 ? `<input class="ic-input ig-val-find" data-bind="v-find" value="${esc(this.find)}" placeholder="Find a value" aria-label="Find a value">` : ''}
      ${groups.map(g => {
        // What is being looked for, or being renamed, is never folded away.
        const open = !!find || this.unfolded.has(g.home.id) || g.rows.some(({n}) => n === this.more);
        return `<div class="ig-val-group${open ? ' open' : ''}" data-key="g-${esc(g.home.id)}">
          <button type="button" class="ig-val-head" data-act="v-group" data-v="${esc(g.home.id)}" aria-expanded="${open}">${svg('chev')}<span class="ig-val-head-t">${esc(g.home.title)}</span><span class="ig-val-head-n">${plural(g.rows.length, 'value')}</span></button>
          ${open ? g.rows.map(({v, n}) => this.rowHtml(v, n)).join('') : ''}</div>`;
      }).join('')}
      ${!this.list.length ? '<div class="muted ig-val-empty">Nothing yet. A recipe you import brings its own values; or add one and write it as <span class="mono">{my.name}</span> wherever a value fits.</div>'
        : !shown.length ? '<div class="muted ig-val-empty">Nothing matches.</div>' : ''}
      <button type="button" class="ig-cond-add" data-act="v-add">${svg('plus')}Add a value</button>
      <div class="muted">Use one in any message or step as <span class="mono">{my.name}</span>. In a counting word, write it with a $: <span class="mono">{viewer.coins#$my.coin#$my.coins}</span> says "1 coin" or "5 coins" in your words.</div>`;
  },
  heldHtml(){
    const held = (Holders.values || []).map(v => holdersHtml(v.name)).join('');
    return !Holders.values ? '<div class="muted ig-val-empty">Asking the app who holds what…</div>'
      : held || '<div class="muted ig-val-empty">Nothing yet. A "Viewer value" step gives viewers coins or points, and they show up here.</div>';
  },
  render(){
    if (Modal.kind !== 'values') return;
    const dirty = this.dirty(), holders = (Holders.values || []).reduce((sum, v) => sum + v.holders, 0);
    const tabs = [['values', `Values · ${this.clean().length}`], ['held', `What viewers hold${holders ? ' · ' + holders : ''}`]];
    Modal.body(`${modalHead('steps', 'var(--accent)', 'Your values', 'Set something once and use it in any message or step. Change it here and everything that uses it follows.')}
      <div class="dest-form-body ig-conn-body ig-val-body">
        <div class="sub-tabs ig-seg ig-val-tabs">${tabs.map(([id, label]) => `<button class="sub-tab${id === this.tab ? ' on' : ''}" data-act="v-tab" data-v="${id}" aria-pressed="${id === this.tab}">${label}</button>`).join('')}${SEG_IND}</div>
        ${this.tab === 'held' ? this.heldHtml() : this.valuesHtml()}
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg">${dirty ? 'Changes not saved yet' : ''}</div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Close</button><button class="ic-btn ic-btn-primary" data-act="v-save" ${dirty ? '' : 'disabled'}>Save</button></div>`);
  },
  input(el){
    const b = el.dataset.bind, v = this.list[+el.dataset.n];
    if (b === 'v-find') this.find = el.value;
    else if (!v) return;
    else if (b === 'v-value') v.value = el.value;
    else if (b === 'v-name'){ v.name = el.value.trim(); v.named = true; }
    else if (b === 'v-label'){
      v.label = el.value;
      // Until a name is typed, it follows the label: "!daily gives" is daily_gives.
      if (!v.named && !this.usedIn(v.name)) v.name = snakeName(el.value, 32);
    }
    this.render();
  },
  // Why the list can't be saved as it is, with the row to look at; or null.
  problem(){
    const names = new Set();
    for (const [n, v] of this.list.entries()){
      if (!v.name && !v.label && !v.value) continue;
      if (!/^[A-Za-z0-9_]{1,32}$/.test(v.name.trim())) return {n, why:`"${v.label || v.name || 'A value'}" needs a name: letters, digits and _`};
      if (names.has(v.name.trim())) return {n, why:`Two values are named ${v.name.trim()}`};
      names.add(v.name.trim());
    }
    return null;
  },
  async act(a, el){
    const n = +el.dataset.n;
    if (a === 'v-tab'){ this.tab = el.dataset.v; this.render(); return true; }
    if (a === 'v-more'){ this.more = this.more === n ? null : n; this.render(); return true; }
    if (a === 'v-group'){
      const id = el.dataset.v, folding = el.getAttribute('aria-expanded') === 'true';
      if (folding){
        this.unfolded.delete(id);
        // Its open pencil would hold the group open.
        if (this.more !== null && this.list[this.more].home.id === id) this.more = null;
      } else this.unfolded.add(id);
      this.render();
      return true;
    }
    if (a === 'v-add'){
      this.list.push({name:'', label:'', value:'', kind:'text', home:NEW_VALUE_HOME});
      this.more = this.list.length - 1;
      this.unfolded.add(NEW_VALUE_HOME.id);
      this.find = '';
      this.render();
      focusField('v-label');
      return true;
    }
    if (a === 'v-del'){
      if (this.usedIn(this.list[n].name) && !armConfirm(el, 'Still in use. Delete?')) return true;
      this.list.splice(n, 1);
      this.more = null;
      this.render();
      return true;
    }
    if (a === 'v-save'){
      const wrong = this.problem();
      if (wrong){ this.more = wrong.n; this.find = ''; this.render(); toast(wrong.why, 'err', 6000); return true; }
      await busy(el, async () => {
        const values = this.clean();
        const r = await api('/integrations/values', {values});
        if (!r || r.ok === false){ toast((r && r.error) || 'That didn\'t save', 'err', 6000); return; }
        this.saved = JSON.stringify(values);
        if (Modal.kind === 'values') Modal.close(true);
        await load();
        toast('Saved', 'ok');
      });
      return true;
    }
    return false;
  },
};
const Recipes = {
  preview:null, text:'', back:null,
  // Which "See every step" and "Raw settings" rows are open.
  folds:new Set(),
  resume(){
    Modal.open('import', 700);
    Modal.back = this.back;
    this.render();
  },
  openImport(){
    this.preview = null;
    this.folds.clear();
    this.text = '';
    this.back = Modal.kind === 'catalog' ? {title:'Back to the catalog', go:() => Catalog.reopen()} : null;
    this.resume();
    focusField('r-text');
  },
  render(){
    if (Modal.kind !== 'import') return;
    const p = this.preview;
    const channels = S.data.connections.discord;
    Modal.body(`${modalHead('download', 'var(--accent)', 'Import a recipe', 'Paste a recipe someone shared. You see everything it adds before anything changes.')}
      <div class="dest-form-body ig-conn-body">
        <textarea class="ic-input ig-textarea-code" data-bind="r-text" placeholder="ic-recipe:1:…" spellcheck="false">${esc(this.text)}</textarea>
        ${p ? `<div class="ig-recipe" data-key="r-${esc(p.name)}">
          <div class="sys-section ig-recipe-list">
            <div class="ig-recipe-head"><span class="ig-ok">${svg('check')}</span><b translate="no">${esc(p.name)}</b><span class="muted">${plural(p.items.length, 'integration')}</span></div>
            ${p.items.map((it, n) => recipeItemHtml(it, n)).join('')}
          </div>
          ${recipeWarningHtml(p)}
          ${p.alerts ? this.alertsHtml(p) : ''}
          ${(p.values || []).length ? `<div class="sys-section" data-key="r-values"><div class="ic-label">Values it brings</div>
            <div class="muted">You change these later under <b>Your values</b>. One you already set stays as you set it.</div>
            <ul class="ig-value-list">${p.values.map(v => `<li><b>${esc(v.label || v.name)}</b> <span class="mono">${esc(v.mine == null ? v.value : v.mine)}</span>${v.mine == null ? '' : ' <span class="muted">(yours, kept)</span>'}</li>`).join('')}</ul></div>` : ''}
          ${p.uses_discord ? (channels.length ? `<div class="dff"><label>Post its Discord messages in</label><select class="ic-input" data-bind="r-discord">${channels.map(c => `<option value="${esc(c.id)}">#${esc(c.name)}</option>`).join('')}</select></div>`
            : warnHtml('It posts to Discord: <a href="#" data-act="conn" data-tab="discord">add a Discord channel</a> first.')) : ''}
          ${p.local_effects && !localLocked() ? `<label class="crash-check lan-warn"><input type="checkbox" data-bind="r-local"><span><span class="crash-check-title">I trust this recipe to run programs or write files on this PC</span><span class="muted">Only tick this if you know who made it.</span></span></label>` : ''}
          <div class="muted ig-recipe-safe">${svg('shield')}<span>Recipes are settings, not code. They can't see your keys, tokens or webhook links, and they arrive switched off.</span></div>
        </div>` : ''}
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Cancel</button>
        ${p ? `<button class="ic-btn ic-btn-primary" data-act="r-apply" ${p.local_effects && localLocked() ? 'disabled' : ''}>Add ${p.items.length}</button>` : '<button class="ic-btn ic-btn-primary" data-act="r-preview">Preview</button>'}
      </div>`);
    this.syncAlerts();
  },
  // A recipe's alerts bring their theme (shown on the real page) and
  // their uploads: taking the theme is a choice, ticked to start only
  // while this alerts page still has the plain default one.
  alertsHtml(p){
    const al = p.alerts, own = THEMES[themeOf(al.look)];
    const names = list => list.map(m => `<b translate="no">${esc(m.name)}</b>`).join(', ');
    return `<div class="sys-section ig-recipe-alerts" data-key="r-alerts">
      <div class="ic-label">Their alerts look</div>
      <div class="ig-stage ig-stage-short"><iframe class="ig-stage-frame" src="/alerts?demo" title="Their alerts look" tabindex="-1" aria-hidden="true"></iframe></div>
      <label class="crash-check"><input type="checkbox" data-bind="r-theme" ${al.mine_is_default ? 'checked' : ''}>
        <span><span class="crash-check-title">Use their alerts look (${esc(own.name)}${al.look.css ? ', with custom CSS' : ''})</span>
        <span class="muted">It becomes an alert design, played by every alert not given another. You can change it any time on the Alerts page.</span></span></label>
      ${al.media.length ? `<div class="muted">Comes with ${names(al.media)}: saved with your uploads.</div>` : ''}
      ${al.left_out.length ? warnHtml(`Too big to come along: ${names(al.left_out)}. Those alerts play their design's sound, or show no picture, until you pick another.`) : ''}
    </div>`;
  },
  // Their look on the stage, with their first alert.
  syncAlerts(){
    const p = this.preview;
    if (!p || !p.alerts) return;
    let card = {title:'New follower', text:'Nat', sound:'theme'};
    for (const it of p.items){
      const h = (it.integration.handlers || []).find(x => (x.steps || []).some(s => s.type === 'overlay'));
      if (h){ card = alertCardOf(h.steps.find(s => s.type === 'overlay'), h); break; }
    }
    // Their uploads aren't saved here yet: the stage shows the look alone.
    card.image = MEDIA_REF.test(card.image) ? '' : card.image;
    stageShow(q('.ig-recipe-alerts .ig-stage-frame'), 1280, {look:Object.assign({}, p.alerts.look, {position:'middle', align:'center'}), card});
  },
  async act(a, el){
    if (a === 'r-preview'){
      await busy(el, async () => {
        this.text = Conn.value('r-text');
        const r = await api('/integrations/import', {recipe:this.text});
        if (!r.ok){ toast(r.error, 'err', 6000); return; }
        this.preview = r;
        this.folds.clear();
        this.render();
      });
      return true;
    }
    if (a === 'r-apply'){
      await busy(el, async () => {
        const sel = q('[data-bind="r-discord"]'), local = q('[data-bind="r-local"]'), theme = q('[data-bind="r-theme"]');
        const r = await api('/integrations/import', {recipe:this.text, apply:true, discord:sel ? sel.value : '', allow_local:!!(local && local.checked),
          use_theme:!!(theme && theme.checked)});
        if (!r.ok){ toast(r.error, 'err', 6000); return; }
        if (Modal.kind === 'import') Modal.close(true);
        await load();
        const added = (r.ids || []).map(find).filter(Boolean);
        await saveNamedDesigns(added);
        this.landed(added, r.added);
      });
      return true;
    }
    if (a === 's-make'){ await busy(el, () => this.share()); return true; }
    return false;
  },
  // What an import added: the first that still needs a detail opens on it
  // (a recipe never carries addresses or channels); otherwise they glow
  // where they landed.
  landed(added, count){
    const n = plural(count || added.length, 'integration');
    const unfinished = added.find(i => issuesOf(i).length);
    if (unfinished){
      toast(`Added ${n}, switched off. ${unfinished.name} needs one more detail first.`, 'info', 5000);
      openEditor(unfinished);
      return;
    }
    added.forEach(i => reveal(i.id));
    toast(`Added ${n}. They're off until you switch them on.`, 'ok', 4500);
  },
  // A pasted recipe previews at once: there is nothing else to do with it.
  pasted(el){
    const text = el.value.trim();
    if (!text.startsWith('ic-recipe:1:') || text === this.text.trim()) return;
    this.act('r-preview', q('.dest-form-foot .ic-btn-primary'));
  },
  // `only`: shared from one integration's menu, the recipe is made (and
  // copied) at once; ticking more remakes it with "Make recipe".
  openShare(only){
    const one = only ? find(only) : null;
    Modal.open('share', 700);
    Modal.body(`${modalHead('copy', 'var(--accent)', 'Share as a recipe', 'Pick what to share. Your connections, keys and secret links are never included.')}
      <div class="dest-form-body ig-conn-body">
        <div class="dff"><label>Recipe name</label><input class="ic-input" data-bind="s-name" placeholder="My crash kit" value="${one ? esc(one.name) : ''}"></div>
        <div class="ig-share-list">${S.data.integrations.map(i =>
          `<label class="crash-check"><input type="checkbox" data-bind="s-pick" value="${esc(i.id)}" ${i.id === only ? 'checked' : ''}><span><span class="crash-check-title">${esc(i.name)}</span></span></label>`).join('')}</div>
        <label class="crash-check"><input type="checkbox" data-bind="s-theme" checked><span><span class="crash-check-title">Alerts bring my alerts look</span>
          <span class="muted">The theme, its changes and custom CSS, and the sounds and pictures they use (up to 4 MB in all). They choose whether to use it.</span></span></label>
        <textarea class="ic-input ig-textarea-code" data-bind="s-out" readonly hidden></textarea>
      </div>
      <div class="dest-form-foot"><div class="dest-form-msg"></div>
        <button class="ic-btn ic-btn-ghost" data-act="modal-close">Close</button><button class="ic-btn ic-btn-primary" data-act="s-make">Make recipe</button></div>`);
    if (one) busy(q('[data-act="s-make"]'), () => this.share());
    else focusField('s-name');
  },
  async share(){
    const ids = [...Modal.el.querySelectorAll('[data-bind="s-pick"]:checked')].map(x => x.value);
    if (!ids.length){ toast('Tick at least one integration to share', 'info'); return; }
    const theme = q('[data-bind="s-theme"]');
    const r = await api('/integrations/export', {ids, name:Conn.value('s-name'), with_theme:!theme || theme.checked});
    if (!r.ok){ toast(r.error, 'err'); return; }
    if (Modal.kind !== 'share') return;
    const out = q('[data-bind="s-out"]');
    out.hidden = false;
    out.value = r.recipe;
    // Selected too, so a copy that fails is one Ctrl+C away.
    if (await writeClipboard(r.recipe)) toast('Recipe copied: paste it anywhere', 'ok', 3500);
    else toast(COPY_FAILED, 'err');
    out.focus({preventScroll:true});
    out.select();
  },
};

