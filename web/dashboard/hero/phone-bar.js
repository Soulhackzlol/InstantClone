// - Phone delay bar -------------------------
// On a phone the delay card scrolls away; once its button is out of sight a
// slim bar keeps the delay and that button at the bottom edge. It mirrors
// the card (same number, state and words) and its button presses the
// card's, so it can never disagree with it.
(() => {
  const bar = $('mini-bar'), cta = $('cp-cta');
  if (!bar || !cta || !('IntersectionObserver' in window)) return;
  let queued = 0;
  function mirror(){
    queued = 0;
    const pill = $('state-pill');
    $('mini-num').textContent = $('delay-big').textContent;
    $('mini-dot').style.setProperty('--sc', pill.style.getPropertyValue('--sc'));
    $('mini-state').textContent = pill.querySelector('.state-label').textContent;
    const label = cta.querySelector('.cta-label:not(.leaving)');
    const button = $('mini-cta');
    button.textContent = (label || cta).textContent.trim();
    button.className = cta.className.split(' ').filter(c => c && !c.startsWith('cp-cta')).join(' ') + ' mini-cta';
    button.disabled = cta.disabled;
  }
  const later = () => { if (!queued) queued = requestAnimationFrame(mirror); };
  const watch = {subtree:true, childList:true, characterData:true, attributes:true, attributeFilter:['class', 'disabled', 'style']};
  [cta, $('delay-big'), $('state-pill')].forEach(el => new MutationObserver(later).observe(el, watch));
  // Shown only once the button has gone off the top, not before it is
  // first scrolled to, nor while the card is hidden (the setup wizard).
  new IntersectionObserver(([entry]) => {
    const away = !entry.isIntersecting && entry.boundingClientRect.bottom < 0;
    bar.classList.toggle('on', away);
    document.body.classList.toggle('mini-on', away);
  }).observe(cta);
  $('mini-cta').onclick = () => cta.click();
  $('mini-read').onclick = () => window.scrollTo({top:0, behavior:prefersReducedMotion() ? 'auto' : 'smooth'});
  bar.hidden = false;
  mirror();
})();

