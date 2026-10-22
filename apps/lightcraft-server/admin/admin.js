// LightCraft server admin page (served at /admin; API: /api/admin/…, see src/admin.rs).
// Views by URL hash: #/overview, #/users, #/users/NAME[/devices|/folders|/shares], #/presets, #/processing (was #/search), #/storage.
// Text from the server (user and device names, errors) is only ever set as text, never as HTML.
'use strict';

const $ = (id) => document.getElementById(id);
const NAME = '[A-Za-z0-9._\\-]{1,64}';
// The session is the browser's `lc_admin` cookie (HttpOnly: this script can't read it, so can't leak it). An older
// version of this page kept a token in the tab, which was lost with it: forget that one.
try {
  sessionStorage.removeItem('lightcraftAdminToken');
} catch (_) {
  // (no storage: nothing to forget)
}
let signedIn = false;
let me = '';
let routeSeq = 0;
let foldersOf = null;
let shownUser = '';
let timer = null;
let recountTimer = null;
let scanWorking = false;
let checkTimer = null;
const cache = new Map(); // GET path -> its last answer and when it came
const inflight = new Map(); // GET path -> the request on its way

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
  // (the server wants this header on what changes something: a form on another site can't send it)
  const headers = { 'Content-Type': 'application/json', 'X-LightCraft-Admin': '1' };
  const r = await fetch('/api/admin/' + path, { method, headers, credentials: 'same-origin', body: body === undefined ? undefined : JSON.stringify(body) });
  let data = null;
  try {
    data = await r.json();
  } catch (_) {
    data = null;
  }
  if (r.status === 401 && signedIn && path !== 'login') {
    forget();
    showAuth('login', 'Your session ended. Sign in again.', 'info');
    const e = new Error('signed out');
    e.signedOut = true;
    throw e;
  }
  if (!r.ok) {
    const msg = (data && data.error) || 'the server answered ' + r.status;
    const e = new Error(msg.charAt(0).toUpperCase() + msg.slice(1));
    e.status = r.status;
    throw e;
  }
  if (method !== 'GET') {
    // (something changed: what was read before is not to be shown as current)
    cache.clear();
    inflight.clear();
  }
  return data;
}

/** GET `path`; asking for what is already on its way shares that request. */
function get(path) {
  let p = inflight.get(path);
  if (!p) {
    const current = () => inflight.get(path) === p;
    p = api('GET', path)
      .then((data) => {
        if (current()) cache.set(path, { data, at: Date.now() });
        return data;
      })
      .finally(() => current() && inflight.delete(path));
    inflight.set(path, p);
  }
  return p;
}

/**
 * Ask for `paths` (all at once) and show them: from the last answers at once if there are, then from
 * fresh ones if they differ (answers less than a second old are fresh). `paint(answers)`; nothing
 * is painted for a view left meanwhile. Resolves to the freshest answers.
 */
async function swr(paths, paint) {
  const seq = routeSeq;
  const old = paths.map((p) => cache.get(p));
  const had = old.every(Boolean);
  const recent = had && old.every((o) => Date.now() - o.at < 1000);
  const asked = recent ? [] : paths.map(get);
  const before = had ? old.map((o) => o.data) : null;
  if (had) paint(before);
  if (recent) return before;
  const now = await Promise.all(asked);
  if (seq === routeSeq && !(had && JSON.stringify(before) === JSON.stringify(now))) paint(now);
  return now;
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

/** A size the server is still counting (`null`) shows as … */
const size = (x) => (x === null ? '…' : bytes(x));

/**
 * What a user's storage is made of, when they have library folders: the files the server keeps for them (uploads and
 * previews) and their photos in those folders, which it only reads, in place. `u.bytes` counts both.
 */
function storageParts(u) {
  if (u.bytes === null || !u.folderBytes) return '';
  // (a size doesn't break between its number and its unit)
  const fixed = (x) => bytes(x).replace(' ', '\u00a0');
  return fixed(u.bytes - u.folderBytes) + ' kept by LightCraft · ' + fixed(u.folderBytes) + ' in library folders';
}

/**
 * What a user's videos take: how many, their own files by the sizes the library knows, and, once the server keeps lighter
 * copies of them, those copies and the room they saved. Videos are not counted among the photos.
 */
function videoParts(u) {
  if (!u.videos) return '';
  const fixed = (x) => bytes(x).replace(' ', '\u00a0');
  const parts = [fixed(u.videoBytes || 0) + ' of originals'];
  if (u.videoCopies) parts.push(plural(u.videoCopies, 'lighter copy', 'lighter copies') + ' (' + fixed(u.videoCopyBytes) + ')', fixed(u.videoSaved) + ' saved');
  return parts.join(' · ');
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
  if (m < 60) return m + ' min';
  const h = Math.floor(m / 60);
  return h < 48 ? h + ' h ' + (m % 60) + ' min' : Math.round(h / 24) + ' days';
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

/** A meter that shows how far something is, not how full something is (no warning colours). */
function progress(meter, fraction) {
  const bar = meter.firstElementChild;
  bar.style.width = (100 * Math.max(0, Math.min(1, fraction || 0))).toFixed(1) + '%';
  bar.className = '';
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
    input.checked = !!f.value;
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
    step: f.step,
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

const VIEWS = ['overview', 'users', 'presets', 'processing', 'storage'];

async function route() {
  if ($('dash').hidden) return;
  const [, view0, name, tab] = location.hash.split('/');
  const view = view0 === 'search' ? 'processing' : view0; // (the page's old name: bookmarks still work)
  clearTimeout(timer);
  clearTimeout(recountTimer);
  clearTimeout(usageTimer);
  foldersOf = null;
  scanWorking = false;
  const v = view === 'users' && name ? 'user' : VIEWS.includes(view) ? view : 'overview';
  for (const a of document.querySelectorAll('[data-nav]')) {
    if (a.dataset.nav === (v === 'user' ? 'users' : v)) a.setAttribute('aria-current', 'page');
    else a.removeAttribute('aria-current');
  }
  ++routeSeq;
  // (the page is shown at once, with what was last known; the answers fill it in)
  for (const s of document.querySelectorAll('[data-view]')) {
    const was = !s.hidden;
    s.hidden = s.dataset.view !== v;
    if (!s.hidden && !was) window.scrollTo(0, 0);
  }
  const load = { overview: loadOverview, users: loadUsers, presets: loadPresets, processing: loadSearch, storage: loadStorage, user: () => loadUser(decode(name), tab === 'folders' || tab === 'shares' ? tab : 'devices') }[v];
  try {
    await load();
  } catch (e) {
    fail(e);
  }
}

/** Sizes the server is still counting are asked for again soon. */
function recount(users) {
  clearTimeout(recountTimer);
  if (users.some((u) => u.bytes === null)) recountTimer = setTimeout(route, 1500);
}

// ---------------------------------------------------------------- presets

/** The shared presets as last read: `{version, items, builtin}` (see src/server_presets.rs). */
let shared = null;

function loadPresets() {
  return swr(['presets'], ([p]) => showPresets(p));
}

const isHiddenItem = (it) => it.id === 'hidden';
/** The built-in presets and groups hidden for everyone. */
const hiddenOf = (items) => items.find(isHiddenItem) || { id: 'hidden', presets: [], groups: [] };

/** `[[group, presets]]`, groups in the order they first come. */
function byGroup(list) {
  const m = new Map();
  for (const p of list) {
    if (!m.has(p.group)) m.set(p.group, []);
    m.get(p.group).push(p);
  }
  return [...m];
}

/** A row's action: icon and word, the icon alone on a phone (the word stays its name). */
function rowButton(name, text, onclick) {
  return el('button', { type: 'button', class: 'btn quiet', title: text, 'aria-label': text, onclick }, icon(name), el('span', { class: 'btn-word' }, text));
}

function showPresets(p) {
  shared = p;
  const presets = p.items.filter((i) => !isHiddenItem(i));
  const body = $('sharedPresets').querySelector('tbody');
  body.replaceChildren();
  if (!presets.length) body.append(emptyRow(2, 'presets', 'No shared presets yet. Import LightCraft, XMP or Lightroom preset files, or a zip of them.'));
  for (const [group, list] of byGroup(presets)) {
    body.append(
      el(
        'tr',
        { class: 'group' },
        el('td', {}, el('b', {}, group), el('span', { class: 'sub' }, plural(list.length, 'preset', 'presets'))),
        el('td', { class: 'end' }, rowButton('edit', 'Rename', () => renameGroup(group)), rowButton('trash', 'Delete', () => deleteGroup(group, list.length))),
      ),
    );
    for (const it of list) {
      body.append(
        el(
          'tr',
          {},
          el('td', {}, el('div', { class: 'who-cell' }, el('span', { class: 'thing' }, icon('presets')), el('b', {}, it.name))),
          el('td', { class: 'end' }, rowButton('edit', 'Rename', () => renamePreset(it)), rowButton('folder', 'Move', () => movePreset(it)), rowButton('trash', 'Delete', () => deletePreset(it))),
        ),
      );
    }
  }
  const hidden = hiddenOf(p.items);
  const builtin = $('builtinPresets').querySelector('tbody');
  builtin.replaceChildren();
  for (const [group, list] of byGroup(p.builtin)) {
    const off = hidden.groups.includes(group);
    builtin.append(el('tr', { class: 'group' }, el('td', {}, el('b', {}, group)), el('td', { class: 'end' }, shownSwitch(group, !off, (on) => hideBuiltin('groups', group, !on)))));
    for (const b of list) {
      const on = !off && !hidden.presets.includes(b.id);
      builtin.append(el('tr', {}, el('td', {}, b.name), el('td', { class: 'end' }, shownSwitch(b.name, on, (now) => hideBuiltin('presets', b.id, !now), off))));
    }
  }
}

/** A "shown to users" switch for `what`; `change(on)` saves. */
function shownSwitch(what, on, change, disabled) {
  const input = el('input', { type: 'checkbox', role: 'switch', 'aria-label': 'Show ' + what + ' to users', disabled });
  input.checked = on;
  input.addEventListener('change', async () => {
    input.disabled = true;
    try {
      await change(input.checked);
    } catch (e) {
      input.checked = !input.checked;
      fail(e);
    }
    input.disabled = !!disabled;
  });
  return el('label', { class: 'switch' }, input);
}

/** Save `items` as the change of the presets last read, then show them. Someone else's change first: theirs are shown, and it throws. */
async function savePresets(items) {
  try {
    await api('PUT', 'presets', { version: shared.version, items });
  } catch (e) {
    if (e.status !== 412) throw e;
    await route();
    throw new Error('Someone else changed the presets meanwhile: here they are now. Try again.');
  }
  await route();
}

const sharedItems = () => shared.items.filter((i) => !isHiddenItem(i));
/** The items with the presets `presets` and the hidden built-ins kept. */
const withHidden = (presets) => {
  const h = hiddenOf(shared.items);
  return h.presets.length || h.groups.length ? [...presets, h] : presets;
};
const groupsHelp = () => 'Groups: ' + byGroup(sharedItems()).map(([g]) => g).join(', ') + '. A new name makes a new group.';

function renamePreset(it) {
  return ask({
    title: 'Rename ' + it.name,
    fields: [{ name: 'name', label: 'Name', value: it.name }],
    ok: 'Rename',
    action: (v) => savePresets(withHidden(sharedItems().map((x) => (x.id === it.id ? { ...x, name: v.name.trim() } : x)))),
  });
}

function movePreset(it) {
  return ask({
    title: 'Move ' + it.name,
    fields: [{ name: 'group', label: 'Group', value: it.group, help: groupsHelp() }],
    ok: 'Move',
    action: (v) => savePresets(withHidden(sharedItems().map((x) => (x.id === it.id ? { ...x, group: v.group.trim() } : x)))),
  });
}

function deletePreset(it) {
  return ask({
    title: 'Delete ' + it.name + '?',
    text: 'It goes from every user’s devices the next time they sync. Edits made with it stay.',
    ok: 'Delete',
    danger: true,
    action: async () => {
      await savePresets(withHidden(sharedItems().filter((x) => x.id !== it.id)));
      toast(it.name + ' was deleted.');
    },
  });
}

function renameGroup(group) {
  return ask({
    title: 'Rename the group ' + group,
    fields: [{ name: 'name', label: 'Name', value: group, help: 'A group that has this name already takes its presets.' }],
    ok: 'Rename',
    action: (v) => savePresets(withHidden(sharedItems().map((x) => (x.group === group ? { ...x, group: v.name.trim() } : x)))),
  });
}

function deleteGroup(group, count) {
  return ask({
    title: 'Delete the group ' + group + '?',
    text: 'Its ' + plural(count, 'preset goes', 'presets go') + ' from every user’s devices the next time they sync. Edits made with them stay.',
    ok: 'Delete',
    danger: true,
    action: async () => {
      await savePresets(withHidden(sharedItems().filter((x) => x.group !== group)));
      toast('The group ' + group + ' was deleted.');
    },
  });
}

/** Hide (or show) the built-in preset or group `name` (`kind`: `presets` or `groups`) for every user. */
function hideBuiltin(kind, name, hide) {
  const h = hiddenOf(shared.items);
  const next = { id: 'hidden', presets: [...h.presets], groups: [...h.groups] };
  next[kind] = next[kind].filter((x) => x !== name);
  if (hide) next[kind].push(name);
  const keep = next.presets.length || next.groups.length;
  return savePresets(keep ? [...sharedItems(), next] : sharedItems());
}

/** A file's bytes as base64. */
function base64(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).split(',')[1] || '');
    r.onerror = () => reject(r.error || new Error('can’t read ' + file.name));
    r.readAsDataURL(file);
  });
}

