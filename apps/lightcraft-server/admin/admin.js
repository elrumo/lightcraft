// LightCraft server admin page (served at /admin; API: /api/admin/…, see src/admin.rs).
// Views by URL hash: #/overview, #/users, #/users/NAME[/devices|/folders], #/storage.
// Text from the server (user and device names, errors) is only ever set as text, never as HTML.
'use strict';

const $ = (id) => document.getElementById(id);
const KEY = 'lightcraftAdminToken';
const NAME = '[A-Za-z0-9._\\-]{1,64}';
let token = sessionStorage.getItem(KEY) || '';
let me = '';
let routeSeq = 0;
let foldersOf = null;
let timer = null;
let scanWorking = false;

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

/** An icon from the sprite in index.html. */
function icon(name) {
  const NS = 'http://www.w3.org/2000/svg';
  const s = document.createElementNS(NS, 'svg');
  s.setAttribute('class', 'icon');
  s.setAttribute('aria-hidden', 'true');
  const u = document.createElementNS(NS, 'use');
  u.setAttribute('href', '#i-' + name);
  s.append(u);
  return s;
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
    showAuth('login', 'Your session ended. Sign in again.', 'info');
    const e = new Error('signed out');
    e.signedOut = true;
    throw e;
  }
  if (!r.ok) {
    const msg = (data && data.error) || 'the server answered ' + r.status;
    throw new Error(msg.charAt(0).toUpperCase() + msg.slice(1));
  }
  return data;
}

// ------------------------------------------------------------- formatting

const enc = encodeURIComponent;
const n = (x) => Number(x || 0).toLocaleString();
const plural = (x, one, many) => n(x) + ' ' + (x === 1 ? one : many);
const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });

function bytes(x) {
  if (!Number.isFinite(x) || x <= 0) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  const i = Math.min(u.length - 1, Math.floor(Math.log(x) / Math.log(1000)));
  return (x / Math.pow(1000, i)).toFixed(i ? 1 : 0) + ' ' + u[i];
}

function ago(secs) {
  const d = secs - Date.now() / 1000;
  for (const [u, s] of [['year', 31536000], ['month', 2592000], ['week', 604800], ['day', 86400], ['hour', 3600], ['minute', 60]]) {
    if (Math.abs(d) >= s) return rtf.format(Math.round(d / s), u);
  }
  return 'just now';
}

const date = (secs) => new Date(secs * 1000).toLocaleString();

function dur(secs) {
  if (secs < 60) return secs + ' s';
  const m = Math.round(secs / 60);
  return m < 60 ? m + ' min' : Math.floor(m / 60) + ' h ' + (m % 60) + ' min';
}

function decode(s) {
  try {
    return decodeURIComponent(s);
  } catch (_) {
    return s;
  }
}

/** The colour of a name's avatar (one of eight). */
function hue(name) {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) % 997;
  return 'c' + (h % 8);
}

function paintAvatar(node, name, size) {
  node.className = 'avatar ' + hue(name) + (size ? ' ' + size : '');
  node.textContent = name.slice(0, 1);
}

const avatar = (name) => el('span', { class: 'avatar ' + hue(name), 'aria-hidden': 'true' }, name.slice(0, 1));
const badge = (text, kind) => el('span', { class: 'badge' + (kind ? ' ' + kind : '') }, text);

function fill(meter, fraction) {
  const bar = meter.firstElementChild;
  const f = Math.max(0, Math.min(1, fraction || 0));
  bar.style.width = (100 * f).toFixed(1) + '%';
  bar.className = f > 0.9 ? 'negative' : f > 0.75 ? 'notice' : '';
}

function emptyRow(cols, name, text) {
  return el('tr', { class: 'empty' }, el('td', { colspan: cols }, icon(name), text));
}

// ------------------------------------------------------- toasts and dialog

