// The pages' language. English lives in the code; for another language the
// server sends this file with that language's strings ahead of it
// (window.IC_I18N, see build.rs) and the translator swaps text on its way to
// the screen, so no page script has to know.
//
// A string is a key as it reads on screen, spaces collapsed. "{}" in a key
// stands for a value the page fills in ("{} is too long for the buffer");
// in the translation "{}" takes the values in order and "{2}" a given one.
// Anything inside translate="no" stays as it is: the Logs, names people
// typed, code.
//
// `tr(text, where)` is the same lookup for code that compares what is on
// screen with what it is about to write, so a redraw with nothing new
// writes nothing (see setText). `where` is the element the text goes in.
// `tr.ATTRS` are the attributes it translates.
(() => {
  const pack = window.IC_I18N || { lang: 'en' };
  document.documentElement.lang = pack.lang;
  const strings = pack.strings;
  if (!strings) {
    window.tr = text => text;
    window.tr.ATTRS = [];
    return;
  }

  const ATTRS = ['title', 'placeholder', 'aria-label', 'alt'];
  const SKIP = 'script,style,textarea,[translate="no"],[contenteditable="true"]';
  /** Texts joined by this ("12 frames · 0 cuts") are looked up a piece at a time. */
  const JOIN = ' · ';
  /** What "{}" matches: never across a join, whose pieces have their own keys. */
  const VALUE = '((?:(?! · ).)+?)';
  const LETTER = /[A-Za-z]/;
  /** Lookups remembered, misses too; emptied when it grows past this. */
  const CACHE_MAX = 5000;
  const cache = new Map();

  // Keys with values in them, as patterns, the most fixed text first: "Add
  // {} to the goal {}" before "Add {}", which would also match. Each keeps
  // its longest fixed piece, so most texts are ruled out with one `includes`.
  const patterns = Object.keys(strings).filter(key => key.includes('{}')).map(key => {
    const pieces = key.split('{}');
    const escaped = pieces.map(piece => piece.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    return {
      fixed: pieces.join('').length,
      longest: pieces.reduce((a, b) => (b.length > a.length ? b : a), ''),
      match: new RegExp('^' + escaped.join(VALUE) + '$'),
      to: strings[key],
    };
  }).sort((a, b) => b.fixed - a.fixed);

  function fill(to, values) {
    let next = 0;
    return to.replace(/\{(\d?)\}/g, (_, index) => {
      const value = index ? values[index - 1] : values[next++];
      return value === undefined ? '' : (lookupValue(value) ?? value);
    });
  }

  /** A value lowered to sit mid-sentence ("When someone follows") reads its capitalized key. */
  function lookupValue(value) {
    const said = lookupWhole(value);
    if (said !== null || !/^[a-z]/.test(value)) return said;
    const capital = lookupWhole(value[0].toUpperCase() + value.slice(1));
    return capital === null ? null : capital[0].toLowerCase() + capital.slice(1);
  }

  /** The most specific pattern that reads `text`: {said, fixed}, or null. */
  function bestPattern(text) {
    for (const pattern of patterns) {
      if (!text.includes(pattern.longest)) continue;
      const found = pattern.match.exec(text);
      if (found) return { said: fill(pattern.to, found.slice(1)), fixed: pattern.fixed };
    }
    return null;
  }

  function lookupWhole(core) {
    const exact = strings[core];
    if (exact !== undefined) return exact;
    const found = bestPattern(core);
    return found ? found.said : null;
  }

  function lookupJoined(core) {
    if (!core.includes(JOIN)) return null;
    const pieces = core.split(JOIN);
    const said = pieces.map(piece => lookupWhole(piece) ?? piece);
    return said.some((piece, i) => piece !== pieces[i]) ? said.join(JOIN) : null;
  }

  function lookup(core) {
    const said = lookupAsIs(core);
    if (said !== null || !/^[A-Z][a-z]/.test(core)) return said;
    // A phrase capitalized to start a line ("Pick a Discord channel").
    const lower = lookupAsIs(core[0].toLowerCase() + core.slice(1));
    return lower === null ? null : lower[0].toUpperCase() + lower.slice(1);
  }

  function lookupAsIs(core) {
    if (strings[core] !== undefined) return strings[core];
    // A piece beside markup keeps its edge: "of 5 running ·", "needs attention."
    const [, lead, inner, trail] = /^([·,:;.→\s]*)([\s\S]*?)([·,:;.→\s]*)$/.exec(core);
    const edged = (lead || trail) && inner;
    if (edged && strings[inner] !== undefined) return lead + strings[inner] + trail;
    // The pattern with the most fixed text wins, edge or not: "of {} running"
    // reads "of 5 running." better than "of {}" does.
    const whole = bestPattern(core);
    const bare = edged ? bestPattern(inner) : null;
    if (bare && (!whole || bare.fixed > whole.fixed)) return lead + bare.said + trail;
    if (whole) return whole.said;
    const joined = lookupJoined(core);
    if (joined !== null || !edged) return joined;
    const joinedInner = lookupJoined(inner);
    return joinedInner === null ? null : lead + joinedInner + trail;
  }

  /** The text in the page's language, or the text itself when it has none. */
  function tr(text, where) {
    if (typeof text !== 'string' || !LETTER.test(text)) return text;
    if (where && where.closest && where.closest(SKIP)) return text;
    let said = cache.get(text);
    if (said === undefined) {
      const [, lead, core, trail] = /^(\s*)([\s\S]*?)(\s*)$/.exec(text);
      // A line break inside (a confirm's paragraphs) splits the lookup.
      const lines = core.split(/(\n\s*\n)/);
      said = lead + lines.map((line, i) => (i % 2 ? line : lookup(line.replace(/\s+/g, ' ')) ?? line)).join('') + trail;
      if (cache.size >= CACHE_MAX) cache.clear();
      cache.set(text, said);
    }
    return said;
  }

  function translateText(node) {
    const said = tr(node.nodeValue);
    if (said !== node.nodeValue) node.nodeValue = said;
  }

  function translateNode(node) {
    if (node.nodeType === Node.TEXT_NODE) return translateText(node);
    if (node.nodeType !== Node.ELEMENT_NODE || node.closest(SKIP)) return;
    translateAttrs(node);
    const walker = document.createTreeWalker(node, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
      acceptNode: n => (n.nodeType === Node.ELEMENT_NODE && n.matches(SKIP) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT),
    });
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      if (n.nodeType === Node.TEXT_NODE) translateText(n);
      else translateAttrs(n);
    }
  }

  function translateAttrs(el) {
    for (const name of ATTRS) {
      const value = el.getAttribute(name);
      if (value === null) continue;
      const said = tr(value);
      if (said !== value) el.setAttribute(name, said);
    }
    // A button drawn as <input type=button value=...>.
    if (el.tagName === 'INPUT' && (el.type === 'button' || el.type === 'submit') && el.value) {
      const said = tr(el.value);
      if (said !== el.value) el.value = said;
    }
  }

  function onChanges(records) {
    for (const record of records) {
      if (record.type === 'childList') {
        record.addedNodes.forEach(node => {
          if (node.nodeType === Node.TEXT_NODE && node.parentElement?.closest(SKIP)) return;
          translateNode(node);
        });
      } else if (record.type === 'characterData') {
        if (!record.target.parentElement?.closest(SKIP)) translateNode(record.target);
      } else if (!record.target.closest(SKIP)) {
        translateAttrs(record.target);
      }
    }
  }

  // Watching from the start catches the page as the parser builds it, before
  // its first paint, and everything a script draws after.
  new MutationObserver(onChanges).observe(document.documentElement, {
    childList: true,
    subtree: true,
    characterData: true,
    attributes: true,
    attributeFilter: ATTRS,
  });

  for (const name of ['alert', 'confirm', 'prompt']) {
    const native = window[name].bind(window);
    window[name] = (message, ...rest) => native(tr(String(message ?? '')), ...rest);
  }
  tr.ATTRS = ATTRS;
  window.tr = tr;
})();
