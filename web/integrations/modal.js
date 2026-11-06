// ---------------------------------------------------------------- modal shell

// One modal shell at a time, built on the destination editor's. Opening
// another while one is up swaps the content in place (no second backdrop).
// Escape, the backdrop and ✕ close it; `onClose` lets an editor hold the
// close to ask about unsaved changes, `after` runs once it really closes.
const Modal = {
  el:null, form:null, kind:null, onClose:null, after:null, opener:null, swapFrom:null,
  // `back`: {go, title, keepsEdits} for a view opened from another one.
  // `pending`: what the unsaved-changes bar's Discard should finish.
  back:null, pending:null,
  open(kind, width, height){
    Menu.close();
    if (this.el){
      this.swapFrom = this.form.getBoundingClientRect();
      this.hideGuard();
    } else {
      this.opener = document.activeElement;
      const wrap = document.createElement('div');
      wrap.className = 'ic-modal ig-modal';
      wrap.setAttribute('role', 'dialog');
      wrap.setAttribute('aria-modal', 'true');
      wrap.innerHTML = '<div class="ic-modal-backdrop" data-act="modal-close"></div><div class="dest-form" tabindex="-1"></div>';
      document.body.appendChild(wrap);
      wrap.addEventListener('click', onClick);
      wrap.addEventListener('input', onInput);
      wrap.addEventListener('keydown', onActivateKey);
      wrap.addEventListener('focusout', onFieldLeave);
      // The last run's steps fold away at a click anywhere else.
      wrap.addEventListener('pointerdown', e => {
        if (S.lastRunOpen && !e.target.closest('.ig-lastrun, [data-act="last-run"]')) setLastRun(false);
      });
      wrap.addEventListener('toggle', e => {
        const key = e.target.dataset && e.target.dataset.fold;
        if (key) e.target.open ? Recipes.folds.add(key) : Recipes.folds.delete(key);
      }, true);
      document.addEventListener('keydown', onModalKey);
      this.el = wrap;
      this.form = wrap.querySelector('.dest-form');
      this.form.focus({preventScroll:true});
    }
    this.kind = kind;
    this.onClose = null;
    this.after = null;
    this.back = null;
    this.pending = null;
    S.lastRunOpen = false;
    // As variables, not inline sizes, so the phone layout can override them.
    this.form.style.setProperty('--ig-w', `min(${width || 820}px,100%)`);
    this.form.style.setProperty('--ig-h', height || 'auto');
    return this.el;
  },
  body(html){
    if (!this.form) return;
    if (!this.swapFrom){ morph(this.form, html); return; }
    // A different modal in the same shell: resize to it and fade it in.
    const from = this.swapFrom;
    this.swapFrom = null;
    this.form.innerHTML = html;
    this.form.focus({preventScroll:true});
    placeIndicators(this.form);
    fillRanges(this.form);
    if (reduced()) return;
    const to = this.form.getBoundingClientRect();
    this.form.animate([{width:from.width + 'px', height:from.height + 'px'}, {width:to.width + 'px', height:to.height + 'px'}],
      {duration:280, easing:EASE});
    for (const c of this.form.children){
      c.animate([{opacity:0, transform:'translateY(4px)'}, {opacity:1, transform:'none'}],
        {duration:240, easing:EASE, delay:60, fill:'backwards'});
    }
  },
  close(force){
    if (!this.el) return;
    if (!force){
      this.pending = 'close';
      if (this.onClose && this.onClose() === false) return;
    }
    const after = this.after;
    this.after = null;
    this.onClose = null;
    this.kind = null;
    if (after){
      after();
      if (this.kind) return;   // it reopened something in this shell
    }
    const el = this.el, opener = this.opener;
    this.el = this.form = this.opener = this.swapFrom = null;
    // Shown secrets hide again: the next time could be on stream.
    S.revealed.clear();
    VarTip.hide();
    ValuePick.detach();
    Keys.stop();
    Meter.stop();
    ObsLive.stop();
    document.removeEventListener('keydown', onModalKey);
    el.classList.add('closing');
    el.inert = true;
    setTimeout(() => el.remove(), reduced() ? 0 : 180);
    if (opener && opener.isConnected && typeof opener.focus === 'function') opener.focus({preventScroll:true});
    if (!S.visible && !S.stage) clearInterval(S.pollTimer);
  },
  // Back to the view this one was opened from. Edits that travel back
  // with it skip the unsaved-changes question; with nowhere to go back to,
  // it closes (and `after` returns to the opener).
  goBack(){
    if (!this.back){ this.close(); return; }
    if (!this.back.keepsEdits){
      this.pending = 'back';
      if (this.onClose && this.onClose() === false) return;
    }
    this.leaveBack();
  },
  leaveBack(){
    const go = this.back.go;
    this.back = null;
    this.pending = null;
    go();
  },
  // The "unsaved changes" bar, over the footer.
  guard(){
    const shown = this.form.querySelector('.ig-guard');
    if (shown){ shown.querySelector('[data-act="guard-keep"]').focus(); return; }
    const bar = document.createElement('div');
    bar.className = 'ig-guard';
    bar.setAttribute('data-persist', '');
    bar.setAttribute('role', 'alert');
    const save = q('.dest-form-foot [data-act="save"]');
    bar.innerHTML = `<span class="ig-guard-text">${svg('warn')}You have unsaved changes</span>
      <button class="ic-btn ic-btn-ghost" data-act="guard-discard">Discard</button>
      <button class="ic-btn ic-btn-ghost" data-act="guard-keep">Keep editing</button>
      ${save && save.disabled ? '' : '<button class="ic-btn ic-btn-primary" data-act="guard-save">Save</button>'}`;
    this.form.appendChild(bar);
    bar.querySelector('[data-act="guard-keep"]').focus();
  },
  hideGuard(){
    const g = this.form && this.form.querySelector('.ig-guard');
    if (g) g.remove();
  },
};
function modalOpen(){ return !!Modal.el; }

