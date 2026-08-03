// Демонстрационный режим для скриншотов документации.
//
// Подменяет мост в ядро заранее заготовленными данными и поднимает **те же**
// компоненты, что и настоящее приложение. Это честные снимки интерфейса, просто
// переписка в них вымышленная.
//
// В сборку приложения не попадает: точка входа только у demo.html.

type Args = Record<string, unknown>;

const id = (seed: string): string => (seed + 'x'.repeat(52)).slice(0, 52);

const ME = id('kaneu');
const SPACE = id('orbita');
const CHANNEL = id('obshiy');

const MEMBERS = [
  { id: id('marina'), nick: 'марина ковач', avatar: null, dh: id('dhm'), online: true, last_seen: 0 },
  { id: id('timur'), nick: 'тимур лаас', avatar: null, dh: id('dht'), online: true, last_seen: 0 },
  { id: id('anya'), nick: 'аня со', avatar: null, dh: id('dha'), online: true, last_seen: 0 },
  { id: ME, nick: 'kaneu', avatar: null, dh: id('dhk'), online: true, last_seen: 0 },
  { id: id('vlad'), nick: 'влад пе', avatar: null, dh: null, online: false, last_seen: Date.now() - 7_200_000 },
  { id: id('yulia'), nick: 'юля ким', avatar: null, dh: null, online: false, last_seen: Date.now() - 86_400_000 },
];

const CHANNELS = [
  { id: CHANNEL, space: SPACE, name: 'общий-канал', category: 'общее', voice: false, unread: 0 },
  { id: id('links'), space: SPACE, name: 'ссылки', category: 'общее', voice: false, unread: 0 },
  { id: id('memes'), space: SPACE, name: 'мемы', category: 'общее', voice: false, unread: 12 },
  { id: id('render'), space: SPACE, name: 'рендерер', category: 'разработка', voice: false, unread: 0 },
  { id: id('bugs'), space: SPACE, name: 'баги', category: 'разработка', voice: false, unread: 3 },
  { id: id('standup'), space: SPACE, name: 'стендап', category: 'голос', voice: true, unread: 0 },
];

const base = Date.now() - 3_600_000;
const message = (
  seed: string,
  author: number,
  body: string,
  extra: Partial<Record<string, unknown>> = {},
) => ({
  id: id(seed),
  channel: CHANNEL,
  author: MEMBERS[author].id,
  nick: MEMBERS[author].nick,
  body,
  ts: base,
  lamport: 1,
  edited: false,
  deleted: false,
  reply_to: null,
  reply_preview: null,
  thread: null,
  thread_replies: 0,
  reactions: [],
  attachments: [],
  ...extra,
});

const MESSAGES = [
  message('m1', 0, 'Ребят, кто трогал новый рендерер? На 4К он ест 40% CPU в простое — вентилятор взлетает буквально от пустого окна.', { ts: base }),
  message('m2', 1, 'Воспроизводится. Тикер не засыпает при потере фокуса — крутит кадры в фон.\n```\nwindow.addEventListener(\'blur\',  () => ticker.pause())\nwindow.addEventListener(\'focus\', () => ticker.resume())\n```', { ts: base + 90_000, lamport: 2 }),
  message('m3', 0, 'То есть это однострочник? Го в отдельную ветку — я соберу и померяю на своём железе.', {
    ts: base + 180_000,
    lamport: 3,
    reply_to: id('m2'),
    reply_preview: { author: MEMBERS[1].id, nick: 'тимур лаас', body: 'Тикер не засыпает при потере фокуса…' },
    reactions: [
      { emoji: '🔥', count: 4, mine: true },
      { emoji: '👀', count: 2, mine: false },
    ],
    thread_replies: 7,
  }),
  message('m4', 2, 'Скрин профайлера до фикса, чтобы было с чем сравнивать:', {
    ts: base + 540_000,
    lamport: 4,
    attachments: [
      { hash: id('shot'), name: 'profiler-idle.png', size: 421_888, mime: 'image/png', local: true },
    ],
  }),
  message('m5', 3, 'Собрал с фиксом — в простое ноль процентов. Заливаю в релизы.', { ts: base + 700_000, lamport: 5 }),
];

