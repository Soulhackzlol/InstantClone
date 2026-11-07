// ---------------------------------------------------------------- module editor

// The simple editor's option rows, by what they set; the last group takes
// every row the others don't name.
const ED_GROUPS = [
  ['Where', ['where', 'url', 'light', 'file']],
  ['What it does', ['action', 'look', 'msg', 'ping', 'reply', 'priority', 'dgroup', 'tiers', 'adesign', 'acolor', 'asound', 'apic', 'atime']],
  ['When', null],
];
// The simple editor for catalog modules: pick a moment, write on the
// preview itself, and every option is a row beside it that opens in place.
const EDITOR_SHOWN_KEY = 'instantclone_editor_shown';
const EDITOR_SHOW_AFTER_MS = 600;
// A numbered heading over a part of the editor, in reading order.
function edSection(n, title, sub = ''){
  return `<div class="ig-ed-sect"><span class="ig-ed-num">${n}</span><b>${title}</b>${sub ? `<span class="ig-ed-sect-s">${sub}</span>` : ''}</div>`;
}
const Editor = {
  d:null, sel:0, chip:null, run:null, dirty:false, back:null, justAdded:false,
  // opts: {back} (see Modal.back); {justAdded} when the catalog just made
  // it as a draft, which closing keeps and Back removes again.
  open(i, switchOn, opts){
    opts = opts || {};
    dropAddUndos();
    this.d = withHandler(clone(i));
    this.d.handlers.forEach(h => sampleArgsFrom(h, presetOf(this.d)));
    this.back = opts.back || null;
    this.justAdded = !!opts.justAdded;
    this.testValues = {};
    // Tests start as a dry run: sending for real is a choice, every time.
    this.testDry = true;
    // Waiting for one detail: shown switched on, so saving once it is
    // complete also turns it on.
    this.dirty = !!switchOn && !this.d.enabled && !this.justAdded;
    if (switchOn) this.d.enabled = true;
    this.sel = Math.max(0, this.d.handlers.findIndex(h => h.enabled));
    this.tier = 0;
    // Missing a detail: open straight on the chip that fixes it.
    const known = id => S.data.connections.discord.some(c => c.id === id);
    const steps = allSteps(this.d);
    const sel = this.d.handlers[this.sel].trigger;
    const keyless = this.d.handlers.some(h => h.trigger.type === 'shortcut' && !h.trigger.hotkey && !h.trigger.midi && !h.trigger.token);
    this.chip = steps.some(s => s.type === 'discord' && !known(s.params.connection)) ? 'where'
      : steps.some(s => s.type === 'http' && !s.params.url) ? 'url' : keyless ? 'key'
      : sel.type === 'scene' && !sel.scene ? 'scene' : commandClash(this.d) ? 'cmd' : null;
    this.run = null;
    this.converted = null;
    TRIES.clear();
    historyStart(this);
    this.show();
  },
  // After `d` is swapped for another version of it (Discard, Undo).
  reset(){
    this.d.handlers.forEach(h => sampleArgsFrom(h, presetOf(this.d)));
    this.sel = Math.min(this.sel, this.d.handlers.length - 1);
    this.chip = null;
    this.run = null;
  },
  resume(){
    if (pickNewChannel(this.d)){
      this.dirty = true;
      if (this.chip === 'where') this.chip = null;
    }
    this.show();
  },
  show(){
    Modal.open('editor', 1180);
    Modal.back = this.back;
    Modal.onClose = () => {
      if (this.dirty){ Modal.guard(); return false; }
      if (this.justAdded && Modal.pending === 'close') toast(`${this.d.name} is kept as a draft, switched off`, 'info', 4000);
      return true;
    };
    this.render();
  },
  // The picked value takes the whole answer's place in the reply, or
  // joins the end of it.
  useInReply(){
    const h = this.handler(), http = firstStep(h, 'http'), step = this.stepOf(h);
    const t = http && tryEntry(http), field = step && (KINDS[step.type] || {}).text;
    if (!t || !t.picked || !field) return;
    const token = '{' + t.picked.token + '}';
    const whole = '{' + ((http.params.save_as || '').trim() || 'response') + '.body}';
    const text = step.params[field] || '';
    step.params[field] = text.includes(whole) ? text.replace(whole, token) : (text.trimEnd() + ' ' + token).trim();
    this.changed();
  },
  convertSoon(text){
    clearTimeout(this.convertTimer);
    this.convertTimer = setTimeout(() => this.convert(text), 300);
  },
  // Fill the address, the reply and the command from a bot command.
  async convert(text){
    const r = await api('/integrations/convert', {text});
    if (Modal.kind !== 'editor') return;
    if (!r.ok){
      this.converted = {error:r.error};
      this.render();
      return;
    }
    const h = this.handler(), step = this.stepOf(h), field = step && (KINDS[step.type] || {}).text;
    allSteps(this.d).filter(s => s.type === 'http').forEach(s => { s.params.url = r.url; });
    if (r.reply && field) step.params[field] = r.reply;
    if (r.command && h.trigger.type === 'chat_command') h.trigger.command = r.command;
    this.converted = {from:r.from, reply:!!r.reply, command:r.command, unknown:r.unknown || []};
    this.changed();
    // The field still shows the pasted text while it has focus.
    const f = q('[data-bind="url"]');
    if (f) f.value = r.url;
  },
  convertedHtml(){
    const c = this.converted;
    if (!c) return '';
    if (c.error) return warnHtml(esc(c.error));
    if (c.start){
      return `<div class="ig-converted">${svg('check', ' stroke-width="3"')}<span>Ready to go: ${esc(c.start)} asks decapi.me, a free service many streamers use. Change the reply on the preview above.</span></div>`;
    }
    const filled = ['the address'].concat(c.reply ? ['the reply'] : [], c.command ? [`the command (${esc(c.command)})`] : []);
    const unknown = c.unknown.length
      ? ` These have no InstantClone match and stay as written: ${c.unknown.map(u => `<code>${esc(u)}</code>`).join(' ')}` : '';
    return `<div class="ig-converted">${svg('check', ' stroke-width="3"')}<span>Converted from ${esc(c.from)}: ${filled.join(', ')} filled in.${unknown}</span></div>`;
  },
  // A ready-made command takes this trigger's place, and answers right
  // away so the preview reads like the real thing.
  async useStart(id){
    const start = (S.data.catalog.api_starts || []).find(x => x.id === id);
    if (!start) return;
    const was = this.handler();
    const fresh = clone(start.handler);
    fresh.enabled = was.enabled;
    this.d.handlers[this.sel] = fresh;
    const p = presetOf(this.d);
    const named = (p && this.d.name === p.name) || S.data.catalog.api_starts.some(x => x.command === this.d.name);
    if (named) this.d.name = start.command;
    this.converted = {start:start.command};
    TRIES.clear();
    this.changed();
    // The request button spins while the real answer comes in.
    await busy(q('[data-act="b-try"]'), () => tryRequest(firstStep(fresh, 'http'), fresh));
    if (Modal.kind === 'editor' && this.handler() === fresh) this.render();
  },
  startsHtml(http){
    const starts = S.data.catalog.api_starts || [];
    return `<div class="ig-starts"><span class="ic-label">Ready-made</span>${starts.map(x =>
      `<button class="ig-start${x.handler.steps[0].params.url === http.params.url ? ' on' : ''}" data-act="ed-start" data-v="${esc(x.id)}">${esc(x.command)}</button>`).join('')}</div>`;
  },
  // Back from "Open in builder", with whatever was changed there.
  fromBuilder(d, dirty){
    this.d = withHandler(d);
    this.d.handlers.forEach(h => sampleArgsFrom(h, presetOf(this.d)));
    this.dirty = dirty;
    this.sel = Math.min(this.sel, this.d.handlers.length - 1);
    this.chip = null;
    this.run = null;
    this.show();
  },
  handler(){ return this.d.handlers[this.sel]; },
  // Esc on an open chip closes just the chip, back on its button.
  closeChip(){
    const open = this.chip;
    if (!open) return false;
    this.chip = null;
    this.render();
    const btn = q(`[data-act="chip"][data-chip="${open}"]`);
    if (btn) btn.focus({preventScroll:true});
    return true;
  },
  // The step it edits: the moment's main one, or the bigger alert picked
  // under Bigger ones.
  stepOf(h){
    const chain = alertTiers(h);
    return chain ? (chain.tiers[this.tier] || chain.tiers[0]).step : primaryStep(h);
  },
  // A bigger alert's amount, typed: a whole number between its neighbours'.
  // False while it isn't (the field says so).
  tierFrom(h, el){
    const chain = alertTiers(h), n = +el.dataset.n, v = +el.value.trim();
    if (!chain || !(n >= 1) || !chain.tiers[n]) return false;
    const below = chain.tiers[n - 1].from, above = (chain.tiers[n + 1] || {}).from;
    const ok = Number.isInteger(v) && v > below && (above === undefined || v < above);
    el.setAttribute('aria-invalid', ok ? 'false' : 'true');
    el.title = ok ? '' : `A whole number above ${below}${above === undefined ? '' : ` and below ${above}`}`;
    if (!ok) return false;
    chain.tiers[n].from = v;
    setTiers(h, chain);
    return true;
  },
  render(){
    const d = this.d, h = this.handler();
    const chain = alertTiers(h);
    if (chain && this.tier) TIER_SAMPLE.set(h, {name:chain.name, value:String(chain.tiers[this.tier].from)});
    else TIER_SAMPLE.delete(h);
    const step = this.stepOf(h);
    const k = step ? KINDS[step.type] : null;
    const p = presetOf(d);
    const head = modalHead(PRESET_ICON[d.preset] || (k ? k.icon : 'steps'), k ? k.c : 'var(--accent)', titleInput(d.name),
      esc(p ? p.description : 'Pick a moment, then write on the message itself. Everything else is one click away.'),
      enableSwitch('enabled', d.enabled));
    // A test opens under the preview; every option is a row beside it.
    // The sections are numbered in reading order; moments come first when there are several.
    const first = d.handlers.length > 1 ? 1 : 0;
    const test = this.chip === '__test'
      ? `<div class="ig-pop" data-autoh><div class="ig-pop-in" data-key="pop-__test-${this.sel}">${this.popHtml(step, h)}</div></div>` : '';
    Modal.body(`${head}
      <div class="dest-form-body ig-ed-body">
        ${this.momentsHtml()}
        ${twitchEventStatusHtml(h.trigger)}
        <div class="ig-ed-cols">
          <div class="ig-ed-main">
            ${edSection(first + 1, k && k.text ? 'What it says' : 'What it does')}
            <div class="ig-ed-preview" data-flip data-autoh><div class="ig-ed-pv" data-key="pv-${this.sel}">${this.previewBoxHtml(step, h)}</div></div>
            ${test}
          </div>
          <aside class="ig-ed-side" aria-label="Options" data-flip>${edSection(first + 2, 'Options')}${this.sideHtml(this.chips(step, h), step, h)}</aside>
        </div>
      </div>
      ${footHtml(d, this.dirty, `${historyButtonsHtml(this)}<button class="ic-btn ic-btn-ghost" data-act="ed-builder">Open in builder</button>
        ${step && step.type === 'overlay'
          ? `<button class="ic-btn ic-btn-ghost" data-act="alert-show">${svg('play', ' class="ig-btn-ic"')}Show it on stream</button>`
          : `<button class="ic-btn ic-btn-ghost" data-act="ed-test">${svg('play', ' class="ig-btn-ic"')}Send a test</button>`}`, this.justAdded)}`);
    Modal.form.querySelectorAll('.ig-msg').forEach(autosize);
    if (this.chip === 'detect') Meter.watch();
    if (this.chip === 'scene') ObsLive.watch();
    if (step && step.type === 'overlay') alertStage(step, h, false);
    this.showTheWay();
  },
  // The first time an editor with several moments opens, the trail flies
  // from the picked moment to what it edits: that's how the two connect.
  showTheWay(){
    if (this.d.handlers.length < 2 || store.get(EDITOR_SHOWN_KEY)) return;
    store.set(EDITOR_SHOWN_KEY, '1');
    setTimeout(() => {
      const from = q('.ig-mchip.on, .ig-moment.on'), to = q('.ig-ed-preview');
      flyTrail(from, to, () => to && to.animate([{boxShadow:'0 0 0 0 transparent'}, {boxShadow:'0 0 0 3px var(--accent)'}, {boxShadow:'0 0 0 0 transparent'}],
        {duration:700, easing:'cubic-bezier(.2,.7,.2,1)'}));
    }, EDITOR_SHOW_AFTER_MS);
  },
  // The options as rows, grouped, under what it still needs before it can
  // run. The open row shows its settings right under it.
  sideHtml(chips, step, h){
    const row = c => `<button class="ig-opt${this.chip === c.id ? ' on' : ''}${c.warn ? ' warn' : ''}" data-act="chip" data-chip="${c.id}" data-key="row-${c.id}" aria-expanded="${this.chip === c.id}">
        <span>${esc(c.k)}</span><b data-tick>${esc(c.v)}</b>${svg('chev')}</button>`
      + (this.chip === c.id ? `<div class="ig-pop-in ig-opt-pop" data-key="pop-${c.id}-${this.sel}">${this.popHtml(step, h)}</div>` : '');
    const placed = new Set();
    const groups = ED_GROUPS.map(([label, ids]) => {
      const list = chips.filter(c => !placed.has(c.id) && (!ids || ids.includes(c.id)));
      list.forEach(c => placed.add(c.id));
      return list.length ? `<div class="ig-side-g" data-key="g-${label}"><div class="ig-side-h">${label}</div>${list.map(row).join('')}</div>` : '';
    }).join('');
    const missing = chips.find(c => c.warn);
    const fix = missing ? `<div class="ig-side-fix" data-key="fix">${svg('warn')}<span><b>Before it can run</b>${esc(missing.k)}: ${esc(missing.v)}</span>
      ${this.chip === missing.id ? '' : `<button class="ic-btn ic-btn-primary ig-small" data-act="chip" data-chip="${missing.id}">Fix it</button>`}</div>` : '';
    return fix + groups;
  },
  // The moments it acts on, each with its switch and its remove button
  // (shown on hover and on the open one). A moment that still misses
  // something says so in its colour and its tooltip.
  momentsHtml(){
    const hs = this.d.handlers;
    if (hs.length === 1) return '';
    const labels = momentLabels(hs);
    const dot = (h, n) => `<button class="ig-dot${h.enabled ? ' on' : ''}" role="switch" aria-checked="${h.enabled}" aria-label="Runs on: ${esc(labels[n])}"
      title="${h.enabled ? 'On: this moment runs. Click to skip it.' : 'Off: this moment is skipped. Click to turn it on.'}" data-act="moment-toggle" data-n="${n}">${svg('check', ' stroke-width="3.2"')}</button>`;
    const name = (n, cls) => {
      const missing = h => h.enabled && handlerMissing(h);
      const warn = missing(hs[n]);
      return `<button class="${cls}${n === this.sel ? ' on' : ''}${warn ? ' warn' : ''}"${n === this.sel ? ' aria-current="true"' : ''}
        data-act="moment" data-n="${n}"${warn ? ` title="${esc(capFirst(warn))}"` : ''}>${esc(labels[n])}</button>`;
    };
    const del = n => `<button class="ig-moment-del" data-act="moment-del" data-n="${n}" aria-label="Remove ${esc(labels[n])}" title="Remove">${svg('x')}</button>`;
    const alert = (primaryStep(hs[this.sel]) || {}).type === 'overlay';
    const head = edSection(1, alert ? 'Which alerts' : 'When it runs', 'Pick one to edit it. Its switch turns it on or off.');
    // Too many for a timeline: chips that wrap, each keeping its switch.
    if (hs.length > 5){
      return `${head}<div class="ig-mchips" role="group" aria-label="Moments">${hs.map((h, n) => `<div class="ig-mchip${n === this.sel ? ' on' : ''}">
        ${dot(h, n)}${name(n, 'ig-mchip-name')}${del(n)}</div>`).join('')}</div>`;
    }
    return `${head}<div class="ig-moments" role="group" aria-label="Moments" style="--n:${hs.length}">${hs.map((h, n) => `<div class="ig-moment${n === this.sel ? ' on' : ''}">
      ${dot(h, n)}${name(n, 'ig-moment-name')}${del(n)}</div>`).join('')}</div>`;
  },
  previewBoxHtml(step, h){
    if (step && step.type === 'light'){
      const state = this.d.preset === 'on_air_light' && (h.trigger.filters || {}).state;
      return `<div class="dcard-screen ig-live-plain">
        <div class="ic-label">${state ? 'When it turns ' + esc({live:'live', delay:'delayed', crash:'crashed', off:'off'}[state] || state) : 'The light'}</div>
        ${previewHtml(step, h, true)}${lightControlsHtml(step.params, name => `data-bind="l-${name}"`)}</div>`;
    }
    if (!step || !(KINDS[step.type] || {}).text){
      const hint = step && step.type === 'delay_action' ? 'Pick what it does with <b>Does</b> below, or' : 'Nothing to write here.';
      return `<div class="dcard-screen ig-live-plain">${previewHtml(step, h, true)}
        <div class="muted">${hint} <a href="#" data-act="ed-builder">Open it in the builder</a> to see every step.</div></div>`;
    }
    const k = KINDS[step.type];
    const value = step.params[k.text] || '';
    const vars = sampleMap(h);
    const discord = step.type === 'discord';
    const sample = sampleLine(value, h, discord);
    // A tried answer can send the run down another branch ("offline").
    const reached = reachedStep(h.steps, vars, step.type);
    const instead = reached && reached !== step
      ? `<div class="ig-sample ig-instead">Right now it would say: <b>${esc(renderSample(reached.params[k.text], vars))}</b></div>` : '';
    const card = discord && step.params.style === 'card';
    const area = `<textarea class="ig-msg ig-tpl-in" data-bind="text" rows="1" aria-label="${card ? 'Card text' : 'Message'}" spellcheck="true" data-keep-style${card ? ' placeholder="Card text (optional)"' : ''}>${esc(value)}</textarea>`;
    const editor = tplHtml(area, value, vars, 'msg', discord)
      + `<div class="ig-sample${sample ? '' : ' empty'}">${sample}</div>${instead}`;
    let box;
    if (discord){
      const set = S.data.connections.discord.some(c => c.id === step.params.connection);
      const ping = step.params.ping === 'here' ? '@here' : step.params.ping === 'everyone' ? '@everyone' : '';
      const edit = step.params.edit || '';
      const note = {last:'updates its last message', close:'updates its last message, then starts fresh'}[edit];
      const body = card ? this.cardEditorHtml(step, h, editor) : editor;
      const channel = set ? esc(channelName(step.params.connection).replace(/^#/, ''))
        : '<button class="none" data-act="chip" data-chip="where">pick a channel</button>';
      box = `<div class="ig-live ig-live-discord">
        <div class="ig-live-head"><span class="hash${set ? '' : ' none'}">#</span>${channel}<span class="ig-live-tag">Live preview</span></div>
        <div class="ig-live-msg"><span class="ig-live-av">${svg('shield')}</span>
          <div class="ig-live-col"><div class="ig-live-meta"><b>InstantClone</b><span class="ig-app">APP</span><span class="ig-when">${esc(discordNow())}</span>
            ${ping && edit !== 'close' ? `<span class="ig-ping">${esc(ping)}</span>` : ''}
            ${note ? `<span class="ig-live-upd">${svg('pencil')}${note}</span>` : ''}</div>
          ${body}</div></div></div>`;
    } else if (step.type === 'chat'){
      const t = h.trigger;
      const ask = t.type === 'chat_command' ? `${t.command || '!command'}${SAMPLE_ARGS.has(h) ? ' ' + SAMPLE_ARGS.get(h) : ''}` : t.type === 'chat_message' ? (t.pattern || 'hey') : '';
      box = `<div class="ig-live ig-live-chat">
        <div class="ig-live-tag">STREAM CHAT · LIVE PREVIEW</div>
        ${ask ? `<div><b class="u1">viewer</b>: ${esc(ask)}</div>` : ''}
        <div><b class="u2">${esc(chatAuthor(step))}</b>:</div>${editor}</div>`;
    } else if (step.type === 'overlay'){
      const p = step.params;
      box = `<div class="ig-live ig-live-alert">
        <div class="ig-live-row"><span class="ig-live-tag">ON STREAM · LIVE PREVIEW</span>
          <span class="ig-live-theme"><button class="ig-link" data-act="alert-replay">${svg('play', ' class="ig-btn-ic"')}Play it</button>
          <span>Looks like ${esc(designLabel(p.design))}</span><button class="ig-link" data-act="chip" data-chip="adesign">Change</button></span></div>
        <div class="ig-ed-stage"><iframe class="ig-stage-frame" src="/alerts?demo" title="Alert preview" tabindex="-1" aria-hidden="true"></iframe></div>
        <div class="ig-alert-fields">
          <div class="dff"><label>Small title</label>${tplHtml(`<input class="ic-input ig-tpl-in" data-bind="title" value="${esc(p.title || '')}" placeholder="Optional, like New follower" aria-label="Small title" spellcheck="false">`, p.title, vars, 'line')}</div>
          <div class="dff"><label>Text</label>${editor}</div>
          ${this.messageFieldHtml(step, h, vars)}
        </div></div>${alertsStatusHtml(true)}`;
    } else if (step.type === 'phone'){
      box = `<div class="ig-live ig-live-phone">
        <div class="ig-live-tag">INSTANTCLONE · now</div>
        ${tplHtml(`<input class="ic-input ig-tpl-in" data-bind="title" value="${esc(step.params.title || '')}" placeholder="Title" aria-label="Title">`, step.params.title, vars, 'line')}
        ${editor}</div>`;
    } else if (step.type === 'http' && this.d.preset === 'on_air_light'){
      const state = (h.trigger.filters || {}).state || 'live';
      box = `<div class="dcard-screen ig-live-plain">
        <div class="ic-label">When it turns ${esc({live:'live', delay:'delayed', crash:'crashed', off:'off'}[state] || state)}</div>
        <div>InstantClone tells your light, at ${esc(maskUrl(step.params.url) || 'the address you set in Light')}.</div>
        <details class="ig-more-fields"><summary>What it sends (advanced)</summary>${editor}</details></div>`;
      return box;
    } else {
      box = `<div class="dcard-screen ig-live-plain">
        <div class="ic-label">${esc(k.label)}${step.type === 'http' ? ' · what it sends' : ''}</div>${editor}</div>`;
    }
    // Every step counts: the simple editor edits more than the message.
    const unknown = [];
    walk(h.steps, s => unknownVars(h, s).forEach(u => { if (!unknown.some(x => x.name === u.name)) unknown.push(u); }));
    return `${box}${unknownVarsHtml(unknown)}${FORMS_HELP}`;
  },
  // A Discord card you write on: the text above it, then the card itself
  // (title, body, fields, footer) in its color, then the color swatches.
  // Every part shows what it will say until it's being edited. `editor`
  // is the body's textarea with its "reads like" line.
  cardEditorHtml(step, h, editor){
    const p = step.params, vars = sampleMap(h);
    const color = cardColor(p.color, vars);
    const fields = parseFields(p.fields, true);
    const ts = p.timestamp === 'yes';
    const input = (bind, value, attrs, cls) => `<input class="ig-tpl-in${cls ? ' ' + cls : ''}" data-bind="${bind}" value="${esc(value || '')}" spellcheck="false" ${attrs}>`;
    const known = CARD_COLORS.some(([c]) => c === color);
    const swatches = CARD_COLORS.map(([c, label]) => {
      const on = c === color;
      return `<button class="ig-swatch${on ? ' on' : ''}" style="--sw:${c}" data-act="set-color" data-v="${c}" role="radio" aria-checked="${on}"
        tabindex="${on || (!known && c === CARD_COLORS[0][0]) ? 0 : -1}" aria-label="${label}" title="${label}"></button>`;
    }).join('');
    return `${tplHtml(`<input class="ig-above ig-tpl-in" data-bind="above" value="${esc(p.above || '')}" placeholder="Optional line above the card. A clip link here shows a video player." aria-label="Line above the card" spellcheck="false">`, p.above, vars, 'above', true)}
      <div class="ig-embed big edit" style="--ec:${esc(color)}">
        ${tplHtml(input('title', p.title, 'placeholder="Title" aria-label="Card title"', 'ig-embed-title-in'), p.title, vars, 'title', true)}
        ${editor}
        <div class="ig-fields-edit" data-flip>${fields.map((f, n) => `<div class="ig-field-edit${f.inline ? '' : ' full'}" style="--span:${fieldSpans(fields)[n]}" data-key="f-${n}">
          ${tplHtml(input('f-name', f.name, `data-n="${n}" placeholder="Name" aria-label="Field name"`), f.name, vars, 'line', false)}
          ${tplHtml(input('f-value', f.value, `data-n="${n}" placeholder="Value, like {reason}" aria-label="Field value"`), f.value, vars, 'line', true)}
          <button class="ig-field-w" data-act="f-inline" data-n="${n}" aria-pressed="${f.inline}" aria-label="Side by side"
            title="${f.inline ? 'Side by side with the next field. Click for full width.' : 'Full width. Click to sit side by side.'}">${svg(f.inline ? 'columns' : 'rows')}</button>
          <button class="ic-btn-tiny" data-act="f-del" data-n="${n}" aria-label="Remove this field">${svg('x')}</button></div>`).join('')}
          <button class="ig-field-add" data-act="f-add" data-key="f-add">${svg('plus')}Add a field</button></div>
        <div class="ig-embed-foot edit"><i>${svg('shield')}</i>
          ${tplHtml(input('footer', p.footer, 'placeholder="Footer" aria-label="Footer"'), p.footer, vars, 'line', false)}
          ${ts && p.footer ? '<span class="ig-foot-dot" aria-hidden="true">•</span>' : ''}
          <button class="ig-ts-toggle${ts ? ' on' : ''}" data-act="set-ts" aria-pressed="${ts}">${svg('clock')}${ts ? esc(discordNow()) : 'Add the time'}</button></div>
      </div>
      <div class="ig-swatches"><span class="ig-swatch-set" role="radiogroup" aria-label="Card color">${swatches}</span>
        <label class="ig-swatch custom${known ? '' : ' on'}" title="Any color"><input type="color" data-bind="color" value="${esc(color)}" aria-label="Any color (now ${esc(color)})"></label></div>`;
  },
  // Edit the card's fields: `change` gets the list, the text is rebuilt.
  fields(step, change){
    const list = parseFields(step.params.fields, true);
    change(list);
    step.params.fields = fieldsText(list);
  },
  // The viewer's own words under the alert, where the moment brings some
  // (a cheer, a resub, a redeem's input), or one already set.
  messageFieldHtml(step, h, vars){
    const p = step.params;
    // A cheer's words without its Cheer100s first.
    const known = varsFor(h);
    const said = ['words', 'message', 'input'].find(name => known.some(v => v.name === name));
    if (!said && !p.message) return '';
    return `<div class="dff"><label>Viewer's message</label>${tplHtml(`<input class="ic-input ig-tpl-in" data-bind="message" value="${esc(p.message || '')}"
      placeholder="Optional, like {${said || 'message'}}" aria-label="Viewer's message" spellcheck="false">`, p.message, vars, 'line')}</div>`;
  },
  // The chips shown for this moment, with their current value.
  chips(step, h){
    const d = this.d, out = [];
    const discordSteps = allSteps(d).filter(s => s.type === 'discord');
    const t = h.trigger;
    if (discordSteps.length){
      const id = discordSteps[0].params.connection;
      const set = S.data.connections.discord.some(c => c.id === id);
      out.push({id:'where', k:'Post in', v:set ? channelName(id) : 'pick a channel', warn:!set});
      if (step && step.type === 'discord'){
        const ping = step.params.ping || '', edit = step.params.edit || '';
        out.push({id:'look', k:'Look', v:step.params.style === 'card' ? 'card' : 'plain message'});
        out.push({id:'msg', k:'Each time', v:{last:'updates the same message', close:'updates it, then starts fresh'}[edit] || 'posts a new message'});
        // Finishing a message never pings; the others can post a new one.
        if (edit !== 'close') out.push({id:'ping', k:'Ping', v:ping === 'here' ? '@here' : ping === 'everyone' ? '@everyone' : 'nobody'});
      }
    }
    if (step && step.type === 'overlay'){
      const p = step.params, chain = alertTiers(h);
      if (chain){
        const bigger = chain.tiers.length - 1;
        out.push({id:'tiers', k:'Levels', v:this.tier ? `editing ${lowerFirst(tierLabel(chain, this.tier))}`
          : bigger ? tierLabel(chain, 1).toLowerCase() + (bigger > 1 ? ` +${bigger - 1}` : '') : 'none'});
      }
      out.push({id:'adesign', k:'Design', v:designLabel(p.design)});
      out.push({id:'acolor', k:'Accent', v:alertColorName(p.color)});
      out.push({id:'asound', k:'Sound', v:soundLabel(p.sound)});
      out.push({id:'apic', k:'Picture', v:p.image ? (MEDIA_REF.test(p.image) ? p.image.slice(6) : 'a link') : 'none',
        warn:!!p.image && !MEDIA_REF.test(p.image) && !WEB_LINK.test(p.image)});
      out.push({id:'atime', k:'On screen for', v:(p.seconds || '6') + ' s'});
    }
    if (t.type === 'shortcut') out.push({id:'key', k:'Starts with', v:shortcutLabel(t) || 'set a key, pad or Stream Deck', warn:!t.hotkey && !t.midi && !t.token});
    const action = firstStep(h, 'delay_action');
    if (action) out.push({id:'action', k:'Does', v:lowerFirst(delayActionLabel(action.params, {}))});
    if (t.type === 'chat_activity') out.push({id:'detect', k:'Fires when', v:activitySummary(t), warn:!(t.rules || []).length});
    if (t.type === 'scene') out.push({id:'scene', k:'Scene', v:t.scene ? sceneLabel(t) : 'pick a scene', warn:!t.scene});
    const tune = t.type === 'event' && tuneSummary(t);
    if (tune) out.push({id:'tune', k:tune.k, v:tune.v, warn:tune.bad});
    if (t.type === 'event' || t.type === 'timer' || t.type === 'webhook') out.push({id:'send', k:'Send', v:this.sendValue(h).label});
    if (t.type === 'event' && this.filterVars(t).length && d.preset !== 'on_air_light'){
      const set = Object.entries(t.filters || {}).filter(([k, v]) => v && !isParam(t, k));
      out.push({id:'only', k:'Only if', v:set.length ? set.map(([k, v]) => `${filterName(k)}: ${v}`).join(', ') : 'no conditions'});
    }
    if (t.type === 'chat_command'){
      out.push({id:'cmd', k:'Command', v:[t.command].concat(t.aliases || []).join(' '), warn:!!commandClash(d)});
      out.push({id:'who', k:'Who can use it', v:rolesLabel(t.roles)});
    }
    if (step && step.type === 'chat') out.push({id:'reply', k:'Reply as', v:step.params.as === 'bot' ? 'bot account' : 'your account'});
    if (step && step.type === 'phone') out.push({id:'priority', k:'Priority', v:step.params.priority || 'normal'});
    if (step && step.type === 'dock') out.push({id:'dgroup', k:'Group', v:step.params.group || 'none'});
    const light = firstStep(h, 'light');
    if (light) out.push({id:'light', k:'Light', v:light.params.name || 'pick your light', warn:!light.params.device});
    const http = firstStep(h, 'http');
    if (http && d.preset === 'on_air_light') out.push({id:'url', k:'Light', v:lightOf(http, t) || maskUrl(http.params.url) || 'pick your light', warn:!http.params.url});
    else if (http) out.push({id:'url', k:t.type.startsWith('chat') ? 'Answer from' : 'Send to', v:maskUrl(http.params.url) || 'set the address', warn:!http.params.url});
    if (step && step.type === 'file') out.push({id:'file', k:'File', v:step.params.path || 'pick a file', warn:!step.params.path});
    out.push({id:'cooldown', k:'Repeat at most', v:d.cooldown_ms ? 'once every ' + fmtDur(d.cooldown_ms) : 'anytime'});
    out.push({id:'quiet', k:'Quiet hours', v:d.quiet ? `${d.quiet.from} to ${d.quiet.to}` : 'off'});
    return out;
  },
  // The other alerts in this integration: one click gives them all this
  // option, as each moment has its own.
  everyAlertHtml(names){
    const others = this.d.handlers.filter(h => h !== this.handler() && (primaryStep(h) || {}).type === 'overlay');
    if (!others.length) return '';
    return `<button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="alert-all" data-names="${names.join(' ')}">Use on every alert here (${others.length + 1})</button>`;
  },
  // Values worth narrowing on ("only YouTube", "only crashes"); counts,
  // durations and reports change every time.
  filterVars(t){
    const e = eventOf(t.event);
    const skip = new Set(['hold', 'hold_ends_at', 'down_for', 'down_s', 'down_total', 'previous', 'drops', 'within',
      'duration', 'crashes', 'highlights', 'report', 'chapters', 'line', 'file',
      // Twitch: amounts and free text are for If steps, not filters.
      'months', 'streak', 'message', 'count', 'total', 'bits', 'viewers', 'cost', 'input', 'seconds']);
    return e ? e.vars.filter(v => !skip.has(v.name)) : [];
  },
  sendValue(h){
    const first = (h.steps || [])[0];
    if (first && first.type === 'wait_delay') return {id:'aired', label:'once viewers see it'};
    if (first && first.type === 'wait'){
      const ms = parseInt(first.params.ms, 10) || 0;
      return {id:'w' + ms, label:`after ${fmtMs(ms)}`};
    }
    return {id:'now', label:'right away'};
  },
  popHtml(step, h){
    if (this.chip === '__test') return testPanelHtml(this);
    const d = this.d, t = h.trigger;
    switch (this.chip){
      case 'where': {
        const cur = (allSteps(d).find(s => s.type === 'discord') || {params:{}}).params.connection;
        return `<div class="ic-label">Post in</div><div class="ig-pick">
          ${S.data.connections.discord.map(c => `<button class="dest-starter${c.id === cur ? ' on' : ''}" data-act="set-where" data-v="${esc(c.id)}" aria-pressed="${c.id === cur}">
            <span class="dest-starter-name">#${esc(c.name)}</span><span class="dest-starter-note mono">${esc(c.hint)}</span></button>`).join('')}
          <button class="dest-add ig-pick-add" data-act="conn" data-tab="discord">${svg('plus')}Add a Discord channel</button></div>`;
      }
      case 'tiers': {
        const chain = alertTiers(h);
        if (!chain) return '';
        const unit = tierUnit(chain.name);
        const rows = chain.tiers.map((t, n) => {
          const on = n === this.tier;
          const what = n ? `<span>From</span><input class="ic-input mono ig-tier-from" data-bind="tier-from" data-n="${n}" inputmode="numeric"
              value="${t.from}" aria-label="From how many ${unit}"><span>${unit}</span>` : `<span>${esc(tierLabel(chain, 0))}</span>`;
          return `<div class="ig-tier${on ? ' on' : ''}" data-key="tier-${n}"><span class="ig-tier-what">${what}</span>
            <span class="ig-tier-look muted">${esc(designLabel(t.step.params.design))}</span>
            <button class="ic-btn ${on ? 'ic-btn-primary' : 'ic-btn-ghost'} ig-small" data-act="tier-pick" data-n="${n}" aria-pressed="${on}">${on ? 'Editing' : 'Edit'}</button>
            ${n ? `<button class="ic-btn-tiny" data-act="tier-del" data-n="${n}" aria-label="Remove the one ${esc(lowerFirst(tierLabel(chain, n)))}">${svg('x')}</button>` : ''}</div>`;
        }).join('');
        const add = chain.tiers.length < MAX_TIERS
          ? `<button class="dest-add ig-add-row" data-act="tier-add" data-key="tier-add"><span class="dest-add-icon">${svg('plus')}</span>Add a level</button>` : '';
        return `<div class="ic-label">Levels</div><div class="ig-tiers" data-flip>${rows}${add}</div>
          <div class="muted">A level is a second alert for big amounts: from that many ${unit} up, it plays instead of the normal one, with its own design, sound and words. <b>Edit</b> picks which one the preview and the options above change.</div>`;
      }
      case 'adesign': return `<div class="ic-label">Design</div>${designTilesHtml(step.params.design)}
          <div class="muted">Made and changed in the Studio, on the Overlay tab's <button class="ig-link" data-act="alerts-open">Alerts page</button>.</div>${this.everyAlertHtml(['design'])}`;
      case 'acolor': {
        return `<div class="ic-label">Accent</div>${accentPickerHtml(step)}
          <div class="muted">The first one comes from the design, and so does the rest of the look: <button class="ig-link" data-act="chip" data-chip="adesign">${esc(designLabel(step.params.design))}</button>.</div>${this.everyAlertHtml(['color'])}`;
      }
      case 'asound': {
        return `<div class="ic-label">Sound</div>${soundControlsHtml(step)}
          <div class="muted">Plays with the alert, through OBS, so viewers hear it too.</div>${this.everyAlertHtml(['sound', 'volume'])}`;
      }
      case 'apic':
        return `<div class="ic-label">Picture</div>${picturePickerHtml(step.params.image || '', 'pick-picture')}
          <div class="muted">A PNG, JPG or GIF (it plays). The design decides where it goes.</div>${this.everyAlertHtml(['image'])}`;
      case 'atime':
        return `<div class="ic-label">On screen for</div>${segHtml(ALERT_SECONDS.map(n => [n, n + ' s']), step.params.seconds || '6', 'set-seconds')}
          <div class="muted">Alerts that come in together wait their turn, so none cuts another short.</div>${this.everyAlertHtml(['seconds'])}`;
      case 'look':
        return `<div class="ic-label">Look</div>${segHtml([['', 'Plain message'], ['card', 'Card']], step.params.style || '', 'set-style')}
          <div class="muted">A card has a colored edge, a title, fields side by side and a timestamp. Write on it right in the preview.</div>`;
      case 'msg': {
        const edit = step.params.edit || '';
        const key = step.params.key || '';
        const perPlatform = /\{destination\}/.test(key);
        const canSplit = varsFor(h).some(v => v.name === 'destination');
        return `<div class="ic-label">Each time it runs</div>${segHtml([['', 'Post a new message'], ['last', 'Update the same message'], ['close', 'Update it, then start fresh']], edit, 'set-edit')}
          <div class="muted">${{
            last:'Edits the message it posted there before, so one alert goes from "down" to "back" instead of piling up. If someone deleted it, a new one goes out.',
            close:'Edits the message it posted before into its final state, then lets it go: the next alert starts a new message. With nothing to finish (a short blip never got a message), it stays quiet.',
          }[edit] || 'Every run posts its own message.'}</div>
          ${edit && canSplit ? `<label class="crash-check"><input type="checkbox" data-bind="key-split" ${perPlatform ? 'checked' : ''}><span>
            <span class="crash-check-title">A separate message for each platform</span>
            <span class="muted">YouTube and Kick each get their own message instead of sharing one.</span></span></label>` : ''}
          ${edit ? `<details class="ig-more-fields"${key && !perPlatform && key !== '{destination}' ? ' open' : ''}><summary>Message group (advanced)</summary>
            <div class="dff ig-narrow"><input class="ic-input mono" data-bind="key" value="${esc(key)}" placeholder="none" spellcheck="false" aria-label="Message group"></div>
            <div class="muted">Moments with the same group update the same message: in the ready-made alerts, "is down" and "is back" share one. Change it on both, or they stop finding each other.</div></details>` : ''}`;
      }
      case 'action': {
        const a = firstStep(h, 'delay_action');
        const secs = a.params.seconds || '';
        const field = /\{/.test(secs)
          ? `<input class="ic-input mono" data-bind="seconds" value="${esc(secs)}" placeholder="30">`
          : durInput('seconds-dur', (parseFloat(secs) || 30) * 1000, 's');
        return `<div class="ic-label">Does</div>${segHtml(['arm', 'cut_after', 'cut', 'activate', 'disarm'].map(x => [x, DELAY_ACTIONS[x]]), a.params.action, 'set-action')}
          ${a.params.action === 'arm' ? `<div class="dff ig-narrow"><label>Delay</label>${field}</div>` : ''}
          <div class="muted">${{
            arm:'Arms this delay, or changes it while one is armed or on air. It goes on air once it\'s ready if "Activate as soon as it\'s ready" is on; otherwise press Activate.',
            cut_after:'Goes back to live once viewers have caught up with you, so they miss nothing. It takes as long as the delay. If a cut is already waiting, it keeps that one.',
            cut:'Goes back to live right away. Viewers skip what was still in the delay.',
            activate:'Puts on air the delay that finished getting ready.',
            disarm:'No delay at all, and none getting ready.',
          }[a.params.action] || ''} Tests never touch the stream.</div>`;
      }
      case 'detect': return rulesHtml(t, '');
      case 'scene': return sceneHtml(t, '', this.d);
      case 'tune': {
        const tune = tuneSummary(t);
        const amount = (eventOf(t.event).params || []).some(p => p.name.startsWith('min_'));
        return `<div class="ic-label">${esc(tune.k)}</div>${paramsHtml(t, '')}<div class="muted">${amount
          ? 'Smaller ones skip this moment. 1 is any amount.'
          : 'Each moment keeps its own numbers: a phone push can wait longer than the Discord card.'}</div>`;
      }
      case 'key': return shortcutHtml(t);
      case 'quiet': return quietHtml(d);
      case 'ping':
        return `<div class="ic-label">Ping</div>${segHtml([['', 'Nobody'], ['here', '@here'], ['everyone', '@everyone']], step.params.ping || '', 'set-ping')}
          <div class="muted">${step.params.edit === 'last' ? 'Only when it posts a new message: an update never pings. ' : ''}Text from chat or a web service can never ping anyone: only this option can.</div>`;
      case 'send':
        return `<div class="ic-label">When to send</div>${segHtml([['now', 'Right away'], ['w10000', '10 s'], ['w20000', '20 s'], ['w30000', '30 s'], ['w60000', '1 min'], ['aired', 'Once viewers see it']],
          this.sendValue(h).id, 'set-send')}
          <div class="muted">${t.type === 'event' && t.event === 'hold_opened' ? 'Waiting a bit means a 3-second blip never reaches anyone.' : '"Once viewers see it" waits out your delay, so the moment has reached them.'}</div>`;
      case 'only':
        return `<div class="ic-label">Only if</div><div class="dfg">${this.filterVars(t).map(v => {
          const cur = (t.filters || {})[v.name] || '';
          const listed = filterChoices(t.event, v.name);
          const choices = listed && (!cur || listed.includes(cur)) ? listed : null;
          const input = choices
            ? `<select class="ic-input" data-bind="filter" data-name="${esc(v.name)}"><option value="">any</option>${choices.map(c => `<option ${c === cur ? 'selected' : ''}>${esc(c)}</option>`).join('')}</select>`
            : `<input class="ic-input" data-bind="filter" data-name="${esc(v.name)}" value="${esc(cur)}" placeholder="any, like ${esc(v.sample)}">`;
          return `<div class="dff"><label>${esc(capFirst(filterName(v.name)))}</label>${input}</div>`;
        }).join('')}</div>
        <div class="muted">Several at once: YouTube, Kick. A * matches anything: *timed out*. Capitals don't matter.</div>`;
      case 'cmd':
        return `${commandClashHtml(d)}<div class="dfg"><div class="dff"><label>Command</label><input class="ic-input mono" data-bind="command" value="${esc(t.command)}" spellcheck="false"></div>
          <div class="dff"><label>Also answers to</label><input class="ic-input mono" data-bind="aliases" value="${esc((t.aliases || []).join(' '))}" placeholder="!retraso !d" spellcheck="false"></div></div>
          <div class="dff"><label>Each viewer, at most once every</label>${segHtml([['0', 'no limit'], ['10000', '10 s'], ['30000', '30 s'], ['60000', '1 min']], String(t.user_cooldown_ms || 0), 'set-ucd')}</div>`;
      case 'who':
        return `<div class="ic-label">Who can use it</div>${rolesHtml(t, 'role')}<div class="muted">You can always use your own commands.</div>`;
      case 'reply':
        return `<div class="ic-label">Reply as</div>${segHtml([['', 'Your account'], ['bot', 'Bot account']], step.params.as || '', 'set-as')}
          ${t.type.startsWith('chat') ? `<label class="crash-check"><input type="checkbox" data-bind="reply" ${step.params.reply === 'yes' ? 'checked' : ''}><span><span class="crash-check-title">Reply in the viewer's thread</span><span class="muted">Twitch shows it as an answer to their message</span></span></label>` : ''}
          ${S.data.twitch.bot.login ? '' : '<div class="muted">No bot account yet: <a href="#" data-act="conn" data-tab="twitch">connect one</a>, or messages go out as you.</div>'}`;
      case 'light': {
        const light = firstStep(h, 'light');
        const web = d.preset === 'on_air_light' ? `<details class="ig-more-fields"><summary>Home Assistant or Philips Hue instead</summary>
          <div class="muted">They take a web request rather than a scan.</div>
          <div class="ig-starts">${(S.data.catalog.light_starts || []).filter(x => x.id !== 'wled').map(x => `<button class="ig-start" data-act="light-to-web" data-v="${esc(x.id)}">${esc(x.label)}</button>`).join('')}</div></details>` : '';
        return lightPickerHtml(light ? light.params : {}) + web;
      }
      case 'dgroup':
        return `<div class="dff ig-narrow"><label>Group</label><input class="ic-input" data-bind="dgroup" value="${esc(step.params.group || '')}" placeholder="Crashes" spellcheck="false"></div>
          <div class="muted">The dock's Events show one group at a time: lines with the same group go together.</div>`;
      case 'priority':
        return `<div class="ic-label">Priority</div>${segHtml([['', 'Normal'], ['high', 'High'], ['urgent', 'Urgent']], step.params.priority || '', 'set-priority')}`;
      case 'url': {
        const http = firstStep(h, 'http'), chatty = t.type.startsWith('chat');
        if (d.preset === 'on_air_light') return `<button class="ic-btn ic-btn-ghost ig-small" data-act="light-to-net">${svg('bulb', ' class="ig-btn-ic"')}Use a light on my network instead (no hub)</button>` + this.lightHtml(http, t);
        return `${chatty && d.preset === 'api_command' ? this.startsHtml(http) : ''}<div class="dfg">${secretField('e-url', `<input class="ic-input mono ig-secret" data-bind="url" value="${esc(http.params.url || '')}" placeholder="${chatty ? 'https://… or $(urlfetch https://…)' : 'https://…'}" spellcheck="false" autocomplete="off">`,
            chatty ? 'Address, or a bot command to convert' : 'Address')}
          <div class="dff"><label>Method</label><select class="ic-input" data-bind="method">${['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map(m => `<option ${m === (http.params.method || 'POST') ? 'selected' : ''}>${m}</option>`).join('')}</select></div></div>
          ${chatty ? '<div class="muted">Paste a command from Nightbot, StreamElements, Fossabot or Streamlabs: the address, the reply and the command name fill in for you.</div>' : ''}
          ${this.convertedHtml()}
          ${tryHtml(http, h, chatty ? 'Use in the reply' : '')}`;
      }
      case 'file':
        return `<div class="dfg"><div class="dff"><label>File</label><input class="ic-input mono" data-bind="path" value="${esc(step.params.path || '')}" placeholder="C:\\Stream\\crashes.txt" spellcheck="false"></div>
          <div class="dff"><label>Each time</label><select class="ic-input" data-bind="mode"><option value="" ${step.params.mode !== 'append' ? 'selected' : ''}>Replace the text</option><option value="append" ${step.params.mode === 'append' ? 'selected' : ''}>Add a line</option></select></div></div>
          <div class="muted">Point an OBS text source at this file to show it on stream.</div>`;
      case 'cooldown':
        return `<div class="ic-label">Repeat at most</div>${segHtml([['0', 'Anytime'], ['10000', 'Every 10 s'], ['30000', '30 s'], ['60000', '1 min'], ['300000', '5 min'], ['600000', '10 min']], String(d.cooldown_ms || 0), 'set-cooldown')}
          <div class="dff ig-narrow"><label>Or once every</label>${durInput('cooldown-dur', d.cooldown_ms, 's')}</div>
          <div class="muted">For every moment here, each counting on its own. A button pressed too soon shows as skipped in Recent activity.</div>`;
    }
    return '';
  },
  // The on-air light's device and address. Picking a device fills in
  // what each state sends it; the address is the user's own.
  lightHtml(http, t){
    const starts = S.data.catalog.light_starts || [];
    const device = lightStart(http, t);
    const help = {
      home_assistant:'In Home Assistant: Settings › Automations › Create, add a Webhook trigger with the id instantclone-onair. InstantClone sends live, delay, crash or off; use {{ trigger.json.state }} there to choose the color.',
      wled:'Your WLED light\'s address. The WLED app shows it (Config › WiFi Setup).',
      hue:'Your Hue bridge\'s address, a username from the bridge, and the light\'s number. The Hue app shows the bridge\'s IP.',
    };
    const example = {home_assistant:'http://homeassistant.local:8123/api/webhook/instantclone-onair', wled:'http://192.168.1.50/json/state',
      hue:'http://192.168.1.20/api/USERNAME/lights/1/state'};
    return `<div class="ic-label">Your light</div><div class="ig-starts">${starts.map(x =>
        `<button class="ig-start${device && device.id === x.id ? ' on' : ''}" data-act="light-start" data-v="${esc(x.id)}" aria-pressed="${!!device && device.id === x.id}">${esc(x.label)}</button>`).join('')}</div>
      ${secretField('e-light-url', `<input class="ic-input mono ig-secret" data-bind="url" value="${esc(http.params.url || '')}"
        placeholder="${esc(device ? example[device.id] || '' : 'Pick your light first')}" spellcheck="false" autocomplete="off">`, 'Its address')}
      <div class="muted">${device ? help[device.id] || '' : 'Pick your light: InstantClone fills in what each state sends it.'} Then <b>Send a test</b>, set to <b>Send it for real</b>, lights it up for the moment picked above.</div>
      <details class="ig-more-fields"><summary>Advanced: method</summary><div class="dff ig-narrow"><select class="ic-input" data-bind="method" aria-label="Method">
        ${['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map(m => `<option ${m === (http.params.method || 'POST') ? 'selected' : ''}>${m}</option>`).join('')}</select></div>
        <div class="muted">What each state sends is in the preview: pick a moment (live, delayed…) and open "What it sends".</div></details>`;
  },
  input(el){
    const h = this.handler(), step = this.stepOf(h), t = h.trigger;
    const b = el.dataset.bind;
    if (b === 'name') this.d.name = el.value;
    else if (b === 'enabled') this.d.enabled = el.checked;
    else if (b === 'text' && step){ step.params[KINDS[step.type].text] = el.value; autosize(el); }
    else if (b === 'title' && step) step.params.title = el.value;
    else if (b === 'message' && step) step.params.message = el.value;
    else if (b === 'dgroup' && step) step.params.group = el.value;
    else if (b === 'tier-from'){ if (!this.tierFrom(h, el)) return; }
    else if (b === 'filter'){ t.filters = t.filters || {}; t.filters[el.dataset.name] = el.value.trim(); }
    else if (b === 'command') t.command = el.value.trim().toLowerCase();
    else if (b === 'aliases') t.aliases = el.value.split(/[\s,]+/).map(x => x.trim().toLowerCase()).filter(Boolean);
    else if (b === 'role'){ t.roles = t.roles || {}; t.roles[el.dataset.name] = el.checked; }
    else if (b === 'reply' && step) step.params.reply = el.checked ? 'yes' : '';
    else if (b === 'url' && t.type.startsWith('chat') && BOT_SYNTAX.test(el.value)){
      // A pasted bot command: converted once typing pauses, not saved raw.
      this.convertSoon(el.value);
      return;
    }
    else if (b === 'url' || b === 'method'){
      // A catalog webhook or light sends to one address: set them all.
      allSteps(this.d).filter(s => s.type === 'http').forEach(s => { s.params[b] = el.value; });
      if (b === 'url') this.converted = null;
    }
    else if (b === 'try'){
      // Values to try the request with; not part of the integration.
      tryEntry(firstStep(h, 'http')).inputs[el.dataset.name] = el.value;
      return;
    }
    else if (b === 'test-value'){ (this.testValues = this.testValues || {})[el.dataset.name] = el.value; return; }
    else if ((b === 'path' || b === 'mode') && step) step.params[b] = el.value;
    else if (/^l-(action|color|brightness)$/.test(b) && step) step.params[b.slice(2)] = el.value.trim();
    else if ((b === 'above' || b === 'footer' || b === 'key' || b === 'color') && step) step.params[b] = el.value;
    else if (b === 'key-split' && step) splitKey(step, el.checked);
    else if (b === 'volume' && step) step.params.volume = el.value.trim();
    else if (b === 'image-link' && step) step.params.image = el.value.trim();
    else if (b === 'sound-link' && step) step.params.sound = el.value.trim();
    else if ((b === 'f-name' || b === 'f-value') && step){
      this.fields(step, list => {
        const f = list[+el.dataset.n];
        if (f) f[b === 'f-name' ? 'name' : 'value'] = el.value;
      });
    }
    else if (b === 'param'){ if (!paramInput(t, el)) return; }
    else if (b === 'seconds' && firstStep(h, 'delay_action')) firstStep(h, 'delay_action').params.seconds = el.value.trim();
    else if (b === 'seconds-dur' && firstStep(h, 'delay_action')){
      // Up to an hour, the backend's config::MAX_DELAY_MS.
      const ms = durValue(el, 1000, 3600000);
      if (ms == null) return;
      firstStep(h, 'delay_action').params.seconds = String(ms / 1000);
    }
    else if (b === 'cooldown-dur'){
      const ms = durValue(el, 0, 86400000);
      if (ms == null) return;
      this.d.cooldown_ms = ms;
    }
    else if (!rulesInput(t, el, '') && !sceneInput(t, el, '') && !onlyLiveInput(t, el, '') && !quietInput(this.d, el)) return;
    this.changed(fieldOf(el));
  },
  /** After any change; `field` is the box typed in, if that was it. */
  changed(field){
    this.dirty = true;
    this.edits = (this.edits || 0) + 1;
    syncSceneRules(this.d);
    historyNote(this, field);
    this.render();
  },
  // Point every state of the on-air light at one device, with what that
  // device needs to hear for each state. An address the user typed stays.
  useLight(id){
    const starts = S.data.catalog.light_starts || [];
    const start = starts.find(x => x.id === id);
    if (!start) return;
    this.d.handlers.forEach(h => {
      const state = (h.trigger.filters || {}).state, http = firstStep(h, 'http');
      if (!http) return;
      http.params.method = start.method;
      const url = (http.params.url || '').trim();
      if (!url || starts.some(x => x.url === url)) http.params.url = start.url;
      if (start.bodies[state]) http.params.body = start.bodies[state];
    });
    this.changed();
  },
  // Whether it still shows `step`: an upload or a design picked a moment
  // ago lands only there.
  editing(step){ return Modal.kind === 'editor' && this.stepOf(this.handler()) === step; },
  act(a, el){
    const h = this.handler(), step = this.stepOf(h), v = el.dataset.v;
    if (rulesAct(h.trigger, a, el) || sceneAct(h.trigger, a, el)){ this.changed(); return true; }
    if (step && alertAct(a, el, step, h, this)) return true;
    switch (a){
      case 'set-style': if (step) step.params.style = v; this.changed(); return true;
      case 'set-ts': if (step) step.params.timestamp = step.params.timestamp === 'yes' ? '' : 'yes'; this.changed(); return true;
      case 'tier-pick':
        this.tier = +el.dataset.n;
        this.render();
        return true;
      case 'tier-add': {
        const chain = alertTiers(h);
        // One more is a check on the amount and its own alert.
        if (!chain || chain.tiers.length >= MAX_TIERS || !stepsFit(this.d, 2)) return true;
        const last = chain.tiers[chain.tiers.length - 1];
        // A copy of the biggest so far, from a bigger amount: then its own
        // design, sound and words.
        chain.tiers.push({from:last.from ? last.from * 5 : TIER_FIRST[chain.name], step:clone(last.step)});
        setTiers(h, chain);
        this.tier = chain.tiers.length - 1;
        this.changed();
        return true;
      }
      case 'tier-del': {
        const chain = alertTiers(h), n = +el.dataset.n;
        if (!chain || !n) return true;
        chain.tiers.splice(n, 1);
        setTiers(h, chain);
        if (this.tier >= n) this.tier = Math.max(0, this.tier - 1);
        this.changed();
        return true;
      }
      case 'alert-all': {
        if (!step) return true;
        const names = el.dataset.names.split(' ');
        let n = 0;
        this.d.handlers.forEach(x => {
          const s = primaryStep(x);
          if (!s || s === step || s.type !== 'overlay') return;
          names.forEach(name => { s.params[name] = step.params[name] || ''; });
          n++;
        });
        this.changed();
        toast(`Done for ${plural(n, 'other alert')}. Save to keep it.`, 'ok');
        return true;
      }

      case 'f-add': {
        if (!step) return true;
        this.fields(step, list => list.push({name:'', value:'', inline:true}));
        this.changed();
        const names = Modal.form.querySelectorAll('[data-bind="f-name"]');
        if (names.length) names[names.length - 1].focus();
        return true;
      }
      case 'f-del': if (step) this.fields(step, list => list.splice(+el.dataset.n, 1)); this.changed(); return true;
      case 'f-inline':
        if (step) this.fields(step, list => { const f = list[+el.dataset.n]; if (f) f.inline = !f.inline; });
        this.changed();
        return true;
      case 'light-start': this.useLight(v); return true;
      // One light for every state: the on-air light is one device.
      case 'light-use': allSteps(this.d).filter(s => s.type === 'light').forEach(s => useDevice(s, el)); this.changed(); return true;
      case 'l-swatch': if (step) step.params.color = v; this.changed(); return true;
      case 'light-to-net': onAirToNetwork(this.d); this.chip = 'light'; this.changed(); return true;
      case 'light-to-web': onAirToWeb(this.d, v); this.chip = 'url'; this.changed(); return true;
      case 'set-action': {
        const a = firstStep(h, 'delay_action');
        if (a){
          a.params.action = v;
          if (v === 'arm' && !a.params.seconds) a.params.seconds = '30';
        }
        this.changed();
        return true;
      }
      case 'moment':
        this.sel = +el.dataset.n;
        this.tier = 0;
        this.testValues = {};
        this.run = null;
        if (this.chip === '__test') this.chip = null;
        this.render();
        return true;
      case 'moment-toggle': { const x = this.d.handlers[+el.dataset.n]; x.enabled = !x.enabled; this.changed(); return true; }
      // Gone at once, with Undo on the toast (the X is too small to ask on).
      case 'moment-del': {
        const d = this.d, at = +el.dataset.n;
        if (d.handlers.length < 2) return true;
        const label = momentLabels(d.handlers)[at];
        const [gone] = d.handlers.splice(at, 1);
        const pick = n => { this.sel = n; this.tier = 0; this.testValues = {}; this.run = null; this.chip = null; };
        pick(Math.min(this.sel > at ? this.sel - 1 : this.sel, d.handlers.length - 1));
        this.changed();
        undoToast(`Removed "${label}"`, () => {
          if (Modal.kind !== 'editor' || this.d !== d) return;
          d.handlers.splice(Math.min(at, d.handlers.length), 0, gone);
          pick(d.handlers.indexOf(gone));
          this.changed();
        });
        return true;
      }
      case 'chip':
        this.chip = this.chip === el.dataset.chip ? null : el.dataset.chip;
        this.render();
        if (this.chip) revealOpened(q('.ig-opt-pop'));
        return true;
      case 'set-where': allSteps(this.d).filter(s => s.type === 'discord').forEach(s => { s.params.connection = v; }); this.changed(); return true;
      case 'set-ping': if (step) step.params.ping = v; this.changed(); return true;
      case 'set-edit': if (step) step.params.edit = v; this.changed(); return true;
      case 'ed-start': this.useStart(v); return true;
      case 'set-send': setSend(h, v); this.changed(); return true;
      case 'set-ucd': h.trigger.user_cooldown_ms = +v; this.changed(); return true;
      case 'set-as': if (step) step.params.as = v; this.changed(); return true;
      case 'set-priority': if (step) step.params.priority = v; this.changed(); return true;
      case 'set-cooldown': this.d.cooldown_ms = +v; this.changed(); return true;
      case 'ed-builder':
        Builder.open(this.d, {dirty:this.dirty, back:{title:'Back to the simple editor', keepsEdits:true,
          go:() => Editor.fromBuilder(Builder.d, Builder.dirty)}});
        return true;
      case 'ed-test': this.chip = '__test'; this.render(); focusAct('test-run'); return true;
      case 'test-run': busy(el, () => this.test()); return true;
      case 'test-mode': this.testDry = v === 'dry'; this.render(); return true;
      // Undo brings it back, so no "are you sure" first.
      case 'delete': busy(el, () => remove(this.d.id)); return true;
      case 'save': {
        const at = this.edits;
        busy(el, () => saveIntegration(this.d, () => { if (this.edits === at) this.dirty = false; }));
        return true;
      }
      case 'b-try':
        busy(el, async () => {
          await tryRequest(firstStep(h, 'http'), h);
          if (Modal.kind === 'editor') this.render();
        });
        return true;
      case 'b-pick':
        tryEntry(firstStep(h, 'http')).picked = {token:el.dataset.token, sample:el.dataset.sample || ''};
        this.render();
        return true;
      case 'b-use-reply': this.useInReply(); return true;
    }
    return false;
  },
  async test(){
    this.chip = '__test';
    this.run = {running:true};
    this.render();
    const r = await api('/integrations/test', {integration:this.d, handler:this.sel, values:testValues(this), dry:!!this.testDry});
    this.run = r.ok ? r : {error:r.error || 'The test failed'};
    if (Modal.kind === 'editor') this.render();
  },
};

// A test's panel, the same in both editors: how it runs (for real or
// dry), the values it runs with, and what each step did.
function testPanelHtml(o){
  const h = o.handler();
  const own = varsFor(h).filter(v => v.group === 'trigger');
  const values = o.testValues || {};
  const mode = `<div class="sub-tabs ig-seg">${[['dry', 'Dry run'], ['', 'Send it for real']].map(([id, label]) => {
    const on = (id === 'dry') === !!o.testDry;
    return `<button class="sub-tab${on ? ' on' : ''}" data-act="test-mode" data-v="${id}" aria-pressed="${on}">${label}</button>`;
  }).join('')}${SEG_IND}</div>`;
  const inputs = own.length ? `<details class="ig-more-fields"${Object.keys(values).length ? ' open' : ''}><summary>Test with other values</summary>
    <div class="ig-test-values">${own.map(v => `<div class="dff"><label title="{${esc(v.name)}}">${esc(capFirst(varLabel(v.name)))}</label>
      <input class="ic-input" data-bind="test-value" data-name="${esc(v.name)}" value="${esc(v.name in values ? values[v.name] : v.sample)}"></div>`).join('')}</div></details>` : '';
  const run = o.run;
  const sent = run && (run.steps || []).some(st => st.status !== 'skipped');
  const verdict = run && (run.status === 'ok' ? (sent ? 'everything worked' : 'nothing was sent, see why below')
    : {failed:'a step failed', stopped:'stopped early'}[run.status] || run.status);
  const result = !run ? '' : run.running ? '<div class="ig-running"><span class="ig-spin"></span>Running the test…</div>'
    : run.error ? warnHtml(esc(run.error))
    : `<div class="ic-label">Result: ${esc(verdict)}</div>${runLogHtml(run.steps)}`;
  const note = o.testDry ? 'A dry run sends nothing at all: each step says what it would have sent.'
    : 'Messages go out for real, marked [TEST], and never ping anyone. Web requests go out too: a light changes and stays that way. Waits are skipped; the stream, the VOD, OBS, programs and files are left alone.';
  return `<div class="ig-testbox"><div class="ig-test-head">${mode}
    <button class="ic-btn ic-btn-primary ig-small" data-act="test-run">${svg('play', ' class="ig-btn-ic"')}${run && !run.running ? 'Run again' : 'Run the test'}</button></div>
    ${inputs}<div class="ig-test-result" aria-live="polite">${result}</div><div class="muted">${note}</div></div>`;
}
// What a test runs with: every value the panel shows, edited or not.
function testValues(o){
  const values = {};
  varsFor(o.handler()).filter(v => v.group === 'trigger').forEach(v => { values[v.name] = v.sample; });
  return Object.assign(values, o.testValues || {});
}

// Another switched-on integration answering one of this one's chat
// commands, as {word, name}, or null: both would reply. Mirrors
// api::command_taken.
function commandClash(d){
  const words = i => (i.handlers || []).filter(h => h.enabled && h.trigger.type === 'chat_command')
    .flatMap(h => [h.trigger.command].concat(h.trigger.aliases || []));
  const mine = words(d);
  if (!mine.length) return null;
  for (const other of S.data.integrations){
    if (!other.enabled || other.id === d.id) continue;
    const word = mine.find(w => words(other).includes(w));
    if (word) return {word, name:other.name};
  }
  return null;
}
function commandClashHtml(d){
  const c = commandClash(d);
  return c ? warnHtml(`<b translate="no">${esc(c.name)}</b> already answers ${esc(c.word)}. This one stays off until you pick another command or switch that one off.`) : '';
}

// What a moment still misses before it can run, or ''.
function handlerMissing(h){
  const t = h.trigger;
  if (t.type === 'scene' && !t.scene) return 'pick the scene';
  if (t.type === 'shortcut' && !t.hotkey && !t.midi && !t.token) return 'set a key, pad or Stream Deck';
  let missing = '';
  const look = steps => (steps || []).forEach(s => {
    if (missing || s.enabled === false) return;
    missing = stepMissing(s);
    look(s.then);
    look(s.else);
  });
  look(h.steps);
  return missing;
}
// What a step still misses, or ''.
function stepMissing(s){
  const p = s.params || {};
  if (s.type === 'discord' && !S.data.connections.discord.some(c => c.id === p.connection)) return 'pick a Discord channel';
  if (s.type === 'http' && !(p.url || '').trim()) return 'set the address';
  if (s.type === 'http' && /BRIDGE-IP|USERNAME/.test(p.url || '')) return 'replace BRIDGE-IP and USERNAME with your bridge\'s';
  if (s.type === 'phone' && !S.data.connections.phone.topic) return 'connect your phone';
  if (s.type === 'discord' && p.style === 'card' && !['title', 'text', 'fields', 'image', 'above'].some(k => (p[k] || '').trim())) return 'give the card a title or some text';
  if (s.type === 'if' && conditionRows(s).some(r => !r.left.trim())) return 'say what it checks';
  if ((s.type === 'set_var' || s.type === 'counter') && !(p.name || '').trim()) return 'name the value';
  if (s.type === 'viewer' && !/^[A-Za-z0-9_]{1,32}$/.test((p.value || '').trim())) return 'name the value: letters, digits and _';
  if (s.type === 'call' && !(p.block || '').trim()) return 'pick the block';
  if (s.type === 'reward' && !(p.title || '').trim()) return 'pick the reward';
  if (s.type === 'goal' && !(p.id || '').trim()) return p.what === 'timer' ? 'pick the timer' : 'pick the goal';
  if (s.type === 'channel' && !(p.title || '').trim() && !(p.category || '').trim()) return 'write the new title or category';
  if (s.type === 'wait' && !/\{|^\d+$/.test((p.ms || '').trim())) return 'set how long to wait';
  if ([p.scene, p.source, p.filter, p.hotkey, s.type === 'light' ? p.device : '', s.type === 'program' || s.type === 'file' ? p.path : ''].some(v => /\{/.test(v || ''))) return 'names here can\'t use values';
  if (s.type === 'light'){
    if (!(p.device || '').trim()) return 'pick the light or plug';
    if (['color', 'flash'].includes(p.action) && !(p.color || '').trim()) return 'pick a color';
  }
  if ((s.type === 'program' || s.type === 'file') && !(p.path || '').trim()) return s.type === 'file' ? 'pick the file' : 'pick the program';
  if (s.type === 'obs'){
    const action = OBS_ACTIONS[p.action];
    if (!action) return 'pick what to do in OBS';
    const need = action[1];
    if (need === 'filter' && !(p.source || '').trim()) return 'pick the source or scene';
    if (need && !(p[need] || '').trim()) return {scene:'pick the scene', source:'pick the source', filter:'pick the filter', hotkey:'pick the hotkey'}[need];
  }
  if ((s.type === 'chat' || s.type === 'phone' || s.type === 'overlay' || s.type === 'timeline' || s.type === 'dock') && !(p.text || '').trim()) return 'write what it says';
  if (s.type === 'discord' && p.style !== 'card' && !(p.text || '').trim()) return 'write the message';
  return '';
}

// "A separate message for each platform": the message group follows the
// platform, or (unticked) goes. What it was is kept, so ticking it again
// brings back "flap {destination}" rather than a plain "{destination}"
// that would share the "is down" card's message.
const KEYS_WERE = new Map();
function splitKey(step, on){
  const id = uidOf(step);
  if (on) step.params.key = KEYS_WERE.get(id) || '{destination}';
  else { KEYS_WERE.set(id, step.params.key || ''); step.params.key = ''; }
}

// Scene delay: the "any other scene" rule never counts the game scene,
// whatever the game scene is called.
function syncSceneRules(d){
  if (d.preset !== 'scene_delay') return;
  const arm = d.handlers.find(h => h.trigger.type === 'scene' && (firstStep(h, 'delay_action') || {params:{}}).params.action === 'arm');
  if (!arm) return;
  d.handlers.forEach(h => {
    if (h !== arm && h.trigger.type === 'scene' && h.trigger.scene === '*') h.trigger.except = arm.trigger.scene || '';
  });
}

