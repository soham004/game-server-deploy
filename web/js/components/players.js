/**
 * A server's players: who is on, operators, the allowlist, bans, and everyone seen, with a card for one
 * player. Changes go wherever the server's state allows (the management protocol, the console, or the
 * stopped server's files); the server says which, and its lists are shown as it has them afterwards.
 *
 *   const players = createPlayers({ instanceId, can });
 *   panel.append(players.element);
 *   players.load();       // on opening the tab
 *   players.refresh();    // when someone joins or leaves, or the server starts or stops
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { dateTime, relativeTime } from '../core/format.js';
import { channelNote, isPlayerName, playedLabel, playerMatches, secondsSince } from '../core/players.js';
import { toast } from '../core/toast.js';
import { createPlayerCard } from './player-card.js';
import { head } from './player-head.js';

/** How often "online for …" moves on. */
const CLOCK_MS = 30_000;

const VIA = {
  management: 'through the management protocol',
  console: 'through the console',
  files: 'in the server’s files',
};

export function createPlayers({ instanceId, can }) {
  const canManage = can('players.manage');
  const canAnyLevel = can('console.write');
  const note = h('p', { class: 'c-muted c-players__note', role: 'status' });
  const online = h('div', { class: 'c-players__section' });
  const operators = h('div', { class: 'c-players__section' });
  const allowlist = h('div', { class: 'c-players__section' });
  const bans = h('div', { class: 'c-players__section' });
  const search = h('input', { class: 'c-input', type: 'search', name: 'player_search', placeholder: 'Find a player', 'aria-label': 'Find a player' });
  const knownList = h('ul', { class: 'c-players__list' });
  const known = h('div', { class: 'c-players__section c-players__known' },
    h('div', { class: 'c-players__section-head' }, h('h2', { class: 'c-players__heading' }, 'Everyone seen'), search),
    knownList);
  const card = createPlayerCard({ instanceId, canManage, canAnyLevel, run });

  const name = h('input', { class: 'c-input', name: 'player_name', placeholder: 'Player name', maxlength: 16, autocomplete: 'off', 'aria-label': 'Player name' });
  const addForm = canManage ? h('form', { class: 'c-players__add', onSubmit: (event) => { event.preventDefault(); add('allow'); } },
    name,
    h('button', { class: 'c-button c-button--primary', type: 'submit' }, 'Add to the allowlist'),
    h('button', { class: 'c-button', type: 'button', onClick: () => add('op') }, 'Make operator'),
    h('button', { class: 'c-button c-button--danger', type: 'button', onClick: () => add('ban') }, 'Ban'),
    h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: () => add('show') }, 'Look up')) : null;

  const element = h('section', { class: 'c-players' },
    note, addForm, card.element,
    h('div', { class: 'c-players__grid' }, online, operators, allowlist, bans),
    known);

  let view = null;
  let loaded = false;
  let clock = null;
  /** Numbers each read, so an older answer can't replace a newer one. */
  let latestRead = 0;

  async function load() {
    loaded = true;
    clock ??= window.setInterval(() => { if (view && view.online.length) render(); }, CLOCK_MS);
    await refresh();
  }

  async function refresh() {
    if (!loaded) return;
    const thisRead = ++latestRead;
    try {
      const fresh = await api.get(`/instances/${instanceId}/players`);
      if (thisRead !== latestRead) return;
      view = fresh;
      render();
    } catch (error) {
      if (thisRead === latestRead) note.textContent = error.message;
    }
  }

  /** Does one change; the lists come back as the server has them afterwards. */
  async function run(action) {
    try {
      const applied = await api.post(`/instances/${instanceId}/players/actions`, action);
      ++latestRead;
      view = applied.players;
      render();
      toast(`Done ${VIA[applied.via] ?? ''}.`.replace(' .', '.'), { kind: 'success' });
      return applied;
    } catch (error) {
      toast(error.message, { kind: 'error' });
      return null;
    }
  }

  function add(what) {
    const player = name.value.trim();
    if (!isPlayerName(player)) {
      toast('A Minecraft name has up to 16 letters, digits and _.', { kind: 'error' });
      name.focus();
      return;
    }
    if (what === 'show') {
      openCard({ name: player, uuid: null });
      return;
    }
    const action = what === 'ban' ? { action: 'ban', name: player } : { action: what, name: player };
    run(action).then((applied) => { if (applied) name.value = ''; });
  }

  function openCard(player) {
    card.show(player, view);
  }

  function render() {
    note.textContent = channelNote(view.channel);
    if (addForm) for (const control of addForm.elements) control.disabled = !view.channel;
    card.update(view);
    online.replaceChildren(
      heading(`Online (${view.online.length})`),
      list(view.online, (player) => playerRow(player, player.since ? `for ${playedLabel(secondsSince(player.since))}` : 'online',
        canManage ? h('button', { class: 'c-button c-button--ghost c-button--small', type: 'button', onClick: () => run({ action: 'kick', name: player.name }) }, 'Kick') : null),
      'Nobody is online.'),
    );
    operators.replaceChildren(
      heading(`Operators (${view.operators.length})`),
      list(view.operators, (op) => playerRow(op, `level ${op.level}`, removeButton({ action: 'deop', name: op.name }, `Remove ${op.name} as operator`)), 'No operators.'),
    );
    const toggle = h('input', { type: 'checkbox', name: 'allowlist_on', checked: view.allowlist.on, disabled: !canManage || !view.channel });
    toggle.addEventListener('change', () => run({ action: 'allowlist', on: toggle.checked }));
    allowlist.replaceChildren(
      heading(`Allowlist (${view.allowlist.players.length})`),
      h('label', { class: 'c-check' }, toggle, h('span', null, 'Only players on the allowlist may join')),
      list(view.allowlist.players, (player) => playerRow(player, '', removeButton({ action: 'disallow', name: player.name }, `Take ${player.name} off the allowlist`)), 'Nobody is on the allowlist.'),
    );
    bans.replaceChildren(
      heading(`Bans (${view.bans.length + view.ip_bans.length})`),
      list([...view.bans, ...view.ip_bans], banRow, 'Nobody is banned.'),
    );
    renderKnown();
  }

  function renderKnown() {
    const shown = view.known.filter((player) => playerMatches(player, search.value));
    knownList.replaceChildren(...(shown.length === 0
      ? [h('li', { class: 'c-muted' }, view.known.length === 0 ? 'Nobody has played here yet.' : 'Nobody by that name.')]
      : shown.map((player) => playerRow(player, knownDetail(player)))));
  }

  function knownDetail(player) {
    const parts = [];
    if (player.online) parts.push('online now');
    else if (player.last_seen) parts.push(`seen ${relativeTime(player.last_seen)}`);
    if (player.played_seconds) parts.push(`played ${playedLabel(player.played_seconds)}`);
    return parts.join(' · ');
  }

  function banRow(ban) {
    const until = ban.expires ? `until ${dateTime(ban.expires)}` : 'forever';
    const detail = [until, ban.reason, ban.source ? `by ${ban.source}` : null].filter(Boolean).join(' · ');
    if (ban.ip) {
      return h('li', { class: 'c-players__row', dataset: { ip: ban.ip } },
        h('span', { class: 'c-player-head is-placeholder', 'aria-hidden': 'true' }, 'IP'),
        h('span', { class: 'c-players__who' }, h('code', null, ban.ip), h('span', { class: 'c-muted c-players__detail' }, detail)),
        removeButton({ action: 'pardon_ip', ip: ban.ip }, `Unban ${ban.ip}`, 'Unban'));
    }
    return playerRow({ name: ban.name ?? '?', uuid: ban.uuid }, detail, removeButton({ action: 'pardon', name: ban.name }, `Unban ${ban.name}`, 'Unban'));
  }

  function playerRow(player, detail, action = null) {
    return h('li', { class: 'c-players__row', dataset: { player: player.name } },
      head(player.uuid, player.name),
      h('button', { class: 'c-players__who c-players__open', type: 'button', onClick: () => openCard(player) },
        h('span', { class: 'c-players__name' }, player.name),
        detail ? h('span', { class: 'c-muted c-players__detail' }, detail) : null),
      action);
  }

  function removeButton(action, label, text = 'Remove') {
    if (!canManage) return null;
    return h('button', { class: 'c-button c-button--ghost c-button--small', type: 'button', 'aria-label': label, disabled: !view.channel, onClick: () => run(action) }, text);
  }

  search.addEventListener('input', () => { if (view) renderKnown(); });

  function destroy() {
    if (clock) window.clearInterval(clock);
    clock = null;
  }

  return { element, load, refresh, destroy };
}

function heading(text) {
  return h('h2', { class: 'c-players__heading' }, text);
}

function list(items, row, empty) {
  return h('ul', { class: 'c-players__list' }, items.length === 0 ? h('li', { class: 'c-muted' }, empty) : items.map(row));
}
