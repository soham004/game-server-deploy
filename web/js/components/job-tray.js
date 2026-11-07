/**
 * Header job tray: a count of running jobs and a dropdown with their progress.
 * Mounted once by main.js into `[data-job-tray]`.
 */
import { api } from '../core/api.js';
import { h } from '../core/dom.js';
import { live } from '../core/ws.js';

const jobs = new Map(); // id -> snapshot

export async function mountJobTray(root) {
  const count = h('span', { class: 'c-job-tray__count', 'aria-hidden': 'true' });
  const button = h('button', {
    class: 'c-button c-button--ghost c-job-tray__button',
    type: 'button',
    'aria-expanded': 'false',
    'aria-haspopup': 'true',
  }, 'Tasks ', count);
  const panel = h('div', { class: 'c-job-tray__panel', role: 'region', 'aria-label': 'Background tasks', hidden: true });
  root.append(button, panel);

  button.addEventListener('click', () => {
    const open = panel.hidden;
    panel.hidden = !open;
    button.setAttribute('aria-expanded', String(open));
  });
  document.addEventListener('click', (event) => {
    if (!root.contains(event.target)) {
      panel.hidden = true;
      button.setAttribute('aria-expanded', 'false');
    }
  });

  const render = () => {
    const list = [...jobs.values()].sort((a, b) => b.started_at.localeCompare(a.started_at)).slice(0, 10);
    const running = list.filter((job) => job.status === 'running').length;
    count.textContent = String(running);
    root.classList.toggle('is-active', running > 0);
    panel.replaceChildren(
      list.length === 0
        ? h('p', { class: 'c-job-tray__empty' }, 'No recent tasks.')
        : h('ul', { class: 'c-job-tray__list' }, list.map(jobItem)),
    );
  };

  const refresh = async () => {
    try {
      const snapshots = await api.get('/jobs');
      jobs.clear();
      for (const job of snapshots) jobs.set(job.id, job);
      render();
    } catch {
      // Not critical; the next event will fill it in.
    }
  };

  live.on('jobs', (event) => {
    if (event.type === 'job') jobs.set(event.data.id, event.data);
    if (event.type === 'job_removed') jobs.delete(event.data);
    render();
  });
  live.onResync(refresh);
  await refresh();
}

function jobItem(job) {
  const percent = job.progress === null || job.progress === undefined ? null : Math.round(job.progress * 100);
  const status = job.status === 'running' ? (job.step ?? 'Working…') : statusText(job);
  const progress = h('progress', { class: 'c-progress', max: '100', 'aria-label': job.title });
  if (percent !== null) progress.value = percent; // indeterminate when absent
  return h('li', { class: `c-job-tray__item is-${job.status}` },
    h('div', { class: 'c-job-tray__title' }, job.title),
    h('div', { class: 'c-job-tray__status' }, status),
    job.status === 'running' ? progress : null,
  );
}

function statusText(job) {
  switch (job.status) {
    case 'succeeded': return 'Done';
    case 'cancelled': return 'Cancelled';
    case 'failed': return `Failed: ${job.error ?? 'unknown error'}`;
    default: return job.status;
  }
}
