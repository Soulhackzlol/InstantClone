// - VOD audio guide ----------------------------------------------------------
// Keeping music out of the Twitch VOD, as three steps a newcomer can follow
// without reading ahead: get the script, add it to OBS, keep the music off
// track 2. One step is lit at a time; finishing it ticks it and the trail
// flies to the next. Every place that offers it (the wizard's Twitch extras,
// System > Twitch & OBS, a Twitch destination) has a [data-vod-guide] host
// and they share the progress, kept in this browser.
const VOD_GUIDE_KEY = 'instantclone_vod_guide';
const VOD_GUIDE_STEPS = ['get', 'add', 'route'];
const VOD_GUIDE_HELP = 'https://www.elgato.com/explorer/products/stream-deck/how-to-set-up-a-twitch-vod-track-in-obs/';

function vodGuideDone(){
  try { return new Set(JSON.parse(localStorage.getItem(VOD_GUIDE_KEY) || '[]')); } catch(_){ return new Set(); }
}
function vodGuideSave(done){
  try { localStorage.setItem(VOD_GUIDE_KEY, JSON.stringify([...done])); } catch(_){}
}

// The mixer's tracks as OBS shows them in Advanced Audio Properties: every
// source on 1 and 2, the music on 1 only.
function vodTracksHtml(){
  const row = (name, two, note) => `<div class="vg-row${two ? '' : ' off'}"><span class="vg-src">${name}</span>
    <span class="vg-box on" aria-label="Track 1 on">1</span><span class="vg-box${two ? ' on' : ''}" aria-label="${two ? 'Track 2 on' : 'Track 2 off'}">2</span>${note ? `<span class="vg-note">${note}</span>` : ''}</div>`;
  return `<div class="vg-tracks" aria-label="Advanced Audio Properties, tracks">
    <div class="vg-row vg-head"><span class="vg-src">Source</span><span>Tracks</span></div>
    ${row('Mic', true)}${row('Game', true)}${row('Music', false, 'live only')}</div>`;
}

function vodGuideHtml(done){
  const next = VOD_GUIDE_STEPS.find(s => !done.has(s));
  const step = (id, n, title, body, acts) => {
    const state = done.has(id) ? 'done' : id === next ? 'now' : 'later';
    return `<li class="vg-step" data-step="${id}" data-state="${state}">
      <span class="vg-num" aria-hidden="true">${state === 'done' ? '✓' : n}</span>
      <div class="vg-main"><div class="vg-t">${title}</div>
        <div class="vg-body"><div class="vg-in">${body}<div class="vg-acts">${acts}</div></div></div></div></li>`;
  };
  const doneBtn = id => `<button type="button" class="ic-btn ic-btn-ghost ig-small" data-vg="${id}">Done</button>`;
  return `<ol class="vg-steps">
    ${step('get', 1, 'Download the script',
      `<p>A tiny OBS script that adds the VOD's second audio track. Save it where you'll find it, like OBS's scripts folder.</p>`,
      `<button type="button" class="ic-btn ic-btn-primary ig-small" data-vg="get">Download the script</button><span class="muted vg-msg"></span>`)}
    ${step('add', 2, 'Add it to OBS',
      `<p>In OBS: <b>Tools &rarr; Scripts</b>, press <b>+</b> and pick <code class="mono">optional-vod-unlocker.lua</code>. Once is enough.</p>`,
      doneBtn('add'))}
    ${step('route', 3, 'Keep the music off the VOD',
      `<p>In OBS: <b>Edit &rarr; Advanced Audio Properties</b>. Tick track <b>2</b> on every source except the music. Then, in the script, set <b>VOD audio track</b> to <b>2</b>.</p>
       ${vodTracksHtml()}
       <a class="vg-help" href="${VOD_GUIDE_HELP}" target="_blank" rel="noopener">A guide with pictures &rarr;</a>`,
      doneBtn('route'))}
  </ol>
  <div class="vg-all" ${next ? 'hidden' : ''}><span>✓</span> All set: your VOD gets every sound except the music.
    <button type="button" class="vg-again" data-vg="again">Show the steps again</button></div>`;
}

function renderVodGuides(){
  const done = vodGuideDone();
  document.querySelectorAll('[data-vod-guide]').forEach(host => { host.innerHTML = vodGuideHtml(done); });
}

// A step done: ticked everywhere, and the trail flies from it to the next.
function finishVodStep(host, id){
  const done = vodGuideDone();
  done.add(id);
  vodGuideSave(done);
  // Measured before the redraw replaces it: a removed element has no box.
  const step = host.querySelector(`.vg-step[data-step="${id}"] .vg-num`);
  const from = step && step.getBoundingClientRect();
  renderVodGuides();
  const next = host.querySelector('.vg-step[data-state="now"] .vg-num') || host.querySelector('.vg-all');
  flyTrail(from, next, () => next && next.animate([{transform:'scale(1)'}, {transform:'scale(1.25)'}, {transform:'scale(1)'}],
    {duration:360, easing:'cubic-bezier(.2,.7,.2,1)'}));
}

document.addEventListener('click', async e => {
  const b = e.target.closest('[data-vg]');
  const host = b && b.closest('[data-vod-guide]');
  if (!host) return;
  const id = b.dataset.vg;
  if (id === 'again'){ vodGuideSave(new Set()); renderVodGuides(); return; }
  if (id === 'get' && !(await installVodScript(b, b.parentElement.querySelector('.vg-msg')))) return;
  finishVodStep(host, id);
});
renderVodGuides();
