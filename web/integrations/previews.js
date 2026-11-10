// ---------------------------------------------------------------- previews

// HTML for what a step sends, styled like where it lands.
function previewHtml(step, handler, big){
  const cls = 'ig-pv' + (big ? ' big' : '');
  if (!step){
    const n = handler ? (handler.steps || []).length : 0;
    return `<div class="${cls} ig-pv-action"><span>${svg('steps')}</span>${plural(n, 'step')}</div>`;
  }
  const vars = sampleMap(handler);
  const k = KINDS[step.type] || {};
  const text = k.text ? renderSample(step.params[k.text], vars) : '';
  const ask = handler && handler.trigger && handler.trigger.type === 'chat_command'
    ? (handler.trigger.command || '!command')
    : handler && handler.trigger && handler.trigger.type === 'chat_message' ? (handler.trigger.pattern || 'hey') : '';
  switch (step.type){
    case 'discord': {
      const ping = step.params.ping === 'here' ? '@here ' : step.params.ping === 'everyone' ? '@everyone ' : '';
      if (step.params.style === 'card'){
        const above = renderSample(step.params.above, vars).trim();
        return `<div class="${cls} ig-pv-discord"><span class="ig-av">${svg('shield')}</span><span class="ig-pv-col">
          <b>InstantClone</b><span class="ig-app">APP</span>
          ${ping || above ? `<div class="ig-txt" translate="no">${discordMarkdown(ping + above)}</div>` : ''}${cardHtml(step, vars, big)}</span></div>`;
      }
      return `<div class="${cls} ig-pv-discord"><span class="ig-av">${svg('shield')}</span><span class="ig-pv-col">
        <b>InstantClone</b><span class="ig-app">APP</span><div class="ig-txt" translate="no">${discordTimes(esc(ping + text))}</div></span></div>`;
    }
    case 'dock':
      return `<div class="${cls} ig-pv-timeline">${step.params.group ? `<span class="ig-pv-tl-kind note">${esc(renderSample(step.params.group, vars))}</span>` : ''}
        <div translate="no">${esc(text || 'An event')}</div></div>`;
    case 'timeline': {
      const live = step.params.at !== 'aired';
      const at = vars[live ? 'vod_time' : 'uptime'] || '1:02:14';
      return `<div class="${cls} ig-pv-timeline"><span class="ig-pv-tl-kind ${esc(step.params.kind || 'note')}">${esc(TIMELINE_KINDS[step.params.kind] || TIMELINE_KINDS.note)}</span>
        <div translate="no"><code>${esc(at)}</code>${discordMarkdown(text || 'A moment')}</div></div>`;
    }
    case 'chat':
      return `<div class="${cls} ig-pv-chat">${ask ? `<div><span class="u1">viewer</span>: <span translate="no">${esc(ask)}</span></div>` : ''}
        <div><span class="u2">${esc(chatAuthor(step))}</span>: <span translate="no">${esc(text)}</span></div></div>`;
    case 'phone':
      return `<div class="${cls} ig-pv-phone"><span class="ig-av"></span><span class="ig-pv-col">
        <small>INSTANTCLONE · now</small><b translate="no">${esc(renderSample(step.params.title, vars) || 'InstantClone')}</b><div translate="no">${esc(text)}</div></span></div>`;
    case 'overlay':
      return `<div class="${cls} ig-pv-overlay"><div class="ig-pv-alert">
        ${step.params.title ? `<small translate="no">${esc(renderSample(step.params.title, vars))}</small>` : ''}<b${text ? ' translate="no"' : ''}>${esc(text || 'Something on stream')}</b><i></i></div></div>`;
    case 'marker':
      return `<div class="${cls} ig-pv-marker"><span class="tag">${esc(text || 'Marker')} · 1:02:14</span>
        <div class="bar"><i></i><b style="left:22%"></b><b style="left:48%"></b><b style="left:80%"></b></div></div>`;
    case 'http': {
      const body = text.trim();
      let shown = body ? body : `${step.params.method || 'POST'} ${maskUrl(step.params.url) || '(address to set)'}`;
      try { shown = JSON.stringify(JSON.parse(body), null, 1).replace(/\n\s*/g, ' '); } catch(_){}
      return `<div class="${cls} ig-pv-json">${esc(shown).replace(/&quot;([^&]*)&quot;:/g, '<span class="k">"$1"</span>:')}</div>`;
    }
    case 'file':
      return `<div class="${cls} ig-pv-file"><code>${esc((step.params.path || 'file.txt').split(/[\\/]/).pop())}</code><span>${esc(text)}</span></div>`;
    case 'clip':
      return `<div class="${cls} ig-pv-action"><span>${svg('film')}</span>Clips the last moments, and hands you the link</div>`;
    case 'delay_action':
      return `<div class="${cls} ig-pv-action"><span>${svg('cut')}</span>${esc(delayActionLabel(step.params, vars))}</div>`;
    case 'light': {
      const rgb = lightRgb(step.params.color);
      const off = step.params.action === 'off';
      return `<div class="${cls} ig-pv-action"><span class="ig-bulb${off ? ' off' : ''}${step.params.action === 'flash' ? ' flash' : ''}" style="--bulb:${off ? 'transparent' : rgb}">${svg('bulb')}</span>${esc(lightSummary(step.params))}</div>`;
    }
    case 'program':
      return `<div class="${cls} ig-pv-action"><span>${svg('terminal')}</span>${esc('Runs ' + ((step.params.path || '').split(/[\\/]/).pop() || 'a program'))}</div>`;
  }
  return `<div class="${cls} ig-pv-action"><span>${svg(k.icon || 'steps')}</span>${esc(k.label || step.type)}</div>`;
}
function chatAuthor(step){
  const t = S.data.twitch;
  return (step.params.as === 'bot' && t.bot.login) || t.main.login || 'you';
}