function onModalKey(e){
  if (!Modal.el) return;
  if (e.key === 'Escape' && !e.defaultPrevented){
    e.preventDefault();
    // One layer at a time: the guard, then an open chip, then the modal.
    if (Modal.form.querySelector('.ig-guard')) Modal.hideGuard();
    else if (S.lastRunOpen) setLastRun(false);
    else if (Modal.kind === 'catalog' && Catalog.query) Catalog.act('cat-clear');
    else if (Modal.kind === 'editor' && Editor.closeChip()) return;
    else Modal.close();
    return;
  }
  // Undo and redo the editor's changes; a text box keeps its own.
  const editor = {editor:Editor, builder:Builder}[Modal.kind];
  const key = e.key.toLowerCase();
  if ((e.ctrlKey || e.metaKey) && editor && (key === 'z' || key === 'y') && !typesText(e.target)){
    e.preventDefault();
    historyStep(editor, key === 'z' && !e.shiftKey);
    return;
  }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's' && (Modal.kind === 'editor' || Modal.kind === 'builder')){
    e.preventDefault();
    const save = q('.dest-form-foot [data-act="save"]');
    if (save && !save.disabled) save.click();
    else if (save && localLocked()) toast(LOCAL_LOCK_TEXT, 'info', 7000);
    return;
  }
  // The catalog's search, from anywhere in it but a field.
  if (e.key === '/' && Modal.kind === 'catalog' && !e.target.matches('input, textarea, select')){
    e.preventDefault();
    $('ig-cat-search').focus();
    return;
  }
  if (e.key === 'Tab') trapFocus(e);
}

// Tab cycles inside the modal instead of wandering into the page behind.
function trapFocus(e){
  const list = [...Modal.form.querySelectorAll('button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])')]
    .filter(x => !x.disabled && x.type !== 'hidden' && x.getClientRects().length && !x.closest('[data-leaving]'));
  if (!list.length) return;
  const first = list[0], last = list[list.length - 1], at = document.activeElement;
  if (e.shiftKey && (at === first || !Modal.form.contains(at))){ e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && (at === last || !Modal.form.contains(at))){ e.preventDefault(); first.focus(); }
}

