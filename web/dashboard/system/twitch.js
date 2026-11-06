// - System > Twitch: the VOD audio track sections ----------------------------
// Fill the System-tab VOD sections: the script button label, the
// version-aware requirement callout, and - crucially - gate the legacy
// "EB + VOD audio track" launcher off on OBS 32.2+, where the VOD track
// must come from the unlocker script instead.
async function refreshSysVodScript(){
  const req = $('sys-vod-req');
  if (!req) return;
  const info = await obsRegInfo();
  const atLeast = obsVerAtLeast(info.obs_version, 32, 2, 0);
  const ver = info.obs_version ? ('OBS ' + info.obs_version) : null;
  const legacyBtn = $('sys-vodeb-btn');
  const legacyHint = $('sys-vodeb-hint');
  const legacySteer = $('sys-vodeb-legacy-note');
  req.hidden = false;
  if (atLeast === true){
    // Hard requirement: 32.2 locked the built-in VOD Track to Custom services.
    req.className = 'sys-callout req';
    setHtml(req, '<span class="sys-callout-ic">⚠</span><span><b>' + ver + ' requires this script for VOD audio.</b> OBS 32.2 stopped the built-in VOD Track working on Custom services, so the old checkbox and the "EB + VOD" launcher no longer add it. Use the script.</span>');
    if (legacyBtn) legacyBtn.hidden = true;
    if (legacyHint) legacyHint.hidden = true;
    if (legacySteer) legacySteer.hidden = false;
  } else if (atLeast === false){
    req.className = 'sys-callout info';
    setHtml(req, '<span class="sys-callout-ic">ℹ</span><span>On ' + ver + ' the built-in VOD Track checkbox still works too, but this script is simpler and keeps working after you update OBS.</span>');
    if (legacyBtn) legacyBtn.hidden = false;
    if (legacyHint) legacyHint.hidden = false;
    if (legacySteer) legacySteer.hidden = true;
  } else {
    // OBS version unknown (never launched / logs unreadable).
    req.className = 'sys-callout info';
    setHtml(req, '<span class="sys-callout-ic">ℹ</span><span><b>On OBS 32.2+ this script is required</b>: that version stopped the built-in VOD Track working on Custom services. Older OBS can still use the built-in VOD Track checkbox.</span>');
    if (legacyBtn) legacyBtn.hidden = false;
    if (legacyHint) legacyHint.hidden = false;
    if (legacySteer) legacySteer.hidden = true;
  }
  // OBS quick launch is the pre-32.2 Enhanced Broadcasting path - it relies
  // on OBS's --config-url flag, which 32.2+ ignores for custom services. It
  // stays folded on the OBS tab; the note says whether this OBS needs it.
  const ebWrap = $('sys-eb-launch');
  const ebNote = $('sys-eb-launch-note');
  if (ebWrap && ebNote){
    setText(ebNote, atLeast === true ? 'not needed on ' + ver : 'for OBS before 32.2');
  }
}
