// Самопроверка разбора своих эмодзи и стикеров.
//
// Стикеры «не работали» потому, что ядро принимало имя на любом алфавите, а
// лента узнавала только латиницу. Проверка держит оба конца одним правилом.
//
// Запуск: pnpm test:format

import { loneSticker, tokenize, validEmojiName } from '../src/lib/format';

let failed = 0;
function ok(name: string, condition: boolean, extra = ''): void {
  if (!condition) failed++;
  console.log(`${condition ? '  ok  ' : 'ПРОВАЛ '} ${name}${extra ? ` — ${extra}` : ''}`);
}

const known = {
  котик: { sticker: true },
  parrot: { sticker: false },
  'ёж_2': { sticker: true },
};

console.log('свои эмодзи и стикеры');
ok('стикер с русским именем узнаётся', loneSticker(':котик:', known) === 'котик');
ok('и с пробелами вокруг', loneSticker('  :котик: ', known) === 'котик');
ok('с ё, цифрой и подчёркиванием', loneSticker(':ёж_2:', known) === 'ёж_2');
ok('эмодзи — не стикер', loneSticker(':parrot:', known) === null);
ok('неизвестное имя — просто текст', loneSticker(':пёс:', known) === null);

const tokens = tokenize('смотри :котик: и :parrot: в 12:30:15', known);
ok(
  'в тексте узнаются оба',
  tokens.filter((t) => t.emoji).map((t) => t.emoji).join(',') === 'котик,parrot',
);
ok('время с двоеточиями не ломается', tokens.map((t) => t.text).join('') === 'смотри :котик: и :parrot: в 12:30:15');

ok('имя на кириллице годится', validEmojiName('котик'));
ok('пробел в имени — нет', !validEmojiName('два слова'));
ok('тридцать три знака — нет', !validEmojiName('я'.repeat(33)));
ok('пустое — нет', !validEmojiName(''));

if (failed > 0) throw new Error(`провалов: ${failed}`);
console.log('\nвсё сошлось');
