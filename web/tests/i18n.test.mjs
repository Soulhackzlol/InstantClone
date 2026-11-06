// The Spanish strings (web/i18n/es.json) and the translator that shows them
// (web/i18n.js). A new string in the pages fails here until it is translated
// or listed in web/i18n/untranslated.txt; `node web/tests/strings.mjs
// missing` lists them.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';
import { missing, stale, pageStrings, readStrings, readUntranslated } from './strings.mjs';

const WEB = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const strings = readStrings('es');
const keep = readUntranslated();

/** The values a translation puts in: "{}" in order, or "{2}" by place. */
const slots = text => (text.match(/\{\d?\}/g) || []);

test('every string the pages show has Spanish, or is marked to stay', () => {
  const lacking = missing(strings, keep).map(([text, where]) => `${where}: ${text}`);
  assert.deepEqual(lacking, [], 'translate these in web/i18n/es.json, or list them in web/i18n/untranslated.txt');
});

test('no Spanish is left for a string the code no longer says', () => {
  assert.deepEqual(stale(strings), []);
  const shown = pageStrings();
  assert.deepEqual([...keep].filter(text => !shown.has(text)), [], 'untranslated.txt lists strings the pages no longer have');
  assert.deepEqual([...keep].filter(text => text in strings), [], 'a string is both translated and kept as it is');
});

test('each translation takes the same values as its English', () => {
  for (const [en, es] of Object.entries(strings)) {
    assert.equal(typeof es, 'string', en);
    const want = slots(en).length;
    const got = slots(es);
    const numbered = got.filter(slot => slot !== '{}');
    if (numbered.length) {
      const places = new Set(numbered.map(slot => +slot[1]));
      assert.ok(got.length === numbered.length && [...places].every(p => p >= 1 && p <= want) && places.size === want, `${en} -> ${es}`);
    } else {
      assert.equal(got.length, want, `${en} -> ${es}`);
    }
    assert.ok(!es.includes('—'), `no em dash: ${es}`);
  }
});

test('no Spanish reads as an English key of another meaning', () => {
  // The translator looks up whatever is on screen, its own writes too: a
  // value that is itself a key would flip on every redraw.
  for (const [en, es] of Object.entries(strings)) {
    if (es in strings) assert.equal(strings[es], es, `${en} -> ${es} -> ${strings[es]}`);
  }
});

/** web/i18n.js run on a page with no elements, with these strings. */
function translator(pack) {
  const page = {
    IC_I18N: pack,
    document: { documentElement: { lang: '' } },
    MutationObserver: class { observe() {} },
    alert() {}, confirm() {}, prompt() {},
  };
  page.window = page;
  vm.runInNewContext(fs.readFileSync(path.join(WEB, 'i18n.js'), 'utf8'), page);
  return page.tr;
}

test('in English the pages show the code\'s own text', () => {
  const tr = translator(undefined);
  assert.equal(tr('Save settings'), 'Save settings');
  assert.deepEqual([...tr.ATTRS], []);
});

test('a value lowered to sit mid-sentence still reads its key', () => {
  const tr = translator({ lang: 'es', strings: { 'When {}': 'Cuando {}', 'Someone follows': 'Alguien te sigue' } });
  assert.equal(tr('When someone follows'), 'Cuando alguien te sigue');
  assert.equal(tr('When nothing known'), 'Cuando nothing known', 'no key leaves it as it is');
});

test('a phrase capitalized to start a line reads its own key, not a pattern', () => {
  const tr = translator({ lang: 'es', strings: { 'pick a Discord channel': 'elige un canal de Discord', '{} channel': '{} canal', Open: 'Abierto', open: 'abierto y ya' } });
  assert.equal(tr('Pick a Discord channel'), 'Elige un canal de Discord');
  assert.equal(tr('Twitch channel'), 'Twitch canal', 'with no key of its own, the pattern still reads it');
  assert.equal(tr('Open'), 'Abierto', 'its capitalized key comes first');
});

test('punctuation a translation starts with sits against the markup before it', () => {
  const tr = translator({ lang: 'es', strings: { 'tab.': '.', 'Save settings': 'Guardar ajustes' } });
  assert.equal(tr(' tab.'), '.', '"la pestaña <b>Destinos</b> ." would read wrong');
  assert.equal(tr(' Save settings'), ' Guardar ajustes', 'a word keeps its space');
  assert.equal(tr(' .'), ' .', 'text with nothing to translate stays');
});

