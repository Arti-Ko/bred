// Отдельное окно боя.
//
// Точка входа своя: бой занимает окно целиком, мессенджера вокруг нет. Победа
// уезжает в главное окно событием — трофей и оформление живут там.

import './lib/styles/global.css';
import { mount } from 'svelte';

import Gundyr from './lib/games/gundyr/Gundyr.svelte';

const target = document.getElementById('app');
if (!target) throw new Error('корневой узел #app не найден');

document.title = 'Судия Гундир';

async function announce(): Promise<void> {
  try {
    const { emit } = await import('@tauri-apps/api/event');
    await emit('гундир:повержен');
  } catch {
    // Окно открыли в браузере — сообщать некому, и это не беда.
  }
}

async function close(): Promise<void> {
  try {
    const { getCurrentWindow } = await import('@tauri-apps/api/window');
    await getCurrentWindow().close();
  } catch {
    window.close();
  }
}

mount(Gundyr, {
  target,
  props: { onwin: () => void announce(), onback: () => void close(), bare: true },
});
