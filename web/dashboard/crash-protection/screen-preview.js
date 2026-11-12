// ── Screen previews (Crash protection, Delay transition) ─────────────────
// Both screens come from /crash-protection/preview, drawn by the Rust
// renderer that also encodes the stream. Unsaved edits ride along in the
// query, so every picture follows each change before Save. Flat styles
// preview as one strip of stills the CSS steps through; soft styles as
// their loop encoded the way the stream carries it.

// Frames in the preview strip; the CSS animation steps through the same 24.
// Every flat style moves in 6, 8 or 12 steps per loop, and 24 is a multiple
// of each, so the preview shows every step at the moment the stream does.
const SCREEN_PREVIEW_FRAMES = 24;
const screenStill = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;

function screenPreviewUrl(params){
  return '/crash-protection/preview?' + new URLSearchParams(params);
}

// Fetch a picture whole and hand it over once it has arrived, so nothing
// flashes empty or half-drawn while it loads.
function screenFetch(url){
  return fetch(url).then(response => response.ok ? response.blob() : Promise.reject(response.status));
}

// The live preview of one editor: its frame, the still strip and the video.
// It plays only while it can be seen: scrolled away it rests, so an open
// System pane costs nothing for a preview nobody is watching.
function screenPreview({frame, strip, video}){
  frame = $(frame); strip = $(strip); video = $(video);
  let generation = 0, stripUrl = '', videoUrl = '', isSeen = true;
  const play = () => {
    if (!video.hidden && isSeen && !screenStill) video.play().catch(() => {});
    else video.pause();
  };
  const showVideo = show => { video.hidden = !show; strip.hidden = show; play(); };
  // A load that lands after a newer one is dropped.
  const load = (url, use) => {
    const mine = ++generation;
    screenFetch(url).then(blob => { if (mine === generation) use(URL.createObjectURL(blob)); }).catch(() => {});
  };
  if ('IntersectionObserver' in window){
    new IntersectionObserver(entries => entries.forEach(e => strip.classList.toggle('resting', !e.isIntersecting)))
      .observe(strip);
    new IntersectionObserver(entries => entries.forEach(e => { isSeen = e.isIntersecting; play(); }))
      .observe(frame);
  }
  return {
    // `params` are crash_protection.* fields; `shape` horizontal or vertical.
    refresh(params, shape, isSoft){
      frame.classList.toggle('vertical', shape === 'vertical');
      const query = {...params, orientation: shape};
      if (isSoft){
        load(screenPreviewUrl({...query, format: 'mp4'}), url => {
          URL.revokeObjectURL(videoUrl);
          videoUrl = url;
          video.src = url;
          showVideo(true);
        });
        return;
      }
      showVideo(false);
      load(screenPreviewUrl({...query, frames: String(SCREEN_PREVIEW_FRAMES)}), url => {
        URL.revokeObjectURL(stripUrl);
        stripUrl = url;
        strip.style.backgroundImage = 'url("' + url + '")';
      });
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
  screenFetch(screenPreviewUrl({...params, orientation: shape, size: 'full', format: 'mp4'}))
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
