// The words the pages show, read from their source, for the translation
// test (i18n.test.mjs). A string counts once it reads like words: markup
// text, a title or placeholder, a JS string that isn't code. Each comes out
// the way the translator looks it up (web/i18n.js): spaces collapsed, and
// "{}" where a template or a `+` puts a value in.
//
//   node web/tests/strings.mjs missing   what web/i18n/es.json lacks, as JSON
//   node web/tests/strings.mjs stale     keys nothing in the code says any more
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const WEB = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const REPO = path.dirname(WEB);
/** The pages the translator runs on, and the scripts they load. */
export const PAGES = ['index.html', 'dock.html', 'dock.js', 'integrations.js', 'studio.js'];
const ATTRS = ['title', 'placeholder', 'aria-label', 'alt'];
const ENTITIES = { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'", nbsp: ' ', rarr: '→', larr: '←', middot: '·', hellip: '…', times: '×', mdash: '-', ndash: '-',
  le: '≤', ge: '≥', uarr: '↑', darr: '↓', minus: '−', ensp: ' ' };

export function readStrings(lang = 'es') {
  return JSON.parse(fs.readFileSync(path.join(WEB, 'i18n', `${lang}.json`), 'utf8'));
}

/** Strings found in the code that stay as they are (web/i18n/untranslated.txt). */
export function readUntranslated() {
  const text = fs.readFileSync(path.join(WEB, 'i18n', 'untranslated.txt'), 'utf8');
  return new Set(text.split(/\r?\n/).filter(line => line && !line.startsWith('#')));
}

const decodeEntities = text => text.replace(/&(#x?[0-9a-f]+|[a-z]+);/gi, (whole, name) => {
  if (name[0] === '#') return String.fromCodePoint(name[1] === 'x' || name[1] === 'X' ? parseInt(name.slice(2), 16) : +name.slice(1));
  return ENTITIES[name.toLowerCase()] ?? whole;
});
const normalize = text => text.replace(/\s+/g, ' ').trim();

// ------------------------------------------------------------ JS literals

