// Every page the app serves: its scripts have to start on an empty page
// without throwing. A typo at the top level of one would otherwise only
// show as a blank dashboard, dock or alerts source in OBS.
import test from 'node:test';
import assert from 'node:assert/strict';
import { loadPage, rustList } from './page.mjs';
import { readWebFile } from './web-files.mjs';

test('each page\'s scripts start up', () => {
  for (const [file, leftBehind] of [
    ['index.html', 'typeof showTab === "function" && typeof window.Integrations.show === "function"'],
    ['dock.html', 'typeof applyState === "function"'],
    ['alerts.html', 'typeof __inside === "function"'],
  ]) {
    const page = loadPage(file);
    assert.equal(page.run(leftBehind), true, `${file} ran to its end`);
  }
});

test('an alert\'s sound or picture can only be an upload or a web address', () => {
  const mediaSrc = loadPage('alerts.html').inside('mediaSrc');
  for (const [reference, loads] of [
    ['media:airhorn.mp3', '/alerts/media/airhorn.mp3'],
    ['media:my-clip_2.webm', '/alerts/media/my-clip_2.webm'],
    ['https://example.com/a.png', 'https://example.com/a.png'],
    ['http://127.0.0.1:8080/a.ogg', 'http://127.0.0.1:8080/a.ogg'],
    // A shared recipe names its media too: nothing below may load.
    ['media:../instantclone.config.json', ''],
    ['media:sub/dir.mp3', ''],
    ['media:UPPER.mp3', ''],
    ['media:', ''],
    ['javascript:alert(1)', ''],
    ['data:text/html,<script>alert(1)</script>', ''],
    ['file:///C:/Windows/win.ini', ''],
    ['//example.com/a.png', ''],
    ['/config', ''],
    ['theme', ''],
    ['', ''],
    [null, ''],
    [undefined, ''],
  ]) {
    assert.equal(mediaSrc(reference), loads, String(reference));
  }
});

test('the alerts page draws every theme the app can save, and falls back for any other', () => {
  const themeOf = loadPage('alerts.html').inside('themeOf');
  const saved = rustList('src/integrations/alerts.rs', 'THEMES');
  assert.ok(saved.length > 0);
  for (const theme of saved) {
    assert.equal(themeOf({ theme }), theme, `${theme} has a build on the page`);
  }
  assert.equal(themeOf({ theme: 'from-a-newer-version' }), 'island');
  assert.equal(themeOf({}), 'island');
});

// A looping animation lasts as long as its state does: in the dock, a whole
// stream. Whatever it animates besides transform, opacity and filter is
// redrawn on the main thread at every frame (the dock's live dot, animating
// box-shadow, took about a tenth of a CPU core inside OBS). Put the glow on a
// pseudo-element and animate its transform and opacity instead. Listed here:
// loops that only last a moment, with what they belong to.
const MOMENTARY_LOOPS = {
  'index.html': {
    welGlow: 'the first-run welcome',
    'hk-pulse': 'a hotkey being recorded',
    igMeterPulse: 'a trigger editor saying it would fire',
    'cta-pulse': 'stepped to 10 frames a second (hero.css says why)',
    'crash-loop': 'the crash-protection settings preview, a film strip at 12 frames a second, paused while scrolled out of view',
  },
  'dock.html': { 'dico-pulse': 'a destination switch waiting 3 s for its second tap' },
  'alerts.html': {},
};
const COMPOSITED = new Set(['transform', 'translate', 'scale', 'rotate', 'opacity', 'filter']);

/** `value` split at its top-level commas: `a 1s cubic-bezier(0,0,1,1), b` is two parts. */
function topLevelParts(value) {
  const parts = [''];
  let depth = 0;
  for (const char of value) {
    if (char === '(') depth++;
    if (char === ')') depth--;
    if (char === ',' && depth === 0) parts.push('');
    else parts[parts.length - 1] += char;
  }
  return parts;
}

test('looping animations move only what the compositor can', () => {
  for (const [file, momentary] of Object.entries(MOMENTARY_LOOPS)) {
    const text = readWebFile(file);
    const keyframes = new Map([...text.matchAll(/@keyframes\s+([\w-]+)\s*\{((?:[^{}]*\{[^{}]*\})*)[^{}]*\}/g)]
      .map(([, name, body]) => [name, [...new Set([...body.matchAll(/([a-z-]+)\s*:/g)].map(match => match[1]))]]));
    const loops = new Set([...text.matchAll(/animation\s*:([^;}]*)/g)]
      .flatMap(([, value]) => topLevelParts(value))
      .filter(part => /\binfinite\b/.test(part))
      .map(part => part.trim().split(/\s+/).find(word => keyframes.has(word)))
      .filter(Boolean));
    assert.ok(loops.size > 0, `${file}: found its looping animations`);
    const redrawn = name => keyframes.get(name).filter(prop => !COMPOSITED.has(prop));
    for (const name of loops) {
      if (name in momentary) continue;
      assert.deepEqual(redrawn(name), [], `${file}: @keyframes ${name} loops and animates ${redrawn(name).join(', ')}`);
    }
    // The list stays true: each one still loops, and still needs listing.
    for (const name of Object.keys(momentary)) {
      assert.ok(loops.has(name) && redrawn(name).length > 0, `${file}: ${name} no longer needs to be listed`);
    }
  }
});
