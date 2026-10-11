// The dashboard's own script (the inline one in web/index.html): how it
// words durations and sizes, the buffer maths it shares with the server,
// what a destination card says, and the Logs tab's reading of a line.
import test from 'node:test';
import assert from 'node:assert/strict';
import { loadPage, plain, rustConst } from './page.mjs';

const page = loadPage('index.html');
const call = (name, ...args) => page.run(name)(...args);
/** What `/state`, `/config` and `/destinations` last answered, as the page keeps them. */
function given({ state = null, config = null, dests = [] } = {}) {
  page.run(`lastState = ${JSON.stringify(state)}; lastConfig = ${JSON.stringify(config)}; lastDests = ${JSON.stringify(dests)}`);
}

test('the limits the page repeats are the app\'s', () => {
  assert.equal(page.run('DELAY_MAX_S') * 1000, rustConst('src/config.rs', 'MAX_DELAY_MS'));
  assert.equal(page.run('BUF_SLACK_MS'), rustConst('src/controller.rs', 'BUFFER_SLACK_MS'));
  assert.equal(page.run('BUF_REF_KBPS'), rustConst('src/controller.rs', 'CAPACITY_REFERENCE_KBPS'));
});

test('durations, rates and sizes are worded one way', () => {
  for (const [fn, input, shown] of [
    // The way the app's own refusals write a duration.
    ['fmtDelay', 45_000, '45s'],
    ['fmtDelay', 59_500, '1m'],
    ['fmtDelay', 63_000, '1m 03s'],
    ['fmtDelay', 300_000, '5m'],
    ['fmtDelay', 3_600_000, '1h'],
    ['fmtDelay', 5_400_000, '1h 30m'],
    ['fmtUptime', -5, '0s'],
    ['fmtUptime', 59, '59s'],
    ['fmtUptime', 61, '1m 1s'],
    ['fmtUptime', 3_700, '1h 1m'],
    ['fmtUptime', 90_000, '1d 1h'],
    ['fmtKbps', 999, '999 kbps'],
    ['fmtKbps', 6000, '6.00 Mbps'],
    ['fmtLogTime', NaN, ''],
    ['fmtLogTime', 59.9, '0:59'],
    ['fmtLogTime', 61, '1:01'],
    ['fmtLogTime', 3_723, '1:02:03'],
  ]) {
    assert.equal(call(fn, input), shown, `${fn}(${input})`);
  }
  for (const [bytes, shown] of [
    [0, ['0', 'B']],
    [1_536, ['1.5', 'KB']],
    [300 * 1_048_576, ['300', 'MB']],
    [2.7 * 1024 ** 3, ['2.7', 'GB']],
  ]) {
    assert.deepEqual(plain(call('fmtBytesPair', bytes)), shown, `${bytes} bytes`);
  }
});

test('how long a buffer holds is worked out like the server does', () => {
  // Each row is what a running app reported for that buffer and bitrate.
  for (const [megabytes, kbps, seconds, from] of [
    [50, 2318, 167, '"it holds about 2m 47s at 2.3 Mbps"'],
    [3072, 6140, 3932, 'the 3 GB buffer of the one-hour run'],
  ]) {
    assert.equal(Math.floor(call('bufferCapMs', megabytes, kbps) / 1000), seconds, from);
  }
  assert.equal(call('bufferCapMs', 0, 6000), 0);
  assert.equal(call('bufferCapMs', -5, 6000), 0);
  assert.equal(call('bufferCapMs', 1, 50_000_000), 0, 'never negative, however small the buffer');
});

test('the buffer size advised for a delay is the smallest that holds it', () => {
  for (const [ms, kbps, megabytes, from] of [
    [3_600_000, 18_626, 8532, 'a 1 hour delay refused on an 18.6 Mbps stream'],
    [900_000, 6000, 689, '15 minutes at 6 Mbps, "about 690 MB"'],
    [3_600_000, 6000, 2749, 'an hour at 6 Mbps, "about 2.7 GB"'],
  ]) {
    given({ state: { capacity_kbps: kbps } });
    assert.equal(call('requiredMBForMs', ms), megabytes, from);
  }
  // Whatever the delay and bitrate, the size it names really holds the
  // delay, and one megabyte less does not.
  for (const kbps of [500, 2318, 6000, 18_626, 50_000]) {
    given({ state: { capacity_kbps: kbps } });
    for (const ms of [1_000, 30_000, 600_000, 3_600_000]) {
      const megabytes = call('requiredMBForMs', ms);
      assert.ok(call('bufferCapMs', megabytes, kbps) >= ms, `${megabytes} MB holds ${ms} ms at ${kbps} kbps`);
      assert.ok(call('bufferCapMs', megabytes - 1, kbps) < ms, `${megabytes - 1} MB does not`);
    }
  }
  // Before a stream is measured, the server plans at 10 Mbps, and so does the page.
  given({});
  assert.equal(call('planningKbps'), 10_000);
  assert.equal(call('requiredMBForMs', 0), 0);
});

