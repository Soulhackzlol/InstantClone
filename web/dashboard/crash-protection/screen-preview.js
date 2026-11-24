// ── Screen previews (Crash protection, Delay transition) ─────────────────
// Both screens come from /crash-protection/preview, drawn by the Rust
// renderer that also encodes the stream. Unsaved edits ride along in the
// query, so every picture follows each change before Save. A preview is
// the loop encoded the way the stream carries it, at the stream's 30 fps.

const screenStill = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;

function screenPreviewUrl(params){
  return '/crash-protection/preview?' + new URLSearchParams(params);
}

// Fetch a picture whole and hand it over once it has arrived, so nothing
// flashes empty or half-drawn while it loads. A 204 means a newer preview
// from the same editor replaced this one before it was drawn.
function screenFetch(url){
  return fetch(url).then(response => response.ok && response.status !== 204
    ? response.blob() : Promise.reject(response.status));
}

// The live preview of one editor: its frame and the video in it. It plays
// only while it can be seen: scrolled away it rests, so an open System
// pane costs nothing for a preview nobody is watching. With reduced
// motion it rests on its first frame.
// `slot` names the editor, so the server drops a preview of it that a
// newer one replaced (crash or transition).
function screenPreview({frame, video, slot}){
  frame = $(frame); video = $(video);
  let generation = 0, videoUrl = '', isSeen = true;
  const play = () => {
    if (video.src && isSeen && !screenStill) video.play().catch(() => {});
    else video.pause();
  };
  if ('IntersectionObserver' in window){
    new IntersectionObserver(entries => entries.forEach(e => { isSeen = e.isIntersecting; play(); }))
      .observe(frame);
  }
  return {
    // `params` are crash_protection.* fields; `shape` horizontal or vertical.
    refresh(params, shape){
      frame.classList.toggle('vertical', shape === 'vertical');
      // A video that lands after a newer one is dropped.
      const mine = ++generation;
      screenFetch(screenPreviewUrl({...params, orientation: shape, format: 'mp4', slot}))
        .then(blob => {
          if (mine !== generation) return;
          URL.revokeObjectURL(videoUrl);
          videoUrl = URL.createObjectURL(blob);
          video.src = videoUrl;
          play();
        })
        .catch(() => {});
    },
    pause(){ video.pause(); },
  };
}

// One card per style in `box`, a radio group: arrow keys move the choice.
// `picked()` says which style is chosen; `pick(id)` chooses one.
function screenBuildThemes(box, themes, picked, pick){
  // Fresh cards have empty thumbnails, whatever was drawn before.
  box.dataset.drawn = '';
  box.replaceChildren(...themes.map(theme => {
    const card = document.createElement('button');
    card.type = 'button';
    card.className = 'crash-theme';
    card.setAttribute('role', 'radio');
    card.dataset.theme = theme.id;
    const thumb = document.createElement('img');
    thumb.alt = '';
    const name = document.createElement('span');
    name.textContent = theme.name;
    card.append(thumb, name);
    card.onclick = () => pick(theme.id);
    return card;
  }));
  box.onkeydown = e => {
    const step = {ArrowRight:1, ArrowDown:1, ArrowLeft:-1, ArrowUp:-1}[e.key];
    if (!step) return;
    e.preventDefault();
    const index = themes.findIndex(t => t.id === picked());
    const id = themes[(index + step + themes.length) % themes.length].id;
    pick(id);
    updateSaveBar();
    box.querySelector('[data-theme="' + id + '"]').focus();
  };
}

function screenMarkTheme(box, picked){
  box.querySelectorAll('.crash-theme').forEach(card => {
    const on = card.dataset.theme === picked;
    card.setAttribute('aria-checked', String(on));
    card.tabIndex = on ? 0 : -1;
  });
}

// Each card's thumbnail in the colours and words of `params`. Picking a
// style doesn't change them, so they reload only when those do.
function screenDrawThumbs(box, params){
  const look = {...params};
  delete look['crash_protection.theme'];
  const key = JSON.stringify(look);
  if (box.dataset.drawn === key) return;
  box.dataset.drawn = key;
  box.querySelectorAll('.crash-theme').forEach(card => {
    card.querySelector('img').src = screenPreviewUrl({...look, 'crash_protection.theme': card.dataset.theme, size: 'thumb', phase: '0.3'});
  });
}

