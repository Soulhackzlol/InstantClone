// ---------------------------------------------------------------- state

const S = {
  data:null, visible:false, filter:'all', search:'', peek:null,
  view: store.get('ig-view') === 'list' ? 'list' : 'grid',
  pollTimer:null, activityOpen:false,
  // The page of the Overlay tab that is showing ('alerts' or 'goals'), and
  // what its last redraw was drawn from.
  stage:null, staged:'',
  // What the last poll that redrew the tab brought, and when that was.
  heard:'', drawnAt:0,
  // The Recent activity row showing its steps (its data-key), and whether
  // an editor shows its last run's steps.
  actOpen:null, lastRunOpen:false,
  // Secret fields the user chose to show (by field id). Hidden by default:
  // a dashboard is often on a stream.
  revealed:new Set(),
  // Folders shown folded, kept between visits; the one being renamed;
  // folders made with New folder that nothing is in yet (this visit only).
  // Whatever the browser kept, a list or not, never stops the page.
  folded:new Set([].concat(readJson(store.get('ig-folded'), []))),
  renaming:null, pending:[],
  // The integration being dragged to a folder, and a redraw that waits for
  // the drop (a redraw mid-drag would drop the dragged card).
  dragging:null, redraw:false,
};

async function load(){
  const d = await api('/integrations');
  if (!d || d.ok === false || !d.integrations){
    const why = (d && d.error) || 'Integrations are not available right now.';
    // A failed refresh keeps what is on screen; only a first load says so.
    if (S.data) toast(why, 'err', 5000);
    else morph($('ig-root'), `<div class="lan-warn"><span class="lw-ic">⚠</span><span>${esc(why)}</span>
      <button class="ic-btn ic-btn-ghost ig-small" data-act="retry">Try again</button></div>`);
    return false;
  }
  S.data = d;
  // The alert designs, by name, for the alerts' Design chip (read quietly:
  // the Studio may have saved or renamed one since).
  Stage.loadDesigns().catch(() => {});
  // The answer redraws the tab: the card around it eases to its new height.
  if (typeof easeCardHeight === 'function') easeCardHeight(render); else render();
  // Opening the dashboard connects OBS: show it soon, not at the next poll.
  if ((d.obs || {}).state === 'connecting') setTimeout(poll, 1200);
  return true;
}

// The most steps one integration holds: the app's own limit (model.rs).
const MAX_STEPS = 256;
// Whether `count` more steps fit in integration `d`; says why when not.
function stepsFit(d, count){
  let total = 0;
  d.handlers.forEach(h => walk(h.steps, () => { total++; }));
  if (total + count <= MAX_STEPS) return true;
  toast(`That's the most steps one integration can have (${MAX_STEPS})`, 'info');
  return false;
}
// How long the tab may go without a redraw while nothing changes.
const IDLE_REDRAW_MS = 30000;

async function poll(){
  if (document.hidden || !S.data || (!S.visible && !S.stage && !modalOpen())) return;
  const a = await api('/integrations/activity');
  if (!S.data || !a || !a.activity) return;
  const fastBefore = !!S.data.twitch.login;
  const twitchChanged = JSON.stringify(a.twitch) !== JSON.stringify(S.data.twitch);
  S.data.activity = a.activity;
  S.data.stats = a.stats;
  S.data.twitch = a.twitch;
  S.data.obs = a.obs;
  const alerts = Object.assign({}, a.alerts, {look:lookOf()});
  const alertsChanged = JSON.stringify(alerts) !== JSON.stringify(S.data.alerts);
  S.data.alerts = alerts;
  S.data.tracking = a.tracking || S.data.tracking;
  if (fastBefore !== !!a.twitch.login) schedulePoll();
  // The Overlay tab's pages show less than this tab, and redraw for less.
  const staged = JSON.stringify([a.tracking, alerts, a.twitch.main.login]);
  if (S.stage && staged !== S.staged){
    S.staged = staged;
    Stage.render();
  }
  // Most polls bring nothing new. Redrawing every card for each one made
  // the whole dashboard crawl once a setup had a few hundred steps, so the
  // tab is redrawn when something changed, and now and then for its
  // "2 min ago" lines.
  const heard = JSON.stringify([a.activity, a.stats, a.twitch, a.obs, alerts]);
  const stale = Date.now() - S.drawnAt > IDLE_REDRAW_MS;
  if (S.visible && (heard !== S.heard || stale)){
    S.heard = heard;
    S.drawnAt = Date.now();
    render();
  }
  // Pages that show live state.
  if (Modal.kind === 'connections' && ((Conn.tab === 'twitch' && twitchChanged) || (Conn.tab === 'stream' && alertsChanged))) Conn.render();
  if (Modal.kind === 'editor' && alertsChanged && (primaryStep(Editor.handler()) || {}).type === 'overlay') Editor.render();
}

function schedulePoll(){
  clearInterval(S.pollTimer);
  const fast = S.data && S.data.twitch && S.data.twitch.login;
  S.pollTimer = setInterval(() => { poll(); }, fast ? 2000 : 3500);
}

async function refreshData(){
  const d = await api('/integrations');
  if (d && d.integrations){ S.data = d; if (S.visible) render(); }
}

