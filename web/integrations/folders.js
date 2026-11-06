// ---- Folders: a name an integration can carry. A folder is there while
// something is in it.
function folderNames(){
  return [...new Set(S.data.integrations.map(i => i.folder || '').concat(S.pending).filter(Boolean))].sort((a, b) => a.localeCompare(b));
}
// "New folder", or "New folder 2" when that one is there.
function newFolderName(){
  const names = folderNames();
  let name = 'New folder';
  for (let n = 2; names.includes(name); n++) name = 'New folder ' + n;
  return name;
}
// Rename a folder now, its name picked.
function renameNow(folder){
  S.renaming = folder;
  render();
  // On the tab itself (q looks in the open window only).
  const field = [...$('ig-root').querySelectorAll('.ig-folder-in')].find(el => !el.closest('[data-leaving]'));
  if (field){ field.focus({preventScroll:true}); field.select(); field.scrollIntoView({block:'nearest', behavior:reduced() ? 'auto' : 'smooth'}); }
}
// A folder's heading: fold it, how many it holds and how many are on, and
// rename it (in place). The rest are "Not in a folder", with no rename.
function folderHeadHtml(folder, list, folded){
  const on = list.filter(i => i.enabled).length;
  const key = 'fh-' + (folder || '~none');
  if (folder && S.renaming === folder){
    return `<div class="ig-folder" data-key="${esc(key)}">${svg('folder', ' class="ig-folder-ic"')}
      <input class="ic-input ig-folder-in" data-folder="${esc(folder)}" value="${esc(folder)}" maxlength="40" aria-label="Folder name" spellcheck="false" autocomplete="off"></div>`;
  }
  return `<div class="ig-folder${folded ? ' folded' : ''}" data-key="${esc(key)}" data-drop-folder="${esc(folder)}">
    <button class="ig-folder-head" data-act="fold" data-folder="${esc(folder)}" aria-expanded="${!folded}">
      ${svg('chev', ' class="ig-folder-chev"')}${svg('folder', ' class="ig-folder-ic"')}<b>${esc(folder || 'Not in a folder')}</b>
      <span class="ig-count" data-tick>${list.length}</span><span class="muted">${on} on</span></button>
    ${folder ? `<button class="ic-btn-tiny" data-act="folder-rename" data-folder="${esc(folder)}" aria-label="Rename ${esc(folder)}" title="Rename">${svg('pencil')}</button>
      <button class="ic-btn-tiny" data-act="folder-remove" data-folder="${esc(folder)}" aria-label="Remove the folder ${esc(folder)}" title="Remove the folder: what's in it stays, out of any folder">${svg('x')}</button>` : ''}</div>`;
}
// Put integrations in a folder ('' for none), in one save.
async function moveToFolder(ids, folder){
  const r = await api('/integrations/folder', {ids, folder});
  if (!r || r.ok === false){ toast((r && r.error) || 'That didn\'t save', 'err', 6000); return false; }
  const kept = oneLine(folder).trim();
  ids.forEach(id => { const i = find(id); if (i) i.folder = kept; });
  render();
  return true;
}
// A folder renamed: everything in it moves, folded or not as it was.
async function renameFolder(was, name){
  S.renaming = null;
  name = oneLine(name).trim().slice(0, 40);
  const ids = S.data.integrations.filter(i => i.folder === was).map(i => i.id);
  // A new folder with nothing in it yet is only here: renamed (or, left
  // blank, gone) here.
  if (!ids.length){
    const others = S.pending.filter(f => f !== was);
    S.pending = others.concat(name && !others.includes(name) ? [name] : []);
    render();
    return;
  }
  if (!name || name === was){ render(); return; }
  if (await moveToFolder(ids, name) && S.folded.delete(was)){
    S.folded.add(name);
    store.set('ig-folded', JSON.stringify([...S.folded]));
  }
}

