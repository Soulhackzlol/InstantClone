// The Integrations tab (web/integrations.js). Its previews repeat rules the
// app applies when an integration really runs: how a template fills in, how
// a check compares, when quiet hours hold. Where the script says "Mirrors
// <a Rust function>", the cases here are that function's own test cases, so
// the preview and the run can't drift apart unnoticed.
import test from 'node:test';
import assert from 'node:assert/strict';
import { loadPage, plain, rustConst } from './page.mjs';

const page = loadPage('index.html');
/** Call one of the script's private functions. */
const call = (name, ...args) => page.inside(name)(...args);

/** What `GET /integrations` answers, cut down to what the helpers read. */
const OVERVIEW = {
  integrations: [],
  obs: { current: 'Gaming' },
  events: [
    { id: 'hold_opened', label: 'OBS crashed', vars: [{ name: 'reason', sample: 'crash' }] },
    { id: 'onair_changed', label: 'On air changes', vars: [{ name: 'state', sample: 'delay' }] },
  ],
  vars: {
    global: [{ name: 'delay', sample: '30s' }, { name: 'phase', sample: 'active' }],
    chat: [{ name: 'user', sample: 'viewer1' }, { name: 'args', sample: '' }, { name: 'arg1', sample: '' }],
    web: [{ name: 'body', sample: '' }],
    activity: [],
    scene: [{ name: 'scene', sample: 'Gaming' }],
  },
};
function given(overview = {}) {
  page.inside(`S.data = ${JSON.stringify({ ...OVERVIEW, ...overview })}`);
}
/** Freeze the page's clock at a local time of day. */
function at(hours, minutes) {
  page.window.__now = new Date(2026, 9, 10, hours, minutes).getTime();
  page.run(`Date = class extends Date {
    constructor(...given) { if (given.length) super(...given); else super(__now); }
    static now() { return __now; }
  }`);
}
given();

test('the limits the builder repeats are the app\'s', () => {
  assert.equal(page.inside('MAX_NEST'), rustConst('src/integrations/model.rs', 'MAX_DEPTH'));
  assert.equal(page.inside('MAX_CONDITIONS'), rustConst('src/integrations/model.rs', 'MAX_CONDITIONS'));
});

test('a template previews the way it will be sent', () => {
  // The cases of template.rs's own tests.
  const vars = {
    delay: '30s', off: '', zero: '0', 'clip.url': 'https://clips.twitch.tv/x',
    one: '1', many: '42', muted: 'yes', live: 'no', web: ' True ',
  };
  for (const [template, sent] of [
    ['Delay: {delay}', 'Delay: 30s'],
    ['{clip.url}!', 'https://clips.twitch.tv/x!'],
    // A fallback covers a value that is empty or zero.
    ['Delay: {off|off}', 'Delay: off'],
    ['{zero|none}', 'none'],
    ['{delay|off}', '30s'],
    ['{missing|n/a}', 'n/a'],
    ['a{nope}b', 'ab'],
    // A count takes the word that fits it.
    ['{one#viewer#viewers}', '1 viewer'],
    ['{many#viewer#viewers}', '42 viewers'],
    ['{none#sub#subs}', '0 subs'],
    ['{one#viewer}', '{one#viewer}'],
    // Yes or no picks its text.
    ['mic {muted?muted:unmuted}', 'mic muted'],
    ['{live?live:offline}', 'offline'],
    ['{web?on:off}', 'on'],
    ['{none?a:b}', 'b'],
    ['{muted?a: b: c}', 'a'],
    ['{muted?no colon}', '{muted?no colon}'],
    // Doubled braces are text, and anything unfinished is left alone.
    ['{{delay}} is {delay}', '{delay} is 30s'],
    ['a }} b {{ c', 'a }} b {{ c'],
    ['{delay', '{delay'],
    ['{delay|off', '{delay|off'],
    ['{ spaced }', '{ spaced }'],
    ['{"event":"{delay}","n":{"x":1}}', '{"event":"30s","n":{"x":1}}'],
  ]) {
    assert.equal(call('renderSample', template, vars), sent, template);
  }
});

