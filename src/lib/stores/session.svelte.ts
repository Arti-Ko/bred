// Состояние сессии. Один объект на приложение: панелей мало, а связей между
// ними много, и таскать пропсы через четыре уровня было бы дороже.

import {
  ask as askDialog,
  open as openFileDialog,
  save as saveFileDialog,
} from '@tauri-apps/plugin-dialog';
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from '@tauri-apps/plugin-notification';

import {
  api,
  errorText,
  onNotice,
  type Attachment,
  type ChannelRow,
  type Id,
  type MemberRow,
  type EmojiRow,
  type MessageRow,
  type Notice,
  type SpaceRow,
} from '../ipc';
import { call } from './call.svelte';
import { forgetPreviews, previewUrl, shouldAutoFetch } from '../previews';
import { forgetPrefetched, prefetchAll } from '../prefetch';
import { chime } from '../chime';
import { validEmojiName } from '../format';
import { news } from './news.svelte';
import { expiresText, governance, usesText } from './governance.svelte';
import { report } from './report.svelte';
import { prefs } from './prefs.svelte';
import { updates } from './updates.svelte';

/** Сколько живёт отметка «печатает…» без подтверждения. */
const TYPING_TTL = 4000;
/** Окно склейки обновлений: при досинхронизации событий прилетают сотни. */
const REFRESH_WINDOW = 60;
/** Должно совпадать с `app::PAGE` в ядре. */
const PAGE = 200;

export class Session {
  me = $state<Id>('');
  nick = $state('');
  endpoint = $state('');
  online = $state(false);
  /** Ретранслятор, внешний адрес и соседи — для раздела «сеть» в настройках. */
  relay = $state<string | null>(null);
  external = $state<string | null>(null);
  neighbors = $state(0);

  spaces = $state<SpaceRow[]>([]);
  channels = $state<ChannelRow[]>([]);
  messages = $state<MessageRow[]>([]);
  members = $state<MemberRow[]>([]);

  spaceId = $state<Id | null>(null);
  channelId = $state<Id | null>(null);

  /** Кто сейчас печатает в открытом канале. */
  typing = $state<string[]>([]);
  /** Последняя строка для статус-бара: ошибки и подтверждения. */
  status = $state('');
  /** Сообщение, на которое отвечаем. */
  replyTo = $state<MessageRow | null>(null);
  /** Файлы, прикреплённые к черновику и ещё не отправленные. */
  pending = $state<Attachment[]>([]);
  /** Кто в каком голосовом канале: [участник, канал]. */
  voice = $state<Array<[Id, Id]>>([]);
  /** Свои эмодзи и стикеры текущего пространства. */
  emojis = $state<EmojiRow[]>([]);
  /**
   * Картинки своих эмодзи по имени — одни на всё приложение.
   *
   * Раньше их собирал каждый компонент сам: каждое сообщение ленты и каждый
   * выбор эмодзи заново просили адрес на всё содержимое набора, а картинка,
   * не приехавшая с первых трёх попыток, не приезжала уже никогда.
   */
  glyphs = $state<Record<string, string>>({});
  /** Для какой картинки получен адрес: сменили картинку под тем же именем — тянем заново. */
  #glyphHash = new Map<string, Id>();
  /** Открытая ветка: корень и все ответы. */
  thread = $state<MessageRow[]>([]);
  threadRoot = $state<Id | null>(null);
  /** Есть ли что подгружать выше по ленте. */
  hasOlder = $state(false);
  loadingOlder = $state(false);

  /** Локальные системные строки: результаты команд прямо в ленте. */
  notes = $state<Array<{ id: number; channel: Id | null; text: string; ts: number }>>([]);
  #noteSeq = 0;
  #canNotify = false;
  #typingSeen = new Map<Id, { nick: string; at: number }>();
  #refreshTimer: number | null = null;
  #typingTimer: number | null = null;

  get channel(): ChannelRow | null {
    return this.channels.find((c) => c.id === this.channelId) ?? null;
  }

  get space(): SpaceRow | null {
    return this.spaces.find((s) => s.id === this.spaceId) ?? null;
  }

  /** Обычные пространства и личные переписки показываются раздельно. */
  get rooms(): SpaceRow[] {
    return this.spaces.filter((s) => !s.direct);
  }

  get directs(): SpaceRow[] {
    return this.spaces.filter((s) => s.direct);
  }