// The accents offered as swatches beside the custom colour.
const SCREEN_ACCENTS = [['Cyan','#5ac8fa'],['Violet','#9d7bff'],['Lime','#7ee06b'],['Coral','#ff6b6b'],['Amber','#ffb73a']];

// The colour controls of the card whose ids start with `prefix`: accent
// swatches, a custom accent and a background. Each edit goes to
// `set({accent})` or `set({background})`, '' meaning the style's own.
function screenBuildColours(prefix, set){
  const box = $(prefix + '-swatches'), custom = $(prefix + '-accent').parentElement;
  SCREEN_ACCENTS.forEach(([name, hex]) => {
    const swatch = document.createElement('button');
    swatch.type = 'button';
    swatch.className = 'crash-swatch';
    swatch.style.background = hex;
    swatch.dataset.hex = hex;
    swatch.title = name;
    swatch.setAttribute('aria-label', name);
    swatch.onclick = () => set({accent: hex});
    box.insertBefore(swatch, custom);
  });
  $(prefix + '-accent').oninput = e => set({accent: e.target.value.toLowerCase()});
  $(prefix + '-bg').oninput = e => set({background: e.target.value.toLowerCase()});
  $(prefix + '-bg-reset').onclick = () => set({background: ''});
}

// Show `accent` and `background` in those controls; a background left on
// the style's own shows as `styleBackground`.
function screenRenderColours(prefix, {accent, background}, styleBackground){
  document.querySelectorAll('#' + prefix + '-swatches .crash-swatch')
    .forEach(b => b.setAttribute('aria-pressed', String(b.dataset.hex === accent)));
  $(prefix + '-accent').value = accent;
  $(prefix + '-bg').value = background || styleBackground || '#000000';
  $(prefix + '-bg-note').hidden = !!background;
  $(prefix + '-bg-reset').hidden = !background;
}

// Character counters for the `prefix`-headline and -subline fields, and
// their "Reset text" once either differs from `defaults`.
function screenRenderText(prefix, defaults){
  ['headline', 'subline'].forEach(line => {
    const input = $(prefix + '-' + line);
    const counter = $(input.id + '-count');
    counter.textContent = input.value.length + '/' + input.maxLength;
    counter.classList.toggle('full', input.value.length >= input.maxLength);
  });
  $(prefix + '-text-reset').hidden = $(prefix + '-headline').value === defaults.headline
    && $(prefix + '-subline').value === defaults.subline;
}

// ── Full-size view ───────────────────────────────────────────────────────
// The loop at the size a 1080p stream carries it (1080x1920 when vertical),
// from the stream's own encoder: what viewers get, pixel for pixel.
let screenFullGeneration = 0;
let screenFullUrl = '';
function screenOpenFull(params, shape){
  const mine = ++screenFullGeneration;
  const video = $('screen-full-video'), wait = $('screen-full-wait');
  $('screen-full-stage').classList.toggle('vertical', shape === 'vertical');
  video.hidden = true;
  wait.hidden = false;
  wait.textContent = 'Drawing it at full size…';
  $('screen-full').hidden = false;
  $('screen-full-close').focus();
  screenFetch(screenPreviewUrl({...params, orientation: shape, size: 'full', format: 'mp4', slot: 'full'}))
    .then(blob => {
      if (mine !== screenFullGeneration) return;
      URL.revokeObjectURL(screenFullUrl);
      screenFullUrl = URL.createObjectURL(blob);
      video.src = screenFullUrl;
      video.hidden = false;
      wait.hidden = true;
      video.play().catch(() => {});
    })
    .catch(() => { if (mine === screenFullGeneration) wait.textContent = 'It could not be drawn. Try again.'; });
}
function screenCloseFull(){
  ++screenFullGeneration;
  $('screen-full').hidden = true;
  const video = $('screen-full-video');
  video.pause();
  video.removeAttribute('src');
  video.load();
  URL.revokeObjectURL(screenFullUrl);
  screenFullUrl = '';
}
document.addEventListener('keydown', e => {
  if (e.key !== 'Escape' || $('screen-full').hidden) return;
  e.preventDefault();
  screenCloseFull();
});