test('values going into a web address or JSON are escaped, the text around them is not', () => {
  const urlComponent = page.inside('urlComponent');
  assert.equal(call('renderSample', 'x?q={q}&r={none|a b}', { q: 'a b&c=d/é' }, urlComponent), 'x?q=a%20b%26c%3Dd%2F%C3%A9&r=a b');
  assert.equal(call('renderSample', '{"n":"{q}"}', { q: 'a","admin":true' }, page.inside('jsonStringContent')), '{"n":"a\\",\\"admin\\":true"}');
  assert.equal(urlComponent('..'), '%2E%2E', 'a value can never climb a path');
  assert.equal(urlComponent('.'), '%2E');
  assert.equal(urlComponent('v1.2'), 'v1.2');
  assert.equal(urlComponent("it's (ok)!*"), 'it%27s%20%28ok%29%21%2A', 'only unreserved characters stay');
  assert.equal(call('oneLine', 'a\r\nX-Evil: 1'), 'a  X-Evil: 1', 'a value can never add a header');
});

test('"Edit text" previews what the step will keep', () => {
  for (const [op, input, a, b, kept] of [
    ['first_line', '\n  Radiant \n450 RR', '', '', 'Radiant'],
    ['between', 'rank: Gold 2 (34 RR)', 'rank:', '(', 'Gold 2'],
    ['between', 'a=1&b=2', 'b=', '', '2'],
    ['between', 'no marker', 'x=', '', ''],
    ['replace', 'a-b-c', '-', ' ', 'a b c'],
    ['replace', 'abc', '', 'x', 'abc'],
    ['cut', 'hello world', '5', '', 'hello…'],
    ['cut', 'hi', '5', '', 'hi'],
    ['round', ' 3.14159 ', '2', '', '3.14'],
    ['round', '2.5e1', '', '', '25'],
    ['round', 'n/a', '2', '', 'n/a'],
    ['digits', '12571578', '', '', '12,571,578'],
    ['digits', '-1000', '', '', '-1,000'],
    ['digits', '999', '', '', '999'],
    ['digits', '1.5', '', '', '1.5'],
    ['upper', 'gg', '', '', 'GG'],
    ['lower', 'GG', '', '', 'gg'],
    ['nope', 'same', '', '', 'same'],
    // "Pick one" really picks at random; the preview shows the first.
    ['random', 'Yes | No | Ask again', '', '', 'Yes'],
    ['random', 'only\n\n', '', '', 'only'],
    ['random', ' | ', '', '', ''],
  ]) {
    assert.equal(call('editText', op, input, a, b), kept, `${op}(${JSON.stringify(input)})`);
  }
});

test('a check compares the way a run does', () => {
  for (const [left, op, right, passes] of [
    ['Crash', 'is', 'crash', true],
    ['Crash', 'is_not', 'crash', false],
    ['12', 'greater', '9', true],
    ['abc', 'greater', '9', false],
    ['9', 'less', '12', true],
    ['', 'less', '12', false],
    ['', 'empty', '', true],
    ['no', 'empty', '', true],
    ['0', 'empty', '', true],
    ['yes', 'not_empty', '', true],
    ['hello world', 'contains', 'WORLD', true],
    ['hello world', 'not_contains', 'mars', true],
    ['!Setdelay 30', 'starts_with', '!setdelay', true],
    ['30', 'whole_number', '', true],
    ['2.5', 'whole_number', '', false],
    ['', 'whole_number', '', false],
    ['-3', 'whole_number', '', false],
    ['x', 'bogus_op', 'x', false],
  ]) {
    assert.equal(call('compareSample', left, op, right), passes, `"${left}" ${op} "${right}"`);
  }
});

