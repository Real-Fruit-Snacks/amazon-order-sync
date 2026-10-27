// RFS project page: theme toggle (Nightshift/Dayshift), install tabs, copy buttons.
(function () {
  var root = document.documentElement;
  var mq = window.matchMedia('(prefers-color-scheme: light)');
  function current() {
    var t = root.getAttribute('data-theme');
    if (t) return t;
    return mq.matches ? 'dayshift' : 'nightshift';
  }
  function swapArt() {
    var dark = current() === 'nightshift';
    // <picture> follows the OS, not data-theme; drive the lockup explicitly.
    var lock = document.querySelector('[data-lockup]');
    var lockSrc = lock && lock.parentElement.querySelector('source');
    var url = 'https://raw.githubusercontent.com/Real-Fruit-Snacks/rfs-brand/main/assets/rfs-lockup-' + (dark ? 'dark' : 'light') + '.svg';
    if (lock) lock.src = url;
    if (lockSrc) lockSrc.srcset = url;
    var meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.content = dark ? '#0C0D10' : '#F6F5F2';
  }
  document.getElementById('theme').addEventListener('click', function () {
    var next = current() === 'nightshift' ? 'dayshift' : 'nightshift';
    root.setAttribute('data-theme', next);
    try { localStorage.setItem('rfs-theme', next); } catch (e) {}
    swapArt();
  });
  mq.addEventListener && mq.addEventListener('change', swapArt);
  swapArt();

  // install tabs + copy
  var tabs = document.querySelectorAll('.install [role="tab"]');
  tabs.forEach(function (b) {
    b.addEventListener('click', function () {
      tabs.forEach(function (x) { x.setAttribute('aria-selected', 'false'); });
      b.setAttribute('aria-selected', 'true');
      document.querySelectorAll('.install .pane').forEach(function (p) {
        if (p.getAttribute('data-id') === b.getAttribute('data-pane')) p.setAttribute('data-active', ''); else p.removeAttribute('data-active');
      });
    });
  });
  document.querySelectorAll('[data-copy]').forEach(function (b) {
    b.addEventListener('click', function () {
      var text = b.getAttribute('data-copy'), old = b.textContent;
      var done = function () { b.textContent = 'Copied'; setTimeout(function () { b.textContent = old; }, 1200); };
      if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).then(done, done); else done();
    });
  });
})();
