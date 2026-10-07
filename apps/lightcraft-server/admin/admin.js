// LightCraft server admin page (served at /admin; API: /api/admin/…, see src/admin.rs).
// Text from the server (user and device names, errors) is only ever set as text, never as HTML.
'use strict';

const $ = (id) => document.getElementById(id);
const KEY = 'lightcraftAdminToken';
let token = sessionStorage.getItem(KEY) || '';

/** An element with attributes and children (strings become text nodes). */
function el(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') e.className = v;
    else if (k.startsWith('on')) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? '' : String(v));
  }
  for (const kid of kids) e.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
  return e;
}

async function api(method, path, body) {
  const headers = { 'Content-Type': 'application/json' };
  if (token) headers.Authorization = 'Bearer ' + token;
  const r = await fetch('/api/admin/' + path, { method, headers, body: body === undefined ? undefined : JSON.stringify(body) });
  let data = null;
  try {
    data = await r.json();
  } catch (_) {
    data = null;
  }
  if (r.status === 401 && token && path !== 'login') {
    forget();
    show('login');
  }
  if (!r.ok) throw new Error((data && data.error) || 'the server answered ' + r.status);
  return data;
}

function notice(text, bad) {
  const n = $('notice');
  n.textContent = text;
  n.className = bad ? 'notice bad' : 'notice';
  n.hidden = !text;
  if (text && !bad) setTimeout(() => { if (n.textContent === text) n.hidden = true; }, 5000);
}

function forget() {
  token = '';
  sessionStorage.removeItem(KEY);
}

function show(view) {
  for (const v of ['setup', 'login', 'dash']) $(v).hidden = v !== view;
  $('who').hidden = view !== 'dash';
}

function bytes(n) {
  if (!Number.isFinite(n) || n <= 0) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(u.length - 1, Math.floor(Math.log(n) / Math.log(1000)));
  return (n / Math.pow(1000, i)).toFixed(i ? 1 : 0) + ' ' + u[i];
}

function when(secs) {
  if (!secs) return '—';
  return new Date(secs * 1000).toLocaleString();
}

/** A modal: a message, optionally one text input; resolves to the input (or true), null if cancelled. */
function ask({ title, text, label, type, ok, danger, check }) {
  return new Promise((resolve) => {
    const d = $('dialog');
    $('dialogTitle').textContent = title;
    $('dialogText').textContent = text || '';
    const input = $('dialogInput');
    $('dialogLabel').hidden = !label;
    $('dialogLabelText').textContent = label || '';
    input.value = '';
    input.type = type || 'text';
    $('dialogOk').textContent = ok || 'OK';
    $('dialogOk').className = danger ? 'danger' : '';
    const done = (v) => {
      $('dialogOk').onclick = null;
      $('dialogCancel').onclick = null;
      d.close();
      resolve(v);
    };
    $('dialogOk').onclick = (e) => {
      e.preventDefault();
      const v = label ? input.value : true;
      if (check && !check(v)) {
        input.focus();
        return;
      }
      done(v);
    };
    $('dialogCancel').onclick = () => done(null);
    d.oncancel = () => resolve(null);
    d.showModal();
    if (label) input.focus();
  });
}

// ------------------------------------------------------------------- server

async function loadStatus() {
  const s = await api('GET', 'status');
  $('whoName').textContent = s.me;
  const facts = $('facts');
  facts.replaceChildren();
  const fact = (k, v) => facts.append(el('dt', {}, k), el('dd', {}, v));
  fact('Version', s.version);
  fact('Data folder', s.data);
  fact('Listening on', s.listen);
  fact('Web build', s.web || 'not served (start with --web)');
  fact('Users', s.users);
  const disk = $('disk');
  disk.hidden = !s.disk;
  if (s.disk) {
    const used = Math.max(0, s.disk.total - s.disk.free);
    $('diskUsed').style.width = (s.disk.total ? (100 * used) / s.disk.total : 0).toFixed(1) + '%';
    $('diskText').textContent = bytes(s.disk.free) + ' free of ' + bytes(s.disk.total);
  }
}

// -------------------------------------------------------------------- users