function switchHtml(i){
  return `<button class="dcard-switch${i.enabled ? ' on' : ''}" role="switch" aria-checked="${i.enabled}"
    aria-label="${esc(i.name)}" data-act="toggle" data-id="${esc(i.id)}"><span></span></button>`;
}
function moreHtml(i){
  return `<button class="ic-btn-tiny ig-more" data-act="menu" data-id="${esc(i.id)}" aria-label="More for ${esc(i.name)}"
    aria-haspopup="menu" aria-expanded="${Menu.id === i.id}">${svg('dots', ' stroke-width="3"')}</button>`;
}
// The line under a card's name: what blocks it, else what's wrong with
// it, else where it goes and when it last ran. Never over the preview.
function cardNoteHtml(c, i){
  const where = whereLine(i, c);
  // Twins with the same warning still have to tell themselves apart.
  const twin = where === c.where ? '' : ` · ${where}`;
  if (c.issue) return `<div class="dcard-host ig-host-warn" title="${esc(c.issue)}">${esc(c.issue + twin)}</div>`;
  if (c.flag) return `<div class="dcard-host ig-host-${c.flag.cls}" title="${esc(c.flag.tip)}">${esc(c.flag.label + twin)}</div>`;
  const note = `${where} · ${c.last}`;
  return `<div class="dcard-host" title="${esc(note)}">${esc(note)}</div>`;
}
// Where a card goes, or, for cards sharing a name and a place, what tells
// this one apart: what starts it, else what it says, else its number. The
// names stay as the streamer wrote them.
function whereLine(i, c){
  const twins = S.data.integrations.filter(o => o.name === i.name && cardInfo(o).where === c.where);
  if (twins.length < 2) return c.where;
  const only = read => { const mine = read(i); return mine && twins.filter(o => read(o) === mine).length === 1 ? mine : ''; };
  const startedBy = o => { const h = cardInfo(o).h; return h ? triggerSentence(h.trigger) : ''; };
  return only(startedBy) || only(o => cardInfo(o).summary) || `#${twins.indexOf(i) + 1}`;
}
function flagHtml(flag){
  if (!flag) return '';
  return `<span class="dcard-status ig-flag ${flag.cls}" title="${esc(flag.tip)}"><span class="d-dot"></span><span class="dcard-status-l">${flag.label}</span></span>`;
}

function gridHtml(items, none, withAdd, folder){
  return `<div class="ig-grid" data-flip data-drop-folder="${esc(folder)}" data-key="g:${esc(folder)}${items.length ? '' : '|add'}">${none}${items.map(i => {
    const c = cardInfo(i);
    return `<div class="dcard ig-card ${i.enabled ? 'alive' : 'off'}" style="--dc:${c.dc}" data-key="c-${esc(i.id)}" draggable="true" data-drag-id="${esc(i.id)}">
      <div class="dcard-screen" data-act="edit" data-id="${esc(i.id)}" role="button" tabindex="0" aria-label="Edit ${esc(i.name)}">
        ${previewHtml(c.step, c.h, false)}${c.hist}
      </div>
      <div class="ig-card-foot">
        <span class="dcard-icon">${svg(c.icon)}</span>
        <div class="dcard-id"><div class="dcard-name" translate="no">${esc(i.name)}</div>${cardNoteHtml(c, i)}</div>
        ${moreHtml(i)}${switchHtml(i)}
      </div>
    </div>`;
  }).join('')}
  ${withAdd ? `<button class="dest-add ig-add-tile" data-act="catalog" data-key="add"><span class="dest-add-icon">${svg('plus')}</span>Add or build your own</button>` : ''}</div>`;
}

function listHtml(items, none, withAdd, folder){
  return `<div class="ig-list" data-flip data-drop-folder="${esc(folder)}" data-key="g:${esc(folder)}${items.length ? '' : '|add'}">${none}${items.map(i => {
    const c = cardInfo(i);
    const open = S.peek === i.id;
    return `<div class="dcard ig-row ${i.enabled ? 'alive' : 'off'}${open ? ' open' : ''}" style="--dc:${c.dc}" data-key="r-${esc(i.id)}" data-autoh draggable="true" data-drag-id="${esc(i.id)}">
      <div class="ig-row-main">
        <span class="dcard-icon">${svg(c.icon)}</span>
        <button class="ig-row-name" data-act="peek" data-id="${esc(i.id)}" aria-expanded="${open}">
          <span class="dcard-name" translate="no">${esc(i.name)}</span><span class="dcard-host${c.issue ? ' ig-host-warn' : ''}">${esc(c.issue || whereLine(i, c))}</span></button>
        <span class="ig-row-sum" title="${esc(c.summary)}">${esc(c.summary)}</span>
        ${c.hist}${c.flag ? flagHtml(c.flag) : `<span class="ig-row-last">${esc(c.last)}</span>`}
        ${switchHtml(i)}
        <button class="ic-btn-tiny" data-act="edit" data-id="${esc(i.id)}" aria-label="Edit ${esc(i.name)}">${svg('pencil')}</button>
        ${moreHtml(i)}
      </div>
      ${open ? `<div class="ig-row-peek"><div class="dcard-screen">${previewHtml(c.step, c.h, true)}</div>
        <div class="ig-row-peek-actions">
          <button class="ic-btn ic-btn-primary" data-act="edit" data-id="${esc(i.id)}">Edit</button>
          <button class="ic-btn ic-btn-ghost" data-act="duplicate" data-id="${esc(i.id)}">Duplicate</button>
        </div></div>` : ''}
    </div>`;
  }).join('')}
  ${withAdd ? `<button class="dest-add ig-add-row" data-act="catalog" data-key="add"><span class="dest-add-icon">${svg('plus')}</span>Add an integration, or build your own</button>` : ''}</div>`;
}

