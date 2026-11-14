/* Oxide — motion + hero terrain. Author: x00f8 */

/* ------------------------------------------------------ hero terrain --- */
(function terrain() {
  const cv = document.getElementById('terrain');
  if (!cv) return;
  const ctx = cv.getContext('2d');
  let W, H, DPR;

  function size() {
    DPR = Math.min(window.devicePixelRatio || 1, 2);
    W = cv.clientWidth; H = cv.clientHeight;
    cv.width = W * DPR; cv.height = H * DPR;
    ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
  }
  size();
  window.addEventListener('resize', size);

  // cheap value-noise
  const perm = new Uint8Array(512);
  for (let i = 0; i < 256; i++) perm[i] = perm[i + 256] = (Math.random() * 256) | 0;
  const fade = t => t * t * t * (t * (t * 6 - 15) + 10);
  const lerp = (a, b, t) => a + (b - a) * t;
  function noise(x, y) {
    const xi = Math.floor(x) & 255, yi = Math.floor(y) & 255;
    const xf = x - Math.floor(x), yf = y - Math.floor(y);
    const u = fade(xf), v = fade(yf);
    const aa = perm[perm[xi] + yi], ab = perm[perm[xi] + yi + 1];
    const ba = perm[perm[xi + 1] + yi], bb = perm[perm[xi + 1] + yi + 1];
    return lerp(lerp(aa, ba, u), lerp(ab, bb, u), v) / 255 - 0.5;
  }
  function fbm(x, y) {
    let v = 0, a = 1, f = 1;
    for (let i = 0; i < 4; i++) { v += noise(x * f, y * f) * a; a *= 0.5; f *= 2.05; }
    return v;
  }

  const COLS = 74, ROWS = 42;
  let t = 0;
  let mx = 0, my = 0, tmx = 0, tmy = 0;

  window.addEventListener('mousemove', e => {
    tmx = (e.clientX / window.innerWidth - 0.5) * 2;
    tmy = (e.clientY / window.innerHeight - 0.5) * 2;
  });

  function frame() {
    t += 0.0016;
    mx += (tmx - mx) * 0.045;
    my += (tmy - my) * 0.045;

    ctx.clearRect(0, 0, W, H);

    const cx = W / 2 + mx * 34;
    const baseY = H * 0.66 + my * 18;
    const spanX = W * 1.5;
    const stepY = 15;

    for (let r = ROWS - 1; r >= 0; r--) {
      const depth = r / ROWS;
      const persp = 0.24 + depth * 0.98;
      const y0 = baseY - r * stepY * (0.5 + depth * 0.62);

      ctx.beginPath();
      let started = false;
      for (let c = 0; c <= COLS; c++) {
        const u = c / COLS - 0.5;
        const x = cx + u * spanX * persp;
        const h = fbm(c * 0.11 + t * 8, r * 0.15 - t * 3.2) * 120 * (0.35 + depth);
        const y = y0 - h;
        if (!started) { ctx.moveTo(x, y); started = true; } else { ctx.lineTo(x, y); }
      }

      const fadeOut = 1 - depth;
      const warm = Math.max(0, 1 - Math.abs(depth - 0.28) * 3.1);
      const alpha = 0.05 + fadeOut * 0.2;
      ctx.strokeStyle = `rgba(${140 + warm * 115 | 0}, ${150 - warm * 42 | 0}, ${168 - warm * 90 | 0}, ${alpha})`;
      ctx.lineWidth = 1;
      ctx.stroke();
    }
    requestAnimationFrame(frame);
  }
  if (!window.matchMedia('(prefers-reduced-motion: reduce)').matches) frame();
})();

/* --------------------------------------------------------------- nav --- */
(function nav() {
  const n = document.querySelector('nav');
  if (!n) return;
  const onScroll = () => n.classList.toggle('stuck', window.scrollY > 24);
  onScroll();
  window.addEventListener('scroll', onScroll, { passive: true });

  // mobile drawer, built from the desktop links so the two never drift apart
  const burger = document.querySelector('.burger');
  const links = document.querySelector('.nav-links');
  if (!burger || !links) return;

  const drawer = document.createElement('div');
  drawer.className = 'drawer';
  drawer.innerHTML = links.innerHTML;
  document.body.appendChild(drawer);

  burger.setAttribute('aria-expanded', 'false');
  burger.addEventListener('click', () => {
    const open = drawer.classList.toggle('open');
    burger.setAttribute('aria-expanded', String(open));
  });
  drawer.addEventListener('click', e => {
    if (e.target.tagName === 'A') {
      drawer.classList.remove('open');
      burger.setAttribute('aria-expanded', 'false');
    }
  });
})();

