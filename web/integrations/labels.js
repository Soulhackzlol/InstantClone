function presetOf(i){
  const cat = S.data && S.data.catalog;
  return cat ? cat.presets.find(p => p.id === i.preset) : null;
}
function categoryOf(i){
  const p = presetOf(i);
  return p ? p.category : (i.preset === 'stream_alerts' ? 'alerts' : 'own');
}
function eventOf(id){ return (S.data && S.data.events.find(e => e.id === id)) || null; }
function channelName(id){
  const c = S.data && S.data.connections.discord.find(x => x.id === id);
  return c ? '#' + c.name : 'no channel yet';
}
function triggerLabel(t){
  if (!t) return '';
  switch (t.type){
    case 'event': {
      const e = eventOf(t.event), f = t.filters || {};
      if (!e) return t.event;
      // Several triggers on one event read apart: "On air: delay",
      // "Timeline: chapter".
      if (f.state || f.kind) return `${{onair_changed:'On air', timeline_updated:'Timeline'}[t.event] || e.label}: ${f.state || f.kind}`;
      return e.label;
    }
    case 'chat_command': return t.command || '!command';
    case 'chat_message': return 'Chat message';
    case 'block': return `Block: ${t.name || 'not named'}`;
    case 'timer': return `Every ${Math.round((t.every_ms||0)/60000)} min`;
    case 'webhook': return 'Web call';
    case 'shortcut': return shortcutLabel(t) || 'A button';
    case 'chat_activity': return 'Chat gets busy';
    case 'scene': return t.scene ? `Scene: ${sceneLabel(t)}` : 'An OBS scene';
  }
  return t.type;
}
// A threshold with its unit, from its label: "180 s", "3 drops", "10 min".
function paramShort(p, v){
  if (/second/i.test(p.label)) return `${v} s`;
  if (/minute/i.test(p.label)) return `${v} min`;
  return `${v} ${p.label.toLowerCase()}`;
}
// `Ctrl+Alt+K, pad 36 or Stream Deck`, or '' when nothing is set yet.
function shortcutLabel(t){
  const parts = [t.hotkey, t.midi && padLabel(t.midi), t.token && 'Stream Deck'].filter(Boolean);
  return parts.length > 1 ? parts.slice(0, -1).join(', ') + ' or ' + parts[parts.length - 1] : parts.join('');
}
// "3× busier, 5 people and 25% saying clip": a chat activity trigger in words.
function activitySummary(t){
  const parts = (t.rules || []).map(r => {
    const v = +r.value || 0;
    switch (r.kind){
      case 'busier': return `${v}× busier`;
      case 'messages': return `${v}+ messages`;
      case 'chatters': return `${v}+ people`;
      case 'words': return `${v}% saying ${(r.words || '').split(',')[0].trim() || '…'}`;
    }
    return r.kind;
  });
  if (!parts.length) return 'no rules yet';
  // Said a piece at a time: the joined sentence has no key to look up.
  const said = parts.map(part => tr(part));
  const joint = tr(t.match === 'any' ? 'or' : 'and');
  return said.length > 1 ? `${said.slice(0, -1).join(', ')} ${joint} ${said[said.length - 1]}` : said[0];
}
// `note:1:36@Launchpad` reads "pad 36 (Launchpad)".
function padLabel(sig){
  const m = /^(note|cc):(\d+):(\d+)(?:@(.+))?$/.exec(sig || '');
  if (!m) return sig || '';
  // A CC control works as a button: it presses at half way and up.
  return `${m[1] === 'note' ? 'pad' : 'button (CC)'} ${m[3]}${m[2] !== '1' ? ' ch ' + m[2] : ''}${m[4] ? ' (' + m[4] + ')' : ''}`;
}
// "The hold runs out" reads "When the hold runs out", "A web call" "When
// a web call"; "OBS crashed" stays.
function lowerFirst(s){
  return /^[A-Z][a-z ]/.test(s || '') ? s[0].toLowerCase() + s.slice(1) : s;
}

