import type { CapabilityKind } from '@/ui';

/**
 * The chat platform the landing draws: what a real project declares, at
 * the granularity a reader can check against the reference docs. The
 * geometry is part of the data because the drawing is hand-laid: three
 * columns on a 1000 x 520 stage, edges routed around them by hand.
 */

/** A declaration in the project, drawn as a box. */
export interface GraphNode {
  id: string;
  /** Top-left corner and width on the 1000 x 520 stage. */
  x: number;
  y: number;
  w: number;
  kind: CapabilityKind;
  /** The word that names the kind in the console. */
  kindLabel: string;
  label: string;
  sub: string;
  /** The line of Lua this box came from. */
  decl: string;
  body: string;
}

/** A call or a stream between two declarations, drawn as a line. */
export interface GraphEdge {
  id: string;
  d: string;
  dashed?: boolean;
  /** Which views light this edge: `live` in all of them. */
  kinds: ('live' | 'msg' | 'ambient' | 'idle')[];
  label: string;
  call: string;
  body: string;
  /** What this edge means while a message is in flight, if anything. */
  msg?: string;
  /** And what it means when nothing is happening. */
  idle?: string;
}

export const NODE_HEIGHT = 44;

export const GRAPH_NODES: GraphNode[] = [
  {
    id: 'assets',
    x: 150,
    y: 78,
    w: 170,
    kind: 'event',
    kindLabel: 'BUNDLE',
    label: 'assets/',
    sub: 'served from the bundle',
    decl: 'assets in the bundle',
    body: 'The client itself. Static files ship inside the same bundle as the script, so there is no second deploy target for the front end.',
  },
  {
    id: 'session',
    x: 150,
    y: 134,
    w: 170,
    kind: 'kv',
    kindLabel: 'CONNECTION',
    label: 'connection "Session"',
    sub: 'one per open socket',
    decl: 'connection "Session" { open, frame, event }',
    body: 'The socket’s program, declared like a class: what to follow when it opens, what to do with a frame the client sends, what to do with an event it is owed. Its identity is minted at upgrade and the client cannot change it.',
  },
  {
    id: 'gateway',
    x: 150,
    y: 190,
    w: 170,
    kind: 'event',
    kindLabel: 'HANDLER',
    label: 'main.lua',
    sub: 'fetch + upgrade',
    decl: 'on "fetch" (handler)',
    body: 'The only public door. It checks who you are, then either answers HTTP or upgrades the socket and hands it a connection program saying what that socket may follow.',
  },
  {
    id: 'cron',
    x: 150,
    y: 300,
    w: 170,
    kind: 'event',
    kindLabel: 'SCHEDULE',
    label: 'on "schedule"',
    sub: 'nightly digest',
    decl: 'on "schedule" (handler)',
    body: 'A schedule declared in the file it runs. It walks the servers once a night and hands the mail off to the queue.',
  },
  {
    id: 'server',
    x: 380,
    y: 40,
    w: 180,
    kind: 'obj',
    kindLabel: 'OBJECT',
    label: 'Server("acme")',
    sub: 'one per community',
    decl: 'object "Server" { … }',
    body: 'Channel list, roles, invites. Every change is one call at a time, so two admins editing roles at once cannot interleave into nonsense.',
  },
  {
    id: 'presence',
    x: 380,
    y: 126,
    w: 180,
    kind: 'obj',
    kindLabel: 'OBJECT',
    label: 'Presence("mira")',
    sub: 'one per member',
    decl: 'object "Presence" { … }',
    body: 'Typing and online state belong to the member instead of a shared table everyone writes to. It publishes a stream the server follows.',
  },
  {
    id: 'general',
    x: 380,
    y: 212,
    w: 180,
    kind: 'obj',
    kindLabel: 'OBJECT',
    label: 'Channel("general")',
    sub: 'busy room',
    decl: 'object "Channel" { … }',
    body: 'A room is one object with one writer, so message order is settled where the room lives. It publishes each message as a stream every follower receives.',
  },
  {
    id: 'dev',
    x: 380,
    y: 298,
    w: 180,
    kind: 'obj',
    kindLabel: 'OBJECT',
    label: 'Channel("dev")',
    sub: 'quiet room',
    decl: 'object "Channel" { … }',
    body: 'The same class under a different name. Nobody is talking, so it is a file in storage costing no memory until someone posts.',
  },
  {
    id: 'thread',
    x: 380,
    y: 384,
    w: 180,
    kind: 'obj',
    kindLabel: 'OBJECT',
    label: 'Thread("#4821")',
    sub: 'lives under a room',
    decl: 'object "Thread" { … }',
    body: 'Threads are objects too, named after the message they hang off. A thread is busy while a room is quiet, and each pays for itself.',
  },
  {
    id: 'report',
    x: 620,
    y: 40,
    w: 200,
    kind: 'obj',
    kindLabel: 'WORKFLOW',
    label: 'workflow "Report"',
    sub: 'runs for days',
    decl: 'workflow "Report" (fn)',
    body: 'A moderation report parks until a human clicks approve, survives deploys, and every step it has taken is readable in the console.',
  },
  {
    id: 'search',
    x: 620,
    y: 126,
    w: 200,
    kind: 'db',
    kindLabel: 'DATABASE',
    label: 'sql "search"',
    sub: 'project database',
    decl: 'sql "search":query(…)',
    body: 'Questions that span rooms need a real database. Each room keeps its private file; this is the shared index beside them.',
  },
  {
    id: 'settings',
    x: 620,
    y: 212,
    w: 200,
    kind: 'kv',
    kindLabel: 'KEY-VALUE',
    label: 'kv "settings"',
    sub: 'small values',
    decl: 'kv "settings":get(key)',
    body: 'Notification preferences, theme, feature flags. Things you look up by name and would rather not model as tables.',
  },
  {
    id: 'pushq',
    x: 620,
    y: 298,
    w: 200,
    kind: 'event',
    kindLabel: 'QUEUE',
    label: 'queue "push"',
    sub: 'off the hot path',
    decl: 'queue "push":send(item)',
    body: 'Mentions leave the request. Retries and backoff are the platform’s problem, and whatever still fails waits in a dead letter queue you can read.',
  },
  {
    id: 'secret',
    x: 620,
    y: 384,
    w: 200,
    kind: 'secret',
    kindLabel: 'SECRET',
    label: 'secret "APNS_KEY"',
    sub: 'versioned',
    decl: 'secret("APNS_KEY"):latest()',
    body: 'The push credential the consumer needs. Rotate it without a deploy, and it never ships inside the bundle.',
  },
];

