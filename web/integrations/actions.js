// ---------------------------------------------------------------- actions

async function toggle(id){
  const i = find(id);
  if (!i) return;
  const want = !i.enabled;
  if (want && localLocked() && runsLocally(i)){ toast(LOCAL_LOCK_TEXT, 'info', 7000); return; }
  const issues = issuesOf(i);
  if (want && issues.length){
    toast(`${i.name}: ${issues[0]} first`, 'info', 4500);
    openEditor(i, true);
    return;
  }
  // Flip at once; put it back if the app says no.
  i.enabled = want;
  render();
  if (want) glow(id);
  const r = await api('/integrations/toggle', {id, enabled:want});
  if (r.ok) return;
  i.enabled = !want;
  render();
  toast(r.error || 'Could not switch it', 'err', 5000);
  if (want) openEditor(i, true);
}

// A card that just switched on gives one soft pulse of its colour.
function glow(id){
  const card = document.querySelector(`[data-key="c-${CSS.escape(id)}"], [data-key="r-${CSS.escape(id)}"]`);
  if (!card || reduced()) return;
  const color = getComputedStyle(card).getPropertyValue('--dc').trim() || 'currentColor';
  card.animate([
    {boxShadow:`0 0 0 0 color-mix(in oklch, ${color} 55%, transparent)`},
    {boxShadow:`0 0 0 10px color-mix(in oklch, ${color} 0%, transparent)`},
  ], {duration:620, easing:'cubic-bezier(.2,.7,.2,1)'});
}

// Add from the catalog. The integrations added, or none.
// The designs `added` integrations' alerts name, saved now: a ready-made
// one only becomes a file the first time it is used, and the alerts page
// needs the file.
async function saveNamedDesigns(added){
  const named = new Set();
  added.forEach(i => allSteps(i).forEach(s => { if (s.type === 'overlay' && s.params.design) named.add(s.params.design); }));
  for (const slug of named) await Stage.ensureDesign(slug);
}
// What a preset says (its name, messages, titles, card fields, the reward it
// listens for) comes from the app in English. In another language the copy
// is saved in it before anyone sees it, a whole text at a time: only words
// with a translation of their own change, never ids, sources or commands.
const SAID_PARAMS = ['title', 'text', 'group', 'footer', 'description'];
const sayExactly = text => tr.exact(text) ?? text;
// A step's words in the page's language, where the strings have them.
function sayStep(s){
  SAID_PARAMS.forEach(k => { if (typeof s.params[k] === 'string') s.params[k] = sayExactly(s.params[k]); });
  if (typeof s.params.fields === 'string') s.params.fields = s.params.fields.split('\n').map(sayExactly).join('\n');
}
function inPageLanguage(i){
  const say = sayExactly;
  const copy = clone(i);
  copy.name = say(copy.name);
  allSteps(copy).forEach(sayStep);
  copy.handlers.forEach(h => {
    const f = h.trigger.filters;
    if (f && typeof f.reward === 'string') f.reward = say(f.reward);
  });
  return JSON.stringify(copy) === JSON.stringify(i) ? null : copy;
}
async function sayAddedInPageLanguage(ids){
  let changed = false;
  for (const id of ids){
    const copy = find(id) && inPageLanguage(find(id));
    if (copy && (await api('/integrations/save', copy)).ok) changed = true;
  }
  if (changed) await load();
}
async function addFrom(body, openAfter){
  const fromCatalog = Modal.kind === 'catalog';
  const r = await api('/integrations/add', body);
  if (!r.ok){ toast(r.error || 'Could not add it', 'err', 5000); return []; }
  await load();
  await sayAddedInPageLanguage(r.ids || []);
  const added = (r.ids || []).map(find).filter(Boolean);
  if (!added.length){ toast('You already have everything in this pack', 'info'); return []; }
  const ids = added.map(i => i.id);
  await saveNamedDesigns(added);
  // Both sets of Twitch alerts on: every follow, sub or cheer plays twice.
  const RIVAL = {twitch_alerts:'hype_alerts', hype_alerts:'twitch_alerts'};
  const rivalOn = i => S.data.integrations.find(x => x.enabled && x.preset === RIVAL[i.preset]);
  const twice = added.find(i => RIVAL[i.preset] && rivalOn(i));
  if (twice) toast(`${rivalOn(twice).name} is on too: switch one off, or every Twitch event plays two alerts`, 'info', 8000);
  const unfinished = added.find(i => !i.enabled);
  // One added opens, to see it or finish it (a pack's waiting draft too).
  const opened = openAfter && (unfinished || (added.length === 1 ? added[0] : null));
  if (opened){
    // Going back from here means "not this one": it goes again.
    const back = fromCatalog ? {title:'Back to the catalog, without adding it', go:async () => {
      await deleteIds([opened.id]);
      Catalog.reopen();
    }} : null;
    openEditor(opened, true, {justAdded:true, back});
    return added;
  }
  undoToast(added.length > 1 ? `Added ${added.length} integrations` : `Added ${added[0] ? added[0].name : ''}`, async () => {
    await deleteIds(ids);
    if (Modal.kind === 'catalog') Catalog.render();
  }, true);
  if (unfinished) toast(`${unfinished.name} needs one more detail before it can switch on`, 'info', 4500);
  if (Modal.kind === 'catalog' && body.preset) Catalog.flashAdded(body.preset);
  return added;
}

