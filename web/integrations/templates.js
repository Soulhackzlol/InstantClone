// Mirrors src/integrations/template.rs, for previews.
// `escape`, when given, is applied to inserted values only, like the
// server does for web requests (see template::render_escaped).
function renderSample(text, vars, escape){
  return String(text || '').replace(/\{\{([A-Za-z0-9_.]+)\}\}|\{([A-Za-z0-9_.]{1,64})([|#?][^}]*)?\}/g,
    (all, escaped, name, tail) => {
      if (escaped) return '{' + escaped + '}';
      const v = vars[name] == null ? '' : String(vars[name]);
      // {name#one#many}: the count, then the word that fits it.
      if (tail && tail[0] === '#'){
        const forms = tail.slice(1).split('#');
        if (forms.length !== 2) return all;
        const count = v.trim() || '0';
        // A word written $name is that variable's value: {n#$my.coin#$my.coins}.
        const word = count === '1' ? forms[0] : forms[1];
        const filled = word[0] === '$' ? (vars[word.slice(1)] == null ? '' : String(vars[word.slice(1)])) : word;
        return (escape ? escape(count) : count) + ' ' + (word[0] === '$' && escape ? escape(filled) : filled);
      }
      // {name?yes:no}: one text or the other, by whether the value is yes.
      if (tail && tail[0] === '?'){
        const cut = tail.indexOf(':');
        if (cut < 0) return all;
        return /^(yes|true|on|1)$/i.test(v.trim()) ? tail.slice(1, cut) : tail.slice(cut + 1);
      }
      if (tail && (v === '' || v === '0')) return tail.slice(1);
      return escape ? escape(v) : v;
    });
}
// Mirrors template::edit_text, for previews. "Pick one" previews its
// first choice.
function editText(op, input, a, b){
  switch (op){
    case 'first_line': return (input.split('\n').map(l => l.trim()).find(Boolean)) || '';
    case 'between': {
      const from = a ? input.indexOf(a) : 0;
      if (from < 0) return '';
      const rest = input.slice(from + a.length), to = b ? rest.indexOf(b) : rest.length;
      return to < 0 ? '' : rest.slice(0, to).trim();
    }
    case 'replace': return a ? input.split(a).join(b) : input;
    case 'cut': {
      const max = Math.max(1, parseInt(a, 10) || 100), chars = [...input];
      return chars.length <= max ? input : chars.slice(0, max).join('').trimEnd() + '…';
    }
    case 'round': {
      const n = Number(input.trim());
      return input.trim() && Number.isFinite(n) ? n.toFixed(Math.min(6, parseInt(a, 10) || 0)) : input;
    }
    case 'digits': return /^-?\d+$/.test(input.trim()) ? input.trim().replace(/\B(?=(\d{3})+(?!\d))/g, ',') : input;
    case 'upper': return input.toUpperCase();
    case 'lower': return input.toLowerCase();
    case 'random': return (input.split(input.includes('\n') ? '\n' : '|').map(p => p.trim()).find(Boolean)) || '';
  }
  return input;
}
// Mirrors runner::compare, so previews follow checks like a run does.
// How a chat message trigger can match, each with what it means in a line.
const MATCH_MODES = {
  word:{label:'Has the word', means:'Fires when the message has it as its own word or phrase. Not when it is only part of a longer word.'},
  contains:{label:'Contains these letters', means:'Fires whenever these letters appear, in the middle of another word too.'},
  starts_with:{label:'Starts with', means:'Fires when the message begins with it.'},
  exact:{label:'Is exactly', means:'Fires when the whole message is just this.'},
  wildcard:{label:'Wildcard (* and ?)', means:'* stands for anything, ? for one character, and the whole message has to fit: gg* is any message starting with gg.'},
};
// Whether a chat message fires a trigger. Mirrors engine::text_matches.
function textMatches(text, pattern, mode){
  const said = String(text).toLowerCase(), want = String(pattern).trim().toLowerCase();
  if (!want) return false;
  const literal = want.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  switch (mode){
    // No letter or digit right before or after it: an apostrophe ends a word.
    case 'word': return new RegExp(`(?<![\\p{Alphabetic}\\p{N}])${literal}(?![\\p{Alphabetic}\\p{N}])`, 'u').test(said);
    case 'starts_with': return said.startsWith(want);
    case 'exact': return said.trim() === want;
    case 'wildcard': return new RegExp('^' + want.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.') + '$', 's').test(said.trim());
  }
  return said.includes(want);
}
// A few messages made from the pattern itself, each marked by whether it
// would fire: the difference between the ways to match, shown, not told.
function matchExamplesHtml(pattern, mode){
  const p = pattern.trim().replace(/[*?]/g, '');
  if (!p) return '';
  const tries = [p, p.toUpperCase() + '!', `oh ${p} yes`, `${p}ing`, `re${p}`, `${p} and more`];
  return `<div class="ig-fires-row">${tries.map(said => {
    const fires = textMatches(said, pattern, mode);
    return `<span class="ig-fires ${fires ? 'yes' : 'no'}" title="${fires ? 'Fires it' : 'Does not fire it'}">${fires ? '✓' : '✗'} ${esc(said)}</span>`;
  }).join('')}</div>`;
}
function compareSample(left, op, right){
  const l = String(left).trim().toLowerCase(), r = String(right).trim().toLowerCase();
  const blank = l === '' || l === '0' || l === 'no';
  const nums = l !== '' && r !== '' && Number.isFinite(+l) && Number.isFinite(+r);
  switch (op){
    case 'is': return l === r;
    case 'is_not': return l !== r;
    case 'contains': return l.includes(r);
    case 'not_contains': return !l.includes(r);
    case 'starts_with': return l.startsWith(r);
    case 'empty': return blank;
    case 'not_empty': return !blank;
    case 'greater': return nums && +l > +r;
    case 'less': return nums && +l < +r;
    case 'whole_number': return /^[0-9]+$/.test(l);
    // A preview can't roll: it shows the branch that runs most often.
    case 'chance': return parseFloat(l) >= 50;
  }
  return false;
}
// The first step of `type` a run would reach with these values: undefined
// when none, null when a Stop comes first.
function reachedStep(steps, vars, type){
  for (const s of steps || []){
    if (s.type === 'stop') return null;
    if (s.type === type) return s;
    if (s.type !== 'if') continue;
    const pass = compareSample(renderSample(s.params.left, vars), s.params.op, renderSample(s.params.right, vars));
    const found = reachedStep(pass ? s.then : s.else, vars, type);
    if (found !== undefined) return found;
  }
  return undefined;
}
function editTextSample(s, vars){
  const p = s.params;
  return editText(p.op, renderSample(p.input, vars), renderSample(p.a, vars), renderSample(p.b, vars));
}
// Discord writes `<t:1790000000:R>` as a time that keeps itself current
// ("in 2 minutes"). Previews show it the same way; `html` is escaped text.
function discordTimes(html){
  return html.replace(/&lt;t:(\d{1,12})(?::([tTdDfFR]))?&gt;/g, (all, secs, style) => {
    const at = +secs * 1000;
    let shown;
    if (style === 'R'){
      const diff = Math.round((at - Date.now()) / 60000);
      shown = diff === 0 ? 'now' : diff > 0 ? `in ${plural(diff, 'minute')}` : `${plural(-diff, 'minute')} ago`;
    } else shown = new Date(at).toLocaleString([], style === 't' || style === 'T' ? {timeStyle:'short'} : {dateStyle:'medium', timeStyle:'short'});
    return `<span class="ig-ts">${esc(shown)}</span>`;
  });
}
// The bits of Discord's Markdown previews need: code blocks, `code`,
// **bold**, *italic*, [links](url) and countdowns. `text` is raw; every
// piece is escaped before it is styled.
function discordMarkdown(text){
  const blocks = [];
  let html = esc(text).replace(/```(?:\w*\n)?([\s\S]*?)```/g, (all, code) => {
    blocks.push(`<pre>${code.replace(/^\n|\n$/g, '')}</pre>`);
    return `\u0000${blocks.length - 1}\u0000`;
  });
  html = html.replace(/`([^`\n]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<i>$2</i>')
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)/g, '<span class="ig-dlink">$1</span>')
    .replace(/(^|[\s(])(https?:\/\/[^\s<]+)/g, '$1<span class="ig-dlink">$2</span>');
  html = discordTimes(html).replace(/\n/g, '<br>');
  return html.replace(/\u0000(\d+)\u0000/g, (all, n) => blocks[+n]);
}
// `Name | value` lines as fields; `||` takes the full width. Mirrors
// runner::parse_fields. `keepEmpty` keeps half-written rows, for editing.
function parseFields(text, keepEmpty){
  return String(text || '').split('\n').map(line => {
    const at = fieldBar(line);
    if (at < 0) return null;
    const full = line.startsWith('||', at);
    return {name:line.slice(0, at).trim(), value:line.slice(at + (full ? 2 : 1)).trim(), inline:!full};
  }).filter(f => f && (keepEmpty || (f.name && f.value)));
}
// The first `|` outside {…}: a name can hold {reason|unknown}.
function fieldBar(line){
  let depth = 0;
  for (let i = 0; i < line.length; i++){
    const c = line[i];
    if (c === '{') depth++;
    else if (c === '}') depth = Math.max(0, depth - 1);
    else if (c === '|' && !depth) return i;
  }
  return -1;
}
// A bar outside braces in a name would split it in the wrong place.
function fieldsText(fields){
  const safe = name => { let out = '', depth = 0;
    for (const c of name){ depth += c === '{' ? 1 : c === '}' && depth ? -1 : 0; out += c === '|' && !depth ? '/' : c; }
    return out; };
  return fields.map(f => `${safe(f.name)} ${f.inline ? '|' : '||'} ${f.value}`).join('\n');
}
// How wide each field sits, in twelfths of the card: inline fields share a
// row two or three at a time, like Discord; the rest take it all.
function fieldSpans(fields){
  const spans = [];
  let run = [];
  const flush = () => {
    for (let at = 0; at < run.length; at += 3){
      const row = run.slice(at, at + 3);
      row.forEach(() => spans.push(12 / row.length));
    }
    run = [];
  };
  fields.forEach(f => { if (f.inline) run.push(f); else { flush(); spans.push(12); } });
  flush();
  return spans;
}
// A card's color, falling back to the InstantClone blue.
function cardColor(value, vars){
  const v = renderSample(value || '', vars || {}).trim();
  return /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.test(v) ? (v[0] === '#' ? v : '#' + v) : '#5ac8fa';
}
// "Today at 21:04", the way Discord labels a card's timestamp.
function discordNow(){
  return 'Today at ' + new Date().toLocaleTimeString([], {hour:'2-digit', minute:'2-digit'});
}
// A Discord card (embed) as it lands, filled with sample values.
function cardHtml(step, vars, big){
  const p = step.params;
  const fill = v => renderSample(v || '', vars);
  const title = fill(p.title), desc = fill(p.text), footer = fill(p.footer);
  const fields = parseFields(p.fields).map(f => ({name:fill(f.name), value:fill(f.value), inline:f.inline}))
    .filter(f => f.name.trim() && f.value.trim());
  const foot = [footer, p.timestamp === 'yes' ? discordNow() : ''].filter(Boolean).join(' • ');
  return `<div class="ig-embed${big ? ' big' : ''}" translate="no" style="--ec:${esc(cardColor(p.color, vars))}">
    ${title ? `<div class="ig-embed-title${/^https?:\/\//.test(fill(p.url)) ? ' link' : ''}">${discordMarkdown(title)}</div>` : ''}
    ${desc ? `<div class="ig-embed-desc">${discordMarkdown(desc)}</div>` : ''}
    ${fields.length ? `<div class="ig-embed-fields">${fields.map((f, n) => `<div class="${f.inline ? 'in' : ''}" style="--span:${fieldSpans(fields)[n]}"><b>${esc(f.name)}</b><span>${discordMarkdown(f.value)}</span></div>`).join('')}</div>` : ''}
    ${foot ? `<div class="ig-embed-foot"><i>${svg('shield')}</i>${esc(foot)}</div>` : ''}
  </div>`;
}

// Mirrors template::url_component: only unreserved characters stay.
// A pasted command from a chat bot rather than a plain address.
const BOT_SYNTAX = /\$\(|\$\{|\{readapi\.|\$readapi\(/i;
// Mirrors template::url_component, `.` and `..` included.
const urlComponent = v => v === '.' || v === '..' ? v.replace(/\./g, '%2E')
  : encodeURIComponent(v).replace(/[!'()*]/g, c => '%' + c.charCodeAt(0).toString(16).toUpperCase());
const jsonStringContent = v => JSON.stringify(v).slice(1, -1);
const oneLine = v => v.replace(/[\u0000-\u001f\u007f]/g, ' ');
// An address as safe to show on stream: scheme and host only, since the
// path and query often carry an API key or a webhook secret.
function maskUrl(url){
  // A login in the address (user:pass@) never shows, like the rest.
  // The login runs to the last @ before the host (user:p@ss@host).
  const m = /^(https?:\/\/)(?:[^/?#\s]*@)?([^/?#\s@]+)(\S*)/i.exec(String(url || '').trim());
  if (!m) return String(url || '').trim() ? 'an address' : '';
  return m[1] + m[2] + (m[3] && m[3] !== '/' ? '/…' : '');
}
// Whether a field holds something that reads differently once sent: a
// variable, or (in Discord) Markdown, a link or a countdown.
function needsRender(raw, discord){
  const text = String(raw || '');
  return /\{[A-Za-z0-9_.]{1,64}(?:[|#?][^}]*)?\}/.test(text)
    || (!!discord && /\[[^\]\n]+\]\(|\*\*|`|<t:\d|https?:\/\//.test(text));
}
// A field that shows what it will say until it's being edited, then what
// was written (variables and all). `control` carries class ig-tpl-in;
// `cls` lays the shown text out like the field: msg, line, title or above.
function tplHtml(control, raw, vars, cls, discord){
  if (!needsRender(raw, discord)) return `<div class="ig-tpl">${control}</div>`;
  const text = renderSample(raw, vars);
  const shown = text.trim() ? (discord ? discordMarkdown(text) : esc(text)) : '<span class="ig-tpl-none">empty with these sample values</span>';
  const tag = cls === 'line' ? '' : '<span class="ig-tpl-hint">sample</span>';
  return `<div class="ig-tpl has-vars">${control}<div class="ig-tpl-out ${cls}" data-act="tpl-edit" aria-hidden="true" title="Click to edit. Shown with sample values.">${tag}${shown}</div></div>`;
}
// A yes or no reads better as words in a message, so its chip writes
// both, ready to change. An If step compares the plain yes, so `plain`.
// By the value's own name, or by the field of a step's answer ({cam.muted}).
const YES_NO_WORDS = {
  muted:['muted', 'unmuted'], visible:['showing', 'hidden'], streaming:['live', 'offline'],
  recording:['recording', 'not recording'], ok:['worked', 'failed'], obs_live:['live', 'offline'],
};
function tokenText(name, plain){
  const parts = name.split('.');
  const words = YES_NO_WORDS[parts[parts.length - 1]];
  return plain || !words ? `{${name}}` : `{${name}?${words[0]}:${words[1]}}`;
}
// Every way to write a value, each with what it gives.
const FORMS_HELP = `<details class="ig-more-fields ig-forms"><summary>Ways to write a value</summary><dl>
  <dt>{viewers}</dt><dd>the value<b>42</b></dd>
  <dt>{game|nothing yet}</dt><dd>this text when it's empty or 0<b>nothing yet</b></dd>
  <dt>{viewers#viewer#viewers}</dt><dd>the number and the word that fits it<b>1 viewer, 42 viewers</b></dd>
  <dt>{obs.muted?muted:unmuted}</dt><dd>the first text for yes, the second for no<b>muted</b></dd>
  <dt>{{user}}</dt><dd>written as it is<b>{user}</b></dd></dl></details>`;
// The fields a step's answer has, said under that step's name.
const FIELD_LABEL = {
  scene:'scene on air', streaming:'streaming?', recording:'recording?', muted:'muted?', visible:'showing?',
  status:'status', ok:'worked?', body:'whole answer', output:'what it printed', code:'exit code',
};
// The name a value goes by: a step's answer fields read under that step.
function valueLabel(v){
  const field = v.step && answerName(v.step) && v.name.split('.').length === 2 ? FIELD_LABEL[v.name.split('.')[1]] : '';
  return v.label || field || varLabel(v.name);
}
// Where a value comes from, as a heading: what started the run, an earlier
// step (under its answer's name), the streamer's own, or always there.
function valueSource(v){
  if (!v.step) return {trigger:'From what happened', mine:'Your values', live:'Always there'}[v.group] || '';
  const name = answerName(v.step);
  return `${esc(stepSource(v.step))}${name ? ` <code>${esc(name)}</code>` : ''}`;
}
const SET_EARLIER = 'A value an earlier step in this run set.';
function valueHelp(v){
  return varHelp(v.name) || (v.group === 'steps' ? SET_EARLIER : '');
}
const shortSample = (text, max) => !text ? '(empty)' : text.length > max ? text.slice(0, max) + '…' : text;
// `text` escaped, with every place one of `words` shows marked.
function markWords(text, words){
  const lower = text.toLowerCase(), hit = new Array(text.length).fill(false);
  words.forEach(w => {
    for (let at = lower.indexOf(w); at >= 0; at = lower.indexOf(w, at + w.length)) hit.fill(true, at, at + w.length);
  });
  let html = '', open = false;
  for (let i = 0; i < text.length; i++){
    if (hit[i] !== open){ html += hit[i] ? '<mark>' : '</mark>'; open = hit[i]; }
    html += esc(text[i]);
  }
  return html + (open ? '</mark>' : '');
}