/* -------------------------------------------------------- card sheen --- */
document.querySelectorAll('.card').forEach(card => {
  card.addEventListener('mousemove', e => {
    const r = card.getBoundingClientRect();
    card.style.setProperty('--mx', `${e.clientX - r.left}px`);
    card.style.setProperty('--my', `${e.clientY - r.top}px`);
  });
});

/* ------------------------------------------------------- copy address --- */
document.querySelectorAll('[data-copy]').forEach(el => {
  el.addEventListener('click', () => {
    navigator.clipboard?.writeText(el.dataset.copy);
    const lbl = el.querySelector('.cp');
    if (!lbl) return;
    const old = lbl.textContent;
    lbl.textContent = 'copied';
    setTimeout(() => (lbl.textContent = old), 1400);
  });
});

/* -------------------------------------------------------------- gsap --- */
function showEverything() {
  document.querySelectorAll('.rise').forEach(el => {
    el.style.opacity = '1';
    el.style.transform = 'none';
  });
  document.querySelectorAll('.bar-fill').forEach(b => (b.style.width = b.dataset.w + '%'));
  document.querySelectorAll('[data-count]').forEach(el => {
    const dec = (el.dataset.dec | 0);
    el.textContent = parseFloat(el.dataset.count)
      .toLocaleString('en-US', { minimumFractionDigits: dec, maximumFractionDigits: dec });
  });
}

window.addEventListener('DOMContentLoaded', () => {
  // If the GSAP CDN is blocked or fails, never leave content stuck at opacity:0.
  if (!window.gsap || !window.ScrollTrigger) { showEverything(); return; }
  gsap.registerPlugin(ScrollTrigger);

  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (reduce) { showEverything(); return; }

  // hero entrance
  const hero = gsap.timeline({ defaults: { ease: 'power3.out' } });
  hero.from('.eyebrow', { y: 16, opacity: 0, duration: .7 })
      .from('h1.display', { y: 30, opacity: 0, duration: .95 }, '-=0.42')
      .from('.lede',      { y: 22, opacity: 0, duration: .8  }, '-=0.62')
      .from('.hero .row > *', { y: 18, opacity: 0, duration: .65, stagger: .08 }, '-=0.5')
      .from('.strip div', { y: 22, opacity: 0, duration: .7, stagger: .07 }, '-=0.34');

  // generic reveal
  gsap.utils.toArray('.rise').forEach(el => {
    gsap.to(el, {
      opacity: 1, y: 0, duration: .85, ease: 'power3.out',
      scrollTrigger: { trigger: el, start: 'top 86%' }
    });
  });

  // staggered groups
  gsap.utils.toArray('[data-stagger]').forEach(group => {
    gsap.from(group.children, {
      y: 26, opacity: 0, duration: .7, ease: 'power3.out', stagger: .075,
      scrollTrigger: { trigger: group, start: 'top 82%' }
    });
  });

  // benchmark bars
  gsap.utils.toArray('.bar-fill').forEach(bar => {
    gsap.to(bar, {
      width: bar.dataset.w + '%', duration: 1.5, ease: 'power4.out',
      scrollTrigger: { trigger: bar, start: 'top 88%' }
    });
  });

  // counters
  gsap.utils.toArray('[data-count]').forEach(el => {
    const target = parseFloat(el.dataset.count);
    const dec = (el.dataset.dec | 0);
    const obj = { v: 0 };
    gsap.to(obj, {
      v: target, duration: 1.7, ease: 'power3.out',
      scrollTrigger: { trigger: el, start: 'top 90%' },
      onUpdate() {
        el.textContent = obj.v.toLocaleString('en-US', {
          minimumFractionDigits: dec, maximumFractionDigits: dec
        });
      }
    });
  });

  // hero parallax on terrain
  gsap.to('#terrain', {
    yPercent: 14, opacity: .18, ease: 'none',
    scrollTrigger: { trigger: '.hero', start: 'top top', end: 'bottom top', scrub: true }
  });
});
