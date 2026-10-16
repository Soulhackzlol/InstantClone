// The Overlay Studio's baker (web/overlay-runtime.js): what a saved overlay
// carries to OBS. It runs as the dashboard loads it, in a bare context.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const WEB = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const window = {};
vm.runInNewContext(fs.readFileSync(path.join(WEB, 'overlay-runtime.js'), 'utf8'), { window });
const IC = window.ICOverlay;

const cssOf = html => html.split('<style>')[1].split('</style>')[0];
const scriptOf = html => html.split('<script>')[1].split('</' + 'script>')[0];
function overlayWith(widget) {
  const doc = IC.defaultDoc('test');
  doc.widgets.push(widget);
  return IC.bake(doc);
}

test('a widget with one look in every state bakes one rule for it', () => {
  const css = cssOf(IC.bake(IC.trackerDoc('Goal', 'subs', 'Sub goal')));
  assert.match(css, /body\[data-state\] #w4\{color:var\(--ic-accent\)/);
  assert.doesNotMatch(css, /body\[data-state="(idle|active)"\] #w4/);
});

test('states that look different keep a rule each, and alike ones share', () => {
  const w = IC.defaultWidget('Text', 'w1');
  w.states = { idle: { visible: false }, active: { color: '#ff0000' } };
  const css = cssOf(overlayWith(w));
  assert.match(css, /body\[data-state="idle"\] #w1\{opacity:0;visibility:hidden/);
  assert.match(css, /body\[data-state="active"\] #w1\{color:#ff0000/);
  assert.match(css, /body\[data-state="preparing"\] #w1,body\[data-state="ready"\] #w1,body\[data-state="error"\] #w1\{color:#5ac8fa/);
});

test('a gradient on text paints every part of it, in every state at once', () => {
  const w = IC.defaultWidget('Timer', 'w1');
  w.base.grad = '#00ff00';
  const css = cssOf(overlayWith(w));
  assert.match(css, /body\[data-state\] #w1 \.v,body\[data-state\] #w1 \.lbl,body\[data-state\] #w1 \.u\{background:linear-gradient/);
});

test('the runtime ships without its comments and still reads the overlay feed', () => {
  const js = scriptOf(IC.bake(IC.defaultDoc('test')));
  assert.doesNotThrow(() => new Function(js));
  assert.ok(!js.split('\n').some(line => line.startsWith('//') || /^\s/.test(line)));
  assert.ok(js.includes("'/overlay-events'") && js.includes("'/overlay-state'"));
});

test('a lower-third left blank says what it is for', () => {
  const html = overlayWith(IC.defaultWidget('Banner', 'w1'));
  assert.match(html, /data-txt-base="ON A DELAY"/);
  assert.match(html, /<span class="lbl">ON A DELAY<\/span>/);
});

test('an overlay says which page of the Overlay tab it is on', () => {
  const kindOf = doc => /<meta name="ic-kind" content="(\w+)">/.exec(IC.bake(doc))[1];
  assert.equal(kindOf(IC.trackerDoc('Goal', 'subs', 'Sub goal')), 'goals');
  assert.equal(kindOf(IC.PRESETS[0]), 'delay');
  const own = IC.defaultDoc('Starting soon');
  own.purpose = 'custom';
  own.widgets.push(IC.defaultWidget('Text', 'w1'));
  assert.equal(kindOf(own), 'custom');
});

test('an overlay from before pages says it is a delay one', () => {
  const old = IC.trackerDoc('Timer', 'sub', 'Subathon');
  delete old.purpose;
  assert.equal(IC.docPurpose(old), 'delay');
  assert.equal(IC.docPurpose(IC.defaultDoc('empty')), 'delay');
});

test('one look in every state is the chosen state\'s, and the rest go', () => {
  const w = IC.defaultWidget('Text', 'w1');
  w.states = { idle: { visible: false }, active: { color: '#ff0000', glow: 8, hide_after_ms: 4000 }, ready: { color: '#00ff00' } };
  IC.oneLook(w, 'active');
  assert.equal(w.always, true);
  assert.equal(w.base.color, '#ff0000');
  assert.equal(w.base.glow, 8);
  assert.deepEqual(w.states, { idle: { visible: false }, active: {}, ready: {} });
});

test('no widget holds a GPU layer for the life of the page', () => {
  assert.doesNotMatch(cssOf(IC.bake(IC.defaultDoc('test'))), /will-change/);
});

test('a widget unticked from one look keeps a look per state', () => {
  const w = IC.defaultWidget('Text', 'w1');
  w.base.text = '{{goal.subs}}';
  w.always = false;
  w.states = { active: { color: '#ff0000' } };
  const css = cssOf(overlayWith(w));
  assert.match(css, /body\[data-state="active"\] #w1\{color:#ff0000/);
});

test('a state pill keeps lower case with a font picked', () => {
  const w = IC.defaultWidget('StatePill', 'w1');
  w.uppercase = false;
  w.font_family = 'Poppins';
  const css = cssOf(overlayWith(w));
  assert.match(css, /#w1\{text-transform:none\}/);
  assert.match(css, /#w1\{font-family:"Poppins"/);
});

test('a one-look widget plays its scaled entrance once, not per state', () => {
  const w = IC.defaultWidget('Text', 'w1');
  w.always = true;
  w.base.anim = { style: 'slide-up', duration_ms: 400, easing: 'ease-out', delay_ms: 0, amount: 2 };
  const css = cssOf(overlayWith(w));
  assert.equal(css.match(/@keyframes ic-slide-up-w1/g).length, 1);
  assert.doesNotMatch(css, /ic-slide-up-w1(idle|active)/);
});

test('one look from a state takes its animation over the widget\'s own', () => {
  const w = IC.defaultWidget('Text', 'w1');
  w.base.anims = [{ style: 'fade', duration_ms: 400 }];
  w.states = { active: { anim: { style: 'pop', duration_ms: 300 } } };
  IC.oneLook(w, 'active');
  assert.equal(w.base.anims, undefined);
  assert.equal(w.base.anim.style, 'pop');
});

test('custom HTML wraps the dotted goal and timer words on load', () => {
  const doc = IC.defaultDoc('test');
  doc.widgets.push(Object.assign(IC.defaultWidget('CustomHTML', 'w1'), { html: '<b>{{goal.subs}}</b>' }));
  assert.ok(IC.bake(doc).includes(String.raw`/\{\{[\w.]+\}\}/.test(n.nodeValue)`));
});

test('an alert design waits hidden, then says the alert it is handed', () => {
  const html = IC.bake(IC.alertDoc('Follow'));
  assert.match(html, /<meta name="ic-kind" content="alerts">/);
  assert.match(html, /<body data-state="idle" data-purpose="alerts" data-alert-sound="chime">/);
  assert.match(html, /<img data-src-tpl="\{\{alert\.image\}\}" alt="">/);
  assert.match(html, /data-txt-base="\{\{alert\.title\}\}"/);
  assert.match(cssOf(html), /body\[data-state="idle"\] #w2\{opacity:0;visibility:hidden/);
  assert.match(scriptOf(html), /__ic_alert/);
});

test('words can decode or type themselves in, and only words', () => {
  const w = IC.defaultWidget('Text', 'w1');
  w.always = true; w.states = {};
  w.base.anim = { style: 'decode', duration_ms: 700, easing: 'ease-out', delay_ms: 100 };
  const html = overlayWith(w);
  assert.match(html, /data-txfx-active="decode:700:100"/);
  assert.match(scriptOf(html), /function textFx/);
  assert.ok(IC.animsFor('Text').some(a => a.key === 'typewriter'));
  assert.ok(!IC.animsFor('Shape').some(a => a.key === 'decode'));
  assert.ok(IC.animsFor('Shape').some(a => a.key === 'wipe'));
});

test('a ready-made alert design is layers: shapes, the picture, the title and the words', () => {
  assert.deepEqual([...IC.ALERT_THEMES], ['island', 'signal', 'banner', 'bubble', 'minimal', 'paper', 'cinema']);
  IC.ALERT_THEMES.forEach(theme => {
    const doc = IC.alertPreset(theme), kinds = doc.widgets.map(w => w.kind);
    assert.ok(kinds.includes('Image') && kinds.filter(k => k === 'Text').length === 3, theme);
    assert.ok(doc.widgets.every(w => w.states.idle.visible === false), theme + ' waits hidden');
    assert.deepEqual([...doc.widgets.filter(w => w.kind === 'Text').map(w => w.base.text)], ['{{alert.title}}', '{{alert.text}}', '{{alert.message}}']);
    const message = doc.widgets.find(w => w.with_message);
    assert.ok(message.wrap && !message.fit, theme + ': the message wraps');
  });
  const html = IC.bake(IC.alertPreset('signal'));
  // The viewer's message: up to three lines, and gone with none.
  assert.match(html, /#w6 \.lbl\{display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:3/);
  assert.match(html, /body\[data-nomsg\] #w6\{display:none\}/);
  assert.match(html, /body\.toggleAttribute\('data-nomsg', !a\.message\)/);
  assert.match(html, /<meta name="ic-kind" content="alerts">/);
  assert.match(html, /data-src-tpl="\{\{alert\.image\}\}"/);
  assert.match(html, /data-fit/);
  assert.match(html, /data-txfx-active="decode:700:380"/);
  assert.match(html, /object-fit:cover/);
});

test('a ready-made alert design says its own sound, which the page plays', () => {
  assert.match(IC.bake(IC.alertPreset('banner')), /<body data-state="idle" data-purpose="alerts" data-alert-sound="fanfare">/);
  assert.match(IC.bake(IC.alertDoc('New design')), /data-alert-sound="chime"/);
  // A delay overlay never carries one.
  const d = IC.defaultDoc('x'); d.sound = 'pop';
  assert.doesNotMatch(IC.bake(d), /<body[^>]*data-alert-sound/);
});

test('with no picture, the words move into its place; the alert brings its accent', () => {
  const html = IC.bake(IC.alertPreset('island'));
  assert.match(html, /body\[data-nopic\] #w3\{translate:-84px 0\}/);
  assert.match(html, /body\[data-nopic\] #w4\{translate:-84px 0\}/);
  assert.doesNotMatch(html, /body\[data-nopic\] #w1\{/);
  assert.match(scriptOf(html), /setProperty\('--ic-accent',a\.color\)/);
});

test('a look from before is one Alert card layer, its theme kept', () => {
  const html = IC.bake(IC.alertCardDoc('signal'));
  assert.match(html, /<iframe class="ic-card" src="\/alerts\?embed" data-look="\{&quot;theme&quot;:&quot;signal&quot;\}"/);
});

test('a look from before becomes its theme\'s layers, what was changed kept', () => {
  const doc = IC.alertLookDoc({ theme: 'bubble', accent: '#ff0000', text: '#00ff00', background: '#123456',
    opacity: 50, radius: 8, font: 'Rubik', sound: 'zap', scale: 200 });
  assert.equal(doc.theme.accent, '#ff0000');
  assert.equal(doc.theme.ink, '#00ff00');
  assert.equal(doc.sound, 'zap');
  const [bubble, tail] = doc.widgets;
  assert.deepEqual([bubble.base.color, tail.base.color, bubble.base.opacity, bubble.radius], ['#123456', '#123456', 0.5, 16]);
  // Twice the size: its corners too.
  assert.deepEqual({ ...bubble.size }, { w: 1200, h: 236 });
  assert.ok(doc.widgets.filter(w => w.kind === 'Text').every(w => w.font_family === 'Rubik'));
  assert.equal(IC.alertLookDoc({ theme: 'island', css: '.card{color:red}' }), null, 'custom CSS keeps the card');
  // What layers can't be keeps the card too: the picture on top, a card's
  // colour where the theme draws none.
  assert.equal(IC.alertLookDoc({ theme: 'island', picture: 'top' }), null);
  assert.equal(IC.alertLookDoc({ theme: 'minimal', background: '#123456' }), null);
  // The card comes in as it did, in the PC's own font.
  const own = IC.alertLookDoc({ theme: 'island', animation: 'bounce', font: 'system' });
  assert.equal(own.widgets[0].base.anim.style, 'boing');
  assert.ok(own.widgets.filter(w => w.kind === 'Text').every(w => w.font_family === IC.SYSTEM_FONT));
  assert.match(IC.bake(own), /font-family:system-ui/);
});

test('a look from before keeps its place, the layers moved as one', () => {
  const top = IC.alertLookDoc({ theme: 'island', position: 'top', align: 'right' }).widgets;
  assert.ok(top.every(w => w.anchor === 'top-right'));
  const card = top[0], dot = top[4];
  assert.deepEqual([card.offset.x, card.offset.y], [36, 64]);
  assert.ok(dot.offset.x < 100 && dot.offset.y > card.offset.y, 'the dot keeps its corner of the card');
  assert.equal(top[5].offset.y, 172, 'from the top, the message goes under the card');
  const middle = IC.alertLookDoc({ theme: 'island', position: 'middle' }).widgets;
  assert.ok(middle.every(w => w.anchor === 'center'));
  assert.equal(middle[0].offset.y, -48, 'the card is centred');
  assert.equal(middle[5].offset.y, 60, 'in the middle too, the message goes under the card');
});

test('a baked overlay says which build of the Studio baked it', () => {
  assert.match(IC.BUILD, /^[0-9a-f]{8}$/);
  assert.match(IC.bake(IC.defaultDoc('x')), new RegExp('<meta name="ic-build" content="' + IC.BUILD + '">'));
});

test('a Shape and a picture take corners, a ring and a shadow', () => {
  const s = IC.defaultWidget('Shape', 'w1');
  Object.assign(s, { radius: 12, border: 3, border_color: '#ff0000', shadow: true });
  const css = overlayWith(s);
  assert.match(css, /#w1\{border-radius:12px;box-sizing:border-box;border:3px solid #ff0000;box-shadow:0 14px 40px/);
  const plain = overlayWith(IC.defaultWidget('Shape', 'w2'));
  assert.doesNotMatch(plain, /#w2\{border-radius|border:\d/);
});