function toast(text, kind) {
  kind = kind || 'positive';
  const t = el(
    'div',
    { class: 'toast ' + kind, role: kind === 'negative' ? 'alert' : 'status' },
    icon(kind === 'negative' ? 'alert' : kind === 'info' ? 'info' : 'check'),
    el('span', {}, text),
    el('button', { type: 'button', 'aria-label': 'Dismiss', onclick: () => t.remove() }, icon('close')),
  );
  $('toasts').append(t);
  setTimeout(() => t.remove(), kind === 'negative' ? 10000 : 5000);
}

function fail(e) {
  if (!e.signedOut) toast(e.message, 'negative');
}

function busy(btn, on) {
  btn.disabled = on;
  btn.classList.toggle('busy', on);
}

function field(f) {
  const id = 'f-' + f.name;
  if (f.type === 'switch') {
    const input = el('input', { type: 'checkbox', id, role: 'switch' });
    const wrap = el('label', { class: 'switch', for: id }, input, el('span', {}, el('span', { class: 'label' }, f.label), f.help ? el('span', { class: 'help' }, f.help) : ''));
    return { f, input, wrap };
  }
  const area = f.type === 'textarea';
  const input = el(area ? 'textarea' : 'input', {
    id,
    name: f.name,
    rows: area ? 6 : undefined,
    type: area ? undefined : f.type || 'text',
    required: f.required !== false,
    minlength: f.minlength,
    pattern: f.pattern,
    placeholder: f.placeholder,
    autocomplete: f.autocomplete || 'off',
    spellcheck: 'false',
  });
  if (f.value) input.value = f.value;
  const wrap = el('div', { class: 'field' }, el('label', { for: id }, f.label), input, f.help ? el('p', { class: 'help' }, f.help) : '');
  return { f, input, wrap };
}

/**
 * A modal with optional fields. `action(values)` runs on OK; if it throws, its message shows in the
 * dialog and the dialog stays open. Resolves to the values, or null if cancelled.
 */
function ask({ title, text, fields, ok, danger, action }) {
  return new Promise((resolve) => {
    const d = $('dialog');
    const form = $('dialogForm');
    const okBtn = $('dialogOk');
    const err = $('dialogError');
    $('dialogTitle').textContent = title;
    $('dialogText').textContent = text || '';
    err.hidden = true;
    const inputs = (fields || []).map(field);
    $('dialogFields').replaceChildren(...inputs.map((i) => i.wrap));
    okBtn.textContent = ok || 'OK';
    okBtn.className = 'btn ' + (danger ? 'negative' : 'accent');
    busy(okBtn, false);
    let done = false;
    const finish = (v) => {
      if (done) return;
      done = true;
      form.onsubmit = null;
      d.onclose = null;
      if (d.open) d.close();
      resolve(v);
    };
    form.onsubmit = async (e) => {
      e.preventDefault();
      const values = {};
      for (const { f, input } of inputs) values[f.name] = f.type === 'switch' ? input.checked : input.value;
      if (action) {
        busy(okBtn, true);
        try {
          await action(values);
        } catch (x) {
          busy(okBtn, false);
          if (x.signedOut) return finish(null);
          err.querySelector('p').textContent = x.message;
          err.hidden = false;
          return;
        }
      }
      finish(values);
    };
    $('dialogCancel').onclick = () => finish(null);
    d.onclose = () => finish(null);
    d.showModal();
    const first = inputs.find((i) => i.f.type !== 'switch');
    (first ? first.input : danger ? $('dialogCancel') : okBtn).focus();
  });
}

// ---------------------------------------------------------------- routing

const VIEWS = ['overview', 'users', 'storage'];