// A pack's drafts that only waited for a Discord channel get the first
// one, and switch on when nothing else is missing.
async function finishDrafts(ids){
  if (!S.data.connections.discord.length) return;
  let on = 0;
  for (const id of ids){
    const i = find(id);
    if (!i || !pickNewChannel(i)) continue;
    const draft = clone(i);
    const r = await api('/integrations/save', Object.assign(clone(draft), {enabled:true}));
    // Kept as a draft by the app (something is still missing) is not on.
    if (r.ok && !r.drafted) on++;
    else if (!r.ok) await api('/integrations/save', draft);
  }
  await load();
  if (on) toast(`${plural(on, 'integration')} from the pack switched on`, 'ok', 4500);
}

// The copy lands right after the original, switched off; it glows so the
// eye finds it, and the toast opens it.
async function duplicate(id){
  const r = await api('/integrations/duplicate', {id});
  if (!r.ok){ toast(r.error || 'Could not duplicate it', 'err'); return; }
  await load();
  const copy = find(r.id);
  if (!copy){ toast('Duplicated', 'ok'); return; }
  reveal(copy.id);
  actionToast(`Duplicated as ${copy.name}, switched off`, 'Edit copy', () => {
    const now = find(copy.id);
    if (now) openEditor(now);
  });
}
// Bring a card or row into view and glow it. A search or a category
// filter that hides it steps aside first.
function reveal(id){
  const i = find(id);
  if (!i) return;
  if (!visibleIntegrations().includes(i)){ S.search = ''; S.filter = 'all'; render(); }
  // After the morph has placed it.
  requestAnimationFrame(() => {
    const card = document.querySelector(`[data-key="c-${CSS.escape(id)}"], [data-key="r-${CSS.escape(id)}"]`);
    if (!card) return;
    card.scrollIntoView({block:'nearest', behavior:reduced() ? 'auto' : 'smooth'});
    glow(id);
  });
}

async function remove(id){
  const kind = Modal.kind;
  const i = find(id);
  // Kept so Undo can put it back exactly, in the same place.
  const snapshot = i ? clone(i) : null;
  const at = i ? S.data.integrations.indexOf(i) : -1;
  const r = await api('/integrations/delete', {id});
  if (!r.ok){ toast(r.error || 'Could not delete it', 'err'); return; }
  // Only close the editor that asked; the user may have moved on.
  if (kind && Modal.kind === kind) Modal.close(true);
  await load();
  if (!snapshot){ toast('Deleted', 'ok'); return; }
  undoToast(`Deleted ${snapshot.name}`, async () => {
    const back = await api('/integrations/save', Object.assign(snapshot, {at}));
    if (!back.ok) toast(back.error || 'Could not bring it back', 'err', 5000);
    await load();
  });
}

async function deleteIds(ids){
  for (const id of ids) await api('/integrations/delete', {id});
  await load();
}

// `switchOn`: it should end up on, and waits for a detail first.
function openEditor(i, switchOn, opts){
  if (i.preset) Editor.open(i, switchOn, opts);
  else Builder.open(i, Object.assign({switchOn}, opts));
  // A failing card says "open it to see why": opened, it shows why.
  const st = stat(i);
  if (i.enabled && st && st.failing > 0 && lastRunOf(i.id)) setLastRun(true);
}
// The last run's steps over an editor's footer: shown or hidden.
function setLastRun(open){
  if (S.lastRunOpen === open) return;
  S.lastRunOpen = open;
  const o = OWNERS[Modal.kind];
  if (o && o.render && (Modal.kind === 'editor' || Modal.kind === 'builder')) o.render();
}

