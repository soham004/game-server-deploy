/**
 * The signed-in user (`GET /api/v1/me`) and permission helpers for showing or hiding UI.
 * The server is the real authority; these checks only keep the UI honest.
 */
import { createStore } from './store.js';

const store = createStore({ me: null });

export const session = {
  get me() {
    return store.get().me;
  },
  setMe(me) {
    store.set({ me });
  },
  subscribe: store.subscribe,

  /** @param {string} permission e.g. "instances.create" */
  canGlobal(permission) {
    return this.me?.global_permissions?.includes(permission) ?? false;
  },

  /** @param {string} permission e.g. "files.write" */
  can(permission, instanceId) {
    const me = this.me;
    if (!me) return false;
    if (me.all_instances) return true;
    const grant = me.instance_grants.find((g) => g.instance_id === instanceId);
    return grant?.permissions.includes(permission) ?? false;
  },
};
