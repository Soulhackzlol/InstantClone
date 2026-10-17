// The Overlay Studio's geometry (web/studio.js, StudioGeometry): where a
// layer sits for its anchor and offset, and what moving, resizing, snapping
// and lining up layers do to them. The runtime places a layer from the same
// two numbers (overlay-runtime.js anchorCss), so these are the rules a drag
// in the editor has to keep.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { loadPage } from './page.mjs';

const REPO = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
const source = file => fs.readFileSync(path.join(REPO, file), 'utf8');

const page = loadPage('index.html');
const G = page.run('StudioGeometry');
/** A plain copy, so deepEqual doesn't trip on the page's own Object. */
const plain = value => JSON.parse(JSON.stringify(value));
const box = (left, top, width, height) => ({ left, top, width, height });
const ANCHORS = ['top-left', 'top', 'top-right', 'left', 'center', 'right', 'bottom-left', 'bottom', 'bottom-right'];

test('an offset pins the edge its anchor names, on each axis', () => {
  const size = { width: 200, height: 100 };
  assert.deepEqual(plain(G.boxAt('top-left', { x: 30, y: 40 }, size)), box(30, 40, 200, 100));
  // Right and bottom count inward from the canvas's right and bottom edges.
  assert.deepEqual(plain(G.boxAt('bottom-right', { x: 30, y: 40 }, size)), box(1920 - 30 - 200, 1080 - 40 - 100, 200, 100));
  // The centre anchors: the left and top edges, from the middle.
  assert.deepEqual(plain(G.boxAt('center', { x: -100, y: 20 }, size)), box(860, 560, 200, 100));
  for (const anchor of ANCHORS) {
    const at = G.boxAt(anchor, { x: 12, y: -7 }, size);
    assert.deepEqual(plain(G.offsetFor(anchor, at)), { x: 12, y: -7 }, anchor);
  }
});

test('a move keeps the layer under the pointer, whatever it is anchored to', () => {
  const size = { width: 300, height: 80 };
  for (const anchor of ANCHORS) {
    const offset = { x: 40, y: 60 };
    const from = G.boxAt(anchor, offset, size);
    const to = G.moved(from, 25, -10);
    const next = G.placeBox(anchor, offset, from, to);
    assert.deepEqual(plain(G.boxAt(anchor, next, size)), plain(to), anchor);
  }
});

test('a resize leaves the opposite side where it was', () => {
  const start = box(100, 100, 200, 100);
  assert.deepEqual(plain(G.resizeBox(start, 'se', 50, 20)), box(100, 100, 250, 120));
  assert.deepEqual(plain(G.resizeBox(start, 'nw', 50, 20)), box(150, 120, 150, 80));
  assert.deepEqual(plain(G.resizeBox(start, 'e', 50, 999)), box(100, 100, 250, 100), 'a side moves one way only');
  // Never smaller than a few px, however far the handle goes.
  const tiny = G.resizeBox(start, 'w', 500, 0);
  assert.equal(tiny.width, 8);
  assert.equal(tiny.left + tiny.width, 300);
  // Shift on a corner keeps the proportions, from the corner opposite.
  assert.deepEqual(plain(G.resizeBox(start, 'nw', -100, 0, true)), box(0, 50, 300, 150));
});

test('a layer anchored bottom-right grows under the pointer, not away from it', () => {
  // The bug alert layers had: they are anchored to the bottom, so growing
  // their height pushed them up while the handle was pulled down.
  const size = { width: 200, height: 100 }, offset = { x: 50, y: 60 };
  const from = G.boxAt('bottom-right', offset, size);
  const to = G.resizeBox(from, 'se', 40, 30);
  const next = G.placeBox('bottom-right', offset, from, to);
  assert.deepEqual(plain(G.boxAt('bottom-right', next, { width: to.width, height: to.height })), plain(to));
  assert.equal(to.left, from.left, 'its left edge stays');
  assert.equal(to.top, from.top, 'and its top');
});

test('a layer sized by its font scales with its corner, whichever way it is dragged', () => {
  const start = box(0, 0, 200, 100);
  assert.equal(G.cornerScale(start, 0, 0), 1);
  assert.equal(G.cornerScale(start, 200, 100), 2, 'along its diagonal');
  // Straight down: its bottom comes to the pointer (it used to follow
  // across only, so a wide line of words hardly grew).
  assert.equal(G.cornerScale(start, 0, 100), 2);
  assert.equal(G.cornerScale(start, -100, 0), 0.5, 'and in, smaller');
  assert.equal(G.cornerScale(start, 20, 100), 2, 'the way it went furthest wins');
  assert.equal(G.cornerScale(start, -999, -999), 0, 'never below nothing');
});