async function route() {
  if ($('dash').hidden) return;
  const [, view, name, tab] = location.hash.split('/');
  clearTimeout(timer);
  foldersOf = null;
  scanWorking = false;
  const v = view === 'users' && name ? 'user' : VIEWS.includes(view) ? view : 'overview';
  for (const a of document.querySelectorAll('[data-nav]')) {
    if (a.dataset.nav === (v === 'user' ? 'users' : v)) a.setAttribute('aria-current', 'page');
    else a.removeAttribute('aria-current');
  }
  const seq = ++routeSeq;
  const load = { overview: loadOverview, users: loadUsers, storage: loadStorage, user: () => loadUser(decode(name), tab === 'folders' ? 'folders' : 'devices') }[v];
  try {
    await load();
  } catch (e) {
    fail(e);
  }
  if (seq !== routeSeq) return;
  for (const s of document.querySelectorAll('[data-view]')) {
    const was = !s.hidden;
    s.hidden = s.dataset.view !== v;
    if (!s.hidden && !was) window.scrollTo(0, 0);
  }
}

// --------------------------------------------------------------- overview

async function loadOverview() {
  const [s, users] = await Promise.all([api('GET', 'status'), api('GET', 'users')]);
  setMe(s.me);
  $('stUsers').textContent = n(users.length);
  $('stPhotos').textContent = n(users.reduce((a, u) => a + u.photos, 0));
  $('stDevices').textContent = n(users.reduce((a, u) => a + u.devices, 0));
  $('stFree').textContent = s.disk ? bytes(s.disk.free) : '—';
  $('stDisk').hidden = !s.disk;
  if (s.disk) fill($('stDisk'), s.disk.total ? 1 - s.disk.free / s.disk.total : 0);
  const facts = $('facts');
  facts.replaceChildren();
  const fact = (k, v, code) => facts.append(el('dt', {}, k), el('dd', { class: code ? 'code' : null }, v));
  fact('Version', s.version);
  fact('Data folder', s.data, true);
  fact('Listening on', s.listen, true);
  fact('Web app', s.web ? 'Served from ' + s.web : 'Not served (start the server with --web)');
  $('webHint').hidden = !s.web;
}

async function copyOrigin() {
  const input = $('origin');
  const btn = $('copyOrigin');
  try {
    await navigator.clipboard.writeText(input.value);
  } catch (_) {
    input.select();
    if (!document.execCommand('copy')) return toast('Select the address and copy it.', 'info');
  }
  btn.querySelector('use').setAttribute('href', '#i-check');
  btn.querySelector('span').textContent = 'Copied';
  setTimeout(() => {
    btn.querySelector('use').setAttribute('href', '#i-copy');
    btn.querySelector('span').textContent = 'Copy';
  }, 1500);
}

// ------------------------------------------------------------------ users

async function loadUsers() {
  const users = await api('GET', 'users');
  const body = $('users').querySelector('tbody');
  body.replaceChildren();
  if (!users.length) body.append(emptyRow(6, 'users', 'No users yet. Add the people who use this server.'));
  for (const u of users) {
    const href = '#/users/' + enc(u.name);
    const sub = [u.admin && 'Administrator', u.name === me && 'You'].filter(Boolean).join(' · ');
    const who = el('div', { class: 'who-cell' }, avatar(u.name), el('div', {}, el('a', { href }, u.name), sub ? el('span', { class: 'sub' }, sub) : ''));
    body.append(
      el(
        'tr',
        { class: 'link', onclick: (e) => e.target.closest('a') || (location.hash = href) },
        el('td', {}, who),
        el('td', { class: 'num' }, n(u.photos)),
        el('td', { class: 'num opt' }, n(u.albums)),
        el('td', { class: 'num opt' }, n(u.devices)),
        el('td', { class: 'num' }, bytes(u.bytes)),
        el('td', { class: 'chev-col' }, el('span', { class: 'chev' }, icon('chevron'))),
      ),
    );
  }
}

function addUser() {
  return ask({
    title: 'Add a user',
    text: 'They sign in to LightCraft on their devices with this user name and password, and get a library of their own.',
    fields: [
      { name: 'name', label: 'User name', pattern: NAME, help: 'Letters, digits, dots, dashes and underscores.' },
      { name: 'password', label: 'Password', type: 'password', minlength: 8, autocomplete: 'new-password', help: 'At least 8 characters. Give it to them privately.' },
      { name: 'admin', label: 'Administrator', type: 'switch', help: 'Can sign in here to manage users, devices and storage.' },
    ],
    ok: 'Add user',
    action: async (v) => {
      await api('POST', 'users', { name: v.name, password: v.password, admin: v.admin });
      toast('Added ' + v.name + '.');
      await route();
    },
  });
}

