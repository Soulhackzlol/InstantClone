// The OBS dock (web/dock.js): what it says and offers for each state the
// server can report. Run with `node --test "web/tests/*.test.mjs"`.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { element, loadPage, plain, rustConst } from './page.mjs';

// Opened the way OBS opens a named dock on a password-protected dashboard.
const page = loadPage('dock.html', 'http://127.0.0.1:7799/dock?dock=cam%202!&token=ab-12_cd');
const call = (name, ...args) => page.run(name)(...args);



/** What `/state` reports while OBS streams 6 Mbps into a 300 MB buffer. */
const STREAMING = {
  phase: 'idle', ingest_alive: true, armed_delay_ms: 0, buffer_fill_ms: 0,
  buffer_capacity_ms_est: 374_000, capacity_kbps: 6000, capacity_measured: true, capacity_short: null,
  safe_cut_pending: false, safe_cut_remaining_ms: 0,
};
/** Put the dock in a state: server fields, the delay field, and whether the number is open for editing. */
function given(state, { field = 0, editing = false } = {}) {
  page.run(`s = ${JSON.stringify({ ...STREAMING, ...state })}; editing = ${editing}`);
  page.window.document.getElementById('d').value = String(field);
}

test("the dock's platform marks are the dashboard's", () => {
  const table = file => {
    const text = fs.readFileSync(new URL('../' + file, import.meta.url), 'utf8').replace(/\r\n/g, '\n');
    const at = text.indexOf('const PLATFORM_MARK = {');
    return text.slice(at, text.indexOf('};', at));
  };
  assert.ok(table('dock.js').length > 100);
  assert.equal(table('dock.js'), table('index.html'));
});

test("the limits the dock repeats are the app's", () => {
  assert.equal(page.run('DELAY_MAX_S') * 1000, rustConst('src/config.rs', 'MAX_DELAY_MS'));
  assert.equal(page.run('DOCKS_MAX'), rustConst('src/config.rs', 'MAX_DOCKS'));
  assert.equal(page.run('BUF_SLACK_MS'), rustConst('src/controller.rs', 'BUFFER_SLACK_MS'));
});

test('delays, clocks and bitrates read the same everywhere', () => {
  for (const [fn, input, shown] of [
    // One unit for a delay, seconds, however long: the big number says 900.
    ['fmtDelay', 30_000, '30s'],
    ['fmtDelay', 900_000, '900s'],
    ['fmtDelay', 3_600_000, '3600s'],
    ['fmtDelay', 1_499, '1s'],
    // A countdown rounds up: "0:00" only when nothing is left.
    ['fmtClock', 20_000, '0:20'],
    ['fmtClock', 61_000, '1:01'],
    ['fmtClock', 1, '0:01'],
    ['fmtClock', 0, '0:00'],
    ['fmtClock', -500, '0:00'],
    ['fmtRate', 6000, '6.00 Mbps'],
    ['fmtRate', 999.6, '1000 kbps'],
    ['fmtRate', 450, '450 kbps'],
    ['fmtRate', 0, ''],
    ['fmtRate', undefined, ''],
    // The readout: one decimal under 10 s, none above, and no "0.04".
    ['fmt', 40, '0.0'],
    ['fmt', 5_940, '5.9'],
    ['fmt', 9_960, '10'],
    ['fmt', 30_200, '30'],
    ['fmtBehind', 5_940, '5.9s'],
    ['fmtBehind', 60_000, '60s'],
  ]) {
    assert.equal(call(fn, input), shown, `${fn}(${input})`);
  }
});

test('text from the server is escaped before it becomes HTML', () => {
  assert.equal(call('esc', '<img src=x onerror="alert(1)"> & co'), '&lt;img src=x onerror=&quot;alert(1)&quot;&gt; &amp; co');
  assert.equal(call('esc', "<b title='x'>"), '&lt;b title=&#39;x&#39;&gt;');
  assert.equal(call('esc', 42), '42');
  assert.equal(call('esc', null), '');
});