test('the preview follows checks to the step a run would reach', () => {
  // !setdelay as shipped: 0 turns it off and stops, anything else is set.
  const off = { type: 'chat', params: { text: 'Delay turned off' } };
  const set = { type: 'chat', params: { text: 'Delay set' } };
  const steps = [
    { type: 'if', params: { left: '{arg1}', op: 'is', right: '0' }, then: [{ type: 'delay_action', params: {} }, off, { type: 'stop', params: {} }], else: [] },
    { type: 'delay_action', params: {} },
    set,
  ];
  assert.equal(call('reachedStep', steps, { arg1: '0' }, 'chat'), off);
  assert.equal(call('reachedStep', steps, { arg1: '30' }, 'chat'), set);
  assert.equal(call('reachedStep', steps, { arg1: '30' }, 'discord'), undefined, 'no such step on the way');
  assert.equal(call('reachedStep', [{ type: 'stop', params: {} }, set], {}, 'chat'), null, 'a Stop comes first');
});

test('a variable nothing fills is flagged, with the name most likely meant', () => {
  const fetch = { type: 'http', params: { url: 'https://api.example/rank', save_as: 'api' } };
  const reply = { type: 'chat', params: { text: 'Hi {usr}, {arg2}: {api.body} {api.json.rank|?} {counter.wins} {{usr}} {delay|off} {text} {zzzzzz}' } };
  const shorten = { type: 'edit_text', params: { op: 'cut', input: '{api.body}', save_as: 'text' } };
  const handler = { trigger: { type: 'chat_command', command: '!rank' }, steps: [fetch, reply, shorten] };

  const flagged = plain(call('unknownVars', handler, reply));
  assert.deepEqual(flagged, [
    { name: 'usr', hint: 'user' },
    // A later step makes {text}: here it would still send as nothing.
    { name: 'text', hint: '' },
    { name: 'zzzzzz', hint: '' },
  ]);
  assert.deepEqual(plain(call('unknownVars', handler, shorten)), [], 'what an earlier step made is known');
  assert.deepEqual(plain(call('unknownVars', handler, { type: 'chat', params: { text: 'plain words' } })), []);

  // A web call's own fields are open-ended; a chat command's are not.
  const hook = { trigger: { type: 'webhook' }, steps: [] };
  assert.deepEqual(plain(call('unknownVars', hook, { type: 'chat', params: { text: '{query.who} {body.event} {arg1}' } })),
    [{ name: 'arg1', hint: '' }]);
});

test('"Did you mean" is two edits away at most', () => {
  for (const [typed, real, edits] of [
    ['usr', 'user', 1], ['viewers', 'viewer', 1], ['dealy', 'delay', 2], ['delay', 'delay', 0],
    // Lengths more than two apart are not worth measuring.
    ['d', 'delay', 3], ['destinations_live', 'delay', 3],
  ]) {
    assert.equal(call('editDistance', typed, real), edits, `${typed} -> ${real}`);
  }
  assert.equal(call('closestName', 'USR', ['delay', 'user', 'users']), 'user', 'case does not count');
  assert.equal(call('closestName', 'reason', ['delay', 'user']), '', 'nothing close: no suggestion');
});

test('the one-click fix renames the variable everywhere, and only the variable', () => {
  const handler = { steps: [
    { type: 'chat', params: { text: 'Hi {usr}! {usr|someone} {usr#fan#fans} {{usr}} {usrname}' } },
    { type: 'if', params: { left: '{usr}', op: 'is', right: 'me' }, then: [{ type: 'chat', params: { text: '{usr}' } }], else: [] },
  ] };
  call('renameVar', handler, 'usr', 'user');
  assert.equal(handler.steps[0].params.text, 'Hi {user}! {user|someone} {user#fan#fans} {{usr}} {usrname}');
  assert.equal(handler.steps[1].params.left, '{user}');
  assert.equal(handler.steps[1].then[0].params.text, '{user}', 'inside a check too');
});

