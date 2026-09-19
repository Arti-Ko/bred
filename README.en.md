<p align="center"><a href="README.md">Русский</a> · <b>English</b></p>

<p align="center">
  <img src="src-tauri/icons/icon.png" width="140" alt="БРЕД" />
</p>

<h1 align="center">БРЕД</h1>

<p align="center">
  <b>B</b>ystraya <b>R</b>etranslyatsiya <b>E</b>lektronnykh <b>D</b>annykh — "fast relay of electronic data"<br />
  Chat, files and calls straight between people — with no server anywhere
</p>

<p align="center">
  <a href="https://github.com/Arti-Ko/bred/releases/latest">Download</a> ·
  <a href="#getting-started">Getting started</a> ·
  <a href="#making-a-call">Making a call</a> ·
  <a href="ARCHITECTURE.md">How it works inside</a>
</p>

---

## What this is

An ordinary messenger: channels, conversations, files, voice and video calls. The
difference is that **there is no middleman between you**. Messages travel straight
from your computer to the other person's computer and are stored on both. No company
in between, no registration, no email and password.

Pleasant consequences follow: it works on a local network with no internet at all,
the conversation cannot be read from outside, and the service can't be shut down or
blocked — there is nothing to shut down.

![Main window](docs/chat.png)

## Installation

