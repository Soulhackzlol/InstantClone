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

