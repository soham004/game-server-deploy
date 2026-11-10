/**
 * One player: head, name, UUID, when they were seen and how long they played, what the lists say about
 * them, what can be done to them, and their latest visits.
 *
 *   const card = createPlayerCard({ instanceId, canManage, canAnyLevel, run, onClose });
 *   card.show(player, view);   // `player` from view.known (or any { name, uuid })
 *   card.update(view);         // after the lists changed
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { dateTime, relativeTime } from '../core/format.js';
import { BAN_LENGTHS, banExpiry, playedLabel, secondsSince, standing } from '../core/players.js';
import { head } from './player-head.js';

const LEVELS = [
  [1, 'Level 1: can build in spawn protection'],
  [2, 'Level 2: also cheat commands and command blocks'],
  [3, 'Level 3: also kick, ban and op others'],
  [4, 'Level 4: everything, stopping the server too'],
];

export function createPlayerCard({ instanceId, canManage, canAnyLevel, run, onClose }) {
  const element = h('section', { class: 'c-card c-player-card', hidden: true, 'aria-live': 'polite' });
  let player = null;
  let view = null;
  /** Numbers each look-up of visits, so an older answer can't replace a newer one. */
  let visitsRead = 0;
  const visits = h('ol', { class: 'c-player-card__visits' });

  function show(chosen, current) {
    player = chosen;
    view = current;
    render();
    element.hidden = false;
    element.scrollIntoView({ block: 'nearest' });
    loadVisits();
  }

  function update(current) {
    view = current;
    if (player) render();
  }

  function close() {
    player = null;
    element.hidden = true;
    onClose?.();
  }

  async function loadVisits() {
    const thisRead = ++visitsRead;
    visits.replaceChildren(h('li', { class: 'c-muted' }, 'Loading…'));
    try {
      const list = await api.get(`/instances/${instanceId}/players/${encodeURIComponent(player.name)}/sessions`);
      if (thisRead !== visitsRead) return;
      visits.replaceChildren(...(list.length === 0
        ? [h('li', { class: 'c-muted' }, 'No visits seen by this app.')]
        : list.map(visitRow)));
    } catch (error) {
      if (thisRead === visitsRead) visits.replaceChildren(h('li', { class: 'c-muted' }, error.message));
    }
  }

  function visitRow(visit) {
    let length = 'end not seen';
    if (visit.online) length = `on now, for ${playedLabel(secondsSince(visit.joined_at))}`;
    else if (visit.left_at) length = playedLabel(secondsSince(visit.joined_at, new Date(visit.left_at).getTime()));
    return h('li', null, `${dateTime(visit.joined_at)} · ${length}`);
  }

  function render() {
    const known = view.known.find((other) => other.name.toLowerCase() === player.name.toLowerCase()) ?? player;
    const { online, operator, allowed, ban } = standing(view, player.name);
    const facts = [
      online ? `Online${online.since ? ` for ${playedLabel(secondsSince(online.since))}` : ''}` : null,
      operator ? `Operator, level ${operator.level}` : null,
      allowed ? 'On the allowlist' : null,
      ban ? `Banned${ban.expires ? ` until ${dateTime(ban.expires)}` : ''}${ban.reason ? `: ${ban.reason}` : ''}` : null,
    ].filter(Boolean);
    const uuid = known.uuid ?? online?.uuid ?? null;
    element.replaceChildren(
      h('header', { class: 'c-player-card__header' },
        head(uuid, player.name, 'is-large'),
        h('div', { class: 'c-player-card__title' },
          h('h2', { class: 'c-player-card__name' }, player.name),
          uuid ? h('div', { class: 'c-player-card__uuid' },
            h('code', null, uuid),
            h('button', { class: 'c-button c-button--ghost c-button--small', type: 'button', onClick: () => copy(uuid) }, 'Copy')) : null),
        h('button', { class: 'c-button c-button--ghost', type: 'button', onClick: close, 'aria-label': 'Close' }, 'Close')),
      h('dl', { class: 'c-details' },
        known.last_seen ? row('Last seen', known.online ? 'Now' : relativeTime(known.last_seen)) : null,
        known.first_seen ? row('First seen', dateTime(known.first_seen)) : null,
        known.played_seconds !== null && known.played_seconds !== undefined ? row('Played', playedLabel(known.played_seconds)) : null,
        known.visits ? row('Visits', String(known.visits)) : null),
      facts.length ? h('ul', { class: 'c-tag-list' }, facts.map((fact) => h('li', { class: 'c-tag' }, fact))) : null,
      canManage && view.channel ? actions(online, operator, allowed, ban) : null,
      h('h3', { class: 'c-player-card__heading' }, 'Latest visits'),
      visits,
    );
  }

  function actions(online, operator, allowed, ban) {
    const name = player.name;
    const levelsChoosable = view.channel !== 'console';
    const level = h('select', { class: 'c-input', name: 'op_level', 'aria-label': 'Operator level' },
      LEVELS.filter(([value]) => canAnyLevel || value <= 2).map(([value, label]) => h('option', { value, selected: value === Math.min(view.default_op_level, canAnyLevel ? 4 : 2) }, label)));
    const reason = h('input', { class: 'c-input', name: 'ban_reason', placeholder: 'Reason (optional)', maxlength: 256 });
    const length = h('select', { class: 'c-input', name: 'ban_length', 'aria-label': 'How long' },
      BAN_LENGTHS.filter(([id]) => id === 'forever' || view.channel !== 'console').map(([id, label]) => h('option', { value: id }, label)));
    const message = h('input', { class: 'c-input', name: 'message_text', placeholder: 'Message', maxlength: 256 });
    const button = (label, action, modifier = '') => h('button', { class: `c-button ${modifier}`.trim(), type: 'button', onClick: () => run(action()) }, label);

    return h('div', { class: 'c-player-card__actions' },
      h('div', { class: 'c-player-card__row' },
        operator
          ? button('Remove operator', () => ({ action: 'deop', name }))
          : [levelsChoosable ? level : null, button('Make operator', () => ({ action: 'op', name, ...(levelsChoosable ? { level: Number(level.value) } : {}) }))],
        allowed
          ? button('Take off the allowlist', () => ({ action: 'disallow', name }))
          : button('Add to the allowlist', () => ({ action: 'allow', name }))),
      h('div', { class: 'c-player-card__row' },
        ban
          ? button('Unban', () => ({ action: 'pardon', name }))
          : [reason, length, button('Ban', () => ({ action: 'ban', name, reason: reason.value || null, expires: banExpiry(length.value) }), 'c-button--danger')]),
      online ? h('div', { class: 'c-player-card__row' },
        h('form', { class: 'c-player-card__row', onSubmit: (event) => { event.preventDefault(); if (message.value.trim()) run({ action: 'message', name, text: message.value }).then(() => { message.value = ''; }); } },
          message, h('button', { class: 'c-button', type: 'submit' }, 'Send')),
        button('Kick', () => ({ action: 'kick', name, reason: reason.value || null }))) : null,
    );
  }

  return { element, show, update, close };
}

function row(term, description) {
  return h('div', { class: 'c-details__row' }, h('dt', null, term), h('dd', null, description));
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Without clipboard access (plain HTTP), the UUID is still there to select.
  }
}
