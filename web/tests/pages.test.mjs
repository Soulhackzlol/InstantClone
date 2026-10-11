// Every page the app serves: its scripts have to start on an empty page
// without throwing. A typo at the top level of one would otherwise only
// show as a blank dashboard, dock or alerts source in OBS.
import test from 'node:test';
import assert from 'node:assert/strict';
import { loadPage, rustList } from './page.mjs';

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
