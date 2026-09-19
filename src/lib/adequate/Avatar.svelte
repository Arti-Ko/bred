<script lang="ts">
  // Аватар: картинка профиля, а если её нет — инициалы на ступени серого.
  //
  // Ступень выбирается по ключу и потому у человека всегда одна и та же: в
  // монохроме это единственный способ узнавать собеседника не читая имени.

  import { previewUrl } from '../previews';
  import { session } from '../stores/session.svelte';

  interface Props {
    id: string;
    nick?: string;
    size?: 'sm' | 'md' | 'lg';
    dim?: boolean;
  }
  const { id, nick = '', size = 'md', dim = false }: Props = $props();

  const member = $derived(session.members.find((m) => m.id === id));
  const name = $derived(nick || member?.nick || id.slice(0, 2));
  const initials = $derived(
    name
      .split(/\s+/)
      .slice(0, 2)
      .map((word) => word[0] ?? '')
      .join('')
      .toUpperCase() || '·',
  );
  /** Шесть ступеней: больше глаз всё равно не различает. */
  const step = $derived(
    [...id].reduce((sum, letter) => (sum + letter.charCodeAt(0)) % 6, 0) + 1,
  );

  let face = $state<string | null>(null);
  let asked: string | null = null;

  $effect(() => {
    const hash = member?.avatar;
    if (!hash) {
      face = null;
      return;
    }
    if (hash === asked) return;
    asked = hash;
    void previewUrl(hash, session.spaceId).then((url) => (face = url));
  });
</script>

<span class="av {size} s{step}" class:dim title={name}>
  {#if face}
    <img src={face} alt="" />
  {:else}
    {initials}
  {/if}
</span>

<style>
  .av {
    display: grid;
    place-items: center;
    overflow: hidden;
    border-radius: 999px;
    color: #0a0a0a;
    font-weight: 650;
    letter-spacing: 0.02em;
    flex: none;
    user-select: none;
  }
  .av img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .sm {
    width: 22px;
    height: 22px;
    font-size: 9.5px;
  }
  .md {
    width: 32px;
    height: 32px;
    font-size: 11.5px;
  }
  .lg {
    width: 44px;
    height: 44px;
    font-size: 15px;
  }
  .dim {
    opacity: 0.55;
  }

  .s1 { background: linear-gradient(140deg, #f2f2f2, #c4c4c4); }
  .s2 { background: linear-gradient(140deg, #a8a8a8, #7c7c7c); }
  .s3 { background: linear-gradient(140deg, #d2d2d2, #a0a0a0); }
  .s4 { background: linear-gradient(140deg, #8d8d8d, #626262); }
  .s5 { background: linear-gradient(140deg, #bcbcbc, #8a8a8a); }
  .s6 { background: linear-gradient(140deg, #767676, #4f4f4f); }
</style>