// --------------------------------------------------------------- one user

async function loadUser(name, tab) {
  const users = await api('GET', 'users');
  const u = users.find((x) => x.name === name);
  if (!u) {
    toast('There is no user ' + name + '.', 'negative');
    location.hash = '#/users';
    return;
  }
  paintAvatar($('uAvatar'), name, 'xl');
  $('uName').textContent = name;
  const role = el('div', { class: 'btn-group' }, u.admin ? badge('Administrator', 'accent') : badge('User'));
  if (name === me) role.append(badge('You'));
  $('uRole').replaceChildren(role);
  $('uPhotos').textContent = n(u.photos);
  $('uAlbums').textContent = n(u.albums);
  $('uDevices').textContent = n(u.devices);
  $('uBytes').textContent = bytes(u.bytes);

  const only = u.admin && users.filter((x) => x.admin).length < 2;
  const why = only ? name + ' is the only administrator.' : '';
  $('uAdmin').querySelector('span').textContent = u.admin ? 'Remove admin' : 'Make admin';
  for (const b of [$('uAdmin'), $('uRemove')]) {
    b.disabled = only;
    b.title = why;
  }
  $('uPassword').onclick = () => resetPassword(name);
  $('uAdmin').onclick = () => setAdmin(name, !u.admin);
  $('uRemove').onclick = () => removeUser(name);

  const counts = { devices: u.devices, folders: (u.folders || []).length };
  for (const a of document.querySelectorAll('.tabs a')) {
    a.href = '#/users/' + enc(name) + '/' + a.dataset.tab;
    a.setAttribute('aria-selected', String(a.dataset.tab === tab));
    let c = a.querySelector('.count');
    if (!c) a.append((c = el('span', { class: 'count' })));
    c.textContent = n(counts[a.dataset.tab]);
  }
  for (const p of document.querySelectorAll('[data-panel]')) p.hidden = p.dataset.panel !== tab;
  $('addFolder').onclick = () => addFolder(name);
  $('scanNow').onclick = () => scanNow(name);
  if (tab === 'folders') {
    foldersOf = name;
    await loadFolders(name);
  } else {
    await loadDevices(name);
  }
}

function resetPassword(name) {
  return ask({
    title: 'Reset password',
    text: 'Choose a new password for ' + name + '. Their devices stay signed in; the new password is for signing in again.',
    fields: [{ name: 'password', label: 'New password', type: 'password', minlength: 8, autocomplete: 'new-password', help: 'At least 8 characters.' }],
    ok: 'Set password',
    action: async (v) => {
      await api('PUT', 'users/' + enc(name) + '/password', { password: v.password });
      toast(name + "'s password was changed.");
    },
  });
}

function setAdmin(name, admin) {
  return ask({
    title: admin ? 'Make ' + name + ' an administrator?' : 'Remove administrator access?',
    text: admin
      ? 'Administrators can sign in here to add and remove users, sign out devices and clean up storage.'
      : name + ' keeps their library and devices, but can no longer sign in to this page.',
    ok: admin ? 'Make administrator' : 'Remove access',
    danger: !admin,
    action: async () => {
      await api('PUT', 'users/' + enc(name) + '/admin', { admin });
      toast(admin ? name + ' is an administrator now.' : name + ' is no longer an administrator.');
      await route();
    },
  });
}