test('quiet hours hold from their start up to their end, past midnight too', () => {
  // The cases of QuietHours::contains.
  for (const [quiet, hours, minutes, held] of [
    [{ from: '23:00', to: '07:00' }, 23, 0, true],
    [{ from: '23:00', to: '07:00' }, 3, 30, true],
    [{ from: '23:00', to: '07:00' }, 6, 59, true],
    [{ from: '23:00', to: '07:00' }, 7, 0, false],
    [{ from: '23:00', to: '07:00' }, 22, 59, false],
    [{ from: '09:00', to: '17:00' }, 9, 0, true],
    [{ from: '09:00', to: '17:00' }, 16, 59, true],
    [{ from: '09:00', to: '17:00' }, 17, 0, false],
    [{ from: '09:00', to: '17:00' }, 8, 59, false],
    [null, 12, 0, false],
    [{ from: '', to: '' }, 12, 0, false],
  ]) {
    at(hours, minutes);
    assert.equal(call('quietNow', quiet), held, `${JSON.stringify(quiet)} at ${hours}:${String(minutes).padStart(2, '0')}`);
  }
});

test('two switched-on integrations answering one chat command are caught', () => {
  const command = (word, aliases = []) => ({ enabled: true, trigger: { type: 'chat_command', command: word, aliases }, steps: [] });
  const mine = { id: 'a', name: 'Mine', enabled: true, handlers: [command('!delay', ['!d'])] };
  const other = (changes = {}) => ({ id: 'b', name: 'Other', enabled: true, handlers: [command('!lag', ['!d'])], ...changes });

  given({ integrations: [mine, other()] });
  assert.deepEqual(plain(call('commandClash', mine)), { word: '!d', name: 'Other' }, 'an alias clashes like a command');
  given({ integrations: [mine, other({ enabled: false })] });
  assert.equal(call('commandClash', mine), null, 'one that is switched off answers nothing');
  given({ integrations: [mine, other({ handlers: [{ ...command('!d'), enabled: false }] })] });
  assert.equal(call('commandClash', mine), null, 'nor does a switched-off trigger');
  given({ integrations: [mine] });
  assert.equal(call('commandClash', mine), null, 'it never clashes with itself');
  assert.equal(call('commandClash', { id: 'c', handlers: [{ enabled: true, trigger: { type: 'timer' }, steps: [] }] }), null);
  given();
});

test('the builder only lets a check go where the app will save it', () => {
  const step = { type: 'chat', params: {} };
  const check = (...inside) => ({ type: 'if', params: {}, then: inside, else: [] });
  assert.equal(call('nestLevels', step), 0);
  assert.equal(call('nestLevels', check()), 1, 'an empty check still opens its own lanes');
  assert.equal(call('nestLevels', check(step, check(check()))), 3);
  assert.equal(call('nestLevels', { type: 'if', params: {}, then: [], else: [check(check())] }), 3, 'the otherwise lane counts too');

  const top = ['steps'];
  const inOne = ['steps', 0, 'then'];
  const inThree = ['steps', 0, 'then', 1, 'else', 0, 'then'];
  const fits = page.inside('nestFits');
  // Four levels at most, the top list being the first: three checks deep.
  assert.equal(fits(top, 3), true);
  assert.equal(fits(top, 4), false);
  assert.equal(fits(inOne, 2), true);
  assert.equal(fits(inOne, 3), false);
  assert.equal(fits(inThree, 0), true, 'a plain step fits in the deepest lane');
  assert.equal(fits(inThree, 1), false, 'a fourth check does not');
});