test('layers restack forward, back, to an end and beside another, together', () => {
  const order = ['a', 'b', 'c', 'd']; // back to front
  assert.deepEqual(G.stepped(order, ['b'], 1), ['a', 'c', 'b', 'd']);
  assert.deepEqual(G.stepped(order, ['a', 'b'], 1), ['c', 'a', 'b', 'd'], 'two side by side move as one');
  assert.deepEqual(G.stepped(order, ['d'], 1), order, 'the front one stays');
  assert.deepEqual(G.stepped(order, ['b', 'd'], -1), ['b', 'a', 'd', 'c']);
  assert.deepEqual(G.toEnd(order, ['a', 'c'], true), ['b', 'd', 'a', 'c'], 'in their own order');
  assert.deepEqual(G.toEnd(order, ['d'], false), ['d', 'a', 'b', 'c']);
  assert.deepEqual(G.beside(order, ['a'], 'c', true), ['b', 'c', 'a', 'd'], 'dropped in front of c');
  assert.deepEqual(G.beside(order, ['d'], 'b', false), ['a', 'd', 'b', 'c'], 'and behind b');
  assert.deepEqual(G.beside(order, ['b'], 'b', true), order, 'onto itself: nothing moves');
  assert.notEqual(G.stepped(order, ['b'], 1), order, 'a new list');
});

test('a moving box snaps to the canvas, the safe area and other layers', () => {
  const lines = G.snapLines([box(500, 300, 100, 50)]);
  // Its centre 3 px from the canvas's middle line: pulled onto it.
  const s = G.snap(box(857, 10, 200, 40), lines, 6);
  assert.deepEqual([s.dx, s.x], [3, 960]);
  // Its left edge 4 px past another layer's right edge.
  assert.equal(G.snap(box(604, 700, 50, 50), lines, 6).x, 600);
  // Too far from everything: left alone.
  const free = G.snap(box(700, 700, 33, 33), lines, 6);
  assert.deepEqual([free.dx, free.dy, free.x, free.y], [0, 0, null, null]);
});

test('layers line up on the box around them, and spread with even gaps', () => {
  const boxes = [box(100, 10, 50, 20), box(300, 40, 100, 20), box(180, 90, 30, 20)];
  assert.deepEqual(G.align(boxes, 'left').map(b => b.left), [100, 100, 100]);
  assert.deepEqual(G.align(boxes, 'right').map(b => b.left + b.width), [400, 400, 400]);
  assert.deepEqual(G.align(boxes, 'middle').map(b => b.top + b.height / 2), [60, 60, 60]);
  const spread = G.distribute(boxes, 'x');
  assert.deepEqual(spread.map(b => b.left), [100, 300, 210], 'the first and last stay put');
  const gaps = [...spread].sort((a, b) => a.left - b.left);
  assert.equal(gaps[1].left - (gaps[0].left + gaps[0].width), gaps[2].left - (gaps[1].left + gaps[1].width));
  assert.equal(G.distribute(boxes.slice(0, 2), 'x').length, 2, 'two have nothing to spread');
});

test('every control the Studio draws names an action it has', () => {
  // A control is data (data-st), not code in an attribute: each name it
  // gives must be one the dispatcher knows, or the click does nothing.
  const studio = source('web/studio.js');
  const html = source('web/index.html');
  const actions = new Set(/const ACTIONS = \{([\s\S]*?)\n {2}\};/.exec(studio)[1]
    .split(/[,\n]/).map(s => s.trim().split(':')[0]).filter(s => /^[a-zA-Z]+$/.test(s)));
  const named = [...studio.matchAll(/(?<![A-Za-z])on\('([a-zA-Z]+)'/g), ...html.matchAll(/data-st="([a-zA-Z]+)"/g)].map(m => m[1]);
  // Controls that pass their action on by name (a colour, a slider).
  const passed = [...studio.matchAll(/(?:colorF|rangeF)\('[^']*',\s*'([a-zA-Z]+)'/g)].map(m => m[1]);
  for (const name of [...named, ...passed]) assert.ok(actions.has(name), `no action "${name}"`);
  assert.ok(!/on(click|input|change)="/.test(studio), 'no handler written as an attribute');
});
