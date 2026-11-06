// ---- An alert's look, picked: the editor's chips and the builder share
// these. A row of segments (one picked), as the editor's options use.
function segHtml(items, cur, act){
  return `<div class="sub-tabs ig-seg">${items.map(([id, label]) =>
    `<button class="sub-tab${id === cur ? ' on' : ''}" data-act="${act}" data-v="${esc(id)}" aria-pressed="${id === cur}">${esc(label)}</button>`).join('')}${SEG_IND}</div>`;
}
// The accent swatches: the design's own first, then the palette and any
// colour.
function accentPickerHtml(step){
  const mine = /^#[0-9a-f]{6}$/i.test(step.params.color || '') ? step.params.color.toLowerCase() : '';
  const shown = alertColor(mine, lookOf());
  const known = !mine || CARD_COLORS.some(([c]) => c === mine);
  const swatch = (c, label, style) => `<button class="ig-swatch${c === mine ? ' on' : ''}" style="--sw:${style}" data-act="set-color" data-v="${c}"
    role="radio" aria-checked="${c === mine}" aria-label="${label}" title="${label}"></button>`;
  const swatches = [swatch('', 'From the design', alertColor('', lookOf()))].concat(CARD_COLORS.map(([c, label]) => swatch(c, label, c))).join('');
  return `<div class="ig-swatches"><span class="ig-swatch-set" role="radiogroup" aria-label="Alert accent">${swatches}</span>
    <label class="ig-swatch custom${known ? '' : ' on'}" title="Any color"><input type="color" data-bind="color" value="${esc(shown)}" aria-label="Any color (now ${esc(shown)})"></label></div>`;
}
// The sounds to pick from, its volume, and Play.
function soundControlsHtml(step){
  const sound = step.params.sound || '';
  const volume = step.params.volume === undefined || step.params.volume === '' ? 70 : +step.params.volume;
  return `${soundPickerHtml(sound, 'pick-sound', true)}
    <div class="ig-volume"><label for="ig-vol">Volume</label><input id="ig-vol" type="range" min="0" max="100" step="5" data-bind="volume" value="${volume}">
      <span class="mono">${volume}%</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="sound-play" ${sound ? '' : 'disabled'}>${svg('play', ' class="ig-btn-ic"')}Play</button></div>`;
}
// The alert on the real page, in the page's look but in the middle of
// the box, so a short stage shows it whole. `replay` 'full' plays it
// coming in and going out (alerts.html showDemo).
function alertStage(step, h, replay){
  const frame = q('.ig-ed-stage .ig-stage-frame');
  if (!frame) return;
  const look = Object.assign({}, lookOf(), {position:'middle', align:'center'});
  stageShow(frame, frame.parentElement.clientWidth / 0.72, {look, card:alertCardOf(step, h), replay, sound:!!replay});
}
// A click on those pieces, for the editor and the builder alike: `step` is
// the alert it changes; `owner` redraws (changed) and says whether it
// still shows that step (editing), as an upload or a design takes a moment.
function alertAct(a, el, step, h, owner){
  const v = el.dataset.v;
  switch (a){
    case 'set-color': step.params.color = v; owner.changed(); return true;
    case 'pick-sound':
      step.params.sound = v;
      owner.changed();
      if (v){ playAlertSound(alertCardOf(step, h)); showPlaying('pick-sound', v); }
      return true;
    case 'pick-picture': step.params.image = v; owner.changed(); return true;
    // A ready-made design is saved first; it plays in as it is picked.
    case 'pick-design':
      Stage.ensureDesign(el.dataset.slug).then(ok => {
        if (!ok || !owner.editing(step)) return;
        step.params.design = el.dataset.slug;
        owner.changed();
        alertStage(step, h, 'full');
      });
      return true;
    case 'media-upload':
      uploadMedia(el.dataset.kind).then(ref => {
        if (!ref || !owner.editing(step)) return;
        step.params[el.dataset.kind === 'sound' ? 'sound' : 'image'] = ref;
        owner.changed();
        if (el.dataset.kind === 'sound') playAlertSound(alertCardOf(step, h));
      });
      return true;
    case 'set-seconds': step.params.seconds = v; owner.changed(); return true;
    case 'sound-play': playAlertSound(alertCardOf(step, h)); return true;
    case 'alert-replay': alertStage(step, h, 'full'); return true;
    case 'alert-show': showAlertNow(step, h, el); return true;
  }
  return false;
}

// What an alert step's card says under its words: its design, its own
// accent (the design's needs no dot) and its sound.
function alertMetaHtml(step){
  const p = step.params;
  const dot = /^#[0-9a-f]{6}$/i.test(p.color || '') ? `<i style="--sw:${esc(p.color)}" aria-hidden="true"></i>` : '';
  return `<span class="ig-step-meta">${dot}${esc(designLabel(p.design))} · ${esc(soundLabel(p.sound))}</span>`;
}

function alertCardOf(step, h){
  const p = step.params, vars = sampleMap(h);
  return {title:renderSample(p.title || '', vars), text:renderSample(p.text || '', vars), message:renderSample(p.message || '', vars), seconds:p.seconds || '6',
    color:p.color || '', sound:p.sound || '', volume:p.volume || '', image:p.image || '', design:p.design || ''};
}

