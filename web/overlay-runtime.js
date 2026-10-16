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
(function icOverlayModule(root) {

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
    { key: 'glitch',      label: 'Glitch',      loop: false },
    // The alerts themes' motions, for any layer: drawn on from the left,
    // opened like a shutter, grown from a dot, stamped down, boinged out.
    { key: 'wipe',        label: 'Wipe on',     loop: false },
    { key: 'shutter',     label: 'Shutter',     loop: false },
    { key: 'grow',        label: 'Grow from a dot', loop: false },
    { key: 'stamp',       label: 'Stamp',       loop: false },
    { key: 'boing',       label: 'Boing',       loop: false },
    // Words only: scrambled letters that settle, or typed one at a time.
    { key: 'decode',      label: 'Decode',      loop: false, words: true },
    { key: 'typewriter',  label: 'Typewriter',  loop: false, words: true }
  ];
  // The layers with words those two can change.
  const WORD_KINDS = ['StatePill', 'Text', 'Banner'];

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
    // Goals and timers (the app's goals.rs): which one a widget shows is its
    // `goal` / `timer`, blank for the first. A goal bar resizes by its box,
    // a timer by its font, like the other text widgets.
    { key: 'Goal',              label: 'Goal bar',        group: 'stream', fields: ['goal'] },
    { key: 'Timer',             label: 'Timer',           group: 'stream', fields: ['timer', 'font'] },
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
    { key: 'AlertCard',         label: 'Alert card',      group: 'alert', fields: ['card'] },
    { key: 'CustomHTML',        label: 'Custom HTML',     group: 'power', fields: ['custom'], cost: 'opt' }
  ];

  const ICONS = ['bolt', 'eye', 'star', 'clock', 'shield', 'dot', 'live', 'wifi', 'alert', 'play'];

  // Fonts a text widget can use, from Google Fonts (as the alerts page offers
  // them), each with the weights it has: a one-weight font asks for none.
  const FONTS = {
    'Inter': 'wght@400;500;600;700;800', 'Poppins': 'wght@400;600;700;800', 'Montserrat': 'wght@400;600;700;800',
    'Fredoka': 'wght@400;600;700', 'Rubik': 'wght@400;600;700;800', 'Bebas Neue': '', 'Oswald': 'wght@400;600;700',
    'Bangers': '', 'Orbitron': 'wght@400;600;700;800', 'Press Start 2P': '', 'Permanent Marker': '',
    'Roboto Mono': 'wght@400;600;700', 'Chakra Petch': 'wght@400;500;600;700', 'Playfair Display': 'wght@400;700',
    'Special Elite': ''
  };
  // The PC's own font (Segoe UI on Windows), which loads from nowhere.
  const SYSTEM_FONT = 'system';
  function fontLink(families) {
    if (!families.length) return '';
    const q = families.map(function (name) {
      return 'family=' + name.split(' ').join('+') + (FONTS[name] ? ':' + FONTS[name] : '');
    }).join('&');
    return '<link rel="stylesheet" href="https://fonts.googleapis.com/css2?' + q + '&display=swap">\n';
  }
  // A colour as the page can use it: a theme token (@live) is the theme's.
  function themed(c, theme) {
    if (typeof c !== 'string' || c.charAt(0) !== '@') return c;
    return Object.assign(defaultTheme(), theme || {})[c.slice(1)] || c;
  }
  // `#rrggbb` at `alpha`, for backgrounds that let the scene show through.
  function rgba(hex, alpha) {
    const m = /^#([0-9a-f]{6})$/i.exec(hex || '');
    if (!m) return hex || 'transparent';
    const n = parseInt(m[1], 16);
    return 'rgba(' + (n >> 16) + ',' + ((n >> 8) & 255) + ',' + (n & 255) + ',' + r(Math.max(0, Math.min(1, alpha))) + ')';
  }
  const SHAPES = ['rect', 'card', 'circle', 'pill', 'line'];

  // ---- Defaults ----

  function defaultWidget(kind, id) {
    const k = KINDS.find(function (x) { return x.key === kind; }) || KINDS[0];
    const SIZES = {
      Icon: { w: 48, h: 48 }, PulsingDot: { w: 16, h: 16 }, BufferRing: { w: 120, h: 120 },
      Heartbeat: { w: 110, h: 110 }, BitrateMeter: { w: 180, h: 36 }, BufferBar: { w: 180, h: 10 },
      Shape: { w: 120, h: 60 }, Divider: { w: 160, h: 2 }, Image: { w: 200, h: 200 }, CustomHTML: { w: 320, h: 120 },
      AlertCard: { w: 1120, h: 360 },
      Goal: { w: 460, h: 14 }
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
    // A goal or a timer is about the stream, not the delay: it shows in
    // every state, and doesn't fade in again each time the delay changes.
    if (kind === 'Goal' || kind === 'Timer') {
      w.states = {};
      w.base.anim = { style: 'instant', duration_ms: 400, easing: 'ease-out', delay_ms: 0 };
      w.goal = ''; w.timer = ''; w.always = true;
    }
    if (kind === 'Timer') { w.font_px = 72; w.end = ''; w.base.color = '@ink'; }
    // An alert card comes in and goes its theme's way, by itself: the layer
    // itself just stays (see ALERT_THEMES).
    if (kind === 'AlertCard') {
      w.card = { theme: 'island' }; w.always = true; w.states = {};
      w.base.anim = { style: 'instant', duration_ms: 400, easing: 'ease-out', delay_ms: 0 };
    }
    // A goal bar fills from the accent into green, with a light passing over.
    if (kind === 'Goal') Object.assign(w.base, { color: '@accent', grad: '@live', grad_anim: 'shine', grad_speed: 4.5, grad_angle: 90 });
    return w;
  }

  // How a part of a goal or timer overlay arrives when the overlay opens (a
  // scene switch in OBS): one after another, `n` steps in. It plays once:
  // the same in every state, so the delay changing never replays it.
  // (At the shared strength: a tuned one bakes keyframes per state, and a
  // new name in each state would replay the entrance at every change.)
  function arrive(n) { return { style: 'slide-up', duration_ms: 560, easing: 'spring', delay_ms: n * 90 }; }
  // The card behind a goal or a timer: dark glass the scene shows through.
  function backCard(id, anchor, offset, size) {
    const w = defaultWidget('Shape', id);
    w.states = {}; w.always = true; w.shape = 'card';
    w.anchor = anchor; w.offset = offset; w.size = size;
    w.base.color = '#0a0c10'; w.base.opacity = 0.62; w.base.anim = arrive(0);
    return w;
  }
  // A layer of words on stream at all times: a goal's name or its numbers.
  function streamText(id, text, o) {
    const w = defaultWidget('Text', id);
    w.states = {}; w.always = true;
    w.base.anim = arrive(o.step || 1);
    w.base.text = text; w.anchor = o.anchor; w.offset = o.offset;
    w.font_px = o.font_px; w.font_weight = '700'; w.base.color = o.color;
    if (o.uppercase) { w.uppercase = true; w.letter_spacing = 3; }
    return w;
  }

  // An overlay for one goal or one timer, ready to save: what the dashboard
  // makes when it is first put on stream. Each part is its own layer, to
  // move, restyle or delete: a goal's bar, its name and its numbers; a
  // timer's name and its clock.
  function trackerDoc(kind, id, name) {
    const d = defaultDoc(name);
    d.purpose = 'goals';
    if (kind === 'Goal') {
      // Bottom centre. Centre anchors start at the middle, so each part is
      // half its width back; the numbers end where the bar does (960 - 310
      // from the right edge).
      const bar = defaultWidget('Goal', 'w4');
      bar.goal = id; bar.anchor = 'bottom'; bar.offset = { x: -310, y: 72 }; bar.size = { w: 620, h: 14 };
      bar.base.anim = arrive(3);
      d.widgets.push(backCard('w1', 'bottom', { x: -340, y: 48 }, { w: 680, h: 96 }),
        streamText('w2', '{{goal.' + id + '.name}}', { anchor: 'bottom', offset: { x: -310, y: 98 }, font_px: 26, color: '@ink', step: 1 }),
        streamText('w3', '{{goal.' + id + '}} / {{goal.' + id + '.target}}', { anchor: 'bottom-right', offset: { x: 650, y: 98 }, font_px: 26, color: '@accent', step: 2 }),
        bar);
    } else {
      // Top right, the name over the clock.
      const clock = defaultWidget('Timer', 'w3');
      clock.timer = id; clock.anchor = 'top-right'; clock.offset = { x: 64, y: 82 }; clock.base.anim = arrive(2);
      d.widgets.push(backCard('w1', 'top-right', { x: 40, y: 36 }, { w: 340, h: 140 }),
        streamText('w2', '{{timer.' + id + '.name}}', { anchor: 'top-right', offset: { x: 66, y: 56 }, font_px: 20, color: '@ink', uppercase: true, step: 1 }),
        clock);
    }
    return d;
  }

  // The alerts page's themes, each a built-in alert design: its card, as a
  // layer, at the bottom of the screen, where the alerts page put it. Each
  // keeps its theme's own motion; everything about it can be changed, and
  // anything can go around it.
  const ALERT_THEMES = ['island', 'signal', 'banner', 'bubble', 'minimal', 'paper', 'cinema'];
  // One theme's card as a single Alert card layer: the alerts page's own
  // card, with its look (what a look from before designs becomes).
  function alertCardDoc(theme) {
    const d = defaultDoc(theme.charAt(0).toUpperCase() + theme.slice(1));
    d.purpose = 'alerts';
    const w = defaultWidget('AlertCard', 'w1');
    w.card = { theme: theme }; w.anchor = 'bottom'; w.offset = { x: -560, y: 36 };
    d.widgets.push(w);
    return d;
  }

  // A ready-made alert design's layer, at the bottom of the screen (x from
  // the middle to its left edge, y up from the bottom): it waits hidden,
  // and comes in its own way (`anim`, `ms`, after `delay`) as an alert plays.
  function alertLayer(kind, id, o) {
    const w = defaultWidget(kind, id);
    w.always = true; w.states = { idle: { visible: false } };
    w.anchor = 'bottom'; w.offset = { x: o.x, y: o.y };
    if (o.w) w.size = { w: o.w, h: o.h || w.size.h };
    w.base.color = o.color || '@accent';
    if (o.op != null) w.base.opacity = o.op;
    w.base.anim = { style: o.anim || 'fade', duration_ms: o.ms || 450, easing: o.ease || 'spring', delay_ms: o.delay || 0 };
    ['name', 'radius', 'border', 'border_color', 'shadow', 'shape', 'nopic', 'with_message'].forEach(function (k) { if (o[k] != null) w[k] = o[k]; });
    return w;
  }
  // The alert's picture, filling its box.
  function alertPicture(id, o) {
    const w = alertLayer('Image', id, o);
    w.src = '{{alert.image}}'; w.cover = true;
    return w;
  }
  // What the alert says (`text`), kept inside its width: long words shrink.
  function alertWords(id, text, o) {
    const w = alertLayer('Text', id, Object.assign({ h: Math.round(o.px * 1.15) }, o));
    w.base.text = text; w.fit = true; w.text_align = o.align || 'left';
    w.font_px = o.px; w.font_family = o.font; w.font_weight = String(o.weight || 700);
    if (o.caps) { w.uppercase = true; w.letter_spacing = o.ls != null ? o.ls : 2; }
    else if (o.ls != null) w.letter_spacing = o.ls;
    return w;
  }
  // The viewer's own words (a cheer's or a resub's), over the card: up to
  // three lines in its width, and only there when the alert has some.
  function alertMessage(id, o) {
    const w = alertWords(id, '{{alert.message}}', Object.assign({ name: 'Message', px: 22, weight: 500, color: '#ffffff', align: 'center', with_message: true, delay: 650 }, o));
    delete w.fit; w.wrap = true;
    return w;
  }
  // Each ready-made design: its sound, its colours (accent, words) and its
  // layers.
  const ALERT_LOOKS = {
    island: { sound: 'chime', accent: '#5ac8fa', ink: '#ffffff', layers: function () { return [
      alertLayer('Shape', 'w1', { name: 'Card', x: -300, y: 64, w: 600, h: 96, color: '#000000', op: 0.96, radius: 48, border: 1, border_color: '#2a2d33', shadow: true, anim: 'grow', ms: 800 }),
      alertPicture('w2', { x: -286, y: 78, w: 68, h: 68, radius: 34, anim: 'scale-in', ms: 420, delay: 320 }),
      alertWords('w3', '{{alert.title}}', { nopic: 84, x: -202, y: 118, w: 430, px: 17, font: 'Inter', weight: 500, color: '#9aa0a8', delay: 360 }),
      alertWords('w4', '{{alert.text}}', { nopic: 84, x: -202, y: 80, w: 430, px: 30, font: 'Inter', color: '@ink', delay: 420 }),
      alertLayer('PulsingDot', 'w5', { name: 'Live dot', x: 256, y: 105, w: 14, h: 14, delay: 520 }),
      alertMessage('w6', { x: -300, y: 174, w: 600, font: 'Inter' })
    ]; } },
    signal: { sound: 'zap', accent: '#3ee0c5', ink: '#eef4ff', layers: function () { return [
      alertLayer('Shape', 'w1', { name: 'Panel', x: -320, y: 60, w: 640, h: 116, color: '#0b0f14', op: 0.88, radius: 0, border: 2, border_color: '@accent', anim: 'wipe', ms: 450, ease: 'sharp' }),
      alertLayer('Shape', 'w2', { name: 'Edge', x: -320, y: 60, w: 8, h: 116, radius: 0, anim: 'shutter', ms: 300, delay: 250 }),
      alertPicture('w3', { x: -296, y: 76, w: 84, h: 84, radius: 0, border: 2, border_color: '@accent', delay: 350 }),
      alertWords('w4', '{{alert.title}}', { nopic: 100, x: -196, y: 128, w: 470, px: 15, font: 'Roboto Mono', weight: 600, color: '@accent', caps: true, ls: 4, delay: 300 }),
      alertWords('w5', '{{alert.text}}', { nopic: 100, x: -196, y: 78, w: 470, px: 38, font: 'Chakra Petch', color: '@ink', anim: 'decode', ms: 700, delay: 380 }),
      alertMessage('w6', { x: -320, y: 190, w: 640, font: 'Chakra Petch', align: 'left' })
    ]; } },
    banner: { sound: 'fanfare', accent: '#fcd34d', ink: '#0b0d12', layers: function () { return [
      alertLayer('Shape', 'w1', { name: 'Band', x: -350, y: 60, w: 700, h: 112, radius: 0, shadow: true, anim: 'wipe', ms: 500, ease: 'sharp' }),
      alertLayer('Shape', 'w2', { name: 'Title tag', nopic: 104, x: -232, y: 126, w: 250, h: 34, color: '@ink', radius: 0, anim: 'wipe', ms: 350, ease: 'sharp', delay: 250 }),
      alertPicture('w3', { x: -336, y: 72, w: 88, h: 88, radius: 0, border: 3, border_color: '@ink', delay: 300 }),
      alertWords('w4', '{{alert.title}}', { nopic: 104, x: -222, y: 129, w: 230, px: 24, font: 'Bebas Neue', weight: 400, color: '@accent', ls: 3, shadow: false, delay: 380 }),
      alertWords('w5', '{{alert.text}}', { nopic: 104, x: -232, y: 64, w: 560, px: 58, font: 'Bebas Neue', weight: 400, color: '@ink', ls: 1, shadow: false, anim: 'slide-right', ms: 420, delay: 300 }),
      alertMessage('w6', { x: -350, y: 186, w: 700, font: 'Inter', align: 'left' })
    ]; } },
    bubble: { sound: 'pop', accent: '#a78bfa', ink: '#1b1d24', layers: function () { return [
      alertLayer('Shape', 'w1', { name: 'Bubble', x: -300, y: 70, w: 600, h: 118, color: '#ffffff', radius: 46, shadow: true, anim: 'boing', ms: 700 }),
      alertLayer('Shape', 'w2', { name: 'Tail', x: -262, y: 50, w: 26, h: 26, color: '#ffffff', shape: 'circle', anim: 'boing', ms: 500, delay: 120 }),
      alertLayer('Shape', 'w3', { name: 'Small tail', x: -284, y: 34, w: 14, h: 14, color: '#ffffff', shape: 'circle', anim: 'boing', ms: 500, delay: 180 }),
      alertPicture('w4', { x: -284, y: 86, w: 86, h: 86, radius: 43, border: 4, border_color: '@accent', anim: 'scale-in', ms: 400, delay: 250 }),
      alertWords('w5', '{{alert.title}}', { nopic: 102, x: -182, y: 136, w: 440, px: 18, font: 'Fredoka', weight: 600, color: '@accent', caps: true, ls: 1, shadow: false, delay: 300 }),
      alertWords('w6', '{{alert.text}}', { nopic: 102, x: -182, y: 86, w: 440, px: 36, font: 'Fredoka', color: '@ink', shadow: false, anim: 'slide-up', delay: 340 }),
      alertMessage('w7', { x: -300, y: 202, w: 600, font: 'Fredoka' })
    ]; } },
    minimal: { sound: 'blip', accent: '#5ac8fa', ink: '#ffffff', layers: function () { return [
      alertLayer('Shape', 'w1', { name: 'Line', x: -300, y: 70, w: 5, h: 104, radius: 3, anim: 'shutter', ms: 400 }),
      alertPicture('w2', { x: -282, y: 76, w: 92, h: 92, radius: 14, delay: 200 }),
      alertWords('w3', '{{alert.title}}', { nopic: 108, x: -174, y: 140, w: 500, px: 16, font: 'Inter', weight: 700, color: '@accent', caps: true, ls: 4, anim: 'slide-right', delay: 150 }),
      alertWords('w4', '{{alert.text}}', { nopic: 108, x: -174, y: 78, w: 500, px: 48, font: 'Inter', weight: 800, color: '@ink', anim: 'slide-right', ms: 500, delay: 230 }),
      alertMessage('w5', { x: -300, y: 188, w: 600, font: 'Inter', align: 'left' })
    ]; } },
    paper: { sound: 'whoosh', accent: '#e2553d', ink: '#1d1a16', layers: function () { return [
      alertLayer('Shape', 'w1', { name: 'Paper', x: -310, y: 62, w: 620, h: 128, color: '#f4efe4', radius: 3, shadow: true, anim: 'stamp', ms: 450 }),
      alertLayer('Shape', 'w2', { name: 'Tape', x: -65, y: 176, w: 130, h: 30, op: 0.85, radius: 2, anim: 'stamp', ms: 350, delay: 250 }),
      alertPicture('w3', { x: -292, y: 80, w: 92, h: 92, radius: 2, border: 5, border_color: '#ffffff', shadow: true, anim: 'stamp', ms: 400, delay: 150 }),
      alertWords('w4', '{{alert.title}}', { nopic: 112, x: -180, y: 138, w: 440, px: 18, font: 'Special Elite', weight: 400, color: '@accent', caps: true, ls: 2, shadow: false, anim: 'typewriter', ms: 500, delay: 300 }),
      alertWords('w5', '{{alert.text}}', { nopic: 112, x: -180, y: 82, w: 440, px: 40, font: 'Permanent Marker', weight: 400, color: '@ink', shadow: false, anim: 'stamp', ms: 400, delay: 380 }),
      alertMessage('w6', { x: -310, y: 218, w: 620, font: 'Special Elite' })
    ]; } },
    cinema: { sound: 'bell', accent: '#d4b06a', ink: '#f4ede1', layers: function () { return [
      alertLayer('Shape', 'w1', { name: 'Letterbox', x: -380, y: 50, w: 760, h: 150, color: '#0a0908', op: 0.9, radius: 2, anim: 'shutter', ms: 700, ease: 'ease-in-out' }),
      alertLayer('Divider', 'w2', { name: 'Top line', x: -280, y: 180, w: 560, h: 2, ms: 600, delay: 450 }),
      alertLayer('Divider', 'w3', { name: 'Bottom line', x: -280, y: 66, w: 560, h: 2, ms: 600, delay: 450 }),
      alertPicture('w4', { x: -50, y: 214, w: 100, h: 100, radius: 50, border: 2, border_color: '@accent', ms: 600, delay: 300 }),
      alertWords('w5', '{{alert.title}}', { x: -300, y: 146, w: 600, px: 16, font: 'Playfair Display', color: '@accent', caps: true, ls: 6, align: 'center', ms: 600, delay: 500 }),
      alertWords('w6', '{{alert.text}}', { x: -310, y: 86, w: 620, px: 44, font: 'Playfair Display', color: '@ink', align: 'center', ms: 800, delay: 650 }),
      alertMessage('w7', { x: -380, y: 326, w: 760, font: 'Playfair Display', delay: 900 })
    ]; } }
  };
  // A ready-made alert design: the theme built from layers (its shapes, the
  // picture, the title, the words), each to move, restyle or delete. The
  // theme's accent and words colours are the design's own (Theme tab).
  function alertPreset(theme) {
    const look = ALERT_LOOKS[theme] || ALERT_LOOKS.island;
    const d = defaultDoc(theme.charAt(0).toUpperCase() + theme.slice(1));
    d.purpose = 'alerts';
    d.theme.accent = look.accent; d.theme.ink = look.ink;
    d.sound = look.sound;
    d.widgets = look.layers();
    return d;
  }

  // The old card's entrances, as a layer's.
  const LOOK_ENTRANCES = { slide: 'slide-up', pop: 'scale-in', fade: 'fade', zoom: 'scale-in', bounce: 'boing', drop: 'slide-down' };

  // A look from before designs (the alerts page's theme and what was
  // changed on it, or an Alert card layer's) as that theme's ready-made
  // layers, with its colours, font, entrance, sound, size and place carried
  // over. What layers can't be stays a card (null), exactly as it was:
  // custom CSS (written for the card's markup), the picture on top, or a
  // card's colour on a theme with no card (minimal).
  function alertLookDoc(look) {
    if (look.css || look.picture === 'top') return null;
    const d = alertPreset(ALERT_THEMES.indexOf(look.theme) >= 0 ? look.theme : 'island');
    const hex = function (c) { return /^#[0-9a-f]{6}$/i.test(c || '') ? c : ''; };
    const set = function (v) { return v != null && v !== ''; };
    if (hex(look.accent)) d.theme.accent = look.accent;
    if (hex(look.text)) d.theme.ink = look.text;
    if (look.sound) d.sound = look.sound;
    // The card is the first wide shape; shapes painted like it (a bubble's
    // tails) take its new colour too.
    const card = d.widgets.filter(function (w) { return w.kind === 'Shape' && w.size.w >= 300; })[0];
    if (!card && (hex(look.background) || set(look.opacity) || set(look.radius))) return null;
    if (card && LOOK_ENTRANCES[look.animation]) card.base.anim.style = LOOK_ENTRANCES[look.animation];
    if (card) {
      const was = card.base.color;
      d.widgets.forEach(function (w) {
        if (w.kind === 'Shape' && w.base.color === was && hex(look.background)) w.base.color = look.background;
      });
      if (set(look.opacity)) card.base.opacity = Math.max(0, Math.min(100, +look.opacity)) / 100;
      if (set(look.radius)) card.radius = Math.max(0, +look.radius);
    }
    if (FONTS[look.font] != null || look.font === SYSTEM_FONT) {
      d.widgets.forEach(function (w) { if (w.kind === 'Text') w.font_family = look.font; });
    }
    const scale = Math.max(50, Math.min(200, +look.scale || 100)) / 100;
    if (scale !== 1) {
      const sized = function (n) { return Math.round(n * scale); };
      d.widgets.forEach(function (w) {
        w.offset = { x: sized(w.offset.x), y: sized(w.offset.y) };
        w.size = { w: sized(w.size.w), h: sized(w.size.h) };
        ['font_px', 'radius', 'border', 'nopic', 'letter_spacing'].forEach(function (k) { if (w[k] != null) w[k] = sized(w[k]); });
      });
    }
    placeAlertLayers(d.widgets, look.position, look.align);
    return d;
  }

  // Move a ready-made design's layers (made at the bottom middle) to the
  // top or the middle of the screen, or to its left or right side, as one
  // piece, the old card's margin from the edge kept. The viewer's message
  // isn't always there: it doesn't count in the piece's size, and from the
  // top it goes under the card.
  function placeAlertLayers(widgets, position, align) {
    const vert = position === 'top' ? 'top' : position === 'middle' ? '' : 'bottom';
    const side = align === 'left' || align === 'right' ? align : '';
    if (vert === 'bottom' && !side) return;
    const card = widgets.filter(function (w) { return !w.with_message; });
    const left = Math.min.apply(null, card.map(function (w) { return w.offset.x; }));
    const right = Math.max.apply(null, card.map(function (w) { return w.offset.x + w.size.w; }));
    const low = Math.min.apply(null, card.map(function (w) { return w.offset.y; }));
    const high = Math.max.apply(null, card.map(function (w) { return w.offset.y + w.size.h; }));
    const margin = 36, gap = 12, half = Math.round((high - low) / 2);
    widgets.forEach(function (w) {
      let x = w.offset.x, y = w.offset.y;
      if (side === 'left') x = margin + (w.offset.x - left);
      if (side === 'right') x = margin + (right - w.offset.x - w.size.w);
      // From the top or the middle: what was highest is nearest the edge,
      // and the message goes under the card, where a long one grows away
      // from it (its top edge is the one that stays put).
      if (vert === 'top') y = w.with_message ? high + gap : low + (high - w.offset.y - w.size.h);
      if (!vert) y = w.with_message ? high - low - half + gap : (high - w.offset.y - w.size.h) - half;
      w.anchor = vert && side ? vert + '-' + side : vert || side || 'center';
      w.offset = { x: x, y: y };
    });
  }

  // An alert design to start from: the alert's picture beside its title and
  // words, on a dark card at the top of the screen. Each part waits hidden
  // and comes in, one after another, when an alert plays.
  function alertDoc(name) {
    const d = defaultDoc(name);
    d.purpose = 'alerts'; d.sound = 'chime';
    const waits = function (w, step) {
      w.always = true; w.states = { idle: { visible: false } }; w.base.anim = arrive(step);
      return w;
    };
    const card = backCard('w1', 'top', { x: -300, y: 90 }, { w: 600, h: 150 });
    const pic = defaultWidget('Image', 'w2');
    pic.src = '{{alert.image}}'; pic.anchor = 'top'; pic.offset = { x: -276, y: 114 }; pic.size = { w: 102, h: 102 };
    const title = streamText('w3', '{{alert.title}}', { anchor: 'top', offset: { x: -152, y: 122 }, font_px: 22, color: '@accent', uppercase: true });
    const words = streamText('w4', '{{alert.text}}', { anchor: 'top', offset: { x: -152, y: 156 }, font_px: 42, color: '@ink' });
    d.widgets.push(waits(card, 0), waits(pic, 1), waits(title, 2), waits(words, 3));
    return d;
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
    glitch: function (a) { return '0%,100%{transform:translate(0)}20%{transform:translate(' + r(-8 * a) + 'px,' + r(3 * a) + 'px)}40%{transform:translate(' + r(8 * a) + 'px,' + r(-3 * a) + 'px)}60%{transform:translate(' + r(-5 * a) + 'px,' + r(-3 * a) + 'px)}80%{transform:translate(' + r(5 * a) + 'px,' + r(3 * a) + 'px)}'; },
    // (Each ends clipped 80px out, so a layer's drop shadow shows whole.)
    wipe: function () { return 'from{clip-path:inset(-80px calc(100% + 80px) -80px -80px)}to{clip-path:inset(-80px)}'; },
    shutter: function () { return 'from{clip-path:inset(50% -80px 50% -80px)}to{clip-path:inset(-80px)}'; },
    grow: function () { return 'from{opacity:0;clip-path:inset(45% 45% round 999px)}30%{opacity:var(--ic-op,1)}70%{clip-path:inset(-2% -2% round 24px)}to{opacity:var(--ic-op,1);clip-path:inset(-80px round 0)}'; },
    stamp: function (a) { return 'from{opacity:0;transform:scale(' + r(1 + 0.8 * a) + ')}60%{opacity:var(--ic-op,1);transform:scale(' + r(1 - 0.03 * a) + ')}to{opacity:var(--ic-op,1);transform:none}'; },
    boing: function (a) { return '0%{opacity:0;transform:scale(0)}35%{opacity:var(--ic-op,1);transform:scale(' + r(1 + 0.12 * a) + ',' + r(1 - 0.12 * a) + ')}55%{transform:scale(' + r(1 - 0.07 * a) + ',' + r(1 + 0.07 * a) + ')}72%{transform:scale(' + r(1 + 0.04 * a) + ',' + r(1 - 0.03 * a) + ')}100%{opacity:var(--ic-op,1);transform:none}'; },
    // The words do the moving (textFx in the runtime): the layer just shows.
    decode: function () { return 'from,to{opacity:var(--ic-op,1)}'; },
    typewriter: function () { return 'from,to{opacity:var(--ic-op,1)}'; }
  };
  // Default (intensity 1) keyframes registered globally, plus the two that
  // take no intensity (fade, animated-gradient).
  const KEYFRAMES = {
    fade: '@keyframes ic-fade{from{opacity:0}to{opacity:var(--ic-op,1)}}',
    gradshift: '@keyframes ic-gradshift{0%{background-position:0% 50%}100%{background-position:200% 50%}}',
    gradwave: '@keyframes ic-gradwave{0%{background-position:0% 50%}100%{background-position:100% 50%}}',
    gradshine: '@keyframes ic-gradshine{0%{background-position:150% 0,0 0}100%{background-position:-50% 0,0 0}}'
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
    if (stateKey === 'idle' && !w.always) return [];
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
    // between phases. Transforms stay free for the keyframe animations. No
    // will-change: it would hold a GPU layer per widget for good (a full
    // screen one for the edge, wash and tide), and OBS shares that GPU.
    '.icw{transition:color .45s cubic-bezier(.2,.7,.2,1),filter .45s ease,' +
      'opacity .45s cubic-bezier(.2,.7,.2,1),' +
      'top .5s cubic-bezier(.2,.8,.2,1),left .5s cubic-bezier(.2,.8,.2,1),' +
      'right .5s cubic-bezier(.2,.8,.2,1),bottom .5s cubic-bezier(.2,.8,.2,1),' +
      'margin-top .5s cubic-bezier(.2,.8,.2,1),margin-left .5s cubic-bezier(.2,.8,.2,1)}',
    '.icw .v{font-variant-numeric:tabular-nums;line-height:1}',
    // A goal bar or timer whose goal or timer no longer exists.
    '.icw.ic-none{visibility:hidden}',
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
      case 'Goal':
        return '<span class="g"><span class="trk"><span class="fill"></span></span><span class="add"></span></span>';
      case 'Timer':
        return '<span class="t"><span class="v">0:00</span><span class="add"></span></span>';
      case 'BufferBar':
        return '<span class="fill"></span>';
      case 'BufferRing':
        return '<svg viewBox="0 0 100 100"><circle class="trk" cx="50" cy="50" r="46"/>' +
          '<circle class="arc" cx="50" cy="50" r="46" stroke-dasharray="289" stroke-dashoffset="289"/></svg>' +
          (w.notext ? '' : '<span class="v">0.0</span>');
      case 'BitrateMeter': {
        const shade = 'ica-' + cssId(w.id);
        return '<svg viewBox="0 0 100 32" preserveAspectRatio="none"><defs><linearGradient id="' + shade + '" x1="0" y1="0" x2="0" y2="1">' +
          '<stop offset="0" stop-color="currentColor" stop-opacity=".3"/><stop offset="1" stop-color="currentColor" stop-opacity="0"/></linearGradient></defs>' +
          '<polygon class="area" points="" fill="url(#' + shade + ')"/><polyline class="spark" points=""/></svg>';
      }
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
        return '<span class="lbl">' + esc(shownText(w)) + '</span>';
      case 'EdgeAccent':
        return '';
      case 'BackgroundTint':
        return '';
      case 'AlertCard':
        // The alerts page draws the card, in this layer's look (alerts.html
        // ?embed); the runtime tells it when (cardEls).
        return '<iframe class="ic-card" src="/alerts?embed" data-look="' + esc(JSON.stringify(w.card || {})) + '" tabindex="-1" aria-hidden="true"></iframe>';
      case 'Image':
        // A picture said in words ({{alert.image}}) is filled in live.
        if (/\{\{/.test(w.src || '')) return '<img data-src-tpl="' + esc(w.src) + '" alt="">';
        return w.src ? '<img src="' + esc(w.src) + '" alt="">' : '';
      case 'CustomHTML':
        return ''; // hydrated by the runtime into a sandboxed iframe
      default:
        return '';
    }
  }

  // The words a text widget starts with. A lower-third left blank says what
  // it is for, rather than showing an empty strip.
  function shownText(w) {
    return w.base.text || (w.kind === 'Banner' ? 'ON A DELAY' : '');
  }

  // CSS for one widget kind, scoped to #<id>. Compositor-only animation.
  function widgetCss(w, theme) {
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
        // Kept to its width (fit): as wide as its box, aligned in it, and
        // shrunk (--ic-fit, set by the runtime) when the words are longer.
        // The shadow keeps words readable over a game; on a card of their
        // own it can go (shadow: false).
        css = id + '{font-size:' + (w.fit ? 'calc(' + fp + 'px * var(--ic-fit,1))' : fp + 'px') + ';font-weight:600;color:currentColor;white-space:nowrap' +
          (w.shadow === false ? '' : ';text-shadow:0 1px 2px rgba(0,0,0,.55),0 1px 12px rgba(0,0,0,.4)') + '}';
        if (w.fit) css += id + '{width:' + r(+w.size.w) + 'px;line-height:' + Math.round(fp * 1.15) + 'px;text-align:' + (['center', 'right'].indexOf(w.text_align) >= 0 ? w.text_align : 'left') + '}';
        // Wrapped instead: onto up to three lines in its width, the rest
        // cut with an ellipsis. Anchored at the bottom, it grows upwards.
        else if (w.wrap) {
          // One line tall even with no words yet, so the Studio's handle
          // sits on it.
          css += id + '{width:' + r(+w.size.w) + 'px;white-space:normal;line-height:' + Math.round(fp * 1.25) + 'px;min-height:' + Math.round(fp * 1.25) + 'px;text-align:' + (['center', 'right'].indexOf(w.text_align) >= 0 ? w.text_align : 'left') + '}' +
            id + ' .lbl{display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:3;overflow:hidden;overflow-wrap:anywhere}';
        }
        break;
      case 'CutCounter':
        css = id + '{display:inline-flex;align-items:baseline;gap:6px;font-size:' + fp + 'px;font-weight:700;color:currentColor}';
        break;
      case 'SessionElapsed':
        css = id + '{font-size:' + fp + 'px;font-weight:600;color:currentColor;font-variant-numeric:tabular-nums}';
        break;
      case 'Goal': {
        // The bar fills by sliding (a transform), which the compositor does
        // alone. Its words are Text layers of their own.
        const round = w.radius != null ? Math.max(0, +w.radius) + 'px' : '999px';
        const track = rgba(themed(w.track || '#0a0c10', theme), w.trackOpacity != null ? +w.trackOpacity : 0.6);
        css = '@keyframes ic-goal-hit{0%,100%{transform:scale(1)}40%{transform:scale(1.035)}}' +
          id + '{width:' + w.size.w + 'px;color:currentColor}' +
          id + ' .g{position:relative;display:block;transform-origin:center}' +
          // What was just added, rising from the end of the fill.
          '@keyframes ic-goal-add{0%{opacity:0;transform:translate(-50%,8px)}18%,65%{opacity:1;transform:translate(-50%,0)}100%{opacity:0;transform:translate(-50%,-14px)}}' +
          id + ' .add{position:absolute;bottom:100%;left:var(--ic-p,0%);margin-bottom:8px;transform:translate(-50%,0);opacity:0;' +
          'font-weight:800;font-size:' + Math.max(14, Math.round(w.size.h * 1.3)) + 'px;line-height:1;color:var(--ic-live);white-space:nowrap;text-shadow:0 1px 2px rgba(0,0,0,.6);pointer-events:none}' +
          id + ' .add.on{animation:ic-goal-add 1.6s cubic-bezier(.2,.7,.2,1) 1 both}' +
          id + ' .g.hit{animation:ic-goal-hit .7s cubic-bezier(.2,.7,.2,1) 1}' +
          id + ' .trk{position:relative;display:block;height:' + Math.max(3, w.size.h) + 'px;border-radius:' + round + ';overflow:hidden;' +
          'background:' + track + ';box-shadow:inset 0 0 0 1px rgba(255,255,255,.14)}' +
          id + ' .fill{position:absolute;inset:0;border-radius:' + round + ';background:currentColor;transform:translateX(-100%);' +
          'transition:transform var(--ic-sm,.7s) cubic-bezier(.16,1,.3,1)}';
        break;
      }
      case 'Timer':
        css = '@keyframes ic-time-add{0%{opacity:0;transform:translateY(40%)}18%,70%{opacity:1;transform:none}100%{opacity:0;transform:translateY(-60%)}}' +
          id + '{display:inline-block;color:currentColor;font-weight:700}' +
          id + ' .t{position:relative;display:block}' +
          id + ' .v{display:block;font-size:' + fp + 'px;letter-spacing:-.02em;white-space:nowrap;' +
          'text-shadow:0 2px 3px rgba(0,0,0,.5),0 0 18px rgba(0,0,0,.35);transition:opacity .45s ease}' +
          id + '.over .v{opacity:.6}' +
          // The last minute: the warning colour, and a beat each second.
          '@keyframes ic-time-beat{0%{transform:scale(1.045)}100%{transform:scale(1)}}' +
          'body[data-state] ' + id + '.soon .v{color:var(--ic-warn);-webkit-text-fill-color:currentColor;background:none;' +
          'animation:ic-time-beat 1s cubic-bezier(.2,.7,.2,1) infinite}' +
          // Time someone just added, beside the clock for a moment.
          id + ' .add{position:absolute;left:100%;top:0;margin-left:.5em;font-size:' + Math.round(fp * 0.34) + 'px;font-weight:800;' +
          'white-space:nowrap;opacity:0;color:var(--ic-live);text-shadow:0 1px 2px rgba(0,0,0,.6)}' +
          id + ' .add.on{animation:ic-time-add 1.8s cubic-bezier(.2,.7,.2,1) 1 both}';
        break;
      case 'BufferBar':
        css = id + '{width:' + w.size.w + 'px;height:' + Math.max(3, w.size.h) + 'px;border-radius:999px;overflow:hidden;' +
          'background:rgba(255,255,255,.12);box-shadow:inset 0 0 0 1px rgba(255,255,255,.08)}' +
          id + ' .fill{display:block;height:100%;border-radius:999px;background:currentColor;transform:translateX(-100%);' +
          'transition:transform var(--ic-sm,.42s) cubic-bezier(.16,1,.3,1)}';
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
        css = '@keyframes ic-dot-down{50%{opacity:.4}}' +
          id + ' .dots{display:inline-flex;gap:8px}' +
          id + ' .dots i{width:11px;height:11px;border-radius:50%;background:rgba(255,255,255,.22);' +
          'transition:background-color .4s ease,box-shadow .4s ease}' +
          id + ' .dots i.up{background:currentColor;box-shadow:0 0 8px currentColor}' +
          id + ' .dots i.down{background:var(--ic-error);box-shadow:0 0 8px var(--ic-error);animation:ic-dot-down 1.4s ease-in-out infinite}';
        break;
      case 'Icon':
        css = id + '{display:inline-flex;color:currentColor}' +
          id + ' svg{width:' + w.size.w + 'px;height:' + w.size.w + 'px;display:block}';
        break;
      case 'Shape':
        css = id + '{width:' + w.size.w + 'px;height:' + w.size.h + 'px;background:currentColor;' +
          shapeRadius(w.shape) + '}' + edgeCss(w, id);
        break;
      case 'Divider':
        css = id + '{width:' + w.size.w + 'px;height:' + Math.max(1, w.size.h) + 'px;border-radius:2px;' +
          'background:linear-gradient(90deg,transparent,currentColor 18%,currentColor 82%,transparent)}';
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
        css = '@keyframes ic-dot-ring{0%{transform:scale(1);opacity:.7}100%{transform:scale(2.6);opacity:0}}' +
          id + ' .dot{position:relative;display:block;width:' + Math.max(10, w.size.h) + 'px;height:' + Math.max(10, w.size.h) + 'px;' +
          'border-radius:50%;background:currentColor;box-shadow:0 0 10px currentColor}' +
          id + ' .dot::after{content:"";position:absolute;inset:0;border-radius:50%;border:2px solid currentColor;' +
          'animation:ic-dot-ring 2s cubic-bezier(.2,.7,.2,1) infinite}';
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
        // Filling its box (cover): its width and height, cropped to them.
        css = id + ' img{width:' + w.size.w + 'px;height:' + (w.cover ? r(+w.size.h) + 'px;object-fit:cover' : 'auto') + ';display:block}' +
          edgeCss(w, id + ' img');
        break;
      case 'AlertCard':
        css = id + '{width:' + w.size.w + 'px;height:' + w.size.h + 'px}' +
          id + ' iframe{width:100%;height:100%;border:0;display:block;background:transparent;color-scheme:normal}';
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
    if (['DelayReadout', 'StatePill', 'Text', 'CutCounter', 'SessionElapsed', 'Banner', 'Timer'].indexOf(w.kind) >= 0) {
      if (w.font_weight) css += id + ',' + id + ' .v,' + id + ' .lbl,' + id + ' .u{font-weight:' + w.font_weight + '}';
      if (w.letter_spacing != null) css += id + '{letter-spacing:' + w.letter_spacing + 'px}';
      if (w.uppercase) css += id + '{text-transform:uppercase}';
      else if (w.uppercase === false && w.kind === 'StatePill') css += id + '{text-transform:none}';
      if (FONTS[w.font_family] != null) css += id + '{font-family:"' + w.font_family + '",sans-serif}';
      else if (w.font_family === SYSTEM_FONT) css += id + '{font-family:system-ui,"Segoe UI",sans-serif}';
    }
    // An alert with no picture: this layer moves left by `nopic` px, into
    // the picture's place (the translate property, so motions still apply).
    if (+w.nopic) css += 'body[data-nopic] ' + id + '{translate:' + r(-w.nopic) + 'px 0}';
    // Only there when the alert brings a message of the viewer's.
    if (w.with_message) css += 'body[data-nomsg] ' + id + '{display:none}';
    return css;
  }

  // A box's own corners (px, over its shape's), ring and drop shadow: a
  // Shape's, or an Image's picture (`sel`).
  function edgeCss(w, sel) {
    let css = '';
    if (w.radius != null && w.radius !== '') css += 'border-radius:' + Math.max(0, +w.radius) + 'px;';
    if (+w.border > 0) css += 'box-sizing:border-box;border:' + r(+w.border) + 'px solid ' + resolveColor(w.border_color || '@ink') + ';';
    if (w.shadow) css += 'box-shadow:0 14px 40px rgba(0,0,0,.42),0 3px 10px rgba(0,0,0,.28);';
    return css ? sel + '{' + css + '}' : '';
  }
  function shapeRadius(shape) {
    if (shape === 'circle') return 'border-radius:50%';
    if (shape === 'pill') return 'border-radius:999px';
    if (shape === 'line') return 'border-radius:2px';
    if (shape === 'card') return 'border-radius:18px';
    return 'border-radius:6px';
  }
  function cssId(id) { return String(id).replace(/[^a-zA-Z0-9_-]/g, ''); }

  // Per-state CSS: color / opacity / visibility / animation keyed on
  // body[data-state]. This is what makes the overlay JS-light - the
  // compositor handles all the per-state visual transitions.
  function stateCss(w) {
    const id = '#' + cssId(w.id);
    const safeId = (w.id || 'w').replace(/[^a-zA-Z0-9_-]/g, '');
    let customKf = '';
    let usesAnim = {};
    // Each state's look: the parts of the widget it styles (' .fill', or ''
    // for the widget itself) and their declarations.
    const looks = STATES.map(function (st) {
      const own = (w.states && w.states[st.key]) || {};
      // "Same look in every state": only whether it shows is per state.
      const sp = w.always ? { visible: own.visible } : own;
      const merged = mergeProps(w.base, sp);
      const parts = [];
      let rule = '';
      // Animations the gradient brings, on the widget itself: they join its
      // own, which would otherwise replace them.
      let extra = [];
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
        // Per-state gradient (a second colour). Text widgets clip the
        // gradient onto the glyph spans directly (clipping on the flex
        // wrapper renders blank), a goal bar paints its fill, and solid
        // widgets get a gradient background.
        // One look in every state: one set of glow keyframes, the same name in
        // each state, so a delay change never restarts the glow.
        const g = merged.grad ? gradientCss(merged, safeId + (w.always ? '' : st.key)) : null;
        const inner = g && (TEXT_KINDS.indexOf(w.kind) >= 0 || w.kind === 'Goal');
        // Per-state glow: a drop-shadow halo in the widget's own colour, or
        // in the gradient's colours, on whatever the gradient paints.
        if (merged.glow && !inner) rule += g ? g.glow : 'filter:drop-shadow(0 0 ' + merged.glow + 'px currentColor);';
        if (g) {
          if (customKf.indexOf(g.kf) < 0) customKf += g.kf;
          g.uses.forEach(function (k) { usesAnim[k] = true; });
          if (TEXT_KINDS.indexOf(w.kind) >= 0) {
            parts.push([[' .v', ' .lbl', ' .u'], g.paint + '-webkit-background-clip:text;background-clip:text;-webkit-text-fill-color:transparent;color:transparent;' +
              animDecl(g.moves.concat(g.glowMoves)) + g.glow]);
          } else if (w.kind === 'Goal') {
            // The fill is clipped inside its track: the glow goes on the track.
            parts.push([[' .fill'], g.paint + animDecl(g.moves)], [[' .trk'], g.glow + animDecl(g.glowMoves)]);
          } else if (w.kind === 'Shape' || w.kind === 'Banner' || w.kind === 'Divider') {
            rule += g.paint;
            extra = g.moves.concat(g.glowMoves);
          }
        }
        // Per-state position: the widget moves to a new spot in this state
        // and slides there via the `.icw` layout transition.
        if (sp.offset) rule += offsetDecls(w.anchor, sp.offset);
        const list = animsForState(w, sp, st.key).filter(function (a) { return a && a.style !== 'instant'; });
        const named = [];
        if (list.length) {
          list.forEach(function (a, idx) {
            // intensity: 1 uses the shared keyframe; anything else bakes a
            // per-state scaled variant so each animation tunes independently.
            const amp = (a.amount != null && a.amount > 0) ? a.amount : 1;
            let name = 'ic-' + a.style;
            if (amp !== 1 && KEYFRAME_FN[a.style]) {
              // One look in every state: one name, so a delay change
              // never plays it again.
              name = 'ic-' + a.style + '-' + safeId + (w.always ? '' : st.key) + idx;
              const kf = '@keyframes ' + name + '{' + KEYFRAME_FN[a.style](amp) + '}';
              if (customKf.indexOf(kf) < 0) customKf += kf;
            } else {
              usesAnim[a.style] = true;
            }
            const v = animValue(a, name);
            if (v) named.push(v);
          });
        }
        rule += animDecl(named.concat(extra));
      }
      parts.push([[''], rule]);
      return { key: st.key, parts: parts.filter(function (p) { return p[1]; }) };
    });
    // One rule for the states that look alike: a widget with one look in
    // every state gets one rule, not five copies of it.
    const groups = [];
    looks.forEach(function (look) {
      const sig = JSON.stringify(look.parts);
      const same = groups.find(function (g) { return g.sig === sig; });
      if (same) same.keys.push(look.key);
      else groups.push({ sig: sig, keys: [look.key], parts: look.parts });
    });
    let out = '';
    groups.forEach(function (g) {
      const heads = g.keys.length === STATES.length ? ['body[data-state] ']
        : g.keys.map(function (k) { return 'body[data-state="' + k + '"] '; });
      g.parts.forEach(function (p) {
        const sels = [];
        heads.forEach(function (head) { p[0].forEach(function (sub) { sels.push(head + id + sub); }); });
        out += sels.join(',') + '{' + p[1] + '}';
      });
    });
    return { css: out + customKf, anims: usesAnim };
  }

  // Goal bars, timers and the words that say them were styled per delay
  // state at first: a look set while the Studio showed "Active" never
  // reached OBS while you weren't live. They have one look for every state
  // now, and an old one keeps the look it was given: its most styled
  // state's, Active first.
  const LOOK_KEYS = ['color', 'opacity', 'glow', 'grad', 'grad_anim', 'grad_speed', 'grad_angle', 'anims', 'anim', 'text'];
  function aboutStream(w) {
    return w.kind === 'Goal' || w.kind === 'Timer' || /\{\{(goal|timer)\./.test((w.base && w.base.text) || '');
  }
  // One look in every state, the one state `from` had: it becomes the
  // widget's own, and the states keep only whether it shows. (The Studio's
  // "Same look in every delay state" switch, and the upgrade below.)
  function oneLook(w, from) {
    const states = w.states || {}, look = states[from] || {};
    LOOK_KEYS.forEach(function (k) { if (look[k] != null) w.base[k] = JSON.parse(JSON.stringify(look[k])); });
    if (look.anims) delete w.base.anim;
    else if (look.anim) delete w.base.anims;
    Object.keys(states).forEach(function (k) {
      LOOK_KEYS.concat(['hide_after_ms', 'exit', 'offset']).forEach(function (x) { delete states[k][x]; });
    });
    w.always = true;
  }
  function upgradeWidget(w) {
    if (w.always !== undefined || !aboutStream(w)) return;
    const states = w.states || {};
    const styled = function (k) { return LOOK_KEYS.filter(function (x) { return (states[k] || {})[x] != null; }).length; };
    oneLook(w, Object.keys(states).reduce(function (best, k) { return styled(k) > styled(best) ? k : best; }, 'active'));
  }
  // The page of the Overlay tab an overlay says it is on: 'delay' (it shows
  // the delay), 'goals' (a goal's or a timer's own overlay, on its card),
  // 'alerts' (an alert design, which the alerts page plays) or 'custom'
  // (anything else), set where it was made. The app has the last word
  // (web.rs overlay_kind); one from before pages is a delay overlay.
  const PURPOSES = ['delay', 'goals', 'alerts', 'custom'];
  function docPurpose(doc) {
    return PURPOSES.indexOf(doc.purpose) >= 0 ? doc.purpose : 'delay';
  }
  // A saved overlay brought up to date, in place, as the Studio opens it.
  function upgradeDoc(doc) {
    (doc.widgets || []).forEach(function (w) { if (w && w.base) upgradeWidget(w); });
    return doc;
  }

  function animDecl(list) { return list.length ? 'animation:' + list.join(',') + ';' : ''; }

  // A state's gradient: how it paints and how it moves. Flow slides it
  // across on a loop, wave sways it back and forth, shine sweeps a light
  // over it. A glow takes its colours: following them as they move, under
  // `key`'s own keyframes, or in the first colour while it stays still.
  function gradientCss(m, key) {
    const c1 = resolveColor(m.color), c2 = resolveColor(m.grad);
    const angle = (m.grad_angle != null ? +m.grad_angle : 135) + 'deg';
    const motion = m.grad_anim === true ? 'flow' : (m.grad_anim || '');
    const secs = Math.max(0.5, m.grad_speed != null ? +m.grad_speed : 3.5);
    const g = { paint: '', moves: [], glowMoves: [], glow: '', kf: '', uses: [] };
    if (motion === 'shine') {
      g.paint = 'background:linear-gradient(100deg,transparent 35%,rgba(255,255,255,.85) 50%,transparent 65%),' +
        'linear-gradient(' + angle + ',' + c1 + ',' + c2 + ');background-size:250% 100%,100% 100%;background-repeat:no-repeat;';
      g.moves.push('ic-gradshine ' + r(secs) + 's ease-in-out infinite');
      g.uses.push('gradshine');
    } else if (motion === 'wave') {
      g.paint = 'background:linear-gradient(' + angle + ',' + c1 + ',' + c2 + ');background-size:200% 100%;';
      g.moves.push('ic-gradwave ' + r(secs) + 's ease-in-out infinite alternate');
      g.uses.push('gradwave');
    } else {
      // 3-stop loop so a flowing gradient wraps seamlessly.
      g.paint = 'background:linear-gradient(' + angle + ',' + c1 + ',' + c2 + ',' + c1 + ');background-size:200% 100%;';
      if (motion === 'flow') { g.moves.push('ic-gradshift ' + r(secs) + 's linear infinite'); g.uses.push('gradshift'); }
    }
    if (!m.glow) return g;
    const halo = function (c) { return 'filter:drop-shadow(0 0 ' + m.glow + 'px ' + c + ')'; };
    if (motion === 'flow' || motion === 'wave') {
      const name = 'ic-glow-' + key;
      g.kf = '@keyframes ' + name + '{0%,100%{' + halo(c1) + '}50%{' + halo(c2) + '}}';
      // A wave goes there and back in two passes; a flow in one.
      g.glowMoves.push(name + ' ' + r(motion === 'wave' ? secs * 2 : secs) + 's ease-in-out infinite');
    } else {
      g.glow = halo(c1) + ';';
    }
    return g;
  }

  function mergeProps(base, over) {
    return {
      visible: over.visible != null ? over.visible : true,
      color: over.color != null ? over.color : base.color,
      opacity: over.opacity != null ? over.opacity : base.opacity,
      glow: over.glow != null ? over.glow : base.glow,
      grad: over.grad != null ? over.grad : base.grad,
      grad_anim: over.grad_anim != null ? over.grad_anim : base.grad_anim,
      grad_speed: over.grad_speed != null ? over.grad_speed : base.grad_speed,
      grad_angle: over.grad_angle != null ? over.grad_angle : base.grad_angle,
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
  const TEXT_KINDS = ['DelayReadout', 'StatePill', 'Text', 'CutCounter', 'SessionElapsed', 'Banner', 'Timer'];

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
  // An alert design waits, hidden ("idle"), until the alerts page (or the
  // Studio) hands it an alert: it shows ("active") for the alert's length,
  // then leaves. The delay's own state never moves it.
  var ALERTS=body.getAttribute('data-purpose')==='alerts', ALERT=null, alertT=0;
  if(ALERTS&&!forced) forced='idle';
  // The feed counts each overlay's saves since the app started, under a short
  // key of its file name (web.rs overlay_key, FNV-1a). A count that changes
  // under this page means the Studio saved it: load the new one.
  var SELF=(function(){
    var m=/\/overlay\/([^\/]+)\.html$/.exec(location.pathname); if(!m) return '';
    var s=decodeURIComponent(m[1]), h=0x811c9dc5;
    for(var i=0;i<s.length;i++){ h^=s.charCodeAt(i); h=Math.imul(h,0x01000193)>>>0; }
    return ('0000000'+h.toString(16)).slice(-8);
  })(), SAVED=null;

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
    if(Math.abs(to-from)<0.05){ if(el.textContent!==to.toFixed(dp)){el.textContent=to.toFixed(dp);el.setAttribute('data-n',to);} return; }
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
    document.querySelectorAll('[data-kind="BufferBar"] .fill').forEach(function(f){f.style.transform='translateX('+(frac*100-100).toFixed(1)+'%)';});
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
      var area=el.querySelector('.area'); if(area) area.setAttribute('points',pts+' 100,32 0,32');
    });
    // CutCounter
    var cuts=(s.stats&&s.stats.cuts)||0;
    document.querySelectorAll('[data-kind="CutCounter"] .lbl').forEach(function(l){setText(l,(l.getAttribute('data-tpl')||'{n}').replace('{n}',cuts));});
    // DestinationStatus: a dot each (one grey while there are none). A dot
    // that changes keeps its element, so its colour fades across.
    var dests=s.destinations||[];
    document.querySelectorAll('[data-kind="DestinationStatus"] .dots').forEach(function(d){
      var n=Math.max(1,dests.length);
      while(d.children.length<n) d.appendChild(document.createElement('i'));
      while(d.children.length>n) d.removeChild(d.lastChild);
      for(var i=0;i<n;i++){ var c=dests[i]?(dests[i].alive?'up':'down'):''; if(d.children[i].className!==c) d.children[i].className=c; }
    });
    // SessionElapsed
    if(s.ingest_alive&&!sessionStart) sessionStart=Date.now();
    if(!s.ingest_alive) sessionStart=0;
    document.querySelectorAll('[data-kind="SessionElapsed"] .v').forEach(function(v){
      var sec=sessionStart?Math.floor((Date.now()-sessionStart)/1000):0;
      var m=Math.floor(sec/60),ss=sec%60; setText(v,m+':'+(ss<10?'0':'')+ss);
    });
  }

  // ---- goals and timers ----
  // The feed says where each stands only when that changes: a running timer
  // comes as the moment it ends, and the page counts down by itself, one
  // wake-up a second and only while a clock on it is running. A page with
  // no goal bar and no timer does none of this.
  var GOALS=[], TIMERS=[], PREVIEW=false, clockT=0;
  var goalEls=[].slice.call(document.querySelectorAll('[data-kind="Goal"]'));
  var timerEls=[].slice.call(document.querySelectorAll('[data-kind="Timer"]'));
  // The timers this page's words say ({{timer.subathon}}), so those words
  // tick with the clock. Words are baked in, so they are read once; in the
  // Studio's preview an edit changes them in place, so they are read anew.
  var wordTimers=null;
  function timerWords(){
    if(wordTimers&&!PREVIEW) return wordTimers;
    var ids={}, said=/\{\{timer\.(\w+)/g, m;
    document.querySelectorAll('[data-has-txt]').forEach(function(el){
      for(var i=0;i<el.attributes.length;i++){ while((m=said.exec(el.attributes[i].value))) ids[m[1]]=1; }
    });
    return (wordTimers=Object.keys(ids));
  }
  // Each goal and timer by name, for text: {{goal.subs}} is the count, and
  // .target, .left, .percent and .name the rest; {{timer.subathon}} the clock.
  function trackerView(view){
    GOALS.forEach(function(g){
      var k='goal.'+g.id, p=Math.max(0,Math.min(1,g.value/(g.target||1)));
      view[k]=g.value; view[k+'.target']=g.target; view[k+'.left']=Math.max(0,g.target-g.value);
      view[k+'.percent']=Math.floor(p*100); view[k+'.name']=g.name;
    });
    TIMERS.forEach(function(t){
      var k='timer.'+t.id;
      view[k]=clockText(t.running?Math.max(0,t.ends_at_ms-Date.now()):t.left_ms); view[k+'.name']=t.name;
    });
    return view;
  }
  // What the Studio shows while there is no goal or timer yet.
  var SAMPLE_GOAL={name:'Sub goal',value:32,target:50}, SAMPLE_TIMER={name:'Subathon',running:false,left_ms:5025000};
  function pick(list,id){
    if(!id) return list[0]||null;
    for(var i=0;i<list.length;i++){ if(list[i].id===id) return list[i]; }
    return null;
  }
  function part(el,cls){ var k='__'+cls; return el[k]||(el[k]=el.querySelector('.'+cls)); }
  function setText(el,t){ if(el&&el.__t!==t){ el.__t=t; el.textContent=t; } }
  function clockText(ms){
    var t=Math.ceil(ms/1000), h=Math.floor(t/3600), m=Math.floor(t/60)%60, s=t%60;
    return (h?h+':'+(m<10?'0':'')+m:m)+':'+(s<10?'0':'')+s;
  }
  function replay(el,cls){ el.classList.remove(cls); void el.offsetWidth; el.classList.add(cls); }
  function paintGoals(){
    goalEls.forEach(function(el){
      var g=pick(GOALS,el.getAttribute('data-goal'))||(PREVIEW?SAMPLE_GOAL:null);
      el.classList.toggle('ic-none',!g); if(!g) return;
      var p=Math.max(0,Math.min(1,g.value/(g.target||1)));
      // More than before (not the first look): say how much, at the fill's end.
      var had=el.__v; el.__v=g.value;
      if(had!=null&&g.value>had){ var add=part(el,'add'); el.style.setProperty('--ic-p',(p*100).toFixed(1)+'%'); add.textContent='+'+(g.value-had); replay(add,'on'); }
      var was=el.__p; if(was===p) return; el.__p=p;
      part(el,'fill').style.transform='translateX('+(p*100-100).toFixed(2)+'%)';
      // Reached just now, not already full when the page opened.
      if(p>=1&&was!=null&&was<1) replay(part(el,'g'),'hit');
    });
  }
  function paintTimers(){
    timerEls.forEach(function(el){
      var t=pick(TIMERS,el.getAttribute('data-timer'))||(PREVIEW?SAMPLE_TIMER:null);
      el.classList.toggle('ic-none',!t); if(!t) return;
      var left=t.running?Math.max(0,t.ends_at_ms-Date.now()):t.left_ms;
      var over=left<=0, end=el.getAttribute('data-end');
      setText(part(el,'v'),over&&end?end:clockText(left));
      el.classList.toggle('over',over);
      el.classList.toggle('soon',t.running&&!over&&left<=60000);
      // It only goes up while running when someone added time: say so. Not
      // from 0:00, where it went up because it was started again.
      var was=el.__left; el.__left=left;
      if(t.running&&was>0&&left-was>1500){ var add=part(el,'add'); add.textContent='+'+clockText(left-was); replay(add,'on'); }
    });
  }
  // One wake-up a second, just after the shown second ends, while a clock
  // this page shows (a Timer's, or one its words say) is running. A feed
  // message has just written the words (render), so only a wake-up does.
  function tick(woke){
    clearTimeout(clockT); clockT=0;
    var words=timerWords();
    if(!timerEls.length&&!words.length) return;
    paintTimers();
    if(woke&&words.length) applyText(curState,buildView(window.__ic_last||{},curState));
    var shown=timerEls.map(function(el){ return pick(TIMERS,el.getAttribute('data-timer')); }), next=0;
    TIMERS.forEach(function(t){
      if(shown.indexOf(t)<0&&words.indexOf(t.id)<0) return;
      var left=t.running?t.ends_at_ms-Date.now():0;
      if(left>0){ var wait=(left%1000)+25; if(!next||wait<next) next=wait; }
    });
    if(next) clockT=setTimeout(function(){ tick(true); },next);
  }

  // Per-state text overrides (data-txt-<state>) with live {{var}} support
  // so a Text/StatePill/Banner can read {{delay}}, {{bitrate}}, etc.
  function substVars(t,view){ return t.replace(/\{\{([\w.]+)\}\}/g,function(m,k){ return view&&view[k]!=null?view[k]:''; }); }
  function applyText(stateKey,view){
    document.querySelectorAll('[data-has-txt]').forEach(function(el){
      var lbl=el.querySelector('.lbl'); if(lbl==null||el.__fx) return;
      var t=el.getAttribute('data-txt-'+stateKey); if(t==null) t=el.getAttribute('data-txt-base');
      if(t==null) return;
      t=(t.indexOf('{{')>=0) ? substVars(t,view) : t;
      if(lbl.textContent!==t){ lbl.textContent=t; if(el.hasAttribute('data-fit')) fitText(el); }
    });
    // A picture said in words: shown once there is one.
    document.querySelectorAll('img[data-src-tpl]').forEach(function(img){
      var src=substVars(img.getAttribute('data-src-tpl'),view);
      if(img.__src!==src){ img.__src=src; if(src) img.src=src; else img.removeAttribute('src'); img.style.visibility=src?'':'hidden'; }
    });
  }
  // Words kept to their width shrink until they fit it (never below 30%).
  function fitText(el){
    var lbl=el.querySelector('.lbl'), room=el.clientWidth; if(!lbl||!room) return;
    var now=parseFloat(el.style.getPropertyValue('--ic-fit'))||1, need=lbl.offsetWidth/now;
    el.style.setProperty('--ic-fit', String(need>room ? Math.max(0.3, room/need) : 1));
  }
  // (Measured again when their width changes, and once the fonts are in:
  // they change the words' width.)
  var fitWatch=window.ResizeObserver ? new ResizeObserver(function(seen){ seen.forEach(function(e){ fitText(e.target); }); }) : null;
  document.querySelectorAll('[data-fit]').forEach(function(el){ if(fitWatch) fitWatch.observe(el); });
  // A font is only fetched once words use it (an alert's, as it plays), so
  // each font arriving fits them again too.
  function fitAll(){ document.querySelectorAll('[data-fit]').forEach(fitText); }
  if(document.fonts){ document.fonts.ready.then(fitAll); if(document.fonts.addEventListener) document.fonts.addEventListener('loadingdone', fitAll); }
  // Words that decode (scrambled letters settling, left to right) or type
  // themselves in as a state begins. While they move, the words aren't
  // written over; once settled, they are what they say.
  var GLYPHS='ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789#%&*+<>/';
  function textFx(st){
    document.querySelectorAll('[data-txfx-'+st+']').forEach(function(el){
      var spec=el.getAttribute('data-txfx-'+st).split(':'), lbl=el.querySelector('.lbl');
      if(!lbl) return;
      var kind=spec[0], dur=Math.max(100,+spec[1]||600), t0=performance.now()+(+spec[2]||0), to=lbl.textContent, run={};
      el.__fx=run;
      (function frame(now){
        if(el.__fx!==run) return;
        var p=Math.max(0,Math.min(1,(now-t0)/dur)), n=Math.floor(p*to.length), out='';
        if(kind==='typewriter') out=to.slice(0,n);
        else for(var i=0;i<to.length;i++) out+=i<n||to[i]===' '?to[i]:GLYPHS.charAt(Math.floor(Math.random()*GLYPHS.length));
        lbl.textContent=out;
        if(p<1) requestAnimationFrame(frame); else { el.__fx=null; lbl.textContent=to; }
      })(performance.now());
    });
  }
  // What an alert says, for words and pictures: {{alert.title}},
  // {{alert.text}}, {{alert.message}} (the viewer's own words),
  // {{alert.image}}. The Studio shows a sample until one is played.
  var SAMPLE_ALERT={title:'New follower',text:'Nat',message:'First time here, love the stream!',image:'data:image/svg+xml,%3Csvg xmlns=%22http://www.w3.org/2000/svg%22 viewBox=%220 0 100 100%22%3E%3Crect width=%22100%22 height=%22100%22 rx=%2224%22 fill=%22%235ac8fa%22/%3E%3Cpath d=%22M50 21l8.6 18.4 20.2 2.3-15 13.8 4.1 19.9L50 65.3 32.1 75.4l4.1-19.9-15-13.8 20.2-2.3z%22 fill=%22%23fff%22/%3E%3C/svg%3E'};
  function alertView(view){
    var a=ALERT||(PREVIEW&&ALERTS?SAMPLE_ALERT:null);
    // The Studio's own play has no picture: it shows the sample's.
    if(a){ view['alert.title']=a.title||''; view['alert.text']=a.text||''; view['alert.message']=a.message||''; view['alert.image']=a.image!=null?a.image:(PREVIEW?SAMPLE_ALERT.image:''); }
    return view;
  }
  // "Alert card" layers: the alerts page drawing one themed card in each
  // (alerts.html ?embed). Still while the Studio edits, played in with an
  // alert, sent out as it ends; blank otherwise.
  var cardEls=[].slice.call(document.querySelectorAll('iframe.ic-card')), cardMode='', alertOn=false;
  function alertCard(){
    var a=ALERT||SAMPLE_ALERT;
    return {title:a.title||'', text:a.text||'', message:a.message||'', image:a.image!=null?a.image:(PREVIEW?SAMPLE_ALERT.image:''), color:a.color||''};
  }
  function postCards(mode,force){
    if(!cardEls.length||(mode===cardMode&&!force)) return;
    cardMode=mode;
    cardEls.forEach(function(f){
      var look={}; try{ look=JSON.parse(f.getAttribute('data-look')||'{}'); }catch(_){}
      try{ f.contentWindow.postMessage({__ic_card:{mode:mode,look:look,card:alertCard(),ms:ALERT&&ALERT.ms}},'*'); }catch(_){}
    });
  }
  cardEls.forEach(function(f){ f.addEventListener('load',function(){ cardMode=''; if(alertOn) postCards('play',true); else if(PREVIEW) postCards('still'); }); });
  // Where the layers are on the canvas: their laid-out boxes, not where a
  // motion has them this frame. Null with none.
  function layersBox(){
    var x0=Infinity, y0=Infinity, x1=-Infinity, y1=-Infinity;
    document.querySelectorAll('.icw').forEach(function(el){
      if(!el.offsetWidth&&!el.offsetHeight) return;
      x0=Math.min(x0,el.offsetLeft); y0=Math.min(y0,el.offsetTop);
      x1=Math.max(x1,el.offsetLeft+el.offsetWidth); y1=Math.max(y1,el.offsetTop+el.offsetHeight);
    });
    return x1>x0&&y1>y0 ? {x:x0, y:y0, w:x1-x0, h:y1-y0} : null;
  }
  function playAlert(a){
    ALERT=a; clearTimeout(alertT); alertOn=true;
    // Its own accent recolours what the design paints in its accent; with
    // no picture, the layers that make room for one close the gap.
    if(/^#[0-9a-f]{6}$/i.test(a.color||'')) body.style.setProperty('--ic-accent',a.color); else body.style.removeProperty('--ic-accent');
    body.toggleAttribute('data-nopic', !alertCard().image);
    body.toggleAttribute('data-nomsg', !a.message);
    forced='active'; body.removeAttribute('data-state'); void body.offsetWidth; lastHideKey='';
    render(window.__ic_last||{});
    postCards('play',true);
    // The page around plays "The design's sound": the design's own, or its
    // first card's theme's. A preview there shows just where its layers are.
    var first=cardEls[0], theme='';
    if(first){ try{ theme=JSON.parse(first.getAttribute('data-look')||'{}').theme||'island'; }catch(_){} }
    try{ parent.postMessage({__ic_design:{theme:theme, sound:body.getAttribute('data-alert-sound')||'', box:layersBox()}},'*'); }catch(_){}
    alertT=setTimeout(function(){ alertOn=false; postCards('out',true); forced='idle'; render(window.__ic_last||{}); },Math.max(1000,+a.ms||6000));
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
    if(SELF&&s.saved){
      var n=s.saved[SELF]||0;
      if(SAVED===null) SAVED=n; else if(n!==SAVED){ location.reload(); return; }
    }
    var st=forced||deriveState(s);
    if(s.goals) GOALS=s.goals;
    if(s.timers) TIMERS=s.timers;
    var view=buildView(s,st);
    window.__ic_last=s;
    var entered=body.getAttribute('data-state')!==st;
    if(entered){ body.setAttribute('data-state',st); }
    applyText(st,view); // every tick so {{vars}} stay live
    if(entered) textFx(st);
    var key=st+'|'+(window.__ic_noHide?'1':'0');
    if(key!==lastHideKey){ lastHideKey=key; applyHides(st); }
    curState=st;
    applyData(s);
    if(goalEls.length) paintGoals();
    if(PREVIEW&&!alertOn) postCards('still');
    tick(false);
    pumpCustom(view,view);
  }

  function buildView(s,st){
    return alertView(trackerView(viewOf(s,st)));
  }
  function viewOf(s,st){
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
      ingest:!!s.ingest_alive,
      goals:GOALS, timers:TIMERS
    };
  }

  // Sim override from the Studio canvas (inert in OBS). noHide keeps
  // auto-hide widgets visible while the user edits a state.
  window.addEventListener('message',function(ev){
    var d=ev.data; if(!d) return;
    // From this app only: a saved overlay is sandboxed (its own origin is
    // opaque, its address is the app's), the Studio's canvas is a srcdoc
    // (no address, the app's origin).
    if(d.__ic_alert&&ALERTS&&(ev.origin===location.origin||ev.origin===window.origin)){ playAlert(d.__ic_alert); return; }
    if(d.__ic_sim){
      PREVIEW=true;
      window.__ic_noHide=!!d.noHide;
      // An alert design keeps what it is doing (an alert playing) unless told.
      forced=d.forceState||(ALERTS?forced||'idle':null);
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
      n.textContent=tpl.replace(/\{\{([\w.]+)\}\}/g,function(_,k){return (s[k]!=null)?s[k]:'';});
    });
  }
  // Wrap any text node containing {{ }} once, on load.
  window.addEventListener('DOMContentLoaded',function(){
    var walk=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT,null);
    var hits=[],n; while(n=walk.nextNode()){ if(/\{\{[\w.]+\}\}/.test(n.nodeValue)) hits.push(n); }
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

  // A script as an overlay carries it: no comment lines, no indentation.
  // Line breaks stay, so the script reads (and runs) as written.
  function compactJs(js) {
    return js.split('\n').map(function (line) { return line.trim(); })
      .filter(function (line) { return line && line.slice(0, 2) !== '//'; }).join('\n');
  }
  const RUNTIME_SHIPPED = compactJs(RUNTIME_JS);
  const BRIDGE_SHIPPED = compactJs(CUSTOM_BRIDGE);
  // This file's fingerprint (FNV-1a over its own source), written into
  // every overlay it bakes: a saved overlay with another one was baked by
  // an older Studio, and the dashboard bakes it again (Studio.upgradeSaved).
  const BUILD = (function () {
    const s = icOverlayModule.toString();
    let h = 0x811c9dc5;
    for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
    return ('0000000' + h.toString(16)).slice(-8);
  })();

  // ---- bake(doc) -> self-contained static HTML string ----

  function bake(doc) {
    doc = normalize(doc);
    const usedAnim = {};
    let widgetCssAll = '';
    let bodyHtml = '';

    doc.widgets.forEach(function (w, idx) {
      if (!w.id) w.id = 'w' + idx;
      widgetCssAll += widgetCss(w, doc.theme);
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
      let dataAttrs = 'data-kind="' + esc(w.kind) + '"';
      // LiquidFill fixed level (vs the default buffer-following fill).
      if (w.kind === 'LiquidFill' && w.level != null && w.level !== 'buffer') {
        dataAttrs += ' data-level="' + r(Math.max(0, Math.min(100, +w.level))) + '"';
      }
      if (w.kind === 'DelayReadout') dataAttrs += ' data-bind="' + esc(w.bind || 'target_delay_ms') + '"';
      if (w.kind === 'Goal') {
        dataAttrs += ' data-goal="' + esc(w.goal || '') + '"';
        if (w.smooth != null) style += '--ic-sm:' + r(Math.max(0, +w.smooth)) + 's;';
      }
      if (w.kind === 'Timer') dataAttrs += ' data-timer="' + esc(w.timer || '') + '" data-end="' + esc(w.end || '') + '"';
      if (w.kind === 'BitrateMeter') {
        dataAttrs += ' data-bind="' + esc(w.bind || 'bitrate_kbps') + '"';
        if (w.smooth != null) dataAttrs += ' data-smooth="' + r(Math.max(0, Math.min(0.95, w.smooth))) + '"';
      }
      // CutCounter is intentionally excluded: its label is the live count
      // (driven by applyData via data-tpl), so per-state text overrides
      // would fight the counter. The rest take per-state text overrides.
      if (w.kind === 'Text' && w.fit) dataAttrs += ' data-fit';
      const hasText = WORD_KINDS.indexOf(w.kind) >= 0;
      if (hasText) {
        dataAttrs += ' data-has-txt data-txt-base="' + esc(shownText(w)) + '"';
        STATES.forEach(function (st) {
          const sp = (w.states && w.states[st.key]) || {};
          if (sp.text != null) dataAttrs += ' data-txt-' + st.key + '="' + esc(sp.text) + '"';
          // Words that decode or type themselves in as this state begins:
          // how, for how long, after how long (the runtime's textFx). Not
          // in a state that hides them: an alert's words would scramble or
          // go blank as it leaves.
          const said = sp.visible !== false && animsForState(w, w.always ? {} : sp, st.key).filter(function (a) { return a && (a.style === 'decode' || a.style === 'typewriter'); })[0];
          if (said) dataAttrs += ' data-txfx-' + st.key + '="' + said.style + ':' + Math.round(+said.duration_ms || 600) + ':' + Math.round(+said.delay_ms || 0) + '"';
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

    // A doc read back from a file in the overlays folder is anyone's: its
    // colours, fonts and easings all land in this stylesheet, and the
    // Studio previews it on the dashboard's own origin. None may close the
    // <style> element: `\3c ` is CSS's own escape for `<`, so a real `<`
    // inside a CSS string still reads as one.
    // The overlay's own CSS comes last, so it wins over everything above.
    const css = (themeCss(doc.theme) + '\n' + BASE_CSS + '\n' + animCss + widgetCssAll + '\n' + (doc.css || ''))
      .split('<').join('\\3c ');
    const families = doc.widgets.map(function (w) { return w.font_family; })
      .filter(function (name, i, all) { return FONTS[name] != null && all.indexOf(name) === i; });
    // The editable source doc rides along inside an HTML comment, URL-
    // encoded so its payload can never contain "-->". OBS ignores it; the
    // Studio extracts it on load via readDoc() to re-edit. One file does
    // double duty: lean live overlay + its own editable source.
    const docComment = '<!--ic-doc:' + encodeURIComponent(JSON.stringify(doc)) + '-->';
    return '<!doctype html>\n' + docComment +
      '\n<!-- Baked by InstantClone Overlay Studio. Edit in the dashboard, not here. -->\n' +
      '<html><head><meta charset="utf-8"><meta name="ic-kind" content="' + docPurpose(doc) + '"><meta name="ic-build" content="' + BUILD + '"><title>' + esc(doc.name) + '</title>\n' + fontLink(families) +
      '<style>\n' + css + '\n</style></head>\n<body data-state="idle" data-purpose="' + docPurpose(doc) + '"' +
      (docPurpose(doc) === 'alerts' && doc.sound ? ' data-alert-sound="' + esc(doc.sound) + '"' : '') + '>\n' + bodyHtml + '\n' +
      '<script>' + RUNTIME_SHIPPED + '</' + 'script>\n</body></html>\n';
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
      '</style></head><body>' + (c.html || '') + BRIDGE_SHIPPED +
      '<script>try{\n' + (c.js || '') + '\n}catch(e){console.error(e)}</' + 'script>' +
      '</body></html>';
  }

  function normalize(doc) {
    const d = JSON.parse(JSON.stringify(doc || {}));
    if (!d.name) d.name = 'Untitled overlay';
    if (!d.canvas) d.canvas = { w: 1920, h: 1080, safe_area: true };
    d.theme = Object.assign(defaultTheme(), d.theme || {});
    if (!Array.isArray(d.widgets)) d.widgets = [];
    // Only the kinds the Studio knows: a made-up one has no CSS or markup,
    // and its name would land in the page as is.
    d.widgets = d.widgets.filter(function (w) {
      return w && KINDS.some(function (k) { return k.key === w.kind; });
    });
    d.widgets.forEach(function (w) {
      if (!w.base) w.base = { color: '#5ac8fa', opacity: 1, text: null, anim: { style: 'fade', duration_ms: 400, easing: 'ease-out', delay_ms: 0 } };
      if (!w.states) w.states = {};
      if (!w.offset) w.offset = { x: 32, y: 32 };
      if (!w.size) w.size = { w: 200, h: 80 };
      upgradeWidget(w);
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
    return ANIMS.filter(function (a) { return !a.words || WORD_KINDS.indexOf(kind) >= 0; });
  }

  // ---- public API ----
  root.ICOverlay = {
    PRESETS: PRESETS,
    bake: bake, readDoc: readDoc,
    STATES: STATES, ANCHORS: ANCHORS, ANIMS: ANIMS, EASINGS: EASINGS, animsFor: animsFor,
    KINDS: KINDS, BINDINGS: BINDINGS, ICONS: ICONS, SHAPES: SHAPES, FONTS: Object.keys(FONTS), SYSTEM_FONT: SYSTEM_FONT,
    defaultDoc: defaultDoc, defaultWidget: defaultWidget, defaultCustom: defaultCustom, trackerDoc: trackerDoc,
    defaultTheme: defaultTheme, iconSvg: iconSvg, upgradeDoc: upgradeDoc, docPurpose: docPurpose, oneLook: oneLook,
    alertDoc: alertDoc, alertPreset: alertPreset, alertCardDoc: alertCardDoc, alertLookDoc: alertLookDoc, ALERT_THEMES: ALERT_THEMES,
    BUILD: BUILD
  };

})(typeof window !== 'undefined' ? window : this);