test('a name someone typed survives inside a sentence', () => {
  const tr = translator({
    lang: 'es',
    strings: { 'Stream to {}': 'Emitir a {}', '{} on': '{} activas', 'Last ran {}': 'Última ejecución {}', '{} min ago': 'hace {} min', "Couldn't save: {}": 'No se pudo guardar: {}', '{}: stream key required': '{}: falta la clave de transmisión' },
  });
  assert.equal(tr('Stream to My server on'), 'Emitir a My server on', 'a short pattern leaves a name alone');
  assert.equal(tr('Last ran 3 min ago'), 'Última ejecución hace 3 min', 'a count is still read');
  assert.equal(tr("Couldn't save: Kick: stream key required"), 'No se pudo guardar: Kick: falta la clave de transmisión', 'a whole message is too');
});

test('a value never swallows the pieces of a joined line', () => {
  const tr = translator({
    lang: 'es',
    strings: { '{} on stream': '{} en el directo', 'Needs Twitch': 'Necesita Twitch', '{} frames': '{} frames', '{} cuts': '{} cortes', '{} reconnects': '{} reconexiones' },
  });
  assert.equal(tr('Show on stream'), 'Show en el directo', 'alone, the pattern still reads it');
  assert.equal(tr('Needs Twitch · #1'), 'Necesita Twitch · #1');
  assert.equal(tr('Needs Twitch · Show on stream'), 'Necesita Twitch · Show en el directo');
  assert.equal(tr('12 frames · 0 cuts · 3 reconnects'), '12 frames · 0 cortes · 3 reconexiones');
});

test('the translator finds a string as it reads on screen', () => {
  const tr = translator({
    lang: 'es',
    strings: {
      'Save settings': 'Guardar ajustes',
      '{} is too long for the buffer': '{} es demasiado para el buffer',
      'Saving video: {} of {}': 'Guardando vídeo: {} de {}',
      '{} of {} left': 'quedan {2} de {1}',
      'Status: {}': 'Estado: {}',
      Armed: 'Armado',
      '{} frames': '{} fotogramas',
      '{} cuts': '{} cortes',
      'Add {}': 'Añadir {}',
      'Add {} to the goal {}': 'Sumar {} a la meta {}',
      'of {}': 'de {}',
      'of {} running': 'de {} activas',
      'pick a Discord channel': 'elige un canal de Discord',
    },
  });
  assert.equal(tr('Save settings'), 'Guardar ajustes');
  assert.equal(tr('\n    Save   settings  '), '\n    Guardar ajustes  ', 'spaces around kept, inside collapsed');
  assert.equal(tr('30 s is too long for the buffer'), '30 s es demasiado para el buffer');
  assert.equal(tr('Saving video: 12s of 30s'), 'Guardando vídeo: 12s de 30s');
  assert.equal(tr('3 of 5 left'), 'quedan 5 de 3');
  assert.equal(tr('Status: Armed'), 'Estado: Armado', 'a value that is a string of its own is translated too');
  assert.equal(tr('12 frames · 0 cuts · 1 reconnect'), '12 fotogramas · 0 cortes · 1 reconnect');
  assert.equal(tr(' Save settings · '), ' Guardar ajustes · ', 'a piece beside markup keeps its edge');
  assert.equal(tr('Status: 12 frames'), 'Estado: 12 fotogramas', 'a value is looked up whole');
  assert.equal(tr('Add 5 to the goal Subs'), 'Sumar 5 a la meta Subs', 'the most specific key wins');
  assert.equal(tr('Add 5'), 'Añadir 5');
  assert.equal(tr(' of 12 running. '), ' de 12 activas. ', 'the closer pattern wins over a shorter one that also fits');
  assert.equal(tr('Pick a Discord channel'), 'Elige un canal de Discord', 'a phrase capitalized to start a line');
  assert.equal(tr('Something new'), 'Something new');
  assert.equal(tr('42'), '42');
  const kept = { closest: selector => (selector.includes('translate') ? {} : null) };
  assert.equal(tr('Save settings', kept), 'Save settings', 'translate="no" keeps it');
});
