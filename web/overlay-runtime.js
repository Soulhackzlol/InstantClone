/*
  InstantClone Overlay Studio - author-time runtime + baker.

  This file is loaded ONLY in the dashboard (Studio editor). It is never
  loaded by a live overlay in OBS. Its job:

    1. Describe the widget catalogue, animation library, and the editable
       per-state property set (so the Studio UI can build its palette and
       property panel from one source of truth).
    2. bake(doc) -> a self-contained static HTML string. That string is
       what gets saved as overlays/<slug>.html and served to OBS. It
       inlines only the CSS + JS the doc actually uses, animates on the
       compositor (transform/opacity), pauses when idle, and talks to the
       existing /events SSE stream. Its cost profile matches a hand-written
       static overlay - the editor's complexity never reaches the stream.

  The Studio canvas is just an iframe whose srcdoc = bake(doc); during a
  drag the editor pokes the (same-origin) canvas DOM directly for
  smoothness, then re-bakes on commit.
*/
'use strict';
(function (root) {

  // ---- Enumerations surfaced to the Studio UI ----

  // The six editable delay-states. Idle folds in "ingest offline".
  const STATES = [
    { key: 'idle',      label: 'Idle',      hint: 'Passthrough / OBS not publishing' },
    { key: 'preparing', label: 'Arming',    hint: 'Buffer filling toward the armed delay' },
    { key: 'ready',     label: 'Ready',     hint: 'Armed - waiting for Activate' },
    { key: 'active',    label: 'Active',    hint: 'Delay is live' },
    { key: 'error',     label: 'Error',     hint: 'A destination dropped while publishing' }
  ];

  const ANCHORS = [
    'top-left', 'top', 'top-right',
    'left', 'center', 'right',
    'bottom-left', 'bottom', 'bottom-right'
  ];

  // Animation library. is_loop = continuous (duration is a tempo);
  // one-shots play once on state entry (duration is a length).
  const ANIMS = [
    { key: 'instant',     label: 'None',        loop: false },
    { key: 'fade',        label: 'Fade',        loop: false },
    { key: 'slide-up',    label: 'Slide up',    loop: false },
    { key: 'slide-down',  label: 'Slide down',  loop: false },
    { key: 'slide-left',  label: 'Slide left',  loop: false },
    { key: 'slide-right', label: 'Slide right', loop: false },
    { key: 'scale-in',    label: 'Pop in',      loop: false },
    { key: 'pulse',       label: 'Pulse',       loop: true },
    { key: 'breathe',     label: 'Breathe',     loop: true },
    { key: 'glow',        label: 'Glow',        loop: true },
    { key: 'flash',       label: 'Flash',       loop: false },
    { key: 'shake',       label: 'Shake',       loop: false },
    { key: 'glitch',      label: 'Glitch',      loop: false }
  ];

  const EASINGS = [
    { key: 'linear',      css: 'linear' },
    { key: 'ease-out',    css: 'cubic-bezier(.2,.7,.2,1)' },
    { key: 'ease-in',     css: 'cubic-bezier(.5,0,.75,0)' },
    { key: 'ease-in-out', css: 'cubic-bezier(.65,0,.35,1)' },
    { key: 'spring',      css: 'cubic-bezier(.16,1,.3,1)' },
    { key: 'sharp',       css: 'cubic-bezier(.8,0,.2,1)' }
  ];

  // /state fields a data widget can bind to. value() pulls a display
  // number; unit is the default suffix the widget shows.
  const BINDINGS = {
    target_delay_ms: { label: 'Delay (active)',  unit: 's', kind: 'sec' },
    armed_delay_ms:  { label: 'Armed delay',     unit: 's', kind: 'sec' },
    buffer_fill_ms:  { label: 'Buffer fill',     unit: 's', kind: 'sec' },
    bitrate_kbps:    { label: 'Bitrate',         unit: 'kbps', kind: 'int' },
    cuts:            { label: 'Cut count',       unit: '', kind: 'int' },
    dest_alive:      { label: 'Destinations up', unit: '', kind: 'int' }
  };

  // Widget catalogue. `fields` lists which editable controls the Studio
  // property panel shows for this kind beyond the universal ones
  // (anchor/offset/size + per-state visible/color/opacity/anim).
  const KINDS = [
    { key: 'DelayReadout',      label: 'Delay readout',  group: 'data',  fields: ['bind', 'font', 'unit', 'text'] },
    { key: 'StatePill',         label: 'State pill',      group: 'data',  fields: ['font', 'text'] },
    { key: 'BufferBar',         label: 'Buffer bar',      group: 'data',  fields: [] },
    { key: 'BufferRing',        label: 'Buffer ring',     group: 'data',  fields: ['font'] },
    { key: 'BitrateMeter',      label: 'Graph meter',     group: 'data',  fields: ['bind'] },
    { key: 'DestinationStatus', label: 'Destinations',    group: 'data',  fields: [] },
    { key: 'CutCounter',        label: 'Cut counter',     group: 'data',  fields: ['font', 'text'] },
    { key: 'SessionElapsed',    label: 'Session timer',   group: 'data',  fields: ['font'] },
    { key: 'Text',              label: 'Text',            group: 'deco',  fields: ['font', 'text'] },
    { key: 'Icon',              label: 'Icon',            group: 'deco',  fields: ['icon'] },
    { key: 'Shape',             label: 'Shape',           group: 'deco',  fields: ['shape'] },
    { key: 'Divider',           label: 'Divider',         group: 'deco',  fields: [] },
    { key: 'CornerFrame',       label: 'Corner frame',    group: 'deco',  fields: [], full: true },
    { key: 'PulsingDot',        label: 'Pulsing dot',     group: 'ambient', fields: [] },
    { key: 'Heartbeat',         label: 'Heartbeat ring',  group: 'ambient', fields: [] },
    { key: 'Banner',            label: 'Lower-third',     group: 'ambient', fields: ['font', 'text'] },
    { key: 'EdgeAccent',        label: 'Edge accent',     group: 'ambient', fields: [], full: true, cost: 'mid' },
    { key: 'BackgroundTint',    label: 'Background wash', group: 'ambient', fields: [], full: true, cost: 'mid' },
    { key: 'LiquidFill',        label: 'Liquid fill',     group: 'ambient', fields: [], full: true, cost: 'mid' },
    { key: 'Image',             label: 'Image',           group: 'media', fields: ['src'] },
    { key: 'CustomHTML',        label: 'Custom HTML',     group: 'power', fields: ['custom'], cost: 'opt' }
  ];

  const ICONS = ['bolt', 'eye', 'star', 'clock', 'shield', 'dot', 'live', 'wifi', 'alert', 'play'];
  const SHAPES = ['rect', 'circle', 'pill', 'line'];

  // ---- Defaults ----

  function defaultStateProps() {
    return {}; // sparse - falls back to base
  }

  function defaultWidget(kind, id) {
    const k = KINDS.find(function (x) { return x.key === kind; }) || KINDS[0];
    const SIZES = {
      Icon: { w: 48, h: 48 }, PulsingDot: { w: 16, h: 16 }, BufferRing: { w: 120, h: 120 },
      Heartbeat: { w: 110, h: 110 }, BitrateMeter: { w: 180, h: 36 }, BufferBar: { w: 180, h: 10 },
      Shape: { w: 120, h: 60 }, Divider: { w: 160, h: 2 }, Image: { w: 200, h: 200 }, CustomHTML: { w: 320, h: 120 }
    };
    const w = {
      id: id,
      kind: kind,
      anchor: k.full ? 'center' : 'top-left',
      offset: { x: 96, y: 62 },
      size: SIZES[kind] || { w: 200, h: 80 },
      font_px: 34,
      bind: kind === 'DelayReadout' ? 'target_delay_ms' : (kind === 'BitrateMeter' ? 'bitrate_kbps' : 'target_delay_ms'),
      unit: 's',
      icon: 'bolt',
      shape: 'rect',
      src: '',
      base: { color: '#5ac8fa', opacity: 1, text: null, anim: { style: 'fade', duration_ms: 400, easing: 'ease-out', delay_ms: 0 } },
      states: { idle: { visible: false } },
      custom: kind === 'CustomHTML' ? defaultCustom() : null
    };
    if (kind === 'StatePill' || kind === 'Text') { w.base.text = 'DELAY'; }
    // The meter smooths its line by default so a steady signal reads calm
    // (0 = raw/jittery, ~1 = glassy). Tunable in the Studio.
    if (kind === 'BitrateMeter') { w.smooth = 0.6; }
    return w;
  }

  function defaultCustom() {
    return {
      html: '<div class="box">{{phase}} - {{delay}}s</div>',
      css: '.box{font:600 28px Inter,sans-serif;color:#5ac8fa;padding:10px 16px;\n  background:rgba(10,12,16,.6);border-radius:12px}',
      js: '// window.ic is live. Example:\n// ic.onUpdate(s => { /* s.delay, s.phase, s.bufferPct ... */ });'
    };
  }

  function defaultTheme() {
    return { accent: '#5ac8fa', live: '#54e38e', warn: '#ffb73a', error: '#ff5a5a', ink: '#f4f6f9', grey: '#8a8f9a' };
  }
  function defaultDoc(name) {
    return {
      name: name || 'Untitled overlay',
      version: 1,
      canvas: { w: 1920, h: 1080, safe_area: true },
      theme: defaultTheme(),
      widgets: []
    };
  }
  // The CSS custom properties that back the colour tokens (@accent, etc.).
  function themeCss(theme) {
    const t = Object.assign(defaultTheme(), theme || {});
    return 'body{' + Object.keys(t).map(function (k) { return '--ic-' + k + ':' + t[k]; }).join(';') + '}';
  }

  // ---- Small shared helpers (pure) ----

  // NOTE: string-based replaces (split/join), not regex literals. The
  // build-time minifier in build.rs misparses a `/"/g`-style regex (the
  // quote opens a phantom string), so we avoid regex with quote chars in
  // this file's top-level code. Regexes that live inside the String.raw
  // runtime templates are fine - those lines pass through verbatim.
  function esc(s) {
    return String(s == null ? '' : s)
      .split('&').join('&amp;').split('<').join('&lt;')
      .split('>').join('&gt;').split('"').join('&quot;');
  }
  function easingCss(key) {
    const e = EASINGS.find(function (x) { return x.key === key; });
    return e ? e.css : 'ease';
  }
  function anchorAxes(anchor) {
    const parts = anchor.split('-');
    return {
      vert: parts.length === 2 ? parts[0] : (anchor === 'top' || anchor === 'bottom' ? anchor : 'cv'),
      horiz: parts.length === 2 ? parts[1] : (anchor === 'left' || anchor === 'right' ? anchor : 'ch')
    };
  }
  // Just the offset declarations (top/left/right/bottom/margin) for an
  // anchor+offset - used both by the base position and by per-state
  // position overrides (which animate via the `.icw` layout transition).
  function offsetDecls(anchor, off) {
    const a = anchorAxes(anchor);
    let css = '';
    if (a.vert === 'top') css += 'top:' + off.y + 'px;';
    else if (a.vert === 'bottom') css += 'bottom:' + off.y + 'px;';
    else css += 'margin-top:' + off.y + 'px;';
    if (a.horiz === 'left') css += 'left:' + off.x + 'px;';
    else if (a.horiz === 'right') css += 'right:' + off.x + 'px;';
    else css += 'margin-left:' + off.x + 'px;';
    return css;
  }
  function anchorCss(anchor, off) {
    const a = anchorAxes(anchor);
    let css = 'position:fixed;';
    if (a.vert === 'cv') css += 'top:50%;';
    if (a.horiz === 'ch') css += 'left:50%;';
    return css + offsetDecls(anchor, off);
  }

  // ---- Animation keyframes (compositor-only: transform/opacity) ----
  // Emitted only for styles a doc actually uses.

  // Opacity-using keyframes reference var(--ic-op,1) so the per-state opacity
  // the user sets still applies while an opacity animation runs (otherwise the
  // animation would override it and the opacity control would look broken).
  // Parameterised keyframe bodies. `a` is an intensity multiplier (1 = the
  // default look); a state can dial each animation subtle (e.g. .4) or strong
  // (e.g. 2). round() keeps the emitted CSS tidy.
  function r(n) { return Math.round(n * 1000) / 1000; }
  const KEYFRAME_FN = {
    'slide-up': function (a) { return 'from{opacity:0;transform:translateY(' + r(40 * a) + 'px)}to{opacity:var(--ic-op,1);transform:none}'; },
    'slide-down': function (a) { return 'from{opacity:0;transform:translateY(' + r(-40 * a) + 'px)}to{opacity:var(--ic-op,1);transform:none}'; },
    'slide-left': function (a) { return 'from{opacity:0;transform:translateX(' + r(48 * a) + 'px)}to{opacity:var(--ic-op,1);transform:none}'; },
    'slide-right': function (a) { return 'from{opacity:0;transform:translateX(' + r(-48 * a) + 'px)}to{opacity:var(--ic-op,1);transform:none}'; },
    'scale-in': function (a) { return 'from{opacity:0;transform:scale(' + r(Math.max(0, 1 - 0.2 * a)) + ')}to{opacity:var(--ic-op,1);transform:none}'; },
    pulse: function (a) { return '0%,100%{transform:scale(1)}50%{transform:scale(' + r(1 + 0.07 * a) + ')}'; },
    breathe: function (a) { return '0%,100%{opacity:calc(var(--ic-op,1)*' + r(Math.max(0, 1 - 0.22 * a)) + ')}50%{opacity:var(--ic-op,1)}'; },
    glow: function (a) { return '0%,100%{opacity:calc(var(--ic-op,1)*' + r(Math.max(0, 1 - 0.45 * a)) + ')}50%{opacity:var(--ic-op,1)}'; },
    flash: function (a) { return '0%{opacity:var(--ic-op,1)}30%{opacity:calc(var(--ic-op,1)*' + r(Math.max(0, 1 - 0.85 * a)) + ')}100%{opacity:var(--ic-op,1)}'; },
    shake: function (a) { return '0%,100%{transform:translateX(0)}15%{transform:translateX(' + r(-12 * a) + 'px)}45%{transform:translateX(' + r(12 * a) + 'px)}70%{transform:translateX(' + r(-7 * a) + 'px)}'; },
    glitch: function (a) { return '0%,100%{transform:translate(0)}20%{transform:translate(' + r(-8 * a) + 'px,' + r(3 * a) + 'px)}40%{transform:translate(' + r(8 * a) + 'px,' + r(-3 * a) + 'px)}60%{transform:translate(' + r(-5 * a) + 'px,' + r(-3 * a) + 'px)}80%{transform:translate(' + r(5 * a) + 'px,' + r(3 * a) + 'px)}'; }
  };
  // Default (intensity 1) keyframes registered globally, plus the two that
  // take no intensity (fade, animated-gradient).
  const KEYFRAMES = {
    fade: '@keyframes ic-fade{from{opacity:0}to{opacity:var(--ic-op,1)}}',
    gradshift: '@keyframes ic-gradshift{0%{background-position:0% 50%}100%{background-position:200% 50%}}'
  };
  Object.keys(KEYFRAME_FN).forEach(function (k) { KEYFRAMES[k] = '@keyframes ic-' + k + '{' + KEYFRAME_FN[k](1) + '}'; });

  // One animation -> a single CSS `animation` value (no property prefix).
  function animValue(a, name) {
    if (!a || a.style === 'instant') return '';
    const dur = (a.duration_ms || 400) + 'ms';
    const ease = easingCss(a.easing || 'ease-out');
    const meta = ANIMS.find(function (x) { return x.key === a.style; });
    const iter = meta && meta.loop ? 'infinite' : '1';
    const delay = (a.delay_ms || 0) + 'ms';
    return (name || ('ic-' + a.style)) + ' ' + dur + ' ' + ease + ' ' + delay + ' ' + iter + ' both';
  }
  // The animation list for a state: per-state `anims` (array) or `anim`
  // (single), falling back to the widget's base animation.
  function animsForState(w, sp, stateKey) {
    if (sp.anims) return sp.anims;
    if (sp.anim) return [sp.anim];
    if (stateKey === 'idle') return [];
    if (w.base.anims) return w.base.anims;
    if (w.base.anim) return [w.base.anim];
    return [];
  }

  // ---- Base CSS (always present, tiny) ----

  const BASE_CSS = [
    '*{box-sizing:border-box}',
    "html,body{margin:0;padding:0;background:transparent;width:100%;height:100%;",
    "font-family:'Inter','SF Pro Display',-apple-system,Segoe UI,Roboto,sans-serif;",
    "-webkit-font-smoothing:antialiased;font-feature-settings:'tnum' 1}",
    // Colour crossfades and the widget glides to its per-state position
    // between phases. Transforms stay free for the keyframe animations.
    '.icw{will-change:transform,opacity;transition:color .45s cubic-bezier(.2,.7,.2,1),filter .45s ease,' +
      'opacity .45s cubic-bezier(.2,.7,.2,1),' +
      'top .5s cubic-bezier(.2,.8,.2,1),left .5s cubic-bezier(.2,.8,.2,1),' +
      'right .5s cubic-bezier(.2,.8,.2,1),bottom .5s cubic-bezier(.2,.8,.2,1),' +
      'margin-top .5s cubic-bezier(.2,.8,.2,1),margin-left .5s cubic-bezier(.2,.8,.2,1)}',
    '.icw .v{font-variant-numeric:tabular-nums;line-height:1}',
    // Auto-hide: fade + gentle drift out, then stop painting (visibility).
    '.icw.ic-gone{opacity:0 !important;visibility:hidden;transform:translateY(8px);transition:opacity .8s ease,transform .8s ease,visibility 0s linear .8s;pointer-events:none}',
    // LiquidFill recedes smoothly first (height -> 0 over .6s), THEN fades,
    // so the tide visibly drains before disappearing.
    '.icw[data-kind="LiquidFill"].ic-gone{transform:none;transition:opacity .5s ease .65s !important}',
    '.icw[data-kind="LiquidFill"].ic-gone .liquid{height:0 !important}',
    // Exit animations. We animate (not transition) the auto-hide exit because
    // the widget is usually mid state-animation, and a transition fired in the
    // same frame as stopping that animation gets skipped - so every exit used
    // to "just pop". A forwards keyframe always runs and holds the hidden pose.
    '@keyframes ic-x-fade{to{opacity:0}}',
    '@keyframes ic-x-down{to{opacity:0;transform:translateY(48px)}}',
    '@keyframes ic-x-up{to{opacity:0;transform:translateY(-48px)}}',
    '@keyframes ic-x-pop{to{opacity:0;transform:scale(.78)}}'
  ].join('\n');

  // ---- Per-widget HTML + CSS ----

  function widgetInner(w) {
    switch (w.kind) {
      case 'DelayReadout':
        return '<span class="v">0.0</span><span class="u">' + esc(w.unit || 's') + '</span>';
      case 'StatePill':
      case 'Text':
      case 'CutCounter':
        return '<span class="lbl">' + esc(w.base.text || '') + '</span>';
      case 'SessionElapsed':
        return '<span class="v">0:00</span>';
      case 'BufferBar':
        return '<span class="fill"></span>';
      case 'BufferRing':
        return '<svg viewBox="0 0 100 100"><circle class="trk" cx="50" cy="50" r="46"/>' +
          '<circle class="arc" cx="50" cy="50" r="46" stroke-dasharray="289" stroke-dashoffset="289"/></svg>' +
          (w.notext ? '' : '<span class="v">0.0</span>');
      case 'BitrateMeter':
        return '<svg viewBox="0 0 100 32" preserveAspectRatio="none"><polyline class="spark" points=""/></svg>';
      case 'DestinationStatus':
        return '<span class="dots"></span>';
      case 'Icon':
        return iconSvg(w.icon || 'bolt');
      case 'Shape':
        return '';
      case 'Divider':
        return '';
      case 'CornerFrame':
        return '<i class="c tl"></i><i class="c tr"></i><i class="c bl"></i><i class="c br"></i>';
      case 'PulsingDot':
        return '<span class="dot"></span>';
      case 'Heartbeat':
        return '<span class="ring r1"></span><span class="ring r2"></span><span class="core"></span>';
      case 'LiquidFill':
        return '<span class="liquid"><span class="wave"></span></span>';
      case 'Banner':
        return '<span class="lbl">' + esc(w.base.text || 'ON A DELAY') + '</span>';
      case 'EdgeAccent':
        return '';
      case 'BackgroundTint':
        return '';
      case 'Image':
        return w.src ? '<img src="' + esc(w.src) + '" alt="">' : '';
      case 'CustomHTML':
        return ''; // hydrated by the runtime into a sandboxed iframe
      default:
        return '';
    }
  }

  // CSS for one widget kind, scoped to #<id>. Compositor-only animation.
  function widgetCss(w) {
    const id = '#' + cssId(w.id);
    const fp = (w.font_px || 34);
    let css = '';
    switch (w.kind) {
      case 'DelayReadout':
        css = id + '{display:flex;align-items:baseline;gap:5px;color:currentColor}' +
          id + ' .v{font-size:' + fp + 'px;font-weight:700;letter-spacing:-1.4px;text-shadow:0 0 16px currentColor}' +
          id + ' .u{font-size:' + Math.round(fp * 0.46) + 'px;font-weight:500;opacity:.7}';
        break;
      case 'StatePill':
        // A clear status badge: glassy fill, a colour-matched ring, a glowing
        // leading dot - readable on any scene.
        css = id + '{display:inline-flex;align-items:center;gap:9px;padding:9px 18px;border-radius:999px;' +
          'background:rgba(8,10,14,.72);box-shadow:0 6px 24px rgba(0,0,0,.4), inset 0 0 0 1.5px currentColor;' +
          'font-size:' + Math.round(fp * 0.5) + 'px;font-weight:800;letter-spacing:1.6px;text-transform:uppercase}' +
          id + '::before{content:"";width:' + Math.round(fp * 0.28) + 'px;height:' + Math.round(fp * 0.28) + 'px;border-radius:50%;' +
          'background:currentColor;box-shadow:0 0 10px currentColor;flex:none}' +
          id + ' .lbl{color:currentColor;text-shadow:0 0 12px currentColor}';
        break;
      case 'Text':
        css = id + '{font-size:' + fp + 'px;font-weight:600;color:currentColor;text-shadow:0 1px 12px rgba(0,0,0,.4)}';
        break;
      case 'CutCounter':
        css = id + '{display:inline-flex;align-items:baseline;gap:6px;font-size:' + fp + 'px;font-weight:700;color:currentColor}';
        break;
      case 'SessionElapsed':
        css = id + '{font-size:' + fp + 'px;font-weight:600;color:currentColor;font-variant-numeric:tabular-nums}';
        break;
      case 'BufferBar':
        css = id + '{width:' + w.size.w + 'px;height:' + Math.max(3, w.size.h) + 'px;border-radius:999px;background:rgba(255,255,255,.12);overflow:hidden}' +
          id + ' .fill{display:block;height:100%;width:0;border-radius:999px;background:currentColor;' +
          'transform-origin:left center;transition:width var(--ic-sm,.42s) cubic-bezier(.16,1,.3,1)}';
        break;
      case 'BufferRing':
        css = id + '{position:relative;width:' + w.size.w + 'px;height:' + w.size.w + 'px;color:currentColor}' +
          id + ' svg{position:absolute;inset:0;transform:rotate(-90deg)}' +
          id + ' circle{fill:none;stroke-width:var(--ic-ring-w,4)}' +
          id + ' .trk{stroke:rgba(255,255,255,.10)}' +
          id + ' .arc{stroke:currentColor;stroke-linecap:round;transition:stroke-dashoffset var(--ic-sm,.42s) cubic-bezier(.16,1,.3,1)}' +
          id + ' .v{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;' +
          'font-size:' + Math.round(fp * 0.9) + 'px;font-weight:700}';
        break;
      case 'BitrateMeter':
        css = id + '{width:' + w.size.w + 'px;height:' + Math.max(16, w.size.h) + 'px;color:currentColor}' +
          id + ' svg{width:100%;height:100%}' +
          // non-scaling-stroke keeps the line crisp when the graph is stretched tall.
          id + ' .spark{fill:none;stroke:currentColor;stroke-width:2;vector-effect:non-scaling-stroke;stroke-linejoin:round;stroke-linecap:round}';
        break;
      case 'DestinationStatus':
        css = id + ' .dots{display:inline-flex;gap:7px}' +
          id + ' .dots i{width:10px;height:10px;border-radius:50%;background:rgba(255,255,255,.25)}' +
          id + ' .dots i.up{background:currentColor;box-shadow:0 0 8px currentColor}' +
          id + ' .dots i.down{background:#ff5a5a;box-shadow:0 0 8px #ff5a5a}';
        break;
      case 'Icon':
        css = id + '{display:inline-flex;color:currentColor}' +
          id + ' svg{width:' + w.size.w + 'px;height:' + w.size.w + 'px;display:block}';
        break;
      case 'Shape':
        css = id + '{width:' + w.size.w + 'px;height:' + w.size.h + 'px;background:currentColor;' +
          shapeRadius(w.shape) + '}';
        break;
      case 'Divider':
        css = id + '{width:' + w.size.w + 'px;height:' + Math.max(1, w.size.h) + 'px;background:currentColor;opacity:.5;border-radius:2px}';
        break;
      case 'CornerFrame':
        css = id + '{position:fixed;inset:0;pointer-events:none;color:currentColor}' +
          id + ' .c{position:absolute;width:var(--ic-cf-size,46px);height:var(--ic-cf-size,46px);' +
          'border:var(--ic-cf-w,3px) solid currentColor;opacity:.85}' +
          id + ' .tl{top:24px;left:24px;border-right:0;border-bottom:0}' +
          id + ' .tr{top:24px;right:24px;border-left:0;border-bottom:0}' +
          id + ' .bl{bottom:24px;left:24px;border-right:0;border-top:0}' +
          id + ' .br{bottom:24px;right:24px;border-left:0;border-top:0}';
        break;
      case 'PulsingDot':
        css = id + ' .dot{display:block;width:' + Math.max(10, w.size.h) + 'px;height:' + Math.max(10, w.size.h) + 'px;' +
          'border-radius:50%;background:currentColor;box-shadow:0 0 10px currentColor}';
        break;
      case 'Heartbeat': {
        const hb = Math.max(40, w.size.w);
        css = id + '{position:relative;width:' + hb + 'px;height:' + hb + 'px;color:currentColor;' +
          'display:flex;align-items:center;justify-content:center}' +
          id + ' .core{width:32%;height:32%;border-radius:50%;background:currentColor;' +
          'box-shadow:0 0 18px currentColor;animation:ic-beat calc(1.5s / var(--ic-spd-mul,1)) ease-in-out infinite}' +
          id + ' .ring{position:absolute;left:50%;top:50%;width:' + hb + 'px;height:' + hb + 'px;margin:' + (-hb / 2) + 'px;' +
          'border:2px solid currentColor;border-radius:50%;opacity:0;animation:ic-ripple calc(2.4s / var(--ic-spd-mul,1)) ease-out infinite}' +
          id + ' .r2{animation-delay:1.2s}';
        break;
      }
      case 'Banner':
        css = id + '{display:inline-flex;align-items:center;padding:11px 24px;border-radius:12px;' +
          'background:linear-gradient(90deg,rgba(8,10,14,.85),rgba(8,10,14,.45));color:currentColor;' +
          'box-shadow:0 8px 30px rgba(0,0,0,.45), inset 0 0 0 1px currentColor;' +
          'font-size:' + Math.round(fp * 0.6) + 'px;font-weight:800;letter-spacing:1.2px;text-shadow:0 0 14px currentColor}';
        break;
      case 'EdgeAccent':
        // Crisp inner line + a soft inward glow falloff = expressive border.
        // --ic-edge-w: line thickness, --ic-glow: glow reach.
        css = id + '{position:fixed;inset:0;pointer-events:none;' +
          'box-shadow:inset 0 0 0 var(--ic-edge-w,3px) currentColor, ' +
          'inset 0 0 var(--ic-glow,38px) -6px currentColor, inset 0 0 140px -52px currentColor}';
        break;
      case 'BackgroundTint':
        // --ic-tint: overall strength.
        css = id + '{position:fixed;inset:0;pointer-events:none;' +
          'background:radial-gradient(120% 80% at 50% 120%, currentColor, transparent 70%);opacity:var(--ic-tint,.22)}';
        break;
      case 'LiquidFill':
        // A tide that rises with the buffer and ripples when live. The
        // .liquid height is driven by applyData (buffer fraction).
        // Tunable: --ic-sm (rise smoothness), --ic-spd-mul (wave speed),
        // --ic-wav (wave intensity). Fallbacks keep the original look.
        css = id + '{position:fixed;inset:0;pointer-events:none;overflow:hidden}' +
          id + ' .liquid{position:absolute;left:0;right:0;bottom:0;height:0;' +
          'background:linear-gradient(to top, currentColor, transparent);opacity:.3;' +
          'transition:height var(--ic-sm,.6s) cubic-bezier(.2,.8,.2,1)}' +
          // Wave (--ic-wav) drives the CREST geometry: height + how it sits, so
          // 0 reads as a flat surface and 2 as a tall choppy crest - not just a
          // fade. Opacity stays constant.
          id + ' .wave{position:absolute;left:-25%;right:-25%;top:calc(-14px * var(--ic-wav,1));' +
          'height:calc(28px * var(--ic-wav,1));background:currentColor;' +
          'border-radius:45%;opacity:.5;animation:ic-tide calc(5s / var(--ic-spd-mul,1)) ease-in-out infinite}' +
          'body[data-state="active"] ' + id + ' .liquid{animation:ic-swell calc(5s / var(--ic-spd-mul,1)) ease-in-out infinite}';
        break;
      case 'Image':
        css = id + ' img{width:' + w.size.w + 'px;height:auto;display:block}';
        break;
      case 'CustomHTML':
        css = id + '{width:' + w.size.w + 'px;height:' + w.size.h + 'px}' +
          id + ' iframe{width:100%;height:100%;border:0;background:transparent}';
        break;
    }
    // Widget-local keyframes, declared lazily next to the widget CSS.
    if (w.kind === 'Heartbeat') {
      css = '@keyframes ic-ripple{0%{transform:scale(.22);opacity:.6}100%{transform:scale(1);opacity:0}}' +
        '@keyframes ic-beat{0%,100%{transform:scale(1)}12%{transform:scale(1.32)}24%{transform:scale(1)}38%{transform:scale(1.22)}52%{transform:scale(1)}}' + css;
    }
    if (w.kind === 'LiquidFill') {
      css = '@keyframes ic-tide{0%,100%{transform:translateX(calc(-4% * var(--ic-wav,1))) rotate(calc(-1deg * var(--ic-wav,1)))}50%{transform:translateX(calc(4% * var(--ic-wav,1))) rotate(calc(1deg * var(--ic-wav,1)))}}' +
        '@keyframes ic-swell{0%,100%{transform:translateY(0)}50%{transform:translateY(calc(-1.2% * var(--ic-wav,1)))}}' + css;
    }
    // Typography controls (text-bearing widgets). Weight targets the inner
    // text nodes so it overrides the per-widget defaults.
    if (['DelayReadout', 'StatePill', 'Text', 'CutCounter', 'SessionElapsed', 'Banner'].indexOf(w.kind) >= 0) {
      if (w.font_weight) css += id + ',' + id + ' .v,' + id + ' .lbl,' + id + ' .u{font-weight:' + w.font_weight + '}';
      if (w.letter_spacing != null) css += id + '{letter-spacing:' + w.letter_spacing + 'px}';
      if (w.uppercase) css += id + '{text-transform:uppercase}';
      else if (w.uppercase === false && w.kind === 'StatePill') css += id + '{text-transform:none}';
    }
    return css;
  }

  function shapeRadius(shape) {
    if (shape === 'circle') return 'border-radius:50%';
    if (shape === 'pill') return 'border-radius:999px';
    if (shape === 'line') return 'border-radius:2px';
    return 'border-radius:6px';
  }
  function cssId(id) { return String(id).replace(/[^a-zA-Z0-9_-]/g, ''); }

  // Per-state CSS: color / opacity / visibility / animation keyed on
  // body[data-state]. This is what makes the overlay JS-light - the
  // compositor handles all the per-state visual transitions.
  function stateCss(w) {
    const id = '#' + cssId(w.id);
    const safeId = (w.id || 'w').replace(/[^a-zA-Z0-9_-]/g, '');
    let out = '';
    let customKf = '';
    let usesAnim = {};
    STATES.forEach(function (st) {
      const sp = (w.states && w.states[st.key]) || {};
      const merged = mergeProps(w.base, sp);
      const sel = 'body[data-state="' + st.key + '"] ' + id;
      let rule = '';
      if (merged.visible === false) {
        // Fade out (then stop painting) instead of snapping off, so a widget
        // that drops out in a phase leaves smoothly. visibility flips after the
        // opacity fade so it costs nothing once hidden.
        rule += 'opacity:0;visibility:hidden;pointer-events:none;transition:opacity .45s ease,visibility 0s linear .45s;';
      } else {
        if (merged.color != null) rule += 'color:' + resolveColor(merged.color) + ';';
        // Opacity goes through --ic-op so opacity animations compose with it.
        const op = merged.opacity != null ? merged.opacity : 1;
        rule += '--ic-op:' + op + ';opacity:var(--ic-op);';
        // Per-state glow (drop-shadow halo in the widget's own colour).
        if (merged.glow) rule += 'filter:drop-shadow(0 0 ' + merged.glow + 'px currentColor);';
        // Per-state gradient (a second colour). Text widgets clip the
        // gradient onto the glyph spans directly (clipping on the flex
        // wrapper renders blank); solid widgets get a gradient background.
        if (merged.grad) {
          // 3-stop loop so an animated gradient wraps seamlessly.
          const c1 = resolveColor(merged.color), c2 = resolveColor(merged.grad);
          const grad = 'linear-gradient(135deg,' + c1 + ',' + c2 + ',' + c1 + ')';
          if (TEXT_KINDS.indexOf(w.kind) >= 0) {
            const ts = sel + ' .v,' + sel + ' .lbl,' + sel + ' .u';
            let g = 'background:' + grad + ';background-size:200% 100%;-webkit-background-clip:text;background-clip:text;-webkit-text-fill-color:transparent;color:transparent;';
            if (merged.grad_anim) { g += 'animation:ic-gradshift 3.5s linear infinite;'; usesAnim['gradshift'] = true; }
            out += ts + '{' + g + '}';
          } else if (w.kind === 'Shape' || w.kind === 'Banner' || w.kind === 'Divider') {
            rule += 'background:' + grad + ';background-size:200% 100%;';
            if (merged.grad_anim) { rule += 'animation:ic-gradshift 3.5s linear infinite;'; usesAnim['gradshift'] = true; }
          }
        }
        // Per-state position: the widget moves to a new spot in this state
        // and slides there via the `.icw` layout transition.
        if (sp.offset) rule += offsetDecls(w.anchor, sp.offset);
        const list = animsForState(w, sp, st.key);
        const named = list.filter(function (a) { return a && a.style !== 'instant'; });
        if (named.length) {
          const parts = [];
          named.forEach(function (a, idx) {
            // intensity: 1 uses the shared keyframe; anything else bakes a
            // per-state scaled variant so each animation tunes independently.
            const amp = (a.amount != null && a.amount > 0) ? a.amount : 1;
            let name = 'ic-' + a.style;
            if (amp !== 1 && KEYFRAME_FN[a.style]) {
              name = 'ic-' + a.style + '-' + safeId + st.key + idx;
              customKf += '@keyframes ' + name + '{' + KEYFRAME_FN[a.style](amp) + '}';
            } else {
              usesAnim[a.style] = true;
            }
            const v = animValue(a, name);
            if (v) parts.push(v);
          });
          if (parts.length) rule += 'animation:' + parts.join(',') + ';';
        }
      }
      if (rule) out += sel + '{' + rule + '}';
    });
    return { css: out + customKf, anims: usesAnim };
  }

  function mergeProps(base, over) {
    return {
      visible: over.visible != null ? over.visible : true,
      color: over.color != null ? over.color : base.color,
      opacity: over.opacity != null ? over.opacity : base.opacity,
      glow: over.glow != null ? over.glow : base.glow,
      grad: over.grad != null ? over.grad : base.grad,
      grad_anim: over.grad_anim != null ? over.grad_anim : base.grad_anim,
      text: over.text != null ? over.text : base.text
    };
  }

  // Colour tokens (@accent, @live, @warn, @error, @ink, @grey) resolve to
  // the overlay's theme variables so one theme drives every widget.
  function resolveColor(c) {
    if (typeof c === 'string' && c.charAt(0) === '@') return 'var(--ic-' + c.slice(1) + ')';
    return c;
  }
  // Text widgets whose glyphs can take a gradient (background-clip:text).
  const TEXT_KINDS = ['DelayReadout', 'StatePill', 'Text', 'CutCounter', 'SessionElapsed', 'Banner'];

  // ---- Icons (curated, currentColor) ----
  function iconSvg(name) {
    const P = 'fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"';
    const paths = {
      bolt: '<path d="M13 3 4 14h7l-1 7 9-11h-7z"/>',
      eye: '<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7-10-7-10-7z"/><circle cx="12" cy="12" r="3"/>',
      star: '<path d="M12 3l2.6 5.3 5.9.9-4.3 4.1 1 5.8L12 16.9 6.8 19.1l1-5.8L3.5 9.2l5.9-.9z"/>',
      clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
      shield: '<path d="M12 3l7 3v6c0 5-3.5 7.5-7 9-3.5-1.5-7-4-7-9V6z"/>',
      dot: '<circle cx="12" cy="12" r="4" fill="currentColor" stroke="none"/>',
      live: '<circle cx="12" cy="12" r="3" fill="currentColor" stroke="none"/><path d="M5 8a9 9 0 0 0 0 8M19 8a9 9 0 0 1 0 8"/>',
      wifi: '<path d="M5 12a10 10 0 0 1 14 0M8 15a6 6 0 0 1 8 0"/><circle cx="12" cy="18" r="1" fill="currentColor" stroke="none"/>',
      alert: '<path d="M12 4 3 19h18z"/><path d="M12 10v4M12 17h.01"/>',
      play: '<path d="M7 5l12 7-12 7z"/>'
    };
    return '<svg viewBox="0 0 24 24" ' + P + '>' + (paths[name] || paths.bolt) + '</svg>';
  }

  // ---- The live runtime that ships INSIDE every baked overlay ----
  // Self-contained: SSE subscribe, state mapping, body[data-state],
  // data-bound widget updates, cut/error detection, CustomHTML bridge.
  // Kept deliberately small. Stored as a string so bake() can inline it.
  const RUNTIME_JS = String.raw`
'use strict';
(function(){
  var body=document.body;
  // ?autohide=off on the OBS browser-source URL keeps the overlay up (skips the
  // baked per-state auto-hide timers) without re-baking the file.
  var AUTOHIDE=new URLSearchParams(location.search).get('autohide')!=='off';
  var forced=new URLSearchParams(location.search).get('state'); // sim override

  function deriveState(s){
    if(!s.ingest_alive) return 'idle';
    if(s.destinations_total>0 && s.destinations_alive===0) return 'error';
    if(s.phase==='preparing') return 'preparing';
    if(s.phase==='ready') return 'ready';
    if(s.phase==='active') return 'active';
    return 'idle';
  }

  var tweens={};
  function tweenNum(el,to,dp){
    var from=parseFloat(el.getAttribute('data-n')||'0')||0;
    if(Math.abs(to-from)<0.05){el.textContent=to.toFixed(dp);el.setAttribute('data-n',to);return;}
    var k=el.__k||(el.__k=Math.random());
    cancelAnimationFrame(tweens[k]);
    var t0=performance.now(),dur=400;
    function step(now){
      var t=Math.min(1,(now-t0)/dur),e=1-Math.pow(1-t,3);
      var v=from+(to-from)*e; el.textContent=v.toFixed(dp); el.setAttribute('data-n',v);
      if(t<1) tweens[k]=requestAnimationFrame(step);
    }
    tweens[k]=requestAnimationFrame(step);
  }

  function secs(s,bind){
    if(bind==='armed_delay_ms') return (s.armed_delay_ms||0)/1000;
    if(bind==='buffer_fill_ms') return (s.buffer_fill_ms||0)/1000;
    if(bind==='target_delay_ms') return (s.target_delay_ms||s.armed_delay_ms||s.current_delay_ms||0)/1000;
    return (s[bind]||0)/1000;
  }

  var spark=[]; var sessionStart=0;

  // Phase-aware readout: the default (target_delay_ms) ramps with the
  // buffer while arming, settles to the armed value when ready, and shows
  // the live delay when active - so a "delay readout" counts up as it arms.
  function readout(s,bind){
    if(bind==='bitrate_kbps') return {v:(s.stats&&s.stats.bitrate_kbps)||0,dp:0};
    if(bind==='cuts') return {v:(s.stats&&s.stats.cuts)||0,dp:0};
    if(bind==='dest_alive') return {v:s.destinations_alive||0,dp:0};
    if(bind==='armed_delay_ms') return {v:(s.armed_delay_ms||0)/1000,dp:1};
    if(bind==='buffer_fill_ms') return {v:(s.buffer_fill_ms||0)/1000,dp:1};
    if(s.phase==='preparing') return {v:(s.buffer_fill_ms||0)/1000,dp:1};
    if(s.phase==='ready') return {v:(s.armed_delay_ms||0)/1000,dp:1};
    return {v:(s.target_delay_ms||s.armed_delay_ms||0)/1000,dp:1};
  }

  function applyData(s){
    // DelayReadout
    var rd=document.querySelectorAll('[data-kind="DelayReadout"]');
    rd.forEach(function(el){
      var bind=el.getAttribute('data-bind')||'target_delay_ms';
      var r=readout(s,bind); var v=el.querySelector('.v'); if(v) tweenNum(v,r.v,r.dp);
    });
    // BufferBar + BufferRing fraction
    var frac=0;
    if(s.phase==='preparing'){var tgt=s.armed_delay_ms||s.buffer_target_ms||1;frac=Math.max(0,Math.min(1,(s.buffer_fill_ms||0)/tgt));}
    else if(s.phase==='ready'||s.phase==='active'){frac=1;}
    document.querySelectorAll('[data-kind="BufferBar"] .fill').forEach(function(f){f.style.width=(frac*100).toFixed(1)+'%';});
    // LiquidFill: a full-screen tide that rises with the buffer (full when
    // live). Skip while the widget is auto-hiding so the recede plays out.
    document.querySelectorAll('[data-kind="LiquidFill"] .liquid').forEach(function(l){
      var host=l.parentNode; if(host && (host.classList.contains('ic-gone')||host.hasAttribute('data-ic-gone'))) return;
      // Fixed level overrides the buffer-following fill.
      var lvl=host&&host.getAttribute('data-level');
      if(lvl!=null&&lvl!==''){ l.style.height=lvl+'%'; return; }
      l.style.height=(Math.max(frac, s.phase==='active'||s.phase==='ready'?1:frac)*100).toFixed(1)+'%';
    });
    document.querySelectorAll('[data-kind="BufferRing"]').forEach(function(el){
      var arc=el.querySelector('.arc'); if(arc) arc.style.strokeDashoffset=String(289*(1-frac));
      var rr=readout(s,'target_delay_ms'); var v=el.querySelector('.v'); if(v) tweenNum(v,rr.v,rr.dp);
    });
    // Graph meter - each meter keeps its own history of its bound variable
    // and auto-scales (min..max) so any metric draws a readable curve.
    document.querySelectorAll('[data-kind="BitrateMeter"]').forEach(function(el){
      var bind=el.getAttribute('data-bind')||'bitrate_kbps';
      var hist=el.__spark||(el.__spark=[]);
      hist.push(readout(s,bind).v); if(hist.length>48) hist.shift();
      // Smoothing: an EMA over the window damps jitter so a rock-steady signal
      // reads as a calm line, not amplified noise. 0 = raw, ~1 = glassy.
      var sm=parseFloat(el.getAttribute('data-smooth')); if(isNaN(sm)) sm=0.6;
      var a=Math.max(0,Math.min(0.95,sm)), prev=hist[0], line=[];
      for(var i=0;i<hist.length;i++){ prev=i?prev*a+hist[i]*(1-a):hist[i]; line.push(prev); }
      var mx=Math.max.apply(null,line), mn=Math.min.apply(null,line);
      // Range floor: tiny variation must NOT blow up to full height - a stable
      // stream should look stable. Only a real swing earns the full vertical.
      var rng=Math.max(mx-mn, mx*0.18, 1);
      var pts=line.map(function(y,i){return (i/(Math.max(1,line.length-1))*100).toFixed(1)+','+(30-((y-mn)/rng)*28).toFixed(1);}).join(' ');
      var p=el.querySelector('.spark'); if(p) p.setAttribute('points',pts);
    });
    // CutCounter
    var cuts=(s.stats&&s.stats.cuts)||0;
    document.querySelectorAll('[data-kind="CutCounter"] .lbl').forEach(function(l){var tpl=l.getAttribute('data-tpl')||'{n}';l.textContent=tpl.replace('{n}',cuts);});
    // DestinationStatus dots
    document.querySelectorAll('[data-kind="DestinationStatus"] .dots').forEach(function(d){
      var list=s.destinations||[]; var html='';
      for(var i=0;i<list.length;i++){html+='<i class="'+(list[i].alive?'up':'down')+'"></i>';}
      if(!list.length) html='<i></i>';
      if(d.getAttribute('data-c')!==String(list.length)+(list.map(function(x){return x.alive?1:0}).join(''))){
        d.innerHTML=html; d.setAttribute('data-c',String(list.length)+(list.map(function(x){return x.alive?1:0}).join('')));
      }
    });
    // SessionElapsed
    if(s.ingest_alive&&!sessionStart) sessionStart=Date.now();
    if(!s.ingest_alive) sessionStart=0;
    document.querySelectorAll('[data-kind="SessionElapsed"] .v').forEach(function(v){
      var sec=sessionStart?Math.floor((Date.now()-sessionStart)/1000):0;
      var m=Math.floor(sec/60),ss=sec%60; v.textContent=m+':'+(ss<10?'0':'')+ss;
    });
  }

  // Per-state text overrides (data-txt-<state>) with live {{var}} support
  // so a Text/StatePill/Banner can read {{delay}}, {{bitrate}}, etc.
  function substVars(t,view){ return t.replace(/\{\{(\w+)\}\}/g,function(m,k){ return view&&view[k]!=null?view[k]:''; }); }
  function applyText(stateKey,view){
    document.querySelectorAll('[data-has-txt]').forEach(function(el){
      var lbl=el.querySelector('.lbl'); if(lbl==null) return;
      var t=el.getAttribute('data-txt-'+stateKey); if(t==null) t=el.getAttribute('data-txt-base');
      if(t!=null) lbl.textContent = (t.indexOf('{{')>=0) ? substVars(t,view) : t;
    });
  }

  // CustomHTML bridge: push live state into each sandboxed iframe.
  function pumpCustom(s,view){
    document.querySelectorAll('iframe[data-ic-custom]').forEach(function(f){
      try{ f.contentWindow.postMessage({__ic:1,state:s,view:view},'*'); }catch(e){}
    });
  }

  // Per-state auto-hide: a widget can be told to disappear N ms after a
  // state begins (e.g. show "ON A DELAY" for 8s when going live, then get
  // out of the way so viewers see the gameplay). data-hide-<state>=ms.
  var curState='idle', hideTimers=[], lastHideKey='';
  function clearHideTimers(){ hideTimers.forEach(clearTimeout); hideTimers=[]; }
  function applyHides(stateKey){
    clearHideTimers();
    var goners=document.querySelectorAll('.icw.ic-gone, .icw[data-ic-gone]');
    for(var i=0;i<goners.length;i++){ var g=goners[i]; g.classList.remove('ic-gone'); g.removeAttribute('data-ic-gone'); g.style.transform=''; g.style.transition=''; g.style.animation=''; g.style.opacity=''; g.style.visibility=''; g.style.pointerEvents=''; }
    // __ic_noHide: editor preview keeps widgets up. !AUTOHIDE: the OBS URL
    // carried ?autohide=off, so the streamer wants the overlay to stay put.
    if(window.__ic_noHide || !AUTOHIDE) return;
    document.querySelectorAll('[data-ah-'+stateKey+']').forEach(function(el){
      var ms=parseInt(el.getAttribute('data-ah-'+stateKey),10)||0;
      if(ms>0) hideTimers.push(setTimeout(function(){
        var ex=el.getAttribute('data-exit-'+stateKey)||'fade';
        el.style.animation='none';
        if(ex==='instant'){ el.style.transition='none'; el.classList.add('ic-gone'); return; }
        // Run an exit KEYFRAME (see base CSS). The reflow
        // commits the animation:none so the exit animation starts cleanly from
        // the current pose; the forwards fill holds opacity:0 until re-show.
        var name = ex==='slide-down'?'ic-x-down':ex==='slide-up'?'ic-x-up':ex==='pop'?'ic-x-pop':'ic-x-fade';
        void el.offsetWidth;
        el.style.animation=name+' .8s ease forwards';
        el.setAttribute('data-ic-gone','1');
        // Stop painting + block clicks once it's faded out.
        hideTimers.push(setTimeout(function(){ el.style.visibility='hidden'; el.style.pointerEvents='none'; }, 850));
      }, ms));
    });
  }
  function render(s){
    var st=forced||deriveState(s);
    var view=buildView(s,st);
    window.__ic_last=s;
    if(body.getAttribute('data-state')!==st){ body.setAttribute('data-state',st); }
    applyText(st,view); // every tick so {{vars}} stay live
    var key=st+'|'+(window.__ic_noHide?'1':'0');
    if(key!==lastHideKey){ lastHideKey=key; applyHides(st); }
    curState=st;
    applyData(s);
    pumpCustom(view,view);
  }

  function buildView(s,st){
    var br=(s.stats&&s.stats.bitrate_kbps)||0;
    var tgt=s.armed_delay_ms||s.buffer_target_ms||1;
    var pct=s.phase==='preparing'?Math.round(Math.max(0,Math.min(1,(s.buffer_fill_ms||0)/tgt))*100):(s.phase==='ready'||s.phase==='active'?100:0);
    return {
      state:st, phase:s.phase,
      delay:+secs(s,'target_delay_ms').toFixed(1),
      armed:+(((s.armed_delay_ms||0)/1000).toFixed(1)),
      bufferPct:pct, bitrate:br,
      destAlive:s.destinations_alive||0, destTotal:s.destinations_total||0,
      cutCount:(s.stats&&s.stats.cuts)||0,
      ingest:!!s.ingest_alive
    };
  }

  // Sim override from the Studio canvas (inert in OBS). noHide keeps
  // auto-hide widgets visible while the user edits a state.
  window.addEventListener('message',function(ev){
    var d=ev.data; if(!d) return;
    if(d.__ic_sim){
      window.__ic_noHide=!!d.noHide; forced=d.forceState||null;
      // replay: drop the state attribute + reflow so one-shot entrance
      // animations restart even when re-selecting the same state.
      if(d.replay){ body.removeAttribute('data-state'); void body.offsetWidth; lastHideKey=''; }
      // rehide: re-arm every auto-hide timer from the current frame (after a
      // Studio edit) WITHOUT restarting entrance animations, so all hides stay
      // in lockstep instead of drifting until the next replay cycle.
      if(d.rehide){ lastHideKey=''; }
      render(d.state||window.__ic_last||{});
    }
  });

  // /overlay-events and /overlay-state, not /events and /state. A saved
  // overlay runs in an OBS browser source that cannot log in, and the
  // overlay feed is the read-only payload it is allowed to have; the
  // dashboard's needs a session, which is why an overlay pointed at it
  // froze the moment a dashboard password was set.
  function connect(){
    if(window.EventSource){
      try{
        var es=new EventSource('/overlay-events');
        es.onmessage=function(e){ try{ render(JSON.parse(e.data)); }catch(_){} };
        es.onerror=function(){ es.close(); setTimeout(poll,1000); };
        return;
      }catch(_){}
    }
    poll();
  }
  function poll(){ function t(){ fetch('/overlay-state').then(function(r){return r.json();}).then(render).catch(function(){}); } t(); setInterval(t,500); }
  // Seed a believable idle frame so a forced-state preview renders instantly.
  render(window.__ic_seed||{ingest_alive:false,phase:'idle',destinations_total:0,destinations_alive:0,stats:{cuts:0,bitrate_kbps:0}});
  connect();
})();
`;

  // The tiny script injected INTO each CustomHTML sandboxed iframe so the
  // author's snippet gets window.ic + {{var}} substitution for free.
  const CUSTOM_BRIDGE = String.raw`
<script>
(function(){
  var cbs=[];
  window.ic={state:{},delay:0,phase:'idle',bufferPct:0,bitrate:0,destAlive:0,destTotal:0,cutCount:0,
    onUpdate:function(f){cbs.push(f); if(window.ic.state) try{f(window.ic.state);}catch(e){}},
    bind:function(sel,key){ window.ic.onUpdate(function(s){ document.querySelectorAll(sel).forEach(function(n){ n.textContent=s[key]; }); }); }};
  function subst(s){
    document.querySelectorAll('[data-ic-tpl]').forEach(function(n){
      var tpl=n.getAttribute('data-ic-tpl');
      n.textContent=tpl.replace(/\{\{(\w+)\}\}/g,function(_,k){return (s[k]!=null)?s[k]:'';});
    });
  }
  // Wrap any text node containing {{ }} once, on load.
  window.addEventListener('DOMContentLoaded',function(){
    var walk=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT,null);
    var hits=[],n; while(n=walk.nextNode()){ if(/\{\{\w+\}\}/.test(n.nodeValue)) hits.push(n); }
    hits.forEach(function(t){ var sp=document.createElement('span'); sp.setAttribute('data-ic-tpl',t.nodeValue); t.parentNode.replaceChild(sp,t); });
  });
  window.addEventListener('message',function(ev){
    var d=ev.data; if(!d||!d.__ic) return; var v=d.view||d.state||{};
    window.ic.state=v; window.ic.delay=v.delay; window.ic.phase=v.phase; window.ic.bufferPct=v.bufferPct;
    window.ic.bitrate=v.bitrate; window.ic.destAlive=v.destAlive; window.ic.destTotal=v.destTotal; window.ic.cutCount=v.cutCount;
    subst(v); cbs.forEach(function(f){ try{f(v);}catch(e){} });
  });
})();
</script>`;

  // ---- bake(doc) -> self-contained static HTML string ----

  function bake(doc) {
    doc = normalize(doc);
    const usedAnim = {};
    let widgetCssAll = '';
    let bodyHtml = '';
    let customScripts = '';

    doc.widgets.forEach(function (w, idx) {
      if (!w.id) w.id = 'w' + idx;
      widgetCssAll += widgetCss(w);
      const sc = stateCss(w);
      widgetCssAll += sc.css;
      Object.keys(sc.anims).forEach(function (k) { usedAnim[k] = true; });
      if (w.base.anim && w.base.anim.style !== 'instant') usedAnim[w.base.anim.style] = true;

      // Position (skip for viewport-spanning kinds; their CSS fixes inset:0).
      const meta = KINDS.find(function (k) { return k.key === w.kind; }) || {};
      // Position only - colour/opacity come from the stylesheet so the
      // per-state `body[data-state] #id` rules can win. Inline styles beat
      // stylesheet rules, so putting colour here would freeze every state
      // to the base colour (the bug that made all states look identical).
      let style = meta.full ? '' : anchorCss(w.anchor, w.offset);

      // Per-widget effect tuning, emitted as inline CSS vars the widget CSS
      // reads (with fallbacks, so an untuned widget looks exactly as before).
      // --ic-spd-mul: animation speed; --ic-sm: data-driven transition
      // smoothness (seconds); --ic-wav: LiquidFill wave intensity.
      if (w.speed != null && +w.speed !== 1) style += '--ic-spd-mul:' + r(Math.max(0.1, +w.speed)) + ';';
      if (w.smooth != null && (w.kind === 'LiquidFill' || w.kind === 'BufferBar' || w.kind === 'BufferRing')) {
        style += '--ic-sm:' + r(Math.max(0, +w.smooth)) + 's;';
      }
      if (w.kind === 'LiquidFill' && w.wave != null && +w.wave !== 1) style += '--ic-wav:' + r(Math.max(0, +w.wave)) + ';';
      if (w.kind === 'BufferRing' && w.ringWidth != null) style += '--ic-ring-w:' + r(Math.max(1, +w.ringWidth)) + ';';
      if (w.kind === 'EdgeAccent') {
        if (w.edgeWidth != null) style += '--ic-edge-w:' + r(Math.max(0, +w.edgeWidth)) + 'px;';
        if (w.glow != null) style += '--ic-glow:' + r(Math.max(0, +w.glow)) + 'px;';
      }
      if (w.kind === 'BackgroundTint' && w.intensity != null) style += '--ic-tint:' + r(Math.max(0, Math.min(1, +w.intensity))) + ';';
      if (w.kind === 'CornerFrame') {
        if (w.frameSize != null) style += '--ic-cf-size:' + r(Math.max(8, +w.frameSize)) + 'px;';
        if (w.frameWidth != null) style += '--ic-cf-w:' + r(Math.max(1, +w.frameWidth)) + 'px;';
      }

      // Per-state text overrides surfaced as data attributes for the runtime.
      let dataAttrs = 'data-kind="' + w.kind + '"';
      // LiquidFill fixed level (vs the default buffer-following fill).
      if (w.kind === 'LiquidFill' && w.level != null && w.level !== 'buffer') {
        dataAttrs += ' data-level="' + r(Math.max(0, Math.min(100, +w.level))) + '"';
      }
      if (w.kind === 'DelayReadout') dataAttrs += ' data-bind="' + esc(w.bind || 'target_delay_ms') + '"';
      if (w.kind === 'BitrateMeter') {
        dataAttrs += ' data-bind="' + esc(w.bind || 'bitrate_kbps') + '"';
        if (w.smooth != null) dataAttrs += ' data-smooth="' + r(Math.max(0, Math.min(0.95, w.smooth))) + '"';
      }
      // CutCounter is intentionally excluded: its label is the live count
      // (driven by applyData via data-tpl), so per-state text overrides
      // would fight the counter. The rest take per-state text overrides.
      const hasText = ['StatePill', 'Text', 'Banner'].indexOf(w.kind) >= 0;
      if (hasText) {
        dataAttrs += ' data-has-txt data-txt-base="' + esc(w.base.text || '') + '"';
        STATES.forEach(function (st) {
          const sp = (w.states && w.states[st.key]) || {};
          if (sp.text != null) dataAttrs += ' data-txt-' + st.key + '="' + esc(sp.text) + '"';
        });
      }
      // Per-state auto-hide-after-ms (+ exit style): show, then leave.
      STATES.forEach(function (st) {
        const sp = (w.states && w.states[st.key]) || {};
        if (sp.hide_after_ms && sp.hide_after_ms > 0) {
          dataAttrs += ' data-ah-' + st.key + '="' + Math.round(sp.hide_after_ms) + '"';
          if (sp.exit) dataAttrs += ' data-exit-' + st.key + '="' + esc(sp.exit) + '"';
        }
      });

      if (w.kind === 'CustomHTML') {
        const frag = bakeCustom(w);
        bodyHtml += '<div class="icw" id="' + esc(cssId(w.id)) + '" style="' + esc2(style) + '" ' + dataAttrs + '>' +
          '<iframe data-ic-custom sandbox="allow-scripts" srcdoc="' + esc(frag) + '"></iframe></div>';
      } else {
        let inner = widgetInner(w);
        if (w.kind === 'CutCounter') {
          const tplv = (w.base.text && w.base.text.indexOf('{n}') >= 0) ? w.base.text : '{n}';
          inner = '<span class="lbl" data-tpl="' + esc(tplv) + '">0</span>';
        }
        bodyHtml += '<div class="icw" id="' + esc(cssId(w.id)) + '" style="' + esc2(style) + '" ' + dataAttrs + '>' + inner + '</div>';
      }
    });

    let animCss = '';
    Object.keys(usedAnim).forEach(function (k) { if (KEYFRAMES[k]) animCss += KEYFRAMES[k] + '\n'; });

    const css = themeCss(doc.theme) + '\n' + BASE_CSS + '\n' + animCss + widgetCssAll;
    // The editable source doc rides along inside an HTML comment, URL-
    // encoded so its payload can never contain "-->". OBS ignores it; the
    // Studio extracts it on load via readDoc() to re-edit. One file does
    // double duty: lean live overlay + its own editable source.
    const docComment = '<!--ic-doc:' + encodeURIComponent(JSON.stringify(doc)) + '-->';
    return '<!doctype html>\n' + docComment +
      '\n<!-- Baked by InstantClone Overlay Studio. Edit in the dashboard, not here. -->\n' +
      '<html><head><meta charset="utf-8"><title>' + esc(doc.name) + '</title>\n' +
      '<style>\n' + css + '\n</style></head>\n<body data-state="idle">\n' + bodyHtml + '\n' +
      '<script>' + RUNTIME_JS + '</' + 'script>\n</body></html>\n';
  }

  // Pull the editable doc back out of a baked overlay's HTML. Returns null
  // for legacy / hand-written overlays that carry no ic-doc comment.
  function readDoc(html) {
    const m = String(html).match(/<!--ic-doc:([^]*?)-->/);
    if (!m) return null;
    try { return JSON.parse(decodeURIComponent(m[1])); } catch (e) { return null; }
  }

  // attribute-value escaping for inline style attrs
  function esc2(s) { return String(s == null ? '' : s).split('"').join('&quot;'); }

  // CustomHTML iframe document: author CSS + HTML + bridge + author JS.
  function bakeCustom(w) {
    const c = w.custom || defaultCustom();
    return '<!doctype html><html><head><meta charset="utf-8"><style>' +
      'html,body{margin:0;background:transparent;overflow:hidden}\n' + (c.css || '') +
      '</style></head><body>' + (c.html || '') + CUSTOM_BRIDGE +
      '<script>try{\n' + (c.js || '') + '\n}catch(e){console.error(e)}</' + 'script>' +
      '</body></html>';
  }

  function normalize(doc) {
    const d = JSON.parse(JSON.stringify(doc || {}));
    if (!d.name) d.name = 'Untitled overlay';
    if (!d.canvas) d.canvas = { w: 1920, h: 1080, safe_area: true };
    d.theme = Object.assign(defaultTheme(), d.theme || {});
    if (!Array.isArray(d.widgets)) d.widgets = [];
    d.widgets.forEach(function (w) {
      if (!w.base) w.base = { color: '#5ac8fa', opacity: 1, text: null, anim: { style: 'fade', duration_ms: 400, easing: 'ease-out', delay_ms: 0 } };
      if (!w.states) w.states = {};
      if (!w.offset) w.offset = { x: 32, y: 32 };
      if (!w.size) w.size = { w: 200, h: 80 };
    });
    return d;
  }

  // ---- Built-in presets (client-side; fork-on-edit) ----
  // One per audience archetype. Every preset is subtle by default,
  // animated, single-idea, and fully editable once forked into the Studio.

  function W(kind, o) {
    o = o || {};
    const w = defaultWidget(kind, o.id || kind.toLowerCase());
    if (o.anchor) w.anchor = o.anchor;
    if (o.offset) w.offset = o.offset;
    if (o.size) w.size = o.size;
    if (o.font_px) w.font_px = o.font_px;
    if (o.bind) w.bind = o.bind;
    if (o.unit != null) w.unit = o.unit;
    if (o.icon) w.icon = o.icon;
    if (o.shape) w.shape = o.shape;
    if (o.color) w.base.color = o.color;
    if (o.opacity != null) w.base.opacity = o.opacity;
    if (o.text != null) w.base.text = o.text;
    if (o.anim) w.base.anim = o.anim;
    if (o.states) w.states = o.states;
    if (o.custom) w.custom = o.custom;
    return w;
  }
  const A = function (style, duration_ms, easing) {
    return { style: style, duration_ms: duration_ms, easing: easing || 'ease-out', delay_ms: 0 };
  };
  // One colour + motion per delay-state so every state reads at a glance:
  // grey idle, AMBER filling, CYAN armed, GREEN live, RED error.
  const GREY = '#8a8f9a', AMBER = '#ffb73a', CYAN = '#5ac8fa', GREEN = '#54e38e',
        RED = '#ff5a5a';

  // Build a 6-state map from a compact spec. idle hides by default; pass
  // `idle` to override (e.g. the technical panel stays visible at idle).
  // Each listed state is forced visible unless it says otherwise.
  function S6(o) {
    const m = { idle: o.idle || { visible: false } };
    ['preparing', 'ready', 'active', 'error'].forEach(function (k) {
      if (o[k]) m[k] = Object.assign({ visible: true }, o[k]);
    });
    // Every shipped preset clears itself 4s after going live (fades out), so the
    // overlay gets out of the way once viewers have registered the delay.
    if (m.active) { m.active.hide_after_ms = 4000; m.active.exit = 'fade'; }
    return m;
  }

  function preset(name, tag, hint, widgets) {
    return { name: name, tag: tag, hint: hint, version: 1, canvas: { w: 1920, h: 1080, safe_area: true }, widgets: widgets };
  }

  const PRESETS = [
    preset('Whisper', 'Minimal', 'A tiny top-left delay number: amber filling, cyan armed, green live.', [
      W('DelayReadout', {
        anchor: 'top-left', offset: { x: 96, y: 62 }, font_px: 40, bind: 'target_delay_ms', color: CYAN,
        states: S6({
          preparing: { color: AMBER, anim: A('slide-down', 420) },
          ready: { color: CYAN, anim: A('fade', 300) },
          active: { color: GREEN, anim: A('breathe', 3000) },
          error: { color: RED, anim: A('shake', 420) }
        })
      })
    ]),

    preset('Corner Glass', 'Minimal', 'Bottom-right ring that fills amber, locks cyan, breathes green live.', [
      W('BufferRing', {
        anchor: 'bottom-right', offset: { x: 96, y: 62 }, size: { w: 132, h: 132 }, font_px: 36, color: CYAN,
        states: S6({
          preparing: { color: AMBER },
          ready: { color: CYAN },
          active: { color: GREEN, anim: A('breathe', 3000) },
          error: { color: RED, anim: A('pulse', 700) }
        })
      })
    ]),

    preset('Pulse Dot', 'Casual', 'A live dot + label that pulses faster while arming, then clears once live.', [
      W('PulsingDot', {
        anchor: 'top-right', offset: { x: 252, y: 72 }, size: { w: 18, h: 18 }, color: CYAN,
        states: S6({
          preparing: { color: AMBER, anim: A('pulse', 700) },
          ready: { color: CYAN, anim: A('pulse', 1400) },
          active: { color: GREEN, anim: A('pulse', 2200) },
          error: { color: RED, anim: A('pulse', 500) }
        })
      }),
      W('StatePill', {
        anchor: 'top-right', offset: { x: 96, y: 62 }, font_px: 30, text: 'DELAY', color: CYAN,
        states: S6({
          preparing: { text: 'BUFFERING', color: AMBER, anim: A('slide-left', 360) },
          ready: { text: 'ARMED', color: CYAN },
          active: { text: 'ON DELAY', color: GREEN },
          error: { text: 'DROPPED', color: RED, anim: A('shake', 400) }
        })
      })
    ]),

    preset('Heartbeat', 'Ambient', 'A beating pulse with radar ripples - amber arming, green live.', [
      W('Heartbeat', {
        anchor: 'bottom-left', offset: { x: 110, y: 96 }, size: { w: 120, h: 120 }, color: CYAN,
        states: S6({
          preparing: { color: AMBER },
          ready: { color: CYAN },
          active: { color: GREEN },
          error: { color: RED }
        })
      })
    ]),

    preset('Edge', 'Ambient', 'A glowing screen border that breathes while arming and pulses with light when live.', [
      W('EdgeAccent', {
        color: CYAN, opacity: 0.75,
        states: S6({
          preparing: { color: AMBER, anim: A('glow', 1600) },
          ready: { color: CYAN, anim: A('glow', 3200) },
          active: { color: GREEN, anim: A('glow', 2600) },
          error: { color: RED, anim: A('pulse', 700) }
        })
      }),
      W('StatePill', {
        anchor: 'bottom', offset: { x: -74, y: 64 }, font_px: 26, text: 'DELAY ON', color: CYAN,
        states: S6({
          preparing: { text: 'ARMING DELAY', color: AMBER },
          ready: { text: 'DELAY ARMED', color: CYAN },
          active: { text: 'DELAY ON', color: GREEN },
          error: { text: 'DESTINATION DROPPED', color: RED, anim: A('shake', 400) }
        })
      })
    ]),

    preset('Tide', 'Ambient', 'A liquid that rises from the bottom as the delay arms, full and rippling once live.', [
      W('LiquidFill', {
        color: CYAN,
        states: S6({
          preparing: { color: AMBER },
          ready: { color: CYAN },
          active: { color: GREEN },
          error: { color: RED }
        })
      }),
      W('StatePill', {
        anchor: 'bottom', offset: { x: 0, y: 70 }, font_px: 26, text: 'ON A DELAY', color: CYAN,
        states: S6({
          preparing: { text: 'ARMING DELAY', color: AMBER },
          ready: { text: 'DELAY ARMED', color: CYAN },
          active: { text: 'ON A DELAY', color: GREEN },
          error: { text: 'SIGNAL LOST', color: RED, anim: A('shake', 400) }
        })
      })
    ]),

    preset('Frame', 'Tournament', 'Corner brackets, an ARMING/ARMED/ON AIR chip, and a live readout.', [
      W('CornerFrame', {
        color: CYAN,
        states: S6({
          preparing: { color: AMBER }, ready: { color: CYAN }, active: { color: GREEN },
          error: { color: RED, anim: A('pulse', 700) }
        })
      }),
      W('StatePill', {
        anchor: 'top', offset: { x: 0, y: 64 }, font_px: 34, text: 'ON AIR', color: CYAN,
        states: S6({
          preparing: { text: 'ARMING', color: AMBER, anim: A('slide-down', 400) },
          ready: { text: 'ARMED', color: CYAN },
          active: { text: 'ON AIR', color: GREEN },
          error: { text: 'SIGNAL LOST', color: RED, anim: A('flash', 300, 'linear') }
        })
      }),
      W('DelayReadout', {
        anchor: 'top-right', offset: { x: 96, y: 62 }, font_px: 30, bind: 'target_delay_ms', color: CYAN,
        states: S6({
          preparing: { color: AMBER }, ready: { color: CYAN }, active: { color: GREEN },
          error: { color: RED }
        })
      })
    ]),

    preset('Badge', 'Just Chatting', 'A corner badge with a clear word per state for your chat.', [
      W('StatePill', {
        anchor: 'bottom-left', offset: { x: 96, y: 62 }, font_px: 30, text: 'DELAY', color: CYAN,
        states: S6({
          preparing: { text: 'DELAY LOADING', color: AMBER, anim: A('slide-right', 380) },
          ready: { text: 'DELAY READY', color: CYAN },
          active: { text: 'ON A DELAY', color: GREEN },
          error: { text: 'STREAM ISSUE', color: RED, anim: A('shake', 400) }
        })
      })
    ]),

    preset('Status Panel', 'Technical', 'Delay, buffer, bitrate and destinations - greyed at idle, lit by state.', [
      W('DelayReadout', { anchor: 'top-left', offset: { x: 96, y: 62 }, font_px: 34, bind: 'target_delay_ms', color: CYAN,
        states: S6({ idle: { visible: true, color: GREY }, preparing: { color: AMBER }, ready: { color: CYAN }, active: { color: GREEN }, error: { color: RED } }) }),
      W('BufferBar', { anchor: 'top-left', offset: { x: 96, y: 112 }, size: { w: 180, h: 8 }, color: CYAN,
        states: S6({ idle: { visible: true, color: GREY }, preparing: { color: AMBER }, ready: { color: CYAN }, active: { color: GREEN }, error: { color: RED } }) }),
      W('BitrateMeter', { anchor: 'top-left', offset: { x: 96, y: 134 }, size: { w: 180, h: 32 }, color: CYAN,
        states: S6({ idle: { visible: true, color: GREY }, preparing: { color: CYAN }, ready: { color: CYAN }, active: { color: GREEN }, error: { color: RED } }) }),
      W('DestinationStatus', { anchor: 'top-left', offset: { x: 96, y: 178 }, color: CYAN,
        states: S6({ idle: { visible: true }, preparing: { visible: true }, ready: { visible: true }, active: { visible: true }, error: { visible: true } }) })
    ]),

    preset('Custom Start', 'Power', 'An empty canvas with one CustomHTML widget wired to live data.', [
      W('CustomHTML', {
        anchor: 'center', offset: { x: 0, y: 0 }, size: { w: 360, h: 140 },
        custom: defaultCustom(),
        states: S6({ idle: { visible: true }, preparing: { visible: true }, ready: { visible: true }, active: { visible: true }, error: { visible: true } })
      })
    ])
  ];

  // Transform-based animations look broken on full-screen layers (the
  // whole viewport slides/shakes), so those widgets only offer opacity
  // effects. This is the fix for "some effects don't work".
  const FULLSCREEN_KINDS = ['EdgeAccent', 'BackgroundTint', 'LiquidFill', 'CornerFrame'];
  const FULLSCREEN_ANIMS = ['instant', 'fade', 'breathe', 'glow', 'flash'];
  function animsFor(kind) {
    if (FULLSCREEN_KINDS.indexOf(kind) >= 0) return ANIMS.filter(function (a) { return FULLSCREEN_ANIMS.indexOf(a.key) >= 0; });
    return ANIMS;
  }

  // ---- public API ----
  root.ICOverlay = {
    PRESETS: PRESETS,
    bake: bake, readDoc: readDoc,
    STATES: STATES, ANCHORS: ANCHORS, ANIMS: ANIMS, EASINGS: EASINGS, animsFor: animsFor,
    KINDS: KINDS, BINDINGS: BINDINGS, ICONS: ICONS, SHAPES: SHAPES,
    defaultDoc: defaultDoc, defaultWidget: defaultWidget, defaultCustom: defaultCustom,
    defaultTheme: defaultTheme, iconSvg: iconSvg
  };

})(typeof window !== 'undefined' ? window : this);