test('a length of time is read and written the same way', () => {
  for (const [text, unit, ms] of [
    ['90', 's', 90_000], ['2', 'm', 120_000], ['10s', 's', 10_000], ['10 sec', 's', 10_000], ['2m', 's', 120_000],
    ['1m 30s', 's', 90_000], ['1.5m', 's', 90_000], ['1,5m', 's', 90_000], ['1h', 's', 3_600_000], ['500ms', 's', 500],
    ['  2M  ', 's', 120_000],
  ]) {
    assert.equal(call('parseDur', text, unit), ms, text);
  }
  for (const rubbish of ['', 'soon', '10 parsecs', '5s later', '-5s']) {
    assert.ok(Number.isNaN(call('parseDur', rubbish, 's')), `"${rubbish}" is not a length of time`);
  }
  for (const [ms, text] of [[0, '0s'], [1_500, '1.5s'], [10_000, '10s'], [90_000, '1m 30s'], [3_600_000, '1h'], [3_661_000, '1h 1m 1s'], [-5, '0s']]) {
    assert.equal(call('fmtDur', ms), text);
  }
  for (const ms of [500, 1_000, 59_000, 60_000, 90_000, 3_600_000, 86_399_000]) {
    assert.equal(call('parseDur', call('fmtDur', ms), 's'), ms, `${ms} ms survives being shown and read back`);
  }
});

test('small wording helpers', () => {
  assert.equal(call('plural', 1, 'run'), '1 run');
  assert.equal(call('plural', 0, 'run'), '0 runs');
  assert.equal(call('fmtMs', 0), '');
  assert.equal(call('fmtMs', 5_000), '5 s');
  assert.equal(call('fmtMs', 120_000), '2 min');
  at(12, 0);
  const now = page.window.__now;
  for (const [ago, text] of [[0, 'never'], [now - 20_000, 'just now'], [now - 5 * 60_000, '5 min ago'], [now - 3 * 3_600_000, '3 h ago'], [now - 2 * 86_400_000, '2 days ago']]) {
    assert.equal(call('fmtAgo', ago), text);
  }
});

test('an address is shown without the part that can hold a secret', () => {
  for (const [address, shown] of [
    ['https://discord.com/api/webhooks/123456/SECRET-TOKEN', 'https://discord.com/…'],
    ['https://ntfy.sh/my-private-topic?auth=SECRET', 'https://ntfy.sh/…'],
    ['http://user:SECRET@host.example/path', 'http://host.example/…'],
    ['http://user:p@ss:SECRET@host.example', 'http://host.example'],
    ['https://example.com/', 'https://example.com'],
    ['  https://example.com  ', 'https://example.com'],
    ['not an address SECRET', 'an address'],
    ['', ''],
    [null, ''],
  ]) {
    const masked = call('maskUrl', address);
    assert.equal(masked, shown, String(address));
    assert.ok(!masked.includes('SECRET'));
  }
});

test('Discord fields are read from "Name | value" lines', () => {
  const text = 'Delay | {delay|off}\n{reason|unknown} | happened\nNotes || the full width\nno bar here\n | no name\nNo value |';
  assert.deepEqual(plain(call('parseFields', text)), [
    { name: 'Delay', value: '{delay|off}', inline: true },
    // The bar inside a variable's fallback does not split the line.
    { name: '{reason|unknown}', value: 'happened', inline: true },
    { name: 'Notes', value: 'the full width', inline: false },
  ]);
  assert.equal(call('parseFields', text, true).length, 5, 'half-written rows are kept while editing');
  const fields = call('parseFields', text);
  assert.deepEqual(plain(call('parseFields', call('fieldsText', fields))), plain(fields), 'and written back the same');
  // Inline fields share a row three at a time, like Discord lays them out.
  const inline = { inline: true }, wide = { inline: false };
  assert.deepEqual(plain(call('fieldSpans', [inline, inline, inline, inline, wide, inline, inline])), [4, 4, 4, 12, 12, 6, 6]);
});

