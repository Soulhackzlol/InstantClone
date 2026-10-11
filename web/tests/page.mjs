// A stand-in browser page for the tests: the shipped scripts are loaded
// as they are, into a Node `vm` context, so a test calls the real functions.
// No dependencies, and nothing in web/ is changed to make it testable.
//
// The page is deliberately dumb. Every element exists, accepts anything
// written to it and draws nothing; no timer ever fires and no request ever
// answers. That is enough for a script to start up and leave its functions
// behind, which is what the tests are after. A test that needs an element to
// hold something sets it on the element, through `page.window.document`.
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const WEB = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const REPO = path.dirname(WEB);

/**
 * A numeric `const` from the Rust source, for the limits a script repeats
 * ("Mirrors model::MAX_DEPTH"): the test reads the app's value instead of
 * holding a third copy that could drift with them.
 */
export function rustConst(file, name) {
  const source = fs.readFileSync(path.join(REPO, file), 'utf8');
  const found = new RegExp(`^\\s*(?:pub(?:\\([a-z]+\\))? )?const ${name}: \\w+ = ([0-9_]+);`, 'm').exec(source);
  if (!found) throw new Error(`no numeric const ${name} in ${file}`);
  return Number(found[1].replaceAll('_', ''));
}

/** The strings of a `const NAME: &[&str] = &[...]` list in the Rust source. */
export function rustList(file, name) {
  const source = fs.readFileSync(path.join(REPO, file), 'utf8');
  const found = new RegExp(`const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\];`).exec(source);
  if (!found) throw new Error(`no string list ${name} in ${file}`);
  return [...found[1].matchAll(/"([^"]*)"/g)].map(match => match[1]);
}

/** Element methods that hand back another element. */
const MAKES_ELEMENT = new Set([
  'querySelector', 'closest', 'cloneNode', 'appendChild', 'insertBefore', 'createElement',
  'createElementNS', 'createTextNode', 'createDocumentFragment', 'getElementById', 'insertAdjacentElement',
]);
/** Element methods that hand back a list. */
const MAKES_LIST = new Set(['querySelectorAll', 'getElementsByClassName', 'getElementsByTagName', 'getAnimations']);
/** Properties a script reads as data: missing ones are undefined, not a method. */
const READ_AS_DATA = /^(on|_|offset|client|scroll(Top|Left|Width|Height)$|selection|next|previous|open$|files$|src$|href$|type$|name$|id$|min$|max$|step$|placeholder$)/;

function classList(element) {
  const names = () => new Set(String(element.className || '').split(/\s+/).filter(Boolean));
  const write = set => { element.className = [...set].join(' '); };
  return {
    add: (...added) => { const set = names(); added.forEach(name => set.add(name)); write(set); },
    remove: (...removed) => { const set = names(); removed.forEach(name => set.delete(name)); write(set); },
    contains: name => names().has(name),
    toggle(name, force) {
      const set = names();
      const on = force === undefined ? !set.has(name) : !!force;
      if (on) set.add(name); else set.delete(name);
      write(set);
      return on;
    },
  };
}

/** An object that keeps what it is given and answers the few calls a style or a media query gets. */
function sponge(known = {}) {
  return new Proxy(known, {
    get(target, prop) {
      if (prop in target || typeof prop === 'symbol') return target[prop];
      return /^(set|remove|get)Property(Value|Priority)?$|^(add|remove)(EventListener|Listener)$/.test(prop) ? () => '' : undefined;
    },
  });
}

/** An element: remembers what is set on it, answers every DOM call, draws nothing. */
export function element(tag = 'div') {
  const attributes = new Map();
  const known = {
    tagName: String(tag).toUpperCase(), className: '', textContent: '', innerHTML: '', value: '', title: '',
    hidden: false, disabled: false, checked: false, children: [], childNodes: [], dataset: {},
    style: sponge(), firstChild: null, lastChild: null, parentNode: null, parentElement: null,
    setAttribute: (name, value) => { attributes.set(name, String(value)); },
    getAttribute: name => (attributes.has(name) ? attributes.get(name) : null),
    hasAttribute: name => attributes.has(name),
    removeAttribute: name => { attributes.delete(name); },
    getBoundingClientRect: () => ({ x: 0, y: 0, top: 0, left: 0, right: 0, bottom: 0, width: 0, height: 0 }),
    getContext: () => sponge({ measureText: () => ({ width: 0 }), getImageData: () => ({ data: [] }) }),
    animate: () => sponge({ finished: new Promise(() => {}), cancel() {}, finish() {} }),
    contains: () => false,
    matches: () => false,
  };
  known.classList = classList(known);
  return new Proxy(known, {
    get(target, prop) {
      if (prop in target || typeof prop === 'symbol') return target[prop];
      if (MAKES_ELEMENT.has(prop)) return () => element();
      if (MAKES_LIST.has(prop)) return () => [];
      // Anything else a script calls on an element (focus, remove, append,
      // addEventListener, scrollIntoView...) does nothing here.
      return READ_AS_DATA.test(prop) ? undefined : () => undefined;
    },
  });
}