// The canvas overlays are laid out for: the Overlay tab shows each one at
// this size, scaled into its preview, as OBS does.
const OVERLAY_CANVAS = 1920;
// Fit a real alerts page (`/alerts?demo`, laid out `canvas` px wide) into
// its box and show it `payload` ({look, card, replay, sound}); a frame
// still loading gets it once it has. Kept as properties, not attributes:
// a redraw resets attributes, and calls this again.
function stageShow(frame, canvas, payload){
  if (!frame) return;
  frame.icCanvas = canvas;
  frame.icPayload = payload;
  stageFit(frame);
  if (!frame.icWired){
    frame.icWired = true;
    frame.addEventListener('load', () => { frame.icReady = true; stagePost(frame, frame.icPayload); });
    // A box that grows or settles (the modal opening) is fitted again.
    new ResizeObserver(() => stageFit(frame)).observe(frame.parentElement);
  }
  if (frame.icReady) stagePost(frame, payload);
}
function stageFit(frame){
  const box = frame.parentElement, s = box && box.clientWidth / frame.icCanvas;
  if (!s) return;
  frame.style.width = frame.icCanvas + 'px';
  frame.style.height = Math.ceil(box.clientHeight / s) + 'px';
  frame.style.transform = `scale(${s})`;
}
function stagePost(frame, payload){
  if (frame.contentWindow && payload) frame.contentWindow.postMessage({instantcloneAlert:payload}, location.origin);
}

// The sound button just pressed shows it playing for a moment.
function showPlaying(act, v){
  const b = Modal.form && [...Modal.form.querySelectorAll(`[data-act="${act}"]`)].find(x => x.dataset.v === v);
  if (!b) return;
  b.classList.remove('ig-playing');
  void b.offsetWidth;
  b.classList.add('ig-playing');
  clearTimeout(b.icPlaying);
  b.icPlaying = setTimeout(() => b.classList.remove('ig-playing'), 1200);
}

// Play a sound as the alerts page would, from a hidden page of its own.
function playAlertSound(card, look){
  let frame = document.getElementById('ig-sound-frame');
  if (!frame){
    frame = document.createElement('iframe');
    frame.id = 'ig-sound-frame';
    frame.src = '/alerts?demo';
    frame.allow = 'autoplay';
    frame.hidden = true;
    document.body.appendChild(frame);
  }
  if (!frame.icWired){
    frame.icWired = true;
    frame.addEventListener('load', () => { frame.icReady = true; stagePost(frame, frame.icPayload); }, {once:true});
  }
  frame.icPayload = {look:look || lookOf(), card, sound:true};
  if (frame.icReady) stagePost(frame, frame.icPayload);
}

// A sound to pick: for an alert, none or its design's; for the theme, its
// own. Then the built-in ones, the uploads (each with its delete), one
// more upload, and a link. Picking one plays it.
function soundPickerHtml(current, act, forAlert){
  const look = lookOf(), own = THEMES[themeOf(look)];
  const opt = (id, label) => `<button class="ig-sound${id === current ? ' on' : ''}" data-act="${act}" data-v="${esc(id)}" aria-pressed="${id === current}">
    ${id ? svg('play', ' class="ig-btn-ic"') : ''}${esc(label)}</button>`;
  const items = forAlert
    ? [opt('', 'None'), opt('theme', 'The design\'s sound')]
    : [opt('', `${own.name}'s own (${soundLabel(own.sound)})`)];
  ALERT_SOUNDS.forEach(([id, label]) => items.push(opt(id, label)));
  mediaOf('sound').forEach(m => items.push(`<span class="ig-media-item">${opt('media:' + m.name, m.name)}${mediaDelHtml(m.name)}</span>`));
  const link = WEB_LINK.test(current) ? current : '';
  return `<div class="ig-sounds">${items.join('')}
      <button class="ig-sound ig-upload" data-act="media-upload" data-kind="sound">${svg('plus', ' class="ig-btn-ic"')}Upload a sound</button></div>
    <details class="ig-more-fields"${link ? ' open' : ''}><summary>Or a link to a sound file</summary>
      <input class="ic-input mono" data-bind="sound-link" value="${esc(link)}" placeholder="https://…/ding.mp3" spellcheck="false" aria-label="Link to a sound file"></details>`;
}

