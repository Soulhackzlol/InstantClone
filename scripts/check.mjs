// Every check CI runs before a merge, in one terminal: formatting, lint,
// the Rust tests and the page tests. Tests are named on screen as they
// pass, so it shows what is being proven and not only how much is left.
//
//   node scripts/check.mjs                   run every check
//   node scripts/check.mjs --report r.json   also save the results as JSON
//   node scripts/check.mjs --hold            wait for a key before closing
//   node scripts/check.mjs --also more.mjs   add the checks a module exports
//   node scripts/check.mjs --only Lint       run the named checks (a,b,c)
//
// Away from a terminal (CI, a pipe) it prints one plain line per check.
// The first check to fail ends the run. No dependencies.
import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';
import { spawn } from 'node:child_process';
import { fileURLToPath, pathToFileURL } from 'node:url';

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const FRAME_MS = 80;
/** How long a test's name stays newest in the list, so it can be read. */
const NAME_MS = 240;
const NAMES_SHOWN = 4;
const BAR_WIDTH = 32;
/** Width of the lit stretch that crosses the bar while no total is known. */
const SWEEP_WIDTH = 6;
/** Lines of a failed check's output shown under the verdict. */
const OUTPUT_KEPT = 40;

const PLAIN = 'NO_COLOR' in process.env || !process.stdout.isTTY;
const INK = {
  accent: [90, 200, 250],
  good: [120, 220, 150],
  bad: [255, 110, 110],
  text: [225, 228, 235],
  soft: [150, 157, 168],
  faint: [92, 99, 110],
};
/** Newest test name first, each older one a step dimmer. */
const FADE = [INK.text, INK.soft, INK.faint, [64, 70, 80]];

/**
 * A check is a command that must exit clean. `proves` says what a pass
 * means. `read`, when a check runs tests, is given each line the command
 * prints and counts the tests into the check's tally (see `newTally`).
 */
const CHECKS = [
  {
    name: 'Format',
    proves: 'the code is laid out the one agreed way',
    run: ['cargo', 'fmt', '--all', '--', '--check'],
  },
  {
    name: 'Lint',
    proves: "the compiler's strictest advice finds nothing",
    run: ['cargo', 'clippy', '--all-targets', '--', '-D', 'warnings'],
  },
  {
    name: 'Rust tests',
    proves: 'streaming, delay, integrations and server behave',
    run: ['cargo', 'test', '--release'],
    read: readCargoTest,
  },
  {
    name: 'Page tests',
    proves: "the dashboard's own logic holds, without a browser",
    run: [process.execPath, '--test', '--test-reporter=tap', 'web/tests/*.test.mjs'],
    read: readTap,
  },
];

function newTally() {
  return { total: 0, passed: 0, skipped: 0, failed: [], names: [], note: '' };
}

/** Count one finished test; `outcome` is `passed`, `skipped` or `failed`. */
function count(tally, outcome, name, area = '') {
  if (outcome === 'failed') tally.failed.push(area ? `${area}: ${name}` : name);
  else tally[outcome] += 1;
  tally.names.push({ name, area });
}

/** `test rtmp::chunk::tests::a_split_header_is_joined ... ok`, from libtest. */
function readCargoTest(line, tally) {
  const running = /^running (\d+) tests?$/.exec(line);
  if (running) {
    tally.total += Number(running[1]);
    return;
  }
  const test = /^test (.+?)(?: - should panic)? \.\.\. (ok|FAILED|ignored)\b/.exec(line);
  if (!test) return;
  const parts = test[1].split('::');
  const name = parts.pop().replaceAll('_', ' ');
  const area = parts.filter((part) => part !== 'tests').join(' › ');
  const outcome = { ok: 'passed', FAILED: 'failed', ignored: 'skipped' }[test[2]];
  count(tally, outcome, name, area);
}