function removeUser(name) {
  return ask({
    title: 'Remove ' + name + '?',
    text: 'Their devices are signed out and they can no longer sign in. Their library and photo files stay on the server, in its data folder, until you delete them by hand.',
    fields: [{ name: 'confirm', label: 'Type ' + name + ' to confirm' }],
    ok: 'Remove user',
    danger: true,
    action: async (v) => {
      if (v.confirm !== name) throw new Error('Type ' + name + ' exactly to confirm.');
      const r = await api('DELETE', 'users/' + enc(name));
      toast('Removed ' + name + '. Their files stay in ' + r.files + '.');
      location.hash = '#/users';
    },
  });
}

// ---------------------------------------------------------------- devices

async function loadDevices(name) {
  const list = await api('GET', 'users/' + enc(name) + '/devices');
  const body = $('devices').querySelector('tbody');
  body.replaceChildren();
  if (!list.length) body.append(emptyRow(4, 'device', 'No devices are signed in as ' + name + '.'));
  const now = Date.now() / 1000;
  for (const d of list) {
    const label = d.name || 'Device ' + d.id;
    const seen = !d.lastSeen
      ? el('span', { class: 'muted' }, 'Not since the server started')
      : now - d.lastSeen < 300
        ? el('span', { class: 'status active', title: date(d.lastSeen) }, 'Active now')
        : el('span', { title: date(d.lastSeen) }, ago(d.lastSeen));
    body.append(
      el(
        'tr',
        {},
        el('td', {}, el('div', { class: 'who-cell' }, el('span', { class: 'thing' }, icon('device')), el('div', {}, el('b', {}, label), el('span', { class: 'sub' }, 'Device ' + d.id + ' · ID space ' + d.space)))),
        el('td', { class: 'opt', title: d.created ? date(d.created) : null }, d.created ? ago(d.created) : '—'),
        el('td', {}, seen),
        el('td', { class: 'end' }, el('button', { type: 'button', class: 'btn quiet', onclick: () => revoke(name, d, label) }, icon('signout'), 'Sign out')),
      ),
    );
  }
}

function revoke(name, d, label) {
  return ask({
    title: 'Sign out ' + label + '?',
    text: 'It stops syncing right away. It keeps its library and can sign in again with ' + name + "'s password.",
    ok: 'Sign out',
    danger: true,
    action: async () => {
      await api('DELETE', 'users/' + enc(name) + '/devices/' + d.id);
      toast(label + ' was signed out.');
      await route();
    },
  });
}

// -------------------------------------------------------- library folders

async function loadFolders(name) {
  showFolders(name, await api('GET', 'users/' + enc(name) + '/folders'));
}

function scanInfo(s) {
  if (s.scanning) {
    if (s.phase === 'reading' && s.todo) {
      return { title: 'Scanning…', text: 'Reading ' + n(s.done) + ' of ' + n(s.todo) + ' new or changed files' + (s.etaSecs ? ', about ' + dur(s.etaSecs) + ' left.' : '.'), progress: s.done / s.todo };
    }
    if (s.phase === 'listing') return { title: 'Scanning…', text: 'Looking for photos: ' + n(s.files) + ' found so far.', progress: null };
    return { title: 'Scanning…', text: 'Finishing up.', progress: null };
  }
  if (s.previews) {
    const total = s.previewsTotal || s.previews;
    return {
      title: 'Building previews',
      text: n(s.previewsDone) + ' of ' + n(total) + ' done' + (s.previewsEtaSecs ? ', about ' + dur(s.previewsEtaSecs) + ' left' : '') + '. Photos show on devices as their previews are ready.',
      progress: total ? s.previewsDone / total : null,
    };
  }
  if (!s.lastScan) return { title: 'Not scanned yet', text: 'Scan to add the photos in these folders to the library.' };
  const changes = [[s.added, 'new'], [s.moved, 'moved'], [s.changed, 'changed'], [s.linked, 'uploaded before'], [s.missing, 'missing'], [s.failed, 'not read']]
    .filter(([v]) => v > 0)
    .map(([v, w]) => n(v) + ' ' + w);
  return { title: 'Last scanned ' + ago(s.lastScan), text: plural(s.files, 'photo file', 'photo files') + '. ' + (changes.length ? 'Changes: ' + changes.join(', ') + '.' : 'No changes.') };
}