test('an OBS step that lists several sources names each one', () => {
  // The cases of obsws.rs's own test.
  assert.deepEqual(plain(call('listedNames', ' Mic/Aux \n\nDesktop Audio\nMic/Aux\n')), ['Mic/Aux', 'Desktop Audio']);
  assert.deepEqual(plain(call('listedNames', '  \n ')), []);
  assert.deepEqual(plain(call('sourceKeys', ['Mic/Aux', 'mic aux', 'Cámara 1', '!!', 'A very long source name for the stream'])),
    ['mic_aux', 'mic_aux2', 'c_mara_1', 'source', 'a_very_long_source_name']);
});

test('a trigger is named in words', () => {
  for (const [trigger, label] of [
    [{ type: 'event', event: 'hold_opened' }, 'OBS crashed'],
    [{ type: 'event', event: 'onair_changed', filters: { state: 'delay' } }, 'On air: delay'],
    [{ type: 'event', event: 'not_in_this_version' }, 'not_in_this_version'],
    [{ type: 'chat_command', command: '!delay' }, '!delay'],
    [{ type: 'chat_command' }, '!command'],
    [{ type: 'timer', every_ms: 600_000 }, 'Every 10 min'],
    [{ type: 'webhook' }, 'Web call'],
    [{ type: 'shortcut', hotkey: 'Ctrl+Alt+K', midi: 'note:1:36', token: 't' }, 'Ctrl+Alt+K, pad 36 or Stream Deck'],
    [{ type: 'shortcut', midi: 'cc:2:7@Launchpad' }, 'button (CC) 7 ch 2 (Launchpad)'],
    [{ type: 'shortcut' }, 'A button'],
    [{ type: 'scene', scene: 'Gaming' }, 'Scene: Gaming'],
    [{ type: 'scene', scene: '*', except: 'Lobby' }, 'Scene: Any other scene'],
    [{ type: 'scene' }, 'An OBS scene'],
    [null, ''],
  ]) {
    assert.equal(call('triggerLabel', trigger), label, JSON.stringify(trigger));
  }
  for (const [roles, who] of [
    [{ everyone: true, mods: true }, 'everyone'],
    [{ subs: true, vips: true, mods: true }, 'subs, VIPs, mods'],
    [{ mods: true }, 'mods'],
    [{}, 'only you'],
    [null, 'everyone'],
  ]) {
    assert.equal(call('rolesLabel', roles), who, JSON.stringify(roles));
  }
});

test('a delay step is named by what it does to the stream', () => {
  for (const [params, vars, label] of [
    [{ action: 'arm', seconds: '30' }, {}, 'Set a 30 s delay'],
    [{ action: 'arm', seconds: '{arg1}' }, { arg1: '45' }, 'Set a 45 s delay'],
    [{ action: 'arm', seconds: '0' }, {}, 'Turn the delay off'],
    [{ action: 'arm', seconds: '{arg1}' }, { arg1: '0' }, 'Turn the delay off'],
  ]) {
    assert.equal(call('delayActionLabel', params, vars), label);
  }
  assert.equal(call('delayActionLabel', { action: 'cut' }, {}), page.inside('DELAY_ACTIONS').cut);
  assert.equal(call('delayActionLabel', { action: 'made_up' }, {}), 'Delay action');
});

test('a web answer is opened up into values a variable can reach', () => {
  const answer = { rank: 'Gold 2', stats: { wins: 12, last: [3, null] }, 'not a name': 1 };
  assert.deepEqual(plain(call('jsonLeaves', answer)), [
    { path: 'rank', text: 'Gold 2', usable: true },
    { path: 'stats.wins', text: '12', usable: true },
    { path: 'stats.last.0', text: '3', usable: true },
    { path: 'stats.last.1', text: '', usable: true },
    { path: 'not a name', text: '1', usable: false },
  ]);
  assert.deepEqual(plain(call('parseJson', '{"a":1}')), { a: 1 });
  for (const notAnObject of ['5', '"text"', 'null', '{broken', '']) {
    assert.equal(call('parseJson', notAnObject), undefined, notAnObject);
  }
});