async function importPresets() {
  const picked = [...$('presetFiles').files];
  if (!picked.length) return;
  const btn = $('importPresets');
  busy(btn, true);
  try {
    const files = await Promise.all(picked.map(async (f) => ({ name: f.name, data: await base64(f) })));
    const r = await api('POST', 'presets/import', { files });
    const already = r.skipped ? ' (' + plural(r.skipped, 'was', 'were') + ' here already)' : '';
    if (r.added) toast('Added ' + plural(r.added, 'preset', 'presets') + already + '.');
    else toast('No new presets in ' + plural(picked.length, 'file', 'files') + already + '.', 'info');
    const failed = r.failed || [];
    if (failed.length) toast(plural(failed.length, 'file', 'files') + ' couldn’t be read: ' + failed.map((f) => f[0] + ': ' + f[1]).join('; '), 'negative');
    await route();
  } catch (e) {
    fail(e);
  } finally {
    busy(btn, false);
  }
}

// --------------------------------------------------------------- overview

function loadOverview() {
  loadUsage();
  return swr(['status', 'users', 'limits'], showOverview);
}

// ------------------------------------------------- server load and activity

let usageTimer = null;

/** The load and activity panels, asked again every half minute while the overview is open. */
async function loadUsage() {
  clearTimeout(usageTimer);
  const seq = routeSeq;
  try {
    const u = await api('GET', 'usage');
    if (seq !== routeSeq) return;
    showUsage(u);
    usageTimer = setTimeout(loadUsage, 30000);
  } catch (e) {
    fail(e);
  }
}

/** A path of an svg (240 by 56) through `values` (`null`: no reading, skipped), 0 to `max`; the first also gets an area. */
function spark(svg, series, max) {
  const NS = 'http://www.w3.org/2000/svg';
  const [W, H] = [240, 56];
  svg.replaceChildren(
    ...series.flatMap((values, k) => {
      const at = (i) => ((i * W) / Math.max(1, values.length - 1)).toFixed(1);
      const pts = values.map((v, i) => (v === null ? null : [at(i), (H - 2 - Math.min(1, v / (max || 1)) * (H - 4)).toFixed(1)])).filter(Boolean);
      if (!pts.length) return [];
      const line = pts.map(([x, y], i) => (i ? 'L' : 'M') + x + ' ' + y).join(' ');
      const path = (cls, d) => {
        const p = document.createElementNS(NS, 'path');
        p.setAttribute('class', cls);
        p.setAttribute('d', d);
        return p;
      };
      const area = k === 0 && pts.length > 1 ? [path('area', line + ' L' + pts[pts.length - 1][0] + ' ' + H + ' L' + pts[0][0] + ' ' + H + ' Z')] : [];
      return [...area, path(k === 0 ? '' : 'other', line)];
    }),
  );
}

function showUsage(u) {
  const h = u.history;
  const last = h[h.length - 1] || {};
  const cores = u.cores || 1;
  const span = h.length > 1 ? last.at - h[0].at : 0;
  const pct = (x) => Math.round(100 * x) + '%';
  // (where the machine can't be read there is nothing to chart for these two)
  $('ldCpuBox').hidden = $('ldMemBox').hidden = !u.supported;
  $('loadNote').textContent = u.supported
    ? 'Read from this machine every 30 seconds' + (span >= 120 ? ' (the last ' + dur(Math.round(span / 60) * 60) + ' are shown; 24 hours are kept, and a restart starts them again)' : '') + '. The dashed lines are everything running on the machine, not only LightCraft.'
    : 'CPU and memory can be read where the server runs on Linux, as it does in Docker. This server runs elsewhere, so only the requests are shown.';
  // CPU: the server's, in cores, against all the cores it may use
  $('ldCpu').textContent = last.cpu == null ? '–' : pct(last.cpu / cores);
  $('ldCpuSub').textContent = last.cpu == null ? '' : n(Math.round(last.cpu * 10) / 10) + ' of ' + plural(cores, 'core', 'cores') + (last.machineCpu == null ? '' : ', the whole machine ' + pct(last.machineCpu));
  spark($('sparkCpu'), [h.map((s) => s.cpu), h.map((s) => (s.machineCpu == null ? null : s.machineCpu * cores))], cores);
  // memory: the server's, and what the machine (or container) uses of its total
  $('ldMem').textContent = last.rss == null ? '–' : bytes(last.rss);
  $('ldMemSub').textContent = last.memUsed == null ? '' : 'the machine uses ' + bytes(last.memUsed) + (u.memory ? ' of ' + bytes(u.memory) : '');
  spark($('sparkMem'), [h.map((s) => s.rss), h.map((s) => s.memUsed)], u.memory || Math.max(1, ...h.map((s) => s.memUsed || s.rss || 0)));
  // requests of devices a minute (a point is the requests since the one before)
  const rates = h.map((s, i) => (s.requests * 60) / Math.max(1, i ? s.at - h[i - 1].at : 30));
  const rate = rates.length ? rates[rates.length - 1] : 0;
  $('ldReq').textContent = rate < 10 ? (Math.round(rate * 10) / 10).toLocaleString() : n(Math.round(rate));
  const all = u.users.flatMap((x) => x.devices).reduce((a, d) => a + d.requests, 0);
  $('ldReqSub').textContent = 'a minute, ' + n(all) + ' since the server started';
  spark($('sparkReq'), [rates], Math.max(1, ...rates));
  // what each user's devices asked of the server
  $('activityNote').textContent =
    "What each user's devices asked of the server since it started " + ago(u.since) + '. Busy is the time the server spent answering, without sending the answer. Counts start again when the server restarts.';
  const body = $('activity').querySelector('tbody');
  body.replaceChildren();
  if (!u.users.length) body.append(emptyRow(6, 'users', 'No users yet.'));
  const busy = (ms) => (ms < 1000 ? n(ms) + ' ms' : dur(Math.round(ms / 1000)));
  const cells = (x, seen) => [
    el('td', { class: 'num' }, n(x.requests)),
    el('td', { class: 'num opt' }, bytes(x.in)),
    el('td', { class: 'num' }, bytes(x.out)),
    el('td', { class: 'num opt' }, busy(x.busyMs)),
    el('td', { class: 'num opt' }, seen ? ago(seen) : ''),
  ];
  for (const user of u.users) {
    const sum = (k) => user.devices.reduce((a, d) => a + d[k], 0);
    const latest = Math.max(0, ...user.devices.map((d) => d.lastSeen || 0));
    body.append(
      el('tr', { class: 'group' }, el('td', {}, el('b', {}, user.name), el('span', { class: 'sub muted' }, plural(user.devices.length, 'device', 'devices'))), ...cells({ requests: sum('requests'), in: sum('in'), out: sum('out'), busyMs: sum('busyMs') }, latest)),
    );
    for (const d of user.devices) body.append(el('tr', {}, el('td', {}, d.name || 'Device ' + d.id), ...cells(d, d.lastSeen)));
  }
}