/** Said the same way at five places, so it is written once. */
const STORAGE_EDGE = {
  label: 'object storage',
  call: 'state.sql',
  body: 'Every instance owns a SQLite file. Loading and flushing happen at the edges of its life, not on every call.',
  idle: 'This is all that is left when the object lets go of its lease.',
};

export const GRAPH_EDGES: GraphEdge[] = [
  {
    id: 'tab1',
    d: 'M32 156 H150',
    kinds: ['live', 'msg'],
    label: 'the open socket',
    call: 'conn:send(value)',
    body: 'The wire itself, held by the connection rather than by a handler. Frames the client sends arrive at its frame handler, and whatever it follows arrives at its event handler.',
    msg: 'One message going up the wire.',
    idle: 'The socket stays open while its vm is gone. That is hibernation.',
  },
  {
    id: 'tab2',
    d: 'M32 200 H150',
    kinds: ['live'],
    label: 'an ordinary request',
    call: 'on "fetch" (handler)',
    body: 'Not every tab holds a socket. A plain request hits the same handler, gets a clean run, and is gone.',
    idle: 'Requests still arrive with nothing loaded. The first one wakes what it touches.',
  },
  {
    id: 'tab3',
    d: 'M32 228 H150',
    kinds: ['live'],
    label: 'an ordinary request',
    call: 'on "fetch" (handler)',
    body: 'Not every tab holds a socket. A plain request hits the same handler, gets a clean run, and is gone.',
    idle: 'Requests still arrive with nothing loaded. The first one wakes what it touches.',
  },
  {
    id: 'gw-session',
    d: 'M236 190 V178',
    kinds: [],
    label: 'the upgrade',
    call: 'request:upgrade(Session, seed, User(name))',
    body: 'A handshake is an ordinary request, so it authenticates in the same router. The upgrade names the connection class, seeds conn.state, and fixes the identity the socket speaks as.',
  },
  {
    id: 'session-general',
    d: 'M320 150 C 350 150 350 224 380 224',
    kinds: ['msg'],
    label: 'connection edge',
    call: 'conn:follow(Channel(room), "message")',
    body: 'Both directions of the socket path. The frame handler posts into the room, and the follow brings the room’s published events back. At-most-once, no retry, and it dies with the socket, so anything that must not be missed is followed one hop back by an object.',
    msg: 'Up: the frame handler posts. Down: the room publishes once and this edge relays it to the wire.',
    idle: 'The follow survives hibernation. Only the vm goes.',
  },
  {
    id: 'assets-gw',
    d: 'M320 104 C 346 122 346 182 320 198',
    dashed: true,
    kinds: [],
    label: 'serving the client',
    call: 'assets served from the bundle',
    body: 'The same handler serves the front end. Static files ride in the bundle with the script, so the client and the code it talks to ship together.',
    idle: 'Static files keep serving whether or not anything is loaded.',
  },
  {
    id: 'gw-server',
    d: 'M320 200 C 350 190 350 62 380 62',
    kinds: ['ambient'],
    label: 'gateway to server',
    call: 'Server(id):channels()',
    body: 'The handler asks the community object for its channel list, roles and invites.',
  },
  {
    id: 'gw-presence',
    d: 'M320 205 C 348 200 348 148 380 148',
    kinds: [],
    label: 'gateway to presence',
    call: 'Presence(user):touch()',
    body: 'Connecting marks the member online inside their own object rather than a shared table.',
  },
  {
    id: 'gw-general',
    d: 'M320 218 C 350 218 350 234 380 234',
    kinds: [],
    label: 'an http post',
    call: 'Channel(name):post(msg)',
    body: 'The path for clients without a socket open. Same call into the room, made from an ordinary request instead of a frame.',
  },
  {
    id: 'gw-dev',
    d: 'M320 228 C 348 240 348 320 380 320',
    kinds: [],
    label: 'gateway to a sleeping room',
    call: 'Channel(name):post(msg)',
    body: 'The same call to a room nobody is using. It wakes where its file is and answers.',
    idle: 'Nothing is loaded behind this call right now.',
  },
  {
    id: 'gw-thread',
    d: 'M320 232 C 344 260 344 406 380 406',
    kinds: [],
    label: 'gateway to thread',
    call: 'Thread(id):post(msg)',
    body: 'Threads take posts directly, so a busy thread never queues behind its room.',
  },
  {
    id: 'cron-server',
    d: 'M320 306 C 352 300 352 90 380 84',
    dashed: true,
    kinds: [],
    label: 'nightly digest',
    call: 'on "schedule" (handler)',
    body: 'The schedule walks each community once a night and hands the mail to the queue.',
  },
  {
    id: 'presence-server',
    d: 'M560 148 C 588 140 588 90 560 84',
    kinds: [],
    label: 'presence stream',
    call: 'state:publish("presence", …)',
    body: 'Presence publishes, the server follows. The member owns the write, so nobody contends for a shared row.',
  },
  {
    id: 'server-general',
    d: 'M560 62 C 596 80 596 216 560 226',
    kinds: [],
    label: 'server to room',
    call: 'Channel(name):configure(…)',
    body: 'Renames, topics and permissions travel from the community object into the room.',
  },
  {
    id: 'server-dev',
    d: 'M560 70 C 604 120 604 310 560 312',
    kinds: [],
    label: 'server to room',
    call: 'Channel(name):configure(…)',
    body: 'Renames, topics and permissions travel from the community object into the room.',
  },
  {
    id: 'thread-general',
    d: 'M560 400 C 600 380 600 250 560 244',
    kinds: [],
    label: 'thread to room',
    call: 'Channel(name):bump(thread)',
    body: 'A thread tells its room it has activity, so the room list can order itself without scanning anything.',
  },
  {
    id: 'server-report',
    d: 'M560 48 C 592 36 592 44 620 50',
    kinds: [],
    label: 'starting a workflow',
    call: 'workflow "Report" (fn)',
    body: 'A report starts a run that parks until a moderator decides, then continues on its own.',
  },
  {
    id: 'push-settings',
    d: 'M620 316 C 596 300 596 250 620 244',
    kinds: [],
    label: 'queue reads prefs',
    call: 'kv "settings":get(user)',
    body: 'Before sending anything the consumer checks whether that member wants the push at all.',
  },
  {
    id: 'general-search',
    d: 'M560 234 C 590 230 590 148 620 148',
    kinds: ['msg', 'ambient'],
    label: 'indexing',
    call: 'sql "search":exec(…)',
    body: 'The room writes a copy into the shared index, which is how search spans rooms without touching their private files.',
    msg: 'This happens after the message is already durable inside the room.',
  },
  {
    id: 'general-push',
    d: 'M560 250 C 592 270 592 316 620 316',
    kinds: ['msg'],
    label: 'mention to queue',
    call: 'queue "push":send(mention)',
    body: 'Mentions leave the request path here.',
    msg: 'The slow part goes to the queue instead of making the sender wait for it.',
  },
  {
    id: 'push-secret',
    d: 'M620 330 C 600 360 600 392 620 400',
    kinds: [],
    label: 'consumer reads the secret',
    call: 'secret "APNS_KEY"',
    body: 'The credential is read at send time. It is versioned, rotatable, and never inside the bundle.',
  },
  {
    id: 'push-apns',
    d: 'M820 320 H870',
    dashed: true,
    kinds: ['msg'],
    label: 'leaving Actias',
    call: 'http to APNs',
    body: 'The one hop that is not yours. What fails comes back as a retry and eventually a dead letter you can read.',
    msg: 'Last hop of the mention path.',
  },
  {
    id: 'st-server',
    d: 'M470 84 V96 H352 V458 H420 V464',
    dashed: true,
    kinds: ['idle'],
    ...STORAGE_EDGE,
  },
  {
    id: 'st-presence',
    d: 'M470 170 V182 H352',
    dashed: true,
    kinds: ['idle'],
    ...STORAGE_EDGE,
  },
  {
    id: 'st-general',
    d: 'M470 256 V268 H352',
    dashed: true,
    kinds: ['idle'],
    ...STORAGE_EDGE,
  },
  {
    id: 'st-dev',
    d: 'M470 342 V354 H352',
    dashed: true,
    kinds: ['idle'],
    ...STORAGE_EDGE,
  },
  {
    id: 'st-thread',
    d: 'M470 428 V440 H352',
    dashed: true,
    kinds: ['idle'],
    ...STORAGE_EDGE,
  },
];