/** Where `/` starts a regex rather than a division, by what came before. */
const BEFORE_REGEX = /(^|[=(,:;!&|?{}[+\-*%<>~^]|\breturn|\btypeof|\bcase|\bin|\bof)\s*$/;

/**
 * Every string literal in JS source: {text, line}, templates with "{}" for
 * each `${...}` (whose own literals come out too). A `+` joining literals
 * around one plain value joins them the same way.
 */
export function jsLiterals(src, baseLine = 1) {
  const found = [];
  let i = 0, line = baseLine, last = null;
  const n = src.length;

  function readQuoted(quote) {
    let text = '';
    i++;
    while (i < n && src[i] !== quote) {
      if (src[i] === '\\') {
        const next = src[i + 1];
        text += next === 'n' ? '\n' : next === 't' ? '\t' : next === 'u' ? String.fromCharCode(parseInt(src.slice(i + 2, i + 6), 16)) : next === '\n' || next === '\r' ? '' : next;
        i += next === 'u' ? 6 : 2;
        continue;
      }
      if (src[i] === '\n') line++;
      text += src[i++];
    }
    i++;
    return text;
  }

  function readTemplate() {
    let text = '';
    i++;
    while (i < n && src[i] !== '`') {
      if (src[i] === '\\') { text += src[i + 1] === 'n' ? '\n' : src[i + 1]; i += 2; continue; }
      if (src[i] === '$' && src[i + 1] === '{') {
        const from = i += 2;
        skipCode('}');
        // An icon put in is markup, not a value in the words.
        if (!ICON.test(src.slice(from, i))) text += '{}';
        i++;
        continue;
      }
      if (src[i] === '\n') line++;
      text += src[i++];
    }
    i++;
    return text;
  }

  // Code up to the closing `stop` at this depth, collecting literals on the way.
  function skipCode(stop) {
    let depth = 0;
    while (i < n) {
      const c = src[i];
      if (c === stop && depth === 0) return;
      if (c === '{' || c === '(' || c === '[') depth++;
      else if (c === '}' || c === ')' || c === ']') depth--;
      if (!step()) i++;
    }
  }

  // One literal or comment at i; false when i holds plain code.
  function step() {
    const c = src[i];
    if (c === '\n') { line++; i++; return true; }
    if (c === '/' && src[i + 1] === '/') { while (i < n && src[i] !== '\n') i++; return true; }
    if (c === '/' && src[i + 1] === '*') {
      const end = src.indexOf('*/', i + 2);
      const stop = end < 0 ? n : end + 2;
      line += countLines(src, i, stop);
      i = stop;
      return true;
    }
    if (c === '/' && BEFORE_REGEX.test(src.slice(Math.max(0, i - 12), i))) {
      i++;
      let inClass = false;
      while (i < n && src[i] !== '\n' && (src[i] !== '/' || inClass)) {
        if (src[i] === '\\') i++;
        else if (src[i] === '[') inClass = true;
        else if (src[i] === ']') inClass = false;
        i++;
      }
      i++;
      return true;
    }
    if (c === "'" || c === '"' || c === '`') {
      const start = i, startLine = line;
      const text = c === '`' ? readTemplate() : readQuoted(c);
      join({ text, line: startLine, start, end: i });
      return true;
    }
    return false;
  }

  // 'a' + value + 'b' reads as one string with "{}" for the value.
  function join(literal) {
    const between = last ? src.slice(last.end, literal.start) : null;
    if (between !== null && /^\s*\+\s*$/.test(between)) {
      last.text += literal.text;
      last.end = literal.end;
      return;
    }
    if (between !== null && /^\s*\+\s*[\w$.]+(\([\w$.,' ]*\))?\s*\+\s*$/.test(between)) {
      last.text += '{}' + literal.text;
      last.end = literal.end;
      return;
    }
    last = literal;
    found.push(literal);
  }

  while (i < n) if (!step()) i++;
  // A value added before or after with `+` ('Updated to v' + version).
  for (const literal of found) {
    if (VALUE_AFTER.test(src.slice(literal.end, literal.end + 80))) literal.text += '{}';
    if (VALUE_BEFORE.test(src.slice(Math.max(0, literal.start - 80), literal.start))) literal.text = '{}' + literal.text;
  }
  return found.map(({ text, line }) => ({ text, line }));
}

/** `+ value` closing an expression, or `value +` opening one, beside a string. */
const VALUE_AFTER = /^\s*\+\s*[\w$.]+(\([^()'"`]*\))?\s*([;,)\]}:?]|$)/;
const VALUE_BEFORE = /([=(,:?[{]|\breturn)\s*[\w$.]+(\([^()'"`]*\))?\s*\+\s*$/;

const countLines = (text, from, to) => {
  let count = 0;
  for (let k = from; k < to; k++) if (text[k] === '\n') count++;
  return count;
};

// ------------------------------------------------------------ markup

const TEXT_ATTR = new RegExp(`\\s(?:${ATTRS.join('|')})\\s*=\\s*(?:"([^"]*)"|'([^']*)')`, 'g');

/** The words in a piece of markup, {word, at}: text between tags, and the text attributes. */
function markupWords(html) {
  const words = [];
  for (const found of html.matchAll(TEXT_ATTR)) words.push({ word: found[1] ?? found[2], at: found.index });
  // A template cut by a value can start inside a tag or end in one.
  const head = /^[^<>]*=[^<>]*>/.exec(html);
  const from = head ? head[0].length : 0;
  const text = html.slice(from).replace(/<[a-zA-Z/][^>]*$/, '');
  for (const found of text.matchAll(/(?:^|>)([^<>]+)(?=<|$)/g)) {
    if (!/=["']/.test(found[1])) words.push({ word: found[1], at: from + found.index });
  }
  return words.map(({ word, at }) => ({ word: edgeless(normalize(decodeEntities(word))), at }));
}

/** Markup without what never shows: styles, scripts, comments (lines kept). */
const blankOut = whole => '\n'.repeat(countLines(whole, 0, whole.length));
const visibleMarkup = html => html.replace(/<style\b[^>]*>[\s\S]*?<\/style>/gi, blankOut).replace(/<!--[\s\S]*?-->/g, blankOut);

/** "{}" glued to a word is markup put in (an icon), not a value in the text. */
/**
 * Values side by side read as one. One glued to a word is markup put in (an
 * icon, a "(maybe)" part), or after a full stop more text: neither is part of
 * the words. A version stays: "Updated to v{}".
 */
const edgeless = text => text.replace(/(\{\})+/g, '{}').replace(/^\{\}(?=[A-Za-z])/, '')
  .replace(/(?<=[A-Za-z.!?])(?<! v)\{\}$/, '');
/** A `${...}` that draws an icon. */
const ICON = /^\s*(svg|icon)\w*\(/;

// ------------------------------------------------------------ what counts

const CODE = [
  /^[.#[:]/, /=>|===|!==|&&|\|\||\(\)|;$/, /^[\w-]+\s*:\s*[^ ]+;?$/, /^[a-z]+:\/\//i, /^\/[\w/-]*$/,
  /^[MmLlHhVvCcSsQqTtAaZz][\d.,-]/, /^[\w$]+\(/, /\bvar\(--/, /^\d[\d.]*(px|em|rem|%|ms|deg|fr)\b/, /^[\d.]+(px)? [\d.]+(px)? /,
];

/**
 * Whether a string from the code reads as words a person sees. Text between
 * tags always does ("tab.", ": then press Save"); a bare JS string has to
 * look like words, not code.
 */
export function readsAsWords(text, inMarkup = false) {
  // "{}: no hotkey set" reads from its words, not the colon a value left.
  const bare = text.replace(/\{\}/g, ' ').trim().replace(/^[·,:;.]\s+(?=\S+\s)/, '');
  if (!/[A-Za-z]{2}/.test(bare)) return false;
  if (inMarkup) return true;
  // One word: a label when capitalized ("Save"), code when not ("click").
  if (!/\s/.test(bare)) return /^[A-Z][a-z]+[!?.:…]*$|^[A-Z]{2,}$/.test(bare) || /^[A-Z][a-z]+[A-Z]?[a-z]*-[a-z]+$/.test(bare);
  // Several lowercase identifiers ("is-busy ig-row") are class lists.
  if (/^[a-z][\w-]*(\s+[a-z][\w-]*)+$/.test(bare) && /-|_/.test(bare)) return false;
  return !CODE.some(code => code.test(bare)) || /^[A-Z][a-z]/.test(bare);
}

/** Every string the pages show, as {text, where}, each text once. */
export function pageStrings() {
  const seen = new Map();
  const add = (text, where, inMarkup) => {
    text = normalize(text);
    if (text && !seen.has(text) && readsAsWords(text, inMarkup)) seen.set(text, where);
  };
  for (const file of PAGES) {
    const src = fs.readFileSync(path.join(WEB, file), 'utf8').replace(/\r\n/g, '\n');
    const literals = [];
    if (file.endsWith('.html')) {
      const markup = visibleMarkup(src).replace(/<script\b[^>]*>([\s\S]*?)<\/script>/gi, (whole, code, at) => {
        literals.push(...jsLiterals(code, countLines(src, 0, at) + 1));
        return blankOut(whole);
      });
      for (const { word, at } of markupWords(markup)) add(word, `${file}:${countLines(markup, 0, at) + 1}`, true);
    } else {
      literals.push(...jsLiterals(src));
    }
    for (const { text, line } of literals) {
      if (/<[a-zA-Z/]/.test(text)) markupWords(text).forEach(found => add(found.word, `${file}:${line}`, true));
      else add(edgeless(normalize(decodeEntities(text))), `${file}:${line}`, false);
    }
  }
  for (const [file, pattern] of RUST_SHOWN) {
    const src = fs.readFileSync(path.join(REPO, file), 'utf8');
    for (const found of src.matchAll(pattern)) add(found[1].replace(/\\(.)/g, '$1'), file, true);
  }
  return seen;
}

/** Strings the app sends for the pages to show: the catalog, the platforms. */
const RUST_SHOWN = [
  ['src/integrations/presets.rs', /^\s*(?:name|description): "((?:[^"\\]|\\.)*)",?$/gm],
  ['src/web.rs', /"(?:label|key_help|tip)":"((?:[^"\\]|\\.)*)"/g],
  ['src/integrations/event.rs', /EventKind::\w+ => "([A-Z][^"]*)"/g],
  ['src/instance_lock.rs', /\("([A-Z][a-z]+)", \[0x/g],
];

/** Strings in the Rust source, for keys the app's own messages need. */
export function rustStrings() {
  const found = [];
  const walk = dir => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (entry.name.endsWith('.rs')) {
        const src = fs.readFileSync(full, 'utf8').replace(/\r\n/g, '\n');
        for (const m of src.matchAll(/r#"([\s\S]*?)"#|"((?:[^"\\]|\\[\s\S])*)"/g)) {
          const text = (m[1] ?? m[2].replace(/\\\n\s*/g, '').replace(/\\(.)/g, '$1'))
            .replace(/\{\{/g, '\u0001').replace(/\}\}/g, '\u0002').replace(/\{[^{}]*\}/g, '{}')
            .replace(/\u0001/g, '{').replace(/\u0002/g, '}');
          found.push(normalize(text));
        }
      }
    }
  };
  walk(path.join(REPO, 'src'));
  return found;
}

/** Page strings the dictionary lacks and that aren't marked to stay. */
export function missing(strings = readStrings(), keep = readUntranslated()) {
  return [...pageStrings()].filter(([text]) => !(text in strings) && !keep.has(text));
}

/** Keys whose fixed pieces no longer appear in any page or Rust string. */
export function stale(strings = readStrings()) {
  // Every string in the scripts counts here, the ones that don't read as
  // words too ("anytime" shown as a value), and the overlay runtime's, which
  // show in the Studio's previews.
  const scripts = [...PAGES, 'overlay-runtime.js'].flatMap(file => {
    const src = fs.readFileSync(path.join(WEB, file), 'utf8');
    const code = file.endsWith('.html') ? [...src.matchAll(/<script\b[^>]*>([\s\S]*?)<\/script>/gi)].map(m => m[1]) : [src];
    // plural(n, 'step') says "steps" without the code spelling it.
    const plurals = [...src.matchAll(/plural\([^\n]*?'([^']+)'\)/g)].map(m => m[1] + 's');
    return [...plurals, ...code.flatMap(part => jsLiterals(part).map(found => normalize(found.text)))];
  });
  const said = [...pageStrings().keys(), ...rustStrings(), ...scripts];
  const all = said.join('\n');
  const exact = new Set(said);
  return Object.keys(strings).filter(key => !exact.has(key)
    && !key.split('{}').every(piece => !piece.trim() || all.includes(piece.trim())));
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const mode = process.argv[2];
  const out = mode === 'stale' ? stale() : Object.fromEntries(missing().map(([text, where]) => [text, where]));
  process.stdout.write(JSON.stringify(out, null, 1) + '\n');
}
