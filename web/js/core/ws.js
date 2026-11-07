/**
 * The live event stream (one WebSocket per tab).
 *
 *   const off = live.on('instances', (event) => { ... });   // subscribe to a topic
 *   off();                                                   // unsubscribe
 *
 * Topics: "instances", "jobs", "console:<instance id>". Subscriptions are reference-counted
 * and re-sent after a reconnect. On reconnect or a server "resync", `onResync` listeners run so
 * views can refetch over REST (events may have been missed).
 */

const RECONNECT_MIN_MS = 1000;
const RECONNECT_MAX_MS = 15000;

const listeners = new Map(); // topic -> Set<fn>
const resyncListeners = new Set();
let socket = null;
let reconnectDelay = RECONNECT_MIN_MS;
let wasConnected = false;

function url() {
  const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  return `${scheme}//${window.location.host}/api/v1/ws`;
}

function connect() {
  socket = new WebSocket(url());

  socket.addEventListener('open', () => {
    reconnectDelay = RECONNECT_MIN_MS;
    send({ op: 'subscribe', topics: [...listeners.keys()] });
    if (wasConnected) notifyResync();
    wasConnected = true;
  });

  socket.addEventListener('message', (message) => {
    let event;
    try {
      event = JSON.parse(message.data);
    } catch {
      return;
    }
    if (event.type === 'resync') {
      notifyResync();
      return;
    }
    for (const listener of listeners.get(event.topic) ?? []) listener(event);
  });

  socket.addEventListener('close', () => {
    socket = null;
    window.setTimeout(connect, reconnectDelay);
    reconnectDelay = Math.min(reconnectDelay * 2, RECONNECT_MAX_MS);
  });
}

function send(message) {
  if (socket?.readyState === WebSocket.OPEN && (message.topics?.length ?? 1) > 0) {
    socket.send(JSON.stringify(message));
  }
}

function notifyResync() {
  for (const listener of resyncListeners) listener();
}

export const live = {
  /** Starts the connection (call once after sign-in). */
  start() {
    if (!socket) connect();
  },

  /**
   * @param {string} topic
   * @param {(event: {topic: string, type: string, instance_id?: string, data: any}) => void} listener
   * @returns {() => void} unsubscribe
   */
  on(topic, listener) {
    if (!listeners.has(topic)) {
      listeners.set(topic, new Set());
      send({ op: 'subscribe', topics: [topic] });
    }
    listeners.get(topic).add(listener);
    return () => {
      const set = listeners.get(topic);
      if (!set) return;
      set.delete(listener);
      if (set.size === 0) {
        listeners.delete(topic);
        send({ op: 'unsubscribe', topics: [topic] });
      }
    };
  },

  /** Called after a reconnect or dropped events; refetch state over REST. */
  onResync(listener) {
    resyncListeners.add(listener);
    return () => resyncListeners.delete(listener);
  },
};
