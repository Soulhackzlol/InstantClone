// The Overlay Studio: the dashboard's editor for overlays and alert designs.
// Loaded by index.html after its main script (it uses that script's helpers:
// $, $$, toast, copyText, lastState...) and before integrations.js, which it
// calls only once the page is running.
'use strict';

// - Overlay Studio --------------------------
// A no-code editor for browser-source overlays. The heavy lifting lives
// in /overlay-runtime.js (window.ICOverlay): widget catalogue, animation
// library, and bake(doc)->self-contained static HTML. The Studio canvas
// is an iframe whose srcdoc is bake(doc). A layer being moved or resized
// moves in that (same-origin) page as the pointer goes; on release its
// offset is written and the page's styles are swapped in place
// (liveStyle), so nothing reloads and no animation starts over.

// Where layers sit, apart from the page so it can be tested. A box is a
// layer's layout box on the 1920 x 1080 canvas ({left, top, width,
// height}). An anchor and an offset place a layer as the runtime does
// (anchorCss): on each axis the offset pins one edge. Anchored left or
// top, it is that edge's distance from the canvas's; at the centre, the
// left or top edge's distance from the middle; anchored right or bottom,
// the right or bottom edge's distance from the canvas's, counted inward.
const StudioGeometry = (function(){
  const CANVAS = {width:1920, height:1080};
  // The safe area the canvas outlines: 5% in from each edge.
  const SAFE = {x:96, y:54};
  function axes(anchor){
    const a = String(anchor || 'center');
    return {
      x: /left$/.test(a) ? 'left' : /right$/.test(a) ? 'right' : 'center',
      y: /^top/.test(a) ? 'top' : /^bottom/.test(a) ? 'bottom' : 'middle',
    };
  }
  // Where a layer of `size` ({width, height}) sits with `anchor` and `offset`.
  function boxAt(anchor, offset, size){
    const a = axes(anchor);
    const left = a.x === 'left' ? offset.x : a.x === 'right' ? CANVAS.width - offset.x - size.width : CANVAS.width / 2 + offset.x;
    const top = a.y === 'top' ? offset.y : a.y === 'bottom' ? CANVAS.height - offset.y - size.height : CANVAS.height / 2 + offset.y;
    return {left, top, width:size.width, height:size.height};
  }
  // The offset that puts a layer with `anchor` at `box`.
  function offsetFor(anchor, box){
    const a = axes(anchor);
    const x = a.x === 'left' ? box.left : a.x === 'right' ? CANVAS.width - box.left - box.width : box.left - CANVAS.width / 2;
    const y = a.y === 'top' ? box.top : a.y === 'bottom' ? CANVAS.height - box.top - box.height : box.top - CANVAS.height / 2;
    return {x:Math.round(x), y:Math.round(y)};
  }
  // The offset that takes a layer laid out at `from` to `to`. Worked out
  // from measured boxes, so a layer moves by what it was asked to wherever
  // it sits (a state's own place, a picture's room).
  function placeBox(anchor, offset, from, to){
    const a = axes(anchor);
    const dx = a.x === 'right' ? (from.left + from.width) - (to.left + to.width) : to.left - from.left;
    const dy = a.y === 'bottom' ? (from.top + from.height) - (to.top + to.height) : to.top - from.top;
    return {x:Math.round(offset.x + dx), y:Math.round(offset.y + dy)};
  }
  const moved = (box, dx, dy) => ({left:box.left + dx, top:box.top + dy, width:box.width, height:box.height});
  // The box around all of `boxes`.
  function bounds(boxes){
    const left = Math.min(...boxes.map(b => b.left)), top = Math.min(...boxes.map(b => b.top));
    const right = Math.max(...boxes.map(b => b.left + b.width)), bottom = Math.max(...boxes.map(b => b.top + b.height));
    return {left, top, width:right - left, height:bottom - top};
  }
  // Whether two boxes share any of the canvas (touching counts).
  const overlaps = (a, b) => a.left <= b.left + b.width && b.left <= a.left + a.width && a.top <= b.top + b.height && b.top <= a.top + a.height;
  // `box` resized by its handle (n, ne, e, se, s, sw, w or nw) moved by dx,
  // dy: the opposite side or corner stays put, and it never gets smaller
  // than MIN. With `keepShape` (Shift, on a corner) it keeps its proportions.
  const MIN = 8;
  function resizeBox(box, handle, dx, dy, keepShape){
    const right0 = box.left + box.width, bottom0 = box.top + box.height;
    let left = box.left, top = box.top, right = right0, bottom = bottom0;
    if(handle.includes('w')) left = Math.min(box.left + dx, right0 - MIN);
    if(handle.includes('e')) right = Math.max(right0 + dx, box.left + MIN);
    if(handle.includes('n')) top = Math.min(box.top + dy, bottom0 - MIN);
    if(handle.includes('s')) bottom = Math.max(bottom0 + dy, box.top + MIN);
    let width = right - left, height = bottom - top;
    if(keepShape && handle.length === 2 && box.width && box.height){
      const k = Math.max(width / box.width, height / box.height);
      width = Math.max(MIN, box.width * k); height = Math.max(MIN, box.height * k);
      left = handle.includes('w') ? right0 - width : box.left;
      top = handle.includes('n') ? bottom0 - height : box.top;
    }
    return {left:Math.round(left), top:Math.round(top), width:Math.round(width), height:Math.round(height)};
  }
  // How much a layer sized by its font grows when its bottom-right corner
  // is dragged by dx, dy: so that the edge the drag goes furthest past (its
  // right, or its bottom) lands under the pointer.
  function cornerScale(box, dx, dy){
    const across = box.width ? (box.width + dx) / box.width : 1;
    const down = box.height ? (box.height + dy) / box.height : 1;
    return Math.max(0, Math.abs(across - 1) >= Math.abs(down - 1) ? across : down);
  }
  // The lines a moving box snaps to: the canvas's edges and middle, the
  // safe area's edges, and the edges and middle of each of `others`.
  function snapLines(others){
    const xs = [0, SAFE.x, CANVAS.width / 2, CANVAS.width - SAFE.x, CANVAS.width];
    const ys = [0, SAFE.y, CANVAS.height / 2, CANVAS.height - SAFE.y, CANVAS.height];
    others.forEach(b => {
      xs.push(b.left, b.left + b.width / 2, b.left + b.width);
      ys.push(b.top, b.top + b.height / 2, b.top + b.height);
    });
    return {xs, ys};
  }
  // How far to nudge `box` so its nearest edge or middle lands on a line
  // within `reach`, on each axis: {dx, dy, x, y}, x and y being the lines it
  // snapped to (null for none).
  function snap(box, lines, reach){
    const nearest = (points, candidates) => {
      let best = null;
      points.forEach(p => candidates.forEach(c => {
        const d = c - p;
        if(Math.abs(d) <= reach && (!best || Math.abs(d) < Math.abs(best.d))) best = {d, at:c};
      }));
      return best;
    };
    const x = nearest([box.left, box.left + box.width / 2, box.left + box.width], lines.xs);
    const y = nearest([box.top, box.top + box.height / 2, box.top + box.height], lines.ys);
    return {dx:x ? x.d : 0, dy:y ? y.d : 0, x:x ? x.at : null, y:y ? y.at : null};
  }
  // `boxes` lined up on the box around them all: by their left edges,
  // centres or right edges ('left', 'center', 'right'), or their tops,
  // middles or bottoms ('top', 'middle', 'bottom'). In the order given.
  function align(boxes, how){
    const all = bounds(boxes);
    return boxes.map(b => {
      if(how === 'left') return moved(b, all.left - b.left, 0);
      if(how === 'center') return moved(b, all.left + all.width / 2 - (b.left + b.width / 2), 0);
      if(how === 'right') return moved(b, all.left + all.width - (b.left + b.width), 0);
      if(how === 'top') return moved(b, 0, all.top - b.top);
      if(how === 'middle') return moved(b, 0, all.top + all.height / 2 - (b.top + b.height / 2));
      if(how === 'bottom') return moved(b, 0, all.top + all.height - (b.top + b.height));
      return b;
    });
  }
  // Three or more `boxes` spread with even gaps, across ('x') or down
  // ('y'), the first and the last staying put. In the order given.
  function distribute(boxes, axis){
    if(boxes.length < 3) return boxes.slice();
    const [start, size] = axis === 'x' ? ['left', 'width'] : ['top', 'height'];
    const order = boxes.map((b, i) => ({b, i})).sort((p, q) => p.b[start] - q.b[start]);
    const first = order[0].b, last = order[order.length - 1].b;
    const room = last[start] + last[size] - first[start] - order.reduce((sum, o) => sum + o.b[size], 0);
    const gap = room / (order.length - 1);
    const out = boxes.slice();
    let at = first[start];
    order.forEach(o => {
      const d = at - o.b[start];
      out[o.i] = axis === 'x' ? moved(o.b, d, 0) : moved(o.b, 0, d);
      at += o.b[size] + gap;
    });
    return out;
  }
  // Stacking, on a list of layer ids from the back to the front. `ids`
  // one step forward (dir 1) or back (-1), each past the next one not
  // among them; at the front or the back; or just in front of `target`
  // (`ahead`) or behind it. A new list each time, the same order when
  // nothing can move.
  function stepped(order, ids, dir){
    const list = order.slice(), moving = i => ids.includes(list[i]);
    const at = list.map((_, i) => i); if(dir > 0) at.reverse();
    at.forEach(i=>{
      const j = i + dir;
      if(moving(i) && j >= 0 && j < list.length && !moving(j)) [list[i], list[j]] = [list[j], list[i]];
    });
    return list;
  }
  function toEnd(order, ids, front){
    const them = order.filter(id => ids.includes(id)), rest = order.filter(id => !ids.includes(id));
    return front ? rest.concat(them) : them.concat(rest);
  }
  function beside(order, ids, target, ahead){
    const them = order.filter(id => ids.includes(id)), rest = order.filter(id => !ids.includes(id));
    const at = rest.indexOf(target);
    if(at < 0) return order.slice();
    rest.splice(ahead ? at + 1 : at, 0, ...them);
    return rest;
  }
  return {CANVAS, SAFE, boxAt, offsetFor, placeBox, moved, bounds, overlaps, resizeBox, cornerScale, snapLines, snap, align, distribute,
    stepped, toEnd, beside};
})();
const Studio = (function(){
  const G = StudioGeometry;
  const clone = o => JSON.parse(JSON.stringify(o));
  // split/join not regex for the quote case - the build-time minifier
  // misparses a /"/g literal. (Same reason as in overlay-runtime.js.)
  const escapeHtml = s => String(s==null?'':s).split('&').join('&amp;').split('<').join('&lt;').split('>').join('&gt;');
  const escapeAttr = s => String(s==null?'':s).split('&').join('&amp;').split('<').join('&lt;').split('"').join('&quot;');
  let doc = null;        // current overlay document
  let slug = null;       // saved slug, or null for an unsaved draft
  // The layers picked, in the order picked. `sel` is the last of them: the
  // one the settings panel shows when it is the only one.
  let picked = [], sel = null;
  let arrived = null;    // a layer just added, animated into the list once
  let editState = 'active'; // the state you preview AND edit (unified)
  let left = 'layers';
  let scale = 1;
  let mounted = false, frameWatched = false, savedList = [];
  // The streamer's goals and timers, for the Goal bar and Timer pickers.
  let tracked = {goals:[], timers:[]};
  function loadTracked(){
    if(!window.Integrations) return;
    Integrations.tracking().then(t=>{ tracked=t; if(sel && $('ov-studio') && !$('ov-studio').hidden) renderProps(); });
  }
  // Undo/redo: a stack of doc snapshots. Every rebake records (debounced),
  // so all edits are captured with minimal wiring. `restoring` guards
  // against re-recording while applying an undo.
  let history=[], histIndex=-1, recordT=0, restoring=false;
  function resetHistory(){ history=[JSON.stringify(doc)]; histIndex=0; updateUndoBtns(); markClean(); }
  function recordNow(){ if(restoring) return; const s=JSON.stringify(doc); if(history[histIndex]===s) return; history=history.slice(0,histIndex+1); history.push(s); if(history.length>120) history.shift(); histIndex=history.length-1; updateUndoBtns(); updateDirty(); }
  function recordSoon(){ if(restoring) return; clearTimeout(recordT); recordT=setTimeout(recordNow,300); }
  // An edit still waiting to be recorded is recorded first: an undo right
  // after a drag takes back the drag, not the step before it.
  function recordPending(){ if(!recordT) return; clearTimeout(recordT); recordT=0; recordNow(); }
  function undo(){ recordPending(); if(histIndex>0){ histIndex--; applyHistory(); } }
  function redo(){ recordPending(); if(histIndex<history.length-1){ histIndex++; applyHistory(); } }
  function applyHistory(){ restoring=true; doc=JSON.parse(history[histIndex]); forgetGone(); $('st-name').value=doc.name||''; renderLeft(); rebake(); renderProps(); restoring=false; updateUndoBtns(); updateDirty(); }
  // Unsaved-changes tracking: "dirty" = the doc differs from the snapshot
  // taken when it was opened (resetHistory) or last saved. Undo back to that
  // snapshot reads as clean again. Drives the Save indicator + close guards.
  let savedSnapshot='';
  function markClean(){ savedSnapshot = doc?JSON.stringify(doc):''; updateDirty(); }
  function isDirty(){ return !!doc && JSON.stringify(doc)!==savedSnapshot; }
  function updateDirty(){ const b=$('st-save'); if(!b) return; const d=isDirty();
    b.textContent = d?'Save ●':'Save'; b.classList.toggle('st-save-dirty',d);
    b.title = d?'Unsaved changes. Click to overwrite this overlay':'Overwrite this overlay'; }
  function updateUndoBtns(){ const u=$('st-undo'),r=$('st-redo'); if(u)u.disabled=histIndex<=0; if(r)r.disabled=histIndex>=history.length-1; }
  // Semantic colour per state, for the dots in the state selector.
  const STATE_DOT = { idle:'#8a8f9a', preparing:'#ffb73a', ready:'#5ac8fa', active:'#54e38e', error:'#ff5a5a' };

  function IC(){ return window.ICOverlay; }
  function widget(id){ return doc.widgets.find(w => w.id === id); }
  function nextId(){ let n=1; while(doc.widgets.some(w=>w.id==='w'+n)) n++; return 'w'+n; }
  function slugify(s){ return (s||'overlay').toLowerCase().replace(/[^a-z0-9]+/g,'-').replace(/^-+|-+$/g,'').slice(0,64) || 'overlay'; }
  // A slug for `name` that no other saved overlay has: a second "Tourney"
  // becomes tourney-2, where it used to be written over the first. `mine`
  // is the slug of the overlay being renamed, which may keep its own.
  // goal-... and timer-... name goals' and timers' own overlays (see
  // trackerSlug): one made here and named so is goals-... or timers-...
  function freeSlug(name, mine){
    const base = slugify(name).replace(/^(goal|timer)-/, '$1s-'), taken = s => s !== mine && savedList.some(o => o.slug === s);
    if(!taken(base)) return base;
    for(let n=2; ; n++){ const s = base.slice(0, 60) + '-' + n; if(!taken(s)) return s; }
  }

  // ---- views: gallery (browse + preview) vs studio (editor) ----
  let gsel = null;          // selected overlay slug (everything is an overlay now)
  let gpicked = false;      // gsel is the user's pick (else a page may lead with its own)
  let autohideOff = false;  // quick "stays up" override for the selection (?autohide=off)
  let previewState = 'auto'; // 'auto' = run the delay-lifecycle simulation
  let gallerySim = null, studioSim = null, playing = false;
  const ARMED_MS = 15000;
  // What the Studio offers, by what the overlay is for (the page it is on):
  // what its preview bar drives, the state edited, and the order of the Add
  // list. Only a delay overlay changes with the delay: an alert design plays
  // an alert, a goal's overlay moves its numbers, anything else stays put.
  const MODES = {
    delay:  {sim:'states', groups:['data', 'stream', 'ambient', 'deco', 'media', 'power']},
    goals:  {sim:'goals',  groups:['stream', 'deco', 'media', 'ambient', 'power', 'data']},
    alerts: {sim:'alert',  groups:['alert', 'deco', 'media', 'ambient', 'stream', 'power', 'data']},
    custom: {sim:'none',   groups:['deco', 'media', 'stream', 'ambient', 'power', 'data']},
  };
  function mode(){ return MODES[doc ? IC().docPurpose(doc) : 'delay']; }
  // Goals and timers moving in a goal overlay's preview: +1 and -1, or
  // counting up and back down by itself, so the bar fills and "+1" pops.
  let simGoals = null, goalAutoT = 0;
  function startGoalSim(){
    stopGoalAuto();
    const goals = clone(tracked.goals || []);
    simGoals = {
      goals: goals.length ? goals : [{id:'subs', name:'Sub goal', value:32, target:50}],
      timers: (tracked.timers || []).map(t => ({...t, running:true, ends_at_ms:Date.now() + Math.max(90000, t.left_ms || 0)})),
    };
  }
  function nudgeGoals(n){
    stopGoalAuto();
    simGoals.goals.forEach(g => { g.value = Math.max(0, Math.min(g.target, g.value + n)); });
    if(studioSim) studioSim.now();
  }
  function stopGoalAuto(){ clearInterval(goalAutoT); goalAutoT = 0; const b=$('st-goal-auto'); if(b) b.classList.remove('on'); }
  function goalAuto(){
    if(goalAutoT){ stopGoalAuto(); return; }
    $('st-goal-auto').classList.add('on');
    let up = true;
    goalAutoT = setInterval(() => {
      simGoals.goals.forEach(g => {
        const step = Math.max(1, Math.round(g.target / 12));
        g.value = Math.max(0, Math.min(g.target, g.value + (up ? step : -step)));
      });
      const g = simGoals.goals[0];
      if(up && g.value >= g.target) up = false; else if(!up && g.value <= 0) up = true;
      if(studioSim) studioSim.now();
    }, 700);
  }
  // An alert design's preview plays a sample alert: its entrance, the hold,
  // its exit; then it is back, held, to edit. The gallery plays the same.
  const ALERT_DEMO_MS = 3500;
  let alertDemoT = 0;
  // The samples take turns: a follow (no message), then a cheer with one,
  // so a design's message layer shows too, then a long name and a long
  // message, to see that the words still fit.
  const SAMPLE_ALERTS = [{title:'New follower', text:'Nat'}, {title:'Bits', text:'Nat cheered 500 bits', message:'That clutch was unreal, gg!'},
    {title:'New subscriber', text:'xX_TheLongestNameInChat_Xx subscribed for 27 months',
      message:'Been here since the very first stream and never missed one. The delay setup is wild and the overlays keep getting better, keep it up!'}];
  let sampleTurn = 0;
  function postSampleAlert(frame){
    const a = SAMPLE_ALERTS[sampleTurn++ % SAMPLE_ALERTS.length];
    if(frame && frame.contentWindow) frame.contentWindow.postMessage({__ic_alert:Object.assign({ms:ALERT_DEMO_MS}, a)}, '*');
  }
  function playAlert(){
    const f = $('st-canvas'); if(!f || !f.contentWindow) return;
    if(studioSim){ studioSim.stop(); studioSim = null; }
    clearTimeout(alertDemoT);
    postSampleAlert(f);
    alertDemoT = setTimeout(studioDrive, ALERT_DEMO_MS + 900);
  }
  // In the gallery, over and over, as on stream: it comes in, stays, goes,
  // and a moment later plays again. (Told once that it is a preview, so the
  // sample has its picture.)
  const ALERT_REST_MS = 1600;
  function alertLoop(frame){
    let t = 0;
    if(frame.contentWindow) frame.contentWindow.postMessage({__ic_sim:true, forceState:'idle', state:simFrame('idle', 0), noHide:true}, '*');
    const play = ()=>{ postSampleAlert(frame); t = setTimeout(play, ALERT_DEMO_MS + ALERT_REST_MS); };
    t = setTimeout(play, 300);
    return { stop(){ clearTimeout(t); } };
  }
  // A fake /state for a phase at progress p (0..1). Arming ramps the fill.
  // Bitrate gets a gentle wave so the BitrateMeter sparkline draws a curve.
  function simFrame(st, p){
    const frame = delayFrame(st, p);
    return simGoals ? {...frame, goals:simGoals.goals, timers:simGoals.timers} : frame;
  }
  function delayFrame(st, p){
    const br=6200+Math.round(Math.sin(performance.now()/350)*520);
    const base={ingest_alive:true,armed_delay_ms:ARMED_MS,buffer_target_ms:ARMED_MS,target_delay_ms:ARMED_MS,
      current_delay_ms:ARMED_MS,destinations_total:2,destinations_alive:2,
      destinations:[{alive:true},{alive:true}],stats:{cuts:0,bitrate_kbps:br}};
    if(st==='idle') return {...base,ingest_alive:false,phase:'idle',buffer_fill_ms:0,destinations_alive:0,destinations:[{alive:false},{alive:false}],stats:{cuts:0,bitrate_kbps:0}};
    if(st==='preparing') return {...base,phase:'preparing',buffer_fill_ms:Math.round((p==null?0.5:Math.min(1,p))*ARMED_MS)};
    if(st==='ready') return {...base,phase:'ready',buffer_fill_ms:ARMED_MS};
    if(st==='active') return {...base,phase:'active',buffer_fill_ms:ARMED_MS};
    if(st==='error') return {...base,phase:'active',buffer_fill_ms:ARMED_MS,destinations_alive:0,destinations:[{alive:false},{alive:false}]};
    return base;
  }
  // Delay lifecycle for the 'Auto' simulation. Arming ramps; rest hold.
  // Active is long so the auto-hide ("show then disappear") is visible in preview.
  const LIFECYCLE = [['idle',1300],['preparing',3400],['ready',1500],['active',13000]];
  // makeSim(iframe, mode, onTick, noHide): mode 'auto' runs the lifecycle; a
  // state key pins it (arming keeps animating its fill). noHide keeps auto-hide
  // widgets visible (used while editing). onTick drives the live caption.
  function makeSim(iframe, mode, onTick, noHide, replayGap){
    let timer=0, firstPost=true, lastReplay=performance.now(); const t0=performance.now();
    const gap = replayGap||5000;
    function post(st,p,forceReplay){ if(iframe && iframe.contentWindow) iframe.contentWindow.postMessage({__ic_sim:true,forceState:st,state:simFrame(st,p),noHide:!!noHide,replay:firstPost||!!forceReplay},'*'); firstPost=false; if(onTick) onTick(st,p); }
    function loop(){
      const total=LIFECYCLE.reduce((a,x)=>a+x[1],0);
      let el=(performance.now()-t0)%total, acc=0, ph='idle', p=0;
      for(const x of LIFECYCLE){ if(el<acc+x[1]){ ph=x[0]; p=(el-acc)/x[1]; break; } acc+=x[1]; }
      post(ph, ph==='preparing'?p:1);
      timer=setTimeout(loop, 90);
    }
    function pinned(){
      if(mode==='preparing'){ const dur=2800, el=(performance.now()-t0)%dur; post('preparing', el/dur); timer=setTimeout(pinned, 90); }
      else {
        // When auto-hide can fire (noHide off), re-show the widget once its
        // hide has had time to play, so the exit loops while editing. The gap
        // adapts to the configured hide duration - a fixed 5s would re-show
        // (and re-arm) BEFORE a longer hide ever fired, so it'd never appear
        // to hide at all. Otherwise just keep ticking so live widgets stay alive.
        const now=performance.now(); let rep=false;
        if(!noHide && now-lastReplay>gap){ rep=true; lastReplay=now; }
        post(mode, 1, rep); timer=setTimeout(pinned, 500);
      }
    }
    if(!mode || mode==='auto'){ loop(); } else { pinned(); }
    // rehide(): after an edit, re-arm all auto-hide timers together from the
    // current pinned frame so they don't drift out of sync until the next
    // replay. No-op while auto-playing (the lifecycle re-arms them anyway).
    function rehide(){ if(!mode||mode==='auto'||!iframe||!iframe.contentWindow) return;
      iframe.contentWindow.postMessage({__ic_sim:true,forceState:mode,state:simFrame(mode,mode==='preparing'?0.5:1),noHide:!!noHide,rehide:true},'*'); }
    // now(): the pinned frame again at once (the preview's goals moved).
    function now(){ if(mode && mode!=='auto') post(mode, mode==='preparing'?0.5:1); }
    return { stop(){ clearTimeout(timer); }, rehide:rehide, now:now };
  }
  // A friendly live caption for the current simulated phase.
  function phaseInfo(st, p){
    if(st==='preparing') return { t:'Arming '+Math.round((p||0)*100)+'%', c:STATE_DOT.preparing };
    if(st==='ready') return { t:'Armed', c:STATE_DOT.ready };
    if(st==='active') return { t:'On delay '+(ARMED_MS/1000).toFixed(1)+'s', c:STATE_DOT.active };
    if(st==='error') return { t:'Signal lost', c:STATE_DOT.error };
    return { t:'Idle, no delay', c:STATE_DOT.idle };
  }
  function setCaption(elId, st, p){
    const el=$(elId); if(!el) return;
    const inf=phaseInfo(st,p);
    el.innerHTML='<span class="ph-dot" style="background:'+inf.c+'"></span>'+inf.t;
  }
  let pendingSelect = null; // slug to auto-select in the gallery after a save

  // The studio is a floating modal; the gallery stays mounted behind it.
  function setView(name){ $('ov-studio').hidden = name!=='studio'; }

  // Entry point from the tab switch. Default to the gallery; the studio
  // editor is only mounted when the user opens it.
  // The Overlay tab page the browser is on: 'delay', 'alerts' or 'custom'.
  // One browser serves them all, moved into the page that shows.
  let page = 'delay';
  function show(which){
    if(!window.ICOverlay) return;
    if(which && which!==page) gpicked = false;
    if(which) page = which;
    const pane = document.querySelector('.sub-pane[data-subpane="'+page+'"]'), browser = $('ovg-browser');
    if(pane && browser.parentElement!==pane) pane.appendChild(browser);
    $$('#ovg-browser [data-only]').forEach(el=>{ el.hidden = el.dataset.only!==page; });
    // A custom overlay looks the same whatever the delay does.
    $('ovg-simbar').hidden = !PAGES[page].states;
    $('ovg-shelf-label').textContent = PAGES[page].label;
    // Alerts have one browser source for them all, in the bar above.
    $('ovg-urlbox').hidden = page==='alerts';
    if($('ov-studio').hidden===false) return; // already in studio
    setView('gallery');
    loadSaved().then(()=> easeCardHeight(()=>{ renderGallery(); fitPreview(); }));
  }

  function ensureStudio(){
    if(mounted) return;
    mounted = true;
    // Reparent the modal to <body>: the tab pane keeps a transform from its
    // entry animation, which would trap position:fixed inside the pane.
    const m=$('ov-studio'); if(m && m.parentElement!==document.body) document.body.appendChild(m);
    // The canvas fits its frame whatever resizes it: the window, the modal
    // opening, or the state bar wrapping onto a second line. Once: a delete
    // mounts the Studio again.
    if(!frameWatched){ frameWatched=true; new ResizeObserver(fitScale).observe($('st-canvas-frame')); }
    if(!doc){ doc = clone(IC().PRESETS[0]); doc.widgets.forEach((w,i)=>{ w.id='w'+(i+1); }); $('st-name').value = doc.name; }
    renderStates();
    renderLeft();
    rebake();
    renderProps();
    resetHistory();
  }
  // The Overlay page the Studio was opened from, when not the gallery.
  let origin = null;
  function openStudio(slugId, from){
    origin = from || null;
    stopGallerySim(); setView('studio'); ensureStudio(); loadTracked();
    if(slugId) openSaved({slug:slugId, studio:true});
  }
  // The three real intents: edit (openStudio), create new (blank), duplicate.
  function newOverlay(purpose){
    origin = null;
    stopGallerySim(); setView('studio'); ensureStudio(); loadTracked();
    doc = purpose==='alerts' ? IC().alertDoc('New design') : IC().defaultDoc('Untitled overlay'); slug=null; clearPicked();
    doc.purpose = ['custom', 'alerts'].includes(purpose) ? purpose : 'delay';
    doc.widgets.forEach((w,n)=>{ if(!w.id) w.id='w'+(n+1); });
    $('st-name').value = doc.name;
    enterMode(); renderLeft(); rebake(); renderProps(); resetHistory(); updateUrlLabel();
  }
  async function duplicateOverlay(slugId){
    origin = null;
    stopGallerySim(); setView('studio'); ensureStudio();
    await openSaved({slug:slugId, studio:true});
    // It didn't open (unreadable, or hand-written): nothing to copy, and
    // whatever was open before must not be renamed "copy" in its place.
    if(slug!==slugId) return;
    slug=null; doc.name=(doc.name||'Overlay')+' copy'; $('st-name').value=doc.name;
    updateUrlLabel(); resetHistory();
    toast('Duplicated - "Save as new" to write it','info');
  }
  function backToGallery(){
    if(isDirty() && !confirm('You have unsaved changes to this overlay. Discard them and go back to the gallery?')) return;
    stopPlay(); stopGoalAuto(); clearTimeout(alertDemoT);
    setView('gallery');
    // Back to where it was opened from, or to the page it is on now (a
    // save may have moved it there, and selects it).
    const from = origin; origin = null;
    const home = doc ? ({custom:'custom', alerts:'alerts', goals:'goals'}[IC().docPurpose(doc)] || 'delay') : page;
    showSubTab('overlay', from || home);
  }

  // The canvas's view: `zoom` times the size that fits the frame (1 shows
  // all of it, centred), and where it sits in the frame (tx, ty, screen
  // px) once it is bigger than the frame.
  const ZOOM_MAX = 8;
  let fit = 1, zoom = 1, view = {tx:0, ty:0}, drawnAt = 0;
  // Fit the 1920x1080 canvas into the frame on BOTH axes, so the editor
  // always shows a true 16:9 regardless of the modal size; zoomed in, it
  // can't be pushed off the frame.
  function fitScale(){
    const frame = $('st-canvas-frame'); if(!frame) return;
    const fw = frame.clientWidth || 640, fh = frame.clientHeight || 360;
    fit = Math.max(0.05, Math.min(fw/1920, fh/1080));
    scale = fit * zoom;
    const place = (room, size, at) => size <= room ? (room - size) / 2 : Math.min(0, Math.max(room - size, at));
    const tx = view.tx = place(fw, 1920*scale, view.tx), ty = view.ty = place(fh, 1080*scale, view.ty);
    $('st-canvas-scale').style.transform = 'translate('+tx+'px,'+ty+'px) scale('+scale+')';
    const z = $('st-zoom-level'); if(z) z.textContent = Math.round(scale*100)+'%';
    // Outlines and handles keep their size on screen at any zoom.
    $('st-canvas-scale').style.setProperty('--st-inv', String(1/scale));
    // Lay the (non-scaled) 16:9 outline + safe guide exactly over the canvas.
    const b = $('st-canvas-bounds');
    if(b){ b.style.left=tx+'px'; b.style.top=ty+'px'; b.style.width=(1920*scale)+'px'; b.style.height=(1080*scale)+'px'; }
    // Handles hide on layers small on screen: how small changed.
    if(doc && mounted && scale!==drawnAt){ drawnAt = scale; drawSelection(); }
  }
  // Zoom to `next`, the canvas point at frame px x, y staying under it
  // (the pointer's, for the wheel; the middle, for the buttons and keys).
  function zoomAt(next, x, y){
    const frame = $('st-canvas-frame');
    if(x==null){ x = frame.clientWidth/2; y = frame.clientHeight/2; }
    const cx = (x - view.tx) / scale, cy = (y - view.ty) / scale;
    zoom = Math.min(ZOOM_MAX, Math.max(1, next));
    view.tx = x - cx * fit * zoom; view.ty = y - cy * fit * zoom;
    fitScale();
  }
  const ZOOM_STEP = 1.25;
  const zoomIn = () => zoomAt(zoom * ZOOM_STEP), zoomOut = () => zoomAt(zoom / ZOOM_STEP);
  function zoomFit(){ zoom = 1; fitScale(); }
  // Ctrl or Cmd and the wheel (a trackpad's pinch too) zooms at the
  // pointer; zoomed in, the wheel pans (Shift: across).
  function onCanvasWheel(e){
    if(!doc) return;
    const r = $('st-canvas-frame').getBoundingClientRect(), px = e.deltaMode ? 16 : 1;
    if(e.ctrlKey || e.metaKey){ e.preventDefault(); zoomAt(zoom * Math.exp(-e.deltaY * px * 0.002), e.clientX - r.left, e.clientY - r.top); return; }
    if(zoom === 1) return;
    e.preventDefault();
    view.tx -= (e.shiftKey ? e.deltaY : e.deltaX) * px; view.ty -= (e.shiftKey ? 0 : e.deltaY) * px;
    fitScale();
  }
  // Space held, or the middle button: a drag moves the view.
  let spaceHeld = false;
  function startPan(e){
    const tx0 = view.tx, ty0 = view.ty, frame = $('st-canvas-frame');
    frame.classList.add('st-panning');
    follow(e, {
      move(dx, dy){ view.tx = tx0 + dx*scale; view.ty = ty0 + dy*scale; fitScale(); },
      done(){ frame.classList.remove('st-panning'); },
      cancel(){ view.tx = tx0; view.ty = ty0; fitScale(); frame.classList.remove('st-panning'); },
    });
  }
  function holdSpace(down){ spaceHeld = down; $('st-canvas-frame').classList.toggle('st-grab', down); }
  function fitPreview(){
    const stage = $('ovg-stage'); if(!stage) return;
    const w = stage.clientWidth || 600;
    $('ovg-scale').style.transform = 'scale('+(w/1920)+')';
  }

  // ---- gallery: one list of overlays, each with a copyable URL ----
  function curOverlay(){ return galleryItems().find(o=>o.slug===gsel)||null; }
  function selUrl(){ return location.origin+'/overlay/'+encodeURIComponent(gsel)+'.html'+(autohideOff?'?autohide=off':''); }
  // How an overlay shows in the browser: its group on its page (yours
  // before the built-in ones), what it is, its icon and colour. The page it
  // is on is the list's `kind` (web.rs overlay_kind); a goal's or a timer's
  // own overlay is on its card, on the Goals and timers page.
  // The browser's pages: the list's heading, its groups, whether the
  // preview plays the delay's states (or a sample alert), and what an empty
  // page says (alerts always have the ready-made designs).
  const PAGES = {
    delay: { label:'Delay overlays', groups:['Yours', 'Built in'], states:true,
      blank:()=>'<b>No delay overlays</b><span>A delay overlay tells viewers you\'re on a delay: a badge, the countdown while it arms, a frame while it\'s live.</span>'+
        '<div class="og-row"><button class="ic-btn ic-btn-primary st-withic"'+on('newOverlay','delay')+'>'+stIcon(UI_ICON.plus)+'New delay overlay</button>'+
        '<button class="ic-btn ic-btn-ghost"'+on('restoreDefaults')+'>Restore built-ins</button></div>' },
    alerts: { label:'Alert designs', groups:['Yours', 'Ready-made'], states:false, alerts:true, blank:()=>'' },
    custom: { label:'Custom overlays', groups:['Made in the Studio', 'Hand-written'], states:false,
      blank:()=>'<b>Anything else viewers should see</b><span>A starting-soon screen, your socials, a logo, your own HTML: made in the Studio, or .html files you drop in the overlays folder.</span>'+
        '<div class="og-row"><button class="ic-btn ic-btn-primary st-withic"'+on('newOverlay','custom')+'>'+stIcon(UI_ICON.plus)+'New overlay</button>'+
        '<button class="ic-btn ic-btn-ghost"'+on('revealOverlays')+'>Open folder</button></div>' }
  };
  const builtIn = slugId => IC().PRESETS.some(p=>slugify(p.name)===slugId);
  // A ready-made alert design's theme (alert-<theme>), or ''.
  const readyTheme = slugId => IC().ALERT_THEMES.find(t=>slugId==='alert-'+t) || '';
  // The overlays on the page showing. Alerts also list the ready-made
  // designs not saved yet (the themes' cards): one is written the first
  // time it is edited, copied or used.
  function galleryItems(){
    const mine = savedList.filter(o=>o.kind===page);
    if(page!=='alerts') return mine;
    const ready = IC().ALERT_THEMES.map(t=>({slug:'alert-'+t, name:Integrations.alertKit.themes[t].name, kind:'alerts', studio:true, unsaved:true}));
    return mine.concat(ready.filter(r=>!mine.some(o=>o.slug===r.slug)));
  }
  function overlayLook(o){
    if(o.kind==='alerts'){
      const theme = readyTheme(o.slug);
      return theme ? {group:'Ready-made', label:'Ready-made alert design', icon:KIND_ICON.AlertCard, color:'var(--warn)', theme}
        : {group:'Yours', label:'Your alert design', icon:KIND_ICON.AlertCard, color:'var(--warn)', theme:''};
    }
    if(o.kind==='custom'){
      return o.studio ? {group:'Made in the Studio', label:'Custom overlay', icon:UI_ICON.shapes, color:'var(--live)'}
        : {group:'Hand-written', label:'Hand-written: edit its .html file', icon:KIND_ICON.CustomHTML, color:'var(--fg-3)'};
    }
    return builtIn(o.slug) ? {group:'Built in', label:'Built-in delay overlay', icon:UI_ICON.hourglass, color:'var(--accent)'}
      : {group:'Yours', label:'Your delay overlay', icon:UI_ICON.hourglass, color:'var(--accent)'};
  }
  // `keepPreview`: the choice the same, its preview plays on as it is.
  function renderGallery(keepPreview){
    const c = $('ovg-list'); if(!c) return;
    c.innerHTML='';
    if(pendingSelect && savedList.some(o=>o.slug===pendingSelect)){ gsel=pendingSelect; pendingSelect=null; gpicked=true; }
    const mine = galleryItems();
    $('ovg-count').textContent = mine.length || '';
    $('ovg-top').hidden = !mine.length; $('ovg-blank').hidden = !!mine.length;
    if(!mine.length){ $('ovg-blank').innerHTML=PAGES[page].blank(); stopGallerySim(); gsel=null; return; }
    // By group, then by name.
    PAGES[page].groups.forEach(group=>{
      const list = mine.filter(o=>overlayLook(o).group===group).sort((a,b)=>(a.name||a.slug).localeCompare(b.name||b.slug));
      if(!list.length) return;
      const head=document.createElement('div'); head.className='ovg-group'; head.textContent=group; c.appendChild(head);
      list.forEach(o=>{
        const k=overlayLook(o), tile=document.createElement('button');
        tile.className='ovg-tile'; tile.dataset.gid=o.slug; tile.style.setProperty('--tc', k.color);
        const alerts = page==='alerts';
        tile.innerHTML=(alerts ? Integrations.designIcon(k.theme) : '<span class="ovg-tile-ic">'+stIcon(k.icon)+'</span>')+
          '<span class="ovg-tile-txt"><span class="ovg-tile-name"></span><span class="ovg-tile-path'+(alerts?'':' mono')+'"></span></span>';
        tile.querySelector('.ovg-tile-name').textContent=o.name||o.slug;
        tile.querySelector('.ovg-tile-path').textContent=alerts ? designNote(o, k) : '/overlay/'+o.slug+'.html';
        tile.onclick=()=>{ gpicked=true; selectGallery(o.slug); };
        c.appendChild(tile);
      });
    });
    const cur = gsel && c.querySelector('.ovg-tile[data-gid="'+CSS.escape(gsel)+'"]');
    // Nothing picked yet: on Alerts, the design alerts play by default.
    const lead = !gpicked && page==='alerts' && mine.find(o=>(Integrations.designUse(o.slug)||{}).isDefault);
    const pick = lead ? lead.slug : cur ? gsel : c.querySelector('.ovg-tile').dataset.gid;
    selectGallery(pick, keepPreview && !!cur && pick===gsel);
  }
  // Under an alert design's name: how many alerts play it, or what it is.
  function designNote(o, k){
    const u = Integrations.designUse(o.slug), n = u ? u.names.length : 0;
    if(n) return 'Plays for '+n+(n===1?' alert':' alerts');
    return k.theme ? Integrations.alertKit.themes[k.theme].motion : 'Not used yet';
  }

  function selectGallery(slugId, keepPreview){
    gsel=slugId; autohideOff=false;
    $$('#ovg-list .ovg-tile').forEach(el=>el.classList.toggle('sel', el.dataset.gid===slugId));
    if(!keepPreview) updatePreview();
    updateGalleryMeta();
  }
  function stopGallerySim(){ if(gallerySim){ gallerySim.stop(); gallerySim=null; } }
  // Quick "keep it up" toggle: off appends ?autohide=off to the copied/preview
  // URL (the runtime honours it). No re-bake - the baked default is unchanged.
  function toggleAutohide(on){ autohideOff=!on; updatePreview(); showUrl(); }
  function updatePreview(){
    const prev=$('ov-preview'); if(!prev||!gsel) return;
    stopGallerySim();
    prev.onload=()=>applyPreviewState();
    // A ready-made alert design not saved yet: as it would be written.
    const o=curOverlay();
    if(o && o.unsaved){ prev.removeAttribute('src'); prev.srcdoc=IC().bake(IC().alertPreset(readyTheme(o.slug))); return; }
    prev.removeAttribute('srcdoc');
    prev.src='/overlay/'+encodeURIComponent(gsel)+'.html'+(autohideOff?'?autohide=off':'');
  }
  // Drive the preview: 'auto' runs the delay-lifecycle simulation, 'live'
  // shows the real OBS state, anything else pins one state.
  function applyPreviewState(){
    const prev=$('ov-preview'); if(!prev) return;
    stopGallerySim();
    const cap=$('ovg-phase');
    if(PAGES[page].alerts){ if(cap) cap.innerHTML='<span class="ph-dot" style="background:var(--warn)"></span>A sample alert'; gallerySim=alertLoop(prev); return; }
    if(!PAGES[page].states){ if(cap) cap.innerHTML=''; gallerySim=makeSim(prev, 'active', null, true); return; }
    if(previewState==='live'){ if(prev.contentWindow) prev.contentWindow.postMessage({__ic_sim:true,forceState:null,state:simFrame('idle',0)},'*'); if(cap) cap.innerHTML='<span class="ph-dot" style="background:#8a8f9a"></span>Live: real OBS state'; return; }
    // Always keep the widget on screen in the gallery preview (noHide=true) -
    // the preview is to SHOW the overlay, not to demo its auto-hide, which
    // would leave the canvas empty for most of the cycle.
    gallerySim=makeSim(prev, previewState==='auto'?'auto':previewState, (st,p)=>setCaption('ovg-phase',st,p), true);
  }
  function previewSim(st){
    previewState=st;
    $$('.st-simbar [data-psim]').forEach(b=>b.classList.toggle('on',(b.dataset.psim||'')===st));
    applyPreviewState();
  }
  function updateGalleryMeta(){
    const o=curOverlay();
    if(!o){
      $('ovg-name').textContent='No overlay chosen'; $('ovg-kind').textContent=''; $('ovg-ic').innerHTML='';
      $('ovg-url').textContent='select an overlay'; $('ovg-primary').innerHTML=''; $('ovg-actions').innerHTML='';
      return;
    }
    const k=overlayLook(o);
    $('ovg-name').textContent=o.name||o.slug;
    $('ovg-kind').textContent=k.label;
    $('ovg-ic').style.setProperty('--tc', k.color); $('ovg-ic').innerHTML=stIcon(k.icon);
    if(page==='alerts'){ designMeta(o); return; }
    showUrl();
    // On stream in one click; then changing how it looks.
    $('ovg-primary').innerHTML='<button class="ic-btn ic-btn-primary st-withic"'+on('addGalleryToObs')+'>'+stIcon(UI_ICON.plus)+'Add to OBS</button>'+
      (o.studio ? '<button class="ic-btn ic-btn-ghost st-withic"'+on('openStudio',o.slug)+'>'+stIcon(UI_ICON.pen)+'Edit in Studio</button>' : '');
    let h='';
    // Quick auto-hide toggle (only meaningful when the file bakes one).
    if(o.autohide) h+='<label class="ovg-switch" title="Off appends ?autohide=off so the overlay stays up"><input type="checkbox"'+(autohideOff?'':' checked')+on('toggleAutohide')+'><span>Auto-hide when live</span></label>';
    if(o.studio) h+='<button class="ic-btn ic-btn-ghost st-mini"'+on('duplicateOverlay',o.slug)+'>Duplicate</button>';
    h+='<button class="ic-btn ic-btn-ghost st-mini st-danger"'+on('deleteFromGallery',o.slug)+'>Delete</button>';
    $('ovg-actions').innerHTML=h;
  }
  // An alert design: which alerts play it, Edit in Studio, Use for every
  // alert; then Duplicate, and Delete (a ready-made one: Reset, as it came).
  function designMeta(o){
    const u=Integrations.designUse(o.slug), n=u ? u.names.length : 0;
    if(u) $('ovg-kind').textContent = !u.total ? 'No alerts yet' : u.everywhere ? 'Every alert plays it'
      : n ? 'Plays for '+n+' of '+u.total+' alerts' : 'No alert plays it yet';
    $('ovg-primary').innerHTML=(u && !u.total ? '<button class="ic-btn ic-btn-primary st-withic"'+on('setUpAlerts')+'>'+stIcon(UI_ICON.plus)+'Set up Twitch alerts</button>' : '')+
      '<button class="ic-btn '+(u && !u.total ? 'ic-btn-ghost' : 'ic-btn-primary')+' st-withic"'+on('editDesign',o.slug)+'>'+stIcon(UI_ICON.pen)+'Edit in Studio</button>'+
      (u && u.total && !u.everywhere ? '<button class="ic-btn ic-btn-ghost"'+on('useDesign',o.slug)+' title="Alerts you add later play it too">Use for every alert</button>' : '');
    $('ovg-actions').innerHTML='<button class="ic-btn ic-btn-ghost st-mini"'+on('duplicateDesign',o.slug)+'>Duplicate</button>'+
      (o.unsaved ? '' : readyTheme(o.slug) ? '<button class="ic-btn ic-btn-ghost st-mini"'+on('resetDesign',o.slug)+'>Reset</button>'
        : '<button class="ic-btn ic-btn-ghost st-mini st-danger"'+on('deleteFromGallery',o.slug)+'>Delete</button>');
  }
  // A ready-made design is written first, then edited or copied as any.
  async function readyDesign(slugId){
    const o=galleryItems().find(x=>x.slug===slugId);
    if(!o || !o.unsaved) return true;
    if(!(await Integrations.ensureDesign(slugId))) return false;
    await loadSaved(); return true;
  }
  async function editDesign(slugId){ if(await readyDesign(slugId)) openStudio(slugId); }
  async function duplicateDesign(slugId){ if(await readyDesign(slugId)) duplicateOverlay(slugId); }
  async function useDesign(slugId, btn){
    // Alerts given a design of their own in Integrations lose it: say so.
    const u=Integrations.designUse(slugId);
    if(u && u.own && !confirm((u.own===1 ? '1 alert has a design of its own' : u.own+' alerts have a design of their own')+', picked in Integrations. Play this one on every alert instead?')) return;
    btn.disabled=true;
    if(!(await Integrations.useDesignEverywhere(slugId))){ btn.disabled=false; return; }
    await loadSaved(); renderGallery(true);
  }
  // A ready-made design you changed, written again as it came: the alerts
  // that play it keep playing it.
  async function resetDesign(slugId){
    const o=curOverlay();
    if(!confirm('Put "'+((o&&o.name)||slugId)+'" back as it came? Your changes to it are lost.')) return;
    const r=await fetch('/overlays/'+encodeURIComponent(slugId),{method:'POST',headers:{'Content-Type':'text/html'},body:IC().bake(IC().alertPreset(readyTheme(slugId)))}).then(x=>x.json(),()=>null);
    if(!r || !r.ok){ toast('Could not reset it'+(r && r.error ? ': '+r.error : ''),'err'); return; }
    await loadSaved(); renderGallery();
    toast('Back as it came','ok');
  }
  // The alerts' data is in (or changed): the Alerts page says again which
  // alerts play each design, its preview playing on.
  function alertsChanged(){
    if(page!=='alerts' || $('ov-studio').hidden===false) return;
    loadSaved().then(()=>renderGallery(true));
  }
  // The path, not the whole address, and free to wrap after /overlay/ when
  // its name is long; Copy URL copies the whole address.
  function showUrl(){
    const u=$('ovg-url'), path=selUrl().slice(location.origin.length);
    u.innerHTML='/overlay/<wbr>'+escapeHtml(path.slice('/overlay/'.length)); u.title=selUrl();
  }
  // A browser source for the chosen overlay, in the scene on air, through
  // the OBS connection Integrations use (as the Goals page's button does).
  function addGalleryToObs(btn){
    const o=curOverlay(); if(o) Integrations.addOverlayToObs(btn, o.slug, o.name||o.slug);
  }
  function copyGallery(btn){ if(gsel) copyText(btn, selUrl(),'Overlay URL'); }
  // A built-in overlay, written as it came.
  function savePreset(p){
    const d=clone(p); d.widgets.forEach((w,n)=>{ w.id='w'+(n+1); });
    return fetch('/overlays/'+encodeURIComponent(slugify(p.name)),{method:'POST',headers:{'Content-Type':'text/html'},body:IC().bake(d)});
  }
  // Restore built-ins: each built-in delay overlay written again as it came
  // (back if deleted). Nothing else is touched.
  async function restoreDefaults(){
    if(!confirm('Put the '+IC().PRESETS.length+' built-in delay overlays back as they came?\n\nAny you changed are reset and any you deleted come back. Your own overlays, goals and timers are kept.')) return;
    const saved=await Promise.all(IC().PRESETS.map(p=>savePreset(p).then(r=>r.ok, ()=>false)));
    const failed=saved.filter(ok=>!ok).length;
    await loadSaved(); renderGallery();
    toast(failed ? 'Could not restore '+failed+' of them' : 'The built-in delay overlays are back', failed ? 'err' : 'ok');
  }
  async function deleteFromGallery(slugId){
    const o=savedList.find(x=>x.slug===slugId);
    // An alert design's alerts play another: say which.
    const u=o && o.kind==='alerts' ? Integrations.designUse(slugId) : null, n=u ? u.names.length : 0;
    const after=n ? '\n\n'+n+(n===1?' alert plays':' alerts play')+' it: '+(n===1?'it':'they')+' will play '+(u.isDefault ? 'the ready-made Island' : u.defaultName)+' instead.' : '';
    if(!confirm('Delete overlay "'+((o&&o.name)||slugId)+'"? This removes its file from the overlays folder.'+after)) return;
    const r = await postAction('/overlays/'+encodeURIComponent(slugId)+'/delete');
    if(!r.ok){ toast(`Could not delete the overlay: ${r.error}`,'err'); return; }
    if(o && o.kind==='alerts') await Integrations.designGone(slugId);
    toast('Deleted','info');
    gsel=null;
    await loadSaved(); renderGallery();
  }

  // ---- canvas ----
  function rebake(){
    const html = IC().bake(doc);
    const f = $('st-canvas');
    f.onload = function(){ studioDrive(); buildHits(); watchLayout(); };
    f.srcdoc = html;
    updateUrlLabel();
    recordSoon(); updatePerfBadge();
  }
  // Drive the studio canvas: lifecycle while playing, else pin the edited state.
  function studioDrive(){
    if(studioSim){ studioSim.stop(); }
    // While pinned, keep widgets visible for editing (noHide) - UNLESS the
    // edited state has an auto-hide, in which case let it play so the exit is
    // visible (makeSim loops it back into view every ~5s).
    // Longest hide configured in the edited state - the preview re-shows a
    // hidden widget this long after it appears (+ a beat), so even a 10s hide
    // actually plays its exit instead of being reset before it fires.
    const maxHide = doc.widgets.reduce((m,w)=>{ const h=w.states&&w.states[editState]&&w.states[editState].hide_after_ms; return h>m?h:m; }, 0);
    const hasHide = maxHide>0;
    studioSim=makeSim($('st-canvas'), playing?'auto':editState, (st,p)=>{
      if(mode().sim==='states') setCaption('st-phase',st,p); else $('st-phase').textContent='';
      if(playing) $$('#st-states .st-st').forEach(b=>b.classList.toggle('on',b.dataset.state===st));
    }, !playing && !hasHide, hasHide ? maxHide+1800 : 5000);
  }
  // ---- canvas: picking, moving and resizing layers ----
  // A widget's box as laid out, not where an animation has it this frame:
  // measured mid-entrance, a box would sit where the slide started.
  function layoutBox(el){ return { left:el.offsetLeft, top:el.offsetTop, width:el.offsetWidth, height:el.offsetHeight }; }
  function kindOf(w){ return IC().KINDS.find(x=>x.key===w.kind)||{fields:[]}; }
  // A layer's element in the canvas page.
  function liveEl(id){ const cdoc=$('st-canvas').contentDocument; return cdoc ? cdoc.getElementById(String(id).replace(/[^a-zA-Z0-9_-]/g,'')) : null; }
  function hitBoxOf(id){ return $('st-hit').querySelector('.st-box[data-id="'+CSS.escape(id)+'"]'); }
  // The offset a move changes: the edited state's own ("Move in Active")
  // when it has one, else the layer's.
  function offsetOf(w){ const sp=!w.always && w.states && w.states[editState]; return sp && sp.offset ? sp.offset : w.offset; }
  // The boxes of the layers showing in the edited state, by id, as last
  // laid out: what picking, snapping and lining up measure.
  let boxes = new Map();
  function buildHits(){
    const hit = $('st-hit'), cdoc = $('st-canvas').contentDocument;
    hit.querySelectorAll('.st-box,.st-group').forEach(n=>n.remove());
    boxes = new Map();
    if(!cdoc) return;
    // Measure with the edited state applied: the baked page starts at
    // data-state="idle" where most widgets are display:none (rect 0x0),
    // which would pin every hitbox to the top-left corner.
    if(cdoc.body) cdoc.body.setAttribute('data-state', editState);
    doc.widgets.forEach(w=>{
      const el = liveEl(w.id); if(!el) return;
      const r = layoutBox(el);
      if(r.width<1 && r.height<1) return; // hidden in this state - no handle
      boxes.set(w.id, r);
      const box = document.createElement('div');
      // A locked layer lets the pointer through to the layers under it.
      box.className = 'st-box'+(w.locked ? ' locked' : ''); box.dataset.id = w.id;
      box.style.left = r.left+'px'; box.style.top = r.top+'px';
      box.style.width = Math.max(16,r.width)+'px'; box.style.height = Math.max(16,r.height)+'px';
      hit.appendChild(box);
    });
    drawSelection();
  }
  // The canvas lays itself out again on its own (a web font arriving, the
  // sample alert's words, words shrunk to fit): the boxes follow, or a
  // drag would start from where a layer was. Not while a drag runs (it
  // redraws them itself) or the delay plays (its states come and go).
  let layoutWatch = null, rehitFrame = 0;
  function watchLayout(){
    if(layoutWatch) layoutWatch.disconnect();
    layoutWatch = null;
    const f = $('st-canvas'), cwin = f.contentWindow, cdoc = f.contentDocument;
    if(!cwin || !cdoc || !cwin.ResizeObserver) return;
    layoutWatch = new cwin.ResizeObserver(()=>{
      if(rehitFrame) return;
      rehitFrame = requestAnimationFrame(()=>{ rehitFrame = 0; if(!gesture && !playing) buildHits(); });
    });
    cdoc.querySelectorAll('.icw').forEach(el=>layoutWatch.observe(el));
  }
  // The sides and corners a picked layer is resized by. One sized by its
  // font (a readout, a label) grows from its corner, and words kept to a
  // width have their sides too.
  const HANDLES = ['nw','n','ne','e','se','s','sw','w'];
  // The least a layer measures on screen, across or down, to have handles
  // in the middle of its sides that way.
  const SIDE_HANDLE_PX = 48;
  const fontSized = w => (kindOf(w).fields||[]).includes('font');
  function handlesOf(w){
    if(kindOf(w).full || w.locked) return [];
    if(!fontSized(w)) return HANDLES;
    return w.kind==='Text' && (w.fit || w.wrap) ? ['w','e','se'] : ['se'];
  }
  // The picked layers marked, on the canvas and in Layers: one with its
  // handles, several with a box around them all.
  function drawSelection(){
    const hit = $('st-hit');
    hit.querySelectorAll('.st-handles,.st-group').forEach(n=>n.remove());
    hit.querySelectorAll('.st-box').forEach(b=>b.classList.toggle('sel', picked.includes(b.dataset.id)));
    $$('#st-left-body .st-layer').forEach(b=>b.classList.toggle('sel', picked.includes(b.dataset.id)));
    const shown = picked.filter(id=>boxes.has(id));
    if(picked.length===1 && shown.length===1){
      const box = hitBoxOf(sel), b = boxes.get(sel);
      // A layer small on screen keeps its middle for dragging: a short one
      // has no top or bottom handle (their grips would cover it), a narrow
      // one no left or right handle.
      const tall = b.height * scale >= SIDE_HANDLE_PX, wide = b.width * scale >= SIDE_HANDLE_PX;
      const list = handlesOf(widget(sel)).filter(h=>h.length===2 || (h==='n' || h==='s' ? tall : wide));
      if(list.length){
        // Last in the canvas, so on top of every layer's box, over its own.
        const over = document.createElement('div'); over.className = 'st-handles';
        ['left', 'top', 'width', 'height'].forEach(k=>{ over.style[k] = box.style[k]; });
        list.forEach(h=>{ const d=document.createElement('div'); d.className='st-handle'; d.dataset.handle=h; over.appendChild(d); });
        hit.appendChild(over);
      }
    } else if(shown.length>1){
      const g = G.bounds(shown.map(id=>boxes.get(id))), o = document.createElement('div');
      o.className = 'st-group';
      o.style.left = g.left+'px'; o.style.top = g.top+'px'; o.style.width = g.width+'px'; o.style.height = g.height+'px';
      hit.appendChild(o);
    }
  }

  // One press of the pointer on the canvas, followed until it lets go
  // (captured, so leaving the canvas or the window doesn't lose it) or
  // Escape cancels it. `move(dx, dy, ev)` gets the distance in canvas px,
  // at most once a frame and only past a few px (a click isn't a drag);
  // `done(dragged)` or `cancel()` ends it.
  let gesture = null;
  const DRAG_PX = 3;
  function follow(e, how){
    const hit = $('st-hit'), x0 = e.clientX, y0 = e.clientY;
    let dragged = false, frame = 0, last = null;
    const step = () => {
      frame = 0;
      if(!dragged && Math.hypot(last.clientX - x0, last.clientY - y0) < DRAG_PX) return;
      dragged = true;
      how.move((last.clientX - x0) / scale, (last.clientY - y0) / scale, last);
    };
    const onMove = ev => { if(ev.pointerId !== e.pointerId) return; last = ev; if(!frame) frame = requestAnimationFrame(step); };
    const end = ok => {
      if(frame) cancelAnimationFrame(frame);
      if(ok && last) step();
      hit.removeEventListener('pointermove', onMove);
      hit.removeEventListener('pointerup', onUp);
      hit.removeEventListener('pointercancel', onCancel);
      if(hit.hasPointerCapture(e.pointerId)) hit.releasePointerCapture(e.pointerId);
      gesture = null;
      if(ok) how.done(dragged); else how.cancel();
    };
    const onUp = ev => { if(ev.pointerId === e.pointerId){ last = ev; end(true); } };
    const onCancel = ev => { if(ev.pointerId === e.pointerId) end(false); };
    hit.setPointerCapture(e.pointerId);
    hit.addEventListener('pointermove', onMove);
    hit.addEventListener('pointerup', onUp);
    hit.addEventListener('pointercancel', onCancel);
    gesture = { cancel: () => end(false) };
  }
  function onCanvasDown(e){
    if(gesture || !doc) return;
    if(e.button===1 || (e.button===0 && spaceHeld)){ e.preventDefault(); startPan(e); return; }
    if(e.button!==0) return;
    e.preventDefault();
    // Arrow keys and Delete act on the canvas once it is clicked.
    if(document.activeElement && document.activeElement!==document.body) document.activeElement.blur();
    const handle = e.target.closest('.st-handle'), box = e.target.closest('.st-box');
    if(handle){ startResize(e, handle.dataset.handle); return; }
    if(!box){ startBand(e); return; }
    // Shift, Ctrl or Cmd adds a layer, or takes a picked one out. A drag
    // from a picked layer moves them all; a click on one picks it alone,
    // or with the key takes it out (on letting go, so Shift can drag too).
    const id = box.dataset.id, adding = e.shiftKey || e.ctrlKey || e.metaKey;
    let onClick = null;
    if(!picked.includes(id)) pick(adding ? picked.concat(id) : [id]);
    else if(adding) onClick = () => pick(picked.filter(x=>x!==id));
    else if(picked.length>1) onClick = () => pick([id]);
    startMove(e, onClick);
  }
  // A move the editor makes lands at once: the page's own glide (a layer
  // moving between delay states, .icw's transition) is for the stream, not
  // for a drag, which would jump back and glide to where it was dropped.
  let stillT = 0;
  function placeNow(){
    const cdoc = $('st-canvas').contentDocument;
    if(cdoc && cdoc.body){
      if(!cdoc.getElementById('st-still')){
        const css = cdoc.createElement('style'); css.id = 'st-still';
        css.textContent = 'body.st-still .icw{transition:none!important}';
        cdoc.head.appendChild(css);
      }
      cdoc.body.classList.add('st-still');
      clearTimeout(stillT); stillT = setTimeout(()=>cdoc.body.classList.remove('st-still'), 300);
    }
    liveStyle();
  }
  // Snapping reaches this far, in screen px; Alt moves freely.
  const SNAP_PX = 6;
  function showGuides(x, y){
    const gv=$('st-guide-v'), gh=$('st-guide-h');
    gv.classList.toggle('on', x!=null); if(x!=null) gv.style.left=x+'px';
    gh.classList.toggle('on', y!=null); if(y!=null) gh.style.top=y+'px';
  }
  // The picked layers a move or a line-up takes: the ones showing, not
  // locked and not full-screen (a wash, an edge glow stays where it is).
  const movable = () => picked.filter(id=>{ const w=widget(id); return boxes.has(id) && !w.locked && !kindOf(w).full; });
  // Drag the picked layers, all together, snapping to the canvas and the
  // other layers. Shift keeps the move straight across or down.
  function startMove(e, onClick){
    const ids = movable();
    const from = new Map(ids.map(id=>[id, boxes.get(id)]));
    const group = ids.length ? G.bounds([...from.values()]) : null;
    const lines = G.snapLines([...boxes].filter(([id])=>!from.has(id)).map(([,b])=>b));
    const marks = () => ids.map(liveEl).concat(ids.map(hitBoxOf), [...$('st-hit').querySelectorAll('.st-group,.st-handles')]).filter(Boolean);
    let dx = 0, dy = 0;
    follow(e, {
      move(mx, my, ev){
        if(!group) return;
        const across = ev.shiftKey && Math.abs(mx) >= Math.abs(my), down = ev.shiftKey && !across;
        if(across) my = 0; if(down) mx = 0;
        const s = ev.altKey ? null : G.snap(G.moved(group, mx, my), lines, SNAP_PX/scale);
        const snapX = s && !down ? s.x : null, snapY = s && !across ? s.y : null;
        dx = mx + (snapX!=null ? s.dx : 0); dy = my + (snapY!=null ? s.dy : 0);
        showGuides(snapX, snapY);
        // `translate`, not `transform`: a finished entrance animation holds
        // transform:none over any inline one, and the layer wouldn't follow.
        marks().forEach(n=>{ n.style.translate = dx+'px '+dy+'px'; });
      },
      done(dragged){
        showGuides(null, null);
        if(!dragged){ if(onClick) onClick(); return; }
        ids.forEach(id=>{ const w=widget(id), off=offsetOf(w); Object.assign(off, G.placeBox(w.anchor, off, from.get(id), G.moved(from.get(id), dx, dy))); });
        placeNow(); renderProps();
      },
      cancel(){ showGuides(null, null); marks().forEach(n=>{ n.style.translate = ''; }); },
    });
  }
  // Resize the one picked layer by a handle: the opposite side stays put,
  // whatever it is anchored to. A corner of one sized by its font scales
  // its words instead.
  function startResize(e, handle){
    const w = widget(sel), from = boxes.get(sel); if(!w || !from) return;
    const off0 = clone(offsetOf(w)), size0 = clone(w.size), font0 = w.font_px || 34;
    const byFont = fontSized(w) && handle.length===2;
    follow(e, {
      move(dx, dy, ev){
        if(byFont){
          // Words kept to a width scale with it, or they couldn't grow.
          const k = G.cornerScale(from, dx, dy);
          w.font_px = Math.max(8, Math.round(font0 * k));
          if(w.fit || w.wrap) w.size = {...size0, w:Math.max(8, Math.round(size0.w * k))};
          placeNow();
          // Its top-left stays where it was, whatever it is anchored to
          // (anchored top-left, it already has).
          if(w.anchor==='top-left') return;
          const el = liveEl(w.id); if(!el) return;
          const now = layoutBox(el);
          Object.assign(offsetOf(w), G.placeBox(w.anchor, offsetOf(w), now, {...now, left:from.left, top:from.top}));
          placeNow();
          return;
        }
        const to = G.resizeBox(from, handle, dx, dy, ev.shiftKey);
        w.size = {w:Math.max(8, size0.w + to.width - from.width), h:Math.max(8, size0.h + to.height - from.height)};
        Object.assign(offsetOf(w), G.placeBox(w.anchor, off0, from, to));
        placeNow();
      },
      done(){ renderProps(); },
      cancel(){ Object.assign(offsetOf(w), off0); w.size = size0; if(byFont) w.font_px = font0; placeNow(); },
    });
  }
  // A drag on empty canvas: a band picks every layer it touches (with Shift,
  // Ctrl or Cmd, as well as those picked already). A click there picks none.
  function startBand(e){
    const hit = $('st-hit'), r = hit.getBoundingClientRect();
    const x0 = (e.clientX - r.left)/scale, y0 = (e.clientY - r.top)/scale;
    const adding = e.shiftKey || e.ctrlKey || e.metaKey, before = adding ? picked.slice() : [];
    const band = document.createElement('div'); band.className = 'st-band'; band.hidden = true; hit.appendChild(band);
    follow(e, {
      move(dx, dy){
        const area = {left:Math.min(x0, x0+dx), top:Math.min(y0, y0+dy), width:Math.abs(dx), height:Math.abs(dy)};
        band.hidden = false;
        band.style.left = area.left+'px'; band.style.top = area.top+'px'; band.style.width = area.width+'px'; band.style.height = area.height+'px';
        const inside = [...boxes].filter(([id,b])=>!widget(id).locked && G.overlaps(b, area)).map(([id])=>id);
        const next = before.concat(inside.filter(id=>!before.includes(id)));
        if(next.join()!==picked.join()) pick(next);
      },
      done(dragged){ band.remove(); if(!dragged && !adding) pick([]); },
      cancel(){ band.remove(); pick(before); },
    });
  }
  // The arrow keys: the picked layers a pixel (with Shift, ten) at a time.
  function nudge(dx, dy){
    picked.forEach(id=>{
      const w = widget(id); if(!w || w.locked || kindOf(w).full) return;
      const off = offsetOf(w), b = boxes.get(id) || {left:0, top:0, width:0, height:0};
      Object.assign(off, G.placeBox(w.anchor, off, b, G.moved(b, dx, dy)));
    });
    placeNow(); showPlace();
  }
  // The one picked layer's place and size in its settings, written in
  // the fields as they are (a key held down would rebuild the panel at
  // every repeat). Several picked have no such fields.
  function showPlace(){
    const w = picked.length===1 ? widget(sel) : null; if(!w) return;
    const sp = !w.always && w.states && w.states[editState], here = (sp && sp.offset) || w.offset;
    const shown = {'offset.x':w.offset.x, 'offset.y':w.offset.y, 'size.w':w.size.w, 'size.h':w.size.h, font_px:w.font_px||34};
    $('st-props').querySelectorAll('[data-st="setNum"],[data-st="setStateOffset"]').forEach(el=>{
      let key; try{ key = JSON.parse(el.dataset.stArgs)[0]; }catch(_){ return; }
      const v = el.dataset.st==='setStateOffset' ? here[key] : shown[key];
      if(v!=null && el!==document.activeElement) el.value = v;
    });
  }
  // Line the picked layers up, or spread them evenly ('x', 'y').
  function lineUp(how){
    const ids = movable();
    if(ids.length<2) return;
    const from = ids.map(id=>boxes.get(id));
    const to = how==='x' || how==='y' ? G.distribute(from, how) : G.align(from, how);
    ids.forEach((id, i)=>{ const w=widget(id), off=offsetOf(w); Object.assign(off, G.placeBox(w.anchor, off, from[i], to[i])); });
    placeNow(); renderProps();
  }
  // ---- state selector (the state you preview IS the state you edit) ----
  function renderStates(){
    const c = $('st-states'); if(!c) return; c.innerHTML='';
    const sim = mode().sim;
    $('st-play').hidden = sim!=='states';
    if(sim!=='states'){ simBar(c, sim); return; }
    IC().STATES.forEach(st=>{
      const b=document.createElement('button');
      b.className='st-st'+(editState===st.key?' on':''); b.dataset.state=st.key; b.title=st.hint;
      b.innerHTML='<span class="dot" style="background:'+STATE_DOT[st.key]+'"></span>'+st.label;
      b.onclick=()=>selectState(st.key);
      c.appendChild(b);
    });
  }
  // The bar of an overlay that isn't about the delay.
  function simBar(c, sim){
    const btn = (label, title, act, id) => {
      const b = document.createElement('button'); b.className = 'st-st'; b.textContent = label; b.title = title; b.onclick = act;
      if(id) b.id = id; c.appendChild(b); return b;
    };
    if(sim==='alert') btn('▶ Play the alert', 'Play a sample alert: in, hold, out', playAlert);
    if(sim==='goals'){
      btn('−1', 'Take one from every goal', () => nudgeGoals(-1));
      btn('+1', 'Add one to every goal', () => nudgeGoals(1));
      btn('Count up and down', 'Goals fill and empty by themselves', goalAuto, 'st-goal-auto');
    }
    const say = {alert:'Waits hidden until an alert plays', goals:'Shows your goals and timers', none:'Looks the same whatever the delay does'}[sim];
    const note = document.createElement('span'); note.className = 'st-bar-note'; note.textContent = say; c.appendChild(note);
  }
  // The state an overlay is edited in, and its preview, for what it is for.
  function enterMode(){
    const sim = mode().sim;
    zoomFit();
    if(sim!=='states') editState = 'active';
    simGoals = null; stopGoalAuto();
    if(sim==='goals') startGoalSim();
    $('st-foot-edit').hidden = sim!=='states';
    renderStates();
  }
  function selectState(st){
    playing=false; editState=st; playLabel();
    $$('#st-states .st-st').forEach(b=>b.classList.toggle('on',b.dataset.state===st));
    const fl=$('st-foot-state'); if(fl) fl.textContent=(IC().STATES.find(s=>s.key===st)||{label:st}).label;
    studioDrive(); renderProps();
  }

  // ---- left panel: Layers + Add ----
  // Each widget kind's icon, in the dashboard's line style (integrations.js
  // PATHS): 24 box, 2 stroke, round ends.
  const KIND_ICON = {
    DelayReadout:'M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 6v6l4 2', StatePill:'M7 8h10a4 4 0 0 1 0 8H7a4 4 0 0 1 0-8z',
    BufferBar:'M3 10h18v4H3zM3 12h11', BufferRing:'M12 21a9 9 0 1 1 9-9', BitrateMeter:'M22 12h-4l-3 9L9 3l-3 9H2',
    DestinationStatus:'M5 12h.01M12 12h.01M19 12h.01', CutCounter:'M6 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM20 4L8.1 15.9M14.5 14.5L20 20M8.1 8.1L12 12',
    SessionElapsed:'M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 7v5h4', Goal:'M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 18a6 6 0 1 0 0-12 6 6 0 0 0 0 12zM12 14a2 2 0 1 0 0-4 2 2 0 0 0 0 4z',
    Timer:'M6 2h12M6 22h12M7 2v4a5 5 0 0 0 10 0V2M7 22v-4a5 5 0 0 1 10 0v4', Text:'M4 7V4h16v3M9 20h6M12 4v16',
    Icon:'M12 2l3.1 6.3 6.9 1-5 4.9 1.2 6.8L12 17.8 5.8 21l1.2-6.8-5-4.9 6.9-1z', Shape:'M4 4h16v16H4z', Divider:'M3 12h18',
    CornerFrame:'M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5', PulsingDot:'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM12 20a8 8 0 1 0 0-16',
    Heartbeat:'M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21.2l8.8-8.8a5.5 5.5 0 0 0 0-7.8z',
    Banner:'M3 14h18v6H3zM6 17h8', EdgeAccent:'M3 3h18v18H3zM7 7h10v10H7z', BackgroundTint:'M3 3h18v18H3zM3 9h18M3 15h18',
    LiquidFill:'M3 14c3-2 6 2 9 0s6-2 9 0v7H3z', Image:'M3 4h18v16H3zM21 15l-5-5L5 20M9 10a1 1 0 1 0 0-.01', AlertCard:'M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0', CustomHTML:'M8 7l-5 5 5 5M16 7l5 5-5 5'
  };
  const UI_ICON = { up:'M18 15l-6-6-6 6', down:'M6 9l6 6 6-6', x:'M18 6L6 18M6 6l12 12', pick:'M5 3l14 7.5-6.2 1.7L10 18.5z',
    plus:'M12 5v14M5 12h14', lock:'M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4', unlock:'M5 11h14v10H5zM8 11V7a4 4 0 0 1 7.5-2', pen:'M4 20h4L19 9l-4-4L4 16zM14 6l4 4', layers:'M12 3l9 4.5-9 4.5-9-4.5zM3 12l9 4.5 9-4.5M3 16.5l9 4.5 9-4.5',
    hourglass:'M6 3h12M6 21h12M7 3v3.5L12 12l-5 5.5V21M17 3v3.5L12 12l5 5.5V21', shapes:'M4 4h7v7H4zM17.5 4.5l3.5 6h-7zM17.5 14a3.5 3.5 0 1 1 0 7 3.5 3.5 0 0 1 0-7zM4 14h7v7H4z' };
  function stIcon(path){ return '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="'+path+'"/></svg>'; }
  function kindIcon(kind){ return stIcon(KIND_ICON[kind]||KIND_ICON.Shape); }
  // A layer's name: its own (Name, at the top of its properties), or what
  // it shows, as its row says it: a goal's words read with the goal's own
  // numbers, not as {{goal.sub_goal.target}}.
  function layerName(w, k){
    if(w.name) return w.name;
    const named = (list, id) => (list||[]).find(x=>x.id===id) || (list||[])[0];
    if(w.kind==='Goal'){ const g=named(tracked.goals,w.goal); return g ? g.name+' bar' : k.label; }
    if(w.kind==='Timer'){ const t=named(tracked.timers,w.timer); return t ? t.name+' clock' : k.label; }
    if(w.kind==='Image' && /\{\{alert\.image\}\}/.test(w.src||'')) return 'Alert picture';
    const text=(w.base&&w.base.text)||'';
    if(!text) return k.label;
    const said=text.replace(/\{\{alert\.(title|text)\}\}/g,(m,part)=>part==='title'?'Alert title':'Alert words').replace(/\{\{(goal|timer)\.([a-z0-9_]+)(?:\.([a-z]+))?\}\}/g,(m,what,id,part)=>{
      const x=((what==='goal'?tracked.goals:tracked.timers)||[]).find(y=>y.id===id); if(!x) return m;
      if(part==='name') return x.name;
      if(what==='timer') return x.name+' clock';
      const v={target:x.target, left:Math.max(0,x.target-x.value), percent:Math.floor(Math.min(1,x.value/(x.target||1))*100)+'%'}[part];
      return String(v!=null ? v : x.value);
    });
    return said;
  }
  function leftTab(name){ left=name; $$('#st-left-tabs button').forEach(b=>b.classList.toggle('on',b.dataset.stleft===name)); renderLeft(); }
  // Representative colour for a layer swatch: the current edit-state colour if
  // set, else the base, resolved through the theme so @tokens show real hues.
  function layerColor(w){ return themeHex((w.states&&w.states[editState]&&w.states[editState].color)||(w.base&&w.base.color)||'#5ac8fa'); }
  // Update layer swatches in place (no panel rebuild, so open pickers survive).
  function refreshLayerColors(){ if(left!=='layers') return;
    document.querySelectorAll('#st-left-body .st-layer').forEach(function(row){
      const w=widget(row.dataset.id); if(!w) return; const sw=row.querySelector('.st-layer-sw'); if(sw) sw.style.background=layerColor(w); }); }
  function renderLeft(){
    const c = $('st-left-body'); c.innerHTML='';
    if(left==='add'){
      const groups={alert:'The alert',data:'Delay',stream:'Goals and timers',ambient:'Ambient',deco:'Decoration',media:'Media',power:'Power user'};
      // A tile per widget, in groups. Goals and timers add a ready text of
      // their numbers too, since that is what a goal's words usually say.
      const tile=(icon,label,cost,act)=>{
        const b=document.createElement('button'); b.className='st-item';
        b.innerHTML='<span class="st-glyph">'+icon+'</span><span class="st-item-l"></span>'+cost;
        b.querySelector('.st-item-l').textContent=label; b.onclick=act; return b;
      };
      mode().groups.forEach(g=>{
        if(g==='alert'){
          const h=document.createElement('div'); h.className='st-sec'; h.textContent=groups[g]; c.appendChild(h);
          const grid=document.createElement('div'); grid.className='st-tiles'; c.appendChild(grid);
          grid.appendChild(tile(kindIcon('AlertCard'), 'Alert card', '', ()=>addWidget('AlertCard')));
          grid.appendChild(tile(kindIcon('Image'), 'Its picture', '', ()=>addAlertPart('picture')));
          grid.appendChild(tile(kindIcon('Text'), 'Its title', '', ()=>addAlertPart('title')));
          grid.appendChild(tile(kindIcon('Text'), 'Its words', '', ()=>addAlertPart('text')));
          return;
        }
        const items=IC().KINDS.filter(k=>k.group===g); if(!items.length) return;
        const h=document.createElement('div'); h.className='st-sec'; h.textContent=groups[g]; c.appendChild(h);
        const grid=document.createElement('div'); grid.className='st-tiles'; c.appendChild(grid);
        items.forEach(k=>{
          const cost = k.cost==='mid'?'<span class="st-cost">GPU</span>':(k.cost==='opt'?'<span class="st-cost">CODE</span>':'');
          grid.appendChild(tile(kindIcon(k.key), k.label, cost, ()=>addWidget(k.key)));
        });
        if(g==='stream') grid.appendChild(tile(kindIcon('Text'), 'Goal numbers', '', addGoalText));
      });
    } else if(left==='theme'){
      const t=ensureTheme();
      const head=document.createElement('div'); head.className='st-sec'; head.textContent='Theme colours'; c.appendChild(head);
      [['accent','Accent'],['live','Live'],['warn','Arming'],['error','Error'],['ink','Text / ink'],['grey','Idle / grey']].forEach(k=>{
        const row=document.createElement('div'); row.className='st-field';
        row.innerHTML='<label>'+k[1]+'</label><div class="st-color"><input type="color" value="'+escapeAttr(t[k[0]])+'"'+on('setTheme',k[0])+'><input class="ic-input mono" value="'+escapeAttr(t[k[0]])+'"'+on('setTheme',k[0])+'></div>';
        c.appendChild(row);
      });
      const hint=document.createElement('div'); hint.className='st-hint'; hint.style.padding='10px 2px';
      hint.textContent='Pick a theme colour from the swatches under any widget colour. Those widgets recolour instantly when you change the theme here.';
      c.appendChild(hint);
      // An alert design's own sound: what its alerts set to "The design's
      // sound" play (an upload or a link carried over from a look stays).
      if(IC().docPurpose(doc)==='alerts'){
        const sh=document.createElement('div'); sh.className='st-sec'; sh.textContent='Its sound'; c.appendChild(sh);
        const cur=doc.sound||'', sounds=Integrations.alertKit.sounds.concat(Integrations.soundUploads()), known=!cur||sounds.some(([v])=>v===cur);
        const row=document.createElement('div'); row.className='st-field';
        row.innerHTML='<div class="st-field-row"><select class="ic-input" aria-label="The design\'s sound"><option value="">None of its own</option>'+
          sounds.map(([v,l])=>'<option value="'+escapeAttr(v)+'"'+(v===cur?' selected':'')+'>'+escapeHtml(l)+'</option>').join('')+
          (known ? '' : '<option value="'+escapeAttr(cur)+'" selected>'+escapeHtml(cur.slice(0,6)==='media:' ? cur.slice(6) : 'A link')+'</option>')+'</select>'+
          '<button class="ic-btn ic-btn-ghost" data-do="play" title="Hear it"'+(cur?'':' disabled')+'>Play</button><button class="ic-btn ic-btn-ghost" data-do="upload">Upload</button></div>';
        const useSound=v=>{ if(v) doc.sound=v; else delete doc.sound; rebake(); renderLeft(); };
        row.querySelector('select').oninput=e=>useSound(e.target.value);
        row.querySelector('[data-do="play"]').onclick=()=>{ if(doc.sound) Integrations.playSound(doc.sound); };
        row.querySelector('[data-do="upload"]').onclick=()=>Integrations.uploadSound().then(ref=>{ if(ref) useSound(ref); });
        c.appendChild(row);
        const said=document.createElement('div'); said.className='st-hint'; said.style.padding='6px 2px';
        said.textContent='Alerts whose sound is "The design\'s sound" play this one.';
        c.appendChild(said);
      }
      // Anything the controls don't reach: CSS of the streamer's own, last
      // in the page so it wins. Widgets are #w1, #w2... as in Layers.
      const own=document.createElement('div'); own.className='st-sec'; own.textContent='Your own CSS'; c.appendChild(own);
      const css=document.createElement('textarea'); css.className='ic-input mono st-css'; css.spellcheck=false;
      css.placeholder='#w1 .fill { border-radius: 4px }'; css.value=doc.css||'';
      css.oninput=()=>{ doc.css=css.value; softRebake(); };
      c.appendChild(css);
      const how=document.createElement('div'); how.className='st-hint'; how.style.padding='6px 2px';
      how.textContent='Each widget is #w1, #w2 and so on, as in Layers. A goal bar has .trk and .fill, a timer .v, a text .lbl.';
      c.appendChild(how);
    } else {
      // Layers - the widgets in this overlay, top = front-most.
      if(!doc || !doc.widgets.length){
        const e=document.createElement('div'); e.className='st-empty-hint';
        e.innerHTML='No widgets yet.<br>Place one from the <b>Add</b> tab.';
        c.appendChild(e); return;
      }
      doc.widgets.slice().reverse().forEach(w=>{
        const k=IC().KINDS.find(x=>x.key===w.kind)||{label:w.kind};
        const row=document.createElement('div'); row.className='st-layer'+(picked.includes(w.id)?' sel':'')+(arrived===w.id?' arrived':'')+(w.locked?' locked':''); row.dataset.id=w.id;
        const label=layerName(w,k);
        const col=layerColor(w);
        row.title=k.label;
        const lockSay=w.locked ? 'Unlock: pick and move it on the canvas again' : 'Lock: clicks on the canvas go through it';
        row.innerHTML='<span class="st-layer-sw" style="--sw:'+escapeAttr(col)+'">'+kindIcon(w.kind)+'</span><span class="st-layer-name"></span>'+
          '<button class="st-layer-btn st-lock" title="'+lockSay+'" aria-label="'+(w.locked?'Unlock':'Lock')+'" aria-pressed="'+!!w.locked+'" data-act="lock">'+stIcon(w.locked?UI_ICON.lock:UI_ICON.unlock)+'</button>'+
          '<button class="st-layer-btn" title="Bring forward" aria-label="Bring forward" data-act="up">'+stIcon(UI_ICON.up)+'</button>'+
          '<button class="st-layer-btn" title="Send back" aria-label="Send back" data-act="down">'+stIcon(UI_ICON.down)+'</button>'+
          '<button class="st-layer-btn" title="Delete" aria-label="Delete" data-act="del">'+stIcon(UI_ICON.x)+'</button>';
        row.querySelector('.st-layer-name').textContent=label;
        // A click picks it (Ctrl adds or takes out, Shift picks the run to it).
        row.addEventListener('click', ev=>{
          const act=(ev.target.closest('[data-act]')||{dataset:{}}).dataset.act;
          if(act==='lock') setLocked([w.id], !w.locked);
          else if(act==='del') delWidgetById(w.id);
          else if(act==='up') stepLayers(stackOf(w.id), 1);
          else if(act==='down') stepLayers(stackOf(w.id), -1);
          else clickPick(w.id, ev);
        });
        wireLayerRow(row, w.id);
        c.appendChild(row);
      });
      arrived=null;
    }
  }

  function addWidget(kind){ placeWidget(IC().defaultWidget(kind, nextId())); }
  // A layer made for, or brought to, this overlay shows its page's way: on
  // a custom overlay in every state, one look; on an alert design hidden
  // until an alert plays (an alert card comes and goes its theme's way by
  // itself). Brought from another page, a layer on a delay or goal
  // overlay shows in every state too, as it did where it was.
  function suitLayer(w, from){
    const purpose = IC().docPurpose(doc);
    if(purpose==='custom'){ w.always=true; w.states={}; }
    else if(purpose==='alerts'){ if(w.kind!=='AlertCard'){ w.always=true; w.states={idle:{visible:false}}; } }
    else if(from && from!==purpose){ w.always=true; w.states={}; }
  }
  function placeWidget(w){
    suitLayer(w);
    // Never exactly on another widget, where it couldn't be seen or grabbed.
    while(doc.widgets.some(o=>o.anchor===w.anchor && o.offset.x===w.offset.x && o.offset.y===w.offset.y)) w.offset={x:w.offset.x+24, y:w.offset.y+24};
    doc.widgets.push(w); picked=[w.id]; sel=w.id; arrived=w.id;
    left='layers'; $$('#st-left-tabs button').forEach(b=>b.classList.toggle('on',b.dataset.stleft==='layers'));
    renderLeft(); rebake(); renderProps();
  }
  // An alert card's look: a choice redraws it; a slider or a colour shows
  // on the canvas as it moves, and is kept with the next bake.
  function setCard(k, v, live){
    const w=widget(sel); if(!w) return;
    w.card=Object.assign({}, w.card);
    if(v===''||v==null||Number.isNaN(v)) delete w.card[k]; else w.card[k]=v;
    if(!live){ rebake(); renderProps(); return; }
    const cdoc=$('st-canvas').contentDocument, f=cdoc && cdoc.querySelector('#'+w.id+' iframe.ic-card');
    if(f){ f.setAttribute('data-look', JSON.stringify(w.card)); f.contentWindow.postMessage({__ic_card:{mode:'still', look:w.card}}, '*'); }
    recordSoon();
  }
  function setCardNum(k, v){ setCard(k, +v, true); }
  function setCardColor(k, v){ if(/^#[0-9a-f]{6}$/i.test(v)) setCard(k, v.toLowerCase(), true); }
  function resetCard(){ const w=widget(sel); if(!w) return; w.card={theme:(w.card||{}).theme||'island'}; rebake(); renderProps(); }
  // A part of the alert: its picture, its title or its words, as it plays.
  function addAlertPart(what){
    const w=IC().defaultWidget(what==='picture'?'Image':'Text', nextId());
    if(what==='picture'){ w.src='{{alert.image}}'; w.size={w:120,h:120}; }
    else { w.base.text='{{alert.'+what+'}}'; w.font_weight='700'; w.base.color='@ink'; }
    placeWidget(w);
  }
  // "12 / 50" for the first goal: a Text layer on stream in every state.
  function addGoalText(){
    const g=(tracked.goals||[])[0], id=g?g.id:'subs', w=IC().defaultWidget('Text', nextId());
    w.always=true; w.states={}; w.font_weight='700'; w.base.color='@ink';
    w.base.anim={style:'instant',duration_ms:400,easing:'ease-out',delay_ms:0};
    w.base.text='{{goal.'+id+'}} / {{goal.'+id+'.target}}';
    placeWidget(w);
  }
  async function openSaved(o){
    try{
      const html = await (await fetch('/overlay/'+encodeURIComponent(o.slug)+'.html')).text();
      const d = IC().readDoc(html);
      if(!d){ toast('That overlay was hand-written. Open the .html in the overlays folder to edit it','info'); return; }
      doc=IC().upgradeDoc(d); slug=o.slug; clearPicked();
      // One from before pages takes the page the app lists it on.
      if(!doc.purpose){ const listed=savedList.find(x=>x.slug===slug); if(listed) doc.purpose=listed.kind; }
      doc.widgets.forEach((w,n)=>{ if(!w.id) w.id='w'+(n+1); });
      $('st-name').value = doc.name;
      enterMode(); renderLeft(); rebake(); renderProps(); resetHistory();
    }catch(e){ toast('Could not load overlay','err'); }
  }

  // ---- picking + the settings panel ----
  // Pick `ids` (ones that exist, each once); the last is `sel`.
  function pick(ids){
    picked = (ids||[]).filter((id,i,all)=>all.indexOf(id)===i && !!widget(id));
    sel = picked.length ? picked[picked.length-1] : null;
    drawSelection(); renderProps();
  }
  function clearPicked(){ picked=[]; sel=null; }
  // After an undo or a delete: forget the picked layers that are gone.
  function forgetGone(){ picked=picked.filter(id=>!!widget(id)); sel=picked.length ? picked[picked.length-1] : null; }
  // A click on a row in Layers picks its layer alone. Ctrl or Cmd adds it
  // or takes it out; Shift picks the run from the last one picked.
  function clickPick(id, e){
    if(e.ctrlKey || e.metaKey){ pick(picked.includes(id) ? picked.filter(x=>x!==id) : picked.concat(id)); return; }
    if(e.shiftKey && sel){
      const order = doc.widgets.map(w=>w.id).reverse(), a = order.indexOf(sel), b = order.indexOf(id);
      pick(picked.filter(x=>x!==id).concat(order.slice(Math.min(a,b), Math.max(a,b)+1).filter(x=>x!==id), id));
      return;
    }
    pick([id]);
  }

  // What a control does, as data: on('setNum', 'offset.x') makes a field
  // call setNum('offset.x', its value), and a button call it with itself
  // last. Only the names in ACTIONS run (see `dispatch`).
  function on(name, ...args){ return ' data-st="'+name+'"'+(args.length ? ' data-st-args="'+escapeAttr(JSON.stringify(args))+'"' : ''); }
  function field(label, inner){ return '<div class="st-field"><label>'+label+'</label>'+inner+'</div>'; }
  function numF(label, path, val){ return field(label,'<input class="ic-input mono" type="number" value="'+escapeAttr(val)+'"'+on('setNum',path)+'>'); }
  // A slider's track fills up to its thumb (--p, as in the dashboard's), and
  // its value reads beside it as it moves (with its unit, when it has one).
  function rangeText(el){ const step=+el.step||1, v=+el.value; return step<1 ? v.toFixed(step<0.1?2:1).replace(/\.?0+$/,'') : String(v); }
  function fillStRange(el){
    fillRange(el);
    const out=el.nextElementSibling; if(out && out.tagName==='OUTPUT') out.textContent=rangeText(el)+(out.dataset.unit||'');
  }
  document.addEventListener('input', e=>{ if(e.target.matches && e.target.matches('#ov-studio input[type=range]')) fillStRange(e.target); });
  // Panel groups fold away by their heading, and stay as left.
  let foldedGroups = new Set();
  try{ foldedGroups = new Set(JSON.parse(localStorage.getItem('ic-st-folded')||'[]')); }catch(_){}
  function wireGroups(root){
    root.querySelectorAll('.st-pgroup').forEach(g=>{
      const head=g.querySelector('.st-pgroup-h'); if(!head) return;
      const name=(head.firstChild&&head.firstChild.textContent||head.textContent).replace(/·.*$/,'').trim();
      g.classList.toggle('folded', foldedGroups.has(name));
      head.setAttribute('role','button'); head.tabIndex=0; head.setAttribute('aria-expanded', String(!foldedGroups.has(name)));
      const flip=e=>{
        if(e.target.closest('button,input,select,label')) return;
        if(e.type==='keydown' && e.key!=='Enter' && e.key!==' ') return;
        e.preventDefault();
        const folded=!g.classList.contains('folded');
        g.classList.toggle('folded', folded); head.setAttribute('aria-expanded', String(!folded));
        if(folded) foldedGroups.add(name); else foldedGroups.delete(name);
        try{ localStorage.setItem('ic-st-folded', JSON.stringify([...foldedGroups])); }catch(_){}
      };
      head.addEventListener('click', flip); head.addEventListener('keydown', flip);
    });
  }
  // A slider with its value beside it, calling `fn(key, value)`.
  function rangeF(label, fn, key, val, min, max, step, unit){
    return field(label,'<div class="st-range"><input type="range" min="'+min+'" max="'+max+'" step="'+step+'" value="'+escapeAttr(val)+'"'+on(fn,key)+'>'+
      '<output'+(unit ? ' data-unit="'+unit+'"' : '')+'></output></div>');
  }
  // A colour: a picker and its value as text, calling `fn(...args, value)`;
  // with `swatches`, the theme's colours one click each.
  const THEME_KEYS = ['accent','live','warn','error','ink','grey'];
  function colorF(label, fn, args, val, swatches){
    const call = on(fn, ...args);
    return field(label,'<div class="st-color"><input type="color" value="'+escapeAttr(themeHex(val))+'"'+call+'><input class="ic-input mono" value="'+escapeAttr(val)+'"'+call+'></div>'+
      (swatches ? '<div class="st-swatches">'+THEME_KEYS.map(k=>'<button class="st-sw" style="background:'+escapeAttr(themeHex('@'+k))+'" title="theme '+k+'"'+on('swatch',fn,args,'@'+k)+'></button>').join('')+'</div>' : ''));
  }
  function swatch(fn, args, value){ ACTIONS[fn](...args, value); renderProps(); }
  // The layers whose words have a font, a weight and spacing.
  const TYPE_KINDS = ['DelayReadout','StatePill','Text','CutCounter','SessionElapsed','Banner','Timer'];
  function fontOptions(current){
    const sys=IC().SYSTEM_FONT;
    return '<option value="">Default (Inter)</option>'+IC().FONTS.map(n=>'<option'+(current===n?' selected':'')+'>'+escapeHtml(n)+'</option>').join('')+
      '<option value="'+sys+'"'+(current===sys?' selected':'')+'>The PC\'s own</option>';
  }
  function renderProps(){
    const p = $('st-props');
    if(picked.length>1){ renderManyProps(p); return; }
    const w = sel ? widget(sel) : null;
    // Redrawn for the widget already shown: keep where it was scrolled to.
    const was = p.querySelector('.st-props-body'), same = !!w && p.dataset.for === w.id;
    const scrolled = same && was ? was.scrollTop : 0;
    p.dataset.for = w ? w.id : '';
    if(!w){
      p.innerHTML='<div class="st-props-empty"><span class="st-empty-ic">'+stIcon(UI_ICON.pick)+'</span><b>Nothing selected</b>'+
        '<span>Click a layer, or a widget on the canvas, to style it. Shift or Ctrl picks more than one, and so does dragging a box around them. Press <kbd>?</kbd> for every shortcut.</span>'+
        '<button class="ic-btn ic-btn-ghost st-mini"'+on('leftTab','add')+'>Add a widget</button></div>';
      return;
    }
    const k = kindOf(w);
    const f = k.fields||[];
    const stLabel = (IC().STATES.find(s=>s.key===editState)||{label:editState}).label;
    let h = '<div class="st-props-head"><span class="st-props-ic" style="--sw:'+escapeAttr(layerColor(w))+'">'+kindIcon(w.kind)+'</span><div><div class="st-props-title">'+escapeHtml(k.label||w.kind)+'</div>'+
      '<div class="st-props-sub" title="Its name in your own CSS (Theme tab)">#'+escapeHtml(w.id)+'</div></div></div><div class="st-props-body">';

    // ---- GLOBAL: position & size (same in every state) ----
    h += '<div class="st-pgroup"><div class="st-pgroup-h">Position &amp; size</div>';
    if(w.locked) h += '<div class="st-hint">Locked, so the canvas picks what is under it. Unlock it in Layers to move it there.</div>';
    h += field('Name','<input class="ic-input" value="'+escapeAttr(w.name||'')+'" placeholder="'+escapeAttr(layerName(Object.assign({},w,{name:''}),k))+'" maxlength="40"'+on('setName')+'>');
    const anchorOpts = IC().ANCHORS.map(a=>'<option value="'+a+'"'+(w.anchor===a?' selected':'')+'>'+a+'</option>').join('');
    h += field('Anchor','<select class="ic-input" title="What it is placed from. Changing it leaves it where it is."'+on('setAnchor')+'>'+anchorOpts+'</select>');
    h += '<div class="st-field-row">'+numF('Offset X','offset.x',w.offset.x)+numF('Offset Y','offset.y',w.offset.y)+'</div>';
    if(!k.full){ h += '<div class="st-field"><label>Align</label><div class="st-align">'+
      [['tl','Top left','start','start'],['top','Top','center','start'],['tr','Top right','end','start'],['left','Left','start','center'],['center','Centre','center','center'],['right','Right','end','center'],['bl','Bottom left','start','end'],['bottom','Bottom','center','end'],['br','Bottom right','end','end']].map(a=>'<button class="st-alignb" title="'+a[1]+'" aria-label="Align '+a[1].toLowerCase()+'" style="justify-content:'+a[2]+';align-items:'+a[3]+'"'+on('align',a[0])+'><i></i></button>').join('')+'</div></div>'; }
    if(f.indexOf('font')>=0) h += numF('Font size','font_px',w.font_px||34);
    if(!(k.full)) h += '<div class="st-field-row">'+numF('Width','size.w',w.size.w)+numF('Height','size.h',w.size.h)+'</div>';
    // On an alert design: where it goes when the alert has no picture.
    if(mode().sim==='alert' && w.kind!=='Image') h += numF('With no picture, move left (px)','nopic',w.nopic||0);
    if(mode().sim==='alert') h += '<label class="st-check"><input type="checkbox"'+(w.with_message?' checked':'')+on('setFlag','with_message')+'> Only when the alert has a message (a cheer\'s, a resub\'s)</label>';
    if(f.indexOf('bind')>=0){
      const bo=Object.keys(IC().BINDINGS).map(b=>'<option value="'+b+'"'+(w.bind===b?' selected':'')+'>'+IC().BINDINGS[b].label+'</option>').join('');
      h += field('Shows','<select class="ic-input"'+on('set','bind')+'>'+bo+'</select>');
    }
    // A goal bar or a timer shows the first goal or timer there is, unless
    // one is picked. The canvas draws a sample while there are none.
    if(w.kind==='Goal'||w.kind==='Timer'){
      const isGoal=w.kind==='Goal', key=isGoal?'goal':'timer', list=(isGoal?tracked.goals:tracked.timers)||[];
      h += '</div><div class="st-pgroup"><div class="st-pgroup-h">'+(isGoal?'Goal bar':'Timer')+'</div>';
      let opts='<option value="">The first one</option>'+list.map(x=>'<option value="'+escapeAttr(x.id)+'"'+(w[key]===x.id?' selected':'')+'>'+escapeHtml(x.name)+'</option>').join('');
      if(w[key] && !list.some(x=>x.id===w[key])) opts+='<option value="'+escapeAttr(w[key])+'" selected>'+escapeHtml(w[key])+' (deleted)</option>';
      h += field(isGoal?'Goal':'Timer','<select class="ic-input"'+on('set',key)+'>'+opts+'</select>');
      if(!list.length) h += '<div class="st-hint">None yet, so the canvas shows a sample. Make one under <b>Goals and timers</b>.</div>';
      if(isGoal){
        h += '<div class="st-hint">Width and Height size the bar. Its name and numbers are Text layers of their own: add one and pick from the chips under its text.</div>';
        h += rangeF('Fill speed','setNum','smooth',(w.smooth!=null?w.smooth:0.7),0,2,0.1);
        h += '<div class="st-hint" style="margin-top:-4px">How long the bar takes to reach a new count (seconds).</div>';
        h += colorF('Track colour','set',['track'],w.track||'#0a0c10',true)+rangeF('Track opacity','setNum','trackOpacity',w.trackOpacity!=null?w.trackOpacity:0.6,0,1,0.05);
        h += rangeF('Bar roundness','setNum','radius',w.radius!=null?w.radius:20,0,20,1);
      } else {
        h += field('When it runs out, show','<input class="ic-input" value="'+escapeAttr(w.end||'')+'" placeholder="0:00"'+on('set','end')+'>');
        h += '<div class="st-hint">Its name is a Text layer of its own. For a box behind it, add a Shape layer under it.</div>';
      }
    }
    // An alert card: a theme's card, with what can be changed in it. Its
    // words, picture and colour are the alert's.
    if(w.kind==='AlertCard'){
      const kit=Integrations.alertKit, c=w.card||{}, own=kit.themes[c.theme]||kit.themes.island;
      const choose=(label,key,opts)=>field(label,'<select class="ic-input"'+on('setCard',key)+'>'+
        opts.map(([v,l])=>'<option value="'+escapeAttr(v)+'"'+(v===(c[key]||'')?' selected':'')+'>'+escapeHtml(l)+'</option>').join('')+'</select>');
      h += '</div><div class="st-pgroup"><div class="st-pgroup-h">Alert card</div>';
      h += choose('Theme','theme',Object.entries(kit.themes).map(([id,t])=>[id,t.name+' · '+t.motion]));
      h += choose('Font','font',[['','The theme\'s ('+own.font+')']].concat(kit.fonts.map(x=>[x,x])));
      h += choose('Comes in','animation',[['','The theme\'s way']].concat(kit.anims));
      h += choose('Its picture','picture',[['','Where the theme puts it'],['left','Beside the words'],['top','Above the words']]);
      h += rangeF('Size','setCardNum','scale',c.scale||100,50,200,5);
      h += colorF('Accent','setCardColor',['accent'],c.accent||own.accent,false);
      h += colorF('Card','setCardColor',['background'],c.background||own.bg,false);
      h += colorF('Words','setCardColor',['text'],c.text||own.fg,false);
      h += rangeF('Card opacity','setCardNum','opacity',c.opacity!=null?c.opacity:own.op,0,100,5);
      h += rangeF('Roundness','setCardNum','radius',c.radius!=null?c.radius:own.r,0,48,1);
      h += '<div class="st-rowbtns"><button class="ic-btn ic-btn-ghost st-mini"'+on('resetCard')+'>Back to '+escapeHtml(own.name)+' as it comes</button></div>';
      h += '<div class="st-hint">Its words, picture and colour are the alert\'s. Width and Height are its box: the card sits at the bottom of it, as wide as its words.</div>';
    }
    if(w.kind==='BitrateMeter'){
      const sm=w.smooth!=null?w.smooth:0.6;
      h += rangeF('Smoothing','setNum','smooth',sm,0,0.95,0.05);
      h += '<div class="st-hint" style="margin-top:-4px">Higher = calmer line. Lower shows every jitter.</div>';
    }
    if(w.kind==='LiquidFill'){
      const fixed=(w.level!=null&&w.level!=='buffer');
      h += field('Fill level','<select class="ic-input"'+on('setLiquidLevel')+'><option value="buffer"'+(!fixed?' selected':'')+'>Follow buffer</option><option value="fixed"'+(fixed?' selected':'')+'>Fixed level</option></select>');
      if(fixed){ const pv=typeof w.level==='number'?w.level:60; h += rangeF('Level','setNum','level',pv,0,100,5); }
      const wav=w.wave!=null?w.wave:1;
      h += rangeF('Wave','setNum','wave',wav,0,2,0.1);
      h += '<div class="st-hint" style="margin-top:-4px">0 = flat, 2 = choppy.</div>';
    }
    // Animation speed for widgets with their own ongoing motion.
    if(['LiquidFill','Heartbeat'].indexOf(w.kind)>=0){
      const spd=w.speed!=null?w.speed:1;
      h += rangeF('Speed','setNum','speed',spd,0.25,3,0.05);
      h += '<div class="st-hint" style="margin-top:-4px">Animation pace. 1 = default.</div>';
    }
    // Smoothness (transition easing) for data-driven widgets that move.
    if(['LiquidFill','BufferBar','BufferRing'].indexOf(w.kind)>=0){
      const smd=w.smooth!=null?w.smooth:(w.kind==='LiquidFill'?0.6:0.42);
      h += rangeF('Smoothness','setNum','smooth',smd,0,1.5,0.05);
      h += '<div class="st-hint" style="margin-top:-4px">How softly it eases to a new value (seconds).</div>';
    }
    if(w.kind==='BufferRing'){
      h += '<label class="st-check"><input type="checkbox"'+(!w.notext?' checked':'')+on('setFlag','notext')+' data-st-not> Show value in centre</label>';
      h += rangeF('Ring thickness','setNum','ringWidth',(w.ringWidth!=null?w.ringWidth:4),2,14,1);
    }
    if(w.kind==='EdgeAccent'){
      h += rangeF('Border thickness','setNum','edgeWidth',(w.edgeWidth!=null?w.edgeWidth:3),1,14,1);
      h += rangeF('Glow reach','setNum','glow',(w.glow!=null?w.glow:38),0,120,4);
    }
    if(w.kind==='BackgroundTint'){
      h += rangeF('Intensity','setNum','intensity',(w.intensity!=null?w.intensity:0.22),0,0.7,0.02);
    }
    if(w.kind==='CornerFrame'){
      h += rangeF('Corner size','setNum','frameSize',(w.frameSize!=null?w.frameSize:46),20,140,4);
      h += rangeF('Thickness','setNum','frameWidth',(w.frameWidth!=null?w.frameWidth:3),1,12,1);
    }
    if(f.indexOf('unit')>=0) h += field('Unit','<input class="ic-input" value="'+escapeAttr(w.unit||'')+'"'+on('set','unit')+'>');
    if(f.indexOf('icon')>=0){ const io=IC().ICONS.map(n=>'<option value="'+n+'"'+(w.icon===n?' selected':'')+'>'+n+'</option>').join(''); h+=field('Icon','<select class="ic-input"'+on('set','icon')+'>'+io+'</select>'); }
    if(f.indexOf('shape')>=0){ const so=IC().SHAPES.map(n=>'<option value="'+n+'"'+(w.shape===n?' selected':'')+'>'+n+'</option>').join(''); h+=field('Shape','<select class="ic-input"'+on('set','shape')+'>'+so+'</select>'); }
    if(f.indexOf('src')>=0){ const isData=(w.src||'').slice(0,5)==='data:';
      h += field('Image','<div class="st-field-row"><input class="ic-input" value="'+(isData?'(uploaded image)':escapeAttr(w.src||''))+'" placeholder="URL or upload" '+(isData?'disabled':'')+on('set','src')+'><button class="ic-btn ic-btn-ghost"'+on('uploadImage')+'>Upload</button></div>'); }
    // A Shape's or a picture's corners, ring and shadow (the runtime's
    // edgeCss). Corners start from the shape's own.
    if(w.kind==='Shape'||w.kind==='Image'){
      if(w.kind==='Image') h += '<label class="st-check"><input type="checkbox"'+(w.cover?' checked':'')+on('setFlag','cover')+'> Fill its box (its width and height, cropped)</label>';
      const ownR={rect:6,card:18,line:2,pill:160,circle:160}[w.shape]||0;
      h += rangeF('Corners','setNum','radius',w.radius!=null?w.radius:(w.kind==='Shape'?ownR:0),0,160,1);
      h += rangeF('Border','setNum','border',w.border||0,0,16,1);
      h += colorF('Border colour','set',['border_color'],w.border_color||'@ink',true);
      h += '<label class="st-check"><input type="checkbox"'+(w.shadow?' checked':'')+on('setFlag','shadow')+'> Drop shadow</label>';
    }
    if(f.indexOf('text')>=0){ h += field('Default text','<input class="ic-input" id="st-text" value="'+escapeAttr(w.base.text||'')+'"'+on('setBase','text')+'>');
      const chip=(v,l)=>'<button class="st-chip" title="{{'+escapeAttr(v)+'}}"'+on('insertVar','#st-text','{{'+v+'}}')+'>'+l+'</button>';
      // An alert design's words are the alert's: one click each.
      if(mode().sim==='alert') h += '<div class="st-say"><div class="st-say-h">What the alert says</div><div class="st-say-row"><span>The alert</span><div>'+
        [['alert.title','title'],['alert.text','words'],['alert.message','message']].map(([v,l])=>chip(v,l)).join('')+'</div></div></div>';
      else h += '<div class="st-hint">Tip: use live data like <code>{{delay}}</code>, <code>{{bitrate}}</code>, <code>{{bufferPct}}</code>, <code>{{phase}}</code>.</div>';
      // Each goal and timer on a line of its own, with what it can say.
      const lines=(tracked.goals||[]).map(g=>{ const key='goal.'+g.id; return [g.name,[[key+'.name','name'],[key,'count'],[key+'.target','target'],[key+'.left','left'],[key+'.percent','%']]]; })
        .concat((tracked.timers||[]).map(t=>{ const key='timer.'+t.id; return [t.name,[[key+'.name','name'],[key,'clock']]]; }));
      if(lines.length) h += '<div class="st-say"><div class="st-say-h">Goals and timers</div>'+lines.map(([name,parts])=>'<div class="st-say-row"><span>'+escapeHtml(name)+'</span><div>'+parts.map(([v,l])=>chip(v,l)).join('')+'</div></div>').join('')+'</div>'; }
    if(f.indexOf('custom')>=0){
      const c = w.custom||IC().defaultCustom();
      h += field('Custom HTML','<textarea class="ic-input"'+on('setCustom','html')+'>'+escapeHtml(c.html||'')+'</textarea>');
      h += field('Custom CSS','<textarea class="ic-input"'+on('setCustom','css')+'>'+escapeHtml(c.css||'')+'</textarea>');
      h += field('Custom JS','<textarea class="ic-input"'+on('setCustom','js')+'>'+escapeHtml(c.js||'')+'</textarea>');
      h += '<div class="st-cheat"><b>Insert live data</b> '+['delay','phase','bufferPct','bitrate','cutCount','destAlive'].map(v=>'<button class="st-chip"'+on('insertVar','#st-props textarea','{{'+v+'}}')+'>{{'+v+'}}</button>').join('')+
        '<br>In JS: <code>ic.onUpdate(s =&gt; ...)</code>, <code>ic.delay</code>, <code>ic.phase</code>, <code>ic.bind(sel,key)</code>.</div>';
    }
    h += '</div>';

    // ---- TYPOGRAPHY (global, text-bearing widgets) ----
    if(TYPE_KINDS.indexOf(w.kind)>=0){
      h += '<div class="st-pgroup"><div class="st-pgroup-h">Typography</div>';
      const wt=String(w.font_weight||'');
      const wo=['','400','500','600','700','800','900'].map(x=>'<option value="'+x+'"'+(wt===x?' selected':'')+'>'+(x||'default')+'</option>').join('');
      h += '<div class="st-field-row">'+field('Weight','<select class="ic-input"'+on('set','font_weight')+'>'+wo+'</select>')+
        field('Letter spacing','<input class="ic-input mono" type="number" step="0.5" value="'+escapeAttr(w.letter_spacing!=null?w.letter_spacing:'')+'" placeholder="default" data-st-empty'+on('setLS')+'>')+'</div>';
      h += field('Font','<select class="ic-input"'+on('set','font_family')+'>'+fontOptions(w.font_family)+'</select>');
      h += '<label class="st-check" style="margin-top:2px"><input type="checkbox"'+(w.uppercase?' checked':'')+on('set','uppercase')+'> Uppercase</label>';
      // Words that must stay inside a card: as wide as Width, and smaller
      // when they're longer (an alert's words, a viewer's name), or wrapped
      // onto more lines (a viewer's message).
      if(w.kind==='Text'){
        h += '<label class="st-check"><input type="checkbox"'+(w.shadow!==false?' checked':'')+on('set','shadow')+'> Shadow behind the words</label>';
        const long = w.fit ? 'fit' : w.wrap ? 'wrap' : '';
        h += field('Longer than its width','<select class="ic-input"'+on('setLong')+'>'+
          [['','Run past it'],['fit','Shrink to fit'],['wrap','Wrap, up to 3 lines']].map(([v,l])=>'<option value="'+v+'"'+(long===v?' selected':'')+'>'+l+'</option>').join('')+'</select>');
        if(long) h += field('Align in its width','<select class="ic-input"'+on('set','text_align')+'>'+
          [['left','Left'],['center','Centre'],['right','Right']].map(([v,l])=>'<option value="'+v+'"'+((w.text_align||'left')===v?' selected':'')+'>'+l+'</option>').join('')+'</select>');
      }
      h += '</div>';
    }

    // ---- VISIBILITY across all states (set once, no per-state hopping) ----
    // (An alert design's layers all wait hidden and show while an alert
    // plays: the delay's states are nothing to them.)
    if(mode().sim!=='alert'){
      h += '<div class="st-pgroup"><div class="st-pgroup-h">Show in</div>';
      const chips = IC().STATES.map(st=>{
        const visS = !(w.states && w.states[st.key] && w.states[st.key].visible===false);
        return '<button class="st-vischip'+(visS?' on':'')+'" style="--dot:'+STATE_DOT[st.key]+'"'+on('toggleVis',st.key)+'>'+st.label+'</button>';
      }).join('');
      h += '<div class="st-vischips">'+chips+'</div>';
      h += '<div class="st-rowbtns"><button class="ic-btn ic-btn-ghost st-mini"'+on('visOnly')+'>Only '+escapeHtml(stLabel)+'</button><button class="ic-btn ic-btn-ghost st-mini"'+on('visAll')+'>All states</button></div>';
      h += '</div>';
    }

    // ---- LOOK in the currently-selected state ----
    const one = !!w.always;
    const sp = one ? w.base : ((w.states && w.states[editState]) || {});
    const vis = one || sp.visible !== false;
    h += '<div class="st-pgroup"><div class="st-pgroup-h"><span>'+
      (one ? 'Look &middot; every state' : '<span class="dot" style="background:'+STATE_DOT[editState]+'"></span>Look &middot; '+escapeHtml(stLabel))+'</span></div>';
    if(mode().sim==='states'){
      h += '<label class="st-check"><input type="checkbox"'+(one?' checked':'')+on('setAlways')+'> Same look in every delay state</label>';
      h += '<div class="st-hint">'+(one ? 'What you set here shows whatever the delay is doing. Untick it to style each delay state on its own.'
        : 'What you set here is for <b>'+escapeHtml(stLabel)+'</b> only: pick another state at the top of the canvas to style it.')+'</div>';
    }
    if(vis && !one) h += '<div class="st-rowbtns"><button class="ic-btn ic-btn-ghost st-mini"'+on('copyLookAll')+'>Copy this look to every state</button></div>';
    if(!vis){
      h += '<div class="st-empty-hint" style="padding:8px 2px">Hidden in '+escapeHtml(stLabel)+'. Toggle it in <b>Show in</b> above to style it here.</div>';
    } else {
      const col = sp.color || w.base.color || '#5ac8fa';
      h += colorF('Colour '+(one?'':'<button class="st-allbtn"'+on('applyColorAll')+'>apply to all states</button>'),'setState',['color'],col,true);
      // Gradient only applies to text glyphs and solid-fill widgets.
      if(['DelayReadout','StatePill','Text','CutCounter','SessionElapsed','Banner','Shape','Divider','Goal','Timer'].indexOf(w.kind)>=0){
        const hasG=sp.grad!=null;
        h += '<label class="st-check" style="margin-top:2px"><input type="checkbox"'+(hasG?' checked':'')+on('toggleGrad')+'> Gradient fill</label>';
        if(hasG){
          h += colorF('Gradient colour','setGrad',[],sp.grad,false);
          const mo = sp.grad_anim===true ? 'flow' : (sp.grad_anim||'');
          h += field('Motion','<select class="ic-input"'+on('setGradAnim')+'>'+[['','None'],['flow','Flow: slides across, on a loop'],['wave','Wave: sways back and forth'],['shine','Shine: a light sweeps across']]
            .map(o=>'<option value="'+o[0]+'"'+(mo===o[0]?' selected':'')+'>'+o[1]+'</option>').join('')+'</select>');
          if(mo) h += rangeF('Speed (seconds a pass)','setStateNum','grad_speed',(sp.grad_speed!=null?sp.grad_speed:3.5),0.5,10,0.5);
          h += rangeF('Angle','setStateNum','grad_angle',(sp.grad_angle!=null?sp.grad_angle:135),0,360,15);
          if(mo==='flow'||mo==='wave') h += '<div class="st-hint">With a glow, the glow follows the colours as they move.</div>';
        }
      }
      const op = sp.opacity!=null?sp.opacity:(w.base.opacity!=null?w.base.opacity:1);
      h += rangeF('Opacity','setStateNum','opacity',op,0,1,0.05);
      const gl = sp.glow!=null?sp.glow:(w.base.glow||0);
      h += rangeF('Glow','setStateNum','glow',gl,0,40,1);
      if(f.indexOf('text')>=0 && !one) h += field('Text override','<input class="ic-input" value="'+escapeAttr(sp.text!=null?sp.text:'')+'" placeholder="(uses default text)"'+on('setStateText')+'>');
      // Multiple animations can run together (a pipeline via per-anim delay).
      h += '<div class="st-field"><label style="display:flex;justify-content:space-between;align-items:center">Animations '+(one?'':'<button class="st-allbtn"'+on('applyAnimAll')+'>apply to all states</button>')+'</label></div>';
      animsDisplay(sp,w).forEach((a,i)=>{
        const ao=IC().animsFor(w.kind).map(x=>'<option value="'+x.key+'"'+(a.style===x.key?' selected':'')+'>'+x.label+(x.loop?' (loop)':'')+'</option>').join('');
        const eo=IC().EASINGS.map(x=>'<option value="'+x.key+'"'+((a.easing||'ease-out')===x.key?' selected':'')+'>'+x.key+'</option>').join('');
        const amp=(a.amount!=null?+a.amount:1);
        // Intensity scales the motion (slide distance, pulse depth, etc). Fade
        // has no magnitude to scale, so the slider is hidden for it.
        const ampRow=(a.style==='fade'||a.style==='instant')?'':
          '<div class="st-amprow"><span>Intensity</span>'+
          '<input type="range" class="st-amp" min="0.2" max="2.5" step="0.1" value="'+escapeAttr(amp)+'"'+on('animSet',i,'amount')+'>'+
          '<output data-unit="x"></output></div>';
        h += '<div class="st-animrow"><div class="st-animrow-top"><select class="ic-input"'+on('animSet',i,'style')+'>'+ao+'</select>'+
          '<button class="st-layer-btn" title="Remove" aria-label="Remove this animation"'+on('animDel',i)+'>'+stIcon(UI_ICON.x)+'</button></div>'+
          '<div class="st-animrow-bot"><label><span>Length (ms)</span><input class="ic-input mono" type="number" value="'+escapeAttr(a.duration_ms||400)+'"'+on('animSet',i,'duration_ms')+'></label>'+
          '<label><span>Delay (ms)</span><input class="ic-input mono" type="number" value="'+escapeAttr(a.delay_ms||0)+'"'+on('animSet',i,'delay_ms')+'></label>'+
          '<label><span>Easing</span><select class="ic-input"'+on('animSet',i,'easing')+'>'+eo+'</select></label></div>'+ampRow+'</div>';
      });
      h += '<button class="ic-btn ic-btn-ghost" style="width:100%;margin-top:4px"'+on('animAdd')+'>+ Add animation</button>';
      const ah = sp.hide_after_ms ? (sp.hide_after_ms/1000) : 0;
      if(!one){
      h += '<label class="st-check" style="margin-top:4px"><input type="checkbox"'+(ah>0?' checked':'')+on('toggleHide')+'> Auto-hide in '+escapeHtml(stLabel)+'</label>';
      if(ah>0){
        h += field('Hide after seconds','<input class="ic-input mono" type="number" min="0.5" step="0.5" value="'+escapeAttr(ah)+'"'+on('setHideAfter')+'>');
        h += '<div class="st-hint">Shows for '+ah+'s, then leaves so viewers see the gameplay.</div>';
        const ex=sp.exit||'fade'; const exo=[['fade','Fade'],['slide-down','Slide down'],['slide-up','Slide up'],['pop','Pop'],['instant','Instant']].map(o=>'<option value="'+o[0]+'"'+(ex===o[0]?' selected':'')+'>'+o[1]+'</option>').join('');
        h += field('Exit style','<select class="ic-input"'+on('setState','exit')+'>'+exo+'</select>');
      }
      // Per-state position: move the widget in this phase (it glides there).
      const eff = sp.offset || w.offset;
      h += '<div class="st-field"><label style="display:flex;justify-content:space-between;align-items:center">Move in '+escapeHtml(stLabel)+
        (sp.offset?' <button class="st-allbtn"'+on('resetStateOffset')+'>reset to base</button>':'')+'</label>'+
        '<div class="st-field-row"><input class="ic-input mono" type="number" value="'+escapeAttr(eff.x)+'" placeholder="X"'+on('setStateOffset','x')+'>'+
        '<input class="ic-input mono" type="number" value="'+escapeAttr(eff.y)+'" placeholder="Y"'+on('setStateOffset','y')+'></div></div>';
      }
    }
    h += '</div>';

    h += '<div class="st-rowbtns"><button class="ic-btn ic-btn-ghost"'+on('duplicatePicked')+'>Duplicate</button><button class="ic-btn ic-btn-ghost st-danger"'+on('deletePicked')+'>Delete widget</button></div>';
    h += '</div>';
    p.innerHTML = h;
    const body = p.querySelector('.st-props-body');
    p.querySelectorAll('input[type=range]').forEach(fillStRange);
    wireGroups(p);
    if(same && body){ body.classList.add('st-quiet'); body.scrollTop = scrolled; }
  }
  // Several layers picked: lining them up, and what they share, set on all
  // of them at once.
  const LINE_UP = [['left','Left edges','M4 3v18M8 7h12M8 15h7'], ['center','Centres','M12 3v18M6 7h12M8 15h8'], ['right','Right edges','M20 3v18M4 7h12M9 15h7'],
    ['top','Tops','M3 4h18M7 8v12M15 8v7'], ['middle','Middles','M3 12h18M7 6v12M15 8v8'], ['bottom','Bottoms','M3 20h18M7 4v12M15 9v7']];
  const SPREAD = [['x','Even gaps across','M3 4v16M21 4v16M9 8h6v8H9z'], ['y','Even gaps down','M4 3h16M4 21h16M8 9h8v6H8z']];
  function renderManyProps(p){
    const ws = picked.map(widget).filter(Boolean), w = widget(sel);
    p.dataset.for = '';
    const btn = ([how, label, path]) => '<button class="st-lineb" title="'+label+'" aria-label="Line up: '+label.toLowerCase()+'"'+on('lineUp',how)+'>'+stIcon(path)+'</button>';
    const sp = w.always ? w.base : ((w.states && w.states[editState]) || {});
    let h = '<div class="st-props-head"><span class="st-props-ic" style="--sw:'+escapeAttr(layerColor(w))+'">'+stIcon(UI_ICON.layers)+'</span><div><div class="st-props-title">'+ws.length+' layers</div>'+
      '<div class="st-props-sub st-props-note">What you set here goes to all of them</div></div></div><div class="st-props-body">';
    h += '<div class="st-pgroup"><div class="st-pgroup-h">Line up</div><div class="st-lineup">'+LINE_UP.map(btn).join('')+'</div>';
    if(ws.length>=3) h += '<div class="st-lineup st-spread">'+SPREAD.map(btn).join('')+'</div>';
    h += '<div class="st-hint">Drag any of them to move them all, or nudge them with the arrow keys (Shift: 10 px).</div></div>';
    h += '<div class="st-pgroup"><div class="st-pgroup-h">Look</div>';
    h += colorF('Colour','setManyLook',['color'],sp.color||w.base.color||'#5ac8fa',true);
    h += rangeF('Opacity','setManyLook','opacity',sp.opacity!=null?sp.opacity:(w.base.opacity!=null?w.base.opacity:1),0,1,0.05);
    if(ws.some(x=>TYPE_KINDS.includes(x.kind))) h += field('Font','<select class="ic-input"'+on('setFont')+'>'+fontOptions(w.font_family)+'</select>');
    h += '</div>';
    h += '<div class="st-rowbtns"><button class="ic-btn ic-btn-ghost"'+on('duplicatePicked')+'>Duplicate</button><button class="ic-btn ic-btn-ghost st-danger"'+on('deletePicked')+'>Delete '+ws.length+' layers</button></div>';
    h += '</div>';
    p.innerHTML = h;
    p.querySelectorAll('input[type=range]').forEach(fillStRange);
    wireGroups(p);
  }
  // ---- property setters ----
  // Live-update the canvas WITHOUT reloading the iframe (a reload restarts
  // every animation). Swap the <style> and reconcile per-widget attributes
  // in place; fall back to a full rebake only when the structure changes.
  let rebakeT=0;
  function liveStyle(){
    const f=$('st-canvas'); const cdoc=f&&f.contentDocument;
    if(!cdoc||!cdoc.body){ rebake(); return; }
    const html=IC().bake(doc);
    const css=html.split('<style>')[1].split('</style>')[0];
    const styleEl=cdoc.querySelector('style'); if(styleEl && styleEl.textContent!==css) styleEl.textContent=css;
    const tmp=document.createElement('div');
    tmp.innerHTML=html.split(/<body[^>]*>/)[1].split('<scr'+'ipt>')[0];
    const news=tmp.querySelectorAll('.icw');
    if(news.length!==cdoc.querySelectorAll('.icw').length){ rebake(); return; }
    let ok=true;
    news.forEach(nw=>{
      const live=cdoc.getElementById(nw.id); if(!live){ ok=false; return; }
      Array.from(nw.attributes).forEach(a=>{ if(live.getAttribute(a.name)!==a.value) live.setAttribute(a.name,a.value); });
      Array.from(live.attributes).forEach(a=>{ if(!nw.hasAttribute(a.name)) live.removeAttribute(a.name); });
    });
    if(!ok){ rebake(); return; }
    buildHits(); recordSoon(); updatePerfBadge();
  }
  function softRebake(){ clearTimeout(rebakeT); rebakeT=setTimeout(function(){ liveStyle(); if(studioSim&&studioSim.rehide) studioSim.rehide(); flashSynced(); }, 80); }
  // Brief "synced" pulse on the preview after a live edit re-arms the timers,
  // so it's clear the preview reflects the change (not the previous cycle).
  let syncedT=0;
  function flashSynced(){ const b=$('st-synced'); if(!b) return; b.classList.add('on'); clearTimeout(syncedT); syncedT=setTimeout(function(){ b.classList.remove('on'); }, 900); }
  // Where the Look controls write: the widget's own look when it is the same
  // in every state, else the edited state's.
  function lookOf(w){ if(w.always) return w.base; if(!w.states) w.states={}; if(!w.states[editState]) w.states[editState]={}; return w.states[editState]; }
  function ensureState(){ return lookOf(widget(sel)); }
  // The picked layers' look (several picked), and their font.
  function setManyLook(k, v){
    picked.forEach(id=>{ const w=widget(id); if(w) lookOf(w)[k] = k==='opacity' ? (parseFloat(v)||0) : v; });
    softRebake(); if(k==='color') refreshLayerColors();
  }
  function setFont(v){ picked.forEach(id=>{ const w=widget(id); if(w && TYPE_KINDS.includes(w.kind)) w.font_family=v; }); rebake(); }
  // A new anchor, the layer staying where it is: its offset (and each
  // state's own) worked out again from where it sits.
  function setAnchor(v){
    const w=widget(sel); if(!w || v===w.anchor) return;
    const b=boxes.get(w.id), size=b ? {width:b.width, height:b.height} : {width:(w.size||{}).w||0, height:(w.size||{}).h||0};
    const again=off=>G.offsetFor(v, G.boxAt(w.anchor, off, size));
    w.offset=again(w.offset);
    Object.values(w.states||{}).forEach(st=>{ if(st && st.offset) st.offset=again(st.offset); });
    w.anchor=v;
    placeNow(); renderProps();
  }
  // One look for every state: the edited state's becomes the widget's own,
  // and what the states held for themselves goes (only showing stays).
  function setAlways(on){ const w=widget(sel); if(!w) return;
    if(on) IC().oneLook(w, editState); else w.always=false;
    rebake(); renderProps(); }
  function set(k,v){ const w=widget(sel); w[k]=v; if(k==='unit'||k==='icon'||k==='shape'||k==='src'||k==='font_family'){ rebake(); } else softRebake(); }
  function setNum(path,v){ const w=widget(sel); const n=parseFloat(v)||0; if(path.indexOf('.')>=0){const[a,b]=path.split('.'); w[a][b]=n;} else w[path]=n; softRebake(); }
  // LiquidFill: switch between buffer-following and a fixed level (number).
  function setLiquidLevel(v){ const w=widget(sel); w.level = (v==='buffer') ? 'buffer' : (typeof w.level==='number'?w.level:60); softRebake(); renderProps(); }
  // Boolean widget flag that changes structure (e.g. BufferRing centre text) -
  // needs a full rebake since liveStyle only diffs attributes.
  function setFlag(k,on){ const w=widget(sel); if(on) w[k]=true; else delete w[k]; rebake(); }
  // What a Text layer does with words longer than its width: run past it,
  // shrink to fit, or wrap.
  function setLong(v){
    const w=widget(sel);
    delete w.fit; delete w.wrap;
    if(v==='fit' || v==='wrap') w[v]=true;
    rebake(); renderProps();
  }
  function setBase(k,v){ widget(sel).base[k]=v; if(k==='text'&&left==='layers') renderLeft(); softRebake(); }
  // The overlay's name as it is typed: an edit like any, so undo takes it
  // back and closing asks about it. (Save tidies it.)
  function setDocName(v){ if(!doc) return; doc.name=v; recordSoon(); }
  // A layer's own name, for the layer list (the page never shows it).
  function setName(v){ const w=widget(sel); v=v.trim(); if(v) w.name=v; else delete w.name; if(left==='layers') renderLeft(); recordSoon(); }
  // CustomHTML lives inside a sandboxed iframe (the widget's inner content),
  // which the in-place live update can't touch - so debounce a full rebake.
  let fullT=0; function softFull(){ clearTimeout(fullT); fullT=setTimeout(rebake,300); }
  function setCustom(k,v){ const w=widget(sel); if(!w.custom) w.custom=IC().defaultCustom(); w.custom[k]=v; softFull(); }
  // Re-render only when visibility flips (to show/hide the look fields).
  // Avoid re-rendering on colour input - it would close the native picker.
  function setState(k,v){ const sp=ensureState(); sp[k]=v; softRebake(); if(k==='visible') renderProps(); if(k==='color') refreshLayerColors(); }
  function setStateNum(k,v){ ensureState()[k]=parseFloat(v)||0; softRebake(); }
  function setStateText(v){ const sp=ensureState(); if(v==='') delete sp.text; else sp.text=v; softRebake(); }

  function setHideAfter(v){ const sp=ensureState(); const had=sp.hide_after_ms>0; const ms=Math.round((parseFloat(v)||0)*1000); if(ms<=0) delete sp.hide_after_ms; else sp.hide_after_ms=ms; softRebake(); studioDrive(); if((ms>0)!==had) renderProps(); }
  // Auto-hide on/off toggle: on defaults to 4s + fade exit; off clears both.
  function toggleHide(on){ const sp=ensureState(); if(on){ sp.hide_after_ms=sp.hide_after_ms||4000; if(!sp.exit) sp.exit='fade'; } else { delete sp.hide_after_ms; delete sp.exit; } softRebake(); studioDrive(); renderProps(); }
  function setLS(v){ const w=widget(sel); if(v==='') delete w.letter_spacing; else w.letter_spacing=parseFloat(v)||0; softRebake(); }
  // Theme colours (overlay-wide). Editing one recolours every widget that
  // uses the matching @token.
  function ensureTheme(){ if(!doc.theme) doc.theme=IC().defaultTheme(); return doc.theme; }
  // (Its picker and its text follow each other: see `dispatch`.)
  function setTheme(key,val){ ensureTheme()[key]=val; softRebake(); refreshLayerColors(); }
  function themeHex(c){ if(typeof c==='string' && c.charAt(0)==='@'){ const t=ensureTheme(); return t[c.slice(1)]||'#5ac8fa'; } return c||'#5ac8fa'; }
  // Per-state gradient (a second colour).
  function setGrad(v){ const sp=ensureState(); if(!v) delete sp.grad; else sp.grad=v; softRebake(); }
  function toggleGrad(on){ const sp=ensureState(); if(on){ sp.grad=sp.color||widget(sel).base.color||'#54e38e'; } else { delete sp.grad; delete sp.grad_anim; } softRebake(); renderProps(); }
  function setGradAnim(motion){ const sp=ensureState(); if(motion) sp.grad_anim=motion; else delete sp.grad_anim; softRebake(); renderProps(); }
  // Alignment helpers - set anchor + offset to a known spot.
  function align(where){ const w=widget(sel); if(!w) return; const S=64;
    const map={ 'center':['center',0,0], 'top':['top',0,S], 'bottom':['bottom',0,S],
      'left':['left',S,0], 'right':['right',S,0],
      'tl':['top-left',96,62], 'tr':['top-right',96,62], 'bl':['bottom-left',96,62], 'br':['bottom-right',96,62] };
    const m=map[where]; if(!m) return; w.anchor=m[0]; w.offset={x:m[1],y:m[2]}; rebake(); renderProps(); }
  // Multi-animation list (per state). Migrates a single `anim` to `anims`.
  function animsDisplay(sp,w){ if(sp.anims) return sp.anims; if(sp.anim) return [sp.anim]; return [w.base.anim||{style:'fade',duration_ms:400,easing:'ease-out',delay_ms:0}]; }
  function stAnims(){ const sp=ensureState(); if(!sp.anims){ sp.anims = sp.anim ? [clone(sp.anim)] : [clone(widget(sel).base.anim||{style:'fade',duration_ms:400,easing:'ease-out',delay_ms:0})]; delete sp.anim; } return sp.anims; }
  function animAdd(){ stAnims().push({style:'pulse',duration_ms:1600,easing:'ease-out',delay_ms:0}); softRebake(); renderProps(); }
  function animDel(i){ const a=stAnims(); a.splice(i,1); if(!a.length) a.push({style:'instant',duration_ms:400,easing:'ease-out',delay_ms:0}); softRebake(); renderProps(); }
  function animSet(i,k,v){ const a=stAnims(); if(!a[i]) return;
    if(k==='duration_ms'||k==='delay_ms') a[i][k]=parseInt(v,10)||0;
    else if(k==='amount') a[i][k]=parseFloat(v)||1;
    else a[i][k]=v;
    softRebake(); if(k==='style') renderProps(); }
  // Image upload -> embed as a self-contained data URL (no server assets).
  function uploadImage(){ const inp=document.createElement('input'); inp.type='file'; inp.accept='image/*';
    inp.onchange=()=>{ const f=inp.files&&inp.files[0]; if(!f) return;
      if(f.size>6*1024*1024){ toast('That image is '+Math.round(f.size/1024/1024)+'MB - consider a smaller one so the overlay loads fast in OBS','info'); }
      const rd=new FileReader(); rd.onload=()=>{ const w=widget(sel); w.src=rd.result; rebake(); renderProps(); toast('Image added','ok'); }; rd.readAsDataURL(f); };
    inp.click(); }
  // Insert a {{var}} into a custom/text field at the cursor.
  function insertVar(fieldSel, v){ const el=document.querySelector(fieldSel); if(!el) return; const s=el.selectionStart||el.value.length;
    el.value=el.value.slice(0,s)+v+el.value.slice(el.selectionEnd||s); el.dispatchEvent(new Event('input')); el.focus(); }
  // Reassuring perf estimate (the overlay runs in an OBS browser source).
  function updatePerfBadge(){ const el=$('st-perf'); if(!el||!doc) return;
    const full=['EdgeAccent','BackgroundTint','LiquidFill','CornerFrame'];
    let fs=0, anim=0; const n=doc.widgets.length;
    doc.widgets.forEach(w=>{ if(full.indexOf(w.kind)>=0) fs++;
      const ha=(w.base&&w.base.anim&&w.base.anim.style&&w.base.anim.style!=='instant')||Object.keys(w.states||{}).some(k=>{const s=w.states[k];return s.anim&&s.anim.style&&s.anim.style!=='instant';}); if(ha) anim++; });
    let level='Lightweight', cls='ok';
    if(fs>=2 || n>10 || anim>8){ level='Moderate'; cls='warn'; }
    if(fs>=3 || n>16){ level='Heavy'; cls='err'; }
    el.textContent=level; el.className='st-perf '+cls; el.title=n+' widgets · '+fs+' full-screen · '+anim+' animated, in an OBS browser source';
  }
  function setStateOffset(axis,v){ const w=widget(sel); const sp=ensureState(); if(!sp.offset) sp.offset={x:w.offset.x,y:w.offset.y}; sp.offset[axis]=parseInt(v,10)||0; softRebake(); }
  function resetStateOffset(){ const sp=ensureState(); delete sp.offset; softRebake(); renderProps(); }
  // Visibility across states - so you don't have to visit each state.
  function stEnsure(st){ const w=widget(sel); if(!w.states) w.states={}; if(!w.states[st]) w.states[st]={}; return w.states[st]; }
  function toggleVis(st){ const o=stEnsure(st); o.visible = o.visible===false; if(o.visible===true) delete o.visible; softRebake(); renderProps(); }
  function visOnly(){ IC().STATES.forEach(s=>{ const o=stEnsure(s.key); if(s.key===editState) delete o.visible; else o.visible=false; }); softRebake(); renderProps(); }
  function visAll(){ const w=widget(sel); IC().STATES.forEach(s=>{ if(w.states&&w.states[s.key]) delete w.states[s.key].visible; }); softRebake(); renderProps(); }
  // Apply-to-all: write the current state's value to base + clear overrides.
  function applyColorAll(){ const w=widget(sel); const c=(w.states&&w.states[editState]&&w.states[editState].color)||w.base.color; w.base.color=c; IC().STATES.forEach(s=>{ if(w.states&&w.states[s.key]) delete w.states[s.key].color; }); softRebake(); renderProps(); renderLeft(); toast('Colour applied to all states','ok'); }
  function applyAnimAll(){ const w=widget(sel); const a=(w.states&&w.states[editState]&&w.states[editState].anim)||w.base.anim; w.base.anim=clone(a); IC().STATES.forEach(s=>{ if(w.states&&w.states[s.key]) delete w.states[s.key].anim; }); softRebake(); renderProps(); toast('Animation applied to all states','ok'); }
  // Copy this state's whole look (colour/opacity/text/anim/hide/move) to all others.
  function copyLookAll(){ const w=widget(sel); const src=(w.states&&w.states[editState])||{}; const look={};
    ['color','opacity','text','anim','hide_after_ms','offset'].forEach(k=>{ if(src[k]!==undefined) look[k]=clone(src[k]); });
    IC().STATES.forEach(s=>{ if(s.key!==editState){ if(!w.states[s.key]) w.states[s.key]={}; Object.assign(w.states[s.key], clone(look)); } });
    softRebake(); renderProps(); renderLeft(); toast('Look copied to all states','ok'); }
  // Copies of `list` (layers, as a doc holds them) on top, each nudged off
  // any layer it would sit exactly on, and picked. Anything that isn't a
  // layer the Studio knows is left out.
  // `from`: the page of the overlay they were copied on, when another.
  function addCopies(list, from){
    const known = (Array.isArray(list) ? list : []).filter(w=>w && w.base && w.offset && IC().KINDS.some(k=>k.key===w.kind));
    const ids = [];
    IC().upgradeDoc({widgets:clone(known)}).widgets.forEach(w=>{
      w.id = nextId();
      if(from && from!==IC().docPurpose(doc)) suitLayer(w, from);
      while(doc.widgets.some(o=>o.anchor===w.anchor && o.offset.x===w.offset.x && o.offset.y===w.offset.y)) w.offset={x:w.offset.x+24, y:w.offset.y+24};
      doc.widgets.push(w); ids.push(w.id);
    });
    if(!ids.length) return;
    picked = ids; sel = ids[ids.length-1]; arrived = sel;
    renderLeft(); rebake(); renderProps();
  }
  function duplicatePicked(){ addCopies(picked.map(widget)); }
  function deletePicked(){
    if(!picked.length) return;
    doc.widgets = doc.widgets.filter(w=>!picked.includes(w.id));
    clearPicked(); renderLeft(); rebake(); renderProps();
  }
  // Lock or unlock layers. A locked one stays where it is and the canvas
  // picks the layers under it (a card's backdrop, a wash), but Layers still
  // picks it, to style. Kept in the design; the stream never sees it.
  function setLocked(ids, lock){
    ids.map(widget).filter(Boolean).forEach(w=>{ if(lock) w.locked=true; else delete w.locked; });
    renderLeft(); buildHits(); renderProps(); recordSoon();
  }
  function delWidgetById(id){ doc.widgets=doc.widgets.filter(w=>w.id!==id); forgetGone(); renderLeft(); rebake(); renderProps(); }
  // Restacking (StudioGeometry's, on the layers' ids back to front: the
  // later in the doc, the further in front and the higher in Layers).
  const stackIds = () => doc.widgets.map(w=>w.id);
  const stepLayers = (ids, dir) => restacked(G.stepped(stackIds(), ids, dir));
  const stackEnd = (ids, front) => restacked(G.toEnd(stackIds(), ids, front));
  const stackBy = (ids, target, ahead) => restacked(G.beside(stackIds(), ids, target, ahead));
  function restacked(order){
    if(order.every((id, i)=>id===doc.widgets[i].id)) return;
    doc.widgets = order.map(widget); renderLeft(); rebake();
  }
  // The layers a row's arrows, a drag in Layers or a key restack: the
  // picked ones when `id` is among them, else that one.
  const stackOf = id => picked.includes(id) ? picked.slice() : [id];
  // A row in Layers: dragged up or down to restack (the picked layers
  // with it), outlining its layer on the canvas while the pointer is on
  // it, and the layer's menu on a right-click.
  let draggingRows = null;
  function wireLayerRow(row, id){
    row.draggable = true;
    row.addEventListener('dragstart', e=>{
      draggingRows = stackOf(id); row.classList.add('st-dragging');
      e.dataTransfer.effectAllowed = 'move'; e.dataTransfer.setData('text/plain', id);
    });
    row.addEventListener('dragend', ()=>{ draggingRows = null; row.classList.remove('st-dragging'); clearDrop(); });
    row.addEventListener('dragover', e=>{
      if(!draggingRows) return;
      e.preventDefault();
      const r = row.getBoundingClientRect(), ahead = e.clientY < r.top + r.height/2;
      if(row.classList.contains(ahead ? 'st-drop-above' : 'st-drop-below')) return;
      clearDrop(); row.classList.add(ahead ? 'st-drop-above' : 'st-drop-below');
    });
    row.addEventListener('drop', e=>{
      if(!draggingRows) return;
      e.preventDefault();
      const ids = draggingRows, ahead = row.classList.contains('st-drop-above');
      draggingRows = null; clearDrop(); stackBy(ids, id, ahead);
    });
    row.addEventListener('pointerenter', ()=>{ const b=hitBoxOf(id); if(b) b.classList.add('hover'); });
    row.addEventListener('pointerleave', ()=>{ const b=hitBoxOf(id); if(b) b.classList.remove('hover'); });
    row.addEventListener('contextmenu', e=>{ e.preventDefault(); if(!picked.includes(id)) pick([id]); openContext(e.clientX, e.clientY); });
  }
  function clearDrop(){ $$('#st-left-body .st-drop-above, #st-left-body .st-drop-below').forEach(r=>r.classList.remove('st-drop-above', 'st-drop-below')); }
  // The pointer on a layer on the canvas: its row in Layers lights up.
  function markRow(id){
    $$('#st-left-body .st-layer.hot').forEach(r=>r.classList.remove('hot'));
    const row = id && $('st-left-body').querySelector('.st-layer[data-id="'+CSS.escape(id)+'"]');
    if(row) row.classList.add('hot');
  }

  // ---- the right-click menu: what can be done to the picked layers ----
  const hasClip = () => { if(clipboard) return true; try{ return !!localStorage.getItem(CLIP_KEY); }catch(_){ return false; } };
  const CONTEXT = {
    copy: () => copyPicked(), paste: () => pasteLayers(), duplicate: () => duplicatePicked(),
    forward: () => stepLayers(picked, 1), back: () => stepLayers(picked, -1),
    front: () => stackEnd(picked, true), bottom: () => stackEnd(picked, false),
    lock: () => setLocked(picked, !picked.every(id=>widget(id).locked)), delete: () => deletePicked(),
  };
  function openContext(x, y){
    const m = $('st-ctx'), ws = picked.map(widget).filter(Boolean), none = !ws.length;
    const item = (act, label, keys, off, danger) => '<button data-ctx="'+act+'"'+(danger ? ' class="st-danger"' : '')+(off ? ' disabled' : '')+
      '><span>'+label+'</span><kbd>'+keys+'</kbd></button>';
    m.innerHTML = item('copy', 'Copy', 'Ctrl+C', none)+item('paste', 'Paste', 'Ctrl+V', !hasClip())+item('duplicate', 'Duplicate', 'Ctrl+D', none)+'<hr>'+
      item('forward', 'Bring forward', 'Ctrl+↑', none)+item('back', 'Send back', 'Ctrl+↓', none)+
      item('front', 'Bring to front', 'Ctrl+Shift+↑', none)+item('bottom', 'Send to back', 'Ctrl+Shift+↓', none)+'<hr>'+
      item('lock', !none && ws.every(w=>w.locked) ? 'Unlock' : 'Lock', 'Ctrl+Shift+L', none)+
      item('delete', ws.length>1 ? 'Delete '+ws.length+' layers' : 'Delete', 'Del', none, true);
    m.style.left = x+'px'; m.style.top = y+'px'; m.hidden = false;
    // Kept on screen, whichever corner it opens near.
    const r = m.getBoundingClientRect();
    m.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8))+'px';
    m.style.top = Math.max(8, Math.min(y, innerHeight - r.height - 8))+'px';
  }
  function closeContext(){ const m = $('st-ctx'); if(!m || m.hidden) return false; m.hidden = true; return true; }
  function onCanvasMenu(e){
    e.preventDefault();
    if(gesture || !doc) return;
    // On a handle: the picked layer's. On a layer: that one (with the
    // others, when picked among them). On the empty canvas: none.
    if(!e.target.closest('.st-handles')){
      const box = e.target.closest('.st-box'), id = box && box.dataset.id;
      if(!id) pick([]); else if(!picked.includes(id)) pick([id]);
    }
    openContext(e.clientX, e.clientY);
  }
  // The shortcuts card (?, or the menu's Keyboard shortcuts).
  function toggleKeys(){ const k = $('st-keys'); k.hidden = !k.hidden; if(!k.hidden) k.querySelector('button').focus(); }
  // Copy and paste, in a design and between them: what was copied is kept
  // in this browser too, so a design opened later can take it.
  const CLIP_KEY = 'ic-st-clip';
  let clipboard = null;
  // {from: the page copied on, layers}.
  function copyPicked(){
    const list = picked.map(widget).filter(Boolean); if(!list.length) return;
    clipboard = {from:IC().docPurpose(doc), layers:clone(list)};
    try{ localStorage.setItem(CLIP_KEY, JSON.stringify(clipboard)); }catch(_){ /* too big to keep: this visit still has it */ }
    toast(list.length===1 ? 'Layer copied' : list.length+' layers copied', 'ok');
  }
  function pasteLayers(){
    let clip = clipboard;
    if(!clip){ try{ clip = JSON.parse(localStorage.getItem(CLIP_KEY)||'null'); }catch(_){} }
    if(clip) addCopies(clip.layers, clip.from);
  }
  // ---- play: run the delay-lifecycle simulation without changing what you edit ----
  function playLabel(){ const pb=$('st-play'); if(pb) pb.classList.toggle('on',playing); }
  function stopPlay(){ playing=false; playLabel(); if(studioSim){ studioSim.stop(); studioSim=null; } }
  function play(){ playing=!playing; playLabel(); if(!playing) $$('#st-states .st-st').forEach(b=>b.classList.toggle('on',b.dataset.state===editState)); studioDrive(); }

  // ---- save / url / delete ----
  function updateUrlLabel(){ $('st-url').textContent = slug ? (location.origin+'/overlay/'+slug+'.html') : 'save to get a URL'; }
  async function save(){
    doc.name = $('st-name').value.trim() || 'Untitled overlay';
    // A save that fails leaves the overlay where it was: unsaved if it was
    // new, or still on its own file (not a second "name-2" at the next try).
    const was = slug;
    if(!slug) slug = freeSlug(doc.name);
    const html = IC().bake(doc);
    try{
      const r = await fetch('/overlays/'+encodeURIComponent(slug), {method:'POST', headers:{'Content-Type':'text/html'}, body:html});
      const j = await r.json();
      // Auto-select this overlay when we return to the gallery so the
      // save reflects immediately (no F5).
      if(j.ok){ updateUrlLabel(); pendingSelect=slug; markClean(); toast('Saved. The URL is ready to drop into OBS','ok'); loadSaved(); return true; }
      toast(j.error||'Save failed','err');
    }catch(e){ toast('Save failed','err'); }
    slug = was;
    return false;
  }
  function copyUrl(btn){ if(!slug){ toast('Save the overlay first to get a URL','info'); return; } copyText(btn, location.origin+'/overlay/'+slug+'.html','Overlay URL'); }
  // ---- overflow menu: save-as / rename / export / import / delete ----
  function toggleMenu(){
    const p=$('st-menu-pop'); if(!p || !doc) return; p.hidden=!p.hidden;
    const purpose=IC().docPurpose(doc), mv=$('st-menu-move');
    mv.hidden = purpose==='goals' || purpose==='alerts';
    mv.textContent = purpose==='custom' ? 'Move to Delay' : 'Move to Custom';
  }
  // Save a separate copy under a new slug (so it never overwrites the current
  // overlay). The user can rename afterwards.
  // A copy that fails to save leaves the original as it was: on its own
  // file and under its own name (it used to forget its file, so the next
  // Save made a second overlay instead of updating it).
  async function saveAsNew(){
    const was = slug, name = doc.name, purpose = doc.purpose;
    slug=null; doc.name=(doc.name||'Overlay')+' copy'; $('st-name').value=doc.name;
    if(IC().docPurpose(doc)==='goals') doc.purpose='custom';
    if(await save()) return true;
    slug=was; doc.name=name; $('st-name').value=name; doc.purpose=purpose;
    return false;
  }
  function menu(action){ const p=$('st-menu-pop'); if(p) p.hidden=true;
    if(action==='delete') return deleteOverlay();
    if(action==='rename') return renameOverlay();
    if(action==='move'){
      doc.purpose = IC().docPurpose(doc)==='custom' ? 'delay' : 'custom'; updateDirty();
      // What the Studio offers follows: the delay's states, or none.
      enterMode(); renderLeft(); renderProps(); studioDrive();
      toast('It moves to the '+(doc.purpose==='custom'?'Custom':'Delay')+' page when you save','info');
      return;
    }
    if(action==='keys') return toggleKeys();
    if(action==='export') return exportDoc();
    if(action==='import') return importDoc();
  }
  async function renameOverlay(){
    const nn=(prompt('Rename overlay', doc.name)||'').trim(); if(!nn) return;
    // A goal's or a timer's overlay keeps its file, which its card finds it by.
    const old=slug; doc.name=nn; $('st-name').value=nn; slug=IC().docPurpose(doc)==='goals' ? old : freeSlug(nn, old);
    // A save that failed: the overlay keeps its old file (it used to be
    // deleted here, losing the overlay with the rename).
    if(!(await save())){ slug=old; return; }
    if(old && old!==slug){ try{ await fetch('/overlays/'+encodeURIComponent(old)+'/delete',{method:'POST'}); }catch(e){} loadSaved(); }
  }
  function exportDoc(){ doc.name=$('st-name').value.trim()||doc.name||'overlay';
    const blob=new Blob([JSON.stringify(doc,null,2)],{type:'application/json'});
    const a=document.createElement('a'); a.href=URL.createObjectURL(blob); a.download=slugify(doc.name)+'.json'; a.click();
    setTimeout(()=>URL.revokeObjectURL(a.href),1500); toast('Exported .json','ok'); }
  function importDoc(){ const inp=document.createElement('input'); inp.type='file'; inp.accept='.json,application/json';
    inp.onchange=()=>{ const f=inp.files&&inp.files[0]; if(!f) return; const rd=new FileReader();
      rd.onload=()=>{ try{ const d=JSON.parse(rd.result); if(!d||!Array.isArray(d.widgets)) throw 0;
        doc=IC().upgradeDoc(d); slug=null; clearPicked(); doc.widgets.forEach((w,n)=>{ if(!w.id) w.id='w'+(n+1); });
        $('st-name').value=doc.name||'Imported overlay';
        renderStates(); renderLeft(); rebake(); renderProps(); resetHistory();
        toast('Imported. Save to keep it','ok');
      }catch(e){ toast('Not a valid overlay .json','err'); } };
      rd.readAsText(f); };
    inp.click(); }
  async function deleteOverlay(){
    if(!slug){ toast('This draft is not saved yet','info'); return; }
    if(!confirm('Delete overlay "'+doc.name+'"? This removes its file from the overlays folder.')) return;
    const r = await postAction('/overlays/'+encodeURIComponent(slug)+'/delete');
    if(!r.ok){ toast(`Could not delete the overlay: ${r.error}`,'err'); return; }
    toast('Deleted','info'); slug=null; doc=null; mounted=false; backToGallery();
  }
  async function loadSaved(){
    try{ savedList = await (await fetch('/overlays')).json(); if(!Array.isArray(savedList)) savedList=[]; }catch(e){ savedList=[]; }
    return savedList;
  }
  // Overlays an older Studio baked (another build, or none written down)
  // are baked again from their own doc, so a fix to how layers draw reaches
  // them without anyone saving. Never while OBS is live: a saved overlay
  // reloads in OBS. Run once per visit; a failure tries again next visit.
  // Off air: OBS isn't sending, nothing goes out, and no hold or tail is
  // keeping a stream up (an overlay rebuilt then reloads in OBS mid-show).
  function offAir(s){ return !!s && !s.ingest_alive && !s.egress_alive && !s.hold && !(s.tail_ms > 0); }
  // Each visit tries once: going live stops it, to finish the next time
  // the stream is off; a failure waits for the next visit.
  let upgrading=false, upgraded=false;
  async function upgradeSaved(){
    if(upgrading || upgraded || !IC() || !offAir(lastState)) return;
    upgrading=true;
    let wrote=false, wentLive=false;
    try{
      const old=(await loadSaved()).filter(o=>o.studio && o.build!==IC().BUILD);
      for(const o of old){
        if(!offAir(lastState)){ wentLive=true; break; }
        const html=await fetch('/overlay/'+encodeURIComponent(o.slug)+'.html').then(r=>r.ok?r.text():'', ()=>'');
        const doc=IC().readDoc(html);
        if(!doc) continue;
        const r=await fetch('/overlays/'+encodeURIComponent(o.slug),{method:'POST',headers:{'Content-Type':'text/html'},
          body:IC().bake(o.build ? doc : layeredCard(o.slug, doc))}).then(x=>x.json(),()=>null);
        if(!r || !r.ok) break;
        wrote=true;
      }
      upgraded=!wentLive;
    }catch(e){ upgraded=true; console.error('saved overlays not updated:', e); }
    finally{ upgrading=false; }
    if(wrote){ await loadSaved(); if($('ovg-browser').offsetParent) renderGallery(true); }
  }
  // An alert design InstantClone made as one Alert card, before designs
  // were layers ("My alerts", the ready-made ones then): the same look, in
  // layers. One that layers can't draw (custom CSS for the card, the
  // picture on top) stays as it is.
  function layeredCard(slugId, doc){
    const w=doc.widgets || [];
    if(!/^(alert-[a-z]+|my-alerts(-\d+)?)$/.test(slugId) || w.length!==1 || w[0].kind!=='AlertCard') return doc;
    const layered=IC().alertLookDoc(Object.assign({theme:readyTheme(slugId) || 'island'}, w[0].card || {}));
    if(!layered) return doc;
    layered.name=doc.name;
    if(doc.sound) layered.sound=doc.sound;
    return layered;
  }
  // First-run seeding: bake the built-in presets to real overlay files so the
  // gallery is one uniform list (every item has a working URL). Gated by the
  // server `overlays_seeded` flag (set via POST /overlays/seeded) so deleted
  // presets stay gone. Skips any slug that already exists.
  let seeding=false;
  async function seedPresets(){
    if(seeding || !IC()) return; seeding=true;
    try{
      await loadSaved();
      for(let i=0;i<IC().PRESETS.length;i++){
        const p=IC().PRESETS[i];
        if(savedList.some(o=>o.slug===slugify(p.name))) continue;
        await savePreset(p);
      }
      await fetch('/overlays/seeded',{method:'POST'});
      await loadSaved(); renderGallery();
    }catch(e){}finally{ seeding=false; }
  }

  new ResizeObserver(fitPreview).observe($('ovg-stage'));
  $('st-hit').addEventListener('pointerdown', onCanvasDown);
  // (The middle button would start the browser's own scrolling.)
  $('st-hit').addEventListener('mousedown', e=>{ if(e.button===1) e.preventDefault(); });
  $('st-hit').addEventListener('contextmenu', onCanvasMenu);
  $('st-hit').addEventListener('pointerover', e=>{ const b=e.target.closest('.st-box'); markRow(b && b.dataset.id); });
  $('st-hit').addEventListener('pointerleave', ()=>markRow(null));
  $('st-canvas-frame').addEventListener('wheel', onCanvasWheel, {passive:false});
  $('st-ctx').addEventListener('click', e=>{
    const b = e.target.closest('[data-ctx]'); if(!b || b.disabled) return;
    closeContext(); CONTEXT[b.dataset.ctx]();
  });
  // A press anywhere but the menu closes it; so does the window losing focus.
  document.addEventListener('pointerdown', e=>{ if(!e.target.closest('#st-ctx')) closeContext(); }, true);
  window.addEventListener('blur', ()=>{ closeContext(); if(spaceHeld) holdSpace(false); });
  $('st-keys').addEventListener('click', e=>{ if(e.target===$('st-keys')) toggleKeys(); });
  // A double-click on a layer goes to its words (or its name) to type.
  $('st-hit').addEventListener('dblclick', e=>{
    const box = e.target.closest('.st-box'); if(!box || picked.length!==1 || sel!==box.dataset.id) return;
    const input = $('st-text') || $('st-props').querySelector('[data-st="setName"]');
    if(input){ input.focus(); input.select(); }
  });
  document.addEventListener('keydown', onKey);
  document.addEventListener('keyup', e=>{ if(e.code==='Space' && spaceHeld) holdSpace(false); });
  // A click anywhere but the menu (or its button) closes it.
  document.addEventListener('click', e=>{ const p=$('st-menu-pop'); if(p && !p.hidden && !e.target.closest('#st-menu-pop,[data-st="toggleMenu"]')) p.hidden=true; });
  document.addEventListener('click', dispatch);
  document.addEventListener('input', dispatch);

  // Keys while the Studio is open. Escape backs out one step at a time: a
  // drag, the field being typed in, what is picked, then the Studio.
  function onKey(e){
    const studio = $('ov-studio'); if(!studio || studio.hidden) return;
    const typing = !!(e.target.closest && e.target.closest('input,textarea,select,[contenteditable]'));
    const mod = e.ctrlKey || e.metaKey, key = e.key.length===1 ? e.key.toLowerCase() : e.key;
    if(key==='Escape'){
      e.preventDefault();
      if(closeContext()) return;
      if(!$('st-keys').hidden) toggleKeys();
      else if(gesture) gesture.cancel();
      else if(typing) e.target.blur();
      else if(picked.length) pick([]);
      else backToGallery();
      return;
    }
    if(mod && key==='z'){ e.preventDefault(); if(e.shiftKey) redo(); else undo(); return; }
    if(mod && key==='y'){ e.preventDefault(); redo(); return; }
    if(typing || gesture || !doc) return;
    // Space held: a drag moves the view. (Kept from pressing a button.)
    if(e.code==='Space'){ e.preventDefault(); if(!spaceHeld) holdSpace(true); return; }
    if(key==='?'){ e.preventDefault(); toggleKeys(); return; }
    if(mod && (key==='=' || key==='+')){ e.preventDefault(); zoomIn(); return; }
    if(mod && key==='-'){ e.preventDefault(); zoomOut(); return; }
    if(mod && key==='0'){ e.preventDefault(); zoomFit(); return; }
    // Every layer that can be picked on the canvas: not one hidden in the
    // state edited, which a Delete would take without it being seen.
    if(mod && key==='a'){ e.preventDefault(); pick([...boxes.keys()].filter(id=>!widget(id).locked)); return; }
    if(mod && key==='v'){ e.preventDefault(); pasteLayers(); return; }
    if(!picked.length) return;
    // Words selected on the page (the URL, a hint) copy as words.
    const selectedText = String(window.getSelection ? window.getSelection() : '');
    if(mod && (key==='c' || key==='x') && selectedText) return;
    if(mod && key==='c'){ e.preventDefault(); copyPicked(); return; }
    if(mod && key==='x'){ e.preventDefault(); copyPicked(); deletePicked(); return; }
    if(mod && e.shiftKey && key==='l'){ e.preventDefault(); setLocked(picked, !picked.every(id=>widget(id).locked)); return; }
    if(mod && key==='d'){ e.preventDefault(); duplicatePicked(); return; }
    if(key==='Delete' || key==='Backspace'){ e.preventDefault(); deletePicked(); return; }
    // Ctrl and up or down (or ] and [) restack; Shift, all the way.
    const ahead = key==='ArrowUp' || e.code==='BracketRight', behind = key==='ArrowDown' || e.code==='BracketLeft';
    if(mod && (ahead || behind)){
      e.preventDefault();
      if(e.shiftKey) stackEnd(picked, ahead); else stepLayers(picked, ahead ? 1 : -1);
      return;
    }
    const arrow = {ArrowLeft:[-1,0], ArrowRight:[1,0], ArrowUp:[0,-1], ArrowDown:[0,1]}[key];
    if(arrow && !mod){ e.preventDefault(); const n = e.shiftKey ? 10 : 1; nudge(arrow[0]*n, arrow[1]*n); }
  }

  // What the controls on the Overlay tab and in the Studio can do (see
  // `on`). A field's handler gets the control's arguments, then its value;
  // a button's gets its arguments, then the button.
  const ACTIONS = {
    newOverlay, restoreDefaults, previewSim, copyGallery, toggleAutohide, addGalleryToObs,
    openStudio: slugId => openStudio(slugId), duplicateOverlay, deleteFromGallery,
    editDesign, duplicateDesign, useDesign, resetDesign,
    revealOverlays: () => reveal('overlays'), setUpAlerts: () => Integrations.addTwitchAlerts(),
    backToGallery, undo, redo, save, saveAsNew, copyUrl, toggleMenu, menu, leftTab, play,
    setDocName, setName, set, setAnchor, setNum, align, setFlag, setLong, setLS, setBase, setCustom, insertVar,
    setCard: (k, v) => setCard(k, v), setCardNum, setCardColor, resetCard, setLiquidLevel, uploadImage,
    setTheme, swatch, setAlways, copyLookAll, setState, setStateNum, setStateText, applyColorAll, applyAnimAll,
    toggleGrad, setGrad, setGradAnim, animSet, animDel, animAdd, toggleHide, setHideAfter,
    setStateOffset, resetStateOffset, toggleVis, visOnly, visAll,
    duplicatePicked, deletePicked, lineUp, setManyLook, setFont,
    zoomIn, zoomOut, zoomFit: () => zoomFit(), toggleKeys: () => toggleKeys(),
  };
  // Fields act as they change, everything else when clicked; data-st-not
  // turns a checkbox around ("Show value" ticked = notext off).
  function dispatch(e){
    const el = e.target.closest && e.target.closest('[data-st]'); if(!el) return;
    const isField = /^(INPUT|SELECT|TEXTAREA)$/.test(el.tagName);
    if(isField !== (e.type==='input')) return;
    const fn = ACTIONS[el.dataset.st]; if(!fn) return;
    let args = [];
    try{ args = JSON.parse(el.dataset.stArgs || '[]'); }catch(_){ return; }
    if(!isField){ fn(...args, el); return; }
    let v = el.type==='checkbox' ? el.checked : el.value;
    if(el.hasAttribute('data-st-not')) v = !v;
    // Half typed, a value waits: "-" on the way to -50 would put the layer
    // at 0, "#5a" on the way to a colour would paint it black. A number
    // field left empty means 0 only where empty means something of its own.
    if(el.type==='number' && (el.validity.badInput || (v==='' && !el.hasAttribute('data-st-empty')))) return;
    const pair = el.closest('.st-color');
    if(pair && el.type!=='color' && !isColor(v)) return;
    // A colour's picker and its text say the same.
    if(pair) pair.querySelectorAll('input').forEach(i=>{ if(i!==el && i.value!==v && (i.type!=='color' || /^#[0-9a-f]{6}$/i.test(v))) i.value=v; });
    fn(...args, v, el);
  }
  // A colour the page can draw: a theme colour (@accent...) or any CSS one.
  const isColor = v => /^@[a-z]+$/.test(v) ? THEME_KEYS.includes(v.slice(1)) : CSS.supports('color', v);

  return { show, openStudio, loadSaved, upgradeSaved, seedPresets, isDirty, alertsChanged };
})();
