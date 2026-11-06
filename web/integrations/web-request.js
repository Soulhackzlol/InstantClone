// ---------------------------------------------------------------- try a web request

// What "Send request" got, per web request step (by uidOf). Cleared
// whenever an editor opens: the steps are fresh objects each time.
const TRIES = new Map();

function tryEntry(s){
  let e = TRIES.get(uidOf(s));
  if (!e){ e = {inputs:{}}; TRIES.set(uidOf(s), e); }
  return e;
}

// "Try it": values for the variables the request uses, the button, and
// what came back, where every value can be picked. `useLabel` names what
// the picked value's main button does in this editor ('' for none).
function tryHtml(s, handler, useLabel){
  const t = tryEntry(s);
  const used = [...new Set(['url', 'headers', 'body'].flatMap(f =>
    [...String(s.params[f] || '').matchAll(/\{([A-Za-z0-9_.]{1,64})(?:[|#?][^}]*)?\}/g)].map(m => m[1])))];
  const samples = sampleMap(handler);
  const inputs = used.map(n => `<div class="dff"><label class="mono">${esc(n)}</label><input class="ic-input" data-bind="try" data-name="${esc(n)}"
    value="${esc(n in t.inputs ? t.inputs[n] : (samples[n] || ''))}" placeholder="a value to try"></div>`).join('');
  return `<div class="ig-try">
    <div class="ig-try-head"><span class="ic-label">Try it</span>
      <button class="ic-btn ic-btn-primary ig-small" data-act="b-try" ${s.params.url ? '' : 'disabled'}>${svg('play', ' class="ig-btn-ic"')}Send request</button></div>
    ${used.length ? `<div class="muted">Try it with:</div><div class="ig-try-inputs">${inputs}</div>` : ''}
    ${tryResultHtml(s, t, useLabel)}
  </div>`;
}

function tryResultHtml(s, t, useLabel){
  if (t.error) return warnHtml(esc(t.error));
  if (!t.status) return '<div class="muted">Sends the request for real and shows the answer. Then pick what to use from it.</div>';
  const n = (s.params.save_as || '').trim() || 'response';
  const ok = t.status >= 200 && t.status < 300;
  const head = `<div class="ig-try-meta"><span class="ig-status-pill ${ok ? 'ok' : 'bad'}">${t.status}</span>
    <span class="muted">${t.ms} ms${t.truncated ? ' · long answer, cut for display' : ''}</span></div>`;
  const picked = t.picked ? `<div class="ig-picked" data-key="picked-${esc(t.picked.token)}">
    <div class="ig-picked-what"><code>{${esc(t.picked.token)}}</code><span title="${esc(t.picked.sample)}">${esc(t.picked.sample) || '<i>empty</i>'}</span></div>
    <div class="ig-picked-actions">${useLabel ? `<button class="ic-btn ic-btn-primary ig-small" data-act="b-use-reply">${useLabel}</button>` : ''}
    <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="{${esc(t.picked.token)}}">Copy</button></div></div>` : '';
  if (t.json === undefined){
    return `${head}${picked}<div class="ig-try-text">${t.body ? esc(t.body.slice(0, 3000)) : '<span class="muted">The answer was empty.</span>'}</div>
      ${t.body ? `<button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="b-pick" data-token="${esc(n)}.body" data-sample="${esc(t.body.slice(0, 200))}">Use the whole answer</button>` : ''}`;
  }
  return `${head}${picked}<div class="muted">Click the value you want to use.</div><div class="ig-jtree" data-key="answer-${t.answer}">${jsonTreeHtml(t.json, n)}</div>`;
}

// Send the request with the typed values, escaped exactly as a run would.
async function tryRequest(s, handler){
  if (!s) return;
  const t = tryEntry(s);
  const vars = Object.assign(sampleMap(handler), t.inputs);
  // The same choice as the runner: `{"a":…}` or `[…]` is JSON, `{user}` a variable.
  const jsonBody = /^\s*(\[|\{\s*["}])/.test(s.params.body || '');
  const formBody = /application\/x-www-form-urlencoded/i.test(s.params.headers || '');
  const r = await api('/integrations/fetch', {
    method: s.params.method || 'POST',
    url: renderSample(s.params.url, vars, urlComponent).trim(),
    headers: renderSample(s.params.headers, vars, oneLine),
    body: renderSample(s.params.body, vars, jsonBody ? jsonStringContent : formBody ? urlComponent : null),
  });
  t.picked = null;
  if (!r.ok){ t.error = r.error || 'The request failed'; t.status = 0; return; }
  Object.assign(t, {error:'', status:r.status, ms:Math.round(r.ms), body:r.body, truncated:r.truncated,
    json:parseJson(r.body), answer:(t.answer || 0) + 1});
}

function firstStep(handler, type){
  let found = null;
  walk(handler && handler.steps, s => { if (!found && s.type === type) found = s; });
  return found;
}

// The "⋯" menu of a card or row: everything you can do to it, Delete
// included, without opening it. Lives on <body> so no card clips it.
const Menu = {
  el:null, id:null, btn:null,
  toggle(btn, id){
    if (this.id === id){ this.close(); return; }
    this.open(btn, id);
  },
  // The issue form's fields for one integration: its name, and its last
  // run when that failed.
  reportFields(i){
    const r = lastRunOf(i.id);
    return {integration:i.name, run:r && r.status === 'failed' ? reportLog(r) : ''};
  },
  open(btn, id){
    this.close();
    const i = find(id);
    if (!i) return;
    const el = document.createElement('div');
    el.className = 'ig-menu';
    el.setAttribute('role', 'menu');
    el.innerHTML = `<button role="menuitem" data-act="edit" data-id="${esc(id)}" style="--i:0">${svg('pencil')}Edit</button>
      <button role="menuitem" data-act="duplicate" data-id="${esc(id)}" style="--i:1">${svg('copy')}Duplicate</button>
      <button role="menuitem" data-act="menu-folder" data-id="${esc(id)}" style="--i:1">${svg('folder')}Move to folder</button>
      <button role="menuitem" data-act="share-one" data-id="${esc(id)}" style="--i:2">${svg('link')}Share as a recipe</button>
      <a role="menuitem" href="${esc(reportUrl(this.reportFields(i)))}" target="_blank" rel="noreferrer" style="--i:3">${svg('warn')}Report a problem</a>
      <div class="ig-menu-sep" role="separator"></div>
      <button role="menuitem" class="danger" data-act="menu-delete" data-id="${esc(id)}" style="--i:4">${svg('x')}Delete</button>`;
    document.body.appendChild(el);
    el.addEventListener('click', onClick);
    // The report opens in a new tab; the menu has done its job.
    el.addEventListener('click', e => { if (e.target.closest('a[role="menuitem"]')) this.close(); });
    el.addEventListener('keydown', e => this.keys(e));
    this.place(el, btn);
    this.el = el;
    this.id = id;
    this.btn = btn;
    btn.setAttribute('aria-expanded', 'true');
    el.querySelector('button').focus({preventScroll:true});
    document.addEventListener('pointerdown', this.outside, true);
    window.addEventListener('scroll', this.dismiss, true);
    window.addEventListener('resize', this.dismiss);
  },
  // Beside its button, inside the window.
  place(el, btn){
    const r = btn.getBoundingClientRect(), w = el.offsetWidth, h = el.offsetHeight;
    el.style.left = Math.max(8, Math.min(r.right - w, innerWidth - w - 8)) + 'px';
    el.style.top = (r.bottom + 6 + h > innerHeight - 8 ? r.top - h - 6 : r.bottom + 6) + 'px';
  },
  // The folders it can go to, in place of the menu: the one it's in ticked,
  // out of any, and a new one typed in (Enter).
  folders(id){
    const i = find(id);
    if (!i || !this.el) return;
    const cur = i.folder || '';
    const item = (v, label, n) => `<button role="menuitem" data-act="set-folder" data-id="${esc(id)}" data-v="${esc(v)}" style="--i:${n}"
      ${v === cur ? 'aria-current="true"' : ''}>${svg(v === cur ? 'check' : 'folder')}${esc(label)}</button>`;
    const names = folderNames();
    this.el.innerHTML = `<div class="ig-menu-label">Move to</div>${names.map((f, n) => item(f, f, n)).join('')}
      ${cur ? item('', 'Out of its folder', names.length) : ''}
      ${names.length || cur ? '<div class="ig-menu-sep" role="separator"></div>' : ''}
      <input class="ic-input ig-menu-new" data-id="${esc(id)}" placeholder="New folder" maxlength="40" aria-label="New folder" spellcheck="false" autocomplete="off">`;
    this.place(this.el, this.btn);
    const typed = this.el.querySelector('.ig-menu-new');
    typed.addEventListener('keydown', e => {
      if (e.key !== 'Enter' || !typed.value.trim()) return;
      e.preventDefault();
      this.close();
      moveToFolder([id], typed.value);
    });
    (names.length ? this.el.querySelector('[role="menuitem"]') : typed).focus({preventScroll:true});
  },
  close(){
    if (!this.el) return;
    const el = this.el, btn = this.btn, hadFocus = el.contains(document.activeElement);
    this.el = this.id = this.btn = null;
    document.removeEventListener('pointerdown', this.outside, true);
    window.removeEventListener('scroll', this.dismiss, true);
    window.removeEventListener('resize', this.dismiss);
    el.classList.add('closing');
    setTimeout(() => el.remove(), reduced() ? 0 : 120);
    if (btn && btn.isConnected){
      btn.setAttribute('aria-expanded', 'false');
      if (hadFocus) btn.focus({preventScroll:true});
    }
  },
  keys(e){
    // A new folder's Enter closed it already.
    if (!this.el) return;
    const items = [...this.el.querySelectorAll('[role="menuitem"]')];
    const at = items.indexOf(document.activeElement);
    const step = {ArrowDown:1, ArrowUp:-1}[e.key];
    if (e.key === 'Escape'){ e.preventDefault(); this.close(); }
    else if (step && items.length){ e.preventDefault(); items[(at + step + items.length) % items.length].focus(); }
    else if (e.key === 'Tab') this.close();
  },
  outside: e => { if (Menu.el && !Menu.el.contains(e.target) && !e.target.closest('[data-act="menu"]')) Menu.close(); },
  // Scrolling or resizing closes it, unless a new folder's name is being
  // typed: a phone's keyboard opening does both, and the menu moves to fit.
  // Resize events target the window, which isn't a Node.
  dismiss: e => {
    if (!Menu.el || (e && e.target instanceof Node && Menu.el.contains(e.target))) return;
    const typing = document.activeElement && document.activeElement.matches('.ig-menu-new');
    if (typing && Menu.el.contains(document.activeElement)) Menu.place(Menu.el, Menu.btn);
    else Menu.close();
  },
};

// The clipboard API only exists on https or this PC; a phone on the LAN
// reaches the dashboard over plain http, where the old copy command still
// works. True when it was copied.
async function writeClipboard(text){
  if (navigator.clipboard && window.isSecureContext){
    try { await navigator.clipboard.writeText(text); return true; } catch(_){}
  }
  const area = document.createElement('textarea');
  area.value = text;
  area.setAttribute('readonly', '');
  area.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0';
  // Inside the modal when one is open: its focus trap would pull focus
  // straight back otherwise.
  (Modal.form || document.body).appendChild(area);
  const had = document.activeElement;
  area.select();
  let done = false;
  try { done = document.execCommand('copy'); } catch(_){}
  area.remove();
  if (had && had.isConnected && typeof had.focus === 'function') had.focus({preventScroll:true});
  return done;
}
const COPY_FAILED = 'Could not copy: select it and copy it by hand';
async function copyText(btn){
  if (!await writeClipboard(btn.dataset.text || '')){ toast(COPY_FAILED, 'err'); return; }
  if (typeof flashCopied === 'function' && btn.classList.contains('ic-btn')) flashCopied(btn); else toast('Copied', 'ok');
}

