// The welcome page: the server's address to copy, its version, and whether the first administrator is still to be made.
(() => {
  const $ = (id) => document.getElementById(id);
  const origin = $('origin');
  origin.value = location.origin;

  $('copyOrigin').addEventListener('click', async () => {
    const label = $('copyOrigin').querySelector('span');
    try {
      await navigator.clipboard.writeText(origin.value);
      label.textContent = 'Copied';
    } catch {
      // (no clipboard on plain http from another machine: select the address for Ctrl+C)
      origin.select();
      label.textContent = 'Press Ctrl+C';
    }
    setTimeout(() => { label.textContent = 'Copy'; }, 2000);
  });

  fetch('/api/health', { cache: 'no-store' })
    .then((r) => (r.ok ? r.json() : Promise.reject(new Error(String(r.status)))))
    .then((h) => { if (h.version) $('version').textContent = `LightBites Server ${h.version}`; })
    .catch(() => {
      $('status').classList.add('down');
      $('statusText').textContent = 'Server is not answering';
    });

  fetch('/api/admin/state', { cache: 'no-store' })
    .then((r) => (r.ok ? r.json() : null))
    .then((s) => {
      if (!s || s.setup) return;
      // an administrator exists: step 1 is done, and the buttons say sign in
      $('stepAdmin').classList.add('done');
      $('stepAdmin').querySelector('h3').textContent = 'Administrator created';
      $('stepAdminText').innerHTML = 'Sign in on the <a href="/admin">admin page</a> to manage users, devices and storage.';
      $('heroAdmin').textContent = 'Sign in to the admin page';
    })
    .catch(() => {});
})();
