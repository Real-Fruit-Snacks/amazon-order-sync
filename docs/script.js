// amazon-order-sync page: theme switch, install tabs, copy buttons, and the bench, where a scan runs
// down a packing slip and the note the server wrote for that order prints line by line.
// Without JS the first slip and its whole note are shown. With reduced motion nothing runs on its
// own and "Sync the next order" swaps orders without the scan.
(function () {
  var root = document.documentElement;
  var still = matchMedia('(prefers-reduced-motion: reduce)');
  var light = matchMedia('(prefers-color-scheme: light)');
  var $ = function (id) { return document.getElementById(id); };
  var all = function (sel, el) { return Array.prototype.slice.call((el || document).querySelectorAll(sel)); };
  var wait = function (ms) { return new Promise(function (ok) { setTimeout(ok, ms); }); };

  // ---- theme ----
  function theme() { return root.dataset.theme || (light.matches ? 'dayshift' : 'nightshift'); }
  function paint() {
    var night = theme() === 'nightshift';
    document.querySelector('meta[name="theme-color"]').content = night ? '#0C0D10' : '#F6F5F2';
    $('theme').setAttribute('aria-label', 'Theme: ' + (night ? 'Nightshift' : 'Dayshift') + '. Switch to ' + (night ? 'Dayshift' : 'Nightshift'));
  }
  $('theme').addEventListener('click', function () {
    root.dataset.theme = theme() === 'nightshift' ? 'dayshift' : 'nightshift';
    try { localStorage.setItem('rfs-theme', root.dataset.theme); } catch (e) {}
    paint();
  });
  light.addEventListener('change', paint);
  paint();

  // ---- install tabs ----
  var tabs = all('.tabs [role="tab"]');
  function pick(tab) {
    tabs.forEach(function (t) {
      var on = t === tab;
      t.setAttribute('aria-selected', on ? 'true' : 'false');
      t.tabIndex = on ? 0 : -1;
      $(t.getAttribute('aria-controls')).hidden = !on;
    });
  }
  tabs.forEach(function (t, i) {
    t.addEventListener('click', function () { pick(t); });
    t.addEventListener('keydown', function (e) {
      if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
      var n = tabs[(i + (e.key === 'ArrowRight' ? 1 : tabs.length - 1)) % tabs.length];
      pick(n); n.focus(); e.preventDefault();
    });
  });

  // ---- copy ----
  all('[data-copy]').forEach(function (b) {
    var label = b.textContent;
    b.addEventListener('click', function () {
      var done = function (ok) { b.textContent = ok ? 'Copied' : 'Select it'; clearTimeout(b._t); b._t = setTimeout(function () { b.textContent = label; }, 1600); };
      if (navigator.clipboard) navigator.clipboard.writeText(b.dataset.copy).then(function () { done(true); }, function () { done(false); });
      else done(false);
    });
  });

  // ---- the bench ----
  var orders = all('#bench .order'), btn = $('next'), said = $('said');
  var cur = 0, busy = false;

  // Scan the slip top to bottom; as the beam reaches a field, light it and print the note lines
  // that came from it. Then the rest of the note, the folder it was filed in, and "Saved".
  // Before the scan: the slip, an empty file, no folder yet.
  function clear(order) {
    order.classList.add('pending');
    all('.ln', order).forEach(function (l) { l.classList.remove('on', 'lit', 'cur'); });
    order.querySelector('.route').classList.remove('on');
    order.querySelector('.saved').dataset.s = 'idle';
    order.querySelector('.note').scrollTop = 0;
  }

  function print(order) {
    var slip = order.querySelector('.slip'), scan = slip.querySelector('.scan');
    var lines = all('.ln', order), route = order.querySelector('.route'), saved = order.querySelector('.saved');
    var top = slip.getBoundingClientRect().top, h = slip.offsetHeight, run = 1900, last = null, jobs = [];
    clear(order);
    saved.textContent = 'Writing'; saved.dataset.s = 'writing';

    function put(line) {
      if (last) last.classList.remove('cur');
      line.classList.add('on', 'lit', 'cur'); last = line;
      setTimeout(function () { line.classList.remove('lit'); }, 140);
    }
    all('.sf', slip).forEach(function (f) {
      var at = run * Math.max(0, f.getBoundingClientRect().top - top + 10) / h;
      var mine = lines.filter(function (l) { return l.dataset.f === f.dataset.f; });
      jobs.push(wait(at).then(function () {
        f.classList.add('hit'); setTimeout(function () { f.classList.remove('hit'); }, 160);
        return mine.reduce(function (p, l) { return p.then(function () { put(l); return wait(65); }); }, Promise.resolve());
      }));
    });
    scan.animate([{ opacity: 1, transform: 'translateY(0)' }, { opacity: 1, transform: 'translateY(' + (h - 3) + 'px)' }, { opacity: 0, transform: 'translateY(' + (h - 3) + 'px)' }],
      { duration: run + 260, easing: 'linear' });

    return Promise.all(jobs).then(function () { return wait(Math.max(0, run - 900)); }).then(function () {
      var body = lines.filter(function (l) { return !l.classList.contains('on'); });
      return body.reduce(function (p, l, i) { return p.then(function () { put(l); return i % 2 ? wait(16) : null; }); }, Promise.resolve());
    }).then(function () { return wait(120); }).then(function () {
      if (last) last.classList.remove('cur');
      route.classList.add('on');
      saved.textContent = 'Saved'; delete saved.dataset.s;
      order.classList.remove('pending');
      said.textContent = 'Saved ' + order.querySelector('.file-name').textContent;
    });
  }

  btn.addEventListener('click', function () {
    if (busy) return;
    var next = orders[(cur + 1) % orders.length];
    orders[cur].hidden = true; orders[cur].classList.remove('arrive');
    next.hidden = false; cur = orders.indexOf(next);
    if (still.matches) { said.textContent = 'Showing ' + next.querySelector('.file-name').textContent; return; }
    busy = true; btn.setAttribute('aria-disabled', 'true');
    clear(next); next.classList.add('arrive');
    wait(420).then(function () { return print(next); }).then(function () {
      busy = false; btn.removeAttribute('aria-disabled');
    });
  });

  // The first order prints once, when its slip is on screen.
  if (!still.matches && root.classList.contains('js-run')) {
    busy = true; btn.setAttribute('aria-disabled', 'true');
    clear(orders[0]); root.classList.remove('js-run');
    var go = function () {
      Promise.race([document.fonts ? document.fonts.ready : null, wait(1500)]).then(function () { return wait(250); })
        .then(function () { return print(orders[0]); })
        .then(function () { busy = false; btn.removeAttribute('aria-disabled'); });
    };
    if (!('IntersectionObserver' in window)) go();
    else {
      var seen = new IntersectionObserver(function (es) {
        if (es.some(function (e) { return e.isIntersecting; })) { seen.disconnect(); go(); }
      }, { threshold: 0.2 });
      seen.observe(orders[0].querySelector('.slip'));
    }
  }
})();
