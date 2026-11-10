// - Logs -------------------------------
// Each line is split into time, source and severity, shown as a row the eye
// can scan: errors red, warnings amber, good news green, each source in its
// own colour. Filters narrow it by severity, source or text, and the text
// found is marked. The log only grows at the end, so new lines are appended
// (and ease in) rather than the whole list rebuilt every poll.
const LOG_ERROR = /\b(disconnected|failed|failure|error|errors|refused|rejected|invalid|denied|aborted)\b|could not|couldn't/i;
const LOG_WARN = /\b(retry|retrying|retries|waiting|timed out|stale|warning|fallback|too small)\b|holding off|nothing goes out|can't|cannot/i;
const LOG_RETRY = /\b(retry|retrying|retries)\b/i;
const LOG_OK = /\b(succeeded|connected|published|saved|started|ready|resumed)\b|is now live|now live/i;
// Source tag colours (OKLCH hues), picked by the source's name so each
// keeps its colour.
const LOG_HUES = [210, 160, 280, 40, 330, 120, 250, 15, 185, 300];
// `started`: when (Unix ms) the `[+s]` stamps count from, from the app.
// `unseen`: lines that arrived while scrolled up to read.
// `clears`: how many times Clear went through, so a poll sent before one
// can't draw the old lines back.
const logView = {lines: [], level: 'all', source: '', text: '', started: 0, unseen: 0, clears: 0};
const logClock = new Intl.DateTimeFormat(undefined, {hour:'2-digit', minute:'2-digit', second:'2-digit'});

function parseLogLine(line){
  const match = line.match(/^\[\+\s*([\d.]+)s\]\s*(.*)$/);
  const secs = match ? parseFloat(match[1]) : NaN;
  const message = match ? match[2] : line;
  const bracket = message.match(/^\[([^\]]+)\]\s*/);
  const source = bracket ? bracket[1]
    : /^ingest:|^Enhanced Broadcasting/.test(message) ? 'OBS'
    : /^pre-arm/i.test(message) || /delay/i.test(message) ? 'Delay'
    : 'App';
  // Something that failed but is being tried again is a warning.
  const level = LOG_ERROR.test(message) && !LOG_RETRY.test(message) ? 'err'
    : LOG_WARN.test(message) || LOG_ERROR.test(message) ? 'warn'
    : LOG_OK.test(message) ? 'ok' : 'info';
  return {secs, source, level, message: bracket ? message.slice(bracket[0].length) : message};
}
// Seconds since the app started as h:mm:ss or m:ss.
function fmtLogTime(secs){
  if (!isFinite(secs)) return '';
  const total = Math.floor(secs);
  const h = Math.floor(total / 3600), m = Math.floor(total / 60) % 60, s = total % 60;
  const pad = n => String(n).padStart(2, '0');
  return h ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}
