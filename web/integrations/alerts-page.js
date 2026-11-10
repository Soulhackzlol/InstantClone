// ---------------------------------------------------------------- alerts page

// The alerts browser source: where "Show on stream" cards appear. Each
// alert plays a design (an overlay made in the Studio): its own, or the
// default (S.data.alerts.look.design). The built-in designs are the
// themes' cards, as layers. Every preview here is the real page
// (/alerts?demo) drawing a sample, so what you see is what OBS shows.

// The name of the design an alert plays: its own (`slug`), or the default
// when it has none or its own is gone.
function designLabel(slug){
  const d = Stage.designNamed(slug || '') || Stage.designNamed(lookOf().design || '');
  return d ? d.name : slug || lookOf().design || 'the default';
}
// The designs an alert can play, as tiles: yours, then the ready-made ones
// in their card's colours. The one it plays (`slug`, or the default) is lit.
function designTilesHtml(slug){
  const cur = Stage.designNamed(slug || '') || Stage.designNamed(lookOf().design || '');
  const tile = d => {
    const t = THEMES[d.theme], on = !!cur && cur.slug === d.slug;
    return `<button class="ovg-tile${on ? ' sel' : ''}" data-act="pick-design" data-slug="${esc(d.slug)}" aria-pressed="${on}" style="--tc:var(--accent)">
      ${designIconHtml(d.theme)}<span class="ovg-tile-txt"><span class="ovg-tile-name">${esc(d.name)}</span>
      <span class="ovg-tile-path">${esc(t ? t.motion : '')}</span></span></button>`;
  };
  const list = Stage.allDesigns(), mine = list.filter(d => !d.theme), ready = list.filter(d => d.theme);
  return (mine.length ? `<div class="ovg-group">Yours</div><div class="og-dgrid">${mine.map(tile).join('')}</div>` : '')
    + `<div class="ovg-group">Ready-made</div><div class="og-dgrid">${ready.map(tile).join('')}</div>`;
}
// A design's icon: a ready-made one's card colours (its words on its
// background, under a line of its accent), or layers for one of yours.
function designIconHtml(theme){
  const t = THEMES[theme];
  return t ? `<span class="og-dswatch" style="--sb:${t.bg};--sf:${t.fg};--sa:${t.accent}">Aa</span>`
    : `<span class="ovg-tile-ic">${svg('layers')}</span>`;
}
// How alerts look is the Overlay tab's Alerts page: there from anywhere,
// leaving an editor only if it can be left.
function openAlertsPage(){
  if (modalOpen()){ Modal.close(); if (modalOpen()) return; }
  showTab('overlay');
  showSubTab('overlay', 'alerts');
}

// Each theme as web/alerts.html draws it: its name, colors, opacity (%),
// corners (px), font, sound, its own entrance and where a picture goes.
const THEMES = {
  island:{name:'Island', fg:'#ffffff', bg:'#000000', accent:'#5ac8fa', op:96, r:40, font:'Inter', sound:'chime', motion:'Grows from a dot', pic:'left'},
  signal:{name:'Signal', fg:'#eef4ff', bg:'#0b0f14', accent:'#3ee0c5', op:86, r:0, font:'Chakra Petch', sound:'zap', motion:'Decodes', pic:'left'},
  banner:{name:'Banner', fg:'#0b0d12', bg:'#fcd34d', accent:'#fcd34d', op:100, r:0, font:'Bebas Neue', sound:'fanfare', motion:'Wipes on', pic:'left'},
  bubble:{name:'Bubble', fg:'#1b1d24', bg:'#ffffff', accent:'#a78bfa', op:100, r:30, font:'Fredoka', sound:'pop', motion:'Boings out', pic:'left'},
  minimal:{name:'Minimal', fg:'#ffffff', bg:'#000000', accent:'#5ac8fa', op:0, r:0, font:'Inter', sound:'blip', motion:'Inks in', pic:'left'},
  paper:{name:'Paper', fg:'#1d1a16', bg:'#f4efe4', accent:'#e2553d', op:100, r:3, font:'Permanent Marker', sound:'whoosh', motion:'Slaps on', pic:'left'},
  cinema:{name:'Cinema', fg:'#f4ede1', bg:'#0a0908', accent:'#d4b06a', op:90, r:2, font:'Playfair Display', sound:'bell', motion:'Opens like a shutter', pic:'top'},
};
// alerts::FONTS, ANIMATIONS and SOUNDS.
const ALERT_FONTS = ['Inter', 'Poppins', 'Montserrat', 'Fredoka', 'Rubik', 'Bebas Neue', 'Oswald', 'Bangers', 'Orbitron',
  'Press Start 2P', 'Permanent Marker', 'Roboto Mono', 'Chakra Petch', 'Playfair Display', 'Special Elite'];
