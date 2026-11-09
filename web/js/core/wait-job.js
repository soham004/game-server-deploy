/**
 * Follows a job until it ends, for a page that needs its outcome before going on.
 *
 *   const job = await finishedJob(jobId, () => element.isConnected);
 *   if (!job) return;   // the page is gone
 */
import { api } from './api.js';

const POLL_MS = 1000;

/** The finished job, or null once `stillWanted()` says nobody is waiting any more. */
export async function finishedJob(jobId, stillWanted) {
  while (stillWanted()) {
    const job = await api.get(`/jobs/${jobId}`);
    if (job.status !== 'running') return job;
    await new Promise((resolve) => { setTimeout(resolve, POLL_MS); });
  }
  return null;
}