1. Open the [download page](https://github.com/Arti-Ko/bred/releases/latest).
2. Download the file for your system:
   - **Mac with an Apple chip (M1–M4)** — `..._aarch64.dmg`
   - **Intel Mac** — `..._x64.dmg`
   - **Windows** — `..._x64-setup.exe`
3. Open the downloaded file and drag the app into Applications (on Mac) or go
   through the installer (on Windows).

> **Important on Mac.** The app is not signed with an Apple certificate — that costs
> $99 a year, and there isn't one yet. So on first launch the system will say the
> file is damaged. It is not damaged. Open the app with a **right click → "Open"**
> and confirm. If that didn't help, run this in Terminal:
>
> ```bash
> xattr -dr com.apple.quarantine /Applications/БРЕД.app
> ```
>
> Windows is a similar story: SmartScreen shows a warning — click "More info" →
> "Run anyway".

## Getting started

There is no registration. On first launch the app creates your key by itself — that
key is your identity. Everything is driven from the **bar at the bottom**: that's
where messages are typed and where commands go too. Above it is a command strip you
can simply click, so nothing has to be memorized.

### Step 1. Give yourself a name

Click `/имя` in the command strip, add your name and press Enter:

```
/имя Кирилл
```

A confirmation appears in the feed as a grey line. Every command answers the same
way — you always see the result.

### Step 2. Create a space

A space is what other messengers call a server: a set of channels where a group of
people hangs out.

```
/простор Наша компания
```

It immediately contains a `#общий-канал` channel. You can add more:

```
/канал баги
```

### Step 3. Invite someone

Press `/позвать` (or `Ctrl+I`). A `bred://join/…` link lands in your clipboard.

**Send it to your friend any way you like** — Telegram, email, whatever. БРЕД has no
delivery channel of its own and can't have one: until you are connected, you don't
exist for each other.

Your friend pastes the link on their side with `/войти`:

```
/войти bred://join/…
```

A few seconds later they are in the space and receive the whole message history — it
arrives directly from you.

> While they are connecting **for the first time**, you have to be online: there is
> nowhere else to take the history and the addresses from. After that it no longer
> matters.

### Step 4. Write

Pick a channel on the left and type into the bar at the bottom. Enter sends,
`Shift+Enter` breaks the line.

What else the feed can do:

- **Reply to a message** — click it and a "reply" button appears.
- **Add a reaction** — click a message and press "＋ реакция".
- **Discuss separately** — the "ветка" button opens a side thread.
- **Send a file** — the `+` button to the left of the input bar, or the `/файл`
  command. Images are visible right in the feed, and every file has a "save" button.
  A caption is optional: a file can be sent on its own.
- **Delete your own message** — click it and press "удалить".
- **Delete a channel** — right-click it in the list on the left.

## Making a call

![Call](docs/call.png)

A call lives in a voice channel — the way Discord does it.

1. Create a voice channel (once): `/голос созвон`
2. **Click it** in the list on the left — you're in the call. The system asks for
   microphone access once; allow it.
3. The other person clicks the same channel — and you're talking.

The call panel at the top has "camera", "screen" and "leave" buttons. Whoever is
speaking gets a highlighted frame. Calls are group calls: as many people as join can
talk.

Useful bits:

- **Per-person volume** — right-click a person to open a slider. One whispers, the
  other shouts over everyone — that's fixable.
- **Fullscreen** — the "развернуть" button. During a screen share, the share takes
  the main area and cameras move to a strip below; the "только экран" button removes
  those too. `Esc` goes back.
- **Audio along with the screen** — turns on by itself if the system provides it.
  macOS does not, and the app will say so honestly.
- `Ctrl+D` — mute the microphone, `Ctrl+E` — leave the call.

## Writing to one person

If you have no space in common, exchange **cards**:

1. Press `/визитка` — your personal link lands in the clipboard.
2. Send it to the person.
3. They paste it through `/войти` — and the conversation shows up under "личные".

Both sides compute the key for that conversation on their own machine, without
sending anything to each other, so an outsider cannot read it in principle.

If you are already in the same space it's simpler: **click the person** in the
member list on the right.

## Settings

![Settings](docs/settings.png)

Opened with the gear in the top right or with `Ctrl+,`. The update check lives there
too: a new version downloads and installs itself, you only have to confirm.

## All commands

| Command | What it does |
|---|---|
| `/помощь` | list of commands |
| `/имя Кирилл` | change your name |
| `/аватар` | set a profile picture (GIF works) |
| `/простор Название` | create a space |
| `/канал баги` | create a channel |
| `/голос созвон` | create a voice channel |
| `/звонок созвон` | join a call |
| `/позвать` | copy an invitation link to the space |
| `/визитка` | copy your personal link for one-on-one contact |
| `/войти ссылка` | connect using any of those links |
| `/лс имя` | open a direct conversation |
| `/файл` | attach a file |
| `/эмодзи имя` | add your own emoji, then written as `:имя:` |
| `/стикер имя` | add a sticker |
| `/экран` | share your screen in a call |
| `/покинуть` | leave a space and erase its history |
| `/обновление` | check for updates |

## Keyboard shortcuts

| Keys | Action |
|---|---|
| `Ctrl+,` | settings |
| `Ctrl+K` | jump to the input bar |
| `Ctrl+U` | attach a file |
| `Ctrl+I` | copy an invitation |
| `Ctrl+1…9` | switch channel |
| `Ctrl+D` | microphone in a call |
| `Ctrl+E` | leave a call |
| `↑` / `↓` | history of what you sent |
| `Esc` | close a thread or cancel a reply |

## Worth knowing

Honestly, about what not to expect:

- **The invitation link is the pass.** Whoever received it is a member. Access can't
  be revoked without changing the space key, and key rotation doesn't exist yet.
  Don't publish invitations where outsiders can see them.
- **Every member holds the whole history.** That's fine for a circle of friends or a
  work team; for a ten-thousand-person community it isn't.
- **A sent file cannot be deleted "for everyone".** It spreads directly, and anyone
  who has it can pass it on.
- **One computer, one identity.** Reading the same conversation from a laptop and a
  home computer isn't possible yet.
- **There is no moderation.** Bans, reports and administrators don't exist: with no
  server, there is nobody to enforce a decision.

## For developers

You need Rust 1.82+, Node 20+ and pnpm.

```bash
pnpm install
pnpm tauri dev      # run in development mode
pnpm tauri build    # build for the current system
```

Checks:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
pnpm check
```

Several independent profiles on one machine, via `BRED_DATA_DIR`:

```bash
BRED_DATA_DIR=/tmp/бред-1 pnpm tauri dev
BRED_DATA_DIR=/tmp/бред-2 pnpm tauri dev
```

The screenshots in this file are taken from the real interface: `demo.html` brings up
the same components with stand-in data instead of the core.

The internals, the decisions and what they cost — in [ARCHITECTURE.md](ARCHITECTURE.md).