function storage() {
  const items = new Map();
  return {
    getItem: key => (items.has(key) ? items.get(key) : null),
    setItem: (key, value) => { items.set(key, String(value)); },
    removeItem: key => { items.delete(key); },
    clear: () => items.clear(),
  };
}

function browser(address) {
  const byId = new Map();
  const document = element('document');
  Object.assign(document, {
    body: element('body'), documentElement: element('html'), head: element('head'),
    hidden: true, visibilityState: 'hidden', readyState: 'complete', cookie: '', activeElement: null,
    getElementById(id) {
      if (!byId.has(id)) byId.set(id, Object.assign(element(), { id }));
      return byId.get(id);
    },
    fonts: sponge({ ready: new Promise(() => {}) }),
  });
  class Observer { observe() {} unobserve() {} disconnect() {} }
  // A server-sent stream that never sends.
  class EventSource { close() {} addEventListener() {} }
  const never = () => new Promise(() => {});
  const url = new URL(address);
  const window = {
    document, console, URL, URLSearchParams, TextEncoder, TextDecoder, AbortController, structuredClone,
    crypto: globalThis.crypto, performance, Blob, atob, btoa,
    location: {
      href: url.href, origin: url.origin, host: url.host, hostname: url.hostname, port: url.port,
      protocol: url.protocol, pathname: url.pathname, search: url.search, hash: '', reload() {}, replace() {}, assign() {},
    },
    navigator: { userAgent: 'tests', platform: 'tests', clipboard: { writeText: async () => {} }, maxTouchPoints: 0 },
    history: { replaceState() {}, pushState() {}, state: null },
    localStorage: storage(), sessionStorage: storage(),
    innerWidth: 1280, innerHeight: 900, devicePixelRatio: 1, scrollX: 0, scrollY: 0,
    // Nothing waits, nothing answers: start-up code that polls or fetches
    // simply never hears back.
    setTimeout: () => 0, clearTimeout() {}, setInterval: () => 0, clearInterval() {},
    requestAnimationFrame: () => 0, cancelAnimationFrame() {}, queueMicrotask() {}, requestIdleCallback: () => 0,
    fetch: never,
    matchMedia: () => sponge({ matches: false }),
    getComputedStyle: () => sponge(),
    addEventListener() {}, removeEventListener() {}, dispatchEvent: () => true, scrollTo() {}, scrollBy() {},
    open() {}, alert() {}, confirm: () => false, prompt: () => null, getSelection: () => null,
    ResizeObserver: Observer, MutationObserver: Observer, IntersectionObserver: Observer, EventSource,
    Image: function Image() { return element('img'); }, Audio: function Audio() { return element('audio'); },
    CSS: { supports: () => false, escape: text => String(text) },
  };
  window.window = window.self = window.top = window.parent = window.globalThis = window;
  return window;
}

/**
 * A script wrapped in `(function(){ ... })();` keeps its helpers to itself.
 * One line goes in just before the wrapper closes, so a test can evaluate
 * inside it; everything above that line is the file as shipped.
 */
function openedUp(source) {
  const close = source.lastIndexOf('})();');
  const wrapped = /^(\s*(\/\/[^\n]*|'use strict';)\s*\n)*\s*\(function\s*\(\)\s*\{/.test(source);
  if (close < 0 || !wrapped) return source;
  return source.slice(0, close) + ';window.__inside = code => eval(code);\n' + source.slice(close);
}

/** The scripts a page runs, in the order the browser would: [name, source]. */
function scriptsOf(file) {
  const html = fs.readFileSync(path.join(WEB, file), 'utf8');
  const out = [];
  let inline = 0;
  for (const tag of html.matchAll(/<script([^>]*)>([\s\S]*?)<\/script>/g)) {
    const src = /\bsrc="\/?([^"]+)"/.exec(tag[1]);
    if (src) out.push([src[1], openedUp(fs.readFileSync(path.join(WEB, src[1]), 'utf8'))]);
    else if (tag[2].trim()) out.push([`${file}#script${++inline}`, openedUp(tag[2])]);
  }
  return out;
}

/**
 * Load a page (`index.html`, `dock.html`, `alerts.html`) and run its scripts.
 * `address` is what `location` reports, for scripts that read `?dock=`.
 *
 * Returns `{ window, run, inside }`. `run(code)` evaluates in the page,
 * where the scripts' top-level `function`, `const` and `let` are all in
 * reach: `page.run('fmtDelay(90000)')`. `inside(code)` does the same within
 * the page's script that wraps itself in a function (integrations.js on the
 * dashboard, the alerts page's own).
 */
export function loadPage(file, address = `http://127.0.0.1:7799/${file === 'index.html' ? '' : file.replace('.html', '')}`) {
  const window = browser(address);
  const context = vm.createContext(window);
  for (const [name, source] of scriptsOf(file)) {
    vm.runInContext(source, context, { filename: path.join(WEB, name) });
  }
  return {
    window,
    run: code => vm.runInContext(code, context),
    inside: code => window.__inside(code),
  };
}

/** A value made inside a page, as a plain object `assert.deepEqual` can compare. */
export function plain(value) {
  return value === undefined ? undefined : JSON.parse(JSON.stringify(value));
}
