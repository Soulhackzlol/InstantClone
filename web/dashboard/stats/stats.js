// - Stats render ---------------------------─
function renderStats(s){
  rateHistory.push(s.stats.bitrate_kbps || 0);
  if (rateHistory.length > HISTORY_LEN) rateHistory.shift();
  cpuHistory.push(s.cpu_pct || 0);
  if (cpuHistory.length > HISTORY_LEN) cpuHistory.shift();
  // The counters tween at every frame: only while someone can see them.
  const pane = document.querySelector('.tab-pane[data-pane="stats"]');
  if (pane && !pane.hidden) paintStats(s);
}
function paintStats(s){
  // Bitrate in: count up, auto kbps -> Mbps so a live stream reads "6.20
  // Mbps" instead of "6200 kbps".
  const rkbps = s.stats.bitrate_kbps || 0;
  if (rkbps >= 1000){
    animateNumber('stat-rate', rkbps, 500, v => (v/1000).toFixed(2));
    $('stat-rate-u').textContent = 'Mbps';
  } else {
    animateNumber('stat-rate', rkbps, 500, v => Math.round(v).toString());
    $('stat-rate-u').textContent = 'kbps';
  }
  animateNumber('stat-cpu', s.cpu_pct || 0, 500, v => v.toFixed(1));
  updateSpark('spark-rate-line', 'spark-rate-fill', rateHistory, 760, 70);
  updateSpark('spark-cpu-line', 'spark-cpu-fill', cpuHistory, 360, 48);
  animateNumber('m-tags', s.stats.tags_sent || 0);
  const [bv, bu] = fmtBytesPair(s.stats.bytes_sent || 0);
  $('m-bytes').textContent = bv; $('m-bytes-u').textContent = bu;
  animateNumber('m-cuts', s.stats.cuts || 0);
  animateNumber('m-idis', s.stats.ingest_disconnects || 0);
  animateNumber('m-erec', s.stats.egress_reconnects || 0);
  const [rv, ru] = fmtBytesPair(s.rss_bytes || 0);
  $('m-rss').textContent = rv; $('m-rss-u').textContent = ru;
}