function showFolders(name, r) {
  if (name !== foldersOf) return;
  const body = $('folders').querySelector('tbody');
  body.replaceChildren();
  if (!r.folders.length) body.append(emptyRow(4, 'folder', 'No library folders yet. Add a folder to show photos that are already on the server.'));
  for (const f of r.folders) {
    body.append(
      el(
        'tr',
        {},
        el('td', {}, el('div', { class: 'who-cell' }, el('span', { class: 'thing' }, icon('folder')), el('b', {}, f.name))),
        el('td', {}, el('code', {}, f.path)),
        el('td', {}, f.there ? el('span', { class: 'status positive' }, 'Available') : el('span', { class: 'status negative', title: 'The server cannot see this folder: is the disk mounted?' }, 'Not found')),
        el('td', { class: 'end' }, el('button', { type: 'button', class: 'btn quiet', onclick: () => removeFolder(name, f) }, icon('trash'), 'Remove')),
      ),
    );
  }
  const ignore = r.ignore || [];
  $('ignoreText').replaceChildren(
    ...(ignore.length
      ? ['Left out of the scan, with everything inside: ', ...ignore.flatMap((p, i) => [i ? ', ' : '', el('code', {}, p)])]
      : ['Nothing is left out. Name what to skip, like ', el('code', {}, '*.fcpbundle'), ' for Final Cut Pro bundles.']),
  );
  $('editIgnore').onclick = () => editIgnore(name, ignore);
  $('editIgnore').closest('.scan').hidden = !r.folders.length;
  const s = r.scan;
  const info = scanInfo(s);
  $('scanNow').closest('.scan').hidden = !r.folders.length;
  $('scanTitle').textContent = info.title;
  $('scanText').textContent = info.text;
  $('scanNow').disabled = !!s.scanning;
  const bar = $('scanBar');
  bar.hidden = info.progress === undefined;
  bar.classList.toggle('indeterminate', info.progress === null);
  if (info.progress === null) bar.firstElementChild.style.width = '';
  else if (info.progress !== undefined) fill(bar, info.progress);
  bar.firstElementChild.className = '';
  const errs = $('scanErrors');
  const all = (s.errors || []).concat((s.previewErrors || []).map((e) => 'Preview: ' + e));
  errs.querySelector('ul').replaceChildren(...all.map((e) => el('li', {}, e)));
  errs.hidden = !all.length;
  clearTimeout(timer);
  const working = !!(s.scanning || s.previews);
  if (working) timer = setTimeout(() => foldersOf === name && loadFolders(name).catch(fail), 2000);
  else if (scanWorking) route(); // done: the photo counts changed
  scanWorking = working;
}

function addFolder(name) {
  return ask({
    title: 'Add a library folder',
    text: "Its photos join " + name + "'s library and show on their devices. Nothing in the folder is copied, moved or changed.",
    fields: [
      { name: 'path', label: 'Folder on the server', placeholder: '/photos/' + name, help: 'A path the server can read. With Docker, mount the folder into the container read-only first (-v /srv/photos:/photos:ro) and enter /photos/…' },
      { name: 'name', label: 'Name on devices (optional)', required: false, help: "The folder's own name if left empty." },
    ],
    ok: 'Add folder',
    action: async (v) => {
      await api('POST', 'users/' + enc(name) + '/folders', { path: v.path, name: v.name || null });
      toast('Added. The server is reading the folder now.');
      await route();
    },
  });
}

function editIgnore(name, list) {
  return ask({
    title: 'Names to leave out',
    text: 'The scan skips any file or folder with one of these names, wherever it is in ' + name + "'s library folders, and everything inside it.",
    fields: [
      {
        name: 'ignore',
        type: 'textarea',
        label: 'One name per line',
        required: false,
        placeholder: '*.fcpbundle',
        value: list.join('\n'),
        help: '* stands for any text and ? for one character; capitals do not matter. Photos already in the library stay there, but their files show as missing.',
      },
    ],
    ok: 'Save',
    action: async (v) => {
      await api('PUT', 'users/' + enc(name) + '/ignore', { ignore: v.ignore.split('\n') });
      toast('Saved. The server is scanning again.');
      await route();
    },
  });
}

