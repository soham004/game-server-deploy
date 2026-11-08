/**
 * Schedule plans as the server stores them (`SchedulePlan` JSON), and the pieces the editor
 * needs that don't touch the DOM: warning lists, "every day at", durations, templates.
 * Pure functions, unit-tested in tests/web/schedule-plan.test.mjs.
 */

/** "10m, 5m, 1m, 10s" or "600, 300" → [600, 300, 60, 10] (seconds). Throws on nonsense. */
export function parseWarnings(text) {
  const parts = text.split(/[\s,]+/).filter(Boolean);
  return parts.map((part) => {
    const match = /^(\d+)\s*(h|m|min|s|sec)?$/i.exec(part);
    if (!match) throw new Error(`"${part}" is not a time like 10m or 30s`);
    const value = Number(match[1]);
    const unit = (match[2] ?? 's').toLowerCase();
    if (unit === 'h') return value * 3600;
    if (unit.startsWith('m')) return value * 60;
    return value;
  });
}

/** [600, 300, 60, 10] → "10m, 5m, 1m, 10s". */
export function formatWarnings(seconds) {
  return seconds.map((secs) => {
    if (secs >= 3600 && secs % 3600 === 0) return `${secs / 3600}h`;
    if (secs >= 60 && secs % 60 === 0) return `${secs / 60}m`;
    return `${secs}s`;
  }).join(', ');
}

/** "04:30" → a daily cron expression ("30 4 * * *"). */
export function dailyCron(time) {
  const match = /^(\d{1,2}):(\d{2})$/.exec(time);
  if (!match) throw new Error('Choose a time of day');
  return `${Number(match[2])} ${Number(match[1])} * * *`;
}

/** The time of a daily cron expression ("30 4 * * *" → "04:30"), or null for any other pattern. */
export function dailyTime(expr) {
  const match = /^(\d{1,2}) (\d{1,2}) \* \* \*$/.exec(expr.trim());
  if (!match) return null;
  return `${match[2].padStart(2, '0')}:${match[1].padStart(2, '0')}`;
}

/** Seconds as the editor shows an interval: a count and a unit that divides it evenly. */
export function splitDuration(secs) {
  if (secs % 86400 === 0) return { count: secs / 86400, unit: 86400 };
  if (secs % 3600 === 0) return { count: secs / 3600, unit: 3600 };
  return { count: Math.round(secs / 60), unit: 60 };
}

/** The browser's IANA time zone, e.g. "Europe/London". */
export function browserTimeZone() {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
}

/** A disabled-until-saved, empty plan for "New schedule". */
export function blankPlan(timeZone) {
  return {
    name: '',
    enabled: true,
    trigger: { kind: 'cron', expr: '0 4 * * *', time_zone: timeZone },
    conditions: [],
    actions: [{ kind: 'command', command: '' }],
    on_error: 'stop',
    catch_up: false,
  };
}

/** Ready-made plans for common jobs (the plan's "templates"). */
export function templates(timeZone) {
  return [
    {
      id: 'daily-restart',
      label: 'Daily restart at 4 AM with warnings',
      plan: {
        name: 'Daily restart',
        enabled: true,
        trigger: { kind: 'cron', expr: '0 4 * * *', time_zone: timeZone },
        conditions: [{ kind: 'server_running' }],
        actions: [{ kind: 'restart', warn_secs: [600, 300, 60, 10] }],
        on_error: 'stop',
        catch_up: false,
      },
    },
    {
      id: 'backup-if-played',
      label: 'Back up every 6 hours if somebody played',
      plan: {
        name: 'Backup when played',
        enabled: true,
        trigger: { kind: 'interval', every_secs: 21600 },
        conditions: [{ kind: 'players_since_last_run' }],
        actions: [{ kind: 'backup', scope: 'world' }],
        on_error: 'stop',
        catch_up: true,
      },
    },
    {
      id: 'announce',
      label: 'Announce a message every 30 minutes',
      plan: {
        name: 'Announcement',
        enabled: true,
        trigger: { kind: 'interval', every_secs: 1800 },
        conditions: [{ kind: 'server_running' }],
        actions: [{ kind: 'broadcast', message: 'Remember to vote for the server!' }],
        on_error: 'stop',
        catch_up: false,
      },
    },
    {
      id: 'save',
      label: 'Save the world every 10 minutes',
      plan: {
        name: 'Save the world',
        enabled: true,
        trigger: { kind: 'interval', every_secs: 600 },
        conditions: [{ kind: 'server_running' }],
        actions: [{ kind: 'command', command: 'save-all' }],
        on_error: 'stop',
        catch_up: false,
      },
    },
  ];
}

/** A one-line summary of a plan's actions: "Restart (warnings 10m, 1m) → Backup (worlds)". */
export function actionsSummary(actions) {
  return actions.map((action) => {
    switch (action.kind) {
      case 'command': return `Command: ${action.command}`;
      case 'broadcast': return `Message: ${action.message}`;
      case 'restart': return action.warn_secs?.length ? `Restart (warnings ${formatWarnings(action.warn_secs)})` : 'Restart';
      case 'stop': return action.warn_secs?.length ? `Stop (warnings ${formatWarnings(action.warn_secs)})` : 'Stop';
      case 'start': return 'Start';
      case 'backup': return `Backup (${action.scope === 'full' ? 'everything' : 'worlds'})`;
      case 'wait': return `Wait ${action.secs}s`;
      default: return action.kind;
    }
  }).join(' → ');
}