function modalHead(icon, color, title, hint, extra){
  const back = Modal.back || Modal.after
    ? `<button type="button" class="dest-form-x ig-back" data-act="modal-back" aria-label="Back" title="${esc((Modal.back && Modal.back.title) || 'Back')}">${svg('back')}</button>` : '';
  return `<div class="dest-form-head">${back}
    <span class="dest-form-mark" style="--dc:${color}">${svg(icon)}</span>
    <div class="dest-form-titles"><h4>${title}</h4><div class="dest-form-hint">${hint}</div></div>
    ${extra || ''}
    <button type="button" class="dest-form-x" data-act="modal-close" aria-label="Close">${svg('x')}</button>
  </div>`;
}
function titleInput(value){
  return `<input class="ig-title-input" data-bind="name" value="${esc(value)}" aria-label="Name" maxlength="80" spellcheck="false" autocomplete="off">`;
}
function enableSwitch(bind, on, label){
  return `<label class="dest-form-enable"><input type="checkbox" data-bind="${bind}" ${on ? 'checked' : ''}><span class="dfe-track"><span></span></span><span data-tick>${label || (on ? 'On' : 'Off (draft)')}</span></label>`;
}
// "Last ran 3 min ago · failed". With that run still in Recent activity,
// a button that opens its steps (see lastRunPanelHtml).
function lastRunHtml(d){
  const st = stat(d);
  if (!st || !st.last_ms) return '<span class="ig-last">Hasn\'t run yet</span>';
  const how = {ok:'worked', failed:'failed', stopped:'stopped early'}[st.last_status] || st.last_status;
  const cls = `ig-last ${st.last_status === 'failed' ? 'bad' : 'ok'}`;
  const text = `<i></i>Last ran ${esc(fmtAgo(st.last_ms))} · ${esc(how)}`;
  if (!lastRunOf(d.id)) return `<span class="${cls}">${text}</span>`;
  return `<button type="button" class="${cls} ig-last-btn" data-act="last-run" aria-expanded="${S.lastRunOpen}"
    title="${S.lastRunOpen ? 'Hide' : 'See'} what each step did">${text}${svg('chev')}</button>`;
}
// The newest real run of an integration in Recent activity: tests and
// skipped presses aren't runs, and the footer counts neither.
function lastRunOf(id){
  return (S.data.activity || []).find(r => r.integration_id === id && !r.test && r.status !== 'busy' && r.status !== 'cooldown') || null;
}
// The last run's steps, over the footer of the editor it belongs to.
function lastRunPanelHtml(d){
  const r = S.lastRunOpen && d.id ? lastRunOf(d.id) : null;
  if (!r) return '';
  return `<div class="ig-lastrun" role="region" aria-label="Last run">
    <div class="ig-lastrun-head"><b>Last run</b><span class="muted">${esc(fmtClock(r.at_ms))} · ${esc(r.trigger)}</span>
      <button type="button" class="ic-btn-tiny" data-act="last-run" aria-label="Close">${svg('x')}</button></div>
    ${runPanelHtml(r, false)}</div>`;
}
// A run as text, for "Copy run log" and the issue form's run log.
function runLogText(r){
  const lines = r.steps.map(s => `${((s.at_ms || 0) / 1000).toFixed(2).padStart(7)} s  ${s.status.padEnd(7)} ${s.label}${s.detail ? ': ' + s.detail : ''}`);
  return [`${r.name} · ${new Date(r.at_ms).toLocaleString()} · ${r.status}${r.test ? ' (test)' : ''}`, `When: ${r.trigger}`]
    .concat(lines.length ? lines : ['No steps ran.']).join('\n');
}
// A run log for the issue form, cut to what a link can carry.
function reportLog(r){
  const log = runLogText(r);
  return log.length > 1500 ? log.slice(0, 1500) + '\n…' : log;
}
// A run's steps with Copy, Report it and, from Recent activity, a way to
// the integration.
function runPanelHtml(r, withOpen){
  const log = runLogText(r);
  const report = reportUrl({integration:r.name, run:reportLog(r)});
  return `${runLogHtml(r.steps)}<div class="ig-run-actions">
    <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="${esc(log)}">${svg('copy', ' class="ig-btn-ic"')}Copy run log</button>
    ${withOpen && find(r.integration_id) ? `<button class="ic-btn ic-btn-ghost ig-small" data-act="edit" data-id="${esc(r.integration_id)}">${svg('pencil', ' class="ig-btn-ic"')}Open integration</button>` : ''}
    <a class="ig-run-report" href="${esc(report)}" target="_blank" rel="noreferrer">Report it</a></div>`;
}
// The integrations issue form with what is known filled in, by the form's
// own field ids (.github/ISSUE_TEMPLATE/integrations.yml).
function reportUrl(fields){
  const p = new URLSearchParams({template:'integrations.yml'});
  if (S.data && S.data.version) p.set('version', 'v' + S.data.version);
  Object.entries(fields || {}).forEach(([k, v]) => { if (v) p.set(k, v); });
  return 'https://github.com/Soulhackzlol/InstantClone/issues/new?' + p;
}
// The editors' footer: Delete away from Save, Save lit only with changes.
function footHtml(d, dirty, extra, canSave){
  const locked = localLocked() && runsLocally(d);
  return `<div class="dest-form-foot">${lastRunPanelHtml(d)}
    ${d.id ? '<button class="ic-btn ic-btn-ghost ig-del" data-act="delete">Delete</button>' : ''}
    <div class="dest-form-msg muted ig-foot-msg">${d.id ? lastRunHtml(d) : '<span class="ig-last">New integration</span>'}${locked ? `<button type="button" class="ig-foot-need ig-foot-why" data-act="lock-why">Needs a dashboard password: why?</button>`
      : dirty ? '<span class="ig-unsaved">Unsaved</span>'
      : d.id && issuesOf(d).length ? `<span class="ig-foot-need">${d.enabled ? 'Before it can run' : 'To switch it on'}: ${esc(issuesOf(d)[0])}</span>` : ''}</div>
    ${extra || ''}
    ${dirty ? `<button class="ic-btn ic-btn-ghost ig-discard" data-act="discard" title="${d.id ? 'Back to the last saved version' : 'Throw this new one away'}">Discard</button>` : ''}
    <button class="ic-btn ic-btn-primary" data-act="save" ${!locked && (dirty || canSave || !d.id) ? '' : 'disabled'} title="${locked ? esc(LOCAL_LOCK_TEXT) : 'Save (Ctrl+S)'}">Save</button>
  </div>`;
}
// Discard: back to the last saved version, staying in the editor (`ed`,
// the simple one or the builder), with Undo for the edits. A new one has
// no saved version, so it closes.
function discardEdits(ed){
  const saved = ed.d.id && find(ed.d.id);
  if (!saved){ Modal.close(true); return; }
  if (document.querySelector('.ig-rewind-badge')) return;
  const kind = Modal.kind, edits = ed.d;
  let fresh = null;
  rewind(true, () => {
    ed.d = fresh = withHandler(clone(saved));
    ed.dirty = false;
    ed.reset();
    historyStart(ed);
    ed.render();
  });
  undoToast('Changes discarded', () => {
    if (Modal.kind !== kind || !fresh || ed.d !== fresh) return;
    rewind(false, () => {
      ed.d = edits;
      ed.dirty = true;
      ed.reset();
      historyStart(ed);
      ed.render();
    });
  });
}
// Discard puts the blocks back one by one: the ones the edits added slide
// out, the ones that stay travel to where the saved version has them, and
// the ones the edits took away slide back in. Undo (`back` false) plays
// the same moves the other way. `swap` changes what is shown. A block is
// known in both versions by its kind and what it says.
function rewind(back, swap){
  const host = q('.ig-canvas');
  if (reduced() || !host){ swap(); return; }
  const sig = el => el.classList.contains('ig-when') ? 'when'
    : `${(el.querySelector('.ig-step-kind') || {}).textContent}|${(el.querySelector('.ig-step-text') || {}).textContent}`;
  const was = new Map();
  host.querySelectorAll('.ig-step').forEach(el => {
    const list = was.get(sig(el)) || [];
    list.push({rect:el.getBoundingClientRect(), copy:el.cloneNode(true)});
    was.set(sig(el), list);
  });
  const badge = document.createElement('div');
  badge.className = 'ig-rewind-badge' + (back ? '' : ' fwd');
  badge.setAttribute('aria-hidden', 'true');
  badge.innerHTML = `${svg('rewind')}<span>${back ? 'Back to saved' : 'Edits back'}</span>`;
  const box = host.getBoundingClientRect();
  Object.assign(badge.style, {left:(box.left + box.width / 2) + 'px', top:(box.top + 22) + 'px'});
  document.body.appendChild(badge);
  swap();
  host.querySelectorAll('[data-leaving]').forEach(n => n.remove());
  const plan = [...host.querySelectorAll('.ig-step')].map(el => {
    const list = was.get(sig(el));
    return {el, from:list && list.shift()};
  });
  let turn = 0;
  const next = () => Math.min(turn++, 16) * 75;
  // First out: what the other version doesn't have.
  was.forEach(list => list.forEach(({rect, copy}) => {
    copy.querySelectorAll('[data-act],[data-key],[id]').forEach(n => ['data-act', 'data-key', 'id'].forEach(a => n.removeAttribute(a)));
    ['data-act', 'data-key', 'data-path'].forEach(a => copy.removeAttribute(a));
    copy.setAttribute('aria-hidden', 'true');
    Object.assign(copy.style, {position:'fixed', left:rect.left + 'px', top:rect.top + 'px', width:rect.width + 'px', height:rect.height + 'px',
      margin:'0', zIndex:'1500', pointerEvents:'none'});
    document.body.appendChild(copy);
    copy.animate([{transform:'none', opacity:1},
      {transform:`translate(${back ? 70 : -70}px,-6px) rotate(${back ? 5 : -5}deg) scale(.9)`, opacity:0}],
      {duration:340, delay:next(), easing:'cubic-bezier(.4,0,1,1)', fill:'both'}).onfinish = () => copy.remove();
  }));
  // Then, top first, each block that moved is picked up and set down.
  plan.forEach(({el, from}) => {
    el.getAnimations().forEach(a => a.cancel());
    const to = el.getBoundingClientRect();
    if (!from){
      el.animate([{transform:`translateX(${back ? -50 : 50}px) scale(.92)`, opacity:0}, {transform:'none', opacity:1}],
        {duration:380, delay:next(), easing:'cubic-bezier(.2,.9,.3,1.2)', fill:'backwards'});
      return;
    }
    const dx = from.rect.left - to.left, dy = from.rect.top - to.top;
    if (Math.abs(dx) < 1 && Math.abs(dy) < 1) return;
    el.animate([
      {transform:`translate(${dx}px,${dy}px)`, zIndex:5},
      {transform:`translate(${dx * 0.85}px,${dy * 0.85 - 8}px) scale(1.03)`, zIndex:5, offset:.18},
      {transform:'translateY(-4px) scale(1.02)', zIndex:5, offset:.8},
      {transform:'none', zIndex:5},
    ], {duration:520, delay:next(), easing:'cubic-bezier(.3,.7,.2,1)', fill:'backwards'});
  });
  // Lanes fade back in under their steps.
  host.querySelectorAll('.ig-branch').forEach(b => {
    b.getAnimations().forEach(a => a.cancel());
    b.animate([{opacity:.25}, {opacity:1}], {duration:420, delay:120, easing:'ease-out', fill:'backwards'});
  });
  setTimeout(() => {
    badge.animate([{opacity:1}, {opacity:0, transform:'translate(-50%,-6px)'}], {duration:220, fill:'forwards'}).onfinish = () => badge.remove();
  }, 650 + turn * 75);
}
function runLogHtml(steps){
  if (!steps || !steps.length) return '<div class="muted">No steps ran.</div>';
  return `<div class="ig-run">${steps.map((s, i) => `<div style="--i:${i}"><span class="t">${(s.at_ms / 1000).toFixed(2)} s</span>
    <span class="${esc(s.status)}">${s.status === 'ok' ? '✓' : s.status === 'failed' ? '✕' : '○'} ${esc(s.label)}</span>
    <span class="d">${esc(s.detail)}</span></div>`).join('')}</div>`;
}
function warnHtml(text){
  return `<div class="lan-warn ig-inline-warn"><span class="lw-ic">⚠</span><span>${text}</span></div>`;
}