  /** Быстрый доступ к своим эмодзи по имени — для разбора `:имя:`. */
  get emojiMap(): Record<string, EmojiRow> {
    return Object.fromEntries(this.emojis.map((e) => [e.name, e]));
  }

  /** Открыть личную переписку с участником. */
  async openDirect(peer: Id): Promise<void> {
    if (peer === this.me) return;
    try {
      const space = await api.openDirect(peer);
      this.spaces = await api.listSpaces();
      await this.selectSpace(space);
    } catch (error) {
      this.status = errorText(error);
    }
  }

  /** Добавить свой эмодзи или стикер. */
  async addEmoji(name: string, sticker: boolean): Promise<void> {
    if (!this.spaceId) {
      this.status = 'сначала выберите пространство';
      return;
    }
    name = name.trim().replace(/^:+|:+$/g, '');
    if (!name) {
      this.status = sticker ? 'нужно имя: /стикер котик' : 'нужно имя: /эмодзи паррот';
      return;
    }
    // Проверяем до выбора файла: отказ ядра после диалога выглядел бы как
    // «выбрал картинку — ничего не произошло».
    if (!validEmojiName(name)) {
      this.status = 'в имени — только буквы, цифры, дефис и подчёркивание, до 32 знаков';
      return;
    }
    try {
      const picked = await openFileDialog({
        multiple: false,
        title: sticker ? 'Картинка стикера' : 'Картинка эмодзи',
        filters: [{ name: 'Картинки', extensions: ['png', 'gif', 'webp', 'jpg', 'jpeg'] }],
      });
      if (!picked) return;
      await api.addEmoji(this.spaceId, name, Array.isArray(picked) ? picked[0] : picked, sticker);
      await this.#loadEmojis();
      this.#note(
        sticker
          ? `стикер :${name}: добавлен — он в выборе эмодзи, во вкладке «стикеры»`
          : `добавлено :${name}: — вставляйте прямо в текст`,
      );
    } catch (error) {
      this.status = errorText(error);
    }
  }

  async #loadEmojis(): Promise<void> {
    if (!this.spaceId) return;
    const space = this.spaceId;
    this.emojis = await api.listEmojis(space).catch(() => []);
    // Картинки эмодзи нужны сразу: без них в тексте будут голые `:имена:`.
    // Но ждать их здесь нельзя — список уже готов, и рисовать можно прямо
    // сейчас. Раньше этот цикл ждал каждый файл по очереди и повторялся на
    // каждой пачке событий, из-за чего лента замирала на ровном месте.
    prefetchAll(
      space,
      this.emojis.map((e) => e.hash),
    );
    // Адреса — в общую карту. Не приехавшее спрашивается снова на следующей
    // пачке событий: автор стикера мог быть не в сети именно в ту минуту.
    for (const emoji of this.emojis) {
      if (this.glyphs[emoji.name] && this.#glyphHash.get(emoji.name) === emoji.hash) continue;
      void previewUrl(emoji.hash, space).then((url) => {
        if (!url || this.spaceId !== space) return;
        this.#glyphHash.set(emoji.name, emoji.hash);
        this.glyphs = { ...this.glyphs, [emoji.name]: url };
      });
    }
  }

  /**
   * Отправить стикер сразу, отдельным сообщением — как в любом мессенджере.
   *
   * Раньше выбор стикера только дописывал `:имя:` в черновик, и его надо было
   * ещё отправить. Черновик и прикреплённые файлы стикер не трогает.
   */
  async sendSticker(name: string): Promise<void> {
    if (!this.spaceId || !this.channelId) return;
    try {
      await api.sendMessage(
        this.spaceId,
        this.channelId,
        `:${name}:`,
        this.replyTo?.id ?? null,
        this.threadRoot,
        [],
      );
      this.replyTo = null;
    } catch (error) {
      this.status = errorText(error);
    }
  }

  /** Ключи тех, кто сидит в голосовом канале. Имя по ключу найдёт тот, кому оно нужно. */
  voiceIds(channel: Id): Id[] {
    return this.voice.filter(([, room]) => room === channel).map(([who]) => who);
  }

  /** Кто сидит в конкретном голосовом канале — именами, для терминала. */
  voiceMembers(channel: Id): string[] {
    return this.voice
      .filter(([, room]) => room === channel)
      .map(([who]) => this.members.find((m) => m.id === who)?.nick ?? who.slice(0, 8));
  }

  /**
   * Что со связью — человеческими словами.
   *
   * Раньше состояний было два: «сеть: p2p» и «поднимается», и второе означало
   * что угодно — от «ещё пара секунд» до «ваш туннель съел разрешение имён, и
   * никто никогда не появится». Человек должен видеть, что именно сломано, и
   * понимать, чинить это ему или ждать.
   */
  get link(): { text: string; trouble: boolean } {
    if (this.neighbors > 0) {
      return { text: `на связи · соседей ${this.neighbors}`, trouble: false };
    }
    if (!this.online) {
      return { text: 'нет выхода наружу — работает только локальная сеть', trouble: true };
    }
    if (!this.relay && !this.external) {
      return { text: 'не вижу ретранслятор — проверьте сеть', trouble: true };
    }
    return { text: 'ищем соседей', trouble: false };
  }

  get unreadTotal(): number {
    return this.channels.reduce((sum, c) => sum + c.unread, 0);
  }

  /** Каналы, сгруппированные по категориям, в порядке появления. */
  get grouped(): Array<{ category: string; items: ChannelRow[] }> {
    const order: string[] = [];
    const map = new Map<string, ChannelRow[]>();
    for (const channel of this.channels) {
      const key = channel.voice ? 'голос' : channel.category || 'общее';
      if (!map.has(key)) {
        map.set(key, []);
        order.push(key);
      }
      map.get(key)!.push(channel);
    }
    return order.map((category) => ({ category, items: map.get(category)! }));
  }

  /** Разрешение на уведомления спрашиваем один раз при старте. */
  async #setupNotifications(): Promise<void> {
    try {
      this.#canNotify = await isPermissionGranted();
      if (!this.#canNotify) {
        this.#canNotify = (await requestPermission()) === 'granted';
      }
    } catch {
      this.#canNotify = false;
    }
  }

  /**
   * Чужое сообщение — повод дёрнуть человека.
   *
   * Два разных сигнала с разными условиями. Звук нужен и тогда, когда окно на
   * виду: человек читает один канал, а написали в другой — раньше он узнавал об
   * этом, только случайно туда заглянув. Молчим лишь про тот канал, который
   * прямо сейчас открыт и виден. Уведомление операционной системы, наоборот,
   * только из фона: показывать его поверх открытого окна незачем.
   *
   * Разрешения тоже разные. Звук не спрашивает ничего, поэтому он больше не
   * привязан к тому, дал ли человек доступ к уведомлениям системы — раньше
   * отказ там означал полную тишину.
   */
  async #notify(event: Id): Promise<void> {
    const background = !document.hasFocus();
    if (!background && !prefs.soundOnMessage) return;
    try {
      const message = await api.getMessage(event);
      if (!message || message.author === this.me || message.deleted) return;

      if (prefs.soundOnMessage && (background || message.channel !== this.channelId)) {
        chime();
      }

      if (!this.#canNotify || !background) return;
      const channel = this.channels.find((c) => c.id === message.channel);
      sendNotification({
        title: channel ? `#${channel.name} · ${message.nick}` : message.nick,
        body: message.body.slice(0, 160) || 'вложение',
      });
    } catch {
      // Уведомление — приятный бонус, ронять из-за него ничего не будем.
    }
  }

  /**
   * Показать результат действия серой строкой в ленте.
   *
   * Статус-строку внизу человек не читает — она узкая и меняется молча.
   * Ответ на команду должен появляться там же, где он её набрал.
   */
  #note(text: string): void {
    this.notes = [
      ...this.notes,
      { id: ++this.#noteSeq, channel: this.channelId, text, ts: Date.now() },
    ].slice(-30);
    this.status = text;
  }

  /** Системные строки текущего канала. */
  get visibleNotes(): Array<{ id: number; text: string; ts: number }> {
    return this.notes.filter((n) => n.channel === null || n.channel === this.channelId);
  }

  /** Подставить команду в поле ввода — по клику на подсказку. */
  suggest = $state('');

  async init(): Promise<void> {
    try {
      const boot = await api.bootstrap();
      this.me = boot.me;
      this.nick = boot.nick;
      this.endpoint = boot.endpoint;
      this.spaces = boot.spaces;

      await onNotice((notice) => this.#onNotice(notice));
      void this.#setupNotifications();
      void this.#pollNet();

      if (boot.spaces.length > 0) {
        await this.selectSpace(boot.spaces[0].id);
      }
    } catch (error) {
      this.status = errorText(error);
    }
  }

  async selectSpace(space: Id): Promise<void> {
    news.hide();
    this.spaceId = space;
    this.channelId = null;
    this.messages = [];
    // У другого пространства другой набор: одноимённый эмодзи там — другая картинка.
    this.glyphs = {};
    this.#glyphHash.clear();
    await this.#loadChannels();
    await this.#loadMembers();
    await this.#loadVoice();
    await this.#loadEmojis();
    void governance.load(space);

    const first = this.channels.find((c) => !c.voice);
    if (first) await this.selectChannel(first.id);
  }

  async selectChannel(channel: Id): Promise<void> {
    news.hide();
    this.channelId = channel;
    this.replyTo = null;
    this.closeThread();
    await this.#loadMessages();
    await api.markRead(channel).catch(() => undefined);
    await this.#loadChannels();
  }

  async send(body: string): Promise<void> {
    const text = body.trim();

    // Команды разбираются до всех проверок. Иначе на свежей установке, где
    // пространств ещё нет, не работает даже `/простор` — то есть единственный
    // способ завести первое. Ровно в эту дыру всё и упиралось.
    if (text.startsWith('/')) {
      await this.#runCommand(text);
      return;
    }

    // Открыт канал обновлений — туда писать нельзя, а молча отправить текст в
    // канал, которого не видно, хуже, чем отказать вслух.
    if (news.open) {
      this.#note('в канал «обновления» писать нельзя — это канал разработчика');
      return;
    }

    if (!this.spaceId || !this.channelId) {
      this.#note('сначала создайте пространство: /простор Название');
      return;
    }
    if (!text && this.pending.length === 0) return;

    try {
      await api.sendMessage(
        this.spaceId,
        this.channelId,
        text,
        this.replyTo?.id ?? null,
        // Открытая ветка перехватывает ввод: писать «в канал», глядя в ветку,
        // почти всегда не то, чего человек хотел.
        this.threadRoot,
        this.pending,
      );
      this.replyTo = null;
      this.pending = [];
      this.status = '';
    } catch (error) {
      this.status = errorText(error);
    }
  }

  /** Поставить картинку профиля. Годится и анимированный GIF. */
  async setAvatar(): Promise<void> {
    try {
      const picked = await openFileDialog({
        multiple: false,
        title: 'Картинка профиля',
        filters: [{ name: 'Картинки', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp'] }],
      });
      if (!picked) return;
      await api.setAvatar(Array.isArray(picked) ? picked[0] : picked);
      await this.#loadMembers();
      this.#note('аватар обновлён');
    } catch (error) {
      this.status = errorText(error);
    }
  }

  /** Скачиваем чужие аватары сами: иначе в списке будут пустые квадраты. */
  #prefetchAvatars(): void {
    if (!this.spaceId) return;
    prefetchAll(
      this.spaceId,
      this.members.flatMap((m) => (m.avatar ? [m.avatar] : [])),
    );
  }

  /** Выбрать файлы и подготовить их к отправке. */
  async attach(): Promise<void> {
    if (!this.spaceId) {
      this.status = 'сначала выберите пространство';
      return;
    }
    try {
      const picked = await openFileDialog({ multiple: true, title: 'Что отправить' });
      if (!picked) return;
      const paths = Array.isArray(picked) ? picked : [picked];

      const prepared: Attachment[] = [];
      for (const path of paths) {
        prepared.push(await api.attachFile(path));
      }
      this.pending = [...this.pending, ...prepared];
      this.#note(`прикреплено: ${prepared.map((a) => a.name).join(', ')}`);
    } catch (error) {
      this.status = errorText(error);
    }
  }

  dropAttachment(hash: Id): void {
    this.pending = this.pending.filter((a) => a.hash !== hash);
  }

  /** Скачать вложение у того, у кого оно есть. */
  async download(hash: Id): Promise<void> {
    if (!this.spaceId) return;
    this.status = 'качаем…';
    try {
      const path = await api.downloadAttachment(this.spaceId, hash);
      this.#note(`сохранено: ${path}`);
      await this.#loadMessages();
    } catch (error) {
      this.status = errorText(error);
    }
  }

  /**
   * Сохранить вложение к себе.
   *
   * Скачивание кладёт файл в служебное хранилище — человеку он нужен там, где
   * он сам укажет, иначе «скачал» превращается в путь внутри Library.
   */
  async saveAttachment(hash: Id, name: string): Promise<void> {
    if (!this.spaceId) return;
    try {
      // Если байтов ещё нет — сперва тянем их у того, у кого они есть.
      await api.downloadAttachment(this.spaceId, hash);

      const target = await saveFileDialog({ defaultPath: name, title: 'Куда сохранить' });
      if (!target) return;

      await api.saveAttachment(hash, target);
      await this.#loadMessages();
      this.#note(`сохранено: ${target}`);
    } catch (error) {
      this.#note(errorText(error));
    }
  }

  /** Удалить своё сообщение. Чужие удалить нельзя — подпись не сойдётся. */
  async deleteMessage(target: Id): Promise<void> {
    if (!this.spaceId) return;
    try {
      await api.deleteMessage(this.spaceId, target);
      await this.#loadMessages();
      this.#note('сообщение удалено');
    } catch (error) {
      this.#note(errorText(error));
    }
  }

  /**
   * Удалить канал вместе со всей перепиской в нём — у всех участников.
   * Спрашиваем подтверждение: отменить это нельзя.
   */
  async deleteChannel(channel: Id, name: string): Promise<void> {
    if (!this.spaceId) return;
    try {
      const yes = await askDialog(
        `Удалить канал «${name}» и всё, что в нём написано? У всех участников.`,
        { title: 'Удаление канала', kind: 'warning' },
      );
      if (!yes) return;

      await api.deleteChannel(this.spaceId, channel);
      if (this.channelId === channel) {
        this.channelId = null;
        this.messages = [];
      }
      await this.#loadChannels();
      const first = this.channels.find((c) => !c.voice);
      if (first && !this.channelId) await this.selectChannel(first.id);
      this.#note(`канал «${name}» удалён`);
    } catch (error) {
      this.#note(errorText(error));
    }
  }

  /** Войти в голосовой канал. Он же комната звонка. */
  async joinVoice(channel: Id, withVideo = false): Promise<void> {
    if (!this.spaceId) return;
    if (call.active) await call.leave();
    await call.join(this.spaceId, channel, withVideo);
    if (call.status) this.status = call.status;
    await this.#loadVoice();
  }

  async toggleReaction(message: MessageRow, emoji: string): Promise<void> {
    if (!this.spaceId) return;
    const mine = message.reactions.some((r) => r.emoji === emoji && r.mine);
    try {
      await api.react(this.spaceId, message.id, emoji, mine);
    } catch (error) {
      this.status = errorText(error);
    }
  }

  async invite(): Promise<void> {
    if (!this.spaceId) return;
    try {
      const ticket = await api.spaceInvite(this.spaceId);
      await navigator.clipboard.writeText(ticket);
      this.#note(
        'ссылка-приглашение скопирована: действует 7 дней, ключа в ней нет — ' +
          'впустит любой участник в сети; погасить — /приглашения',
      );
    } catch (error) {
      this.#note(errorText(error));
    }
  }

  /** Строка результата в ленте — для действий из других хранилищ. */
  say(text: string): void {
    this.#note(text);
  }

  async reloadMembers(): Promise<void> {
    await this.#loadMembers();
  }

  /** Пространства больше нет у нас — исключили или вышли. */
  async dropSpace(space: Id, message: string): Promise<void> {
    this.spaces = await api.listSpaces();
    if (this.spaceId === space) {
      this.spaceId = null;
      this.channelId = null;
      this.messages = [];
      this.channels = [];
      this.members = [];
      if (this.spaces.length > 0) await this.selectSpace(this.spaces[0].id);
      else void governance.load(null);
    }
    this.#note(message);
  }

  /** Участник по имени или началу ключа — для команд. */
  #member(argument: string): MemberRow {
    const who = this.members.find((m) => m.nick === argument || m.id.startsWith(argument));
    if (!who) throw new Error(`не нашёл участника «${argument}»`);
    return who;
  }

  notifyTyping(): void {
    if (this.spaceId && this.channelId) {
      void api.typing(this.spaceId, this.channelId).catch(() => undefined);
    }
  }

  // ── команды ───────────────────────────────────────────────────────────────

  // ── действия ──────────────────────────────────────────────────────────────
  //
  // Одни и те же для кнопок и для команд: интерфейс в адекватном режиме зовёт
  // их напрямую, `#runCommand` — тоже. Двух путей к одному действию быть не
  // должно, иначе они разойдутся на первой же правке.

  async createSpace(name: string): Promise<void> {
    const title = name.trim();
    if (!title) throw new Error('нужно название пространства');
    await this.#refreshSpaces(await api.createSpace(title));
    this.#note(`пространство «${title}» создано`);
  }

  async createChannel(name: string, category: string, voice: boolean): Promise<void> {
    if (!this.spaceId) throw new Error('сначала выберите пространство');
    const title = name.trim();
    if (!title) throw new Error('нужно название канала');
    await api.createChannel(this.spaceId, title, category.trim() || (voice ? 'голос' : 'общее'), voice);
    await this.#loadChannels();
    this.#note(voice ? `голосовой канал «${title}» создан` : `канал #${title} создан`);
  }

  /** Вход по любой ссылке: приглашение в пространство или чужая визитка. */
  async joinByLink(link: string): Promise<void> {
    const ticket = link.trim();
    if (!ticket) throw new Error('нужна ссылка вида bred://…');
    const space = ticket.includes('bred://hello/')
      ? await api.openDirectLink(ticket)
      : await api.joinSpace(ticket);
    await this.#refreshSpaces(space);
    this.#note('подключено');
  }

  async rename(name: string): Promise<void> {
    const next = name.trim();
    if (!next) throw new Error('имя не может быть пустым');
    const was = this.nick;
    await api.setNick(next);
    this.nick = next;
    await this.#loadMembers();
    this.#note(`имя изменено: ${was} → ${next}`);
  }

  async leaveSpace(): Promise<void> {
    if (!this.spaceId) throw new Error('пространство не выбрано');
    const name = this.space?.name ?? '';
    await api.leaveSpace(this.spaceId);
    this.spaces = await api.listSpaces();
    this.spaceId = null;
    this.channelId = null;
    this.messages = [];
    this.channels = [];
    this.members = [];
    forgetPreviews();
    forgetPrefetched();
    await api.collectGarbage().catch(() => undefined);
    if (this.spaces.length > 0) await this.selectSpace(this.spaces[0].id);
    this.#note(`вы вышли из «${name}», история стёрта`);
  }

  async #runCommand(line: string): Promise<void> {
    const [command, ...rest] = line.slice(1).split(' ');
    const argument = rest.join(' ').trim();
    try {
      switch (command) {
        case 'простор':
        case 'space': {
          if (!argument) throw new Error('нужно название: /простор Орбита');
          await this.createSpace(argument);
          break;
        }
        case 'канал':
        case 'channel': {
          if (!argument) throw new Error('нужно название: /канал баги');
          await this.createChannel(argument, 'общее', false);
          break;
        }
        case 'голос':
        case 'voice': {
          if (!argument) throw new Error('нужно название: /голос стендап');
          await this.createChannel(argument, 'голос', true);
          break;
        }
        case 'звонок':
        case 'call': {
          const room = this.channels.find((c) => c.voice && c.name === argument);
          if (!room) throw new Error(`голосового канала «${argument}» нет`);
          await this.joinVoice(room.id, false);
          this.#note(`вы в звонке «${argument}»`);
          break;
        }
        case 'войти':
        case 'join': {
          if (!argument) throw new Error('нужна ссылка: /войти bred://…');
          // Одна команда на обе ссылки: человеку незачем помнить, какая из них
          // на пространство, а какая на личную переписку.
          await this.joinByLink(argument);
          break;
        }
        case 'имя':
        case 'nick': {
          if (!argument) throw new Error('нужно имя: /имя тимур');
          await this.rename(argument);
          break;
        }
        case 'позвать':
        case 'invite':
          // «/позвать всех» из комнаты — звонок друзьям; просто «/позвать» —
          // по-прежнему ссылка-приглашение в пространство.
          if (/^(всех|all)$/i.test(argument) && call.active) {
            this.#note(await call.ring());
          } else {
            await this.invite();
          }
          break;
        case 'визитка':
        case 'me':
          await this.copyPersonalLink();
          break;
        case 'лс':
        case 'dm': {
          const who = this.#member(argument);
          await this.openDirect(who.id);
          this.#note(`открыта переписка с ${who.nick}`);
          break;
        }
        case 'покинуть':
        case 'leave':
          await this.leaveSpace();
          break;
        case 'исключить':
        case 'kick':
          await governance.remove(this.#member(argument));
          break;
        case 'админ':
        case 'admin':
          await governance.setAdmin(this.#member(argument), true);
          break;
        case 'неадмин':
        case 'unadmin':
          await governance.setAdmin(this.#member(argument), false);
          break;
        case 'ключ':
        case 'rekey':
          await governance.rotate();
          break;
        case 'приглашения':
        case 'invites': {
          await governance.load(this.spaceId);
          const live = governance.invites.filter((i) => i.live);
          if (argument) {
            const target = live.find((i) => i.id.startsWith(argument));
            if (!target) throw new Error(`нет действующего приглашения «${argument}»`);
            await governance.revoke(target.id);
            this.#note('приглашение погашено');
            break;
          }
          this.#note(
            live.length === 0
              ? 'действующих приглашений нет'
              : live
                  .map((i) => `${i.id.slice(0, 6)} от ${i.author_nick}: ${expiresText(i.expires)}, ${usesText(i)}`)
                  .join(' · ') + ' — погасить: /приглашения <код>',
          );
          break;
        }
        case 'экран':
        case 'screen':
          await call.toggleScreen();
          this.#note(call.screenOn ? 'показываете экран' : 'показ экрана выключен');
          break;
        case 'файл':
        case 'file':
          await this.attach();
          break;
        case 'аватар':
        case 'avatar':
          await this.setAvatar();
          break;
        case 'эмодзи':
        case 'emoji':
          await this.addEmoji(argument, false);
          break;
        case 'стикер':
        case 'sticker':
          await this.addEmoji(argument, true);
          break;
        case 'обновление':
        case 'update': {
          await updates.check();
          this.#note(
            updates.stage === 'available'
              ? `доступна версия ${updates.next} — установить в настройках (^,)`
              : updates.stage === 'current'
                ? 'установлена последняя версия'
                : (updates.error || 'проверяем…'),
          );
          break;
        }
        case 'баг':
        case 'bug':
          await report.show();
          break;
        case 'обновления':
        case 'news':
          await news.show();
          break;
        case 'помощь':
        case 'help':
          this.#note(
            'команды: /простор /канал /голос /звонок /войти /позвать [всех] /приглашения /визитка ' +
              '/лс /имя /аватар /файл /эмодзи /стикер /экран /покинуть /обновление /обновления /баг · ' +
              'для администраторов: /исключить /админ /неадмин /ключ',
          );
          break;
        default:
          throw new Error(`неизвестная команда /${command} — наберите /помощь`);
      }
    } catch (error) {
      this.#note(errorText(error));
    }
  }

  /** Скопировать свою визитку: по ней с вами свяжутся напрямую. */
  async copyPersonalLink(): Promise<void> {
    try {
      const link = await api.personalLink();
      await navigator.clipboard.writeText(link);
      this.#note('ваша ссылка для связи скопирована — отправьте её собеседнику');
    } catch (error) {
      this.#note(errorText(error));
    }
  }

  async #refreshSpaces(select: Id): Promise<void> {
    this.spaces = await api.listSpaces();
    await this.selectSpace(select);
  }

  // ── реакция на уведомления ядра ───────────────────────────────────────────

  #onNotice(notice: Notice): void {
    switch (notice.kind) {
      case 'applied':
        void this.#notify(notice.event);
        if (notice.space === this.spaceId) this.#scheduleRefresh();
        else void this.#refreshSpaceBadges();
        break;
      case 'presence':
        if (notice.space === this.spaceId) {
          void this.#loadMembers();
          void this.#loadVoice();
        }
        break;
      case 'typing':
        if (notice.space === this.spaceId && notice.channel === this.channelId) {
          this.#noteTyping(notice.author, notice.nick);
        }
        break;
      case 'fork': {
        // Тихо это проглатывать нельзя: узел либо сломан, либо переписывает
        // свою историю, и человек должен об этом узнать.
        const who = this.members.find((m) => m.id === notice.author)?.nick ?? notice.author.slice(0, 8);
        this.status = `внимание: ${who} выдал два разных события под одним номером — вторая версия отвергнута`;
        break;
      }
      case 'version':
        // Иначе это выглядит как «человек в сети, но молчит».
        this.#note(
          `у собеседника другая версия БРЕД (формат ${notice.theirs} против ${notice.ours}) — ` +
            'обновитесь оба, иначе сообщения друг друга вы не увидите',
        );
        break;
      case 'net':
        void this.#pollNet();
        break;
      case 'player':
        if (notice.space === this.spaceId) void call.refreshPlayer();
        break;
      case 'keyframe':
        // Декодер собеседника не начнёт работу, пока не увидит ключевой кадр.
        call.requestKeyframe();
        break;
      case 'bitrate':
        // Величину считает ядро: только оно видит исходящий канал и то,
        // на скольких собеседников он делится.
        call.applyBitrate(notice.track, notice.bps);
        break;
      case 'speaking':
        call.noteSpeaking(notice.authors);
        break;
    }
  }

  /** События приходят пачками — перерисовываем один раз на окно. */
  #scheduleRefresh(): void {
    if (this.#refreshTimer !== null) return;
    this.#refreshTimer = window.setTimeout(async () => {
      this.#refreshTimer = null;
      await this.#loadMessages();
      await this.#reloadThread();
      await this.#loadChannels();
      await this.#loadEmojis();
      // Читателем считаем только того, кто действительно смотрит на окно:
      // иначе непрочитанное обнуляется, пока приложение висит в фоне.
      if (this.channelId && document.hasFocus()) {
        await api.markRead(this.channelId).catch(() => undefined);
        await this.#loadChannels();
      }
    }, REFRESH_WINDOW);
  }

  #noteTyping(author: Id, nick: string): void {
    if (author === this.me) return;
    this.#typingSeen.set(author, { nick, at: Date.now() });
    this.#recomputeTyping();

    if (this.#typingTimer === null) {
      this.#typingTimer = window.setInterval(() => {
        this.#recomputeTyping();
        if (this.#typingSeen.size === 0 && this.#typingTimer !== null) {
          window.clearInterval(this.#typingTimer);
          this.#typingTimer = null;
        }
      }, 1000);
    }
  }

  #recomputeTyping(): void {
    const now = Date.now();
    for (const [id, seen] of this.#typingSeen) {
      if (now - seen.at > TYPING_TTL) this.#typingSeen.delete(id);
    }
    this.typing = [...this.#typingSeen.values()].map((v) => v.nick);
  }

  async #loadChannels(): Promise<void> {
    if (!this.spaceId) return;
    this.channels = await api.listChannels(this.spaceId);
  }

  async #loadMessages(): Promise<void> {
    if (!this.channelId) return;
    const page = await api.listMessages(this.channelId);
    this.messages = page;
    // Полная страница означает, что выше почти наверняка есть ещё.
    this.hasOlder = page.length >= PAGE;
    this.#prefetchImages(page);
  }

  /** Подгрузка вверх по курсору. Возвращает, сколько добавилось. */
  async loadOlder(): Promise<number> {
    if (!this.channelId || this.loadingOlder || !this.hasOlder) return 0;
    const oldest = this.messages[0];
    if (!oldest) return 0;

    this.loadingOlder = true;
    try {
      const page = await api.listMessages(this.channelId, oldest.lamport);
      this.hasOlder = page.length >= PAGE;
      if (page.length > 0) {
        this.messages = [...page, ...this.messages];
        this.#prefetchImages(page);
      }
      return page.length;
    } catch (error) {
      this.status = errorText(error);
      return 0;
    } finally {
      this.loadingOlder = false;
    }
  }

  /** Небольшие картинки тянем заранее — иначе лента дырявая до клика. */
  #prefetchImages(page: MessageRow[]): void {
    if (!this.spaceId) return;
    prefetchAll(
      this.spaceId,
      page.flatMap((m) => m.attachments.filter(shouldAutoFetch).map((f) => f.hash)),
    );
  }

  // ── ветки ─────────────────────────────────────────────────────────────────

  async openThread(root: Id): Promise<void> {
    this.threadRoot = root;
    this.thread = await api.listThread(root).catch(() => []);
  }

  closeThread(): void {
    this.threadRoot = null;
    this.thread = [];
  }

  async #reloadThread(): Promise<void> {
    if (this.threadRoot) this.thread = await api.listThread(this.threadRoot).catch(() => []);
  }

  async #loadMembers(): Promise<void> {
    if (!this.spaceId) return;
    this.members = await api.listMembers(this.spaceId);
    this.#prefetchAvatars();
  }

  async #loadVoice(): Promise<void> {
    if (!this.spaceId) return;
    this.voice = await api.voiceMap(this.spaceId);
  }

  async #refreshSpaceBadges(): Promise<void> {
    this.spaces = await api.listSpaces();
  }

  async #pollNet(): Promise<void> {
    try {
      const status = await api.netStatus();
      this.online = status.online;
      this.endpoint = status.endpoint;
      this.relay = status.relay;
      this.external = status.external;
      this.neighbors = status.neighbors;
    } catch {
      this.online = false;
    }
  }
}

export const session = new Session();