function emptyHtml(){
  const packs = S.data.catalog.packs;
  const preset = id => S.data.catalog.presets.find(p => p.id === id);
  const works = p => S.data.twitch.available || !(p.needs || []).includes('twitch');
  const best = Math.max(0, packs.findIndex(works));
  return `<div class="dest-empty" data-key="empty">
    <div class="dest-empty-title">What should InstantClone do for you?</div>
    <div class="dest-empty-sub">Pick a pack to add a few integrations at once. Anything that needs a connection waits, switched off, until it's connected. You can change or remove anything later.</div>
    <div class="dest-empty-grid ig-packs">
      ${packs.map((p, n) => `<button class="dest-starter ig-pack${n === best ? ' ig-pack-rec' : ''}" data-act="pack" data-pack="${esc(p.id)}">
        <span class="dest-starter-name">${esc(p.name)}${n === best ? ' <span class="dcard-status s-ready ig-rec">Recommended</span>' : ''}</span>
        <span class="dest-starter-note">${esc(p.description)}</span>
        <span class="ig-pack-list">${p.presets.map(id => `<span>${svg(PRESET_ICON[id] || 'steps')}${esc((preset(id) || {}).name || id)}</span>`).join('')}</span>
        ${needsHtml(p.needs)}
      </button>`).join('')}
    </div>
    <div class="muted ig-empty-more">or <a href="#" data-act="catalog">browse everything</a> · <a href="#" data-act="build">build your own</a> · <a href="#" data-act="import">import a recipe</a></div>
  </div>`;
}

function activityHtml(){
  const rows = (S.data.activity || []).slice(0, 40);
  const seen = new Map();
  const body = rows.length ? rows.map(r => {
    let key = `${r.at_ms}-${r.integration_id}`;
    const dup = seen.get(key) || 0;
    seen.set(key, dup + 1);
    if (dup) key += '-' + dup;
    const failed = r.steps.find(s => s.status === 'failed');
    const detail = failed ? failed.label + ': ' + failed.detail
      : r.status === 'busy' ? 'skipped: too many runs at once'
      : r.status === 'cooldown' ? 'skipped: pressed again too soon (Repeat at most)'
      : r.steps.filter(s => s.status === 'ok').map(s => s.label).join(' · ') || r.trigger;
    // A row opens onto its steps, to read, copy or report.
    const open = S.actOpen === key;
    return `<div class="ig-act-item${open ? ' open' : ''}" data-key="a-${esc(key)}" data-autoh>
      <div class="ig-act-row link" data-act="act-row" data-k="${esc(key)}" role="button" tabindex="0" aria-expanded="${open}"><span class="t">${esc(fmtClock(r.at_ms))}</span>
      <span class="n"><span translate="no">${esc(r.name)}</span>${r.test ? ' <span class="muted">(test)</span>' : ''}</span>
      <span class="d">${esc(r.trigger)} → ${esc(detail)}</span><span class="s ${esc(r.status)}">${esc(r.status === 'cooldown' ? 'skipped' : r.status)}</span>${svg('chev', ' class="c"')}</div>
      ${open ? `<div class="ig-act-more">${runPanelHtml(r, true)}</div>` : ''}</div>`;
  }).join('') : '<div class="ig-act-empty" data-key="a-none">Runs show up here, with every step and what it answered.</div>';
  return `<details class="sys-section ig-activity-box" id="ig-activity-box" data-key="activity" ${S.activityOpen ? 'open' : ''}>
    <summary><span class="ig-act-title">Recent activity</span><span class="muted">${rows.length ? 'last run ' + esc(fmtAgo(rows[0].at_ms)) : 'nothing has run yet'}</span>${svg('chev')}</summary>
    <div class="ig-activity" data-flip>${body}</div>
  </details>`;
}

