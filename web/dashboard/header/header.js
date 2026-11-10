// - Header chips ---------------------------─
function renderHeader(s){
  // OBS pill - plain-language state + a tooltip that tells the user what
  // to do when it's not connected yet.
  const obs = $('hdr-obs');
  obs.className = 'ic-pill ' + (s.ingest_alive ? 'live ok' : '');
  // Named after the app sending, when that's certain (see encoderApp).
  const pillName = obs.querySelector('.ic-pill-k'), app = encoderApp() || 'Stream';
  if (pillName.textContent !== app) pillName.textContent = app;
  $('hdr-obs-v').textContent = s.ingest_alive ? 'Live' : 'Offline';
  obs.title = s.ingest_alive
    ? `${app === 'Stream' ? 'Your stream' : app} is streaming to InstantClone.`
    : `Waiting for your stream. Start streaming in OBS with InstantClone as its service, or Custom with rtmp://localhost:${(lastConfig && lastConfig.ingest_port) || 1935}/live.`;
  // Egress pill - how many destinations are actually connected.
  const eg = $('hdr-egress');
  const total = s.destinations_total || 0;
  const live = s.destinations_alive || 0;
  eg.className = 'ic-pill ' + (live > 0 ? 'ok' : '');
  if (total === 0){
    $('hdr-egress-v').textContent = 'Nothing yet';
    eg.title = 'No destinations yet. Add Twitch / YouTube / Kick in the Destinations tab.';
  } else {
    $('hdr-egress-v').textContent = `${live}/${total} live`;
    eg.title = live > 0
      ? (total === 1 ? 'Your destination is connected and receiving.' : `${live} of ${total} destinations connected and receiving.`)
      : (total === 1 ? '1 destination set up, not connected yet.' : `${total} destinations set up, none connected yet.`);
  }
  // Bitrate pill: "Idle" when nothing flows, auto kbps -> Mbps otherwise.
  const kbps = s.stats.bitrate_kbps || 0;
  const ratePill = $('hdr-rate-pill');
  const rateU = $('hdr-rate-u');
  ratePill.classList.toggle('idle', kbps <= 0);
  if (kbps <= 0){
    $('hdr-rate').textContent = 'Idle';
    rateU.textContent = '';
    ratePill.classList.remove('ok');
  } else if (kbps >= 1000){
    $('hdr-rate').textContent = (kbps / 1000).toFixed(2);
    rateU.textContent = 'Mbps';
    ratePill.classList.add('ok');
  } else {
    $('hdr-rate').textContent = Math.round(kbps);
    rateU.textContent = 'kbps';
    ratePill.classList.add('ok');
  }
  // Codec chip
  const haveV = s.video_codec && s.video_codec !== '?' && s.video_codec !== '';
  const haveA = s.audio_codec && s.audio_codec !== '?' && s.audio_codec !== '';
  if (haveV || haveA){
    $('hdr-codec').hidden = false;
    $('hdr-codec-v').textContent = (haveV ? s.video_codec : '?') + ' / ' + (haveA ? s.audio_codec : '?');
  } else $('hdr-codec').hidden = true;
  $('hdr-ebroadcast').hidden = !s.multitrack_video;
  $('hdr-dualformat').hidden = !s.vertical_present;
  $('hdr-backpressure').hidden = !s.backpressure;
  renderTwitchCapPill(s);
  renderCompatWarning(s);
}

// - Encoder compatibility strip --------------------
// Dismissal is keyed by the MESSAGE, not by a plain "hidden" flag: if the
// user fixes their keyframe interval and a different problem appears, the
// strip comes back on its own. Dismissal belongs to the publishing session
// that raised it: it clears when ingest drops, so a fresh dashboard load OR
// a new stream on the same page starts clean instead of inheriting a stale
// "already dismissed" that would silence a genuinely broken new session.
let compatDismissed = '';
function renderCompatWarning(s){
  if (!s.ingest_alive) compatDismissed = '';
  const msg = s.compat_warning || '';
  const el = $('compat-warn');
  const show = msg !== '' && msg !== compatDismissed;
  animateHidden(el, !show);
  if (!show) return;
  // Skip the DOM write when the text is unchanged - this runs ~4x/s and
  // rewriting it every tick would defeat text selection mid-read.
  const t = $('compat-warn-text');
  if (t.dataset.msg !== msg){
    t.dataset.msg = msg;
    const lines = msg.split(' · ').map(part => {
      const line = document.createElement('div');
      line.className = 'cw-line';
      line.textContent = part;
      return line;
    });
    const when = document.createElement('div');
    when.className = 'cw-when';
    when.textContent = 'OBS applies a change the next time you start streaming.';
    t.replaceChildren(...lines, when);
  }
}
function dismissCompatWarning(){
  compatDismissed = $('compat-warn-text').dataset.msg || '';
  animateHidden($('compat-warn'), true);
}