async function loadUsers() {
  const users = await api('GET', 'users');
  const body = $('users').querySelector('tbody');
  body.replaceChildren();
  if (!users.length) body.append(el('tr', {}, el('td', { colspan: 6, class: 'empty' }, 'No users yet.')));
  const admins = users.filter((u) => u.admin).length;
  for (const u of users) {
    const name = el('td', {}, u.name);
    if (u.admin) name.append(' ', el('span', { class: 'badge' }, 'admin'));
    const actions = el(
      'td',
      { class: 'actions' },
      el('button', { type: 'button', class: 'secondary', onclick: () => openDevices(u.name) }, 'Devices'),
      el('button', { type: 'button', class: 'secondary', onclick: () => openFolders(u.name) }, u.folders && u.folders.length ? 'Folders (' + u.folders.length + ')' : 'Folders'),
      el('button', { type: 'button', class: 'secondary', onclick: () => resetPassword(u.name) }, 'Reset password'),
      el(
        'button',
        { type: 'button', class: 'secondary', disabled: u.admin && admins < 2, title: u.admin && admins < 2 ? 'The only admin' : null, onclick: () => setAdmin(u.name, !u.admin) },
        u.admin ? 'Remove admin' : 'Make admin',
      ),
      el('button', { type: 'button', class: 'danger', disabled: u.admin && admins < 2, onclick: () => removeUser(u.name) }, 'Remove'),
    );
    body.append(
      el('tr', {}, name, el('td', { class: 'num' }, u.photos), el('td', { class: 'num' }, u.albums), el('td', { class: 'num' }, u.devices), el('td', { class: 'num' }, bytes(u.bytes)), actions),
    );
  }
}

async function resetPassword(name) {
  const pw = await ask({
    title: 'Reset ' + name + "'s password",
    text: 'Their devices stay signed in; the new password is for signing in again.',
    label: 'New password',
    type: 'password',
    ok: 'Set password',
    check: (v) => v.length >= 8 || (notice('The password needs at least 8 characters.', true), false),
  });
  if (pw === null) return;
  try {
    await api('PUT', 'users/' + encodeURIComponent(name) + '/password', { password: pw });
    notice(name + "'s password was changed.");
  } catch (e) {
    notice(e.message, true);
  }
}

async function setAdmin(name, admin) {
  try {
    await api('PUT', 'users/' + encodeURIComponent(name) + '/admin', { admin });
    notice(admin ? name + ' is an admin now.' : name + ' is no longer an admin.');
    await loadUsers();
  } catch (e) {
    notice(e.message, true);
  }
}

async function removeUser(name) {
  const typed = await ask({
    title: 'Remove ' + name + '?',
    text: 'Their devices are signed out and they can no longer sign in. Their library and photo files stay on the server (in its data folder) until deleted by hand.',
    label: 'Type the user name to confirm',
    ok: 'Remove',
    danger: true,
    check: (v) => v === name,
  });
  if (typed === null) return;
  try {
    const r = await api('DELETE', 'users/' + encodeURIComponent(name));
    notice(name + ' was removed. Their files stay in ' + r.files + '.');
    $('devicesCard').hidden = true;
    closeFolders();
    await refresh();
  } catch (e) {
    notice(e.message, true);
  }
}

// ------------------------------------------------------------------ devices

let devicesOf = null;

async function openDevices(name) {
  devicesOf = name;
  $('devicesTitle').textContent = name + "'s devices";
  $('devicesCard').hidden = false;
  await loadDevices();
  $('devicesCard').scrollIntoView({ behavior: 'smooth', block: 'nearest' });
}

async function loadDevices() {
  if (!devicesOf) return;
  const name = devicesOf;
  let list;
  try {
    list = await api('GET', 'users/' + encodeURIComponent(name) + '/devices');
  } catch (e) {
    notice(e.message, true);
    return;
  }
  const body = $('devices').querySelector('tbody');
  body.replaceChildren();
  if (!list.length) body.append(el('tr', {}, el('td', { colspan: 5, class: 'empty' }, 'No devices signed in.')));
  for (const d of list) {
    body.append(
      el(
        'tr',
        {},
        el('td', {}, d.name || 'Device ' + d.id),
        el('td', {}, when(d.created)),
        el('td', {}, d.lastSeen ? when(d.lastSeen) : 'not since the server started'),
        el('td', { class: 'num' }, d.space),
        el('td', { class: 'actions' }, el('button', { type: 'button', class: 'danger', onclick: () => revoke(name, d) }, 'Sign out')),
      ),
    );
  }
}

