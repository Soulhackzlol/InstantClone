// ---- An alert's bigger ones
// From an amount up, a moment's alert can play a bigger one instead (its
// own design, sound and words). Kept as If steps in the moment's own
// steps, "if {bits} is less than 1000: the alert, otherwise: the bigger
// one", each inside the last, so the builder shows them as they are.
const TIER_VARS = {twitch_cheer:'bits', twitch_raid:'viewers', twitch_gift:'count', twitch_resub:'months'};
const TIER_UNITS = {bits:'bits', viewers:'viewers', count:'subs', months:'months'};
const TIER_FIRST = {bits:1000, viewers:50, count:5, months:12};
// The amount's word in the page's language ("months", "meses").
function tierUnit(name){ return tr(TIER_UNITS[name]); }
// The alert and three bigger ones: three checks inside each other, as deep
// as steps go (model::MAX_DEPTH).
const MAX_TIERS = 4;
function tierVar(h){ return h && h.trigger.type === 'event' ? TIER_VARS[h.trigger.event] || '' : ''; }
// A link of the chain: a check on the amount with an alert on its yes side.
function isTierCheck(s, name){
  const p = s.params || {};
  return s.type === 'if' && s.enabled !== false && p.left === `{${name}}` && p.op === 'less' && +p.right > 0 && !p.match
    && (s.then || []).length === 1 && s.then[0].type === 'overlay' && (s.else || []).length === 1;
}
// The moment's alert and its bigger ones, smallest first ({from, step}),
// where the chain is in its steps, and the amount it goes by. Null for a
// moment with no amount, or whose alert is somewhere else (in a check of
// its own, built by hand).
function alertTiers(h){
  const name = tierVar(h);
  if (!name) return null;
  const at = h.steps.findIndex(s => s.type === 'overlay' || isTierCheck(s, name));
  if (at < 0) return null;
  const tiers = [];
  let s = h.steps[at], from = 0;
  while (isTierCheck(s, name)){
    tiers.push({from, step:s.then[0]});
    from = +s.params.right;
    s = s.else[0];
  }
  if (s.type !== 'overlay') return null;
  tiers.push({from, step:s});
  return {at, name, tiers};
}
// Write a chain back into the moment's steps.
function setTiers(h, chain){
  const t = chain.tiers;
  let s = t[t.length - 1].step;
  for (let n = t.length - 2; n >= 0; n--){
    s = {type:'if', params:{left:`{${chain.name}}`, op:'less', right:String(t[n + 1].from)}, then:[t[n].step], else:[s]};
  }
  h.steps[chain.at] = s;
}
// "From 1000 bits", or what the first one covers.
function tierLabel(chain, n){
  const unit = tierUnit(chain.name);
  if (n) return `From ${chain.tiers[n].from} ${unit}`;
  return chain.tiers.length > 1 ? `Below ${chain.tiers[1].from} ${unit}` : 'Every one';
}