test('"buffer too small" says to restart when the bigger size is already saved', () => {
  given({ state: { capacity_kbps: 6000, restart_pending: true }, config: { buffer_mb: 3000 } });
  assert.match(call('bufferAdviceHtml', 3_600_000), /Your new 3000 MB buffer is saved and starts with the next restart/);
  // Saved, but still too small for this delay: raise it further.
  given({ state: { capacity_kbps: 6000, restart_pending: true }, config: { buffer_mb: 1000 } });
  assert.match(call('bufferAdviceHtml', 3_600_000), /Raise the buffer in System.*data-mb="2749"/);
  given({ state: { capacity_kbps: 6000, restart_pending: false }, config: { buffer_mb: 3000 } });
  assert.match(call('bufferAdviceHtml', 900_000), /data-mb="689"/, 'no restart pending: name the size');
});

const REFUSED = { advice: 'Stream key rejected. Check the key in Edit.', detail: 'closed after publish', retries: true, retry_in_ms: 4_200 };
/** A destination card, updated once with `dest`, as [status, its style, big line, is it marked failing]. */
function card(dest) {
  const full = { id: 'd1', name: 'Main', platform: 'kick', enabled: true, alive: false, ...dest };
  const refs = call('buildDestCard', full);
  call('updateDestCard', refs, full);
  const style = refs.statE.className.replace('dcard-status s-', '');
  return { refs, shows: [refs.statL.textContent, style, String(refs.rate.textContent), refs.el.classList.contains('failing')] };
}

test('a destination card says what it is doing', () => {
  for (const [why, obsStreaming, dest, shows] of [
    ['switched off', true, { enabled: false }, ['Disabled', 'off', 'Off. Switch it on to stream here.', false]],
    ['waiting for OBS', false, {}, ['Idle', 'off', 'Goes live when OBS does', false]],
    ['OBS streaming, not connected yet', true, {}, ['Idle', 'off', 'Connecting…', false]],
    ['connected, no video yet', true, { alive: true, bitrate_kbps: 0 }, ['Connected', 'ready', 'Connected, waiting for video', false]],
    ['live', true, { alive: true, bitrate_kbps: 6000 }, ['Streaming', 'live', '6.00', false]],
    ['live under 1 Mbps', true, { alive: true, bitrate_kbps: 449.6 }, ['Streaming', 'live', '450', false]],
    ['held by crash protection', true, { alive: true, bitrate_kbps: 900, on_hold: true }, ['Reconnect screen', 'wait', '900', false]],
    ['refused, waiting to retry', true, { failure: REFUSED }, ['Retrying', 'wait', 'Not connected', true]],
    ['refused, trying again now', true, { failure: { ...REFUSED, retry_in_ms: null } }, ['Connecting', 'wait', 'Connecting…', true]],
    // Nothing will try again until it is edited: it must not read "Connecting".
    ['a bad address, nothing to retry', true, { failure: { ...REFUSED, retries: false, retry_in_ms: null } }, ['Not connected', 'wait', 'Not connected', true]],
    // The server keeps the last reason for a while after a reconnect.
    ['live again, the old reason still in the state', true, { alive: true, bitrate_kbps: 6000, failure: REFUSED }, ['Streaming', 'live', '6.00', false]],
    ['on the reconnect screen with an old reason', true, { alive: true, bitrate_kbps: 900, on_hold: true, failure: REFUSED }, ['Reconnect screen', 'wait', '900', false]],
  ]) {
    given({ state: { ingest_alive: obsStreaming } });
    assert.deepEqual(card(dest).shows, shows, why);
  }
});

test('a failing card says why, and when it tries again', () => {
  given({ state: { ingest_alive: true } });
  for (const [failure, next, retryHidden] of [
    [REFUSED, 'Next try in 5 s', false],
    [{ ...REFUSED, retry_in_ms: 300 }, 'Next try in 1 s', false],
    [{ ...REFUSED, retry_in_ms: null }, 'Trying again now…', true],
    [{ ...REFUSED, retries: false, retry_in_ms: null }, 'Fix it in Edit, then it connects.', true],
  ]) {
    const { refs } = card({ failure });
    assert.equal(refs.whyAdvice.textContent, REFUSED.advice);
    assert.equal(refs.whyDetail.textContent, REFUSED.detail);
    assert.equal(refs.whyNext.textContent, next);
    assert.equal(refs.whyRetry.hidden, retryHidden, `Retry now for "${next}"`);
  }
});