const LIMITS = [
  ['threads', 'CPU threads', 'Cores that decoding, developing and the search models share.'],
  ['previews', 'Previews at once', 'Photos whose previews are built side by side. The rest wait their turn.'],
  ['renders', 'Renders at once', 'Full-size exports for devices. Devices ask again when all are busy.'],
  ['requests', 'Requests at once', 'Requests answered side by side. Devices retry the others.'],
];

/** The limits form: each box holds what an admin set, empty for automatic, locked when the deployment set it. */
function showLimits(l) {
  const box = $('limFields');
  if (box.contains(document.activeElement)) return;
  $('limMachine').textContent =
    'How much of this machine the server may use (' + plural(l.cores, 'core', 'cores') + (l.memory ? ', ' + bytes(l.memory) + ' of memory' : '') + '). ' +
    'Leave a box empty for the automatic number. Nothing is dropped: work over a limit waits.';
  box.replaceChildren(
    ...LIMITS.map(([key, label, help]) => {
      const r = l[key];
      const { input, wrap } = field({ name: 'lim-' + key, label, type: 'number', required: false, placeholder: 'Automatic (' + r.automatic + ')', help: r.fixed ? 'Set by ' + r.fixed + ' when the server starts.' : help });
      input.min = 1;
      input.max = r.max;
      input.dataset.key = key;
      input.inputMode = 'numeric';
      if (r.fixed) {
        input.value = r.value;
        input.disabled = true;
      } else if (r.saved !== null) input.value = r.saved;
      return wrap;
    }),
  );
  const pending = l.threads.value !== l.threadsNow;
  $('limRestart').hidden = !pending;
  if (pending) $('limRestart').querySelector('p').textContent = 'The server keeps using ' + n(l.threadsNow) + ' until it restarts, then ' + n(l.threads.value) + '.';
}

async function saveLimits(e) {
  e.preventDefault();
  const btn = e.target.querySelector('button[type=submit]');
  const body = {};
  for (const input of $('limFields').querySelectorAll('input:not(:disabled)')) {
    const v = input.value.trim();
    if (v && !(/^\d+$/.test(v) && +v >= 1 && +v <= +input.max)) {
      fail(new Error(input.labels[0].textContent + ': a number from 1 to ' + input.max + ', or empty for automatic.'));
      input.focus();
      return;
    }
    body[input.dataset.key] = v ? +v : null;
  }
  busy(btn, true);
  try {
    const l = await api('PUT', 'limits', body);
    document.activeElement.blur();
    showLimits(l);
    toast('Limits saved.');
  } catch (x) {
    fail(x);
  } finally {
    busy(btn, false);
  }
}