/** One way of reading the same graph. */
export interface GraphView {
  key: 'structure' | 'message' | 'idle';
  label: string;
  short: string;
  hint: string;
  /** Nodes kept at full strength; empty means all of them. */
  focus: string[];
  caption: string;
}

export const GRAPH_VIEWS: GraphView[] = [
  {
    key: 'structure',
    label: 'what it is',
    short: 'the pieces, and the calls between them',
    hint: 'Hover a box or a line. Click either to lock it open.',
    focus: [],
    caption:
      'One project, one repository. Every box is a line somebody wrote, and the lines between them are calls and streams. Hover either to read what it does.',
  },
  {
    key: 'message',
    label: 'a message being sent',
    short: 'one message, socket to socket',
    hint: 'The lit path is one message going out.',
    focus: ['session', 'general', 'search', 'pushq', 'secret'],
    caption:
      'A frame lands on the socket, the connection’s frame handler calls the room, and the room writes to its own file and publishes once. The same connection relays that event back down the wire. Indexing and the mention leave on their own paths.',
  },
  {
    key: 'idle',
    label: 'nothing is happening',
    short: 'no traffic, sockets still open',
    hint: 'The socket outlives the vm.',
    focus: ['gateway'],
    caption:
      'Quiet does not mean empty. Nobody is typing, but the tabs are still here and their sockets are still open. Rooms drop their leases and flush to storage; an idle connection sheds only its vm and keeps the wire, its inbox, conn.state and its follows for about the cost of a file descriptor. The next frame or event rebuilds it.',
  },
];