test('a card counts frames, cuts and reconnects once there is something to count', () => {
  given({ state: { ingest_alive: true } });
  assert.equal(card({ alive: true }).refs.meta.hidden, true, 'nothing sent yet: no counters');
  for (const [dest, text] of [
    [{ tags_sent: 1, cuts: 1, reconnects: 1 }, '1 frame · 1 cut · 1 reconnect'],
    [{ tags_sent: 431, cuts: 2, reconnects: 0 }, '431 frames · 2 cuts · 0 reconnects'],
    [{ tags_sent: 12_540, cuts: 0, reconnects: 3 }, '12.5k frames · 0 cuts · 3 reconnects'],
    [{ tags_sent: 2_300_000 }, '2.3M frames · 0 cuts · 0 reconnects'],
  ]) {
    const { refs } = card({ alive: true, bitrate_kbps: 6000, ...dest });
    assert.equal(refs.meta.hidden, false);
    assert.equal(refs.meta.textContent, text);
  }
});

test('a vertical destination with no 9:16 canvas says what to change, and where', () => {
  const vertical = { platform: 'kick', enabled: true, stream_format: 'vertical', vertical_ready: false };
  assert.equal(call('isVerticalDest', vertical), true);
  assert.equal(call('isVerticalDest', { ...vertical, platform: 'twitch' }), false, 'Twitch takes both canvases itself');
  assert.equal(call('isVerticalDest', { ...vertical, enabled: false }), false);

  const twitch = { platform: 'twitch', enabled: true, stream_key_set: true };
  for (const [why, state, dests, dest, want] of [
    ['a canvas is arriving', { ingest_alive: true }, [], { ...vertical, vertical_ready: true }, { ready: true }],
    ['the state lags, but one is on the wire', { ingest_alive: true, vertical_present: true }, [], vertical, { ready: true }],
    ['OBS is not streaming', { ingest_alive: false }, [], vertical, { ready: true }],
    ['Enhanced Broadcasting off', { ingest_alive: true, multitrack_video: false }, [], vertical,
      { ready: false, label: 'Needs OBS setup', hint: 'Turn on Enhanced Broadcasting in OBS' }],
    ['Enhanced Broadcasting on, no extra canvas', { ingest_alive: true, multitrack_video: true }, [], vertical,
      { ready: false, label: 'No 9:16 canvas', hint: 'In OBS, pick an Additional canvas' }],
    ["Twitch's config decides, and has no vertical", { ingest_alive: true, multitrack_video: true }, [twitch], vertical,
      { ready: false, label: 'No 9:16 canvas', hint: 'Twitch sends no vertical track (Dual Format is off)' }],
    ['Enhanced Broadcasting off, with a Twitch destination: still an OBS setting', { ingest_alive: true, multitrack_video: false },
      [twitch], vertical, { ready: false, label: 'Needs OBS setup', hint: 'Turn on Enhanced Broadcasting in OBS' }],
    ['a Twitch destination that is switched off decides nothing', { ingest_alive: true, multitrack_video: true },
      [{ ...twitch, enabled: false }], vertical, { ready: false, label: 'No 9:16 canvas', hint: 'In OBS, pick an Additional canvas' }],
  ]) {
    given({ state, dests });
    const got = plain(call('verticalDiag', dest));
    assert.deepEqual({ ready: got.ready, label: got.label, hint: got.hint }, { label: undefined, hint: undefined, ...want }, why);
  }
  given({ state: { ingest_alive: true, multitrack_video: false } });
  assert.deepEqual(card(vertical).shows, ['Needs OBS setup', 'wait', 'Connecting…', false], 'and the card leads with it');
});

test('a log line is read for when, who and how it went', () => {
  for (const [line, want] of [
    ['[+  12.345s] [Main] connect failed (refused), retrying in 2s',
      { secs: 12.345, source: 'Main', level: 'warn', message: 'connect failed (refused), retrying in 2s' }],
    ['[+ 5.0s] [Main] disconnected (connection reset)', { secs: 5, source: 'Main', level: 'err', message: 'disconnected (connection reset)' }],
    // Something that failed but is being tried again is a warning.
    ["[+ 501.697s] [twitch] events: Twitch's reconnect didn't work out; retrying in 5 s",
      { secs: 501.697, source: 'twitch', level: 'warn', message: "events: Twitch's reconnect didn't work out; retrying in 5 s" }],
    ['[+ 0.5s] ingest: publisher connected', { secs: 0.5, source: 'OBS', level: 'ok', message: 'ingest: publisher connected' }],
    ['[+ 9s] pre-arm: stream started -> armed 30 s of delay',
      { secs: 9, source: 'Delay', level: 'ok', message: 'pre-arm: stream started -> armed 30 s of delay' }],
    ['[+ 2s] [hotkey] delay off - cut to live', { secs: 2, source: 'hotkey', level: 'info', message: 'delay off - cut to live' }],
    ['[+ 4s] the buffer is too small for 600 s', { secs: 4, source: 'App', level: 'warn', message: 'the buffer is too small for 600 s' }],
  ]) {
    assert.deepEqual(plain(call('parseLogLine', line)), want, line);
  }
  const bare = call('parseLogLine', 'a line with no stamp');
  assert.ok(Number.isNaN(bare.secs));
  assert.equal(bare.message, 'a line with no stamp');
});

