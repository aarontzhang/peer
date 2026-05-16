export default function handler(req, res) {
  res.statusCode = 200;
  res.setHeader('content-type', 'text/html; charset=utf-8');
  res.end(`<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Signing in to Peer</title>
  <style>
    body{margin:0;font:14px/1.5 -apple-system,BlinkMacSystemFont,"SF Pro Text",Segoe UI,sans-serif;background:#151515;color:#f5f2ec;display:grid;place-items:center;min-height:100vh}
    main{width:min(420px,calc(100vw - 40px));border:1px solid #3a3833;border-radius:12px;padding:24px;background:#20201e;text-align:center}
    h1{font-size:22px;margin:0 0 8px}
    p{color:#b9b3a8;margin:0 0 18px}
    button{box-sizing:border-box;border-radius:8px;border:1px solid #8ec5ff;padding:10px 18px;font:inherit;background:#8ec5ff;color:#151515;font-weight:600;cursor:pointer}
    code{display:block;white-space:pre-wrap;word-break:break-all;background:#111;padding:12px;border-radius:8px;margin-top:14px;color:#f08a8a;text-align:left}
  </style>
</head>
<body>
  <main>
    <h1>Signing in to Peer</h1>
    <p id="status">Returning you to the app…</p>
    <div id="error"></div>
    <button id="open">Open Peer</button>
  </main>
  <script>
    (function () {
      var hash = window.location.hash || '';
      var search = new URLSearchParams(window.location.search);
      var nonce = search.get('nonce') || '';
      var requestedScheme = search.get('scheme') || 'peer';
      // Whitelist accepted schemes so this page can't be turned into an open
      // redirector to arbitrary peer-* handlers.
      var scheme = (requestedScheme === 'peer-dev') ? 'peer-dev' : 'peer';
      var query = nonce ? '?nonce=' + encodeURIComponent(nonce) : '';
      var target = scheme + '://auth' + query + hash;
      var openBtn = document.getElementById('open');
      var status = document.getElementById('status');
      var errorBox = document.getElementById('error');
      openBtn.addEventListener('click', function () { window.location.replace(target); });
      if (hash && hash.indexOf('error=') === -1) {
        window.location.replace(target);
      } else if (hash.indexOf('error=') !== -1) {
        status.textContent = 'Sign-in failed.';
        var params = new URLSearchParams(hash.replace(/^#/, ''));
        var msg = params.get('error_description') || params.get('error') || 'unknown error';
        errorBox.innerHTML = '<code>' + msg.replace(/[&<>]/g, function (c) { return ({'&':'&amp;','<':'&lt;','>':'&gt;'})[c]; }) + '</code>';
      } else {
        status.textContent = 'No sign-in tokens were returned.';
      }
    })();
  </script>
</body>
</html>`);
}