async function revoke(name, d) {
  const ok = await ask({ title: 'Sign out ' + (d.name || 'device ' + d.id) + '?', text: 'It keeps its library and can sign in again with the password.', ok: 'Sign out', danger: true });
  if (!ok) return;
  try {
    await api('DELETE', 'users/' + encodeURIComponent(name) + '/devices/' + d.id);
    notice('Signed out.');
    await Promise.all([loadDevices(), loadUsers()]);
  } catch (e) {
    notice(e.message, true);
  }
}

// ---------------------------------------------------------- library folders

let foldersOf = null;
let foldersTimer = null;

async function openFolders(name) {
  foldersOf = name;
  $('foldersTitle').textContent = name + "'s library folders";
  $('foldersCard').hidden = false;
  await loadFolders();
  $('foldersCard').scrollIntoView({ behavior: 'smooth', block: 'nearest' });
}

function closeFolders() {
  $('foldersCard').hidden = true;
  foldersOf = null;
  clearTimeout(foldersTimer);
}

function scanText(s) {
  if (s.scanning) return s.todo ? 'Scanning: read ' + s.done + ' of ' + s.todo + ' file(s)…' : 'Scanning…';
  const parts = [];
  if (s.lastScan) parts.push('Last scan ' + when(s.lastScan) + ': ' + s.files + ' photo file(s)');
  const n = [[s.added, 'new'], [s.moved, 'moved'], [s.changed, 'changed'], [s.linked, 'uploaded before'], [s.failed, 'not read'], [s.missing, 'missing']]
    .filter(([v]) => v > 0)
    .map(([v, w]) => v + ' ' + w);
  if (n.length) parts.push(n.join(', '));
  if (s.previews) parts.push('building previews: ' + s.previews + ' to go');
  return parts.length ? parts.join('; ') + '.' : 'Not scanned yet.';
}

function showFolders(r) {
  const body = $('folders').querySelector('tbody');
  body.replaceChildren();
  if (!r.folders.length) body.append(el('tr', {}, el('td', { colspan: 3, class: 'empty' }, 'No library folders.')));
  for (const f of r.folders) {
    const where = el('td', {}, el('code', {}, f.path));
    if (!f.there) where.append(' ', el('span', { class: 'badge bad' }, 'not there'));
    body.append(el('tr', {}, el('td', {}, f.name), where, el('td', { class: 'actions' }, el('button', { type: 'button', class: 'danger', onclick: () => removeFolder(foldersOf, f) }, 'Remove'))));
  }
  $('scanText').textContent = scanText(r.scan);
  const errs = $('scanErrors');
  errs.replaceChildren(...(r.scan.errors || []).map((e) => el('li', {}, e)));
  errs.hidden = !(r.scan.errors || []).length;
  clearTimeout(foldersTimer);
  if (r.scan.scanning || r.scan.previews) foldersTimer = setTimeout(loadFolders, 2000);
}

async function loadFolders() {
  if (!foldersOf) return;
  try {
    showFolders(await api('GET', 'users/' + encodeURIComponent(foldersOf) + '/folders'));
  } catch (e) {
    notice(e.message, true);
  }
}

async function removeFolder(name, f) {
  const ok = await ask({
    title: 'Stop reading ' + f.name + '?',
    text: 'Its photos stay in ' + name + "'s library with their edits, but their originals can't be downloaded any more. Nothing in " + f.path + ' is touched.',
    ok: 'Remove',
    danger: true,
  });
  if (!ok) return;
  try {
    showFolders(await api('DELETE', 'users/' + encodeURIComponent(name) + '/folders/' + encodeURIComponent(f.name)));
    notice(f.name + ' is no longer read.');
    await loadUsers();
  } catch (e) {
    notice(e.message, true);
  }
}