test('a failed request says what to do about it', () => {
  for (const [reply, said, why] of [
    [{ status: 401 }, /missing its access token/, 'a dock link without its token, password on'],
    [{ status: 0 }, /Can't reach InstantClone/, 'the app is not running'],
    [{ status: 409, j: { error: 'Buffer too small for 3m 00s' } }, /^Buffer too small for 3m 00s$/, "the server's own reason"],
    [{ status: 500 }, /^Couldn't arm$/, 'no reason given'],
  ]) {
    assert.match(call('failText', reply, "Couldn't arm"), said, why);
  }
});

test('the dock id and token from the address are cleaned, and links carry them', () => {
  assert.equal(page.run('SLOT'), 'cam2', 'only letters, digits, - and _ survive');
  assert.equal(page.run('TOKEN'), 'ab12cd');
  assert.equal(call('dockUrl', 'second cam'), 'http://127.0.0.1:7799/dock?dock=second%20cam&token=ab12cd');
});

test('a new dock gets a readable id nothing else uses', () => {
  page.run("knownSlots = ['default', 'webcam', 'webcam-2']");
  for (const [name, id] of [
    ['Second PC', 'second-pc'],
    ['  --Ünïcode & Symbols!!  ', 'ncode-symbols'],
    ['Webcam', 'webcam-3'],
    ['Cam2', 'cam2-2'],  // this dock's own id is taken too
    ['!!!', 'dock'],
    ['x'.repeat(60), 'x'.repeat(32)],
  ]) {
    assert.equal(call('slotIdFor', name), id, name);
  }
});

test('a saved layout from an older version still draws every widget', () => {
  const rows = plain(page.run('ROWS'));
  // Unknown rows and repeats go, rows added since are appended.
  assert.deepEqual(plain(call('sanitizeOrder', ['tips', 'gone', 'number', 'tips'])),
    ['tips', 'number', ...rows.filter(id => id !== 'tips' && id !== 'number')]);
  assert.deepEqual(plain(call('sanitizeOrder', 'not a list')), rows);

  const merged = plain(call('mergeCfg', {
    name: '  Left\u0007 monitor  ', density: 'huge', accent: 'neon', order: ['dest'],
    w: { number: { step: 30 }, made_up: { on: true }, bar: 'yes' },
  }));
  const defaults = plain(call('defaultCfg'));
  assert.equal(merged.name, 'Left monitor', 'control characters and padding are dropped');
  assert.equal(merged.density, defaults.density, 'an unknown density keeps the default');
  assert.equal(merged.accent, defaults.accent, 'so does an unknown accent');
  assert.equal(merged.order[0], 'dest');
  assert.deepEqual(merged.order.slice().sort(), rows.slice().sort(), 'every row is still there');
  assert.deepEqual(merged.w.number, { ...defaults.w.number, step: 30 }, 'a saved option wins, the rest are defaults');
  assert.deepEqual(merged.w.bar, defaults.w.bar, 'a widget saved as rubbish keeps its defaults');
  assert.equal(merged.w.made_up, undefined, 'an unknown widget is not carried along');
  assert.deepEqual(plain(call('mergeCfg', null)), defaults);
});

test('the buffer size a delay needs is the one the server asks for', () => {
  // Each row is a refusal the app itself printed, or the changelog's figure:
  // (delay, bitrate) -> "Raise the buffer size to at least N MB".
  for (const [ms, kbps, megabytes, from] of [
    [3_600_000, 18_626, 8532, 'a 1 hour delay refused on an 18.6 Mbps stream'],
    [900_000, 6000, 689, '15 minutes at 6 Mbps, "about 690 MB"'],
    [3_600_000, 6000, 2749, 'an hour at 6 Mbps, "about 2.7 GB"'],
  ]) {
    given({ capacity_kbps: kbps });
    assert.equal(call('requiredMB', ms), megabytes, from);
  }
  given({ capacity_kbps: 0 });
  assert.equal(call('requiredMB', 30_000), 0, 'no bitrate, no figure');
  given({});
  assert.equal(call('requiredMB', 0), 0);
});

test('a delay past what the buffer holds is flagged, firmly only once the bitrate is measured', () => {
  given({ buffer_capacity_ms_est: 125_000, capacity_kbps: 18_500 });
  assert.equal(call('overCap', 125_000), false, 'exactly what it holds still fits');
  assert.equal(call('overCap', 125_001), true);
  assert.match(call('capAdvice', 180_000), /^Buffer too small for 180s\. Raise the buffer size to at least \d+ MB in System, then restart\.$/);

  given({ buffer_capacity_ms_est: 234_000, capacity_measured: false });
  assert.match(call('capAdvice', 600_000), /^Likely too long: at a typical rate the buffer holds about 234s\./);

  given({ buffer_capacity_ms_est: 0 });
  assert.equal(call('overCap', 3_600_000), false, 'no estimate yet refuses nothing');
});

test('the main button offers the one thing that can happen next', () => {
  for (const [why, state, ui, want] of [
    ['OBS not streaming', { ingest_alive: false }, {}, { key: 'wait', label: 'Waiting for OBS', disabled: true }],
    ['OBS not streaming, and the delay typed is past the buffer', { ingest_alive: false, capacity_measured: false }, { field: 600 },
      { key: 'wait-cap', label: 'Buffer holds about 374s', disabled: true }],
    ['streaming, nothing typed', {}, {}, { key: 'set', label: 'Set a delay first', disabled: true }],
    ['streaming, 30 typed', {}, { field: 30 }, { key: 'arm', label: 'Arm 30s' }],
    ['more typed than the server takes', { buffer_capacity_ms_est: 4_000_000 }, { field: 99_999 }, { key: 'arm', label: 'Arm 3600s' }],
    ['more typed than the buffer holds', {}, { field: 600 }, { key: 'cap', label: 'Buffer holds only 374s', disabled: true }],
    ['filling', { phase: 'preparing', armed_delay_ms: 30_000, buffer_fill_ms: 10_000 }, {},
      { key: 'arming', label: 'Ready in 0:20', disabled: true }],
    ['filling, OBS gone', { phase: 'preparing', armed_delay_ms: 30_000, ingest_alive: false }, {},
      { key: 'arming', label: 'Waiting for OBS', disabled: true }],
    ['ready', { phase: 'ready', armed_delay_ms: 30_000 }, {}, { key: 'armed', label: 'Activate 30s' }],
    ['ready, but the buffer only holds part of it', { phase: 'ready', armed_delay_ms: 600_000,
      capacity_short: { armed_ms: 600_000, holds_ms: 374_000 } }, {}, { key: 'armed', label: 'Activate 374s' }],
    ['ready, OBS gone', { phase: 'ready', armed_delay_ms: 30_000, ingest_alive: false }, {},
      { key: 'wait-armed', label: 'Waiting for OBS', disabled: true }],
    ['on air', { phase: 'active', armed_delay_ms: 30_000 }, {}, { key: 'cut', label: 'Cut delay' }],
    ['on air, the number opened and left as it was', { phase: 'active', armed_delay_ms: 30_000 }, { editing: true, field: 30 },
      { key: 'keep', label: 'Keep 30s' }],
    ['on air, a new number typed', { phase: 'active', armed_delay_ms: 30_000 }, { editing: true, field: 45 },
      { key: 'adjust', label: 'Adjust to 45s' }],
    ['armed, a new number typed', { phase: 'ready', armed_delay_ms: 30_000 }, { editing: true, field: 45 },
      { key: 'rearm', label: 'Re-arm at 45s' }],
    ['on air, a number past the buffer typed', { phase: 'active', armed_delay_ms: 30_000 }, { editing: true, field: 600 },
      { key: 'cap', label: 'Buffer holds only 374s', disabled: true }],
  ]) {
    given(state, ui);
    const look = plain(call('mainLook', call('phaseNow')));
    assert.deepEqual({ key: look.key, label: look.label, disabled: look.disabled }, { disabled: undefined, ...want }, why);
  }
});

test('the fill bar under "Ready in" shows how much of the wait is done', () => {
  given({ phase: 'preparing', armed_delay_ms: 40_000, buffer_fill_ms: 10_000 });
  assert.equal(call('mainLook', 'arming').p, 0.25);
  given({ phase: 'preparing', armed_delay_ms: 40_000, buffer_fill_ms: 55_000 });
  assert.equal(call('mainLook', 'arming').p, 1, 'never past full');
});

test('the second button cancels, disarms, or schedules the cut', () => {
  for (const [why, state, title] of [
    ['filling', { phase: 'preparing' }, 'Cancel arming'],
    ['ready', { phase: 'ready' }, 'Disarm'],
    ['on air', { phase: 'active' }, 'Cut to live after everything buffered has aired'],
    ['on air, a cut already waiting', { phase: 'active', safe_cut_pending: true, safe_cut_remaining_ms: 12_400 }, 'Auto-cut armed. Tap to cancel.'],
    ['nothing armed', { phase: 'idle' }, null],
  ]) {
    given(state);
    const look = call('secondaryLook', call('phaseNow'));
    assert.equal(look ? look.title : null, title, why);
  }
  given({ phase: 'active', safe_cut_pending: true, safe_cut_remaining_ms: 12_400 });
  assert.match(call('secondaryLook', 'active').text, /~12s/, 'the countdown to the cut');

  // With "Cut after" switched off in the layout, a cut scheduled from the
  // dashboard still shows: hiding it would lie.
  page.run('cfg.w.activate.safecut = false');
  given({ phase: 'active' });
  assert.equal(call('secondaryLook', 'active'), null);
  given({ phase: 'active', safe_cut_pending: true, safe_cut_remaining_ms: 5_000 });
  assert.notEqual(call('secondaryLook', 'active'), null);
  page.run('cfg.w.activate.safecut = true');
});

test('the delay shown is what the buffer really runs, for the delay it was measured for', () => {
  given({ armed_delay_ms: 600_000, capacity_short: { armed_ms: 600_000, holds_ms: 374_000 } });
  assert.equal(call('reachMs'), 374_000);
  given({ armed_delay_ms: 30_000, capacity_short: { armed_ms: 600_000, holds_ms: 374_000 } });
  assert.equal(call('reachMs'), 30_000, 'a shortfall noted for another delay says nothing about this one');
  given({ armed_delay_ms: 30_000 });
  assert.equal(call('reachMs'), 30_000);
});

test('a profile chip says what a tap will do', () => {
  given({});
  for (const [state, apply, current, title] of [
    ['off', true, false, 'Arm 30s'],
    ['off', false, false, 'Set 30s'],
    ['arming', true, false, 'Wait for the buffer to fill, or cancel arming'],
    ['armed', true, false, 'Re-arm at 30s'],
    ['armed', false, false, 'Pick 30s, then Re-arm'],
    ['active', true, false, 'Adjust to 30s'],
    ['active', false, false, 'Pick 30s, then Adjust'],
    ['active', true, true, 'Standard: running now'],
  ]) {
    page.run(`cfg.w.profiles.arm = ${apply}`);
    assert.equal(call('profileTitle', 'Standard', 30_000, state, current), title, `${state}, one-tap ${apply}`);
  }
});

/** A destination row as `destRow()` builds it, without the page. */
function row() {
  return Object.assign(element(), { _glyph: element(), _name: element(), _meta: element(), _sw: element('button') });
}
const REFUSED = { advice: 'Stream key rejected. Check the key in Edit.', retries: true, retry_in_ms: 4_200 };

test('a destination row says how it stands', () => {
  page.run("cfg.w.dest.stats = true; destPlatforms = { main: 'twitch' }");
  for (const [why, streaming, dest, state, meta] of [
    ['switched off', true, { enabled: false, alive: false }, 'off', 'twitch'],
    ['live', true, { enabled: true, alive: true, bitrate_kbps: 6000 }, 'alive', '6.00 Mbps'],
    ['connected, nothing sent yet', true, { enabled: true, alive: true, bitrate_kbps: 0 }, 'wait', 'Connecting…'],
    ['on the reconnect screen', true, { enabled: true, alive: true, bitrate_kbps: 900, on_hold: true }, 'held', 'Holding'],
    ["can't connect", true, { enabled: true, alive: false, failure: REFUSED }, 'wait', "Can't connect"],
    ['OBS not streaming', false, { enabled: true, alive: false }, 'idle', 'twitch'],
  ]) {
    given({ ingest_alive: streaming });
    const drawn = row();
    call('patchDestRow', drawn, { id: 'main', name: 'Main', ...dest });
    assert.equal(call('destState', dest), state, why);
    assert.equal(drawn._meta.textContent, meta, why);
    assert.equal(drawn._sw.getAttribute('aria-checked'), String(!!dest.enabled), why);
  }
  // With "Show bitrate" off (the default) a live row names its platform,
  // beside the platform's mark.
  page.run('cfg.w.dest.stats = false');
  const quiet = row();
  call('patchDestRow', quiet, { id: 'main', name: 'Main', enabled: true, alive: true, bitrate_kbps: 6000 });
  assert.equal(quiet._meta.textContent, 'twitch');
  assert.equal(quiet._glyph.innerHTML, page.run('PLATFORM_MARK.twitch'));
});

test('why a destination failed is on its row only while it is failing', () => {
  given({});
  const failing = row();
  call('patchDestRow', failing, { id: 'a', name: 'Twitch', enabled: true, alive: false, failure: REFUSED });
  assert.equal(failing.title, 'Stream key rejected. Check the key in Edit. Next try in 5s.');

  // The server keeps the last reason for a while after a reconnect. A live
  // row must not carry it: "Stream key rejected" on a stream that is up.
  page.run('cfg.w.dest.stats = true');
  const recovered = row();
  call('patchDestRow', recovered, { id: 'a', name: 'Twitch', enabled: true, alive: true, bitrate_kbps: 6000, failure: REFUSED });
  assert.equal(recovered.title, '');
  assert.equal(recovered._meta.textContent, '6.00 Mbps');
});