/** `ok 12 - the delay is worded one way`, from Node's TAP reporter. */
function readTap(line, tally) {
  const test = /^(not )?ok \d+ - (.+?)( # SKIP.*)?$/.exec(line);
  if (!test) return;
  const outcome = test[1] ? 'failed' : test[3] ? 'skipped' : 'passed';
  count(tally, outcome, test[2]);
}

/** Run one check to its end, feeding its lines to the tally as they come. */
function runCheck(check, tally) {
  return new Promise((resolve) => {
    const [command, ...args] = check.run;
    const child = spawn(command, args, { cwd: REPO, stdio: ['ignore', 'pipe', 'pipe'] });
    const output = [];
    const keep = (line) => {
      output.push(line);
      if (output.length > OUTPUT_KEPT) output.shift();
    };
    readline.createInterface({ input: child.stdout }).on('line', (line) => {
      keep(line);
      check.read?.(line, tally);
    });
    // Cargo reports what it is building here: that is the note shown
    // while a check has no test to name yet.
    readline.createInterface({ input: child.stderr }).on('line', (line) => {
      keep(line);
      if (line.trim()) tally.note = line.trim();
    });
    child.on('error', (error) => {
      keep(`could not run ${command}: ${error.message}`);
      resolve({ ok: false, output });
    });
    child.on('close', (code) => resolve({ ok: code === 0 && tally.failed.length === 0, output }));
  });
}

function clock(ms) {
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds}s`;
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

function tallied(tally) {
  const skipped = tally.skipped ? `, ${tally.skipped} skipped` : '';
  return `${tally.passed} passed${skipped}`;
}

/** What a check's row says next to its name, for the state it is in. */
function detail({ check, state, tally }) {
  const counted = tally.names.length;
  if (state === 'running' && counted) {
    return tally.total ? `${counted} of ${tally.total}` : `${counted} so far`;
  }
  if (state === 'failed') return tally.failed.length ? `${tally.failed.length} failed` : 'failed';
  if (state === 'passed' && counted) return `${tallied(tally)}: ${check.proves}`;
  return check.proves;
}

function paint(text, ink) {
  return !ink || PLAIN ? text : `\x1b[38;2;${ink.join(';')}m${text}\x1b[0m`;
}

/** A row is `[text, ink]` pairs; cut to `width` so it can never wrap. */
function draw(row, width) {
  let room = width;
  let drawn = '';
  for (const [text, ink] of row) {
    const shown = text.slice(0, room);
    room -= shown.length;
    drawn += paint(shown, ink);
  }
  return drawn;
}

/** The running check's dot: it breathes between faint and the accent. */
function pulse(now) {
  const lit = 0.5 + 0.5 * Math.sin(now / 260);
  return INK.faint.map((dim, i) => Math.round(dim + (INK.accent[i] - dim) * lit));
}

function checkRow(result, now) {
  const { check, state } = result;
  const mark = {
    waiting: ['○', INK.faint],
    running: ['●', pulse(now)],
    passed: ['√', INK.good],
    failed: ['×', INK.bad],
  }[state];
  const took = state === 'waiting' ? '' : clock(state === 'running' ? now - result.began : result.took);
  return [
    ['  '],
    mark,
    [` ${check.name.padEnd(12)}`, state === 'waiting' ? INK.soft : INK.text],
    [`${took.padStart(5)}  `, INK.faint],
    [detail(result), state === 'failed' ? INK.bad : INK.soft],
  ];
}

/** Move the bar toward the real progress, and name a newer test when due. */
function advance(ticker, now) {
  const { names, total } = ticker.of.tally;
  if (total) ticker.fraction += (Math.min(1, names.length / total) - ticker.fraction) * 0.3;
  if (names.length === ticker.read || now - ticker.at < NAME_MS) return;
  ticker.shown.unshift(names[names.length - 1]);
  ticker.shown.length = Math.min(ticker.shown.length, NAMES_SHOWN);
  ticker.read = names.length;
  ticker.at = now;
}

function progressBar(fraction) {
  const filled = Math.round(fraction * BAR_WIDTH);
  return [
    ['█'.repeat(filled), INK.accent],
    ['░'.repeat(BAR_WIDTH - filled), INK.faint],
    [`  ${Math.round(fraction * 100)}%`, INK.soft],
  ];
}

/** Busy with no total to count toward yet: a lit stretch crosses the bar. */
function sweepBar(now) {
  const start = Math.floor(now / FRAME_MS) % (BAR_WIDTH + SWEEP_WIDTH) - SWEEP_WIDTH;
  const from = Math.max(0, start);
  const to = Math.min(BAR_WIDTH, start + SWEEP_WIDTH);
  return [
    ['░'.repeat(from), INK.faint],
    ['█'.repeat(to - from), INK.accent],
    ['░'.repeat(BAR_WIDTH - to), INK.faint],
  ];
}

/**
 * The rows under the running check: its progress, then the latest tests,
 * each with the part of the app it covers. Always the same height, so the
 * list below it stays still.
 */
function tickerRows(ticker, now) {
  const { check, tally } = ticker.of;
  const indent = '      ';
  const note = [[indent + tally.note, INK.faint]];
  if (!check.read) return [note];
  const rows = [[[indent], ...(tally.total ? progressBar(ticker.fraction) : sweepBar(now))]];
  for (let age = 0; age < NAMES_SHOWN; age++) {
    const test = ticker.shown[age];
    if (!test) rows.push(age === 0 ? note : []);
    else rows.push([[indent + test.name, FADE[age]], [`  ${test.area}`, age === 0 ? INK.accent : FADE.at(-1)]]);
  }
  return rows;
}

/** Redraws the checks in place until stopped; the last frame stays up. */
function liveView(results, started) {
  const out = process.stdout;
  const showCursor = () => out.write('\x1b[?25h');
  let drawn = 0;
  let ticker = null;
  const show = () => {
    const now = Date.now();
    const running = results.find((result) => result.state === 'running');
    if (running && ticker?.of !== running) {
      ticker = { of: running, shown: [], read: 0, at: 0, fraction: 0 };
    }
    if (running) advance(ticker, now);
    const rows = [[], [['  InstantClone checks', INK.text], [`   ${clock(now - started)}`, INK.faint]], []];
    for (const result of results) {
      rows.push(checkRow(result, now));
      if (result === running) rows.push(...tickerRows(ticker, now));
    }
    rows.push([]);
    const width = (out.columns || 80) - 1;
    const up = drawn ? `\x1b[${drawn}A` : '';
    out.write(up + rows.map((row) => `\r\x1b[2K${draw(row, width)}\n`).join('') + '\x1b[J');
    drawn = rows.length;
  };
  out.write('\x1b[?25l');
  process.once('exit', showCursor);
  process.once('SIGINT', () => process.exit(130));
  const timer = setInterval(show, FRAME_MS);
  return {
    stop() {
      clearInterval(timer);
      show();
      showCursor();
    },
  };
}

/** The verdict, and for a failure the tests that failed and what it printed. */
function verdict(results, started) {
  const failed = results.find((result) => result.state === 'failed');
  if (!failed) {
    return [paint(`  All ${results.length} checks passed in ${clock(Date.now() - started)}`, INK.good)];
  }
  return [
    paint(`  ${failed.check.name} failed`, INK.bad),
    ...failed.tally.failed.map((name) => paint(`    × ${name}`, INK.bad)),
    '',
    ...failed.output.map((line) => `    ${line}`),
  ];
}

function saveReport(file, results, started) {
  const checks = results.map(({ check, state, tally, took, output }) => ({
    name: check.name,
    state,
    seconds: Math.round((took ?? 0) / 1000),
    passed: tally.passed,
    skipped: tally.skipped,
    failed: tally.failed,
    output: state === 'failed' ? output : [],
  }));
  const ok = results.every((result) => result.state === 'passed');
  const seconds = Math.round((Date.now() - started) / 1000);
  fs.writeFileSync(file, `${JSON.stringify({ ok, seconds, checks }, null, 2)}\n`);
}

function keyPress() {
  return new Promise((resolve) => {
    process.stdin.setRawMode(true);
    process.stdin.resume();
    process.stdin.once('data', () => {
      process.stdin.setRawMode(false);
      process.stdin.pause();
      resolve();
    });
  });
}

/** Run the checks in order, stopping at the first failure. True when all pass. */
async function runChecks(checks, { report, hold }) {
  const live = process.stdout.isTTY;
  const started = Date.now();
  const results = checks.map((check) => ({ check, state: 'waiting', tally: newTally() }));
  const view = live ? liveView(results, started) : null;
  for (const result of results) {
    result.state = 'running';
    result.began = Date.now();
    const { ok, output } = await runCheck(result.check, result.tally);
    Object.assign(result, { state: ok ? 'passed' : 'failed', took: Date.now() - result.began, output });
    if (!live) {
      const counted = result.tally.names.length ? `: ${tallied(result.tally)}` : '';
      console.log(`${ok ? 'ok  ' : 'FAIL'}  ${result.check.name} (${clock(result.took)})${counted}`);
    }
    if (!ok) break;
  }
  view?.stop();
  console.log(verdict(results, started).join('\n'));
  if (report) saveReport(report, results, started);
  if (hold && live && process.stdin.isTTY) {
    console.log(paint('\n  Press any key to close', INK.faint));
    await keyPress();
  }
  return results.every((result) => result.state === 'passed');
}

const args = process.argv.slice(2);
const valueOf = (flag) => (args.includes(flag) ? args[args.indexOf(flag) + 1] : null);
const also = valueOf('--also');
const more = also ? (await import(pathToFileURL(path.resolve(also)).href)).default : [];
const only = valueOf('--only')?.split(',');
const checks = [...CHECKS, ...more].filter((check) => !only || only.includes(check.name));
if (!checks.length) throw new Error(`no check is named ${only}`);
const passed = await runChecks(checks, {
  report: valueOf('--report'),
  hold: args.includes('--hold'),
});
process.exitCode = passed ? 0 : 1;