function showOverview([s, users, limits]) {
  recount(users);
  setMe(s.me);
  $('stUsers').textContent = n(users.length);
  $('stPhotos').textContent = n(users.reduce((a, u) => a + u.photos, 0));
  $('stVideos').textContent = n(users.reduce((a, u) => a + (u.videos || 0), 0));
  $('stDevices').textContent = n(users.reduce((a, u) => a + u.devices, 0));
  $('stFree').textContent = s.disk ? bytes(s.disk.free) : '—';
  $('stDisk').hidden = !s.disk;
  if (s.disk) fill($('stDisk'), s.disk.total ? 1 - s.disk.free / s.disk.total : 0);
  const facts = $('facts');
  facts.replaceChildren();
  const fact = (k, v, code) => facts.append(el('dt', {}, k), el('dd', { class: code ? 'code' : null }, v));
  const build = (b) => (b.version || '?') + ' (' + (b.commit || 'unknown') + ')';
  fact('Server version', build(s), true);
  fact('Data folder', s.data, true);
  fact('Listening on', s.listen, true);
  fact('Web app', s.web ? 'Served from ' + s.web : 'Not served (start the server with --web)');
  if (s.web) fact('Web app version', s.webBuild ? build(s.webBuild) : 'Unknown (built without version.json)', !!s.webBuild);
  $('webHint').hidden = !s.web;
  showLimits(limits);
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

function loadUsers() {
  return swr(['users'], showUsers);
}

function showUsers([users]) {
  recount(users);
  const body = $('users').querySelector('tbody');
  body.replaceChildren();
  if (!users.length) body.append(emptyRow(7, 'users', 'No users yet. Add the people who use this server.'));
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
        el('td', { class: 'num opt', title: videoParts(u) }, n(u.videos)),
        el('td', { class: 'num opt' }, n(u.albums)),
        el('td', { class: 'num opt' }, n(u.devices)),
        el('td', { class: 'num', title: storageParts(u) }, size(u.bytes)),
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

function loadUser(name, tab) {
  paintAvatar($('uAvatar'), name, 'xl');
  $('uName').textContent = name;
  if (name !== shownUser) {
    // (not the last user's numbers, devices or folders)
    shownUser = name;
    $('uRole').replaceChildren();
    for (const id of ['uPhotos', 'uAlbums', 'uDevices', 'uBytes']) $(id).textContent = '–';
    $('uBytesNote').textContent = '';
    for (const t of ['devices', 'shares', 'folders']) $(t).querySelector('tbody').replaceChildren();
    for (const c of document.querySelectorAll('.scan')) c.hidden = true;
    for (const c of document.querySelectorAll('.tabs .count')) c.textContent = '';
  }
  for (const a of document.querySelectorAll('.tabs a')) {
    a.href = '#/users/' + enc(name) + '/' + a.dataset.tab;
    a.setAttribute('aria-selected', String(a.dataset.tab === tab));
  }
  for (const p of document.querySelectorAll('[data-panel]')) p.hidden = p.dataset.panel !== tab;
  $('addFolder').onclick = () => addFolder(name);
  $('scanNow').onclick = $('scanChanged').onclick = () => startScan(name, 'scan');
  $('scanEverything').onclick = () => rescanEverything(name);
  if (tab === 'folders') foldersOf = name;
  // (the user and the tab's list are asked for together)
  const seq = routeSeq;
  const list = (tab === 'folders' ? loadFolders : tab === 'shares' ? loadShares : loadDevices)(name);
  const known = swr(['users'], ([users]) => showUser(name, users)).then(([users]) => {
    if (seq === routeSeq && !users.some((x) => x.name === name)) {
      toast('There is no user ' + name + '.', 'negative');
      location.hash = '#/users';
    }
  });
  // (no such user: the users list says so, once)
  return Promise.all([known, list.catch((e) => { if (!/^No user /.test(e.message)) throw e; })]);
}

function showUser(name, users) {
  recount(users);
  const u = users.find((x) => x.name === name);
  if (!u) return;
  const role = el('div', { class: 'btn-group' }, u.admin ? badge('Administrator', 'accent') : badge('User'));
  if (name === me) role.append(badge('You'));
  $('uRole').replaceChildren(role);
  $('uPhotos').textContent = n(u.photos);
  $('uVideos').textContent = n(u.videos);
  $('uVideosNote').textContent = videoParts(u);
  $('uAlbums').textContent = n(u.albums);
  $('uDevices').textContent = n(u.devices);
  $('uBytes').textContent = size(u.bytes);
  $('uBytesNote').textContent = storageParts(u);
  // (the limit counts what LightCraft keeps for them, not their library folders)
  const limit = $('uQuotaText');
  const meter = $('uQuotaMeter');
  limit.hidden = meter.hidden = !u.quota;
  if (u.quota) {
    const kept = Math.max(0, (u.bytes || 0) - (u.folderBytes || 0));
    limit.textContent = size(u.bytes === null ? null : kept) + ' of ' + bytes(u.quota) + ' allowed';
    fill(meter, kept / u.quota);
  }

  const only = u.admin && users.filter((x) => x.admin).length < 2;
  const why = only ? name + ' is the only administrator.' : '';
  $('uAdmin').querySelector('span').textContent = u.admin ? 'Remove admin' : 'Make admin';
  for (const b of [$('uAdmin'), $('uRemove')]) {
    b.disabled = only;
    b.title = why;
  }
  $('uPassword').onclick = () => resetPassword(name);
  $('uQuota').onclick = () => setQuota(name, u.quota);
  $('uAdmin').onclick = () => setAdmin(name, !u.admin);
  $('uRemove').onclick = () => removeUser(name);

  const counts = { devices: u.devices, folders: (u.folders || []).length, shares: u.shares || 0 };
  for (const a of document.querySelectorAll('.tabs a')) {
    let c = a.querySelector('.count');
    if (!c) a.append((c = el('span', { class: 'count' })));
    c.textContent = n(counts[a.dataset.tab]);
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

/** The most the server keeps for a user (uploaded originals and previews; library folders don't count). */
function setQuota(name, quota) {
  return ask({
    title: 'Storage limit for ' + name,
    text:
      'The most the server keeps for ' + name + ': the originals their devices upload, previews, and photos added through their shared links. ' +
      'Photos in library folders don\'t count. Over the limit, uploads wait until there is room; nothing is deleted. Leave it empty for no limit.',
    fields: [{ name: 'gb', label: 'Limit in GB', type: 'number', step: 'any', required: false, value: quota ? String(Math.round(quota / 1e8) / 10) : '', placeholder: 'No limit', help: 'At least 0.1 GB.' }],
    ok: 'Set limit',
    action: async (v) => {
      const gb = v.gb.trim() === '' ? null : Number(v.gb);
      if (gb !== null && !(gb >= 0.1)) throw new Error('Type a size of at least 0.1 GB, or leave it empty for no limit.');
      await api('PUT', 'users/' + enc(name) + '/quota', { quota: gb === null ? null : Math.round(gb * 1e9) });
      toast(gb === null ? name + ' has no storage limit.' : name + ' may keep ' + bytes(gb * 1e9) + ' on the server.');
      await route();
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

function loadDevices(name) {
  return swr(['users/' + enc(name) + '/devices'], ([list]) => showDevices(name, list));
}

function showDevices(name, list) {
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

// ----------------------------------------------------------- shared links

function loadShares(name) {
  return swr(['users/' + enc(name) + '/shares'], ([list]) => showShares(name, list));
}

function showShares(name, list) {
  const body = $('shares').querySelector('tbody');
  body.replaceChildren();
  if (!list.length) body.append(emptyRow(4, 'copy', name + ' has no shared links.'));
  const now = Date.now() / 1000;
  for (const s of list) {
    const ends = !s.expires
      ? el('span', { class: 'muted' }, 'Never')
      : s.expires < now
        ? el('span', { class: 'muted' }, 'Ended')
        : el('span', { title: date(s.expires) }, 'in ' + dur(s.expires - now));
    body.append(
      el(
        'tr',
        {},
        el('td', {}, el('div', { class: 'who-cell' }, el('span', { class: 'thing' }, icon('copy')), el('div', {}, el('b', {}, s.name), el('span', { class: 'sub' }, [s.originals ? 'Originals can be downloaded' : 'Pictures only', s.password && 'Password', s.upload && 'Visitors add photos'].filter(Boolean).join(' · '))))),
        el('td', { class: 'opt', title: date(s.created) }, ago(s.created)),
        el('td', {}, ends),
        el('td', { class: 'end' }, el('button', { type: 'button', class: 'btn quiet', onclick: () => revokeShare(name, s) }, icon('trash'), 'Revoke')),
      ),
    );
  }
}

function revokeShare(name, s) {
  return ask({
    title: 'Revoke the link to ' + s.name + '?',
    text: 'Anyone who has it stops seeing the album at once. ' + name + ' can make a new link.',
    ok: 'Revoke',
    danger: true,
    action: async () => {
      await api('DELETE', 'users/' + enc(name) + '/shares/' + encodeURIComponent(s.id));
      toast('The link to ' + s.name + ' was revoked.');
      await route();
    },
  });
}

// -------------------------------------------------------- library folders

function loadFolders(name) {
  return Promise.all([swr(['users/' + enc(name) + '/folders'], ([r]) => showFolders(name, r)), loadCheck(name)]);
}

/** The folders again, while a scan or the previews are going on. */
async function pollFolders(name) {
  const r = await get('users/' + enc(name) + '/folders');
  if (foldersOf === name) showFolders(name, r);
}

function scanInfo(s) {
  if (s.scanning) {
    // (a rescan reads every file again, not only the new or changed ones)
    const title = s.full ? 'Rescanning everything…' : 'Scanning…';
    if (s.phase === 'reading' && s.todo) {
      const what = s.full ? ' files again' : ' new or changed files';
      return { title, text: 'Reading ' + n(s.done) + ' of ' + n(s.todo) + what + (s.etaSecs ? ', about ' + dur(s.etaSecs) + ' left.' : '.'), progress: s.done / s.todo };
    }
    if (s.phase === 'listing') return { title, text: 'Looking for photos: ' + n(s.files) + ' found so far.', progress: null };
    return { title, text: 'Finishing up.', progress: null };
  }
  if (!s.lastScan) return { title: 'Not scanned yet', text: 'Scan to add the photos in these folders to the library.' };
  const changes = [[s.added, 'new'], [s.moved, 'moved'], [s.changed, 'changed'], [s.linked, 'uploaded before'], [s.missing, 'missing'], [s.failed, 'not read']]
    .filter(([v]) => v > 0)
    .map(([v, w]) => n(v) + ' ' + w);
  return { title: 'Last scanned ' + ago(s.lastScan), text: plural(s.files, 'photo file', 'photo files') + '. ' + (changes.length ? 'Changes: ' + changes.join(', ') + '.' : 'No changes.') };
}

/** The previews panel: built / left in this batch, and the photo files being built right now (also during a scan). */
function showPreviews(name, s) {
  const paused = !!s.previewsPaused;
  $('previewsTitle').textContent = paused ? 'Previews paused' : s.previews ? (s.previewsRebuilding ? 'Rebuilding previews' : 'Building previews') : 'Previews';
  const have = s.previewsHave, photos = s.previewsPhotos;
  $('previewsCover').textContent = photos == null ? 'Counting previews…' : n(have) + ' of ' + plural(photos, 'photo has', 'photos have') + ' previews' + (photos ? ' (' + Math.floor((100 * have) / photos) + '%)' : '');
  const total = s.previewsTotal || s.previews;
  const batch = s.previews
    ? n(s.previewsDone) + ' built, ' + n(s.previews) + ' left' + (s.previewsEtaSecs && !paused ? ', about ' + dur(s.previewsEtaSecs) + ' to go' : '') + (s.previewsPerMin && !paused ? ' (' + n(s.previewsPerMin) + ' a minute)' : '') + '.'
    : 'Nothing waiting: 0 left.' + (s.previewsLastAt ? ' The last batch, ' + plural(s.previewsLastDone, 'preview', 'previews') + ', finished ' + ago(s.previewsLastAt) + '.' : '');
  $('previewsText').textContent = batch + (paused ? ' Paused for every user: the ones being built finish, then it waits.' : '');
  // (while every preview is built again, those there count for nothing: the bar shows how far the batch is)
  progress($('previewsBar'), s.previewsRebuilding && total ? s.previewsDone / total : photos ? have / photos : s.previews && total ? s.previewsDone / total : 0);
  const now = s.previewsBuilding || [];
  $('previewsNow').replaceChildren(
    ...now.map((p) => el('li', {}, el('b', {}, 'Now'), el('code', { title: p }, p.slice(p.lastIndexOf('/') + 1)))),
    ...(s.previews && !now.length && !paused ? [el('li', { class: 'muted' }, 'Waiting for a builder.')] : []),
  );
  const pause = $('previewsPause');
  pause.hidden = !paused && !s.previews;
  pause.querySelector('span').textContent = paused ? 'Resume' : 'Pause';
  pause.querySelector('use').setAttribute('href', paused ? '#i-play' : '#i-pause');
  pause.onclick = () => previewsAction(name, paused ? 'resume' : 'pause');
  $('previewsAll').onclick = $('previewsMissing').onclick = () => previewsAction(name, 'generate');
  $('previewsRebuild').onclick = () => rebuildPreviews(name);
}

async function previewsAction(name, action) {
  try {
    const r = await api('POST', 'users/' + enc(name) + '/previews/' + action);
    if (action === 'generate') toast('Queued every missing preview.');
    if (action === 'rebuild') toast('Rebuilding every preview.');
    showFolders(name, r);
  } catch (e) {
    fail(e);
  }
}

/** Every preview again, the ones there too: slow, so it asks first. */
function rebuildPreviews(name) {
  return ask({
    title: 'Rebuild all previews?',
    text:
      'The server builds the previews of every photo in ' + name + "'s library folders again, the ones that exist too, and replaces them as each new one is ready. " +
      'For a big library that takes hours and keeps the processors busy. Use it after an update changed how photos are decoded or look. ' +
      'To make only the previews that are missing, choose Build missing previews.',
    ok: 'Rebuild all previews',
    action: async () => {
      await previewsAction(name, 'rebuild');
    },
  });
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
        el(
          'td',
          {},
          f.there ? el('span', { class: 'status positive' }, 'Available') : el('span', { class: 'status negative', title: 'The server cannot see this folder: is the disk mounted?' }, 'Not found'),
          ' ',
          f.writeError
            ? el('span', { class: 'status negative', title: f.writeError }, "Can't write")
            : f.imports
              ? el('span', { class: 'status active', title: 'Writes XMP sidecars here and files the photos devices upload into it' }, 'Takes uploads')
              : f.writable
                ? el('span', { class: 'status active', title: 'Writes XMP sidecars here; never changes the photos' }, 'Writable')
                : el('span', { class: 'muted' }, 'Read-only'),
        ),
        el(
          'td',
          { class: 'end' },
          el('button', { type: 'button', class: 'btn quiet', onclick: () => folderMode(name, f) }, icon('key'), 'Writing'),
          el('button', { type: 'button', class: 'btn quiet', onclick: () => removeFolder(name, f) }, icon('trash'), 'Remove'),
        ),
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
  $('previewsPanel').hidden = !r.folders.length;
  showPreviews(name, s);
  const info = scanInfo(s);
  $('scanNow').closest('.scan').hidden = !r.folders.length;
  $('scanTitle').textContent = info.title;
  $('scanText').textContent = info.text;
  $('scanNow').disabled = $('scanMore').disabled = !!s.scanning;
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
  const working = !!(s.scanning || s.previews || s.previewsPaused || s.previewsPhotos == null);
  if (working) timer = setTimeout(() => foldersOf === name && pollFolders(name).catch(fail), 2000);
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
      { name: 'writable', label: 'Let the server write XMP sidecars here', type: 'switch', help: 'Off: the folder is only read (and may be a read-only mount). On: edits made on devices are written beside the photos as .xmp files, for Lightroom and other programs. The photos themselves are never changed.' },
      { name: 'imports', label: 'File uploaded photos here', type: 'switch', help: 'Photos devices upload are filed in this folder as year/date/name instead of kept in the server\'s own store. One folder per user; needs writing.' },
    ],
    ok: 'Add folder',
    action: async (v) => {
      await api('POST', 'users/' + enc(name) + '/folders', { path: v.path, name: v.name || null, writable: !!v.writable, imports: !!v.imports });
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

function folderMode(name, f) {
  return ask({
    title: 'What the server may do in ' + f.name,
    text: 'The server never changes, moves or deletes the photos. With writing on, it adds and updates .xmp sidecar files beside them' + (f.writeError ? ' — but it cannot write there now: ' + f.writeError : '') + '.',
    fields: [
      { name: 'writable', label: 'Write XMP sidecars here', type: 'switch', value: !!f.writable, help: 'Mount the folder read-write, with the server user allowed to write, before turning this on.' },
      { name: 'imports', label: 'File uploaded photos here', type: 'switch', value: !!f.imports, help: 'One folder per user; turns writing on.' },
    ],
    ok: 'Save',
    action: async (v) => {
      await api('PUT', 'users/' + enc(name) + '/folders/' + enc(f.name), { writable: !!v.writable || !!v.imports, imports: !!v.imports });
      toast('Saved.');
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

/** Start a scan: `scan` reads the files that are new or changed, `rescan` every file. */
async function startScan(name, path) {
  try {
    showFolders(name, await api('POST', 'users/' + enc(name) + '/' + path));
    $('scanTitle').textContent = path === 'rescan' ? 'Rescanning everything…' : 'Scanning…';
    $('scanNow').disabled = $('scanMore').disabled = true;
    clearTimeout(timer);
    timer = setTimeout(() => foldersOf === name && pollFolders(name).catch(fail), 1000);
  } catch (e) {
    fail(e);
  }
}

// -------------------------------------------------------- file check

/** The last check of a user's files for ones cut short, and how far a running one is. */
async function loadCheck(name) {
  const r = await get('users/' + enc(name) + '/check');
  if (foldersOf === name) showCheck(name, r);
}

async function startCheck(name) {
  try {
    showCheck(name, await api('POST', 'users/' + enc(name) + '/check'));
  } catch (e) {
    fail(e);
  }
}

function showCheck(name, r) {
  const rep = r.report;
  const bad = rep ? rep.cut.concat(rep.unreadable) : [];
  $('checkPanel').hidden = false;
  $('checkRun').disabled = r.running;
  $('checkRun').onclick = () => startCheck(name);
  $('checkTitle').textContent = r.running ? 'Checking files…' : rep ? 'Files checked ' + ago(rep.finished) : 'Check files';
  $('checkText').textContent = r.running
    ? 'Checked ' + n(r.done) + ' of ' + plural(r.total, 'file', 'files') + '.'
    : rep
      ? plural(rep.files, 'file', 'files') + ' checked: ' + (bad.length ? n(rep.cut.length) + ' cut short, ' + n(rep.unreadable.length) + " couldn't be read." : 'none is damaged.')
      : 'Not checked yet.';
  const bar = $('checkBar');
  bar.hidden = !r.running;
  if (r.running) progress(bar, r.total ? r.done / r.total : 0);
  $('checkFound').hidden = r.running || !bad.length;
  $('checkFoundTitle').textContent = plural(bad.length, 'damaged file', 'damaged files');
  // (a photo is named only when its name isn't the file's own)
  const named = (d) => d.photos.filter((p) => !d.path.endsWith('/' + p));
  const why = (d) => d.why + (d.missing ? ' (' + bytes(d.missing) + ' missing)' : '') + (named(d).length ? ': ' + named(d).join(', ') : '');
  const shown = bad.slice(0, 200);
  $('checkList').replaceChildren(
    ...shown.map((d) => el('li', {}, el('code', {}, d.path), ' ', el('span', { class: 'muted' }, why(d)))),
    ...(bad.length > shown.length ? [el('li', { class: 'muted' }, n(bad.length - shown.length) + ' more in the list.')] : []),
  );
  $('checkSave').onclick = () => saveList(name + '-damaged-files.csv', bad);
  clearTimeout(checkTimer);
  if (r.running) checkTimer = setTimeout(() => foldersOf === name && loadCheck(name).catch(fail), 1000);
}

/** The damaged files as a CSV the browser saves. */
function saveList(file, rows) {
  const q = (v) => '"' + String(v == null ? '' : v).replace(/"/g, '""') + '"';
  const lines = rows.map((d) => [d.path, d.size, d.missing, d.why, d.photos.join('; ')].map(q).join(','));
  const url = URL.createObjectURL(new Blob(['path,size,missing_bytes,problem,photos\n' + lines.join('\n') + '\n'], { type: 'text/csv' }));
  el('a', { href: url, download: file }).click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/** Read every photo file again: slow, so it asks first. */
function rescanEverything(name) {
  return ask({
    title: 'Rescan everything?',
    text:
      'The server reads every photo file in ' + name + "'s library folders again, the ones that haven't changed too, and checks each against its photo. " +
      'It also tries again the files that could not be read. For a big library that takes hours and keeps the disks busy. Photos and their edits stay as they are unless a file now holds something else. ' +
      "To read only what is new or changed, choose Scan for changes.",
    ok: 'Rescan everything',
    action: async () => {
      await startScan(name, 'rescan');
    },
  });
}

/**
 * A split button (`.split`): the main button does the usual thing, the arrow opens a menu of its variants, each with
 * what it does. The menu closes on a choice, a click elsewhere or Esc; the arrow keys move between its items.
 */
function splitMenu(id) {
  const root = $(id);
  const more = root.querySelector('.split-more');
  const menu = root.querySelector('.menu');
  const items = () => [...menu.querySelectorAll('[role=menuitem]')];
  const open = (on) => {
    menu.hidden = !on;
    more.setAttribute('aria-expanded', String(on));
    if (on) items()[0]?.focus();
  };
  more.addEventListener('click', () => open(menu.hidden));
  menu.addEventListener('click', () => open(false));
  document.addEventListener('click', (e) => {
    if (!menu.hidden && !root.contains(e.target)) open(false);
  });
  root.addEventListener('keydown', (e) => {
    if (menu.hidden) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      open(false);
      more.focus();
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const all = items();
      const at = all.indexOf(document.activeElement);
      all[(at + (e.key === 'ArrowDown' ? 1 : all.length - 1)) % all.length]?.focus();
    }
  });
}

// ---------------------------------------------------------------- storage

function loadStorage() {
  return swr(['status', 'users'], showStorage);
}

function showStorage([s, users]) {
  recount(users);
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
  const total = (key) => users.reduce((a, u) => a + (u[key] || 0), 0);
  $('libraryLine').textContent = users.length
    ? [plural(total('photos'), 'photo', 'photos'), plural(total('videos'), 'video', 'videos')].join(' · ') +
      (total('videos') ? ' · ' + videoParts({ videos: 1, videoBytes: total('videoBytes'), videoCopies: total('videoCopies'), videoCopyBytes: total('videoCopyBytes'), videoSaved: total('videoSaved') }) : '')
    : '';
  const list = $('usage');
  const sorted = users.slice().sort((a, b) => (b.bytes || 0) - (a.bytes || 0));
  const top = sorted.length ? sorted[0].bytes : 0;
  list.replaceChildren();
  if (!sorted.length) list.append(el('li', { class: 'muted' }, 'No users yet.'));
  for (const u of sorted) {
    const meter = el('div', { class: 'meter' }, el('div', {}));
    list.append(el('li', { title: [storageParts(u), videoParts(u)].filter(Boolean).join(' · ') }, avatar(u.name), el('a', { class: 'name', href: '#/users/' + enc(u.name) }, u.name), meter, el('span', { class: 'bytes' }, size(u.bytes))));
    meter.firstElementChild.style.width = (top ? (100 * (u.bytes || 0)) / top : 0).toFixed(1) + '%';
  }
}

// ----------------------------------------------------------------- search

/** What each switch says about itself: its words, and where it keeps its state in `/api/admin/search`. */
const FEATURES = {
  sr: {
    path: 'search',
    pick: (s) => s,
    users: 'indexed',
    what: 'search model',
    on: 'On: every device signed in to this server searches here.',
    off: 'Off. The model stays on this server, so turning this on again needs no download.',
    needs: (f) => 'Needs the search model: ' + bytes(f.sizeBytes) + ', downloaded once onto this server.',
    getting: 'Downloading the search model onto this server…',
    label: 'Photos are described once the model is ready.',
    idle: 'Nothing is described while search is off.',
    title: 'Download the search model?',
    ask: (f) =>
      'Search by description uses an AI model that understands photos and words. It is not part of LightCraft: it is SigLIP 2 from Google, a ' +
      bytes(f.sizeBytes) + ' download onto this server under the ' + f.license + '. Downloading it means accepting that licence: ' + f.licenseUrl +
      " Afterwards the server describes every user's photos in the background, and their devices search through it.",
    done: 'Search is on for everyone on this server.',
  },
  tx: {
    path: 'search/text',
    pick: (s) => s.text,
    users: 'read',
    what: 'text-reading models',
    on: "On: the server reads the words in every user's photos in the background, and searches find photos by them.",
    off: 'Off. What was read is kept, so turning this on again picks up where it left off.',
    needs: (f) => 'Needs the text-reading models: ' + bytes(f.sizeBytes) + ', downloaded once onto this server.',
    getting: 'Downloading the text-reading models onto this server…',
    label: 'Photos are read once the models are ready.',
    idle: 'Nothing is read while this is off.',
    title: 'Download the text-reading models?',
    ask: (f) =>
      'Finding words in photos uses two small AI models that read text, in English, Chinese, Japanese and many other languages. They are not part of LightCraft: they are PP-OCRv6 from Baidu, a ' +
      bytes(f.sizeBytes) + ' download onto this server under the ' + f.license + '. Downloading them means accepting that licence: ' + f.licenseUrl +
      " Afterwards the server reads every user's photos in the background, about a second each.",
    done: 'Finding photos by their words is on for everyone on this server.',
    models: 'PP-OCRv6',
  },
  sn: {
    path: 'search/sensitive',
    pick: (s) => s.sensitive,
    users: 'checked',
    what: 'sensitive-content model',
    on: "On: the server checks every user's photos in the background, and their devices list and blur the ones that might be sensitive. Nothing is hidden by it.",
    off: 'Off. What was checked is kept, so turning this on again picks up where it left off.',
    needs: (f) => 'Needs the sensitive-content model: ' + bytes(f.sizeBytes) + ', downloaded once onto this server.',
    getting: 'Downloading the sensitive-content model onto this server…',
    label: 'Photos are checked once the model is ready.',
    idle: 'Nothing is checked while this is off.',
    title: 'Download the sensitive-content model?',
    ask: (f) =>
      'Flagging photos that might be sensitive (nudity and the like) uses an AI model that looks at each photo. It is not part of LightCraft: it is nsfw_image_detection from Falcons.ai, a ' +
      bytes(f.sizeBytes) + ' download onto this server under the ' + f.license + '. Downloading it means accepting that licence: ' + f.licenseUrl +
      " Afterwards the server checks every user's photos in the background. It can be wrong both ways: it only suggests, and each person decides what to hide.",
    done: 'Flagging photos that might be sensitive is on for everyone on this server.',
    models: 'nsfw_image_detection',
  },
};

function loadSearch() {
  return swr(['search'], showSearch);
}

/** The progress of a download in the fetch box with prefix `p`, or the box hidden. */
function showFetch(p, d) {
  $(p + 'Fetch').hidden = !d.running;
  if (d.running) {
    const f = d.total ? d.done / d.total : 0;
    $(p + 'FetchText').textContent = bytes(d.done) + ' of ' + bytes(d.total) + (d.file ? ' · ' + d.file : '');
    $(p + 'FetchPct').textContent = Math.round(100 * f) + '%';
    progress($(p + 'FetchBar').parentElement, f);
  }
}

/** A download the admin cancelled is not an error. */
function showFetchError(p, d, installed) {
  const failed = d.error && !d.running && !installed && !/cancel/i.test(d.error);
  $(p + 'Error').hidden = !failed;
  if (failed) $(p + 'Error').querySelector('p').textContent = d.error + ' Start it again to resume where it stopped.';
}

function fact(list, k, v, code) {
  list.append(el('dt', {}, k), el('dd', { class: code ? 'code' : null }, v));
}

const PART_KEY = { sr: 'search', tx: 'text', fc: 'faces', sn: 'sensitive' };
const hhmm = (m) => String(Math.floor(m / 60)).padStart(2, '0') + ':' + String(m % 60).padStart(2, '0');
// (the server keeps the window in UTC; the admin sees and sets their own time)
const toLocalMin = (utc) => (((utc - new Date().getTimezoneOffset()) % 1440) + 1440) % 1440;
const toUtcMin = (local) => (((local + new Date().getTimezoneOffset()) % 1440) + 1440) % 1440;
const parseMin = (v) => (/^\d\d:\d\d$/.test(v) ? +v.slice(0, 2) * 60 + +v.slice(3) : null);

const pace = (perMinute) => (perMinute >= 10 ? n(Math.round(perMinute)) + ' a minute' : n(Math.round(perMinute * 60)) + ' an hour');
const etaOf = (photos, perMinute) => 'About ' + dur(Math.max(1, Math.round((photos / perMinute) * 60))) + ' left';

/** Whether the worker is at one kind of index, for the line with the dot: what it is on, and how long ago it last finished a photo. */
function liveState(p, c) {
  if (!c.left || c.paused || !c.runsNow) return null;
  const what = { sr: 'Describing ', tx: 'Reading the words in ', fc: 'Looking for faces in ', sn: 'Checking ' }[p] + (c.now ? c.now.user + "'s photos" : 'photos');
  const last = c.lastSecs;
  const lastly = last === null ? '' : ' · last one ' + (last < 5 ? 'just now' : dur(last) + ' ago');
  // (each turn finishes a photo every few seconds: ten minutes on one without any means something hangs)
  if (c.now && c.now.secs > 600 && (last === null || last > 600)) {
    return { kind: 'stuck', text: what + ' for ' + dur(c.now.secs) + ' without finishing one. It may be stuck: restarting the server starts it again.' };
  }
  if (c.now) return { kind: 'on', text: what + lastly + (c.first ? ' · put first' : '') };
  if (c.first) return { kind: 'on', text: 'Put first: starting now' };
  // (devices that do a part for their users: the worker here leaves those photos to them)
  if (c.deviceSecs !== null && c.deviceSecs !== undefined) {
    const seen = c.deviceSecs < 5 ? 'just now' : dur(c.deviceSecs) + ' ago';
    return c.deviceSecs < 600 ? { kind: 'on', text: 'A device is processing photos for this · last asked for more ' + seen + lastly } : { kind: 'idle', text: 'Devices do this for their users, and none has asked for more since ' + seen + '. Open the app.' };
  }
  // (the three take turns, a few minutes each on a slow server)
  if (c.perMinute) return { kind: 'on', text: 'Working, in turn with the others' + lastly };
  if (!c.ready) return { kind: 'idle', text: 'Nothing to do until the photos left have their previews.' };
  if (last !== null && last > 1800) return { kind: 'stuck', text: 'Nothing done for ' + dur(last) + '.' };
  return { kind: 'idle', text: 'Waiting for its turn' + lastly + '.' };
}

/** Time left, pause and the daily window of one kind of index, in the box `<p>Sched`. */
function showSched(p, c) {
  const box = $(p + 'Sched');
  if (!box.firstChild) {
    const live = el('p', { class: 'live', role: 'status' }, el('span', { class: 'dot', 'aria-hidden': 'true' }), el('span', {}));
    const eta = el('p', { class: 'eta' });
    const pause = el('button', { type: 'button', class: 'btn secondary' });
    const mode = el('select', { 'aria-label': 'When to make the index' }, el('option', { value: 'any' }, 'Any time'), el('option', { value: 'window' }, 'Only between'));
    const from = el('input', { type: 'time', 'aria-label': 'From' });
    const to = el('input', { type: 'time', 'aria-label': 'Until' });
    const send = async (paused) => {
      const win = mode.value === 'window' ? [parseMin(from.value), parseMin(to.value)] : null;
      if (win && (win[0] === null || win[1] === null)) return;
      try {
        await api('PUT', 'search/schedule/' + PART_KEY[p], { paused, from: win && toUtcMin(win[0]), to: win && toUtcMin(win[1]) });
      } catch (x) {
        fail(x);
      }
      await route();
    };
    const first = el('button', { type: 'button', class: 'btn quiet', title: 'Skip the wait: this runs next, before the others take their turn.' }, 'Start now');
    first.addEventListener('click', async () => {
      busy(first, true);
      try {
        const s = await api('POST', 'search/first/' + PART_KEY[p]);
        // (the worker leaves a user's photos to their devices while they do this part: starting now doesn't touch those)
        const held = (s.users || []).filter((u) => {
          const done = u[FEATURES[p].users];
          return u.work && u.work[PART_KEY[p]].device && done !== null && done !== undefined && done < u.total;
        });
        if (held.length) {
          toast(held.map((u) => u.name).join(', ') + "'s devices are working on this, so the server leaves their photos to them. Press Continue here under their name to do them on the server.", 'info');
        }
      } catch (x) {
        fail(x);
      }
      busy(first, false);
      await route();
    });
    pause.addEventListener('click', () => send(pause.dataset.paused !== '1'));
    for (const i of [mode, from, to]) i.addEventListener('change', () => { from.hidden = to.hidden = mode.value !== 'window'; send(pause.dataset.paused === '1'); });
    box.append(live, eta, el('div', { class: 'row' }, pause, mode, from, to, first));
  }
  const [live, eta, row] = box.children;
  const state = liveState(p, c);
  live.hidden = !state;
  if (state) {
    live.className = 'live ' + state.kind;
    live.lastChild.textContent = state.text;
  }
  const [pause, mode, from, to, first] = row.children;
  first.hidden = !c.left || c.paused || !c.runsNow || !!c.now || c.first;
  pause.dataset.paused = c.paused ? '1' : '0';
  pause.textContent = c.paused ? 'Resume' : 'Pause';
  if (!row.contains(document.activeElement) || document.activeElement === pause) {
    mode.value = c.from === null ? 'any' : 'window';
    from.hidden = to.hidden = c.from === null;
    if (c.from !== null) { from.value = hhmm(toLocalMin(c.from)); to.value = hhmm(toLocalMin(c.to)); }
  }
  const left = c.left === 1 ? '1 photo' : n(c.left) + ' photos';
  eta.textContent = !c.left
    ? 'Up to date.'
    : c.paused
      ? 'Paused. ' + left + ' waiting.'
      : !c.runsNow
        ? left + ' waiting for ' + hhmm(toLocalMin(c.from)) + '–' + hhmm(toLocalMin(c.to)) + ' (your time).'
        : c.perMinute && c.ready
          ? etaOf(c.ready, c.perMinute) + ' (' + left + ', ' + pace(c.perMinute) + ').'
          : left + ' to go.';
}

/** Under a user's bar: their time left at the pace of the last ten minutes, what waits for previews,
 * what couldn't be done, and the last problem. */
function userLines(p, w, done, total, c, user) {
  if (!w) return [];
  const bits = [];
  const left = Math.max(0, total - done);
  const ready = Math.max(0, left - w.waiting - w.failed);
  const going = !c.paused && c.runsNow;
  if (w.device) bits.push('A device is doing this' + (w.device.seenSecs === null ? '' : ', last asked for more ' + (w.device.seenSecs < 5 ? 'just now' : dur(w.device.seenSecs) + ' ago')));
  if (ready && going && w.perMinute) bits.push(etaOf(ready, w.perMinute) + ' (' + pace(w.perMinute) + ')');
  else if (ready && !w.problem && liveState(p, c)?.kind === 'on') bits.push('Working out the time left…');
  if (w.waiting) bits.push(n(w.waiting) + ' waiting for previews');
  if (w.failed) bits.push(n(w.failed) + " couldn't be " + { sr: 'described', tx: 'read', fc: 'looked at', sn: 'checked' }[p]);
  const out = bits.length ? [el('p', { class: 'sub', title: w.failed ? 'Photos that could not be done are tried again when the server restarts.' : null }, bits.join(' · '))] : [];
  if (w.device && left) out.push(el('button', { type: 'button', class: 'btn quiet', title: 'Stop waiting for their devices: this server does the rest.', onclick: (e) => continueHere(e.currentTarget, p, user) }, 'Continue here'));
  if (w.problem && left) out.push(el('p', { class: 'sub problem' }, w.problem.charAt(0).toUpperCase() + w.problem.slice(1)));
  return out;
}

function showFeature(p, s) {
  const F = FEATURES[p];
  const f = F.pick(s);
  const d = f.download;
  $(p + 'Switch').checked = f.enabled && (f.installed || d.running);
  $(p + 'Switch').disabled = false;
  $(p + 'Help').textContent = d.running ? F.getting : f.active ? F.on : f.installed ? F.off : F.needs(f);
  showFetch(p, d);
  showFetchError(p, d, f.installed);
  showSched(p, f.schedule);
  const facts = $(p + 'Facts');
  facts.replaceChildren();
  if (p === 'sr') {
    fact(facts, 'Model', s.model + (s.installed ? '' : ' (not downloaded)'));
    fact(facts, 'Licence', el('a', { href: f.licenseUrl, target: '_blank', rel: 'noopener noreferrer' }, f.license));
    fact(facts, 'Kept in', s.dir, true);
  } else {
    fact(facts, 'Models', F.models + (f.installed ? ' (installed)' : ' (not downloaded)'));
    fact(facts, 'Licence', el('a', { href: f.licenseUrl, target: '_blank', rel: 'noopener noreferrer' }, f.license));
  }
  const list = $(p + 'Users');
  list.replaceChildren();
  let any = false;
  for (const u of s.users) {
    const done = u[F.users];
    if (done === null || done === undefined) continue;
    any = true;
    const w = u.work && u.work[PART_KEY[p]];
    const meter = el('div', { class: 'meter' + (w && w.perMinute && done < u.total && !f.schedule.paused ? ' busy' : '') }, el('div', {}));
    list.append(
      el(
        'li',
        {},
        avatar(u.name),
        el('a', { class: 'name', href: '#/users/' + enc(u.name) }, u.name),
        el('span', { class: 'bytes' }, n(done) + ' / ' + n(u.total)),
        meter,
        ...userLines(p, w, done, u.total, f.schedule, u.name),
      ),
    );
    progress(meter, u.total ? done / u.total : 1);
  }
  const none = $(p + 'UsersNone');
  none.hidden = any;
  none.textContent = f.active ? 'No users yet.' : d.running ? F.label : F.idle;
  return d.running || (f.active && s.users.some((u) => u[F.users] !== null && u[F.users] < u.total));
}

function showPeople(s) {
  const f = s.faces;
  const d = f.download;
  showFetch('fc', d);
  showFetchError('fc', d, f.installed);
  showSched('fc', f.schedule);
  $('fcGet').hidden = f.installed || d.running;
  const needs = $('fcNeeds');
  needs.hidden = f.installed;
  needs.textContent = d.running ? 'Downloading the face models onto this server…' : 'Needs the face models: ' + bytes(f.sizeBytes) + ', downloaded once onto this server.';
  const list = $('fcUsers');
  list.replaceChildren();
  if (!f.installed) return d.running;
  for (const u of s.users) {
    const input = el('input', { type: 'checkbox', role: 'switch', 'aria-label': 'Find the people in ' + u.name + "'s photos" });
    input.checked = u.faces;
    input.addEventListener('change', () => togglePeople(u, input));
    const text = u.faces ? (u.found === null ? 'Waiting for the server…' : n(u.found) + ' of ' + n(u.total) + ' photos looked at') : 'Off';
    const w = u.work && u.work.faces;
    const lines = u.faces && u.found !== null ? userLines('fc', w, u.found, u.total, f.schedule, u.name) : [];
    list.append(el('li', {}, avatar(u.name), el('a', { class: 'name', href: '#/users/' + enc(u.name) }, u.name), el('span', { class: 'bytes' }, text), el('label', { class: 'switch' }, input), ...lines));
  }
  if (!s.users.length) list.append(el('li', { class: 'muted' }, 'No users yet.'));
  return s.users.some((u) => u.faces && u.found !== null && u.found < u.total);
}

/** Building previews for everyone: the batch added up, what is being built, and each user's coverage. */
function showPreviewsAll(v) {
  const paused = v.paused;
  const cover = v.photos == null ? 'Counting previews…' : n(v.have) + ' of ' + plural(v.photos, 'photo has', 'photos have') + ' previews' + (v.photos ? ' (' + Math.floor((100 * v.have) / v.photos) + '%)' : '');
  $('pvCover').textContent = (paused ? 'Paused. ' : v.left ? (v.rebuilding ? 'Rebuilding. ' : 'Building. ') : '') + cover;
  const total = v.done + v.left;
  $('pvText').textContent =
    (v.left
      ? n(v.done) + ' built, ' + n(v.left) + ' left' + (v.etaSecs && !paused ? ', about ' + dur(v.etaSecs) + ' to go' : '') + (v.perMinute && !paused ? ' (' + n(v.perMinute) + ' a minute)' : '') + '.'
      : 'Nothing waiting: 0 left.' + (v.lastAt ? ' The last batch, ' + plural(v.lastDone, 'preview', 'previews') + ', finished ' + ago(v.lastAt) + '.' : '')) +
    (paused ? ' Paused for every user: the ones being built finish, then it waits.' : '');
  progress($('pvBar'), v.rebuilding && total ? v.done / total : v.photos ? v.have / v.photos : v.left && total ? v.done / total : 0);
  $('pvNow').replaceChildren(
    ...v.building.slice(0, 6).map((f) => el('li', {}, el('b', {}, 'Now'), el('code', { title: f }, f.slice(f.lastIndexOf('/') + 1)))),
    ...(v.left && !v.building.length && !paused ? [el('li', { class: 'muted' }, 'Waiting for a builder.')] : []),
  );
  const pause = $('pvPause');
  pause.hidden = !paused && !v.left;
  pause.querySelector('span').textContent = paused ? 'Resume' : 'Pause';
  pause.querySelector('use').setAttribute('href', paused ? '#i-play' : '#i-pause');
  pause.onclick = () => previewsAll(paused ? 'resume' : 'pause');
  $('pvAll').onclick = $('pvMissing').onclick = () => previewsAll('generate');
  $('pvRebuild').onclick = () =>
    ask({
      title: 'Rebuild all previews?',
      text: "The server builds the previews of every photo in every user's library folders again, the ones that exist too, and replaces them as each new one is ready. It can take hours on a big library. To make only the missing ones, choose Build missing previews.",
      ok: 'Rebuild all previews',
      action: () => previewsAll('rebuild'),
    });
  const list = $('pvUsers');
  list.replaceChildren();
  $('pvUsersNone').hidden = v.users.length > 0;
  for (const u of v.users) {
    const meter = el('div', { class: 'meter' + (u.left && !paused ? ' busy' : '') }, el('div', {}));
    const text = u.photos == null ? 'Counting…' : n(u.have) + ' / ' + n(u.photos);
    list.append(
      el('li', {}, avatar(u.name), el('a', { class: 'name', href: '#/users/' + enc(u.name) }, u.name), el('span', { class: 'bytes' }, text), meter,
        ...(u.left || u.errors ? [el('p', { class: 'sub' }, [u.left ? n(u.left) + ' left' : '', u.errors ? plural(u.errors, 'preview', 'previews') + " couldn't be built" : ''].filter(Boolean).join(' · '))] : [])),
    );
    progress(meter, u.photos ? u.have / u.photos : u.photos === 0 ? 1 : 0);
  }
  return !!v.left;
}

async function previewsAll(action) {
  try {
    await api('POST', 'previews/' + action);
    if (action === 'generate') toast('Queued every missing preview.');
    if (action === 'rebuild') toast('Rebuilding every preview.');
  } catch (e) {
    fail(e);
  }
  await route();
}

/** Whether a model is in the server's memory, how many users' indexes are with it, and when it is let go. */
function memoryLine(m, key) {
  const h = m[key];
  if (!h.loaded) return 'Not in memory · loads when it is needed';
  const left = m.unloadInSecs === null ? '' : m.unloadInSecs ? ' · let go in about ' + dur(m.unloadInSecs) + ' if not used' : ' · being let go';
  return 'In memory' + (h.indexes ? ' with ' + plural(h.indexes, "user's index", "users' indexes") : '') + left;
}

/** One row per AI model: what it is for, its size, whether it is here, whether it is in memory, and a button to get it. */
function showModels(s) {
  const m = s.memory;
  const any = ['search', 'text', 'faces', 'sensitive'].some((k) => m[k].loaded);
  $('mmMemory').textContent =
    (m.rss === null ? '' : 'The server holds ' + bytes(m.rss) + ' of memory (read every 30 s). ') +
    (any
      ? 'Models are let go after ' + dur(m.unloadAfterSecs) + ' without use' + (m.idleSecs === null ? '.' : '; the last use was ' + dur(m.idleSecs) + ' ago. The check runs every 30 s.')
      : 'No model is in memory now. Each loads when it has work.');
  const rows = [
    ['sr', 'Search by description', s.model, s, () => downloadFeature('sr', s)],
    ['tx', 'Words in photos', FEATURES.tx.models, s.text, () => downloadFeature('tx', s.text)],
    ['fc', 'People', 'YuNet and SFace', s.faces, downloadFaces],
    ['sn', 'Sensitive content', FEATURES.sn.models, s.sensitive, () => downloadFeature('sn', s.sensitive)],
  ];
  let going = false;
  $('mmList').replaceChildren(
    ...rows.map(([p, what, model, f, get]) => {
      const d = f.download;
      going ||= d.running;
      const btn = d.running
        ? el('button', { type: 'button', class: 'btn secondary', onclick: () => cancelDownload(p) }, 'Cancel')
        : f.installed
          ? el('span', { class: 'ok' }, el('span', {}, 'Installed'), el('button', { type: 'button', class: 'btn quiet', onclick: () => reprocessModel(p, what) }, 'Reprocess'), el('button', { type: 'button', class: 'btn quiet', onclick: () => removeModel(p, what) }, 'Remove'))
          : el('button', { type: 'button', class: 'btn accent', onclick: get }, 'Download');
      const meter = d.running ? el('div', { class: 'meter' }, el('div', {})) : null;
      const li = el('li', {}, el('span', { class: 'what' }, el('b', {}, what), el('span', { class: 'muted' }, model + ' · ' + bytes(f.sizeBytes))), btn, ...(meter ? [meter] : []), ...(f.installed ? [el('span', { class: 'mem' + (m[PART_KEY[p]].loaded ? ' on' : '') }, memoryLine(m, PART_KEY[p]))] : []));
      if (meter) progress(meter, d.total ? d.done / d.total : 0);
      return li;
    }),
  );
  return going ? 2000 : any ? 5000 : 0;
}

/** Delete a part's model files from the server (what it found stays), after asking. */
function removeModel(p, what) {
  return ask({
    title: 'Delete the ' + what.toLowerCase() + ' model?',
    text:
      'This deletes its files from this server and frees their disk space. What it already found for your users is kept, but nothing new is found until you download the model again. ' +
      (p === 'fc' ? 'People stays switched on for the users who have it. ' : '') + 'It can be downloaded again at any time.',
    ok: 'Delete',
    danger: true,
    action: () => api('DELETE', 'search/models/' + PART_KEY[p]),
  }).then(async (ok) => {
    if (ok) toast('Deleted the ' + what.toLowerCase() + ' model.', 'info');
    await route();
  });
}

/** Forget what a model found for every user, and have it done again, after asking. */
function reprocessModel(p, what) {
  return ask({
    title: 'Process every photo again with the ' + what.toLowerCase() + ' model?',
    text:
      'This deletes what the model found for all your users, then processes every photo again, on this server and on the devices that do it for their users. ' +
      'Until that is done, searching and the lists that use it find less. The model itself stays.',
    ok: 'Process again',
    danger: true,
    action: () => api('DELETE', 'search/results/' + PART_KEY[p]),
  }).then(async (ok) => {
    if (ok) toast('Processing every photo again with the ' + what.toLowerCase() + ' model.', 'info');
    await route();
  });
}

function showSearch([s]) {
  const again = [showPreviewsAll(s.previews), showFeature('sr', s), showFeature('tx', s), showFeature('sn', s), showPeople(s)].some(Boolean);
  const models = showModels(s);
  clearTimeout(timer);
  // (downloads and describing run on the server: watch them; so does the countdown to a model leaving memory, less often)
  if (again || models) timer = setTimeout(route, again ? 2000 : models);
}

/** Turn a feature on or off; turning it on without its models asks first, then downloads them. */
async function toggleFeature(p, e) {
  const F = FEATURES[p];
  const sw = e.target;
  const want = sw.checked;
  sw.disabled = true;
  try {
    const s = await api('GET', 'search');
    const f = F.pick(s);
    if (!want) {
      if (f.download.running) await api('DELETE', F.path + '/download');
      await api('PUT', F.path, { enabled: false });
      toast('Turned off.', 'info');
    } else if (f.installed) {
      await api('PUT', F.path, { enabled: true });
      toast(F.done);
    } else {
      await downloadFeature(p, f);
    }
  } catch (x) {
    fail(x);
  } finally {
    // (shows what is true now, whichever way it went, and puts the switch back if it was cancelled)
    await route();
  }
}

/** Ask about the licence, then download a feature's models and turn it on. */
async function downloadFeature(p, f) {
  const F = FEATURES[p];
  const ok = await ask({ title: F.title, text: F.ask(f), ok: 'Download and turn on', action: () => api('POST', F.path + '/download', { acknowledged: true }) });
  if (ok) toast('Downloading the ' + F.what + '. You can leave this page: it keeps going.', 'info');
}

async function downloadFaces() {
  const btn = $('fcDownload');
  busy(btn, true);
  try {
    const s = await api('GET', 'search');
    const f = s.faces;
    const ok = await ask({
      title: 'Download the face models?',
      text:
        'Finding people uses two small AI models: YuNet finds faces and SFace tells them apart. They are not part of LightCraft. Together they are a ' +
        bytes(f.sizeBytes) + ' download onto this server, under ' + f.licenses.map((l) => 'the ' + l.name + ' (' + l.url + ')').join(' and ') +
        '. Downloading them means accepting those licences. Nothing is looked at until you turn it on for a user.',
      ok: 'Download',
      action: () => api('POST', 'search/faces/download', { acknowledged: true }),
    });
    if (ok) toast('Downloading the face models. You can leave this page: it keeps going.', 'info');
  } catch (x) {
    fail(x);
  } finally {
    busy(btn, false);
    await route();
  }
}

/** Finding the people in a user's photos: turning it on asks first, because faces are personal data. */
async function togglePeople(u, input) {
  const want = input.checked;
  input.disabled = true;
  try {
    if (want) {
      const ok = await ask({
        title: 'Find the people in ' + u.name + "'s photos?",
        text:
          'The server will look at the faces in ' + u.name + "'s photos and group them into people, which they name on their own devices. What it finds stays on this server, and " +
          u.name + ' can delete all of it from their devices (Forget All Faces). Only do this if they agree.',
        ok: 'Turn on',
        action: () => api('PUT', 'users/' + enc(u.name) + '/faces', { enabled: true }),
      });
      if (ok) toast('Looking for the people in ' + u.name + "'s photos.", 'info');
    } else {
      await api('PUT', 'users/' + enc(u.name) + '/faces', { enabled: false });
      toast('Stopped looking for people in ' + u.name + "'s photos. What was found stays until they delete it.", 'info');
    }
  } catch (x) {
    fail(x);
  } finally {
    await route();
  }
}

/** The server does a part itself again for a user whose devices were doing it. */
async function continueHere(btn, p, user) {
  busy(btn, true);
  try {
    await api('POST', 'users/' + enc(user) + '/search/' + PART_KEY[p] + '/here');
    toast('This server will do the rest for ' + user + '.', 'info');
  } catch (x) {
    fail(x);
  } finally {
    busy(btn, false);
    await route();
  }
}

async function cancelDownload(p) {
  const btn = $(p + 'Cancel');
  busy(btn, true);
  try {
    await api('DELETE', (p === 'fc' ? 'search/faces' : FEATURES[p].path) + '/download');
  } catch (x) {
    fail(x);
  } finally {
    busy(btn, false);
    await route();
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
  signedIn = false;
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
  clearTimeout(usageTimer);
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

/** Sign in: the server answers with a cookie, which the browser keeps (for weeks with `remember`, else until it closes). */
async function signIn(user, password, remember) {
  const r = await api('POST', 'login', { user, password, remember: !!remember, cookie: true });
  signedIn = true;
  setMe(r.user);
  showDash();
}

async function start() {
  $('origin').value = location.origin;
  // Still signed in from the last visit? The cookie says so (nearly every page needs the users: asked for
  // with the sign-in check, not after it).
  get('users').catch(() => {});
  try {
    setMe((await get('status')).me);
    signedIn = true;
    showDash();
    return;
  } catch (_) {
    forget();
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
    await signIn(f.get('user'), f.get('password'), true);
    toast('Welcome! Add the people who use this server, then connect their devices.', 'info');
  });
  onSubmit('loginForm', async (f) => {
    await signIn(f.get('user'), f.get('password'), f.get('remember') === 'on');
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
  splitMenu('scanSplit');
  splitMenu('previewsSplit');
  splitMenu('pvSplit');
  $('limForm').addEventListener('submit', saveLimits);
  $('addUser').addEventListener('click', addUser);
  $('gcCheck').addEventListener('click', gcCheck);
  $('importPresets').addEventListener('click', () => {
    $('presetFiles').value = '';
    $('presetFiles').click();
  });
  $('presetFiles').addEventListener('change', importPresets);
  $('gcRun').addEventListener('click', gcRun);
  for (const p of ['sr', 'tx', 'sn']) $(p + 'Switch').addEventListener('change', (e) => toggleFeature(p, e));
  for (const p of ['sr', 'tx', 'fc', 'sn']) $(p + 'Cancel').addEventListener('click', () => cancelDownload(p));
  $('fcDownload').addEventListener('click', downloadFaces);
  window.addEventListener('hashchange', route);
  start();
});