// Twitch mobile-decoder warning. Twitch's transcoded quality ladder
// (1080p / 720p / 480p / 360p / 160p) is account-tier gated, NOT
// bitrate gated - non-Affiliate accounts get Source-Only regardless
// of bitrate, Affiliate / Partner get the ladder. When the stream
// IS in Source-Only mode (no ladder), every viewer has to decode
// the full source bitrate. At ≤ 8 Mbps 1080p60 H.264 most mobile
// hardware decoders cope; above ~8 Mbps they start failing - that's
// the original beta.4 symptom (Error #1000 on PC, black screen with
// audio on mobile). The 8000 cap matches what the user verified is
// the "sweet spot" on an actual no-Affiliate alt: 8k Source-Only is
// watchable, 10k Source-Only is not.
//
// We can't tell from the wire whether the user's account has the
// transcoded ladder, so we warn unconditionally on Twitch + > 8 Mbps.
// Streamers with confirmed transcoded ladders can dismiss it; the
// alternative (silently letting beta.4 happen again) is worse.
//
// The check reads the video bitrate alone (viewers decode video; the
// audio and VOD tracks don't load the decoder), averaged over 10 s. The
// meter samples 1 s windows, and with a 2 s keyframe interval every other
// window holds a keyframe, so a steady 8 Mbps stream reads 7.4, 9.1,
// 7.6... and a raw threshold made the chip flicker. Showing above 8.0 and
// clearing below 7.7 keeps it steady when the average sits on the line.
const TWITCH_MOBILE_SAFE_KBPS = 8000;
const TWITCH_MOBILE_CLEAR_KBPS = 7700;
const TWITCH_CAP_AVG_MS = 10000;
let twitchCapSamples = [];
let twitchCapOn = false;
let twitchCapKbps = 0;
// Average of the last TWITCH_CAP_AVG_MS of samples, or 0 until most of a
// window has been seen, so a stream's first second can't light the chip.
function smoothedVideoKbps(s){
  const kbps = (s.stats && s.stats.video_kbps) || 0;
  if (!s.ingest_alive || kbps <= 0){ twitchCapSamples = []; return 0; }
  const now = Date.now();
  twitchCapSamples.push({t: now, kbps});
  while (now - twitchCapSamples[0].t > TWITCH_CAP_AVG_MS) twitchCapSamples.shift();
  if (now - twitchCapSamples[0].t < TWITCH_CAP_AVG_MS * 0.8) return 0;
  return twitchCapSamples.reduce((sum, x) => sum + x.kbps, 0) / twitchCapSamples.length;
}
function renderTwitchCapPill(s){
  const pill = document.getElementById('hdr-twitch-cap');
  const v = document.getElementById('hdr-twitch-cap-v');
  if (!pill || !v) return;
  // /state's destination entries carry no platform, so join on id.
  const platformOf = id => ((lastDests || []).find(x => x.id === id) || {}).platform;
  const hasTwitchDest = (s.destinations || []).some(
    d => platformOf(d.id) === 'twitch' && d.alive && d.enabled
  );
  const kbps = smoothedVideoKbps(s);
  twitchCapOn = twitchCapOn ? kbps >= TWITCH_MOBILE_CLEAR_KBPS : kbps > TWITCH_MOBILE_SAFE_KBPS;
  twitchCapKbps = kbps;
  // When Enhanced Broadcasting is active the multi-track stream
  // bypasses Twitch's account-tier source-only fallback - viewers
  // get the full transcoded ladder regardless of bitrate, so the
  // mobile-decoder warning we'd otherwise show is moot. The "⚡
  // Enhanced Broadcasting" chip next to this one already
  // communicates the EB-active state, so silently suppress this
  // warning to avoid two chips telling the user different things.
  const ebActive = !!s.multitrack_video;
  if (hasTwitchDest && twitchCapOn && !ebActive){
    v.textContent = `Twitch · mobile risk at ${(kbps/1000).toFixed(1)} Mbps`;
    pill.hidden = false;
    pill.style.cursor = 'pointer';
    pill.title = '';
    pill.onclick = openTwitchCapInfo;
  } else {
    pill.hidden = true;
    pill.onclick = null;
  }
}
function openTwitchCapInfo(){
  const at = twitchCapKbps ? `${(twitchCapKbps/1000).toFixed(1)} Mbps` : 'your current bitrate';
  toast(
    `If Twitch shows your stream as "Source-Only" (no quality ladder), viewers must decode the full ${at}. Above ~8 Mbps that's too heavy for many mobile decoders: they hit Error #1000 or black screen with audio. The transcoded ladder is account-tier gated (Affiliate / Partner), not bitrate gated. If you're not sure your account gets transcoded, drop OBS to ≤ 8 Mbps for safe mobile playback. If you know it's transcoded, ignore this - viewers will pick lower qualities themselves.`,
    'err'
  );
}

// - One-line header ------------------------------
// Wider than a phone the header is one line. When the pills don't fit, it
// tightens a step at a time (the steps are in header.css). A pill showing or
// hiding, or the window resizing, fits it again from scratch; the pills' text
// changing (four times a second while live) only ever adds a step, so a tick
// costs one measurement.
const HEADER_FIT_STEPS = 6;
const headerFit = {step:0, full:false, frame:0};
function fitHeader(full){
  const bar = $('hdr-bar'), row = bar.querySelector('.ic-header-right');
  if (full){
    for (let i = 1; i <= HEADER_FIT_STEPS; i++) bar.classList.remove('fit-' + i);
    headerFit.step = 0;
  }
  if (bar.hidden || matchMedia('(max-width:520px)').matches) return;
  while (headerFit.step < HEADER_FIT_STEPS && row.scrollWidth > row.clientWidth + 1){
    bar.classList.add('fit-' + ++headerFit.step);
  }
}
function fitHeaderSoon(full){
  headerFit.full = headerFit.full || full;
  if (headerFit.frame) return;
  headerFit.frame = requestAnimationFrame(() => {
    const fullFit = headerFit.full;
    headerFit.frame = 0;
    headerFit.full = false;
    fitHeader(fullFit);
  });
}
new ResizeObserver(() => fitHeaderSoon(true)).observe($('hdr-bar'));
new MutationObserver(changes => fitHeaderSoon(changes.some(c => c.type === 'attributes')))
  .observe($('hdr-bar').querySelector('.ic-header-right'),
    {subtree:true, childList:true, characterData:true, attributes:true, attributeFilter:['hidden']});