test('a log source keeps its colour', () => {
  const hues = plain(page.run('LOG_HUES'));
  for (const name of ['Main', 'twitch', 'OBS', 'Delay', 'App', 'Kick Test', '']) {
    assert.ok(hues.includes(call('logSourceHue', name)), `${name} gets a hue from the palette`);
    assert.equal(call('logSourceHue', name), call('logSourceHue', name));
  }
});

test('new log lines are added to the view without rebuilding it', () => {
  for (const [why, shown, now, kept] of [
    ['one new line', ['a', 'b', 'c'], ['a', 'b', 'c', 'd'], 3],
    ['nothing new', ['a', 'b', 'c'], ['a', 'b', 'c'], 3],
    ['the log is full, the oldest fell off', ['a', 'b', 'c'], ['b', 'c', 'd'], 2],
    ['cleared', ['a', 'b', 'c'], ['x', 'y'], -1],
    ['the app restarted with fewer lines', ['a', 'b', 'c'], ['a', 'b'], -1],
    // The last line shown turns up again, but what is before it is not
    // what was shown: a restart that logs the same line, not a continuation.
    ['a restart that repeats the last line', ['a', 'b', 'c'], ['x', 'c'], -1],
    ['more history than was shown', ['c'], ['a', 'b', 'c'], -1],
    ['nothing was shown yet', [], ['a'], -1],
  ]) {
    assert.equal(call('logOverlap', shown, now), kept, why);
  }
});

test('the color picker reads a color and writes the same one back', () => {
  for (const hex of [...plain(page.run('CP_APP_COLORS')), '#000000', '#ffffff', '#ff0000', '#00ff00', '#0000ff', '#808080', '#11141a']) {
    assert.equal(call('hsvToHex', call('hexToHsv', hex)), hex, hex);
  }
  assert.deepEqual(plain(call('hexToHsv', 'FF0000')), { h: 0, s: 1, v: 1 }, 'with or without the #, any case');
  assert.deepEqual(plain(call('hexToHsv', '#00ff00')), { h: 120, s: 1, v: 1 });
  // Anything that is not a color opens the picker on the app's accent.
  for (const rubbish of ['', 'red', '#12', '#gggggg', null, undefined]) {
    assert.equal(call('hsvToHex', call('hexToHsv', rubbish)), '#5ac8fa', String(rubbish));
  }
});

test('a pressed key is named the way the app saves it', () => {
  for (const [code, name] of [
    ['KeyK', 'K'], ['Digit7', '7'], ['Numpad3', 'Numpad3'], ['F1', 'F1'], ['F12', 'F12'], ['F24', 'F24'],
    ['F25', null], ['F0', null], ['KeyKK', null],
  ]) {
    assert.equal(call('hkMainKey', code), name, code);
  }
  // Modifiers always in the server's order, whatever was pressed first.
  assert.deepEqual(plain(call('hkMods', { metaKey: true, shiftKey: true, altKey: true, ctrlKey: true })), ['Ctrl', 'Alt', 'Shift', 'Win']);
  assert.deepEqual(plain(call('hkMods', { shiftKey: true })), ['Shift']);
  assert.deepEqual(plain(call('hkMods', {})), []);
});

test('a MIDI pad is named by what it is, and which device', () => {
  for (const [signature, full, label] of [
    ['note:1:36', false, 'Note 36 · ch 1'],
    ['cc:10:7', false, 'CC 7 · ch 10'],
    ['note:1:36@Launchpad', false, 'Note 36 · Launchpad'],
    ['cc:1:7@A very long MIDI controller name', false, 'CC 7 · A very long MIDI …'],
    ['cc:1:7@A very long MIDI controller name', true, 'CC 7 · A very long MIDI controller name'],
    ['not-a-pad', false, 'not-a-pad'],
    ['', false, ''],
    [null, false, ''],
  ]) {
    assert.equal(call('midiLabel', signature, full), label, String(signature));
  }
});