test('an alert card takes its color, or the app\'s blue', () => {
  for (const [value, vars, color] of [
    ['#ff0000', {}, '#ff0000'], ['f00', {}, '#f00'], ['{color}', { color: '00ff88' }, '#00ff88'],
    ['{color}', {}, '#5ac8fa'], ['red', {}, '#5ac8fa'], ['', {}, '#5ac8fa'], [undefined, undefined, '#5ac8fa'],
  ]) {
    assert.equal(call('cardColor', value, vars), color, String(value));
  }
});

test('viewer values, maths, chat answers and blocks read like what they do', () => {
  const sentence = (type, params) => call('stepSentence', { type, params }).replace(/<[^>]+>/g, '');
  assert.equal(sentence('viewer', { value: 'coins', op: 'spend', by: '{bet}' }), 'Spend {bet} coins of the viewer, if they have it');
  assert.equal(sentence('viewer', { value: 'coins', who: 'target', by: '50' }), 'Give 50 coins to who they named');
  assert.equal(sentence('viewer', { value: 'coins', who: 'active', by: '10' }), 'Give 10 coins to everyone who chatted');
  assert.equal(sentence('math', { a: '{bet}', op: 'multiply', b: '2', save_as: 'prize' }), '{bet} times 2 → {prize}');
  assert.equal(sentence('wait_chat', { text: '!accept', from: 'target', seconds: '30' }), 'Wait up to 30 s for “!accept” from who they named');
  assert.equal(sentence('call', { block: 'take the bet' }), 'Run the block take the bet');
  assert.equal(sentence('stop', { quiet: 'yes' }), 'Stop here, out of the history');
  assert.equal(sentence('set_var', { name: 'winner', value: 'x', keep: 'yes' }), 'Keep winner = x');
  assert.deepEqual(plain(call('blockNames', { handlers: [{ trigger: { type: 'block', name: 'take the bet' } }, { trigger: { type: 'timer' } }] })), ['take the bet']);
});

test('what the new steps hand on is known, and kept values are read by name', () => {
  given();
  const spend = { type: 'viewer', params: { value: 'coins', op: 'spend', by: '{arg1}' } };
  const double = { type: 'math', params: { a: '{arg1}', op: 'multiply', b: '2', save_as: 'prize' } };
  const wait = { type: 'wait_chat', params: { text: '!accept', save_as: 'answer' } };
  const reply = { type: 'chat', params: { text: '{paid} {prize} {answer.ok} {answer.user} {viewer.coins} {target.coins.rank} {top.coins} {saved.winner} {reply.ok}' } };
  const handler = { trigger: { type: 'chat_command', command: '!bet' }, steps: [spend, double, wait, reply] };
  assert.deepEqual(plain(call('unknownVars', handler, reply)), [{ name: 'reply.ok', hint: '' }], 'only the answer nothing saved under that name');
  // A block is run from a command: it reads what the command brought.
  const block = { trigger: { type: 'block', name: 'take the bet' }, steps: [spend] };
  assert.deepEqual(plain(call('unknownVars', block, { type: 'chat', params: { text: '{user} {arg1}' } })), []);
});

test('a chance check previews its likelier side', () => {
  assert.equal(call('compareSample', '75', 'chance', ''), true);
  assert.equal(call('compareSample', '3.3', 'chance', ''), false);
});

test('your own values fill previews, counting words included', () => {
  given({ values: [{ name: 'coin', label: 'One coin', value: 'gem' }, { name: 'coins', label: 'Several coins', value: 'gems' }] });
  const vars = { 'viewer.coins': '1', 'my.coin': 'gem', 'my.coins': 'gems' };
  assert.equal(call('renderSample', '{viewer.coins#$my.coin#$my.coins} / {my.coins}', vars), '1 gem / gems');
  assert.equal(call('renderSample', '{viewer.coins#$my.coin#$my.coins}', { ...vars, 'viewer.coins': '7' }), '7 gems');
  assert.equal(call('renderSample', '{n#$nothing#coins}', { n: '1' }), '1 ', 'a name nothing fills counts in nothing');
  const reply = { type: 'chat', params: { text: '{my.coins} {my.cions}' } };
  const handler = { trigger: { type: 'timer' }, steps: [reply] };
  assert.deepEqual(plain(call('unknownVars', handler, reply)), [{ name: 'my.cions', hint: 'my.coins' }], 'a typo in a value name is caught');
});

