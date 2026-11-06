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
// `tr.ATTRS` are the attributes it translates; `tr.exact(text)` is a whole-text
// lookup only (null without one).
(() => {
  const pack = window.IC_I18N || { lang: 'en' };
  document.documentElement.lang = pack.lang;
  const strings = pack.strings;
  if (!strings) {
    window.tr = text => text;
    window.tr.ATTRS = [];
    window.tr.exact = () => null;
    return;
  }

  const ATTRS = ['title', 'placeholder', 'aria-label', 'alt'];
  const SKIP = 'script,style,textarea,[translate="no"],[contenteditable="true"]';
  /** A text box's words are someone's own, but its label and hint are ours. */
  const SKIP_ATTRS = 'script,style,[translate="no"],[contenteditable="true"]';
  /** Texts joined by this ("12 frames · 0 cuts") are looked up a piece at a time. */
  const JOIN = ' · ';
  /** What "{}" matches: never across a join, whose pieces have their own keys. */
  const VALUE = '((?:(?! · ).)+?)';
  const LETTER = /[A-Za-z]/;
  /** Lookups remembered, misses too; emptied when it grows past this. */
  const CACHE_MAX = 5000;
  const cache = new Map();
  /** The translations handed out, which read as themselves. */
  const written = new Set();

  // Keys with values in them, as patterns, the most fixed text first: "Add
  // {} to the goal {}" before "Add {}", which would also match.
  const patterns = Object.keys(strings).filter(key => key.includes('{}')).map(key => {
    const pieces = key.split('{}');
    return {
      fixed: pieces.join('').length,
      pieces,
      to: strings[key],
    };
  }).sort((a, b) => b.fixed - a.fixed);
  patterns.forEach((pattern, order) => { pattern.order = order; });

  // A pattern matches a whole text, so its first or last piece must be where
  // the text starts or ends: filed under those letters, a lookup tries a
  // handful of patterns instead of all of them.
  const EDGE = 2;
  const byStart = new Map(), byEnd = new Map(), anywhere = [];
  const file = (index, edge, pattern) => {
    const list = index.get(edge);
    if (list) list.push(pattern);
    else index.set(edge, [pattern]);
  };
  for (const pattern of patterns) {
    const first = pattern.pieces[0], last = pattern.pieces[pattern.pieces.length - 1];
    if (first.length >= EDGE) file(byStart, first.slice(0, EDGE), pattern);
    else if (last.length >= EDGE) file(byEnd, last.slice(-EDGE), pattern);
    else anywhere.push(pattern);
  }
  const NONE = [];
  function candidates(text) {
    return [...(byStart.get(text.slice(0, EDGE)) || NONE), ...(byEnd.get(text.slice(-EDGE)) || NONE), ...anywhere]
      .sort((a, b) => a.order - b.order);
  }
  /** Built when first needed: most patterns are never tried in a session. */
  function matcher(pattern) {
    if (!pattern.match) {
      const escaped = pattern.pieces.map(piece => piece.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
      pattern.match = new RegExp('^' + escaped.join(VALUE) + '$');
    }
    return pattern.match;
  }

  function fill(to, values) {
    let next = 0;
    return to.replace(/\{(\d?)\}/g, (_, index) => {
      const value = index ? values[index - 1] : values[next++];
      return value === undefined ? '' : (lookupValue(value) ?? value);
    });
  }

  /** A value lowered to sit mid-sentence ("When someone follows") reads its capitalized key. */
  function lookupValue(value) {
    const said = readValue(value);
    if (said !== null) return said;
    if (/^[a-z]/.test(value)) {
      const capital = readValue(value[0].toUpperCase() + value.slice(1));
      if (capital !== null) return capital[0].toLowerCase() + capital.slice(1);
    }
    // A labelled value inside a sentence ("Remove On air: live").
    return lookupLabelled(value);
  }

  // A value is often a name someone typed ("Stream to My server on"), which a
  // short pattern would mangle. Those read only a count ("3 min ago"); a
  // pattern with this much fixed text is really that message.
  const VALUE_MIN_FIXED = 12;
  function readValue(value) {
    if (strings[value] !== undefined) return strings[value];
    const found = bestPattern(value);
    return found && (found.fixed >= VALUE_MIN_FIXED || /^\W*\d/.test(value)) ? found.said : null;
  }

  /** The most specific pattern that reads `text`: {said, fixed}, or null. */
  function bestPattern(text) {
    for (const pattern of candidates(text)) {
      const found = matcher(pattern).exec(text);
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
    // A phrase lowered to follow a label ("Does: set a 30 s delay") reads
    // its capitalized key when it has none of its own.
    if (/^[a-z]{2}/.test(core)) {
      const said = lookupAsIs(core);
      if (said !== null) return said;
      const upper = lookupAsIs(core[0].toUpperCase() + core.slice(1));
      return upper === null ? null : upper[0].toLowerCase() + upper.slice(1);
    }
    if (!/^[A-Z][a-z]/.test(core)) return lookupAsIs(core);
    // A phrase capitalized to start a line ("Pick a Discord channel"). Its own
    // key comes before any pattern that would read it in pieces ("{} channel").
    const lowered = core[0].toLowerCase() + core.slice(1);
    const capitalized = text => text[0].toUpperCase() + text.slice(1);
    if (strings[core] === undefined && strings[lowered] !== undefined) return capitalized(strings[lowered]);
    const said = lookupAsIs(core);
    if (said !== null) return said;
    const lower = lookupAsIs(lowered);
    return lower === null ? null : capitalized(lower);
  }

  // "Post in: pick a channel": a label and its value, each with a key of its
  // own, read better whole than through a short pattern ("{} channel").
  const LABEL = ': ';
  function lookupLabelled(core) {
    const at = core.indexOf(LABEL);
    if (at < 1) return null;
    const head = strings[core.slice(0, at)];
    if (head === undefined) return null;
    const rest = core.slice(at + LABEL.length);
    const tail = lookupValue(rest) ?? lookupLabelled(rest);
    return tail === null ? null : head + LABEL + tail;
  }

  function lookupAsIs(core) {
    if (strings[core] !== undefined) return strings[core];
    const labelled = lookupLabelled(core);
    if (labelled !== null) return labelled;
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
    // Writing a translation wakes the observer: what it wrote isn't read
    // again, or a short pattern could bite ("Emitir a My server on").
    if (written.has(text)) return text;
    let said = cache.get(text);
    if (said === undefined) {
      const [, lead, core, trail] = /^(\s*)([\s\S]*?)(\s*)$/.exec(text);
      // A line break inside (a confirm's paragraphs) splits the lookup.
      const lines = core.split(/(\n\s*\n)/);
      const saidCore = lines.map((line, i) => (i % 2 ? line : lookup(line.replace(/\s+/g, ' ')) ?? line)).join('');
      // After markup ("the <b>OBS</b> tab." is "la pestaña <b>OBS</b>."), a
      // translation that starts with punctuation drops the space before it.
      const tight = saidCore !== core && /^[.,;:!?)]/.test(saidCore);
      said = (tight ? '' : lead) + saidCore + trail;
      if (cache.size >= CACHE_MAX) {
        cache.clear();
        written.clear();
      }
      cache.set(text, said);
      if (said !== text) written.add(said);
    }
    return said;
  }

  function translateText(node) {
    const said = tr(node.nodeValue);
    if (said !== node.nodeValue) node.nodeValue = said;
  }

  function translateNode(node) {
    if (node.nodeType === Node.TEXT_NODE) return translateText(node);
    if (node.nodeType !== Node.ELEMENT_NODE || node.closest(SKIP_ATTRS)) return;
    translateAttrs(node);
    if (node.closest(SKIP)) return;
    const walker = document.createTreeWalker(node, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
      acceptNode: n => {
        if (n.nodeType !== Node.ELEMENT_NODE || !n.matches(SKIP)) return NodeFilter.FILTER_ACCEPT;
        if (!n.matches(SKIP_ATTRS)) translateAttrs(n);
        return NodeFilter.FILTER_REJECT;
      },
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
      } else if (!record.target.closest(SKIP_ATTRS)) {
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
  /** The translation of exactly this text, or null: no patterns, for words
   * that get saved (a preset's messages), where a near match would mangle. */
  tr.exact = text => (typeof text === 'string' && Object.hasOwn(strings, text) ? strings[text] : null);
  window.tr = tr;
})();
