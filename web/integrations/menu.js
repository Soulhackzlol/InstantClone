// ---------------------------------------------------------------- the ⋯ menu

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