test('a chat message matches the way the app will match it', () => {
  // The cases of engine.rs: a_word_match_never_fires_inside_another_word, wildcards_match_like_file_globs, matching_ignores_case.
  for (const said of ['nt', 'NT!', 'wow, nt.', '(nt)', 'that was nt though']) assert.equal(call('textMatches', said, 'nt', 'word'), true, said);
  for (const said of ['want', "don't", 'isnt', 'ntf', 'front', 'n t']) assert.equal(call('textMatches', said, 'nt', 'word'), false, said);
  assert.equal(call('textMatches', 'NICE TRY lol', 'nice try', 'word'), true);
  assert.equal(call('textMatches', 'nice tryhard', 'nice try', 'word'), false);
  assert.equal(call('textMatches', 'ñam ñam', 'ñam', 'word'), true);
  assert.equal(call('textMatches', 'want', 'nt', 'contains'), true);
  assert.equal(call('textMatches', 'POG champ', 'pog', 'starts_with'), true);
  assert.equal(call('textMatches', ' GG ', 'gg', 'exact'), true);
  assert.equal(call('textMatches', 'gg', '', 'contains'), false);
  for (const [said, pattern, fits] of [['gg wp everyone', 'gg*', true], ['clip it', 'clip ??', true], ['abc', '*b*', true], ['abc', 'a?', false], ['a.c', 'a.c', true], ['abc', 'a.c', false]]) {
    assert.equal(call('textMatches', said, pattern, 'wildcard'), fits, `${said} / ${pattern}`);
  }
  const examples = call('matchExamplesHtml', 'nt', 'word').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
  assert.equal(examples, '✓ nt ✓ NT! ✓ oh nt yes ✗ nting ✗ rent ✓ nt and more');
});

test('a bigger alert from an amount up is a check on that amount, and reads back', () => {
  const alert = text => ({ type: 'overlay', params: { text }, then: [], else: [] });
  const h = { trigger: { type: 'event', event: 'twitch_cheer', filters: {} },
    steps: [alert('cheer'), { type: 'chat', params: { text: 'thanks' }, then: [], else: [] }] };
  const chain = call('alertTiers', h);
  assert.equal(chain.name, 'bits');
  assert.deepEqual([...chain.tiers.map(t => t.from)], [0]);
  chain.tiers.push({ from: 1000, step: alert('big') }, { from: 5000, step: alert('huge') });
  call('setTiers', h, chain);
  const check = h.steps[0];
  assert.deepEqual([check.type, check.params.left, check.params.op, check.params.right], ['if', '{bits}', 'less', '1000']);
  assert.equal(check.then[0].params.text, 'cheer');
  assert.equal(check.else[0].params.right, '5000');
  assert.equal(h.steps[1].type, 'chat', 'the steps around it stay');
  const back = call('alertTiers', h);
  assert.deepEqual(JSON.parse(JSON.stringify(back.tiers.map(t => [t.from, t.step.params.text]))), [[0, 'cheer'], [1000, 'big'], [5000, 'huge']]);
  assert.equal(call('tierLabel', back, 0), 'Below 1000 bits');
  assert.equal(call('tierLabel', back, 2), 'From 5000 bits');
  const follow = { trigger: { type: 'event', event: 'twitch_follow' }, steps: [alert('hi')] };
  assert.equal(call('alertTiers', follow), null, 'a follow has no amount');
});