function logSourceHue(name){
  let hash = 0;
  for (const ch of name) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return LOG_HUES[hash % LOG_HUES.length];
}
// `fresh`: it arrived while the log was on screen, so it eases in.
function logRow(line, fresh){
  const entry = parseLogLine(line);
  const row = document.createElement('div');
  row.className = 'log-row lvl-' + entry.level + (fresh ? ' log-new' : '');
  row.dataset.level = entry.level;
  row.dataset.source = entry.source;
  row.dataset.text = line.toLowerCase();
  const time = document.createElement('span');
  time.className = 'log-t';
  const since = fmtLogTime(entry.secs);
  if (logView.started && isFinite(entry.secs)){
    const at = new Date(logView.started + entry.secs * 1000);
    time.textContent = logClock.format(at);
    time.title = `${at.toLocaleString()} (${since} after InstantClone started)`;
  } else time.textContent = since;
  const source = document.createElement('span');
  source.className = 'log-src';
  source.textContent = entry.source;
  source.title = entry.source;
  source.style.setProperty('--h', logSourceHue(entry.source));
  const message = document.createElement('span');
  message.className = 'log-msg';
  message.textContent = entry.message;
  row._message = entry.message;
  row._line = line;
  row.title = 'Click to copy this line';
  row.append(time, source, message);
  return row;
}
// A click copies the line as the app wrote it; selecting text still works.
async function copyLogRow(row){
  if (String(getSelection())) return;
  if (!await writeClipboard(row._line)){ toast("Couldn't copy here. Select the line and press Ctrl+C.", 'err'); return; }
  row.classList.remove('copied');
  void row.offsetWidth;   // restarts the flash on a second click
  row.classList.add('copied');
}
// Every warning and error, oldest first.
function copyLogProblems(btn){
  const problems = logView.lines.filter(line => {
    const level = parseLogLine(line).level;
    return level === 'err' || level === 'warn';
  });
  copyText(btn, problems.join('\n'), 'the problems');
}
function logRowVisible(row){
  if (logView.level === 'err' && row.dataset.level !== 'err') return false;
  if (logView.level === 'problems' && row.dataset.level !== 'err' && row.dataset.level !== 'warn') return false;
  if (logView.source && row.dataset.source !== logView.source) return false;
  return !logView.text || row.dataset.text.includes(logView.text);
}
// Mark where the filter text is in a row's message.
function markLogText(row){
  const box = row.lastElementChild, text = row._message, lower = text.toLowerCase();
  const needle = logView.text;
  const at = needle && lower.length === text.length ? lower.indexOf(needle) : -1;
  if (at < 0){
    if (box.firstElementChild) box.textContent = text;
    return;
  }
  const parts = [];
  let from = 0;
  for (let i = at; i >= 0; i = lower.indexOf(needle, from)){
    parts.push(text.slice(from, i));
    const mark = document.createElement('mark');
    mark.textContent = text.slice(i, i + needle.length);
    parts.push(mark);
    from = i + needle.length;
  }
  parts.push(text.slice(from));
  box.replaceChildren(...parts);
}
function applyLogFilters(){
  let shown = 0, lastSource = null;
  const counts = {err: 0, warn: 0};
  const seen = new Set();
  for (const row of $('logs').children){
    row.hidden = !logRowVisible(row);
    if (row.dataset.level in counts) counts[row.dataset.level]++;
    seen.add(row.dataset.source);
    if (row.hidden) continue;
    shown++;
    row.classList.toggle('same-src', row.dataset.source === lastSource);
    lastSource = row.dataset.source;
    markLogText(row);
  }
  if (logView.source && !seen.has(logView.source)){
    logView.source = '';
    return applyLogFilters();
  }
  fadeHidden($('logs-nomatch'), shown > 0 || !logView.lines.length);
  $('logs-n-problems').textContent = counts.err + counts.warn || '';
  fadeHidden($('logs-copy-problems'), !(counts.err + counts.warn));
  $('logs-n-err').textContent = counts.err || '';
  const select = $('logs-source');
  const known = [...select.options].map(o => o.value).filter(Boolean);
  const sources = [...seen].sort();
  if (sources.join('|') !== known.join('|')){
    select.replaceChildren(new Option('All sources', ''), ...sources.map(name => { const o = new Option(name, name); o.translate = false; return o; }));
    select.value = sources.includes(logView.source) ? logView.source : '';
  }
}
// Per-line motion in the console. Only rows in or near its view animate:
// the rest switch at once, as a log of 1,500 rows would cost more in
// animations than anyone sees. A row's own rect is empty while hidden.
const LOG_STAGGER_MS = 14, LOG_STAGGER_ROWS = 20;
function logRowsInView(rows){
  const box = $('logs').getBoundingClientRect();
  return rows.filter(row => {
    const r = row.getBoundingClientRect();
    return r.height > 0 && r.bottom > box.top - box.height && r.top < box.bottom + box.height;
  });
}
// Fade rows in (rising slightly) or out, one after another from the top,
// `stagger` ms apart.
function fadeLogRows(rows, out, stagger = LOG_STAGGER_MS){
  return rows.map((row, i) => {
    const anim = row.animate(out ? [{opacity: 1}, {opacity: 0}]
      : [{opacity: 0, transform: 'translateY(4px)'}, {opacity: 1, transform: 'none'}],
      {duration: out ? 120 : 220, delay: Math.min(i, LOG_STAGGER_ROWS) * stagger,
       easing: 'cubic-bezier(.2,.7,.2,1)', fill: 'both'});
    if (!out) anim.onfinish = () => anim.cancel();   // its end is the row's own look
    return anim;
  });
}
// The console or "No events yet", one fading out before the other fades
// in, with the filters folding open or shut beside them. Safe to call on
// every poll: only a change of view moves anything.
function showLogsView(hasLines){
  const list = $('logs'), empty = $('logs-empty');
  animateHidden($('logs-tools'), !hasLines);
  const [gone, shown] = hasLines ? [empty, list] : [list, empty];
  // Already this view, or turning back to it mid-swap: the fade reverses.
  if (gone.hidden && !gone._fade){ fadeHidden(shown, false); return; }
  fadeHidden(gone, true, () => {
    fadeHidden(shown, false);
    // A console that was hidden couldn't follow the tail: catch up.
    if (hasLines && list._follow !== false) list.scrollTop = list.scrollHeight;
  });
}
// A filter change: the lines on screen it drops fade out together, then
// the lines it brings fade in one after another. Another change while that
// runs applies at once, so typing in the filter box never waits on a fade.
let logFilterFade = null;
function refilterLogs(){
  const el = $('logs');
  const typing = !!logFilterFade;
  if (logFilterFade){
    logFilterFade.anims.forEach(anim => anim.cancel());
    clearTimeout(logFilterFade.timer);
    logFilterFade = null;
  }
  if (!motionOK() || el.hidden || !el.clientHeight){ applyLogFilters(); return; }
  const before = new Set([...el.children].filter(row => !row.hidden));
  let fadingOut = [];
  const settle = () => {
    logFilterFade = null;
    applyLogFilters();
    // Their rows are hidden now. Left running, the held opacity 0 would
    // come back with a row that a later filter shows without a fade-in.
    fadingOut.forEach(anim => anim.cancel());
    if (el._follow !== false) el.scrollTop = el.scrollHeight;
    fadeLogRows(logRowsInView([...el.children].filter(row => !row.hidden && !before.has(row))), false);
  };
  const leaving = typing ? [] : logRowsInView([...before].filter(row => !logRowVisible(row)));
  if (!leaving.length){ settle(); return; }
  fadingOut = fadeLogRows(leaving, true, 0);
  logFilterFade = {anims: fadingOut, timer: setTimeout(settle, 120)};
}
// How many of the shown lines are still in `lines`, at its front. The
// server's log is a ring: once full, each new line pushes the oldest out, so
// the view drops rows from the top and appends at the bottom instead of
// rebuilding all 1500. -1 when they don't line up (cleared or restarted).
function logOverlap(old, lines){
  if (!old.length) return -1;
  const last = lines.lastIndexOf(old[old.length - 1]);
  const dropped = old.length - (last + 1);
  return last >= 0 && dropped >= 0 && lines[0] === old[dropped] ? last + 1 : -1;
}
function showUnseenLogs(){
  const button = $('logs-latest');
  fadeHidden(button, $('logs')._follow !== false);
  button.textContent = logView.unseen === 1 ? '↓ 1 new line' : logView.unseen ? `↓ ${logView.unseen} new lines` : '↓ Jump to latest';
}
// The log is read only while the Logs tab is on screen: a new line moves
// its counter (see Live), and opening the tab or the window catches up.
function logsOnScreen(){
  const pane = document.querySelector('.tab-pane[data-pane="logs"]');
  return !!pane && !pane.hidden && !document.hidden;
}
async function pollLogs(){
  if (!logsOnScreen()) return;
  try {
    const clears = logView.clears;
    const j = await (await fetch('/logs')).json();
    if (clears !== logView.clears) return;
    const lines = j.lines || [];
    const el = $('logs');
    showLogsView(lines.length > 0);
    const old = logView.lines;
    let kept = logOverlap(old, lines);
    // The first answer, or a restart, sets the clock the stamps count
    // from: every row is drawn again with its time of day.
    if (j.started_ms && Math.abs(j.started_ms - logView.started) > 1000){
      logView.started = j.started_ms;
      kept = -1;
    }
    if (kept === old.length && kept === lines.length) return;
    if (kept < 0) el.replaceChildren();
    else for (let i = old.length - kept; i > 0; i--) el.firstElementChild.remove();
    // Only lines that come in while the log is on screen ease in.
    const fresh = kept >= 0 && el.clientHeight > 0;
    const added = lines.slice(Math.max(kept, 0));
    const rows = added.map(line => logRow(line, fresh));
    // Lines that came in together ease in one after another.
    if (fresh) rows.slice(-LOG_STAGGER_ROWS).forEach((row, i) => row.style.setProperty('--log-delay', i * 35 + 'ms'));
    el.append(...rows);
    logView.lines = lines;
    applyLogFilters();
    // Follow the tail unless you've scrolled up to read (see the scroll
    // listener). A hidden pane can't scroll, so it catches up when shown.
    if (el._follow !== false) el.scrollTop = el.scrollHeight;
    else if (kept >= 0){
      logView.unseen += added.length;
      showUnseenLogs();
    }
  } catch(_){}
}
(() => {
  const el = $('logs');
  // The time column fits this language's clock ("19:20:05" or "7:20:05 PM").
  el.style.setProperty('--log-tw', logClock.format(new Date(2000, 0, 1, 22, 22, 22)).length + 0.5 + 'ch');
  el.addEventListener('scroll', () => {
    if (!el.clientHeight) return;
    el._follow = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    if (el._follow) logView.unseen = 0;
    showUnseenLogs();
  });
  $('logs-latest').onclick = () => {
    el._follow = true;
    logView.unseen = 0;
    el.scrollTo({top: el.scrollHeight, behavior: prefersReducedMotion() ? 'auto' : 'smooth'});
    fadeHidden($('logs-latest'), true);
  };
  document.querySelectorAll('[data-log-level]').forEach(chip => {
    chip.onclick = () => {
      logView.level = chip.dataset.logLevel;
      document.querySelectorAll('[data-log-level]').forEach(c => c.setAttribute('aria-pressed', String(c === chip)));
      refilterLogs();
    };
  });
  el.addEventListener('click', e => {
    const row = e.target.closest('.log-row');
    if (row) copyLogRow(row);
  });
  $('logs-copy-problems').onclick = e => copyLogProblems(e.currentTarget);
  $('logs-source').onchange = e => { logView.source = e.target.value; refilterLogs(); };
  $('logs-search').oninput = e => { logView.text = e.target.value.trim().toLowerCase(); refilterLogs(); };
})();
async function clearLogs(){
  const r = await postAction('/logs/clear');
  if (!r.ok){ toast(`Could not clear the logs: ${r.error}`, 'err'); return; }
  logView.clears++;
  logView.lines = [];
  logView.unseen = 0;
  toast('Logs cleared','info');
  const el = $('logs');
  const rows = motionOK() && el.clientHeight ? logRowsInView([...el.children].filter(row => !row.hidden)) : [];
  const fades = fadeLogRows(rows, true);
  await Promise.all(fades.map(anim => anim.finished.catch(() => {})));
  // A line that came in meanwhile has drawn its own view.
  if (logView.lines.length) return;
  showLogsView(false);
  el.replaceChildren();
}