function removeFolder(name, f) {
  return ask({
    title: 'Stop reading ' + f.name + '?',
    text: "Its photos stay in " + name + "'s library with their edits, but their originals can no longer be downloaded. Nothing in " + f.path + ' is touched.',
    ok: 'Remove folder',
    danger: true,
    action: async () => {
      await api('DELETE', 'users/' + enc(name) + '/folders/' + enc(f.name));
      toast(f.name + ' is no longer read.');
      await route();
    },
  });
}

async function scanNow(name) {
  try {
    showFolders(name, await api('POST', 'users/' + enc(name) + '/scan'));
    $('scanTitle').textContent = 'Scanning…';
    $('scanNow').disabled = true;
    clearTimeout(timer);
    timer = setTimeout(() => foldersOf === name && loadFolders(name).catch(fail), 1000);
  } catch (e) {
    fail(e);
  }
}

// ---------------------------------------------------------------- storage

async function loadStorage() {
  const [s, users] = await Promise.all([api('GET', 'status'), api('GET', 'users')]);
  $('diskBox').hidden = !s.disk;
  $('diskNone').hidden = !!s.disk;
  if (s.disk) {
    const used = Math.max(0, s.disk.total - s.disk.free);
    const f = s.disk.total ? used / s.disk.total : 0;
    $('diskUsedText').textContent = bytes(used) + ' used';
    $('diskPct').textContent = Math.round(100 * f) + '%';
    fill($('diskUsed').parentElement, f);
    $('diskFree').textContent = bytes(s.disk.free) + ' free of ' + bytes(s.disk.total) + ', on the disk that holds ' + s.data + '.';
  }
  const list = $('usage');
  const sorted = users.slice().sort((a, b) => b.bytes - a.bytes);
  const top = sorted.length ? sorted[0].bytes : 0;
  list.replaceChildren();
  if (!sorted.length) list.append(el('li', { class: 'muted' }, 'No users yet.'));
  for (const u of sorted) {
    const meter = el('div', { class: 'meter' }, el('div', {}));
    list.append(el('li', {}, avatar(u.name), el('a', { class: 'name', href: '#/users/' + enc(u.name) }, u.name), meter, el('span', { class: 'bytes' }, bytes(u.bytes))));
    meter.firstElementChild.style.width = (top ? (100 * u.bytes) / top : 0).toFixed(1) + '%';
  }
}

let unused = 0;

function gcResult(kind, title, text) {
  const box = $('gcResult');
  box.className = 'alert ' + kind;
  box.querySelector('use').setAttribute('href', kind === 'positive' ? '#i-check' : kind === 'negative' ? '#i-alert' : '#i-info');
  $('gcTitle').textContent = title;
  $('gcText').textContent = text;
  box.hidden = false;
}

async function gcCheck() {
  const btn = $('gcCheck');
  busy(btn, true);
  try {
    const r = await api('POST', 'gc', { dryRun: true });
    unused = r.removed;
    const problems = r.errors.length ? ' ' + plural(r.errors.length, 'problem', 'problems') + ': ' + r.errors.join('; ') : '';
    if (r.removed) gcResult('info', plural(r.removed, 'unused file', 'unused files') + ', ' + bytes(r.bytes), 'Removing them frees this space. It cannot be undone.' + problems);
    else gcResult(problems ? 'negative' : 'positive', 'Nothing to clean up', 'Every photo file on the server is in use.' + problems);
    $('gcRun').hidden = !r.removed;
  } catch (e) {
    fail(e);
  } finally {
    busy(btn, false);
  }
}