// ----------------------------------------------------------------------- gc

async function gc(dryRun) {
  $('gcText').textContent = dryRun ? 'Checking…' : 'Removing…';
  try {
    const r = await api('POST', 'gc', { dryRun });
    $('gcText').textContent = (dryRun ? r.removed + ' file(s), ' + bytes(r.bytes) + ' can be removed.' : 'Removed ' + r.removed + ' file(s), ' + bytes(r.bytes) + '.') + (r.errors.length ? ' ' + r.errors.length + ' problem(s): ' + r.errors.join('; ') : '');
    $('gcRun').hidden = !dryRun || r.removed === 0;
    if (!dryRun) await loadUsers();
  } catch (e) {
    $('gcText').textContent = '';
    notice(e.message, true);
  }
}

// --------------------------------------------------------------------- flow

async function refresh() {
  await Promise.all([loadStatus(), loadUsers()]);
}

async function start() {
  $('origin').textContent = location.origin;
  $('origin2').textContent = location.origin;
  if (token) {
    try {
      await refresh();
      show('dash');
      return;
    } catch (_) {
      forget();
    }
  }
  try {
    const s = await api('GET', 'state');
    show(s.setup ? 'setup' : 'login');
  } catch (e) {
    show('login');
    notice(e.message, true);
  }
}

async function signIn(user, password) {
  const r = await api('POST', 'login', { user, password });
  token = r.token;
  sessionStorage.setItem(KEY, token);
  await refresh();
  show('dash');
}

document.addEventListener('DOMContentLoaded', () => {
  $('setupForm').addEventListener('submit', async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    if (f.get('password') !== f.get('password2')) return notice('The passwords differ.', true);
    try {
      await api('POST', 'setup', { code: f.get('code'), user: f.get('user'), password: f.get('password') });
      await signIn(f.get('user'), f.get('password'));
      notice('Welcome. Add the people who use this server, then connect their devices.');
    } catch (err) {
      notice(err.message, true);
    }
  });
  $('loginForm').addEventListener('submit', async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    try {
      await signIn(f.get('user'), f.get('password'));
      notice('');
    } catch (err) {
      notice(err.message, true);
    }
  });
  $('signOut').addEventListener('click', async () => {
    try {
      await api('POST', 'logout');
    } catch (_) {
      // signed out either way
    }
    forget();
    show('login');
  });
  $('addUserToggle').addEventListener('click', () => {
    $('addUser').hidden = !$('addUser').hidden;
  });
  $('addUser').addEventListener('submit', async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    try {
      await api('POST', 'users', { name: f.get('name'), password: f.get('password'), admin: f.get('admin') === 'on' });
      notice('Added ' + f.get('name') + '.');
      e.target.reset();
      e.target.hidden = true;
      await refresh();
    } catch (err) {
      notice(err.message, true);
    }
  });
  $('devicesClose').addEventListener('click', () => {
    $('devicesCard').hidden = true;
    devicesOf = null;
  });
  $('foldersClose').addEventListener('click', closeFolders);
  $('addFolder').addEventListener('submit', async (e) => {
    e.preventDefault();
    const f = new FormData(e.target);
    try {
      const r = await api('POST', 'users/' + encodeURIComponent(foldersOf) + '/folders', { path: f.get('path'), name: f.get('name') || null });
      e.target.reset();
      showFolders(r);
      notice('Added. The server is reading the folder now.');
      await loadUsers();
    } catch (err) {
      notice(err.message, true);
    }
  });
  $('scanNow').addEventListener('click', async () => {
    try {
      showFolders(await api('POST', 'users/' + encodeURIComponent(foldersOf) + '/scan'));
      $('scanText').textContent = 'Scanning…';
      clearTimeout(foldersTimer);
      foldersTimer = setTimeout(loadFolders, 1000);
    } catch (err) {
      notice(err.message, true);
    }
  });
  $('gcCheck').addEventListener('click', () => gc(true));
  $('gcRun').addEventListener('click', async () => {
    const ok = await ask({ title: 'Remove unused photo files?', text: "This can't be undone.", ok: 'Remove', danger: true });
    if (ok) gc(false);
  });
  start();
});
