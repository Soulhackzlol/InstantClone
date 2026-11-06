// How the page tests build the pieced files (web-files.mjs): the same rules,
// case for case, as the build's own (src/pieced.rs tests), so the tests
// always check the file the app serves.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { isPieceName, joinSkeleton, readWebFile, sourceOf } from './web-files.mjs';

/** Builds `skeleton` over `pieces` ({path: text}) written to a fresh folder. */
function joinWith(skeleton, pieces = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ic-pieced-'));
  try {
    fs.writeFileSync(path.join(dir, 'skel.html'), skeleton);
    for (const [file, text] of Object.entries(pieces)) {
      fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
      fs.writeFileSync(path.join(dir, file), text);
    }
    return joinSkeleton(path.join(dir, 'skel.html')).text;
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test('include lines become their pieces and other lines stay byte for byte', () => {
  const skeleton = '<style>\r\n  <!-- include a/one.css -->\r\n</style>\r\n<!-- a note -->\r\n';
  assert.equal(joinWith(skeleton, { 'a/one.css': '.x{}\r\n' }), '<style>\r\n.x{}\r\n</style>\r\n<!-- a note -->\r\n');
});

test('a piece left out of the skeleton is named', () => {
  assert.throws(() => joinWith('<!-- include a.css -->\n', { 'a.css': '', 'b.js': '' }), /no include line for b\.js/);
});

test('a piece included twice is refused', () => {
  assert.throws(() => joinWith('<!-- include a.css -->\n<!-- include a.css -->\n', { 'a.css': 'x\n' }), /skel\.html:2: .*second time/);
});

test('a piece that is not there is refused, even by case', () => {
  assert.throws(() => joinWith('<!-- include A.css -->\n', { 'a.css': 'x\n' }), /no such piece/);
});

test('a line that starts like an include must be one', () => {
  for (const bad of [
    '<!--include a.css-->', '<!-- include  a.css -->', '<!-- include a.css-->', '<!-- include ../a.css -->',
    '<!-- include ./a.css -->', '<!-- include a\\b.css -->', '<!-- include /a.css -->', '<!-- include a b.css -->',
    '<!-- include a.txt -->', '<!-- include -->',
  ]) {
    assert.throws(() => joinWith(`${bad}\n`, { 'a.css': 'x\n' }), /is not an include line/, bad);
  }
});

test('a comment that only mentions include is text', () => {
  const skeleton = '<!-- included below -->\n<!-- the include lines -->\n';
  assert.equal(joinWith(skeleton), skeleton);
});

test('a piece without a final line break gets one', () => {
  const joined = joinWith('<!-- include a.js -->\n<!-- include b.js -->\n', { 'a.js': 'f() // done', 'b.js': 'g()\n' });
  assert.equal(joined, 'f() // done\ng()\n');
});

test('a byte order mark is dropped from a piece and the skeleton', () => {
  assert.equal(joinWith('<!-- include a.css -->\n', { 'a.css': '﻿.x{}\n' }), '.x{}\n');
  assert.equal(joinWith('﻿<!-- include a.css -->\n', { 'a.css': '.x{}\n' }), '.x{}\n');
});

test('an include line inside a piece is refused', () => {
  const pieces = { 'a.html': '<p>\n<!-- include b.html -->\n', 'b.html': '' };
  assert.throws(() => joinWith('<!-- include a.html -->\n<!-- include b.html -->\n', pieces), /^Error: a\.html:2: an include line only works/);
});

test('only page files count as pieces', () => {
  for (const piece of ['hero/hero.css', 'a.html', 'page/dom.js']) assert.ok(isPieceName(piece), piece);
  for (const junk of ['.DS_Store', 'Thumbs.db', 'desktop.ini', 'hero/.hero.js.swp', 'hero/.#hero.js', 'hero/hero.js~', 'notes.md']) {
    assert.ok(!isPieceName(junk), junk);
  }
});

test('junk files are left alone and a piece name an include could not write is refused', () => {
  const pieces = { 'hero/hero.css': '', 'hero/.hero.css.swp': '', 'Thumbs.db': '', 'b.js': '' };
  assert.equal(joinWith('<!-- include b.js -->\n<!-- include hero/hero.css -->\n', pieces), '');
  assert.throws(() => joinWith('<!-- include b.js -->\n', { 'b.js': '', 'my piece.css': '' }), /^Error: my piece\.css/);
});

test('the shipped files build, framed as served, and each line knows its piece', () => {
  const page = readWebFile('index.html');
  assert.ok(page.startsWith('<!doctype html>') && page.trimEnd().endsWith('</html>'));
  const script = readWebFile('integrations.js');
  assert.ok(script.startsWith('// Integrations tab') && script.trimEnd().endsWith('})();'));
  assert.equal(sourceOf('index.html', 1), 'web/dashboard/index.html:1');
  const domLine = page.split('\n').findIndex(line => line.startsWith('const $ = id =>')) + 1;
  assert.equal(sourceOf('index.html', domLine), 'web/dashboard/page/dom.js:2');
  assert.equal(sourceOf('dock.js', 7), 'dock.js:7');
});