const ALERT_ANIMS = [['slide', 'Slide'], ['pop', 'Pop'], ['fade', 'Fade'], ['zoom', 'Zoom'], ['bounce', 'Bounce'], ['drop', 'Drop']];
const ALERT_SOUNDS = [['chime', 'Chime'], ['pop', 'Pop'], ['bell', 'Bell'], ['rise', 'Rise'], ['coin', 'Coin'],
  ['sparkle', 'Sparkle'], ['whoosh', 'Whoosh'], ['fanfare', 'Fanfare'], ['zap', 'Zap'], ['blip', 'Blip']];
const ALERT_SECONDS = ['3', '6', '10', '15', '30'];
// A link the alerts page can load as it is (alerts::is_web_link), and an
// upload (alert_media's `media:<name>`).
const WEB_LINK = /^https?:\/\/[^\s"'<>\\`{}]+$/;
const MEDIA_REF = /^media:[a-z0-9._-]+$/;

function lookOf(){ return (S.data.alerts || {}).look || {}; }
function themeOf(look){ return THEMES[look.theme] ? look.theme : 'island'; }
function mediaSrc(ref){ return MEDIA_REF.test(ref || '') ? '/alerts/media/' + ref.slice(6) : WEB_LINK.test(ref || '') ? ref : ''; }
function mediaOf(kind){ return ((S.data.alerts || {}).media || []).filter(m => m.kind === kind); }

function soundLabel(sound){
  if (!sound) return 'none';
  if (sound === 'theme') return 'the design\'s';
  const found = ALERT_SOUNDS.find(([id]) => id === sound);
  if (found) return found[1].toLowerCase();
  return MEDIA_REF.test(sound) ? sound.slice(6) : 'a link';
}
// The accent an alert shows: its own, the page's, or the theme's.
function alertColor(value, look){
  if (/^#[0-9a-f]{6}$/i.test(value || '')) return value.toLowerCase();
  return /^#[0-9a-f]{6}$/i.test(look.accent || '') ? look.accent : THEMES[themeOf(look)].accent;
}
function alertColorName(value){
  if (!value) return 'the design\'s';
  const found = CARD_COLORS.find(([c]) => c === value.toLowerCase());
  return found ? found[1].toLowerCase() : value.toLowerCase();
}

// The Connections pill: [class, words].
function alertsPill(){
  const a = S.data.alerts || {};
  if (a.in_obs) return ['ok', 'in OBS'];
  if (a.elsewhere) return ['warn', 'not in OBS'];
  return ['todo', 'Add to OBS'];
}

// Is the alerts page in OBS, with one click to put it there. `compact`
// is the editor's line under an alert's preview; `after` ends the line.
function alertsStatusHtml(compact, after = ''){
  const a = S.data.alerts || {};
  const inObs = a.in_obs > 0;
  const line = inObs ? 'Your alerts page is open in OBS.'
    : a.elsewhere ? 'Your alerts page is open in a browser, but not in OBS: viewers won\'t see alerts.'
    : 'Your alerts page isn\'t in OBS yet: viewers won\'t see alerts.';
  const action = !inObs
    ? `<button class="ic-btn ic-btn-primary ig-small" data-act="alerts-add">${svg('plus', ' class="ig-btn-ic"')}Add to OBS</button>`
    : compact ? '' : '<button class="ic-btn ic-btn-ghost ig-small" data-act="alerts-add">Add to the scene on air</button>';
  return `<div class="ig-alerts-status${inObs ? ' ok' : ''}" data-key="alerts-status">
    <span class="ig-meter-dot ${inObs ? 'ok' : 'off'}"></span><span class="ig-alerts-line">${line}</span>${action}${after}</div>`;
}