// A picture to pick: none, the uploads, one more upload, or a link.
function picturePickerHtml(current, act){
  const tile = (ref, inner, label) => `<button class="ig-pic${ref === current ? ' on' : ''}" data-act="${act}" data-v="${esc(ref)}"
    aria-pressed="${ref === current}" aria-label="${esc(label)}" title="${esc(label)}">${inner}</button>`;
  const items = [tile('', '<span>None</span>', 'No picture')];
  mediaOf('picture').forEach(m => items.push(`<span class="ig-media-item">${tile('media:' + m.name,
    `<img src="${esc(mediaSrc('media:' + m.name))}" alt="" loading="lazy">`, m.name)}${mediaDelHtml(m.name)}</span>`));
  const link = WEB_LINK.test(current) ? current : '';
  return `<div class="ig-pics">${items.join('')}
      <button class="ig-pic ig-upload" data-act="media-upload" data-kind="picture">${svg('plus')}<span>Upload</span></button></div>
    <details class="ig-more-fields"${link || (current && !MEDIA_REF.test(current)) ? ' open' : ''}><summary>Or a link to a picture or GIF</summary>
      <input class="ic-input mono" data-bind="image-link" value="${esc(MEDIA_REF.test(current) ? '' : current)}" placeholder="https://…/party.gif" spellcheck="false" aria-label="Link to a picture"></details>
    ${current && !MEDIA_REF.test(current) && !WEB_LINK.test(current) ? warnHtml('This isn\'t a web link: it has to start with https://') : ''}`;
}

function mediaDelHtml(name){
  return `<button class="ig-media-x" data-act="media-del" data-name="${esc(name)}" aria-label="Delete ${esc(name)}" title="Delete ${esc(name)}">${svg('x')}</button>`;
}

// The newest uploads and where the page is open, without touching the
// look being edited.
async function refreshAlerts(){
  const a = await api('/integrations/activity');
  if (a && a.alerts) S.data.alerts = Object.assign({}, a.alerts, {look:lookOf()});
}

// Pick a file and upload it. Resolves to its `media:` name, or null. A
// closed file dialog never resolves, which is fine: nothing waits on it.
function uploadMedia(kind){
  return new Promise(resolve => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = kind === 'sound' ? '.mp3,.ogg,.wav,.m4a,.webm' : '.png,.jpg,.jpeg,.gif,.webp';
    input.onchange = async () => {
      const file = input.files[0];
      if (!file) return resolve(null);
      if (file.size > 10 * 1024 * 1024){ toast('That file is over 10 MB', 'err'); return resolve(null); }
      const data = await new Promise(done => {
        const reader = new FileReader();
        reader.onload = () => done(String(reader.result).split(',')[1] || '');
        reader.onerror = () => done('');
        reader.readAsDataURL(file);
      });
      // base64url: the alphabet recipes use too.
      const r = await api('/integrations/alerts/media/upload', {name:file.name, data:data.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')});
      if (!r.ok){ toast(r.error || 'The upload didn\'t work', 'err', 6000); return resolve(null); }
      await refreshAlerts();
      toast(`Uploaded ${r.name}`, 'ok');
      resolve('media:' + r.name);
    };
    input.click();
  });
}

async function deleteMedia(el){
  if (!armConfirm(el, 'Delete?')) return;
  const name = el.dataset.name;
  const r = await api('/integrations/alerts/media/delete', {name});
  if (!r.ok){ toast(r.error || 'Couldn\'t delete it', 'err'); return; }
  await refreshAlerts();
  toast(`Deleted ${name}. Alerts that used it go without until you pick another.`, 'info', 5000);
  const o = OWNERS[Modal.kind];
  if (o && o.render) o.render();
}

// Put this alert on the alerts page for real, with sample values.
async function showAlertNow(step, h, btn){
  await busy(btn, async () => {
    const r = await api('/integrations/alerts/test', alertCardOf(step, h));
    if (!r.ok){ toast(r.error || 'Could not show it', 'err'); return; }
    const a = S.data.alerts || {};
    toast(a.in_obs ? 'It\'s on your alerts page in OBS now' : 'Sent, but your alerts page isn\'t in OBS: add it to OBS to see it',
      a.in_obs ? 'ok' : 'info', 5000);
  });
}

// A browser source for an overlay, in the scene on air, in one click.
async function addOverlayToObs(btn, slug, name){
  await busy(btn, async () => {
    const r = await api('/integrations/overlays/add-to-obs', {slug, name});
    const ok = r && r.ok !== false;
    toast(ok ? 'Added to the scene on air in OBS' : (r && r.error) || 'OBS didn\'t add it', ok ? 'ok' : 'err', 6000);
  });
}
async function addAlertsToObs(btn){
  await busy(btn, async () => {
    const r = await api('/integrations/alerts/add-to-obs', {});
    if (!r.ok){ toast(r.error || 'OBS didn\'t add it', 'err', 7000); return; }
    toast('Added to the scene on air in OBS', 'ok');
    // OBS opens the page a moment later: the pill catches up then.
    setTimeout(() => poll(), 1500);
  });
}

// System > Twitch & OBS, with the Twitch login app section open.
function openTwitchAppSettings(){
  Modal.close(true);
  if (typeof showTab === 'function') showTab('system');
  if (typeof showSubTab === 'function') showSubTab('system', 'twitch');
  const box = document.getElementById('sys-twitch-app');
  if (!box) return;
  box.open = true;
  // After the sub-tab has laid out and started its entrance.
  setTimeout(() => {
    box.scrollIntoView({block:'start', behavior:reduced() ? 'auto' : 'smooth'});
    const field = document.getElementById('s-twitch-client');
    if (field) setTimeout(() => field.focus({preventScroll:true}), 300);
  }, 80);
}

function focusField(bind){
  const f = q(`[data-bind="${bind}"]`);
  if (f) f.focus({preventScroll:true});
}

