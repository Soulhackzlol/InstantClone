// ---------------------------------------------------------------- try a web request

// What "Send request" got, per web request step (by uidOf). Cleared
// whenever an editor opens: the steps are fresh objects each time.
const TRIES = new Map();

function tryEntry(s){
  let e = TRIES.get(uidOf(s));
  if (!e){ e = {inputs:{}}; TRIES.set(uidOf(s), e); }
  return e;
}

// "Try it": values for the variables the request uses, the button, and
// what came back, where every value can be picked. `useLabel` names what
// the picked value's main button does in this editor ('' for none).
function tryHtml(s, handler, useLabel){
  const t = tryEntry(s);
  const used = [...new Set(['url', 'headers', 'body'].flatMap(f =>
    [...String(s.params[f] || '').matchAll(/\{([A-Za-z0-9_.]{1,64})(?:[|#?][^}]*)?\}/g)].map(m => m[1])))];
  const samples = sampleMap(handler);
  const inputs = used.map(n => `<div class="dff"><label class="mono">${esc(n)}</label><input class="ic-input" data-bind="try" data-name="${esc(n)}"
    value="${esc(n in t.inputs ? t.inputs[n] : (samples[n] || ''))}" placeholder="a value to try"></div>`).join('');
  return `<div class="ig-try">
    <div class="ig-try-head"><span class="ic-label">Try it</span>
      <button class="ic-btn ic-btn-primary ig-small" data-act="b-try" ${s.params.url ? '' : 'disabled'}>${svg('play', ' class="ig-btn-ic"')}Send request</button></div>
    ${used.length ? `<div class="muted">Try it with:</div><div class="ig-try-inputs">${inputs}</div>` : ''}
    ${tryResultHtml(s, t, useLabel)}
  </div>`;
}

function tryResultHtml(s, t, useLabel){
  if (t.error) return warnHtml(esc(t.error));
  if (!t.status) return '<div class="muted">Sends the request for real and shows the answer. Then pick what to use from it.</div>';
  const n = (s.params.save_as || '').trim() || 'response';
  const ok = t.status >= 200 && t.status < 300;
  const head = `<div class="ig-try-meta"><span class="ig-status-pill ${ok ? 'ok' : 'bad'}">${t.status}</span>
    <span class="muted">${t.ms} ms${t.truncated ? ' · long answer, cut for display' : ''}</span></div>`;
  const picked = t.picked ? `<div class="ig-picked" data-key="picked-${esc(t.picked.token)}">
    <div class="ig-picked-what"><code>{${esc(t.picked.token)}}</code><span title="${esc(t.picked.sample)}">${esc(t.picked.sample) || '<i>empty</i>'}</span></div>
    <div class="ig-picked-actions">${useLabel ? `<button class="ic-btn ic-btn-primary ig-small" data-act="b-use-reply">${useLabel}</button>` : ''}
    <button class="ic-btn ic-btn-ghost ig-small" data-act="copy" data-text="{${esc(t.picked.token)}}">Copy</button></div></div>` : '';
  if (t.json === undefined){
    return `${head}${picked}<div class="ig-try-text">${t.body ? esc(t.body.slice(0, 3000)) : '<span class="muted">The answer was empty.</span>'}</div>
      ${t.body ? `<button class="ic-btn ic-btn-ghost ig-small ig-self-start" data-act="b-pick" data-token="${esc(n)}.body" data-sample="${esc(t.body.slice(0, 200))}">Use the whole answer</button>` : ''}`;
  }
  return `${head}${picked}<div class="muted">Click the value you want to use.</div><div class="ig-jtree" data-key="answer-${t.answer}">${jsonTreeHtml(t.json, n)}</div>`;
}

// Send the request with the typed values, escaped exactly as a run would.
async function tryRequest(s, handler){
  if (!s) return;
  const t = tryEntry(s);
  const vars = Object.assign(sampleMap(handler), t.inputs);
  // The same choice as the runner: `{"a":…}` or `[…]` is JSON, `{user}` a variable.
  const jsonBody = /^\s*(\[|\{\s*["}])/.test(s.params.body || '');
  const formBody = /application\/x-www-form-urlencoded/i.test(s.params.headers || '');
  const r = await api('/integrations/fetch', {
    method: s.params.method || 'POST',
    url: renderSample(s.params.url, vars, urlComponent).trim(),
    headers: renderSample(s.params.headers, vars, oneLine),
    body: renderSample(s.params.body, vars, jsonBody ? jsonStringContent : formBody ? urlComponent : null),
  });
  t.picked = null;
  if (!r.ok){ t.error = r.error || 'The request failed'; t.status = 0; return; }
  Object.assign(t, {error:'', status:r.status, ms:Math.round(r.ms), body:r.body, truncated:r.truncated,
    json:parseJson(r.body), answer:(t.answer || 0) + 1});
}

function firstStep(handler, type){
  let found = null;
  walk(handler && handler.steps, s => { if (!found && s.type === type) found = s; });
  return found;
}

function parseJson(text){
  try {
    const v = JSON.parse(text);
    return v !== null && typeof v === 'object' ? v : undefined;
  } catch(_){ return undefined; }
}
// Every value in a JSON answer, with the path a variable uses to reach it.
// `usable`: the path fits a variable name (letters, digits, _).
function jsonLeaves(root){
  const out = [];
  const visit = (v, path) => {
    if (out.length >= 200) return;
    if (v !== null && typeof v === 'object'){
      (Array.isArray(v) ? v.slice(0, 25).map((x, i) => [i, x]) : Object.entries(v)).forEach(([k, x]) => visit(x, path.concat(k)));
      return;
    }
    out.push({path:path.join('.'), text:v === null ? '' : String(v), usable:path.length > 0 && path.every(k => /^[A-Za-z0-9_]+$/.test(String(k)))});
  };
  visit(root, []);
  return out;
}
// The answer as a tree; every value is a button that picks its variable.
function jsonTreeHtml(root, saveAs){
  let budget = 400;
  const node = (v, path, key, depth, index) => {
    if (budget-- <= 0) return '';
    const label = key === null ? '' : `<span class="k">${esc(key)}</span>`;
    // The first level eases in one row after another.
    const order = depth === 1 ? ` style="--i:${index}"` : '';
    if (v !== null && typeof v === 'object'){
      const all = Array.isArray(v) ? v.length : Object.keys(v).length;
      const entries = Array.isArray(v) ? v.slice(0, 25).map((x, i) => [i, x]) : Object.entries(v).slice(0, 60);
      const kids = entries.map(([k, x], i) => node(x, path.concat(k), String(k), depth + 1, i)).join('')
        + (all > entries.length ? `<div class="ig-jmore">… ${all - entries.length} more</div>` : '');
      if (key === null) return kids || '<div class="muted">(empty)</div>';
      return `<details class="ig-jnode"${order} ${depth < 2 ? 'open' : ''}><summary>${svg('chev')}${label}<span class="ig-jcount">${Array.isArray(v) ? `[${all}]` : `{${all}}`}</span></summary><div class="ig-jkids">${kids}</div></details>`;
    }
    const name = `${saveAs}.json.${path.join('.')}`;
    const usable = path.every(k => /^[A-Za-z0-9_]+$/.test(String(k))) && name.length <= 64;
    const text = v === null ? 'null' : String(v);
    const shown = typeof v === 'string' ? `"${text.length > 80 ? text.slice(0, 80) + '…' : text}"` : text;
    const kind = v === null ? 'null' : typeof v;
    return usable
      ? `<button class="ig-jleaf"${order} data-act="b-pick" data-token="${esc(name)}" data-sample="${esc(text.slice(0, 200))}" title="{${esc(name)}}">${label}<span class="v ${kind}">${esc(shown)}</span><span class="ig-juse">Use</span></button>`
      : `<div class="ig-jleaf off"${order} title="This name has characters a variable can't reach">${label}<span class="v ${kind}">${esc(shown)}</span></div>`;
  };
  return node(root, [], null, 0, 0);
}

