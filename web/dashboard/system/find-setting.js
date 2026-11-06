// - Find a setting ------------------------------------
// Every System section carries data-find keywords for words people search
// with that its own text may not contain ("keybind", "brb", "port 7799").
// A section matches when every typed word appears in its title, keywords or
// text. Title hits rank first, then keyword hits, then body text; a title
// or keyword counts when one of its words starts with what was typed, so
// "port" ranks Ports above a section that merely mentions "support".
const SYS_FIND_LIMIT = 6;
let sysFindMatches = [];
let sysFindActive = 0;
function sysFindTitle(section){
  if (section.dataset.title) return section.dataset.title;
  const label = section.querySelector('.ic-label');
  if (!label) return '';
  const copy = label.cloneNode(true);
  copy.querySelectorAll('.tag, .muted').forEach(extra => extra.remove());
  return copy.textContent.trim().replace(/\s+/g, ' ');
}
function sysFindSearch(query){
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  return [...document.querySelectorAll('[data-pane="system"] .sub-pane > [data-find]')]
    .filter(section => !section.hidden)
    .map(section => {
      const title = sysFindTitle(section);
      const haystack = (title + ' ' + section.dataset.find + ' ' + section.textContent).toLowerCase();
      if (!words.every(word => haystack.includes(word))) return null;
      const startsAWord = (text, word) => text.toLowerCase().split(/[^a-z0-9]+/).some(part => part.startsWith(word));
      const score = words.reduce((sum, word) =>
        sum + (startsAWord(title, word) ? 2 : startsAWord(section.dataset.find, word) ? 1 : 0), 0);
      const subtab = section.closest('.sub-pane').dataset.subpane;
      const tab = document.querySelector(`#sys-sub-tabs [data-subtab="${subtab}"]`);
      return {section, title, subtab, tabName: tab ? tab.textContent.trim() : '', score};
    })
    .filter(Boolean)
    .sort((a, b) => b.score - a.score)
    .slice(0, SYS_FIND_LIMIT);
}
function sysFindRender(){
  const box = $('sys-find-results');
  const input = $('sys-find');
  const open = input.value.trim() !== '';
  box.hidden = !open;
  input.setAttribute('aria-expanded', String(open));
  if (!open) return;
  if (!sysFindMatches.length){
    const empty = document.createElement('div');
    empty.className = 'sys-find-empty';
    empty.textContent = 'No setting matches "' + input.value.trim() + '"';
    box.replaceChildren(empty);
    return;
  }
  box.replaceChildren(...sysFindMatches.map((match, index) => {
    const option = document.createElement('button');
    option.type = 'button';
    option.className = 'sys-find-option';
    option.id = 'sys-find-option-' + index;
    option.setAttribute('role', 'option');
    option.setAttribute('aria-selected', String(index === sysFindActive));
    const title = document.createElement('b');
    title.textContent = match.title;
    const where = document.createElement('span');
    where.textContent = match.tabName;
    option.append(title, where);
    // mousedown, not click: it lands before the input's blur closes the list.
    option.onmousedown = e => { e.preventDefault(); sysFindGo(match); };
    return option;
  }));
  input.setAttribute('aria-activedescendant', 'sys-find-option-' + sysFindActive);
}
function sysFindGo(match){
  const input = $('sys-find');
  input.value = '';
  sysFindMatches = [];
  sysFindRender();
  input.blur();
  showSubTab('system', match.subtab);
  if (match.section.tagName === 'DETAILS') match.section.open = true;
  match.section.scrollIntoView({behavior: 'smooth', block: 'start'});
  match.section.classList.remove('sys-found');
  void match.section.offsetWidth;
  match.section.classList.add('sys-found');
}
(() => {
  const input = $('sys-find');
  input.addEventListener('input', () => {
    sysFindMatches = sysFindSearch(input.value);
    sysFindActive = 0;
    sysFindRender();
  });
  input.addEventListener('keydown', e => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp'){
      e.preventDefault();
      if (!sysFindMatches.length) return;
      const step = e.key === 'ArrowDown' ? 1 : -1;
      sysFindActive = (sysFindActive + step + sysFindMatches.length) % sysFindMatches.length;
      sysFindRender();
    } else if (e.key === 'Enter'){
      e.preventDefault();
      if (sysFindMatches[sysFindActive]) sysFindGo(sysFindMatches[sysFindActive]);
    } else if (e.key === 'Escape'){
      input.value = '';
      sysFindMatches = [];
      sysFindRender();
      input.blur();
    }
  });
  input.addEventListener('blur', () => { $('sys-find-results').hidden = true; input.setAttribute('aria-expanded', 'false'); });
  input.addEventListener('focus', () => { if (input.value.trim()) sysFindRender(); });
  // "/" jumps to the search while the System tab is open, unless the user
  // is already typing somewhere.
  document.addEventListener('keydown', e => {
    if (e.key !== '/' || e.ctrlKey || e.metaKey || e.altKey) return;
    if (document.querySelector('.tab-pane[data-pane="system"]').hidden) return;
    const target = e.target;
    if (target.closest('input, textarea, select, [contenteditable="true"]')) return;
    e.preventDefault();
    input.focus();
  });
})();