const SPACES = [
  { id: SPACE, name: 'Орбита', unread: 15, direct: null },
  { id: id('design'), name: 'Дизайн-цех', unread: 0, direct: null },
  { id: id('dmtimur'), name: 'тимур лаас', unread: 2, direct: MEMBERS[1].id },
];

const HANDLERS: Record<string, (args: Args) => unknown> = {
  bootstrap: () => ({
    me: ME,
    nick: 'kaneu',
    endpoint: 'f6658d724d2988119ff99fe1b68531e9a00d44bc42238cec7646986ddbcda0e7',
    spaces: SPACES,
  }),
  net_status: () => ({ endpoint: 'f6658d724d2988…', online: true, spaces: 2 }),
  list_spaces: () => SPACES,
  list_channels: () => CHANNELS,
  list_messages: (args) => (args.before ? [] : MESSAGES),
  list_thread: () => [],
  list_members: () => MEMBERS,
  list_emojis: () => [],
  voice_map: () => [
    [MEMBERS[0].id, id('standup')],
    [MEMBERS[1].id, id('standup')],
  ],
  call_state: () => ({
    space: SPACE,
    channel: id('standup'),
    participants: [ME, MEMBERS[0].id, MEMBERS[1].id],
  }),
  get_message: () => null,
  mark_read: () => null,
  join_call: () => null,
  leave_call: () => null,
  typing: () => null,
  media_stream: () => null,
  send_media: () => null,
  collect_garbage: () => 0,
  attachment_url: () => fakeScreenshotUrl(),
  personal_link: () => 'bred://hello/nfxwc3tjnzxxg5dbmvsa…',
  space_invite: () => 'bred://join/nfxwc3tjnzxxg5dbmvsa…',
  'plugin:app|version': () => '0.1.5',
  'plugin:event|listen': () => 1,
  'plugin:event|unlisten': () => null,
  'plugin:notification|is_permission_granted': () => false,
  'plugin:notification|request_permission': () => 'denied',
};


let cachedShot: string | null = null;

/** Рисуем правдоподобный «скрин профайлера», чтобы превью было настоящим. */
function fakeScreenshotUrl(): string {
  cachedShot ??= fakeScreenshot();
  return cachedShot;
}

function fakeScreenshot(): string {
  const canvas = document.createElement('canvas');
  canvas.width = 720;
  canvas.height = 300;
  const ctx = canvas.getContext('2d')!;

  ctx.fillStyle = '#0d0d0d';
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.strokeStyle = '#242424';
  for (let y = 40; y < 300; y += 40) {
    ctx.beginPath();
    ctx.moveTo(0, y);
    ctx.lineTo(720, y);
    ctx.stroke();
  }

  ctx.fillStyle = '#e9e9e9';
  for (let x = 0; x < 720; x += 9) {
    const h = 40 + Math.abs(Math.sin(x / 46)) * 170 + (x % 27);
    ctx.fillRect(x, 300 - h, 6, h);
  }

  ctx.fillStyle = '#8b8b8b';
  ctx.font = '14px monospace';
  ctx.fillText('CPU · idle · 4K', 16, 26);

  return canvas.toDataURL('image/png');
}

// Мост в ядро: те же имена команд, что и в настоящем приложении.
(window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
  invoke: async (cmd: string, args: Args = {}) => HANDLERS[cmd]?.(args) ?? null,
  transformCallback: (callback: (payload: unknown) => void) => {
    const name = `_demo_${Math.random().toString(36).slice(2)}`;
    (window as unknown as Record<string, unknown>)[name] = callback;
    return name;
  },
};

void import('./main').then(async () => {
  // Режим снимка задаётся якорем: #call — панель звонка, иначе обычная лента.
  if (location.hash === '#call') {
    const { session } = await import('./lib/stores/session.svelte');
    await new Promise((r) => setTimeout(r, 600));
    const voice = session.channels.find((c) => c.voice);
    if (voice) await session.joinVoice(voice.id, true);
  }
});