function gcRun() {
  return ask({
    title: 'Remove ' + plural(unused, 'unused file', 'unused files') + '?',
    text: "No library uses them any more. This can't be undone.",
    ok: 'Remove files',
    danger: true,
    action: async () => {
      const r = await api('POST', 'gc', { dryRun: false });
      await loadStorage();
      $('gcRun').hidden = true;
      const problems = r.errors.length ? ' ' + plural(r.errors.length, 'problem', 'problems') + ': ' + r.errors.join('; ') : '';
      gcResult(problems ? 'negative' : 'positive', 'Removed ' + plural(r.removed, 'file', 'files'), 'Freed ' + bytes(r.bytes) + '.' + problems);
    },
  });
}

// ------------------------------------------------------------ sign in/out

function forget() {
  token = '';
  sessionStorage.removeItem(KEY);
}

function setMe(name) {
  me = name;
  $('whoName').textContent = name;
  paintAvatar($('meAvatar'), name);
}

function formAlert(form, message, kind) {
  const a = $(form).querySelector('.alert');
  a.className = 'alert ' + (kind || 'negative');
  a.querySelector('use').setAttribute('href', kind === 'info' ? '#i-info' : '#i-alert');
  a.querySelector('p').textContent = message || '';
  a.hidden = !message;
}

function showAuth(which, message, kind) {
  clearTimeout(timer);
  if ($('dialog').open) $('dialog').close();
  $('dash').hidden = true;
  $('auth').hidden = false;
  $('setup').hidden = which !== 'setup';
  $('login').hidden = which !== 'login';
  const form = which === 'setup' ? 'setupForm' : 'loginForm';
  formAlert(form, message, kind);
  const first = $(form).querySelector('input');
  if (first) first.focus();
}

function showDash() {
  $('auth').hidden = true;
  $('dash').hidden = false;
  route();
}

async function signIn(user, password) {
  const r = await api('POST', 'login', { user, password });
  token = r.token;
  sessionStorage.setItem(KEY, token);
  setMe(r.user);
  showDash();
}

async function start() {
  $('origin').value = location.origin;
  if (token) {
    try {
      setMe((await api('GET', 'status')).me);
      showDash();
      return;
    } catch (e) {
      forget();
      if (e.signedOut) return;
    }
  }
  try {
    const s = await api('GET', 'state');
    showAuth(s.setup ? 'setup' : 'login');
  } catch (e) {
    showAuth('login', e.message);
  }
}

/** Submit handler for the sign-in and setup forms: the button is busy while it runs. */
function onSubmit(form, run) {
  $(form).addEventListener('submit', async (e) => {
    e.preventDefault();
    const btn = e.target.querySelector('button[type=submit]');
    busy(btn, true);
    try {
      await run(new FormData(e.target));
    } catch (err) {
      if (!err.signedOut) formAlert(form, err.message);
    } finally {
      busy(btn, false);
    }
  });
}

document.addEventListener('DOMContentLoaded', () => {
  onSubmit('setupForm', async (f) => {
    if (f.get('password') !== f.get('password2')) throw new Error("The passwords don't match.");
    await api('POST', 'setup', { code: f.get('code'), user: f.get('user'), password: f.get('password') });
    location.hash = '#/users';
    await signIn(f.get('user'), f.get('password'));
    toast('Welcome! Add the people who use this server, then connect their devices.', 'info');
  });
  onSubmit('loginForm', async (f) => {
    await signIn(f.get('user'), f.get('password'));
    $('loginForm').reset();
  });
  $('signOut').addEventListener('click', async () => {
    try {
      await api('POST', 'logout');
    } catch (_) {
      // signed out either way
    }
    forget();
    showAuth('login');
  });
  $('copyOrigin').addEventListener('click', copyOrigin);
  $('addUser').addEventListener('click', addUser);
  $('gcCheck').addEventListener('click', gcCheck);
  $('gcRun').addEventListener('click', gcRun);
  window.addEventListener('hashchange', route);
  start();
});