// The Save button turns into "Saved" with its check popping in, for a
// beat, before the editor closes.
function savedBeat(){
  const btn = q('.dest-form-foot [data-act="save"]');
  if (!btn || reduced()) return Promise.resolve();
  busyEls.delete(btn);
  btn.classList.remove('is-busy');
  btn.classList.add('ig-saved');
  btn.innerHTML = `${svg('check', ' stroke-width="3"')}Saved`;
  return new Promise(done => setTimeout(done, 380));
}

// `opts.stay`: the editor stays open after saving (the builder).
async function saveIntegration(d, onSaved, opts){
  const kind = Modal.kind;
  const r = await api('/integrations/save', d);
  if (!r.ok){ toast(r.error || 'Could not save', 'err', 6000); return false; }
  if (r.id) d.id = r.id;
  // Unfinished work is kept as a draft, switched off: the switch says so.
  if (r.drafted) d.enabled = false;
  if (onSaved) onSaved();
  const missing = r.missing || [], unknown = r.warnings || [];
  const names = unknown.length ? `Nothing fills ${unknown.map(w => '{' + w + '}').join(', ')}: check the spelling.` : '';
  if (missing.length){
    toast(`Saved as a draft${r.drafted ? ', switched off' : ''}. Still to do: ${missing.join('; ')}.${names ? ' ' + names : ''}`, 'info', 8000);
  } else if (names) toast(`Saved. ${names}`, 'info', 7000);
  else toast('Saved', 'ok');
  if (Modal.kind === kind){
    await savedBeat();
    // Something typed while it saved stays on screen, unsaved, instead of
    // closing with the editor.
    const owner = OWNERS[kind];
    if (Modal.kind === kind && !(opts && opts.stay)){
      if (owner && owner.dirty) owner.render();
      else Modal.close(true);
    }
  }
  await load();
  if (opts && opts.stay && Modal.kind === kind && OWNERS[kind]) OWNERS[kind].render();
  return true;
}

