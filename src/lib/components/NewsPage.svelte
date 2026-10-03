<script lang="ts">
  // Канал «Обновления БРЕД»: выпуски, новые сверху. Писать сюда нельзя —
  // посты пишет разработчик в релизе, — поэтому и поля ввода здесь нет.
  import { parse } from '../markdown';
  import { newer, news } from '../stores/news.svelte';
  import { prefs } from '../stores/prefs.svelte';
  import { updates } from '../stores/updates.svelte';

  function day(ms: number): string {
    if (!ms) return '';
    return new Date(ms).toLocaleDateString('ru-RU', { day: 'numeric', month: 'long', year: 'numeric' });
  }

  async function install(): Promise<void> {
    await updates.check();
    if (updates.stage === 'available') await updates.install();
  }
</script>

<section class="news" class:soft={prefs.adequate} aria-label="Обновления БРЕД">
  <header>
    <div>
      <b>{prefs.adequate ? 'Обновления БРЕД' : '~ обновления'}</b>
      <span>канал разработчика · писать сюда нельзя</span>
    </div>
    <button class="ghost" onclick={() => news.refresh()}>
      {prefs.adequate ? 'Проверить' : 'проверить'}
    </button>
  </header>

  <div class="posts">
    {#each news.posts as post (post.version)}
      {@const fresh = newer(post.version, news.current)}
      <article class:fresh>
        <h2>{post.title}</h2>
        <div class="meta">
          <span class="tag">{post.version}</span>
          <span>{day(post.published)}</span>
          {#if post.version === news.current}
            <span class="mine">у вас эта версия</span>
          {:else if fresh}
            <button class="primary" onclick={install}>
              {updates.stage === 'downloading'
                ? `Загружаем… ${updates.progress}%`
                : updates.stage === 'ready'
                  ? 'Готово — перезапустите'
                  : 'Установить'}
            </button>
            {#if updates.stage === 'ready'}
              <button class="ghost" onclick={() => updates.restart()}>Перезапустить</button>
            {/if}
          {/if}
        </div>

        <div class="body">
          {#each parse(post.body) as block, index (index)}
            {#snippet line()}
              {#each block.inline as piece, at (at)}
                {#if piece.code}<code>{piece.text}</code>
                {:else if piece.bold}<b>{piece.text}</b>
                {:else if piece.href}<span class="link" title={piece.href}>{piece.text}</span>
                {:else}{piece.text}{/if}
              {/each}
            {/snippet}
            {#if block.kind === 'h1' || block.kind === 'h2'}
              <h3>{@render line()}</h3>
            {:else if block.kind === 'h3'}
              <h4>{@render line()}</h4>
            {:else if block.kind === 'li'}
              <p class="li"><span class="dot">{block.number ? `${block.number}.` : '•'}</span>{@render line()}</p>
            {:else if block.kind === 'quote'}
              <blockquote>{@render line()}</blockquote>
            {:else if block.kind === 'rule'}
              <hr />
            {:else}
              <p>{@render line()}</p>
            {/if}
          {/each}
        </div>
      </article>
    {:else}
      <p class="empty">
        Здесь появятся описания обновлений — как только выйдет следующая версия. Если
        интернета до GitHub нет, лента подтянется, когда он появится.
      </p>
    {/each}
  </div>
</section>

<style>
  .news {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 11px 16px;
    border-bottom: 1px solid var(--line);
  }
  header div {
    display: grid;
  }
  header b {
    color: var(--fg-hi);
    font-size: 15px;
  }
  header span {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }

  .posts {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 18px 20px 40px;
    display: grid;
    align-content: start;
    gap: 18px;
  }

  article {
    max-width: 46rem;
    padding: 16px 18px;
    border: 1px solid var(--line);
    background: var(--bg-raised);
  }
  .soft article {
    border-radius: 14px;
  }
  /* Выпуск, который ещё можно поставить, — светлее рамкой: его и ищут глазами. */
  article.fresh {
    border-color: var(--fg-dim);
  }

  h2 {
    margin: 0 0 6px;
    color: var(--fg-hi);
    font-size: 19px;
    line-height: 1.25;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-bottom: 12px;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }
  .tag {
    padding: 1px 7px;
    border: 1px solid var(--fg-faint);
    color: var(--fg);
    font-family: var(--mono, monospace);
  }
  .mine {
    color: var(--fg-dim);
  }

  .body {
    color: var(--fg);
    font-size: var(--text-sm);
    line-height: 1.6;
  }
  .body p {
    margin: 0 0 8px;
    white-space: pre-line;
  }
  .body .li {
    display: flex;
    gap: 8px;
    margin: 0 0 4px;
  }
  .body .dot {
    flex: 0 0 auto;
    color: var(--fg-dimmer);
  }
  .body h3 {
    margin: 14px 0 6px;
    color: var(--fg-hi);
    font-size: 15px;
  }
  .body h4 {
    margin: 12px 0 4px;
    color: var(--fg-hi);
    font-size: var(--text-sm);
  }
  .body blockquote {
    margin: 0 0 8px;
    padding-left: 10px;
    border-left: 2px solid var(--fg-faint);
    color: var(--fg-dim);
  }
  .body hr {
    border: 0;
    border-top: 1px solid var(--line);
    margin: 12px 0;
  }
  .body code {
    padding: 0 4px;
    background: var(--bg);
    font-size: 0.92em;
  }
  .body b {
    color: var(--fg-hi);
  }
  .link {
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  button.primary,
  button.ghost {
    padding: 3px 10px;
    font-size: var(--text-xs);
    border: 1px solid var(--fg-faint);
  }
  .soft button.primary,
  .soft button.ghost {
    border-radius: 7px;
  }
  button.primary {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }
  button.ghost {
    color: var(--fg-dim);
  }
  button.ghost:hover {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }

  .empty {
    margin: 0;
    max-width: 34rem;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
</style>