/** Dots that travel a path while a view is showing traffic. */
export interface GraphPulse {
  path: string;
  tone: CapabilityKind | 'quiet';
  seconds: number;
  delay: number;
}

export const MESSAGE_PULSES: GraphPulse[] = [
  { path: 'M32 156 H150', tone: 'db', seconds: 1.3, delay: 0 },
  {
    path: 'M320 150 C 350 150 350 224 380 224',
    tone: 'db',
    seconds: 1.5,
    delay: 0.3,
  },
  {
    path: 'M560 234 C 590 230 590 148 620 148',
    tone: 'db',
    seconds: 1.9,
    delay: 0.9,
  },
  {
    path: 'M560 250 C 592 270 592 316 620 316',
    tone: 'event',
    seconds: 2.1,
    delay: 1,
  },
  { path: 'M820 320 H870', tone: 'event', seconds: 1.2, delay: 1.8 },
  {
    path: 'M380 224 C 350 224 350 150 320 150',
    tone: 'kv',
    seconds: 1.5,
    delay: 1.1,
  },
  { path: 'M150 156 H32', tone: 'kv', seconds: 1.3, delay: 1.5 },
];

export const AMBIENT_PULSES: GraphPulse[] = [
  {
    path: 'M320 200 C 350 190 350 62 380 62',
    tone: 'quiet',
    seconds: 4.6,
    delay: 0,
  },
  {
    path: 'M560 234 C 590 230 590 148 620 148',
    tone: 'quiet',
    seconds: 5.4,
    delay: 2.2,
  },
];
